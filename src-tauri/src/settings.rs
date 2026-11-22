use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::executor::Collision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Es,
    En,
    // WHY: a language written by a newer version must not make the whole settings file unreadable
    #[default]
    #[serde(other)]
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub folders: Vec<PathBuf>,
    pub watched: Vec<PathBuf>,
    pub on_collision: Collision,
    pub language: Language,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_for_a_first_launch() {
        let fresh = Settings::default();

        assert!(fresh.folders.is_empty());
        assert!(
            fresh.watched.is_empty(),
            "watching must not start on by itself"
        );
        assert_eq!(fresh.on_collision, Collision::Suffix);
        assert_eq!(fresh.language, Language::Auto);
    }

    #[test]
    fn an_old_file_without_language_loads_as_auto() {
        let old: Settings =
            serde_json::from_str(r#"{"folders":[],"watched":[],"onCollision":"skip"}"#).unwrap();

        assert_eq!(old.language, Language::Auto);
        assert_eq!(old.on_collision, Collision::Skip);
    }

    #[test]
    fn an_unknown_language_loads_as_auto() {
        let unknown: Settings = serde_json::from_str(r#"{"language":"fr"}"#).unwrap();

        assert_eq!(unknown.language, Language::Auto);
    }

    #[test]
    fn language_round_trips_in_lowercase() {
        let json = serde_json::to_string(&Language::Es).unwrap();

        assert_eq!(json, r#""es""#);
        assert_eq!(
            serde_json::from_str::<Language>(&json).unwrap(),
            Language::Es
        );
    }

    #[test]
    fn settings_survive_a_json_round_trip() {
        let chosen = Settings {
            folders: vec![PathBuf::from("/watched")],
            watched: vec![PathBuf::from("/watched")],
            on_collision: Collision::Skip,
            language: Language::En,
        };

        let json = serde_json::to_string(&chosen).unwrap();

        assert!(json.contains("onCollision"));
        assert!(json.contains(r#""language":"en""#));
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), chosen);
    }

    #[test]
    fn a_partial_file_falls_back_to_defaults_field_by_field() {
        let partial: Settings = serde_json::from_str(r#"{"folders":["/only-this"]}"#).unwrap();

        assert_eq!(partial.folders, [PathBuf::from("/only-this")]);
        assert!(partial.watched.is_empty());
        assert_eq!(partial.on_collision, Collision::Suffix);
    }
}
