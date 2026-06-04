//! Eval Harness contract for deterministic trajectory checks.

use serde::{Deserialize, Serialize};

use crate::agent::AgentEvent;
use crate::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunPhase};
use crate::approval::{ApprovalDecision, ApprovalRequest, ApprovalRisk};
use crate::error::CoreError;
use crate::llm::{Message, Role, Usage};
use crate::runtime::{
    validate_runtime_turn_events, AgentSession, AgentSessionConfig, AgentTurnHandle,
    AgentTurnInput, AgentTurnState, RuntimeProtocolError, RuntimeTerminalStatus,
};
use crate::trajectory::{Trajectory, TrajectoryStore, TrajectoryStoreSummary};

pub const EVAL_HARNESS_CONTRACT_VERSION: u16 = 1;
pub const DEVELOPER_EVAL_SMOKE_TRAJECTORY_LIMIT: usize = 50;
pub const DEVELOPER_EVAL_NIGHTLY_TRAJECTORY_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperEvalWorkflowProfile {
    Smoke,
    Nightly,
}

impl DeveloperEvalWorkflowProfile {
    fn default_trajectory_limit(self) -> usize {
        match self {
            Self::Smoke => DEVELOPER_EVAL_SMOKE_TRAJECTORY_LIMIT,
            Self::Nightly => DEVELOPER_EVAL_NIGHTLY_TRAJECTORY_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvalAssertionKind {
    TrajectoryAvailability,
    EventOrder,
    ToolUse,
    ApprovalBehavior,
    TaskOrchestration,
    EvidenceSupport,
    FinalAnswerContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalAssertion {
    pub kind: EvalAssertionKind,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCase {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub trajectory_id: Option<String>,
    #[serde(default)]
    pub assertions: Vec<EvalAssertion>,
    #[serde(default)]
    pub allowed_nondeterminism: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalPack {
    pub version: u16,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalFailure {
    pub case_id: String,
    pub assertion: EvalAssertionKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub pack_id: String,
    pub passed: bool,
    #[serde(default)]
    pub failures: Vec<EvalFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTrajectoryEvalCaseReport {
    pub trajectory_id: String,
    pub source_kind: String,
    #[serde(default)]
    pub source_run_id: Option<String>,
    pub user_input_summary: String,
    pub passed: bool,
    #[serde(default)]
    pub failures: Vec<EvalFailure>,
    #[serde(default)]
    pub replay_terminal_status: Option<RuntimeTerminalStatus>,
    #[serde(default)]
    pub replay_event_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTrajectoryEvalReport {
    pub status: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    #[serde(default)]
    pub cases: Vec<StoredTrajectoryEvalCaseReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperEvalSmokeReport {
    pub profile: DeveloperEvalWorkflowProfile,
    pub trajectory_limit: usize,
    pub status: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub quality_eval: crate::quality_eval::QualityEvalReport,
    pub stored_trajectory_eval: StoredTrajectoryEvalReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrajectoryReplayCheck {
    RuntimeContract,
    EventKindSequence,
    ToolCallSequence,
    ApprovalSequence,
    TaskOrchestration,
    EvidenceIds,
    FinalAnswer,
    Outcome,
}

impl TrajectoryReplayCheck {
    pub const DEFAULT: [Self; 8] = [
        Self::RuntimeContract,
        Self::EventKindSequence,
        Self::ToolCallSequence,
        Self::ApprovalSequence,
        Self::TaskOrchestration,
        Self::EvidenceIds,
        Self::FinalAnswer,
        Self::Outcome,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryReplayRequest {
    pub expected_trajectory_id: String,
    pub replayed_trajectory_id: String,
    #[serde(default)]
    pub checks: Vec<TrajectoryReplayCheck>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryReplayMismatch {
    pub check: TrajectoryReplayCheck,
    pub message: String,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryReplayReport {
    pub expected_trajectory_id: String,
    pub replayed_trajectory_id: String,
    pub passed: bool,
    #[serde(default)]
    pub mismatches: Vec<TrajectoryReplayMismatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryReplayRuntimeMode {
    RecordedEvents,
    MockRuntime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryReplayExecution {
    pub trajectory_id: String,
    pub runtime_mode: TrajectoryReplayRuntimeMode,
    pub run_id: String,
    pub turn_id: String,
    pub terminal_status: RuntimeTerminalStatus,
    pub event_count: usize,
    #[serde(default)]
    pub final_message: Option<String>,
    #[serde(default)]
    pub events: Vec<crate::agent_run::AgentRunEvent>,
}

pub trait ReplayRuntimeAdapter: Send + Sync {
    fn mode(&self) -> TrajectoryReplayRuntimeMode;

    fn build_events(
        &self,
        trajectory: &Trajectory,
        input: &AgentTurnInput,
    ) -> Result<Vec<AgentRunEvent>, CoreError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RecordedReplayRuntimeAdapter;

impl ReplayRuntimeAdapter for RecordedReplayRuntimeAdapter {
    fn mode(&self) -> TrajectoryReplayRuntimeMode {
        TrajectoryReplayRuntimeMode::RecordedEvents
    }

    fn build_events(
        &self,
        trajectory: &Trajectory,
        _input: &AgentTurnInput,
    ) -> Result<Vec<AgentRunEvent>, CoreError> {
        Ok(trajectory.run_events.clone())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MockReplayRuntimeAdapter;

impl ReplayRuntimeAdapter for MockReplayRuntimeAdapter {
    fn mode(&self) -> TrajectoryReplayRuntimeMode {
        TrajectoryReplayRuntimeMode::MockRuntime
    }

    fn build_events(
        &self,
        trajectory: &Trajectory,
        input: &AgentTurnInput,
    ) -> Result<Vec<AgentRunEvent>, CoreError> {
        Ok(mock_replay_events(trajectory, input))
    }
}

pub struct ReplayAgentSession {
    config: AgentSessionConfig,
    trajectory: Trajectory,
    runtime_adapter: Box<dyn ReplayRuntimeAdapter>,
    consumed: bool,
    handle: Option<AgentTurnHandle>,
    events: Vec<AgentRunEvent>,
}

impl ReplayAgentSession {
    pub fn new(trajectory: Trajectory) -> Self {
        Self::with_runtime_adapter(trajectory, RecordedReplayRuntimeAdapter)
    }

    pub fn with_runtime_adapter<A>(trajectory: Trajectory, runtime_adapter: A) -> Self
    where
        A: ReplayRuntimeAdapter + 'static,
    {
        let mut config = trajectory.session_config.clone();
        config.apply_protocol_defaults();
        Self {
            config,
            trajectory,
            runtime_adapter: Box::new(runtime_adapter),
            consumed: false,
            handle: None,
            events: Vec::new(),
        }
    }

    pub fn mock_runtime(trajectory: Trajectory) -> Self {
        Self::with_runtime_adapter(trajectory, MockReplayRuntimeAdapter)
    }
}

#[async_trait::async_trait]
impl AgentSession for ReplayAgentSession {
    fn config(&self) -> &AgentSessionConfig {
        &self.config
    }

    async fn configure(&mut self, config: AgentSessionConfig) -> Result<(), CoreError> {
        self.config = config;
        self.config.apply_protocol_defaults();
        Ok(())
    }

    async fn start_turn(&mut self, input: AgentTurnInput) -> Result<AgentTurnHandle, CoreError> {
        if self.consumed {
            return Err(CoreError::InvalidInput(
                "ReplayAgentSession can replay a trajectory only once".to_string(),
            ));
        }
        if let Some(raw_user_input) = self.trajectory.raw_user_input.as_deref() {
            if !raw_user_input.trim().is_empty() && input.user_text.trim() != raw_user_input.trim()
            {
                return Err(CoreError::InvalidInput(
                    "replay input does not match trajectory raw user input".to_string(),
                ));
            }
        }

        let events = self
            .runtime_adapter
            .build_events(&self.trajectory, &input)?;
        let report = validate_runtime_turn_events(&events)
            .map_err(|err| CoreError::Agent(format!("Replay trajectory contract failed: {err}")))?;
        let handle = AgentTurnHandle {
            session_id: self.config.session_id.clone(),
            run_id: report.run_id,
            turn_id: report.turn_id,
            state: AgentTurnState::Terminal(report.terminal_status),
        };
        self.consumed = true;
        self.events = events;
        self.handle = Some(handle.clone());
        Ok(handle)
    }

    async fn steer_turn(&mut self, _turn_id: &str, _text: String) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "ReplayAgentSession is deterministic and cannot be steered".to_string(),
        ))
    }

    async fn interrupt_turn(&mut self, _turn_id: &str, _reason: String) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "ReplayAgentSession is deterministic and cannot be interrupted".to_string(),
        ))
    }

    async fn resolve_approval(
        &mut self,
        _request_id: &str,
        _decision: crate::approval::ApprovalDecision,
    ) -> Result<(), CoreError> {
        Err(CoreError::InvalidInput(
            "ReplayAgentSession replays recorded approval outcomes".to_string(),
        ))
    }

    async fn read_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<crate::agent_run::AgentRunEvent>, CoreError> {
        let Some(handle) = &self.handle else {
            return Err(CoreError::InvalidInput(
                "ReplayAgentSession has not started a turn".to_string(),
            ));
        };
        if handle.run_id != run_id {
            return Err(CoreError::NotFound(format!("replay events for {run_id}")));
        }
        Ok(self.events.clone())
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        Ok(())
    }
}

pub fn evaluate_trajectory_contract(
    pack: &EvalPack,
    case: &EvalCase,
    trajectory: &Trajectory,
) -> EvalReport {
    let mut failures = Vec::new();

    if pack.version != EVAL_HARNESS_CONTRACT_VERSION {
        failures.push(EvalFailure {
            case_id: case.id.clone(),
            assertion: EvalAssertionKind::EventOrder,
            message: format!("unsupported eval pack version {}", pack.version),
        });
    }

    for assertion in &case.assertions {
        match assertion.kind {
            EvalAssertionKind::TrajectoryAvailability => {
                if trajectory.trajectory_id.trim().is_empty() {
                    failures.push(failure(
                        &case.id,
                        assertion.kind,
                        "expected a loaded trajectory id",
                    ));
                }
            }
            EvalAssertionKind::EventOrder => {
                if let Err(err) = validate_runtime_turn_events(&trajectory.run_events) {
                    failures.push(failure(&case.id, assertion.kind, event_order_message(err)));
                }
            }
            EvalAssertionKind::ToolUse => {
                if trajectory.tool_calls.is_empty() {
                    failures.push(failure(
                        &case.id,
                        assertion.kind,
                        "expected at least one tool call",
                    ));
                }
            }
            EvalAssertionKind::ApprovalBehavior => {
                if trajectory.approvals.is_empty() {
                    failures.push(failure(
                        &case.id,
                        assertion.kind,
                        "expected at least one approval record",
                    ));
                }
            }
            EvalAssertionKind::TaskOrchestration => {
                if trajectory.task_queue_items.is_empty()
                    && trajectory.task_runs.is_empty()
                    && trajectory.scheduler_events.is_empty()
                {
                    failures.push(failure(
                        &case.id,
                        assertion.kind,
                        "expected task orchestration queue, run, or scheduler event context",
                    ));
                }
            }
            EvalAssertionKind::EvidenceSupport => {
                if trajectory.retrieved_evidence.is_empty() {
                    failures.push(failure(
                        &case.id,
                        assertion.kind,
                        "expected retrieved evidence",
                    ));
                }
            }
            EvalAssertionKind::FinalAnswerContract => {
                if trajectory
                    .final_answer
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    failures.push(failure(&case.id, assertion.kind, "expected a final answer"));
                }
            }
        }
    }

    EvalReport {
        pack_id: pack.id.clone(),
        passed: failures.is_empty(),
        failures,
    }
}

pub async fn evaluate_pack_from_store<S>(
    store: &S,
    pack: &EvalPack,
) -> Result<EvalReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    let mut failures = Vec::new();

    if pack.version != EVAL_HARNESS_CONTRACT_VERSION {
        failures.push(failure(
            "__pack__",
            EvalAssertionKind::TrajectoryAvailability,
            format!("unsupported eval pack version {}", pack.version),
        ));
        return Ok(EvalReport {
            pack_id: pack.id.clone(),
            passed: false,
            failures,
        });
    }

    for case in &pack.cases {
        let Some(trajectory_id) = case
            .trajectory_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            failures.push(failure(
                &case.id,
                EvalAssertionKind::TrajectoryAvailability,
                "eval case must reference a trajectory id",
            ));
            continue;
        };

        let trajectory = match store.load_trajectory(trajectory_id).await {
            Ok(trajectory) => trajectory,
            Err(err) => {
                failures.push(failure(
                    &case.id,
                    EvalAssertionKind::TrajectoryAvailability,
                    format!("failed to load trajectory '{trajectory_id}': {err}"),
                ));
                continue;
            }
        };

        let case_report = evaluate_trajectory_contract(pack, case, &trajectory);
        failures.extend(case_report.failures);
    }

    Ok(EvalReport {
        pack_id: pack.id.clone(),
        passed: failures.is_empty(),
        failures,
    })
}

pub async fn run_stored_trajectory_smoke_eval<S>(
    store: &S,
    limit: usize,
) -> Result<StoredTrajectoryEvalReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    let summaries = store.list_trajectory_summaries(limit.clamp(1, 500)).await?;
    let pack = smoke_eval_pack();
    let mut cases = Vec::with_capacity(summaries.len());

    for summary in summaries {
        cases.push(run_stored_trajectory_smoke_case(store, &pack, summary).await);
    }

    let passed = cases.iter().filter(|case| case.passed).count();
    let failed = cases.len().saturating_sub(passed);
    let status = if cases.is_empty() {
        "empty"
    } else if failed == 0 {
        "passed"
    } else {
        "failed"
    };

    Ok(StoredTrajectoryEvalReport {
        status: status.to_string(),
        total: cases.len(),
        passed,
        failed,
        cases,
    })
}

pub async fn run_developer_eval_smoke_workflow<S>(
    store: &S,
    trajectory_limit: usize,
) -> Result<DeveloperEvalSmokeReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    run_developer_eval_workflow(
        store,
        DeveloperEvalWorkflowProfile::Smoke,
        Some(trajectory_limit),
    )
    .await
}

pub async fn run_developer_eval_nightly_workflow<S>(
    store: &S,
) -> Result<DeveloperEvalSmokeReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    run_developer_eval_workflow(store, DeveloperEvalWorkflowProfile::Nightly, None).await
}

pub async fn run_developer_eval_workflow<S>(
    store: &S,
    profile: DeveloperEvalWorkflowProfile,
    trajectory_limit: Option<usize>,
) -> Result<DeveloperEvalSmokeReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    let trajectory_limit = trajectory_limit.unwrap_or(profile.default_trajectory_limit());
    let quality_eval = crate::quality_eval::run_agent_quality_eval();
    let stored_trajectory_eval = run_stored_trajectory_smoke_eval(store, trajectory_limit).await?;
    let total = quality_eval.total + stored_trajectory_eval.total;
    let passed = quality_eval.passed + stored_trajectory_eval.passed;
    let failed = quality_eval.failed + stored_trajectory_eval.failed;
    let status = if failed == 0 { "passed" } else { "failed" };

    Ok(DeveloperEvalSmokeReport {
        profile,
        trajectory_limit,
        status: status.to_string(),
        total,
        passed,
        failed,
        quality_eval,
        stored_trajectory_eval,
    })
}

async fn run_stored_trajectory_smoke_case<S>(
    store: &S,
    pack: &EvalPack,
    summary: TrajectoryStoreSummary,
) -> StoredTrajectoryEvalCaseReport
where
    S: TrajectoryStore + ?Sized,
{
    let mut failures = Vec::new();
    let mut replay_terminal_status = None;
    let mut replay_event_count = None;

    match store.load_trajectory(&summary.trajectory_id).await {
        Ok(trajectory) => {
            let case = smoke_eval_case(&summary.trajectory_id);
            let report = evaluate_trajectory_contract(pack, &case, &trajectory);
            failures.extend(report.failures);

            match replay_trajectory_through_session(trajectory).await {
                Ok(execution) => {
                    replay_terminal_status = Some(execution.terminal_status);
                    replay_event_count = Some(execution.event_count);
                }
                Err(err) => failures.push(failure(
                    &case.id,
                    EvalAssertionKind::EventOrder,
                    format!("failed to replay trajectory through AgentSession: {err}"),
                )),
            }
        }
        Err(err) => failures.push(failure(
            &summary.trajectory_id,
            EvalAssertionKind::TrajectoryAvailability,
            format!(
                "failed to load trajectory '{}': {err}",
                summary.trajectory_id
            ),
        )),
    }

    StoredTrajectoryEvalCaseReport {
        trajectory_id: summary.trajectory_id,
        source_kind: summary.source_kind,
        source_run_id: summary.source_run_id,
        user_input_summary: summary.user_input_summary,
        passed: failures.is_empty(),
        failures,
        replay_terminal_status,
        replay_event_count,
    }
}

pub async fn evaluate_replay_from_store<S>(
    store: &S,
    request: &TrajectoryReplayRequest,
) -> Result<TrajectoryReplayReport, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    let expected = store
        .load_trajectory(request.expected_trajectory_id.trim())
        .await?;
    let replayed = store
        .load_trajectory(request.replayed_trajectory_id.trim())
        .await?;
    Ok(compare_trajectory_replay(
        &expected,
        &replayed,
        &request.checks,
    ))
}

pub async fn replay_trajectory_from_store<S>(
    store: &S,
    trajectory_id: &str,
) -> Result<TrajectoryReplayExecution, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    replay_trajectory_from_store_with_runtime_mode(
        store,
        trajectory_id,
        TrajectoryReplayRuntimeMode::RecordedEvents,
    )
    .await
}

pub async fn replay_trajectory_from_store_with_runtime_mode<S>(
    store: &S,
    trajectory_id: &str,
    runtime_mode: TrajectoryReplayRuntimeMode,
) -> Result<TrajectoryReplayExecution, CoreError>
where
    S: TrajectoryStore + ?Sized,
{
    let trajectory = store.load_trajectory(trajectory_id.trim()).await?;
    match runtime_mode {
        TrajectoryReplayRuntimeMode::RecordedEvents => {
            replay_trajectory_through_session(trajectory).await
        }
        TrajectoryReplayRuntimeMode::MockRuntime => {
            replay_trajectory_through_mock_runtime(trajectory).await
        }
    }
}

pub async fn replay_trajectory_through_session(
    trajectory: Trajectory,
) -> Result<TrajectoryReplayExecution, CoreError> {
    replay_trajectory_with_runtime_adapter(trajectory, RecordedReplayRuntimeAdapter).await
}

pub async fn replay_trajectory_through_mock_runtime(
    trajectory: Trajectory,
) -> Result<TrajectoryReplayExecution, CoreError> {
    replay_trajectory_with_runtime_adapter(trajectory, MockReplayRuntimeAdapter).await
}

async fn replay_trajectory_with_runtime_adapter<A>(
    trajectory: Trajectory,
    runtime_adapter: A,
) -> Result<TrajectoryReplayExecution, CoreError>
where
    A: ReplayRuntimeAdapter + 'static,
{
    let trajectory_id = trajectory.trajectory_id.clone();
    let input = AgentTurnInput::text(replay_input_text(&trajectory));
    let final_message = trajectory.final_answer.clone();
    let runtime_mode = runtime_adapter.mode();
    let mut session = ReplayAgentSession::with_runtime_adapter(trajectory, runtime_adapter);
    let handle = session.start_turn(input).await?;
    let events = session.read_events(&handle.run_id).await?;
    let terminal_status = match handle.state {
        AgentTurnState::Terminal(status) => status,
        _ => RuntimeTerminalStatus::Failed,
    };

    Ok(TrajectoryReplayExecution {
        trajectory_id,
        runtime_mode,
        run_id: handle.run_id,
        turn_id: handle.turn_id,
        terminal_status,
        event_count: events.len(),
        final_message,
        events,
    })
}

fn mock_replay_events(trajectory: &Trajectory, input: &AgentTurnInput) -> Vec<AgentRunEvent> {
    let run_id = replay_runtime_identifier("mock-run", &trajectory.trajectory_id);
    let turn_id = replay_runtime_identifier("mock-turn", &trajectory.trajectory_id);
    let mut event_seq = 1;
    let mut events = Vec::new();
    let routing_payload = serde_json::json!({
        "mode": "mock_runtime",
        "trajectoryId": &trajectory.trajectory_id,
        "userInput": &input.user_text,
    });

    events.push(AgentRunEvent::status_update(
        &run_id,
        Some(&turn_id),
        event_seq,
        AgentRunPhase::Routing,
        "Mock replay runtime started",
        Some("running"),
        Some(&routing_payload),
    ));
    event_seq += 1;

    let step_count = trajectory.tool_calls.len().max(trajectory.approvals.len());
    for index in 0..step_count {
        if let Some(approval) = trajectory.approvals.get(index) {
            let tool_name = tool_name_from_value(approval)
                .or_else(|| {
                    trajectory
                        .tool_calls
                        .get(index)
                        .and_then(tool_name_from_value)
                })
                .unwrap_or_else(|| "mock_tool".to_string());
            let request_id = approval_request_id_from_value(approval, index);
            let arguments = approval_arguments_from_value(approval);
            let request = ApprovalRequest::new(
                request_id.clone(),
                tool_name,
                &arguments,
                approval_risk_from_value(approval),
                "Mock replay approval",
            );
            push_mock_agent_event(
                &mut events,
                AgentEvent::ApprovalRequested { request },
                &run_id,
                &turn_id,
                &mut event_seq,
            );
            push_mock_agent_event(
                &mut events,
                AgentEvent::ApprovalResolved {
                    request_id,
                    decision: approval_decision_from_value(approval),
                },
                &run_id,
                &turn_id,
                &mut event_seq,
            );
        }

        if let Some(tool_call) = trajectory.tool_calls.get(index) {
            let tool_name = tool_name_from_value(tool_call)
                .unwrap_or_else(|| format!("mock_tool_{}", index + 1));
            let call_id = tool_call_id_from_value(tool_call, index);
            push_mock_agent_event(
                &mut events,
                AgentEvent::ToolCallStart {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    arguments: tool_arguments_from_value(tool_call),
                },
                &run_id,
                &turn_id,
                &mut event_seq,
            );
            push_mock_agent_event(
                &mut events,
                AgentEvent::ToolCallResult {
                    call_id,
                    tool_name: tool_name.clone(),
                    content: tool_result_content_from_value(tool_call, &tool_name),
                    is_error: tool_call_is_error(tool_call),
                    artifacts: tool_artifacts_from_value(tool_call),
                },
                &run_id,
                &turn_id,
                &mut event_seq,
            );
        }
    }

    match mock_terminal_status_from_trajectory(trajectory) {
        RuntimeTerminalStatus::Failed => {
            let message = trajectory
                .final_answer
                .as_deref()
                .filter(|message| !message.trim().is_empty())
                .unwrap_or("Mock replay failed.");
            let payload = serde_json::json!({
                "mode": "mock_runtime",
                "trajectoryId": &trajectory.trajectory_id,
            });
            events.push(AgentRunEvent::terminal_error(
                &run_id,
                Some(&turn_id),
                event_seq,
                message,
                "failed",
                Some(&payload),
            ));
        }
        status => {
            let finish_reason = match status {
                RuntimeTerminalStatus::Cancelled => "cancelled",
                RuntimeTerminalStatus::TimedOut => "timed_out",
                _ => "stop",
            };
            let message = trajectory
                .final_answer
                .clone()
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| "Mock replay completed.".to_string());
            push_mock_agent_event(
                &mut events,
                AgentEvent::Done {
                    message: Message::text(Role::Assistant, message),
                    usage_total: Usage::default(),
                    last_prompt_tokens: 0,
                    context_breakdown: None,
                    cached: false,
                    finish_reason: Some(finish_reason.to_string()),
                },
                &run_id,
                &turn_id,
                &mut event_seq,
            );
        }
    }

    events
}

fn push_mock_agent_event(
    events: &mut Vec<AgentRunEvent>,
    event: AgentEvent,
    run_id: &str,
    turn_id: &str,
    event_seq: &mut u64,
) {
    events.push(AgentRunEvent::from_agent_event(&event).with_context(
        Some(run_id),
        Some(turn_id),
        Some(*event_seq),
    ));
    *event_seq += 1;
}

fn replay_runtime_identifier(prefix: &str, trajectory_id: &str) -> String {
    let slug = trajectory_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}-{slug}")
    }
}

pub fn compare_trajectory_replay(
    expected: &Trajectory,
    replayed: &Trajectory,
    checks: &[TrajectoryReplayCheck],
) -> TrajectoryReplayReport {
    let checks = if checks.is_empty() {
        TrajectoryReplayCheck::DEFAULT.as_slice()
    } else {
        checks
    };
    let mut mismatches = Vec::new();

    for check in checks {
        let (expected_value, actual_value, message) = match check {
            TrajectoryReplayCheck::RuntimeContract => (
                runtime_contract_value(expected),
                runtime_contract_value(replayed),
                "runtime event contract differs",
            ),
            TrajectoryReplayCheck::EventKindSequence => (
                event_kind_sequence(expected),
                event_kind_sequence(replayed),
                "durable event kind sequence differs",
            ),
            TrajectoryReplayCheck::ToolCallSequence => (
                serde_json::json!(tool_call_sequence(expected)),
                serde_json::json!(tool_call_sequence(replayed)),
                "tool call sequence differs",
            ),
            TrajectoryReplayCheck::ApprovalSequence => (
                approval_sequence(expected),
                approval_sequence(replayed),
                "approval sequence differs",
            ),
            TrajectoryReplayCheck::TaskOrchestration => (
                task_orchestration_signature(expected),
                task_orchestration_signature(replayed),
                "task orchestration projection differs",
            ),
            TrajectoryReplayCheck::EvidenceIds => (
                serde_json::json!(evidence_ids(expected)),
                serde_json::json!(evidence_ids(replayed)),
                "retrieved evidence ids differ",
            ),
            TrajectoryReplayCheck::FinalAnswer => (
                serde_json::json!(normalize_optional_text(expected.final_answer.as_deref())),
                serde_json::json!(normalize_optional_text(replayed.final_answer.as_deref())),
                "final answer differs",
            ),
            TrajectoryReplayCheck::Outcome => (
                serde_json::json!(normalize_optional_text(expected.outcome.as_deref())),
                serde_json::json!(normalize_optional_text(replayed.outcome.as_deref())),
                "outcome differs",
            ),
        };

        if expected_value != actual_value {
            mismatches.push(TrajectoryReplayMismatch {
                check: *check,
                message: message.to_string(),
                expected: expected_value,
                actual: actual_value,
            });
        }
    }

    TrajectoryReplayReport {
        expected_trajectory_id: expected.trajectory_id.clone(),
        replayed_trajectory_id: replayed.trajectory_id.clone(),
        passed: mismatches.is_empty(),
        mismatches,
    }
}

fn failure(case_id: &str, assertion: EvalAssertionKind, message: impl Into<String>) -> EvalFailure {
    EvalFailure {
        case_id: case_id.to_string(),
        assertion,
        message: message.into(),
    }
}

fn event_order_message(err: RuntimeProtocolError) -> String {
    format!("runtime event order contract failed: {err}")
}

fn smoke_eval_pack() -> EvalPack {
    EvalPack {
        version: EVAL_HARNESS_CONTRACT_VERSION,
        id: "stored-trajectory-smoke".to_string(),
        name: "Stored Trajectory Smoke Eval".to_string(),
        cases: Vec::new(),
    }
}

fn smoke_eval_case(trajectory_id: &str) -> EvalCase {
    EvalCase {
        id: trajectory_id.to_string(),
        name: trajectory_id.to_string(),
        trajectory_id: Some(trajectory_id.to_string()),
        assertions: vec![
            EvalAssertion {
                kind: EvalAssertionKind::TrajectoryAvailability,
                description: "trajectory can be loaded".to_string(),
            },
            EvalAssertion {
                kind: EvalAssertionKind::EventOrder,
                description: "durable runtime events satisfy the Runtime Protocol".to_string(),
            },
        ],
        allowed_nondeterminism: Vec::new(),
    }
}

fn runtime_contract_value(trajectory: &Trajectory) -> serde_json::Value {
    match validate_runtime_turn_events(&trajectory.run_events) {
        Ok(report) => serde_json::json!({
            "valid": true,
            "terminalStatus": report.terminal_status.as_str(),
            "approvalDenied": report.approval_denied,
        }),
        Err(err) => serde_json::json!({
            "valid": false,
            "error": err.to_string(),
        }),
    }
}

fn event_kind_sequence(trajectory: &Trajectory) -> serde_json::Value {
    serde_json::json!(trajectory
        .run_events
        .iter()
        .map(|event| {
            serde_json::json!({
                "kind": event.kind.as_str(),
                "phase": event.phase.as_str(),
                "status": event.status,
            })
        })
        .collect::<Vec<_>>())
}

fn tool_call_sequence(trajectory: &Trajectory) -> Vec<String> {
    let from_tool_calls = trajectory
        .tool_calls
        .iter()
        .filter_map(tool_name_from_value)
        .collect::<Vec<_>>();
    if !from_tool_calls.is_empty() {
        return from_tool_calls;
    }

    trajectory
        .run_events
        .iter()
        .filter(|event| event.kind == AgentRunEventKind::ToolCompleted)
        .filter_map(|event| tool_name_from_value(&event.payload))
        .collect()
}

fn approval_sequence(trajectory: &Trajectory) -> serde_json::Value {
    if !trajectory.approvals.is_empty() {
        return serde_json::json!(trajectory.approvals);
    }

    serde_json::json!(trajectory
        .run_events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                AgentRunEventKind::ApprovalRequested | AgentRunEventKind::ApprovalResolved
            )
        })
        .map(|event| {
            serde_json::json!({
                "kind": event.kind.as_str(),
                "status": event.status,
                "decision": event.payload.get("decision").cloned(),
            })
        })
        .collect::<Vec<_>>())
}

