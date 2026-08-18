use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local};

const MAX_TEMPLATE_LENGTH: usize = 256;
const MAX_NAME_LENGTH: usize = 200;

const FORBIDDEN_CHARACTERS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

const RESERVED_STEMS: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("the template is longer than the allowed limit")]
    TooLong,
    #[error("the template contains an unknown placeholder")]
    UnknownPlaceholder,
    #[error("the template would escape the destination folder")]
    PathTraversal,
    #[error("the result contains a character that is not valid in a file name")]
    ForbiddenCharacter,
    #[error("the result is a name reserved by the operating system")]
    ReservedName,
    #[error("the result is empty or would be trimmed away")]
    Unusable,
}

#[derive(Debug, Clone, Copy)]
pub struct Substitutions<'a> {
    pub path: &'a Path,
    pub modified: SystemTime,
    pub counter: u32,
}

pub fn render(template: &str, values: Substitutions) -> Result<String, TemplateError> {
    if template.len() > MAX_TEMPLATE_LENGTH {
        return Err(TemplateError::TooLong);
    }
    reject_traversal(template)?;
    let rendered = substitute(template, values)?;
    validate(&rendered)?;
    Ok(rendered)
}

pub fn render_subfolder(template: &str, values: Substitutions) -> Result<PathBuf, TemplateError> {
    if template.len() > MAX_TEMPLATE_LENGTH {
        return Err(TemplateError::TooLong);
    }
    reject_traversal(template)?;
    build_relative(&substitute(template, values)?)
}

fn build_relative(rendered: &str) -> Result<PathBuf, TemplateError> {
    let mut relative = PathBuf::new();
    for component in rendered.split(['/', '\\']) {
        validate(component)?;
        relative.push(component);
    }
    Ok(relative)
}

fn reject_traversal(template: &str) -> Result<(), TemplateError> {
    if template.contains("..") {
        return Err(TemplateError::PathTraversal);
    }
    Ok(())
}

