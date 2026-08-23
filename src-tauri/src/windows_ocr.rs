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
    Graphics::Imaging::BitmapDecoder,
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

const OCR_RENDER_LONG_EDGE_BASE: f32 = 1500.0;
const OCR_RENDER_LONG_EDGE_DETAIL: f32 = 2400.0;
const OCR_TIMEOUT_SECONDS: u64 = 60;
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
            return Err("L'OCR de ce document est déjà en cours. Attendez sa fin avant de réessayer."
                .to_string());
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
    let destination_width = (width * scale).round().max(1.0) as u32;
    let destination_height = (height * scale).round().max(1.0) as u32;
    (destination_width, destination_height)
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
        .collect::<String>()
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
    let mut normalized = value
        .trim()
        .replace('€', "")
        .replace('$', "")
        .replace('£', "")
        .replace("EUR", "")
        .replace("CAD", "")
        .replace("USD", "")
        .replace("GBP", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if normalized.is_empty() {
        return None;
    }
    let negative_parentheses = normalized.starts_with('(') && normalized.ends_with(')');
    if negative_parentheses {
        normalized = normalized
            .trim_start_matches('(')
            .trim_end_matches(')')
            .to_string();
    }
    let comma = normalized.rfind(',');
    let dot = normalized.rfind('.');
    normalized = match (comma, dot) {
        (Some(comma_index), Some(dot_index)) if comma_index > dot_index => {
            normalized.replace('.', "").replace(',', ".")
        }
        (Some(_), Some(_)) => normalized.replace(',', ""),
        (Some(_), None) => normalized.replace(',', "."),
        (None, Some(_)) if normalized.matches('.').count() > 1 => {
            let mut pieces = normalized.split('.').collect::<Vec<_>>();
            let decimal = pieces.pop().unwrap_or("0");
            format!("{}.{}", pieces.join(""), decimal)
        }
        _ => normalized,
    };
    let parsed = normalized.parse::<f64>().ok()?.filter_finite()?;
    Some(if negative_parentheses { -parsed } else { parsed })
}

trait FiniteNumber {
    fn filter_finite(self) -> Option<f64>;
}

impl FiniteNumber for f64 {
    fn filter_finite(self) -> Option<f64> {
        self.is_finite().then_some(self)
    }
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
    let raw = value.trim();
    let iso = Regex::new(r"(?i)\b(19\d{2}|20\d{2})[/.\-](\d{1,2})[/.\-](\d{1,2})\b").ok()?;
    if let Some(captures) = iso.captures(raw) {
        return normalized_date(
            captures.get(3)?.as_str().parse().ok()?,
            captures.get(2)?.as_str().parse().ok()?,
            captures.get(1)?.as_str().parse().ok()?,
        );
    }

