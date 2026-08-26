use regex::Regex;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process,
    sync::{mpsc, Mutex, OnceLock},
    thread,
    time::Duration,
};
use windows::{
    core::HSTRING,
    Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions},
    Foundation::Rect,
    Graphics::Imaging::BitmapDecoder,
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

const OCR_RENDER_LONG_EDGE_BASE: f32 = 1500.0;
const OCR_RENDER_LONG_EDGE_DETAIL: f32 = 2400.0;
const OCR_RENDER_LONG_EDGE_ZONE: f32 = 2600.0;
const OCR_TIMEOUT_SECONDS: u64 = 90;
static ACTIVE_OCR: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

struct WinRtGuard;

impl WinRtGuard {
    fn initialize() -> Result<Self, String> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)
                .map_err(|error| format!("Initialisation OCR Windows impossible : {error}"))?;
        }
        Ok(Self)
    }
}

impl Drop for WinRtGuard {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

struct ActiveOcrGuard {
    source: String,
}

impl ActiveOcrGuard {
    fn acquire(source: &str) -> Result<Self, String> {
        let active = ACTIVE_OCR.get_or_init(|| Mutex::new(HashSet::new()));
        let mut guard = active
            .lock()
            .map_err(|_| "Le verrou OCR Windows est indisponible.".to_string())?;
        if !guard.insert(source.to_string()) {
            return Err(
                "L'OCR de ce document est déjà en cours. Attendez sa fin avant de réessayer."
                    .to_string(),
            );
        }
        Ok(Self {
            source: source.to_string(),
        })
    }
}

impl Drop for ActiveOcrGuard {
    fn drop(&mut self) {
        if let Some(active) = ACTIVE_OCR.get() {
            if let Ok(mut guard) = active.lock() {
                guard.remove(&self.source);
            }
        }
    }
}

fn temporary_pdf_path(source: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    std::env::temp_dir().join(format!(
        "app-comptabiliter-ocr-{}-{:x}.pdf",
        process::id(),
        hasher.finish()
    ))
}

fn render_dimensions(
    width: f32,
    height: f32,
    max_dimension: u32,
    requested_long_edge: f32,
) -> (u32, u32) {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let target_long_edge = requested_long_edge
        .min(max_dimension as f32)
        .max(1.0);
    let scale = target_long_edge / width.max(height);
    (
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
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

fn normalize_label(value: &str) -> String {
    normalize_word(value)
        .chars()
        .map(|character| match character {
            '0' => 'o',
            '1' => 'l',
            '5' => 's',
            other => other,
        })
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_label(value: &str) -> String {
    normalize_label(value)
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
        .replace("CAD", "")
        .replace("USD", "")
        .replace("GBP", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if raw.is_empty() {
        return None;
    }
    let negative_parentheses = raw.starts_with('(') && raw.ends_with(')');
    if negative_parentheses {
        raw = raw
            .trim_start_matches('(')
            .trim_end_matches(')')
            .to_string();
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
    let value = raw.parse::<f64>().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(if negative_parentheses { -value } else { value })
}

fn format_money(value: f64) -> String {
    format!("{value:.2}")
}

fn textual_month(value: &str) -> Option<u32> {
    let month = normalize_word(value)
        .trim_matches(|character: char| !character.is_alphabetic())
        .to_string();
    match month.as_str() {
        "jan" | "janv" | "janvier" | "january" | "enero" | "januar" | "gennaio" => Some(1),
        "feb" | "fev" | "fevr" | "fevrier" | "february" | "febrero" | "februar" | "febbraio" => Some(2),
        "mar" | "mars" | "march" | "marzo" | "marz" => Some(3),
        "apr" | "avr" | "avril" | "april" | "abril" | "aprile" => Some(4),
        "may" | "mai" | "mayo" | "maggio" => Some(5),
        "jun" | "juin" | "june" | "junio" | "juni" | "giugno" => Some(6),
        "jul" | "juil" | "juillet" | "july" | "julio" | "juli" | "luglio" => Some(7),
        "aug" | "aou" | "aout" | "august" | "agosto" => Some(8),
        "sep" | "sept" | "septembre" | "september" | "septiembre" | "settembre" => Some(9),
        "oct" | "octobre" | "october" | "octubre" | "oktober" | "ottobre" => Some(10),
        "nov" | "novembre" | "november" | "noviembre" => Some(11),
        "dec" | "decembre" | "december" | "diciembre" | "dezember" | "dicembre" => Some(12),
        _ => None,
    }
}

fn normalized_date(day: u32, month: u32, year: u32) -> Option<String> {
    if (1..=31).contains(&day)
        && (1..=12).contains(&month)
        && (1900..=2100).contains(&year)
    {
        Some(format!("{day:02}/{month:02}/{year:04}"))
    } else {
        None
    }
}

fn parse_date_candidate(value: &str) -> Option<String> {
    let iso = Regex::new(r"(?i)\b(19\d{2}|20\d{2})[/.\-](\d{1,2})[/.\-](\d{1,2})\b").ok()?;
    if let Some(captures) = iso.captures(value) {
        return normalized_date(
            captures.get(3)?.as_str().parse().ok()?,
            captures.get(2)?.as_str().parse().ok()?,
            captures.get(1)?.as_str().parse().ok()?,
        );
    }
    let numeric = Regex::new(r"(?i)\b(\d{1,2})[/.\-](\d{1,2})[/.\-](\d{2,4})\b").ok()?;
    if let Some(captures) = numeric.captures(value) {
        let mut year = captures.get(3)?.as_str().parse::<u32>().ok()?;
        if captures.get(3)?.as_str().len() == 2 {
            year += 2000;
        }
        return normalized_date(
            captures.get(1)?.as_str().parse().ok()?,
            captures.get(2)?.as_str().parse().ok()?,
            year,
        );
    }
    let day_month = Regex::new(
        r"(?i)\b(\d{1,2})\s+([A-Za-zÀ-ÿ.]+)[,\s]+(19\d{2}|20\d{2})\b",
    )
    .ok()?;
    if let Some(captures) = day_month.captures(value) {
        return normalized_date(
            captures.get(1)?.as_str().parse().ok()?,
            textual_month(captures.get(2)?.as_str())?,
            captures.get(3)?.as_str().parse().ok()?,
        );
    }
    let month_day = Regex::new(
        r"(?i)\b([A-Za-zÀ-ÿ.]+)\s+(\d{1,2})[,]?\s+(19\d{2}|20\d{2})\b",
    )
    .ok()?;
    let captures = month_day.captures(value)?;
    normalized_date(
        captures.get(2)?.as_str().parse().ok()?,
        textual_month(captures.get(1)?.as_str())?,
        captures.get(3)?.as_str().parse().ok()?,
    )
}

fn is_invoice_label(line: &str) -> bool {
    let compact = compact_label(line);
    [
        "facture", "invoice", "factura", "rechnung", "fattura", "factuur", "ticket", "receipt",
        "recu",
    ]
    .iter()
    .any(|label| compact.contains(label))
}

fn is_receipt_label(line: &str) -> bool {
    let compact = compact_label(line);
    [
        "ticket", "receipt", "recu", "caisse", "caissier", "cartebancaire", "paiementcb",
        "rendumonnaie",
    ]
    .iter()
    .any(|label| compact.contains(label))
}

fn is_date_label(line: &str) -> bool {
    let compact = compact_label(line);
    [
        "datedelafacture",
        "datefacture",
        "invoicedate",
        "rechnungsdatum",
        "fechafactura",
        "datafattura",
        "factuurdatum",
    ]
    .iter()
    .any(|label| compact.contains(label))
        || compact.starts_with("date")
}

fn extract_invoice_date_hint(text: &str) -> Option<String> {
    for line in text.lines().take(100) {
        if is_date_label(line) || is_invoice_label(line) {
            if let Some(date) = parse_date_candidate(line) {
                return Some(date);
            }
        }
    }
    text.lines().take(80).find_map(parse_date_candidate)
}

fn invoice_number_candidate(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && !"-_/ .".contains(character)
    });
    if trimmed.len() < 3 || trimmed.len() > 40 {
        return false;
    }
    let digits = trimmed
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    let letters = trimmed
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .count();
    digits >= 2
        && (letters >= 1 || trimmed.contains('-') || trimmed.contains('/') || trimmed.contains('_'))
        && parse_date_candidate(trimmed).is_none()
}

fn extract_invoice_number_hint(text: &str) -> Option<String> {
    let patterns = [
        r"(?i)(?:facture|invoice|factura|rechnung|fattura|factuur)\s*(?:n[°oº]?|num(?:e|é)ro|number|nr\.?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)(?:ticket|receipt|reçu|recu)\s*(?:n[°oº]?|number|nr\.?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
        r"(?i)(?:transaction|trx)\s*(?:n[°oº]?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
    ];
    for pattern in patterns {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        if let Some(value) = regex
            .captures(text)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().trim().to_string())
            .filter(|value| invoice_number_candidate(value))
        {
            return Some(value);
        }
    }
    None
}

fn extract_decimal_amounts(line: &str) -> Vec<f64> {
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

fn last_money_amount(line: &str) -> Option<f64> {
    extract_decimal_amounts(line).last().copied()
}

fn compact_contains_any(compact: &str, labels: &[&str]) -> bool {
    labels.iter().any(|label| compact.contains(label))
}

fn is_subtotal_label(line: &str) -> bool {
    let compact = compact_label(line);
    compact_contains_any(
        &compact,
        &[
            "totalht",
            "montanttotalht",
            "totalhorstaxe",
            "totalhorstaxes",
            "sousto",
            "soustotal",
            "subtotal",
            "totalavanttaxe",
            "netamount",
            "netto",
            "zwischensumme",
            "subtotalfactura",
            "imponibile",
        ],
    )
}

fn is_payment_noise(line: &str) -> bool {
    let compact = compact_label(line);
    [
        "rendu",
        "monnaie",
        "change",
        "remise",
        "discount",
        "carte",
        "visa",
        "mastercard",
        "amex",
        "especes",
        "cash",
        "montantpaye",
        "paiement",
        "payment",
        "acompte",
        "avoir",
    ]
    .iter()
    .any(|label| compact.contains(label))
}

fn is_grand_total_label(line: &str) -> bool {
    if is_subtotal_label(line) || is_payment_noise(line) {
        return false;
    }
    let compact = compact_label(line);
    compact_contains_any(
        &compact,
        &[
            "totalttc",
            "montanttotalttc",
            "netapayer",
            "totalapayer",
            "montantapayer",
            "totaldelafacture",
            "totalfacture",
            "grandtotal",
            "amountdue",
            "balancedue",
            "totaldue",
            "invoicetotal",
            "rechnungsbetrag",
            "gesamtbetrag",
            "totalfactura",
            "totalefattura",
            "totaalfactuur",
        ],
    ) || (is_receipt_label(line) && compact.contains("total"))
}

fn tax_bucket(line: &str) -> Option<&'static str> {
    let compact = compact_label(line);
    let labels = [
        ("tva", "TVA"),
        ("vat", "VAT"),
        ("tps", "TPS"),
        ("tvq", "TVQ"),
        ("gst", "GST"),
        ("qst", "QST"),
        ("hst", "HST"),
        ("mwst", "MWST"),
        ("iva", "IVA"),
        ("btw", "BTW"),
        ("tax", "TAX"),
    ];
    labels
        .iter()
        .find(|(needle, _)| compact.contains(needle))
        .map(|(_, label)| *label)
}

fn extract_subtotal(text: &str) -> Option<f64> {
    text.lines()
        .filter(|line| is_subtotal_label(line))
        .filter_map(last_money_amount)
        .last()
}

fn extract_grand_total(text: &str) -> Option<f64> {
    text.lines()
        .filter(|line| is_grand_total_label(line))
        .filter_map(last_money_amount)
        .last()
}

fn extract_tax_total(text: &str) -> Option<f64> {
    let mut buckets: HashMap<&'static str, f64> = HashMap::new();
    for line in text.lines() {
        let Some(bucket) = tax_bucket(line) else {
            continue;
        };
        let compact = compact_label(line);
        if compact.contains("numero") || compact.contains("ident") || is_grand_total_label(line) {
            continue;
        }
        let Some(amount) = last_money_amount(line) else {
            continue;
        };
        buckets
            .entry(bucket)
            .and_modify(|existing| {
                if amount.abs() > existing.abs() {
                    *existing = amount;
                }
            })
            .or_insert(amount);
    }
    if buckets.is_empty() {
        None
    } else {
        Some(buckets.values().sum())
    }
}

fn supplier_line_is_usable(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < 2 || trimmed.len() > 60 || trimmed.contains('@') {
        return false;
    }
    let normalized = normalize_word(trimmed);
    let excluded = [
        "facture", "invoice", "factura", "rechnung", "fattura", "ticket", "receipt", "date",
        "client", "customer", "coordonnees", "telephone", "telecopie", "courriel", "email",
        "site web", "www.", "http", "licence", "membre", "soumission", "modalites",
        "paiement", "quantite", "description", "prix", "montant", "total", "commentaire",
        "merci", "adresse", "page", "subtotal", "tax", "tva", "vat", "tps", "tvq",
        "caisse", "caissier",
    ];
    if excluded.iter().any(|needle| normalized.contains(needle)) {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|character| character.is_alphabetic())
        .count();
    letters >= 2
        && !trimmed.contains('$')
        && !trimmed.contains('€')
        && !trimmed.contains('£')
        && !normalized.chars().all(|character| character.is_ascii_digit())
}

fn uppercase_ratio(value: &str) -> f32 {
    let letters = value
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    if letters.is_empty() {
        return 0.0;
    }
    letters.iter().filter(|character| character.is_uppercase()).count() as f32
        / letters.len() as f32
}

fn infer_supplier_hint(text: &str) -> Option<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(32)
        .collect::<Vec<_>>();
    let mut best: Option<(usize, i32)> = None;
    for (index, line) in lines.iter().enumerate() {
        if !supplier_line_is_usable(line) {
            continue;
        }
        let mut score = 115 - index as i32 * 5;
        if uppercase_ratio(line) >= 0.7 {
            score += 20;
        }
        if line.len() <= 32 {
            score += 10;
        }
        if line.contains(':')
            || line
                .chars()
                .filter(|character| character.is_ascii_digit())
                .count()
                > 6
        {
            score -= 35;
        }
        if best.map(|(_, old)| score > old).unwrap_or(true) {
            best = Some((index, score));
        }
    }
    let (index, score) = best?;
    if score < 55 {
        return None;
    }
    let first = lines[index];
    if first.len() <= 12 {
        if let Some(second) = lines.get(index + 1).copied() {
            if second.len() <= 22
                && supplier_line_is_usable(second)
                && uppercase_ratio(second) >= 0.6
                && !second.contains(':')
            {
                return Some(format!("{first} {second}"));
            }
        }
    }
    Some(first.to_string())
}

fn extract_siret_hint(text: &str) -> Option<String> {
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

fn extract_iban_hint(text: &str) -> Option<String> {
    let regex = Regex::new(
        r"(?i)\bIBAN\b\s*[:\-]?\s*([A-Z]{2}\s*[0-9OIl]{2}(?:[\s-]*[A-Z0-9OIl]){10,34})",
    )
    .ok()?;
    let raw = regex.captures(text)?.get(1)?.as_str();
    let cleaned = raw
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    (15..=34).contains(&cleaned.len()).then_some(cleaned)
}

fn postprocess_ocr_text(text: &str) -> String {
    let supplier = infer_supplier_hint(text);
    let invoice_number = extract_invoice_number_hint(text);
    let invoice_date = extract_invoice_date_hint(text);
    let amount_ht = extract_subtotal(text);
    let amount_ttc = extract_grand_total(text);
    let explicit_tax = extract_tax_total(text);
    let amount_vat = explicit_tax.or_else(|| match (amount_ht, amount_ttc) {
        (Some(ht), Some(ttc)) if ttc >= ht => Some(ttc - ht),
        _ => None,
    });
    let amount_ht = amount_ht.or_else(|| match (amount_vat, amount_ttc) {
        (Some(vat), Some(ttc)) if ttc >= vat => Some(ttc - vat),
        _ => None,
    });
    let siret = extract_siret_hint(text);
    let iban = extract_iban_hint(text);

    let mut normalized = String::new();
    if let Some(supplier) = supplier {
        normalized.push_str(&supplier);
        normalized.push('\n');
    }
    normalized.push_str(text.trim());
    normalized.push_str("\n\n--- CHAMPS NORMALISES OCR ---\n");
    if let Some(number) = invoice_number {
        normalized.push_str(&format!("Facture N° : {number}\n"));
    }
    if let Some(date) = invoice_date {
        normalized.push_str(&format!("Date facture : {date}\n"));
    }
    if let Some(ht) = amount_ht {
        normalized.push_str(&format!("Total HT : {}\n", format_money(ht)));
    }
    if let Some(vat) = amount_vat {
        normalized.push_str(&format!("TVA : {}\n", format_money(vat)));
    }
    if let Some(ttc) = amount_ttc {
        normalized.push_str(&format!("Total TTC : {}\n", format_money(ttc)));
    }
    if let Some(siret) = siret {
        normalized.push_str(&format!("SIRET : {siret}\n"));
    }
    if let Some(iban) = iban {
        normalized.push_str(&format!("IBAN : {iban}\n"));
    }
    normalized
}

fn line_key(line: &str) -> String {
    normalize_word(line)
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn merge_ocr_passes<'a>(passes: impl IntoIterator<Item = &'a str>) -> String {
    let mut output = String::new();
    let mut seen = HashSet::new();
    for pass in passes {
        for line in pass.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = line_key(trimmed);
            if key.len() < 2 || !seen.insert(key) {
                continue;
            }
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(trimmed);
        }
    }
    output
}

fn money_signal_count(text: &str) -> usize {
    let regex = match Regex::new(
        r"(?i)(?:\d{1,3}(?:[ .\u{00a0}\u{202f}]\d{3})+|\d+)[,.]\d{2}\s*(?:€|\$|£|EUR|CAD|USD|GBP)?",
    ) {
        Ok(value) => value,
        Err(_) => return 0,
    };
    regex.find_iter(text).take(24).count()
}

fn ocr_signal_count(text: &str) -> usize {
    let mut signals = 0;
    if text.lines().any(is_invoice_label) {
        signals += 1;
    }
    if text
        .lines()
        .take(100)
        .any(|line| parse_date_candidate(line).is_some())
    {
        signals += 1;
    }
    if money_signal_count(text) >= 2 {
        signals += 1;
    }
    if text
        .lines()
        .any(|line| is_subtotal_label(line) || is_grand_total_label(line))
    {
        signals += 1;
    }
    if infer_supplier_hint(text).is_some() {
        signals += 1;
    }
    signals
}

fn ocr_text_is_weak(text: &str) -> bool {
    let meaningful = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let signals = ocr_signal_count(text);
    signals < 3 || (meaningful < 70 && signals < 4)
}

fn ocr_text_needs_zones(text: &str) -> bool {
    let meaningful = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let receipt = text.lines().any(is_receipt_label);
    ocr_signal_count(text) < 4 || (receipt && meaningful < 350)
}

fn ocr_page_region(
    page: &PdfPage,
    engine: &OcrEngine,
    max_dimension: u32,
    long_edge: f32,
    source_rect: Option<Rect>,
) -> Result<String, String> {
    let page_size = page.Size().map_err(|error| error.to_string())?;
    let (source_width, source_height) = source_rect
        .as_ref()
        .map(|rect| (rect.Width, rect.Height))
        .unwrap_or((page_size.Width, page_size.Height));
    let (destination_width, destination_height) =
        render_dimensions(source_width, source_height, max_dimension, long_edge);

    let options = PdfPageRenderOptions::new().map_err(|error| error.to_string())?;
    if let Some(rect) = source_rect {
        options
            .SetSourceRect(rect)
            .map_err(|error| error.to_string())?;
    }
    options
        .SetDestinationWidth(destination_width)
        .map_err(|error| error.to_string())?;
    options
        .SetDestinationHeight(destination_height)
        .map_err(|error| error.to_string())?;

    let stream = InMemoryRandomAccessStream::new().map_err(|error| error.to_string())?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    stream.Seek(0).map_err(|error| error.to_string())?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    result
        .Text()
        .map_err(|error| error.to_string())
        .map(|text| text.to_string_lossy())
}

fn horizontal_regions(page: &PdfPage) -> Result<Vec<Rect>, String> {
    let size = page.Size().map_err(|error| error.to_string())?;
    let h = size.Height.max(1.0);
    let w = size.Width.max(1.0);
    Ok(vec![
        Rect {
            X: 0.0,
            Y: 0.0,
            Width: w,
            Height: h * 0.44,
        },
        Rect {
            X: 0.0,
            Y: h * 0.28,
            Width: w,
            Height: h * 0.44,
        },
        Rect {
            X: 0.0,
            Y: h * 0.56,
            Width: w,
            Height: h * 0.44,
        },
    ])
}

fn ocr_page_maximum(
    page: &PdfPage,
    engine: &OcrEngine,
    max_dimension: u32,
) -> Result<String, String> {
    let primary = ocr_page_region(
        page,
        engine,
        max_dimension,
        OCR_RENDER_LONG_EDGE_BASE,
        None,
    )?;
    if !ocr_text_is_weak(&primary) && !ocr_text_needs_zones(&primary) {
        return Ok(primary);
    }

    let detail = ocr_page_region(
        page,
        engine,
        max_dimension,
        OCR_RENDER_LONG_EDGE_DETAIL,
        None,
    )?;
    let mut merged = merge_ocr_passes([primary.as_str(), detail.as_str()]);
    if !ocr_text_needs_zones(&merged) {
        return Ok(merged);
    }

    let mut zone_texts = Vec::new();
    for region in horizontal_regions(page)? {
        if let Ok(text) = ocr_page_region(
            page,
            engine,
            max_dimension,
            OCR_RENDER_LONG_EDGE_ZONE,
            Some(region),
        ) {
            if !text.trim().is_empty() {
                zone_texts.push(text);
            }
        }
    }
    let mut passes = vec![merged.as_str()];
    passes.extend(zone_texts.iter().map(String::as_str));
    merged = merge_ocr_passes(passes);
    Ok(merged)
}

fn ocr_local_pdf(path: &Path) -> Result<String, String> {
    let _winrt = WinRtGuard::initialize()?;
    let path_string = path.to_string_lossy().into_owned();
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path_string))
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    let document = PdfDocument::LoadFromFileAsync(&file)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|error| error.to_string())?;
    let max_dimension = OcrEngine::MaxImageDimension().map_err(|error| error.to_string())?;
    let page_count = document.PageCount().map_err(|error| error.to_string())?;
    let mut output = String::new();

    for page_index in 0..page_count {
        let page = document.GetPage(page_index).map_err(|error| error.to_string())?;
        let page_text = ocr_page_maximum(&page, &engine, max_dimension)?;
        if !page_text.trim().is_empty() {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(&page_text);
        }
        page.Close().map_err(|error| error.to_string())?;
    }

    Ok(postprocess_ocr_text(&output))
}

