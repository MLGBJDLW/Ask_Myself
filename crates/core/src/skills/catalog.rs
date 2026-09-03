use serde::{Deserialize, Serialize};

use super::model::{Skill, SkillPolicy, SkillResourceInfo, SkillToolDependency};
use super::trust_policy::{trust_state_for_skill, SkillTrustState};

pub const SKILL_CATALOG_ENVELOPE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCatalogEntry {
    pub skill_id: String,
    pub canonical_name: String,
    pub name: String,
    pub short_description: String,
    pub use_when: String,
    pub source: String,
    pub trust_state: SkillTrustState,
    pub policy: SkillPolicy,
    #[serde(default)]
    pub dependencies: Vec<SkillToolDependency>,
    #[serde(default)]
    pub resources: Vec<SkillResourceInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCatalogEnvelope {
    pub version: u16,
    pub budget_chars: usize,
    pub truncated: bool,
    pub entries: Vec<SkillCatalogEntry>,
}

pub fn build_skill_catalog_entry(skill: &Skill, worktree_trusted: bool) -> SkillCatalogEntry {
    let display_name = if skill.interface.display_name.trim().is_empty() {
        skill.name.as_str()
    } else {
        skill.interface.display_name.as_str()
    };
    let short_description = if skill.interface.short_description.trim().is_empty() {
        skill.description.as_str()
    } else {
        skill.interface.short_description.as_str()
    };
    let source = skill.source_path.as_deref().unwrap_or(if skill.builtin {
        "bundled"
    } else {
        "user-defined"
    });

    SkillCatalogEntry {
        skill_id: skill.id.clone(),
        canonical_name: skill.canonical_name.clone(),
        name: display_name.to_string(),
        short_description: truncate_one_line(short_description, 250),
        use_when: truncate_one_line(&skill.description, 250),
        source: source.to_string(),
        trust_state: trust_state_for_skill(skill, worktree_trusted),
        policy: skill.policy.clone(),
        dependencies: skill.dependencies.tools.clone(),
        resources: skill.resources.clone(),
    }
}

pub fn build_skill_catalog_envelope(
    skills: &[Skill],
    budget_chars: usize,
    worktree_trusted: bool,
) -> SkillCatalogEnvelope {
    let mut ordered_skills = skills.iter().collect::<Vec<_>>();
    ordered_skills.sort_by(|a, b| {
        a.builtin
            .cmp(&b.builtin)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });

    SkillCatalogEnvelope {
        version: SKILL_CATALOG_ENVELOPE_VERSION,
        budget_chars,
        truncated: false,
        entries: ordered_skills
            .into_iter()
            .map(|skill| build_skill_catalog_entry(skill, worktree_trusted))
            .collect(),
    }
}

pub fn render_skill_catalog_prompt_envelope(
    skills: &[Skill],
    budget_chars: usize,
    worktree_trusted: bool,
) -> String {
    let envelope = build_skill_catalog_envelope(skills, budget_chars, worktree_trusted);
    let mut rendered = format!(
        "<skill_catalog version=\"{}\" budget_chars=\"{}\">\n",
        envelope.version, envelope.budget_chars
    );

    for entry in envelope.entries {
        let next = render_catalog_entry(&entry);
        let closing_budget = "</skill_catalog>\n".len() + "<truncated />\n".len();
        if rendered.len() + next.len() + closing_budget > budget_chars {
            rendered.push_str("<truncated />\n");
            rendered.push_str("</skill_catalog>\n");
            return rendered;
        }
        rendered.push_str(&next);
    }

    rendered.push_str("</skill_catalog>\n");
    rendered
}

pub fn escape_prompt_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_catalog_entry(entry: &SkillCatalogEntry) -> String {
    let dependencies = if entry.dependencies.is_empty() {
        "none".to_string()
    } else {
        entry
            .dependencies
            .iter()
            .map(|dependency| {
                if dependency.kind.trim().is_empty() {
                    escape_prompt_xml(&dependency.value)
                } else {
                    format!(
                        "{}:{}",
                        escape_prompt_xml(&dependency.kind),
                        escape_prompt_xml(&dependency.value)
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let resources = if entry.resources.is_empty() {
        "none".to_string()
    } else {
        let mut paths = entry
            .resources
            .iter()
            .map(|resource| resource.path.as_str())
            .take(8)
            .map(escape_prompt_xml)
            .collect::<Vec<_>>();
        let extra = entry.resources.len().saturating_sub(paths.len());
        if extra > 0 {
            paths.push(format!("+{extra} more"));
        }
        paths.join(", ")
    };

    format!(
        "<skill id=\"{}\" name=\"{}\" canonical_name=\"{}\" source=\"{}\" trust_state=\"{}\" implicit=\"{}\">\n\
  <short_description>{}</short_description>\n\
  <use_when>{}</use_when>\n\
  <dependencies>{}</dependencies>\n\
  <resources>{}</resources>\n\
</skill>\n",
        escape_prompt_xml(&entry.skill_id),
        escape_prompt_xml(&entry.name),
        escape_prompt_xml(&entry.canonical_name),
        escape_prompt_xml(&entry.source),
        entry.trust_state.as_str(),
        entry.policy.allow_implicit_invocation,
        escape_prompt_xml(&entry.short_description),
        escape_prompt_xml(&entry.use_when),
        dependencies,
        resources
    )
}

fn truncate_one_line(text: &str, max_chars: usize) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    compact.chars().take(max_chars).collect::<String>() + "..."
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillDependencies, SkillInterfaceMetadata, SkillResourceEncoding, SkillResourceFile,
        SkillResourceKind,
    };

    #[test]
    fn catalog_prompt_envelope_escapes_metadata() {
        let skill = Skill {
            id: "skill-1".to_string(),
            canonical_name: "a-skill".to_string(),
            name: "A <Skill>".to_string(),
            description: "Use when A & B".to_string(),
            content: "Body should not appear".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: SkillInterfaceMetadata::default(),
            dependencies: SkillDependencies::default(),
            policy: SkillPolicy::default(),
            source_path: Some("skills/user/A & B/SKILL.md".to_string()),
            resources: vec![SkillResourceInfo {
                path: "references/a<b>.md".to_string(),
                kind: SkillResourceKind::Reference,
                bytes: 10,
            }],
            resource_bundle: vec![SkillResourceFile {
                path: "references/a<b>.md".to_string(),
                kind: SkillResourceKind::Reference,
                encoding: SkillResourceEncoding::Utf8,
                content: "secret bundled content".to_string(),
            }],
        };

        let rendered = render_skill_catalog_prompt_envelope(&[skill], 4_000, true);

        assert!(rendered.contains("<skill_catalog version=\"1\""));
        assert!(rendered.contains("name=\"A &lt;Skill&gt;\""));
        assert!(rendered.contains("Use when A &amp; B"));
        assert!(rendered.contains("references/a&lt;b&gt;.md"));
        assert!(!rendered.contains("Body should not appear"));
        assert!(!rendered.contains("secret bundled content"));
    }

    #[test]
    fn catalog_prompt_envelope_truncates_deterministically() {
        let skills = (0..10)
            .map(|index| Skill {
                id: format!("skill-{index}"),
                canonical_name: format!("skill-{index}"),
                name: format!("Skill {index}"),
                description: "Use for tests".to_string(),
                content: "Body".to_string(),
                enabled: true,
                created_at: index.to_string(),
                updated_at: String::new(),
                builtin: false,
                interface: SkillInterfaceMetadata::default(),
                dependencies: SkillDependencies::default(),
                policy: SkillPolicy::default(),
                source_path: None,
                resources: Vec::new(),
                resource_bundle: Vec::new(),
            })
            .collect::<Vec<_>>();

        let rendered = render_skill_catalog_prompt_envelope(&skills, 220, true);

        assert!(rendered.contains("<truncated />"));
        assert!(rendered.ends_with("</skill_catalog>\n"));
    }
}