fn task_orchestration_signature(trajectory: &Trajectory) -> serde_json::Value {
    serde_json::json!({
        "queueItems": trajectory.task_queue_items.iter().map(|item| {
            serde_json::json!({
                "queueId": item.queue_id,
                "taskDefinitionId": item.task_definition_id,
                "state": item.state,
                "triggerKind": item.trigger_kind,
            })
        }).collect::<Vec<_>>(),
        "runs": trajectory.task_runs.iter().map(|run| {
            serde_json::json!({
                "runId": run.run_id,
                "taskRunId": run.task_run_id,
                "taskDefinitionId": run.task_definition_id,
                "kind": run.kind,
                "state": run.status.state,
                "rawStatus": run.status.raw_status,
            })
        }).collect::<Vec<_>>(),
        "schedulerEvents": trajectory.scheduler_events.iter().map(|event| {
            serde_json::json!({
                "automationId": event.automation_id,
                "runId": event.run_id,
                "eventType": event.event_type,
                "status": event.status,
            })
        }).collect::<Vec<_>>(),
    })
}

fn evidence_ids(trajectory: &Trajectory) -> Vec<String> {
    trajectory
        .retrieved_evidence
        .iter()
        .filter_map(|value| {
            first_string_field(value, &["id", "chunkId", "documentId", "sourceId"])
                .or_else(|| nested_string_field(value, &["chunk", "id"]))
                .or_else(|| nested_string_field(value, &["document", "id"]))
        })
        .collect()
}

