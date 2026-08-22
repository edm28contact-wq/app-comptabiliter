use crate::charlemagne::PreparedCharlemagneEntry;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ColumnMapping {
    pub date: Option<usize>,
    pub journal: Option<usize>,
    pub entry_number: Option<usize>,
    pub account: Option<usize>,
    pub account_label: Option<usize>,
    pub aux_account: Option<usize>,
    pub aux_label: Option<usize>,
    pub piece: Option<usize>,
    pub label: Option<usize>,
    pub debit: Option<usize>,
    pub credit: Option<usize>,
    pub amount: Option<usize>,
    pub direction: Option<usize>,
    pub analytic_code: Option<usize>,
    pub supplier: Option<usize>,
    pub currency: Option<usize>,
}

impl ColumnMapping {
    fn is_complete(&self) -> bool {
        self.date.is_some()
            && self.account.is_some()
            && ((self.debit.is_some() && self.credit.is_some())
                || (self.amount.is_some() && self.direction.is_some()))
    }
}

#[derive(Serialize, Clone)]
pub struct SyncImportRecord {
    pub path: String,
    pub file_name: String,
    pub kind: String,
    pub status: String,
    pub content_hash: String,
    pub line_count: i64,
    pub column_count: i64,
    pub separator: Option<String>,
    pub format_label: Option<String>,
    pub imported_rows: i64,
    pub skipped_rows: i64,
    pub error: Option<String>,
    pub imported_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Clone)]
pub struct SyncPreview {
    pub path: String,
    pub file_name: String,
    pub kind: String,
    pub line_count: usize,
    pub column_count: usize,
    pub separator: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub raw_preview: String,
    pub duplicate: bool,
    pub duplicate_of: Option<String>,
    pub format_label: String,
    pub mapping: ColumnMapping,
    pub mapping_complete: bool,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct SyncCommitResult {
    pub status: String,
    pub content_hash: String,
    pub imported_rows: usize,
    pub updated_rows: usize,
    pub skipped_rows: usize,
    pub mirror_entries: i64,
    pub inferred_supplier_rules: usize,
    pub years: Vec<String>,
}

#[derive(Serialize)]
pub struct SyncScanResult {
    pub detected: usize,
    pub imported: usize,
    pub pending_mapping: usize,
    pub duplicates: usize,
    pub errors: usize,
}

#[derive(Serialize)]
pub struct SyncSummary {
    pub folder: Option<String>,
    pub import_files: i64,
    pub imported_files: i64,
    pub pending_mapping: i64,
    pub error_files: i64,
    pub mirror_entries: i64,
    pub accounts: i64,
    pub suppliers: i64,
    pub last_imported_at: Option<String>,
}

#[derive(Serialize)]
pub struct CharlemagneAccountRecord {
    pub account: String,
    pub label: String,
    pub use_count: i64,
}

#[derive(Serialize)]
pub struct CharlemagneSupplierRecord {
    pub supplier: String,
    pub account: String,
    pub use_count: i64,
}

#[derive(Serialize)]
pub struct JournalEntryRow {
    pub class_code: String,
    pub class_label: String,
    pub account: String,
    pub date: String,
    pub supplier: String,
    pub invoice_number: String,
    pub label: String,
    pub debit: String,
    pub credit: String,
    pub analytic_code: Option<String>,
    pub document_path: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug)]
struct ParsedTable {
    headers: Vec<String>,
    rows: Vec<(usize, Vec<String>)>,
    separator: Option<char>,
    raw_preview: String,
    line_count: usize,
    column_count: usize,
}

#[derive(Clone, Debug)]
struct MirrorEntry {
    source_line: usize,
    entry_number: String,
    date: String,
    journal: String,
    account: String,
    account_label: String,
    aux_account: String,
    aux_label: String,
    piece: String,
    label: String,
    debit: String,
    credit: String,
    analytic_code: Option<String>,
    currency: Option<String>,
    supplier: String,
    business_key: String,
    occurrence: i64,
}

#[derive(Default)]
struct RuleAccumulator {
    supplier_name: String,
    supplier_accounts: HashMap<String, usize>,
    expense_accounts: HashMap<String, usize>,
    vat_accounts: HashMap<String, usize>,
    analytic_codes: HashMap<String, usize>,
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
             CREATE TABLE IF NOT EXISTS charlemagne_sync_imports (
                path TEXT PRIMARY KEY,
                file_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                line_count INTEGER NOT NULL DEFAULT 0,
                column_count INTEGER NOT NULL DEFAULT 0,
                separator TEXT,
                raw_preview TEXT,
                mapping_json TEXT,
                format_label TEXT,
                imported_rows INTEGER NOT NULL DEFAULT 0,
                skipped_rows INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                source_size INTEGER,
                source_modified_ms INTEGER,
                stable_observations INTEGER NOT NULL DEFAULT 0,
                imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_charlemagne_sync_hash
                ON charlemagne_sync_imports(content_hash);
             CREATE TABLE IF NOT EXISTS charlemagne_mirror_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                business_key TEXT NOT NULL,
                occurrence INTEGER NOT NULL DEFAULT 1,
                import_hash TEXT NOT NULL,
                source_path TEXT NOT NULL,
                source_line INTEGER NOT NULL,
                entry_number TEXT,
                date TEXT NOT NULL,
                journal TEXT,
                account TEXT NOT NULL,
                account_label TEXT,
                aux_account TEXT,
                aux_label TEXT,
                piece TEXT,
                label TEXT,
                debit TEXT NOT NULL DEFAULT '0.00',
                credit TEXT NOT NULL DEFAULT '0.00',
                analytic_code TEXT,
                currency TEXT,
                supplier TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(business_key, occurrence)
             );
             CREATE INDEX IF NOT EXISTS idx_charlemagne_mirror_account
                ON charlemagne_mirror_entries(account);
             CREATE INDEX IF NOT EXISTS idx_charlemagne_mirror_date
                ON charlemagne_mirror_entries(date);
             CREATE INDEX IF NOT EXISTS idx_charlemagne_mirror_import
                ON charlemagne_mirror_entries(import_hash);
             CREATE TABLE IF NOT EXISTS supplier_accounting_rules (
                supplier_key TEXT PRIMARY KEY,
                supplier_name TEXT NOT NULL,
                supplier_account TEXT,
                expense_account TEXT,
                vat_account TEXT,
                analytic_code TEXT,
                use_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
        )
        .map_err(|error| error.to_string())?;

