//! Declarative policy and lifecycle hook primitives for agent runs.

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::tools::{ToolAccessProfile, ToolInvocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    RequireApproval,
    Deny,
}

impl PolicyEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::RequireApproval => "require_approval",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_prefix: Option<String>,
    pub effect: PolicyEffect,
    pub reason: String,
}

impl PolicyRule {
    pub fn matches(&self, subject: &PolicySubject<'_>) -> bool {
        if let Some(tool_name) = &self.tool_name {
            if tool_name != subject.tool_name {
                return false;
            }
        }
        if let Some(category) = &self.category {
            if category != subject.access_profile.category.as_str() {
                return false;
            }
        }
        if let Some(prefix) = &self.resource_prefix {
            if !subject
                .resource_keys
                .iter()
                .any(|resource| resource.starts_with(prefix))
            {
                return false;
            }
        }
        true
    }
}

pub struct PolicySubject<'a> {
    pub tool_name: &'a str,
    pub access_profile: &'a ToolAccessProfile,
    pub resource_keys: &'a [String],
}

impl<'a> PolicySubject<'a> {
    pub fn from_invocation(invocation: &'a ToolInvocation) -> Self {
        Self {
            tool_name: &invocation.tool_name,
            access_profile: &invocation.access_profile,
            resource_keys: &invocation.capabilities.resource_keys,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDecision {
    pub effect: PolicyEffect,
    pub needs_approval: bool,
    pub denied: bool,
    pub risk_level: ApprovalRisk,
    pub reasons: Vec<String>,
    pub matched_rule_ids: Vec<String>,
}

pub fn evaluate_policy(rules: &[PolicyRule], subject: &PolicySubject<'_>) -> PolicyDecision {
    let baseline = if subject.access_profile.needs_approval {
        PolicyEffect::RequireApproval
    } else {
        PolicyEffect::Allow
    };
    evaluate_policy_with_baseline(rules, subject, baseline)
}

pub fn evaluate_policy_with_baseline(
    rules: &[PolicyRule],
    subject: &PolicySubject<'_>,
    baseline: PolicyEffect,
) -> PolicyDecision {
    let mut effect = baseline;
    let mut reasons = vec![subject.access_profile.risk_reason.clone()];
    let mut matched_rule_ids = Vec::new();

    for rule in rules.iter().filter(|rule| rule.matches(subject)) {
        matched_rule_ids.push(rule.id.clone());
        reasons.push(rule.reason.clone());
        match rule.effect {
            PolicyEffect::Deny => {
                effect = PolicyEffect::Deny;
                break;
            }
            PolicyEffect::RequireApproval => {
                if effect != PolicyEffect::Deny {
                    effect = PolicyEffect::RequireApproval;
                }
            }
            PolicyEffect::Allow => {
                if effect == PolicyEffect::Allow {
                    effect = PolicyEffect::Allow;
                }
            }
        }
    }

    PolicyDecision {
        effect,
        needs_approval: effect == PolicyEffect::RequireApproval,
        denied: effect == PolicyEffect::Deny,
        risk_level: subject.access_profile.risk_level,
        reasons,
        matched_rule_ids,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleHookPoint {
    BeforeToolCall,
    AfterToolCall,
    BeforeModelRequest,
    AfterModelResponse,
    BeforeContextPack,
    BeforeCompaction,
    AfterCompaction,
    OnTaskEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleHookEvent {
    pub point: LifecycleHookPoint,
    pub label: String,
    pub payload: serde_json::Value,
}

impl LifecycleHookEvent {
    pub fn new(
        point: LifecycleHookPoint,
        label: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            point,
            label: label.into(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalRisk;

    fn profile(needs_approval: bool) -> ToolAccessProfile {
        ToolAccessProfile {
            category: "filesystem".to_string(),
            can_read: true,
            can_write: true,
            can_execute: false,
            can_access_network: false,
            needs_approval,
            risk_level: ApprovalRisk::High,
            risk_reason: "writes files".to_string(),
        }
    }

    #[test]
    fn deny_rule_takes_precedence_over_approval() {
        let resources = vec!["file:C:/Users/me/.ssh/config".to_string()];
        let subject = PolicySubject {
            tool_name: "edit_file",
            access_profile: &profile(true),
            resource_keys: &resources,
        };
        let rules = vec![
            PolicyRule {
                id: "review-file-writes".to_string(),
                tool_name: Some("edit_file".to_string()),
                category: None,
                resource_prefix: Some("file:".to_string()),
                effect: PolicyEffect::RequireApproval,
                reason: "review file writes".to_string(),
            },
            PolicyRule {
                id: "deny-ssh".to_string(),
                tool_name: None,
                category: None,
                resource_prefix: Some("file:C:/Users/me/.ssh".to_string()),
                effect: PolicyEffect::Deny,
                reason: "protect ssh config".to_string(),
            },
        ];

        let decision = evaluate_policy(&rules, &subject);

        assert!(decision.denied);
        assert_eq!(decision.effect, PolicyEffect::Deny);
        assert_eq!(
            decision.matched_rule_ids,
            vec!["review-file-writes", "deny-ssh"]
        );
    }

    #[test]
    fn explicit_baseline_can_allow_high_risk_tool_without_prompt() {
        let resources = vec!["process:git".to_string()];
        let profile = ToolAccessProfile {
            category: "system".to_string(),
            can_read: true,
            can_write: true,
            can_execute: true,
            can_access_network: true,
            needs_approval: true,
            risk_level: ApprovalRisk::High,
            risk_reason: "executes commands".to_string(),
        };
        let subject = PolicySubject {
            tool_name: "run_shell",
            access_profile: &profile,
            resource_keys: &resources,
        };

        let decision = evaluate_policy_with_baseline(&[], &subject, PolicyEffect::Allow);

        assert_eq!(decision.effect, PolicyEffect::Allow);
        assert!(!decision.needs_approval);
        assert!(!decision.denied);
    }

    #[test]
    fn hook_event_preserves_payload() {
        let event = LifecycleHookEvent::new(
            LifecycleHookPoint::BeforeToolCall,
            "edit_file",
            serde_json::json!({ "path": "notes.md" }),
        );

        assert_eq!(event.point, LifecycleHookPoint::BeforeToolCall);
        assert_eq!(event.payload["path"], "notes.md");
    }
}
