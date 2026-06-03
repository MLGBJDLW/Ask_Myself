use serde::{Deserialize, Serialize};

use super::model::{Skill, SkillResourceFile, SkillResourceInfo, SkillResourceKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillResourceSummary {
    pub path: String,
    pub kind: SkillResourceKind,
    pub bytes: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SkillResourceAccessError {
    #[error("resource path is empty")]
    EmptyPath,
    #[error("resource path must be relative to the skill directory")]
    AbsolutePath,
    #[error("resource path may not contain parent directory traversal")]
    ParentTraversal,
    #[error("resource path contains a control character")]
    ControlCharacter,
}

pub fn resource_summary_for_skill(skill: &Skill) -> Vec<SkillResourceSummary> {
    skill
        .resources
        .iter()
        .map(|resource| SkillResourceSummary {
            path: resource.path.clone(),
            kind: resource.kind.clone(),
            bytes: resource.bytes,
        })
        .collect()
}

pub fn normalize_skill_resource_path(path: &str) -> Result<String, SkillResourceAccessError> {
    let raw = path.trim().replace('\\', "/");
    if raw.is_empty() {
        return Err(SkillResourceAccessError::EmptyPath);
    }
    if raw.starts_with('/') || raw.starts_with('~') || has_windows_drive_prefix(&raw) {
        return Err(SkillResourceAccessError::AbsolutePath);
    }
    if raw.chars().any(|ch| ch.is_control()) {
        return Err(SkillResourceAccessError::ControlCharacter);
    }

    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(SkillResourceAccessError::ParentTraversal),
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        return Err(SkillResourceAccessError::EmptyPath);
    }

    Ok(parts.join("/"))
}

pub fn find_skill_resource<'a>(
    skill: &'a Skill,
    resource_path: &str,
) -> Result<Option<&'a SkillResourceFile>, SkillResourceAccessError> {
    let normalized = normalize_skill_resource_path(resource_path)?;
    Ok(skill
        .resource_bundle
        .iter()
        .find(|resource| resource.path == normalized))
}

pub fn normalize_resource_metadata(
    resources: &[SkillResourceInfo],
) -> Result<Vec<SkillResourceInfo>, SkillResourceAccessError> {
    resources
        .iter()
        .map(|resource| {
            let mut resource = resource.clone();
            resource.path = normalize_skill_resource_path(&resource.path)?;
            Ok(resource)
        })
        .collect()
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_resource_paths() {
        assert_eq!(
            normalize_skill_resource_path(r".\references\guide.md").unwrap(),
            "references/guide.md"
        );
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert_eq!(
            normalize_skill_resource_path("../secret.txt").unwrap_err(),
            SkillResourceAccessError::ParentTraversal
        );
        assert_eq!(
            normalize_skill_resource_path("C:/Users/test/secret.txt").unwrap_err(),
            SkillResourceAccessError::AbsolutePath
        );
        assert_eq!(
            normalize_skill_resource_path("/tmp/secret.txt").unwrap_err(),
            SkillResourceAccessError::AbsolutePath
        );
    }
}
