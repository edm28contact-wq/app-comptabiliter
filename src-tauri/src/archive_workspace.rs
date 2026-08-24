use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use tauri::AppHandle;

#[derive(Serialize)]
pub struct ArchiveWorkspaceItem {
    pub path: String,
    pub file_name: String,
    pub status: String,
    pub archive_path: Option<String>,
    pub archive_error: Option<String>,
    pub archived_at: Option<String>,
    pub supplier: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub amount_ttc: Option<String>,
    pub confidence: i32,
    pub target_folder: Option<String>,
    pub target_source: String,
    pub charlemagne_status: String,
}

#[derive(Serialize)]
pub struct ArchiveRuleRow {
    pub supplier: String,
    pub archive_folder: String,
    pub use_count: i64,
    pub updated_at: String,
}

fn parsed_invoice(value: Option<String>) -> super::ParsedInvoice {
    value
        .as_deref()
        .and_then(|json| serde_json::from_str::<super::ParsedInvoice>(json).ok())
        .unwrap_or_default()
}

fn storage_assignment(value: Option<String>) -> super::StorageAssignment {
    value
        .as_deref()
        .and_then(|json| serde_json::from_str::<super::StorageAssignment>(json).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn list_archive_workspace(app: AppHandle) -> Result<Vec<ArchiveWorkspaceItem>, String> {
    let connection = super::open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,file_name,status,archive_path,archive_error,archived_at,
                    COALESCE(validated_json,parsed_json),validated_storage_json,charlemagne_status
             FROM invoices
             WHERE status<>'doublon'
             ORDER BY COALESCE(archived_at,validated_at,updated_at) DESC,file_name ASC",
        )
        .map_err(|error| error.to_string())?;

    let raw = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);

    let mut output = Vec::with_capacity(raw.len());
    for (path, file_name, status, archive_path, archive_error, archived_at, parsed_json, storage_json, charlemagne_status) in raw {
        let parsed = parsed_invoice(parsed_json);
        let explicit_storage = storage_assignment(storage_json);
        let learned_storage = if explicit_storage.archive_folder.is_none() {
            match parsed.supplier.as_deref() {
                Some(supplier) => super::get_storage_rule(&connection, supplier).ok().flatten(),
                None => None,
            }
        } else {
            None
        };
        let (target_folder, target_source) = if let Some(folder) = explicit_storage.archive_folder {
            (Some(folder), if explicit_storage.source.is_empty() { "validation".to_string() } else { explicit_storage.source })
        } else if let Some(rule) = learned_storage {
            (rule.archive_folder, if rule.source.is_empty() { "memoire_fournisseur".to_string() } else { rule.source })
        } else {
            (None, "aucune".to_string())
        };

        output.push(ArchiveWorkspaceItem {
            path,
            file_name,
            status,
            archive_path,
            archive_error,
            archived_at,
            supplier: parsed.supplier,
            invoice_number: parsed.invoice_number,
            invoice_date: parsed.invoice_date,
            amount_ttc: parsed.amount_ttc,
            confidence: parsed.confidence,
            target_folder,
            target_source,
            charlemagne_status,
        });
    }
    Ok(output)
}

#[tauri::command]
pub fn list_archive_rules(app: AppHandle) -> Result<Vec<ArchiveRuleRow>, String> {
    let connection = super::open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT supplier_name,archive_folder,use_count,updated_at
             FROM supplier_storage_rules
             ORDER BY supplier_name COLLATE NOCASE ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(ArchiveRuleRow {
                supplier: row.get(0)?,
                archive_folder: row.get(1)?,
                use_count: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_invoice_archive_destination(
    app: AppHandle,
    path: String,
    folder: String,
    remember_supplier: bool,
) -> Result<(), String> {
    let folder = folder.trim();
    if folder.is_empty() || !Path::new(folder).is_dir() {
        return Err("Le dossier de classement n'est pas accessible.".to_string());
    }

    let mut connection = super::open_database(&app)?;
    let row: (String, Option<String>, Option<String>, Option<String>) = connection
        .query_row(
            "SELECT status,archive_path,COALESCE(validated_json,parsed_json),validated_storage_json
             FROM invoices WHERE path=?1",
            params![path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Le document n'existe plus dans la base locale.".to_string())?;

    if row.1.is_some() {
        return Err("Une copie d'archive est déjà enregistrée. Utilisez Reprendre l'archivage plutôt que changer sa destination.".to_string());
    }
    if row.0 != "validee" && row.0 != "archive_erreur" {
        return Err("La facture doit être validée avant de choisir sa destination définitive.".to_string());
    }

    let parsed = parsed_invoice(row.2);
    let mut storage = storage_assignment(row.3);
    storage.archive_folder = Some(folder.to_string());
    storage.confidence = 100;
    storage.source = "classement_manuel".to_string();
    let storage_json = serde_json::to_string(&storage).map_err(|error| error.to_string())?;

    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE invoices
             SET validated_storage_json=?2,status='validee',archive_error=NULL,updated_at=CURRENT_TIMESTAMP
             WHERE path=?1",
            params![path, storage_json],
        )
        .map_err(|error| error.to_string())?;

    if remember_supplier {
        if let Some(supplier) = parsed.supplier.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            super::save_storage_rule(&transaction, supplier, &storage)?;
        }
    }
    super::record_audit(
        &transaction,
        Some(&path),
        "archive_destination_selected",
        Some(folder),
    )?;
    transaction.commit().map_err(|error| error.to_string())
}
