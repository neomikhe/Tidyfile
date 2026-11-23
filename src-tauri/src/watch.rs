use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::ipc::current_collision;
use crate::paths;
use crate::rules::Rule;
use crate::service::{Arrival, BatchReport, ServiceError, Tidyfile, Watched};
use crate::settings::Settings;
use crate::store;
use crate::watcher::FolderWatcher;

const QUIET_PERIOD: Duration = Duration::from_millis(1_500);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub const TIDIED_EVENT: &str = "tidied";
pub const LOOP_STOPPED_EVENT: &str = "loopStopped";

pub struct WatchSession {
    requested: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

// WHY: the interface matches folders as the user wrote them, not in their resolved verbatim form
impl WatchSession {
    pub fn folder(&self) -> &Path {
        &self.requested
    }
}

impl Drop for WatchSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn resume(
    app: &AppHandle,
    service: Arc<Mutex<Tidyfile>>,
    rules_file: &Path,
    settings_file: &Path,
) -> Vec<WatchSession> {
    let settings: Settings = store::load(settings_file).unwrap_or_default();
    settings
        .watched
        .iter()
        .filter_map(|folder| {
            start(
                app.clone(),
                service.clone(),
                rules_file.to_path_buf(),
                settings_file.to_path_buf(),
                folder,
            )
            .ok()
        })
        .collect()
}

pub fn start(
    app: AppHandle,
    service: Arc<Mutex<Tidyfile>>,
    rules_file: PathBuf,
    settings_file: PathBuf,
    folder: &Path,
) -> Result<WatchSession, ServiceError> {
    let root = paths::accept_watched_folder(folder)?;
    let watcher = FolderWatcher::start(&root, QUIET_PERIOD)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let watched = root.clone();

    let worker = std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            if let Some(file) = watcher.next_settled(POLL_INTERVAL) {
                tidy_one(&app, &service, &rules_file, &settings_file, &watched, &file);
            }
        }
        watcher.stop();
    });

    Ok(WatchSession {
        requested: folder.to_path_buf(),
        stop,
        worker: Some(worker),
    })
}

fn tidy_one(
    app: &AppHandle,
    service: &Mutex<Tidyfile>,
    rules_file: &Path,
    settings_file: &Path,
    root: &Path,
    file: &Path,
) {
    let Ok(rules) = store::load::<Vec<Rule>>(rules_file) else {
        return;
    };
    let enabled: Vec<Rule> = rules.into_iter().filter(|rule| rule.enabled).collect();
    if enabled.is_empty() {
        return;
    }
    let collision = current_collision(settings_file);
    let watched = {
        let Ok(mut tidyfile) = service.lock() else {
            return;
        };
        tidyfile.organize_file(&enabled, Arrival { root, file }, collision)
    };
    if let Ok(watched) = watched {
        announce(app, watched);
    }
}

fn announce(app: &AppHandle, watched: Watched) {
    let _ = match watched {
        Watched::Tidied(report) if worth_announcing(&report) => app.emit(TIDIED_EVENT, report),
        Watched::LoopStopped(stopped) => app.emit(LOOP_STOPPED_EVENT, stopped),
        Watched::Tidied(_) | Watched::Held => Ok(()),
    };
}

fn worth_announcing(report: &BatchReport) -> bool {
    report.applied > 0 || report.failed > 0 || report.skipped > 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_watched_folder_is_reported_as_the_user_wrote_it() {
        let written = Path::new("Downloads/");
        let session = WatchSession {
            requested: written.to_path_buf(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        };

        assert_eq!(session.folder(), written);
    }

    fn report(applied: usize, skipped: usize, failed: usize) -> BatchReport {
        BatchReport {
            batch: "b1".into(),
            applied,
            skipped,
            failed,
            needs_manual_restore: 0,
            not_in_trash: 0,
            already_gone: 0,
            not_recycled: 0,
        }
    }

    #[test]
    fn work_that_moved_a_file_is_announced() {
        assert!(worth_announcing(&report(1, 0, 0)));
    }

    #[test]
    fn work_that_failed_is_announced() {
        assert!(worth_announcing(&report(0, 0, 1)));
    }

    #[test]
    fn a_conflict_left_waiting_is_announced_too() {
        assert!(
            worth_announcing(&report(0, 1, 0)),
            "with the ask policy nothing is applied, so a silent skip would look like a dead watcher"
        );
    }

    #[test]
    fn a_file_no_rule_cared_about_stays_quiet() {
        assert!(!worth_announcing(&report(0, 0, 0)));
    }
}
