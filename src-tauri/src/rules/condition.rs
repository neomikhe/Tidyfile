use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::patterns;

const MAX_PATTERN_LENGTH: usize = 512;
const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("the pattern is longer than the allowed limit")]
    PatternTooLong,
    #[error("the regular expression could not be compiled")]
    InvalidRegex,
    #[error("the glob pattern could not be compiled")]
    InvalidGlob,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Condition {
    Extension { any_of: Vec<String> },
    NameContains { text: String },
    NameMatchesGlob { pattern: String },
    NameMatchesRegex { pattern: String },
    LargerThan { bytes: u64 },
    SmallerThan { bytes: u64 },
    OlderThan { days: u64 },
    NewerThan { days: u64 },
    InSubfolder { name: String },
}

#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: PathBuf,
    pub root: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

impl FileFacts {
    pub fn gather(path: &Path, root: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }

    fn name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }

    fn extension(&self) -> Option<String> {
        Some(self.path.extension()?.to_str()?.to_ascii_lowercase())
    }

    fn age_in_days(&self, now: SystemTime) -> u64 {
        now.duration_since(self.modified)
            .map(|elapsed| elapsed.as_secs() / SECONDS_PER_DAY)
            .unwrap_or_default()
    }
}

impl Condition {
    pub fn matches(&self, facts: &FileFacts, now: SystemTime) -> Result<bool, ConditionError> {
        match self {
            Self::Extension { any_of } => Ok(matches_extension(facts, any_of)),
            Self::NameContains { text } => Ok(matches_contains(facts, text)),
            Self::NameMatchesGlob { pattern } => matches_glob(facts, pattern),
            Self::NameMatchesRegex { pattern } => matches_regex(facts, pattern),
            Self::LargerThan { bytes } => Ok(facts.size > *bytes),
            Self::SmallerThan { bytes } => Ok(facts.size < *bytes),
            Self::OlderThan { days } => Ok(facts.age_in_days(now) >= *days),
            Self::NewerThan { days } => Ok(facts.age_in_days(now) < *days),
            Self::InSubfolder { name } => Ok(matches_subfolder(facts, name)),
        }
    }
}

fn matches_extension(facts: &FileFacts, accepted: &[String]) -> bool {
    let Some(extension) = facts.extension() else {
        return false;
    };
    accepted.iter().any(|candidate| {
        candidate
            .trim_start_matches('.')
            .eq_ignore_ascii_case(&extension)
    })
}

fn matches_contains(facts: &FileFacts, text: &str) -> bool {
    facts
        .name()
        .is_some_and(|name| name.to_lowercase().contains(&text.to_lowercase()))
}

fn matches_glob(facts: &FileFacts, pattern: &str) -> Result<bool, ConditionError> {
    reject_oversized(pattern)?;
    let glob = patterns::glob(pattern)?;
    Ok(facts.name().is_some_and(|name| glob.is_match(name)))
}

fn matches_regex(facts: &FileFacts, pattern: &str) -> Result<bool, ConditionError> {
    reject_oversized(pattern)?;
    let regex = patterns::regex(pattern)?;
    Ok(facts.name().is_some_and(|name| regex.is_match(name)))
}

fn matches_subfolder(facts: &FileFacts, name: &str) -> bool {
    let Ok(relative) = facts.path.strip_prefix(&facts.root) else {
        return false;
    };
    relative.parent().is_some_and(|parent| {
        parent
            .components()
            .any(|component| component.as_os_str() == name)
    })
}

