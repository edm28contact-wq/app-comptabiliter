mod windows_ocr;

use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
struct InvoiceRecord {
    path: String,
    file_name: String,
    source: String,
    status: String,
    extraction_status: String,
    text_length: i64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct ParsedInvoice {
    supplier: Option<String>,
    invoice_number: Option<String>,
    invoice_date: Option<String>,
    amount_ht: Option<String>,
    amount_vat: Option<String>,
    amount_ttc: Option<String>,
    siret: Option<String>,
    iban: Option<String>,
    amounts_consistent: Option<bool>,
    confidence: i32,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct AccountingAssignment {
    supplier_account: Option<String>,
    expense_account: Option<String>,
    vat_account: Option<String>,
    analytic_code: Option<String>,
    confidence: i32,
    source: String,
    use_count: i64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct StorageAssignment {
    archive_folder: Option<String>,
    confidence: i32,
    source: String,
    use_count: i64,
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn ensure_column(connection: &Connection, table: &str, column: &str, definition: &str) -> Result<(), String> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})")).map_err(|error| error.to_string())?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1)).map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"), []).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS invoices (
            path TEXT PRIMARY KEY, file_name TEXT NOT NULL, source TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'nouvelle', first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, extracted_text TEXT,
            extraction_status TEXT NOT NULL DEFAULT 'a_analyser', extraction_error TEXT,
            text_length INTEGER NOT NULL DEFAULT 0, parsed_json TEXT, validated_json TEXT,
            validated_accounting_json TEXT, validated_storage_json TEXT, validated_at TEXT
         );
         CREATE TABLE IF NOT EXISTS supplier_accounting_rules (
            supplier_key TEXT PRIMARY KEY, supplier_name TEXT NOT NULL, supplier_account TEXT,
            expense_account TEXT, vat_account TEXT, analytic_code TEXT,
            use_count INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         CREATE TABLE IF NOT EXISTS supplier_storage_rules (
            supplier_key TEXT PRIMARY KEY, supplier_name TEXT NOT NULL, archive_folder TEXT NOT NULL,
            use_count INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );"
    ).map_err(|error| error.to_string())?;
    ensure_column(&connection, "invoices", "validated_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_accounting_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_storage_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_at", "TEXT")?;
    Ok(connection)
}

fn first_capture(text: &str, patterns: &[&str]) -> Option<String> {
    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            if let Some(captures) = regex.captures(text) {
                if let Some(value) = captures.get(1) {
                    let cleaned = value.as_str().trim().trim_matches(':').trim().to_string();
                    if !cleaned.is_empty() { return Some(cleaned); }
                }
            }
        }
    }
    None
}

fn parse_amount(value: &str) -> Option<f64> {
    value.replace('€', "").replace("EUR", "").replace('\u{00a0}', "").replace(' ', "").replace(',', ".").parse().ok()
}

fn normalize_amount(value: Option<String>) -> Option<String> {
    value.and_then(|raw| parse_amount(&raw).map(|amount| format!("{amount:.2}")))
}

fn compute_amount_consistency(data: &ParsedInvoice) -> Option<bool> {
    match (
        data.amount_ht.as_deref().and_then(parse_amount),
        data.amount_vat.as_deref().and_then(parse_amount),
        data.amount_ttc.as_deref().and_then(parse_amount),
    ) {
        (Some(ht), Some(vat), Some(ttc)) => Some(((ht + vat) - ttc).abs() <= 0.02),
        _ => None,
    }
}

