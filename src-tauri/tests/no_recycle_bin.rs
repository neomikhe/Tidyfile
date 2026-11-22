#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tidyfile_lib::executor::Collision;
use tidyfile_lib::rules::{Action, Combinator, Condition, Rule};
use tidyfile_lib::service::Tidyfile;

const FOLDER_VARIABLE: &str = "TIDYFILE_NO_RECYCLE_DIR";

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

fn drive_without_recycle_bin() -> PathBuf {
    PathBuf::from(std::env::var_os(FOLDER_VARIABLE).expect(FOLDER_VARIABLE))
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
        (undone.applied, undone.failed),
        (0, 1),
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
        (report.applied, report.failed),
        (0, 1),
        "the trash rule was not refused"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "must survive",
        "the file is gone: it was deleted for good"
    );
}