fn tool_name_from_value(value: &serde_json::Value) -> Option<String> {
    first_string_field(value, &["toolName", "name"])
        .or_else(|| nested_string_field(value, &["run", "toolName"]))
        .or_else(|| nested_string_field(value, &["function", "name"]))
        .or_else(|| nested_string_field(value, &["payload", "toolName"]))
        .or_else(|| nested_string_field(value, &["payload", "run", "toolName"]))
}

fn tool_call_id_from_value(value: &serde_json::Value, index: usize) -> String {
    first_string_field(value, &["callId", "id"])
        .or_else(|| nested_string_field(value, &["run", "callId"]))
        .or_else(|| nested_string_field(value, &["payload", "callId"]))
        .or_else(|| nested_string_field(value, &["payload", "run", "callId"]))
        .unwrap_or_else(|| format!("mock-call-{}", index + 1))
}

fn tool_arguments_from_value(value: &serde_json::Value) -> String {
    first_string_field(value, &["arguments", "argumentsPreview"])
        .or_else(|| nested_string_field(value, &["payload", "arguments"]))
        .or_else(|| nested_string_field(value, &["payload", "request", "argumentsPreview"]))
        .or_else(|| nested_string_field(value, &["run", "arguments"]))
        .unwrap_or_else(|| {
            first_value_field(value, &["args"])
                .or_else(|| nested_value(value, &["payload", "args"]))
                .and_then(|args| serde_json::to_string(args).ok())
                .unwrap_or_else(|| "{}".to_string())
        })
}