fn parse_invoice_text(text: &str) -> ParsedInvoice {
    let invoice_number = first_capture(text, &[
        r"(?i)(?:facture|invoice)\s*(?:n[°oº]?|num(?:e|é)ro)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)n[°oº]\s*facture\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
    ]);
    let invoice_date = first_capture(text, &[r"(?i)(?:date\s*(?:de\s*)?facture|date)\s*[:\-]?\s*(\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4})"]);
    let amount_ht = normalize_amount(first_capture(text, &[
        r"(?i)(?:total\s*)?H\.?T\.?\s*[:\-]?\s*([0-9][0-9\s\u{00a0}.,]*)\s*(?:€|EUR)?",
        r"(?i)total\s+hors\s+taxe[s]?\s*[:\-]?\s*([0-9][0-9\s\u{00a0}.,]*)",
    ]));
    let amount_vat = normalize_amount(first_capture(text, &[r"(?i)(?:total\s*)?TVA\s*[:\-]?\s*([0-9][0-9\s\u{00a0}.,]*)\s*(?:€|EUR)?"]));
    let amount_ttc = normalize_amount(first_capture(text, &[r"(?i)(?:net\s+[àa]\s+payer|total\s*T\.?T\.?C\.?)\s*[:\-]?\s*([0-9][0-9\s\u{00a0}.,]*)\s*(?:€|EUR)?"]));
    let siret = first_capture(text, &[r"(?i)SIRET\s*[:\-]?\s*([0-9][0-9\s]{12,18})"])
        .map(|value| value.chars().filter(|character| character.is_ascii_digit()).collect());
    let iban = first_capture(text, &[r"(?i)IBAN\s*[:\-]?\s*([A-Z]{2}[0-9]{2}(?:\s?[A-Z0-9]){10,30})"])
        .map(|value| value.replace(' ', "").to_uppercase());
    let supplier = text.lines().map(str::trim).filter(|line| line.len() >= 3 && line.len() <= 80)
        .find(|line| {
            let upper = line.to_uppercase();
            !upper.contains("FACTURE") && !upper.contains("INVOICE") && !upper.contains("TOTAL")
        }).map(str::to_string);

    let mut data = ParsedInvoice { supplier, invoice_number, invoice_date, amount_ht, amount_vat, amount_ttc, siret, iban, amounts_consistent: None, confidence: 0 };
    data.amounts_consistent = compute_amount_consistency(&data);
    if data.supplier.is_some() { data.confidence += 15; }
    if data.invoice_number.is_some() { data.confidence += 20; }
    if data.invoice_date.is_some() { data.confidence += 15; }
    if data.amount_ttc.is_some() { data.confidence += 20; }
    if data.amount_ht.is_some() { data.confidence += 10; }
    if data.amount_vat.is_some() { data.confidence += 10; }
    if data.siret.is_some() { data.confidence += 5; }
    if data.iban.is_some() { data.confidence += 5; }
    data
}

fn normalize_supplier_key(value: &str) -> String {
    value.chars().flat_map(|character| character.to_uppercase()).filter(|character| character.is_alphanumeric()).collect()
}

fn rule_confidence(use_count: i64) -> i32 {
    (75 + (use_count.saturating_sub(1) * 5)).min(95) as i32
}

