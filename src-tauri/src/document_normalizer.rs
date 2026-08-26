use regex::Regex;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashSet;
use tauri::AppHandle;

const MARKER: &str = "--- CHAMPS NORMALISES TEXTE PDF ---";
const ORIGINAL_MARKER: &str = "--- TEXTE PDF ORIGINAL ---";

#[derive(Default, Serialize)]
pub struct NativeNormalizationResult {
    pub inspected: usize,
    pub normalized: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Default, Debug)]
struct StrongFields {
    supplier: Option<String>,
    invoice_number: Option<String>,
    invoice_date: Option<String>,
    amount_ht: Option<f64>,
    amount_vat: Option<f64>,
    amount_ttc: Option<f64>,
    siret: Option<String>,
    iban: Option<String>,
}

fn normalize_word(value: &str) -> String {
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
        .collect()
}

fn compact(value: &str) -> String {
    normalize_word(value)
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn original_text(text: &str) -> &str {
    text.rsplit_once(ORIGINAL_MARKER)
        .map(|(_, original)| original.trim())
        .unwrap_or(text)
}

fn parse_money(value: &str) -> Option<f64> {
    let mut raw = value
        .trim()
        .replace('€', "")
        .replace('$', "")
        .replace('£', "")
        .replace("EUR", "")
        .replace("eur", "")
        .replace("USD", "")
        .replace("CAD", "")
        .replace("GBP", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if raw.is_empty() {
        return None;
    }
    let comma = raw.rfind(',');
    let dot = raw.rfind('.');
    raw = match (comma, dot) {
        (Some(comma_index), Some(dot_index)) if comma_index > dot_index => {
            raw.replace('.', "").replace(',', ".")
        }
        (Some(_), Some(_)) => raw.replace(',', ""),
        (Some(_), None) => raw.replace(',', "."),
        (None, Some(_)) if raw.matches('.').count() > 1 => {
            let mut pieces = raw.split('.').collect::<Vec<_>>();
            let decimal = pieces.pop().unwrap_or("0");
            format!("{}.{}", pieces.join(""), decimal)
        }
        _ => raw,
    };
    let parsed = raw.parse::<f64>().ok()?;
    parsed.is_finite().then_some(parsed)
}

fn money_values(line: &str) -> Vec<f64> {
    let Ok(regex) = Regex::new(
        r"(?i)(?:\d{1,3}(?:[ .\u{00a0}\u{202f}]\d{3})+|\d+)(?:[,.]\d{2,3})",
    ) else {
        return Vec::new();
    };
    regex
        .find_iter(line)
        .filter_map(|capture| {
            let after = line.get(capture.end()..).unwrap_or("");
            if after.trim_start().starts_with('%') {
                None
            } else {
                parse_money(capture.as_str())
            }
        })
        .collect()
}

fn normalize_date(day: u32, month: u32, mut year: u32) -> Option<String> {
    if year < 100 {
        year += 2000;
    }
    if (1..=31).contains(&day)
        && (1..=12).contains(&month)
        && (1900..=2100).contains(&year)
    {
        Some(format!("{day:02}/{month:02}/{year:04}"))
    } else {
        None
    }
}

fn date_in_line(line: &str) -> Option<String> {
    let iso = Regex::new(
        r"(?:^|[^0-9])(19\d{2}|20\d{2})\s*[./-]\s*(\d{1,2})\s*[./-]\s*(\d{1,2})(?:[^0-9]|$)",
    )
    .ok()?;
    if let Some(captures) = iso.captures(line) {
        return normalize_date(
            captures.get(3)?.as_str().parse().ok()?,
            captures.get(2)?.as_str().parse().ok()?,
            captures.get(1)?.as_str().parse().ok()?,
        );
    }
    let french = Regex::new(
        r"(?:^|[^0-9])(\d{1,2})\s*[./-]\s*(\d{1,2})\s*[./-]\s*(\d{2,4})(?:[^0-9]|$)",
    )
    .ok()?;
    let captures = french.captures(line)?;
    normalize_date(
        captures.get(1)?.as_str().parse().ok()?,
        captures.get(2)?.as_str().parse().ok()?,
        captures.get(3)?.as_str().parse().ok()?,
    )
}

fn invoice_date(text: &str) -> Option<String> {
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().take(120).enumerate() {
        let key = compact(line);
        let priority = key.contains("facture")
            || key.contains("invoice")
            || key.contains("datefacturation")
            || key.contains("datefacture")
            || key.starts_with("du")
            || key.starts_with("date");
        if !priority {
            continue;
        }
        if let Some(date) = date_in_line(line) {
            return Some(date);
        }
        for next in lines.iter().skip(index + 1).take(4) {
            if let Some(date) = date_in_line(next) {
                return Some(date);
            }
        }
    }
    None
}

fn clean_invoice_number(value: &str) -> Option<String> {
    let mut cleaned = value
        .trim()
        .trim_matches('#')
        .trim_matches(':')
        .trim_matches('-')
        .trim()
        .to_string();
    if cleaned
        .chars()
        .all(|character| character.is_ascii_digit() || character == ' ')
    {
        cleaned.retain(|character| character != ' ');
    }
    if !(3..=40).contains(&cleaned.len()) || date_in_line(&cleaned).is_some() {
        return None;
    }
    let key = compact(&cleaned);
    if ["france", "atelier", "facture", "invoice", "date"]
        .iter()
        .any(|blocked| key == *blocked)
    {
        return None;
    }
    let digits = cleaned
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    (digits >= 2).then_some(cleaned)
}

fn invoice_number(text: &str) -> Option<String> {
    let patterns = [
        r"(?i)FACTURE\s*/\s*INVOICE\s*[:#-]\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)INVOICE\s*(?:NO\.?|NUMBER)\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)FACTURE\s+F\d{3}\s+([A-Z0-9][A-Z0-9._/-]{3,})\s+du\b",
        r"(?i)#\s*FACTURE\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)FACTURE\s+NUM(?:E|É)RO\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)F\s*A\s*C\s*T\s*U\s*R\s*E\s*N\s*[°Oº]?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)FACTURE\s*N\s*[°Oº]?\s*[:#-]?\s*([0-9][0-9 ._/-]{2,20})(?:\s+du\b|\s*$)",
        r"(?i)(?:N|NO|NR)\s*[°Oº.]?\s*(?:DE\s+)?FACTURE\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
    ];
    for pattern in patterns {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        if let Some(value) = regex
            .captures(text)
            .and_then(|captures| captures.get(1))
            .and_then(|value| clean_invoice_number(value.as_str()))
        {
            return Some(value);
        }
    }

    let lines = text.lines().collect::<Vec<_>>();
    let standalone_year_number = Regex::new(r"\b(20\d{2}/\d{4,8})\b").ok()?;
    let standalone_number = Regex::new(r"(?i)N\s*[°Oº]?\s*([0-9][0-9 ]{4,20})\s+du\b").ok()?;
    let generic_reference = Regex::new(r"(?i)#?([A-Z]{0,5}\d[A-Z0-9/_-]{2,39})").ok()?;
    for (index, line) in lines.iter().take(90).enumerate() {
        let key = compact(line);
        if key.contains("facture") || key.contains("invoice") {
            for next in lines.iter().skip(index + 1).take(5) {
                for regex in [&standalone_number, &standalone_year_number, &generic_reference] {
                    if let Some(value) = regex
                        .captures(next)
                        .and_then(|captures| captures.get(1))
                        .and_then(|value| clean_invoice_number(value.as_str()))
                    {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn column_segments(line: &str) -> Vec<String> {
    let Ok(regex) = Regex::new(r"(?:\t+|\s{4,})") else {
        return vec![line.trim().to_string()];
    };
    regex
        .split(line)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn looks_like_address(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.chars().next().is_some_and(|character| character.is_ascii_digit()) {
        return true;
    }
    Regex::new(r"\b\d{5}\b")
        .ok()
        .is_some_and(|regex| regex.is_match(trimmed))
}

fn supplier_segment_usable(value: &str) -> bool {
    let trimmed = value.trim().trim_matches('-').trim_matches('|').trim();
    if trimmed.len() < 3 || trimmed.len() > 70 || trimmed.contains('@') || looks_like_address(trimmed) {
        return false;
    }
    let key = compact(trimmed);
    let blocked_contains = [
        "facture", "invoice", "commande", "client", "livraison", "page", "date",
        "siret", "siren", "iban", "tva", "total", "montant", "reglement", "reference",
        "designation", "quantite", "telephone", "servicecomptabilite", "echeance", "garantie",
        "certificat", "codeclient", "numeroclient", "reparateuragrees", "vehiculesutilitaires",
    ];
    if blocked_contains.iter().any(|needle| key.contains(needle)) {
        return false;
    }
    if ["france", "atelier", "adresse", "vendeur", "magasin", "commercial"]
        .iter()
        .any(|blocked| key == *blocked)
    {
        return false;
    }
    let personal = Regex::new(r"(?i)^(?:M|MME|MR|MRS|MONSIEUR|MADAME)\b").ok();
    if personal
        .as_ref()
        .is_some_and(|regex| regex.is_match(trimmed))
    {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    let digits = trimmed
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    letters.len() >= 3 && digits <= 5
}

fn legal_supplier(text: &str) -> Option<String> {
    let legal = Regex::new(
        r"(?i)\b(?:S\.?A\.?S\.?|S\.?A\.?R\.?L\.?|S\.?N\.?C\.?|S\.?A\.?|GMBH|LTD|LIMITED|B\.?V\.?)\b",
    )
    .ok()?;
    for line in text.lines().rev().take(100) {
        let key = compact(line);
        if !(key.contains("capital")
            || key.contains("rcs")
            || key.contains("siret")
            || key.contains("tva"))
        {
            continue;
        }
        for segment in column_segments(line) {
            let Some(found) = legal.find(&segment) else {
                continue;
            };
            let prefix = segment[..found.start()]
                .trim()
                .trim_matches('-')
                .trim_matches('|')
                .trim();
            if supplier_segment_usable(prefix) {
                return Some(prefix.to_string());
            }
        }
    }
    None
}

fn supplier(text: &str) -> Option<String> {
    if let Some(value) = legal_supplier(text) {
        return Some(value);
    }
    let mut best: Option<(i32, String)> = None;
    for (line_index, line) in text.lines().take(40).enumerate() {
        for (column_index, segment) in column_segments(line).into_iter().enumerate() {
            if !supplier_segment_usable(&segment) {
                continue;
            }
            let letters = segment
                .chars()
                .filter(|character| character.is_alphabetic())
                .collect::<Vec<_>>();
            let uppercase = letters
                .iter()
                .filter(|character| character.is_uppercase())
                .count();
            let ratio = uppercase as f32 / letters.len().max(1) as f32;
            let key = compact(&segment);
            let mut score = 190 - line_index as i32 * 5 - column_index as i32 * 28;
            if ratio >= 0.8 {
                score += 30;
            } else if ratio >= 0.55 {
                score += 12;
            }
            if segment.len() <= 38 {
                score += 10;
            }
            if ["sarl", "sas", "snc", "gmbh", "ltd", "limited", "bv"]
                .iter()
                .any(|needle| key.contains(needle))
            {
                score += 25;
            }
            if best.as_ref().map(|(old, _)| score > *old).unwrap_or(true) {
                best = Some((score, segment));
            }
        }
    }
    best.filter(|(score, _)| *score >= 105)
        .map(|(_, value)| value.trim().to_string())
}

fn is_ht_label(key: &str) -> bool {
    [
        "totalht", "montantht", "totalhorstaxe", "totalhorstaxes", "soustotal", "subtotal",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn is_ttc_label(key: &str) -> bool {
    [
        "totalttc", "montantttc", "netapayer", "totalapayer", "montantapayer", "grandtotal",
        "amountdue", "balancedue", "invoicetotal", "totalgeneralttc",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn explicit_tax_label(key: &str) -> bool {
    [
        "totaltva", "montanttva", "totaltaxes", "taxetotale", "donttva", "totalvat",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn amounts(text: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let lines = text.lines().collect::<Vec<_>>();
    let mut ht = None;
    let mut vat = None;
    let mut ttc = None;
    let mut tax_rows = Vec::new();
    let mut seen_tax_rows = HashSet::new();

    for (index, line) in lines.iter().enumerate() {
        let key = compact(line);
        let values = money_values(line);

        if key.contains("totalfacture") || key.contains("totalfacturee") || key.contains("totalfacture") {
            if values.len() >= 3 {
                ht.get_or_insert(values[values.len() - 3]);
                vat.get_or_insert(values[values.len() - 2]);
                ttc.get_or_insert(values[values.len() - 1]);
            } else if values.len() == 1 && ttc.is_none() {
                ttc = values.last().copied();
            }
        }

        if ht.is_none() && is_ht_label(&key) {
            ht = values.last().copied();
        }
        if vat.is_none() && explicit_tax_label(&key) {
            vat = values.last().copied();
        }
        if ttc.is_none() && is_ttc_label(&key) {
            ttc = values.last().copied().or_else(|| {
                lines
                    .get(index + 1)
                    .and_then(|next| money_values(next).last().copied())
            });
        }
        if ttc.is_none() && (key.contains("totaldelafacture") || key.contains("totalfacture")) {
            ttc = values.last().copied();
        }

        let is_tax_row = ["tva", "vat", "tax", "mwst", "iva", "gst", "qst", "tps", "tvq"]
            .iter()
            .any(|needle| key.contains(needle));
        if is_tax_row
            && !explicit_tax_label(&key)
            && !key.contains("intracom")
            && !key.contains("numero")
            && !key.contains("ident")
            && !is_ttc_label(&key)
        {
            if let Some(value) = values.last().copied() {
                let row_key = format!("{}:{value:.3}", key);
                if seen_tax_rows.insert(row_key) {
                    tax_rows.push(value);
                }
            }
        }
    }

    if vat.is_none() && !tax_rows.is_empty() {
        vat = Some(tax_rows.iter().sum());
    }

    match (ht, vat, ttc) {
        (Some(h), None, Some(t)) if t + 0.02 >= h => vat = Some((t - h).max(0.0)),
        (None, Some(v), Some(t)) if t + 0.02 >= v => ht = Some((t - v).max(0.0)),
        (Some(h), Some(v), None) => ttc = Some(h + v),
        _ => {}
    }

    if let (Some(h), Some(v), Some(t)) = (ht, vat, ttc) {
        if (h + v - t).abs() > 0.02 {
            let derived_vat = t - h;
            if derived_vat >= -0.02 {
                vat = Some(derived_vat.max(0.0));
            } else {
                ttc = None;
            }
        }
    }
    (ht, vat, ttc)
}

fn extract_siret(text: &str) -> Option<String> {
    let regex = Regex::new(r"(?i)\bSIRET\b\s*[:#-]?\s*([0-9OIl .-]{14,24})").ok()?;
    let raw = regex.captures(text)?.get(1)?.as_str();
    let digits = raw
        .chars()
        .filter_map(|character| match character {
            '0'..='9' => Some(character),
            'O' | 'o' => Some('0'),
            'I' | 'l' => Some('1'),
            _ => None,
        })
        .collect::<String>();
    (digits.len() == 14).then_some(digits)
}

fn extract_iban(text: &str) -> Option<String> {
    let regex = Regex::new(
        r"(?i)\bIBAN\b\s*[:#-]?\s*([A-Z]{2}\s*[0-9OIl]{2}(?:[ -]*[A-Z0-9OIl]){10,34})",
    )
    .ok()?;
    let value = regex.captures(text)?.get(1)?.as_str();
    let cleaned = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    (15..=34).contains(&cleaned.len()).then_some(cleaned)
}

fn strong_fields(text: &str) -> StrongFields {
    let (amount_ht, amount_vat, amount_ttc) = amounts(text);
    StrongFields {
        supplier: supplier(text),
        invoice_number: invoice_number(text),
        invoice_date: invoice_date(text),
        amount_ht,
        amount_vat,
        amount_ttc,
        siret: extract_siret(text),
        iban: extract_iban(text),
    }
}

fn evidence_count(fields: &StrongFields) -> usize {
    usize::from(fields.supplier.is_some())
        + usize::from(fields.invoice_number.is_some())
        + usize::from(fields.invoice_date.is_some())
        + usize::from(fields.amount_ht.is_some() || fields.amount_ttc.is_some())
        + usize::from(fields.siret.is_some() || fields.iban.is_some())
}

fn looks_like_accounting_document(text: &str) -> bool {
    let body = compact(text);
    [
        "facture", "invoice", "ticket", "receipt", "totalttc", "netapayer", "totalapayer",
        "montantht", "totalht",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

fn build_normalized_text(text: &str, fields: &StrongFields) -> Option<String> {
    if !looks_like_accounting_document(text) || evidence_count(fields) < 2 {
        return None;
    }
    let mut output = String::new();
    if let Some(supplier) = &fields.supplier {
        output.push_str(supplier);
        output.push('\n');
    }
    output.push_str("FACTURE NORMALISEE TEXTE PDF\n");
    if let Some(number) = &fields.invoice_number {
        output.push_str(&format!("Facture N° : {number}\n"));
    }
    if let Some(date) = &fields.invoice_date {
        output.push_str(&format!("Date facture : {date}\n"));
    }
    if let Some(value) = fields.amount_ht {
        output.push_str(&format!("Total HT : {value:.2}\n"));
    }
    if let Some(value) = fields.amount_vat {
        output.push_str(&format!("TVA : {value:.2}\n"));
    }
    if let Some(value) = fields.amount_ttc {
        output.push_str(&format!("Total TTC : {value:.2}\n"));
    }
    if let Some(siret) = &fields.siret {
        output.push_str(&format!("SIRET : {siret}\n"));
    }
    if let Some(iban) = &fields.iban {
        output.push_str(&format!("IBAN : {iban}\n"));
    }
    output.push_str(MARKER);
    output.push('\n');
    output.push_str(ORIGINAL_MARKER);
    output.push('\n');
    output.push_str(text.trim());
    Some(output)
}

pub(crate) fn augment_text(text: &str) -> String {
    let base = original_text(text);
    let fields = strong_fields(base);
    build_normalized_text(base, &fields).unwrap_or_else(|| base.to_string())
}

#[tauri::command]
pub fn normalize_native_invoice_texts(app: AppHandle) -> Result<NativeNormalizationResult, String> {
    let connection = super::open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,COALESCE(extracted_text,'')
             FROM invoices
             WHERE status='nouvelle'
               AND extraction_status IN ('texte_extrait','ocr_termine')
             ORDER BY updated_at ASC
             LIMIT 50",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);

    let mut result = NativeNormalizationResult::default();
    for (path, text) in rows {
        result.inspected += 1;
        let normalized = augment_text(&text);
        if normalized == text {
            result.skipped += 1;
            continue;
        }
        let mut parsed = super::parse_invoice_text(&normalized);
        parsed.amounts_consistent = super::compute_amount_consistency(&parsed);
        let json = match serde_json::to_string(&parsed) {
            Ok(value) => value,
            Err(_) => {
                result.errors += 1;
                continue;
            }
        };
        let length = normalized
            .chars()
            .filter(|character| !character.is_whitespace())
            .count() as i64;
        match connection.execute(
            "UPDATE invoices SET extracted_text=?2,text_length=?3,parsed_json=?4,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, normalized, length, json],
        ) {
            Ok(_) => {
                result.normalized += 1;
                let _ = super::record_audit(
                    &connection,
                    Some(&path),
                    "document_text_normalized",
                    Some("corpus_rules_v2"),
                );
            }
            Err(_) => result.errors += 1,
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{augment_text, date_in_line, strong_fields, MARKER};

    #[test]
    fn reads_date_followed_by_time() {
        assert_eq!(date_in_line("31.08.2023 09:07").as_deref(), Some("31/08/2023"));
        assert_eq!(date_in_line("31 / 08 / 2023 - 09:07").as_deref(), Some("31/08/2023"));
    }

    #[test]
    fn reads_darty_style_invoice_without_ocr() {
        let text = "DARTY ILE DE FRANCE\nFACTURE France\nN°27583276 du 22/09/2022\nTotal facturé : 420,82 € 84,16 € 504,98 €\nTVA intracommunautaire FR55542086616";
        let fields = strong_fields(text);
        assert_eq!(fields.supplier.as_deref(), Some("DARTY ILE DE FRANCE"));
        assert_eq!(fields.invoice_number.as_deref(), Some("27583276"));
        assert_eq!(fields.invoice_date.as_deref(), Some("22/09/2022"));
        assert_eq!(fields.amount_ht, Some(420.82));
        assert_eq!(fields.amount_vat, Some(84.16));
        assert_eq!(fields.amount_ttc, Some(504.98));
    }

    #[test]
    fn reads_boulanger_style_header_and_totals() {
        let text = "FACTURE F905 EM10278-23/002 du 31.08.2023 09:07\nBOULANGER PLATEFORME PARIS NORD\nTOTAL HT (Euros) 181,67\nTOTAL TTC (Euros) 218,00\nDont TVA (20,00%) 36,33";
        let fields = strong_fields(text);
        assert_eq!(fields.invoice_number.as_deref(), Some("EM10278-23/002"));
        assert_eq!(fields.invoice_date.as_deref(), Some("31/08/2023"));
        assert_eq!(fields.amount_ht, Some(181.67));
        assert_eq!(fields.amount_vat, Some(36.33));
        assert_eq!(fields.amount_ttc, Some(218.0));
    }

    #[test]
    fn reads_workshop_invoice_with_standalone_reference() {
        let text = "DAVIS DREUX\nFACTURE ATELIER\n2026/303824\n08/04/2026\nMontant HT en € 1 077,75\nTVA 215,55\nMontant TTC en € 1 293,30";
        let fields = strong_fields(text);
        assert_eq!(fields.invoice_number.as_deref(), Some("2026/303824"));
        assert_eq!(fields.invoice_date.as_deref(), Some("08/04/2026"));
        assert_eq!(fields.amount_ht, Some(1077.75));
        assert_eq!(fields.amount_vat, Some(215.55));
        assert_eq!(fields.amount_ttc, Some(1293.30));
    }

    #[test]
    fn reads_fnac_style_total_on_next_line() {
        let text = "FACTURE/INVOICE : 657056382\nDu : 15/12/15\nTotal HT 229,51 €\nTotal Taxes 45,90 €\nTOTAL GENERAL TTC\nEUR 275,41€\nFnac Direct - SA au capital de 13000000 EUR RCS Nanterre";
        let fields = strong_fields(text);
        assert_eq!(fields.supplier.as_deref(), Some("Fnac Direct"));
        assert_eq!(fields.invoice_number.as_deref(), Some("657056382"));
        assert_eq!(fields.invoice_date.as_deref(), Some("15/12/2015"));
        assert_eq!(fields.amount_ht, Some(229.51));
        assert_eq!(fields.amount_vat, Some(45.90));
        assert_eq!(fields.amount_ttc, Some(275.41));
    }

    #[test]
    fn sums_multiple_vat_rows_when_total_tax_is_absent() {
        let text = "MAGASIN TEST\nFACTURE N° A-4256\nDate 20/08/2026\nTotal HT 100,00\nTVA 5,5% 5,50\nTVA 20% 9,00\nTotal TTC 114,50";
        let fields = strong_fields(text);
        assert_eq!(fields.amount_vat, Some(14.5));
        assert_eq!(fields.amount_ttc, Some(114.5));
    }

    #[test]
    fn normalizer_can_rebuild_after_a_new_ocr_pass() {
        let first = augment_text("DARTY ILE DE FRANCE\nFACTURE N° 27583276\nDate 22/09/2022\nTotal TTC 504,98");
        let merged = format!("{first}\nTotal HT 420,82\nTVA 84,16");
        let second = augment_text(&merged);
        assert!(second.contains("Total HT : 420.82"));
        assert!(second.contains("TVA : 84.16"));
        assert_eq!(second.matches(MARKER).count(), 1);
    }

    #[test]
    fn does_not_promote_guarantee_without_invoice_structure() {
        let text = "CERTIFICAT DE GARANTIE\nDescription du produit\nDurée de garantie 5 ans";
        let fields = strong_fields(text);
        assert!(fields.invoice_number.is_none());
        assert!(fields.amount_ttc.is_none());
        assert_eq!(augment_text(text), text);
    }
}