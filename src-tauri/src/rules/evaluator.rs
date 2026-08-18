use std::time::SystemTime;

use super::{Action, Combinator, ConditionError, FileFacts, Rule};
use crate::journal::Operation;

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("a condition of rule '{rule}' could not be evaluated")]
    Condition {
        rule: String,
        #[source]
        source: ConditionError,
    },
}

pub fn plan(
    rules: &[Rule],
    facts: &FileFacts,
    now: SystemTime,
) -> Result<Vec<Operation>, EvaluationError> {
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if applies(rule, facts, now)? {
            return Ok(operations(rule, facts));
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

fn operations(rule: &Rule, facts: &FileFacts) -> Vec<Operation> {
    let Some(name) = facts.path.file_name() else {
        return Vec::new();
    };
    rule.actions
        .iter()
        .map(|action| match action {
            Action::MoveTo { folder } => Operation::Move {
                from: facts.path.clone(),
                to: folder.join(name),
            },
            Action::Trash => Operation::Trash {
                from: facts.path.clone(),
            },
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::rules::Condition;
    use std::path::PathBuf;
    use std::time::Duration;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn facts(relative: &str, size: u64) -> FileFacts {
        let root = PathBuf::from("/watched");
        FileFacts {
            path: root.join(relative),
            root,
            size,
            modified: now(),
        }
    }

    fn rule(name: &str, combinator: Combinator, conditions: Vec<Condition>) -> Rule {
        Rule {
            id: format!("id-{name}"),
            name: name.into(),
            enabled: true,
            combinator,
            conditions,
            actions: vec![Action::MoveTo {
                folder: PathBuf::from("/out").join(name),
            }],
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

        let operations = plan(&rules, &facts("invoice-2026.pdf", 10), now()).unwrap();

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
            plan(&rules, &facts("invoice.txt", 10), now())
                .unwrap()
                .is_empty()
        );
        assert!(
            plan(&rules, &facts("receipt.pdf", 10), now())
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
            plan(&rules, &facts("invoice.txt", 10), now())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            plan(&rules, &facts("receipt.pdf", 10), now())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_rule_without_conditions_never_matches() {
        let rules = [rule("empty", Combinator::All, Vec::new())];

        let operations = plan(&rules, &facts("anything.pdf", 10), now()).unwrap();

        assert!(
            operations.is_empty(),
            "an empty condition list must not match every file"
        );
    }

    #[test]
    fn a_disabled_rule_is_ignored() {
        let mut disabled = rule("invoices", Combinator::All, vec![is_pdf()]);
        disabled.enabled = false;

        let operations = plan(&[disabled], &facts("invoice.pdf", 10), now()).unwrap();

        assert!(operations.is_empty());
    }

    #[test]
    fn contradictory_rules_resolve_to_the_first_match() {
        let rules = [
            rule("first", Combinator::All, vec![is_pdf()]),
            rule("second", Combinator::All, vec![is_pdf()]),
        ];

        let operations = plan(&rules, &facts("a.pdf", 10), now()).unwrap();

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

        let operations = plan(&[multi], &facts("a.pdf", 10), now()).unwrap();

        assert_eq!(operations.len(), 2);
        assert!(matches!(operations[1], Operation::Trash { .. }));
    }

    #[test]
    fn no_rules_means_no_operations() {
        assert!(plan(&[], &facts("a.pdf", 10), now()).unwrap().is_empty());
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

        let error = plan(&rules, &facts("a.pdf", 10), now()).unwrap_err();

        let EvaluationError::Condition { rule, .. } = error;
        assert_eq!(rule, "broken");
    }

    #[test]
    fn a_rule_survives_a_json_round_trip() {
        let original = rule("invoices", Combinator::All, vec![is_pdf(), named_invoice()]);

        let json = serde_json::to_string(&original).unwrap();
        let restored: Rule = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, original);
    }
}