    ensure_column(&connection, "charlemagne_sync_imports", "mapping_json", "TEXT")?;
    ensure_column(&connection, "charlemagne_sync_imports", "format_label", "TEXT")?;
    ensure_column(
        &connection,
        "charlemagne_sync_imports",
        "imported_rows",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "charlemagne_sync_imports",
        "skipped_rows",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(&connection, "charlemagne_sync_imports", "error", "TEXT")?;
    ensure_column(
        &connection,
        "charlemagne_sync_imports",
        "source_size",
        "INTEGER",
    )?;
    ensure_column(
        &connection,
        "charlemagne_sync_imports",
        "source_modified_ms",
        "INTEGER",
    )?;
    ensure_column(
        &connection,
        "charlemagne_sync_imports",
        "stable_observations",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(connection)
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

fn sha_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
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

fn kind_for_path(path: &Path) -> Result<&'static str, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Ok("pdf"),
        "csv" => Ok("csv"),
        "tsv" => Ok("tsv"),
        "txt" => Ok("txt"),
        "fec" => Ok("fec"),
        _ => Err(
            "Format non pris en charge. Utilisez FEC, TXT, CSV, TSV ou PDF."
                .to_string(),
        ),
    }
}

fn is_supported_path(path: &Path) -> bool {
    kind_for_path(path).is_ok()
}

fn cp1252_character(byte: u8) -> char {
    match byte {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        value => char::from_u32(value as u32).unwrap_or('�'),
    }
}

fn decode_text_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes.clone()) {
        Ok(value) => value,
        Err(_) => bytes.into_iter().map(cp1252_character).collect(),
    }
}

fn read_document(path: &Path, kind: &str) -> Result<String, String> {
    if kind == "pdf" {
        return pdf_extract::extract_text(path)
            .map_err(|error| format!("Lecture PDF impossible : {error}"));
    }
    let bytes = fs::read(path).map_err(|error| format!("Lecture impossible : {error}"))?;
    Ok(decode_text_bytes(bytes))
}

fn parse_delimited_line(line: &str, separator: char) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;

    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            value if value == separator && !quoted => {
                values.push(current.trim().to_string());
                current.clear();
            }
            value => current.push(value),
        }
    }
    values.push(current.trim().to_string());
    values
}

fn detect_separator(text: &str, kind: &str) -> Option<char> {
    if kind == "tsv" || kind == "fec" {
        if text.lines().take(5).any(|line| line.contains('\t')) {
            return Some('\t');
        }
    }
    let sample = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(10)
        .collect::<Vec<_>>();
    if sample.is_empty() {
        return None;
    }

    let candidates = ['\t', ';', '|', ','];
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let widths = sample
                .iter()
                .map(|line| parse_delimited_line(line, candidate).len())
                .collect::<Vec<_>>();
            let useful = widths.iter().filter(|width| **width > 1).count();
            if useful == 0 {
                return None;
            }
            let max_width = *widths.iter().max().unwrap_or(&1);
            let min_useful = widths
                .iter()
                .filter(|width| **width > 1)
                .min()
                .copied()
                .unwrap_or(1);
            let consistency = max_width.saturating_sub(min_useful);
            let preference = match candidate {
                '\t' => 4,
                ';' => 3,
                '|' => 2,
                ',' => 1,
                _ => 0,
            };
            let score = useful * 1000 + max_width * 10 + preference - consistency.min(9);
            Some((candidate, score))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(candidate, _)| candidate)
}

fn parse_table(text: &str, kind: &str) -> ParsedTable {
    let separator = detect_separator(text, kind);
    let lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let line_count = lines.len();
    let raw_preview = text.chars().take(8000).collect::<String>();

    let Some(separator) = separator else {
        return ParsedTable {
            headers: Vec::new(),
            rows: Vec::new(),
            separator: None,
            raw_preview,
            line_count,
            column_count: 0,
        };
    };

    let headers = lines
        .first()
        .map(|(_, line)| parse_delimited_line(line, separator))
        .unwrap_or_default();
    let mut column_count = headers.len();
    let rows = lines
        .iter()
        .skip(1)
        .map(|(line_index, line)| {
            let row = parse_delimited_line(line, separator);
            column_count = column_count.max(row.len());
            (line_index + 1, row)
        })
        .collect::<Vec<_>>();

    ParsedTable {
        headers,
        rows,
        separator: Some(separator),
        raw_preview,
        line_count,
        column_count,
    }
}

