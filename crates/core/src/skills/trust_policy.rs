use serde::{Deserialize, Serialize};

use super::model::{Skill, SkillResourceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceKind {
    Builtin,
    User,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillTrustState {
    Builtin,
    TrustedUser,
    TrustedProject,
    UntrustedProject,
    Disabled,
}

impl SkillTrustState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::TrustedUser => "trusted_user",
            Self::TrustedProject => "trusted_project",
            Self::UntrustedProject => "untrusted_project",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillTrustAction {
    List,
    Activate,
    ImplicitActivate,
    ReadResource,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTrustPolicyInput {
    pub action: SkillTrustAction,
    #[serde(default)]
    pub worktree_trusted: bool,
    #[serde(default)]
    pub resource_kind: Option<SkillResourceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTrustDecision {
    pub source_kind: SkillSourceKind,
    pub trust_state: SkillTrustState,
    pub allowed: bool,
    pub requires_approval: bool,
    pub reason: String,
}

pub fn classify_skill_source(skill: &Skill) -> SkillSourceKind {
    if skill.builtin {
        return SkillSourceKind::Builtin;
    }
    if skill
        .source_path
        .as_deref()
        .is_some_and(|path| path.replace('\\', "/").contains("/project/"))
    {
        SkillSourceKind::Project
    } else {
        SkillSourceKind::User
    }
}

pub fn trust_state_for_skill(skill: &Skill, worktree_trusted: bool) -> SkillTrustState {
    if !skill.enabled {
        return SkillTrustState::Disabled;
    }
    match classify_skill_source(skill) {
        SkillSourceKind::Builtin => SkillTrustState::Builtin,
        SkillSourceKind::User => SkillTrustState::TrustedUser,
        SkillSourceKind::Project if worktree_trusted => SkillTrustState::TrustedProject,
        SkillSourceKind::Project => SkillTrustState::UntrustedProject,
    }
}

pub fn evaluate_skill_trust_policy(
    skill: &Skill,
    input: &SkillTrustPolicyInput,
) -> SkillTrustDecision {
    let source_kind = classify_skill_source(skill);
    let trust_state = trust_state_for_skill(skill, input.worktree_trusted);

    if matches!(trust_state, SkillTrustState::Disabled) {
        return SkillTrustDecision {
            source_kind,
            trust_state,
            allowed: false,
            requires_approval: false,
            reason: "disabled skills are not available to the runtime".to_string(),
        };
    }

    if matches!(input.action, SkillTrustAction::Edit) && skill.builtin {
        return SkillTrustDecision {
            source_kind,
            trust_state,
            allowed: false,
            requires_approval: false,
            reason: "builtin skills are read-only".to_string(),
        };
    }

    let untrusted_project = matches!(trust_state, SkillTrustState::UntrustedProject);
    let script_or_asset = matches!(
        input.resource_kind,
        Some(SkillResourceKind::Script | SkillResourceKind::Asset)
    );

    let requires_approval = match input.action {
        SkillTrustAction::ImplicitActivate => untrusted_project,
        SkillTrustAction::Activate => untrusted_project,
        SkillTrustAction::ReadResource => untrusted_project && script_or_asset,
        SkillTrustAction::Edit => true,
        SkillTrustAction::List => false,
    };

    SkillTrustDecision {
        source_kind,
        trust_state,
        allowed: true,
        requires_approval,
        reason: if requires_approval {
            "skill action crosses an untrusted or sensitive skill trust boundary".to_string()
        } else {
            "skill action is allowed by current trust policy".to_string()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillDependencies, SkillInterfaceMetadata, SkillPolicy, SkillResourceFile,
        SkillResourceInfo,
    };

    fn skill_with_source(source_path: Option<&str>, builtin: bool, enabled: bool) -> Skill {
        Skill {
            id: "skill-1".to_string(),
            name: "Skill".to_string(),
            description: "Use for tests".to_string(),
            content: "Body".to_string(),
            enabled,
            created_at: String::new(),
            updated_at: String::new(),
            builtin,
            interface: SkillInterfaceMetadata::default(),
            dependencies: SkillDependencies::default(),
            policy: SkillPolicy::default(),
            source_path: source_path.map(str::to_string),
            resources: Vec::<SkillResourceInfo>::new(),
            resource_bundle: Vec::<SkillResourceFile>::new(),
        }
    }

    #[test]
    fn project_skill_requires_approval_when_worktree_is_untrusted() {
        let skill = skill_with_source(Some("skills/project/local/SKILL.md"), false, true);
        let decision = evaluate_skill_trust_policy(
            &skill,
            &SkillTrustPolicyInput {
                action: SkillTrustAction::ImplicitActivate,
                worktree_trusted: false,
                resource_kind: None,
            },
        );

        assert_eq!(decision.trust_state, SkillTrustState::UntrustedProject);
        assert!(decision.allowed);
        assert!(decision.requires_approval);
    }

    #[test]
    fn disabled_skill_is_not_available() {
        let skill = skill_with_source(None, false, false);
        let decision = evaluate_skill_trust_policy(
            &skill,
            &SkillTrustPolicyInput {
                action: SkillTrustAction::List,
                worktree_trusted: true,
                resource_kind: None,
            },
        );

        assert_eq!(decision.trust_state, SkillTrustState::Disabled);
        assert!(!decision.allowed);
    }
}
