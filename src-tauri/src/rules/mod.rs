pub mod condition;
pub mod evaluator;

pub use condition::{Condition, ConditionError, FileFacts};
pub use evaluator::{EvaluationError, plan};

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Combinator {
    All,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    MoveTo { folder: PathBuf },
    Trash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub combinator: Combinator,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
}
