use std::{fs, path::Path};

#[tauri::command]
fn scan_pdf_folder(path: String) -> Result<Vec<String>, String> {
    let folder = Path::new(&path);

    if !folder.is_dir() {
        return Err("Le chemin sélectionné n'est pas un dossier accessible.".to_string());
    }

    let entries = fs::read_dir(folder).map_err(|error| error.to_string())?;
    let mut pdfs = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_path = entry.path();

        if file_path.is_file()
            && file_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            pdfs.push(file_path.to_string_lossy().into_owned());
        }
    }

    pdfs.sort();
    Ok(pdfs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![scan_pdf_folder])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
