use std::fs;
use std::path::{Path, PathBuf};

use crate::rules::Rule;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the rule file could not be read or written")]
    Unreachable(#[from] std::io::Error),
    #[error("the rule file is not valid Tidyfile JSON")]
    Malformed(#[from] serde_json::Error),
}

pub fn load(path: &Path) -> Result<Vec<Rule>, StoreError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(StoreError::Unreachable(error)),
    }
}

pub fn save(path: &Path, rules: &[Rule]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let staging = staging_path(path);
    fs::write(&staging, serde_json::to_string_pretty(rules)?)?;
    fs::rename(&staging, path)?;
    Ok(())
}

fn staging_path(path: &Path) -> PathBuf {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".writing");
    PathBuf::from(staging)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::rules::{Action, Combinator, Condition};
    use tempfile::TempDir;

    fn sample() -> Rule {
        Rule {
            id: "id-1".into(),
            name: "Facturas".into(),
            enabled: true,
            combinator: Combinator::All,
            conditions: vec![Condition::Extension {
                any_of: vec!["pdf".into()],
            }],
            actions: vec![Action::MoveTo {
                folder: PathBuf::from("/out"),
                subfolder: Some("{year}".into()),
                rename: None,
            }],
        }
    }

    #[test]
    fn rules_survive_a_save_and_load_cycle() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("rules.json");

        save(&path, &[sample()]).unwrap();

        assert_eq!(load(&path).unwrap(), [sample()]);
    }

    #[test]
    fn a_missing_file_reads_as_no_rules() {
        let folder = TempDir::new().unwrap();

        let rules = load(&folder.path().join("absent.json")).unwrap();

        assert!(rules.is_empty());
    }

    #[test]
    fn saving_creates_the_folder_it_needs() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("nested/deeper/rules.json");

        save(&path, &[sample()]).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn a_malformed_file_is_reported_rather_than_silently_dropped() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("rules.json");
        fs::write(&path, "{ not json").unwrap();

        assert!(matches!(load(&path), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn saving_leaves_no_staging_file_behind() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("rules.json");

        save(&path, &[sample()]).unwrap();

        assert!(!staging_path(&path).exists());
    }

    #[test]
    fn a_failed_save_does_not_destroy_the_previous_rules() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("rules.json");
        save(&path, &[sample()]).unwrap();

        let blocked = save(folder.path(), &[sample()]);

        assert!(blocked.is_err());
        assert_eq!(load(&path).unwrap(), [sample()]);
    }

    #[test]
    fn an_empty_rule_set_round_trips() {
        let folder = TempDir::new().unwrap();
        let path = folder.path().join("rules.json");

        save(&path, &[]).unwrap();

        assert!(load(&path).unwrap().is_empty());
    }
}
