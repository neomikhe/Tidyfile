pub mod executor;
pub mod journal;
pub mod watcher;

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default().run(tauri::generate_context!())
}
