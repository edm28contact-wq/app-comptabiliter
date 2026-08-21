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

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS invoices (
            path TEXT PRIMARY KEY,
            file_name TEXT NOT NULL,
            source TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'nouvelle',
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            extracted_text TEXT,
            extraction_status TEXT NOT NULL DEFAULT 'a_analyser',
            extraction_error TEXT,
            text_length INTEGER NOT NULL DEFAULT 0,
            parsed_json TEXT,
            validated_json TEXT,
            validated_at TEXT
         );"
    ).map_err(|error| error.to_string())?;

    let mut statement = connection.prepare("PRAGMA table_info(invoices)").map_err(|e| e.to_string())?;
    let cols = statement.query_map([], |row| row.get::<_, String>(1)).map_err(|e| e.to_string())?;
    let existing = cols.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    if !existing.iter().any(|c| c == "validated_json") {
        connection.execute("ALTER TABLE invoices ADD COLUMN validated_json TEXT", []).map_err(|e| e.to_string())?;
    }
    if !existing.iter().any(|c| c == "validated_at") {
        connection.execute("ALTER TABLE invoices ADD COLUMN validated_at TEXT", []).map_err(|e| e.to_string())?;
    }
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
        .map(|value| value.chars().filter(|c| c.is_ascii_digit()).collect());
    let iban = first_capture(text, &[r"(?i)IBAN\s*[:\-]?\s*([A-Z]{2}[0-9]{2}(?:\s?[A-Z0-9]){10,30})"])
        .map(|value| value.replace(' ', "").to_uppercase());
    let supplier = text.lines().map(str::trim).filter(|line| line.len() >= 3 && line.len() <= 80)
        .find(|line| {
            let upper = line.to_uppercase();
            !upper.contains("FACTURE") && !upper.contains("INVOICE") && !upper.contains("TOTAL")
        }).map(str::to_string);
    let amounts_consistent = match (
        amount_ht.as_deref().and_then(parse_amount),
        amount_vat.as_deref().and_then(parse_amount),
        amount_ttc.as_deref().and_then(parse_amount),
    ) {
        (Some(ht), Some(vat), Some(ttc)) => Some(((ht + vat) - ttc).abs() <= 0.02),
        _ => None,
    };
    let mut confidence = 0;
    if supplier.is_some() { confidence += 15; }
    if invoice_number.is_some() { confidence += 20; }
    if invoice_date.is_some() { confidence += 15; }
    if amount_ttc.is_some() { confidence += 20; }
    if amount_ht.is_some() { confidence += 10; }
    if amount_vat.is_some() { confidence += 10; }
    if siret.is_some() { confidence += 5; }
    if iban.is_some() { confidence += 5; }
    ParsedInvoice { supplier, invoice_number, invoice_date, amount_ht, amount_vat, amount_ttc, siret, iban, amounts_consistent, confidence }
}

