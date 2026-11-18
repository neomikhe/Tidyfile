use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::executor::{Collision, Executor, Outcome, SkipReason};
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
    #[error("the folder could not be watched")]
    Watcher(#[from] crate::watcher::WatcherError),
    #[error("the folder changed since the preview")]
    PlanChanged,
}

pub struct Shown<'a> {
    pub changes: &'a [PlannedChange],
    pub on_collision: Collision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub skipped: usize,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub folder: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub batch: String,
    pub applied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub needs_manual_restore: usize,
    pub not_in_trash: usize,
    pub already_gone: usize,
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
        folders: &[PathBuf],
    ) -> Result<Vec<PlannedChange>, ServiceError> {
        Ok(self
            .plan_folders(rules, folders)?
            .iter()
            .map(describe)
            .collect())
    }

    pub fn organize(
        &self,
        rules: &[Rule],
        folders: &[PathBuf],
        on_collision: Collision,
    ) -> Result<BatchReport, ServiceError> {
        let operations = self.plan_folders(rules, folders)?;
        self.apply_batch(&operations, on_collision)
    }

    // WHY: comparing under the same lock that applies leaves no gap for the disk to change unseen
    pub fn organize_as_shown(
        &self,
        rules: &[Rule],
        folders: &[PathBuf],
        shown: Shown<'_>,
    ) -> Result<BatchReport, ServiceError> {
        let operations = self.plan_folders(rules, folders)?;
        if !operations
            .iter()
            .map(describe)
            .eq(shown.changes.iter().cloned())
        {
            return Err(ServiceError::PlanChanged);
        }
        self.apply_batch(&operations, shown.on_collision)
    }

    fn apply_batch(
        &self,
        operations: &[Operation],
        on_collision: Collision,
    ) -> Result<BatchReport, ServiceError> {
        let batch = next_batch_id();
        let outcomes = self.executor.apply(&batch, operations, on_collision)?;
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

    pub fn settle_interrupted(&self) -> Result<usize, ServiceError> {
        Ok(self
            .executor
            .journal()
            .settle_interrupted("interrupted run")?)
    }

    pub fn organize_file(
        &self,
        rules: &[Rule],
        root: &Path,
        file: &Path,
        on_collision: Collision,
    ) -> Result<BatchReport, ServiceError> {
        if !is_really_inside(root, file) {
            return Ok(nothing_happened());
        }
        let Ok(facts) = crate::rules::FileFacts::gather(file, root) else {
            return Ok(nothing_happened());
        };
        let context = PlanContext {
            now: SystemTime::now(),
            counter: 1,
        };
        let operations = plan(rules, &facts, context)?;
        if operations.is_empty() {
            return Ok(nothing_happened());
        }
        self.apply_batch(&operations, on_collision)
    }

    pub fn resolve_conflicts(
        &self,
        batch: &str,
        on_collision: Collision,
    ) -> Result<BatchReport, ServiceError> {
        let outcomes = self.executor.retry_skipped(batch, on_collision)?;
        Ok(summarize(batch.to_owned(), &outcomes))
    }

    pub fn undo_operation(&self, id: i64) -> Result<BatchReport, ServiceError> {
        let outcome = self.executor.undo_operation(id)?;
        Ok(summarize(String::new(), std::slice::from_ref(&outcome)))
    }

    pub fn operations(&self, batch: &str) -> Result<Vec<RecordedChange>, ServiceError> {
        Ok(self
            .executor
            .journal()
            .operations_in_batch(batch)?
            .iter()
            .map(recorded)
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
                skipped: summary.skipped,
                recorded_at: summary.recorded_at,
            })
            .collect())
    }

    fn plan_folders(
        &self,
        rules: &[Rule],
        folders: &[PathBuf],
    ) -> Result<Vec<Operation>, ServiceError> {
        let now = SystemTime::now();
        let mut operations = Vec::new();
        for (index, (root, file)) in files_in(folders)?.into_iter().enumerate() {
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

// WHY: a watcher event can name a file through a link that leads out of the watched folder
fn is_really_inside(root: &Path, file: &Path) -> bool {
    if is_link(root) || is_link(file) {
        return false;
    }
    let real_parent = file.parent().and_then(|parent| parent.canonicalize().ok());
    match (real_parent, root.canonicalize()) {
        (Some(parent), Ok(root)) => parent.starts_with(root),
        _ => false,
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).map_or(true, |data| data.is_symlink())
}

pub fn statuses(folders: &[PathBuf]) -> Vec<FolderStatus> {
    folders
        .iter()
        .map(PathBuf::as_path)
        .map(status_of)
        .collect()
}

fn status_of(folder: &Path) -> FolderStatus {
    let state = match paths::accept_watched_folder(folder) {
        Ok(_) => "ok",
        Err(PathError::Forbidden) => "forbidden",
        Err(PathError::Unresolvable | PathError::NotAFolder) => "unavailable",
    };
    FolderStatus {
        folder: folder.to_string_lossy().into_owned(),
        state: state.to_owned(),
    }
}

fn files_in(folders: &[PathBuf]) -> Result<Vec<(PathBuf, PathBuf)>, ServiceError> {
    let mut found = Vec::new();
    for folder in folders {
        let root = match paths::accept_watched_folder(folder) {
            Ok(root) => root,
            Err(PathError::Unresolvable | PathError::NotAFolder) => continue,
            Err(refused) => return Err(refused.into()),
        };
        let inside = scan(&root)
            .into_iter()
            .filter(|file| paths::is_within(&root, file))
            .map(|file| (root.clone(), file));
        found.extend(inside);
    }
    Ok(found)
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
        visit(&entry, depth, found);
    }
}

// PERF: the listing already knows each entry's type; asking the disk again cost three opens per file
fn visit(entry: &fs::DirEntry, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(kind) = entry.file_type() else {
        return;
    };
    let path = entry.path();
    if kind.is_symlink() {
        return;
    }
    if kind.is_dir() {
        collect(&path, depth + 1, found);
    } else if kind.is_file() && !is_temporary(&path) {
        found.push(path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedChange {
    pub id: i64,
    pub kind: String,
    pub source: String,
    pub destination: Option<String>,
    pub state: String,
    pub undoable: bool,
}

fn recorded(record: &crate::journal::RecordedOperation) -> RecordedChange {
    let change = describe(&record.operation);
    RecordedChange {
        id: record.id,
        kind: change.kind,
        source: change.source,
        destination: change.destination,
        state: format!("{:?}", record.state).to_lowercase(),
        undoable: record.state == crate::journal::State::Done,
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
        needs_manual_restore: count(outcomes, |outcome| {
            matches!(outcome, Outcome::Skipped(SkipReason::RestoreUnsupported))
        }),
        not_in_trash: count(outcomes, |outcome| {
            matches!(outcome, Outcome::Skipped(SkipReason::NotFoundInTrash))
        }),
        already_gone: count(outcomes, |outcome| {
            matches!(outcome, Outcome::Skipped(SkipReason::SourceVanished))
        }),
    }
}

fn count(outcomes: &[Outcome], predicate: impl Fn(&Outcome) -> bool) -> usize {
    outcomes.iter().filter(|outcome| predicate(outcome)).count()
}

fn nothing_happened() -> BatchReport {
    BatchReport {
        batch: String::new(),
        applied: 0,
        skipped: 0,
        failed: 0,
        needs_manual_restore: 0,
        not_in_trash: 0,
        already_gone: 0,
    }
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
    use crate::testing::watchable_folder;

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
    fn a_trashed_file_comes_back_when_its_batch_is_undone() {
        let root = watchable_folder();
        let source = root.path().join("old-installer.pdf");
        write(&source, "the original content");
        let mut trashing = pdf_rule(Path::new("/unused"));
        trashing.actions = vec![Action::Trash];
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize(&[trashing], &[root.path().to_path_buf()], Collision::Suffix)
            .unwrap();
        assert_eq!(report.applied, 1);
        assert!(!source.exists());
        let undone = service.undo(&report.batch).unwrap();

        if cfg!(target_os = "macos") {
            assert_eq!(undone.needs_manual_restore, 1);
        } else {
            assert_eq!(undone.applied, 1);
            assert_eq!(
                fs::read_to_string(&source).unwrap(),
                "the original content",
                "undo did not bring the same file back from the trash"
            );
        }
    }

    #[test]
    fn a_file_no_longer_in_the_trash_is_counted_so_it_can_be_announced() {
        let outcomes = [
            Outcome::Skipped(SkipReason::NotFoundInTrash),
            Outcome::Skipped(SkipReason::NothingToUndo),
            Outcome::Skipped(SkipReason::SourceVanished),
            Outcome::Failed("a file with that name is already there".into()),
        ];

        let report = summarize("b1".into(), &outcomes);

        assert_eq!(
            (
                report.not_in_trash,
                report.already_gone,
                report.skipped,
                report.failed
            ),
            (1, 1, 3, 1)
        );
    }

    #[test]
    fn undoing_a_move_whose_file_was_deleted_since_reports_it_as_gone() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        let service = Tidyfile::in_memory().unwrap();
        let report = service
            .organize(
                &[pdf_rule(out.path())],
                &[root.path().to_path_buf()],
                Collision::Suffix,
            )
            .unwrap();
        fs::rename(
            out.path().join("invoice.pdf"),
            out.path().join("renamed-by-the-user.pdf"),
        )
        .unwrap();

        let undone = service.undo(&report.batch).unwrap();

        assert_eq!((undone.already_gone, undone.applied), (1, 0));
    }

    #[test]
    fn organizing_as_shown_applies_when_nothing_changed() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();
        let shown = service.simulate(&rules, &folders).unwrap();

        let report = service
            .organize_as_shown(
                &rules,
                &folders,
                Shown {
                    changes: &shown,
                    on_collision: Collision::Suffix,
                },
            )
            .unwrap();

        assert_eq!(report.applied, 1);
    }

    #[test]
    fn a_folder_that_changed_since_the_preview_is_left_untouched() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();
        let shown = service.simulate(&rules, &folders).unwrap();
        write(&root.path().join("arrived-later.pdf"), "content");

        let outcome = service.organize_as_shown(
            &rules,
            &folders,
            Shown {
                changes: &shown,
                on_collision: Collision::Suffix,
            },
        );

        assert!(matches!(outcome, Err(ServiceError::PlanChanged)));
        assert!(root.path().join("invoice.pdf").exists());
        assert!(root.path().join("arrived-later.pdf").exists());
        assert!(
            service.activity(10).unwrap().is_empty(),
            "nothing was journaled"
        );
    }

    #[test]
    fn simulation_reports_what_would_happen_without_touching_anything() {
        let root = watchable_folder();
        let out = watchable_folder();
        let source = root.path().join("invoice.pdf");
        write(&source, "content");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], &[root.path().to_path_buf()])
            .unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].kind, "move");
        assert!(source.exists(), "simulation must not move anything");
        assert!(service.interrupted().unwrap().is_empty());
    }

    #[test]
    fn organizing_applies_exactly_what_the_simulation_promised() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        write(&root.path().join("notes.txt"), "content");
        let rules = [pdf_rule(out.path())];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&rules, &[root.path().to_path_buf()])
            .unwrap();
        let report = service
            .organize(&rules, &[root.path().to_path_buf()], Collision::Suffix)
            .unwrap();

        assert_eq!(report.applied, planned.len());
        assert_eq!(report.failed, 0);
        assert!(out.path().join("invoice.pdf").exists());
        assert!(root.path().join("notes.txt").exists());
    }

    #[test]
    fn organizing_can_be_undone_as_a_batch() {
        let root = watchable_folder();
        let out = watchable_folder();
        let source = root.path().join("invoice.pdf");
        write(&source, "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize(
                &[pdf_rule(out.path())],
                &[root.path().to_path_buf()],
                Collision::Suffix,
            )
            .unwrap();
        service.undo(&report.batch).unwrap();

        assert!(source.exists(), "undo did not bring the file back");
        assert!(!out.path().join("invoice.pdf").exists());
    }

    #[test]
    fn the_scan_reaches_subfolders() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("deep/deeper/invoice.pdf"), "content");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], &[root.path().to_path_buf()])
            .unwrap();

        assert_eq!(planned.len(), 1);
    }

    #[test]
    fn the_scan_ignores_download_temporaries() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf.crdownload"), "partial");
        write(&root.path().join("~$invoice.pdf"), "lock");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], &[root.path().to_path_buf()])
            .unwrap();

        assert!(planned.is_empty(), "temporaries leaked into the plan");
    }

    #[test]
    fn a_forbidden_folder_is_refused_before_any_scanning() {
        let service = Tidyfile::in_memory().unwrap();
        let root = if cfg!(windows) { r"C:\Windows" } else { "/usr" };

        let outcome = service.simulate(&[], &[PathBuf::from(root)]);

        assert!(matches!(
            outcome,
            Err(ServiceError::Folder(PathError::Forbidden))
        ));
    }

    #[test]
    fn a_missing_folder_is_reported_as_unavailable_rather_than_refused() {
        let root = watchable_folder();
        let service = Tidyfile::in_memory().unwrap();

        let ghost = root.path().join("ghost");

        assert!(
            service
                .simulate(&[], std::slice::from_ref(&ghost))
                .unwrap()
                .is_empty(),
            "a folder that is not there yields no changes instead of an error"
        );
        assert_eq!(statuses(&[ghost])[0].state, "unavailable");
    }

    #[test]
    fn batch_identifiers_do_not_repeat() {
        let first = next_batch_id();
        let second = next_batch_id();

        assert_ne!(first, second);
    }

    #[test]
    fn nothing_matching_produces_an_empty_report() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("notes.txt"), "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize(
                &[pdf_rule(out.path())],
                &[root.path().to_path_buf()],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!((report.applied, report.skipped, report.failed), (0, 0, 0));
    }

    #[cfg(windows)]
    fn link_folder(link: &Path, target: &Path) {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "could not create the junction");
    }

    #[cfg(unix)]
    fn link_folder(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn a_linked_folder_does_not_take_the_scan_outside() {
        let root = watchable_folder();
        let watched = root.path().join("watched");
        let outside = root.path().join("outside");
        fs::create_dir_all(&watched).unwrap();
        write(&outside.join("secret.pdf"), "outside the watched folder");

        let link = watched.join("escape");
        link_folder(&link, &outside);

        assert!(
            fs::symlink_metadata(&link).unwrap().is_symlink(),
            "Rust must report a junction or symlink as a link, or the scan filter misses it"
        );

        let found = scan(&watched);

        assert!(
            found.is_empty(),
            "the scan walked through a link and left the watched folder: {found:?}"
        );
    }

    #[test]
    fn a_watched_file_that_is_really_inside_is_tidied() {
        let root = watchable_folder();
        let out = watchable_folder();
        let file = root.path().join("invoice.pdf");
        write(&file, "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize_file(
                &[pdf_rule(out.path())],
                root.path(),
                &file,
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(report.applied, 1);
        assert!(out.path().join("invoice.pdf").exists());
    }

    #[test]
    fn a_watcher_event_through_a_link_does_not_reach_outside() {
        let base = watchable_folder();
        let out = watchable_folder();
        let watched = base.path().join("watched");
        let outside = base.path().join("outside");
        fs::create_dir_all(&watched).unwrap();
        write(&outside.join("secret.pdf"), "outside the watched folder");
        link_folder(&watched.join("escape"), &outside);
        let through_link = watched.join("escape").join("secret.pdf");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize_file(
                &[pdf_rule(out.path())],
                &watched,
                &through_link,
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(report.applied, 0);
        assert!(
            outside.join("secret.pdf").exists(),
            "the file outside was touched"
        );
    }

    #[test]
    fn a_watched_folder_swapped_for_a_link_is_not_followed() {
        let base = watchable_folder();
        let out = watchable_folder();
        let elsewhere = base.path().join("elsewhere");
        write(&elsewhere.join("invoice.pdf"), "not in the watched folder");
        let swapped = base.path().join("Downloads");
        link_folder(&swapped, &elsewhere);
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize_file(
                &[pdf_rule(out.path())],
                &swapped,
                &swapped.join("invoice.pdf"),
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(report.applied, 0);
        assert!(elsewhere.join("invoice.pdf").exists());
    }

    #[test]
    fn several_folders_are_planned_together_in_one_batch() {
        let first = watchable_folder();
        let second = watchable_folder();
        let out = watchable_folder();
        write(&first.path().join("one.pdf"), "a");
        write(&second.path().join("two.pdf"), "b");
        let service = Tidyfile::in_memory().unwrap();
        let roots = [first.path().to_path_buf(), second.path().to_path_buf()];

        let report = service
            .organize(&[pdf_rule(out.path())], &roots, Collision::Suffix)
            .unwrap();

        assert_eq!(report.applied, 2);
        assert_eq!(
            service.activity(10).unwrap().len(),
            1,
            "both folders belong to a single undoable batch"
        );
    }

    #[test]
    fn a_forbidden_folder_stops_the_whole_plan_before_anything_moves() {
        let good = watchable_folder();
        let out = watchable_folder();
        let source = good.path().join("one.pdf");
        write(&source, "a");
        let service = Tidyfile::in_memory().unwrap();
        let system = PathBuf::from(if cfg!(windows) { r"C:\Windows" } else { "/usr" });
        let roots = [good.path().to_path_buf(), system];

        let outcome = service.organize(&[pdf_rule(out.path())], &roots, Collision::Suffix);

        assert!(
            outcome.is_err(),
            "a rule aimed at a system folder must be shown, never quietly skipped"
        );
        assert!(
            source.exists(),
            "planning failed, so nothing should have been touched"
        );
    }

    #[test]
    fn an_unavailable_folder_lets_the_reachable_ones_through() {
        let good = watchable_folder();
        let out = watchable_folder();
        write(&good.path().join("one.pdf"), "a");
        let service = Tidyfile::in_memory().unwrap();
        let roots = [good.path().to_path_buf(), good.path().join("ghost")];

        let report = service
            .organize(&[pdf_rule(out.path())], &roots, Collision::Suffix)
            .unwrap();

        assert_eq!(report.applied, 1);
        assert!(out.path().join("one.pdf").exists());
    }

    #[test]
    fn the_counter_runs_across_folders_not_per_folder() {
        let first = watchable_folder();
        let second = watchable_folder();
        write(&first.path().join("a.pdf"), "a");
        write(&second.path().join("b.pdf"), "b");
        let mut counted = pdf_rule(Path::new("/out"));
        counted.actions = vec![Action::RenameTo {
            template: "{counter}.{ext}".into(),
        }];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(
                &[counted],
                &[first.path().to_path_buf(), second.path().to_path_buf()],
            )
            .unwrap();

        let names: Vec<String> = planned
            .iter()
            .filter_map(|change| change.destination.clone())
            .map(|path| {
                PathBuf::from(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["1.pdf", "2.pdf"]);
    }
}
