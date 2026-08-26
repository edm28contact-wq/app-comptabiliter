use rusqlite::Connection;
use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CharlemagneTxtCandidate {
    pub path: String,
    pub file_name: String,
    pub charlemagne_status: String,
    pub archive_path: Option<String>,
    pub prepared_at: Option<String>,
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

#[tauri::command]
pub fn list_charlemagne_txt_candidates(
    app: AppHandle,
) -> Result<Vec<CharlemagneTxtCandidate>, String> {
    let connection = Connection::open(database_path(&app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA busy_timeout=5000;")
        .map_err(|error| error.to_string())?;

    let mut statement = connection
        .prepare(
            "SELECT path,
                    file_name,
                    charlemagne_status,
                    archive_path,
                    charlemagne_prepared_at
               FROM invoices
              WHERE prepared_charlemagne_json IS NOT NULL
                AND trim(prepared_charlemagne_json) <> ''
              ORDER BY COALESCE(charlemagne_prepared_at, updated_at) DESC, file_name ASC
              LIMIT 100",
        )
        .map_err(|error| error.to_string())?;

    let rows = statement
        .query_map([], |row| {
            Ok(CharlemagneTxtCandidate {
                path: row.get(0)?,
                file_name: row.get(1)?,
                charlemagne_status: row.get(2)?,
                archive_path: row.get(3)?,
                prepared_at: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}