fn get_supplier_rule(connection: &Connection, supplier: &str) -> Result<Option<AccountingAssignment>, String> {
    let supplier_key = normalize_supplier_key(supplier);
    if supplier_key.is_empty() { return Ok(None); }
    match connection.query_row(
        "SELECT supplier_account, expense_account, vat_account, analytic_code, use_count FROM supplier_accounting_rules WHERE supplier_key=?1",
        params![supplier_key],
        |row| {
            let use_count: i64 = row.get(4)?;
            Ok(AccountingAssignment {
                supplier_account: row.get(0)?, expense_account: row.get(1)?, vat_account: row.get(2)?, analytic_code: row.get(3)?,
                confidence: rule_confidence(use_count), source: "regle_fournisseur".to_string(), use_count,
            })
        },
    ) {
        Ok(rule) => Ok(Some(rule)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_supplier_rule(connection: &Connection, supplier: &str, accounting: &AccountingAssignment) -> Result<(), String> {
    let supplier_name = supplier.trim();
    let supplier_key = normalize_supplier_key(supplier_name);
    if supplier_key.is_empty() { return Err("Le fournisseur est requis pour mémoriser une règle comptable.".to_string()); }
    connection.execute(
        "INSERT INTO supplier_accounting_rules (supplier_key,supplier_name,supplier_account,expense_account,vat_account,analytic_code,use_count)
         VALUES (?1,?2,?3,?4,?5,?6,1)
         ON CONFLICT(supplier_key) DO UPDATE SET supplier_name=excluded.supplier_name,supplier_account=excluded.supplier_account,
         expense_account=excluded.expense_account,vat_account=excluded.vat_account,analytic_code=excluded.analytic_code,
         use_count=supplier_accounting_rules.use_count+1,updated_at=CURRENT_TIMESTAMP",
        params![supplier_key,supplier_name,accounting.supplier_account,accounting.expense_account,accounting.vat_account,accounting.analytic_code],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn get_storage_rule(connection: &Connection, supplier: &str) -> Result<Option<StorageAssignment>, String> {
    let supplier_key = normalize_supplier_key(supplier);
    if supplier_key.is_empty() { return Ok(None); }
    match connection.query_row(
        "SELECT archive_folder,use_count FROM supplier_storage_rules WHERE supplier_key=?1",
        params![supplier_key],
        |row| {
            let use_count: i64 = row.get(1)?;
            Ok(StorageAssignment { archive_folder: row.get(0)?, confidence: rule_confidence(use_count), source: "regle_fournisseur".to_string(), use_count })
        },
    ) {
        Ok(rule) => Ok(Some(rule)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_storage_rule(connection: &Connection, supplier: &str, storage: &StorageAssignment) -> Result<(), String> {
    let folder = storage.archive_folder.as_deref().map(str::trim).filter(|value| !value.is_empty())
        .ok_or_else(|| "Le dossier d'archive est requis pour mémoriser le classement.".to_string())?;
    if !Path::new(folder).is_dir() { return Err("Le dossier d'archive n'est pas accessible.".to_string()); }
    let supplier_name = supplier.trim();
    let supplier_key = normalize_supplier_key(supplier_name);
    if supplier_key.is_empty() { return Err("Le fournisseur est requis pour mémoriser le classement.".to_string()); }
    connection.execute(
        "INSERT INTO supplier_storage_rules (supplier_key,supplier_name,archive_folder,use_count) VALUES (?1,?2,?3,1)
         ON CONFLICT(supplier_key) DO UPDATE SET supplier_name=excluded.supplier_name,archive_folder=excluded.archive_folder,
         use_count=supplier_storage_rules.use_count+1,updated_at=CURRENT_TIMESTAMP",
        params![supplier_key,supplier_name,folder],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn persist_text_and_parse(connection: &Connection, path: &str, text: &str, extraction_status: &str) -> Result<(), String> {
    let parsed = parse_invoice_text(text);
    let json = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
    let length = text.chars().filter(|character| !character.is_whitespace()).count() as i64;
    connection.execute(
        "UPDATE invoices SET extracted_text=?2,extraction_status=?3,extraction_error=NULL,text_length=?4,parsed_json=?5,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
        params![path,text,extraction_status,length,json],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn extract_native_text(connection: &Connection, path: &str) -> Result<(), String> {
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let length = text.chars().filter(|character| !character.is_whitespace()).count();
            if length >= 40 { persist_text_and_parse(connection,path,&text,"texte_extrait")?; }
            else { connection.execute("UPDATE invoices SET extracted_text=?2,extraction_status='ocr_requis',text_length=?3,updated_at=CURRENT_TIMESTAMP WHERE path=?1",params![path,text,length as i64]).map_err(|error| error.to_string())?; }
        }
        Err(error) => { connection.execute("UPDATE invoices SET extraction_status='ocr_requis',extraction_error=?2,text_length=0,updated_at=CURRENT_TIMESTAMP WHERE path=?1",params![path,error.to_string()]).map_err(|database_error| database_error.to_string())?; }
    }
    Ok(())
}

fn store_invoice(connection: &Connection, path: &str, source: &str) -> Result<(), String> {
    let file_name = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path).to_string();
    let inserted = connection.execute("INSERT OR IGNORE INTO invoices (path,file_name,source) VALUES (?1,?2,?3)",params![path,file_name,source]).map_err(|error| error.to_string())? > 0;
    if inserted { extract_native_text(connection,path)?; }
    Ok(())
}

#[tauri::command]
fn get_watched_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row("SELECT value FROM settings WHERE key='watched_folder'",[],|row| row.get(0)) {
        Ok(value) => Ok(Some(value)), Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None), Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn set_watched_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() { return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string()); }
    let connection = open_database(&app)?;
    connection.execute("INSERT INTO settings(key,value) VALUES('watched_folder',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![path]).map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn register_invoice(app: AppHandle, path: String, source: String) -> Result<(), String> {
    if !path.to_lowercase().ends_with(".pdf") { return Err("Seuls les fichiers PDF sont acceptés pour le moment.".to_string()); }
    let connection = open_database(&app)?;
    store_invoice(&connection,&path,&source)
}

#[tauri::command]
fn analyze_invoice(app: AppHandle, path: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    extract_native_text(&connection,&path)
}

#[tauri::command]
fn run_invoice_ocr(app: AppHandle, path: String) -> Result<(), String> {
    let text = windows_ocr::ocr_pdf(&path)?;
    if text.chars().filter(|character| !character.is_whitespace()).count() < 20 { return Err("L'OCR Windows n'a pas trouvé assez de texte exploitable.".to_string()); }
    let connection = open_database(&app)?;
    persist_text_and_parse(&connection,&path,&text,"ocr_termine")
}

#[tauri::command]
fn get_invoice_text(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row("SELECT extracted_text FROM invoices WHERE path=?1",params![path],|row| row.get(0)) {
        Ok(value) => Ok(value), Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None), Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn get_invoice_parsed(app: AppHandle, path: String) -> Result<Option<ParsedInvoice>, String> {
    let connection = open_database(&app)?;
    let value: Option<String> = match connection.query_row("SELECT parsed_json FROM invoices WHERE path=?1",params![path],|row| row.get(0)) {
        Ok(value) => value, Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None), Err(error) => return Err(error.to_string()),
    };
    value.map(|json| serde_json::from_str(&json).map_err(|error| error.to_string())).transpose()
}

#[tauri::command]
fn get_supplier_accounting(app: AppHandle, supplier: String) -> Result<Option<AccountingAssignment>, String> {
    let connection = open_database(&app)?;
    get_supplier_rule(&connection,&supplier)
}

#[tauri::command]
fn get_supplier_storage(app: AppHandle, supplier: String) -> Result<Option<StorageAssignment>, String> {
    let connection = open_database(&app)?;
    get_storage_rule(&connection,&supplier)
}

#[tauri::command]
fn validate_invoice(app: AppHandle, path: String, mut data: ParsedInvoice, accounting: AccountingAssignment, storage: StorageAssignment, remember_rule: bool, remember_storage: bool) -> Result<(), String> {
    data.amounts_consistent = compute_amount_consistency(&data);
    let connection = open_database(&app)?;
    let invoice_json = serde_json::to_string(&data).map_err(|error| error.to_string())?;
    let accounting_json = serde_json::to_string(&accounting).map_err(|error| error.to_string())?;
    let storage_json = serde_json::to_string(&storage).map_err(|error| error.to_string())?;
    connection.execute(
        "UPDATE invoices SET validated_json=?2,validated_accounting_json=?3,validated_storage_json=?4,validated_at=CURRENT_TIMESTAMP,status='validee',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
        params![path,invoice_json,accounting_json,storage_json],
    ).map_err(|error| error.to_string())?;
    if let Some(supplier) = data.supplier.as_deref() {
        if remember_rule { save_supplier_rule(&connection,supplier,&accounting)?; }
        if remember_storage && storage.archive_folder.is_some() { save_storage_rule(&connection,supplier,&storage)?; }
    }
    Ok(())
}

#[tauri::command]
fn list_invoices(app: AppHandle) -> Result<Vec<InvoiceRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection.prepare("SELECT path,file_name,source,status,extraction_status,text_length FROM invoices ORDER BY first_seen_at DESC,file_name ASC").map_err(|error| error.to_string())?;
    let rows = statement.query_map([],|row| Ok(InvoiceRecord { path: row.get(0)?,file_name: row.get(1)?,source: row.get(2)?,status: row.get(3)?,extraction_status: row.get(4)?,text_length: row.get(5)? })).map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|error| error.to_string())
}

#[tauri::command]
fn scan_pdf_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    if !Path::new(&path).is_dir() { return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string()); }
    let connection = open_database(&app)?;
    let mut pdfs = Vec::new();
    for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
        let file_path = entry.map_err(|error| error.to_string())?.path();
        if file_path.is_file() && file_path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")) {
            let value = file_path.to_string_lossy().into_owned(); store_invoice(&connection,&value,"dossier")?; pdfs.push(value);
        }
    }
    pdfs.sort(); Ok(pdfs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default().plugin(tauri_plugin_dialog::init()).invoke_handler(tauri::generate_handler![
        get_watched_folder,set_watched_folder,register_invoice,analyze_invoice,run_invoice_ocr,
        get_invoice_text,get_invoice_parsed,get_supplier_accounting,get_supplier_storage,validate_invoice,
        list_invoices,scan_pdf_folder
    ]).run(tauri::generate_context!()).expect("error while running tauri application");
}
