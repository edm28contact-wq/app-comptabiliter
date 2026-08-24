use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::HashSet;
use tauri::AppHandle;

#[derive(Serialize, Default)]
pub struct ReadingOptimizationResult {
    pub inspected: usize,
    pub promoted_99: usize,
    pub deep_ocr: usize,
    pub receipts_normalized: usize,
    pub changed: usize,
    pub errors: usize,
}

fn normalized_line_key(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn merge_texts(primary: &str, secondary: &str) -> String {
    let mut seen = HashSet::new();
    let mut output = String::new();
    for line in primary.lines().chain(secondary.lines()) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = normalized_line_key(trimmed);
        if key.len() < 2 || !seen.insert(key) {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(trimmed);
    }
    output
}

fn non_empty(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| !value.is_empty())
}

fn normalize_strict_fields(mut parsed: super::ParsedInvoice) -> super::ParsedInvoice {
    let ht = parsed.amount_ht.as_deref().and_then(super::parse_amount);
    let vat = parsed.amount_vat.as_deref().and_then(super::parse_amount);
    let ttc = parsed.amount_ttc.as_deref().and_then(super::parse_amount);

    if parsed.amount_vat.is_none() {
        if let (Some(ht), Some(ttc)) = (ht, ttc) {
            if (ht - ttc).abs() <= 0.02 {
                parsed.amount_vat = Some("0.00".to_string());
            }
        }
    }

    parsed.amounts_consistent = super::compute_amount_consistency(&parsed);

    let supplier_ok = non_empty(parsed.supplier.as_deref());
    let reference_ok = parsed
        .invoice_number
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| value.len() >= 3);
    let date_ok = parsed
        .invoice_date
        .as_deref()
        .is_some_and(super::is_plausible_invoice_date);
    let ht_ok = parsed.amount_ht.as_deref().and_then(super::parse_amount).is_some();
    let vat_ok = parsed.amount_vat.as_deref().and_then(super::parse_amount).is_some();
    let ttc_ok = parsed.amount_ttc.as_deref().and_then(super::parse_amount).is_some();
    let arithmetic_ok = parsed.amounts_consistent == Some(true);
    let siret_ok = parsed
        .siret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(super::identifiers::is_valid_siret)
        .unwrap_or(true);
    let iban_ok = parsed
        .iban
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(super::identifiers::is_valid_iban)
        .unwrap_or(true);

    if supplier_ok
        && reference_ok
        && date_ok
        && ht_ok
        && vat_ok
        && ttc_ok
        && arithmetic_ok
        && siret_ok
        && iban_ok
    {
        parsed.confidence = 99;
    } else {
        parsed.confidence = parsed.confidence.min(98);
    }
    parsed
}

fn strict_complete(parsed: &super::ParsedInvoice) -> bool {
    parsed.confidence >= 99
}

