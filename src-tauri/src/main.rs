#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

include!("lib.rs");
mod bank;
mod bank_workspace;
mod charlemagne_connector;
mod charlemagne_sync;
mod ingestion;

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
            charlemagne_sync::get_charlemagne_sync_folder,
            charlemagne_sync::set_charlemagne_sync_folder,
            charlemagne_sync::import_charlemagne_sync_file,
            charlemagne_sync::commit_charlemagne_sync_file,
            charlemagne_sync::scan_charlemagne_sync_folder,
            charlemagne_sync::list_charlemagne_sync_imports,
            charlemagne_sync::get_charlemagne_sync_summary,
            charlemagne_sync::list_charlemagne_accounts,
            charlemagne_sync::list_charlemagne_suppliers,
            charlemagne_sync::list_journal_entries,
            bank_workspace::get_bank_watched_folder,
            bank_workspace::set_bank_watched_folder,
            bank_workspace::scan_bank_folder,
            bank_workspace::list_bank_documents,
            bank_workspace::run_bank_ocr,
            bank_workspace::get_bank_document_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
