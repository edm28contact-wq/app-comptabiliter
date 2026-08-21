use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
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

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;

    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?;

    for name in names {
        if name.map_err(|error| error.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_invoice_columns(connection: &Connection) -> Result<(), String> {
    let columns = [
        ("extracted_text", "TEXT"),
        ("extraction_status", "TEXT NOT NULL DEFAULT 'a_analyser'"),
        ("extraction_error", "TEXT"),
        ("text_length", "INTEGER NOT NULL DEFAULT 0"),
    ];

    for (name, definition) in columns {
        if !column_exists(connection, "invoices", name)? {
            connection
                .execute(
                    &format!("ALTER TABLE invoices ADD COLUMN {name} {definition}"),
                    [],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
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
                text_length INTEGER NOT NULL DEFAULT 0
            );"
        )
        .map_err(|error| error.to_string())?;
    ensure_invoice_columns(&connection)?;
    Ok(connection)
}

#[tauri::command]
fn get_watched_folder(app: AppHandle) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare("SELECT value FROM settings WHERE key = 'watched_folder'")
        .map_err(|error| error.to_string())?;

    match statement.query_row([], |row| row.get::<_, String>(0)) {
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
            "INSERT INTO settings (key, value) VALUES ('watched_folder', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![path],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn extract_native_text(connection: &Connection, path: &str) -> Result<(), String> {
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let meaningful_length = text.chars().filter(|character| !character.is_whitespace()).count();
            let extraction_status = if meaningful_length >= 40 {
                "texte_extrait"
            } else {
                "ocr_requis"
            };

            connection
                .execute(
                    "UPDATE invoices
                     SET extracted_text = ?2,
                         extraction_status = ?3,
                         extraction_error = NULL,
                         text_length = ?4,
                         updated_at = CURRENT_TIMESTAMP
                     WHERE path = ?1",
                    params![path, text, extraction_status, meaningful_length as i64],
                )
                .map_err(|error| error.to_string())?;
        }
        Err(error) => {
            connection
                .execute(
                    "UPDATE invoices
                     SET extraction_status = 'ocr_requis',
                         extraction_error = ?2,
                         text_length = 0,
                         updated_at = CURRENT_TIMESTAMP
                     WHERE path = ?1",
                    params![path, error.to_string()],
                )
                .map_err(|database_error| database_error.to_string())?;
        }
    }
    Ok(())
}

fn store_invoice(connection: &Connection, path: &str, source: &str) -> Result<bool, String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string();

    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO invoices (path, file_name, source)
             VALUES (?1, ?2, ?3)",
            params![path, file_name, source],
        )
        .map_err(|error| error.to_string())?
        > 0;

    if inserted {
        extract_native_text(connection, path)?;
    } else {
        connection
            .execute(
                "UPDATE invoices SET updated_at = CURRENT_TIMESTAMP WHERE path = ?1",
                params![path],
            )
            .map_err(|error| error.to_string())?;
    }

    Ok(inserted)
}

#[tauri::command]
fn register_invoice(app: AppHandle, path: String, source: String) -> Result<(), String> {
    if !path.to_lowercase().ends_with(".pdf") {
        return Err("Seuls les fichiers PDF sont acceptés pour le moment.".to_string());
    }
    let connection = open_database(&app)?;
    store_invoice(&connection, &path, &source)?;
    Ok(())
}

#[tauri::command]
fn analyze_invoice(app: AppHandle, path: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    extract_native_text(&connection, &path)
}

#[tauri::command]
fn get_invoice_text(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare("SELECT extracted_text FROM invoices WHERE path = ?1")
        .map_err(|error| error.to_string())?;

    match statement.query_row(params![path], |row| row.get::<_, Option<String>>(0)) {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn list_invoices(app: AppHandle) -> Result<Vec<InvoiceRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path, file_name, source, status, extraction_status, text_length
             FROM invoices
             ORDER BY first_seen_at DESC, file_name ASC"
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
            })
        })
        .map_err(|error| error.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
}

#[tauri::command]
fn scan_pdf_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    let folder = Path::new(&path);

    if !folder.is_dir() {
        return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string());
    }

    let connection = open_database(&app)?;
    let entries = fs::read_dir(folder).map_err(|error| error.to_string())?;
    let mut pdfs = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_path = entry.path();

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_watched_folder,
            set_watched_folder,
            register_invoice,
            analyze_invoice,
            get_invoice_text,
            list_invoices,
            scan_pdf_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
