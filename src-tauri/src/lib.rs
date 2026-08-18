pub mod executor;
pub mod ipc;
pub mod journal;
pub mod paths;
pub mod rules;
pub mod service;
pub mod settings;
pub mod store;
pub mod templates;
pub mod watch;
pub mod watcher;

use tauri::Manager;

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let folder = app.path().app_data_dir()?;
            std::fs::create_dir_all(&folder)?;
            app.manage(ipc::AppState::open(&folder)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::simulate,
            ipc::organize,
            ipc::undo,
            ipc::interrupted,
            ipc::settle_interrupted,
            ipc::load_rules,
            ipc::save_rules,
            ipc::activity,
            ipc::load_settings,
            ipc::save_settings,
            ipc::undo_operation,
            ipc::operations,
            ipc::start_watching,
            ipc::stop_watching,
            ipc::watched_folders
        ])
        .run(tauri::generate_context!())
}
