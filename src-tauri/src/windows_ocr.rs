use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process,
    thread,
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

struct WinRtGuard;

impl WinRtGuard {
    fn initialize() -> Result<Self, String> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)
                .ok()
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
        let page_text = result.Text().map_err(|error| error.to_string())?.to_string_lossy();

        if !page_text.trim().is_empty() {
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(&page_text);
        }

        page.Close().map_err(|error| error.to_string())?;
    }

    Ok(output)
}

pub fn ocr_pdf(source: &str) -> Result<String, String> {
    let source_path = Path::new(source);
    if !source_path.is_file() {
        return Err("Le PDF n'est plus accessible.".to_string());
    }

    let temporary_path = temporary_pdf_path(source);
    fs::copy(source_path, &temporary_path)
        .map_err(|error| format!("Impossible de préparer le PDF pour l'OCR Windows : {error}"))?;

    let worker_path = temporary_path.clone();
    let worker = thread::Builder::new()
        .name("app-comptabiliter-ocr".to_string())
        .spawn(move || ocr_local_pdf(&worker_path))
        .map_err(|error| format!("Impossible de démarrer le moteur OCR : {error}"))?;

    let result = worker
        .join()
        .map_err(|_| "Le moteur OCR Windows s'est arrêté de manière inattendue.".to_string())?;
    let _ = fs::remove_file(&temporary_path);
    result
}

#[cfg(test)]
mod tests {
    use super::render_dimensions;

    #[test]
    fn render_dimensions_limit_long_edge() {
        assert_eq!(render_dimensions(595.0, 842.0, 10_000), (989, 1400));
        assert_eq!(render_dimensions(842.0, 595.0, 10_000), (1400, 989));
    }

    #[test]
    fn render_dimensions_respect_windows_limit() {
        assert_eq!(render_dimensions(1000.0, 2000.0, 1000), (500, 1000));
    }
}
