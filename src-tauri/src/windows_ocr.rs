use regex::Regex;
use std::{
    collections::{hash_map::DefaultHasher, HashSet},
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
    Data::Pdf::{PdfDocument, PdfPageRenderOptions},
    Graphics::Imaging::BitmapDecoder,
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

const OCR_RENDER_LONG_EDGE: f32 = 1400.0;
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

fn render_dimensions(width: f32, height: f32, max_dimension: u32) -> (u32, u32) {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let target_long_edge = OCR_RENDER_LONG_EDGE.min(max_dimension as f32).max(1.0);
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

fn parse_money(value: &str) -> Option<f64> {
    let mut normalized = value
        .trim()
        .replace('€', "")
        .replace('$', "")
        .replace("EUR", "")
        .replace("CAD", "")
        .replace("USD", "")
        .replace('\u{00a0}', "")
        .replace('\u{202f}', "")
        .replace(' ', "");
    if normalized.is_empty() {
        return None;
    }
    let comma = normalized.rfind(',');
    let dot = normalized.rfind('.');
    normalized = match (comma, dot) {
        (Some(comma_index), Some(dot_index)) if comma_index > dot_index => {
            normalized.replace('.', "").replace(',', ".")
        }
        (Some(_), Some(_)) => normalized.replace(',', ""),
        (Some(_), None) => normalized.replace(',', "."),
        _ => normalized,
    };
    normalized.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn format_money(value: f64) -> String {
    format!("{value:.2}")
}

fn textual_month(value: &str) -> Option<u32> {
    let month = normalize_word(value)
        .trim_matches(|character: char| !character.is_alphabetic())
        .to_string();
    match month.as_str() {
        "jan" | "janv" | "janvier" | "january" => Some(1),
        "feb" | "fev" | "fevr" | "fevrier" | "february" => Some(2),
        "mar" | "mars" | "march" => Some(3),
        "apr" | "avr" | "avril" | "april" => Some(4),
        "may" | "mai" => Some(5),
        "jun" | "juin" | "june" => Some(6),
        "jul" | "juil" | "juillet" | "july" => Some(7),
        "aug" | "aou" | "aout" | "august" => Some(8),
        "sep" | "sept" | "septembre" | "september" => Some(9),
        "oct" | "octobre" | "october" => Some(10),
        "nov" | "novembre" | "november" => Some(11),
        "dec" | "decembre" | "december" => Some(12),
        _ => None,
    }
}

fn extract_invoice_date_hint(text: &str) -> Option<String> {
    let numeric = Regex::new(
        r"(?i)(?:date\s*(?:de\s*la\s*)?(?:facture)?|invoice\s*date)\s*[:\-]?\s*(\d{1,2})[/.\-](\d{1,2})[/.\-](\d{2,4})",
    )
    .ok()?;
    if let Some(captures) = numeric.captures(text) {
        let day = captures.get(1)?.as_str().parse::<u32>().ok()?;
        let month = captures.get(2)?.as_str().parse::<u32>().ok()?;
        let mut year = captures.get(3)?.as_str().parse::<u32>().ok()?;
        if captures.get(3)?.as_str().len() == 2 {
            year += 2000;
        }
        if (1..=31).contains(&day) && (1..=12).contains(&month) {
            return Some(format!("{day:02}/{month:02}/{year:04}"));
        }
    }

    let textual = Regex::new(
        r"(?i)(?:date\s*(?:de\s*la\s*)?(?:facture)?|invoice\s*date)\s*[:\-]?\s*(\d{1,2})\s+([A-Za-zÀ-ÿ.]+)[,\s]+(\d{4})",
    )
    .ok()?;
    let captures = textual.captures(text)?;
    let day = captures.get(1)?.as_str().parse::<u32>().ok()?;
    let month = textual_month(captures.get(2)?.as_str())?;
    let year = captures.get(3)?.as_str().parse::<u32>().ok()?;
    if (1..=31).contains(&day) && (1900..=2100).contains(&year) {
        Some(format!("{day:02}/{month:02}/{year:04}"))
    } else {
        None
    }
}

fn extract_invoice_number_hint(text: &str) -> Option<String> {
    let regex = Regex::new(
        r"(?i)(?:facture|invoice)\s*(?:n[°oº]?|num(?:e|é)ro|number|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,})",
    )
    .ok()?;
    regex
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().trim().to_string())
}

fn extract_labeled_money(text: &str, labels: &[&str]) -> Option<f64> {
    for label in labels {
        let pattern = format!(
            r"(?i){}\s*[:\-]?\s*([0-9][0-9\s\u{{00a0}}\u{{202f}}.,]*)\s*(?:€|\$|EUR|CAD|USD)?",
            label
        );
        if let Ok(regex) = Regex::new(&pattern) {
            if let Some(value) = regex
                .captures(text)
                .and_then(|captures| captures.get(1))
                .and_then(|value| parse_money(value.as_str()))
            {
                return Some(value);
            }
        }
    }
    None
}

fn extract_tax_total(text: &str) -> Option<f64> {
    let regex = Regex::new(
        r"(?i)\b(?:TVA|VAT|TPS|TVQ|GST|QST|HST)\b\s*[:=\-]?\s*([0-9][0-9\s\u{00a0}\u{202f}.,]*)\s*(?:€|\$|EUR|CAD|USD)?",
    )
    .ok()?;
    let mut count = 0usize;
    let mut total = 0.0;
    for captures in regex.captures_iter(text) {
        let Some(raw) = captures.get(1) else {
            continue;
        };
        let tail_start = raw.end();
        let tail = text.get(tail_start..tail_start.saturating_add(2).min(text.len())).unwrap_or("");
        if tail.trim_start().starts_with('%') {
            continue;
        }
        if let Some(value) = parse_money(raw.as_str()) {
            total += value;
            count += 1;
        }
    }
    (count > 0).then_some(total)
}

fn supplier_line_is_usable(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < 2 || trimmed.len() > 50 || trimmed.contains('@') {
        return false;
    }
    let normalized = normalize_word(trimmed);
    let excluded = [
        "facture",
        "invoice",
        "date",
        "client",
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
    ];
    if excluded.iter().any(|needle| normalized.contains(needle)) {
        return false;
    }
    let letters = trimmed.chars().filter(|character| character.is_alphabetic()).count();
    letters >= 2
        && !trimmed.contains('$')
        && !trimmed.contains('€')
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
        .take(24)
        .collect::<Vec<_>>();
    let mut best: Option<(usize, i32)> = None;
    for (index, line) in lines.iter().enumerate() {
        if !supplier_line_is_usable(line) {
            continue;
        }
        let mut score = 90 - (index as i32 * 5);
        if uppercase_ratio(line) >= 0.7 {
            score += 20;
        }
        if line.len() <= 28 {
            score += 10;
        }
        if line.contains(':') || line.chars().filter(|character| character.is_ascii_digit()).count() > 6 {
            score -= 30;
        }
        if best.map(|(_, best_score)| score > best_score).unwrap_or(true) {
            best = Some((index, score));
        }
    }
    let (index, score) = best?;
    if score < 45 {
        return None;
    }
    let first = lines[index];
    if first.len() <= 10 {
        if let Some(second) = lines.get(index + 1).copied() {
            if second.len() <= 18
                && supplier_line_is_usable(second)
                && uppercase_ratio(second) >= 0.7
                && !second.contains('-')
                && !second.contains(':')
            {
                return Some(format!("{first} {second}"));
            }
        }
    }
    Some(first.to_string())
}

fn postprocess_ocr_text(text: &str) -> String {
    let supplier = infer_supplier_hint(text);
    let invoice_number = extract_invoice_number_hint(text);
    let invoice_date = extract_invoice_date_hint(text);
    let amount_ht = extract_labeled_money(
        text,
        &[
            r"(?:montant\s+)?total\s+H\.?T\.??",
            r"total\s+hors\s+taxes?",
            r"hors\s+taxes?",
            r"sous[-\s]?total",
            r"subtotal",
            r"total\s+avant\s+taxes?",
        ],
    );
    let amount_ttc = extract_labeled_money(
        text,
        &[
            r"montant\s+total\s+T\.?T\.?C\.??",
            r"total\s+T\.?T\.?C\.??",
            r"net\s+[àa]\s+payer",
            r"total\s+de\s+la\s+facture",
            r"total\s+facture",
            r"grand\s+total",
            r"amount\s+due",
            r"balance\s+due",
            r"total\s+due",
        ],
    );
    let explicit_tax = extract_tax_total(text);
    let amount_vat = explicit_tax.or_else(|| match (amount_ht, amount_ttc) {
        (Some(ht), Some(ttc)) if ttc >= ht => Some(ttc - ht),
        _ => None,
    });
    let amount_ht = amount_ht.or_else(|| match (amount_vat, amount_ttc) {
        (Some(vat), Some(ttc)) if ttc >= vat => Some(ttc - vat),
        _ => None,
    });

    let mut normalized = String::new();
    if let Some(supplier) = supplier {
        normalized.push_str(&supplier);
        normalized.push('\n');
    }
    normalized.push_str(text.trim());
    normalized.push_str("\n\n");
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
    normalized
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
        let page_size = page.Size().map_err(|error| error.to_string())?;
        let (destination_width, destination_height) =
            render_dimensions(page_size.Width, page_size.Height, max_dimension);

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
        let page_text = result
            .Text()
            .map_err(|error| error.to_string())?
            .to_string_lossy();

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
    use super::{postprocess_ocr_text, render_dimensions};

    #[test]
    fn render_dimensions_limit_long_edge() {
        assert_eq!(render_dimensions(595.0, 842.0, 10_000), (989, 1400));
        assert_eq!(render_dimensions(842.0, 595.0, 10_000), (1400, 989));
    }

    #[test]
    fn render_dimensions_respect_windows_limit() {
        assert_eq!(render_dimensions(1000.0, 2000.0, 1000), (500, 1000));
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
}
