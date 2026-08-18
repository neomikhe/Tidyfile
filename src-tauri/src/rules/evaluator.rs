use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Action, Combinator, ConditionError, FileFacts, Rule};
use crate::journal::Operation;
use crate::templates::{self, Substitutions, TemplateError};

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("a condition of rule '{rule}' could not be evaluated")]
    Condition {
        rule: String,
        #[source]
        source: ConditionError,
    },
    #[error("a rename template of rule '{rule}' could not be applied")]
    Template {
        rule: String,
        #[source]
        source: TemplateError,
    },
    #[error("the file has no usable name")]
    NamelessFile,
}

#[derive(Debug, Clone, Copy)]
pub struct PlanContext {
    pub now: SystemTime,
    pub counter: u32,
}

pub fn plan(
    rules: &[Rule],
    facts: &FileFacts,
    context: PlanContext,
) -> Result<Vec<Operation>, EvaluationError> {
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if applies(rule, facts, context.now)? {
            return operations(rule, facts, context);
        }
    }
    Ok(Vec::new())
}

fn applies(rule: &Rule, facts: &FileFacts, now: SystemTime) -> Result<bool, EvaluationError> {
    if rule.conditions.is_empty() {
        return Ok(false);
    }
    let mut outcomes = Vec::with_capacity(rule.conditions.len());
    for condition in &rule.conditions {
        outcomes.push(condition.matches(facts, now).map_err(|source| {
            EvaluationError::Condition {
                rule: rule.name.clone(),
                source,
            }
        })?);
    }
    Ok(match rule.combinator {
        Combinator::All => outcomes.iter().all(|matched| *matched),
        Combinator::Any => outcomes.iter().any(|matched| *matched),
    })
}

fn operations(
    rule: &Rule,
    facts: &FileFacts,
    context: PlanContext,
) -> Result<Vec<Operation>, EvaluationError> {
    rule.actions
        .iter()
        .map(|action| operation_for(rule, action, facts, context))
        .collect()
}

fn operation_for(
    rule: &Rule,
    action: &Action,
    facts: &FileFacts,
    context: PlanContext,
) -> Result<Operation, EvaluationError> {
    let from = facts.path.clone();
    match action {
        Action::MoveTo { folder, rename } => Ok(Operation::Move {
            to: folder.join(target_name(rule, rename.as_deref(), facts, context)?),
            from,
        }),
        Action::CopyTo { folder, rename } => Ok(Operation::Copy {
            to: folder.join(target_name(rule, rename.as_deref(), facts, context)?),
            from,
        }),
        Action::RenameTo { template } => Ok(Operation::Move {
            to: parent_of(facts)?.join(render(rule, template, facts, context)?),
            from,
        }),
        Action::Trash => Ok(Operation::Trash { from }),
    }
}

fn target_name(
    rule: &Rule,
    template: Option<&str>,
    facts: &FileFacts,
    context: PlanContext,
) -> Result<OsString, EvaluationError> {
    match template {
        Some(template) => Ok(OsString::from(render(rule, template, facts, context)?)),
        None => facts
            .path
            .file_name()
            .map(OsString::from)
            .ok_or(EvaluationError::NamelessFile),
    }
}

fn render(
    rule: &Rule,
    template: &str,
    facts: &FileFacts,
    context: PlanContext,
) -> Result<String, EvaluationError> {
    templates::render(
        template,
        Substitutions {
            path: &facts.path,
            modified: facts.modified,
            counter: context.counter,
        },
    )
    .map_err(|source| EvaluationError::Template {
        rule: rule.name.clone(),
        source,
    })
}