fn normalize_header(value: &str) -> String {
    value
        .trim_start_matches('\u{feff}')
        .chars()
        .flat_map(|character| character.to_lowercase())
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn find_alias(headers: &[String], aliases: &[&str]) -> Option<usize> {
    let normalized = headers
        .iter()
        .map(|header| normalize_header(header))
        .collect::<Vec<_>>();
    aliases.iter().find_map(|alias| {
        let alias = normalize_header(alias);
        normalized.iter().position(|header| header == &alias)
    })
}

fn guess_mapping(headers: &[String]) -> (ColumnMapping, String, Vec<String>) {
    let normalized = headers
        .iter()
        .map(|header| normalize_header(header))
        .collect::<HashSet<_>>();
    let is_fec = normalized.contains("journalcode")
        && normalized.contains("ecrituredate")
        && normalized.contains("comptenum")
        && normalized.contains("debit")
        && normalized.contains("credit");
    let is_charlemagne = normalized.contains("date")
        && (normalized.contains("journal") || normalized.contains("codejournal"))
        && (normalized.contains("account") || normalized.contains("compte"))
        && (normalized.contains("amount") || normalized.contains("montant"))
        && (normalized.contains("s") || normalized.contains("sens"));

    let mapping = ColumnMapping {
        date: find_alias(
            headers,
            &["EcritureDate", "DateEcriture", "Date comptable", "Date"],
        ),
        journal: find_alias(headers, &["JournalCode", "CodeJournal", "Journal", "Jnal"]),
        entry_number: find_alias(
            headers,
            &["EcritureNum", "NumeroEcriture", "N Ecriture", "Ecriture"],
        ),
        account: find_alias(
            headers,
            &[
                "CompteNum",
                "NumeroCompte",
                "Num compte",
                "Compte",
                "Account",
                "CPTG",
            ],
        ),
        account_label: find_alias(
            headers,
            &["CompteLib", "LibelleCompte", "IntituleCompte", "LabelAccount"],
        ),
        aux_account: find_alias(
            headers,
            &["CompAuxNum", "CompteAux", "CompteAuxiliaire", "CPTA"],
        ),
        aux_label: find_alias(
            headers,
            &["CompAuxLib", "LibelleAux", "LibelleAuxiliaire", "TiersLibelle"],
        ),
        piece: find_alias(
            headers,
            &[
                "PieceRef",
                "Piece",
                "ReferencePiece",
                "NumeroPiece",
                "NumPiece",
                "NPIE",
                "NumFacture",
            ],
        ),
        label: find_alias(
            headers,
            &[
                "EcritureLib",
                "LibelleEcriture",
                "LibelleOperation",
                "LabelOperation",
                "Libelle",
                "LIBE",
            ],
        ),
        debit: find_alias(headers, &["Debit", "MontantDebit", "MouvementDebit"]),
        credit: find_alias(headers, &["Credit", "MontantCredit", "MouvementCredit"]),
        amount: find_alias(headers, &["Amount", "Montant", "MONT"]),
        direction: find_alias(headers, &["S", "Sens", "Direction", "CODC"]),
        analytic_code: find_alias(
            headers,
            &[
                "Analytic 1",
                "Analytique 1",
                "Analytique",
                "CodeAnalytique",
                "SectionAnalytique",
                "Axe1",
            ],
        ),
        supplier: find_alias(
            headers,
            &["Fournisseur", "Supplier", "NomFournisseur", "Tiers"],
        ),
        currency: find_alias(headers, &["Idevise", "Devise", "Currency"]),
    };

    let format_label = if is_fec {
        "FEC standard".to_string()
    } else if is_charlemagne {
        "Format Charlemagne tabulé".to_string()
    } else {
        "Export tabulaire générique".to_string()
    };

    let mut warnings = Vec::new();
    if !mapping.is_complete() {
        warnings.push(
            "Le mapping automatique est incomplet : choisissez les colonnes obligatoires avant synchronisation."
                .to_string(),
        );
    }
    if headers.is_empty() {
        warnings.push(
            "Aucune structure tabulaire n'a été reconnue. Préférez l'export FEC, TXT tabulé ou CSV de Charlemagne."
                .to_string(),
        );
    }
    (mapping, format_label, warnings)
}

fn separator_label(separator: Option<char>) -> Option<String> {
    separator.map(|value| match value {
        '\t' => "TAB".to_string(),
        other => other.to_string(),
    })
}

fn build_preview(
    path: &Path,
    kind: &str,
    table: &ParsedTable,
    duplicate_of: Option<String>,
) -> SyncPreview {
    let (mapping, format_label, warnings) = guess_mapping(&table.headers);
    SyncPreview {
        path: path.to_string_lossy().into_owned(),
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("export")
            .to_string(),
        kind: kind.to_string(),
        line_count: table.line_count,
        column_count: table.column_count,
        separator: separator_label(table.separator),
        headers: table.headers.clone(),
        rows: table
            .rows
            .iter()
            .take(20)
            .map(|(_, row)| row.clone())
            .collect(),
        raw_preview: table.raw_preview.clone(),
        duplicate: duplicate_of.is_some(),
        duplicate_of,
        format_label,
        mapping_complete: mapping.is_complete(),
        mapping,
        warnings,
    }
}

fn cell(row: &[String], index: Option<usize>) -> String {
    index
        .and_then(|index| row.get(index))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

fn normalize_date(value: &str) -> Option<String> {
    let raw = value.trim();
    let digits = raw
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
    if digits.len() == 8 && !raw.contains('/') && !raw.contains('-') && !raw.contains('.') {
        let first_four = digits[0..4].parse::<u32>().ok()?;
        if (1900..=2100).contains(&first_four) {
            let year = &digits[0..4];
            let month = &digits[4..6];
            let day = &digits[6..8];
            let month_number = month.parse::<u32>().ok()?;
            let day_number = day.parse::<u32>().ok()?;
            if (1..=12).contains(&month_number) && (1..=31).contains(&day_number) {
                return Some(format!("{day}/{month}/{year}"));
            }
        }
        let day = &digits[0..2];
        let month = &digits[2..4];
        let year = &digits[4..8];
        let month_number = month.parse::<u32>().ok()?;
        let day_number = day.parse::<u32>().ok()?;
        if (1..=12).contains(&month_number) && (1..=31).contains(&day_number) {
            return Some(format!("{day}/{month}/{year}"));
        }
    }

    let parts = raw
        .split(|character| character == '/' || character == '-' || character == '.')
        .map(str::trim)
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].len() == 4 {
        let year = parts[0].parse::<u32>().ok()?;
        let month = parts[1].parse::<u32>().ok()?;
        let day = parts[2].parse::<u32>().ok()?;
        if (1900..=2100).contains(&year)
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
        {
            return Some(format!("{day:02}/{month:02}/{year:04}"));
        }
    }
    let day = parts[0].parse::<u32>().ok()?;
    let month = parts[1].parse::<u32>().ok()?;
    let mut year = parts[2].parse::<u32>().ok()?;
    if parts[2].len() == 2 {
        year += 2000;
    }
    if (1..=31).contains(&day) && (1..=12).contains(&month) && (1900..=2100).contains(&year) {
        Some(format!("{day:02}/{month:02}/{year:04}"))
    } else {
        None
    }
}

fn parse_amount(value: &str) -> Option<f64> {
    let mut raw = value
        .trim()
        .replace('€', "")
        .replace("EUR", "")
        .replace("eur", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if raw.is_empty() || raw == "-" {
        return Some(0.0);
    }
    let negative_parentheses = raw.starts_with('(') && raw.ends_with(')');
    if negative_parentheses {
        raw = raw.trim_start_matches('(').trim_end_matches(')').to_string();
    }
    let comma = raw.rfind(',');
    let dot = raw.rfind('.');
    let normalized = match (comma, dot) {
        (Some(comma_index), Some(dot_index)) if comma_index > dot_index => {
            raw.replace('.', "").replace(',', ".")
        }
        (Some(_), Some(_)) => raw.replace(',', ""),
        (Some(_), None) => raw.replace(',', "."),
        (None, Some(_)) => {
            if raw.matches('.').count() > 1 {
                let mut pieces = raw.split('.').collect::<Vec<_>>();
                let decimal = pieces.pop().unwrap_or("0");
                format!("{}.{}", pieces.join(""), decimal)
            } else {
                raw
            }
        }
        _ => raw,
    };
    let amount = normalized.parse::<f64>().ok()?;
    let amount = if negative_parentheses { -amount } else { amount };
    amount.is_finite().then_some(amount)
}

fn format_amount(value: f64) -> String {
    format!("{:.2}", if value.abs() < 0.005 { 0.0 } else { value })
}

fn direction_is_debit(value: &str) -> Option<bool> {
    let normalized = normalize_header(value);
    match normalized.as_str() {
        "d" | "debit" | "deb" | "dr" => Some(true),
        "c" | "credit" | "cred" | "cr" => Some(false),
        _ => None,
    }
}

fn normalize_supplier_key(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_uppercase())
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn is_summary_row(row: &[String]) -> bool {
    let joined = normalize_header(&row.join(" "));
    joined.contains("total")
        || joined.contains("solde")
        || joined.contains("report")
        || joined.contains("cumul")
}

fn map_row(
    row_number: usize,
    row: &[String],
    mapping: &ColumnMapping,
) -> Result<Option<MirrorEntry>, String> {
    if row.iter().all(|value| value.trim().is_empty()) {
        return Ok(None);
    }
    let account = cell(row, mapping.account);
    let raw_date = cell(row, mapping.date);
    if account.is_empty() && raw_date.is_empty() && is_summary_row(row) {
        return Ok(None);
    }
    if account.is_empty() {
        return Err(format!("ligne {row_number}: compte manquant"));
    }
    let date = normalize_date(&raw_date)
        .ok_or_else(|| format!("ligne {row_number}: date invalide « {raw_date} »"))?;

    let (mut debit, mut credit) = if mapping.debit.is_some() && mapping.credit.is_some() {
        let raw_debit = cell(row, mapping.debit);
        let raw_credit = cell(row, mapping.credit);
        let debit = parse_amount(&raw_debit)
            .ok_or_else(|| format!("ligne {row_number}: débit invalide « {raw_debit} »"))?;
        let credit = parse_amount(&raw_credit)
            .ok_or_else(|| format!("ligne {row_number}: crédit invalide « {raw_credit} »"))?;
        (debit, credit)
    } else {
        let raw_amount = cell(row, mapping.amount);
        let raw_direction = cell(row, mapping.direction);
        let amount = parse_amount(&raw_amount)
            .ok_or_else(|| format!("ligne {row_number}: montant invalide « {raw_amount} »"))?;
        match direction_is_debit(&raw_direction) {
            Some(true) => (amount.abs(), 0.0),
            Some(false) => (0.0, amount.abs()),
            None if amount < 0.0 => (0.0, amount.abs()),
            None => {
                return Err(format!(
                    "ligne {row_number}: sens débit/crédit invalide « {raw_direction} »"
                ))
            }
        }
    };

    if debit < 0.0 && credit.abs() < 0.005 {
        credit = debit.abs();
        debit = 0.0;
    }
    if credit < 0.0 && debit.abs() < 0.005 {
        debit = credit.abs();
        credit = 0.0;
    }
    if debit > 0.005 && credit > 0.005 {
        return Err(format!(
            "ligne {row_number}: débit et crédit sont tous les deux renseignés"
        ));
    }
    if debit.abs() < 0.005 && credit.abs() < 0.005 {
        return Ok(None);
    }

    let journal = cell(row, mapping.journal);
    let entry_number = cell(row, mapping.entry_number);
    let account_label = cell(row, mapping.account_label);
    let aux_account = cell(row, mapping.aux_account);
    let aux_label = cell(row, mapping.aux_label);
    let piece = {
        let value = cell(row, mapping.piece);
        if value.is_empty() {
            entry_number.clone()
        } else {
            value
        }
    };
    let label = cell(row, mapping.label);
    let analytic_raw = cell(row, mapping.analytic_code);
    let analytic_code = (!analytic_raw.is_empty()).then_some(analytic_raw);
    let currency_raw = cell(row, mapping.currency);
    let currency = (!currency_raw.is_empty()).then_some(currency_raw);
    let explicit_supplier = cell(row, mapping.supplier);
    let supplier = if !explicit_supplier.is_empty() {
        explicit_supplier
    } else if !aux_label.is_empty() {
        aux_label.clone()
    } else if account.starts_with("40") {
        account_label.clone()
    } else {
        String::new()
    };
    let direction = if debit > 0.005 { "D" } else { "C" };
    let business_key = sha_text(&format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        date,
        normalize_header(&journal),
        normalize_header(&entry_number),
        normalize_header(&piece),
        normalize_header(&account),
        normalize_header(&aux_account),
        normalize_header(&label),
        normalize_header(&supplier),
        normalize_header(account_label.as_str()),
        direction
    ));

    Ok(Some(MirrorEntry {
        source_line: row_number,
        entry_number,
        date,
        journal,
        account,
        account_label,
        aux_account,
        aux_label,
        piece,
        label,
        debit: format_amount(debit),
        credit: format_amount(credit),
        analytic_code,
        currency,
        supplier,
        business_key,
        occurrence: 1,
    }))
}

fn validate_mapping(mapping: &ColumnMapping, columns: usize) -> Result<(), String> {
    if !mapping.is_complete() {
        return Err(
            "Mapping incomplet : Date, Compte et Débit/Crédit (ou Montant/Sens) sont obligatoires."
                .to_string(),
        );
    }
    let indexes = [
        mapping.date,
        mapping.journal,
        mapping.entry_number,
        mapping.account,
        mapping.account_label,
        mapping.aux_account,
        mapping.aux_label,
        mapping.piece,
        mapping.label,
        mapping.debit,
        mapping.credit,
        mapping.amount,
        mapping.direction,
        mapping.analytic_code,
        mapping.supplier,
        mapping.currency,
    ];
    if indexes.into_iter().flatten().any(|index| index >= columns) {
        return Err("Le mapping référence une colonne inexistante.".to_string());
    }
    Ok(())
}

fn parse_entries(
    table: &ParsedTable,
    mapping: &ColumnMapping,
) -> Result<(Vec<MirrorEntry>, usize), String> {
    validate_mapping(mapping, table.column_count)?;
    let mut entries = Vec::new();
    let mut ignored = 0usize;
    let mut errors = Vec::new();
    for (line_number, row) in &table.rows {
        match map_row(*line_number, row, mapping) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => ignored += 1,
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        let shown = errors.iter().take(8).cloned().collect::<Vec<_>>().join(" · ");
        let suffix = if errors.len() > 8 {
            format!(" · +{} autre(s) erreur(s)", errors.len() - 8)
        } else {
            String::new()
        };
        return Err(format!(
            "Synchronisation refusée : {} ligne(s) non interprétable(s). {shown}{suffix}",
            errors.len()
        ));
    }

    let mut occurrences: HashMap<String, i64> = HashMap::new();
    for entry in &mut entries {
        let count = occurrences.entry(entry.business_key.clone()).or_insert(0);
        *count += 1;
        entry.occurrence = *count;
    }
    if entries.is_empty() {
        return Err("Aucune écriture comptable exploitable n'a été trouvée.".to_string());
    }
    Ok((entries, ignored))
}

fn upsert_preview_record(
    connection: &Connection,
    path: &Path,
    kind: &str,
    hash: &str,
    preview: &SyncPreview,
) -> Result<(), String> {
    let mapping_json = serde_json::to_string(&preview.mapping).map_err(|error| error.to_string())?;
    let (size, modified_ms) = file_observation(path)?;
    let previous: Option<(String, String)> = connection
        .query_row(
            "SELECT content_hash,status FROM charlemagne_sync_imports WHERE path=?1",
            params![path.to_string_lossy().into_owned()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let status = match previous {
        Some((previous_hash, previous_status)) if previous_hash == hash && previous_status == "importe" => {
            "importe"
        }
        _ if preview.mapping_complete => "pret_a_importer",
        _ => "a_mapper",
    };
    connection
        .execute(
            "INSERT INTO charlemagne_sync_imports
             (path,file_name,kind,status,content_hash,line_count,column_count,separator,raw_preview,mapping_json,format_label,source_size,source_modified_ms,stable_observations,error)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,2,NULL)
             ON CONFLICT(path) DO UPDATE SET
                file_name=excluded.file_name,
                kind=excluded.kind,
                status=excluded.status,
                content_hash=excluded.content_hash,
                line_count=excluded.line_count,
                column_count=excluded.column_count,
                separator=excluded.separator,
                raw_preview=excluded.raw_preview,
                mapping_json=excluded.mapping_json,
                format_label=excluded.format_label,
                source_size=excluded.source_size,
                source_modified_ms=excluded.source_modified_ms,
                stable_observations=2,
                error=NULL,
                updated_at=CURRENT_TIMESTAMP",
            params![
                path.to_string_lossy().into_owned(),
                preview.file_name,
                kind,
                status,
                hash,
                preview.line_count as i64,
                preview.column_count as i64,
                preview.separator,
                preview.raw_preview,
                mapping_json,
                preview.format_label,
                size,
                modified_ms,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn analyze_file_internal(connection: &Connection, path: &Path) -> Result<SyncPreview, String> {
    if !path.is_file() {
        return Err("Le fichier d'export Charlemagne n'est pas accessible.".to_string());
    }
    let kind = kind_for_path(path)?;
    let hash = file_sha256(path)?;
    let path_text = path.to_string_lossy().into_owned();
    let duplicate_of: Option<String> = connection
        .query_row(
            "SELECT path FROM charlemagne_sync_imports WHERE content_hash=?1 AND path<>?2 LIMIT 1",
            params![hash, path_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let text = read_document(path, kind)?;
    let table = parse_table(&text, kind);
    let preview = build_preview(path, kind, &table, duplicate_of.clone());
    if duplicate_of.is_none() {
        upsert_preview_record(connection, path, kind, &hash, &preview)?;
    }
    Ok(preview)
}

fn year_from_date(date: &str) -> Option<String> {
    date.rsplit('/').next().filter(|value| value.len() == 4).map(str::to_string)
}

fn top_value(values: &HashMap<String, usize>) -> Option<String> {
    values
        .iter()
        .filter(|(value, _)| !value.trim().is_empty())
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(value, _)| value.clone())
}

fn infer_supplier_rules(
    transaction: &Transaction<'_>,
    entries: &[MirrorEntry],
) -> Result<usize, String> {
    let mut groups: HashMap<String, Vec<&MirrorEntry>> = HashMap::new();
    for entry in entries {
        let group_key = format!(
            "{}|{}|{}|{}",
            entry.date, entry.journal, entry.entry_number, entry.piece
        );
        groups.entry(group_key).or_default().push(entry);
    }

    let mut accumulators: HashMap<String, RuleAccumulator> = HashMap::new();
    for group in groups.values() {
        let supplier_line = group.iter().copied().find(|entry| {
            entry.account.starts_with("401")
                || (entry.account.starts_with("40") && !entry.supplier.trim().is_empty())
        });
        let Some(supplier_line) = supplier_line else {
            continue;
        };
        let supplier_name = supplier_line.supplier.trim();
        if supplier_name.is_empty() {
            continue;
        }
        let supplier_key = normalize_supplier_key(supplier_name);
        if supplier_key.is_empty() {
            continue;
        }
        let accumulator = accumulators.entry(supplier_key).or_default();
        accumulator.supplier_name = supplier_name.to_string();
        *accumulator
            .supplier_accounts
            .entry(supplier_line.account.clone())
            .or_insert(0) += 1;

        if let Some(expense) = group.iter().copied().find(|entry| {
            (entry.account.starts_with('6') || entry.account.starts_with('2'))
                && parse_amount(&entry.debit).unwrap_or(0.0) > 0.005
        }) {
            *accumulator
                .expense_accounts
                .entry(expense.account.clone())
                .or_insert(0) += 1;
            if let Some(code) = expense.analytic_code.as_ref().filter(|value| !value.trim().is_empty()) {
                *accumulator.analytic_codes.entry(code.clone()).or_insert(0) += 1;
            }
        }
        if let Some(vat) = group.iter().copied().find(|entry| {
            entry.account.starts_with("4456")
                && parse_amount(&entry.debit).unwrap_or(0.0) > 0.005
        }) {
            *accumulator
                .vat_accounts
                .entry(vat.account.clone())
                .or_insert(0) += 1;
        }
    }

    let mut written = 0usize;
    for (supplier_key, accumulator) in accumulators {
        let supplier_account = top_value(&accumulator.supplier_accounts);
        let expense_account = top_value(&accumulator.expense_accounts);
        if supplier_account.is_none() || expense_account.is_none() {
            continue;
        }
        let changed = transaction
            .execute(
                "INSERT INTO supplier_accounting_rules
                 (supplier_key,supplier_name,supplier_account,expense_account,vat_account,analytic_code,use_count)
                 VALUES(?1,?2,?3,?4,?5,?6,0)
                 ON CONFLICT(supplier_key) DO UPDATE SET
                    supplier_name=excluded.supplier_name,
                    supplier_account=excluded.supplier_account,
                    expense_account=excluded.expense_account,
                    vat_account=excluded.vat_account,
                    analytic_code=excluded.analytic_code,
                    updated_at=CURRENT_TIMESTAMP
                 WHERE supplier_accounting_rules.use_count=0",
                params![
                    supplier_key,
                    accumulator.supplier_name,
                    supplier_account,
                    expense_account,
                    top_value(&accumulator.vat_accounts),
                    top_value(&accumulator.analytic_codes),
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed > 0 {
            written += 1;
        }
    }
    Ok(written)
}

fn commit_file_internal(
    connection: &mut Connection,
    path: &Path,
    mapping: ColumnMapping,
    replace_existing: bool,
) -> Result<SyncCommitResult, String> {
    if !path.is_file() {
        return Err("Le fichier d'export Charlemagne n'est plus accessible.".to_string());
    }
    let kind = kind_for_path(path)?;
    let hash = file_sha256(path)?;
    let path_text = path.to_string_lossy().into_owned();
    let duplicate_of: Option<String> = connection
        .query_row(
            "SELECT path FROM charlemagne_sync_imports WHERE content_hash=?1 AND path<>?2 AND status='importe' LIMIT 1",
            params![hash, path_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = duplicate_of {
        return Err(format!("Cet export a déjà été synchronisé depuis : {existing}"));
    }

    let text = read_document(path, kind)?;
    let table = parse_table(&text, kind);
    let (mut entries, ignored_rows) = parse_entries(&table, &mapping)?;
    let mut occurrence_counts: HashMap<String, i64> = HashMap::new();
    for entry in &mut entries {
        let occurrence = occurrence_counts.entry(entry.business_key.clone()).or_insert(0);
        *occurrence += 1;
        entry.occurrence = *occurrence;
    }
    let mut years = entries
        .iter()
        .filter_map(|entry| year_from_date(&entry.date))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    years.sort();
    let mapping_json = serde_json::to_string(&mapping).map_err(|error| error.to_string())?;
    let (size, modified_ms) = file_observation(path)?;

    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    if replace_existing {
        for year in &years {
            transaction
                .execute(
                    "DELETE FROM charlemagne_mirror_entries WHERE substr(date,-4)=?1",
                    params![year],
                )
                .map_err(|error| error.to_string())?;
        }
    }

    let mut inserted = 0usize;
    let mut updated = 0usize;
    for entry in &entries {
        let exists: bool = transaction
            .query_row(
                "SELECT 1 FROM charlemagne_mirror_entries WHERE business_key=?1 AND occurrence=?2",
                params![entry.business_key, entry.occurrence],
                |_| Ok(true),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .unwrap_or(false);
        transaction
            .execute(
                "INSERT INTO charlemagne_mirror_entries
                 (business_key,occurrence,import_hash,source_path,source_line,entry_number,date,journal,account,account_label,aux_account,aux_label,piece,label,debit,credit,analytic_code,currency,supplier)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
                 ON CONFLICT(business_key,occurrence) DO UPDATE SET
                    import_hash=excluded.import_hash,
                    source_path=excluded.source_path,
                    source_line=excluded.source_line,
                    entry_number=excluded.entry_number,
                    date=excluded.date,
                    journal=excluded.journal,
                    account=excluded.account,
                    account_label=excluded.account_label,
                    aux_account=excluded.aux_account,
                    aux_label=excluded.aux_label,
                    piece=excluded.piece,
                    label=excluded.label,
                    debit=excluded.debit,
                    credit=excluded.credit,
                    analytic_code=excluded.analytic_code,
                    currency=excluded.currency,
                    supplier=excluded.supplier,
                    updated_at=CURRENT_TIMESTAMP",
                params![
                    entry.business_key,
                    entry.occurrence,
                    hash,
                    path_text,
                    entry.source_line as i64,
                    entry.entry_number,
                    entry.date,
                    entry.journal,
                    entry.account,
                    entry.account_label,
                    entry.aux_account,
                    entry.aux_label,
                    entry.piece,
                    entry.label,
                    entry.debit,
                    entry.credit,
                    entry.analytic_code,
                    entry.currency,
                    entry.supplier,
                ],
            )
            .map_err(|error| error.to_string())?;
        if exists {
            updated += 1;
        } else {
            inserted += 1;
        }
    }

    let inferred_supplier_rules = infer_supplier_rules(&transaction, &entries)?;
    let (mapping_guess, format_label, _) = guess_mapping(&table.headers);
    let mapping_for_record = if mapping == ColumnMapping::default() {
        mapping_guess
    } else {
        mapping.clone()
    };
    let mapping_json = serde_json::to_string(&mapping_for_record).map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO charlemagne_sync_imports
             (path,file_name,kind,status,content_hash,line_count,column_count,separator,raw_preview,mapping_json,format_label,imported_rows,skipped_rows,error,source_size,source_modified_ms,stable_observations,imported_at,updated_at)
             VALUES(?1,?2,?3,'importe',?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,?13,?14,2,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
             ON CONFLICT(path) DO UPDATE SET
                file_name=excluded.file_name,
                kind=excluded.kind,
                status='importe',
                content_hash=excluded.content_hash,
                line_count=excluded.line_count,
                column_count=excluded.column_count,
                separator=excluded.separator,
                raw_preview=excluded.raw_preview,
                mapping_json=excluded.mapping_json,
                format_label=excluded.format_label,
                imported_rows=excluded.imported_rows,
                skipped_rows=excluded.skipped_rows,
                error=NULL,
                source_size=excluded.source_size,
                source_modified_ms=excluded.source_modified_ms,
                stable_observations=2,
                imported_at=CURRENT_TIMESTAMP,
                updated_at=CURRENT_TIMESTAMP",
            params![
                path_text,
                path.file_name().and_then(|value| value.to_str()).unwrap_or("export"),
                kind,
                hash,
                table.line_count as i64,
                table.column_count as i64,
                separator_label(table.separator),
                table.raw_preview,
                mapping_json,
                format_label,
                entries.len() as i64,
                ignored_rows as i64,
                size,
                modified_ms,
            ],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;

    let mirror_entries: i64 = connection
        .query_row("SELECT COUNT(*) FROM charlemagne_mirror_entries", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    Ok(SyncCommitResult {
        status: "importe".to_string(),
        content_hash: hash,
        imported_rows: inserted,
        updated_rows: updated,
        skipped_rows: ignored_rows,
        mirror_entries,
        inferred_supplier_rules,
        years,
    })
}

fn record_scan_error(connection: &Connection, path: &Path, error: &str) {
    let path_text = path.to_string_lossy().into_owned();
    let _ = connection.execute(
        "UPDATE charlemagne_sync_imports SET status='erreur',error=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
        params![path_text, error],
    );
}

fn observe_sync_file(connection: &Connection, path: &Path) -> Result<i64, String> {
    let path_text = path.to_string_lossy().into_owned();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("export")
        .to_string();
    let kind = kind_for_path(path)?;
    let (size, modified_ms) = file_observation(path)?;
    let existing: Option<(Option<i64>, Option<i64>, i64, String)> = connection
        .query_row(
            "SELECT source_size,source_modified_ms,stable_observations,status FROM charlemagne_sync_imports WHERE path=?1",
            params![path_text],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match existing {
        None => {
            let pending_hash = format!("pending:{}", sha_text(&path_text));
            connection
                .execute(
                    "INSERT INTO charlemagne_sync_imports
                     (path,file_name,kind,status,content_hash,source_size,source_modified_ms,stable_observations)
                     VALUES(?1,?2,?3,'attente_stabilite',?4,?5,?6,1)",
                    params![path_text, file_name, kind, pending_hash, size, modified_ms],
                )
                .map_err(|error| error.to_string())?;
            Ok(1)
        }
        Some((previous_size, previous_modified_ms, observations, status)) => {
            if previous_size == Some(size) && previous_modified_ms == Some(modified_ms) {
                if status == "importe" || status == "doublon" || status == "a_mapper" {
                    return Ok(observations.max(2));
                }
                let next = observations.saturating_add(1);
                connection
                    .execute(
                        "UPDATE charlemagne_sync_imports SET stable_observations=?2,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                        params![path_text, next],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(next)
            } else {
                let pending_hash = format!("pending:{}:{}:{}", sha_text(&path_text), size, modified_ms);
                connection
                    .execute(
                        "UPDATE charlemagne_sync_imports SET file_name=?2,kind=?3,status='attente_stabilite',content_hash=?4,source_size=?5,source_modified_ms=?6,stable_observations=1,error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                        params![path_text, file_name, kind, pending_hash, size, modified_ms],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(1)
            }
        }
    }
}

#[tauri::command]
pub fn get_charlemagne_sync_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    connection
        .query_row(
            "SELECT value FROM settings WHERE key='charlemagne_sync_folder'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_charlemagne_sync_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("Le dossier de synchronisation Charlemagne n'est pas accessible.".to_string());
    }
    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('charlemagne_sync_folder',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![path],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn import_charlemagne_sync_file(app: AppHandle, path: String) -> Result<SyncPreview, String> {
    let connection = open_database(&app)?;
    analyze_file_internal(&connection, Path::new(&path))
}

#[tauri::command]
pub fn commit_charlemagne_sync_file(
    app: AppHandle,
    path: String,
    mapping: ColumnMapping,
    replace_existing: bool,
) -> Result<SyncCommitResult, String> {
    let mut connection = open_database(&app)?;
    let result = commit_file_internal(
        &mut connection,
        Path::new(&path),
        mapping,
        replace_existing,
    );
    if let Err(error) = &result {
        record_scan_error(&connection, Path::new(&path), error);
    }
    result
}

#[tauri::command]
pub fn scan_charlemagne_sync_folder(app: AppHandle) -> Result<SyncScanResult, String> {
    let mut connection = open_database(&app)?;
    let folder: Option<String> = connection
        .query_row(
            "SELECT value FROM settings WHERE key='charlemagne_sync_folder'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let folder = folder.ok_or_else(|| "Aucun dossier de synchronisation Charlemagne configuré.".to_string())?;
    if !Path::new(&folder).is_dir() {
        return Err("Le dossier de synchronisation Charlemagne n'est plus accessible.".to_string());
    }

    let mut result = SyncScanResult {
        detected: 0,
        imported: 0,
        pending_mapping: 0,
        duplicates: 0,
        errors: 0,
    };
    let mut paths = fs::read_dir(&folder)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_path(path))
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        result.detected += 1;
        let observations = match observe_sync_file(&connection, &path) {
            Ok(value) => value,
            Err(error) => {
                result.errors += 1;
                record_scan_error(&connection, &path, &error);
                continue;
            }
        };
        if observations < 2 {
            continue;
        }
        let status: Option<String> = connection
            .query_row(
                "SELECT status FROM charlemagne_sync_imports WHERE path=?1",
                params![path.to_string_lossy().into_owned()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if status.as_deref() == Some("importe") {
            continue;
        }
        if status.as_deref() == Some("a_mapper") {
            result.pending_mapping += 1;
            continue;
        }
        if status.as_deref() == Some("doublon") {
            result.duplicates += 1;
            continue;
        }

        let preview = match analyze_file_internal(&connection, &path) {
            Ok(value) => value,
            Err(error) => {
                result.errors += 1;
                record_scan_error(&connection, &path, &error);
                continue;
            }
        };
        if preview.duplicate {
            result.duplicates += 1;
            let _ = connection.execute(
                "UPDATE charlemagne_sync_imports SET status='doublon',updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path.to_string_lossy().into_owned()],
            );
            continue;
        }
        if !preview.mapping_complete {
            result.pending_mapping += 1;
            continue;
        }
        match commit_file_internal(&mut connection, &path, preview.mapping, false) {
            Ok(_) => result.imported += 1,
            Err(error) => {
                result.errors += 1;
                record_scan_error(&connection, &path, &error);
            }
        }
    }
    Ok(result)
}

#[tauri::command]
pub fn list_charlemagne_sync_imports(app: AppHandle) -> Result<Vec<SyncImportRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,file_name,kind,status,content_hash,line_count,column_count,separator,format_label,imported_rows,skipped_rows,error,imported_at,updated_at
             FROM charlemagne_sync_imports ORDER BY updated_at DESC,file_name ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(SyncImportRecord {
                path: row.get(0)?,
                file_name: row.get(1)?,
                kind: row.get(2)?,
                status: row.get(3)?,
                content_hash: row.get(4)?,
                line_count: row.get(5)?,
                column_count: row.get(6)?,
                separator: row.get(7)?,
                format_label: row.get(8)?,
                imported_rows: row.get(9)?,
                skipped_rows: row.get(10)?,
                error: row.get(11)?,
                imported_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_charlemagne_sync_summary(app: AppHandle) -> Result<SyncSummary, String> {
    let connection = open_database(&app)?;
    let folder = connection
        .query_row(
            "SELECT value FROM settings WHERE key='charlemagne_sync_folder'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let import_files = connection
        .query_row("SELECT COUNT(*) FROM charlemagne_sync_imports", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let imported_files = connection
        .query_row(
            "SELECT COUNT(*) FROM charlemagne_sync_imports WHERE status='importe'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let pending_mapping = connection
        .query_row(
            "SELECT COUNT(*) FROM charlemagne_sync_imports WHERE status IN ('a_mapper','pret_a_importer','attente_stabilite')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let error_files = connection
        .query_row(
            "SELECT COUNT(*) FROM charlemagne_sync_imports WHERE status='erreur'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mirror_entries = connection
        .query_row("SELECT COUNT(*) FROM charlemagne_mirror_entries", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let accounts = connection
        .query_row(
            "SELECT COUNT(DISTINCT account) FROM charlemagne_mirror_entries",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let suppliers = connection
        .query_row(
            "SELECT COUNT(DISTINCT supplier) FROM charlemagne_mirror_entries WHERE trim(COALESCE(supplier,''))<>''",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let last_imported_at = connection
        .query_row(
            "SELECT MAX(imported_at) FROM charlemagne_sync_imports WHERE status='importe'",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(SyncSummary {
        folder,
        import_files,
        imported_files,
        pending_mapping,
        error_files,
        mirror_entries,
        accounts,
        suppliers,
        last_imported_at,
    })
}

#[tauri::command]
pub fn list_charlemagne_accounts(app: AppHandle) -> Result<Vec<CharlemagneAccountRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT account,COALESCE(MAX(NULLIF(account_label,'')),''),COUNT(*)
             FROM charlemagne_mirror_entries
             GROUP BY account ORDER BY account ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(CharlemagneAccountRecord {
                account: row.get(0)?,
                label: row.get(1)?,
                use_count: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_charlemagne_suppliers(
    app: AppHandle,
) -> Result<Vec<CharlemagneSupplierRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT supplier,COALESCE(MAX(CASE WHEN account LIKE '40%' THEN account ELSE '' END),''),COUNT(*)
             FROM charlemagne_mirror_entries
             WHERE trim(COALESCE(supplier,''))<>''
             GROUP BY supplier ORDER BY supplier COLLATE NOCASE ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(CharlemagneSupplierRecord {
                supplier: row.get(0)?,
                account: row.get(1)?,
                use_count: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn class_label(code: char) -> &'static str {
    match code {
        '1' => "Capitaux",
        '2' => "Immobilisations",
        '3' => "Stocks et en-cours",
        '4' => "Tiers",
        '5' => "Financiers",
        '6' => "Charges",
        '7' => "Produits",
        _ => "Autres comptes",
    }
}

fn journal_dedupe_key(
    date: &str,
    account: &str,
    invoice_number: &str,
    debit: &str,
    credit: &str,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        normalize_header(date),
        normalize_header(account),
        normalize_header(invoice_number),
        format_amount(parse_amount(debit).unwrap_or(0.0)),
        format_amount(parse_amount(credit).unwrap_or(0.0))
    )
}

#[tauri::command]
pub fn list_journal_entries(app: AppHandle) -> Result<Vec<JournalEntryRow>, String> {
    let connection = open_database(&app)?;
    let mode = connection
        .query_row(
            "SELECT value FROM settings WHERE key='charlemagne_connector_mode'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let use_mirror = mode.as_deref() == Some("sync_files_v2");
    let mut entries = Vec::new();
    let mut mirror_keys = HashSet::new();

    if use_mirror {
        let mut statement = connection
            .prepare(
                "SELECT date,journal,account,COALESCE(piece,''),COALESCE(label,''),debit,credit,COALESCE(analytic_code,''),COALESCE(supplier,'')
                 FROM charlemagne_mirror_entries
                 ORDER BY date,account,id",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            let (date, _journal, account, piece, label, debit, credit, analytic, supplier) =
                row.map_err(|error| error.to_string())?;
            let class = account.chars().next().unwrap_or('0');
            mirror_keys.insert(journal_dedupe_key(&date, &account, &piece, &debit, &credit));
            entries.push(JournalEntryRow {
                class_code: class.to_string(),
                class_label: class_label(class).to_string(),
                account,
                date,
                supplier,
                invoice_number: piece,
                label,
                debit,
                credit,
                analytic_code: (!analytic.is_empty()).then_some(analytic),
                document_path: None,
                source: "Charlemagne V2".to_string(),
            });
        }
    }

    let local_query = connection.prepare(
        "SELECT prepared_charlemagne_json FROM invoices
         WHERE prepared_charlemagne_json IS NOT NULL
           AND status IN ('validee','classee','archive_source_presente')
         ORDER BY validated_at ASC",
    );
    if let Ok(mut statement) = local_query {
        let json_rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        for json in json_rows {
            let json = json.map_err(|error| error.to_string())?;
            let prepared: PreparedCharlemagneEntry = match serde_json::from_str(&json) {
                Ok(value) => value,
                Err(_) => continue,
            };
            for line in prepared.lines {
                let key = journal_dedupe_key(
                    &prepared.date,
                    &line.account,
                    &prepared.invoice_number,
                    &line.debit,
                    &line.credit,
                );
                if use_mirror && mirror_keys.contains(&key) {
                    continue;
                }
                let class = line.account.chars().next().unwrap_or('0');
                entries.push(JournalEntryRow {
                    class_code: class.to_string(),
                    class_label: class_label(class).to_string(),
                    account: line.account,
                    date: prepared.date.clone(),
                    supplier: prepared.supplier.clone(),
                    invoice_number: prepared.invoice_number.clone(),
                    label: line.label,
                    debit: line.debit,
                    credit: line.credit,
                    analytic_code: line.analytic_code,
                    document_path: prepared.document_path.clone(),
                    source: "Préparation locale".to_string(),
                });
            }
        }
    }

    entries.sort_by(|left, right| {
        left.class_code
            .cmp(&right.class_code)
            .then(left.account.cmp(&right.account))
            .then(left.date.cmp(&right.date))
            .then(left.invoice_number.cmp(&right.invoice_number))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{
        guess_mapping, map_row, normalize_date, parse_amount, parse_delimited_line, parse_table,
        ColumnMapping,
    };

    #[test]
    fn parses_quoted_semicolon_rows() {
        let row = parse_delimited_line("20260822;6063;\"Papier; ramettes\";12,50", ';');
        assert_eq!(row, vec!["20260822", "6063", "Papier; ramettes", "12,50"]);
    }

    #[test]
    fn detects_fec_mapping() {
        let text = "JournalCode\tJournalLib\tEcritureNum\tEcritureDate\tCompteNum\tCompteLib\tCompAuxNum\tCompAuxLib\tPieceRef\tPieceDate\tEcritureLib\tDebit\tCredit\tEcritureLet\tDateLet\tValidDate\tMontantdevise\tIdevise\nACH\tAchats\t1\t20260822\t606300\tFournitures\t\t\tF1\t20260822\tPapier\t10,00\t0,00\t\t\t20260822\t\tEUR";
        let table = parse_table(text, "fec");
        let (mapping, label, _) = guess_mapping(&table.headers);
        assert_eq!(label, "FEC standard");
        assert!(mapping.is_complete());
        assert_eq!(mapping.account, Some(4));
        assert_eq!(mapping.debit, Some(11));
    }

    #[test]
    fn detects_charlemagne_tab_format() {
        let headers = vec![
            "Date".to_string(),
            "Journal".to_string(),
            "Account".to_string(),
            "LabelAccount".to_string(),
            "Piece".to_string(),
            "LabelOperation".to_string(),
            "Amount".to_string(),
            "S".to_string(),
        ];
        let (mapping, label, _) = guess_mapping(&headers);
        assert_eq!(label, "Format Charlemagne tabulé");
        assert!(mapping.is_complete());
        assert_eq!(mapping.amount, Some(6));
        assert_eq!(mapping.direction, Some(7));
    }

    #[test]
    fn normalizes_french_dates_and_amounts() {
        assert_eq!(normalize_date("20260823").as_deref(), Some("23/08/2026"));
        assert_eq!(normalize_date("23/08/2026").as_deref(), Some("23/08/2026"));
        assert_eq!(parse_amount("1 248,72"), Some(1248.72));
        assert_eq!(parse_amount("1,248.72"), Some(1248.72));
    }

    #[test]
    fn maps_amount_and_direction_to_debit_credit() {
        let mapping = ColumnMapping {
            date: Some(0),
            account: Some(1),
            amount: Some(2),
            direction: Some(3),
            ..ColumnMapping::default()
        };
        let row = vec![
            "20260823".to_string(),
            "606300".to_string(),
            "120,50".to_string(),
            "D".to_string(),
        ];
        let entry = map_row(2, &row, &mapping).unwrap().unwrap();
        assert_eq!(entry.date, "23/08/2026");
        assert_eq!(entry.debit, "120.50");
        assert_eq!(entry.credit, "0.00");
    }
}
