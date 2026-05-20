use serde::{Deserialize, Serialize};

pub(crate) const OPENAI_AGENT_METADATA_PATH: &str = "agents/openai.yaml";

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
    /// Standard agent-facing interface metadata, usually loaded from
    /// `agents/openai.yaml`.
    #[serde(default)]
    pub interface: SkillInterfaceMetadata,
    /// Optional tool/runtime dependencies declared by the skill bundle.
    #[serde(default)]
    pub dependencies: SkillDependencies,
    /// Runtime activation policy for implicit discovery and prompting.
    #[serde(default)]
    pub policy: SkillPolicy,
    /// Best-effort materialized SKILL.md path. Built-ins and user skills may be
    /// `None` before the app has materialized bundles to disk.
    #[serde(default)]
    pub source_path: Option<String>,
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
    Metadata,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillInterfaceMetadata {
    #[serde(default, alias = "display_name")]
    pub display_name: String,
    #[serde(default, alias = "short_description")]
    pub short_description: String,
    #[serde(default, alias = "icon_small")]
    pub icon_small: Option<String>,
    #[serde(default, alias = "icon_large")]
    pub icon_large: Option<String>,
    #[serde(default, alias = "default_prompt")]
    pub default_prompt: Option<String>,
}

impl SkillInterfaceMetadata {
    pub(crate) fn with_defaults(mut self, name: &str, description: &str) -> Self {
        if self.display_name.trim().is_empty() {
            self.display_name = name.trim().to_string();
        }
        if self.short_description.trim().is_empty() {
            self.short_description = description.trim().to_string();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillDependencies {
    #[serde(default)]
    pub tools: Vec<SkillToolDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillToolDependency {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPolicy {
    #[serde(
        default = "default_allow_implicit_invocation",
        alias = "allow_implicit_invocation"
    )]
    pub allow_implicit_invocation: bool,
}

impl Default for SkillPolicy {
    fn default() -> Self {
        Self {
            allow_implicit_invocation: true,
        }
    }
}

fn default_allow_implicit_invocation() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillAgentMetadata {
    #[serde(default)]
    pub interface: SkillInterfaceMetadata,
    #[serde(default)]
    pub dependencies: SkillDependencies,
    #[serde(default)]
    pub policy: SkillPolicy,
}

pub(crate) fn derive_skill_metadata(
    name: &str,
    description: &str,
    resource_bundle: &[SkillResourceFile],
) -> (SkillInterfaceMetadata, SkillDependencies, SkillPolicy) {
    let parsed = resource_bundle
        .iter()
        .find(|resource| resource.path == OPENAI_AGENT_METADATA_PATH)
        .filter(|resource| matches!(resource.encoding, SkillResourceEncoding::Utf8))
        .and_then(|resource| serde_yaml::from_str::<SkillAgentMetadata>(&resource.content).ok())
        .unwrap_or_default();

    (
        parsed.interface.with_defaults(name, description),
        parsed.dependencies,
        parsed.policy,
    )
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
