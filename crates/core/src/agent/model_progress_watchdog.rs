//! Lightweight liveness policy around model execution.
//!
//! Transport activity is authoritative for stream liveness. This module only
//! owns the deadlines that cannot be expressed by the transport adapter: the
//! initial connection deadline and the absolute cap for provider-hosted tools
//! that may already have side effects. Reasoning, answer, and tool-argument
//! deltas are deliberately not assigned semantic milestone deadlines.

use std::time::Duration;

use tokio::time::Instant;

use super::route::AgentRouteKind;
use crate::llm::{ProviderType, ReasoningEffort};
use crate::provider_catalog::model_capabilities_from_catalog;

const LONG_REASONER_CONNECT_DEADLINE: Duration = Duration::from_secs(90);
const DEFAULT_CONNECT_DEADLINE: Duration = Duration::from_secs(180);
const LOCAL_MODEL_CONNECT_DEADLINE: Duration = Duration::from_secs(300);
const HOSTED_TOOL_IDLE_DEADLINE: Duration = Duration::from_secs(180);
const HOSTED_TOOL_HARD_DEADLINE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelProgressPolicy {
    pub(super) hosted_tool_idle_deadline: Duration,
    pub(super) connect_deadline: Duration,
}

impl ModelProgressPolicy {
    pub(super) fn for_model(
        provider: Option<ProviderType>,
        model: &str,
        _route: AgentRouteKind,
        _has_executable_tools: bool,
    ) -> Self {
        let normalized = model.trim().to_ascii_lowercase();
        let catalog_limits = provider.and_then(|provider| {
            crate::provider_catalog::model_limits_from_catalog(provider, model)
        });
        let catalog_reasoning = provider
            .and_then(|provider| model_capabilities_from_catalog(provider, model))
            .and_then(|capabilities| capabilities.reasoning);
        let capability_long_reasoner = catalog_limits.as_ref().is_some_and(|limits| {
            limits
                .context_tokens
                .is_some_and(|tokens| tokens >= 500_000)
                || limits
                    .max_output_tokens
                    .is_some_and(|tokens| tokens >= 65_536)
        }) || catalog_reasoning.as_ref().is_some_and(|reasoning| {
            matches!(reasoning.default_effort.as_deref(), Some("xhigh" | "max"))
                || reasoning
                    .thinking_budget
                    .as_ref()
                    .and_then(|budget| budget.max_tokens)
                    .is_some_and(|tokens| tokens >= 65_536)
        });
        let long_reasoner = capability_long_reasoner
            || normalized.contains("kimi-k3")
            || normalized.contains("qwen3.8-max")
            || normalized.contains("qwen3.8_max");

        Self {
            hosted_tool_idle_deadline: HOSTED_TOOL_IDLE_DEADLINE,
            connect_deadline: if matches!(
                provider,
                Some(ProviderType::Ollama | ProviderType::LmStudio)
            ) {
                LOCAL_MODEL_CONNECT_DEADLINE
            } else if long_reasoner {
                LONG_REASONER_CONNECT_DEADLINE
            } else {
                DEFAULT_CONNECT_DEADLINE
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelProgressRecoveryControls {
    pub(super) reasoning_enabled: Option<bool>,
    pub(super) reasoning_effort: Option<ReasoningEffort>,
    pub(super) thinking_budget: Option<u32>,
    pub(super) description: &'static str,
}

/// Request-side controls used by explicit user recovery and replay-safety
/// recovery. They are never applied merely because an active stream is slow.
pub(super) fn recovery_controls(
    provider: Option<ProviderType>,
    model: &str,
) -> ModelProgressRecoveryControls {
    let Some(reasoning) = provider
        .and_then(|provider| model_capabilities_from_catalog(provider, model))
        .and_then(|capabilities| capabilities.reasoning)
    else {
        return ModelProgressRecoveryControls {
            reasoning_enabled: Some(false),
            reasoning_effort: None,
            thinking_budget: None,
            description: "reasoning disabled",
        };
    };
    let always_on = reasoning.mode.as_deref() == Some("always");
    if !always_on {
        let disabled_effort = (provider == Some(ProviderType::OpenRouter)
            && reasoning.effort_levels.iter().any(|level| level == "none"))
        .then_some(ReasoningEffort::None);
        return ModelProgressRecoveryControls {
            reasoning_enabled: Some(false),
            reasoning_effort: disabled_effort,
            thinking_budget: reasoning
                .thinking_budget
                .filter(|budget| budget.enabled && budget.allow_zero == Some(true))
                .map(|_| 0),
            description: "reasoning disabled",
        };
    }
    let effort = reasoning
        .effort_levels
        .iter()
        .filter_map(|effort| ReasoningEffort::from_wire(effort))
        .find(|effort| *effort != ReasoningEffort::None);
    ModelProgressRecoveryControls {
        reasoning_enabled: Some(true),
        reasoning_effort: effort.clone(),
        thinking_budget: None,
        description: if matches!(
            effort,
            Some(ReasoningEffort::Minimal | ReasoningEffort::Low)
        ) {
            "reasoning reduced to the lowest supported effort"
        } else {
            "always-on reasoning reduced to the lowest supported effort"
        },
    }
}

/// Keep DeepSeek's same-turn reasoning replay valid when continuing a real
/// output limit. Disabling it and later enabling it again invalidates the
/// native tool history and makes the model repeat already completed work.
pub(super) fn continuation_controls(
    provider: Option<ProviderType>,
    model: &str,
) -> ModelProgressRecoveryControls {
    if provider == Some(ProviderType::DeepSeek)
        && model_capabilities_from_catalog(ProviderType::DeepSeek, model)
            .and_then(|capabilities| capabilities.reasoning)
            .is_some_and(|reasoning| reasoning.effort_levels.iter().any(|effort| effort == "low"))
    {
        return ModelProgressRecoveryControls {
            reasoning_enabled: Some(true),
            reasoning_effort: Some(ReasoningEffort::Low),
            thinking_budget: None,
            description: "reasoning reduced while preserving native continuation state",
        };
    }
    recovery_controls(provider, model)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelProgressDeadlineAction {
    StopConnecting,
    StopHostedTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelProgressPhase {
    Connecting,
    Active,
    HostedTool,
    Complete,
}

pub(super) struct ModelProgressWatchdog {
    policy: ModelProgressPolicy,
    deadline: Option<Instant>,
    phase: ModelProgressPhase,
    hosted_tool_hard_deadline: Option<Instant>,
}

impl ModelProgressWatchdog {
    pub(super) fn new(
        provider: Option<ProviderType>,
        model: &str,
        route: AgentRouteKind,
        has_executable_tools: bool,
    ) -> Self {
        Self::with_policy(ModelProgressPolicy::for_model(
            provider,
            model,
            route,
            has_executable_tools,
        ))
    }

    fn with_policy(policy: ModelProgressPolicy) -> Self {
        let started_at = Instant::now();
        Self {
            policy,
            deadline: Some(started_at + policy.connect_deadline),
            phase: ModelProgressPhase::Connecting,
            hosted_tool_hard_deadline: None,
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(super) fn arm(&mut self) {
        if self.phase == ModelProgressPhase::Connecting {
            self.phase = ModelProgressPhase::Active;
            self.deadline = None;
        }
    }

    pub(super) fn reset_for_new_attempt(&mut self) {
        self.phase = ModelProgressPhase::Connecting;
        self.deadline = Some(Instant::now() + self.policy.connect_deadline);
        self.hosted_tool_hard_deadline = None;
    }

    pub(super) fn reset_for_context_retry(&mut self) {
        self.reset_for_new_attempt();
    }

    pub(super) fn observe_answer_progress(&mut self) {
        self.observe_stream_activity();
    }

    pub(super) fn observe_tool_call_progress(&mut self) {
        self.observe_stream_activity();
    }

    fn observe_stream_activity(&mut self) {
        if self.phase != ModelProgressPhase::Complete {
            self.phase = ModelProgressPhase::Active;
            self.deadline = None;
            self.hosted_tool_hard_deadline = None;
        }
    }

    pub(super) fn observe_hosted_tool_progress(&mut self) {
        let now = Instant::now();
        let hard_deadline = *self
            .hosted_tool_hard_deadline
            .get_or_insert(now + HOSTED_TOOL_HARD_DEADLINE);
        self.phase = ModelProgressPhase::HostedTool;
        self.deadline = Some((now + self.policy.hosted_tool_idle_deadline).min(hard_deadline));
    }

    pub(super) fn complete(&mut self) {
        self.phase = ModelProgressPhase::Complete;
        self.deadline = None;
        self.hosted_tool_hard_deadline = None;
    }

    pub(super) fn on_deadline(&self) -> ModelProgressDeadlineAction {
        match self.phase {
            ModelProgressPhase::Connecting => ModelProgressDeadlineAction::StopConnecting,
            ModelProgressPhase::HostedTool => ModelProgressDeadlineAction::StopHostedTool,
            ModelProgressPhase::Active | ModelProgressPhase::Complete => {
                unreachable!("active model streams have no semantic progress deadline")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ModelProgressPolicy {
        ModelProgressPolicy {
            hosted_tool_idle_deadline: Duration::from_secs(4),
            connect_deadline: Duration::from_secs(3),
        }
    }

    #[test]
    fn provider_profile_changes_only_real_connection_deadlines() {
        let long_reasoner = ModelProgressPolicy::for_model(
            Some(ProviderType::Moonshot),
            "kimi-k3",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        let ordinary = ModelProgressPolicy::for_model(
            Some(ProviderType::OpenAi),
            "ordinary-model",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        let local = ModelProgressPolicy::for_model(
            Some(ProviderType::Ollama),
            "qwen3.8-max",
            AgentRouteKind::CodebaseOperation,
            true,
        );

        assert_eq!(long_reasoner.connect_deadline, Duration::from_secs(90));
        assert_eq!(ordinary.connect_deadline, Duration::from_secs(180));
        assert_eq!(local.connect_deadline, Duration::from_secs(300));
    }

    #[tokio::test(start_paused = true)]
    async fn stream_open_removes_semantic_progress_deadlines() {
        let mut watchdog = ModelProgressWatchdog::with_policy(policy());
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(3)
        );
        watchdog.arm();
        assert_eq!(watchdog.deadline(), None);

        tokio::time::advance(Duration::from_secs(600)).await;
        watchdog.observe_tool_call_progress();
        watchdog.observe_answer_progress();
        assert_eq!(watchdog.deadline(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn hosted_tool_heartbeats_cannot_extend_the_absolute_side_effect_deadline() {
        let mut watchdog = ModelProgressWatchdog::with_policy(policy());
        watchdog.arm();
        watchdog.observe_hosted_tool_progress();
        tokio::time::advance(Duration::from_secs(599)).await;
        watchdog.observe_hosted_tool_progress();
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(1)
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::StopHostedTool
        );
    }

    #[test]
    fn recovery_controls_are_request_side_only() {
        let qwen = recovery_controls(Some(ProviderType::Qwen), "qwen3.8-max");
        assert_eq!(qwen.reasoning_enabled, Some(false));
        let kimi = recovery_controls(Some(ProviderType::Moonshot), "kimi-k3");
        assert_eq!(kimi.reasoning_enabled, Some(true));
        assert_eq!(kimi.reasoning_effort, Some(ReasoningEffort::Low));
    }
}
