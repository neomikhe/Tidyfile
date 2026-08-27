#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tidyfile_lib::executor::Collision;
use tidyfile_lib::rules::{Action, Combinator, Condition, Rule};
use tidyfile_lib::service::Tidyfile;
use tidyfile_lib::settings::Settings;
use tidyfile_lib::store;

fn put(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn messy_downloads(root: &Path) {
    put(&root.join("invoice-march.pdf"), "an invoice");
    put(&root.join("invoice-april.pdf"), "another invoice");
    put(&root.join("Screenshot 2026-03-07.png"), "a screenshot");
    put(&root.join("holiday.jpg"), "a photo");
    put(&root.join("notes.txt"), "some notes");
    put(&root.join("big-download.zip.crdownload"), "still arriving");
    put(&root.join("~$budget.xlsx"), "an office lock file");
    put(
        &root.join("nested/deep/invoice-may.pdf"),
        "a nested invoice",
    );
}

fn rule(name: &str, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
    Rule {
        id: format!("id-{name}"),
        name: name.into(),
        enabled: true,
        combinator: Combinator::All,
        conditions,
        actions,
    }
}

fn invoice_rule(archive: &Path) -> Rule {
    rule(
        "invoices",
        vec![
            Condition::Extension {
                any_of: vec!["pdf".into()],
            },
            Condition::NameContains {
                text: "invoice".into(),
            },
        ],
        vec![Action::MoveTo {
            folder: archive.to_path_buf(),
            subfolder: Some("{year}".into()),
            rename: Some("{name}.{ext}".into()),
        }],
    )
}

fn screenshot_rule(archive: &Path) -> Rule {
    rule(
        "screenshots",
        vec![Condition::NameMatchesGlob {
            pattern: "Screenshot*.png".into(),
        }],
        vec![Action::MoveTo {
            folder: archive.to_path_buf(),
            subfolder: Some("shots".into()),
            rename: None,
        }],
    )
}

fn survivors(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                names.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    names
}

#[test]
fn a_whole_session_tidies_a_messy_folder_and_puts_it_all_back() {
    let downloads = TempDir::new().unwrap();
    let archive = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    messy_downloads(downloads.path());
    let before = survivors(downloads.path());

    let rules_file = config.path().join("rules.json");
    let settings_file = config.path().join("settings.json");
    let rules = vec![
        invoice_rule(archive.path()),
        screenshot_rule(archive.path()),
    ];
    store::save(&rules_file, &rules).unwrap();
    store::save(
        &settings_file,
        &Settings {
            folders: vec![downloads.path().to_path_buf()],
            watched: Vec::new(),
            on_collision: Collision::Suffix,
        },
    )
    .unwrap();

    let settings: Settings = store::load(&settings_file).unwrap();
    let saved: Vec<Rule> = store::load(&rules_file).unwrap();
    let service = Tidyfile::open(&config.path().join("journal.sqlite")).unwrap();

    let planned = service.simulate(&saved, &settings.folders).unwrap();
    assert_eq!(
        planned.len(),
        4,
        "three invoices and one screenshot should be planned, got {planned:?}"
    );
    assert_eq!(
        survivors(downloads.path()),
        before,
        "simulating must not touch a single file"
    );

    let report = service
        .organize(&saved, &settings.folders, settings.on_collision)
        .unwrap();
    assert_eq!((report.applied, report.failed), (4, 0));

    assert!(archive.path().join("2026/invoice-march.pdf").exists());
    assert!(archive.path().join("2026/invoice-april.pdf").exists());
    assert!(archive.path().join("2026/invoice-may.pdf").exists());
    assert!(
        archive
            .path()
            .join("shots/Screenshot 2026-03-07.png")
            .exists()
    );

    assert_eq!(
        survivors(downloads.path()),
        [
            "big-download.zip.crdownload",
            "holiday.jpg",
            "notes.txt",
            "~$budget.xlsx"
        ],
        "only the untouched files and the ignored temporaries should remain"
    );

    let history = service.activity(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].done, 4);

    service.undo(&report.batch).unwrap();

    assert_eq!(
        survivors(downloads.path()),
        before,
        "undo must restore the folder exactly as it was"
    );
    assert_eq!(service.activity(10).unwrap()[0].undone, 4);
}

#[test]
fn a_second_pass_over_an_already_tidy_folder_changes_nothing() {
    let downloads = TempDir::new().unwrap();
    let archive = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    put(&downloads.path().join("invoice-march.pdf"), "an invoice");
    let rules = vec![invoice_rule(archive.path())];
    let folders = vec![downloads.path().to_path_buf()];
    let service = Tidyfile::open(&config.path().join("journal.sqlite")).unwrap();

    service
        .organize(&rules, &folders, Collision::Suffix)
        .unwrap();
    let second = service
        .organize(&rules, &folders, Collision::Suffix)
        .unwrap();

    assert_eq!(
        (second.applied, second.failed),
        (0, 0),
        "nothing is left to tidy, so the second pass must be a no-op"
    );
    assert_eq!(
        fs::read_dir(archive.path().join("2026")).unwrap().count(),
        1,
        "the second pass must not have produced a duplicate"
    );
}

#[test]
fn two_folders_holding_the_same_name_both_survive_the_move() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let archive = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    put(&first.path().join("invoice.pdf"), "from the first folder");
    put(&second.path().join("invoice.pdf"), "from the second folder");
    let folders = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    let service = Tidyfile::open(&config.path().join("journal.sqlite")).unwrap();

    let report = service
        .organize(&[invoice_rule(archive.path())], &folders, Collision::Suffix)
        .unwrap();

    assert_eq!(report.applied, 2);
    let landed: Vec<PathBuf> = fs::read_dir(archive.path().join("2026"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(landed.len(), 2, "one file overwrote the other: {landed:?}");
    let contents: Vec<String> = landed
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    assert!(contents.contains(&"from the first folder".to_owned()));
    assert!(contents.contains(&"from the second folder".to_owned()));
}

#[test]
fn a_rule_whose_destination_sits_inside_the_watched_folder_settles() {
    let watched = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    put(&watched.path().join("invoice.pdf"), "content");
    let inside = watched.path().join("PDFs");
    let rules = vec![Rule {
        id: "r1".into(),
        name: "pdfs".into(),
        enabled: true,
        combinator: Combinator::All,
        conditions: vec![Condition::Extension {
            any_of: vec!["pdf".into()],
        }],
        actions: vec![Action::MoveTo {
            folder: inside.clone(),
            subfolder: None,
            rename: None,
        }],
    }];
    let folders = vec![watched.path().to_path_buf()];
    let service = Tidyfile::open(&config.path().join("journal.sqlite")).unwrap();

    service
        .organize(&rules, &folders, Collision::Suffix)
        .unwrap();
    assert!(
        inside.join("invoice.pdf").exists(),
        "the first pass moved it"
    );

    let second = service
        .organize(&rules, &folders, Collision::Suffix)
        .unwrap();

    let landed: Vec<String> = fs::read_dir(&inside)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        landed,
        ["invoice.pdf"],
        "the file was duplicated by moving it onto itself: {landed:?}, report {second:?}"
    );
}
