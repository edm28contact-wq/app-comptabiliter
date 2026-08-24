use regex::Regex;
use rusqlite::params;
use serde::Serialize;
use tauri::AppHandle;

const MARKER: &str = "--- CHAMPS NORMALISES TEXTE PDF ---";

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

fn parse_money(value: &str) -> Option<f64> {
    let mut raw = value
        .trim()
        .replace('€', "")
        .replace('$', "")
        .replace('£', "")
        .replace("EUR", "")
        .replace("eur", "")
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
    let compact_spaces = line.split_whitespace().collect::<String>();
    let iso = Regex::new(r"\b(19\d{2}|20\d{2})[./-](\d{1,2})[./-](\d{1,2})\b").ok()?;
    if let Some(captures) = iso.captures(&compact_spaces) {
        return normalize_date(
            captures.get(3)?.as_str().parse().ok()?,
            captures.get(2)?.as_str().parse().ok()?,
            captures.get(1)?.as_str().parse().ok()?,
        );
    }
    let french = Regex::new(r"\b(\d{1,2})[./-](\d{1,2})[./-](\d{2,4})\b").ok()?;
    let captures = french.captures(&compact_spaces)?;
    normalize_date(
        captures.get(1)?.as_str().parse().ok()?,
        captures.get(2)?.as_str().parse().ok()?,
        captures.get(3)?.as_str().parse().ok()?,
    )
}

