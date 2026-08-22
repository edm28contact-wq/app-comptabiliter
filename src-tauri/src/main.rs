#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bank;
mod identifiers;

fn main() {
    let _ = bank::check_statement;
    let _ = identifiers::is_valid_siret;
    let _ = identifiers::is_valid_iban;
    app_comptabiliter_lib::run()
}
