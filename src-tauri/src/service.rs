use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::executor::{Collision, Executor, Outcome, SkipReason};
use crate::journal::{Journal, JournalError, Operation, Origin};
use crate::paths::{self, PathError};
use crate::rules::{EvaluationError, FileFacts, PlanContext, Rule, plan};
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

#[derive(Debug, Clone, Copy)]
pub struct Provenance<'a> {
    pub origin: Origin,
    pub folder: Option<&'a Path>,
}

impl Provenance<'_> {
    const MANUAL: Self = Self {
        origin: Origin::Manual,
        folder: None,
    };
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
pub struct RuleExample {
    pub change: Option<PlannedChange>,
    pub taken_by: Option<String>,
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
    pub origin: Option<String>,
    pub folder: Option<String>,
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
        self.apply_batch(&operations, on_collision, Provenance::MANUAL)
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
        self.apply_batch(&operations, shown.on_collision, Provenance::MANUAL)
    }

    fn apply_batch(
        &self,
        operations: &[Operation],
        on_collision: Collision,
        provenance: Provenance<'_>,
    ) -> Result<BatchReport, ServiceError> {
        let batch = next_batch_id();
        if !operations.is_empty() {
            self.executor
                .journal()
                .record_batch(&batch, provenance.origin, provenance.folder)?;
        }
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
        let Some(facts) = plannable(file, root) else {
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
        let provenance = Provenance {
            origin: Origin::Watch,
            folder: Some(root),
        };
        self.apply_batch(&operations, on_collision, provenance)
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
                origin: summary.origin,
                folder: summary.folder,
            })
            .collect())
    }

    fn plan_folders(
        &self,
        rules: &[Rule],
        folders: &[PathBuf],
    ) -> Result<Vec<Operation>, ServiceError> {
        let mut operations = Vec::new();
        for (facts, context) in plannable_files(folders)? {
            operations.extend(plan(rules, &facts, context)?);
        }
        Ok(operations)
    }
}

pub fn example_for_rule(
    rule: &Rule,
    earlier: &[Rule],
    folders: &[PathBuf],
) -> Result<RuleExample, ServiceError> {
    let edited = [switched_on(rule)];
    let mut taken_by = None;
    for (facts, context) in plannable_files(folders)? {
        let Some(first) = plan(&edited, &facts, context)?.into_iter().next() else {
            continue;
        };
        let Some(taker) = first_taker(earlier, &facts, context)? else {
            return Ok(RuleExample {
                change: Some(describe(&first)),
                taken_by: None,
            });
        };
        taken_by.get_or_insert(taker);
    }
    Ok(RuleExample {
        change: None,
        taken_by,
    })
}

fn switched_on(rule: &Rule) -> Rule {
    Rule {
        enabled: true,
        ..rule.clone()
    }
}

fn first_taker(
    earlier: &[Rule],
    facts: &FileFacts,
    context: PlanContext,
) -> Result<Option<String>, ServiceError> {
    for rule in earlier.iter().filter(|rule| rule.enabled) {
        if !plan(std::slice::from_ref(rule), facts, context)?.is_empty() {
            return Ok(Some(rule.name.clone()));
        }
    }
    Ok(None)
}

// WHY: the live example and the preview must number files alike, or {counter} would differ between them
fn plannable_files(
    folders: &[PathBuf],
) -> Result<impl Iterator<Item = (FileFacts, PlanContext)>, ServiceError> {
    let now = SystemTime::now();
    let files = files_in(folders)?.into_iter().enumerate();
    Ok(files.filter_map(move |(index, (root, file))| {
        let context = PlanContext {
            now,
            counter: u32::try_from(index + 1).unwrap_or(u32::MAX),
        };
        plannable(&file, &root).map(|facts| (facts, context))
    }))
}

fn plannable(file: &Path, root: &Path) -> Option<FileFacts> {
    FileFacts::gather(file, root).ok().filter(is_worth_planning)
}

// WHY: an empty file may be a browser's placeholder, and a non-UTF-8 path cannot be journaled (ADR-010)
fn is_worth_planning(facts: &FileFacts) -> bool {
    facts.size > 0 && facts.path.to_str().is_some()
}

