//! Progress watchdog for model planning before the first answer/tool action.
//!
//! Provider stream activity is not the same as task progress. Long-reasoning
//! models can emit thinking tokens indefinitely, keeping transport idle
//! deadlines alive without producing an answer or tool call. This watchdog
//! owns a separate semantic deadline and one bounded tool-first recovery.

use std::time::Duration;

use tokio::time::Instant;

use super::route::AgentRouteKind;
use crate::llm::{ProviderType, ReasoningEffort};
use crate::provider_catalog::model_capabilities_from_catalog;

const DEFAULT_SOFT_WARNING: Duration = Duration::from_secs(45);
const LONG_REASONER_CONNECT_DEADLINE: Duration = Duration::from_secs(90);
const DEFAULT_CONNECT_DEADLINE: Duration = Duration::from_secs(180);
const LOCAL_MODEL_CONNECT_DEADLINE: Duration = Duration::from_secs(300);
const DEFAULT_TOOL_FIRST_DEADLINE: Duration = Duration::from_secs(120);
const DEFAULT_DIRECT_DEADLINE: Duration = Duration::from_secs(180);
const LONG_REASONER_SOFT_WARNING: Duration = Duration::from_secs(30);
const LONG_REASONER_TOOL_FIRST_DEADLINE: Duration = Duration::from_secs(90);
const LONG_REASONER_DIRECT_DEADLINE: Duration = Duration::from_secs(150);
const RECOVERY_DEADLINE: Duration = Duration::from_secs(60);
const TOOL_ASSEMBLY_DEADLINE: Duration = Duration::from_secs(60);
const ANSWER_STREAM_DEADLINE: Duration = Duration::from_secs(180);
const HOSTED_TOOL_DEADLINE: Duration = Duration::from_secs(180);
const HOSTED_TOOL_HARD_DEADLINE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModelProgressPolicy {
    pub(super) soft_warning_after: Duration,
    pub(super) first_progress_deadline: Duration,
    pub(super) recovery_deadline: Duration,
    pub(super) tool_assembly_deadline: Duration,
    pub(super) answer_stream_deadline: Duration,
    pub(super) hosted_tool_deadline: Duration,
    pub(super) connect_deadline: Duration,
    pub(super) requires_tool_action: bool,
}

