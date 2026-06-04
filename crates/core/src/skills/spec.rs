use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::model::Skill;
use super::resource_access::normalize_skill_resource_path;

pub const NEXA_SKILL_SPEC_VERSION: u16 = 1;
pub const MAX_SKILL_NAME_CHARS: usize = 96;
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 1_200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSpecIssue {
    pub code: String,
    pub message: String,
}

impl SkillSpecIssue {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSpecReport {
    pub version: u16,
    pub valid: bool,
    pub issues: Vec<SkillSpecIssue>,
}

pub fn validate_skill_spec(skill: &Skill) -> SkillSpecReport {
    let mut issues = Vec::new();

    validate_skill_id(&skill.id, &mut issues);
    validate_skill_name(&skill.name, &mut issues);
    validate_description(&skill.description, &mut issues);
    validate_resources(skill, &mut issues);
    validate_dependencies(skill, &mut issues);

    SkillSpecReport {
        version: NEXA_SKILL_SPEC_VERSION,
        valid: issues.is_empty(),
        issues,
    }
}

fn validate_skill_id(id: &str, issues: &mut Vec<SkillSpecIssue>) {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        issues.push(SkillSpecIssue::new("id.empty", "skill id must be set"));
        return;
    }
    if trimmed != id {
        issues.push(SkillSpecIssue::new(
            "id.whitespace",
            "skill id must not contain leading or trailing whitespace",
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        issues.push(SkillSpecIssue::new(
            "id.invalid_chars",
            "skill id may only contain ASCII letters, digits, hyphen, underscore, or dot",
        ));
    }
}

fn validate_skill_name(name: &str, issues: &mut Vec<SkillSpecIssue>) {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        issues.push(SkillSpecIssue::new("name.empty", "skill name must be set"));
        return;
    }
    if trimmed.chars().count() > MAX_SKILL_NAME_CHARS {
        issues.push(SkillSpecIssue::new(
            "name.too_long",
            format!("skill name must be at most {MAX_SKILL_NAME_CHARS} characters"),
        ));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        issues.push(SkillSpecIssue::new(
            "name.control_char",
            "skill name must not contain control characters",
        ));
    }
}

fn validate_description(description: &str, issues: &mut Vec<SkillSpecIssue>) {
    if description.chars().count() > MAX_SKILL_DESCRIPTION_CHARS {
        issues.push(SkillSpecIssue::new(
            "description.too_long",
            format!("skill description must be at most {MAX_SKILL_DESCRIPTION_CHARS} characters"),
        ));
    }
    if description
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        issues.push(SkillSpecIssue::new(
            "description.control_char",
            "skill description must not contain control characters",
        ));
    }
}

fn validate_resources(skill: &Skill, issues: &mut Vec<SkillSpecIssue>) {
    let mut seen = HashSet::new();
    for resource in &skill.resources {
        match normalize_skill_resource_path(&resource.path) {
            Ok(normalized) => {
                if normalized != resource.path {
                    issues.push(SkillSpecIssue::new(
                        "resource.path_not_normalized",
                        format!(
                            "resource path `{}` should normalize to `{normalized}`",
                            resource.path
                        ),
                    ));
                }
                if !seen.insert(normalized.clone()) {
                    issues.push(SkillSpecIssue::new(
                        "resource.duplicate_path",
                        format!("resource path `{normalized}` is duplicated"),
                    ));
                }
            }
            Err(err) => issues.push(SkillSpecIssue::new(
                "resource.invalid_path",
                format!("resource path `{}` is invalid: {err}", resource.path),
            )),
        }
    }

    for resource in &skill.resource_bundle {
        if let Err(err) = normalize_skill_resource_path(&resource.path) {
            issues.push(SkillSpecIssue::new(
                "resource.invalid_bundle_path",
                format!(
                    "bundled resource path `{}` is invalid: {err}",
                    resource.path
                ),
            ));
        }
    }
}

fn validate_dependencies(skill: &Skill, issues: &mut Vec<SkillSpecIssue>) {
    for dependency in &skill.dependencies.tools {
        let kind = dependency.kind.trim();
        let value = dependency.value.trim();
        if kind.is_empty() && value.is_empty() {
            issues.push(SkillSpecIssue::new(
                "dependency.empty",
                "tool dependency must include a kind or value",
            ));
        }
        if kind.chars().any(|ch| ch.is_control()) || value.chars().any(|ch| ch.is_control()) {
            issues.push(SkillSpecIssue::new(
                "dependency.control_char",
                "tool dependency fields must not contain control characters",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillDependencies, SkillInterfaceMetadata, SkillPolicy, SkillResourceEncoding,
        SkillResourceFile, SkillResourceInfo, SkillResourceKind,
    };

    fn valid_skill() -> Skill {
        Skill {
            id: "custom-skill".to_string(),
            name: "Custom Skill".to_string(),
            description: "Use for custom work".to_string(),
            content: "Body".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: SkillInterfaceMetadata::default(),
            dependencies: SkillDependencies::default(),
            policy: SkillPolicy::default(),
            source_path: None,
            resources: vec![SkillResourceInfo {
                path: "references/guide.md".to_string(),
                kind: SkillResourceKind::Reference,
                bytes: 42,
            }],
            resource_bundle: vec![SkillResourceFile {
                path: "references/guide.md".to_string(),
                kind: SkillResourceKind::Reference,
                encoding: SkillResourceEncoding::Utf8,
                content: "Guide".to_string(),
            }],
        }
    }

    #[test]
    fn validates_well_formed_skill() {
        let report = validate_skill_spec(&valid_skill());

        assert!(report.valid, "{:?}", report.issues);
        assert_eq!(report.version, NEXA_SKILL_SPEC_VERSION);
    }

    #[test]
    fn rejects_invalid_skill_contract_fields() {
        let mut skill = valid_skill();
        skill.id = "bad id".to_string();
        skill.name = " ".to_string();
        skill.description = "x".repeat(MAX_SKILL_DESCRIPTION_CHARS + 1);
        skill.resources[0].path = "../secret.md".to_string();

        let report = validate_skill_spec(&skill);

        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "id.invalid_chars"));
        assert!(report.issues.iter().any(|issue| issue.code == "name.empty"));
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "description.too_long"));
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "resource.invalid_path"));
    }
}
