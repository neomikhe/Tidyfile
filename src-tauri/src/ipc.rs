use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

use crate::paths::PathError;
use crate::rules::Rule;
use crate::service::{BatchReport, PlannedChange, ServiceError, Tidyfile};

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
    }
}

pub struct AppState {
    service: Arc<Mutex<Tidyfile>>,
}

impl AppState {
    pub fn open(journal: &Path) -> Result<Self, ServiceError> {
        Ok(Self {
            service: Arc::new(Mutex::new(Tidyfile::open(journal)?)),
        })
    }
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
