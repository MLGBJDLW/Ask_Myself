use serde::{Deserialize, Serialize};

/// A skill (instruction snippet) — either a built-in (bundled SKILL.md) or a
/// user-created record in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    /// Concise trigger-match description (when to activate this skill).
    #[serde(default)]
    pub description: String,
    pub content: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    /// True when the skill originates from a bundled SKILL.md file. Built-in
    /// skills are read-only in the UI.
    #[serde(default)]
    pub builtin: bool,
    /// File-level metadata for bundled resources. Only metadata is serialized
    /// to the frontend; the full resource content stays server-side.
    #[serde(default)]
    pub resources: Vec<SkillResourceInfo>,
    #[serde(skip)]
    pub resource_bundle: Vec<SkillResourceFile>,
}

/// Input for creating or updating a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSkillInput {
    /// `None` = create new, `Some` = update existing.
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub content: String,
    pub enabled: bool,
    #[serde(default)]
    pub resource_bundle: Vec<SkillResourceFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillResourceKind {
    Script,
    Reference,
    Asset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillResourceEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillResourceInfo {
    pub path: String,
    pub kind: SkillResourceKind,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillResourceFile {
    pub path: String,
    pub kind: SkillResourceKind,
    pub encoding: SkillResourceEncoding,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkillBundle {
    pub skill_file: String,
    pub skill_dir: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub resources: Vec<SkillResourceInfo>,
    #[serde(default)]
    pub warnings: Vec<SkillWarning>,
}

/// Parsed YAML frontmatter of a SKILL.md file.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Severity of a [`SkillWarning`].
///
/// * `Info` — purely informational (e.g. missing optional frontmatter field).
/// * `Warn` — suspicious but legal (large file, risky pattern).
/// * `Block` — dangerous pattern that strongly suggests malicious import.
///
/// Scanning never refuses the import on its own; the UI decides based on these
/// severity levels whether to surface a confirmation dialog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillWarningSeverity {
    Info,
    Warn,
    Block,
}

/// A single finding produced by [`scan_skill_content`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillWarning {
    pub severity: SkillWarningSeverity,
    /// Stable machine-readable identifier (suitable for i18n lookup).
    pub code: String,
    /// Human-readable English message — UI may translate via `code`.
    pub message: String,
}

impl SkillWarning {
    pub(crate) fn new(
        severity: SkillWarningSeverity,
        code: &str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.to_string(),
            message: message.into(),
        }
    }
}
