use rusqlite::{params, Connection};
use serde::Serialize;
use std::{fs, path::{Path, PathBuf}};
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
struct InvoiceRecord {
    path: String,
    file_name: String,
    source: String,
    status: String,
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
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );"
        )
        .map_err(|error| error.to_string())?;
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

fn store_invoice(connection: &Connection, path: &str, source: &str) -> Result<(), String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string();

    connection
        .execute(
            "INSERT INTO invoices (path, file_name, source)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET updated_at = CURRENT_TIMESTAMP",
            params![path, file_name, source],
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
fn list_invoices(app: AppHandle) -> Result<Vec<InvoiceRecord>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path, file_name, source, status
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
            list_invoices,
            scan_pdf_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
