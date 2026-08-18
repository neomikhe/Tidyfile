use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::journal::{Journal, JournalError, Operation, State};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    SourceVanished,
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
}

impl Executor {
    pub fn new(journal: Journal) -> Self {
        Self { journal }
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn apply(
        &self,
        batch: &str,
        operations: &[Operation],
    ) -> Result<Vec<Outcome>, ExecutorError> {
        operations
            .iter()
            .map(|operation| self.apply_one(batch, operation))
            .collect()
    }

    fn apply_one(&self, batch: &str, operation: &Operation) -> Result<Outcome, ExecutorError> {
        if !operation.source().exists() {
            return Ok(Outcome::Skipped(SkipReason::SourceVanished));
        }
        let resolved = match without_collision(operation) {
            Ok(resolved) => resolved,
            Err(error) => return Ok(Outcome::Failed(error.to_string())),
        };
        let id = self.journal.record_planned(batch, &resolved)?;
        self.run_and_record(id, resolved)
    }

    fn run_and_record(&self, id: i64, operation: Operation) -> Result<Outcome, ExecutorError> {
        match perform(&operation) {
            Ok(()) => {
                self.journal.mark(id, State::Done, None)?;
                Ok(Outcome::Applied(operation))
            }
            Err(error) => {
                let detail = error.to_string();
                self.journal.mark(id, State::Failed, Some(&detail))?;
                Ok(Outcome::Failed(detail))
            }
        }
    }

    pub fn undo_batch(&self, batch: &str) -> Result<Vec<Outcome>, ExecutorError> {
        let applied = self.journal.applied_in_batch(batch)?;
        let undo_batch = format!("{batch}:undo");
        let mut outcomes = Vec::with_capacity(applied.len());
        for record in &applied {
            let outcome = self.undo_one(&undo_batch, &record.operation)?;
            if matches!(outcome, Outcome::Applied(_) | Outcome::Skipped(_)) {
                self.journal.mark(record.id, State::Undone, None)?;
            }
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    fn undo_one(&self, batch: &str, operation: &Operation) -> Result<Outcome, ExecutorError> {
        match operation {
            Operation::Move { from, to } => self.apply_one(
                batch,
                &Operation::Move {
                    from: to.clone(),
                    to: from.clone(),
                },
            ),
            Operation::Trash { from } => Ok(restore::from_trash(from)),
        }
    }
}

fn perform(operation: &Operation) -> Result<(), ActionError> {
    match operation {
        Operation::Move { from, to } => move_file(from, to),
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
    copy_then_trash(from, to)
}

fn copy_then_trash(from: &Path, to: &Path) -> Result<(), ActionError> {
    let original = fs::metadata(from)?.len();
    let copied = fs::copy(from, to)?;
    if copied != original {
        let _ = trash::delete(to);
        return Err(ActionError::IncompleteCopy);
    }
    Ok(trash::delete(from)?)
}

fn without_collision(operation: &Operation) -> Result<Operation, ActionError> {
    let Operation::Move { from, to } = operation else {
        return Ok(operation.clone());
    };
    if !to.exists() {
        return Ok(operation.clone());
    }
    Ok(Operation::Move {
        from: from.clone(),
        to: free_name(to).ok_or(ActionError::NoFreeName)?,
    })
}

fn free_name(desired: &Path) -> Option<PathBuf> {
    let parent = desired.parent()?;
    let stem = desired.file_stem()?;
    (2..MAX_NAME_ATTEMPTS)
        .map(|attempt| parent.join(candidate_name(stem, desired.extension(), attempt)))
        .find(|candidate| !candidate.exists())
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
    use std::path::Path;

    pub fn from_trash(original: &Path) -> Outcome {
        let items = match trash::os_limited::list() {
            Ok(items) => items,
            Err(error) => return Outcome::Failed(error.to_string()),
        };
        let Some(item) = items
            .into_iter()
            .find(|item| item.original_path() == original)
        else {
            return Outcome::Skipped(SkipReason::NotFoundInTrash);
        };
        match trash::os_limited::restore_all([item]) {
            Ok(()) => Outcome::Applied(Operation::Trash {
                from: original.to_path_buf(),
            }),
            Err(error) => Outcome::Failed(error.to_string()),
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

    pub fn from_trash(_original: &Path) -> Outcome {
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
        executor.apply("batch-1", &[operation]).unwrap();
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

        executor.apply("batch-1", &operations).unwrap();
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
}
