use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::executor::Collision;
use crate::paths::PathError;
use crate::rules::{Condition, FileFacts, Rule};
use crate::service::{
    ActivityEntry, BatchReport, FolderStatus, PlannedChange, RecordedChange, ServiceError, Tidyfile,
};
use crate::settings::Settings;
use crate::store::{self, StoreError};
use crate::watch::{self, WatchSession};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl IpcError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }

    fn unavailable() -> Self {
        Self::new("unavailable", "Tidyfile is busy. Try again in a moment.")
    }
}

impl From<StoreError> for IpcError {
    fn from(error: StoreError) -> Self {
        let code = match error {
            StoreError::Unreachable(_) => "rulesUnreachable",
            StoreError::Malformed(_) => "rulesMalformed",
        };
        Self::new(code, &error.to_string())
    }
}

impl From<ServiceError> for IpcError {
    fn from(error: ServiceError) -> Self {
        Self::new(code_for(&error), &error.to_string())
    }
}

fn code_for(error: &ServiceError) -> &'static str {
    match error {
        ServiceError::Folder(PathError::Forbidden) => "forbiddenFolder",
        ServiceError::Folder(PathError::NotAFolder) => "notAFolder",
        ServiceError::Folder(PathError::Unresolvable) => "folderNotFound",
        ServiceError::Rules(_) => "invalidRule",
        ServiceError::Journal(_) => "historyUnavailable",
        ServiceError::Executor(_) => "executionFailed",
        ServiceError::Watcher(_) => "watchFailed",
    }
}

pub struct AppState {
    service: Arc<Mutex<Tidyfile>>,
    rules_file: PathBuf,
    settings_file: PathBuf,
    sessions: Mutex<Vec<WatchSession>>,
}

impl AppState {
    pub fn open(folder: &Path) -> Result<Self, ServiceError> {
        Ok(Self {
            service: Arc::new(Mutex::new(Tidyfile::open(&folder.join("journal.sqlite"))?)),
            rules_file: folder.join("rules.json"),
            settings_file: folder.join("settings.json"),
            sessions: Mutex::new(Vec::new()),
        })
    }
}

#[tauri::command]
pub fn start_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    folders: Vec<String>,
) -> Result<(), IpcError> {
    let mut started = Vec::with_capacity(folders.len());
    for folder in &folders {
        started.push(watch::start(
            app.clone(),
            state.service.clone(),
            state.rules_file.clone(),
            state.settings_file.clone(),
            Path::new(folder),
        )?);
    }
    let mut running = state.sessions.lock().map_err(|_| IpcError::unavailable())?;
    *running = started;
    Ok(())
}

#[tauri::command]
pub fn stop_watching(state: State<'_, AppState>) -> Result<(), IpcError> {
    let mut running = state.sessions.lock().map_err(|_| IpcError::unavailable())?;
    running.clear();
    Ok(())
}

#[tauri::command]
pub fn watched_folders(state: State<'_, AppState>) -> Result<Vec<String>, IpcError> {
    let running = state.sessions.lock().map_err(|_| IpcError::unavailable())?;
    Ok(running
        .iter()
        .map(|session| session.folder().to_string_lossy().into_owned())
        .collect())
}

#[tauri::command]
pub async fn simulate(
    state: State<'_, AppState>,
    rules: Vec<Rule>,
    folders: Vec<String>,
) -> Result<Vec<PlannedChange>, IpcError> {
    let service = state.service.clone();
    off_thread(move || {
        let roots = into_paths(folders);
        with(&service, |tidyfile| tidyfile.simulate(&rules, &roots))
    })
    .await
}

#[tauri::command]
pub async fn organize(
    state: State<'_, AppState>,
    rules: Vec<Rule>,
    folders: Vec<String>,
) -> Result<BatchReport, IpcError> {
    let service = state.service.clone();
    let collision = current_collision(&state.settings_file);
    off_thread(move || {
        let roots = into_paths(folders);
        with(&service, |tidyfile| {
            tidyfile.organize(&rules, &roots, collision)
        })
    })
    .await
}

#[tauri::command]
pub async fn undo(state: State<'_, AppState>, batch: String) -> Result<BatchReport, IpcError> {
    let service = state.service.clone();
    off_thread(move || with(&service, |tidyfile| tidyfile.undo(&batch))).await
}

#[tauri::command]
pub async fn interrupted(state: State<'_, AppState>) -> Result<Vec<PlannedChange>, IpcError> {
    let service = state.service.clone();
    off_thread(move || with(&service, Tidyfile::interrupted)).await
}

#[tauri::command]
pub async fn folder_status(
    state: State<'_, AppState>,
    folders: Vec<String>,
) -> Result<Vec<FolderStatus>, IpcError> {
    let service = state.service.clone();
    let paths: Vec<PathBuf> = folders.into_iter().map(PathBuf::from).collect();
    off_thread(move || {
        let guard = service.lock().map_err(|_| IpcError::unavailable())?;
        Ok(guard.folder_status(&paths))
    })
    .await
}

#[tauri::command]
pub async fn check_pattern(kind: String, pattern: String) -> Result<(), IpcError> {
    off_thread(move || {
        let outcome = match kind.as_str() {
            "glob" => Condition::NameMatchesGlob { pattern },
            _ => Condition::NameMatchesRegex { pattern },
        }
        .matches(&nothing_in_particular(), SystemTime::UNIX_EPOCH);
        outcome
            .map(|_| ())
            .map_err(|error| IpcError::new("invalidPattern", &error.to_string()))
    })
    .await
}

