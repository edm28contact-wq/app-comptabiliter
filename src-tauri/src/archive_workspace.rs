use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tauri::AppHandle;

const ARCHIVE_ROOT_KEY: &str = "archive_root";
const MAX_ARCHIVE_DEPTH: usize = 7;
const MAX_ARCHIVE_FOLDERS: usize = 6000;

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
    pub target_confidence: i32,
    pub charlemagne_status: String,
}

#[derive(Serialize)]
pub struct ArchiveRuleRow {
    pub supplier: String,
    pub archive_folder: String,
    pub use_count: i64,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct ArchiveScanResult {
    pub root: String,
    pub folders_scanned: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct CatalogFolder {
    path: String,
    name: String,
    normalized_name: String,
    normalized_path: String,
    depth: usize,
}

#[derive(Debug, Clone)]
struct FolderSuggestion {
    path: String,
    score: i32,
    exercise_match: bool,
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

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect::<String>()
}

fn compact(value: &str) -> String {
    normalize(value)
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn meaningful_tokens(value: &str) -> Vec<String> {
    let normalized = normalize(value);
    let ignored = [
        "sas",
        "sarl",
        "sa",
        "snc",
        "eurl",
        "gmbh",
        "ltd",
        "limited",
        "societe",
        "company",
        "france",
        "facture",
        "factures",
        "invoice",
        "invoices",
        "archive",
        "archives",
        "fournisseur",
        "fournisseurs",
        "documents",
        "compta",
        "comptabilite",
    ];
    let mut tokens = normalized
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.len() >= 3)
        .filter(|token| !ignored.contains(token))
        .filter(|token| {
            !(token.len() == 4 && token.chars().all(|character| character.is_ascii_digit()))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn extract_year(value: &str) -> Option<i32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 4)
        .filter_map(|part| part.parse::<i32>().ok())
        .find(|year| (2000..=2100).contains(year))
}

fn path_years(value: &str) -> HashSet<i32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 4)
        .filter_map(|part| part.parse::<i32>().ok())
        .filter(|year| (2000..=2100).contains(year))
        .collect()
}

fn ensure_catalog_table(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS archive_folder_catalog (
                path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                normalized_name TEXT NOT NULL,
                normalized_path TEXT NOT NULL,
                depth INTEGER NOT NULL,
                scanned_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE INDEX IF NOT EXISTS idx_archive_folder_catalog_name
             ON archive_folder_catalog(normalized_name);",
        )
        .map_err(|error| error.to_string())
}

fn get_setting(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn collect_archive_folders(root: &Path) -> Result<(Vec<CatalogFolder>, bool), String> {
    let mut folders = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut truncated = false;

    while let Some((folder, depth)) = stack.pop() {
        if folders.len() >= MAX_ARCHIVE_FOLDERS {
            truncated = true;
            break;
        }
        if depth > MAX_ARCHIVE_DEPTH {
            continue;
        }

        let name = folder
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| folder.to_string_lossy().into_owned());
        let path = folder.to_string_lossy().into_owned();
        folders.push(CatalogFolder {
            normalized_name: compact(&name),
            normalized_path: compact(&path),
            name,
            path,
            depth,
        });

        if depth == MAX_ARCHIVE_DEPTH {
            continue;
        }
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut children = Vec::<PathBuf>::new();
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                children.push(child);
            }
        }
        children.sort();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1));
        }
    }
    Ok((folders, truncated))
}