fn tool_result_content_from_value(value: &serde_json::Value, tool_name: &str) -> String {
    first_string_field(value, &["content", "result", "output"])
        .or_else(|| nested_string_field(value, &["payload", "content"]))
        .or_else(|| nested_string_field(value, &["run", "content"]))
        .or_else(|| nested_string_field(value, &["payload", "run", "content"]))
        .unwrap_or_else(|| format!("mock result for {tool_name}"))
}

fn tool_artifacts_from_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    first_value_field(value, &["artifacts"])
        .or_else(|| nested_value(value, &["payload", "artifacts"]))
        .or_else(|| nested_value(value, &["run", "artifacts"]))
        .or_else(|| nested_value(value, &["payload", "run", "artifacts"]))
        .cloned()
}

fn tool_call_is_error(value: &serde_json::Value) -> bool {
    first_bool_field(value, &["isError"])
        .or_else(|| nested_bool_field(value, &["payload", "isError"]))
        .or_else(|| nested_bool_field(value, &["run", "isError"]))
        .or_else(|| nested_bool_field(value, &["payload", "run", "isError"]))
        .unwrap_or_else(|| {
            first_string_field(value, &["status"])
                .or_else(|| nested_string_field(value, &["payload", "status"]))
                .or_else(|| nested_string_field(value, &["run", "status"]))
                .or_else(|| nested_string_field(value, &["payload", "run", "status"]))
                .map(|status| matches!(status.to_ascii_lowercase().as_str(), "failed" | "error"))
                .unwrap_or(false)
        })
}

