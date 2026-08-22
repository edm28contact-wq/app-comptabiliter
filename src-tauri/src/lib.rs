mod charlemagne;
mod identifiers;
mod windows_ocr;

use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

static PROCESSING_PATHS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

struct PathProcessingGuard {
    path: String,
}

impl PathProcessingGuard {
    fn acquire(path: &str) -> Result<Option<Self>, String> {
        let paths = PROCESSING_PATHS.get_or_init(|| Mutex::new(HashSet::new()));
        let mut guard = paths
            .lock()
            .map_err(|_| "Le verrou de traitement des documents est indisponible.".to_string())?;
        if !guard.insert(path.to_string()) {
            return Ok(None);
        }
        Ok(Some(Self {
            path: path.to_string(),
        }))
    }
}

impl Drop for PathProcessingGuard {
    fn drop(&mut self) {
        if let Some(paths) = PROCESSING_PATHS.get() {
            if let Ok(mut guard) = paths.lock() {
                guard.remove(&self.path);
            }
        }
    }
}

#[derive(Serialize)]
struct InvoiceRecord {
    path: String,
    file_name: String,
    source: String,
    status: String,
    extraction_status: String,
    text_length: i64,
    archive_path: Option<String>,
    archive_error: Option<String>,
    charlemagne_status: String,
    charlemagne_error: Option<String>,
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

#[derive(Serialize)]
struct ArchiveResult {
    archive_path: String,
    content_hash: String,
    source_deleted: bool,
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
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
                validated_accounting_json TEXT,
                validated_storage_json TEXT,
                validated_at TEXT,
                original_path TEXT,
                archive_path TEXT,
                content_hash TEXT,
                archive_error TEXT,
                archived_at TEXT,
                prepared_charlemagne_json TEXT,
                charlemagne_status TEXT NOT NULL DEFAULT 'a_preparer',
                charlemagne_error TEXT,
                charlemagne_prepared_at TEXT,
                source_size INTEGER,
                source_modified_ms INTEGER,
                stable_observations INTEGER NOT NULL DEFAULT 0,
                duplicate_of TEXT
             );
             CREATE TABLE IF NOT EXISTS supplier_accounting_rules (
                supplier_key TEXT PRIMARY KEY,
                supplier_name TEXT NOT NULL,
                supplier_account TEXT,
                expense_account TEXT,
                vat_account TEXT,
                analytic_code TEXT,
                use_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE TABLE IF NOT EXISTS supplier_storage_rules (
                supplier_key TEXT PRIMARY KEY,
                supplier_name TEXT NOT NULL,
                archive_folder TEXT NOT NULL,
                use_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invoice_path TEXT,
                event_type TEXT NOT NULL,
                details TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE INDEX IF NOT EXISTS idx_invoices_content_hash ON invoices(content_hash);
             CREATE INDEX IF NOT EXISTS idx_audit_invoice_path ON audit_events(invoice_path);",
        )
        .map_err(|error| error.to_string())?;

    ensure_column(&connection, "invoices", "validated_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_accounting_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_storage_json", "TEXT")?;
    ensure_column(&connection, "invoices", "validated_at", "TEXT")?;
    ensure_column(&connection, "invoices", "original_path", "TEXT")?;
    ensure_column(&connection, "invoices", "archive_path", "TEXT")?;
    ensure_column(&connection, "invoices", "content_hash", "TEXT")?;
    ensure_column(&connection, "invoices", "archive_error", "TEXT")?;
    ensure_column(&connection, "invoices", "archived_at", "TEXT")?;
    ensure_column(&connection, "invoices", "prepared_charlemagne_json", "TEXT")?;
    ensure_column(
        &connection,
        "invoices",
        "charlemagne_status",
        "TEXT NOT NULL DEFAULT 'a_preparer'",
    )?;
    ensure_column(&connection, "invoices", "charlemagne_error", "TEXT")?;
    ensure_column(&connection, "invoices", "charlemagne_prepared_at", "TEXT")?;
    ensure_column(&connection, "invoices", "source_size", "INTEGER")?;
    ensure_column(&connection, "invoices", "source_modified_ms", "INTEGER")?;
    ensure_column(
        &connection,
        "invoices",
        "stable_observations",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(&connection, "invoices", "duplicate_of", "TEXT")?;
    Ok(connection)
}

fn record_audit(
    connection: &Connection,
    path: Option<&str>,
    event_type: &str,
    details: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO audit_events(invoice_path,event_type,details) VALUES (?1,?2,?3)",
            params![path, event_type, details],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn first_capture(text: &str, patterns: &[&str]) -> Option<String> {
    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            if let Some(captures) = regex.captures(text) {
                if let Some(value) = captures.get(1) {
                    let cleaned = value
                        .as_str()
                        .trim()
                        .trim_matches(':')
                        .trim()
                        .to_string();
                    if !cleaned.is_empty() {
                        return Some(cleaned);
                    }
                }
            }
        }
    }
    None
}

fn parse_amount(value: &str) -> Option<f64> {
    let mut normalized = value
        .to_uppercase()
        .replace('€', "")
        .replace("EUR", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if normalized.contains(',') {
        normalized = normalized.replace('.', "").replace(',', ".");
    }
    normalized
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite())
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

fn is_plausible_invoice_date(value: &str) -> bool {
    let parts: Vec<&str> = value
        .trim()
        .split(|character| character == '/' || character == '.' || character == '-')
        .collect();
    if parts.len() != 3 {
        return false;
    }
    let Ok(day) = parts[0].parse::<u32>() else {
        return false;
    };
    let Ok(month) = parts[1].parse::<u32>() else {
        return false;
    };
    let Ok(year) = parts[2].parse::<u32>() else {
        return false;
    };
    (1..=31).contains(&day)
        && (1..=12).contains(&month)
        && ((parts[2].len() == 2) || (parts[2].len() == 4 && (1900..=2100).contains(&year)))
}

fn parse_invoice_text(text: &str) -> ParsedInvoice {
    let invoice_number = first_capture(
        text,
        &[
            r"(?i)(?:facture|invoice)\s*(?:n[°oº]?|num(?:e|é)ro)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
            r"(?i)n[°oº]\s*facture\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        ],
    );
    let invoice_date = first_capture(
        text,
        &[r"(?i)(?:date\s*(?:de\s*)?facture|date)\s*[:\-]?\s*(\d{1,2}[/.\-]\d{1,2}[/.\-]\d{2,4})"],
    );
    let amount_ht = normalize_amount(first_capture(
        text,
        &[
            r"(?i)(?:total\s*)?H\.?T\.?\s*[:\-]?\s*([0-9][0-9\s\u{00a0}\u{202f}.,]*)\s*(?:€|EUR)?",
            r"(?i)total\s+hors\s+taxe[s]?\s*[:\-]?\s*([0-9][0-9\s\u{00a0}\u{202f}.,]*)",
        ],
    ));
    let amount_vat = normalize_amount(first_capture(
        text,
        &[r"(?i)(?:total\s*)?TVA\s*[:\-]?\s*([0-9][0-9\s\u{00a0}\u{202f}.,]*)\s*(?:€|EUR)?"],
    ));
    let amount_ttc = normalize_amount(first_capture(
        text,
        &[r"(?i)(?:net\s+[àa]\s+payer|total\s*T\.?T\.?C\.?)\s*[:\-]?\s*([0-9][0-9\s\u{00a0}\u{202f}.,]*)\s*(?:€|EUR)?"],
    ));
    let siret = first_capture(
        text,
        &[r"(?i)SIRET\s*[:\-]?\s*([0-9][0-9\s]{12,18})"],
    )
    .map(|value| {
        value
            .chars()
            .filter(|character| character.is_ascii_digit())
            .collect()
    });
    let iban = first_capture(
        text,
        &[r"(?i)IBAN\s*[:\-]?\s*([A-Z]{2}[0-9]{2}(?:\s?[A-Z0-9]){10,30})"],
    )
    .map(|value| value.replace(' ', "").to_uppercase());
    let supplier = text
        .lines()
        .map(str::trim)
        .filter(|line| line.len() >= 3 && line.len() <= 80)
        .find(|line| {
            let upper = line.to_uppercase();
            !upper.contains("FACTURE")
                && !upper.contains("INVOICE")
                && !upper.contains("TOTAL")
        })
        .map(str::to_string);

    let mut data = ParsedInvoice {
        supplier,
        invoice_number,
        invoice_date,
        amount_ht,
        amount_vat,
        amount_ttc,
        siret,
        iban,
        amounts_consistent: None,
        confidence: 0,
    };
    data.amounts_consistent = compute_amount_consistency(&data);
    if data.supplier.is_some() {
        data.confidence += 15;
    }
    if data.invoice_number.is_some() {
        data.confidence += 20;
    }
    if data.invoice_date.is_some() {
        data.confidence += 15;
    }
    if data.amount_ttc.is_some() {
        data.confidence += 20;
    }
    if data.amount_ht.is_some() {
        data.confidence += 10;
    }
    if data.amount_vat.is_some() {
        data.confidence += 10;
    }
    if data.siret.is_some() {
        data.confidence += 5;
    }
    if data.iban.is_some() {
        data.confidence += 5;
    }
    data
}

fn validate_invoice_fields(
    data: &ParsedInvoice,
    accounting: &AccountingAssignment,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if data
        .supplier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        errors.push("Fournisseur manquant".to_string());
    }
    if data
        .invoice_number
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        errors.push("Numéro de facture manquant".to_string());
    }
    match data.invoice_date.as_deref() {
        Some(date) if is_plausible_invoice_date(date) => {}
        _ => errors.push("Date de facture invalide ou manquante".to_string()),
    }
    if data.amount_ht.as_deref().and_then(parse_amount).is_none() {
        errors.push("Montant HT invalide ou manquant".to_string());
    }
    if data.amount_ttc.as_deref().and_then(parse_amount).is_none() {
        errors.push("Montant TTC invalide ou manquant".to_string());
    }
    if data.amount_vat.is_some() && data.amount_vat.as_deref().and_then(parse_amount).is_none() {
        errors.push("Montant TVA invalide".to_string());
    }
    if data.amounts_consistent == Some(false) {
        errors.push("HT + TVA ne correspond pas au TTC".to_string());
    }
    if let Some(siret) = data.siret.as_deref() {
        if !siret.trim().is_empty() && !identifiers::is_valid_siret(siret) {
            errors.push("SIRET invalide".to_string());
        }
    }
    if let Some(iban) = data.iban.as_deref() {
        if !iban.trim().is_empty() && !identifiers::is_valid_iban(iban) {
            errors.push("IBAN invalide".to_string());
        }
    }
    if accounting
        .supplier_account
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        errors.push("Compte fournisseur manquant".to_string());
    }
    if accounting
        .expense_account
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        errors.push("Compte de charge manquant".to_string());
    }
    let vat = data
        .amount_vat
        .as_deref()
        .and_then(parse_amount)
        .unwrap_or(0.0);
    if vat.abs() > 0.02
        && accounting
            .vat_account
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        errors.push("Compte de TVA manquant".to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(" · "))
    }
}