fn persist_text_and_parse(connection: &Connection, path: &str, text: &str, extraction_status: &str) -> Result<(), String> {
    let parsed = parse_invoice_text(text);
    let json = serde_json::to_string(&parsed).map_err(|e| e.to_string())?;
    let length = text.chars().filter(|c| !c.is_whitespace()).count() as i64;
    connection.execute(
        "UPDATE invoices SET extracted_text=?2, extraction_status=?3, extraction_error=NULL, text_length=?4, parsed_json=?5, updated_at=CURRENT_TIMESTAMP WHERE path=?1",
        params![path, text, extraction_status, length, json],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_native_text(connection: &Connection, path: &str) -> Result<(), String> {
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let len = text.chars().filter(|c| !c.is_whitespace()).count();
            if len >= 40 { persist_text_and_parse(connection, path, &text, "texte_extrait")?; }
            else {
                connection.execute("UPDATE invoices SET extracted_text=?2, extraction_status='ocr_requis', text_length=?3 WHERE path=?1", params![path, text, len as i64]).map_err(|e| e.to_string())?;
            }
        }
        Err(error) => {
            connection.execute("UPDATE invoices SET extraction_status='ocr_requis', extraction_error=?2, text_length=0 WHERE path=?1", params![path, error.to_string()]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn store_invoice(connection: &Connection, path: &str, source: &str) -> Result<(), String> {
    let file_name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
    let inserted = connection.execute("INSERT OR IGNORE INTO invoices (path,file_name,source) VALUES (?1,?2,?3)", params![path,file_name,source]).map_err(|e| e.to_string())? > 0;
    if inserted { extract_native_text(connection, path)?; }
    Ok(())
}

#[tauri::command]
fn get_watched_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row("SELECT value FROM settings WHERE key='watched_folder'", [], |row| row.get(0)) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn set_watched_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() { return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string()); }
    let connection = open_database(&app)?;
    connection.execute("INSERT INTO settings(key,value) VALUES('watched_folder',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![path]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn register_invoice(app: AppHandle, path: String, source: String) -> Result<(), String> {
    if !path.to_lowercase().ends_with(".pdf") { return Err("Seuls les fichiers PDF sont acceptés pour le moment.".to_string()); }
    let connection = open_database(&app)?;
    store_invoice(&connection, &path, &source)
}

#[tauri::command]
fn analyze_invoice(app: AppHandle, path: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    extract_native_text(&connection, &path)
}

#[tauri::command]
fn run_invoice_ocr(app: AppHandle, path: String) -> Result<(), String> {
    let text = windows_ocr::ocr_pdf(&path)?;
    if text.chars().filter(|c| !c.is_whitespace()).count() < 20 {
        return Err("L'OCR Windows n'a pas trouvé assez de texte exploitable.".to_string());
    }
    let connection = open_database(&app)?;
    persist_text_and_parse(&connection, &path, &text, "ocr_termine")
}

#[tauri::command]
fn get_invoice_text(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row("SELECT extracted_text FROM invoices WHERE path=?1", params![path], |row| row.get(0)) {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn get_invoice_parsed(app: AppHandle, path: String) -> Result<Option<ParsedInvoice>, String> {
    let connection = open_database(&app)?;
    let value: Option<String> = match connection.query_row("SELECT parsed_json FROM invoices WHERE path=?1", params![path], |row| row.get(0)) {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    value.map(|json| serde_json::from_str(&json).map_err(|e| e.to_string())).transpose()
}

#[tauri::command]
fn validate_invoice(app: AppHandle, path: String, data: ParsedInvoice) -> Result<(), String> {
    let connection = open_database(&app)?;
    let json = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    connection.execute("UPDATE invoices SET validated_json=?2, validated_at=CURRENT_TIMESTAMP, status='validee', updated_at=CURRENT_TIMESTAMP WHERE path=?1", params![path, json]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_invoices(app: AppHandle) -> Result<Vec<InvoiceRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection.prepare("SELECT path,file_name,source,status,extraction_status,text_length FROM invoices ORDER BY first_seen_at DESC,file_name ASC").map_err(|e| e.to_string())?;
    let rows = statement.query_map([], |row| Ok(InvoiceRecord { path: row.get(0)?, file_name: row.get(1)?, source: row.get(2)?, status: row.get(3)?, extraction_status: row.get(4)?, text_length: row.get(5)? })).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
fn scan_pdf_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    if !Path::new(&path).is_dir() { return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string()); }
    let connection = open_database(&app)?;
    let mut pdfs = Vec::new();
    for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
        let file_path = entry.map_err(|e| e.to_string())?.path();
        if file_path.is_file() && file_path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            let value = file_path.to_string_lossy().into_owned();
            store_invoice(&connection, &value, "dossier")?;
            pdfs.push(value);
        }
    }
    pdfs.sort();
    Ok(pdfs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_watched_folder, set_watched_folder, register_invoice, analyze_invoice,
            run_invoice_ocr, get_invoice_text, get_invoice_parsed, validate_invoice,
            list_invoices, scan_pdf_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
