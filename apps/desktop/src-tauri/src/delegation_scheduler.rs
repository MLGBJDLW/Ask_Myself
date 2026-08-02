use std::sync::Arc;
use std::time::Duration;

use nexa_core::agent::AgentConfig;
use nexa_core::agent::CancellationToken;
use nexa_core::error::CoreError;
use nexa_core::llm::Usage;
use serde::Serialize;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const DEFAULT_QUEUE_DEADLINE_MS: u64 = 15_000;
const DEFAULT_CONNECT_DEADLINE_MS: u64 = 15_000;
const DEFAULT_FIRST_TOKEN_DEADLINE_MS: u64 = 45_000;
const DEFAULT_RUN_DEADLINE_MS: u64 = 180_000;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DelegationLimitPolicy {
    Auto,
    Explicit(u64),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegationLimitsV2 {
    pub input_context_policy: DelegationLimitPolicy,
    pub max_output_tokens_per_worker: DelegationLimitPolicy,
    pub total_actual_tokens_soft_limit: Option<u64>,
    pub total_cost_soft_limit_micros: Option<u64>,
    pub cost_accounting_available: bool,
    pub max_parallel: u32,
    pub max_calls_per_turn: u32,
    pub exploration_lane_slots: u32,
    pub verification_lane_slots: u32,
    pub judge_lane_slots: u32,
    pub queue_deadline_ms: u64,
    pub connect_deadline_ms: u64,
    pub first_token_deadline_ms: u64,
    pub run_deadline_ms: u64,
}

impl DelegationLimitsV2 {
    pub(crate) fn resolve(config: &AgentConfig) -> Self {
        let configured = config.delegation_limits_v2.as_ref();
        let max_parallel = configured
            .and_then(|limits| limits.max_parallel)
            .or(config.subagent_max_parallel)
            .unwrap_or(3)
            .clamp(1, 12);
        let max_calls_per_turn = configured
            .and_then(|limits| limits.max_calls_per_turn)
            .or(config.subagent_max_calls_per_turn)
            .unwrap_or(6)
            .clamp(1, 32);
        let dedicated_lanes = config
            .subagent_verification_reserve_percent
            .unwrap_or_default()
            > 0
            && max_parallel >= 3
            && max_calls_per_turn >= 3;
        let (exploration_lane_slots, verification_lane_slots, judge_lane_slots) = if dedicated_lanes
        {
            (max_parallel - 2, 1, 1)
        } else {
            (max_parallel, 0, 0)
        };
        let run_deadline_ms = configured
            .and_then(|limits| limits.run_deadline_ms)
            .unwrap_or(DEFAULT_RUN_DEADLINE_MS)
            .clamp(1_000, 3_600_000);
        Self {
            input_context_policy: configured
                .and_then(|limits| limits.input_context_limit)
                .or_else(|| {
                    configured
                        .is_none()
                        .then_some(config.context_window.map(u64::from))
                        .flatten()
                })
                .map(|value| DelegationLimitPolicy::Explicit(value.clamp(1_024, 10_000_000)))
                .unwrap_or(DelegationLimitPolicy::Auto),
            max_output_tokens_per_worker: configured
                .and_then(|limits| limits.max_output_tokens_per_worker)
                .or_else(|| {
                    configured
                        .is_none()
                        .then_some(config.max_tokens.map(u64::from))
                        .flatten()
                })
                .map(|value| DelegationLimitPolicy::Explicit(value.clamp(256, 1_000_000)))
                .unwrap_or(DelegationLimitPolicy::Auto),
            total_actual_tokens_soft_limit: match configured {
                Some(limits) => limits
                    .total_actual_tokens_soft_limit
                    .map(|value| value.clamp(256, 10_000_000)),
                None => Some(u64::from(
                    config
                        .subagent_token_budget
                        .unwrap_or(32_000)
                        .clamp(256, 200_000),
                )),
            },
            total_cost_soft_limit_micros: configured
                .and_then(|limits| limits.total_cost_soft_limit_micros),
            cost_accounting_available: nexa_core::usage_analytics::usage_cost_metadata(
                config.provider_type,
            )
            .0
            .is_some(),
            max_parallel,
            max_calls_per_turn,
            exploration_lane_slots,
            verification_lane_slots,
            judge_lane_slots,
            queue_deadline_ms: configured
                .and_then(|limits| limits.queue_deadline_ms)
                .unwrap_or(DEFAULT_QUEUE_DEADLINE_MS)
                .clamp(100, run_deadline_ms),
            connect_deadline_ms: configured
                .and_then(|limits| limits.connect_deadline_ms)
                .unwrap_or(DEFAULT_CONNECT_DEADLINE_MS)
                .clamp(100, run_deadline_ms),
            first_token_deadline_ms: configured
                .and_then(|limits| limits.first_token_deadline_ms)
                .unwrap_or(DEFAULT_FIRST_TOKEN_DEADLINE_MS)
                .clamp(100, run_deadline_ms),
            run_deadline_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DelegationLane {
    Exploration,
    Verification,
    Judge,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BudgetSnapshot {
    pub max_parallel: u32,
    pub max_calls_per_turn: u32,
    pub calls_started: u32,
    pub remaining_calls: u32,
    pub token_budget: u32,
    pub tokens_spent: u32,
    pub tokens_reserved: u32,
    pub remaining_tokens: u32,
    pub verification_reserve_tokens: u32,
    pub cost_soft_limit_micros: Option<u64>,
    pub cost_spent_micros: u64,
    pub cost_accounting_available: bool,
    pub exploration_lane_slots: u32,
    pub verification_lane_slots: u32,
    pub judge_lane_slots: u32,
}

#[derive(Debug)]
struct DelegationBudgetState {
    limits: DelegationLimitsV2,
    calls_started: u32,
    verification_calls_started: u32,
    judge_calls_started: u32,
    tokens_spent: u32,
    tokens_reserved: u32,
    cost_spent_micros: u64,
}

#[derive(Clone)]
pub(crate) struct DelegationScheduler {
    shared_lane: Option<Arc<Semaphore>>,
    exploration_lane: Arc<Semaphore>,
    verification_lane: Option<Arc<Semaphore>>,
    judge_lane: Option<Arc<Semaphore>>,
    state: Arc<Mutex<DelegationBudgetState>>,
}

impl DelegationScheduler {
    pub(crate) fn new(config: &AgentConfig) -> Self {
        Self::with_limits(DelegationLimitsV2::resolve(config))
    }

    fn with_limits(limits: DelegationLimitsV2) -> Self {
        let dedicated_lanes = limits.verification_lane_slots > 0 || limits.judge_lane_slots > 0;
        let shared_lane =
            (!dedicated_lanes).then(|| Arc::new(Semaphore::new(limits.max_parallel as usize)));
        Self {
            shared_lane,
            exploration_lane: Arc::new(Semaphore::new(limits.exploration_lane_slots as usize)),
            verification_lane: (limits.verification_lane_slots > 0)
                .then(|| Arc::new(Semaphore::new(limits.verification_lane_slots as usize))),
            judge_lane: (limits.judge_lane_slots > 0)
                .then(|| Arc::new(Semaphore::new(limits.judge_lane_slots as usize))),
            state: Arc::new(Mutex::new(DelegationBudgetState {
                limits,
                calls_started: 0,
                verification_calls_started: 0,
                judge_calls_started: 0,
                tokens_spent: 0,
                tokens_reserved: 0,
                cost_spent_micros: 0,
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_queue_deadline(config: &AgentConfig, queue_deadline: Duration) -> Self {
        let mut limits = DelegationLimitsV2::resolve(config);
        limits.queue_deadline_ms = u64::try_from(queue_deadline.as_millis()).unwrap_or(u64::MAX);
        Self::with_limits(limits)
    }

    pub(crate) async fn begin_call(
        &self,
        label: &str,
        reserved_tokens: u32,
        is_verification: bool,
        cancel_token: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, CoreError> {
        let lane = if is_verification {
            DelegationLane::Verification
        } else {
            DelegationLane::Exploration
        };
        self.admit(label, reserved_tokens, lane, cancel_token).await
    }

    pub(crate) async fn begin_judge_call(
        &self,
        label: &str,
        reserved_tokens: u32,
        cancel_token: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, CoreError> {
        self.admit(label, reserved_tokens, DelegationLane::Judge, cancel_token)
            .await
    }

    async fn admit(
        &self,
        label: &str,
        reserved_tokens: u32,
        lane: DelegationLane,
        cancel_token: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, CoreError> {
        // Phase one: enqueue and wait for a concrete worker lane. No call or
        // token credit is consumed while this future is merely queued.
        let queue_deadline_ms = {
            let state = self.state.lock().await;
            Self::validate_admission(&state, label, lane)?;
            state.limits.queue_deadline_ms
        };
        let semaphore = self.semaphore_for(lane);
        let permit = tokio::select! {
            _ = cancel_token.cancelled() => Err(CoreError::Agent(format!(
                "Delegated execution '{label}' was cancelled while waiting for a worker slot."
            ))),
            result = tokio::time::timeout(
                Duration::from_millis(queue_deadline_ms),
                semaphore.acquire_owned(),
            ) => match result {
                Ok(Ok(permit)) => Ok(permit),
                Ok(Err(_)) => Err(CoreError::Internal(
                    "delegated execution semaphore closed".into()
                )),
                Err(_) => Err(CoreError::Agent(format!(
                    "Delegated execution '{label}' exceeded its {queue_deadline_ms}ms queue deadline."
                ))),
            },
        }?;

        // Phase two: once a slot exists, consume call admission and the small
        // role credit. Actual usage replaces this reservation at completion.
        let mut state = self.state.lock().await;
        Self::validate_admission(&state, label, lane)?;
        state.calls_started += 1;
        if lane == DelegationLane::Verification {
            state.verification_calls_started += 1;
        } else if lane == DelegationLane::Judge {
            state.judge_calls_started += 1;
        }
        state.tokens_reserved = state.tokens_reserved.saturating_add(reserved_tokens);
        drop(state);
        Ok(permit)
    }

    fn validate_admission(
        state: &DelegationBudgetState,
        label: &str,
        lane: DelegationLane,
    ) -> Result<(), CoreError> {
        let reserved_for_other_control_lanes = match lane {
            DelegationLane::Exploration => {
                u32::from(
                    state.limits.verification_lane_slots > 0
                        && state.verification_calls_started == 0,
                ) + u32::from(state.limits.judge_lane_slots > 0 && state.judge_calls_started == 0)
            }
            DelegationLane::Verification => {
                u32::from(state.limits.judge_lane_slots > 0 && state.judge_calls_started == 0)
            }
            DelegationLane::Judge => u32::from(
                state.limits.verification_lane_slots > 0 && state.verification_calls_started == 0,
            ),
        };
        let lane_call_limit = state
            .limits
            .max_calls_per_turn
            .saturating_sub(reserved_for_other_control_lanes);
        if state.calls_started >= lane_call_limit {
            return Err(CoreError::InvalidInput(format!(
                "Delegated execution budget exhausted before starting {label}: {} call(s) started and {reserved_for_other_control_lanes} call credit(s) remain reserved for unused control lanes (maximum {} per turn).",
                state.calls_started,
                state.limits.max_calls_per_turn,
            )));
        }
        let token_budget = state
            .limits
            .total_actual_tokens_soft_limit
            .unwrap_or(u64::MAX);
        if u64::from(state.tokens_spent) >= token_budget {
            return Err(CoreError::InvalidInput(format!(
                "Delegated execution token soft limit exhausted before starting {label}. Spent: {} of {token_budget} tokens.",
                state.tokens_spent,
            )));
        }
        if let Some(cost_limit) = state.limits.total_cost_soft_limit_micros {
            // A soft cost limit is enforceable only when the selected provider
            // has versioned pricing metadata. Unknown pricing remains visible
            // in BudgetSnapshot, but must not disable all remote delegation.
            if state.limits.cost_accounting_available && state.cost_spent_micros >= cost_limit {
                return Err(CoreError::InvalidInput(format!(
                    "Delegated execution cost soft limit exhausted before starting {label}. Spent: {} of {cost_limit} micros.",
                    state.cost_spent_micros,
                )));
            }
        }
        Ok(())
    }

    fn semaphore_for(&self, lane: DelegationLane) -> Arc<Semaphore> {
        if let Some(shared) = &self.shared_lane {
            return Arc::clone(shared);
        }
        match lane {
            DelegationLane::Exploration => Arc::clone(&self.exploration_lane),
            DelegationLane::Verification => self
                .verification_lane
                .as_ref()
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::clone(&self.exploration_lane)),
            DelegationLane::Judge => self
                .judge_lane
                .as_ref()
                .map(Arc::clone)
                .or_else(|| self.verification_lane.as_ref().map(Arc::clone))
                .unwrap_or_else(|| Arc::clone(&self.exploration_lane)),
        }
    }

    pub(crate) async fn finish_call(
        &self,
        reserved_tokens: u32,
        usage: &Usage,
        estimated_cost_micros: Option<u64>,
    ) {
        let mut state = self.state.lock().await;
        state.tokens_reserved = state.tokens_reserved.saturating_sub(reserved_tokens);
        state.tokens_spent = state.tokens_spent.saturating_add(usage.total_tokens);
        state.cost_spent_micros = state
            .cost_spent_micros
            .saturating_add(estimated_cost_micros.unwrap_or(0));
    }

    pub(crate) async fn release_reservation(&self, reserved_tokens: u32) {
        let mut state = self.state.lock().await;
        state.tokens_reserved = state.tokens_reserved.saturating_sub(reserved_tokens);
    }

    pub(crate) async fn rollback_unstarted_worker(
        &self,
        reserved_tokens: u32,
        is_verification: bool,
    ) {
        let mut state = self.state.lock().await;
        state.calls_started = state.calls_started.saturating_sub(1);
        if is_verification {
            state.verification_calls_started = state.verification_calls_started.saturating_sub(1);
        }
        state.tokens_reserved = state.tokens_reserved.saturating_sub(reserved_tokens);
    }

    pub(crate) async fn snapshot(&self) -> BudgetSnapshot {
        let state = self.state.lock().await;
        let token_budget = state
            .limits
            .total_actual_tokens_soft_limit
            .unwrap_or(u64::from(u32::MAX))
            .min(u64::from(u32::MAX)) as u32;
        BudgetSnapshot {
            max_parallel: state.limits.max_parallel,
            max_calls_per_turn: state.limits.max_calls_per_turn,
            calls_started: state.calls_started,
            remaining_calls: state
                .limits
                .max_calls_per_turn
                .saturating_sub(state.calls_started),
            token_budget,
            tokens_spent: state.tokens_spent,
            tokens_reserved: state.tokens_reserved,
            remaining_tokens: token_budget
                .saturating_sub(state.tokens_spent.saturating_add(state.tokens_reserved)),
            // V2 reserves worker lanes and role credits, not a frozen fraction
            // of the entire delegated token budget.
            verification_reserve_tokens: 0,
            cost_soft_limit_micros: state.limits.total_cost_soft_limit_micros,
            cost_spent_micros: state.cost_spent_micros,
            cost_accounting_available: state.limits.cost_accounting_available,
            exploration_lane_slots: state.limits.exploration_lane_slots,
            verification_lane_slots: state.limits.verification_lane_slots,
            judge_lane_slots: state.limits.judge_lane_slots,
        }
    }

    pub(crate) async fn limits(&self) -> DelegationLimitsV2 {
        self.state.lock().await.limits.clone()
    }
}
