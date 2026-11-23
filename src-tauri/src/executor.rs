use std::ffi::OsString;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::journal::{Journal, JournalError, Operation, RecordedOperation, State};
use crate::native;
use crate::recycle::{self, Verdict};

const MAX_NAME_ATTEMPTS: u32 = 100;
const COPY_BUFFER: usize = 1024 * 1024;
pub const PARTIAL_EXTENSION: &str = "tidyfile-partial";
const NAME_LIMIT: usize = 255;
const NUMBER_ROOM: usize = " (99)".len();
const MARKED_NAME_ROOM: usize = NAME_LIMIT - NUMBER_ROOM - PARTIAL_EXTENSION.len() - 1;

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
    #[error("the copy did not reproduce the whole file{0}")]
    IncompleteCopy(Leftover),
    #[error("the original could not be sent to the trash, so it stays where it was{0}")]
    SourceKept(Leftover),
    #[error("the original was replaced while it was being moved, so both are kept{0}")]
    SourceReplaced(Leftover),
    #[error("the file was replaced by something that is not a plain file")]
    NotAPlainFile,
    #[cfg(windows)]
    #[error("the file is in use by another program")]
    InUse,
    #[cfg(windows)]
    #[error("the file is encrypted by Windows, and a copy would not stay encrypted")]
    Encrypted,
    #[cfg(windows)]
    #[error(
        "a folder or the name ends in a space or a dot, which Windows would read as another file"
    )]
    MisleadingName,
    #[error("the trash said it was done but the file is still there, so check the Recycle Bin{0}")]
    TrashMissed(Leftover),
    #[error("every candidate name was already taken")]
    NoFreeName,
    #[error("the Recycle Bin is not available on this drive")]
    NoRecycleBin,
    #[error("the file is larger than the Recycle Bin of this drive")]
    TooBigForRecycleBin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Leftover {
    Cleared,
    KeptAs(String),
    StillCalled(String),
}