    let numeric = Regex::new(r"(?i)\b(\d{1,2})[/.\-](\d{1,2})[/.\-](\d{2,4})\b").ok()?;
    if let Some(captures) = numeric.captures(raw) {
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

    let day_month = Regex::new(r"(?i)\b(\d{1,2})\s+([A-Za-zÀ-ÿ.]+)[,\s]+(19\d{2}|20\d{2})\b").ok()?;
    if let Some(captures) = day_month.captures(raw) {
        return normalized_date(
            captures.get(1)?.as_str().parse().ok()?,
            textual_month(captures.get(2)?.as_str())?,
            captures.get(3)?.as_str().parse().ok()?,
        );
    }

    let month_day = Regex::new(r"(?i)\b([A-Za-zÀ-ÿ.]+)\s+(\d{1,2})[,]?\s+(19\d{2}|20\d{2})\b").ok()?;
    if let Some(captures) = month_day.captures(raw) {
        return normalized_date(
            captures.get(2)?.as_str().parse().ok()?,
            textual_month(captures.get(1)?.as_str())?,
            captures.get(3)?.as_str().parse().ok()?,
        );
    }
    None
}

fn is_invoice_label(line: &str) -> bool {
    let compact = compact_label(line);
    [
        "facture",
        "invoice",
        "factura",
        "rechnung",
        "fattura",
        "factuur",
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
    for line in text.lines().take(80) {
        if is_date_label(line) {
            if let Some(date) = parse_date_candidate(line) {
                return Some(date);
            }
        }
    }
    for line in text.lines().take(60) {
        if let Some(date) = parse_date_candidate(line) {
            return Some(date);
        }
    }
    None
}

fn invoice_number_candidate(token: &str) -> bool {
    let trimmed = token.trim_matches(|character: char| !character.is_ascii_alphanumeric() && !"-_/ .".contains(character));
    if trimmed.len() < 4 || trimmed.len() > 36 {
        return false;
    }
    let digits = trimmed.chars().filter(|character| character.is_ascii_digit()).count();
    let letters = trimmed.chars().filter(|character| character.is_ascii_alphabetic()).count();
    let separators = trimmed.chars().filter(|character| "-_/".contains(*character)).count();
    digits >= 2 && (letters >= 1 || separators >= 1) && parse_date_candidate(trimmed).is_none()
}

fn extract_invoice_number_hint(text: &str) -> Option<String> {
    let regex = Regex::new(
        r"(?i)(?:facture|invoice|factura|rechnung|fattura|factuur)\s*(?:n[°oº]?|num(?:e|é)ro|number|nr\.?|no\.?|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
    )
    .ok()?;
    if let Some(value) = regex
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_string())
        .filter(|value| invoice_number_candidate(value))
    {
        return Some(value);
    }

    let token_regex = Regex::new(r"[A-Za-z0-9][A-Za-z0-9._/-]{3,}").ok()?;
    for line in text.lines().take(80) {
        if !is_invoice_label(line) {
            continue;
        }
        let mut candidates = token_regex
            .find_iter(line)
            .map(|value| value.as_str())
            .filter(|value| invoice_number_candidate(value))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|value| std::cmp::Reverse(value.len()));
        if let Some(value) = candidates.first() {
            return Some((*value).to_string());
        }
    }
    None
}

