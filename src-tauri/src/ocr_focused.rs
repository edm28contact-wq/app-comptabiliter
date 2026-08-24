use std::{collections::HashSet, path::Path};
use windows::{
    core::HSTRING,
    Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions},
    Foundation::{Rect, Size},
    Graphics::Imaging::BitmapDecoder,
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

const FOCUSED_LONG_EDGE: f32 = 3000.0;
const MAX_FOCUSED_PAGES: usize = 4;

struct WinRtGuard;

impl WinRtGuard {
    fn initialize() -> Result<Self, String> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)
                .map_err(|error| format!("Initialisation OCR focalisé impossible : {error}"))?;
        }
        Ok(Self)
    }
}

impl Drop for WinRtGuard {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn render_dimensions(
    width: f32,
    height: f32,
    max_dimension: u32,
    requested_long_edge: f32,
) -> (u32, u32) {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let target = requested_long_edge.min(max_dimension as f32).max(1.0);
    let scale = target / width.max(height);
    (
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

fn line_key(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn merge_lines(target: &mut String, seen: &mut HashSet<String>, text: &str) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = line_key(trimmed);
        if key.len() < 2 || !seen.insert(key) {
            continue;
        }
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(trimmed);
    }
}

fn bounded_rect(x: f32, y: f32, width: f32, height: f32, size: Size) -> Rect {
    let page_width = size.Width.max(1.0);
    let page_height = size.Height.max(1.0);
    let x = x.clamp(0.0, page_width - 1.0);
    let y = y.clamp(0.0, page_height - 1.0);
    Rect {
        X: x,
        Y: y,
        Width: width.min(page_width - x).max(1.0),
        Height: height.min(page_height - y).max(1.0),
    }
}

fn receipt_regions(size: Size) -> Vec<Rect> {
    let width = size.Width.max(1.0);
    let height = size.Height.max(1.0);
    let band_height = height * 0.28;
    [0.0_f32, 0.15, 0.30, 0.45, 0.60, 0.72]
        .into_iter()
        .map(|start| bounded_rect(0.0, height * start, width, band_height, size))
        .collect()
}

fn invoice_regions(size: Size) -> Vec<Rect> {
    let width = size.Width.max(1.0);
    let height = size.Height.max(1.0);
    vec![
        // En-tête fournisseur / référence / date, lu en deux moitiés qui se chevauchent.
        bounded_rect(0.0, 0.0, width * 0.60, height * 0.42, size),
        bounded_rect(width * 0.40, 0.0, width * 0.60, height * 0.42, size),
        // Corps : les tableaux très denses deviennent nettement plus grands pour Windows OCR.
        bounded_rect(0.0, height * 0.24, width, height * 0.48, size),
        // Bas de page : TVA et totaux sont souvent à droite mais parfois à gauche.
        bounded_rect(0.0, height * 0.56, width * 0.60, height * 0.44, size),
        bounded_rect(width * 0.40, height * 0.56, width * 0.60, height * 0.44, size),
    ]
}

fn selected_page_indexes(page_count: u32) -> Vec<u32> {
    let count = page_count as usize;
    if count <= MAX_FOCUSED_PAGES {
        return (0..page_count).collect();
    }
    let mut indexes = vec![0, 1, page_count - 2, page_count - 1];
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

fn ocr_region(
    page: &PdfPage,
    engine: &OcrEngine,
    max_dimension: u32,
    region: Rect,
) -> Result<String, String> {
    let (destination_width, destination_height) = render_dimensions(
        region.Width,
        region.Height,
        max_dimension,
        FOCUSED_LONG_EDGE,
    );
    let options = PdfPageRenderOptions::new().map_err(|error| error.to_string())?;
    options
        .SetSourceRect(region)
        .map_err(|error| error.to_string())?;
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
        .map(|value| value.to_string_lossy())
}

pub fn ocr_focused_pdf(source: &str, receipt_hint: bool) -> Result<String, String> {
    let source_path = Path::new(source);
    if !source_path.is_file() {
        return Err("Le PDF n'est plus accessible pour la lecture focalisée.".to_string());
    }

    let _winrt = WinRtGuard::initialize()?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(
        source_path.to_string_lossy().into_owned(),
    ))
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
    let mut seen = HashSet::new();
    for page_index in selected_page_indexes(page_count) {
        let page = document
            .GetPage(page_index)
            .map_err(|error| error.to_string())?;
        let size = page.Size().map_err(|error| error.to_string())?;
        let receipt_profile = receipt_hint || size.Height > size.Width * 1.65;
        let regions = if receipt_profile {
            receipt_regions(size)
        } else {
            invoice_regions(size)
        };

        for region in regions {
            if let Ok(text) = ocr_region(&page, &engine, max_dimension, region) {
                merge_lines(&mut output, &mut seen, &text);
            }
        }
        page.Close().map_err(|error| error.to_string())?;
    }

    if output.trim().is_empty() {
        Err("La lecture OCR focalisée n'a produit aucun texte exploitable.".to_string())
    } else {
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::{invoice_regions, receipt_regions, render_dimensions, selected_page_indexes};
    use windows::Foundation::Size;

    #[test]
    fn focused_render_respects_windows_limit() {
        assert_eq!(render_dimensions(500.0, 1000.0, 2000, 3000.0), (1000, 2000));
    }

    #[test]
    fn receipt_is_split_into_overlapping_bands() {
        let regions = receipt_regions(Size {
            Width: 300.0,
            Height: 1000.0,
        });
        assert_eq!(regions.len(), 6);
        assert!(regions[0].Height > regions[1].Y - regions[0].Y);
        assert!(regions.last().unwrap().Y + regions.last().unwrap().Height <= 1000.01);
    }

    #[test]
    fn invoice_targets_header_body_and_totals() {
        let regions = invoice_regions(Size {
            Width: 600.0,
            Height: 840.0,
        });
        assert_eq!(regions.len(), 5);
        assert!(regions[0].Width > 300.0);
        assert!(regions[4].Y > 400.0);
    }

    #[test]
    fn long_documents_focus_first_and_last_pages() {
        assert_eq!(selected_page_indexes(8), vec![0, 1, 6, 7]);
        assert_eq!(selected_page_indexes(3), vec![0, 1, 2]);
    }
}
