use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::ipc::current_collision;
use crate::paths;
use crate::rules::Rule;
use crate::service::{BatchReport, ServiceError, Tidyfile};
use crate::settings::Settings;
use crate::store;
use crate::watcher::FolderWatcher;

const QUIET_PERIOD: Duration = Duration::from_millis(1_500);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub const TIDIED_EVENT: &str = "tidied";

pub struct WatchSession {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl WatchSession {
    pub fn folder(&self) -> &Path {
        &self.root
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
        root,
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
    let Ok(tidyfile) = service.lock() else {
        return;
    };
    let collision = current_collision(settings_file);
    if let Ok(report) = tidyfile.organize_file(&enabled, root, file, collision) {
        announce(app, report);
    }
}

fn announce(app: &AppHandle, report: BatchReport) {
    if !worth_announcing(&report) {
        return;
    }
    let _ = app.emit(TIDIED_EVENT, report);
}

fn worth_announcing(report: &BatchReport) -> bool {
    report.applied > 0 || report.failed > 0 || report.skipped > 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn report(applied: usize, skipped: usize, failed: usize) -> BatchReport {
        BatchReport {
            batch: "b1".into(),
            applied,
            skipped,
            failed,
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
