use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer_opt,
};

const TEMPORARY_EXTENSIONS: [&str; 8] = [
    "crdownload",
    "part",
    "partial",
    "tmp",
    "temp",
    "download",
    "opdownload",
    crate::executor::PARTIAL_EXTENSION,
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
        .filter(|path| !is_temporary(path) && has_content(path))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

// WHY: Firefox reserves the final name with an empty file while it downloads into a .part beside it
fn has_content(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|data| data.is_file() && data.len() > 0)
}

fn forward_settled(sender: Sender<PathBuf>) -> impl FnMut(DebounceEventResult) + Send + 'static {
    move |result| {
        let Ok(events) = result else {
            return;
        };
        for path in settled_paths(events) {
            let _ = sender.send(path);
        }
    }
}

pub struct FolderWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    settled: Receiver<PathBuf>,
}

impl FolderWatcher {
    pub fn start(folder: &Path, quiet_period: Duration) -> Result<Self, WatcherError> {
        let (sender, settled) = mpsc::channel();
        // WHY: notify's default walks into symlinked folders, which can lead far outside this one
        let config = Config::default().with_follow_symlinks(false);
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, RecommendedCache>(
            quiet_period,
            None,
            forward_settled(sender),
            RecommendedCache::new(),
            config,
        )?;
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
    fn a_copy_tidyfile_set_aside_is_never_tidied_again() {
        for name in [
            "video.mkv.tidyfile-partial",
            "video.mkv (2).tidyfile-partial",
            "VIDEO.MKV.TIDYFILE-PARTIAL",
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

    #[cfg(unix)]
    #[test]
    fn a_link_is_never_reported_as_a_settled_file() {
        let folder = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        fs::write(elsewhere.path().join("secret.pdf"), b"outside").unwrap();
        let watcher = FolderWatcher::start(folder.path(), Duration::from_millis(200)).unwrap();

        std::os::unix::fs::symlink(
            elsewhere.path().join("secret.pdf"),
            folder.path().join("link.pdf"),
        )
        .unwrap();
        fs::write(folder.path().join("real.pdf"), b"content").unwrap();
        let mut reported = Vec::new();
        let mut wait = Duration::from_secs(10);
        while let Some(path) = watcher.next_settled(wait) {
            reported.push(path.file_name().unwrap().to_owned());
            wait = Duration::from_secs(1);
        }
        watcher.stop();

        assert!(reported.iter().any(|name| name == "real.pdf"));
        assert!(
            reported.iter().all(|name| name != "link.pdf"),
            "a link was reported as a file"
        );
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

    #[test]
    fn an_empty_placeholder_waits_until_its_download_finishes() {
        let folder = TempDir::new().unwrap();
        let watcher = FolderWatcher::start(folder.path(), Duration::from_millis(200)).unwrap();
        let placeholder = folder.path().join("report.pdf");
        let partial = folder.path().join("report.Ab12Cd34.pdf.part");
        fs::write(&placeholder, b"").unwrap();
        fs::write(&partial, b"half of it").unwrap();

        let too_early = watcher.next_settled(Duration::from_secs(2));
        fs::write(&partial, b"all of the report").unwrap();
        fs::rename(&partial, &placeholder).unwrap();
        let finished = watcher.next_settled(Duration::from_secs(10));
        watcher.stop();

        assert!(
            too_early.is_none(),
            "the empty name Firefox reserves was tidied before the download finished"
        );
        let finished = finished.expect("the finished download was never reported");
        assert_eq!(finished.file_name().unwrap(), "report.pdf");
    }
}