fn persist_optimized(
    app: &AppHandle,
    path: &str,
    text: &str,
    extraction_status: &str,
) -> Result<(bool, bool, bool), String> {
    let connection = super::open_database(app)?;
    let old: Option<(Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT extracted_text,parsed_json FROM invoices WHERE path=?1",
            params![path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let old_text = old.as_ref().and_then(|value| value.0.as_deref()).unwrap_or("");
    let old_json = old.as_ref().and_then(|value| value.1.as_deref()).unwrap_or("");

    let receipt_like = super::receipt::is_receipt_like(text);
    let augmented = super::receipt::augment_if_receipt(text);
    let parsed = normalize_strict_fields(super::parse_invoice_text(&augmented));
    let promoted = strict_complete(&parsed);
    let json = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
    let changed = old_text != augmented || old_json != json;

    if changed {
        let length = augmented
            .chars()
            .filter(|character| !character.is_whitespace())
            .count() as i64;
        connection
            .execute(
                "UPDATE invoices SET extracted_text=?2,extraction_status=?3,extraction_error=NULL,text_length=?4,parsed_json=?5,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, augmented, extraction_status, length, json],
            )
            .map_err(|error| error.to_string())?;
        let detail = if promoted { "strict_99" } else { "manual_required" };
        let _ = super::record_audit(
            &connection,
            Some(path),
            "reading_optimized",
            Some(detail),
        );
    }
    Ok((changed, promoted, receipt_like))
}

fn read_text(app: &AppHandle, path: &str) -> Result<String, String> {
    let connection = super::open_database(app)?;
    connection
        .query_row(
            "SELECT COALESCE(extracted_text,'') FROM invoices WHERE path=?1",
            params![path],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn candidate_paths(app: &AppHandle) -> Result<Vec<(String, String)>, String> {
    let connection = super::open_database(app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,extraction_status
             FROM invoices
             WHERE status='nouvelle'
               AND extraction_status IN ('texte_extrait','ocr_termine')
             ORDER BY updated_at ASC
             LIMIT 25",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn optimize_invoice_readings(app: AppHandle) -> Result<ReadingOptimizationResult, String> {
    let candidates = candidate_paths(&app)?;
    let mut result = ReadingOptimizationResult::default();
    let mut deep_ocr_used = false;

    for (path, extraction_status) in candidates {
        result.inspected += 1;
        let native_text = match read_text(&app, &path) {
            Ok(value) => value,
            Err(_) => {
                result.errors += 1;
                continue;
            }
        };

        let preliminary = normalize_strict_fields(super::parse_invoice_text(
            &super::receipt::augment_if_receipt(&native_text),
        ));

        if strict_complete(&preliminary) {
            match persist_optimized(&app, &path, &native_text, &extraction_status) {
                Ok((changed, promoted, receipt)) => {
                    result.changed += usize::from(changed);
                    result.promoted_99 += usize::from(promoted && changed);
                    result.receipts_normalized += usize::from(receipt && changed);
                }
                Err(_) => result.errors += 1,
            }
            continue;
        }

        if extraction_status == "texte_extrait" && !deep_ocr_used {
            deep_ocr_used = true;
            match super::run_invoice_ocr(app.clone(), path.clone()) {
                Ok(()) => {
                    result.deep_ocr += 1;
                    let ocr_text = read_text(&app, &path).unwrap_or_default();
                    let merged = merge_texts(&native_text, &ocr_text);
                    match persist_optimized(&app, &path, &merged, "ocr_termine") {
                        Ok((changed, promoted, receipt)) => {
                            result.changed += usize::from(changed);
                            result.promoted_99 += usize::from(promoted);
                            result.receipts_normalized += usize::from(receipt && changed);
                        }
                        Err(_) => result.errors += 1,
                    }
                }
                Err(_) => result.errors += 1,
            }
            continue;
        }

        if extraction_status == "ocr_termine" {
            match persist_optimized(&app, &path, &native_text, "ocr_termine") {
                Ok((changed, promoted, receipt)) => {
                    result.changed += usize::from(changed);
                    result.promoted_99 += usize::from(promoted && changed);
                    result.receipts_normalized += usize::from(receipt && changed);
                }
                Err(_) => result.errors += 1,
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{merge_texts, normalize_strict_fields};
    use crate::ParsedInvoice;

    #[test]
    fn merges_native_and_ocr_without_duplicate_lines() {
        let merged = merge_texts(
            "EDF\nFacture N° A-42\nTotal TTC 120,00",
            "EDF\nDate facture : 23/08/2026\nTotal TTC 120,00",
        );
        assert_eq!(merged.matches("EDF").count(), 1);
        assert_eq!(merged.matches("Total TTC 120,00").count(), 1);
        assert!(merged.contains("Date facture : 23/08/2026"));
    }

    #[test]
    fn gives_99_only_to_complete_consistent_reading() {
        let parsed = ParsedInvoice {
            supplier: Some("FOURNISSEUR TEST".to_string()),
            invoice_number: Some("F-2026-42".to_string()),
            invoice_date: Some("23/08/2026".to_string()),
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            confidence: 80,
            ..ParsedInvoice::default()
        };
        let normalized = normalize_strict_fields(parsed);
        assert_eq!(normalized.confidence, 99);
        assert_eq!(normalized.amounts_consistent, Some(true));
    }

    #[test]
    fn infers_zero_vat_only_when_ht_equals_ttc() {
        let parsed = ParsedInvoice {
            supplier: Some("ASSOCIATION TEST".to_string()),
            invoice_number: Some("R-42".to_string()),
            invoice_date: Some("23/08/2026".to_string()),
            amount_ht: Some("50.00".to_string()),
            amount_vat: None,
            amount_ttc: Some("50.00".to_string()),
            confidence: 70,
            ..ParsedInvoice::default()
        };
        let normalized = normalize_strict_fields(parsed);
        assert_eq!(normalized.amount_vat.as_deref(), Some("0.00"));
        assert_eq!(normalized.confidence, 99);
    }

    #[test]
    fn incomplete_reading_never_reaches_99() {
        let parsed = ParsedInvoice {
            supplier: Some("TEST".to_string()),
            invoice_number: None,
            invoice_date: Some("23/08/2026".to_string()),
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            confidence: 100,
            ..ParsedInvoice::default()
        };
        assert_eq!(normalize_strict_fields(parsed).confidence, 98);
    }
}
