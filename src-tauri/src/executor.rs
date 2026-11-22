use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::journal::{Journal, JournalError, Operation, RecordedOperation, State};
use crate::recycle;

const MAX_NAME_ATTEMPTS: u32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("the journal could not be updated")]
    Journal(#[from] JournalError),
}

#[derive(Debug, thiserror::Error)]
enum ActionError {
    #[error("the file could not be moved")]
    Move(#[from] std::io::Error),
    #[error("the file could not be sent to the trash")]
    Trash(#[from] trash::Error),
    #[error("the copy did not reproduce the whole file")]
    IncompleteCopy,
    #[error("every candidate name was already taken")]
    NoFreeName,
    #[error("the Recycle Bin is not available on this drive")]
    NoRecycleBin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Collision {
    #[default]
    Suffix,
    Skip,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    SourceVanished,
    AlreadyInPlace,
    DestinationTaken,
    NothingToUndo,
    RestoreUnsupported,
    NotFoundInTrash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied(Operation),
    Skipped(SkipReason),
    Failed(String),
}

pub struct Executor {
    journal: Journal,
    has_recycle_bin: fn(&Path) -> bool,
}

impl Executor {
    pub fn new(journal: Journal) -> Self {
        Self {
            journal,
            has_recycle_bin: recycle::has_recycle_bin,
        }
    }

    #[cfg(test)]
    pub fn judging_recycle_bin(journal: Journal, has_recycle_bin: fn(&Path) -> bool) -> Self {
        Self {
            journal,
            has_recycle_bin,
        }
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn apply(
        &self,
        batch: &str,
        operations: &[Operation],
        on_collision: Collision,
    ) -> Result<Vec<Outcome>, ExecutorError> {
        operations
            .iter()
            .map(|operation| self.apply_planned(batch, operation, on_collision))
            .collect()
    }

    // WHY: Windows deletes for good where it keeps no Recycle Bin, yet still reports success
    fn apply_planned(
        &self,
        batch: &str,
        operation: &Operation,
        on_collision: Collision,
    ) -> Result<Outcome, ExecutorError> {
        if self.would_be_lost(operation) && refuse_early(operation).is_none() {
            let id = self.journal.record_planned(batch, operation)?;
            return self.give_up(id, &ActionError::NoRecycleBin.to_string());
        }
        self.apply_one(batch, operation, on_collision)
    }

    fn would_be_lost(&self, operation: &Operation) -> bool {
        matches!(operation, Operation::Trash { from } if !(self.has_recycle_bin)(from))
    }

    fn apply_one(
        &self,
        batch: &str,
        operation: &Operation,
        on_collision: Collision,
    ) -> Result<Outcome, ExecutorError> {
        if let Some(refusal) = refuse_early(operation) {
            return Ok(refusal);
        }
        let id = self.journal.record_planned(batch, operation)?;
        self.carry_out(id, operation, on_collision)
    }

    pub fn retry_skipped(
        &self,
        batch: &str,
        on_collision: Collision,
    ) -> Result<Vec<Outcome>, ExecutorError> {
        let waiting = self.journal.skipped_in_batch(batch)?;
        waiting
            .iter()
            .map(|record| self.retry_one(record, on_collision))
            .collect()
    }

    fn retry_one(
        &self,
        record: &RecordedOperation,
        on_collision: Collision,
    ) -> Result<Outcome, ExecutorError> {
        if let Some(refusal) = refuse_early(&record.operation) {
            return Ok(refusal);
        }
        self.journal.mark(record.id, State::Planned, None)?;
        self.carry_out(record.id, &record.operation, on_collision)
    }

    fn carry_out(
        &self,
        id: i64,
        operation: &Operation,
        on_collision: Collision,
    ) -> Result<Outcome, ExecutorError> {
        let claimed = match claim(operation, on_collision) {
            Ok(Some(claimed)) => claimed,
            Ok(None) => return self.step_aside(id),
            Err(error) => return self.give_up(id, &error.to_string()),
        };
        if let Some(destination) = destination_of(&claimed) {
            self.journal.retarget(id, destination)?;
        }
        self.run_and_record(id, claimed)
    }

    fn step_aside(&self, id: i64) -> Result<Outcome, ExecutorError> {
        self.journal
            .mark(id, State::Skipped, Some("destination already exists"))?;
        Ok(Outcome::Skipped(SkipReason::DestinationTaken))
    }

    fn run_and_record(&self, id: i64, operation: Operation) -> Result<Outcome, ExecutorError> {
        match perform(&operation) {
            Ok(()) => {
                self.journal.mark(id, State::Done, None)?;
                Ok(Outcome::Applied(operation))
            }
            Err(error) => {
                discard_reservation(&operation);
                self.give_up(id, &error.to_string())
            }
        }
    }

    fn give_up(&self, id: i64, detail: &str) -> Result<Outcome, ExecutorError> {
        self.journal.mark(id, State::Failed, Some(detail))?;
        Ok(Outcome::Failed(detail.to_owned()))
    }

    pub fn undo_batch(&self, batch: &str) -> Result<Vec<Outcome>, ExecutorError> {
        let applied = self.journal.applied_in_batch(batch)?;
        applied
            .iter()
            .map(|record| self.undo_operation(record.id))
            .collect()
    }

    pub fn undo_operation(&self, id: i64) -> Result<Outcome, ExecutorError> {
        let Some(record) = self.journal.operation(id)? else {
            return Ok(Outcome::Skipped(SkipReason::NothingToUndo));
        };
        if record.state != State::Done {
            return Ok(Outcome::Skipped(SkipReason::NothingToUndo));
        }
        let outcome = self.undo_one(&record)?;
        if matches!(outcome, Outcome::Applied(_) | Outcome::Skipped(_)) {
            self.journal.mark(id, State::Undone, None)?;
        }
        Ok(outcome)
    }

    fn undo_one(&self, record: &RecordedOperation) -> Result<Outcome, ExecutorError> {
        let batch = &format!("{}:undo", record.batch);
        match &record.operation {
            Operation::Move { from, to } => self.apply_one(
                batch,
                &Operation::Move {
                    from: to.clone(),
                    to: from.clone(),
                },
                Collision::Suffix,
            ),
            Operation::Copy { to, .. } => self.apply_planned(
                batch,
                &Operation::Trash { from: to.clone() },
                Collision::Suffix,
            ),
            Operation::Trash { from } => Ok(restore::from_trash(from, record.recorded_at)),
        }
    }
}

fn perform(operation: &Operation) -> Result<(), ActionError> {
    match operation {
        Operation::Move { from, to } => move_file(from, to),
        Operation::Copy { from, to } => copy_file(from, to),
        Operation::Trash { from } => Ok(trash::delete(from)?),
    }
}

fn move_file(from: &Path, to: &Path) -> Result<(), ActionError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_file(from, to)?;
    Ok(trash::delete(from)?)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), ActionError> {
    let original = fs::metadata(from)?.len();
    let copied = fs::copy(from, to)?;
    if copied != original {
        let _ = trash::delete(to);
        return Err(ActionError::IncompleteCopy);
    }
    Ok(())
}

fn claim(operation: &Operation, on_collision: Collision) -> Result<Option<Operation>, ActionError> {
    let Some(desired) = destination_of(operation) else {
        return Ok(Some(operation.clone()));
    };
    let Some(claimed) = reserve(desired, on_collision)? else {
        return Ok(None);
    };
    Ok(Some(retarget(operation, claimed)))
}

fn reserve(desired: &Path, on_collision: Collision) -> Result<Option<PathBuf>, ActionError> {
    if let Some(parent) = desired.parent() {
        fs::create_dir_all(parent)?;
    }
    for candidate in candidates(desired, on_collision) {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ActionError::Move(error)),
        }
    }
    match on_collision {
        Collision::Skip | Collision::Ask => Ok(None),
        Collision::Suffix => Err(ActionError::NoFreeName),
    }
}

fn candidates(desired: &Path, on_collision: Collision) -> Vec<PathBuf> {
    let mut names = vec![desired.to_path_buf()];
    if on_collision != Collision::Suffix {
        return names;
    }
    let (Some(parent), Some(stem)) = (desired.parent(), desired.file_stem()) else {
        return names;
    };
    names.extend(
        (2..MAX_NAME_ATTEMPTS)
            .map(|attempt| parent.join(candidate_name(stem, desired.extension(), attempt))),
    );
    names
}

fn discard_reservation(operation: &Operation) {
    let Some(destination) = destination_of(operation) else {
        return;
    };
    if fs::metadata(destination).is_ok_and(|data| data.len() == 0) {
        let _ = trash::delete(destination);
    }
}

fn refuse_early(operation: &Operation) -> Option<Outcome> {
    if !operation.source().exists() {
        return Some(Outcome::Skipped(SkipReason::SourceVanished));
    }
    if is_already_in_place(operation) {
        return Some(Outcome::Skipped(SkipReason::AlreadyInPlace));
    }
    None
}

fn is_already_in_place(operation: &Operation) -> bool {
    destination_of(operation).is_some_and(|destination| same_file(operation.source(), destination))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn destination_of(operation: &Operation) -> Option<&Path> {
    match operation {
        Operation::Move { to, .. } | Operation::Copy { to, .. } => Some(to),
        Operation::Trash { .. } => None,
    }
}

fn retarget(operation: &Operation, to: PathBuf) -> Operation {
    match operation {
        Operation::Move { from, .. } => Operation::Move {
            from: from.clone(),
            to,
        },
        Operation::Copy { from, .. } => Operation::Copy {
            from: from.clone(),
            to,
        },
        Operation::Trash { from } => Operation::Trash { from: from.clone() },
    }
}

fn candidate_name(
    stem: &std::ffi::OsStr,
    extension: Option<&std::ffi::OsStr>,
    attempt: u32,
) -> OsString {
    let mut name = OsString::from(stem);
    name.push(format!(" ({attempt})"));
    if let Some(extension) = extension {
        name.push(".");
        name.push(extension);
    }
    name
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
mod restore {
    use super::{Outcome, SkipReason};
    use crate::journal::Operation;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use trash::TrashItem;

    #[cfg(windows)]
    const ACCEPTED_DRIFT: (i64, i64) = (2, 300);
    // WHY: freedesktop stores the deletion time as local time without a zone, so it drifts by the UTC offset
    #[cfg(not(windows))]
    const ACCEPTED_DRIFT: (i64, i64) = (93_600, 93_600);

    pub fn from_trash(original: &Path, recorded_at: i64) -> Outcome {
        let items = match trash::os_limited::list() {
            Ok(items) => items,
            Err(error) => return Outcome::Failed(error.to_string()),
        };
        let wanted = as_the_trash_records_it(original);
        let Some(item) = the_one_trashed(items, &wanted, recorded_at) else {
            return Outcome::Skipped(SkipReason::NotFoundInTrash);
        };
        match trash::os_limited::restore_all([item]) {
            Ok(()) => Outcome::Applied(Operation::Trash {
                from: original.to_path_buf(),
            }),
            Err(error) => Outcome::Failed(error.to_string()),
        }
    }

    fn the_one_trashed(
        items: Vec<TrashItem>,
        original: &Path,
        recorded_at: i64,
    ) -> Option<TrashItem> {
        let folder = original.parent()?;
        let name = original.file_name()?;
        let mut chosen = items
            .into_iter()
            .filter(|item| item.original_parent == folder && is_listed_as(item, name))
            .filter(|item| is_close_in_time(item, recorded_at))
            .min_by_key(|item| drift(item, recorded_at).saturating_abs())?;
        chosen.name = name.to_os_string();
        Some(chosen)
    }

    fn drift(item: &TrashItem, recorded_at: i64) -> i64 {
        item.time_deleted.saturating_sub(recorded_at)
    }

    fn is_close_in_time(item: &TrashItem, recorded_at: i64) -> bool {
        let (before, after) = ACCEPTED_DRIFT;
        (-before..=after).contains(&drift(item, recorded_at))
    }

    // WHY: with "hide extensions for known file types" on, Windows lists `report.pdf` as `report`
    #[cfg(windows)]
    fn is_listed_as(item: &TrashItem, name: &OsStr) -> bool {
        let stored = Path::new(&item.id).file_name().unwrap_or_default();
        let stem = Path::new(name).file_stem();
        extension_of(stored) == extension_of(name)
            && (item.name == name || stem == Some(item.name.as_os_str()))
    }

    #[cfg(not(windows))]
    fn is_listed_as(item: &TrashItem, name: &OsStr) -> bool {
        item.name == name
    }

    #[cfg(windows)]
    fn extension_of(name: &OsStr) -> Option<String> {
        let (_, extension) = name.to_str()?.rsplit_once('.')?;
        (!extension.is_empty()).then(|| extension.to_ascii_lowercase())
    }

    // WHY: the trash keeps `C:\..`; the journal holds `\\?\C:\..`, and tests on CI an 8.3 short name
    fn as_the_trash_records_it(path: &Path) -> PathBuf {
        let parent = path.parent().map(|parent| {
            parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf())
        });
        let full = match (parent, path.file_name()) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => path.to_path_buf(),
        };
        dunce::simplified(&full).to_path_buf()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    mod tests {
        use super::*;
        use std::ffi::OsString;

        const RECORDED: i64 = 1_000_000;

        fn trashed(parent: &str, name: &str, at: i64) -> TrashItem {
            TrashItem {
                id: OsString::from(format!("/bin/$R{at}-{name}")),
                name: name.into(),
                original_parent: PathBuf::from(parent),
                time_deleted: at,
            }
        }

        #[test]
        fn the_file_this_operation_trashed_wins_over_its_namesakes() {
            let (before, _) = ACCEPTED_DRIFT;
            let items = vec![
                trashed("/w", "setup.exe", RECORDED + 60),
                trashed("/w", "setup.exe", RECORDED + 1),
                trashed("/w", "setup.exe", RECORDED - before - 1),
            ];

            let chosen = the_one_trashed(items, Path::new("/w/setup.exe"), RECORDED).unwrap();

            assert_eq!(chosen.time_deleted, RECORDED + 1);
        }

        #[test]
        fn a_namesake_trashed_outside_the_window_is_never_restored() {
            let (before, after) = ACCEPTED_DRIFT;
            let items = vec![
                trashed("/w", "setup.exe", RECORDED - before - 1),
                trashed("/w", "setup.exe", RECORDED + after + 1),
            ];

            assert!(the_one_trashed(items, Path::new("/w/setup.exe"), RECORDED).is_none());
        }

        #[test]
        fn another_folder_or_another_name_is_not_a_match() {
            let items = vec![
                trashed("/elsewhere", "setup.exe", 1_000),
                trashed("/w", "other.exe", 1_000),
            ];

            assert!(the_one_trashed(items, Path::new("/w/setup.exe"), 1_000).is_none());
        }

        #[cfg(windows)]
        #[test]
        fn a_name_listed_without_its_extension_matches_and_comes_back_whole() {
            let mut listed = trashed("/w", "setup", 1_000);
            listed.id = OsString::from(r"C:\$Recycle.Bin\S-1-5-21\$RAB12CD.exe");

            let chosen = the_one_trashed(vec![listed], Path::new("/w/setup.exe"), 1_000).unwrap();

            assert_eq!(
                chosen.name, "setup.exe",
                "restoring as `setup` would drop the extension"
            );
        }

        #[cfg(windows)]
        #[test]
        fn a_name_without_extension_from_another_type_is_not_a_match() {
            let mut listed = trashed("/w", "setup", 1_000);
            listed.id = OsString::from(r"C:\$Recycle.Bin\S-1-5-21\$RAB12CD.txt");

            assert!(the_one_trashed(vec![listed], Path::new("/w/setup.exe"), 1_000).is_none());
        }

        #[cfg(windows)]
        #[test]
        fn a_double_extension_with_the_last_one_hidden_is_not_a_match() {
            let mut impostor = trashed("/w", "invoice.pdf", 1_000);
            impostor.id = OsString::from(r"C:\$Recycle.Bin\S-1-5-21\$RAB12CD.exe");

            assert!(
                the_one_trashed(vec![impostor], Path::new("/w/invoice.pdf"), 1_000).is_none(),
                "invoice.pdf.exe must never come back as invoice.pdf"
            );
        }

        #[cfg(windows)]
        #[test]
        fn a_file_without_extension_does_not_take_one_whose_extension_is_hidden() {
            let mut listed = trashed("/w", "notes", 1_000);
            listed.id = OsString::from(r"C:\$Recycle.Bin\S-1-5-21\$RAB12CD.txt");

            assert!(the_one_trashed(vec![listed], Path::new("/w/notes"), 1_000).is_none());
        }

        #[cfg(windows)]
        #[test]
        fn a_verbatim_journal_path_matches_even_when_its_folder_is_gone() {
            let journal = Path::new(r"\\?\C:\tidyfile-folder-that-is-gone\a.pdf");

            assert_eq!(
                as_the_trash_records_it(journal),
                PathBuf::from(r"C:\tidyfile-folder-that-is-gone\a.pdf")
            );
        }
    }
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
mod restore {
    use super::{Outcome, SkipReason};
    use std::path::Path;

    pub fn from_trash(_original: &Path, _recorded_at: i64) -> Outcome {
        Outcome::Skipped(SkipReason::RestoreUnsupported)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn executor() -> Executor {
        Executor::new(Journal::open_in_memory().unwrap())
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn a_move_relocates_the_file_and_undo_puts_it_back() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("invoice.pdf");
        let destination = root.path().join("Invoices/invoice.pdf");
        write(&source, "content");
        let executor = executor();

        let operation = Operation::Move {
            from: source.clone(),
            to: destination.clone(),
        };
        executor
            .apply("batch-1", &[operation], Collision::Suffix)
            .unwrap();
        assert!(destination.exists() && !source.exists());

        executor.undo_batch("batch-1").unwrap();

        assert!(source.exists() && !destination.exists());
        assert_eq!(fs::read_to_string(&source).unwrap(), "content");
    }

    #[test]
    fn a_collision_gets_a_suffix_and_never_overwrites() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");

        let outcomes = executor()
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source,
                    to: occupied.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        let Outcome::Applied(Operation::Move { to, .. }) = &outcomes[0] else {
            panic!("expected an applied move, got {:?}", outcomes[0]);
        };
        assert_eq!(to.file_name().unwrap(), "notes (2).txt");
        assert_eq!(fs::read_to_string(to).unwrap(), "new");
    }

    #[test]
    fn a_vanished_source_is_skipped_and_never_journaled() {
        let root = TempDir::new().unwrap();
        let executor = executor();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: root.path().join("ghost.pdf"),
                    to: root.path().join("out/ghost.pdf"),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Skipped(SkipReason::SourceVanished)]);
        assert!(executor.journal().interrupted().unwrap().is_empty());
    }

    #[test]
    fn every_operation_is_journaled_before_it_runs() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("a.txt");
        write(&source, "x");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source,
                    to: root.path().join("out/a.txt"),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(
            executor
                .journal()
                .applied_in_batch("batch-1")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn undoing_twice_does_not_move_the_file_again() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.pdf");
        let destination = root.path().join("out/report.pdf");
        write(&source, "content");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: destination,
                }],
                Collision::Suffix,
            )
            .unwrap();
        executor.undo_batch("batch-1").unwrap();
        let second = executor.undo_batch("batch-1").unwrap();

        assert!(second.is_empty());
        assert!(source.exists());
    }

    #[test]
    fn a_batch_undoes_in_reverse_order() {
        let root = TempDir::new().unwrap();
        let operations: Vec<Operation> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|name| {
                let from = root.path().join(name);
                write(&from, name);
                Operation::Move {
                    from,
                    to: root.path().join("out").join(name),
                }
            })
            .collect();
        let executor = executor();

        executor
            .apply("batch-1", &operations, Collision::Suffix)
            .unwrap();
        executor.undo_batch("batch-1").unwrap();

        for name in ["a.txt", "b.txt", "c.txt"] {
            assert!(root.path().join(name).exists(), "{name} was not restored");
        }
    }

    #[test]
    fn trashing_records_the_operation_and_removes_the_file() {
        let root = TempDir::new().unwrap();
        let doomed = root.path().join("obsolete.log");
        write(&doomed, "content");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Trash {
                    from: doomed.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert!(!doomed.exists());
        assert_eq!(
            executor
                .journal()
                .applied_in_batch("batch-1")
                .unwrap()
                .len(),
            1
        );

        let undone = executor.undo_batch("batch-1").unwrap();
        assert_eq!(undone.len(), 1);
        if cfg!(target_os = "macos") {
            assert_eq!(undone[0], Outcome::Skipped(SkipReason::RestoreUnsupported));
        } else {
            assert!(doomed.exists(), "the file was not restored from the trash");
        }
    }

    fn without_recycle_bin() -> Executor {
        Executor::judging_recycle_bin(Journal::open_in_memory().unwrap(), |_| false)
    }

    #[test]
    fn trashing_on_a_drive_without_recycle_bin_fails_and_keeps_the_file() {
        let root = TempDir::new().unwrap();
        let precious = root.path().join("precious.log");
        write(&precious, "only copy");
        let executor = without_recycle_bin();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Trash {
                    from: precious.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(
            outcomes,
            [Outcome::Failed(
                "the Recycle Bin is not available on this drive".into()
            )]
        );
        assert_eq!(fs::read_to_string(&precious).unwrap(), "only copy");
        let recorded = executor.journal().operations_in_batch("batch-1").unwrap();
        assert_eq!(recorded.len(), 1, "the refusal must show in the history");
        assert_eq!(recorded[0].state, State::Failed);
    }

    #[test]
    fn a_vanished_file_is_still_skipped_when_there_is_no_recycle_bin() {
        let root = TempDir::new().unwrap();
        let executor = without_recycle_bin();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Trash {
                    from: root.path().join("gone.log"),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Skipped(SkipReason::SourceVanished)]);
    }

    #[test]
    fn moving_and_undoing_a_move_do_not_depend_on_the_recycle_bin_check() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.pdf");
        let moved = root.path().join("moved/report.pdf");
        write(&source, "content");
        let executor = without_recycle_bin();
        let moving = Operation::Move {
            from: source.clone(),
            to: moved.clone(),
        };

        let applied = executor
            .apply("move", &[moving], Collision::Suffix)
            .unwrap();
        assert!(matches!(applied[..], [Outcome::Applied(_)]));
        assert_eq!(fs::read_to_string(&moved).unwrap(), "content");
        let undone = executor.undo_batch("move").unwrap();

        assert!(matches!(undone[..], [Outcome::Applied(_)]));
        assert_eq!(fs::read_to_string(&source).unwrap(), "content");
        assert!(!moved.exists());
    }

    fn copy_of(source: &Path, duplicate: &Path) -> Operation {
        Operation::Copy {
            from: source.to_path_buf(),
            to: duplicate.to_path_buf(),
        }
    }

    #[test]
    fn undoing_a_copy_on_a_drive_without_recycle_bin_fails_and_keeps_the_copy() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("invoice.pdf");
        let duplicate = root.path().join("nas/invoice.pdf");
        write(&source, "original");
        let executor = without_recycle_bin();
        let copy = copy_of(&source, &duplicate);
        executor.apply("copy", &[copy], Collision::Suffix).unwrap();
        write(&duplicate, "edited after the copy");

        let undone = executor.undo_batch("copy").unwrap();

        assert_eq!(
            undone,
            [Outcome::Failed(
                "the Recycle Bin is not available on this drive".into()
            )]
        );
        assert_eq!(
            fs::read_to_string(&duplicate).unwrap(),
            "edited after the copy"
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        let refusals = executor.journal().operations_in_batch("copy:undo").unwrap();
        assert_eq!(refusals.len(), 1, "the refusal must show in the history");
        assert_eq!(refusals[0].state, State::Failed);
    }

    #[test]
    fn a_refused_copy_undo_succeeds_once_the_recycle_bin_is_back() {
        let root = TempDir::new().unwrap();
        let journal = root.path().join("journal.sqlite");
        let source = root.path().join("invoice.pdf");
        let duplicate = root.path().join("usb/invoice.pdf");
        write(&source, "original");
        let refusing = Executor::judging_recycle_bin(Journal::open(&journal).unwrap(), |_| false);
        let copy = copy_of(&source, &duplicate);
        refusing.apply("copy", &[copy], Collision::Suffix).unwrap();
        refusing.undo_batch("copy").unwrap();
        assert_eq!(
            refusing.journal().applied_in_batch("copy").unwrap().len(),
            1
        );
        drop(refusing);

        let restored = Executor::judging_recycle_bin(Journal::open(&journal).unwrap(), |_| true);
        let undone = restored.undo_batch("copy").unwrap();

        assert!(matches!(
            undone[..],
            [Outcome::Applied(Operation::Trash { .. })]
        ));
        assert!(!duplicate.exists(), "the second undo must remove the copy");
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        assert!(
            restored
                .journal()
                .applied_in_batch("copy")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_copy_leaves_the_original_where_it_was() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.pdf");
        let duplicate = root.path().join("backup/report.pdf");
        write(&source, "content");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Copy {
                    from: source.clone(),
                    to: duplicate.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert!(source.exists(), "the original must survive a copy");
        assert_eq!(fs::read_to_string(&duplicate).unwrap(), "content");
    }

    #[test]
    fn undoing_a_copy_removes_the_duplicate_and_spares_the_original() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.pdf");
        let duplicate = root.path().join("backup/report.pdf");
        write(&source, "content");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Copy {
                    from: source.clone(),
                    to: duplicate.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();
        executor.undo_batch("batch-1").unwrap();

        assert!(!duplicate.exists(), "the duplicate was not removed");
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "content",
            "undoing a copy must never touch the original"
        );
    }

    #[test]
    fn a_copy_onto_an_existing_file_gets_a_suffix() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("backup/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");

        let outcomes = executor()
            .apply(
                "batch-1",
                &[Operation::Copy {
                    from: source,
                    to: occupied.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        let Outcome::Applied(Operation::Copy { to, .. }) = &outcomes[0] else {
            panic!("expected an applied copy, got {:?}", outcomes[0]);
        };
        assert_eq!(to.file_name().unwrap(), "notes (2).txt");
    }

    #[test]
    fn the_filesystem_primitives_would_overwrite_if_we_let_them() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("a.txt");
        let occupied = root.path().join("b.txt");

        write(&source, "NEW");
        write(&occupied, "EXISTING");
        fs::rename(&source, &occupied).unwrap();
        assert_eq!(
            fs::read_to_string(&occupied).unwrap(),
            "NEW",
            "fs::rename replaces an existing file, so reserve() must claim the name first"
        );

        write(&source, "NEW2");
        write(&occupied, "EXISTING2");
        fs::copy(&source, &occupied).unwrap();
        assert_eq!(
            fs::read_to_string(&occupied).unwrap(),
            "NEW2",
            "fs::copy replaces an existing file too"
        );

        let refused = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&occupied);
        assert_eq!(
            refused.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists,
            "create_new is the only one of the three that refuses, which is why reserve uses it"
        );
    }

    #[test]
    fn reserving_never_hands_back_an_occupied_name() {
        let root = TempDir::new().unwrap();
        let occupied = root.path().join("notes.txt");
        write(&occupied, "existing");

        let claimed = reserve(&occupied, Collision::Suffix).unwrap().unwrap();

        assert_ne!(claimed, occupied);
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        assert!(
            claimed.exists(),
            "a reservation is a real file held on disk"
        );
    }

    #[test]
    fn a_second_claim_cannot_reuse_a_name_the_first_is_holding() {
        let root = TempDir::new().unwrap();
        let desired = root.path().join("report.pdf");

        let first = reserve(&desired, Collision::Suffix).unwrap().unwrap();
        let second = reserve(&desired, Collision::Suffix).unwrap().unwrap();

        assert_eq!(first, desired);
        assert_ne!(
            second, first,
            "the claim is atomic, so the second caller gets a different name"
        );
    }

    #[test]
    fn a_failed_operation_leaves_no_empty_reservation_behind() {
        let root = TempDir::new().unwrap();
        let destination = root.path().join("out/ghost.txt");
        let operation = Operation::Move {
            from: root.path().join("vanished.txt"),
            to: destination.clone(),
        };
        reserve(&destination, Collision::Suffix).unwrap().unwrap();
        assert!(destination.exists());

        discard_reservation(&operation);

        assert!(
            !destination.exists(),
            "the empty placeholder was not cleaned up"
        );
    }

    #[test]
    fn a_reservation_holding_real_content_is_never_discarded() {
        let root = TempDir::new().unwrap();
        let destination = root.path().join("result.txt");
        write(&destination, "a real result");
        let operation = Operation::Move {
            from: root.path().join("source.txt"),
            to: destination.clone(),
        };

        discard_reservation(&operation);

        assert_eq!(fs::read_to_string(&destination).unwrap(), "a real result");
    }

    #[test]
    fn the_skip_policy_leaves_the_existing_file_and_does_not_move_the_source() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");

        let outcomes = executor()
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: occupied.clone(),
                }],
                Collision::Skip,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Skipped(SkipReason::DestinationTaken)]);
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        assert!(source.exists(), "the source must stay put when skipping");
    }

    #[test]
    fn the_skip_policy_still_moves_when_the_destination_is_free() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let destination = root.path().join("archive/notes.txt");
        write(&source, "content");

        let outcomes = executor()
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                }],
                Collision::Skip,
            )
            .unwrap();

        assert!(matches!(outcomes[0], Outcome::Applied(_)));
        assert!(destination.exists() && !source.exists());
    }

    #[test]
    fn a_skipped_operation_is_not_offered_for_undo() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source,
                    to: occupied,
                }],
                Collision::Skip,
            )
            .unwrap();

        assert!(
            executor
                .journal()
                .applied_in_batch("batch-1")
                .unwrap()
                .is_empty(),
            "a skipped operation never happened, so there is nothing to revert"
        );
    }

    #[test]
    fn one_operation_can_be_undone_without_touching_the_rest_of_the_batch() {
        let root = TempDir::new().unwrap();
        let names = ["a.txt", "b.txt"];
        let operations: Vec<Operation> = names
            .iter()
            .map(|name| {
                let from = root.path().join(name);
                write(&from, name);
                Operation::Move {
                    from,
                    to: root.path().join("out").join(name),
                }
            })
            .collect();
        let executor = executor();
        executor
            .apply("batch-1", &operations, Collision::Suffix)
            .unwrap();

        let applied = executor.journal().applied_in_batch("batch-1").unwrap();
        let chosen = applied
            .iter()
            .find(|record| record.operation.source().file_name().unwrap() == "a.txt");
        executor.undo_operation(chosen.unwrap().id).unwrap();

        assert!(root.path().join("a.txt").exists(), "a.txt was not restored");
        assert!(
            root.path().join("out/b.txt").exists(),
            "b.txt should not have moved back"
        );
    }

    #[test]
    fn undoing_the_same_operation_twice_is_a_no_op() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.pdf");
        write(&source, "content");
        let executor = executor();
        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: root.path().join("out/report.pdf"),
                }],
                Collision::Suffix,
            )
            .unwrap();
        let id = executor.journal().applied_in_batch("batch-1").unwrap()[0].id;

        executor.undo_operation(id).unwrap();
        let again = executor.undo_operation(id).unwrap();

        assert_eq!(again, Outcome::Skipped(SkipReason::NothingToUndo));
        assert!(source.exists());
    }

    #[test]
    fn undoing_an_unknown_operation_reports_that_there_is_nothing_to_do() {
        let outcome = executor().undo_operation(9_999).unwrap();

        assert_eq!(outcome, Outcome::Skipped(SkipReason::NothingToUndo));
    }

    #[test]
    fn moving_a_file_onto_itself_is_skipped_rather_than_renamed() {
        let root = TempDir::new().unwrap();
        let file = root.path().join("invoice.pdf");
        write(&file, "content");
        let executor = executor();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: file.clone(),
                    to: file.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Skipped(SkipReason::AlreadyInPlace)]);
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            1,
            "the file was duplicated instead of left alone"
        );
    }

    #[test]
    fn a_no_op_move_is_never_journaled() {
        let root = TempDir::new().unwrap();
        let file = root.path().join("invoice.pdf");
        write(&file, "content");
        let executor = executor();

        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: file.clone(),
                    to: file,
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert!(executor.journal().interrupted().unwrap().is_empty());
        assert!(
            executor
                .journal()
                .operations_in_batch("batch-1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_genuine_move_to_a_different_name_still_runs() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("invoice.pdf");
        let destination = root.path().join("PDFs/invoice.pdf");
        write(&source, "content");

        let outcomes = executor()
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert!(matches!(outcomes[0], Outcome::Applied(_)));
        assert!(destination.exists() && !source.exists());
    }

    #[test]
    fn the_ask_policy_leaves_the_conflict_for_the_user_to_decide() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");
        let executor = executor();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: occupied.clone(),
                }],
                Collision::Ask,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Skipped(SkipReason::DestinationTaken)]);
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        assert!(source.exists(), "nothing moves until the user decides");
        assert_eq!(
            executor
                .journal()
                .skipped_in_batch("batch-1")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn choosing_to_keep_both_resolves_the_waiting_conflict() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");
        let executor = executor();
        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: occupied.clone(),
                }],
                Collision::Ask,
            )
            .unwrap();

        let resolved = executor
            .retry_skipped("batch-1", Collision::Suffix)
            .unwrap();

        assert!(matches!(resolved[0], Outcome::Applied(_)));
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
        assert_eq!(
            fs::read_to_string(root.path().join("archive/notes (2).txt")).unwrap(),
            "new"
        );
        assert!(!source.exists());
    }

    #[test]
    fn a_resolved_conflict_stops_waiting_and_becomes_undoable() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");
        let executor = executor();
        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source,
                    to: occupied,
                }],
                Collision::Ask,
            )
            .unwrap();

        executor
            .retry_skipped("batch-1", Collision::Suffix)
            .unwrap();

        assert!(
            executor
                .journal()
                .skipped_in_batch("batch-1")
                .unwrap()
                .is_empty(),
            "the row was reused, so the conflict is no longer waiting"
        );
        assert_eq!(
            executor
                .journal()
                .applied_in_batch("batch-1")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn choosing_to_leave_them_keeps_the_conflict_untouched() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("notes.txt");
        let occupied = root.path().join("archive/notes.txt");
        write(&source, "new");
        write(&occupied, "existing");
        let executor = executor();
        executor
            .apply(
                "batch-1",
                &[Operation::Move {
                    from: source.clone(),
                    to: occupied.clone(),
                }],
                Collision::Ask,
            )
            .unwrap();

        let resolved = executor.retry_skipped("batch-1", Collision::Skip).unwrap();

        assert_eq!(resolved, [Outcome::Skipped(SkipReason::DestinationTaken)]);
        assert!(source.exists());
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing");
    }
}