fn approval_request_id_from_value(value: &serde_json::Value, index: usize) -> String {
    first_string_field(value, &["requestId", "id"])
        .or_else(|| nested_string_field(value, &["request", "id"]))
        .or_else(|| nested_string_field(value, &["payload", "requestId"]))
        .or_else(|| nested_string_field(value, &["payload", "request", "id"]))
        .unwrap_or_else(|| format!("mock-approval-{}", index + 1))
}

fn approval_arguments_from_value(value: &serde_json::Value) -> serde_json::Value {
    first_value_field(value, &["arguments", "args"])
        .or_else(|| nested_value(value, &["request", "arguments"]))
        .or_else(|| nested_value(value, &["payload", "arguments"]))
        .or_else(|| nested_value(value, &["payload", "request", "arguments"]))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "source": "trajectoryReplay" }))
}

fn approval_decision_from_value(value: &serde_json::Value) -> ApprovalDecision {
    let decision = first_string_field(value, &["decision", "status"])
        .or_else(|| nested_string_field(value, &["payload", "decision"]))
        .or_else(|| nested_string_field(value, &["payload", "status"]))
        .unwrap_or_default()
        .to_ascii_lowercase();

    match decision.as_str() {
        "allow_session" => ApprovalDecision::AllowSession,
        "never" => ApprovalDecision::Never,
        "deny" | "denied" | "declined" | "rejected" => ApprovalDecision::Deny,
        "allow_once" | "allow" | "allowed" | "approved" | "approval" | "completed" => {
            ApprovalDecision::AllowOnce
        }
        _ => ApprovalDecision::AllowOnce,
    }
}

fn approval_risk_from_value(value: &serde_json::Value) -> ApprovalRisk {
    let risk = first_string_field(value, &["riskLevel", "risk"])
        .or_else(|| nested_string_field(value, &["request", "riskLevel"]))
        .or_else(|| nested_string_field(value, &["payload", "request", "riskLevel"]))
        .unwrap_or_default()
        .to_ascii_lowercase();

    match risk.as_str() {
        "low" => ApprovalRisk::Low,
        "high" => ApprovalRisk::High,
        _ => ApprovalRisk::Medium,
    }
}

