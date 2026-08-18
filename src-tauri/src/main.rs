#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = tidyfile_lib::run() {
        eprintln!("tidyfile failed to start: {error}");
        std::process::exit(1);
    }
}