fn load_catalog(connection: &Connection) -> Result<Vec<CatalogFolder>, String> {
    ensure_catalog_table(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT path,name,normalized_name,normalized_path,depth
             FROM archive_folder_catalog ORDER BY depth ASC,path ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(CatalogFolder {
                path: row.get(0)?,
                name: row.get(1)?,
                normalized_name: row.get(2)?,
                normalized_path: row.get(3)?,
                depth: row.get::<_, i64>(4)? as usize,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn supplier_score(supplier: &str, folder: &CatalogFolder) -> i32 {
    let supplier_compact = compact(supplier);
    if supplier_compact.len() < 3 || folder.normalized_name.is_empty() {
        return 0;
    }
    if folder.normalized_name == supplier_compact {
        return 91;
    }
    if folder.normalized_name.contains(&supplier_compact) {
        return 89;
    }
    if supplier_compact.contains(&folder.normalized_name) && folder.normalized_name.len() >= 5 {
        return 86;
    }

    let supplier_tokens = meaningful_tokens(supplier);
    if supplier_tokens.is_empty() {
        return 0;
    }
    let folder_tokens = meaningful_tokens(&folder.name);
    let path_tokens = meaningful_tokens(&folder.path);
    let folder_set = folder_tokens.iter().collect::<HashSet<_>>();
    let path_set = path_tokens.iter().collect::<HashSet<_>>();
    let direct_matches = supplier_tokens
        .iter()
        .filter(|token| folder_set.contains(token))
        .count();
    let path_matches = supplier_tokens
        .iter()
        .filter(|token| path_set.contains(token))
        .count();

    if direct_matches == supplier_tokens.len() {
        return 88 - (folder.depth.min(4) as i32);
    }
    if path_matches == supplier_tokens.len() {
        return 83 - (folder.depth.min(5) as i32);
    }
    if direct_matches > 0 {
        return 68 + ((direct_matches * 18) / supplier_tokens.len()) as i32;
    }
    if path_matches > 0 {
        return 60 + ((path_matches * 16) / supplier_tokens.len()) as i32;
    }
    if folder.normalized_path.contains(&supplier_compact) {
        return 82 - (folder.depth.min(5) as i32);
    }
    0
}

fn suggestion_score(
    supplier: &str,
    invoice_date: Option<&str>,
    folder: &CatalogFolder,
) -> (i32, bool) {
    let base = supplier_score(supplier, folder);
    if base == 0 {
        return (0, false);
    }

    let Some(invoice_year) = invoice_date.and_then(extract_year) else {
        return (base, false);
    };
    let years = path_years(&folder.path);
    if years.contains(&invoice_year) {
        return ((base + 8).min(99), true);
    }
    if !years.is_empty() {
        return ((base - 24).max(0), false);
    }
    (base, false)
}

fn suggest_existing_folder(
    catalog: &[CatalogFolder],
    supplier: &str,
    invoice_date: Option<&str>,
) -> Option<FolderSuggestion> {
    let mut candidates = catalog
        .iter()
        .filter_map(|folder| {
            let (score, exercise_match) = suggestion_score(supplier, invoice_date, folder);
            (score >= 72).then_some(FolderSuggestion {
                path: folder.path.clone(),
                score,
                exercise_match,
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.exercise_match.cmp(&left.exercise_match))
            .then_with(|| right.path.len().cmp(&left.path.len()))
    });

    let best = candidates.first()?.clone();
    if let Some(second) = candidates.get(1) {
        let same_strength = second.score >= best.score - 1;
        let same_exercise = second.exercise_match == best.exercise_match;
        if same_strength && same_exercise && second.path != best.path {
            return None;
        }
    }
    Some(best)
}

#[tauri::command]
pub fn get_archive_root(app: AppHandle) -> Result<Option<String>, String> {
    let connection = super::open_database(&app)?;
    get_setting(&connection, ARCHIVE_ROOT_KEY)
}

#[tauri::command]
pub fn set_archive_root(app: AppHandle, path: String) -> Result<ArchiveScanResult, String> {
    let root = Path::new(path.trim());
    if !root.is_dir() {
        return Err("Le dossier racine des archives n'est pas accessible.".to_string());
    }
    let connection = super::open_database(&app)?;
    connection
        .execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![ARCHIVE_ROOT_KEY, root.to_string_lossy().as_ref()],
        )
        .map_err(|error| error.to_string())?;
    drop(connection);
    scan_archive_tree(app)
}

#[tauri::command]
pub fn scan_archive_tree(app: AppHandle) -> Result<ArchiveScanResult, String> {
    let mut connection = super::open_database(&app)?;
    ensure_catalog_table(&connection)?;
    let root_value = get_setting(&connection, ARCHIVE_ROOT_KEY)?
        .ok_or_else(|| "Aucun dossier racine d'archives n'est configuré.".to_string())?;
    let root = Path::new(&root_value);
    if !root.is_dir() {
        return Err("Le dossier racine des archives n'est plus accessible.".to_string());
    }
    let (folders, truncated) = collect_archive_folders(root)?;

    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM archive_folder_catalog", [])
        .map_err(|error| error.to_string())?;
    for folder in &folders {
        transaction
            .execute(
                "INSERT INTO archive_folder_catalog(path,name,normalized_name,normalized_path,depth,scanned_at)
                 VALUES(?1,?2,?3,?4,?5,CURRENT_TIMESTAMP)",
                params![
                    folder.path,
                    folder.name,
                    folder.normalized_name,
                    folder.normalized_path,
                    folder.depth as i64
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;

    Ok(ArchiveScanResult {
        root: root_value,
        folders_scanned: folders.len(),
        truncated,
    })
}

#[tauri::command]
pub fn list_archive_workspace(app: AppHandle) -> Result<Vec<ArchiveWorkspaceItem>, String> {
    let connection = super::open_database(&app)?;
    let catalog = load_catalog(&connection).unwrap_or_default();
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
    for (
        path,
        file_name,
        status,
        archive_path,
        archive_error,
        archived_at,
        parsed_json,
        storage_json,
        charlemagne_status,
    ) in raw
    {
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
        let inferred_storage = if explicit_storage.archive_folder.is_none() && learned_storage.is_none()
        {
            parsed.supplier.as_deref().and_then(|supplier| {
                suggest_existing_folder(&catalog, supplier, parsed.invoice_date.as_deref())
            })
        } else {
            None
        };

        let (target_folder, target_source, target_confidence) =
            if let Some(folder) = explicit_storage.archive_folder {
                (
                    Some(folder),
                    if explicit_storage.source.is_empty() {
                        "validation".to_string()
                    } else {
                        explicit_storage.source
                    },
                    explicit_storage.confidence.max(100),
                )
            } else if let Some(rule) = learned_storage {
                (
                    rule.archive_folder,
                    if rule.source.is_empty() {
                        "memoire_fournisseur".to_string()
                    } else {
                        rule.source
                    },
                    rule.confidence.max(99),
                )
            } else if let Some(suggestion) = inferred_storage {
                (
                    Some(suggestion.path),
                    if suggestion.exercise_match {
                        "arborescence_exercice".to_string()
                    } else {
                        "arborescence_existante".to_string()
                    },
                    suggestion.score,
                )
            } else {
                (None, "aucune".to_string(), 0)
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
            target_confidence,
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
        if let Some(supplier) = parsed
            .supplier
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
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

#[cfg(test)]
mod tests {
    use super::{
        extract_year, suggest_existing_folder, suggestion_score, CatalogFolder,
    };

    fn folder(path: &str, name: &str, depth: usize) -> CatalogFolder {
        CatalogFolder {
            path: path.to_string(),
            name: name.to_string(),
            normalized_name: super::compact(name),
            normalized_path: super::compact(path),
            depth,
        }
    }

    #[test]
    fn extracts_exercise_from_invoice_date() {
        assert_eq!(extract_year("31/08/2026"), Some(2026));
        assert_eq!(extract_year("2026-08-31"), Some(2026));
    }

    #[test]
    fn exact_supplier_folder_is_high_confidence() {
        let candidate = folder(r"C:\Archives\2026\DARTY ILE DE FRANCE", "DARTY ILE DE FRANCE", 2);
        let (score, exercise_match) = suggestion_score(
            "DARTY ILE DE FRANCE",
            Some("31/08/2026"),
            &candidate,
        );
        assert!(score >= 95);
        assert!(exercise_match);
    }

    #[test]
    fn generic_year_folder_is_not_a_supplier_match() {
        let candidate = folder(r"C:\Archives\2026\Factures", "Factures", 2);
        assert_eq!(
            suggestion_score("DARTY ILE DE FRANCE", Some("31/08/2026"), &candidate).0,
            0
        );
    }

    #[test]
    fn prefers_invoice_exercise_when_supplier_exists_in_multiple_years() {
        let catalog = vec![
            folder(r"C:\Archives\2025\DARTY", "DARTY", 2),
            folder(r"C:\Archives\2026\DARTY", "DARTY", 2),
        ];
        let result = suggest_existing_folder(
            &catalog,
            "DARTY ILE DE FRANCE",
            Some("22/09/2026"),
        )
        .expect("une destination 2026 doit etre trouvee");
        assert!(result.path.contains("2026"));
        assert!(result.exercise_match);
    }

    #[test]
    fn refuses_ambiguous_supplier_folders_without_invoice_date() {
        let catalog = vec![
            folder(r"C:\Archives\2025\DARTY", "DARTY", 2),
            folder(r"C:\Archives\2026\DARTY", "DARTY", 2),
        ];
        assert!(suggest_existing_folder(&catalog, "DARTY ILE DE FRANCE", None).is_none());
    }

    #[test]
    fn picks_supplier_folder_over_generic_parent() {
        let catalog = vec![
            folder(r"C:\Archives", "Archives", 0),
            folder(r"C:\Archives\2026", "2026", 1),
            folder(r"C:\Archives\2026\DARTY", "DARTY", 2),
        ];
        let result = suggest_existing_folder(
            &catalog,
            "DARTY ILE DE FRANCE",
            Some("22/09/2026"),
        );
        assert!(result.is_some());
        assert!(result.unwrap().path.ends_with("DARTY"));
    }
}