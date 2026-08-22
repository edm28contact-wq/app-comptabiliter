#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

include!("lib.rs");
mod bank;
mod charlemagne_connector;
mod ingestion;
mod workspace;

fn main() {
    let _ = bank::check_statement;
    let _ = ingestion::is_stable;
    let _ = ingestion::sha256;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_watched_folder,
            set_watched_folder,
            register_invoice,
            analyze_invoice,
            run_invoice_ocr,
            get_invoice_text,
            get_invoice_parsed,
            get_supplier_accounting,
            get_supplier_storage,
            validate_invoice,
            prepare_charlemagne_invoice,
            get_charlemagne_prepared,
            archive_invoice,
            list_invoices,
            scan_pdf_folder,
            charlemagne_connector::get_charlemagne_connector_status,
            charlemagne_connector::set_charlemagne_connector_mode,
            workspace::get_bank_watched_folder,
            workspace::set_bank_watched_folder,
            workspace::scan_bank_folder,
            workspace::list_bank_documents,
            workspace::run_bank_ocr,
            workspace::get_bank_document_text,
            workspace::list_journal_entries
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
