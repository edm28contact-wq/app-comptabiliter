use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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
    let ht_ok = parsed
        .amount_ht
        .as_deref()
        .and_then(super::parse_amount)
        .is_some();
    let vat_ok = parsed
        .amount_vat
        .as_deref()
        .and_then(super::parse_amount)
        .is_some();
    let ttc_ok = parsed
        .amount_ttc
        .as_deref()
        .and_then(super::parse_amount)
        .is_some();
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

fn canonical_field(value: &str) -> String {
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
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn select_string_value<F>(values: Vec<&str>, validator: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let mut groups: HashMap<String, (usize, usize, String)> = HashMap::new();
    for (index, value) in values.into_iter().enumerate() {
        let trimmed = value.trim();
        if !validator(trimmed) {
            continue;
        }
        let key = canonical_field(trimmed);
        if key.is_empty() {
            continue;
        }
        groups
            .entry(key)
            .and_modify(|entry| entry.0 += 1)
            .or_insert((1, index, trimmed.to_string()));
    }
    groups
        .into_values()
        .max_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.cmp(&left.1))
        })
        .map(|(_, _, value)| value)
}

fn select_identifier_value<F>(values: Vec<&str>, validator: F) -> Option<String>
where
    F: Fn(&str) -> bool + Copy,
{
    let valid = select_string_value(values.clone(), validator);
    valid.or_else(|| {
        values
            .into_iter()
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn push_amount_candidate(values: &mut Vec<f64>, candidate: f64) {
    if !candidate.is_finite() || candidate < -0.001 {
        return;
    }
    if !values
        .iter()
        .any(|existing| (existing - candidate).abs() <= 0.005)
    {
        values.push(candidate);
    }
}

fn collect_amount_candidates<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<f64> {
    let mut output = Vec::new();
    for candidate in values.flatten().filter_map(super::parse_amount) {
        push_amount_candidate(&mut output, candidate);
    }
    output
}

fn amount_support<'a>(
    candidate: f64,
    values: impl Iterator<Item = Option<&'a str>>,
) -> i32 {
    values
        .flatten()
        .filter_map(super::parse_amount)
        .filter(|value| (value - candidate).abs() <= 0.02)
        .count() as i32
}

fn format_amount(value: f64) -> String {
    format!("{value:.2}")
}

fn reconcile_amounts(candidates: &[super::ParsedInvoice]) -> Option<(String, String, String)> {
    let mut ht_values = collect_amount_candidates(candidates.iter().map(|item| item.amount_ht.as_deref()));
    let mut vat_values =
        collect_amount_candidates(candidates.iter().map(|item| item.amount_vat.as_deref()));
    let mut ttc_values =
        collect_amount_candidates(candidates.iter().map(|item| item.amount_ttc.as_deref()));

    let base_ht = ht_values.clone();
    let base_vat = vat_values.clone();
    let base_ttc = ttc_values.clone();

    for ht in &base_ht {
        for ttc in &base_ttc {
            if ttc + 0.02 >= *ht {
                push_amount_candidate(&mut vat_values, (ttc - ht).max(0.0));
            }
        }
    }
    for vat in &base_vat {
        for ttc in &base_ttc {
            if ttc + 0.02 >= *vat {
                push_amount_candidate(&mut ht_values, (ttc - vat).max(0.0));
            }
        }
    }
    for ht in &base_ht {
        for vat in &base_vat {
            push_amount_candidate(&mut ttc_values, ht + vat);
        }
    }

    if ht_values.is_empty() || vat_values.is_empty() || ttc_values.is_empty() {
        return None;
    }

    let mut best: Option<(i32, f64, f64, f64)> = None;
    for ht in &ht_values {
        for vat in &vat_values {
            for ttc in &ttc_values {
                if (ht + vat - ttc).abs() > 0.02 {
                    continue;
                }
                let support = amount_support(
                    *ht,
                    candidates.iter().map(|item| item.amount_ht.as_deref()),
                ) * 4
                    + amount_support(
                        *vat,
                        candidates.iter().map(|item| item.amount_vat.as_deref()),
                    ) * 4
                    + amount_support(
                        *ttc,
                        candidates.iter().map(|item| item.amount_ttc.as_deref()),
                    ) * 5;
                let explicit_fields = i32::from(base_ht.iter().any(|value| (value - ht).abs() <= 0.02))
                    + i32::from(base_vat.iter().any(|value| (value - vat).abs() <= 0.02))
                    + i32::from(base_ttc.iter().any(|value| (value - ttc).abs() <= 0.02));
                let score = support + explicit_fields;
                if best
                    .as_ref()
                    .map(|(best_score, _, _, _)| score > *best_score)
                    .unwrap_or(true)
                {
                    best = Some((score, *ht, *vat, *ttc));
                }
            }
        }
    }

    best.map(|(_, ht, vat, ttc)| {
        (
            format_amount(ht),
            format_amount(vat),
            format_amount(ttc),
        )
    })
}

fn fuse_parsed_candidates(candidates: &[super::ParsedInvoice]) -> super::ParsedInvoice {
    if candidates.is_empty() {
        return super::ParsedInvoice::default();
    }
    let mut output = candidates
        .last()
        .cloned()
        .unwrap_or_else(super::ParsedInvoice::default);

    output.supplier = select_string_value(
        candidates
            .iter()
            .filter_map(|item| item.supplier.as_deref())
            .collect(),
        |value| value.len() >= 2,
    );
    output.invoice_number = select_string_value(
        candidates
            .iter()
            .filter_map(|item| item.invoice_number.as_deref())
            .collect(),
        |value| (3..=40).contains(&value.len()),
    );
    output.invoice_date = select_string_value(
        candidates
            .iter()
            .filter_map(|item| item.invoice_date.as_deref())
            .collect(),
        super::is_plausible_invoice_date,
    );

    if let Some((ht, vat, ttc)) = reconcile_amounts(candidates) {
        output.amount_ht = Some(ht);
        output.amount_vat = Some(vat);
        output.amount_ttc = Some(ttc);
    } else {
        output.amount_ht = select_string_value(
            candidates
                .iter()
                .filter_map(|item| item.amount_ht.as_deref())
                .collect(),
            |value| super::parse_amount(value).is_some(),
        );
        output.amount_vat = select_string_value(
            candidates
                .iter()
                .filter_map(|item| item.amount_vat.as_deref())
                .collect(),
            |value| super::parse_amount(value).is_some(),
        );
        output.amount_ttc = select_string_value(
            candidates
                .iter()
                .filter_map(|item| item.amount_ttc.as_deref())
                .collect(),
            |value| super::parse_amount(value).is_some(),
        );
    }

    output.siret = select_identifier_value(
        candidates
            .iter()
            .filter_map(|item| item.siret.as_deref())
            .collect(),
        super::identifiers::is_valid_siret,
    );
    output.iban = select_identifier_value(
        candidates
            .iter()
            .filter_map(|item| item.iban.as_deref())
            .collect(),
        super::identifiers::is_valid_iban,
    );

    normalize_strict_fields(output)
}

fn optimized_parse(text: &str) -> (String, super::ParsedInvoice, bool) {
    let receipt_like = super::receipt::is_receipt_like(text);
    let augmented = super::receipt::augment_if_receipt(text);
    let parsed = normalize_strict_fields(super::parse_invoice_text(&augmented));
    (augmented, parsed, receipt_like)
}

fn persist_optimized(
    app: &AppHandle,
    path: &str,
    text: &str,
    extraction_status: &str,
    parsed: super::ParsedInvoice,
    receipt_like: bool,
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
    let old_text = old
        .as_ref()
        .and_then(|value| value.0.as_deref())
        .unwrap_or("");
    let old_json = old
        .as_ref()
        .and_then(|value| value.1.as_deref())
        .unwrap_or("");

    let promoted = strict_complete(&parsed);
    let json = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
    let changed = old_text != text || old_json != json;

    if changed {
        let length = text
            .chars()
            .filter(|character| !character.is_whitespace())
            .count() as i64;
        connection
            .execute(
                "UPDATE invoices SET extracted_text=?2,extraction_status=?3,extraction_error=NULL,text_length=?4,parsed_json=?5,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
                params![path, text, extraction_status, length, json],
            )
            .map_err(|error| error.to_string())?;
        let detail = if promoted {
            "strict_99"
        } else {
            "manual_required"
        };
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
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
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
        let (native_augmented, preliminary, native_receipt) = optimized_parse(&native_text);

        if strict_complete(&preliminary) {
            match persist_optimized(
                &app,
                &path,
                &native_augmented,
                &extraction_status,
                preliminary,
                native_receipt,
            ) {
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
                    let merged_raw = merge_texts(&native_text, &ocr_text);
                    let (ocr_augmented, ocr_parsed, ocr_receipt) = optimized_parse(&ocr_text);
                    let (merged_augmented, merged_parsed, merged_receipt) =
                        optimized_parse(&merged_raw);
                    let native_parsed = super::parse_invoice_text(&native_augmented);
                    let fused = fuse_parsed_candidates(&[
                        native_parsed,
                        super::parse_invoice_text(&ocr_augmented),
                        ocr_parsed,
                        merged_parsed,
                    ]);
                    match persist_optimized(
                        &app,
                        &path,
                        &merged_augmented,
                        "ocr_termine",
                        fused,
                        native_receipt || ocr_receipt || merged_receipt,
                    ) {
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
            match persist_optimized(
                &app,
                &path,
                &native_augmented,
                "ocr_termine",
                preliminary,
                native_receipt,
            ) {
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
    use super::{
        fuse_parsed_candidates, merge_texts, normalize_strict_fields, reconcile_amounts,
    };
    use crate::ParsedInvoice;

    fn complete_base() -> ParsedInvoice {
        ParsedInvoice {
            supplier: Some("FOURNISSEUR TEST".to_string()),
            invoice_number: Some("F-2026-42".to_string()),
            invoice_date: Some("23/08/2026".to_string()),
            ..ParsedInvoice::default()
        }
    }

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
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            confidence: 80,
            ..complete_base()
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

    #[test]
    fn reconciles_amounts_split_across_independent_reads() {
        let native = ParsedInvoice {
            amount_ht: Some("100.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            ..complete_base()
        };
        let ocr = ParsedInvoice {
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("12.00".to_string()),
            ..complete_base()
        };
        let merged = ParsedInvoice {
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            ..complete_base()
        };
        let amounts = reconcile_amounts(&[native, ocr, merged]).expect("amounts");
        assert_eq!(amounts.0, "100.00");
        assert_eq!(amounts.1, "20.00");
        assert_eq!(amounts.2, "120.00");
    }

    #[test]
    fn field_voting_prefers_agreement_between_reads() {
        let native = ParsedInvoice {
            supplier: Some("FOURNISSEUR TEST".to_string()),
            invoice_number: Some("F-2026-42".to_string()),
            invoice_date: Some("23/08/2026".to_string()),
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            ..ParsedInvoice::default()
        };
        let ocr = ParsedInvoice {
            invoice_number: Some("F-2026-47".to_string()),
            ..native.clone()
        };
        let merged = ParsedInvoice {
            invoice_number: Some("F-2026-42".to_string()),
            ..native.clone()
        };
        let fused = fuse_parsed_candidates(&[native, ocr, merged]);
        assert_eq!(fused.invoice_number.as_deref(), Some("F-2026-42"));
        assert_eq!(fused.confidence, 99);
    }

    #[test]
    fn invalid_identifier_blocks_strict_99_when_no_pass_repairs_it() {
        let parsed = ParsedInvoice {
            amount_ht: Some("100.00".to_string()),
            amount_vat: Some("20.00".to_string()),
            amount_ttc: Some("120.00".to_string()),
            siret: Some("12345678901234".to_string()),
            ..complete_base()
        };
        let fused = fuse_parsed_candidates(&[parsed]);
        assert!(fused.confidence < 99);
    }
}
