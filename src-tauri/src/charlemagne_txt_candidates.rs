use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CharlemagneTxtCandidate {
    pub path: String,
    pub file_name: String,
    pub charlemagne_status: String,
    pub archive_path: Option<String>,
    pub prepared_at: Option<String>,
}

#[tauri::command]
pub fn list_charlemagne_txt_candidates(
    app: AppHandle,
) -> Result<Vec<CharlemagneTxtCandidate>, String> {
    let connection = crate::open_database(&app)?;
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
