use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::executor::{Executor, Outcome};
use crate::journal::{Journal, JournalError, Operation};
use crate::paths::{self, PathError};
use crate::rules::{EvaluationError, PlanContext, Rule, plan};
use crate::watcher::is_temporary;

const MAX_SCAN_DEPTH: usize = 16;

static BATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("the folder cannot be used")]
    Folder(#[from] PathError),
    #[error("the rules could not be evaluated")]
    Rules(#[from] EvaluationError),
    #[error("the history could not be reached")]
    Journal(#[from] JournalError),
    #[error("the operation could not be carried out")]
    Executor(#[from] crate::executor::ExecutorError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedChange {
    pub kind: String,
    pub source: String,
    pub destination: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub batch: String,
    pub done: usize,
    pub undone: usize,
    pub failed: usize,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub batch: String,
    pub applied: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub struct Tidyfile {
    executor: Executor,
}

impl Tidyfile {
    pub fn open(journal: &Path) -> Result<Self, ServiceError> {
        Ok(Self {
            executor: Executor::new(Journal::open(journal)?),
        })
    }

    pub fn in_memory() -> Result<Self, ServiceError> {
        Ok(Self {
            executor: Executor::new(Journal::open_in_memory()?),
        })
    }

    pub fn simulate(
        &self,
        rules: &[Rule],
        folder: &Path,
    ) -> Result<Vec<PlannedChange>, ServiceError> {
        Ok(self
            .plan_folder(rules, folder)?
            .iter()
            .map(describe)
            .collect())
    }

    pub fn organize(&self, rules: &[Rule], folder: &Path) -> Result<BatchReport, ServiceError> {
        let operations = self.plan_folder(rules, folder)?;
        let batch = next_batch_id();
        let outcomes = self.executor.apply(&batch, &operations)?;
        Ok(summarize(batch, &outcomes))
    }

    pub fn undo(&self, batch: &str) -> Result<BatchReport, ServiceError> {
        let outcomes = self.executor.undo_batch(batch)?;
        Ok(summarize(batch.to_owned(), &outcomes))
    }

    pub fn interrupted(&self) -> Result<Vec<PlannedChange>, ServiceError> {
        Ok(self
            .executor
            .journal()
            .interrupted()?
            .iter()
            .map(|record| describe(&record.operation))
            .collect())
    }

    pub fn activity(&self, limit: usize) -> Result<Vec<ActivityEntry>, ServiceError> {
        Ok(self
            .executor
            .journal()
            .recent_batches(limit)?
            .into_iter()
            .map(|summary| ActivityEntry {
                batch: summary.batch,
                done: summary.done,
                undone: summary.undone,
                failed: summary.failed,
                recorded_at: summary.recorded_at,
            })
            .collect())
    }

    fn plan_folder(&self, rules: &[Rule], folder: &Path) -> Result<Vec<Operation>, ServiceError> {
        let root = paths::accept_watched_folder(folder)?;
        let now = SystemTime::now();
        let mut operations = Vec::new();
        for (index, file) in scan(&root).into_iter().enumerate() {
            let Ok(facts) = crate::rules::FileFacts::gather(&file, &root) else {
                continue;
            };
            let context = PlanContext {
                now,
                counter: u32::try_from(index + 1).unwrap_or(u32::MAX),
            };
            operations.extend(plan(rules, &facts, context)?);
        }
        Ok(operations)
    }
}

fn scan(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(root, 0, &mut found);
    found.sort();
    found
}

fn collect(folder: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|data| data.is_symlink()) {
            continue;
        }
        if path.is_dir() {
            collect(&path, depth + 1, found);
        } else if path.is_file() && !is_temporary(&path) {
            found.push(path);
        }
    }
}

fn describe(operation: &Operation) -> PlannedChange {
    let (kind, destination) = match operation {
        Operation::Move { to, .. } => ("move", Some(show(to))),
        Operation::Copy { to, .. } => ("copy", Some(show(to))),
        Operation::Trash { .. } => ("trash", None),
    };
    PlannedChange {
        kind: kind.to_owned(),
        source: show(operation.source()),
        destination,
    }
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn summarize(batch: String, outcomes: &[Outcome]) -> BatchReport {
    BatchReport {
        batch,
        applied: count(outcomes, |outcome| matches!(outcome, Outcome::Applied(_))),
        skipped: count(outcomes, |outcome| matches!(outcome, Outcome::Skipped(_))),
        failed: count(outcomes, |outcome| matches!(outcome, Outcome::Failed(_))),
    }
}

fn count(outcomes: &[Outcome], predicate: impl Fn(&Outcome) -> bool) -> usize {
    outcomes.iter().filter(|outcome| predicate(outcome)).count()
}

fn next_batch_id() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let sequence = BATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{seconds}-{sequence}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::rules::{Action, Combinator, Condition};
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn pdf_rule(destination: &Path) -> Rule {
        Rule {
            id: "id-1".into(),
            name: "pdfs".into(),
            enabled: true,
            combinator: Combinator::All,
            conditions: vec![Condition::Extension {
                any_of: vec!["pdf".into()],
            }],
            actions: vec![Action::MoveTo {
                folder: destination.to_path_buf(),
                subfolder: None,
                rename: None,
            }],
        }
    }

    #[test]
    fn simulation_reports_what_would_happen_without_touching_anything() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        let source = root.path().join("invoice.pdf");
        write(&source, "content");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], root.path())
            .unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].kind, "move");
        assert!(source.exists(), "simulation must not move anything");
        assert!(service.interrupted().unwrap().is_empty());
    }

    #[test]
    fn organizing_applies_exactly_what_the_simulation_promised() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        write(&root.path().join("invoice.pdf"), "content");
        write(&root.path().join("notes.txt"), "content");
        let rules = [pdf_rule(out.path())];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service.simulate(&rules, root.path()).unwrap();
        let report = service.organize(&rules, root.path()).unwrap();

        assert_eq!(report.applied, planned.len());
        assert_eq!(report.failed, 0);
        assert!(out.path().join("invoice.pdf").exists());
        assert!(root.path().join("notes.txt").exists());
    }

    #[test]
    fn organizing_can_be_undone_as_a_batch() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        let source = root.path().join("invoice.pdf");
        write(&source, "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize(&[pdf_rule(out.path())], root.path())
            .unwrap();
        service.undo(&report.batch).unwrap();

        assert!(source.exists(), "undo did not bring the file back");
        assert!(!out.path().join("invoice.pdf").exists());
    }

    #[test]
    fn the_scan_reaches_subfolders() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        write(&root.path().join("deep/deeper/invoice.pdf"), "content");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], root.path())
            .unwrap();

        assert_eq!(planned.len(), 1);
    }

    #[test]
    fn the_scan_ignores_download_temporaries() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        write(&root.path().join("invoice.pdf.crdownload"), "partial");
        write(&root.path().join("~$invoice.pdf"), "lock");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], root.path())
            .unwrap();

        assert!(planned.is_empty(), "temporaries leaked into the plan");
    }

    #[test]
    fn a_forbidden_folder_is_refused_before_any_scanning() {
        let service = Tidyfile::in_memory().unwrap();
        let root = if cfg!(windows) { r"C:\Windows" } else { "/usr" };

        let outcome = service.simulate(&[], Path::new(root));

        assert!(matches!(
            outcome,
            Err(ServiceError::Folder(PathError::Forbidden))
        ));
    }

    #[test]
    fn a_missing_folder_is_refused() {
        let root = TempDir::new().unwrap();
        let service = Tidyfile::in_memory().unwrap();

        let outcome = service.simulate(&[], &root.path().join("ghost"));

        assert!(matches!(
            outcome,
            Err(ServiceError::Folder(PathError::Unresolvable))
        ));
    }

    #[test]
    fn batch_identifiers_do_not_repeat() {
        let first = next_batch_id();
        let second = next_batch_id();

        assert_ne!(first, second);
    }

    #[test]
    fn nothing_matching_produces_an_empty_report() {
        let root = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        write(&root.path().join("notes.txt"), "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize(&[pdf_rule(out.path())], root.path())
            .unwrap();

        assert_eq!((report.applied, report.skipped, report.failed), (0, 0, 0));
    }
}
