#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod charlemagne;

fn main() {
    let _ = charlemagne::prepare;
    app_comptabiliter_lib::run()
}
