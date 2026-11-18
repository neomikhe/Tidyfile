#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use tempfile::TempDir;

// WHY: macOS keeps the system temp folder under /private, where no watched folder is allowed
pub fn watchable_folder() -> TempDir {
    if crate::paths::accept_watched_folder(&std::env::temp_dir()).is_ok() {
        return TempDir::new().unwrap();
    }
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("tmp");
    std::fs::create_dir_all(&base).unwrap();
    TempDir::new_in(base).unwrap()
}