fn mock_terminal_status_from_trajectory(trajectory: &Trajectory) -> RuntimeTerminalStatus {
    let outcome = trajectory
        .outcome
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    match outcome.as_str() {
        "failed" | "failure" | "error" => RuntimeTerminalStatus::Failed,
        "cancelled" | "canceled" => RuntimeTerminalStatus::Cancelled,
        "timed_out" | "timeout" | "timedout" => RuntimeTerminalStatus::TimedOut,
        _ => RuntimeTerminalStatus::Completed,
    }
}

fn first_string_field(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .filter_map(|field| value.get(*field)?.as_str())
        .find(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn first_bool_field(value: &serde_json::Value, fields: &[&str]) -> Option<bool> {
    fields
        .iter()
        .filter_map(|field| value.get(*field)?.as_bool())
        .next()
}

fn first_value_field<'a>(
    value: &'a serde_json::Value,
    fields: &[&str],
) -> Option<&'a serde_json::Value> {
    fields.iter().filter_map(|field| value.get(*field)).next()
}

fn nested_bool_field(value: &serde_json::Value, path: &[&str]) -> Option<bool> {
    nested_value(value, path)?.as_bool()
}

fn nested_value<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn nested_string_field(value: &serde_json::Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn normalize_optional_text(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn replay_input_text(trajectory: &Trajectory) -> String {
    trajectory
        .raw_user_input
        .as_deref()
        .filter(|input| !input.trim().is_empty())
        .or_else(|| {
            if trajectory.user_input_summary.trim().is_empty() {
                None
            } else {
                Some(trajectory.user_input_summary.as_str())
            }
        })
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::agent::AgentEvent;
    use crate::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunPhase};
    use crate::error::CoreError;
    use crate::llm::{Message, Role, Usage};
    use crate::runtime::{
        AgentSession, AgentSessionConfig, AgentTurnInput, AgentTurnState, RuntimeTerminalStatus,
    };
    use crate::task_orchestrator::{
        TaskOrchestratorQueueItem, TaskOrchestratorState, TASK_ORCHESTRATOR_CONTRACT_VERSION,
    };
    use crate::trajectory::{Trajectory, TrajectoryRedactionProfile, TrajectoryStoreSummary};
    use crate::workflow_automation::WorkflowAutomationSchedulerEvent;

    fn queue_item() -> TaskOrchestratorQueueItem {
        TaskOrchestratorQueueItem {
            version: TASK_ORCHESTRATOR_CONTRACT_VERSION,
            queue_id: "workflow_due:automation-1".to_string(),
            task_definition_id: "automation-1".to_string(),
            state: TaskOrchestratorState::Queued,
            ownership: Default::default(),
            trigger_kind: "schedule".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
            prompt: "Run workflow.".to_string(),
            approval_required: true,
            allowed_tools: Vec::new(),
            risk_level: Some("medium".to_string()),
        }
    }

    fn replay_trajectory(id: &str) -> Trajectory {
        let mut trajectory =
            Trajectory::new(id, "2026-06-03T00:00:00Z", AgentSessionConfig::default());
        trajectory.run_events = vec![
            AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                1,
                AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ),
            AgentRunEvent::from_agent_event(&AgentEvent::Done {
                message: Message::text(Role::Assistant, "done"),
                usage_total: Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".to_string()),
            })
            .with_context(Some("run-1"), Some("turn-1"), Some(2)),
        ];
        trajectory.tool_calls = vec![serde_json::json!({ "toolName": "search" })];
        trajectory.approvals = vec![serde_json::json!({ "decision": "approved" })];
        trajectory.retrieved_evidence = vec![serde_json::json!({ "chunkId": "chunk-1" })];
        trajectory.final_answer = Some("done".to_string());
        trajectory.outcome = Some("completed".to_string());
        trajectory.refresh_metrics();
        trajectory
    }

    fn trajectory_summary(trajectory: &Trajectory) -> TrajectoryStoreSummary {
        TrajectoryStoreSummary {
            trajectory_id: trajectory.trajectory_id.clone(),
            schema_version: trajectory.schema_version,
            source_kind: "manual".to_string(),
            source_run_id: None,
            user_input_summary: trajectory.user_input_summary.clone(),
            outcome: trajectory.outcome.clone(),
            event_count: trajectory.metrics.event_count,
            tool_call_count: trajectory.metrics.tool_call_count,
            approval_count: trajectory.metrics.approval_count,
            task_run_count: trajectory.metrics.task_run_count,
            redaction_profile: TrajectoryRedactionProfile::FullLocalPrivate,
            created_at: trajectory.created_at.clone(),
            updated_at: trajectory.created_at.clone(),
        }
    }

    #[derive(Default)]
    struct MemoryTrajectoryStore {
        trajectories: HashMap<String, Trajectory>,
    }

    #[async_trait::async_trait]
    impl crate::trajectory::TrajectoryStore for MemoryTrajectoryStore {
        async fn save_trajectory(&self, _trajectory: Trajectory) -> Result<(), CoreError> {
            Ok(())
        }

        async fn load_trajectory(&self, trajectory_id: &str) -> Result<Trajectory, CoreError> {
            self.trajectories
                .get(trajectory_id)
                .cloned()
                .ok_or_else(|| CoreError::NotFound(format!("Trajectory {trajectory_id}")))
        }

        async fn list_trajectory_summaries(
            &self,
            limit: usize,
        ) -> Result<Vec<TrajectoryStoreSummary>, CoreError> {
            let mut trajectories = self.trajectories.values().collect::<Vec<_>>();
            trajectories.sort_by(|left, right| left.trajectory_id.cmp(&right.trajectory_id));
            Ok(trajectories
                .into_iter()
                .take(limit)
                .map(trajectory_summary)
                .collect())
        }
    }

    #[test]
    fn eval_contract_passes_event_order_and_final_answer() {
        let mut trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        trajectory.run_events = vec![
            AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                1,
                AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ),
            AgentRunEvent::from_agent_event(&AgentEvent::Done {
                message: Message::text(Role::Assistant, "done"),
                usage_total: Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".to_string()),
            })
            .with_context(Some("run-1"), Some("turn-1"), Some(2)),
        ];
        trajectory.final_answer = Some("done".to_string());

        let pack = EvalPack {
            version: EVAL_HARNESS_CONTRACT_VERSION,
            id: "pack-1".to_string(),
            name: "Pack".to_string(),
            cases: Vec::new(),
        };
        let case = EvalCase {
            id: "case-1".to_string(),
            name: "Case".to_string(),
            trajectory_id: Some("traj-1".to_string()),
            assertions: vec![
                EvalAssertion {
                    kind: EvalAssertionKind::EventOrder,
                    description: "events are ordered".to_string(),
                },
                EvalAssertion {
                    kind: EvalAssertionKind::FinalAnswerContract,
                    description: "final answer exists".to_string(),
                },
            ],
            allowed_nondeterminism: Vec::new(),
        };

        let report = evaluate_trajectory_contract(&pack, &case, &trajectory);

        assert!(report.passed, "{:?}", report.failures);
    }

    #[test]
    fn eval_contract_reports_missing_evidence() {
        let trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        let pack = EvalPack {
            version: EVAL_HARNESS_CONTRACT_VERSION,
            id: "pack-1".to_string(),
            name: "Pack".to_string(),
            cases: Vec::new(),
        };
        let case = EvalCase {
            id: "case-1".to_string(),
            name: "Case".to_string(),
            trajectory_id: Some("traj-1".to_string()),
            assertions: vec![EvalAssertion {
                kind: EvalAssertionKind::EvidenceSupport,
                description: "evidence exists".to_string(),
            }],
            allowed_nondeterminism: Vec::new(),
        };

        let report = evaluate_trajectory_contract(&pack, &case, &trajectory);

        assert!(!report.passed);
        assert_eq!(
            report.failures[0].assertion,
            EvalAssertionKind::EvidenceSupport
        );
    }

    #[test]
    fn eval_contract_checks_task_orchestration_context() {
        let mut trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        let pack = EvalPack {
            version: EVAL_HARNESS_CONTRACT_VERSION,
            id: "pack-1".to_string(),
            name: "Pack".to_string(),
            cases: Vec::new(),
        };
        let case = EvalCase {
            id: "case-1".to_string(),
            name: "Case".to_string(),
            trajectory_id: Some("traj-1".to_string()),
            assertions: vec![EvalAssertion {
                kind: EvalAssertionKind::TaskOrchestration,
                description: "task orchestration context exists".to_string(),
            }],
            allowed_nondeterminism: Vec::new(),
        };

        let missing = evaluate_trajectory_contract(&pack, &case, &trajectory);
        assert!(!missing.passed);
        assert_eq!(
            missing.failures[0].assertion,
            EvalAssertionKind::TaskOrchestration
        );

        trajectory
            .scheduler_events
            .push(WorkflowAutomationSchedulerEvent {
                id: "scheduler-event-1".to_string(),
                automation_id: Some("automation-1".to_string()),
                run_id: Some("workflow-run-1".to_string()),
                event_type: "launch_succeeded".to_string(),
                status: Some("running".to_string()),
                summary: "Scheduler launched due workflow".to_string(),
                payload: serde_json::json!({ "queueId": "workflow_due:automation-1" }),
                created_at: "2026-06-03T00:00:00Z".to_string(),
            });
        let scheduler_context = evaluate_trajectory_contract(&pack, &case, &trajectory);
        assert!(scheduler_context.passed, "{:?}", scheduler_context.failures);

        trajectory.task_queue_items.push(queue_item());
        let queue_context = evaluate_trajectory_contract(&pack, &case, &trajectory);
        assert!(queue_context.passed, "{:?}", queue_context.failures);
    }

    #[tokio::test]
    async fn eval_pack_loads_trajectories_from_store() {
        let mut trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        trajectory.final_answer = Some("done".to_string());

        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("traj-1".to_string(), trajectory)]),
        };
        let pack = EvalPack {
            version: EVAL_HARNESS_CONTRACT_VERSION,
            id: "pack-1".to_string(),
            name: "Pack".to_string(),
            cases: vec![EvalCase {
                id: "case-1".to_string(),
                name: "Case".to_string(),
                trajectory_id: Some("traj-1".to_string()),
                assertions: vec![EvalAssertion {
                    kind: EvalAssertionKind::FinalAnswerContract,
                    description: "final answer exists".to_string(),
                }],
                allowed_nondeterminism: Vec::new(),
            }],
        };

        let report = evaluate_pack_from_store(&store, &pack).await.unwrap();

        assert!(report.passed, "{:?}", report.failures);
    }

    #[tokio::test]
    async fn eval_pack_reports_missing_trajectory_fixture() {
        let store = MemoryTrajectoryStore::default();
        let pack = EvalPack {
            version: EVAL_HARNESS_CONTRACT_VERSION,
            id: "pack-1".to_string(),
            name: "Pack".to_string(),
            cases: vec![EvalCase {
                id: "case-1".to_string(),
                name: "Case".to_string(),
                trajectory_id: Some("missing".to_string()),
                assertions: vec![EvalAssertion {
                    kind: EvalAssertionKind::FinalAnswerContract,
                    description: "final answer exists".to_string(),
                }],
                allowed_nondeterminism: Vec::new(),
            }],
        };

        let report = evaluate_pack_from_store(&store, &pack).await.unwrap();

        assert!(!report.passed);
        assert_eq!(
            report.failures[0].assertion,
            EvalAssertionKind::TrajectoryAvailability
        );
        assert!(report.failures[0].message.contains("failed to load"));
    }

    #[test]
    fn trajectory_replay_equivalence_passes_matching_stable_signals() {
        let expected = replay_trajectory("expected");
        let replayed = replay_trajectory("replayed");

        let report = compare_trajectory_replay(&expected, &replayed, &[]);

        assert!(report.passed, "{:?}", report.mismatches);
    }

    #[test]
    fn trajectory_replay_equivalence_reports_tool_sequence_mismatch() {
        let expected = replay_trajectory("expected");
        let mut replayed = replay_trajectory("replayed");
        replayed.tool_calls = vec![serde_json::json!({ "toolName": "write_file" })];
        replayed.refresh_metrics();

        let report = compare_trajectory_replay(
            &expected,
            &replayed,
            &[TrajectoryReplayCheck::ToolCallSequence],
        );

        assert!(!report.passed);
        assert_eq!(
            report.mismatches[0].check,
            TrajectoryReplayCheck::ToolCallSequence
        );
        assert_eq!(report.mismatches[0].expected, serde_json::json!(["search"]));
        assert_eq!(
            report.mismatches[0].actual,
            serde_json::json!(["write_file"])
        );
    }

    #[tokio::test]
    async fn trajectory_replay_equivalence_loads_expected_and_replayed_from_store() {
        let expected = replay_trajectory("expected");
        let replayed = replay_trajectory("replayed");
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([
                ("expected".to_string(), expected),
                ("replayed".to_string(), replayed),
            ]),
        };
        let request = TrajectoryReplayRequest {
            expected_trajectory_id: "expected".to_string(),
            replayed_trajectory_id: "replayed".to_string(),
            checks: Vec::new(),
        };

        let report = evaluate_replay_from_store(&store, &request).await.unwrap();

        assert!(report.passed, "{:?}", report.mismatches);
        assert_eq!(report.expected_trajectory_id, "expected");
        assert_eq!(report.replayed_trajectory_id, "replayed");
    }

    #[tokio::test]
    async fn replay_agent_session_replays_trajectory_through_runtime_interface() {
        let mut trajectory = replay_trajectory("traj-1");
        trajectory.raw_user_input = Some("hello".to_string());
        let mut session = ReplayAgentSession::new(trajectory);

        let handle = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .unwrap();
        let events = session.read_events(&handle.run_id).await.unwrap();

        assert_eq!(handle.run_id, "run-1");
        assert_eq!(handle.turn_id, "turn-1");
        assert_eq!(
            handle.state,
            AgentTurnState::Terminal(RuntimeTerminalStatus::Completed)
        );
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn replay_agent_session_executes_mock_runtime_adapter_from_stable_signals() {
        let mut trajectory = replay_trajectory("traj-1");
        trajectory.raw_user_input = Some("hello".to_string());
        trajectory.run_events.clear();
        let mut session = ReplayAgentSession::mock_runtime(trajectory);

        let handle = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .unwrap();
        let events = session.read_events(&handle.run_id).await.unwrap();
        let report = validate_runtime_turn_events(&events).unwrap();

        assert_eq!(handle.run_id, "mock-run-traj-1");
        assert_eq!(handle.turn_id, "mock-turn-traj-1");
        assert_eq!(
            handle.state,
            AgentTurnState::Terminal(RuntimeTerminalStatus::Completed)
        );
        assert_eq!(report.terminal_status, RuntimeTerminalStatus::Completed);
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                AgentRunEventKind::Status,
                AgentRunEventKind::ApprovalRequested,
                AgentRunEventKind::ApprovalResolved,
                AgentRunEventKind::ToolStarted,
                AgentRunEventKind::ToolCompleted,
                AgentRunEventKind::Done,
            ]
        );
        assert_eq!(
            events[4]
                .payload
                .get("toolName")
                .and_then(|value| value.as_str()),
            Some("search")
        );
    }

    #[tokio::test]
    async fn replay_agent_session_mock_runtime_projects_failed_outcomes() {
        let mut trajectory = replay_trajectory("failed-traj");
        trajectory.raw_user_input = Some("hello".to_string());
        trajectory.run_events.clear();
        trajectory.approvals.clear();
        trajectory.tool_calls = vec![serde_json::json!({
            "toolName": "search",
            "status": "Failed"
        })];
        trajectory.final_answer = Some("tool failed".to_string());
        trajectory.outcome = Some("failed".to_string());
        trajectory.refresh_metrics();
        let mut session = ReplayAgentSession::mock_runtime(trajectory);

        let handle = session
            .start_turn(AgentTurnInput::text("hello"))
            .await
            .unwrap();
        let events = session.read_events(&handle.run_id).await.unwrap();
        let report = validate_runtime_turn_events(&events).unwrap();

        assert_eq!(
            handle.state,
            AgentTurnState::Terminal(RuntimeTerminalStatus::Failed)
        );
        assert_eq!(report.terminal_status, RuntimeTerminalStatus::Failed);
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                AgentRunEventKind::Status,
                AgentRunEventKind::ToolStarted,
                AgentRunEventKind::ToolCompleted,
                AgentRunEventKind::Error,
            ]
        );
        assert_eq!(
            events[2]
                .payload
                .get("isError")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn replay_agent_session_rejects_mismatched_raw_user_input() {
        let mut trajectory = replay_trajectory("traj-1");
        trajectory.raw_user_input = Some("hello".to_string());
        let mut session = ReplayAgentSession::new(trajectory);

        let err = session
            .start_turn(AgentTurnInput::text("different"))
            .await
            .unwrap_err();

        assert!(matches!(err, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn replay_trajectory_from_store_returns_execution_summary() {
        let mut trajectory = replay_trajectory("traj-1");
        trajectory.raw_user_input = Some("hello".to_string());
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("traj-1".to_string(), trajectory)]),
        };

        let execution = replay_trajectory_from_store(&store, "traj-1")
            .await
            .unwrap();

        assert_eq!(execution.trajectory_id, "traj-1");
        assert_eq!(execution.run_id, "run-1");
        assert_eq!(execution.turn_id, "turn-1");
        assert_eq!(
            execution.runtime_mode,
            TrajectoryReplayRuntimeMode::RecordedEvents
        );
        assert_eq!(execution.terminal_status, RuntimeTerminalStatus::Completed);
        assert_eq!(execution.event_count, 2);
        assert_eq!(execution.final_message.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn replay_trajectory_from_store_can_use_mock_runtime_adapter() {
        let mut trajectory = replay_trajectory("traj-1");
        trajectory.raw_user_input = Some("hello".to_string());
        trajectory.run_events.clear();
        trajectory.refresh_metrics();
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("traj-1".to_string(), trajectory)]),
        };

        let execution = replay_trajectory_from_store_with_runtime_mode(
            &store,
            "traj-1",
            TrajectoryReplayRuntimeMode::MockRuntime,
        )
        .await
        .unwrap();

        assert_eq!(execution.trajectory_id, "traj-1");
        assert_eq!(
            execution.runtime_mode,
            TrajectoryReplayRuntimeMode::MockRuntime
        );
        assert_eq!(execution.run_id, "mock-run-traj-1");
        assert_eq!(execution.turn_id, "mock-turn-traj-1");
        assert_eq!(execution.terminal_status, RuntimeTerminalStatus::Completed);
        assert_eq!(execution.event_count, 6);
        assert_eq!(execution.final_message.as_deref(), Some("done"));
        validate_runtime_turn_events(&execution.events).unwrap();
    }

    #[tokio::test]
    async fn stored_trajectory_smoke_eval_replays_listed_trajectories() {
        let mut first = replay_trajectory("traj-1");
        first.raw_user_input = Some("hello".to_string());
        first.user_input_summary = "hello".to_string();
        let mut second = replay_trajectory("traj-2");
        second.raw_user_input = Some("follow up".to_string());
        second.user_input_summary = "follow up".to_string();
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([
                ("traj-1".to_string(), first),
                ("traj-2".to_string(), second),
            ]),
        };

        let report = run_stored_trajectory_smoke_eval(&store, 10).await.unwrap();

        assert_eq!(report.status, "passed");
        assert_eq!(report.total, 2);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(report.cases[0].trajectory_id, "traj-1");
        assert_eq!(report.cases[0].replay_event_count, Some(2));
        assert_eq!(
            report.cases[0].replay_terminal_status,
            Some(RuntimeTerminalStatus::Completed)
        );
    }

    #[tokio::test]
    async fn stored_trajectory_smoke_eval_reports_invalid_runtime_contract() {
        let mut broken = replay_trajectory("broken");
        broken.run_events.pop();
        broken.refresh_metrics();
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("broken".to_string(), broken)]),
        };

        let report = run_stored_trajectory_smoke_eval(&store, 10).await.unwrap();

        assert_eq!(report.status, "failed");
        assert_eq!(report.total, 1);
        assert_eq!(report.failed, 1);
        assert!(!report.cases[0].passed);
        assert!(report.cases[0]
            .failures
            .iter()
            .any(|failure| failure.assertion == EvalAssertionKind::EventOrder));
    }

    #[tokio::test]
    async fn developer_eval_smoke_workflow_combines_quality_and_trajectory_harness() {
        let trajectory = replay_trajectory("developer-smoke-trajectory");
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("developer-smoke-trajectory".to_string(), trajectory)]),
        };

        let report = run_developer_eval_smoke_workflow(&store, 10).await.unwrap();

        assert_eq!(report.profile, DeveloperEvalWorkflowProfile::Smoke);
        assert_eq!(report.trajectory_limit, 10);
        assert_eq!(report.status, "passed");
        assert_eq!(report.failed, 0);
        assert!(report.quality_eval.gate.passed);
        assert_eq!(report.stored_trajectory_eval.status, "passed");
        assert_eq!(report.stored_trajectory_eval.total, 1);
    }

    #[tokio::test]
    async fn developer_eval_nightly_workflow_uses_nightly_profile_limit() {
        let trajectory = replay_trajectory("developer-nightly-trajectory");
        let store = MemoryTrajectoryStore {
            trajectories: HashMap::from([("developer-nightly-trajectory".to_string(), trajectory)]),
        };

        let report = run_developer_eval_nightly_workflow(&store).await.unwrap();

        assert_eq!(report.profile, DeveloperEvalWorkflowProfile::Nightly);
        assert_eq!(
            report.trajectory_limit,
            DEVELOPER_EVAL_NIGHTLY_TRAJECTORY_LIMIT
        );
        assert_eq!(report.status, "passed");
        assert_eq!(report.failed, 0);
        assert_eq!(report.stored_trajectory_eval.total, 1);
    }
}