fn nothing_in_particular() -> FileFacts {
    FileFacts {
        path: PathBuf::from("sample.txt"),
        root: PathBuf::from(""),
        size: 0,
        modified: SystemTime::UNIX_EPOCH,
    }
}

#[tauri::command]
pub async fn resolve_conflicts(
    state: State<'_, AppState>,
    batch: String,
    keep_both: bool,
) -> Result<BatchReport, IpcError> {
    let service = state.service.clone();
    let choice = if keep_both {
        Collision::Suffix
    } else {
        Collision::Skip
    };
    off_thread(move || {
        with(&service, |tidyfile| {
            tidyfile.resolve_conflicts(&batch, choice)
        })
    })
    .await
}

#[tauri::command]
pub async fn undo_operation(state: State<'_, AppState>, id: i64) -> Result<BatchReport, IpcError> {
    let service = state.service.clone();
    off_thread(move || with(&service, |tidyfile| tidyfile.undo_operation(id))).await
}

#[tauri::command]
pub async fn operations(
    state: State<'_, AppState>,
    batch: String,
) -> Result<Vec<RecordedChange>, IpcError> {
    let service = state.service.clone();
    off_thread(move || with(&service, |tidyfile| tidyfile.operations(&batch))).await
}

#[tauri::command]
pub async fn settle_interrupted(state: State<'_, AppState>) -> Result<usize, IpcError> {
    let service = state.service.clone();
    off_thread(move || with(&service, Tidyfile::settle_interrupted)).await
}

const MAX_ACTIVITY: usize = 500;

#[tauri::command]
pub async fn load_rules(state: State<'_, AppState>) -> Result<Vec<Rule>, IpcError> {
    let path = state.rules_file.clone();
    off_thread(move || store::load(&path).map_err(IpcError::from)).await
}

#[tauri::command]
pub async fn save_rules(state: State<'_, AppState>, rules: Vec<Rule>) -> Result<(), IpcError> {
    let path = state.rules_file.clone();
    off_thread(move || store::save(&path, &rules).map_err(IpcError::from)).await
}

#[tauri::command]
pub async fn activity(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<Vec<ActivityEntry>, IpcError> {
    let service = state.service.clone();
    let capped = limit.clamp(1, MAX_ACTIVITY);
    off_thread(move || with(&service, |tidyfile| tidyfile.activity(capped))).await
}

pub fn current_collision(settings_file: &Path) -> Collision {
    store::load::<Settings>(settings_file)
        .map(|settings| settings.on_collision)
        .unwrap_or_default()
}

#[tauri::command]
pub async fn load_settings(state: State<'_, AppState>) -> Result<Settings, IpcError> {
    let path = state.settings_file.clone();
    off_thread(move || store::load::<Settings>(&path).map_err(IpcError::from)).await
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), IpcError> {
    let path = state.settings_file.clone();
    off_thread(move || store::save(&path, &settings).map_err(IpcError::from)).await
}

fn into_paths(folders: Vec<String>) -> Vec<PathBuf> {
    folders.into_iter().map(PathBuf::from).collect()
}

fn with<T>(
    service: &Mutex<Tidyfile>,
    work: impl FnOnce(&Tidyfile) -> Result<T, ServiceError>,
) -> Result<T, IpcError> {
    let guard = service.lock().map_err(|_| IpcError::unavailable())?;
    work(&guard).map_err(IpcError::from)
}

async fn off_thread<T>(
    work: impl FnOnce() -> Result<T, IpcError> + Send + 'static,
) -> Result<T, IpcError>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| Err(IpcError::unavailable()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_forbidden_folder_maps_to_a_stable_code() {
        let error = IpcError::from(ServiceError::Folder(PathError::Forbidden));

        assert_eq!(error.code, "forbiddenFolder");
    }

    #[test]
    fn every_folder_problem_has_its_own_code() {
        let codes = [
            PathError::Forbidden,
            PathError::NotAFolder,
            PathError::Unresolvable,
        ]
        .map(|problem| IpcError::from(ServiceError::Folder(problem)).code);

        let mut unique = codes.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "codes collide: {codes:?}");
    }

    #[test]
    fn messages_crossing_the_boundary_carry_no_paths() {
        let error = IpcError::from(ServiceError::Folder(PathError::Unresolvable));

        assert!(!error.message.contains('/'));
        assert!(!error.message.contains('\\'));
    }

    #[test]
    fn a_valid_glob_passes_the_check() {
        let condition = Condition::NameMatchesGlob {
            pattern: "Screenshot*.png".into(),
        };
        assert!(
            condition
                .matches(&nothing_in_particular(), SystemTime::UNIX_EPOCH)
                .is_ok()
        );
    }

    #[test]
    fn an_invalid_regex_is_caught_by_the_same_compiler_that_runs_it() {
        let condition = Condition::NameMatchesRegex {
            pattern: "(unclosed".into(),
        };
        assert!(
            condition
                .matches(&nothing_in_particular(), SystemTime::UNIX_EPOCH)
                .is_err()
        );
    }

    #[test]
    fn an_oversized_pattern_is_caught_before_it_is_saved() {
        let condition = Condition::NameMatchesRegex {
            pattern: "a".repeat(1_000),
        };
        assert!(
            condition
                .matches(&nothing_in_particular(), SystemTime::UNIX_EPOCH)
                .is_err()
        );
    }
}
