use crate::windows_ocr;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
pub struct BankDocumentRecord {
    pub path: String,
    pub file_name: String,
    pub status: String,
    pub extraction_status: String,
    pub text_length: i64,
    pub duplicate_of: Option<String>,
    pub error: Option<String>,
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bank_documents (
                path TEXT PRIMARY KEY,
                file_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'nouveau',
                extraction_status TEXT NOT NULL DEFAULT 'attente_stabilite',
                extracted_text TEXT,
                text_length INTEGER NOT NULL DEFAULT 0,
                content_hash TEXT,
                duplicate_of TEXT,
                source_size INTEGER,
                source_modified_ms INTEGER,
                stable_observations INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE INDEX IF NOT EXISTS idx_bank_documents_hash ON bank_documents(content_hash);",
        )
        .map_err(|error| error.to_string())?;
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

fn observation(path: &Path) -> Result<(i64, i64), String> {
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

fn extract_bank_text(connection: &Connection, path: &str) -> Result<(), String> {
    let file_path = Path::new(path);
    let content_hash = file_sha256(file_path)?;
    let duplicate_of: Option<String> = connection
        .query_row(
            "SELECT path FROM bank_documents WHERE content_hash=?1 AND path<>?2 AND status<>'doublon' LIMIT 1",
            params![content_hash, path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    if let Some(existing) = duplicate_of {
        connection
            .execute(
                "UPDATE bank_documents SET content_hash=?2,duplicate_of=?3,status='doublon',extraction_status='doublon',error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, content_hash, existing],
            )
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let length = text
                .chars()
                .filter(|character| !character.is_whitespace())
                .count() as i64;
            let extraction_status = if length >= 40 { "texte_extrait" } else { "ocr_requis" };
            connection
                .execute(
                    "UPDATE bank_documents SET content_hash=?2,duplicate_of=NULL,extracted_text=?3,text_length=?4,extraction_status=?5,status='a_verifier',error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, content_hash, text, length, extraction_status],
                )
                .map_err(|error| error.to_string())?;
        }
        Err(error) => {
            connection
                .execute(
                    "UPDATE bank_documents SET content_hash=?2,duplicate_of=NULL,extraction_status='ocr_requis',status='a_verifier',error=?3,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path, content_hash, error.to_string()],
                )
                .map_err(|database_error| database_error.to_string())?;
        }
    }
    Ok(())
}

fn observe_bank_pdf(connection: &Connection, path: &Path) -> Result<(), String> {
    let path_text = path.to_string_lossy().into_owned();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("releve.pdf")
        .to_string();
    let (size, modified_ms) = observation(path)?;
    let existing: Option<(Option<i64>, Option<i64>, i64, String)> = connection
        .query_row(
            "SELECT source_size,source_modified_ms,stable_observations,extraction_status FROM bank_documents WHERE path=?1",
            params![path_text],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    match existing {
        None => {
            connection
                .execute(
                    "INSERT INTO bank_documents(path,file_name,source_size,source_modified_ms,stable_observations) VALUES (?1,?2,?3,?4,1)",
                    params![path_text, file_name, size, modified_ms],
                )
                .map_err(|error| error.to_string())?;
        }
        Some((previous_size, previous_modified, observations, extraction_status)) => {
            if extraction_status != "attente_stabilite" {
                return Ok(());
            }
            let unchanged = previous_size == Some(size) && previous_modified == Some(modified_ms);
            let new_observations = if unchanged { observations.saturating_add(1) } else { 1 };
            connection
                .execute(
                    "UPDATE bank_documents SET source_size=?2,source_modified_ms=?3,stable_observations=?4,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                    params![path_text, size, modified_ms, new_observations],
                )
                .map_err(|error| error.to_string())?;
            if new_observations >= 2 {
                extract_bank_text(connection, &path_text)?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_bank_watched_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    connection
        .query_row(
            "SELECT value FROM settings WHERE key='bank_watched_folder'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_bank_watched_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err("Le dossier des relevés bancaires n'est pas accessible.".to_string());
    }
    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('bank_watched_folder',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![path],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn scan_bank_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    if !Path::new(&path).is_dir() {
        return Err("Le dossier des relevés bancaires n'est pas accessible.".to_string());
    }
    let connection = open_database(&app)?;
    let mut documents = Vec::new();
    for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
        let file_path = entry.map_err(|error| error.to_string())?.path();
        if file_path.is_file()
            && file_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            observe_bank_pdf(&connection, &file_path)?;
            documents.push(file_path.to_string_lossy().into_owned());
        }
    }
    documents.sort();
    Ok(documents)
}

#[tauri::command]
pub fn list_bank_documents(app: AppHandle) -> Result<Vec<BankDocumentRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,file_name,status,extraction_status,text_length,duplicate_of,error FROM bank_documents ORDER BY first_seen_at DESC,file_name ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(BankDocumentRecord {
                path: row.get(0)?,
                file_name: row.get(1)?,
                status: row.get(2)?,
                extraction_status: row.get(3)?,
                text_length: row.get(4)?,
                duplicate_of: row.get(5)?,
                error: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn run_bank_ocr(app: AppHandle, path: String) -> Result<(), String> {
    let text = windows_ocr::ocr_pdf(&path)?;
    let length = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as i64;
    if length < 20 {
        return Err("L'OCR du relevé bancaire n'a pas trouvé assez de texte exploitable."
            .to_string());
    }
    let connection = open_database(&app)?;
    connection
        .execute(
            "UPDATE bank_documents SET extracted_text=?2,text_length=?3,extraction_status='ocr_termine',status='a_verifier',error=NULL,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, text, length],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_bank_document_text(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    connection
        .query_row(
            "SELECT extracted_text FROM bank_documents WHERE path=?1",
            params![path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
        .map(|value| value.flatten())
}