fn invoice_date(text: &str) -> Option<String> {
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().take(100).enumerate() {
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
        for next in lines.iter().skip(index + 1).take(3) {
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
        .trim()
        .to_string();
    if cleaned.chars().all(|character| character.is_ascii_digit() || character == ' ') {
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
    for (index, line) in lines.iter().take(70).enumerate() {
        let key = compact(line);
        if key.contains("facture") || key.contains("invoice") {
            for next in lines.iter().skip(index + 1).take(4) {
                if let Some(value) = standalone_number
                    .captures(next)
                    .and_then(|captures| captures.get(1))
                    .and_then(|value| clean_invoice_number(value.as_str()))
                {
                    return Some(value);
                }
                if let Some(value) = standalone_year_number
                    .captures(next)
                    .and_then(|captures| captures.get(1))
                    .and_then(|value| clean_invoice_number(value.as_str()))
                {
                    return Some(value);
                }
            }
        }
        if key.contains("numerodefacture") {
            for next in lines.iter().skip(index + 1).take(3) {
                let Ok(regex) = Regex::new(r"(?i)#?([A-Z]{0,4}\d[A-Z0-9/_-]{3,})") else {
                    continue;
                };
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
    None
}

fn column_segments(line: &str) -> Vec<String> {
    let Ok(regex) = Regex::new(r"\s{4,}") else {
        return vec![line.trim().to_string()];
    };
    regex
        .split(line)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn supplier_segment_usable(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < 4 || trimmed.len() > 70 || trimmed.contains('@') {
        return false;
    }
    let key = compact(trimmed);
    let blocked = [
        "facture",
        "invoice",
        "commande",
        "client",
        "livraison",
        "adresse",
        "page",
        "date",
        "siret",
        "siren",
        "iban",
        "tva",
        "total",
        "montant",
        "reglement",
        "reference",
        "designation",
        "quantite",
        "telephone",
        "servicecomptabilite",
        "reparateuragrees",
        "vehiculesutilitaires",
        "france",
        "echeance",
        "garantie",
        "certificat",
        "codeclient",
        "numeroclient",
        "avenue",
        "rue",
        "boulevard",
        "route",
        "cedex",
        "zac",
    ];
    if blocked.iter().any(|needle| key.contains(needle)) {
        return false;
    }
    let personal = Regex::new(r"(?i)\b(?:M|MME|MR|MRS|MONSIEUR|MADAME)\b").ok();
    if personal
        .as_ref()
        .is_some_and(|regex| regex.is_match(trimmed))
    {
        return false;
    }
    if Regex::new(r"\b\d{5}\b")
        .ok()
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
    if letters.len() < 3 || digits > 4 {
        return false;
    }
    let uppercase = letters
        .iter()
        .filter(|character| character.is_uppercase())
        .count();
    let ratio = uppercase as f32 / letters.len() as f32;
    ratio >= 0.55
        || ["sarl", "sas", "snc", "gmbh", "ltd", "limited", "bv"]
            .iter()
            .any(|needle| key.contains(needle))
}

fn legal_supplier(text: &str) -> Option<String> {
    let legal = Regex::new(r"(?i)\b(?:S\.?A\.?S\.?|S\.?A\.?R\.?L\.?|S\.?N\.?C\.?|S\.?A\.?|GMBH|LTD|LIMITED|B\.?V\.?)\b").ok()?;
    for line in text.lines().rev().take(80) {
        let key = compact(line);
        if !(key.contains("capital") || key.contains("rcs") || key.contains("siret") || key.contains("tva")) {
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
    for (line_index, line) in text.lines().take(35).enumerate() {
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
            let mut score = 180 - line_index as i32 * 5 - column_index as i32 * 25;
            if ratio >= 0.8 {
                score += 25;
            }
            if segment.len() <= 35 {
                score += 10;
            }
            if best.as_ref().map(|(old, _)| score > *old).unwrap_or(true) {
                best = Some((score, segment));
            }
        }
    }
    best.filter(|(score, _)| *score >= 100).map(|(_, value)| value)
}

fn amount_after_regex(line: &str, pattern: &str) -> Option<f64> {
    let regex = Regex::new(pattern).ok()?;
    let captures = regex.captures(line)?;
    parse_money(captures.get(1)?.as_str())
}

fn first_strong_amount(text: &str, patterns: &[&str]) -> Option<f64> {
    for line in text.lines() {
        for pattern in patterns {
            if let Some(value) = amount_after_regex(line, pattern) {
                return Some(value);
            }
        }
    }
    None
}

fn amounts(text: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let lines = text.lines().collect::<Vec<_>>();
    let mut ht = first_strong_amount(
        text,
        &[
            r"(?i)(?:TOTAL|MONTANT)\s*H\.?\s*T\.?\s*(?:\([^)]*\))?\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
            r"(?i)TOTAL\s+HORS\s+TAXE[S]?\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
            r"(?i)TOTAL\s*\(\s*HT\s*\)\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
        ],
    );
    let mut vat = first_strong_amount(
        text,
        &[
            r"(?i)(?:TOTAL\s+TAXES?|TAXE\s+TOTALE|TOTAL\s+TVA|MONTANT\s+TVA)\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
            r"(?i)TVA\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})\s*(?:€|EUR)?\s*$",
        ],
    );
    let mut ttc = first_strong_amount(
        text,
        &[
            r"(?i)(?:TOTAL|MONTANT)\s*T\.?\s*T\.?\s*C\.?\s*(?:\([^)]*\))?\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
            r"(?i)(?:NET|TOTAL|MONTANT)\s+[ÀA]\s+PAYER\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
            r"(?i)INVOICE\s+(?:TOTAL|AMOUNT)\s*[:=]?\s*([0-9][0-9 .\u{00a0}\u{202f}]*[,.][0-9]{2,3})",
        ],
    );

    for (index, line) in lines.iter().enumerate() {
        let key = compact(line);
        let values = money_values(line);
        if key.contains("totalfacture") && values.len() >= 3 {
            ht.get_or_insert(values[values.len() - 3]);
            vat.get_or_insert(values[values.len() - 2]);
            ttc.get_or_insert(values[values.len() - 1]);
        }
        if (key.contains("totalgeneralttc") || key == "totalgeneralttc") && ttc.is_none() {
            if let Some(value) = values.last().copied() {
                ttc = Some(value);
            } else if let Some(next) = lines.get(index + 1).and_then(|value| money_values(value).last().copied()) {
                ttc = Some(next);
            }
        }
        if vat.is_none() && (key.contains("donttva") || key.contains("taxetotale") || key.contains("totaltaxes")) {
            if let Some(value) = values.last().copied() {
                vat = Some(value);
            }
        }
    }

    match (ht, vat, ttc) {
        (Some(h), None, Some(t)) if t + 0.02 >= h => vat = Some((t - h).max(0.0)),
        (None, Some(v), Some(t)) if t + 0.02 >= v => ht = Some((t - v).max(0.0)),
        (Some(h), Some(v), None) => ttc = Some(h + v),
        _ => {}
    }

    if let (Some(h), Some(v), Some(t)) = (ht, vat, ttc) {
        if (h + v - t).abs() > 0.02 {
            return (ht, vat, None);
        }
    }
    (ht, vat, ttc)
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
    }
}

fn build_normalized_text(text: &str, fields: &StrongFields) -> Option<String> {
    let supplier = fields.supplier.as_deref()?;
    let mut output = String::new();
    output.push_str(supplier);
    output.push('\n');
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
        output.push_str(&format!("Total TVA : {value:.2}\n"));
    }
    if let Some(value) = fields.amount_ttc {
        output.push_str(&format!("Total TTC : {value:.2}\n"));
    }
    output.push_str(MARKER);
    output.push_str("\n--- TEXTE PDF ORIGINAL ---\n");
    output.push_str(text.trim());
    Some(output)
}

#[tauri::command]
pub fn normalize_native_invoice_texts(app: AppHandle) -> Result<NativeNormalizationResult, String> {
    let connection = super::open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT path,COALESCE(extracted_text,'')
             FROM invoices
             WHERE status='nouvelle'
               AND extraction_status='texte_extrait'
               AND COALESCE(extracted_text,'') NOT LIKE '%CHAMPS NORMALISES TEXTE PDF%'
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
        let fields = strong_fields(&text);
        let Some(normalized) = build_normalized_text(&text, &fields) else {
            result.skipped += 1;
            continue;
        };
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
                    "native_text_normalized",
                    Some("corpus_rules_v1"),
                );
            }
            Err(_) => result.errors += 1,
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::strong_fields;

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
    fn reads_spaced_facture_label() {
        let text = "GARAGE TEST\nF A C T U R E N° 16669\nDate : 31/10/2019\nTotal H.T 100,00\nTVA 20,00\nNET A PAYER 120,00";
        let fields = strong_fields(text);
        assert_eq!(fields.invoice_number.as_deref(), Some("16669"));
        assert_eq!(fields.invoice_date.as_deref(), Some("31/10/2019"));
        assert_eq!(fields.amount_ttc, Some(120.0));
    }

    #[test]
    fn does_not_promote_guarantee_without_supplier_invoice_structure() {
        let text = "CERTIFICAT DE GARANTIE\nDescription du produit\nDurée de garantie 5 ans";
        let fields = strong_fields(text);
        assert!(fields.supplier.is_none());
        assert!(fields.invoice_number.is_none());
        assert!(fields.amount_ttc.is_none());
    }
}