impl std::fmt::Display for Leftover {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cleared => Ok(()),
            Self::KeptAs(name) => write!(formatter, "; what was copied is kept as {name}"),
            Self::StillCalled(name) => write!(
                formatter,
                "; what was copied could not be renamed and is still called {name}"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotRecycled {
    NoRecycleBin,
    TooBigForRecycleBin,
}

// WHY: the journal keeps the refusal as its message, so the reason is read back from that one wording
impl NotRecycled {
    pub fn from_detail(detail: &str) -> Option<Self> {
        [Self::NoRecycleBin, Self::TooBigForRecycleBin]
            .into_iter()
            .find(|reason| reason.refusal().to_string() == detail)
    }

    fn refusal(self) -> ActionError {
        match self {
            Self::NoRecycleBin => ActionError::NoRecycleBin,
            Self::TooBigForRecycleBin => ActionError::TooBigForRecycleBin,
        }
    }
}

pub fn undo_batch_of(batch: &str) -> String {
    format!("{batch}:undo")
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

type Judge = fn(&Path, u64) -> Verdict;
type Copier = fn(&Path, &mut fs::File, &mut fs::File) -> std::io::Result<u64>;
type Renamer = fn(&Path, &Path) -> std::io::Result<()>;
type Trasher = fn(&Path) -> Result<(), trash::Error>;

#[derive(Clone, Copy)]
struct Disk {
    judge: Judge,
    copy: Copier,
    rename: Renamer,
    trash: Trasher,
}

impl Disk {
    fn real() -> Self {
        Self {
            judge: recycle::judge,
            copy: |_, source, copy| {
                std::io::copy(&mut BufReader::with_capacity(COPY_BUFFER, source), copy)
            },
            rename: |from, to| fs::rename(from, to),
            trash: by_shell,
        }
    }
}

pub struct Executor {
    journal: Journal,
    disk: Disk,
}

impl Executor {
    pub fn new(journal: Journal) -> Self {
        Self {
            journal,
            disk: Disk::real(),
        }
    }

    #[cfg(test)]
    pub fn judging_recycle_bin(journal: Journal, judge: Judge) -> Self {
        Self {
            journal,
            disk: Disk {
                judge,
                ..Disk::real()
            },
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
            .map(|operation| self.apply_one(batch, operation, on_collision))
            .collect()
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
        if let Err(refusal) = refuse_misleading_names(operation) {
            return self.give_up(id, &refusal.to_string());
        }
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
        match perform(&operation, self.disk) {
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
        let batch = &undo_batch_of(&record.batch);
        match &record.operation {
            Operation::Move { from, to } => self.apply_one(
                batch,
                &Operation::Move {
                    from: to.clone(),
                    to: from.clone(),
                },
                Collision::Suffix,
            ),
            Operation::Copy { to, .. } => self.apply_one(
                batch,
                &Operation::Trash { from: to.clone() },
                Collision::Suffix,
            ),
            Operation::Trash { from } => Ok(restore::from_trash(from, record.recorded_at)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    len: u64,
    modified: Option<SystemTime>,
}

// WHY: a folder or link swapped in for the planned file reads as size 0 and would go to the bin whole
impl Snapshot {
    fn of(path: &Path) -> Result<Self, ActionError> {
        let data = fs::symlink_metadata(path)?;
        if !data.is_file() {
            return Err(ActionError::NotAPlainFile);
        }
        Ok(Self::read(&data))
    }

    fn read(data: &fs::Metadata) -> Self {
        Self {
            len: data.len(),
            modified: data.modified().ok(),
        }
    }

    fn describes(self, file: &fs::File) -> bool {
        file.metadata()
            .is_ok_and(|data| data.is_file() && Self::read(&data) == self)
    }
}

struct Transfer<'a> {
    from: &'a Path,
    to: &'a Path,
    found: Snapshot,
}

fn perform(operation: &Operation, disk: Disk) -> Result<(), ActionError> {
    let found = Snapshot::of(operation.source())?;
    match operation {
        Operation::Move { from, to } => move_file(&Transfer { from, to, found }, disk),
        Operation::Copy { from, to } => copy_file(&Transfer { from, to, found }, disk).map(drop),
        Operation::Trash { from } => send_to_trash(from, found.len, disk),
    }
}

fn send_to_trash(path: &Path, size: u64, disk: Disk) -> Result<(), ActionError> {
    if let Some(refusal) = trash_refusal((disk.judge)(path, size)) {
        return Err(refusal);
    }
    shell_trash(path, disk.trash)?;
    if !is_gone(path) {
        return Err(ActionError::TrashMissed(Leftover::Cleared));
    }
    Ok(())
}

// WHY: the shell resolves the path again on its own, so the trash only takes a path that reads the same to it
fn shell_trash(path: &Path, trash: Trasher) -> Result<(), ActionError> {
    refuse_misleading(path)?;
    Ok(trash(path)?)
}

fn move_file(transfer: &Transfer<'_>, disk: Disk) -> Result<(), ActionError> {
    if let Some(parent) = transfer.to.parent() {
        fs::create_dir_all(parent)?;
    }
    match (disk.rename)(transfer.from, transfer.to) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() != std::io::ErrorKind::CrossesDevices => {
            return Err(refusal_of(error));
        }
        Err(_) => {}
    }
    if let Some(refusal) = move_refusal((disk.judge)(transfer.from, transfer.found.len)) {
        return Err(refusal);
    }
    let guard = copy_file(transfer, disk)?;
    let_go(transfer, guard, disk.trash)
}

// WHY: the guard pins the copied file, but the trash acts on the path, which a save-by-rename may have replaced
fn let_go(transfer: &Transfer<'_>, guard: fs::File, trash: Trasher) -> Result<(), ActionError> {
    if is_gone(transfer.from) {
        return Ok(());
    }
    if !is_the_same_file(&guard, transfer.from) {
        return Err(ActionError::SourceReplaced(set_aside(transfer.to)));
    }
    let released = shell_trash(transfer.from, trash);
    drop(guard);
    if is_gone(transfer.from) {
        return Ok(());
    }
    let leftover = set_aside(transfer.to);
    if released.is_ok() {
        return Err(ActionError::TrashMissed(leftover));
    }
    Err(ActionError::SourceKept(leftover))
}

fn by_shell(path: &Path) -> Result<(), trash::Error> {
    trash::delete(path)
}

// WHY: FAT and some network drives report no links at all, which is still nobody else's file
fn is_unshared(links: Option<u64>) -> bool {
    links.is_some_and(|count| count <= 1)
}

fn is_the_same_file(guard: &fs::File, path: &Path) -> bool {
    let identities = (native::identity_of(guard), native::identity_at(path));
    matches!(identities, (Some(held), Some(found)) if held == found)
}

fn is_gone(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

// WHY: with no bin Windows deletes for good yet reports success; a file too big raises a blocking dialog
fn trash_refusal(verdict: Verdict) -> Option<ActionError> {
    match verdict {
        Verdict::Recyclable => None,
        Verdict::NoRecycleBin | Verdict::Unknown { .. } => Some(ActionError::NoRecycleBin),
        Verdict::TooBig => Some(ActionError::TooBigForRecycleBin),
    }
}

// WHY: a move only trashes what it copied and verified; only a bin that may be too small brings the dialog
fn move_refusal(verdict: Verdict) -> Option<ActionError> {
    let may_be_too_small = matches!(
        verdict,
        Verdict::TooBig
            | Verdict::Unknown {
                fits_default: false
            }
    );
    may_be_too_small.then_some(ActionError::TooBigForRecycleBin)
}

fn copy_file(transfer: &Transfer<'_>, disk: Disk) -> Result<fs::File, ActionError> {
    let mut source = open_source(transfer)?;
    let mut duplicate = open_slot(transfer.to)?;
    let written = write_copy(transfer, disk.copy, (&mut source, &mut duplicate));
    drop(duplicate);
    if written.is_err() {
        return Err(ActionError::IncompleteCopy(discard_partial(
            transfer.to,
            disk.judge,
        )));
    }
    if !transfer.found.describes(&source) {
        return Err(ActionError::IncompleteCopy(set_aside(transfer.to)));
    }
    Ok(source)
}

fn open_source(transfer: &Transfer<'_>) -> Result<fs::File, ActionError> {
    let source = native::open_guarded(transfer.from).map_err(refusal_of)?;
    if !transfer.found.describes(&source) {
        return Err(ActionError::IncompleteCopy(Leftover::Cleared));
    }
    refuse_if_encrypted(&source)?;
    Ok(source)
}

// WHY: the claimed name may have been swapped for a link since, and writing through it would change another file
fn open_slot(path: &Path) -> Result<fs::File, ActionError> {
    let slot = native::open_slot(path)?;
    let is_empty_file = slot
        .metadata()
        .is_ok_and(|data| data.is_file() && data.len() == 0);
    if !is_empty_file || !is_unshared(native::link_count(&slot)) {
        return Err(ActionError::Move(std::io::ErrorKind::AlreadyExists.into()));
    }
    Ok(slot)
}

// WHY: reopening a copy that inherited the read-only flag is refused, so it is written and checked through one handle
fn write_copy(
    transfer: &Transfer<'_>,
    copy: Copier,
    (source, duplicate): (&mut fs::File, &mut fs::File),
) -> std::io::Result<()> {
    copy(transfer.from, source, duplicate)?;
    duplicate.sync_all()?;
    if duplicate.metadata()?.len() != transfer.found.len {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    carry_attributes(transfer, source, duplicate)
}

// WHY: the content is already verified on disk; a date or flag the drive will not keep does not undo that
fn carry_attributes(
    transfer: &Transfer<'_>,
    source: &fs::File,
    duplicate: &fs::File,
) -> std::io::Result<()> {
    native::carry_mark_of_the_web(transfer.from, transfer.to);
    native::carry_extended_attributes(source, duplicate)?;
    if let Some(modified) = transfer.found.modified {
        let _ = duplicate.set_modified(modified);
    }
    if let Ok(data) = source.metadata() {
        let _ = duplicate.set_permissions(data.permissions());
    }
    Ok(())
}

#[cfg(windows)]
fn refusal_of(error: std::io::Error) -> ActionError {
    if native::is_in_use(&error) {
        return ActionError::InUse;
    }
    ActionError::Move(error)
}

#[cfg(not(windows))]
fn refusal_of(error: std::io::Error) -> ActionError {
    ActionError::Move(error)
}

// WHY: a copy written by hand from an EFS file lands in plain text, so it is refused rather than decrypted
#[cfg(windows)]
fn refuse_if_encrypted(source: &fs::File) -> Result<(), ActionError> {
    if native::is_encrypted(source) {
        return Err(ActionError::Encrypted);
    }
    Ok(())
}

#[cfg(not(windows))]
fn refuse_if_encrypted(_source: &fs::File) -> Result<(), ActionError> {
    Ok(())
}

fn refuse_misleading_names(operation: &Operation) -> Result<(), ActionError> {
    refuse_misleading(operation.source())?;
    destination_of(operation).map_or(Ok(()), refuse_misleading)
}

// WHY: the shell drops a trailing space or dot from any folder or name, so `Old.\a.pdf` would trash `Old\a.pdf`
#[cfg(windows)]
fn refuse_misleading(path: &Path) -> Result<(), ActionError> {
    let Some(seen) = as_the_shell_sees(path) else {
        return Err(ActionError::MisleadingName);
    };
    if has_misleading_part(path) || has_misleading_part(&seen) || is_an_alias(path) {
        return Err(ActionError::MisleadingName);
    }
    Ok(())
}

#[cfg(windows)]
fn has_misleading_part(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if native::ends_in_space_or_dot(name))
    })
}

// WHY: trash hands the shell the canonical parent, and a folder still to be made is judged below the deepest one there
#[cfg(windows)]
fn as_the_shell_sees(path: &Path) -> Option<PathBuf> {
    if let Some(seen) = recycle::as_trash_resolves(path) {
        return Some(seen);
    }
    let parent = path.parent()?;
    let resolved = parent.ancestors().find_map(|ancestor| {
        let rest = parent.strip_prefix(ancestor).ok()?;
        Some(ancestor.canonicalize().ok()?.join(rest))
    })?;
    Some(resolved.join(path.file_name()?))
}

// WHY: NTFS answers to an 8.3 alias too, whose long name may end in a dot; links are left to the plain-file check
#[cfg(windows)]
fn is_an_alias(path: &Path) -> bool {
    if fs::symlink_metadata(path).is_ok_and(|data| data.file_type().is_symlink()) {
        return false;
    }
    match path.canonicalize() {
        Ok(real) => !matches!(
            (path.file_name(), real.file_name()),
            (Some(given), Some(long)) if given.eq_ignore_ascii_case(long)
        ),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

#[cfg(not(windows))]
fn refuse_misleading(_path: &Path) -> Result<(), ActionError> {
    Ok(())
}

fn discard_partial(partial: &Path, judge: Judge) -> Leftover {
    let Some(size) = fs::symlink_metadata(partial)
        .ok()
        .filter(|data| data.is_file() && data.len() > 0)
        .map(|data| data.len())
    else {
        return Leftover::Cleared;
    };
    let may_trash = move_refusal(judge(partial, size)).is_none();
    if may_trash && shell_trash(partial, by_shell).is_ok() && is_gone(partial) {
        return Leftover::Cleared;
    }
    set_aside(partial)
}

// WHY: fs::rename replaces what it lands on, so the marked name is claimed first exactly as reserve does
fn set_aside(partial: &Path) -> Leftover {
    let still_called = || Leftover::StillCalled(name_of(partial));
    let Ok(Some(slot)) = reserve(&marked_as_partial(partial), Collision::Suffix) else {
        return still_called();
    };
    if fs::rename(partial, &slot).is_err() {
        discard_if_empty(&slot);
        return still_called();
    }
    Leftover::KeptAs(name_of(&slot))
}

fn marked_as_partial(path: &Path) -> PathBuf {
    let kept = shortened(&name_of(path), MARKED_NAME_ROOM);
    path.with_file_name(format!("{kept}.{PARTIAL_EXTENSION}"))
}

// WHY: a name already near the limit leaves no room for the mark and a number, so its end is cut instead
fn shortened(name: &str, room: usize) -> String {
    let mut used = 0;
    name.chars()
        .take_while(|&character| {
            used += native::units_of(character);
            used <= room
        })
        .collect()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
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
    if let Some(destination) = destination_of(operation) {
        discard_if_empty(destination);
    }
}

fn discard_if_empty(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|data| data.is_file() && data.len() == 0) {
        let _ = shell_trash(path, by_shell);
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

    fn judging(judge: Judge) -> Executor {
        Executor::judging_recycle_bin(Journal::open_in_memory().unwrap(), judge)
    }

    fn without_recycle_bin() -> Executor {
        judging(|_, _| Verdict::NoRecycleBin)
    }

    fn with_a_small_recycle_bin() -> Executor {
        judging(|_, _| Verdict::TooBig)
    }

    const TOO_BIG: &str = "the file is larger than the Recycle Bin of this drive";

    #[cfg(windows)]
    struct Pinned {
        _handle: fs::File,
    }

    // WHY: a handle that grants no share-delete stops both rename and trash, as a program holding the file does
    #[cfg(windows)]
    fn pinned_against_removal(file: &Path) -> Pinned {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
        let open = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .open(file);
        Pinned {
            _handle: open.unwrap(),
        }
    }

    #[cfg(unix)]
    struct Pinned(PathBuf);

    // WHY: a folder that cannot be written stops both rename and trash of what it holds
    #[cfg(unix)]
    fn pinned_against_removal(file: &Path) -> Pinned {
        use std::os::unix::fs::PermissionsExt;
        let folder = file.parent().unwrap().to_path_buf();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
        Pinned(folder)
    }

    #[cfg(unix)]
    impl Drop for Pinned {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn trashing_a_file_larger_than_the_recycle_bin_fails_and_keeps_the_file() {
        let root = TempDir::new().unwrap();
        let video = root.path().join("holiday.mkv");
        write(&video, "only copy");
        let executor = with_a_small_recycle_bin();

        let outcomes = executor
            .apply(
                "batch-1",
                &[Operation::Trash {
                    from: video.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(TOO_BIG.into())]);
        assert_eq!(fs::read_to_string(&video).unwrap(), "only copy");
        let recorded = executor.journal().operations_in_batch("batch-1").unwrap();
        assert_eq!(recorded.len(), 1, "the refusal must show in the history");
        assert_eq!(recorded[0].state, State::Failed);
    }

    #[test]
    fn undoing_a_copy_larger_than_the_recycle_bin_fails_and_keeps_the_copy() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("video.mkv");
        let duplicate = root.path().join("backup/video.mkv");
        write(&source, "original");
        let executor = with_a_small_recycle_bin();
        let copy = copy_of(&source, &duplicate);
        executor.apply("copy", &[copy], Collision::Suffix).unwrap();

        let undone = executor.undo_batch("copy").unwrap();

        assert_eq!(undone, [Outcome::Failed(TOO_BIG.into())]);
        assert_eq!(fs::read_to_string(&duplicate).unwrap(), "original");
        assert_eq!(
            executor.journal().applied_in_batch("copy").unwrap().len(),
            1,
            "the copy stays undoable"
        );
    }

    #[test]
    fn a_move_that_must_copy_a_source_too_big_for_its_bin_makes_no_copy() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        let destination = root.path().join("out/video.mkv");
        write(&source, "only copy");
        let executor = executor_with(Disk {
            judge: |_, _| Verdict::TooBig,
            rename: across_drives,
            ..Disk::real()
        });

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(TOO_BIG.into())]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "only copy");
        assert!(!destination.exists(), "a copy was made before refusing");
        let recorded = executor.journal().operations_in_batch("move").unwrap();
        assert_eq!(recorded[0].state, State::Failed);
    }

    #[test]
    fn undoing_a_move_that_must_copy_back_a_file_too_big_for_its_bin_leaves_it_in_place() {
        let root = TempDir::new().unwrap();
        let journal = root.path().join("journal.sqlite");
        let source = root.path().join("in/video.mkv");
        let moved = root.path().join("out/video.mkv");
        write(&source, "only copy");
        let moving =
            Executor::judging_recycle_bin(Journal::open(&journal).unwrap(), |_, _| Verdict::TooBig);
        let applied = moving.apply("move", &[move_of(&source, &moved)], Collision::Suffix);
        assert!(matches!(applied.unwrap()[..], [Outcome::Applied(_)]));
        drop(moving);
        let executor = Executor {
            journal: Journal::open(&journal).unwrap(),
            disk: Disk {
                judge: |_, _| Verdict::TooBig,
                rename: across_drives,
                ..Disk::real()
            },
        };

        let undone = executor.undo_batch("move").unwrap();

        assert_eq!(undone, [Outcome::Failed(TOO_BIG.into())]);
        assert_eq!(fs::read_to_string(&moved).unwrap(), "only copy");
        assert!(!source.exists(), "a copy was made back before refusing");
        assert_eq!(
            executor.journal().applied_in_batch("move").unwrap().len(),
            1,
            "the move stays undoable"
        );
    }

    #[test]
    fn a_journaled_refusal_maps_back_to_why_the_file_was_not_recycled() {
        for (detail, expected) in [
            (
                ActionError::NoRecycleBin.to_string(),
                Some(NotRecycled::NoRecycleBin),
            ),
            (
                ActionError::TooBigForRecycleBin.to_string(),
                Some(NotRecycled::TooBigForRecycleBin),
            ),
            (
                ActionError::IncompleteCopy(Leftover::Cleared).to_string(),
                None,
            ),
            ("interrupted run".to_owned(), None),
        ] {
            assert_eq!(NotRecycled::from_detail(&detail), expected, "{detail}");
        }
    }

    #[test]
    fn only_a_bin_that_may_be_too_small_stops_a_move_that_must_copy() {
        for (verdict, refused) in [
            (Verdict::Recyclable, false),
            (Verdict::NoRecycleBin, false),
            (Verdict::TooBig, true),
            (Verdict::Unknown { fits_default: true }, false),
            (
                Verdict::Unknown {
                    fits_default: false,
                },
                true,
            ),
        ] {
            assert_eq!(move_refusal(verdict).is_some(), refused, "{verdict:?}");
        }
    }

    #[test]
    fn a_trash_is_refused_as_without_bin_whenever_the_bin_is_in_doubt() {
        for fits_default in [true, false] {
            let refusal = trash_refusal(Verdict::Unknown { fits_default });

            assert_eq!(
                refusal.map(|error| error.to_string()),
                Some(NO_BIN.to_owned()),
                "fits_default={fits_default}"
            );
        }
    }

    const NO_BIN: &str = "the Recycle Bin is not available on this drive";
    const INCOMPLETE: &str = "the copy did not reproduce the whole file";
    const NOT_A_PLAIN_FILE: &str = "the file was replaced by something that is not a plain file";
    const COULD_NOT_MOVE: &str = "the file could not be moved";
    const SOURCE_KEPT: &str =
        "the original could not be sent to the trash, so it stays where it was";
    const SOURCE_REPLACED: &str =
        "the original was replaced while it was being moved, so both are kept";
    const TRASH_MISSED: &str =
        "the trash said it was done but the file is still there, so check the Recycle Bin";
    #[cfg(windows)]
    const MISLEADING: &str =
        "a folder or the name ends in a space or a dot, which Windows would read as another file";
    #[cfg(windows)]
    const IN_USE: &str = "the file is in use by another program";
    #[cfg(windows)]
    const ENCRYPTED: &str = "the file is encrypted by Windows, and a copy would not stay encrypted";
    #[cfg(windows)]
    const INTERNET_MARK: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

    fn executor_with(disk: Disk) -> Executor {
        Executor {
            journal: Journal::open_in_memory().unwrap(),
            disk,
        }
    }

    fn copying_with(judge: Judge, copy: Copier) -> Executor {
        executor_with(Disk {
            judge,
            copy,
            rename: across_drives,
            ..Disk::real()
        })
    }

    fn across_drives(_from: &Path, _to: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::CrossesDevices.into())
    }

    fn refused_as_in_use(_from: &Path, _to: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }

    fn half_then_disk_full(
        _from: &Path,
        _source: &mut fs::File,
        copy: &mut fs::File,
    ) -> std::io::Result<u64> {
        std::io::Write::write_all(copy, b"half")?;
        Err(std::io::ErrorKind::StorageFull.into())
    }

    fn half_and_stops(
        _from: &Path,
        _source: &mut fs::File,
        copy: &mut fs::File,
    ) -> std::io::Result<u64> {
        std::io::Write::write_all(copy, b"half")?;
        Ok(4)
    }

    // WHY: attribute access is not subject to share modes, so this reaches a file the executor guards
    #[cfg(windows)]
    fn attribute_writer(file: &Path) -> fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        let opened = fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(file);
        opened.unwrap()
    }

    #[cfg(unix)]
    fn attribute_writer(file: &Path) -> fs::File {
        fs::OpenOptions::new().write(true).open(file).unwrap()
    }

    fn touch(file: &Path) {
        let moment = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1 << 30);
        attribute_writer(file).set_modified(moment).unwrap();
    }

    fn copy_then_the_source_is_touched(
        from: &Path,
        source: &mut fs::File,
        copy: &mut fs::File,
    ) -> std::io::Result<u64> {
        let copied = std::io::copy(source, copy)?;
        touch(from);
        Ok(copied)
    }

    fn grows_while_judged(path: &Path, _size: u64) -> Verdict {
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        std::io::Write::write_all(&mut file, b", second part").unwrap();
        Verdict::NoRecycleBin
    }

    fn move_of(source: &Path, destination: &Path) -> Operation {
        Operation::Move {
            from: source.to_path_buf(),
            to: destination.to_path_buf(),
        }
    }

    #[test]
    fn a_failed_copy_too_big_for_its_bin_is_renamed_so_it_cannot_pass_for_the_file() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("video.mkv");
        let duplicate = root.path().join("backup/video.mkv");
        write(&source, "the whole video");
        let executor = copying_with(|_, _| Verdict::TooBig, half_then_disk_full);

        let outcomes = executor
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        let detail = format!("{INCOMPLETE}; what was copied is kept as video.mkv.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail.clone())]);
        assert!(!duplicate.exists(), "a truncated copy kept the real name");
        let kept = root.path().join("backup/video.mkv.tidyfile-partial");
        assert_eq!(fs::read_to_string(kept).unwrap(), "half");
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
        let recorded = executor.journal().operations_in_batch("copy").unwrap();
        assert_eq!(recorded[0].detail.as_deref(), Some(detail.as_str()));
    }

    #[test]
    fn a_short_copy_takes_a_numbered_marked_name_when_the_first_is_taken() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("video.mkv");
        let duplicate = root.path().join("backup/video.mkv");
        let older = root.path().join("backup/video.mkv.tidyfile-partial");
        write(&source, "the whole video");
        write(&older, "an older leftover");
        let executor = copying_with(|_, _| Verdict::TooBig, half_and_stops);

        let outcomes = executor
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        let detail =
            format!("{INCOMPLETE}; what was copied is kept as video.mkv (2).tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&older).unwrap(), "an older leftover");
        let kept = root.path().join("backup/video.mkv (2).tidyfile-partial");
        assert_eq!(fs::read_to_string(kept).unwrap(), "half");
        assert!(!duplicate.exists());
    }

    #[test]
    fn a_failed_copy_its_bin_can_hold_goes_to_the_trash() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("video.mkv");
        let duplicate = root.path().join("backup/video.mkv");
        write(&source, "the whole video");
        let executor = copying_with(|_, _| Verdict::Recyclable, half_then_disk_full);

        let outcomes = executor
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(INCOMPLETE.into())]);
        assert_eq!(fs::read_dir(root.path().join("backup")).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
    }

