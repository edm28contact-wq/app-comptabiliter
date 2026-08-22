use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs, path::{Path, PathBuf}};
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
pub struct SyncImportRecord {
    pub path: String,
    pub file_name: String,
    pub kind: String,
    pub status: String,
    pub content_hash: String,
    pub line_count: i64,
    pub column_count: i64,
    pub separator: Option<String>,
    pub imported_at: String,
}

#[derive(Serialize)]
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
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA busy_timeout=5000;
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
            imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_charlemagne_sync_hash
            ON charlemagne_sync_imports(content_hash);"
    ).map_err(|error| error.to_string())?;
    Ok(connection)
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn kind_for_path(path: &Path) -> Result<&'static str, String> {
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Ok("pdf"),
        "csv" => Ok("csv"),
        "tsv" => Ok("tsv"),
        "txt" => Ok("txt"),
        _ => Err("Format non pris en charge. Utilisez PDF, CSV, TSV ou TXT.".to_string()),
    }
}

fn read_document(path: &Path, kind: &str) -> Result<String, String> {
    if kind == "pdf" {
        return pdf_extract::extract_text(path).map_err(|error| error.to_string());
    }
    fs::read_to_string(path).map_err(|error| format!("Lecture impossible : {error}"))
}

fn detect_separator(text: &str, kind: &str) -> Option<char> {
    if kind == "tsv" { return Some('\t'); }
    let first = text.lines().find(|line| !line.trim().is_empty())?;
    let candidates = ['\t', ';', ','];
    candidates
        .iter()
        .map(|candidate| (*candidate, first.matches(*candidate).count()))
        .filter(|(_, count)| *count > 0)
        .max_by_key(|(_, count)| *count)
        .map(|(candidate, _)| candidate)
}

fn split_row(line: &str, separator: char) -> Vec<String> {
    line.split(separator).map(|value| value.trim().trim_matches('"').to_string()).collect()
}

fn build_preview(path: &Path, kind: &str, text: &str, duplicate: bool) -> SyncPreview {
    let separator = detect_separator(text, kind);
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut column_count = 0usize;

    if let Some(separator) = separator {
        if let Some(first) = lines.first() {
            headers = split_row(first, separator);
            column_count = headers.len();
        }
        for line in lines.iter().skip(1).take(25) {
            let row = split_row(line, separator);
            column_count = column_count.max(row.len());
            rows.push(row);
        }
    }

    let raw_preview = text.chars().take(6000).collect::<String>();
    SyncPreview {
        path: path.to_string_lossy().into_owned(),
        file_name: path.file_name().and_then(|value| value.to_str()).unwrap_or("export").to_string(),
        kind: kind.to_string(),
        line_count: lines.len(),
        column_count,
        separator: separator.map(|value| if value == '\t' { "TAB".to_string() } else { value.to_string() }),
        headers,
        rows,
        raw_preview,
        duplicate,
    }
}

#[tauri::command]
pub fn import_charlemagne_sync_file(app: AppHandle, path: String) -> Result<SyncPreview, String> {
    let file_path = Path::new(&path);
    if !file_path.is_file() {
        return Err("Le fichier d'export Charlemagne n'est pas accessible.".to_string());
    }
    let kind = kind_for_path(file_path)?;
    let hash = file_sha256(file_path)?;
    let connection = open_database(&app)?;
    let existing_path: Option<String> = connection.query_row(
        "SELECT path FROM charlemagne_sync_imports WHERE content_hash=?1 LIMIT 1",
        params![hash],
        |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;

    let text = read_document(file_path, kind)?;
    let preview = build_preview(file_path, kind, &text, existing_path.is_some());
    if existing_path.is_none() {
        connection.execute(
            "INSERT INTO charlemagne_sync_imports(path,file_name,kind,status,content_hash,line_count,column_count,separator,raw_preview)
             VALUES(?1,?2,?3,'importe_a_mapper',?4,?5,?6,?7,?8)",
            params![
                path,
                preview.file_name,
                kind,
                hash,
                preview.line_count as i64,
                preview.column_count as i64,
                preview.separator,
                preview.raw_preview,
            ],
        ).map_err(|error| error.to_string())?;
    }
    Ok(preview)
}

#[tauri::command]
pub fn list_charlemagne_sync_imports(app: AppHandle) -> Result<Vec<SyncImportRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection.prepare(
        "SELECT path,file_name,kind,status,content_hash,line_count,column_count,separator,imported_at
         FROM charlemagne_sync_imports ORDER BY imported_at DESC,file_name ASC"
    ).map_err(|error| error.to_string())?;
    let rows = statement.query_map([], |row| Ok(SyncImportRecord {
        path: row.get(0)?,
        file_name: row.get(1)?,
        kind: row.get(2)?,
        status: row.get(3)?,
        content_hash: row.get(4)?,
        line_count: row.get(5)?,
        column_count: row.get(6)?,
        separator: row.get(7)?,
        imported_at: row.get(8)?,
    })).map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{build_preview, detect_separator};
    use std::path::Path;

    #[test]
    fn detects_tabular_exports() {
        assert_eq!(detect_separator("Date\tCompte\tDebit\n20260822\t6063\t10", "txt"), Some('\t'));
        assert_eq!(detect_separator("Date;Compte;Debit\n20260822;6063;10", "csv"), Some(';'));
    }

    #[test]
    fn preview_keeps_raw_text_for_unknown_mapping() {
        let preview = build_preview(Path::new("journal.txt"), "txt", "Date\tCompte\n20260822\t6063", false);
        assert_eq!(preview.headers, vec!["Date", "Compte"]);
        assert_eq!(preview.rows.len(), 1);
        assert!(!preview.duplicate);
    }
}
