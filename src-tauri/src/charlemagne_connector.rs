use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

pub const MODE_IMPORT_V1: &str = "import_file_v1";
pub const MODE_SYNC_V2: &str = "sync_files_v2";
pub const MODE_API_V3: &str = "api_v3";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharlemagneConnectorStatus {
    pub mode: String,
    pub version_label: String,
    pub transport_label: String,
    pub live_ready: bool,
    pub preparation_ready: bool,
    pub blocked_reason: Option<String>,
    pub switch_available: bool,
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
             );",
        )
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn normalize_mode(mode: &str) -> Result<&'static str, String> {
    match mode.trim() {
        MODE_IMPORT_V1 => Ok(MODE_IMPORT_V1),
        MODE_SYNC_V2 => Ok(MODE_SYNC_V2),
        MODE_API_V3 => Ok(MODE_API_V3),
        "api_v2" => Ok(MODE_API_V3),
        _ => Err("Mode Charlemagne inconnu.".to_string()),
    }
}

fn status_for_mode(mode: &str) -> CharlemagneConnectorStatus {
    match mode {
        MODE_SYNC_V2 => CharlemagneConnectorStatus {
            mode: MODE_SYNC_V2.to_string(),
            version_label: "Version 2".to_string(),
            transport_label: "Synchronisation par exports Charlemagne".to_string(),
            live_ready: true,
            preparation_ready: true,
            blocked_reason: Some(
                "Mode principal : les exports Charlemagne alimentent le plan de comptes, le journal et les règles fournisseurs. Aucun accès direct à la base Charlemagne."
                    .to_string(),
            ),
            switch_available: true,
        },
        MODE_API_V3 => CharlemagneConnectorStatus {
            mode: MODE_API_V3.to_string(),
            version_label: "Version 3".to_string(),
            transport_label: "API officielle / partenaire".to_string(),
            live_ready: false,
            preparation_ready: true,
            blocked_reason: Some(
                "Accès API/SDK Aplim non configuré. Le mode reste préparé pour une future connexion officielle."
                    .to_string(),
            ),
            switch_available: true,
        },
        _ => CharlemagneConnectorStatus {
            mode: MODE_IMPORT_V1.to_string(),
            version_label: "Version 1".to_string(),
            transport_label: "Fichier d'import vers Charlemagne".to_string(),
            live_ready: false,
            preparation_ready: true,
            blocked_reason: Some(
                "Mode de secours : le format d'import doit être confirmé sur votre installation avant génération de fichiers de production."
                    .to_string(),
            ),
            switch_available: true,
        },
    }
}

#[tauri::command]
pub fn get_charlemagne_connector_status(
    app: AppHandle,
) -> Result<CharlemagneConnectorStatus, String> {
    let connection = open_database(&app)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT value FROM settings WHERE key='charlemagne_connector_mode'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let mode = stored
        .as_deref()
        .and_then(|value| normalize_mode(value).ok())
        .unwrap_or(MODE_SYNC_V2);
    Ok(status_for_mode(mode))
}

#[tauri::command]
pub fn set_charlemagne_connector_mode(
    app: AppHandle,
    mode: String,
) -> Result<CharlemagneConnectorStatus, String> {
    let mode = normalize_mode(&mode)?;
    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES('charlemagne_connector_mode',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![mode],
        )
        .map_err(|error| error.to_string())?;
    Ok(status_for_mode(mode))
}

#[cfg(test)]
mod tests {
    use super::{normalize_mode, status_for_mode, MODE_API_V3, MODE_IMPORT_V1, MODE_SYNC_V2};

    #[test]
    fn v2_is_the_primary_ready_mode() {
        let status = status_for_mode(MODE_SYNC_V2);
        assert!(status.preparation_ready);
        assert!(status.live_ready);
        assert_eq!(status.mode, MODE_SYNC_V2);
    }

    #[test]
    fn v1_remains_a_safe_fallback() {
        let status = status_for_mode(MODE_IMPORT_V1);
        assert!(!status.live_ready);
    }

    #[test]
    fn api_mode_remains_non_live_until_credentials_exist() {
        let status = status_for_mode(MODE_API_V3);
        assert_eq!(status.mode, MODE_API_V3);
        assert!(!status.live_ready);
    }

    #[test]
    fn migrates_old_api_v2_setting_to_api_v3() {
        assert_eq!(normalize_mode("api_v2").unwrap(), MODE_API_V3);
    }

    #[test]
    fn rejects_unknown_connector_mode() {
        assert!(normalize_mode("direct_database").is_err());
    }
}
