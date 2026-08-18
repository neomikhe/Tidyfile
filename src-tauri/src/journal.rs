use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS operations (
    id           INTEGER PRIMARY KEY,
    batch        TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    source       TEXT    NOT NULL,
    destination  TEXT,
    state        TEXT    NOT NULL,
    detail       TEXT,
    recorded_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS operations_by_batch ON operations(batch);
CREATE INDEX IF NOT EXISTS operations_by_state ON operations(state);
";

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("the journal could not be read or written")]
    Database(#[from] rusqlite::Error),
    #[error("the path is not valid UTF-8 and cannot be journaled")]
    UnrepresentablePath,
    #[error("the journal holds an unknown operation kind")]
    UnknownKind,
    #[error("a move operation was journaled without a destination")]
    MissingDestination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Move { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    Trash { from: PathBuf },
}

impl Operation {
    fn kind(&self) -> &str {
        match self {
            Self::Move { .. } => "move",
            Self::Copy { .. } => "copy",
            Self::Trash { .. } => "trash",
        }
    }

    pub fn source(&self) -> &Path {
        match self {
            Self::Move { from, .. } | Self::Copy { from, .. } | Self::Trash { from } => from,
        }
    }

    fn destination(&self) -> Option<&Path> {
        match self {
            Self::Move { to, .. } | Self::Copy { to, .. } => Some(to),
            Self::Trash { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Planned,
    Done,
    Failed,
    Undone,
}

impl State {
    fn as_text(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Undone => "undone",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordedOperation {
    pub id: i64,
    pub batch: String,
    pub operation: Operation,
}

pub struct Journal {
    connection: Connection,
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, JournalError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&connection)?;
        Ok(Self { connection })
    }

    pub fn record_planned(&self, batch: &str, operation: &Operation) -> Result<i64, JournalError> {
        let source = path_to_text(operation.source())?;
        let destination = operation.destination().map(path_to_text).transpose()?;
        self.connection.execute(
            "INSERT INTO operations (batch, kind, source, destination, state, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                batch,
                operation.kind(),
                source,
                destination,
                State::Planned.as_text(),
                now_seconds()
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn mark(&self, id: i64, state: State, detail: Option<&str>) -> Result<(), JournalError> {
        self.connection.execute(
            "UPDATE operations SET state = ?1, detail = ?2 WHERE id = ?3",
            params![state.as_text(), detail, id],
        )?;
        Ok(())
    }

    pub fn interrupted(&self) -> Result<Vec<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination FROM operations
             WHERE state = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([State::Planned.as_text()], read_row)?;
        collect(rows)
    }

    pub fn settle_interrupted(&self, detail: &str) -> Result<usize, JournalError> {
        let changed = self.connection.execute(
            "UPDATE operations SET state = ?1, detail = ?2 WHERE state = ?3",
            params![State::Failed.as_text(), detail, State::Planned.as_text()],
        )?;
        Ok(changed)
    }

    pub fn applied_in_batch(&self, batch: &str) -> Result<Vec<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination FROM operations
             WHERE batch = ?1 AND state = ?2 ORDER BY id DESC",
        )?;
        let rows = statement.query_map(params![batch, State::Done.as_text()], read_row)?;
        collect(rows)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSummary {
    pub batch: String,
    pub done: usize,
    pub undone: usize,
    pub failed: usize,
    pub recorded_at: i64,
}

impl Journal {
    pub fn recent_batches(&self, limit: usize) -> Result<Vec<BatchSummary>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT batch,
                    SUM(state = 'done'),
                    SUM(state = 'undone'),
                    SUM(state = 'failed'),
                    MAX(recorded_at)
             FROM operations
             WHERE batch NOT LIKE '%:undo'
             GROUP BY batch
             ORDER BY MAX(recorded_at) DESC, batch DESC
             LIMIT ?1",
        )?;
        let capped = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map([capped], |row| {
            Ok(BatchSummary {
                batch: row.get(0)?,
                done: row.get::<_, i64>(1)?.max(0) as usize,
                undone: row.get::<_, i64>(2)?.max(0) as usize,
                failed: row.get::<_, i64>(3)?.max(0) as usize,
                recorded_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(JournalError::from)
    }
}

type Row = (i64, String, String, String, Option<String>);

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn collect<I>(rows: I) -> Result<Vec<RecordedOperation>, JournalError>
where
    I: Iterator<Item = rusqlite::Result<Row>>,
{
    let mut recorded = Vec::new();
    for row in rows {
        let (id, batch, kind, source, destination) = row?;
        recorded.push(RecordedOperation {
            id,
            batch,
            operation: rebuild(&kind, source, destination)?,
        });
    }
    Ok(recorded)
}

fn rebuild(
    kind: &str,
    source: String,
    destination: Option<String>,
) -> Result<Operation, JournalError> {
    match kind {
        "move" => Ok(Operation::Move {
            from: PathBuf::from(source),
            to: destination
                .map(PathBuf::from)
                .ok_or(JournalError::MissingDestination)?,
        }),
        "copy" => Ok(Operation::Copy {
            from: PathBuf::from(source.clone()),
            to: destination
                .map(PathBuf::from)
                .ok_or(JournalError::MissingDestination)?,
        }),
        "trash" => Ok(Operation::Trash {
            from: PathBuf::from(source),
        }),
        _ => Err(JournalError::UnknownKind),
    }
}

fn migrate(connection: &Connection) -> Result<(), JournalError> {
    let version: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    connection.execute_batch(SCHEMA_V1)?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn path_to_text(path: &Path) -> Result<&str, JournalError> {
    path.to_str().ok_or(JournalError::UnrepresentablePath)
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or_default())
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn move_operation(from: &str, to: &str) -> Operation {
        Operation::Move {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }
    }

    #[test]
    fn a_planned_operation_is_reported_as_interrupted() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_planned("batch-1", &move_operation("/a/x.pdf", "/b/x.pdf"))
            .unwrap();

        let interrupted = journal.interrupted().unwrap();

        assert_eq!(interrupted.len(), 1);
        assert_eq!(
            interrupted[0].operation,
            move_operation("/a/x.pdf", "/b/x.pdf")
        );
    }

    #[test]
    fn marking_done_clears_the_interrupted_list() {
        let journal = Journal::open_in_memory().unwrap();
        let id = journal
            .record_planned("batch-1", &move_operation("/a/x.pdf", "/b/x.pdf"))
            .unwrap();

        journal.mark(id, State::Done, None).unwrap();

        assert!(journal.interrupted().unwrap().is_empty());
        assert_eq!(journal.applied_in_batch("batch-1").unwrap().len(), 1);
    }

    #[test]
    fn a_failed_operation_is_neither_interrupted_nor_applied() {
        let journal = Journal::open_in_memory().unwrap();
        let id = journal
            .record_planned("batch-1", &move_operation("/a/x.pdf", "/b/x.pdf"))
            .unwrap();

        journal
            .mark(id, State::Failed, Some("permission denied"))
            .unwrap();

        assert!(journal.interrupted().unwrap().is_empty());
        assert!(journal.applied_in_batch("batch-1").unwrap().is_empty());
    }

    #[test]
    fn undo_order_is_the_reverse_of_application() {
        let journal = Journal::open_in_memory().unwrap();
        for name in ["first", "second", "third"] {
            let id = journal
                .record_planned("batch-1", &move_operation(name, "dest"))
                .unwrap();
            journal.mark(id, State::Done, None).unwrap();
        }

        let applied = journal.applied_in_batch("batch-1").unwrap();

        let sources: Vec<_> = applied
            .iter()
            .map(|record| record.operation.source().to_string_lossy().into_owned())
            .collect();
        assert_eq!(sources, ["third", "second", "first"]);
    }

    #[test]
    fn trash_operations_round_trip_without_a_destination() {
        let journal = Journal::open_in_memory().unwrap();
        let operation = Operation::Trash {
            from: PathBuf::from("/a/old.log"),
        };
        let id = journal.record_planned("batch-2", &operation).unwrap();
        journal.mark(id, State::Done, None).unwrap();

        let applied = journal.applied_in_batch("batch-2").unwrap();

        assert_eq!(applied[0].operation, operation);
    }

    #[test]
    fn batches_do_not_leak_into_each_other() {
        let journal = Journal::open_in_memory().unwrap();
        let first = journal
            .record_planned("batch-a", &move_operation("a", "b"))
            .unwrap();
        let second = journal
            .record_planned("batch-b", &move_operation("c", "d"))
            .unwrap();
        journal.mark(first, State::Done, None).unwrap();
        journal.mark(second, State::Done, None).unwrap();

        assert_eq!(journal.applied_in_batch("batch-a").unwrap().len(), 1);
        assert_eq!(journal.applied_in_batch("batch-b").unwrap().len(), 1);
    }

    #[test]
    fn reopening_the_same_file_keeps_the_history() {
        let folder = tempfile::TempDir::new().unwrap();
        let path = folder.path().join("journal.sqlite");

        let id = {
            let journal = Journal::open(&path).unwrap();
            journal
                .record_planned("batch-1", &move_operation("/a/x.pdf", "/b/x.pdf"))
                .unwrap()
        };

        let reopened = Journal::open(&path).unwrap();
        reopened.mark(id, State::Done, None).unwrap();

        assert_eq!(reopened.applied_in_batch("batch-1").unwrap().len(), 1);
    }

    #[test]
    fn recent_batches_summarises_each_batch_by_state() {
        let journal = Journal::open_in_memory().unwrap();
        let done = journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        let failed = journal
            .record_planned("batch-1", &move_operation("c", "d"))
            .unwrap();
        journal.mark(done, State::Done, None).unwrap();
        journal.mark(failed, State::Failed, Some("denied")).unwrap();

        let summaries = journal.recent_batches(10).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!((summaries[0].done, summaries[0].failed), (1, 1));
    }

    #[test]
    fn undo_batches_are_kept_out_of_the_activity_list() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        journal
            .record_planned("batch-1:undo", &move_operation("b", "a"))
            .unwrap();

        let summaries = journal.recent_batches(10).unwrap();

        let names: Vec<_> = summaries.iter().map(|entry| entry.batch.clone()).collect();
        assert_eq!(names, ["batch-1"]);
    }

    #[test]
    fn the_activity_limit_is_honoured() {
        let journal = Journal::open_in_memory().unwrap();
        for index in 0..5 {
            journal
                .record_planned(&format!("batch-{index}"), &move_operation("a", "b"))
                .unwrap();
        }

        assert_eq!(journal.recent_batches(2).unwrap().len(), 2);
    }

    #[test]
    fn an_undone_batch_reports_its_undone_count() {
        let journal = Journal::open_in_memory().unwrap();
        let id = journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        journal.mark(id, State::Done, None).unwrap();
        journal.mark(id, State::Undone, None).unwrap();

        let summaries = journal.recent_batches(10).unwrap();

        assert_eq!((summaries[0].done, summaries[0].undone), (0, 1));
    }

    #[test]
    fn settling_clears_interrupted_operations_and_reports_how_many() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        journal
            .record_planned("batch-1", &move_operation("c", "d"))
            .unwrap();

        let settled = journal.settle_interrupted("interrupted run").unwrap();

        assert_eq!(settled, 2);
        assert!(journal.interrupted().unwrap().is_empty());
    }

    #[test]
    fn settling_does_not_disturb_finished_operations() {
        let journal = Journal::open_in_memory().unwrap();
        let done = journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        journal.mark(done, State::Done, None).unwrap();
        journal
            .record_planned("batch-1", &move_operation("c", "d"))
            .unwrap();

        journal.settle_interrupted("interrupted run").unwrap();

        assert_eq!(journal.applied_in_batch("batch-1").unwrap().len(), 1);
    }

    #[test]
    fn settling_nothing_reports_zero() {
        let journal = Journal::open_in_memory().unwrap();

        assert_eq!(journal.settle_interrupted("interrupted run").unwrap(), 0);
    }
}