fn extract_decimal_amounts(line: &str) -> Vec<(usize, usize, f64)> {
    let regex = match Regex::new(r"(?i)(?:\d{1,3}(?:[ .\u{00a0}\u{202f}]\d{3})+|\d+)(?:[,.]\d{2,3})") {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    regex
        .find_iter(line)
        .filter_map(|capture| {
            let after = line.get(capture.end()..).unwrap_or("");
            if after.trim_start().starts_with('%') {
                return None;
            }
            parse_money(capture.as_str()).map(|value| (capture.start(), capture.end(), value))
        })
        .collect()
}

fn last_money_amount(line: &str) -> Option<f64> {
    extract_decimal_amounts(line).last().map(|(_, _, value)| *value)
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

fn is_grand_total_label(line: &str) -> bool {
    let compact = compact_label(line);
    compact_contains_any(
        &compact,
        &[
            "totalttc",
            "montanttotalttc",
            "netapayer",
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
            "totale",
            "totaalfactuur",
        ],
    ) && !is_subtotal_label(line)
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
        let Some(amount) = last_money_amount(line) else {
            continue;
        };
        let normalized = compact_label(line);
        if normalized.contains("total") && is_grand_total_label(line) {
            continue;
        }
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
        "facture",
        "invoice",
        "factura",
        "rechnung",
        "fattura",
        "date",
        "client",
        "customer",
        "coordonnees",
        "travaux effectues",
        "telephone",
        "telecopie",
        "courriel",
        "email",
        "site web",
        "www.",
        "http",
        "licence",
        "membre",
        "soumission",
        "modalites",
        "paiement",
        "quantite",
        "description",
        "prix",
        "montant",
        "total",
        "commentaire",
        "residentiel",
        "commercial",
        "industriel",
        "institutionnel",
        "merci",
        "adresse",
        "page",
        "subtotal",
        "tax",
        "tva",
        "vat",
        "tps",
        "tvq",
    ];
    if excluded.iter().any(|needle| normalized.contains(needle)) {
        return false;
    }
    let letters = trimmed.chars().filter(|character| character.is_alphabetic()).count();
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
    let uppercase = letters.iter().filter(|character| character.is_uppercase()).count();
    uppercase as f32 / letters.len() as f32
}

fn infer_supplier_hint(text: &str) -> Option<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(30)
        .collect::<Vec<_>>();
    let mut best: Option<(usize, i32)> = None;
    for (index, line) in lines.iter().enumerate() {
        if !supplier_line_is_usable(line) {
            continue;
        }
        let mut score = 110 - (index as i32 * 5);
        if uppercase_ratio(line) >= 0.7 {
            score += 20;
        }
        if line.len() <= 32 {
            score += 10;
        }
        if line.contains(':') || line.chars().filter(|character| character.is_ascii_digit()).count() > 6 {
            score -= 35;
        }
        if normalize_word(line).contains("logo") {
            score -= 40;
        }
        if best.map(|(_, best_score)| score > best_score).unwrap_or(true) {
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
                && !second.contains('-')
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
    let regex = Regex::new(r"(?i)\bIBAN\b\s*[:\-]?\s*([A-Z]{2}\s*[0-9OIl]{2}(?:[\s-]*[A-Z0-9OIl]){10,34})").ok()?;
    let raw = regex.captures(text)?.get(1)?.as_str();
    let cleaned = raw
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    (cleaned.len() >= 15 && cleaned.len() <= 34).then_some(cleaned)
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

fn merge_ocr_passes(primary: &str, detail: &str) -> String {
    let mut output = String::new();
    let mut seen = HashSet::new();
    for line in primary.lines().chain(detail.lines()) {
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
    output
}

fn money_signal_count(text: &str) -> usize {
    let regex = match Regex::new(r"(?i)(?:\d{1,3}(?:[ .\u{00a0}\u{202f}]\d{3})+|\d+)[,.]\d{2}\s*(?:€|\$|£|EUR|CAD|USD|GBP)?") {
        Ok(value) => value,
        Err(_) => return 0,
    };
    regex.find_iter(text).take(20).count()
}

fn has_date_signal(text: &str) -> bool {
    text.lines().take(80).any(|line| parse_date_candidate(line).is_some())
}

fn ocr_text_is_weak(text: &str) -> bool {
    let meaningful = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let mut signals = 0;
    if text.lines().any(is_invoice_label) {
        signals += 1;
    }
    if has_date_signal(text) {
        signals += 1;
    }
    if money_signal_count(text) >= 2 {
        signals += 1;
    }
    if text.lines().any(|line| is_subtotal_label(line) || is_grand_total_label(line)) {
        signals += 1;
    }
    meaningful < 180 || signals < 3
}

fn ocr_page_at_resolution(
    page: &PdfPage,
    engine: &OcrEngine,
    max_dimension: u32,
    long_edge: f32,
) -> Result<String, String> {
    let page_size = page.Size().map_err(|error| error.to_string())?;
    let (destination_width, destination_height) =
        render_dimensions(page_size.Width, page_size.Height, max_dimension, long_edge);

    let options = PdfPageRenderOptions::new().map_err(|error| error.to_string())?;
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
        let primary = ocr_page_at_resolution(
            &page,
            &engine,
            max_dimension,
            OCR_RENDER_LONG_EDGE_BASE,
        )?;
        let page_text = if ocr_text_is_weak(&primary) {
            let detail = ocr_page_at_resolution(
                &page,
                &engine,
                max_dimension,
                OCR_RENDER_LONG_EDGE_DETAIL,
            )?;
            merge_ocr_passes(&primary, &detail)
        } else {
            primary
        };

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
            "L'OCR dépasse {OCR_TIMEOUT_SECONDS} secondes. Il continue en arrière-plan mais l'application a repris la main."
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Le moteur OCR Windows s'est arrêté de manière inattendue.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        merge_ocr_passes, ocr_text_is_weak, postprocess_ocr_text, render_dimensions,
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
    fn merges_second_ocr_pass_without_duplicate_lines() {
        let merged = merge_ocr_passes(
            "FACTURE\nTotal 10,00 EUR",
            "FACTURE\nDate 23/08/2026\nTotal 10,00 EUR",
        );
        assert_eq!(merged.matches("FACTURE").count(), 1);
        assert!(merged.contains("Date 23/08/2026"));
    }

    #[test]
    fn requests_detail_pass_when_core_signals_are_missing() {
        assert!(ocr_text_is_weak("DMS ELECTRIQUE logo seulement"));
        assert!(!ocr_text_is_weak(
            "FACTURE F-42\nDate 23/08/2026\nSous-total 100,00 EUR\nTVA 20,00 EUR\nTotal TTC 120,00 EUR"
        ));
    }
}
