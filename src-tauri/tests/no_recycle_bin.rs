#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tidyfile_lib::executor::Collision;
use tidyfile_lib::rules::{Action, Combinator, Condition, Rule};
use tidyfile_lib::service::Tidyfile;

const FOLDER_VARIABLE: &str = "TIDYFILE_NO_RECYCLE_DIR";
const SMALL_BIN_VARIABLE: &str = "TIDYFILE_SMALL_BIN_DIR";
const BIGGER_THAN_THE_BIN: usize = 3 * 1024 * 1024;

fn trash_logs() -> Rule {
    Rule {
        id: "trash-logs".into(),
        name: "Trash logs".into(),
        enabled: true,
        combinator: Combinator::All,
        conditions: vec![Condition::Extension {
            any_of: vec!["log".into()],
        }],
        actions: vec![Action::Trash],
    }
}

fn copy_logs_to(folder: &Path) -> Rule {
    Rule {
        id: "copy-logs".into(),
        name: "Copy logs".into(),
        actions: vec![Action::CopyTo {
            folder: folder.to_path_buf(),
            subfolder: None,
            rename: None,
        }],
        ..trash_logs()
    }
}

fn move_logs_to(folder: &Path) -> Rule {
    Rule {
        id: "move-logs".into(),
        name: "Move logs".into(),
        actions: vec![Action::MoveTo {
            folder: folder.to_path_buf(),
            subfolder: None,
            rename: None,
        }],
        ..trash_logs()
    }
}

fn drive_without_recycle_bin() -> PathBuf {
    PathBuf::from(std::env::var_os(FOLDER_VARIABLE).expect(FOLDER_VARIABLE))
}

fn drive_with_a_one_megabyte_bin() -> PathBuf {
    PathBuf::from(std::env::var_os(SMALL_BIN_VARIABLE).expect(SMALL_BIN_VARIABLE))
}

fn big_log(folder: &Path) -> PathBuf {
    let file = folder.join("tidyfile-probe.log");
    fs::write(&file, vec![b'x'; BIGGER_THAN_THE_BIN]).unwrap();
    file
}

fn size_of(file: &Path) -> usize {
    usize::try_from(fs::metadata(file).unwrap().len()).unwrap()
}

#[test]
#[ignore = "needs a drive whose Recycle Bin holds 1 MB: set TIDYFILE_SMALL_BIN_DIR and run with --ignored"]
fn trash_of_a_file_larger_than_the_recycle_bin_keeps_the_file() {
    let folder = TempDir::new_in(drive_with_a_one_megabyte_bin()).unwrap();
    let file = big_log(folder.path());
    let service = Tidyfile::in_memory().unwrap();

    let report = service
        .organize(
            &[trash_logs()],
            &[folder.path().to_path_buf()],
            Collision::Suffix,
        )
        .unwrap();

    assert_eq!(
        (report.applied, report.failed, report.not_recycled),
        (0, 1, 1),
        "the trash rule was not refused"
    );
    assert_eq!(
        size_of(&file),
        BIGGER_THAN_THE_BIN,
        "the file is gone or changed"
    );
}

#[test]
#[ignore = "needs a drive whose Recycle Bin holds 1 MB: set TIDYFILE_SMALL_BIN_DIR and run with --ignored"]
fn undoing_a_copy_larger_than_the_recycle_bin_keeps_the_copy() {
    let base = drive_with_a_one_megabyte_bin();
    let (folder, copies) = (
        TempDir::new_in(&base).unwrap(),
        TempDir::new_in(&base).unwrap(),
    );
    big_log(folder.path());
    let service = Tidyfile::in_memory().unwrap();
    let rules = [copy_logs_to(copies.path())];
    let report = service
        .organize(&rules, &[folder.path().to_path_buf()], Collision::Suffix)
        .unwrap();
    assert_eq!(report.applied, 1, "the copy did not run");

    let undone = service.undo(&report.batch).unwrap();

    assert_eq!(
        (undone.applied, undone.failed, undone.not_recycled),
        (0, 1, 1),
        "the undo was not refused"
    );
    let copy = copies.path().join("tidyfile-probe.log");
    assert_eq!(size_of(&copy), BIGGER_THAN_THE_BIN, "the copy is gone");
}

#[test]
#[ignore = "needs TIDYFILE_SMALL_BIN_DIR and, on another drive, TIDYFILE_NO_RECYCLE_DIR; run with --ignored"]
fn moving_a_file_too_big_for_its_bin_to_another_drive_makes_no_copy() {
    let folder = TempDir::new_in(drive_with_a_one_megabyte_bin()).unwrap();
    let elsewhere = TempDir::new_in(drive_without_recycle_bin()).unwrap();
    let file = big_log(folder.path());
    let service = Tidyfile::in_memory().unwrap();
    let rules = [move_logs_to(elsewhere.path())];

    let report = service
        .organize(&rules, &[folder.path().to_path_buf()], Collision::Suffix)
        .unwrap();

    assert_eq!(
        (report.applied, report.failed, report.not_recycled),
        (0, 1, 1),
        "the move was not refused"
    );
    assert_eq!(size_of(&file), BIGGER_THAN_THE_BIN, "the source is gone");
    assert_eq!(
        fs::read_dir(elsewhere.path()).unwrap().count(),
        0,
        "a copy was left on the other drive"
    );
}

#[test]
#[ignore = "needs a drive without Recycle Bin: set TIDYFILE_NO_RECYCLE_DIR and run with --ignored"]
fn undoing_a_copy_on_a_drive_without_recycle_bin_keeps_the_copy() {
    let base = drive_without_recycle_bin();
    let (folder, copies) = (
        TempDir::new_in(&base).unwrap(),
        TempDir::new_in(&base).unwrap(),
    );
    fs::write(folder.path().join("tidyfile-probe.log"), "original").unwrap();
    let service = Tidyfile::in_memory().unwrap();
    let rules = [copy_logs_to(copies.path())];
    let report = service
        .organize(&rules, &[folder.path().to_path_buf()], Collision::Suffix)
        .unwrap();
    assert_eq!(report.applied, 1, "the copy did not run");
    let copy = copies.path().join("tidyfile-probe.log");
    fs::write(&copy, "edited after the copy").unwrap();

    let undone = service.undo(&report.batch).unwrap();

    assert_eq!(
        (undone.applied, undone.failed, undone.not_recycled),
        (0, 1, 1),
        "the undo was not refused"
    );
    assert_eq!(
        fs::read_to_string(&copy).unwrap(),
        "edited after the copy",
        "the copy is gone: it was deleted for good"
    );
}

#[test]
#[ignore = "needs a drive without Recycle Bin: set TIDYFILE_NO_RECYCLE_DIR and run with --ignored"]
fn trash_on_a_drive_without_recycle_bin_keeps_the_file() {
    let base = drive_without_recycle_bin();
    let folder = TempDir::new_in(&base).unwrap();
    let file = folder.path().join("tidyfile-probe.log");
    fs::write(&file, "must survive").unwrap();
    let service = Tidyfile::in_memory().unwrap();

    let report = service
        .organize(
            &[trash_logs()],
            &[folder.path().to_path_buf()],
            Collision::Suffix,
        )
        .unwrap();

    assert_eq!(
        (report.applied, report.failed, report.not_recycled),
        (0, 1, 1),
        "the trash rule was not refused"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "must survive",
        "the file is gone: it was deleted for good"
    );
}
