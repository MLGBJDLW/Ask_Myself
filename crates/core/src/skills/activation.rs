use serde::{Deserialize, Serialize};

use super::model::{Skill, SkillPolicy, SkillResourceInfo};
use super::resource_access::resource_summary_for_skill;
use super::trust_policy::{trust_state_for_skill, SkillTrustState};

pub const SKILL_ACTIVATION_ENVELOPE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillActivationEnvelope {
    pub version: u16,
    pub skill_id: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub directory: Option<String>,
    pub trust_state: SkillTrustState,
    pub policy: SkillPolicy,
    pub body: String,
    #[serde(default)]
    pub resource_summary: Vec<SkillResourceInfo>,
    #[serde(default)]
    pub activation_reason: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
}

pub fn build_skill_activation_envelope(
    skill: &Skill,
    activation_reason: Option<&str>,
    turn_id: Option<&str>,
    worktree_trusted: bool,
) -> SkillActivationEnvelope {
    let source = skill.source_path.as_deref().unwrap_or(if skill.builtin {
        "bundled"
    } else {
        "user-defined"
    });

    SkillActivationEnvelope {
        version: SKILL_ACTIVATION_ENVELOPE_VERSION,
        skill_id: skill.id.clone(),
        name: skill.name.clone(),
        source: source.to_string(),
        directory: skill_directory(source),
        trust_state: trust_state_for_skill(skill, worktree_trusted),
        policy: skill.policy.clone(),
        body: skill.content.clone(),
        resource_summary: resource_summary_for_skill(skill)
            .into_iter()
            .map(|summary| SkillResourceInfo {
                path: summary.path,
                kind: summary.kind,
                bytes: summary.bytes,
            })
            .collect(),
        activation_reason: activation_reason.map(str::to_string),
        turn_id: turn_id.map(str::to_string),
    }
}

fn skill_directory(source: &str) -> Option<String> {
    let normalized = source.replace('\\', "/");
    normalized
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .filter(|dir| !dir.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillDependencies, SkillInterfaceMetadata, SkillResourceFile, SkillResourceKind,
    };

    #[test]
    fn activation_envelope_contains_body_without_resource_content() {
        let skill = Skill {
            id: "skill-1".to_string(),
            name: "Skill".to_string(),
            description: "Use for tests".to_string(),
            content: "Full body".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: SkillInterfaceMetadata::default(),
            dependencies: SkillDependencies::default(),
            policy: SkillPolicy::default(),
            source_path: Some("skills/user/skill-1/SKILL.md".to_string()),
            resources: vec![SkillResourceInfo {
                path: "references/guide.md".to_string(),
                kind: SkillResourceKind::Reference,
                bytes: 10,
            }],
            resource_bundle: vec![SkillResourceFile {
                path: "references/guide.md".to_string(),
                kind: SkillResourceKind::Reference,
                encoding: crate::skills::SkillResourceEncoding::Utf8,
                content: "resource content".to_string(),
            }],
        };

        let envelope = build_skill_activation_envelope(
            &skill,
            Some("matched user request"),
            Some("turn-1"),
            true,
        );

        assert_eq!(envelope.version, SKILL_ACTIVATION_ENVELOPE_VERSION);
        assert_eq!(envelope.skill_id, "skill-1");
        assert_eq!(envelope.body, "Full body");
        assert_eq!(envelope.resource_summary.len(), 1);
        assert_eq!(envelope.resource_summary[0].path, "references/guide.md");
        assert_eq!(
            envelope.activation_reason.as_deref(),
            Some("matched user request")
        );
    }
}
