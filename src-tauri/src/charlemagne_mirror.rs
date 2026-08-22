use rusqlite::Connection;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

pub fn initialize(app: &AppHandle) -> Result<(), String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA busy_timeout=5000;
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

             CREATE TRIGGER IF NOT EXISTS trg_charlemagne_supplier_after_insert_known
             AFTER INSERT ON charlemagne_mirror_entries
             WHEN trim(COALESCE(NEW.supplier,'')) <> ''
             BEGIN
                UPDATE charlemagne_mirror_entries
                   SET supplier=NEW.supplier
                 WHERE id<>NEW.id
                   AND date=NEW.date
                   AND COALESCE(journal,'')=COALESCE(NEW.journal,'')
                   AND COALESCE(entry_number,'')=COALESCE(NEW.entry_number,'')
                   AND COALESCE(piece,'')=COALESCE(NEW.piece,'')
                   AND trim(COALESCE(supplier,''))='';
             END;

             CREATE TRIGGER IF NOT EXISTS trg_charlemagne_supplier_after_insert_empty
             AFTER INSERT ON charlemagne_mirror_entries
             WHEN trim(COALESCE(NEW.supplier,'')) = ''
             BEGIN
                UPDATE charlemagne_mirror_entries
                   SET supplier=(
                       SELECT s.supplier
                         FROM charlemagne_mirror_entries s
                        WHERE s.id<>NEW.id
                          AND s.date=NEW.date
                          AND COALESCE(s.journal,'')=COALESCE(NEW.journal,'')
                          AND COALESCE(s.entry_number,'')=COALESCE(NEW.entry_number,'')
                          AND COALESCE(s.piece,'')=COALESCE(NEW.piece,'')
                          AND trim(COALESCE(s.supplier,''))<>''
                        ORDER BY CASE WHEN s.account LIKE '401%' THEN 0 ELSE 1 END,s.id
                        LIMIT 1
                   )
                 WHERE id=NEW.id
                   AND EXISTS(
                       SELECT 1
                         FROM charlemagne_mirror_entries s
                        WHERE s.id<>NEW.id
                          AND s.date=NEW.date
                          AND COALESCE(s.journal,'')=COALESCE(NEW.journal,'')
                          AND COALESCE(s.entry_number,'')=COALESCE(NEW.entry_number,'')
                          AND COALESCE(s.piece,'')=COALESCE(NEW.piece,'')
                          AND trim(COALESCE(s.supplier,''))<>''
                   );
             END;

             UPDATE charlemagne_mirror_entries AS e
                SET supplier=(
                    SELECT s.supplier
                      FROM charlemagne_mirror_entries s
                     WHERE s.id<>e.id
                       AND s.date=e.date
                       AND COALESCE(s.journal,'')=COALESCE(e.journal,'')
                       AND COALESCE(s.entry_number,'')=COALESCE(e.entry_number,'')
                       AND COALESCE(s.piece,'')=COALESCE(e.piece,'')
                       AND trim(COALESCE(s.supplier,''))<>''
                     ORDER BY CASE WHEN s.account LIKE '401%' THEN 0 ELSE 1 END,s.id
                     LIMIT 1
                )
              WHERE trim(COALESCE(e.supplier,''))=''
                AND EXISTS(
                    SELECT 1
                      FROM charlemagne_mirror_entries s
                     WHERE s.id<>e.id
                       AND s.date=e.date
                       AND COALESCE(s.journal,'')=COALESCE(e.journal,'')
                       AND COALESCE(s.entry_number,'')=COALESCE(e.entry_number,'')
                       AND COALESCE(s.piece,'')=COALESCE(e.piece,'')
                       AND trim(COALESCE(s.supplier,''))<>''
                );",
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}