fn normalize_supplier_key(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_uppercase())
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn rule_confidence(use_count: i64) -> i32 {
    (75 + (use_count.saturating_sub(1) * 5)).min(95) as i32
}

fn get_supplier_rule(
    connection: &Connection,
    supplier: &str,
) -> Result<Option<AccountingAssignment>, String> {
    let supplier_key = normalize_supplier_key(supplier);
    if supplier_key.is_empty() {
        return Ok(None);
    }
    match connection.query_row(
        "SELECT supplier_account,expense_account,vat_account,analytic_code,use_count
         FROM supplier_accounting_rules WHERE supplier_key=?1",
        params![supplier_key],
        |row| {
            let use_count: i64 = row.get(4)?;
            Ok(AccountingAssignment {
                supplier_account: row.get(0)?,
                expense_account: row.get(1)?,
                vat_account: row.get(2)?,
                analytic_code: row.get(3)?,
                confidence: rule_confidence(use_count),
                source: "regle_fournisseur".to_string(),
                use_count,
            })
        },
    ) {
        Ok(rule) => Ok(Some(rule)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_supplier_rule(
    connection: &Connection,
    supplier: &str,
    accounting: &AccountingAssignment,
) -> Result<(), String> {
    let supplier_name = supplier.trim();
    let supplier_key = normalize_supplier_key(supplier_name);
    if supplier_key.is_empty() {
        return Err("Le fournisseur est requis pour mémoriser une règle comptable.".to_string());
    }
    connection
        .execute(
            "INSERT INTO supplier_accounting_rules
             (supplier_key,supplier_name,supplier_account,expense_account,vat_account,analytic_code,use_count)
             VALUES (?1,?2,?3,?4,?5,?6,1)
             ON CONFLICT(supplier_key) DO UPDATE SET
                supplier_name=excluded.supplier_name,
                supplier_account=excluded.supplier_account,
                expense_account=excluded.expense_account,
                vat_account=excluded.vat_account,
                analytic_code=excluded.analytic_code,
                use_count=supplier_accounting_rules.use_count+1,
                updated_at=CURRENT_TIMESTAMP",
            params![
                supplier_key,
                supplier_name,
                accounting.supplier_account,
                accounting.expense_account,
                accounting.vat_account,
                accounting.analytic_code
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn get_storage_rule(
    connection: &Connection,
    supplier: &str,
) -> Result<Option<StorageAssignment>, String> {
    let supplier_key = normalize_supplier_key(supplier);
    if supplier_key.is_empty() {
        return Ok(None);
    }
    match connection.query_row(
        "SELECT archive_folder,use_count FROM supplier_storage_rules WHERE supplier_key=?1",
        params![supplier_key],
        |row| {
            let use_count: i64 = row.get(1)?;
            Ok(StorageAssignment {
                archive_folder: row.get(0)?,
                confidence: rule_confidence(use_count),
                source: "regle_fournisseur".to_string(),
                use_count,
            })
        },
    ) {
        Ok(rule) => Ok(Some(rule)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn save_storage_rule(
    connection: &Connection,
    supplier: &str,
    storage: &StorageAssignment,
) -> Result<(), String> {
    let folder = storage
        .archive_folder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Le dossier d'archive est requis pour mémoriser le classement.".to_string())?;
    if !Path::new(folder).is_dir() {
        return Err("Le dossier d'archive n'est pas accessible.".to_string());
    }
    let supplier_name = supplier.trim();
    let supplier_key = normalize_supplier_key(supplier_name);
    if supplier_key.is_empty() {
        return Err("Le fournisseur est requis pour mémoriser le classement.".to_string());
    }
    connection
        .execute(
            "INSERT INTO supplier_storage_rules (supplier_key,supplier_name,archive_folder,use_count)
             VALUES (?1,?2,?3,1)
             ON CONFLICT(supplier_key) DO UPDATE SET
                supplier_name=excluded.supplier_name,
                archive_folder=excluded.archive_folder,
                use_count=supplier_storage_rules.use_count+1,
                updated_at=CURRENT_TIMESTAMP",
            params![supplier_key, supplier_name, folder],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn sanitize_filename_component(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn normalize_date_for_filename(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "date-inconnue".to_string();
    };
    let parts: Vec<&str> = value
        .split(|character| character == '/' || character == '.' || character == '-')
        .collect();
    if parts.len() == 3 {
        let day = parts[0].trim();
        let month = parts[1].trim();
        let year = parts[2].trim();
        if day.len() <= 2 && month.len() <= 2 && (year.len() == 2 || year.len() == 4) {
            let full_year = if year.len() == 2 {
                format!("20{year}")
            } else {
                year.to_string()
            };
            return format!("{full_year}-{:0>2}-{:0>2}", month, day);
        }
    }
    sanitize_filename_component(value, "date-inconnue")
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn file_observation(path: &Path) -> Result<(i64, i64), String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let size = i64::try_from(metadata.len()).map_err(|error| error.to_string())?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    Ok((size, modified_ms))
}

fn unique_archive_path(folder: &Path, file_name: &str) -> PathBuf {
    let desired = folder.join(file_name);
    if !desired.exists() {
        return desired;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("facture");
    let extension = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("pdf");
    for index in 2..10_000 {
        let candidate = folder.join(format!("{stem}_{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for index in 0..1_000 {
        let candidate = folder.join(format!("{stem}_{timestamp}_{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join(format!("{stem}_{timestamp}_ultime.{extension}"))
}

fn build_archive_name(data: &ParsedInvoice) -> String {
    let date = normalize_date_for_filename(data.invoice_date.as_deref());
    let supplier = sanitize_filename_component(
        data.supplier.as_deref().unwrap_or("fournisseur"),
        "fournisseur",
    );
    let number = sanitize_filename_component(
        data.invoice_number.as_deref().unwrap_or("sans-numero"),
        "sans-numero",
    );
    let amount = sanitize_filename_component(
        data.amount_ttc.as_deref().unwrap_or("montant-inconnu"),
        "montant-inconnu",
    );
    format!("{date}_{supplier}_{number}_{amount}EUR.pdf")
}

fn copy_verified(source: &Path, destination: &Path) -> Result<String, String> {
    let source_hash_before = file_sha256(source)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let temp_name = format!(
        ".{}.{}.part",
        destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("facture.pdf"),
        timestamp
    );
    let temp_path = destination
        .parent()
        .ok_or_else(|| "Dossier de destination invalide.".to_string())?
        .join(temp_name);

    let copy_result = (|| -> Result<(), String> {
        let mut input = File::open(source).map_err(|error| error.to_string())?;
        let mut output = File::create(&temp_path).map_err(|error| error.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        let copied_hash = file_sha256(&temp_path)?;
        let source_hash_after = file_sha256(source)?;
        if source_hash_before != source_hash_after || copied_hash != source_hash_before {
            return Err(
                "Le fichier source a changé pendant l'archivage ou la copie SHA-256 est différente."
                    .to_string(),
            );
        }
        fs::rename(&temp_path, destination).map_err(|error| error.to_string())?;
        Ok(())
    })();

    if let Err(error) = copy_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    Ok(source_hash_before)
}

fn persist_text_and_parse(
    connection: &Connection,
    path: &str,
    text: &str,
    extraction_status: &str,
) -> Result<(), String> {
    let parsed = parse_invoice_text(text);
    let json = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
    let length = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as i64;
    connection
        .execute(
            "UPDATE invoices SET extracted_text=?2,extraction_status=?3,extraction_error=NULL,text_length=?4,parsed_json=?5,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, text, extraction_status, length, json],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn extract_native_text(connection: &Connection, path: &str) -> Result<(), String> {
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let length = text
                .chars()
                .filter(|character| !character.is_whitespace())
                .count();
            if length >= 40 {
                persist_text_and_parse(connection, path, &text, "texte_extrait")?;
            } else {
                connection
                    .execute(
                        "UPDATE invoices SET extracted_text=?2,extraction_status='ocr_requis',extraction_error=NULL,text_length=?3,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                        params![path, text, length as i64],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        Err(error) => {
            connection
                .execute(
                    "UPDATE invoices SET extraction_status='ocr_requis',extraction_error=?2,text_length=0,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, error.to_string()],
                )
                .map_err(|database_error| database_error.to_string())?;
        }
    }
    Ok(())
}

fn process_stable_invoice(
    connection: &Connection,
    path: &str,
    expected_size: i64,
    expected_modified_ms: i64,
) -> Result<(), String> {
    let file_path = Path::new(path);
    let content_hash = file_sha256(file_path)?;
    let (size_after_hash, modified_after_hash) = file_observation(file_path)?;
    if size_after_hash != expected_size || modified_after_hash != expected_modified_ms {
        connection
            .execute(
                "UPDATE invoices SET source_size=?2,source_modified_ms=?3,stable_observations=1,extraction_status='attente_stabilite',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, size_after_hash, modified_after_hash],
            )
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    let duplicate_of: Option<String> = connection
        .query_row(
            "SELECT path FROM invoices WHERE content_hash=?1 AND path<>?2 AND status<>'doublon' LIMIT 1",
            params![content_hash, path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    if let Some(existing_path) = duplicate_of {
        connection
            .execute(
                "UPDATE invoices SET content_hash=?2,duplicate_of=?3,status='doublon',extraction_status='doublon',extraction_error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, content_hash, existing_path],
            )
            .map_err(|error| error.to_string())?;
        record_audit(
            connection,
            Some(path),
            "duplicate_detected",
            Some(&existing_path),
        )?;
        return Ok(());
    }

    connection
        .execute(
            "UPDATE invoices SET content_hash=?2,duplicate_of=NULL,extraction_status='a_analyser',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, content_hash],
        )
        .map_err(|error| error.to_string())?;
    extract_native_text(connection, path)
}

fn store_invoice(connection: &Connection, path: &str, source: &str) -> Result<(), String> {
    let Some(_processing_guard) = PathProcessingGuard::acquire(path)? else {
        return Ok(());
    };
    let file_path = Path::new(path);
    if !file_path.is_file() {
        return Err("Le PDF n'est plus accessible.".to_string());
    }
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string();
    let (size, modified_ms) = file_observation(file_path)?;
    let existing: Option<(Option<i64>, Option<i64>, i64, String)> = connection
        .query_row(
            "SELECT source_size,source_modified_ms,stable_observations,extraction_status FROM invoices WHERE path=?1",
            params![path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    match existing {
        None => {
            let watched = source == "dossier";
            let observations = if watched { 1 } else { 2 };
            let extraction_status = if watched {
                "attente_stabilite"
            } else {
                "a_analyser"
            };
            connection
                .execute(
                    "INSERT INTO invoices (path,file_name,source,original_path,source_size,source_modified_ms,stable_observations,extraction_status) VALUES (?1,?2,?3,?1,?4,?5,?6,?7)",
                    params![
                        path,
                        file_name,
                        source,
                        size,
                        modified_ms,
                        observations,
                        extraction_status
                    ],
                )
                .map_err(|error| error.to_string())?;
            record_audit(connection, Some(path), "invoice_detected", Some(source))?;
            if !watched {
                process_stable_invoice(connection, path, size, modified_ms)?;
            }
        }
        Some((previous_size, previous_modified_ms, observations, extraction_status)) => {
            if extraction_status != "attente_stabilite" && extraction_status != "a_analyser" {
                return Ok(());
            }
            if source != "dossier" {
                connection
                    .execute(
                        "UPDATE invoices SET source_size=?2,source_modified_ms=?3,stable_observations=2,extraction_status='a_analyser',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                        params![path, size, modified_ms],
                    )
                    .map_err(|error| error.to_string())?;
                process_stable_invoice(connection, path, size, modified_ms)?;
                return Ok(());
            }

            let unchanged = previous_size == Some(size) && previous_modified_ms == Some(modified_ms);
            let new_observations = if unchanged {
                observations.saturating_add(1)
            } else {
                1
            };
            connection
                .execute(
                    "UPDATE invoices SET source_size=?2,source_modified_ms=?3,stable_observations=?4,extraction_status='attente_stabilite',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, size, modified_ms, new_observations],
                )
                .map_err(|error| error.to_string())?;
            if new_observations >= 2 {
                process_stable_invoice(connection, path, size, modified_ms)?;
            }
        }
    }
    Ok(())
}

fn prepare_charlemagne_for_connection(
    connection: &Connection,
    path: &str,
) -> Result<charlemagne::PreparedCharlemagneEntry, String> {
    let row: (Option<String>, Option<String>, Option<String>) = connection
        .query_row(
            "SELECT validated_json,validated_accounting_json,archive_path FROM invoices WHERE path=?1",
            params![path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?;

    let invoice_json = row
        .0
        .ok_or_else(|| "La facture n'a pas encore été validée.".to_string())?;
    let accounting_json = row
        .1
        .ok_or_else(|| "L'imputation comptable validée est absente.".to_string())?;
    let invoice: ParsedInvoice =
        serde_json::from_str(&invoice_json).map_err(|error| error.to_string())?;
    let accounting: AccountingAssignment =
        serde_json::from_str(&accounting_json).map_err(|error| error.to_string())?;
    let document_path = row.2.or_else(|| Some(path.to_string()));

    let result = charlemagne::prepare(charlemagne::PreparationInput {
        supplier: invoice.supplier.clone(),
        invoice_number: invoice.invoice_number.clone(),
        invoice_date: invoice.invoice_date.clone(),
        amount_ht: invoice.amount_ht.clone(),
        amount_vat: invoice.amount_vat.clone(),
        amount_ttc: invoice.amount_ttc.clone(),
        supplier_account: accounting.supplier_account.clone(),
        expense_account: accounting.expense_account.clone(),
        vat_account: accounting.vat_account.clone(),
        analytic_code: accounting.analytic_code.clone(),
        document_path,
    });

    match result {
        Ok(entry) => {
            let json = serde_json::to_string(&entry).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "UPDATE invoices SET prepared_charlemagne_json=?2,charlemagne_status='pret',charlemagne_error=NULL,charlemagne_prepared_at=CURRENT_TIMESTAMP,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, json],
                )
                .map_err(|error| error.to_string())?;
            Ok(entry)
        }
        Err(errors) => {
            let message = errors.join(" · ");
            connection
                .execute(
                    "UPDATE invoices SET prepared_charlemagne_json=NULL,charlemagne_status='incomplet',charlemagne_error=?2,charlemagne_prepared_at=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, message],
                )
                .map_err(|error| error.to_string())?;
            Err(message)
        }
    }
}

fn set_archive_error(connection: &Connection, path: &str, error: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE invoices SET status='archive_erreur',archive_error=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, error],
        )
        .map_err(|database_error| database_error.to_string())?;
    let _ = record_audit(connection, Some(path), "archive_error", Some(error));
    Ok(())
}

fn finalize_existing_archive(
    connection: &Connection,
    path: &str,
    archive_path: &str,
    content_hash: &str,
) -> Result<ArchiveResult, String> {
    let archive = Path::new(archive_path);
    if !archive.is_file() {
        let error = "La copie d'archive enregistrée n'est plus accessible.".to_string();
        set_archive_error(connection, path, &error)?;
        return Err(error);
    }
    let archive_hash = file_sha256(archive)?;
    if archive_hash != content_hash {
        let error = "La copie d'archive enregistrée ne correspond plus à son SHA-256.".to_string();
        set_archive_error(connection, path, &error)?;
        return Err(error);
    }

    let source = Path::new(path);
    let source_deleted = if source.is_file() {
        let current_source_hash = file_sha256(source)?;
        if current_source_hash != content_hash {
            let error = "Le fichier source a changé depuis la copie d'archive ; il n'a pas été supprimé."
                .to_string();
            connection
                .execute(
                    "UPDATE invoices SET status='archive_source_presente',archive_error=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, error],
                )
                .map_err(|database_error| database_error.to_string())?;
            return Err(error);
        }
        fs::remove_file(source).is_ok()
    } else {
        true
    };

    let new_status = if source_deleted {
        "classee"
    } else {
        "archive_source_presente"
    };
    let archive_error = if source_deleted {
        None
    } else {
        Some("Archive vérifiée, mais le fichier source n'a pas pu être supprimé.")
    };
    connection
        .execute(
            "UPDATE invoices SET status=?2,archive_error=?3,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, new_status, archive_error],
        )
        .map_err(|error| error.to_string())?;
    record_audit(
        connection,
        Some(path),
        if source_deleted {
            "archive_completed"
        } else {
            "archive_source_retained"
        },
        Some(archive_path),
    )?;
    let _ = prepare_charlemagne_for_connection(connection, path);
    Ok(ArchiveResult {
        archive_path: archive_path.to_string(),
        content_hash: content_hash.to_string(),
        source_deleted,
    })
}

#[tauri::command]
fn get_watched_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row(
        "SELECT value FROM settings WHERE key='watched_folder'",
        [],
        |row| row.get(0),
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn set_watched_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string());
    }
    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('watched_folder',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![path],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn register_invoice(app: AppHandle, path: String, source: String) -> Result<(), String> {
    if !path.to_lowercase().ends_with(".pdf") {
        return Err("Seuls les fichiers PDF sont acceptés pour le moment.".to_string());
    }
    let connection = open_database(&app)?;
    store_invoice(&connection, &path, &source)
}

#[tauri::command]
fn analyze_invoice(app: AppHandle, path: String) -> Result<(), String> {
    let Some(_processing_guard) = PathProcessingGuard::acquire(&path)? else {
        return Err("Cette facture est déjà en cours de traitement.".to_string());
    };
    let connection = open_database(&app)?;
    extract_native_text(&connection, &path)
}

#[tauri::command]
fn run_invoice_ocr(app: AppHandle, path: String) -> Result<(), String> {
    let Some(_processing_guard) = PathProcessingGuard::acquire(&path)? else {
        return Err("Cette facture est déjà en cours de traitement.".to_string());
    };
    {
        let connection = open_database(&app)?;
        connection
            .execute(
                "UPDATE invoices SET extraction_status='ocr_en_cours',extraction_error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path],
            )
            .map_err(|error| error.to_string())?;
    }

    match windows_ocr::ocr_pdf(&path) {
        Ok(text) => {
            if text
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                < 20
            {
                let error = "L'OCR Windows n'a pas trouvé assez de texte exploitable.".to_string();
                let connection = open_database(&app)?;
                connection
                    .execute(
                        "UPDATE invoices SET extraction_status='ocr_requis',extraction_error=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                        params![path, error],
                    )
                    .map_err(|database_error| database_error.to_string())?;
                return Err(error);
            }
            let connection = open_database(&app)?;
            persist_text_and_parse(&connection, &path, &text, "ocr_termine")?;
            let _ = record_audit(&connection, Some(&path), "ocr_completed", None);
            Ok(())
        }
        Err(error) => {
            let connection = open_database(&app)?;
            connection
                .execute(
                    "UPDATE invoices SET extraction_status='ocr_requis',extraction_error=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, error],
                )
                .map_err(|database_error| database_error.to_string())?;
            let _ = record_audit(&connection, Some(&path), "ocr_error", Some(&error));
            Err(error)
        }
    }
}

#[tauri::command]
fn get_invoice_text(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    match connection.query_row(
        "SELECT extracted_text FROM invoices WHERE path=?1",
        params![path],
        |row| row.get(0),
    ) {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn get_invoice_parsed(app: AppHandle, path: String) -> Result<Option<ParsedInvoice>, String> {
    let connection = open_database(&app)?;
    let value: Option<String> = match connection.query_row(
        "SELECT COALESCE(validated_json,parsed_json) FROM invoices WHERE path=?1",
        params![path],
        |row| row.get(0),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    value
        .map(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
        .transpose()
}

#[tauri::command]
fn get_supplier_accounting(
    app: AppHandle,
    supplier: String,
) -> Result<Option<AccountingAssignment>, String> {
    let connection = open_database(&app)?;
    get_supplier_rule(&connection, &supplier)
}

#[tauri::command]
fn get_supplier_storage(
    app: AppHandle,
    supplier: String,
) -> Result<Option<StorageAssignment>, String> {
    let connection = open_database(&app)?;
    get_storage_rule(&connection, &supplier)
}

#[tauri::command]
fn validate_invoice(
    app: AppHandle,
    path: String,
    mut data: ParsedInvoice,
    accounting: AccountingAssignment,
    storage: StorageAssignment,
    remember_rule: bool,
    remember_storage: bool,
) -> Result<(), String> {
    data.amounts_consistent = compute_amount_consistency(&data);
    validate_invoice_fields(&data, &accounting)?;
    if let Some(folder) = storage
        .archive_folder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !Path::new(folder).is_dir() {
            return Err("Le dossier d'archive sélectionné n'est pas accessible.".to_string());
        }
    }

    let mut connection = open_database(&app)?;
    let invoice_json = serde_json::to_string(&data).map_err(|error| error.to_string())?;
    let accounting_json = serde_json::to_string(&accounting).map_err(|error| error.to_string())?;
    let storage_json = serde_json::to_string(&storage).map_err(|error| error.to_string())?;
    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    let updated = transaction
        .execute(
            "UPDATE invoices SET validated_json=?2,validated_accounting_json=?3,validated_storage_json=?4,validated_at=CURRENT_TIMESTAMP,status='validee',archive_error=NULL,prepared_charlemagne_json=NULL,charlemagne_status='a_preparer',charlemagne_error=NULL,charlemagne_prepared_at=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, invoice_json, accounting_json, storage_json],
        )
        .map_err(|error| error.to_string())?;
    if updated == 0 {
        return Err("La facture à valider n'existe plus dans la base locale.".to_string());
    }

    if let Some(supplier) = data.supplier.as_deref() {
        if remember_rule {
            save_supplier_rule(&transaction, supplier, &accounting)?;
        }
        if remember_storage && storage.archive_folder.is_some() {
            save_storage_rule(&transaction, supplier, &storage)?;
        }
    }
    record_audit(
        &transaction,
        Some(&path),
        "invoice_validated",
        data.supplier.as_deref(),
    )?;
    transaction.commit().map_err(|error| error.to_string())?;

    let _ = prepare_charlemagne_for_connection(&connection, &path);
    Ok(())
}

#[tauri::command]
fn prepare_charlemagne_invoice(
    app: AppHandle,
    path: String,
) -> Result<charlemagne::PreparedCharlemagneEntry, String> {
    let connection = open_database(&app)?;
    prepare_charlemagne_for_connection(&connection, &path)
}

#[tauri::command]
fn get_charlemagne_prepared(
    app: AppHandle,
    path: String,
) -> Result<Option<charlemagne::PreparedCharlemagneEntry>, String> {
    let connection = open_database(&app)?;
    let value: Option<String> = match connection.query_row(
        "SELECT prepared_charlemagne_json FROM invoices WHERE path=?1",
        params![path],
        |row| row.get(0),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    value
        .map(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
        .transpose()
}

#[tauri::command]
fn archive_invoice(app: AppHandle, path: String) -> Result<ArchiveResult, String> {
    let Some(_processing_guard) = PathProcessingGuard::acquire(&path)? else {
        return Err("Cette facture est déjà en cours de traitement.".to_string());
    };
    let mut connection = open_database(&app)?;
    let row: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT status,validated_json,validated_storage_json,archive_path,content_hash FROM invoices WHERE path=?1",
            params![path],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|error| error.to_string())?;

    if row.0 == "classee" {
        if let (Some(archive_path), Some(content_hash)) = (row.3.as_deref(), row.4.as_deref()) {
            let archive = Path::new(archive_path);
            if archive.is_file() && file_sha256(archive)? == content_hash {
                return Ok(ArchiveResult {
                    archive_path: archive_path.to_string(),
                    content_hash: content_hash.to_string(),
                    source_deleted: true,
                });
            }
        }
        return Err("La facture est marquée classée mais sa copie d'archive vérifiée est introuvable."
            .to_string());
    }

    if row.0 != "validee" && row.0 != "archive_erreur" && row.0 != "archive_source_presente" {
        return Err("La facture doit être validée avant son archivage.".to_string());
    }

    if let (Some(archive_path), Some(content_hash)) = (row.3.as_deref(), row.4.as_deref()) {
        if Path::new(archive_path).is_file() {
            return finalize_existing_archive(&connection, &path, archive_path, content_hash);
        }
    }

    let invoice_json = row
        .1
        .ok_or_else(|| "Données validées absentes.".to_string())?;
    let storage_json = row
        .2
        .ok_or_else(|| "Destination d'archive absente.".to_string())?;
    let data: ParsedInvoice =
        serde_json::from_str(&invoice_json).map_err(|error| error.to_string())?;
    let storage: StorageAssignment =
        serde_json::from_str(&storage_json).map_err(|error| error.to_string())?;
    let folder_value = match storage
        .archive_folder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(folder) => folder,
        None => {
            let error = "Aucun dossier d'archive n'a été sélectionné.".to_string();
            set_archive_error(&connection, &path, &error)?;
            return Err(error);
        }
    };
    let folder = Path::new(folder_value);
    if !folder.is_dir() {
        let error = "Le dossier d'archive n'est pas accessible.".to_string();
        set_archive_error(&connection, &path, &error)?;
        return Err(error);
    }

    let source = Path::new(&path);
    if !source.is_file() {
        let error = "Le fichier source n'est plus accessible.".to_string();
        set_archive_error(&connection, &path, &error)?;
        return Err(error);
    }
    let archive_name = build_archive_name(&data);
    let destination = unique_archive_path(folder, &archive_name);
    let content_hash = match copy_verified(source, &destination) {
        Ok(hash) => hash,
        Err(error) => {
            set_archive_error(&connection, &path, &error)?;
            return Err(error);
        }
    };
    let destination_string = destination.to_string_lossy().into_owned();

    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    let persist_result = (|| -> Result<(), String> {
        transaction
            .execute(
                "UPDATE invoices SET original_path=COALESCE(original_path,path),archive_path=?2,content_hash=?3,archive_error=NULL,archived_at=CURRENT_TIMESTAMP,status='archive_source_presente',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, destination_string, content_hash],
            )
            .map_err(|error| error.to_string())?;
        record_audit(
            &transaction,
            Some(&path),
            "archive_copy_verified",
            Some(&destination_string),
        )?;
        Ok(())
    })();
    if let Err(error) = persist_result {
        let _ = transaction.rollback();
        let _ = fs::remove_file(&destination);
        return Err(format!(
            "La copie était vérifiée mais son état n'a pas pu être enregistré ; la source a été conservée : {error}"
        ));
    }
    if let Err(error) = transaction.commit() {
        let _ = fs::remove_file(&destination);
        return Err(format!(
            "La copie était vérifiée mais la transaction SQLite a échoué ; la source a été conservée : {error}"
        ));
    }

    finalize_existing_archive(
        &connection,
        &path,
        &destination_string,
        &content_hash,
    )
}

#[tauri::command]
fn list_invoices(app: AppHandle) -> Result<Vec<InvoiceRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,file_name,source,status,extraction_status,text_length,archive_path,archive_error,charlemagne_status,charlemagne_error
             FROM invoices ORDER BY first_seen_at DESC,file_name ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(InvoiceRecord {
                path: row.get(0)?,
                file_name: row.get(1)?,
                source: row.get(2)?,
                status: row.get(3)?,
                extraction_status: row.get(4)?,
                text_length: row.get(5)?,
                archive_path: row.get(6)?,
                archive_error: row.get(7)?,
                charlemagne_status: row.get(8)?,
                charlemagne_error: row.get(9)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn scan_pdf_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    if !Path::new(&path).is_dir() {
        return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string());
    }
    let connection = open_database(&app)?;
    let mut pdfs = Vec::new();
    for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
        let file_path = entry.map_err(|error| error.to_string())?.path();
        if file_path.is_file()
            && file_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            let value = file_path.to_string_lossy().into_owned();
            store_invoice(&connection, &value, "dossier")?;
            pdfs.push(value);
        }
    }
    pdfs.sort();
    Ok(pdfs)
}

#[cfg(test)]
mod tests {
    use super::{is_plausible_invoice_date, parse_amount};

    #[test]
    fn parse_amount_accepts_french_thousands() {
        assert_eq!(parse_amount("1.248,72 €"), Some(1248.72));
        assert_eq!(parse_amount("1\u{202f}248,72 EUR"), Some(1248.72));
    }

    #[test]
    fn invoice_date_has_basic_sanity_checks() {
        assert!(is_plausible_invoice_date("22/08/2026"));
        assert!(is_plausible_invoice_date("22-08-26"));
        assert!(!is_plausible_invoice_date("40/08/2026"));
        assert!(!is_plausible_invoice_date("22/15/2026"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_watched_folder,
            set_watched_folder,
            register_invoice,
            analyze_invoice,
            run_invoice_ocr,
            get_invoice_text,
            get_invoice_parsed,
            get_supplier_accounting,
            get_supplier_storage,
            validate_invoice,
            prepare_charlemagne_invoice,
            get_charlemagne_prepared,
            archive_invoice,
            list_invoices,
            scan_pdf_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
