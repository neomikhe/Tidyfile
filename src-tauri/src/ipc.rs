use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::paths::PathError;
use crate::rules::Rule;
use crate::service::{ActivityEntry, BatchReport, PlannedChange, ServiceError, Tidyfile};
use crate::store::{self, StoreError};
use crate::watch::{self, WatchSession};

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
    session: Mutex<Option<WatchSession>>,
}

impl AppState {
    pub fn open(folder: &Path) -> Result<Self, ServiceError> {
        Ok(Self {
            service: Arc::new(Mutex::new(Tidyfile::open(&folder.join("journal.sqlite"))?)),
            rules_file: folder.join("rules.json"),
            session: Mutex::new(None),
        })
    }
}

#[tauri::command]
pub fn start_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
) -> Result<(), IpcError> {
    let started = watch::start(
        app,
        state.service.clone(),
        state.rules_file.clone(),
        Path::new(&folder),
    )?;
    let mut slot = state.session.lock().map_err(|_| IpcError::unavailable())?;
    if let Some(previous) = slot.take() {
        previous.halt();
    }
    *slot = Some(started);
    Ok(())
}

#[tauri::command]
pub fn stop_watching(state: State<'_, AppState>) -> Result<(), IpcError> {
    let mut slot = state.session.lock().map_err(|_| IpcError::unavailable())?;
    if let Some(session) = slot.take() {
        session.halt();
    }
    Ok(())
}

#[tauri::command]
pub fn watched_folder(state: State<'_, AppState>) -> Result<Option<String>, IpcError> {
    let slot = state.session.lock().map_err(|_| IpcError::unavailable())?;
    Ok(slot
        .as_ref()
        .map(|session| session.folder().to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn simulate(
    state: State<'_, AppState>,
    rules: Vec<Rule>,
    folder: String,
) -> Result<Vec<PlannedChange>, IpcError> {
    let service = state.service.clone();
    off_thread(move || {
        with(&service, |tidyfile| {
            tidyfile.simulate(&rules, &into_path(folder))
        })
    })
    .await
}

#[tauri::command]
pub async fn organize(
    state: State<'_, AppState>,
    rules: Vec<Rule>,
    folder: String,
) -> Result<BatchReport, IpcError> {
    let service = state.service.clone();
    off_thread(move || {
        with(&service, |tidyfile| {
            tidyfile.organize(&rules, &into_path(folder))
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

fn into_path(folder: String) -> PathBuf {
    PathBuf::from(folder)
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
}