fn reject_oversized(pattern: &str) -> Result<(), ConditionError> {
    if pattern.len() > MAX_PATTERN_LENGTH {
        return Err(ConditionError::PatternTooLong);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn facts(relative: &str, size: u64, age_days: u64) -> FileFacts {
        let root = PathBuf::from("/watched");
        FileFacts {
            path: root.join(relative),
            root,
            size,
            modified: now() - Duration::from_secs(age_days * SECONDS_PER_DAY),
        }
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn check(condition: &Condition, facts: &FileFacts) -> bool {
        condition.matches(facts, now()).unwrap()
    }

    #[test]
    fn extension_matches_regardless_of_case_and_leading_dot() {
        let file = facts("invoice.PDF", 10, 0);
        let with_dot = Condition::Extension {
            any_of: vec![".pdf".into()],
        };
        let without_dot = Condition::Extension {
            any_of: vec!["pdf".into()],
        };

        assert!(check(&with_dot, &file));
        assert!(check(&without_dot, &file));
    }

    #[test]
    fn extension_does_not_match_a_file_without_one() {
        let condition = Condition::Extension {
            any_of: vec!["pdf".into()],
        };
        assert!(!check(&condition, &facts("README", 10, 0)));
    }

    #[test]
    fn name_contains_is_case_insensitive_and_handles_unicode() {
        let condition = Condition::NameContains {
            text: "factura".into(),
        };
        assert!(check(&condition, &facts("FACTURA-2026.pdf", 1, 0)));
        assert!(check(&condition, &facts("mi factura año.pdf", 1, 0)));
        assert!(!check(&condition, &facts("recibo.pdf", 1, 0)));
    }

    #[test]
    fn glob_matches_the_file_name_only() {
        let condition = Condition::NameMatchesGlob {
            pattern: "Screenshot*.png".into(),
        };
        assert!(check(&condition, &facts("Screenshot 2026-08-18.png", 1, 0)));
        assert!(check(&condition, &facts("sub/screenshot A.png", 1, 0)));
        assert!(!check(&condition, &facts("photo.png", 1, 0)));
    }

    #[test]
    fn regex_is_case_sensitive_unlike_glob() {
        let condition = Condition::NameMatchesRegex {
            pattern: r"^IMG_\d{4}\.jpe?g$".into(),
        };
        assert!(check(&condition, &facts("IMG_0421.jpg", 1, 0)));
        assert!(check(&condition, &facts("IMG_0421.jpeg", 1, 0)));
        assert!(!check(&condition, &facts("img_0421.jpg", 1, 0)));
    }

    #[test]
    fn an_oversized_pattern_is_refused_instead_of_compiled() {
        let condition = Condition::NameMatchesRegex {
            pattern: "a".repeat(MAX_PATTERN_LENGTH + 1),
        };
        let outcome = condition.matches(&facts("x.txt", 1, 0), now());
        assert!(matches!(outcome, Err(ConditionError::PatternTooLong)));
    }

    #[test]
    fn an_invalid_regex_reports_an_error_rather_than_matching() {
        let condition = Condition::NameMatchesRegex {
            pattern: "(unclosed".into(),
        };
        let outcome = condition.matches(&facts("x.txt", 1, 0), now());
        assert!(matches!(outcome, Err(ConditionError::InvalidRegex)));
    }

    #[test]
    fn size_bounds_are_strict() {
        let file = facts("a.bin", 1_000, 0);
        assert!(check(&Condition::LargerThan { bytes: 999 }, &file));
        assert!(!check(&Condition::LargerThan { bytes: 1_000 }, &file));
        assert!(check(&Condition::SmallerThan { bytes: 1_001 }, &file));
        assert!(!check(&Condition::SmallerThan { bytes: 1_000 }, &file));
    }

    #[test]
    fn age_is_measured_in_whole_days() {
        let old = facts("a.txt", 1, 30);
        let fresh = facts("b.txt", 1, 1);
        assert!(check(&Condition::OlderThan { days: 30 }, &old));
        assert!(!check(&Condition::OlderThan { days: 31 }, &old));
        assert!(check(&Condition::NewerThan { days: 7 }, &fresh));
        assert!(!check(&Condition::NewerThan { days: 7 }, &old));
    }

    #[test]
    fn a_file_modified_in_the_future_counts_as_brand_new() {
        let mut file = facts("a.txt", 1, 0);
        file.modified = now() + Duration::from_secs(SECONDS_PER_DAY * 5);
        assert!(check(&Condition::NewerThan { days: 1 }, &file));
    }

    #[test]
    fn subfolder_matches_any_component_below_the_root() {
        let condition = Condition::InSubfolder {
            name: "Invoices".into(),
        };
        assert!(check(&condition, &facts("Invoices/a.pdf", 1, 0)));
        assert!(check(&condition, &facts("2026/Invoices/a.pdf", 1, 0)));
        assert!(!check(&condition, &facts("a.pdf", 1, 0)));
        assert!(!check(&condition, &facts("Receipts/a.pdf", 1, 0)));
    }

    #[test]
    fn conditions_survive_a_json_round_trip() {
        let original = Condition::NameMatchesGlob {
            pattern: "*.pdf".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("nameMatchesGlob"));
        assert_eq!(serde_json::from_str::<Condition>(&json).unwrap(), original);
    }

    fn many_facts(count: usize) -> Vec<FileFacts> {
        (0..count)
            .map(|index| facts(&format!("Screenshot {index}.png"), 1, 0))
            .collect()
    }

    fn elapsed_millis(condition: &Condition, files: &[FileFacts]) -> u128 {
        let started = std::time::Instant::now();
        for file in files {
            let _ = condition.matches(file, now());
        }
        started.elapsed().as_millis()
    }

    #[test]
    #[ignore = "measurement, run with: cargo test -- --ignored --nocapture"]
    fn measure_cost_of_recompiling_patterns_per_file() {
        const FILES: usize = 10_000;
        let files = many_facts(FILES);

        let extension = Condition::Extension {
            any_of: vec!["png".into()],
        };
        let contains = Condition::NameContains {
            text: "screenshot".into(),
        };
        let glob = Condition::NameMatchesGlob {
            pattern: "Screenshot*.png".into(),
        };
        let regex = Condition::NameMatchesRegex {
            pattern: r"^Screenshot \d+\.png$".into(),
        };

        println!("--- {FILES} files, one condition each ---");
        println!(
            "extension      {:>6} ms",
            elapsed_millis(&extension, &files)
        );
        println!("nameContains   {:>6} ms", elapsed_millis(&contains, &files));
        println!("glob           {:>6} ms", elapsed_millis(&glob, &files));
        println!("regex          {:>6} ms", elapsed_millis(&regex, &files));
    }
}
