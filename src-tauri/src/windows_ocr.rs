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

    Ok(output)
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
