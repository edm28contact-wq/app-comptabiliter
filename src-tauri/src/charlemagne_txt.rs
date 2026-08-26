use crate::charlemagne::{CharlemagneLine, PreparedCharlemagneEntry};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Manager};

const CHARLEMAGNE_TXT_COLUMN_COUNT: usize = 10;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharlemagneTxtProfile {
    pub journal: String,
    pub debit_marker: String,
    pub credit_marker: String,
    pub decimal_separator: String,
    pub analytic_label: Option<String>,
    pub specification_confirmed: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CharlemagneTxtPreview {
    pub format_label: String,
    pub column_count: usize,
    pub line_count: usize,
    pub separator: String,
    pub content: String,
    pub rows: Vec<Vec<String>>,
    pub production_ready: bool,
    pub warnings: Vec<String>,
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    Ok(data_dir.join("app-comptabiliter.sqlite3"))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA busy_timeout=5000;")
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn sanitize_field(value: &str) -> String {
    value
        .replace(['\t', '\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn valid_date(year: u32, month: u32, day: u32) -> bool {
    if !(1900..=2100).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let max_day = match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

fn normalize_date_yyyymmdd(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() == 8 && value.chars().all(|character| character.is_ascii_digit()) {
        let year = value[0..4].parse::<u32>().map_err(|error| error.to_string())?;
        let month = value[4..6].parse::<u32>().map_err(|error| error.to_string())?;
        let day = value[6..8].parse::<u32>().map_err(|error| error.to_string())?;
        if valid_date(year, month, day) {
            return Ok(value.to_string());
        }
        return Err(format!("Date Charlemagne invalide : {value}"));
    }

    let parts = value
        .split(|character| matches!(character, '/' | '.' | '-'))
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(format!("Format de date non reconnu : {value}"));
    }

    let (year, month, day) = if parts[0].len() == 4 {
        (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        )
    } else {
        if parts[2].len() != 4 {
            return Err(format!(
                "Année à quatre chiffres requise pour l'aperçu Charlemagne : {value}"
            ));
        }
        (
            parts[2].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[0].parse::<u32>(),
        )
    };

    let year = year.map_err(|_| format!("Année invalide : {value}"))?;
    let month = month.map_err(|_| format!("Mois invalide : {value}"))?;
    let day = day.map_err(|_| format!("Jour invalide : {value}"))?;
    if !valid_date(year, month, day) {
        return Err(format!("Date Charlemagne invalide : {value}"));
    }
    Ok(format!("{year:04}{month:02}{day:02}"))
}

fn parse_amount(value: &str) -> Result<f64, String> {
    let cleaned = value
        .replace('€', "")
        .replace("EUR", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    let normalized = if cleaned.contains(',') {
        cleaned.replace('.', "").replace(',', ".")
    } else {
        cleaned
    };
    normalized
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
        .ok_or_else(|| format!("Montant invalide : {value}"))
}

fn format_amount(value: f64, decimal_separator: &str) -> Result<String, String> {
    let formatted = format!("{value:.2}");
    match decimal_separator {
        "." => Ok(formatted),
        "," => Ok(formatted.replace('.', ",")),
        _ => Err("Le séparateur décimal doit être '.' ou ','.".to_string()),
    }
}

fn amount_and_direction(
    line: &CharlemagneLine,
    profile: &CharlemagneTxtProfile,
) -> Result<(String, String), String> {
    let debit = parse_amount(&line.debit)?;
    let credit = parse_amount(&line.credit)?;
    let debit_non_zero = debit.abs() >= 0.005;
    let credit_non_zero = credit.abs() >= 0.005;

    match (debit_non_zero, credit_non_zero) {
        (true, false) => Ok((
            format_amount(debit, &profile.decimal_separator)?,
            sanitize_field(&profile.debit_marker),
        )),
        (false, true) => Ok((
            format_amount(credit, &profile.decimal_separator)?,
            sanitize_field(&profile.credit_marker),
        )),
        (true, true) => Err(format!(
            "La ligne du compte {} contient simultanément un débit et un crédit.",
            line.account
        )),
        (false, false) => Err(format!(
            "La ligne du compte {} ne contient aucun montant.",
            line.account
        )),
    }
}

fn resolve_account_label(
    connection: &Connection,
    account: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT account_label
               FROM charlemagne_mirror_entries
              WHERE account=?1
                AND trim(COALESCE(account_label,''))<>''
              GROUP BY account_label
              ORDER BY COUNT(*) DESC, MAX(updated_at) DESC
              LIMIT 1",
            params![account],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn row_for_line(
    entry: &PreparedCharlemagneEntry,
    line: &CharlemagneLine,
    profile: &CharlemagneTxtProfile,
    account_label: &str,
) -> Result<Vec<String>, String> {
    if profile.journal.trim().is_empty() {
        return Err("Le code journal Charlemagne est requis.".to_string());
    }
    if profile.debit_marker.trim().is_empty() || profile.credit_marker.trim().is_empty() {
        return Err("Les marqueurs débit et crédit sont requis.".to_string());
    }
    if profile.debit_marker.trim() == profile.credit_marker.trim() {
        return Err("Les marqueurs débit et crédit doivent être différents.".to_string());
    }

    let (amount, direction) = amount_and_direction(line, profile)?;
    let analytic_code = line.analytic_code.as_deref().unwrap_or("");
    let analytic_label = if analytic_code.trim().is_empty() {
        String::new()
    } else {
        sanitize_field(profile.analytic_label.as_deref().unwrap_or(""))
    };

    Ok(vec![
        normalize_date_yyyymmdd(&entry.date)?,
        sanitize_field(&profile.journal),
        sanitize_field(&line.account),
        sanitize_field(account_label),
        sanitize_field(&entry.reference),
        sanitize_field(&line.label),
        amount,
        direction,
        sanitize_field(analytic_code),
        analytic_label,
    ])
}

fn push_length_warning(
    warnings: &mut Vec<String>,
    row_index: usize,
    label: &str,
    value: &str,
    observed_max: usize,
) {
    if value.chars().count() > observed_max {
        warnings.push(format!(
            "Ligne {row_index} : {label} dépasse la limite observée de {observed_max} caractères."
        ));
    }
}

fn inspect_observed_limits(row: &[String], row_index: usize, warnings: &mut Vec<String>) {
    push_length_warning(warnings, row_index, "journal", &row[1], 6);
    push_length_warning(warnings, row_index, "compte", &row[2], 15);
    push_length_warning(warnings, row_index, "libellé compte", &row[3], 60);
    push_length_warning(warnings, row_index, "pièce", &row[4], 20);
    push_length_warning(warnings, row_index, "libellé opération", &row[5], 60);
    push_length_warning(warnings, row_index, "analytique", &row[8], 15);
    push_length_warning(warnings, row_index, "libellé analytique", &row[9], 60);
}

fn serialize_rows(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut output = rows
        .iter()
        .map(|row| row.join("\t"))
        .collect::<Vec<_>>()
        .join("\r\n");
    output.push_str("\r\n");
    output
}

fn load_prepared_entry(
    connection: &Connection,
    invoice_path: &str,
) -> Result<PreparedCharlemagneEntry, String> {
    let prepared_json = connection
        .query_row(
            "SELECT prepared_charlemagne_json FROM invoices WHERE path=?1",
            params![invoice_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten()
        .ok_or_else(|| "Aucune écriture Charlemagne préparée pour cette facture.".to_string())?;
    serde_json::from_str(&prepared_json).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn preview_charlemagne_import_txt(
    app: AppHandle,
    invoice_path: String,
    profile: CharlemagneTxtProfile,
) -> Result<CharlemagneTxtPreview, String> {
    let connection = open_database(&app)?;
    let entry = load_prepared_entry(&connection, &invoice_path)?;
    let mut rows = Vec::with_capacity(entry.lines.len());
    let mut warnings = Vec::new();

    if !profile.specification_confirmed {
        warnings.push(
            "Structure TXT provisoire : la spécification exacte doit encore être confirmée dans l'aide APLIM/Charlemagne."
                .to_string(),
        );
    }
    warnings.push(
        "Aucun fichier n'est écrit : l'en-tête éventuel et l'encodage final restent à confirmer."
            .to_string(),
    );

    for (index, line) in entry.lines.iter().enumerate() {
        let account_label = resolve_account_label(&connection, &line.account)?.unwrap_or_default();
        if account_label.is_empty() {
            warnings.push(format!(
                "Ligne {} : aucun libellé de compte Charlemagne connu pour {}.",
                index + 1,
                line.account
            ));
        }
        if line.analytic_code.as_deref().is_some_and(|value| !value.trim().is_empty())
            && profile
                .analytic_label
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            warnings.push(format!(
                "Ligne {} : code analytique présent mais libellé analytique non configuré.",
                index + 1
            ));
        }

        let row = row_for_line(&entry, line, &profile, &account_label)?;
        debug_assert_eq!(row.len(), CHARLEMAGNE_TXT_COLUMN_COUNT);
        inspect_observed_limits(&row, index + 1, &mut warnings);
        rows.push(row);
    }

    Ok(CharlemagneTxtPreview {
        format_label: "Charlemagne TXT provisoire - 10 colonnes".to_string(),
        column_count: CHARLEMAGNE_TXT_COLUMN_COUNT,
        line_count: rows.len(),
        separator: "tabulation".to_string(),
        content: serialize_rows(&rows),
        rows,
        production_ready: false,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> CharlemagneTxtProfile {
        CharlemagneTxtProfile {
            journal: "ACH".to_string(),
            debit_marker: "D".to_string(),
            credit_marker: "C".to_string(),
            decimal_separator: ".".to_string(),
            analytic_label: Some("COLLEGE".to_string()),
            specification_confirmed: false,
        }
    }

    fn entry() -> PreparedCharlemagneEntry {
        PreparedCharlemagneEntry {
            date: "18/08/2026".to_string(),
            reference: "FAC-874521".to_string(),
            supplier: "EDF".to_string(),
            invoice_number: "FAC-874521".to_string(),
            currency: "EUR".to_string(),
            total: "1200.00".to_string(),
            document_path: None,
            lines: Vec::new(),
            warnings: Vec::new(),
            adapter_status: "adaptateur_non_configure".to_string(),
        }
    }

    #[test]
    fn converts_supported_dates_to_yyyymmdd() {
        assert_eq!(normalize_date_yyyymmdd("18/08/2026").unwrap(), "20260818");
        assert_eq!(normalize_date_yyyymmdd("2026-08-18").unwrap(), "20260818");
        assert_eq!(normalize_date_yyyymmdd("20260818").unwrap(), "20260818");
        assert!(normalize_date_yyyymmdd("18/08/26").is_err());
        assert!(normalize_date_yyyymmdd("31/02/2026").is_err());
    }

    #[test]
    fn produces_exactly_ten_columns_in_observed_order() {
        let entry = entry();
        let line = CharlemagneLine {
            account: "606100".to_string(),
            debit: "1000.00".to_string(),
            credit: "0.00".to_string(),
            analytic_code: Some("ENS".to_string()),
            label: "EDF - facture FAC-874521".to_string(),
        };
        let row = row_for_line(&entry, &line, &profile(), "Electricite").unwrap();
        assert_eq!(row.len(), 10);
        assert_eq!(row[0], "20260818");
        assert_eq!(row[1], "ACH");
        assert_eq!(row[2], "606100");
        assert_eq!(row[3], "Electricite");
        assert_eq!(row[4], "FAC-874521");
        assert_eq!(row[5], "EDF - facture FAC-874521");
        assert_eq!(row[6], "1000.00");
        assert_eq!(row[7], "D");
        assert_eq!(row[8], "ENS");
        assert_eq!(row[9], "COLLEGE");
    }

    #[test]
    fn maps_credit_to_configured_credit_marker() {
        let entry = entry();
        let line = CharlemagneLine {
            account: "401EDF".to_string(),
            debit: "0".to_string(),
            credit: "1200".to_string(),
            analytic_code: None,
            label: "EDF".to_string(),
        };
        let row = row_for_line(&entry, &line, &profile(), "EDF").unwrap();
        assert_eq!(row[6], "1200.00");
        assert_eq!(row[7], "C");
        assert_eq!(row[8], "");
        assert_eq!(row[9], "");
    }

    #[test]
    fn supports_configurable_decimal_separator() {
        let entry = entry();
        let line = CharlemagneLine {
            account: "606100".to_string(),
            debit: "1.248,72".to_string(),
            credit: "0".to_string(),
            analytic_code: None,
            label: "Test".to_string(),
        };
        let mut profile = profile();
        profile.decimal_separator = ",".to_string();
        let row = row_for_line(&entry, &line, &profile, "Test").unwrap();
        assert_eq!(row[6], "1248,72");
    }

    #[test]
    fn rejects_ambiguous_accounting_line() {
        let entry = entry();
        let line = CharlemagneLine {
            account: "606100".to_string(),
            debit: "100".to_string(),
            credit: "10".to_string(),
            analytic_code: None,
            label: "Test".to_string(),
        };
        assert!(row_for_line(&entry, &line, &profile(), "Test").is_err());
    }

    #[test]
    fn sanitizes_delimiters_and_uses_windows_line_endings() {
        let row = vec![
            "20260818".to_string(),
            sanitize_field("A\tCH"),
            "606100".to_string(),
            "Compte".to_string(),
            "FAC".to_string(),
            sanitize_field("ligne\noperation"),
            "10.00".to_string(),
            "D".to_string(),
            "".to_string(),
            "".to_string(),
        ];
        let serialized = serialize_rows(&[row]);
        assert_eq!(serialized.matches('\t').count(), 9);
        assert!(serialized.ends_with("\r\n"));
        assert!(!serialized.contains("A\tCH"));
        assert!(!serialized.contains("ligne\noperation"));
    }
}
