use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};

const TEMPORARY_EXTENSIONS: [&str; 7] = [
    "crdownload",
    "part",
    "partial",
    "tmp",
    "temp",
    "download",
    "opdownload",
];

const TEMPORARY_PREFIXES: [&str; 3] = ["~$", ".~lock.", ".goutputstream"];

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("could not watch the folder")]
    Start(#[from] notify::Error),
}

pub fn is_temporary(path: &Path) -> bool {
    has_temporary_extension(path) || has_temporary_prefix(path)
}

fn has_temporary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| TEMPORARY_EXTENSIONS.contains(&extension.as_str()))
}

fn has_temporary_prefix(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            TEMPORARY_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
}

fn is_content_change(kind: EventKind) -> bool {
    matches!(kind, EventKind::Create(_) | EventKind::Modify(_))
}

fn settled_paths(events: Vec<DebouncedEvent>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = events
        .into_iter()
        .filter(|event| is_content_change(event.kind))
        .flat_map(|event| event.paths.clone())
        .filter(|path| path.is_file() && !is_temporary(path))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

pub struct FolderWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    settled: Receiver<PathBuf>,
}

impl FolderWatcher {
    pub fn start(folder: &Path, quiet_period: Duration) -> Result<Self, WatcherError> {
        let (sender, settled) = mpsc::channel();
        let mut debouncer =
            new_debouncer(quiet_period, None, move |result: DebounceEventResult| {
                let Ok(events) = result else {
                    return;
                };
                for path in settled_paths(events) {
                    let _ = sender.send(path);
                }
            })?;
        debouncer.watch(folder, RecursiveMode::Recursive)?;
        Ok(Self { debouncer, settled })
    }

    pub fn next_settled(&self, timeout: Duration) -> Option<PathBuf> {
        self.settled.recv_timeout(timeout).ok()
    }

    pub fn stop(self) {
        self.debouncer.stop();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn download_temporaries_are_ignored() {
        for name in [
            "movie.crdownload",
            "archive.zip.part",
            "notes.TMP",
            "setup.download",
            "~$budget.xlsx",
            ".~lock.report.odt#",
        ] {
            assert!(is_temporary(Path::new(name)), "should ignore {name}");
        }
    }

    #[test]
    fn real_files_are_not_ignored() {
        for name in ["invoice.pdf", "photo.jpeg", "notes", "archive.tar.gz"] {
            assert!(!is_temporary(Path::new(name)), "should accept {name}");
        }
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert!(is_temporary(Path::new("a.CRDOWNLOAD")));
        assert!(is_temporary(Path::new("b.Part")));
    }

    #[test]
    fn unicode_and_hostile_names_are_not_temporary() {
        for name in ["informe año.pdf", "\u{202e}gnp.exe", "  leading space.txt"] {
            assert!(!is_temporary(Path::new(name)), "should accept {name}");
        }
    }

    #[test]
    fn watcher_reports_a_new_file_and_skips_its_temporary() {
        let folder = TempDir::new().unwrap();
        let watcher = FolderWatcher::start(folder.path(), Duration::from_millis(200)).unwrap();

        fs::write(folder.path().join("download.crdownload"), b"partial").unwrap();
        fs::write(folder.path().join("invoice.pdf"), b"content").unwrap();

        let settled = watcher.next_settled(Duration::from_secs(10));
        watcher.stop();

        let settled = settled.expect("watcher reported no file");
        assert_eq!(settled.file_name().unwrap(), "invoice.pdf");
    }
}
