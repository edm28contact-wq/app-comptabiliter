use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process,
};
use windows::{
    core::HSTRING,
    Data::Pdf::PdfDocument,
    Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat},
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
};

fn temporary_pdf_path(source: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    std::env::temp_dir().join(format!(
        "app-comptabiliter-ocr-{}-{:x}.pdf",
        process::id(),
        hasher.finish()
    ))
}

fn ocr_local_pdf(path: &Path) -> Result<String, String> {
    let path_string = path.to_string_lossy().into_owned();
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path_string))
        .map_err(|error| error.to_string())?
        .get()
        .map_err(|error| error.to_string())?;
    let document = PdfDocument::LoadFromFileAsync(&file)
        .map_err(|error| error.to_string())?
        .get()
        .map_err(|error| error.to_string())?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|error| error.to_string())?;
    let page_count = document.PageCount().map_err(|error| error.to_string())?;
    let mut output = String::new();

    for page_index in 0..page_count {
        let page = document.GetPage(page_index).map_err(|error| error.to_string())?;
        let stream = InMemoryRandomAccessStream::new().map_err(|error| error.to_string())?;
        page.RenderToStreamAsync(&stream)
            .map_err(|error| error.to_string())?
            .get()
            .map_err(|error| error.to_string())?;
        stream.Seek(0).map_err(|error| error.to_string())?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|error| error.to_string())?
            .get()
            .map_err(|error| error.to_string())?;
        let bitmap = decoder
            .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)
            .map_err(|error| error.to_string())?
            .get()
            .map_err(|error| error.to_string())?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|error| error.to_string())?
            .get()
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

    let result = ocr_local_pdf(&temporary_path);
    let _ = fs::remove_file(&temporary_path);
    result
}