impl ModelProgressPolicy {
    pub(super) fn for_model(
        provider: Option<ProviderType>,
        model: &str,
        route: AgentRouteKind,
        has_executable_tools: bool,
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
        let tool_first_route = has_executable_tools
            && !matches!(
                route,
                AgentRouteKind::DirectResponse | AgentRouteKind::ConversationRecall
            );
        Self {
            soft_warning_after: if long_reasoner {
                LONG_REASONER_SOFT_WARNING
            } else {
                DEFAULT_SOFT_WARNING
            },
            first_progress_deadline: match (long_reasoner, tool_first_route) {
                (true, true) => LONG_REASONER_TOOL_FIRST_DEADLINE,
                (true, false) => LONG_REASONER_DIRECT_DEADLINE,
                (false, true) => DEFAULT_TOOL_FIRST_DEADLINE,
                (false, false) => DEFAULT_DIRECT_DEADLINE,
            },
            recovery_deadline: RECOVERY_DEADLINE,
            tool_assembly_deadline: TOOL_ASSEMBLY_DEADLINE,
            answer_stream_deadline: ANSWER_STREAM_DEADLINE,
            hosted_tool_deadline: HOSTED_TOOL_DEADLINE,
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
            requires_tool_action: tool_first_route,
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
            "always-on reasoning bounded by the recovery deadline"
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelProgressDeadlineAction {
    StopConnecting,
    RestartWithoutReasoning,
    StopBeforeAction,
    StopAfterVisibleOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelProgressPhase {
    Connecting,
    Planning,
    ToolAssembly,
    AnswerStreaming,
    HostedTool,
    Complete,
}

pub(super) struct ModelProgressWatchdog {
    policy: ModelProgressPolicy,
    started_at: Instant,
    deadline: Instant,
    phase: ModelProgressPhase,
    warning_emitted: bool,
    recovery_used: bool,
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
            started_at,
            deadline: started_at + policy.connect_deadline,
            phase: ModelProgressPhase::Connecting,
            warning_emitted: false,
            recovery_used: false,
            hosted_tool_hard_deadline: None,
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        (self.phase != ModelProgressPhase::Complete).then_some(self.deadline)
    }

    pub(super) fn arm(&mut self) {
        if self.phase == ModelProgressPhase::Connecting {
            self.phase = ModelProgressPhase::Planning;
            self.started_at = Instant::now();
            self.deadline = self.started_at
                + if self.recovery_used {
                    self.policy.recovery_deadline
                } else {
                    self.policy.first_progress_deadline
                };
        }
    }

    pub(super) fn reset_for_context_retry(&mut self) {
        self.phase = ModelProgressPhase::Connecting;
        self.started_at = Instant::now();
        self.deadline = self.started_at + self.policy.connect_deadline;
        self.hosted_tool_hard_deadline = None;
    }

    pub(super) fn observe_thinking(&mut self) -> bool {
        if self.phase != ModelProgressPhase::Planning
            || self.warning_emitted
            || Instant::now().duration_since(self.started_at) < self.policy.soft_warning_after
        {
            return false;
        }
        self.warning_emitted = true;
        true
    }

    pub(super) fn observe_answer_progress(&mut self) {
        if self.policy.requires_tool_action
            && matches!(
                self.phase,
                ModelProgressPhase::Planning | ModelProgressPhase::ToolAssembly
            )
        {
            // Tool-required turns may stream plans or misplaced reasoning in
            // the answer channel. Visible bytes do not satisfy first-action.
            return;
        }
        // Visible answer deltas are useful progress. Bound silence between
        // deltas instead of truncating a long response that is still moving.
        self.phase = ModelProgressPhase::AnswerStreaming;
        self.deadline = Instant::now() + self.policy.answer_stream_deadline;
    }

    pub(super) fn observe_tool_call_progress(&mut self) {
        if self.phase != ModelProgressPhase::ToolAssembly {
            self.phase = ModelProgressPhase::ToolAssembly;
            self.deadline = Instant::now() + self.policy.tool_assembly_deadline;
        }
    }

    pub(super) fn observe_hosted_tool_progress(&mut self) {
        let now = Instant::now();
        let hard_deadline = *self
            .hosted_tool_hard_deadline
            .get_or_insert(now + HOSTED_TOOL_HARD_DEADLINE);
        self.phase = ModelProgressPhase::HostedTool;
        self.deadline = (now + self.policy.hosted_tool_deadline).min(hard_deadline);
    }

    pub(super) fn complete(&mut self) {
        self.phase = ModelProgressPhase::Complete;
        self.hosted_tool_hard_deadline = None;
    }

    pub(super) fn on_deadline(&mut self) -> ModelProgressDeadlineAction {
        if self.phase == ModelProgressPhase::Connecting {
            return ModelProgressDeadlineAction::StopConnecting;
        }
        if matches!(
            self.phase,
            ModelProgressPhase::AnswerStreaming | ModelProgressPhase::HostedTool
        ) {
            return ModelProgressDeadlineAction::StopAfterVisibleOutput;
        }
        if self.recovery_used {
            return ModelProgressDeadlineAction::StopBeforeAction;
        }
        self.recovery_used = true;
        self.phase = ModelProgressPhase::Planning;
        self.warning_emitted = true;
        self.started_at = Instant::now();
        self.deadline = self.started_at + self.policy.recovery_deadline;
        ModelProgressDeadlineAction::RestartWithoutReasoning
    }
}

pub(super) const TOOL_PROGRESS_RECOVERY_PROMPT: &str = "## Model Progress Recovery\nThe previous sample spent its planning deadline emitting reasoning without producing an answer or tool call. Reasoning has been disabled or reduced to the lowest level this model supports. Do not continue private analysis. If the task needs evidence or action, emit the single best next tool call immediately. Otherwise provide a concise final answer now.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_reasoning_models_get_a_shorter_first_progress_deadline() {
        let kimi = ModelProgressPolicy::for_model(
            Some(ProviderType::Moonshot),
            "kimi-k3",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        let qwen = ModelProgressPolicy::for_model(
            Some(ProviderType::Qwen),
            "qwen3.8-max",
            AgentRouteKind::KnowledgeRetrieval,
            true,
        );
        let ordinary = ModelProgressPolicy::for_model(
            Some(ProviderType::OpenAi),
            "ordinary-model",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        assert_eq!(kimi.first_progress_deadline, Duration::from_secs(90));
        assert_eq!(qwen.first_progress_deadline, Duration::from_secs(90));
        assert_eq!(ordinary.first_progress_deadline, Duration::from_secs(120));
        assert_eq!(kimi.connect_deadline, Duration::from_secs(90));
        let local = ModelProgressPolicy::for_model(
            Some(ProviderType::Ollama),
            "qwen3.8-max",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        assert_eq!(local.connect_deadline, Duration::from_secs(300));
        let no_tools = ModelProgressPolicy::for_model(
            Some(ProviderType::Qwen),
            "qwen3.8-max",
            AgentRouteKind::CodebaseOperation,
            false,
        );
        assert_eq!(no_tools.first_progress_deadline, Duration::from_secs(150));
    }

    #[tokio::test(start_paused = true)]
    async fn connecting_has_its_own_deadline_and_stream_open_arms_planning() {
        let policy = ModelProgressPolicy {
            soft_warning_after: Duration::from_secs(1),
            first_progress_deadline: Duration::from_secs(5),
            recovery_deadline: Duration::from_secs(3),
            tool_assembly_deadline: Duration::from_secs(2),
            answer_stream_deadline: Duration::from_secs(4),
            hosted_tool_deadline: Duration::from_secs(4),
            connect_deadline: Duration::from_secs(2),
            requires_tool_action: false,
        };
        let mut watchdog = ModelProgressWatchdog::with_policy(policy);
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(2)
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::StopConnecting
        );

        let mut opened = ModelProgressWatchdog::with_policy(policy);
        opened.arm();
        assert_eq!(
            opened.deadline().unwrap() - Instant::now(),
            Duration::from_secs(5)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn thinking_does_not_count_as_productive_progress() {
        let policy = ModelProgressPolicy {
            soft_warning_after: Duration::from_secs(2),
            first_progress_deadline: Duration::from_secs(5),
            recovery_deadline: Duration::from_secs(3),
            tool_assembly_deadline: Duration::from_secs(2),
            answer_stream_deadline: Duration::from_secs(4),
            hosted_tool_deadline: Duration::from_secs(4),
            connect_deadline: Duration::from_secs(2),
            requires_tool_action: false,
        };
        let mut watchdog = ModelProgressWatchdog::with_policy(policy);
        watchdog.arm();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(watchdog.observe_thinking());
        assert!(watchdog.deadline().is_some());
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::RestartWithoutReasoning
        );
        tokio::time::advance(Duration::from_secs(3)).await;
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::StopBeforeAction
        );
    }

    #[tokio::test(start_paused = true)]
    async fn answer_channel_planning_does_not_bypass_a_required_first_tool_action() {
        let mut watchdog = ModelProgressWatchdog::new(
            Some(ProviderType::Qwen),
            "qwen3.8-max",
            AgentRouteKind::CodebaseOperation,
            true,
        );
        watchdog.arm();
        let first_action_deadline = watchdog.deadline();
        tokio::time::advance(Duration::from_secs(20)).await;
        watchdog.observe_answer_progress();

        assert_eq!(watchdog.deadline(), first_action_deadline);
        tokio::time::advance(Duration::from_secs(70)).await;
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::RestartWithoutReasoning
        );
    }

    #[tokio::test(start_paused = true)]
    async fn answer_and_partial_tool_calls_get_bounded_phase_deadlines() {
        let policy = ModelProgressPolicy {
            soft_warning_after: Duration::from_secs(1),
            first_progress_deadline: Duration::from_secs(2),
            recovery_deadline: Duration::from_secs(1),
            tool_assembly_deadline: Duration::from_secs(3),
            answer_stream_deadline: Duration::from_secs(4),
            hosted_tool_deadline: Duration::from_secs(5),
            connect_deadline: Duration::from_secs(1),
            requires_tool_action: false,
        };
        let mut watchdog = ModelProgressWatchdog::with_policy(policy);
        watchdog.arm();
        watchdog.observe_tool_call_progress();
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(3)
        );
        watchdog.observe_answer_progress();
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(4)
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        watchdog.observe_answer_progress();
        assert_eq!(
            watchdog.deadline().unwrap() - Instant::now(),
            Duration::from_secs(4)
        );
        assert_eq!(
            watchdog.on_deadline(),
            ModelProgressDeadlineAction::StopAfterVisibleOutput
        );
        watchdog.complete();
        assert_eq!(watchdog.deadline(), None);
        assert!(!watchdog.observe_thinking());
    }

    #[tokio::test(start_paused = true)]
    async fn hosted_tool_heartbeats_cannot_extend_the_absolute_side_effect_deadline() {
        let policy = ModelProgressPolicy {
            soft_warning_after: Duration::from_secs(1),
            first_progress_deadline: Duration::from_secs(2),
            recovery_deadline: Duration::from_secs(1),
            tool_assembly_deadline: Duration::from_secs(3),
            answer_stream_deadline: Duration::from_secs(4),
            hosted_tool_deadline: Duration::from_secs(180),
            connect_deadline: Duration::from_secs(1),
            requires_tool_action: true,
        };
        let mut watchdog = ModelProgressWatchdog::with_policy(policy);
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
            ModelProgressDeadlineAction::StopAfterVisibleOutput
        );
    }

