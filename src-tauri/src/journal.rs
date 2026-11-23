use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

const SCHEMA_VERSION: i32 = 2;

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

const SCHEMA_V2: &str = "
CREATE TABLE IF NOT EXISTS batches (
    batch        TEXT    PRIMARY KEY,
    origin       TEXT    NOT NULL CHECK (origin IN ('manual','watch')),
    folder       TEXT,
    recorded_at  INTEGER NOT NULL
);
";

const RECENT_BATCHES: &str = "
SELECT batch,
       SUM(state = 'done'),
       SUM(state = 'undone'),
       SUM(state = 'failed'),
       SUM(state = 'skipped'),
       MAX(operations.recorded_at),
       batches.origin,
       batches.folder
FROM operations LEFT JOIN batches USING (batch)
WHERE batch NOT LIKE '%:undo'
GROUP BY batch
ORDER BY MAX(operations.recorded_at) DESC, batch DESC
LIMIT ?1
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

    pub fn destination(&self) -> Option<&Path> {
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
    Skipped,
}

impl State {
    pub fn from_text(text: &str) -> Option<Self> {
        match text {
            "planned" => Some(Self::Planned),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "undone" => Some(Self::Undone),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }

    fn as_text(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Undone => "undone",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Manual,
    Watch,
}

impl Origin {
    fn as_text(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Watch => "watch",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordedOperation {
    pub id: i64,
    pub batch: String,
    pub operation: Operation,
    pub state: State,
    pub recorded_at: i64,
    pub detail: Option<String>,
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

    // WHY: a folder that is not UTF-8 only loses its label in Activity, never the batch itself
    pub fn record_batch(
        &self,
        batch: &str,
        origin: Origin,
        folder: Option<&Path>,
    ) -> Result<(), JournalError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO batches (batch, origin, folder, recorded_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                batch,
                origin.as_text(),
                folder.and_then(Path::to_str),
                now_seconds()
            ],
        )?;
        Ok(())
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
            "SELECT id, batch, kind, source, destination, state, recorded_at, detail FROM operations
             WHERE state = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([State::Planned.as_text()], read_row)?;
        collect(rows)
    }

    pub fn operation(&self, id: i64) -> Result<Option<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination, state, recorded_at, detail FROM operations WHERE id = ?1",
        )?;
        let rows = statement.query_map([id], read_row)?;
        Ok(collect(rows)?.into_iter().next())
    }

    pub fn operations_in_batch(&self, batch: &str) -> Result<Vec<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination, state, recorded_at, detail FROM operations
             WHERE batch = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([batch], read_row)?;
        collect(rows)
    }

    pub fn retarget(&self, id: i64, destination: &Path) -> Result<(), JournalError> {
        self.connection.execute(
            "UPDATE operations SET destination = ?1 WHERE id = ?2",
            params![path_to_text(destination)?, id],
        )?;
        Ok(())
    }

    pub fn settle_interrupted(&self, detail: &str) -> Result<usize, JournalError> {
        let changed = self.connection.execute(
            "UPDATE operations SET state = ?1, detail = ?2 WHERE state = ?3",
            params![State::Failed.as_text(), detail, State::Planned.as_text()],
        )?;
        Ok(changed)
    }

    pub fn skipped_in_batch(&self, batch: &str) -> Result<Vec<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination, state, recorded_at, detail FROM operations
             WHERE batch = ?1 AND state = ?2 ORDER BY id",
        )?;
        let rows = statement.query_map(params![batch, State::Skipped.as_text()], read_row)?;
        collect(rows)
    }

    pub fn applied_in_batch(&self, batch: &str) -> Result<Vec<RecordedOperation>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, batch, kind, source, destination, state, recorded_at, detail FROM operations
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
    pub skipped: usize,
    pub recorded_at: i64,
    pub origin: Option<String>,
    pub folder: Option<String>,
}

impl Journal {
    pub fn recent_batches(&self, limit: usize) -> Result<Vec<BatchSummary>, JournalError> {
        let mut statement = self.connection.prepare(RECENT_BATCHES)?;
        let capped = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map([capped], read_summary)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(JournalError::from)
    }
}

fn read_summary(row: &rusqlite::Row) -> rusqlite::Result<BatchSummary> {
    Ok(BatchSummary {
        batch: row.get(0)?,
        done: row.get::<_, i64>(1)?.max(0) as usize,
        undone: row.get::<_, i64>(2)?.max(0) as usize,
        failed: row.get::<_, i64>(3)?.max(0) as usize,
        skipped: row.get::<_, i64>(4)?.max(0) as usize,
        recorded_at: row.get(5)?,
        origin: row.get(6)?,
        folder: row.get(7)?,
    })
}

