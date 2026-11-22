#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

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

#[test]
#[ignore = "needs a drive without Recycle Bin: set TIDYFILE_NO_RECYCLE_DIR and run with --ignored"]
fn trash_on_a_drive_without_recycle_bin_keeps_the_file() {
    let base = PathBuf::from(std::env::var_os(FOLDER_VARIABLE).expect(FOLDER_VARIABLE));
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