    #[test]
    fn recovery_controls_disable_optional_reasoning_and_reduce_always_on_models() {
        let qwen = recovery_controls(Some(ProviderType::Qwen), "qwen3.8-max");
        assert_eq!(qwen.reasoning_enabled, Some(false));
        assert_eq!(qwen.reasoning_effort, None);

        let kimi = recovery_controls(Some(ProviderType::Moonshot), "kimi-k3");
        assert_eq!(kimi.reasoning_enabled, Some(true));
        assert_eq!(kimi.reasoning_effort, Some(ReasoningEffort::Low));

        let openrouter = recovery_controls(Some(ProviderType::OpenRouter), "moonshotai/kimi-k3");
        assert_eq!(openrouter.reasoning_enabled, Some(true));
        assert_eq!(openrouter.reasoning_effort, Some(ReasoningEffort::Low));

        let openrouter_qwen = recovery_controls(Some(ProviderType::OpenRouter), "qwen/qwen3.8-max");
        assert_eq!(openrouter_qwen.reasoning_enabled, Some(false));
        assert_eq!(
            openrouter_qwen.reasoning_effort,
            Some(ReasoningEffort::None)
        );

        let routed = recovery_controls(Some(ProviderType::AlibabaModelStudio), "kimi/kimi-k3");
        assert_eq!(routed.reasoning_enabled, Some(true));
        assert_eq!(routed.reasoning_effort, Some(ReasoningEffort::Max));
        assert!(routed.description.contains("bounded"));
    }
}
