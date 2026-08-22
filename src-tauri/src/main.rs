#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bank;
mod identifiers;
mod ingestion;

fn main() {
    let _ = bank::check_statement;
    let _ = identifiers::is_valid_siret;
    let _ = identifiers::is_valid_iban;
    let _ = ingestion::is_stable;
    let _ = ingestion::sha256;
    app_comptabiliter_lib::run()
}