    #[test]
    fn a_partial_copy_that_cannot_be_renamed_is_left_and_said_so() {
        let root = TempDir::new().unwrap();
        let partial = root.path().join("video.mkv");
        write(&partial, "half");

        let pin = pinned_against_removal(&partial);
        let leftover = set_aside(&partial);
        drop(pin);

        assert_eq!(
            ActionError::IncompleteCopy(leftover).to_string(),
            format!(
                "{INCOMPLETE}; what was copied could not be renamed and is still called video.mkv"
            )
        );
        assert_eq!(fs::read_to_string(&partial).unwrap(), "half");
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            1,
            "the name it claimed stayed"
        );
    }

    #[test]
    fn a_source_changed_during_a_cross_drive_move_keeps_both_files() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/mail.pst");
        let destination = root.path().join("out/mail.pst");
        write(&source, "version one");
        let executor = copying_with(
            |_, _| Verdict::NoRecycleBin,
            copy_then_the_source_is_touched,
        );

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        let detail = format!("{INCOMPLETE}; what was copied is kept as mail.pst.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "version one");
        let kept = root.path().join("out/mail.pst.tidyfile-partial");
        assert_eq!(fs::read_to_string(kept).unwrap(), "version one");
        assert!(!destination.exists(), "a stale copy kept the real name");
    }

    #[test]
    fn a_source_changed_during_a_copy_leaves_the_copy_under_a_marked_name() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("report.docx");
        let duplicate = root.path().join("backup/report.docx");
        write(&source, "draft");
        let executor = copying_with(|_, _| Verdict::Recyclable, copy_then_the_source_is_touched);

        let outcomes = executor
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        let detail =
            format!("{INCOMPLETE}; what was copied is kept as report.docx.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "draft");
        assert!(!duplicate.exists());
    }

    #[test]
    fn a_file_that_grows_after_it_was_judged_is_not_copied() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/download.iso");
        let destination = root.path().join("out/download.iso");
        write(&source, "first part");
        let executor = executor_with(Disk {
            judge: grows_while_judged,
            rename: across_drives,
            ..Disk::real()
        });

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(INCOMPLETE.into())]);
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "first part, second part"
        );
        assert_eq!(fs::read_dir(root.path().join("out")).unwrap().count(), 0);
    }

    #[test]
    fn a_planned_file_swapped_for_a_folder_is_never_trashed() {
        let root = TempDir::new().unwrap();
        let planned = root.path().join("old.log");
        write(&planned.join("inside.txt"), "kept");

        let outcomes = executor()
            .apply(
                "trash",
                &[Operation::Trash {
                    from: planned.clone(),
                }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(NOT_A_PLAIN_FILE.into())]);
        assert_eq!(
            fs::read_to_string(planned.join("inside.txt")).unwrap(),
            "kept"
        );
    }

    #[test]
    fn a_planned_file_swapped_for_a_link_is_never_trashed_or_moved() {
        let root = TempDir::new().unwrap();
        let (target, planned) = (
            root.path().join("elsewhere"),
            root.path().join("report.pdf"),
        );
        write(&target.join("inside.txt"), "kept");
        crate::testing::link_folder(&planned, &target);
        let executor = executor();

        let outcomes = executor
            .apply(
                "batch",
                &[
                    Operation::Trash {
                        from: planned.clone(),
                    },
                    move_of(&planned, &root.path().join("out/report.pdf")),
                ],
                Collision::Suffix,
            )
            .unwrap();

        let refused = Outcome::Failed(NOT_A_PLAIN_FILE.into());
        assert_eq!(outcomes, [refused.clone(), refused]);
        assert_eq!(
            fs::read_to_string(planned.join("inside.txt")).unwrap(),
            "kept"
        );
        assert_eq!(fs::read_dir(root.path().join("out")).unwrap().count(), 0);
    }

    fn set_read_only(path: &Path, read_only: bool) {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn modified(path: &Path) -> std::time::SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    fn across_drives_executor() -> Executor {
        executor_with(Disk {
            rename: across_drives,
            ..Disk::real()
        })
    }

    #[test]
    fn a_read_only_file_is_copied_whole_and_stays_read_only() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("contract.pdf");
        let duplicate = root.path().join("backup/contract.pdf");
        write(&source, "signed");
        set_read_only(&source, true);

        let outcomes = executor()
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(fs::read_to_string(&duplicate).unwrap(), "signed");
        assert!(fs::metadata(&duplicate).unwrap().permissions().readonly());
        assert_eq!(modified(&duplicate), modified(&source));
        set_read_only(&source, false);
        set_read_only(&duplicate, false);
    }

    #[test]
    fn a_read_only_file_moved_to_another_drive_arrives_whole() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/contract.pdf");
        let destination = root.path().join("out/contract.pdf");
        write(&source, "signed");
        set_read_only(&source, true);

        let outcomes = across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(fs::read_to_string(&destination).unwrap(), "signed");
        assert!(fs::metadata(&destination).unwrap().permissions().readonly());
        assert!(!source.exists(), "the original stayed behind");
        set_read_only(&destination, false);
    }

    #[test]
    fn a_rename_refused_for_any_reason_but_another_drive_makes_no_copy() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/report.docx");
        write(&source, "open in Word");
        let executor = executor_with(Disk {
            judge: |_, _| Verdict::NoRecycleBin,
            rename: refused_as_in_use,
            ..Disk::real()
        });

        let outcomes = executor
            .apply(
                "move",
                &[move_of(&source, &root.path().join("out/report.docx"))],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(COULD_NOT_MOVE.into())]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "open in Word");
        assert_eq!(fs::read_dir(root.path().join("out")).unwrap().count(), 0);
    }

    #[test]
    fn a_verified_copy_whose_original_cannot_be_let_go_is_set_aside() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        let destination = root.path().join("out/video.mkv");
        write(&source, "the whole video");

        let pin = pinned_against_removal(&source);
        let outcomes = across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();
        drop(pin);

        let detail =
            format!("{SOURCE_KEPT}; what was copied is kept as video.mkv.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
        let kept = root.path().join("out/video.mkv.tidyfile-partial");
        assert_eq!(fs::read_to_string(kept).unwrap(), "the whole video");
        assert!(!destination.exists(), "a second copy kept the real name");
    }

    #[cfg(windows)]
    #[test]
    fn a_file_another_program_is_writing_is_neither_copied_nor_moved() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/mail.pst");
        write(&source, "mailbox");
        let operations = [
            copy_of(&source, &root.path().join("backup/mail.pst")),
            move_of(&source, &root.path().join("out/mail.pst")),
        ];

        let writer = fs::OpenOptions::new().write(true).open(&source).unwrap();
        let outcomes = across_drives_executor()
            .apply("batch", &operations, Collision::Suffix)
            .unwrap();
        drop(writer);

        let in_use = Outcome::Failed(IN_USE.into());
        assert_eq!(outcomes, [in_use.clone(), in_use]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "mailbox");
        for folder in ["backup", "out"] {
            let left = fs::read_dir(root.path().join(folder)).unwrap().count();
            assert_eq!(left, 0, "a copy was left in {folder}");
        }
    }

    #[cfg(windows)]
    fn zone_of(path: &Path) -> PathBuf {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        PathBuf::from(stream)
    }

    #[cfg(windows)]
    #[test]
    fn a_download_moved_to_another_drive_keeps_its_mark_of_the_web() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/setup.exe");
        let destination = root.path().join("out/setup.exe");
        write(&source, "MZ");
        let mark = "[ZoneTransfer]\r\nZoneId=3\r\n";
        fs::write(zone_of(&source), mark).unwrap();

        let outcomes = across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(fs::read_to_string(zone_of(&destination)).unwrap(), mark);
    }

    #[test]
    fn only_an_empty_plain_file_is_discarded_as_a_claimed_name() {
        let root = TempDir::new().unwrap();
        let folder = root.path().join("claimed.pdf");
        fs::create_dir_all(&folder).unwrap();
        let slot = root.path().join("slot.pdf");
        write(&slot, "");

        discard_if_empty(&folder);
        discard_if_empty(&slot);

        assert!(
            folder.is_dir(),
            "an empty folder was taken for a claimed name"
        );
        assert!(!slot.exists(), "the empty claimed name stayed");
    }

    fn copy_then_the_source_is_saved_over(
        from: &Path,
        source: &mut fs::File,
        copy: &mut fs::File,
    ) -> std::io::Result<u64> {
        let copied = std::io::copy(source, copy)?;
        let draft = from.with_file_name("~saving.tmp");
        fs::write(&draft, "version two")?;
        fs::rename(&draft, from)?;
        Ok(copied)
    }

    #[test]
    fn a_source_saved_over_during_a_cross_drive_move_is_kept_with_its_copy() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/mail.pst");
        let destination = root.path().join("out/mail.pst");
        write(&source, "version one");
        let executor = copying_with(
            |_, _| Verdict::NoRecycleBin,
            copy_then_the_source_is_saved_over,
        );

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        let detail =
            format!("{SOURCE_REPLACED}; what was copied is kept as mail.pst.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "version two");
        let kept = root.path().join("out/mail.pst.tidyfile-partial");
        assert_eq!(fs::read_to_string(kept).unwrap(), "version one");
        assert!(!destination.exists());
    }

    fn trashed_but_reported_as_failed(path: &Path) -> Result<(), trash::Error> {
        trash::delete(path)?;
        Err(trash::Error::Unknown {
            description: "the share answered late".into(),
        })
    }

    #[test]
    fn an_original_that_is_gone_after_a_failed_trash_counts_as_let_go() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        let destination = root.path().join("out/video.mkv");
        write(&source, "the whole video");
        let executor = executor_with(Disk {
            rename: across_drives,
            trash: trashed_but_reported_as_failed,
            ..Disk::real()
        });

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(fs::read_to_string(&destination).unwrap(), "the whole video");
        assert!(!source.exists());
    }

    #[test]
    fn a_partial_whose_name_is_at_the_limit_is_still_set_aside_under_a_marked_name() {
        let root = TempDir::new().unwrap();
        let long = format!("{}.mkv", "v".repeat(250));
        let source = root.path().join(&long);
        let duplicate = root.path().join("backup").join(&long);
        write(&source, "the whole video");
        let executor = copying_with(|_, _| Verdict::TooBig, half_then_disk_full);

        let outcomes = executor
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        let left: Vec<String> = fs::read_dir(root.path().join("backup"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].ends_with(".tidyfile-partial") && left[0].len() <= 255);
        let detail = format!("{INCOMPLETE}; what was copied is kept as {}", left[0]);
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
    }

    fn swap_the_claimed_name_for_a_link(_from: &Path, to: &Path) -> std::io::Result<()> {
        let precious = to.with_file_name("precious.txt");
        let link = to.with_file_name("link.tmp");
        fs::hard_link(&precious, &link)?;
        fs::rename(&link, to)?;
        Err(std::io::ErrorKind::CrossesDevices.into())
    }

    #[test]
    fn a_claimed_name_swapped_for_a_link_to_another_file_is_never_written_through() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        let precious = root.path().join("out/precious.txt");
        write(&source, "the whole video");
        write(&precious, "keep me");
        let executor = executor_with(Disk {
            rename: swap_the_claimed_name_for_a_link,
            ..Disk::real()
        });

        let outcomes = executor
            .apply(
                "move",
                &[move_of(&source, &root.path().join("out/video.mkv"))],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(COULD_NOT_MOVE.into())]);
        assert_eq!(fs::read_to_string(&precious).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
    }

    #[cfg(windows)]
    #[test]
    fn a_rename_refused_because_the_file_is_open_or_locked_says_it_is_in_use() {
        let sharing_violation: Renamer = |_, _| Err(std::io::Error::from_raw_os_error(32));
        let lock_violation: Renamer = |_, _| Err(std::io::Error::from_raw_os_error(33));
        for rename in [sharing_violation, lock_violation] {
            let root = TempDir::new().unwrap();
            let source = root.path().join("in/report.docx");
            write(&source, "open in Word");
            let executor = executor_with(Disk {
                rename,
                ..Disk::real()
            });

            let destination = root.path().join("out/report.docx");
            let outcomes = executor
                .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
                .unwrap();

            assert_eq!(outcomes, [Outcome::Failed(IN_USE.into())]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_oversized_mark_of_the_web_is_carried_as_a_plain_internet_mark() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/setup.exe");
        let destination = root.path().join("out/setup.exe");
        write(&source, "MZ");
        let oversized = format!("{INTERNET_MARK}{}", " ".repeat(70 * 1024));
        fs::write(zone_of(&source), oversized).unwrap();

        let outcomes = across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(
            fs::read_to_string(zone_of(&destination)).unwrap(),
            INTERNET_MARK
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_file_without_a_mark_of_the_web_is_not_given_one() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/notes.txt");
        let destination = root.path().join("out/notes.txt");
        write(&source, "local");

        across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert!(fs::read(zone_of(&destination)).is_err());
    }

    #[cfg(windows)]
    fn encrypt_with_efs(file: &Path) {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
        let done = std::process::Command::new("cipher")
            .args(["/e", "/a"])
            .arg(file)
            .output()
            .unwrap();
        let attributes = fs::metadata(file).unwrap().file_attributes();
        assert!(done.status.success(), "cipher failed");
        assert_ne!(
            attributes & FILE_ATTRIBUTE_ENCRYPTED,
            0,
            "EFS is not available"
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_encrypted_file_is_refused_rather_than_copied_in_plain_text() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("secret.txt");
        write(&source, "for my eyes only");
        encrypt_with_efs(&source);

        let outcomes = executor()
            .apply(
                "copy",
                &[copy_of(&source, &root.path().join("backup/secret.txt"))],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(ENCRYPTED.into())]);
        assert_eq!(fs::read_dir(root.path().join("backup")).unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_quarantined_download_keeps_its_quarantine_when_copied() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("setup.dmg");
        let duplicate = root.path().join("backup/setup.dmg");
        write(&source, "disk image");
        let flag = "0081;5f000000;Safari;";
        let xattr = |args: &[&str], file: &Path| {
            std::process::Command::new("xattr")
                .args(args)
                .arg(file)
                .output()
                .unwrap()
        };
        assert!(
            xattr(&["-w", "com.apple.quarantine", flag], &source)
                .status
                .success()
        );

        let outcomes = executor()
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        let read = xattr(&["-p", "com.apple.quarantine"], &duplicate);
        assert_eq!(String::from_utf8_lossy(&read.stdout).trim(), flag);
    }

    #[cfg(windows)]
    #[test]
    fn a_move_out_of_a_folder_ending_in_a_dot_never_trashes_the_namesake() {
        let root = TempDir::new().unwrap();
        let verbatim = root.path().canonicalize().unwrap();
        let source = verbatim.join("Old.").join("video.mkv");
        let namesake = verbatim.join("Old").join("video.mkv");
        let destination = verbatim.join("out").join("video.mkv");
        write(&source, "the odd one");
        write(&namesake, "the real video");

        let outcomes = across_drives_executor()
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(MISLEADING.into())]);
        assert_eq!(fs::read_to_string(&namesake).unwrap(), "the real video");
        assert_eq!(fs::read_to_string(&source).unwrap(), "the odd one");
        assert!(!destination.exists());
    }

    #[cfg(windows)]
    #[test]
    fn undoing_a_copy_into_a_folder_ending_in_a_dot_never_trashes_the_namesake() {
        let root = TempDir::new().unwrap();
        let verbatim = root.path().canonicalize().unwrap();
        let source = verbatim.join("report.pdf");
        let copied = verbatim.join("Backup.").join("report.pdf");
        let namesake = verbatim.join("Backup").join("report.pdf");
        write(&source, "original");
        write(&copied, "the copy");
        write(&namesake, "someone else's");
        let executor = executor();
        let journal = executor.journal();
        let id = journal.record_planned("copy", &copy_of(&source, &copied));
        journal.mark(id.unwrap(), State::Done, None).unwrap();

        let undone = executor.undo_batch("copy").unwrap();

        assert_eq!(undone, [Outcome::Failed(MISLEADING.into())]);
        assert_eq!(fs::read_to_string(&namesake).unwrap(), "someone else's");
        assert_eq!(fs::read_to_string(&copied).unwrap(), "the copy");
    }

    #[cfg(windows)]
    fn record_a_done_copy(executor: &Executor, from: &Path, to: &Path) {
        let journal = executor.journal();
        let id = journal.record_planned("copy", &copy_of(from, to)).unwrap();
        journal.mark(id, State::Done, None).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn undoing_a_copy_through_a_junction_to_a_folder_ending_in_a_dot_spares_the_namesake() {
        let root = TempDir::new().unwrap();
        let verbatim = root.path().canonicalize().unwrap();
        let odd_folder = verbatim.join("Archive 2026.");
        let namesake = verbatim.join("Archive 2026").join("report.pdf");
        write(&verbatim.join("report.pdf"), "original");
        write(&odd_folder.join("report.pdf"), "the copy");
        write(&namesake, "someone else's");
        let Some(short) = short_name_of(&odd_folder) else {
            eprintln!("skipped: this volume keeps no 8.3 names to aim a junction at");
            return;
        };
        let junction = root.path().join("Backup");
        crate::testing::link_folder(&junction, dunce::simplified(&short));
        let copied = junction.join("report.pdf");
        assert_eq!(fs::read_to_string(&copied).unwrap(), "the copy");
        let executor = executor();
        record_a_done_copy(&executor, &verbatim.join("report.pdf"), &copied);

        let undone = executor.undo_batch("copy").unwrap();

        assert_eq!(undone, [Outcome::Failed(MISLEADING.into())]);
        assert_eq!(fs::read_to_string(&namesake).unwrap(), "someone else's");
        assert_eq!(fs::read_to_string(&copied).unwrap(), "the copy");
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn short_name_of(path: &Path) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut buffer = vec![0u16; 2048];
        let capacity = u32::try_from(buffer.len()).unwrap();
        // SAFETY: `wide` ends in NUL and `buffer` has `capacity` writable units for the whole call
        let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), capacity) };
        let written = usize::try_from(written).unwrap();
        let short = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written]));
        (written > 0 && written < buffer.len() && short.file_name() != path.file_name())
            .then_some(short)
    }

    #[cfg(windows)]
    #[test]
    fn trashing_through_the_short_name_of_a_folder_ending_in_a_dot_spares_the_namesake() {
        let root = TempDir::new().unwrap();
        let verbatim = root.path().canonicalize().unwrap();
        let odd_folder = verbatim.join("Invoices 2026.");
        let namesake = verbatim.join("Invoices 2026").join("report.pdf");
        write(&odd_folder.join("report.pdf"), "the odd one");
        write(&namesake, "the real report");
        let Some(short) = short_name_of(&odd_folder) else {
            eprintln!("skipped: this volume keeps no 8.3 names");
            return;
        };

        let trashing = Operation::Trash {
            from: short.join("report.pdf"),
        };
        let outcomes = executor()
            .apply("trash", &[trashing], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(MISLEADING.into())]);
        assert_eq!(fs::read_to_string(&namesake).unwrap(), "the real report");
        assert!(odd_folder.join("report.pdf").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_file_named_by_its_8_3_alias_is_refused() {
        let root = TempDir::new().unwrap();
        let verbatim = root.path().canonicalize().unwrap();
        let odd = verbatim.join("report.pdf ");
        write(&odd, "the odd one");
        write(&verbatim.join("report.pdf"), "the real report");
        let Some(short) = short_name_of(&odd) else {
            eprintln!("skipped: this volume keeps no 8.3 names");
            return;
        };

        let outcomes = executor()
            .apply(
                "trash",
                &[Operation::Trash { from: short }],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(MISLEADING.into())]);
        assert!(odd.exists());
        assert!(verbatim.join("report.pdf").exists());
    }

    fn says_done_but_trashes_nothing(_path: &Path) -> Result<(), trash::Error> {
        Ok(())
    }

    #[test]
    fn a_trash_that_says_done_while_the_file_is_still_there_is_a_failure() {
        let root = TempDir::new().unwrap();
        let doomed = root.path().join("old.log");
        write(&doomed, "still here");
        let executor = executor_with(Disk {
            trash: says_done_but_trashes_nothing,
            ..Disk::real()
        });

        let trashing = Operation::Trash {
            from: doomed.clone(),
        };
        let outcomes = executor
            .apply("trash", &[trashing], Collision::Suffix)
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(TRASH_MISSED.into())]);
        assert_eq!(fs::read_to_string(&doomed).unwrap(), "still here");
    }

    #[test]
    fn a_move_whose_trash_says_done_while_the_original_stays_is_a_failure() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        let destination = root.path().join("out/video.mkv");
        write(&source, "the whole video");
        let executor = executor_with(Disk {
            rename: across_drives,
            trash: says_done_but_trashes_nothing,
            ..Disk::real()
        });

        let outcomes = executor
            .apply("move", &[move_of(&source, &destination)], Collision::Suffix)
            .unwrap();

        let detail =
            format!("{TRASH_MISSED}; what was copied is kept as video.mkv.tidyfile-partial");
        assert_eq!(outcomes, [Outcome::Failed(detail)]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
        assert!(!destination.exists(), "the copy kept the real name");
    }

    #[test]
    fn a_claimed_name_counts_as_unshared_with_one_link_or_with_none_reported() {
        for (links, expected) in [
            (Some(0), true),
            (Some(1), true),
            (Some(2), false),
            (None, false),
        ] {
            assert_eq!(is_unshared(links), expected, "{links:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_mark_of_the_web_that_cannot_be_read_is_carried_as_a_plain_internet_mark() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = TempDir::new().unwrap();
        let source = root.path().join("setup.exe");
        let duplicate = root.path().join("backup/setup.exe");
        write(&source, "MZ");
        fs::write(zone_of(&source), format!("{INTERNET_MARK}HostUrl=x\r\n")).unwrap();

        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(zone_of(&source))
            .unwrap();
        let outcomes = executor()
            .apply("copy", &[copy_of(&source, &duplicate)], Collision::Suffix)
            .unwrap();
        drop(held);

        assert!(
            matches!(outcomes[..], [Outcome::Applied(_)]),
            "{outcomes:?}"
        );
        assert_eq!(
            fs::read_to_string(zone_of(&duplicate)).unwrap(),
            INTERNET_MARK
        );
    }

    #[cfg(unix)]
    fn swap_the_claimed_name_for_a_fifo(_from: &Path, to: &Path) -> std::io::Result<()> {
        let fifo = to.with_file_name("pipe.tmp");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status()?;
        assert!(made.success(), "mkfifo failed");
        fs::rename(&fifo, to)?;
        Err(std::io::ErrorKind::CrossesDevices.into())
    }

    #[cfg(unix)]
    #[test]
    fn a_claimed_name_swapped_for_a_fifo_fails_at_once_instead_of_waiting_for_a_reader() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("in/video.mkv");
        write(&source, "the whole video");
        let executor = executor_with(Disk {
            rename: swap_the_claimed_name_for_a_fifo,
            ..Disk::real()
        });

        let outcomes = executor
            .apply(
                "move",
                &[move_of(&source, &root.path().join("out/video.mkv"))],
                Collision::Suffix,
            )
            .unwrap();

        assert_eq!(outcomes, [Outcome::Failed(COULD_NOT_MOVE.into())]);
        assert_eq!(fs::read_to_string(&source).unwrap(), "the whole video");
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
        let refusing = Executor::judging_recycle_bin(Journal::open(&journal).unwrap(), |_, _| {
            Verdict::NoRecycleBin
        });
        let copy = copy_of(&source, &duplicate);
        refusing.apply("copy", &[copy], Collision::Suffix).unwrap();
        refusing.undo_batch("copy").unwrap();
        assert_eq!(
            refusing.journal().applied_in_batch("copy").unwrap().len(),
            1
        );
        drop(refusing);

        let restored = Executor::judging_recycle_bin(Journal::open(&journal).unwrap(), |_, _| {
            Verdict::Recyclable
        });
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