// WHY: a watcher event can name a file through a link, or inside a download that is still running
fn is_really_inside(root: &Path, file: &Path) -> bool {
    if is_link(root) || !is_plain_file(file) {
        return false;
    }
    folders_between(root, file).is_some_and(|folders| !holds_a_download(&folders))
}

fn folders_between(root: &Path, file: &Path) -> Option<PathBuf> {
    let parent = file.parent()?.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    parent.strip_prefix(&root).ok().map(Path::to_path_buf)
}

fn holds_a_download(folders: &Path) -> bool {
    folders
        .components()
        .any(|part| is_download_in_progress(Path::new(part.as_os_str())))
}

// WHY: Safari downloads into name.ext.download/, where the partial file already has its real name
fn is_download_in_progress(folder: &Path) -> bool {
    folder
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("download"))
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).map_or(true, |data| data.is_symlink())
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|data| data.is_file())
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
    if kind.is_dir() && !is_download_in_progress(&path) {
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

    fn named(name: &str, enabled: bool, mut rule: Rule) -> Rule {
        rule.name = name.into();
        rule.enabled = enabled;
        rule
    }

    fn file_name_of(path: &str) -> String {
        Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn the_example_is_the_first_file_the_rule_would_take() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a.pdf"), "a");
        write(&root.path().join("b.pdf"), "b");

        let example =
            example_for_rule(&pdf_rule(out.path()), &[], &[root.path().to_path_buf()]).unwrap();

        let change = example.change.unwrap();
        assert_eq!(file_name_of(&change.source), "a.pdf");
        assert_eq!(change.kind, "move");
        assert_eq!(example.taken_by, None);
    }

    #[test]
    fn an_earlier_rule_that_takes_the_file_is_named() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a.pdf"), "a");
        let earlier = [named("Primera", true, pdf_rule(out.path()))];

        let example = example_for_rule(
            &pdf_rule(out.path()),
            &earlier,
            &[root.path().to_path_buf()],
        )
        .unwrap();

        assert_eq!(example.change, None);
        assert_eq!(example.taken_by.as_deref(), Some("Primera"));
    }

    #[test]
    fn a_disabled_earlier_rule_does_not_take_the_example() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a.pdf"), "a");
        let earlier = [named("Primera", false, pdf_rule(out.path()))];

        let example = example_for_rule(
            &pdf_rule(out.path()),
            &earlier,
            &[root.path().to_path_buf()],
        )
        .unwrap();

        assert!(
            example.change.is_some(),
            "a switched-off rule took the file"
        );
        assert_eq!(example.taken_by, None);
    }

    #[test]
    fn the_rule_counts_as_enabled_for_its_own_example() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a.pdf"), "a");
        let edited = named("pdfs", false, pdf_rule(out.path()));

        let example = example_for_rule(&edited, &[], &[root.path().to_path_buf()]).unwrap();

        assert!(example.change.is_some());
    }

    #[test]
    fn a_later_file_no_earlier_rule_takes_is_still_shown() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a-invoice.pdf"), "a");
        write(&root.path().join("b-manual.pdf"), "b");
        let mut invoices = named("Primera", true, pdf_rule(out.path()));
        invoices.conditions.push(Condition::NameContains {
            text: "invoice".into(),
        });

        let example = example_for_rule(
            &pdf_rule(out.path()),
            &[invoices],
            &[root.path().to_path_buf()],
        )
        .unwrap();

        assert_eq!(
            file_name_of(&example.change.unwrap().source),
            "b-manual.pdf"
        );
        assert_eq!(example.taken_by, None);
    }

    #[test]
    fn no_match_gives_an_empty_example() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("notes.txt"), "text");

        let example =
            example_for_rule(&pdf_rule(out.path()), &[], &[root.path().to_path_buf()]).unwrap();

        assert_eq!(
            example,
            RuleExample {
                change: None,
                taken_by: None
            }
        );
    }

    #[test]
    fn the_example_matches_what_the_preview_shows() {
        let root = watchable_folder();
        write(&root.path().join("a-notes.txt"), "text");
        write(&root.path().join("b-report.pdf"), "pdf");
        let mut counted = pdf_rule(Path::new("/unused"));
        counted.actions = vec![Action::RenameTo {
            template: "{counter}.{ext}".into(),
        }];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();

        let example = example_for_rule(&counted, &[], &folders).unwrap();
        let preview = service.simulate(&[counted], &folders).unwrap();

        assert_eq!(example.change.as_ref(), preview.first());
        assert_eq!(
            file_name_of(&preview[0].destination.clone().unwrap()),
            "2.pdf"
        );
    }

    #[test]
    fn an_invalid_pattern_is_reported_with_the_rule_name() {
        let root = watchable_folder();
        write(&root.path().join("a.pdf"), "a");
        let mut broken = named("Facturas", true, pdf_rule(Path::new("/unused")));
        broken.conditions = vec![Condition::NameMatchesRegex {
            pattern: "(unclosed".into(),
        }];

        let outcome = example_for_rule(&broken, &[], &[root.path().to_path_buf()]);

        assert!(matches!(
            outcome,
            Err(ServiceError::Rules(EvaluationError::Condition { ref rule, .. })) if rule == "Facturas"
        ));
    }

    #[test]
    fn a_forbidden_folder_is_refused() {
        let system = PathBuf::from(if cfg!(windows) { r"C:\Windows" } else { "/usr" });

        let outcome = example_for_rule(&pdf_rule(Path::new("/unused")), &[], &[system]);

        assert!(matches!(
            outcome,
            Err(ServiceError::Folder(PathError::Forbidden))
        ));
    }

    #[test]
    fn the_example_touches_nothing() {
        let root = watchable_folder();
        let out = watchable_folder();
        let source = root.path().join("a.pdf");
        write(&source, "the original content");
        let mut trashing = pdf_rule(out.path());
        trashing.actions.push(Action::Trash);

        let example = example_for_rule(&trashing, &[], &[root.path().to_path_buf()]).unwrap();

        assert!(example.change.is_some());
        assert_eq!(fs::read_to_string(&source).unwrap(), "the original content");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_manual_run_is_recorded_as_manual() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        let service = Tidyfile::in_memory().unwrap();

        service
            .organize(
                &[pdf_rule(out.path())],
                &[root.path().to_path_buf()],
                Collision::Suffix,
            )
            .unwrap();

        let entry = &service.activity(10).unwrap()[0];
        assert_eq!(entry.origin.as_deref(), Some("manual"));
        assert_eq!(entry.folder, None);
    }

    #[test]
    fn a_run_from_the_preview_is_recorded_as_manual() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("invoice.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();
        let shown = service.simulate(&rules, &folders).unwrap();

        service
            .organize_as_shown(
                &rules,
                &folders,
                Shown {
                    changes: &shown,
                    on_collision: Collision::Suffix,
                },
            )
            .unwrap();

        let entry = &service.activity(10).unwrap()[0];
        assert_eq!(
            (entry.origin.as_deref(), entry.folder.as_deref()),
            (Some("manual"), None)
        );
    }

    #[test]
    fn a_watched_file_is_recorded_with_its_folder() {
        let root = watchable_folder();
        let out = watchable_folder();
        let file = root.path().join("invoice.pdf");
        write(&file, "content");
        let service = Tidyfile::in_memory().unwrap();

        service
            .organize_file(
                &[pdf_rule(out.path())],
                root.path(),
                &file,
                Collision::Suffix,
            )
            .unwrap();

        let entry = &service.activity(10).unwrap()[0];
        assert_eq!(entry.origin.as_deref(), Some("watch"));
        assert_eq!(entry.folder.as_deref(), root.path().to_str());
    }

    #[test]
    fn an_empty_run_records_no_batch() {
        let data = tempfile::TempDir::new().unwrap();
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("notes.txt"), "content");
        let journal = data.path().join("journal.sqlite");
        let service = Tidyfile::open(&journal).unwrap();

        service
            .organize(
                &[pdf_rule(out.path())],
                &[root.path().to_path_buf()],
                Collision::Suffix,
            )
            .unwrap();

        let recorded: i64 = rusqlite::Connection::open(&journal)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM batches", [], |row| row.get(0))
            .unwrap();
        assert_eq!(recorded, 0);
        assert!(service.activity(10).unwrap().is_empty());
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

    #[cfg(windows)]
    fn unrecordable_name() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = "m-caf".encode_utf16().collect();
        units.push(0xD800);
        units.extend(".pdf".encode_utf16());
        std::ffi::OsString::from_wide(&units)
    }

    #[cfg(unix)]
    fn unrecordable_name() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"m-caf\xe9.pdf".to_vec())
    }

    fn facts_for(name: std::ffi::OsString, size: u64) -> FileFacts {
        FileFacts {
            path: Path::new("watched").join(name),
            root: PathBuf::from("watched"),
            size,
            modified: SystemTime::now(),
        }
    }

    #[test]
    fn only_files_with_content_and_a_recordable_name_are_planned() {
        assert!(is_worth_planning(&facts_for("report.pdf".into(), 10)));
        assert!(!is_worth_planning(&facts_for("report.pdf".into(), 0)));
        assert!(!is_worth_planning(&facts_for(unrecordable_name(), 10)));
    }

    #[test]
    fn an_empty_file_is_left_out_of_the_preview_and_the_run() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("placeholder.pdf"), "");
        write(&root.path().join("invoice.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service.simulate(&rules, &folders).unwrap();
        let report = service
            .organize(&rules, &folders, Collision::Suffix)
            .unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(report.applied, 1);
        assert!(root.path().join("placeholder.pdf").exists());
    }

    #[test]
    fn a_folder_named_by_a_watcher_event_is_never_acted_on() {
        let root = watchable_folder();
        let out = watchable_folder();
        let folder = root.path().join("looks-like.pdf");
        write(&folder.join("inside.txt"), "content");
        let service = Tidyfile::in_memory().unwrap();

        let report = service
            .organize_file(
                &[pdf_rule(out.path())],
                root.path(),
                &folder,
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(report.applied, 0);
        assert!(folder.join("inside.txt").exists());
    }

    #[test]
    fn the_scan_does_not_enter_a_folder_a_download_is_still_using() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("report.pdf.download/report.pdf"), "half");
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&[pdf_rule(out.path())], &[root.path().to_path_buf()])
            .unwrap();

        assert!(
            planned.is_empty(),
            "a download in progress entered the plan"
        );
    }

    #[test]
    fn a_watched_file_inside_a_download_in_progress_is_left_alone() {
        let root = watchable_folder();
        let out = watchable_folder();
        let direct = root.path().join("report.pdf.download/report.pdf");
        let nested = root.path().join("photos.zip.download/inner/a.pdf");
        write(&direct, "half");
        write(&nested, "half");
        let rules = [pdf_rule(out.path())];
        let service = Tidyfile::in_memory().unwrap();

        for file in [&direct, &nested] {
            let report = service
                .organize_file(&rules, root.path(), file, Collision::Suffix)
                .unwrap();
            assert_eq!(
                report.applied,
                0,
                "{} was tidied mid-download",
                file.display()
            );
        }
    }

    #[test]
    fn folders_that_only_look_temporary_are_still_tidied() {
        let base = watchable_folder();
        let out = watchable_folder();
        let root = base.path().join("Site.download");
        write(&root.join("Backup.tmp/invoice.pdf"), "content");
        write(&root.join("receipt.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service
            .simulate(&rules, std::slice::from_ref(&root))
            .unwrap();
        let report = service
            .organize_file(
                &rules,
                &root,
                &root.join("Backup.tmp/invoice.pdf"),
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(
            planned.len(),
            2,
            "the watched folder's own name or a .tmp folder was skipped"
        );
        assert_eq!(report.applied, 1);
    }

    // WHY: APFS refuses names that are not UTF-8, so macOS cannot even create this file
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_name_the_journal_cannot_record_is_left_alone_and_the_rest_still_tidied() {
        let root = watchable_folder();
        let out = watchable_folder();
        write(&root.path().join("a-first.pdf"), "content");
        let unrecordable = root.path().join(unrecordable_name());
        fs::write(&unrecordable, "a name from an old zip").unwrap();
        write(&root.path().join("z-last.pdf"), "content");
        let rules = [pdf_rule(out.path())];
        let folders = [root.path().to_path_buf()];
        let service = Tidyfile::in_memory().unwrap();

        let planned = service.simulate(&rules, &folders).unwrap();
        let report = service.organize(&rules, &folders, Collision::Suffix);

        assert_eq!(
            planned.len(),
            2,
            "the preview promised a change that cannot be recorded"
        );
        let report = report.expect("one unrecordable name made the whole run fail");
        assert_eq!((report.applied, report.failed), (2, 0));
        assert!(out.path().join("z-last.pdf").exists());
        assert!(unrecordable.exists());
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
