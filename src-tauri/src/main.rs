// src-tauri/src/main.rs

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Entry point for the application binary
fn main() {
    // Call the run function from the library crate (dictator_lib)
    // The #[tokio::main] attribute is now in lib.rs's run function,
    // so we don't need it here directly on main unless main itself becomes async.
    // However, since lib.rs::run is async and decorated with #[tokio::main],
    // it sets up its own runtime. Calling it directly should work.
    dictator_lib::run();
}
