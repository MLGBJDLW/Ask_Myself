use serde::{Deserialize, Serialize};

use crate::llm::{ProviderType, ReasoningEffort};
use crate::provider_catalog::model_capabilities_from_catalog;

pub const NEXUS_MAX_ITERATIONS_FLOOR: u32 = 48;
pub const NEXUS_MAX_PARALLEL: u32 = 6;
pub const NEXUS_MAX_CALLS_PER_TURN: u32 = 12;
pub const NEXUS_TOKEN_BUDGET: u32 = 96_000;
/// Backward-compatible signal that enables dedicated verification/judge lanes.
/// Delegation v2 no longer freezes this percentage of the token budget.
pub const NEXUS_VERIFICATION_RESERVE_PERCENT: u32 = 25;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentPowerMode {
    #[default]
    Standard,
    Nexus,
}

impl AgentPowerMode {
    pub fn is_nexus(self) -> bool {
        self == Self::Nexus
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Nexus => "nexus",
        }
    }

    pub fn from_wire(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|text| !text.is_empty()) {
            None => Ok(Self::Standard),
            Some(value) if value.eq_ignore_ascii_case("standard") => Ok(Self::Standard),
            Some(value) if value.eq_ignore_ascii_case("default") => Ok(Self::Standard),
            Some(value) if value.eq_ignore_ascii_case("nexus") => Ok(Self::Nexus),
            Some(value) => Err(format!("Unsupported agent power mode '{value}'.")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentPowerPolicyInput<'a> {
    pub mode: AgentPowerMode,
    pub provider_type: ProviderType,
    pub model: &'a str,
    pub max_iterations: u32,
    pub reasoning_enabled: Option<bool>,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub subagent_max_parallel: Option<u32>,
    pub subagent_max_calls_per_turn: Option<u32>,
    pub subagent_token_budget: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAgentPowerPolicy {
    pub mode: AgentPowerMode,
    pub max_iterations: u32,
    pub reasoning_enabled: Option<bool>,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub subagent_max_parallel: Option<u32>,
    pub subagent_max_calls_per_turn: Option<u32>,
    pub subagent_token_budget: Option<u32>,
    pub verification_reserve_percent: Option<u32>,
    pub model_capability_resolved: bool,
}

impl ResolvedAgentPowerPolicy {
    pub fn prompt_section(&self) -> &'static str {
        if !self.mode.is_nexus() {
            return "";
        }

        "## Nexus Execution Policy\n\n\
         The user explicitly enabled Nexus mode for this turn. The parent agent is the lead and must actively orchestrate specialist subagents rather than merely doing all work itself.\n\
         - For any non-trivial task with two or more independent investigation, implementation-planning, debugging, research, or review tracks, call `spawn_subagent_batch` early with 3-6 focused subagents. This is a required Nexus behavior, not an optional suggestion.\n\
         - Give each subagent a narrow role, explicit deliverable, relevant read-only tools, and disjoint ownership. Keep final synthesis and write ownership with the parent unless parallel writes are provably isolated.\n\
         - Work in evidence-driven waves: parallel reconnaissance first, parent synthesis and implementation second, then an independent verifier or `judge_subagent_results` pass. Reuse workflow templates such as `research_verify` when they fit.\n\
         - Use the runtime's dedicated verification and judge lanes after exploration. These lanes preserve concurrency and call admission without freezing a fixed percentage of the delegated token budget.\n\
         - Prefer objective checks, primary evidence, independent reproduction, and an explicit verifier or judge over majority voting. Correlated agreement is not proof.\n\
         - The runtime has selected only reasoning controls declared by the active model catalog. Never claim an unsupported reasoning level.\n\
         - Do not fan out a truly trivial or tightly coupled one-step task. Extra agents can add latency, cost, overthinking, and conflicting edits. If you do not delegate, state internally why the task is trivial or inseparable."
    }
}

pub fn resolve_agent_power_policy(input: AgentPowerPolicyInput<'_>) -> ResolvedAgentPowerPolicy {
    if !input.mode.is_nexus() {
        return ResolvedAgentPowerPolicy {
            mode: input.mode,
            max_iterations: input.max_iterations,
            reasoning_enabled: input.reasoning_enabled,
            thinking_budget: input.thinking_budget,
            reasoning_effort: input.reasoning_effort,
            subagent_max_parallel: input.subagent_max_parallel,
            subagent_max_calls_per_turn: input.subagent_max_calls_per_turn,
            subagent_token_budget: input.subagent_token_budget,
            verification_reserve_percent: None,
            model_capability_resolved: false,
        };
    }

    let capabilities = model_capabilities_from_catalog(input.provider_type, input.model);
    let model_capability_resolved = capabilities.is_some();
    let mut reasoning_enabled = input.reasoning_enabled;
    let mut thinking_budget = input.thinking_budget;
    let mut reasoning_effort = input.reasoning_effort;

    if let Some(reasoning) = capabilities.and_then(|capabilities| capabilities.reasoning) {
        reasoning_enabled = Some(true);
        reasoning_effort = reasoning
            .effort_levels
            .iter()
            .rev()
            .find_map(|level| parse_reasoning_effort(level))
            .filter(|effort| *effort != ReasoningEffort::None);

        if let Some(budget) = reasoning.thinking_budget.filter(|budget| budget.enabled) {
            if reasoning.effort_budget_exclusive && reasoning_effort.is_some() {
                thinking_budget = None;
            } else {
                let catalog_budget = budget.max_tokens.or(budget.default_tokens);
                if let Some(catalog_budget) = catalog_budget {
                    thinking_budget = Some(
                        thinking_budget
                            .unwrap_or_default()
                            .max(catalog_budget)
                            .max(budget.min_tokens.unwrap_or_default()),
                    );
                }
            }
        } else {
            thinking_budget = None;
        }
    }

    ResolvedAgentPowerPolicy {
        mode: input.mode,
        max_iterations: input.max_iterations.max(NEXUS_MAX_ITERATIONS_FLOOR),
        reasoning_enabled,
        thinking_budget,
        reasoning_effort,
        subagent_max_parallel: Some(NEXUS_MAX_PARALLEL),
        subagent_max_calls_per_turn: Some(NEXUS_MAX_CALLS_PER_TURN),
        subagent_token_budget: Some(NEXUS_TOKEN_BUDGET),
        verification_reserve_percent: Some(NEXUS_VERIFICATION_RESERVE_PERCENT),
        model_capability_resolved,
    }
}

fn parse_reasoning_effort(value: &str) -> Option<ReasoningEffort> {
    ReasoningEffort::from_wire(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        mode: AgentPowerMode,
        provider_type: ProviderType,
        model: &str,
    ) -> AgentPowerPolicyInput<'_> {
        AgentPowerPolicyInput {
            mode,
            provider_type,
            model,
            max_iterations: 20,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: Some(ReasoningEffort::Low),
            subagent_max_parallel: Some(2),
            subagent_max_calls_per_turn: Some(3),
            subagent_token_budget: Some(12_000),
        }
    }

    #[test]
    fn wire_values_are_explicit_and_safe_by_default() {
        assert_eq!(
            AgentPowerMode::from_wire(None).unwrap(),
            AgentPowerMode::Standard
        );
        assert_eq!(
            AgentPowerMode::from_wire(Some("nexus")).unwrap(),
            AgentPowerMode::Nexus
        );
        assert!(AgentPowerMode::from_wire(Some("ultra")).is_err());
    }

    #[test]
    fn nexus_uses_the_catalogs_strongest_supported_effort() {
        let policy = resolve_agent_power_policy(input(
            AgentPowerMode::Nexus,
            ProviderType::OpenAi,
            "gpt-5.6",
        ));

        assert_eq!(policy.reasoning_effort, Some(ReasoningEffort::Max));
        assert_eq!(policy.subagent_max_parallel, Some(NEXUS_MAX_PARALLEL));
        assert_eq!(
            policy.subagent_max_calls_per_turn,
            Some(NEXUS_MAX_CALLS_PER_TURN)
        );
        assert_eq!(policy.subagent_token_budget, Some(NEXUS_TOKEN_BUDGET));
        assert_eq!(policy.verification_reserve_percent, Some(25));
        assert!(policy.model_capability_resolved);
        let prompt = policy.prompt_section();
        assert!(prompt.contains("required Nexus behavior"));
        assert!(prompt.contains("spawn_subagent_batch"));
        assert!(prompt.contains("3-6 focused subagents"));
        assert!(prompt.contains("judge_subagent_results"));
    }

    #[test]
    fn nexus_respects_exclusive_reasoning_controls() {
        let policy = resolve_agent_power_policy(input(
            AgentPowerMode::Nexus,
            ProviderType::Qwen,
            "qwen3.8-max-preview",
        ));

        assert_eq!(policy.reasoning_effort, Some(ReasoningEffort::Max));
        assert_eq!(policy.thinking_budget, None);
        assert_eq!(policy.reasoning_enabled, Some(true));
    }

    #[test]
    fn nexus_does_not_invent_effort_or_budget_for_budget_only_models() {
        let policy = resolve_agent_power_policy(input(
            AgentPowerMode::Nexus,
            ProviderType::AlibabaModelStudio,
            "kimi-k2.6",
        ));

        assert_eq!(policy.reasoning_effort, None);
        assert_eq!(policy.thinking_budget, None);
        assert_eq!(policy.reasoning_enabled, Some(true));
        assert!(policy.model_capability_resolved);
    }

    #[test]
    fn unknown_models_preserve_reasoning_controls() {
        let policy = resolve_agent_power_policy(input(
            AgentPowerMode::Nexus,
            ProviderType::OpenAi,
            "custom-unlisted-model",
        ));

        assert_eq!(policy.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(policy.reasoning_enabled, None);
        assert!(!policy.model_capability_resolved);
    }

    #[test]
    fn standard_mode_preserves_the_users_configuration() {
        let policy = resolve_agent_power_policy(input(
            AgentPowerMode::Standard,
            ProviderType::OpenAi,
            "gpt-5.6",
        ));

        assert_eq!(policy.max_iterations, 20);
        assert_eq!(policy.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(policy.subagent_max_parallel, Some(2));
        assert_eq!(policy.verification_reserve_percent, None);
        assert!(policy.prompt_section().is_empty());
    }
}
