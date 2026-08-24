use regex::Regex;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::AppHandle;

#[derive(Default, Debug, Clone, Serialize)]
struct ReceiptHints {
    merchant: Option<String>,
    document_number: Option<String>,
    date: Option<String>,
    amount_ht: Option<f64>,
    amount_vat: Option<f64>,
    amount_ttc: Option<f64>,
    siret: Option<String>,
    is_receipt: bool,
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
        .collect()
}

fn compact(value: &str) -> String {
    normalize(value)
        .chars()
        .map(|character| match character {
            '0' => 'o',
            '1' => 'l',
            '5' => 's',
            other => other,
        })
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
        .replace("CAD", "")
        .replace("USD", "")
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
        _ => raw,
    };
    raw.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn decimal_amounts(line: &str) -> Vec<f64> {
    let regex = match Regex::new(
        r"(?i)(?:\d{1,3}(?:[ .\u{00a0}\u{202f}]\d{3})+|\d+)(?:[,.]\d{2,3})",
    ) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
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

fn last_amount(line: &str) -> Option<f64> {
    decimal_amounts(line).last().copied()
}

fn parse_date(value: &str) -> Option<String> {
    let iso = Regex::new(r"\b(19\d{2}|20\d{2})[/.\-](\d{1,2})[/.\-](\d{1,2})\b").ok()?;
    if let Some(captures) = iso.captures(value) {
        let year = captures.get(1)?.as_str().parse::<u32>().ok()?;
        let month = captures.get(2)?.as_str().parse::<u32>().ok()?;
        let day = captures.get(3)?.as_str().parse::<u32>().ok()?;
        if (1..=12).contains(&month) && (1..=31).contains(&day) {
            return Some(format!("{day:02}/{month:02}/{year:04}"));
        }
    }
    let french = Regex::new(r"\b(\d{1,2})[/.\-](\d{1,2})[/.\-](\d{2,4})\b").ok()?;
    let captures = french.captures(value)?;
    let day = captures.get(1)?.as_str().parse::<u32>().ok()?;
    let month = captures.get(2)?.as_str().parse::<u32>().ok()?;
    let mut year = captures.get(3)?.as_str().parse::<u32>().ok()?;
    if captures.get(3)?.as_str().len() == 2 {
        year += 2000;
    }
    if (1..=12).contains(&month) && (1..=31).contains(&day) && (1900..=2100).contains(&year) {
        Some(format!("{day:02}/{month:02}/{year:04}"))
    } else {
        None
    }
}

fn receipt_signal(text: &str) -> bool {
    let body = compact(text);
    [
        "ticket",
        "receipt",
        "recu",
        "caisse",
        "caissier",
        "cartebancaire",
        "paiementcb",
        "terminal",
        "rendumonnaie",
        "mercidevotrevisite",
        "mercidevotreachat",
        "totalapayer",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

pub fn is_receipt_like(text: &str) -> bool {
    receipt_signal(text)
}

fn merchant_line_is_usable(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 2 || trimmed.len() > 55 || trimmed.contains('@') {
        return false;
    }
    let value = normalize(trimmed);
    let blocked = [
        "ticket", "receipt", "recu", "facture", "date", "heure", "caisse", "caissier",
        "siret", "siren", "tva", "total", "sous-total", "subtotal", "montant", "paiement",
        "carte", "especes", "rendu", "monnaie", "merci", "tel", "telephone", "www",
        "http", "adresse",
    ];
    if blocked.iter().any(|needle| value.contains(needle)) {
        return false;
    }
    let letters = trimmed.chars().filter(|character| character.is_alphabetic()).count();
    let digits = trimmed.chars().filter(|character| character.is_ascii_digit()).count();
    letters >= 2
        && digits <= 8
        && !trimmed.contains('€')
        && !trimmed.contains('$')
        && !trimmed.contains('£')
}

fn infer_merchant(text: &str) -> Option<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(18)
        .collect::<Vec<_>>();
    let mut best: Option<(&str, i32)> = None;
    for (index, line) in lines.iter().enumerate() {
        if !merchant_line_is_usable(line) {
            continue;
        }
        let letters = line
            .chars()
            .filter(|character| character.is_alphabetic())
            .collect::<Vec<_>>();
        let uppercase_ratio = if letters.is_empty() {
            0.0
        } else {
            letters.iter().filter(|character| character.is_uppercase()).count() as f32
                / letters.len() as f32
        };
        let mut score = 100 - index as i32 * 6;
        if uppercase_ratio >= 0.65 {
            score += 20;
        }
        if line.len() <= 30 {
            score += 10;
        }
        if line.contains(':') {
            score -= 25;
        }
        if best.map(|(_, old)| score > old).unwrap_or(true) {
            best = Some((line, score));
        }
    }
    best.filter(|(_, score)| *score >= 45)
        .map(|(line, _)| line.to_string())
}

fn extract_document_number(text: &str) -> Option<String> {
    let patterns = [
        r"(?i)(?:n[°oº]?\s*)?(?:ticket|receipt|reçu|recu)\s*(?:n[°oº]?|no\.?|nr\.?|number|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)(?:transaction|transac(?:tion)?|trx)\s*(?:n[°oº]?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{3,})",
        r"(?i)(?:facture|invoice)\s*(?:n[°oº]?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{3,})",
    ];
    for pattern in patterns {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        if let Some(value) = regex
            .captures(text)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().trim().to_string())
        {
            if (3..=40).contains(&value.len()) {
                return Some(value);
            }
        }
    }
    None
}

fn extract_receipt_date(text: &str) -> Option<String> {
    let lines = text.lines().take(80).collect::<Vec<_>>();
    for line in &lines {
        let key = compact(line);
        if key.contains("date") || key.contains("ticket") || key.contains("receipt") {
            if let Some(date) = parse_date(line) {
                return Some(date);
            }
        }
    }
    lines.into_iter().find_map(parse_date)
}

fn is_ht_line(line: &str) -> bool {
    let key = compact(line);
    [
        "totalht",
        "montantht",
        "totalhorstaxe",
        "sousto",
        "soustotal",
        "subtotal",
        "netht",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn is_explicit_total_line(line: &str) -> bool {
    let key = compact(line);
    [
        "totalttc",
        "netapayer",
        "totalapayer",
        "montantapayer",
        "grandtotal",
        "amountdue",
        "balancedue",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn payment_or_noise_line(line: &str) -> bool {
    if is_explicit_total_line(line) {
        return false;
    }
    let key = compact(line);
    [
        "rendu",
        "monnaie",
        "change",
        "remise",
        "discount",
        "economise",
        "economie",
        "carte",
        "cb",
        "visa",
        "mastercard",
        "amex",
        "especes",
        "cash",
        "montantpaye",
        "paye",
        "payment",
        "acompte",
        "avoir",
        "soldeprecedent",
        "totalarticles",
        "nombrearticles",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn total_score(line: &str, index: usize, total_lines: usize) -> Option<i32> {
    let key = compact(line);
    if is_ht_line(line) || payment_or_noise_line(line) {
        return None;
    }
    if key.contains("tva") || key.contains("vat") || key.contains("tax") {
        return None;
    }
    let mut score = if key.contains("totalttc") || key.contains("netapayer") {
        140
    } else if key.contains("totalapayer") || key.contains("montantapayer") {
        135
    } else if key.contains("grandtotal") || key.contains("amountdue") || key.contains("balancedue") {
        125
    } else if key.starts_with("total") || key.ends_with("total") || key.contains("total") {
        90
    } else {
        return None;
    };
    if total_lines > 0 {
        score += ((index * 25) / total_lines) as i32;
    }
    if line.contains('€') || line.contains('$') || line.contains('£') {
        score += 8;
    }
    Some(score)
}

fn extract_ttc(text: &str) -> Option<f64> {
    let lines = text.lines().collect::<Vec<_>>();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            Some((total_score(line, index, lines.len())?, last_amount(line)?))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, amount)| amount)
}

fn extract_ht(text: &str) -> Option<f64> {
    text.lines()
        .filter(|line| is_ht_line(line))
        .filter_map(last_amount)
        .last()
}

fn extract_tax(text: &str) -> Option<f64> {
    let explicit_total = text.lines().find_map(|line| {
        let key = compact(line);
        if key.contains("totaltva") || key.contains("totaltax") || key.contains("totalvat") {
            last_amount(line)
        } else {
            None
        }
    });
    if explicit_total.is_some() {
        return explicit_total;
    }

    let mut tax_buckets = std::collections::HashMap::<String, f64>::new();
    for line in text.lines() {
        let key = compact(line);
        let bucket = ["tva", "vat", "gst", "qst", "hst", "tps", "tvq", "mwst", "iva", "btw"]
            .iter()
            .find(|needle| key.contains(**needle))
            .map(|value| (*value).to_string());
        let Some(bucket) = bucket else {
            continue;
        };
        if key.contains("numero") || key.contains("ident") || is_explicit_total_line(line) {
            continue;
        }
        if let Some(amount) = last_amount(line) {
            tax_buckets
                .entry(bucket)
                .and_modify(|existing| {
                    if amount.abs() > existing.abs() {
                        *existing = amount;
                    }
                })
                .or_insert(amount);
        }
    }
    if tax_buckets.is_empty() {
        None
    } else {
        let sum = tax_buckets.values().sum::<f64>();
        (sum.abs() > 0.0001).then_some(sum)
    }
}

fn extract_siret(text: &str) -> Option<String> {
    let regex = Regex::new(r"(?i)\bSIRET\b\s*[:\-]?\s*([0-9OIl\s.-]{14,24})").ok()?;
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

fn analyze(text: &str) -> ReceiptHints {
    let is_receipt = receipt_signal(text);
    if !is_receipt {
        return ReceiptHints::default();
    }
    let amount_ttc = extract_ttc(text);
    let amount_vat = extract_tax(text);
    let amount_ht = extract_ht(text).or_else(|| match (amount_ttc, amount_vat) {
        (Some(ttc), Some(vat)) if ttc + 0.001 >= vat => Some(ttc - vat),
        _ => None,
    });
    ReceiptHints {
        merchant: infer_merchant(text),
        document_number: extract_document_number(text),
        date: extract_receipt_date(text),
        amount_ht,
        amount_vat,
        amount_ttc,
        siret: extract_siret(text),
        is_receipt,
    }
}

pub fn augment_if_receipt(text: &str) -> String {
    let hints = analyze(text);
    if !hints.is_receipt {
        return text.to_string();
    }
    if text.contains("--- TICKET DE CAISSE NORMALISE ---") {
        return text.to_string();
    }
    let mut result = String::new();
    if let Some(merchant) = &hints.merchant {
        result.push_str(merchant);
        result.push('\n');
    }
    result.push_str(text.trim());
    result.push_str("\n\n--- TICKET DE CAISSE NORMALISE ---\n");
    if let Some(number) = &hints.document_number {
        result.push_str(&format!("Facture N° : {number}\n"));
    }
    if let Some(date) = &hints.date {
        result.push_str(&format!("Date facture : {date}\n"));
    }
    if let Some(value) = hints.amount_ht {
        result.push_str(&format!("Total HT : {value:.2}\n"));
    }
    if let Some(value) = hints.amount_vat {
        result.push_str(&format!("TVA : {value:.2}\n"));
    }
    if let Some(value) = hints.amount_ttc {
        result.push_str(&format!("Total TTC : {value:.2}\n"));
    }
    if let Some(siret) = &hints.siret {
        result.push_str(&format!("SIRET : {siret}\n"));
    }
    result
}

#[tauri::command]
pub fn enhance_invoice_receipt(app: AppHandle, path: String) -> Result<bool, String> {
    let connection = super::open_database(&app)?;
    let text: Option<String> = connection
        .query_row(
            "SELECT extracted_text FROM invoices WHERE path=?1",
            params![path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    let Some(text) = text else {
        return Ok(false);
    };
    if !is_receipt_like(&text) {
        return Ok(false);
    }
    let augmented = augment_if_receipt(&text);
    let parsed = super::parse_invoice_text(&augmented);
    let json = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
    let length = augmented
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as i64;
    connection
        .execute(
            "UPDATE invoices SET extracted_text=?2,text_length=?3,parsed_json=?4,updated_at=CURRENT_TIMESTAMP WHERE path=?1",
            params![path, augmented, length, json],
        )
        .map_err(|error| error.to_string())?;
    let merchant = analyze(&text).merchant;
    let _ = super::record_audit(
        &connection,
        Some(&path),
        "receipt_enhanced",
        merchant.as_deref(),
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{analyze, augment_if_receipt};

    #[test]
    fn reads_french_supermarket_receipt() {
        let text = "CARREFOUR MARKET\nTicket N° 004512\n23/08/2026 14:32\n3 ARTICLES\nSOUS-TOTAL 25,00 EUR\nTVA 5,5% 0,55 EUR\nTVA 20% 2,40 EUR\nTOTAL TTC 27,95 EUR\nCARTE BANCAIRE 27,95 EUR\nMERCI DE VOTRE VISITE";
        let hints = analyze(text);
        assert!(hints.is_receipt);
        assert_eq!(hints.merchant.as_deref(), Some("CARREFOUR MARKET"));
        assert_eq!(hints.document_number.as_deref(), Some("004512"));
        assert_eq!(hints.date.as_deref(), Some("23/08/2026"));
        assert_eq!(hints.amount_ht, Some(25.0));
        assert_eq!(hints.amount_vat, Some(2.95));
        assert_eq!(hints.amount_ttc, Some(27.95));
    }

    #[test]
    fn ignores_card_payment_and_change_as_total() {
        let text = "BOULANGERIE TEST\nREÇU # A22445\n23-08-2026\nTOTAL 12,80 €\nESPECES 20,00 €\nRENDU MONNAIE 7,20 €\nMERCI DE VOTRE ACHAT";
        let hints = analyze(text);
        assert_eq!(hints.amount_ttc, Some(12.8));
        assert_eq!(hints.document_number.as_deref(), Some("A22445"));
    }

    #[test]
    fn derives_ht_when_only_tax_and_ttc_are_printed() {
        let text = "RESTAURANT TEST\nTICKET 99881\n23/08/2026\nTVA 10% 2,00 €\nTOTAL A PAYER 22,00 €\nPAIEMENT CB 22,00 €";
        let hints = analyze(text);
        assert_eq!(hints.amount_vat, Some(2.0));
        assert_eq!(hints.amount_ttc, Some(22.0));
        assert_eq!(hints.amount_ht, Some(20.0));
        let augmented = augment_if_receipt(text);
        assert!(augmented.contains("Total HT : 20.00"));
        assert!(augmented.contains("Total TTC : 22.00"));
    }

    #[test]
    fn does_not_duplicate_normalized_block() {
        let text = "MAGASIN TEST\nTICKET A-42\n23/08/2026\nTOTAL TTC 12,00 €";
        let once = augment_if_receipt(text);
        let twice = augment_if_receipt(&once);
        assert_eq!(twice.matches("--- TICKET DE CAISSE NORMALISE ---").count(), 1);
    }
}