fn parent_of(facts: &FileFacts) -> Result<PathBuf, EvaluationError> {
    facts
        .path
        .parent()
        .map(Path::to_path_buf)
        .ok_or(EvaluationError::NamelessFile)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::rules::Condition;
    use chrono::{Local, TimeZone};

    fn now() -> SystemTime {
        SystemTime::from(
            Local
                .with_ymd_and_hms(2026, 3, 7, 15, 30, 0)
                .single()
                .unwrap(),
        )
    }

    fn context() -> PlanContext {
        PlanContext {
            now: now(),
            counter: 3,
        }
    }

    fn facts(relative: &str) -> FileFacts {
        let root = PathBuf::from("/watched");
        FileFacts {
            path: root.join(relative),
            root,
            size: 10,
            modified: now(),
        }
    }

    fn move_action(folder: &str) -> Action {
        Action::MoveTo {
            folder: PathBuf::from(folder),
            rename: None,
        }
    }

    fn rule(name: &str, combinator: Combinator, conditions: Vec<Condition>) -> Rule {
        Rule {
            id: format!("id-{name}"),
            name: name.into(),
            enabled: true,
            combinator,
            conditions,
            actions: vec![move_action(&format!("/out/{name}"))],
        }
    }

    fn is_pdf() -> Condition {
        Condition::Extension {
            any_of: vec!["pdf".into()],
        }
    }

    fn named_invoice() -> Condition {
        Condition::NameContains {
            text: "invoice".into(),
        }
    }

    #[test]
    fn a_matching_rule_produces_its_operations() {
        let rules = [rule(
            "invoices",
            Combinator::All,
            vec![is_pdf(), named_invoice()],
        )];

        let operations = plan(&rules, &facts("invoice-2026.pdf"), context()).unwrap();

        assert_eq!(
            operations,
            [Operation::Move {
                from: PathBuf::from("/watched/invoice-2026.pdf"),
                to: PathBuf::from("/out/invoices/invoice-2026.pdf"),
            }]
        );
    }

    #[test]
    fn all_requires_every_condition() {
        let rules = [rule(
            "invoices",
            Combinator::All,
            vec![is_pdf(), named_invoice()],
        )];

        assert!(
            plan(&rules, &facts("invoice.txt"), context())
                .unwrap()
                .is_empty()
        );
        assert!(
            plan(&rules, &facts("receipt.pdf"), context())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn any_requires_only_one_condition() {
        let rules = [rule(
            "loose",
            Combinator::Any,
            vec![is_pdf(), named_invoice()],
        )];

        assert_eq!(
            plan(&rules, &facts("invoice.txt"), context())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            plan(&rules, &facts("receipt.pdf"), context())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_rule_without_conditions_never_matches() {
        let rules = [rule("empty", Combinator::All, Vec::new())];

        let operations = plan(&rules, &facts("anything.pdf"), context()).unwrap();

        assert!(
            operations.is_empty(),
            "an empty condition list must not match every file"
        );
    }

    #[test]
    fn a_disabled_rule_is_ignored() {
        let mut disabled = rule("invoices", Combinator::All, vec![is_pdf()]);
        disabled.enabled = false;

        let operations = plan(&[disabled], &facts("invoice.pdf"), context()).unwrap();

        assert!(operations.is_empty());
    }

    #[test]
    fn contradictory_rules_resolve_to_the_first_match() {
        let rules = [
            rule("first", Combinator::All, vec![is_pdf()]),
            rule("second", Combinator::All, vec![is_pdf()]),
        ];

        let operations = plan(&rules, &facts("a.pdf"), context()).unwrap();

        assert_eq!(operations.len(), 1);
        let Operation::Move { to, .. } = &operations[0] else {
            panic!("expected a move");
        };
        assert!(to.starts_with("/out/first"), "the later rule won: {to:?}");
    }

    #[test]
    fn a_rule_can_carry_several_actions() {
        let mut multi = rule("both", Combinator::All, vec![is_pdf()]);
        multi.actions.push(Action::Trash);

        let operations = plan(&[multi], &facts("a.pdf"), context()).unwrap();

        assert_eq!(operations.len(), 2);
        assert!(matches!(operations[1], Operation::Trash { .. }));
    }

    #[test]
    fn no_rules_means_no_operations() {
        assert!(plan(&[], &facts("a.pdf"), context()).unwrap().is_empty());
    }

    #[test]
    fn an_invalid_pattern_names_the_rule_that_carries_it() {
        let rules = [rule(
            "broken",
            Combinator::All,
            vec![Condition::NameMatchesRegex {
                pattern: "(unclosed".into(),
            }],
        )];

        let error = plan(&rules, &facts("a.pdf"), context()).unwrap_err();

        assert!(matches!(
            error,
            EvaluationError::Condition { ref rule, .. } if rule == "broken"
        ));
    }

    #[test]
    fn copy_to_produces_a_copy_operation() {
        let mut copying = rule("backup", Combinator::All, vec![is_pdf()]);
        copying.actions = vec![Action::CopyTo {
            folder: PathBuf::from("/backup"),
            rename: None,
        }];

        let operations = plan(&[copying], &facts("a.pdf"), context()).unwrap();

        assert_eq!(
            operations,
            [Operation::Copy {
                from: PathBuf::from("/watched/a.pdf"),
                to: PathBuf::from("/backup/a.pdf"),
            }]
        );
    }

    #[test]
    fn moving_can_rename_at_the_same_time() {
        let mut renaming = rule("dated", Combinator::All, vec![is_pdf()]);
        renaming.actions = vec![Action::MoveTo {
            folder: PathBuf::from("/out"),
            rename: Some("{date} {name}.{ext}".into()),
        }];

        let operations = plan(&[renaming], &facts("invoice.pdf"), context()).unwrap();

        let Operation::Move { to, .. } = &operations[0] else {
            panic!("expected a move");
        };
        assert_eq!(to, &PathBuf::from("/out/2026-03-07 invoice.pdf"));
    }

    #[test]
    fn rename_to_keeps_the_file_in_its_folder() {
        let mut renaming = rule("counted", Combinator::All, vec![is_pdf()]);
        renaming.actions = vec![Action::RenameTo {
            template: "scan-{counter}.{ext}".into(),
        }];

        let operations = plan(&[renaming], &facts("sub/a.pdf"), context()).unwrap();

        let Operation::Move { to, .. } = &operations[0] else {
            panic!("expected a move");
        };
        assert_eq!(to, &PathBuf::from("/watched/sub/scan-3.pdf"));
    }

    #[test]
    fn a_template_that_escapes_the_folder_names_the_rule() {
        let mut hostile = rule("escaping", Combinator::All, vec![is_pdf()]);
        hostile.actions = vec![Action::RenameTo {
            template: "../{name}".into(),
        }];

        let error = plan(&[hostile], &facts("a.pdf"), context()).unwrap_err();

        assert!(matches!(
            error,
            EvaluationError::Template {
                ref rule,
                source: TemplateError::PathTraversal
            } if rule == "escaping"
        ));
    }

    #[test]
    fn a_rule_survives_a_json_round_trip() {
        let original = rule("invoices", Combinator::All, vec![is_pdf(), named_invoice()]);

        let json = serde_json::to_string(&original).unwrap();
        let restored: Rule = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, original);
    }

    #[test]
    fn move_to_still_parses_without_a_rename_field() {
        let json = r#"{"type":"moveTo","folder":"/out"}"#;

        let action: Action = serde_json::from_str(json).unwrap();

        assert_eq!(action, move_action("/out"));
    }
}
