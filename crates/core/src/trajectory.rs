//! Versioned trajectory records for replay and evaluation.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::agent_run::{AgentRunEvent, AgentRunEventContractError, AgentRunEventKind};
use crate::conversation::{AgentTaskRun, ConversationMessage};
use crate::db::Database;
use crate::error::CoreError;
use crate::runtime::AgentSessionConfig;
use crate::task_orchestrator::{
    agent_task_run_projection, workflow_automation_run_projection, TaskOrchestratorQueueItem,
    TaskOrchestratorRun,
};
use crate::trace::AgentTrace;

pub const TRAJECTORY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryRedactionProfile {
    FullLocalPrivate,
    SanitizedLocal,
    ShareableMinimal,
    EvalFixture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectorySanitizationReport {
    pub profile: TrajectoryRedactionProfile,
    #[serde(default)]
    pub redacted_fields: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryMetrics {
    #[serde(default)]
    pub event_count: usize,
    #[serde(default)]
    pub tool_call_count: usize,
    #[serde(default)]
    pub approval_count: usize,
    #[serde(default)]
    pub task_queue_item_count: usize,
    #[serde(default)]
    pub task_run_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trajectory {
    pub trajectory_id: String,
    pub schema_version: u16,
    pub created_at: String,
    #[serde(default)]
    pub product_version: Option<String>,
    pub session_config: AgentSessionConfig,
    #[serde(default)]
    pub user_input_summary: String,
    #[serde(default)]
    pub raw_user_input: Option<String>,
    #[serde(default)]
    pub tools_offered: Vec<String>,
    #[serde(default)]
    pub skills_available: Vec<String>,
    #[serde(default)]
    pub skills_activated: Vec<String>,
    #[serde(default)]
    pub approvals: Vec<serde_json::Value>,
    #[serde(default)]
    pub task_queue_items: Vec<TaskOrchestratorQueueItem>,
    #[serde(default)]
    pub task_runs: Vec<TaskOrchestratorRun>,
    #[serde(default)]
    pub run_events: Vec<AgentRunEvent>,
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
    #[serde(default)]
    pub retrieved_evidence: Vec<serde_json::Value>,
    #[serde(default)]
    pub final_answer: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    pub metrics: TrajectoryMetrics,
    pub sanitization: TrajectorySanitizationReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrajectoryStoreSummary {
    pub trajectory_id: String,
    pub schema_version: u16,
    pub source_kind: String,
    #[serde(default)]
    pub source_run_id: Option<String>,
    pub user_input_summary: String,
    #[serde(default)]
    pub outcome: Option<String>,
    pub event_count: usize,
    pub tool_call_count: usize,
    pub approval_count: usize,
    pub task_run_count: usize,
    pub redaction_profile: TrajectoryRedactionProfile,
    pub created_at: String,
    pub updated_at: String,
}

impl Trajectory {
    pub fn new(
        trajectory_id: impl Into<String>,
        created_at: impl Into<String>,
        session_config: AgentSessionConfig,
    ) -> Self {
        Self {
            trajectory_id: trajectory_id.into(),
            schema_version: TRAJECTORY_SCHEMA_VERSION,
            created_at: created_at.into(),
            product_version: None,
            session_config,
            user_input_summary: String::new(),
            raw_user_input: None,
            tools_offered: Vec::new(),
            skills_available: Vec::new(),
            skills_activated: Vec::new(),
            approvals: Vec::new(),
            task_queue_items: Vec::new(),
            task_runs: Vec::new(),
            run_events: Vec::new(),
            tool_calls: Vec::new(),
            retrieved_evidence: Vec::new(),
            final_answer: None,
            outcome: None,
            metrics: TrajectoryMetrics::default(),
            sanitization: TrajectorySanitizationReport {
                profile: TrajectoryRedactionProfile::FullLocalPrivate,
                redacted_fields: Vec::new(),
            },
        }
    }

    pub fn refresh_metrics(&mut self) {
        self.metrics.event_count = self.run_events.len();
        self.metrics.tool_call_count = self.tool_calls.len();
        self.metrics.approval_count = self.approvals.len();
        self.metrics.task_queue_item_count = self.task_queue_items.len();
        self.metrics.task_run_count = self.task_runs.len();
    }

    pub fn validate_run_events(&self) -> Result<(), AgentRunEventContractError> {
        for event in &self.run_events {
            event.validate_durable_contract()?;
        }
        Ok(())
    }

    pub fn sanitized(mut self, profile: TrajectoryRedactionProfile) -> Self {
        let mut redacted = Vec::new();
        match profile {
            TrajectoryRedactionProfile::FullLocalPrivate => {}
            TrajectoryRedactionProfile::SanitizedLocal => {
                if self.raw_user_input.take().is_some() {
                    redacted.push("rawUserInput".to_string());
                }
            }
            TrajectoryRedactionProfile::ShareableMinimal
            | TrajectoryRedactionProfile::EvalFixture => {
                if self.raw_user_input.take().is_some() {
                    redacted.push("rawUserInput".to_string());
                }
                if !self.retrieved_evidence.is_empty() {
                    self.retrieved_evidence.clear();
                    redacted.push("retrievedEvidence".to_string());
                }
                if self.final_answer.take().is_some() {
                    redacted.push("finalAnswer".to_string());
                }
                if profile == TrajectoryRedactionProfile::ShareableMinimal
                    && self
                        .task_queue_items
                        .iter()
                        .any(|item| !item.prompt.is_empty())
                {
                    for item in &mut self.task_queue_items {
                        item.prompt.clear();
                    }
                    redacted.push("taskQueueItems.prompt".to_string());
                }
            }
        }
        self.sanitization = TrajectorySanitizationReport {
            profile,
            redacted_fields: redacted,
        };
        self.refresh_metrics();
        self
    }
}

pub fn trajectory_source_identity(trajectory_id: &str) -> (String, Option<String>) {
    match trajectory_id.split_once(':') {
        Some((kind, id)) if !kind.trim().is_empty() && !id.trim().is_empty() => {
            (kind.to_string(), Some(id.to_string()))
        }
        _ => ("manual".to_string(), None),
    }
}

#[async_trait]
pub trait TrajectoryStore: Send + Sync {
    async fn save_trajectory(&self, trajectory: Trajectory) -> Result<(), CoreError>;

    async fn load_trajectory(&self, trajectory_id: &str) -> Result<Trajectory, CoreError>;

    async fn list_trajectory_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<TrajectoryStoreSummary>, CoreError>;
}

pub fn export_agent_task_run_trajectory(
    db: &Database,
    run_id: &str,
    redaction_profile: TrajectoryRedactionProfile,
) -> Result<Trajectory, CoreError> {
    let run = db.get_agent_task_run(run_id)?;
    let turn = db.get_conversation_turn(&run.turn_id).ok();
    let messages = db.get_messages(&run.conversation_id)?;
    let user_message = messages
        .iter()
        .find(|message| message.id == run.user_message_id);
    let raw_user_input = user_message
        .map(|message| message.content.clone())
        .filter(|content| !content.trim().is_empty());
    let traces = db.get_agent_traces(&run.conversation_id)?;
    let trace = matching_agent_trace(traces, &run, raw_user_input.as_deref());

    let mut session_config = session_config_from_task_artifacts(run.artifacts.as_ref())?
        .unwrap_or_else(|| fallback_session_config(&run));
    complete_session_config_from_run(&mut session_config, &run);

    let run_events = db.list_agent_run_events(&run.id)?;
    let mut trajectory = Trajectory::new(
        format!("agent_task_run:{}", run.id),
        run.created_at.clone(),
        session_config,
    );
    trajectory.raw_user_input = raw_user_input.clone();
    trajectory.user_input_summary = raw_user_input
        .as_deref()
        .map(summarize_input)
        .unwrap_or_else(|| run.title.clone());
    trajectory.run_events = run_events;
    trajectory.final_answer = final_answer_from_run_events(&trajectory.run_events).or_else(|| {
        final_answer_from_messages(
            &messages,
            turn.as_ref()
                .and_then(|turn| turn.assistant_message_id.as_deref()),
        )
    });
    trajectory.outcome = trace
        .as_ref()
        .map(|trace| trace.outcome.to_string())
        .or_else(|| Some(run.status.clone()));
    trajectory.tools_offered = trace.as_ref().map(tools_from_trace).unwrap_or_default();
    trajectory.tool_calls = tool_call_records_from_run_events(&trajectory.run_events);
    if trajectory.tool_calls.is_empty() {
        if let Some(trace) = &trace {
            trajectory.tool_calls = tool_call_records_from_trace(trace);
        }
    }
    trajectory.approvals = event_records_by_kind(
        &trajectory.run_events,
        &[
            AgentRunEventKind::ApprovalRequested,
            AgentRunEventKind::ApprovalResolved,
        ],
    );
    trajectory.retrieved_evidence = evidence_records_from_artifacts(
        run.artifacts.as_ref(),
        turn.as_ref().and_then(|t| t.trace.as_ref()),
    );
    trajectory.task_runs.push(
        agent_task_run_projection(
            &run,
            trajectory.session_config.source_scope.source_ids.clone(),
        )
        .map_err(|err| CoreError::Internal(format!("project agent task run: {err}")))?,
    );
    trajectory.refresh_metrics();
    trajectory.validate_run_events().map_err(|err| {
        CoreError::Internal(format!("exported trajectory has invalid run event: {err}"))
    })?;
    Ok(trajectory.sanitized(redaction_profile))
}

pub fn export_workflow_automation_run_trajectory(
    db: &Database,
    workflow_run_id: &str,
    redaction_profile: TrajectoryRedactionProfile,
) -> Result<Trajectory, CoreError> {
    let workflow_run = db.get_workflow_automation_run(workflow_run_id)?;
    let automation = db.get_workflow_automation(&workflow_run.automation_id)?;
    let workflow_projection = workflow_automation_run_projection(&automation, &workflow_run)
        .map_err(|err| CoreError::Internal(format!("project workflow automation run: {err}")))?;

    let mut trajectory = match workflow_run.task_run_id.as_deref() {
        Some(task_run_id) => export_agent_task_run_trajectory(db, task_run_id, redaction_profile)?,
        None => {
            let mut config = AgentSessionConfig::default();
            config.source_scope.source_ids = automation.source_scope.clone();
            config.metadata = serde_json::json!({
                "kind": "workflowAutomationRun",
                "automationId": &automation.id,
                "workflowTemplateId": &automation.workflow_template_id,
                "triggerKind": &automation.trigger_kind,
            });
            let mut trajectory = Trajectory::new(
                format!("workflow_automation_run:{}", workflow_run.id),
                workflow_run.created_at.clone(),
                config,
            );
            trajectory.user_input_summary = workflow_run
                .summary
                .clone()
                .unwrap_or_else(|| summarize_workflow_automation(&automation));
            trajectory.outcome = Some(workflow_run.status.clone());
            trajectory.sanitized(redaction_profile)
        }
    };

    trajectory.trajectory_id = format!("workflow_automation_run:{}", workflow_run.id);
    trajectory.created_at = workflow_run.created_at.clone();
    trajectory.outcome = Some(workflow_run.status.clone());
    if !trajectory
        .task_runs
        .iter()
        .any(|run| run.run_id == workflow_projection.run_id)
    {
        trajectory.task_runs.insert(0, workflow_projection);
    }
    trajectory.refresh_metrics();
    Ok(trajectory)
}

fn summarize_workflow_automation(
    automation: &crate::workflow_automation::WorkflowAutomation,
) -> String {
    let name = automation.name.trim();
    let description = automation.description.trim();
    if description.is_empty() {
        format!("Workflow automation: {name}")
    } else {
        summarize_input(&format!("Workflow automation: {name}. {description}"))
    }
}

fn session_config_from_task_artifacts(
    artifacts: Option<&serde_json::Value>,
) -> Result<Option<AgentSessionConfig>, CoreError> {
    let Some(artifacts) = artifacts else {
        return Ok(None);
    };
    let config_value = artifacts
        .get("runtimeSession")
        .and_then(|value| value.get("config"))
        .or_else(|| {
            if artifacts.get("kind").and_then(|value| value.as_str()) == Some("agentSessionConfig")
            {
                artifacts.get("config")
            } else {
                None
            }
        });
    match config_value {
        Some(value) => AgentSessionConfig::from_versioned_json(value.clone())
            .map(Some)
            .map_err(|err| CoreError::Internal(format!("deserialize agent session config: {err}"))),
        None => Ok(None),
    }
}

fn fallback_session_config(run: &AgentTaskRun) -> AgentSessionConfig {
    AgentSessionConfig {
        session_id: run.conversation_id.clone(),
        conversation_id: Some(run.conversation_id.clone()),
        task_run_id: Some(run.id.clone()),
        provider: run.provider.clone(),
        model: run.model.clone(),
        ..Default::default()
    }
}

fn complete_session_config_from_run(config: &mut AgentSessionConfig, run: &AgentTaskRun) {
    if config.session_id.trim().is_empty() {
        config.session_id = run.conversation_id.clone();
    }
    if config.conversation_id.is_none() {
        config.conversation_id = Some(run.conversation_id.clone());
    }
    if config.task_run_id.is_none() {
        config.task_run_id = Some(run.id.clone());
    }
    if config.provider.is_none() {
        config.provider = run.provider.clone();
    }
    if config.model.is_none() {
        config.model = run.model.clone();
    }
    config.apply_protocol_defaults();
}

fn matching_agent_trace(
    mut traces: Vec<AgentTrace>,
    run: &AgentTaskRun,
    raw_user_input: Option<&str>,
) -> Option<AgentTrace> {
    if let Some(index) = traces
        .iter()
        .rposition(|trace| agent_trace_matches_task(trace, run, raw_user_input))
    {
        return Some(traces.swap_remove(index));
    }
    if let Some(model) = &run.model {
        if let Some(index) = traces.iter().rposition(|trace| trace.model_id == *model) {
            return Some(traces.swap_remove(index));
        }
    }
    traces.pop()
}

fn agent_trace_matches_task(
    trace: &AgentTrace,
    run: &AgentTaskRun,
    raw_user_input: Option<&str>,
) -> bool {
    let model_matches = run
        .model
        .as_deref()
        .map(|model| model == trace.model_id)
        .unwrap_or(true);
    let input_matches = raw_user_input
        .map(|input| {
            input.starts_with(&trace.user_message_preview)
                || trace.user_message_preview.starts_with(input)
        })
        .unwrap_or_else(|| trace.user_message_preview == run.title);
    model_matches && input_matches
}

fn summarize_input(input: &str) -> String {
    let single_line = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= 200 {
        return single_line;
    }
    let mut summary = single_line.chars().take(197).collect::<String>();
    summary.push_str("...");
    summary
}

fn final_answer_from_run_events(events: &[AgentRunEvent]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|event| event.kind == AgentRunEventKind::Done)
        .and_then(|event| event.payload.get("message"))
        .and_then(message_text_from_value)
}

fn final_answer_from_messages(
    messages: &[ConversationMessage],
    assistant_message_id: Option<&str>,
) -> Option<String> {
    let assistant_message_id = assistant_message_id?;
    messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .map(|message| message.content.clone())
        .filter(|content| !content.trim().is_empty())
}

fn message_text_from_value(message: &serde_json::Value) -> Option<String> {
    let parts = message.get("parts")?.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn event_records_by_kind(
    events: &[AgentRunEvent],
    kinds: &[AgentRunEventKind],
) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|event| kinds.contains(&event.kind))
        .map(event_record)
        .collect()
}

fn tool_call_records_from_run_events(events: &[AgentRunEvent]) -> Vec<serde_json::Value> {
    let mut records = BTreeMap::new();
    for event in events {
        if !matches!(
            event.kind,
            AgentRunEventKind::ToolStarted | AgentRunEventKind::ToolCompleted
        ) {
            continue;
        }
        records.insert(tool_call_key(event), event_record(event));
    }
    records.into_values().collect()
}

fn tool_call_key(event: &AgentRunEvent) -> String {
    event
        .payload
        .get("run")
        .and_then(|run| run.get("callId"))
        .and_then(|value| value.as_str())
        .or_else(|| event.payload.get("callId").and_then(|value| value.as_str()))
        .map(str::to_string)
        .unwrap_or_else(|| format!("event:{}", event.event_seq))
}

fn event_record(event: &AgentRunEvent) -> serde_json::Value {
    serde_json::json!({
        "source": "agentRunEvent",
        "eventSeq": event.event_seq,
        "kind": event.kind.as_str(),
        "phase": event.phase.as_str(),
        "label": &event.label,
        "status": &event.status,
        "payload": &event.payload,
    })
}

fn tool_call_records_from_trace(trace: &AgentTrace) -> Vec<serde_json::Value> {
    trace
        .steps
        .iter()
        .filter_map(|step| {
            let tool_name = step.tool_name.as_ref()?;
            Some(serde_json::json!({
                "source": "agentTrace",
                "iteration": step.iteration,
                "toolName": tool_name,
                "durationMs": step.tool_duration_ms,
                "inputTokens": step.input_tokens,
                "outputTokens": step.output_tokens,
            }))
        })
        .collect()
}

fn tools_from_trace(trace: &AgentTrace) -> Vec<String> {
    trace
        .steps
        .iter()
        .filter_map(|step| step.tool_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn evidence_records_from_artifacts(
    task_artifacts: Option<&serde_json::Value>,
    turn_trace: Option<&serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(artifacts) = task_artifacts {
        push_verification_artifact(&mut records, &mut seen, "taskArtifacts", artifacts);
        if let Some(trace) = artifacts.get("trace").and_then(|value| value.get("trace")) {
            push_verification_artifact(&mut records, &mut seen, "taskArtifacts.trace", trace);
        }
    }
    if let Some(trace) = turn_trace {
        push_verification_artifact(&mut records, &mut seen, "conversationTurn.trace", trace);
    }
    records
}

fn push_verification_artifact(
    records: &mut Vec<serde_json::Value>,
    seen: &mut BTreeSet<String>,
    source: &str,
    value: &serde_json::Value,
) {
    let candidate = if value.get("kind").and_then(|kind| kind.as_str()) == Some("verification") {
        Some(value)
    } else {
        value.get("verification")
    };
    let Some(artifact) = candidate else {
        return;
    };
    let key = serde_json::to_string(artifact).unwrap_or_else(|_| source.to_string());
    if seen.insert(key) {
        records.push(serde_json::json!({
            "source": source,
            "kind": "verification",
            "artifact": artifact,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_run::{
        AgentRunEvent, AgentRunEventKind, AgentRunPhase, AGENT_RUN_EVENT_VERSION,
    };
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::db::Database;
    use crate::llm::Role;
    use crate::runtime::RuntimeSourceScope;
    use crate::task_orchestrator::{
        TaskOrchestratorQueueItem, TaskOrchestratorRunKind, TaskOrchestratorState,
    };
    use crate::trace::{AgentTrace, TraceOutcome, TraceStep};
    use crate::workflow_automation::{
        SaveWorkflowAutomationInput, WorkflowAutomationApprovalPolicy, WorkflowAutomationTrigger,
    };

    fn queue_item() -> TaskOrchestratorQueueItem {
        TaskOrchestratorQueueItem {
            version: crate::task_orchestrator::TASK_ORCHESTRATOR_CONTRACT_VERSION,
            queue_id: "workflow_due:automation-1".to_string(),
            task_definition_id: "automation-1".to_string(),
            state: TaskOrchestratorState::Queued,
            ownership: Default::default(),
            trigger_kind: "schedule".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
            prompt: "Private workflow prompt".to_string(),
            approval_required: true,
            allowed_tools: Vec::new(),
            risk_level: Some("medium".to_string()),
        }
    }

    #[test]
    fn trajectory_metrics_are_derived_from_recorded_behavior() {
        let mut trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        trajectory.run_events.push(AgentRunEvent::status_update(
            "run-1",
            Some("turn-1"),
            1,
            AgentRunPhase::Routing,
            "Route selected: Direct",
            Some("running"),
            None,
        ));
        trajectory
            .tool_calls
            .push(serde_json::json!({ "tool": "search" }));
        trajectory
            .approvals
            .push(serde_json::json!({ "id": "approval-1" }));
        trajectory.task_queue_items.push(queue_item());

        trajectory.refresh_metrics();

        assert_eq!(trajectory.metrics.event_count, 1);
        assert_eq!(trajectory.metrics.tool_call_count, 1);
        assert_eq!(trajectory.metrics.approval_count, 1);
        assert_eq!(trajectory.metrics.task_queue_item_count, 1);
    }

    #[test]
    fn shareable_redaction_removes_sensitive_fields() {
        let mut trajectory = Trajectory::new(
            "traj-1",
            "2026-06-03T00:00:00Z",
            AgentSessionConfig::default(),
        );
        trajectory.raw_user_input = Some("private prompt".to_string());
        trajectory.final_answer = Some("private answer".to_string());
        trajectory.retrieved_evidence = vec![serde_json::json!({ "path": "private.md" })];
        trajectory.task_queue_items = vec![queue_item()];

        let redacted = trajectory.sanitized(TrajectoryRedactionProfile::ShareableMinimal);

        assert!(redacted.raw_user_input.is_none());
        assert!(redacted.final_answer.is_none());
        assert!(redacted.retrieved_evidence.is_empty());
        assert_eq!(redacted.task_queue_items[0].prompt, "");
        assert!(redacted
            .sanitization
            .redacted_fields
            .contains(&"rawUserInput".to_string()));
        assert!(redacted
            .sanitization
            .redacted_fields
            .contains(&"taskQueueItems.prompt".to_string()));
    }

    #[test]
    fn exports_agent_task_run_trajectory_from_persisted_runtime_records() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user_message = ConversationMessage {
            id: "user-message-1".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Find the cited source and summarize it.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 8,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user_message).unwrap();
        let turn = db
            .create_conversation_turn(
                &conversation.id,
                &user_message.id,
                Some("KnowledgeRetrieval"),
            )
            .unwrap();
        let assistant_message = ConversationMessage {
            id: "assistant-message-1".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::Assistant,
            content: "The source says the workflow should cite evidence [1].".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 11,
            created_at: String::new(),
            sort_order: 1,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&assistant_message).unwrap();

        let task_run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user_message.id,
                "Find the cited source and summarize it.",
                Some("openai"),
                Some("gpt-4o"),
            )
            .unwrap();

        let mut session_config = AgentSessionConfig::default();
        session_config.session_id = conversation.id.clone();
        session_config.conversation_id = Some(conversation.id.clone());
        session_config.task_run_id = Some(task_run.id.clone());
        session_config.provider = Some("openai".to_string());
        session_config.model = Some("gpt-4o".to_string());
        session_config.source_scope = RuntimeSourceScope {
            source_ids: vec!["source-1".to_string()],
            collection_id: None,
            working_directory: None,
        };
        session_config.skill_context.available_skill_ids = vec!["research".to_string()];
        session_config.skill_context.loaded_skill_ids = vec!["research".to_string()];
        let verification = serde_json::json!({
            "kind": "verification",
            "overallStatus": "passed",
        });
        let artifacts = serde_json::json!({
            "runtimeSession": {
                "kind": "agentSessionConfig",
                "version": 1,
                "config": session_config,
            },
            "verification": verification,
        });
        db.finish_agent_task_run(
            &task_run.id,
            "completed",
            Some("Task completed"),
            None,
            Some(&artifacts),
        )
        .unwrap();
        db.finalize_conversation_turn(
            &turn.id,
            "success",
            Some(&assistant_message.id),
            Some(&serde_json::json!({
                "kind": "turnTrace",
                "routeKind": "KnowledgeRetrieval",
                "verification": verification,
            })),
        )
        .unwrap();

        let mut trace =
            AgentTrace::begin(&conversation.id, &user_message.content, "gpt-4o", 128_000);
        trace.add_step(TraceStep {
            iteration: 0,
            tool_name: Some("retrieve_evidence".to_string()),
            tool_duration_ms: Some(24),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: None,
            cache_miss_tokens: None,
            cache_creation_tokens: None,
            context_usage_pct: 0.2,
            was_compacted: false,
        });
        trace.finish(TraceOutcome::Success, None);
        db.save_agent_trace(&trace).unwrap();

        db.save_agent_run_events(&[
            AgentRunEvent::status_update(
                &task_run.id,
                Some(&turn.id),
                1,
                AgentRunPhase::Routing,
                "Route selected: KnowledgeRetrieval",
                Some("running"),
                None,
            ),
            AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: task_run.id.clone(),
                turn_id: turn.id.clone(),
                event_seq: 2,
                kind: AgentRunEventKind::ApprovalRequested,
                phase: AgentRunPhase::Approval,
                label: "retrieve_evidence".to_string(),
                status: Some("pending".to_string()),
                payload: serde_json::json!({
                    "request": {
                        "id": "approval-1",
                        "toolName": "retrieve_evidence",
                    },
                }),
                created_at: None,
            },
            AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: task_run.id.clone(),
                turn_id: turn.id.clone(),
                event_seq: 3,
                kind: AgentRunEventKind::ToolCompleted,
                phase: AgentRunPhase::Tooling,
                label: "retrieve_evidence".to_string(),
                status: Some("completed".to_string()),
                payload: serde_json::json!({
                    "callId": "call-1",
                    "toolName": "retrieve_evidence",
                    "content": "Evidence card [1]",
                    "isError": false,
                }),
                created_at: None,
            },
            AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: task_run.id.clone(),
                turn_id: turn.id.clone(),
                event_seq: 4,
                kind: AgentRunEventKind::Done,
                phase: AgentRunPhase::Done,
                label: "Final answer produced".to_string(),
                status: Some("completed".to_string()),
                payload: serde_json::json!({
                    "message": {
                        "role": "assistant",
                        "parts": [{ "type": "text", "text": "The source says the workflow should cite evidence [1]." }],
                    },
                    "usageTotal": {
                        "promptTokens": 10,
                        "completionTokens": 20,
                        "totalTokens": 30,
                    },
                }),
                created_at: None,
            },
        ])
        .unwrap();

        let trajectory = export_agent_task_run_trajectory(
            &db,
            &task_run.id,
            TrajectoryRedactionProfile::FullLocalPrivate,
        )
        .unwrap();

        assert_eq!(
            trajectory.raw_user_input.as_deref(),
            Some("Find the cited source and summarize it.")
        );
        assert_eq!(
            trajectory.session_config.task_run_id,
            Some(task_run.id.clone())
        );
        assert_eq!(
            trajectory.session_config.source_scope.source_ids,
            vec!["source-1".to_string()]
        );
        assert_eq!(
            trajectory.tools_offered,
            vec!["retrieve_evidence".to_string()]
        );
        assert_eq!(trajectory.metrics.event_count, 4);
        assert_eq!(trajectory.metrics.tool_call_count, 1);
        assert_eq!(trajectory.metrics.approval_count, 1);
        assert_eq!(trajectory.metrics.task_run_count, 1);
        assert_eq!(
            trajectory.final_answer.as_deref(),
            Some("The source says the workflow should cite evidence [1].")
        );
        assert_eq!(trajectory.outcome.as_deref(), Some("success"));
        assert_eq!(trajectory.retrieved_evidence.len(), 1);
        assert_eq!(
            trajectory.task_runs[0].status.state,
            TaskOrchestratorState::Completed
        );

        let redacted = export_agent_task_run_trajectory(
            &db,
            &task_run.id,
            TrajectoryRedactionProfile::ShareableMinimal,
        )
        .unwrap();
        assert!(redacted.raw_user_input.is_none());
        assert!(redacted.final_answer.is_none());
        assert!(redacted.retrieved_evidence.is_empty());
    }

    #[test]
    fn exports_workflow_run_trajectory_with_task_orchestrator_projection() {
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation(&SaveWorkflowAutomationInput {
                id: None,
                name: "Daily evidence report".to_string(),
                description: "Summarize evidence every day.".to_string(),
                workflow_template_id: "report_brief".to_string(),
                prompt: "Summarize evidence.".to_string(),
                trigger: WorkflowAutomationTrigger::Manual,
                source_scope: vec!["source-1".to_string()],
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: vec!["retrieve_evidence".to_string()],
                    risk_level: "medium".to_string(),
                },
                enabled: true,
            })
            .unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user_message = ConversationMessage {
            id: "workflow-user-message-1".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Run the daily evidence report.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 6,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user_message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &user_message.id, Some("workflow"))
            .unwrap();
        let task_run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user_message.id,
                "Run the daily evidence report.",
                Some("openai"),
                Some("gpt-4o"),
            )
            .unwrap();
        let workflow_run = db
            .record_workflow_automation_run(
                &automation.id,
                Some(&task_run.id),
                "running",
                Some("Workflow is running"),
            )
            .unwrap();

        let trajectory = export_workflow_automation_run_trajectory(
            &db,
            &workflow_run.id,
            TrajectoryRedactionProfile::FullLocalPrivate,
        )
        .unwrap();

        assert_eq!(
            trajectory.trajectory_id,
            format!("workflow_automation_run:{}", workflow_run.id)
        );
        assert_eq!(
            trajectory.raw_user_input.as_deref(),
            Some("Run the daily evidence report.")
        );
        assert_eq!(trajectory.outcome.as_deref(), Some("running"));
        assert_eq!(trajectory.metrics.task_run_count, 2);
        assert_eq!(
            trajectory.task_runs[0].kind,
            TaskOrchestratorRunKind::WorkflowAutomation
        );
        assert_eq!(trajectory.task_runs[0].run_id, workflow_run.id);
        assert_eq!(
            trajectory.task_runs[0].task_run_id,
            Some(task_run.id.clone())
        );
        assert_eq!(
            trajectory.task_runs[0].ownership.source_scope,
            vec!["source-1".to_string()]
        );
        assert_eq!(
            trajectory.task_runs[1].kind,
            TaskOrchestratorRunKind::AgentTask
        );
    }
}