type Row = (
    i64,
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    Option<String>,
);

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn collect<I>(rows: I) -> Result<Vec<RecordedOperation>, JournalError>
where
    I: Iterator<Item = rusqlite::Result<Row>>,
{
    let mut recorded = Vec::new();
    for row in rows {
        let (id, batch, kind, source, destination, state, recorded_at, detail) = row?;
        recorded.push(RecordedOperation {
            id,
            batch,
            operation: rebuild(&kind, source, destination)?,
            state: State::from_text(&state).ok_or(JournalError::UnknownKind)?,
            recorded_at,
            detail,
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
    let upgrade = connection.unchecked_transaction()?;
    for (target, schema) in [(1, SCHEMA_V1), (2, SCHEMA_V2)] {
        if version < target {
            upgrade.execute_batch(schema)?;
        }
    }
    upgrade.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    upgrade.commit()?;
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

    fn version_one(connection: &Connection) {
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
    }

    fn insert_done_move(connection: &Connection, batch: &str, from: &str, to: &str) {
        connection
            .execute(
                "INSERT INTO operations (batch, kind, source, destination, state, recorded_at)
                 VALUES (?1, 'move', ?2, ?3, 'done', 1700000000)",
                params![batch, from, to],
            )
            .unwrap();
    }

    fn only_summary(journal: &Journal) -> BatchSummary {
        let mut summaries = journal.recent_batches(10).unwrap();
        assert_eq!(summaries.len(), 1);
        summaries.remove(0)
    }

    #[test]
    fn a_version_one_journal_upgrades_without_losing_anything() {
        let connection = Connection::open_in_memory().unwrap();
        version_one(&connection);
        insert_done_move(&connection, "old-batch", "/a/x.pdf", "/b/x.pdf");

        let journal = Journal::from_connection(connection).unwrap();

        let summary = only_summary(&journal);
        assert_eq!((summary.batch.as_str(), summary.done), ("old-batch", 1));
        assert_eq!((summary.origin, summary.folder), (None, None));
        assert_eq!(
            journal.applied_in_batch("old-batch").unwrap()[0].operation,
            move_operation("/a/x.pdf", "/b/x.pdf")
        );
        let version: i32 = journal
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn a_recorded_batch_reports_its_origin_and_folder() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_batch("b1", Origin::Watch, Some(Path::new("/w/Downloads")))
            .unwrap();
        journal
            .record_planned("b1", &move_operation("/w/Downloads/x.pdf", "/d/x.pdf"))
            .unwrap();

        let summary = only_summary(&journal);

        assert_eq!(summary.origin.as_deref(), Some("watch"));
        assert_eq!(summary.folder.as_deref(), Some("/w/Downloads"));
    }

    #[test]
    fn recording_a_batch_twice_keeps_the_first() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_batch("b1", Origin::Watch, Some(Path::new("/w")))
            .unwrap();
        journal.record_batch("b1", Origin::Manual, None).unwrap();
        journal
            .record_planned("b1", &move_operation("a", "b"))
            .unwrap();

        let summary = only_summary(&journal);

        assert_eq!(
            (summary.origin.as_deref(), summary.folder.as_deref()),
            (Some("watch"), Some("/w"))
        );
    }

    #[cfg(windows)]
    fn unrecordable_folder() -> PathBuf {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_wide(&[0x0077, 0xD800]))
    }

    #[cfg(unix)]
    fn unrecordable_folder() -> PathBuf {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(b"/w/caf\xe9".to_vec()))
    }

    #[test]
    fn a_folder_that_is_not_utf8_is_recorded_without_it() {
        let journal = Journal::open_in_memory().unwrap();
        journal
            .record_batch("b1", Origin::Watch, Some(&unrecordable_folder()))
            .unwrap();
        journal
            .record_planned("b1", &move_operation("a", "b"))
            .unwrap();

        let summary = only_summary(&journal);

        assert_eq!(
            (summary.origin.as_deref(), summary.folder),
            (Some("watch"), None)
        );
    }

    #[test]
    fn undo_still_works_after_the_upgrade() {
        let data = tempfile::TempDir::new().unwrap();
        let files = tempfile::TempDir::new().unwrap();
        let source = files.path().join("invoice.pdf");
        let moved = files.path().join("archive").join("invoice.pdf");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::write(&moved, "content").unwrap();
        let path = data.path().join("journal.sqlite");
        let connection = Connection::open(&path).unwrap();
        version_one(&connection);
        insert_done_move(
            &connection,
            "old-batch",
            source.to_str().unwrap(),
            moved.to_str().unwrap(),
        );
        drop(connection);

        let service = crate::service::Tidyfile::open(&path).unwrap();
        let report = service.undo("old-batch").unwrap();

        assert_eq!(report.applied, 1);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "content");
        assert!(!moved.exists());
        assert_eq!(service.activity(10).unwrap()[0].undone, 1);
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
    fn a_failure_detail_is_read_back_with_the_operation() {
        let journal = Journal::open_in_memory().unwrap();
        let id = journal
            .record_planned("batch-1", &move_operation("/a/x.pdf", "/b/x.pdf"))
            .unwrap();
        journal
            .mark(id, State::Failed, Some("permission denied"))
            .unwrap();

        let recorded = journal.operations_in_batch("batch-1").unwrap();

        assert_eq!(recorded[0].detail.as_deref(), Some("permission denied"));
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

    #[test]
    fn a_skipped_operation_is_reported_in_the_history() {
        let journal = Journal::open_in_memory().unwrap();
        let id = journal
            .record_planned("batch-1", &move_operation("a", "b"))
            .unwrap();
        journal
            .mark(id, State::Skipped, Some("destination already exists"))
            .unwrap();

        let summaries = journal.recent_batches(10).unwrap();

        assert_eq!(
            summaries[0].skipped, 1,
            "a batch that skipped everything must not look like it did nothing"
        );
    }
}
