pub mod executor;
pub mod ipc;
pub mod journal;
pub mod paths;
pub mod rules;
pub mod service;
pub mod templates;
pub mod watcher;

use tauri::Manager;

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            let folder = app.path().app_data_dir()?;
            std::fs::create_dir_all(&folder)?;
            app.manage(ipc::AppState::open(&folder.join("journal.sqlite"))?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::simulate,
            ipc::organize,
            ipc::undo,
            ipc::interrupted
        ])
        .run(tauri::generate_context!())
}