fn substitute(template: &str, values: Substitutions) -> Result<String, TemplateError> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('}').ok_or(TemplateError::UnknownPlaceholder)?;
        rendered.push_str(&resolve(&after[..end], values)?);
        rest = &after[end + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn resolve(placeholder: &str, values: Substitutions) -> Result<String, TemplateError> {
    let local: DateTime<Local> = values.modified.into();
    match placeholder {
        "name" => Ok(component(values.path.file_stem())),
        "ext" => Ok(component(values.path.extension())),
        "date" => Ok(local.format("%Y-%m-%d").to_string()),
        "year" => Ok(local.format("%Y").to_string()),
        "month" => Ok(local.format("%m").to_string()),
        "day" => Ok(local.format("%d").to_string()),
        "counter" => Ok(values.counter.to_string()),
        _ => Err(TemplateError::UnknownPlaceholder),
    }
}

fn component(part: Option<&std::ffi::OsStr>) -> String {
    part.and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn validate(rendered: &str) -> Result<(), TemplateError> {
    if rendered
        .chars()
        .any(|character| FORBIDDEN_CHARACTERS.contains(&character) || character.is_control())
    {
        return Err(TemplateError::ForbiddenCharacter);
    }
    let trimmed = rendered.trim_end_matches([' ', '.']).trim();
    if trimmed.is_empty() || trimmed != rendered || rendered.len() > MAX_NAME_LENGTH {
        return Err(TemplateError::Unusable);
    }
    reject_reserved(rendered)
}

fn reject_reserved(rendered: &str) -> Result<(), TemplateError> {
    let stem = rendered
        .split_once('.')
        .map_or(rendered, |(before, _)| before)
        .to_ascii_lowercase();
    if RESERVED_STEMS.contains(&stem.as_str()) {
        return Err(TemplateError::ReservedName);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn values(name: &str) -> (PathBuf, SystemTime) {
        let moment = Local
            .with_ymd_and_hms(2026, 3, 7, 15, 30, 0)
            .single()
            .unwrap();
        (PathBuf::from(name), SystemTime::from(moment))
    }

    fn render_for(template: &str, name: &str) -> Result<String, TemplateError> {
        let (path, modified) = values(name);
        render(
            template,
            Substitutions {
                path: &path,
                modified,
                counter: 3,
            },
        )
    }

    #[test]
    fn placeholders_are_replaced_with_their_values() {
        assert_eq!(
            render_for("{date} {name}.{ext}", "invoice.pdf").unwrap(),
            "2026-03-07 invoice.pdf"
        );
    }

    #[test]
    fn date_parts_are_available_separately() {
        assert_eq!(
            render_for("{year}-{month}/{day}", "a.txt").unwrap_err(),
            TemplateError::ForbiddenCharacter
        );
        assert_eq!(
            render_for("{year}_{month}_{day}", "a.txt").unwrap(),
            "2026_03_07"
        );
    }

    #[test]
    fn the_counter_is_substituted() {
        assert_eq!(
            render_for("scan-{counter}.{ext}", "x.png").unwrap(),
            "scan-3.png"
        );
    }

    #[test]
    fn text_without_placeholders_passes_through() {
        assert_eq!(
            render_for("fixed name.txt", "a.pdf").unwrap(),
            "fixed name.txt"
        );
    }

    #[test]
    fn a_file_without_extension_yields_an_empty_ext() {
        assert_eq!(render_for("{name}", "README").unwrap(), "README");
    }

    #[test]
    fn unicode_names_survive_intact() {
        assert_eq!(
            render_for("{name}.{ext}", "informe año señor.pdf").unwrap(),
            "informe año señor.pdf"
        );
    }

    #[test]
    fn an_unknown_placeholder_is_refused() {
        assert_eq!(
            render_for("{nombre}.pdf", "a.pdf").unwrap_err(),
            TemplateError::UnknownPlaceholder
        );
    }

    #[test]
    fn an_unclosed_placeholder_is_refused() {
        assert_eq!(
            render_for("{name.pdf", "a.pdf").unwrap_err(),
            TemplateError::UnknownPlaceholder
        );
    }

    #[test]
    fn traversal_is_refused_before_anything_is_rendered() {
        for template in ["../{name}", "..\\{name}", "{name}/../../etc/passwd"] {
            assert_eq!(
                render_for(template, "a.pdf").unwrap_err(),
                TemplateError::PathTraversal,
                "template {template} was not refused"
            );
        }
    }

    #[test]
    fn separators_are_refused_so_a_template_cannot_build_a_path() {
        for template in ["folder/{name}", "folder\\{name}", "C:{name}"] {
            assert_eq!(
                render_for(template, "a.pdf").unwrap_err(),
                TemplateError::ForbiddenCharacter,
                "template {template} was not refused"
            );
        }
    }

    #[test]
    fn characters_windows_rejects_are_refused() {
        for template in ["a*b", "a?b", "a<b", "a>b", "a|b", "a\"b"] {
            assert_eq!(
                render_for(template, "a.pdf").unwrap_err(),
                TemplateError::ForbiddenCharacter,
                "template {template} was not refused"
            );
        }
    }

    #[test]
    fn a_substituted_value_can_still_be_a_reserved_name() {
        assert_eq!(
            render_for("{name}", "CON.txt").unwrap_err(),
            TemplateError::ReservedName
        );
    }

    #[test]
    fn a_substituted_value_cannot_become_a_relative_component() {
        for name in ["..", "."] {
            assert_eq!(
                render_for("{name}", name).unwrap_err(),
                TemplateError::Unusable,
                "name {name} was not refused"
            );
        }
    }

    #[test]
    fn a_directory_component_in_the_source_never_reaches_the_result() {
        assert_eq!(render_for("{name}", "../../escape").unwrap(), "escape");
    }

    #[test]
    fn windows_reserved_names_are_refused() {
        for template in ["CON", "nul", "COM1.txt", "LPT9.log", "Aux"] {
            assert_eq!(
                render_for(template, "a.pdf").unwrap_err(),
                TemplateError::ReservedName,
                "template {template} was not refused"
            );
        }
    }

    #[test]
    fn a_name_windows_would_trim_is_refused() {
        for template in ["name.", "name ", " name", "..."] {
            let outcome = render_for(template, "a.pdf").unwrap_err();
            assert!(
                matches!(
                    outcome,
                    TemplateError::Unusable | TemplateError::PathTraversal
                ),
                "template {template} gave {outcome:?}"
            );
        }
    }

    #[test]
    fn an_empty_result_is_refused() {
        assert_eq!(
            render_for("", "a.pdf").unwrap_err(),
            TemplateError::Unusable
        );
        assert_eq!(
            render_for("{ext}", "README").unwrap_err(),
            TemplateError::Unusable
        );
    }

    #[test]
    fn an_oversized_template_is_refused_before_rendering() {
        let template = "a".repeat(MAX_TEMPLATE_LENGTH + 1);
        assert_eq!(
            render_for(&template, "a.pdf").unwrap_err(),
            TemplateError::TooLong
        );
    }

    #[test]
    fn a_result_that_grows_past_the_name_limit_is_refused() {
        let long = "a".repeat(MAX_NAME_LENGTH);
        let template = format!("{long}{{name}}");
        assert_eq!(
            render_for(&template, "a.pdf").unwrap_err(),
            TemplateError::Unusable
        );
    }

    fn subfolder_for(template: &str, name: &str) -> Result<PathBuf, TemplateError> {
        let (path, modified) = values(name);
        render_subfolder(
            template,
            Substitutions {
                path: &path,
                modified,
                counter: 3,
            },
        )
    }

    #[test]
    fn a_subfolder_template_builds_a_relative_path() {
        assert_eq!(
            subfolder_for("{year}/{month}", "a.pdf").unwrap(),
            PathBuf::from("2026").join("03")
        );
    }

    #[test]
    fn a_subfolder_can_group_by_extension() {
        assert_eq!(
            subfolder_for("by type/{ext}", "photo.jpg").unwrap(),
            PathBuf::from("by type").join("jpg")
        );
    }

    #[test]
    fn a_subfolder_accepts_backslashes_as_separators() {
        assert_eq!(
            subfolder_for(r"{year}\{month}", "a.pdf").unwrap(),
            PathBuf::from("2026").join("03")
        );
    }

    #[test]
    fn a_subfolder_cannot_be_absolute() {
        for template in ["/{year}", r"\{year}", "C:/{year}"] {
            assert!(
                subfolder_for(template, "a.pdf").is_err(),
                "template {template} was not refused"
            );
        }
    }

    #[test]
    fn a_subfolder_cannot_climb_out() {
        assert_eq!(
            subfolder_for("{year}/../..", "a.pdf").unwrap_err(),
            TemplateError::PathTraversal
        );
    }

    #[test]
    fn a_subfolder_rejects_empty_components() {
        assert_eq!(
            subfolder_for("{year}//{month}", "a.pdf").unwrap_err(),
            TemplateError::Unusable
        );
        assert_eq!(
            subfolder_for("", "a.pdf").unwrap_err(),
            TemplateError::Unusable
        );
    }

    #[test]
    fn a_subfolder_component_cannot_be_a_reserved_name() {
        assert_eq!(
            subfolder_for("archive/{name}", "NUL.txt").unwrap_err(),
            TemplateError::ReservedName
        );
    }
}