pub fn ocr_pdf(source: &str) -> Result<String, String> {
    let source_path = Path::new(source);
    if !source_path.is_file() {
        return Err("Le PDF n'est plus accessible.".to_string());
    }

    let active_guard = ActiveOcrGuard::acquire(source)?;
    let temporary_path = temporary_pdf_path(source);
    fs::copy(source_path, &temporary_path)
        .map_err(|error| format!("Impossible de préparer le PDF pour l'OCR Windows : {error}"))?;

    let worker_path = temporary_path.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("app-comptabiliter-ocr".to_string())
        .spawn(move || {
            let _active_guard = active_guard;
            let result = ocr_local_pdf(&worker_path);
            let _ = fs::remove_file(&worker_path);
            let _ = sender.send(result);
        })
        .map_err(|error| {
            let _ = fs::remove_file(&temporary_path);
            format!("Impossible de démarrer le moteur OCR : {error}")
        })?;

    match receiver.recv_timeout(Duration::from_secs(OCR_TIMEOUT_SECONDS)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "L'OCR approfondi dépasse {OCR_TIMEOUT_SECONDS} secondes. L'application a repris la main."
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Le moteur OCR Windows s'est arrêté de manière inattendue.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        merge_ocr_passes, ocr_text_is_weak, ocr_text_needs_zones, postprocess_ocr_text,
        render_dimensions,
    };

    #[test]
    fn render_dimensions_limit_long_edge() {
        assert_eq!(
            render_dimensions(595.0, 842.0, 10_000, 1400.0),
            (989, 1400)
        );
        assert_eq!(
            render_dimensions(842.0, 595.0, 10_000, 1400.0),
            (1400, 989)
        );
    }

    #[test]
    fn render_dimensions_respect_windows_limit() {
        assert_eq!(
            render_dimensions(1000.0, 2000.0, 1000, 2400.0),
            (500, 1000)
        );
    }

    #[test]
    fn normalizes_complex_scanned_invoice_fields() {
        let raw = "DMS\nÉLECTRIQUE\n1554 Frédéric-Courtemanche, Chambly\nFACTURE N° : F-15886-001\nDATE DE LA FACTURE : 29 Jan, 2019\nCOORDONNÉES DU CLIENT\nFormatek Info-Services\nSous-total: 4.00\nTX - TPS @ 5%; TVQ @ 9,975%\nTPS 0.20\nTVQ 0.40\nTOTAL DE LA FACTURE : 4.60 $";
        let normalized = postprocess_ocr_text(raw);
        assert!(normalized.starts_with("DMS ÉLECTRIQUE\n"));
        assert!(normalized.contains("Facture N° : F-15886-001"));
        assert!(normalized.contains("Date facture : 29/01/2019"));
        assert!(normalized.contains("Total HT : 4.00"));
        assert!(normalized.contains("TVA : 0.60"));
        assert!(normalized.contains("Total TTC : 4.60"));
    }

    #[test]
    fn recognizes_international_invoice_vocabulary() {
        let raw = "ACME GMBH\nRECHNUNG Nr. RE-2026-0042\nRechnungsdatum 23.08.2026\nZwischensumme 100,00 EUR\nMWST 19,00 EUR\nGesamtbetrag 119,00 EUR";
        let normalized = postprocess_ocr_text(raw);
        assert!(normalized.contains("Facture N° : RE-2026-0042"));
        assert!(normalized.contains("Date facture : 23/08/2026"));
        assert!(normalized.contains("Total HT : 100.00"));
        assert!(normalized.contains("TVA : 19.00"));
        assert!(normalized.contains("Total TTC : 119.00"));
    }

    #[test]
    fn recognizes_receipt_reference_and_totals() {
        let raw = "CARREFOUR MARKET\nTicket N° A-004512\n23/08/2026 14:32\nSous-total 25,00 EUR\nTVA 2,95 EUR\nTOTAL TTC 27,95 EUR\nCARTE BANCAIRE 27,95 EUR";
        let normalized = postprocess_ocr_text(raw);
        assert!(normalized.contains("Facture N° : A-004512"));
        assert!(normalized.contains("Date facture : 23/08/2026"));
        assert!(normalized.contains("Total HT : 25.00"));
        assert!(normalized.contains("TVA : 2.95"));
        assert!(normalized.contains("Total TTC : 27.95"));
    }

    #[test]
    fn merges_multiple_ocr_passes_without_duplicate_lines() {
        let primary = "FACTURE\nTotal 10,00 EUR";
        let detail = "FACTURE\nDate 23/08/2026\nTotal 10,00 EUR";
        let zone = "Date 23/08/2026\nTVA 2,00 EUR";
        let merged = merge_ocr_passes([primary, detail, zone]);
        assert_eq!(merged.matches("FACTURE").count(), 1);
        assert_eq!(merged.matches("Date 23/08/2026").count(), 1);
        assert!(merged.contains("TVA 2,00 EUR"));
    }

    #[test]
    fn complete_core_signals_do_not_force_detail_pass() {
        assert!(ocr_text_is_weak("DMS ELECTRIQUE logo seulement"));
        assert!(!ocr_text_is_weak(
            "FACTURE F-42\nDate 23/08/2026\nSous-total 100,00 EUR\nTVA 20,00 EUR\nTotal TTC 120,00 EUR"
        ));
    }

    #[test]
    fn receipt_can_request_zone_reading() {
        assert!(ocr_text_needs_zones(
            "TICKET 42\n23/08/2026\nTOTAL TTC 12,00 EUR"
        ));
    }
}
