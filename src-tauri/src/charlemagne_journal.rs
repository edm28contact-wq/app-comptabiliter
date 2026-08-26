use crate::charlemagne::PreparedCharlemagneEntry;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{collections::HashSet, fs, path::PathBuf};
use tauri::{AppHandle, Manager};

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

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );",
        )
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get::<_, bool>(0),
        )
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

fn normalized_amount(value: &str) -> String {
    let cleaned = value.trim().replace(',', ".");
    cleaned
        .parse::<f64>()
        .map(|amount| format!("{amount:.2}"))
        .unwrap_or_else(|_| "0.00".to_string())
}

fn dedupe_key(date: &str, account: &str, piece: &str, debit: &str, credit: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        date.trim().to_lowercase(),
        account.trim().to_lowercase(),
        piece.trim().to_lowercase(),
        normalized_amount(debit),
        normalized_amount(credit)
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
    let mirror_exists = table_exists(&connection, "charlemagne_mirror_entries")?;
    let use_mirror = mode.as_deref().unwrap_or("sync_files_v2") == "sync_files_v2" && mirror_exists;

    let mut entries = Vec::new();
    let mut mirror_keys = HashSet::new();

    if use_mirror {
        let mut statement = connection
            .prepare(
                "SELECT
                    e.date,
                    e.account,
                    COALESCE(e.piece,''),
                    COALESCE(e.label,''),
                    e.debit,
                    e.credit,
                    NULLIF(COALESCE(e.analytic_code,''),''),
                    COALESCE(
                        NULLIF(e.supplier,''),
                        (
                            SELECT NULLIF(s.supplier,'')
                            FROM charlemagne_mirror_entries s
                            WHERE s.date=e.date
                              AND COALESCE(s.journal,'')=COALESCE(e.journal,'')
                              AND COALESCE(s.entry_number,'')=COALESCE(e.entry_number,'')
                              AND COALESCE(s.piece,'')=COALESCE(e.piece,'')
                              AND trim(COALESCE(s.supplier,''))<>''
                            ORDER BY CASE WHEN s.account LIKE '401%' THEN 0 ELSE 1 END, s.id
                            LIMIT 1
                        ),
                        ''
                    ) AS effective_supplier
                 FROM charlemagne_mirror_entries e
                 ORDER BY e.date,e.account,e.id",
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
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(|error| error.to_string())?;

        for row in rows {
            let (date, account, piece, label, debit, credit, analytic_code, supplier) =
                row.map_err(|error| error.to_string())?;
            let class = account.chars().next().unwrap_or('0');
            mirror_keys.insert(dedupe_key(&date, &account, &piece, &debit, &credit));
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
                analytic_code,
                document_path: None,
                source: "Charlemagne V2".to_string(),
            });
        }
    }

    if table_exists(&connection, "invoices")? {
        let mut statement = connection
            .prepare(
                "SELECT prepared_charlemagne_json FROM invoices
                 WHERE prepared_charlemagne_json IS NOT NULL
                   AND status IN ('validee','classee','archive_source_presente')
                 ORDER BY validated_at ASC",
            )
            .map_err(|error| error.to_string())?;
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
                let key = dedupe_key(
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
    use super::dedupe_key;

    #[test]
    fn journal_dedupe_normalizes_amounts() {
        assert_eq!(
            dedupe_key("23/08/2026", "606300", "F1", "10,00", "0"),
            dedupe_key("23/08/2026", "606300", "F1", "10.00", "0.00")
        );
    }
}
