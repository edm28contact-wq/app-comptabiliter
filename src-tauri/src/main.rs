#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bank;

fn main() {
    let _ = bank::check_statement;
    app_comptabiliter_lib::run()
}
