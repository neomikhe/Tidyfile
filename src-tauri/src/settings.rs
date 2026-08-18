use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::executor::Collision;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub folder: Option<PathBuf>,
    pub watching: bool,
    pub on_collision: Collision,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_for_a_first_launch() {
        let fresh = Settings::default();

        assert_eq!(fresh.folder, None);
        assert!(!fresh.watching, "watching must not start on by itself");
        assert_eq!(fresh.on_collision, Collision::Suffix);
    }

    #[test]
    fn settings_survive_a_json_round_trip() {
        let chosen = Settings {
            folder: Some(PathBuf::from("/watched")),
            watching: true,
            on_collision: Collision::Skip,
        };

        let json = serde_json::to_string(&chosen).unwrap();

        assert!(json.contains("onCollision"));
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), chosen);
    }

    #[test]
    fn a_partial_file_falls_back_to_defaults_field_by_field() {
        let partial: Settings = serde_json::from_str(r#"{"folder":"/only-this"}"#).unwrap();

        assert_eq!(partial.folder, Some(PathBuf::from("/only-this")));
        assert!(!partial.watching);
        assert_eq!(partial.on_collision, Collision::Suffix);
    }
}
