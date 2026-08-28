//! Product-level workflow automation, resumability, governance, and evidence graph contracts.
//!
//! This module is intentionally product-facing. It keeps the durable records
//! that let the desktop app expose scheduled workflows, pause/resume state,
//! learning governance, investigation graphs, and read-only browser evidence
//! without making those concepts depend on a specific chat UI.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use globset::{Glob, GlobSetBuilder};
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::agent_run::{AgentRunEvent, AgentRunEventKind};
use crate::conversation::{
    AgentTaskArtifact, AgentTaskArtifactSummary, AgentTaskRun, AgentTurnLaunchRecord,
    ConversationMessage,
};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::Role;
use crate::workflow_scheduler::{
    latest_workflow_cron_occurrence_at_or_before, next_workflow_cron_occurrence,
    WorkflowAutomationScheduleConfig, WorkflowScheduleMisfirePolicy, WorkflowScheduleOverlapPolicy,
    WorkflowScheduleWorkspacePolicy,
};

const AUTOMATION_NAME_MAX_CHARS: usize = 160;
const AUTOMATION_DESCRIPTION_MAX_CHARS: usize = 2_000;
const AUTOMATION_PROMPT_MAX_CHARS: usize = 12_000;
const RESUME_PROMPT_MAX_STATE_CHARS: usize = 7_000;
const RESUME_PARTIAL_OUTPUT_MAX_CHARS: usize = 24_000;
const SCHEDULER_RETRY_EVENT_LOOKBACK_LIMIT: usize = 50;
const SCHEDULER_RETRY_BACKOFF_SECONDS: [i64; 4] = [300, 900, 3_600, 14_400];
const SCHEDULER_RETRY_MAX_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowAutomationTrigger {
    Manual,
    Schedule { cron: String },
    Folder { path: String, pattern: String },
}

impl WorkflowAutomationTrigger {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule { .. } => "schedule",
            Self::Folder { .. } => "folder",
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Manual => "manual run".to_string(),
            Self::Schedule { cron } => format!("schedule {cron}"),
            Self::Folder { path, pattern } => format!("folder watch {path} ({pattern})"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationApprovalPolicy {
    pub require_before_run: bool,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    pub risk_level: String,
}

impl Default for WorkflowAutomationApprovalPolicy {
    fn default() -> Self {
        Self {
            require_before_run: true,
            allowed_tools: Vec::new(),
            risk_level: "medium".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWorkflowAutomationInput {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub workflow_template_id: String,
    pub prompt: String,
    pub trigger: WorkflowAutomationTrigger,
    #[serde(default)]
    pub source_scope: Vec<String>,
    #[serde(default)]
    pub approval_policy: WorkflowAutomationApprovalPolicy,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomation {
    pub id: String,
    pub name: String,
    pub description: String,
    pub workflow_template_id: String,
    pub prompt: String,
    pub trigger_kind: String,
    pub trigger: WorkflowAutomationTrigger,
    pub source_scope: Vec<String>,
    pub approval_policy: WorkflowAutomationApprovalPolicy,
    #[serde(default)]
    pub schedule_config: WorkflowAutomationScheduleConfig,
    pub enabled: bool,
    pub status: String,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationDueRun {
    pub automation: WorkflowAutomation,
    pub prompt: String,
    pub due_reason: String,
    pub scheduled_for: Option<String>,
    #[serde(default)]
    pub origin: WorkflowAutomationOccurrenceOrigin,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAutomationOccurrenceOrigin {
    #[default]
    Schedule,
    ManualRunNow,
}

impl WorkflowAutomationOccurrenceOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::ManualRunNow => "manual_run_now",
        }
    }

    fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "schedule" => Ok(Self::Schedule),
            "manual_run_now" => Ok(Self::ManualRunNow),
            other => Err(CoreError::Internal(format!(
                "Unknown workflow occurrence origin '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationDueRunClaim {
    pub due_run: WorkflowAutomationDueRun,
    pub occurrence: Option<WorkflowAutomationOccurrence>,
    pub run: Option<WorkflowAutomationRun>,
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAutomationApprovalState {
    NotRequired,
    Pending,
    Approved,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAutomationRunStatus {
    Draft,
    Queued,
    Running,
    WaitingApproval,
    Paused,
    Resuming,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Disabled,
    Cancelling,
}

impl WorkflowAutomationRunStatus {
    pub fn parse(value: &str) -> Result<Self, CoreError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "draft" | "ready" => Ok(Self::Draft),
            "queued" | "pending" => Ok(Self::Queued),
            "running" | "initializing" | "in_progress" => Ok(Self::Running),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "paused" => Ok(Self::Paused),
            "resuming" => Ok(Self::Resuming),
            "completed" | "cached" | "done" => Ok(Self::Completed),
            "failed" | "error" => Ok(Self::Failed),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            "timed_out" | "timeout" => Ok(Self::TimedOut),
            "disabled" => Ok(Self::Disabled),
            "cancelling" => Ok(Self::Cancelling),
            other => Err(CoreError::InvalidInput(format!(
                "Unknown workflow run status '{other}'"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Paused => "paused",
            Self::Resuming => "resuming",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Disabled => "disabled",
            Self::Cancelling => "cancelling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAutomationOccurrenceStatus {
    Planned,
    Claimed,
    RetryWait,
    WaitingApproval,
    Queued,
    Running,
    Paused,
    Resuming,
    Completed,
    Skipped,
    Failed,
    Cancelled,
    TimedOut,
    Disabled,
    Cancelling,
}

impl WorkflowAutomationOccurrenceStatus {
    fn parse(value: &str) -> Result<Self, CoreError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "planned" => Ok(Self::Planned),
            "claimed" => Ok(Self::Claimed),
            "retry_wait" => Ok(Self::RetryWait),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "queued" | "pending" => Ok(Self::Queued),
            "running" | "initializing" | "in_progress" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "resuming" => Ok(Self::Resuming),
            "completed" | "cached" | "done" => Ok(Self::Completed),
            "skipped" => Ok(Self::Skipped),
            "failed" | "error" => Ok(Self::Failed),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            "timed_out" | "timeout" => Ok(Self::TimedOut),
            "disabled" => Ok(Self::Disabled),
            "cancelling" => Ok(Self::Cancelling),
            other => Err(CoreError::InvalidInput(format!(
                "Unknown workflow occurrence status '{other}'"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Claimed => "claimed",
            Self::RetryWait => "retry_wait",
            Self::WaitingApproval => "waiting_approval",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Resuming => "resuming",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Disabled => "disabled",
            Self::Cancelling => "cancelling",
        }
    }
}

impl WorkflowAutomationApprovalState {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }

    fn from_str(value: &str) -> Result<Self, CoreError> {
        match value {
            "not_required" => Ok(Self::NotRequired),
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            other => Err(CoreError::Internal(format!(
                "Unknown workflow occurrence approval state '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationOccurrence {
    pub id: String,
    pub automation_id: String,
    pub definition_revision: u32,
    pub scheduled_for: String,
    pub status: WorkflowAutomationOccurrenceStatus,
    pub attempt_count: u32,
    pub retry_at: Option<String>,
    pub last_error: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationRun {
    pub id: String,
    pub automation_id: String,
    pub task_run_id: Option<String>,
    pub status: WorkflowAutomationRunStatus,
    pub summary: Option<String>,
    pub occurrence_id: Option<String>,
    pub scheduled_for: Option<String>,
    pub definition_revision: u32,
    pub attempt: u32,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationSchedulerEvent {
    pub id: String,
    pub automation_id: Option<String>,
    pub run_id: Option<String>,
    pub event_type: String,
    pub status: Option<String>,
    pub summary: String,
    pub payload: Value,
    pub created_at: String,
}

/// Canonical scheduler-event vocabulary shared by core and desktop adapters.
/// `as_str` values are durable database values and must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSchedulerEventType {
    DefinitionSuperseded,
    ApprovalRequested,
    ApprovalResolved,
    SkippedRetryLimit,
    SkippedBackoff,
    SkippedPreRunApproval,
    OccurrenceSkipped,
    ClaimFailed,
    SkippedNoAgentConfig,
    Claimed,
    LaunchSucceeded,
    LaunchFailed,
    SkippedActive,
}

impl WorkflowSchedulerEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefinitionSuperseded => "definition_superseded",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalResolved => "approval_resolved",
            Self::SkippedRetryLimit => "skipped_retry_limit",
            Self::SkippedBackoff => "skipped_backoff",
            Self::SkippedPreRunApproval => "skipped_pre_run_approval",
            Self::OccurrenceSkipped => "occurrence_skipped",
            Self::ClaimFailed => "claim_failed",
            Self::SkippedNoAgentConfig => "skipped_no_agent_config",
            Self::Claimed => "claimed",
            Self::LaunchSucceeded => "launch_succeeded",
            Self::LaunchFailed => "launch_failed",
            Self::SkippedActive => "skipped_active",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "definition_superseded" => Some(Self::DefinitionSuperseded),
            "approval_requested" => Some(Self::ApprovalRequested),
            "approval_resolved" => Some(Self::ApprovalResolved),
            "skipped_retry_limit" => Some(Self::SkippedRetryLimit),
            "skipped_backoff" => Some(Self::SkippedBackoff),
            "skipped_pre_run_approval" => Some(Self::SkippedPreRunApproval),
            "occurrence_skipped" => Some(Self::OccurrenceSkipped),
            "claim_failed" => Some(Self::ClaimFailed),
            "skipped_no_agent_config" => Some(Self::SkippedNoAgentConfig),
            "claimed" => Some(Self::Claimed),
            "launch_succeeded" => Some(Self::LaunchSucceeded),
            "launch_failed" => Some(Self::LaunchFailed),
            "skipped_active" => Some(Self::SkippedActive),
            _ => None,
        }
    }

    const fn is_retryable_failure(self) -> bool {
        matches!(self, Self::ClaimFailed | Self::LaunchFailed)
    }

    const fn is_retry_audit_only(self) -> bool {
        matches!(self, Self::SkippedBackoff | Self::SkippedRetryLimit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationSchedulerRetryDecision {
    pub allowed: bool,
    pub max_attempts: usize,
    pub attempts_exhausted: bool,
    pub retryable_failure_count: usize,
    pub last_retryable_event_type: Option<String>,
    pub last_retryable_event_at: Option<String>,
    pub backoff_seconds: Option<i64>,
    pub backoff_until: Option<String>,
    pub retry_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResumeCheckpoint {
    pub id: String,
    pub run_id: String,
    pub reason: String,
    pub status: String,
    pub phase: String,
    pub state: Value,
    pub resume_prompt: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResumePrompt {
    pub run: AgentTaskRun,
    pub checkpoint: TaskResumeCheckpoint,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSkillUsageInput {
    pub skill_id: String,
    pub conversation_id: Option<String>,
    pub task_run_id: Option<String>,
    pub outcome: String,
    #[serde(default)]
    pub evidence: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsageStats {
    pub skill_id: String,
    pub name: String,
    pub enabled: bool,
    pub usage_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub last_used_at: Option<String>,
    pub recent_failure_evidence: Option<Value>,
    pub disable_recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningGovernanceSnapshot {
    pub skill_stats: Vec<SkillUsageStats>,
    pub pending_proposals: u32,
    pub procedural_memory_count: u32,
    pub memory_injection_count: u32,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationGraph {
    pub run_id: String,
    pub nodes: Vec<InvestigationGraphNode>,
    pub edges: Vec<InvestigationGraphEdge>,
    pub citations: Vec<String>,
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationGraphNode {
    pub id: String,
    pub node_type: String,
    pub label: String,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub source_url: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationGraphEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEvidenceCapture {
    pub id: String,
    pub url: String,
    pub final_url: String,
    pub title: String,
    pub excerpt: String,
    pub method: String,
    pub payload: Value,
    pub created_at: String,
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn normalize_required(value: &str, field: &str, max_chars: usize) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(format!("{field} cannot be empty")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(CoreError::InvalidInput(format!(
            "{field} is too long (max {max_chars} chars)"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional(value: &str, max_chars: usize) -> Result<String, CoreError> {
    let trimmed = value.trim();
    if trimmed.chars().count() > max_chars {
        return Err(CoreError::InvalidInput(format!(
            "Text is too long (max {max_chars} chars)"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_string_list(values: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter_map(|value| {
            let normalized = value.replace('\\', "/");
            seen.insert(normalized.clone()).then_some(normalized)
        })
        .collect()
}

fn parse_json_or_default<T>(raw: String) -> T
where
    T: for<'de> Deserialize<'de> + Default,
{
    serde_json::from_str::<T>(&raw).unwrap_or_default()
}

fn workflow_automation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowAutomation> {
    let trigger_json: String = row.get(5)?;
    let trigger_kind: String = row.get(6)?;
    let source_scope_json: String = row.get(7)?;
    let approval_policy_json: String = row.get(8)?;
    let schedule_config_json: String = row.get(15)?;
    let trigger = serde_json::from_str::<WorkflowAutomationTrigger>(&trigger_json)
        .unwrap_or(WorkflowAutomationTrigger::Manual);
    let parsed_schedule_config =
        serde_json::from_str::<WorkflowAutomationScheduleConfig>(&schedule_config_json);
    let schedule_config_is_valid = if trigger_kind == "schedule" {
        !matches!(schedule_config_json.trim(), "" | "{}" | "null")
            && matches!(
                (&trigger, parsed_schedule_config.as_ref()),
                (WorkflowAutomationTrigger::Schedule { cron }, Ok(config))
                    if config.validate_for_save(cron).is_ok()
            )
    } else {
        true
    };
    let schedule_config = if schedule_config_is_valid {
        parsed_schedule_config.unwrap_or_default()
    } else {
        WorkflowAutomationScheduleConfig::legacy_utc_needs_review()
    };
    let persisted_enabled = row.get::<_, i64>(9)? != 0;
    let persisted_status: String = row.get(10)?;
    let persisted_next_run_at: Option<String> = row.get(12)?;
    Ok(WorkflowAutomation {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        workflow_template_id: row.get(3)?,
        prompt: row.get(4)?,
        trigger_kind,
        trigger,
        source_scope: parse_json_or_default::<Vec<String>>(source_scope_json),
        approval_policy: parse_json_or_default::<WorkflowAutomationApprovalPolicy>(
            approval_policy_json,
        ),
        schedule_config,
        enabled: persisted_enabled && schedule_config_is_valid,
        status: if schedule_config_is_valid {
            persisted_status
        } else {
            "needs_review".into()
        },
        last_run_at: row.get(11)?,
        next_run_at: schedule_config_is_valid
            .then_some(persisted_next_run_at)
            .flatten(),
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn task_resume_checkpoint_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TaskResumeCheckpoint> {
    let state_json: String = row.get(5)?;
    Ok(TaskResumeCheckpoint {
        id: row.get(0)?,
        run_id: row.get(1)?,
        reason: row.get(2)?,
        status: row.get(3)?,
        phase: row.get(4)?,
        state: serde_json::from_str::<Value>(&state_json).unwrap_or_else(|_| serde_json::json!({})),
        resume_prompt: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn browser_evidence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrowserEvidenceCapture> {
    let payload_json: String = row.get(6)?;
    Ok(BrowserEvidenceCapture {
        id: row.get(0)?,
        url: row.get(1)?,
        final_url: row.get(2)?,
        title: row.get(3)?,
        excerpt: row.get(4)?,
        method: row.get(5)?,
        payload: serde_json::from_str::<Value>(&payload_json)
            .unwrap_or_else(|_| serde_json::json!({})),
        created_at: row.get(7)?,
    })
}

fn workflow_scheduler_event_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkflowAutomationSchedulerEvent> {
    let payload_json: String = row.get(6)?;
    Ok(WorkflowAutomationSchedulerEvent {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        run_id: row.get(2)?,
        event_type: row.get(3)?,
        status: row.get(4)?,
        summary: row.get(5)?,
        payload: serde_json::from_str::<Value>(&payload_json)
            .unwrap_or_else(|_| serde_json::json!({})),
        created_at: row.get(7)?,
    })
}

fn next_run_for_trigger(
    trigger: &WorkflowAutomationTrigger,
    schedule_config: &WorkflowAutomationScheduleConfig,
    enabled: bool,
    after: DateTime<Utc>,
) -> Result<Option<String>, CoreError> {
    let WorkflowAutomationTrigger::Schedule { cron } = trigger else {
        return Ok(None);
    };
    schedule_config.validate_for_save(cron)?;
    if !enabled {
        return Ok(None);
    }
    next_workflow_cron_occurrence(cron, &schedule_config.timezone, after)
        .map(|value| Some(value.to_rfc3339()))
}

fn workflow_automation_run_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkflowAutomationRun> {
    let status_raw: String = row.get(3)?;
    let status = WorkflowAutomationRunStatus::parse(&status_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    Ok(WorkflowAutomationRun {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        task_run_id: row.get(2)?,
        status,
        summary: row.get(4)?,
        created_at: row.get(5)?,
        finished_at: row.get(6)?,
        occurrence_id: row.get(7)?,
        scheduled_for: row.get(8)?,
        definition_revision: row.get::<_, i64>(9)?.max(1) as u32,
        attempt: row.get::<_, i64>(10)?.max(1) as u32,
    })
}

fn workflow_automation_occurrence_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkflowAutomationOccurrence> {
    let status_raw: String = row.get(4)?;
    let status = WorkflowAutomationOccurrenceStatus::parse(&status_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    Ok(WorkflowAutomationOccurrence {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        definition_revision: row.get::<_, i64>(2)?.max(1) as u32,
        scheduled_for: row.get(3)?,
        status,
        attempt_count: row.get::<_, i64>(5)?.max(0) as u32,
        retry_at: row.get(6)?,
        last_error: row.get(7)?,
        lease_token: row.get(8)?,
        lease_expires_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const WORKFLOW_RUN_SELECT: &str = "SELECT id, automation_id, task_run_id, status, summary, created_at, finished_at, occurrence_id, scheduled_for, definition_revision, attempt FROM workflow_automation_runs";
const WORKFLOW_OCCURRENCE_SELECT: &str = "SELECT id, automation_id, definition_revision, scheduled_for, status, attempt_count, retry_at, last_error, lease_token, lease_expires_at, created_at, updated_at FROM workflow_automation_occurrences";
const WORKFLOW_AUTOMATION_SELECT: &str = "SELECT id, name, description, workflow_template_id, prompt, trigger_json, trigger_kind, source_scope_json, approval_policy_json, enabled, status, last_run_at, next_run_at, created_at, updated_at, COALESCE((SELECT config_json FROM workflow_automation_schedule_configs c WHERE c.automation_id = workflow_automations.id), '{}') FROM workflow_automations";

fn fetch_workflow_run(
    conn: &rusqlite::Connection,
    run_id: &str,
) -> Result<WorkflowAutomationRun, CoreError> {
    conn.query_row(
        &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
        rusqlite::params![run_id],
        workflow_automation_run_from_row,
    )
    .map_err(CoreError::Database)
}

fn fetch_workflow_occurrence(
    conn: &rusqlite::Connection,
    occurrence_id: &str,
) -> Result<WorkflowAutomationOccurrence, CoreError> {
    conn.query_row(
        &format!("{WORKFLOW_OCCURRENCE_SELECT} WHERE id = ?1"),
        rusqlite::params![occurrence_id],
        workflow_automation_occurrence_from_row,
    )
    .map_err(CoreError::Database)
}

struct SchedulerEventRecord<'a> {
    automation_id: Option<&'a str>,
    run_id: Option<&'a str>,
    event_type: WorkflowSchedulerEventType,
    status: Option<&'a str>,
    summary: &'a str,
    payload: Option<&'a Value>,
}

fn insert_scheduler_event(
    tx: &rusqlite::Transaction<'_>,
    record: SchedulerEventRecord<'_>,
) -> Result<String, CoreError> {
    let summary = normalize_optional(record.summary, 2_000)?;
    let status = record
        .status
        .map(|value| normalize_optional(value, 120))
        .transpose()?
        .filter(|value| !value.is_empty());
    let payload = record
        .payload
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let payload_json = serde_json::to_string(&payload)?;
    let id = new_id();
    tx.execute(
        "INSERT INTO workflow_automation_scheduler_events
         (id, automation_id, run_id, event_type, status, summary, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            &id,
            record.automation_id,
            record.run_id,
            record.event_type.as_str(),
            status.as_deref(),
            &summary,
            &payload_json,
        ],
    )?;
    Ok(id)
}

fn parse_utc_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
        })
}

fn scheduler_retry_backoff_seconds(failure_count: usize) -> Option<i64> {
    if failure_count == 0 {
        return None;
    }
    SCHEDULER_RETRY_BACKOFF_SECONDS
        .get(failure_count.saturating_sub(1))
        .copied()
        .or_else(|| SCHEDULER_RETRY_BACKOFF_SECONDS.last().copied())
}

pub fn workflow_automation_scheduler_retry_decision_from_events(
    events: &[WorkflowAutomationSchedulerEvent],
    now_rfc3339: &str,
) -> Result<WorkflowAutomationSchedulerRetryDecision, CoreError> {
    let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
        CoreError::InvalidInput(format!(
            "Invalid scheduler retry decision timestamp '{now_rfc3339}'"
        ))
    })?;
    let mut ordered = Vec::with_capacity(events.len());
    for event in events {
        let created_at = parse_utc_timestamp(&event.created_at).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid scheduler event timestamp '{}'",
                event.created_at
            ))
        })?;
        ordered.push((created_at, event));
    }
    ordered.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| right.id.cmp(&left.id))
    });

    let mut retryable_failure_count = 0usize;
    let mut last_retryable_event_type = None;
    let mut last_retryable_event_at = None;
    let mut last_retryable_event_time = None;

    for (created_at, event) in ordered {
        let Some(event_type) = WorkflowSchedulerEventType::parse(&event.event_type) else {
            break;
        };
        if event_type.is_retry_audit_only() {
            continue;
        }
        if !event_type.is_retryable_failure() {
            break;
        }
        retryable_failure_count += 1;
        if last_retryable_event_type.is_none() {
            last_retryable_event_type = Some(event_type.as_str().to_string());
            last_retryable_event_at = Some(event.created_at.clone());
            last_retryable_event_time = Some(created_at);
        }
    }

    let Some(backoff_seconds) = scheduler_retry_backoff_seconds(retryable_failure_count) else {
        return Ok(WorkflowAutomationSchedulerRetryDecision {
            allowed: true,
            max_attempts: SCHEDULER_RETRY_MAX_ATTEMPTS,
            attempts_exhausted: false,
            retryable_failure_count,
            last_retryable_event_type,
            last_retryable_event_at,
            backoff_seconds: None,
            backoff_until: None,
            retry_after_seconds: None,
        });
    };
    let Some(last_failure_time) = last_retryable_event_time else {
        return Ok(WorkflowAutomationSchedulerRetryDecision {
            allowed: true,
            max_attempts: SCHEDULER_RETRY_MAX_ATTEMPTS,
            attempts_exhausted: false,
            retryable_failure_count: 0,
            last_retryable_event_type: None,
            last_retryable_event_at: None,
            backoff_seconds: None,
            backoff_until: None,
            retry_after_seconds: None,
        });
    };
    let attempts_exhausted = retryable_failure_count >= SCHEDULER_RETRY_MAX_ATTEMPTS;
    if attempts_exhausted {
        return Ok(WorkflowAutomationSchedulerRetryDecision {
            allowed: false,
            max_attempts: SCHEDULER_RETRY_MAX_ATTEMPTS,
            attempts_exhausted,
            retryable_failure_count,
            last_retryable_event_type,
            last_retryable_event_at,
            backoff_seconds: Some(backoff_seconds),
            backoff_until: None,
            retry_after_seconds: None,
        });
    }
    let backoff_until = last_failure_time + Duration::seconds(backoff_seconds);
    let retry_after_seconds = if now < backoff_until {
        Some((backoff_until - now).num_seconds().max(1))
    } else {
        None
    };

    Ok(WorkflowAutomationSchedulerRetryDecision {
        allowed: retry_after_seconds.is_none(),
        max_attempts: SCHEDULER_RETRY_MAX_ATTEMPTS,
        attempts_exhausted,
        retryable_failure_count,
        last_retryable_event_type,
        last_retryable_event_at,
        backoff_seconds: Some(backoff_seconds),
        backoff_until: Some(backoff_until.to_rfc3339()),
        retry_after_seconds,
    })
}

fn folder_trigger_due(
    trigger: &WorkflowAutomationTrigger,
    last_run_at: Option<&str>,
) -> Result<bool, CoreError> {
    let WorkflowAutomationTrigger::Folder { path, pattern } = trigger else {
        return Ok(false);
    };
    let root = Path::new(path);
    if !root.is_dir() {
        return Ok(false);
    }

    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern).map_err(|err| {
        CoreError::InvalidInput(format!("Invalid folder trigger pattern: {err}"))
    })?);
    if !pattern.contains('/') && !pattern.contains('\\') {
        builder.add(Glob::new(&format!("**/{pattern}")).map_err(|err| {
            CoreError::InvalidInput(format!("Invalid folder trigger pattern: {err}"))
        })?);
    }
    let globset = builder
        .build()
        .map_err(|err| CoreError::InvalidInput(format!("Invalid folder trigger pattern: {err}")))?;
    let since = last_run_at.and_then(parse_utc_timestamp);

    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !globset.is_match(relative) && !globset.is_match(entry.file_name()) {
            continue;
        }
        let Some(since) = since else {
            return Ok(true);
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let modified = DateTime::<Utc>::from(modified);
        if modified.timestamp() > since.timestamp() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn automation_prompt(automation: &WorkflowAutomation) -> String {
    let scope = if automation.source_scope.is_empty() {
        "Use the active conversation or user-selected sources.".to_string()
    } else {
        automation.source_scope.join(", ")
    };
    let approval = if automation.approval_policy.require_before_run {
        "Ask for approval before executing write, network, shell, or desktop actions."
    } else {
        "Use the saved approval policy for this automation."
    };
    let allowed_tools = if automation.approval_policy.allowed_tools.is_empty() {
        "No tool whitelist is pinned.".to_string()
    } else {
        automation.approval_policy.allowed_tools.join(", ")
    };
    format!(
        "Run the saved Nexa workflow automation.\n\nAutomation: {}\nTemplate: {}\nTrigger: {}\nSource scope: {}\nRisk: {}\nAllowed tools: {}\nApproval: {}\n\nGoal:\n{}",
        automation.name,
        automation.workflow_template_id,
        automation.trigger.label(),
        scope,
        automation.approval_policy.risk_level,
        allowed_tools,
        approval,
        automation.prompt
    )
}

fn compact_json(value: &Value, max_chars: usize) -> String {
    let serialized = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    if serialized.chars().count() <= max_chars {
        return serialized;
    }
    let mut out = serialized.chars().take(max_chars).collect::<String>();
    out.push_str("\n...truncated...");
    out
}

fn partial_assistant_output(events: &[AgentRunEvent]) -> Option<Value> {
    let mut order = Vec::<String>::new();
    let mut blocks = HashMap::<String, String>::new();
    for event in events {
        if event.kind == AgentRunEventKind::StreamReset {
            order.clear();
            blocks.clear();
            continue;
        }
        if event.kind != AgentRunEventKind::OutputDelta
            || event.payload.get("channel").and_then(Value::as_str) != Some("answer")
        {
            continue;
        }
        let Some(block_id) = event.payload.get("blockId").and_then(Value::as_str) else {
            continue;
        };
        let Some(delta) = event.payload.get("delta").and_then(Value::as_str) else {
            continue;
        };
        let offset = event
            .payload
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|offset| usize::try_from(offset).ok())
            .unwrap_or_default();
        if !blocks.contains_key(block_id) {
            if offset != 0 {
                continue;
            }
            order.push(block_id.to_string());
        }
        let block = blocks.entry(block_id.to_string()).or_default();
        if offset > block.len() || !block.is_char_boundary(offset) {
            continue;
        }
        block.truncate(offset);
        block.push_str(delta);
    }

    let output = order
        .into_iter()
        .filter_map(|block_id| blocks.remove(&block_id))
        .filter(|block| !block.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if output.trim().is_empty() {
        return None;
    }
    let char_count = output.chars().count();
    let truncated_prefix_chars = char_count.saturating_sub(RESUME_PARTIAL_OUTPUT_MAX_CHARS);
    let text = if truncated_prefix_chars == 0 {
        output
    } else {
        output.chars().skip(truncated_prefix_chars).collect()
    };
    Some(serde_json::json!({
        "text": text,
        "truncatedPrefixChars": truncated_prefix_chars,
    }))
}

fn build_resume_prompt(
    run: &AgentTaskRun,
    checkpoint_id: &str,
    reason: &str,
    state: &Value,
) -> String {
    let partial_output = state
        .get("partialAssistantOutput")
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(|text| {
            format!(
                "\n\nAssistant output already shown before the pause (continue after it without repeating it):\n<paused_assistant_output>\n{text}\n</paused_assistant_output>"
            )
        })
        .unwrap_or_default();
    let reconciliation = reason
        .strip_prefix("user_stop_requires_action_reconciliation:")
        .map(|activity_ids| {
            format!(
                "\n- SAFETY FENCE: interactive action receipt(s) {activity_ids} may have executed while the turn was stopping. Before any further browser/computer input, obtain a fresh observation and reconcile the visible state. Never redispatch the prior action merely because its tool result is absent."
            )
        })
        .unwrap_or_default();
    format!(
        "Resume this Nexa task from a durable checkpoint.\n\nTask: {}\nRun ID: {}\nCheckpoint ID: {}\nCheckpoint reason: {}\nPrevious status: {}\nPrevious phase: {}\nRoute: {}\nSummary: {}{}\n\nInstructions:\n- Start by naming the resumed checkpoint and the next unfinished phase.\n- Prefer liveTurnState.taskPlan when present; it is the freshest in-memory execution state captured at the checkpoint boundary.\n- Continue after partialAssistantOutput exactly where it stopped; do not repeat text already shown.\n- Continue from the checkpoint state instead of restarting completed work.\n- Do not redo completed tool work unless the checkpoint shows stale, failed, missing, or contradictory evidence.\n- Treat recentEvents and artifacts as durable pointers; inspect only the files, sources, or records needed for the next decision.\n- Reuse existing evidence and artifacts when they are still valid.\n- Re-check stale or missing evidence before making final claims.\n- Preserve the user's source scope and approval boundaries.\n- Run verification before the final answer, then say what was resumed and what still needs verification.{}\n\nCheckpoint state:\n{}",
        run.title,
        run.id,
        checkpoint_id,
        reason,
        run.status,
        run.phase,
        run.route_kind.as_deref().unwrap_or("unknown"),
        run.summary.as_deref().unwrap_or("No summary yet."),
        partial_output,
        reconciliation,
        compact_json(state, RESUME_PROMPT_MAX_STATE_CHARS)
    )
}

fn collect_string_field(value: &Value, key: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get(key).and_then(|item| item.as_str()) {
                if !text.trim().is_empty() {
                    out.insert(text.trim().to_string());
                }
            }
            for nested in map.values() {
                collect_string_field(nested, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_string_field(item, key, out);
            }
        }
        _ => {}
    }
}

fn collect_open_questions(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for key in ["openQuestions", "open_questions", "questions", "gaps"] {
                if let Some(Value::Array(items)) = map.get(key) {
                    for item in items {
                        if let Some(text) = item.as_str() {
                            if !text.trim().is_empty() {
                                out.insert(text.trim().to_string());
                            }
                        }
                    }
                }
            }
            for nested in map.values() {
                collect_open_questions(nested, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_open_questions(item, out);
            }
        }
        _ => {}
    }
}

fn collect_citations_from_text(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("[cite:") {
        let after = &rest[start..];
        if let Some(end) = after.find(']') {
            out.insert(after[..=end].to_string());
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
}

fn artifact_to_node(artifact: &AgentTaskArtifactSummary) -> InvestigationGraphNode {
    InvestigationGraphNode {
        id: format!("artifact:{}", artifact.id),
        node_type: "artifact".to_string(),
        label: artifact.title.clone(),
        summary: artifact.summary.clone(),
        status: Some(artifact.kind.clone()),
        source_url: None,
        created_at: Some(artifact.created_at.clone()),
    }
}

fn persisted_artifact_to_node(artifact: &AgentTaskArtifact) -> InvestigationGraphNode {
    InvestigationGraphNode {
        id: format!("persisted-artifact:{}", artifact.id),
        node_type: "artifact".to_string(),
        label: artifact.title.clone(),
        summary: artifact.summary.clone(),
        status: Some(artifact.kind.clone()),
        source_url: None,
        created_at: Some(artifact.updated_at.clone()),
    }
}

pub fn browser_evidence_payload(
    url: &str,
    final_url: &str,
    title: &str,
    excerpt: &str,
    method: &str,
) -> Value {
    serde_json::json!({
        "kind": "browserEvidence",
        "version": 1,
        "source": {
            "url": url,
            "finalUrl": final_url,
            "title": title,
        },
        "capture": {
            "method": method,
            "capturedAt": Utc::now().to_rfc3339(),
            "readOnly": true,
            "approvalScoped": true,
        },
        "evidence": {
            "excerpt": excerpt,
            "citation": format!("[cite:web:{}]", blake3::hash(final_url.as_bytes()).to_hex().chars().take(10).collect::<String>()),
        }
    })
}

impl Database {
    pub fn save_workflow_automation(
        &self,
        input: &SaveWorkflowAutomationInput,
    ) -> Result<WorkflowAutomation, CoreError> {
        self.save_workflow_automation_with_schedule_config(
            input,
            &WorkflowAutomationScheduleConfig::default(),
        )
    }

    pub fn save_workflow_automation_with_schedule_config(
        &self,
        input: &SaveWorkflowAutomationInput,
        schedule_config: &WorkflowAutomationScheduleConfig,
    ) -> Result<WorkflowAutomation, CoreError> {
        let name = normalize_required(&input.name, "Automation name", AUTOMATION_NAME_MAX_CHARS)?;
        let description = normalize_optional(&input.description, AUTOMATION_DESCRIPTION_MAX_CHARS)?;
        let workflow_template_id =
            normalize_required(&input.workflow_template_id, "Workflow template", 120)?;
        let prompt = normalize_required(
            &input.prompt,
            "Automation prompt",
            AUTOMATION_PROMPT_MAX_CHARS,
        )?;
        let source_scope = normalize_string_list(&input.source_scope);
        let trigger_json = serde_json::to_string(&input.trigger)?;
        let source_scope_json = serde_json::to_string(&source_scope)?;
        let approval_policy_json = serde_json::to_string(&input.approval_policy)?;
        let trigger_kind = input.trigger.kind();
        let next_run_at =
            next_run_for_trigger(&input.trigger, schedule_config, input.enabled, Utc::now())?;
        let schedule_config_json = serde_json::to_string(schedule_config)?;
        let enabled = if input.enabled { 1 } else { 0 };

        let id = input.id.clone().unwrap_or_else(new_id);
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_automations WHERE id = ?1)",
            rusqlite::params![&id],
            |row| row.get(0),
        )?;
        let is_schedule = matches!(&input.trigger, WorkflowAutomationTrigger::Schedule { .. });
        let previous_definition_revision = tx
            .query_row(
                "SELECT revision FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
                rusqlite::params![&id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let latest_definition_revision = tx.query_row(
            "SELECT MAX(revision) FROM workflow_automation_definition_revisions
             WHERE automation_id = ?1",
            rusqlite::params![&id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        let definition_revision = latest_definition_revision
            .map(|revision| revision.saturating_add(1))
            .unwrap_or(1);
        if exists {
            tx.execute(
                "UPDATE workflow_automations
                 SET name = ?2,
                     description = ?3,
                     workflow_template_id = ?4,
                     prompt = ?5,
                     trigger_json = ?6,
                     trigger_kind = ?7,
                     source_scope_json = ?8,
                     approval_policy_json = ?9,
                     enabled = ?10,
                     status = CASE WHEN ?10 = 1 THEN 'ready' ELSE 'disabled' END,
                     next_run_at = ?11,
                     updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![
                    &id,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    enabled,
                    &next_run_at,
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO workflow_automations
                 (id, name, description, workflow_template_id, prompt, trigger_json,
                  trigger_kind, source_scope_json, approval_policy_json, enabled, status, next_run_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         CASE WHEN ?10 = 1 THEN 'ready' ELSE 'disabled' END, ?11)",
                rusqlite::params![
                    &id,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    enabled,
                    &next_run_at,
                ],
            )?;
        }
        if is_schedule {
            tx.execute(
                "INSERT INTO workflow_automation_schedule_configs
                      (automation_id, config_json, revision, updated_at)
                  VALUES (?1, ?2, ?3, datetime('now'))
                  ON CONFLICT(automation_id) DO UPDATE SET
                      config_json = excluded.config_json,
                      revision = excluded.revision,
                      updated_at = datetime('now')",
                rusqlite::params![&id, &schedule_config_json, definition_revision],
            )?;
            tx.execute(
                "INSERT INTO workflow_automation_definition_revisions
                     (automation_id, revision, name, description, workflow_template_id,
                      prompt, trigger_json, trigger_kind, source_scope_json,
                      approval_policy_json, schedule_config_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    &id,
                    definition_revision,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    &schedule_config_json,
                ],
            )?;
        } else {
            tx.execute(
                "DELETE FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
                rusqlite::params![&id],
            )?;
        }
        if let Some(previous_revision) = previous_definition_revision {
            tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'cancelled', last_error = 'definition_superseded',
                     retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,
                     updated_at = datetime('now')
                 WHERE automation_id = ?1 AND definition_revision = ?2
                   AND status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval')",
                rusqlite::params![&id, previous_revision],
            )?;
            tx.execute(
                "UPDATE workflow_automation_runs
                 SET status = 'cancelled',
                     summary = COALESCE(summary, 'Definition superseded before execution'),
                     finished_at = COALESCE(finished_at, datetime('now'))
                 WHERE automation_id = ?1 AND definition_revision = ?2
                   AND status IN ('queued', 'waiting_approval')",
                rusqlite::params![&id, previous_revision],
            )?;
            let payload = serde_json::json!({
                "previousDefinitionRevision": previous_revision,
                "definitionRevision": is_schedule.then_some(definition_revision),
                "resolution": "cancelled_pending_occurrences",
            });
            insert_scheduler_event(
                &tx,
                SchedulerEventRecord {
                    automation_id: Some(&id),
                    run_id: None,
                    event_type: WorkflowSchedulerEventType::DefinitionSuperseded,
                    status: Some("cancelled"),
                    summary: "Pending occurrences were cancelled because the definition changed",
                    payload: Some(&payload),
                },
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation(&id)
    }

    pub fn get_workflow_automation(&self, id: &str) -> Result<WorkflowAutomation, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!("{WORKFLOW_AUTOMATION_SELECT} WHERE id = ?1"),
            rusqlite::params![id],
            workflow_automation_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_workflow_automations(&self) -> Result<Vec<WorkflowAutomation>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "{WORKFLOW_AUTOMATION_SELECT} ORDER BY enabled DESC, updated_at DESC, name ASC"
        ))?;
        let rows = stmt.query_map([], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn set_workflow_automation_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<WorkflowAutomation, CoreError> {
        let existing = self.get_workflow_automation(id)?;
        self.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: Some(existing.id),
                name: existing.name,
                description: existing.description,
                workflow_template_id: existing.workflow_template_id,
                prompt: existing.prompt,
                trigger: existing.trigger,
                source_scope: existing.source_scope,
                approval_policy: existing.approval_policy,
                enabled,
            },
            &existing.schedule_config,
        )
    }

    pub fn delete_workflow_automation(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM workflow_automations WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Workflow automation {id}")));
        }
        Ok(())
    }

    pub fn workflow_automation_occurrence_approval_state(
        &self,
        occurrence_id: &str,
    ) -> Result<WorkflowAutomationApprovalState, CoreError> {
        let state = self
            .conn()
            .query_row(
                "SELECT state FROM workflow_automation_occurrence_approvals
                 WHERE occurrence_id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| "pending".to_string());
        WorkflowAutomationApprovalState::from_str(&state)
    }

    /// Atomically turns a claimed occurrence into one durable, actionable
    /// approval request. The definition remains enabled and the due timestamp
    /// remains fenced by the occurrence; repeated scheduler ticks observe the
    /// same waiting run instead of manufacturing a new one.
    pub fn mark_workflow_automation_run_waiting_approval(
        &self,
        run_id: &str,
    ) -> Result<bool, CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = tx
            .query_row(
                "SELECT r.automation_id, r.occurrence_id, r.definition_revision
                 FROM workflow_automation_runs r
                 JOIN workflow_automations a ON a.id = r.automation_id
                 JOIN workflow_automation_schedule_configs c ON c.automation_id = r.automation_id
                 WHERE r.id = ?1 AND r.status = 'queued'
                   AND r.occurrence_id IS NOT NULL
                   AND c.revision = r.definition_revision
                   AND COALESCE(json_extract(a.approval_policy_json, '$.requireBeforeRun'), 1) = 1",
                rusqlite::params![run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((automation_id, occurrence_id, definition_revision)) = candidate else {
            let _already_waiting: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_automation_runs
                               WHERE id = ?1 AND status = 'waiting_approval')",
                rusqlite::params![run_id],
                |row| row.get(0),
            )?;
            tx.commit()?;
            return Ok(false);
        };
        let occurrence_updated = tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'waiting_approval', lease_token = NULL,
                 lease_expires_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'claimed'",
            rusqlite::params![&occurrence_id],
        )?;
        if occurrence_updated != 1 {
            return Err(CoreError::InvalidInput(format!(
                "Workflow occurrence {occurrence_id} is not claimable for approval"
            )));
        }
        tx.execute(
            "INSERT INTO workflow_automation_occurrence_approvals
                 (occurrence_id, state, requested_at, resolved_at, updated_at)
             VALUES (?1, 'pending', datetime('now'), NULL, datetime('now'))
             ON CONFLICT(occurrence_id) DO UPDATE SET
                 state = 'pending',
                 requested_at = COALESCE(workflow_automation_occurrence_approvals.requested_at,
                                         datetime('now')),
                 resolved_at = NULL,
                 updated_at = datetime('now')",
            rusqlite::params![&occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automation_runs SET status = 'waiting_approval'
             WHERE id = ?1 AND status = 'queued'",
            rusqlite::params![run_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations SET status = 'waiting_approval',
                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&automation_id],
        )?;
        let payload = serde_json::json!({
            "occurrenceId": occurrence_id,
            "definitionRevision": definition_revision,
            "durableApproval": true,
        });
        insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id: Some(&automation_id),
                run_id: Some(run_id),
                event_type: WorkflowSchedulerEventType::ApprovalRequested,
                status: Some("waiting_approval"),
                summary: "Scheduled occurrence is waiting for pre-run approval",
                payload: Some(&payload),
            },
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn list_workflow_automation_runs_waiting_approval(
        &self,
    ) -> Result<Vec<WorkflowAutomationRun>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT r.id, r.automation_id, r.task_run_id, r.status, r.summary,
                    r.created_at, r.finished_at, r.occurrence_id, r.scheduled_for,
                    r.definition_revision, r.attempt
             FROM workflow_automation_runs r
             JOIN workflow_automation_occurrence_approvals p
               ON p.occurrence_id = r.occurrence_id
             WHERE r.status = 'waiting_approval' AND p.state = 'pending'
             ORDER BY datetime(r.created_at) ASC, r.id ASC",
        )?;
        let rows = stmt.query_map([], workflow_automation_run_from_row)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    pub fn approve_workflow_automation_run_at(
        &self,
        run_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow approval time '{now_rfc3339}'"))
        })?;
        let lease_token = new_id();
        let lease_expires_at = (now + Duration::minutes(2)).to_rfc3339();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = tx
            .query_row(
                "SELECT r.automation_id, r.occurrence_id, r.definition_revision,
                        r.scheduled_for, COALESCE(g.origin, 'schedule')
                 FROM workflow_automation_runs r
                 JOIN workflow_automation_occurrence_approvals p
                   ON p.occurrence_id = r.occurrence_id
                 JOIN workflow_automation_occurrences o ON o.id = r.occurrence_id
                 LEFT JOIN workflow_automation_occurrence_origins g
                   ON g.occurrence_id = r.occurrence_id
                 JOIN workflow_automation_schedule_configs c ON c.automation_id = r.automation_id
                 WHERE r.id = ?1 AND r.status = 'waiting_approval'
                   AND p.state = 'pending' AND o.status = 'waiting_approval'
                   AND c.revision = r.definition_revision",
                rusqlite::params![run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "Workflow run {run_id} is no longer waiting for approval"
                ))
            })?;
        let (automation_id, occurrence_id, definition_revision, scheduled_for, origin) = candidate;
        let origin = WorkflowAutomationOccurrenceOrigin::parse(&origin)?;
        tx.execute(
            "UPDATE workflow_automation_occurrence_approvals
             SET state = 'approved', resolved_at = datetime('now'), updated_at = datetime('now')
             WHERE occurrence_id = ?1 AND state = 'pending'",
            rusqlite::params![&occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'claimed', lease_token = ?2, lease_expires_at = ?3,
                 updated_at = datetime('now') WHERE id = ?1 AND status = 'waiting_approval'",
            rusqlite::params![&occurrence_id, &lease_token, &lease_expires_at],
        )?;
        tx.execute(
            "UPDATE workflow_automation_runs SET status = 'queued'
             WHERE id = ?1 AND status = 'waiting_approval'",
            rusqlite::params![run_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations SET status = 'queued', updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&automation_id],
        )?;
        let payload = serde_json::json!({
            "occurrenceId": occurrence_id,
            "definitionRevision": definition_revision,
            "decision": "approved",
        });
        insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id: Some(&automation_id),
                run_id: Some(run_id),
                event_type: WorkflowSchedulerEventType::ApprovalResolved,
                status: Some("queued"),
                summary: "Scheduled occurrence was approved for launch",
                payload: Some(&payload),
            },
        )?;
        let run = fetch_workflow_run(&tx, run_id)?;
        let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
        tx.commit()?;
        drop(conn);
        let automation = self.get_workflow_automation(&automation_id)?;
        let due_reason = if origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
            "manual run requested".to_string()
        } else {
            automation.trigger.label()
        };
        Ok(WorkflowAutomationDueRunClaim {
            due_run: WorkflowAutomationDueRun {
                prompt: automation_prompt(&automation),
                due_reason,
                scheduled_for,
                origin,
                automation,
            },
            occurrence: Some(occurrence),
            run: Some(run),
            skip_reason: None,
        })
    }

    pub fn deny_workflow_automation_run_at(
        &self,
        run_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow denial time '{now_rfc3339}'"))
        })?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let automation = tx
            .query_row(
                "SELECT a.id, a.name, a.description, a.workflow_template_id, a.prompt,
                    a.trigger_json, a.trigger_kind, a.source_scope_json,
                    a.approval_policy_json, a.enabled, a.status, a.last_run_at,
                    a.next_run_at, a.created_at, a.updated_at, c.config_json
             FROM workflow_automations a
             JOIN workflow_automation_schedule_configs c ON c.automation_id = a.id
             JOIN workflow_automation_runs r ON r.automation_id = a.id
             JOIN workflow_automation_occurrence_approvals p ON p.occurrence_id = r.occurrence_id
             JOIN workflow_automation_occurrences o ON o.id = r.occurrence_id
             WHERE r.id = ?1 AND r.status = 'waiting_approval'
               AND p.state = 'pending' AND o.status = 'waiting_approval'
               AND c.revision = r.definition_revision",
                rusqlite::params![run_id],
                workflow_automation_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => CoreError::InvalidInput(format!(
                    "Workflow run {run_id} is no longer waiting for approval"
                )),
                other => CoreError::Database(other),
            })?;
        let run = fetch_workflow_run(&tx, run_id)?;
        let occurrence_id = run.occurrence_id.clone().ok_or_else(|| {
            CoreError::Internal(format!("Workflow run {run_id} lost its occurrence"))
        })?;
        let (origin, resume_next_run_at): (String, Option<String>) = tx.query_row(
            "SELECT origin, resume_next_run_at
             FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
            rusqlite::params![&occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let next_run_at = if WorkflowAutomationOccurrenceOrigin::parse(&origin)?
            == WorkflowAutomationOccurrenceOrigin::ManualRunNow
        {
            resume_next_run_at
        } else {
            next_run_for_trigger(
                &automation.trigger,
                &automation.schedule_config,
                automation.enabled,
                now,
            )?
        };
        tx.execute(
            "UPDATE workflow_automation_occurrence_approvals
             SET state = 'denied', resolved_at = datetime('now'), updated_at = datetime('now')
             WHERE occurrence_id = ?1 AND state = 'pending'",
            rusqlite::params![&occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'skipped', last_error = 'pre_run_approval_denied',
                 retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,
                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automation_runs
             SET status = 'cancelled', summary = 'Pre-run approval denied',
                 finished_at = datetime('now') WHERE id = ?1",
            rusqlite::params![run_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET status = CASE WHEN enabled = 1 THEN 'ready' ELSE 'disabled' END,
                 next_run_at = ?2,
                 last_run_at = CASE WHEN trigger_kind = 'folder' THEN ?3 ELSE last_run_at END,
                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&automation.id, &next_run_at, now_rfc3339],
        )?;
        let payload = serde_json::json!({
            "occurrenceId": occurrence_id,
            "definitionRevision": run.definition_revision,
            "decision": "denied",
        });
        insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id: Some(&automation.id),
                run_id: Some(run_id),
                event_type: WorkflowSchedulerEventType::ApprovalResolved,
                status: Some("cancelled"),
                summary: "Scheduled occurrence was denied before launch",
                payload: Some(&payload),
            },
        )?;
        let denied = fetch_workflow_run(&tx, run_id)?;
        tx.commit()?;
        Ok(denied)
    }

    /// Builds an immediate occurrence for a saved scheduled definition without
    /// consuming or moving its recurring cron cursor. The occurrence is still
    /// claimed by the same durable scheduler seam as timer-generated work.
    pub fn workflow_automation_run_now_due_at(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationDueRun, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow run-now time '{now_rfc3339}'"))
        })?;
        let automation = self.get_workflow_automation(automation_id)?;
        if !automation.enabled {
            return Err(CoreError::InvalidInput(format!(
                "Workflow automation '{automation_id}' is disabled"
            )));
        }
        if !matches!(
            automation.trigger,
            WorkflowAutomationTrigger::Schedule { .. }
        ) {
            return Err(CoreError::InvalidInput(format!(
                "Workflow automation '{automation_id}' is not scheduled"
            )));
        }
        Ok(WorkflowAutomationDueRun {
            prompt: automation_prompt(&automation),
            due_reason: "manual run requested".to_string(),
            scheduled_for: Some(now.to_rfc3339()),
            origin: WorkflowAutomationOccurrenceOrigin::ManualRunNow,
            automation,
        })
    }

    pub fn list_due_workflow_automations(
        &self,
        now_rfc3339: &str,
    ) -> Result<Vec<WorkflowAutomationDueRun>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "{WORKFLOW_AUTOMATION_SELECT}
             WHERE enabled = 1
               AND status != 'waiting_approval'
               AND trigger_kind IN ('schedule', 'folder')
               AND (
                    trigger_kind = 'folder'
                    OR (next_run_at IS NOT NULL AND next_run_at <= ?1)
                    OR EXISTS (
                        SELECT 1
                        FROM workflow_automation_occurrences o
                        JOIN workflow_automation_occurrence_origins g
                          ON g.occurrence_id = o.id
                        WHERE o.automation_id = workflow_automations.id
                          AND g.origin = 'manual_run_now'
                          AND (
                              o.status = 'planned'
                              OR (o.status = 'claimed'
                                  AND (o.lease_expires_at IS NULL OR o.lease_expires_at <= ?1))
                              OR (o.status = 'retry_wait'
                                  AND (o.retry_at IS NULL OR o.retry_at <= ?1))
                          )
                    )
               )
             ORDER BY COALESCE(next_run_at, updated_at) ASC, name ASC
             LIMIT 100"
        ))?;
        let rows = stmt.query_map(rusqlite::params![now_rfc3339], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let automation = row?;
            if !automation.enabled {
                continue;
            }
            let pending_manual = conn
                .query_row(
                    "SELECT o.scheduled_for
                     FROM workflow_automation_occurrences o
                     JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
                     WHERE o.automation_id = ?1 AND g.origin = 'manual_run_now'
                       AND (
                           o.status = 'planned'
                           OR (o.status = 'claimed'
                               AND (o.lease_expires_at IS NULL OR o.lease_expires_at <= ?2))
                           OR (o.status = 'retry_wait'
                               AND (o.retry_at IS NULL OR o.retry_at <= ?2))
                       )
                     ORDER BY datetime(o.created_at) ASC, o.id ASC
                     LIMIT 1",
                    rusqlite::params![&automation.id, now_rfc3339],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let (due_reason, scheduled_for, origin) = match &automation.trigger {
                WorkflowAutomationTrigger::Schedule { .. } => {
                    if let Some(scheduled_for) = pending_manual {
                        (
                            "manual run requested".to_string(),
                            Some(scheduled_for),
                            WorkflowAutomationOccurrenceOrigin::ManualRunNow,
                        )
                    } else {
                        (
                            automation.trigger.label(),
                            automation.next_run_at.clone(),
                            WorkflowAutomationOccurrenceOrigin::Schedule,
                        )
                    }
                }
                WorkflowAutomationTrigger::Folder { .. } => {
                    if !folder_trigger_due(&automation.trigger, automation.last_run_at.as_deref())?
                    {
                        continue;
                    }
                    (
                        "folder trigger matched a new or updated file".to_string(),
                        automation.next_run_at.clone(),
                        WorkflowAutomationOccurrenceOrigin::Schedule,
                    )
                }
                WorkflowAutomationTrigger::Manual => continue,
            };
            out.push(WorkflowAutomationDueRun {
                prompt: automation_prompt(&automation),
                due_reason,
                scheduled_for,
                origin,
                automation,
            });
        }
        Ok(out)
    }

    pub fn claim_workflow_automation_due_run(
        &self,
        due_run: WorkflowAutomationDueRun,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        self.claim_workflow_automation_due_run_at(due_run, &Utc::now().to_rfc3339(), summary)
    }

    pub fn claim_workflow_automation_due_run_at(
        &self,
        mut due_run: WorkflowAutomationDueRun,
        now_rfc3339: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        let Some(mut cached_scheduled_for) = due_run.scheduled_for.clone() else {
            let run = self.record_workflow_automation_run(
                &due_run.automation.id,
                None,
                "queued",
                summary.or(Some(due_run.due_reason.as_str())),
            )?;
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: None,
                run: Some(run),
                skip_reason: None,
            });
        };
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow claim time '{now_rfc3339}'"))
        })?;
        let lease_token = new_id();
        let lease_expires_at = (now + Duration::minutes(2)).to_rfc3339();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let authoritative_automation = tx.query_row(
            &format!("{WORKFLOW_AUTOMATION_SELECT} WHERE id = ?1"),
            rusqlite::params![&due_run.automation.id],
            workflow_automation_from_row,
        )?;
        if !authoritative_automation.enabled {
            return Err(CoreError::InvalidInput(
                "Workflow occurrence was already claimed, rescheduled, or disabled".into(),
            ));
        }
        if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow
            && !matches!(
                authoritative_automation.trigger,
                WorkflowAutomationTrigger::Schedule { .. }
            )
        {
            return Err(CoreError::InvalidInput(
                "Only scheduled definitions support durable run-now occurrences".into(),
            ));
        }
        due_run.automation = authoritative_automation;
        due_run.prompt = automation_prompt(&due_run.automation);
        due_run.due_reason = if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
            "manual run requested".to_string()
        } else {
            due_run.automation.trigger.label()
        };
        let definition_revision: i64 = tx.query_row(
            "SELECT revision FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
            rusqlite::params![&due_run.automation.id],
            |row| row.get(0),
        )?;
        let pending_candidate = tx
            .query_row(
                "SELECT id, automation_id, definition_revision, scheduled_for, status,
                        attempt_count, retry_at, last_error, lease_token, lease_expires_at,
                        o.created_at, o.updated_at, COALESCE(g.origin, 'schedule'),
                        g.resume_next_run_at
                 FROM workflow_automation_occurrences o
                 LEFT JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
                 WHERE o.automation_id = ?1 AND o.definition_revision = ?2
                   AND o.status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval')
                 ORDER BY datetime(o.created_at) DESC, o.id DESC
                 LIMIT 1",
                rusqlite::params![&due_run.automation.id, definition_revision],
                |row| {
                    Ok((
                        workflow_automation_occurrence_from_row(row)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, Option<String>>(13)?,
                    ))
                },
            )
            .optional()?;
        let pending_occurrence = pending_candidate
            .map(|(occurrence, origin, resume_next_run_at)| {
                Ok::<_, CoreError>((
                    occurrence,
                    WorkflowAutomationOccurrenceOrigin::parse(&origin)?,
                    resume_next_run_at,
                ))
            })
            .transpose()?
            .filter(|(occurrence, origin, _)| {
                *origin == due_run.origin
                    && (due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
                        || occurrence.scheduled_for == cached_scheduled_for)
            });
        if let Some((pending, origin, _)) = pending_occurrence.as_ref() {
            cached_scheduled_for = pending.scheduled_for.clone();
            due_run.origin = *origin;
        }
        if due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
            && due_run.automation.next_run_at.as_deref() != Some(cached_scheduled_for.as_str())
            && pending_occurrence.is_none()
        {
            return Err(CoreError::InvalidInput(
                "Workflow occurrence was already claimed, rescheduled, or disabled".into(),
            ));
        }
        let cached_scheduled_at = parse_utc_timestamp(&cached_scheduled_for).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid workflow scheduled occurrence '{cached_scheduled_for}'"
            ))
        })?;
        if cached_scheduled_at > now {
            return Err(CoreError::InvalidInput(format!(
                "Workflow occurrence '{cached_scheduled_for}' is not due yet"
            )));
        }
        let scheduled_for = if let Some((pending, _, _)) = pending_occurrence.as_ref() {
            pending.scheduled_for.clone()
        } else if due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
            && due_run.automation.schedule_config.misfire_policy
                == WorkflowScheduleMisfirePolicy::RunLatest
            && cached_scheduled_at < now
        {
            let WorkflowAutomationTrigger::Schedule { cron } = &due_run.automation.trigger else {
                return Err(CoreError::Internal(
                    "A scheduled occurrence must retain a schedule trigger".into(),
                ));
            };
            latest_workflow_cron_occurrence_at_or_before(
                cron,
                &due_run.automation.schedule_config.timezone,
                cached_scheduled_at,
                now,
            )?
            .to_rfc3339()
        } else {
            cached_scheduled_for.clone()
        };
        let scheduled_at = parse_utc_timestamp(&scheduled_for).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid workflow scheduled occurrence '{scheduled_for}'"
            ))
        })?;
        due_run.scheduled_for = Some(scheduled_for.clone());
        let resume_next_run_at = pending_occurrence
            .as_ref()
            .and_then(|(_, _, resume)| resume.clone())
            .or_else(|| {
                (due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow)
                    .then(|| due_run.automation.next_run_at.clone())
                    .flatten()
            });
        let next_run_at = if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
            resume_next_run_at.clone()
        } else {
            next_run_for_trigger(
                &due_run.automation.trigger,
                &due_run.automation.schedule_config,
                due_run.automation.enabled,
                now,
            )?
        };
        let existing = if let Some((occurrence, _, _)) = pending_occurrence {
            Some(occurrence)
        } else {
            tx.query_row(
                "SELECT id, automation_id, definition_revision, scheduled_for, status,
                        attempt_count, retry_at, last_error, lease_token, lease_expires_at,
                        o.created_at, o.updated_at
                 FROM workflow_automation_occurrences o
                 JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
                 WHERE o.automation_id = ?1 AND o.definition_revision = ?2
                   AND o.scheduled_for = ?3 AND g.origin = ?4",
                rusqlite::params![
                    &due_run.automation.id,
                    definition_revision,
                    &scheduled_for,
                    due_run.origin.as_str()
                ],
                workflow_automation_occurrence_from_row,
            )
            .optional()?
        };
        let occurrence_id = existing
            .as_ref()
            .map(|item| item.id.clone())
            .unwrap_or_else(new_id);
        if existing.is_none() {
            tx.execute(
                "INSERT INTO workflow_automation_occurrences
                     (id, automation_id, definition_revision, scheduled_for, status)
                 VALUES (?1, ?2, ?3, ?4, 'planned')",
                rusqlite::params![
                    &occurrence_id,
                    &due_run.automation.id,
                    definition_revision,
                    &scheduled_for
                ],
            )?;
            tx.execute(
                "INSERT INTO workflow_automation_occurrence_origins
                     (occurrence_id, origin, resume_next_run_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![&occurrence_id, due_run.origin.as_str(), &resume_next_run_at],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO workflow_automation_occurrence_approvals
                 (occurrence_id, state)
             VALUES (?1, ?2)",
            rusqlite::params![
                &occurrence_id,
                if due_run.automation.approval_policy.require_before_run {
                    WorkflowAutomationApprovalState::Pending.as_str()
                } else {
                    WorkflowAutomationApprovalState::NotRequired.as_str()
                }
            ],
        )?;
        let current = existing.as_ref();
        if let Some(current) = current {
            if current.status == WorkflowAutomationOccurrenceStatus::WaitingApproval {
                let occurrence = current.clone();
                tx.commit()?;
                drop(conn);
                return Ok(WorkflowAutomationDueRunClaim {
                    due_run,
                    occurrence: Some(occurrence),
                    run: None,
                    skip_reason: Some("waiting_approval".into()),
                });
            }
            let lease_is_live = current
                .lease_expires_at
                .as_deref()
                .and_then(parse_utc_timestamp)
                .is_some_and(|expires| expires > now);
            if current.status == WorkflowAutomationOccurrenceStatus::Claimed && lease_is_live {
                let occurrence = current.clone();
                tx.commit()?;
                drop(conn);
                return Ok(WorkflowAutomationDueRunClaim {
                    due_run,
                    occurrence: Some(occurrence),
                    run: None,
                    skip_reason: Some("already_claimed_live".into()),
                });
            }
            if matches!(
                current.status,
                WorkflowAutomationOccurrenceStatus::Running
                    | WorkflowAutomationOccurrenceStatus::Completed
                    | WorkflowAutomationOccurrenceStatus::Skipped
                    | WorkflowAutomationOccurrenceStatus::Failed
                    | WorkflowAutomationOccurrenceStatus::Cancelled
                    | WorkflowAutomationOccurrenceStatus::TimedOut
                    | WorkflowAutomationOccurrenceStatus::Disabled
            ) {
                tx.execute(
                    "UPDATE workflow_automations
                     SET next_run_at = ?2, status = CASE WHEN status = 'queued' THEN 'ready' ELSE status END,
                         updated_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![&due_run.automation.id, &next_run_at],
                )?;
                let occurrence = current.clone();
                tx.commit()?;
                drop(conn);
                return Ok(WorkflowAutomationDueRunClaim {
                    due_run,
                    occurrence: Some(occurrence),
                    run: None,
                    skip_reason: Some("already_consumed".into()),
                });
            }
            if current
                .retry_at
                .as_deref()
                .and_then(parse_utc_timestamp)
                .is_some_and(|retry_at| retry_at > now)
            {
                let occurrence = current.clone();
                tx.commit()?;
                drop(conn);
                return Ok(WorkflowAutomationDueRunClaim {
                    due_run,
                    occurrence: Some(occurrence),
                    run: None,
                    skip_reason: Some("retry_backoff".into()),
                });
            }
        }
        let active_run_exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM workflow_automation_runs
                 WHERE automation_id = ?1
                   AND (occurrence_id IS NULL OR occurrence_id != ?2)
                   AND status IN ('queued', 'running', 'initializing', 'in_progress',
                                  'waiting_approval', 'paused', 'resuming', 'cancelling')
             )",
            rusqlite::params![&due_run.automation.id, &occurrence_id],
            |row| row.get(0),
        )?;
        let isolated_source_lock_exists = if due_run
            .automation
            .schedule_config
            .execution_policy
            .workspace_policy
            == WorkflowScheduleWorkspacePolicy::IsolatedPatch
        {
            let source_fingerprint = due_run
                .automation
                .schedule_config
                .execution_policy
                .source_root_fingerprint
                .as_deref()
                .ok_or_else(|| {
                    CoreError::InvalidInput(
                    "Isolated scheduled patch lost its canonical source fingerprint before claim"
                        .into(),
                )
                })?;
            tx.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM workflow_automation_runs r
                     JOIN workflow_automation_definition_revisions d
                       ON d.automation_id = r.automation_id
                      AND d.revision = r.definition_revision
                     WHERE r.automation_id != ?1
                       AND r.status IN ('queued', 'running', 'initializing', 'in_progress',
                                        'waiting_approval', 'paused', 'resuming', 'cancelling')
                       AND json_extract(d.schedule_config_json,
                                        '$.executionPolicy.workspacePolicy') = 'isolated_patch'
                       AND json_extract(d.schedule_config_json,
                                        '$.executionPolicy.sourceRootFingerprint') = ?2
                 )",
                rusqlite::params![&due_run.automation.id, source_fingerprint],
                |row| row.get::<_, bool>(0),
            )?
        } else {
            false
        };
        if isolated_source_lock_exists {
            let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
            tx.commit()?;
            drop(conn);
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: Some(occurrence),
                run: None,
                skip_reason: Some("source_workspace_locked".into()),
            });
        }
        if active_run_exists
            && due_run.automation.schedule_config.overlap_policy
                == WorkflowScheduleOverlapPolicy::Skip
        {
            tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'skipped', last_error = 'overlap_policy_skip',
                     retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,
                     updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&occurrence_id],
            )?;
            tx.execute(
                "UPDATE workflow_automations
                 SET next_run_at = ?2, updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&due_run.automation.id, &next_run_at],
            )?;
            let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
            tx.commit()?;
            drop(conn);
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: Some(occurrence),
                run: None,
                skip_reason: Some("overlap_active".into()),
            });
        }
        let attempt = current.map_or(1, |item| item.attempt_count.saturating_add(1));
        if attempt as usize > SCHEDULER_RETRY_MAX_ATTEMPTS {
            tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
                     updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&occurrence_id],
            )?;
            tx.execute(
                "UPDATE workflow_automations
                 SET next_run_at = ?2, status = 'ready', updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![&due_run.automation.id, &next_run_at],
            )?;
            let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
            tx.commit()?;
            drop(conn);
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: Some(occurrence),
                run: None,
                skip_reason: Some("retry_exhausted".into()),
            });
        }
        if attempt > 1 {
            tx.execute(
                "UPDATE workflow_automation_runs
                 SET status = 'cancelled',
                     summary = COALESCE(summary, 'Occurrence lease superseded by a newer attempt'),
                     finished_at = COALESCE(finished_at, datetime('now'))
                 WHERE occurrence_id = ?1 AND status = 'queued' AND attempt < ?2",
                rusqlite::params![&occurrence_id, i64::from(attempt)],
            )?;
        }
        let misfire_expired = due_run.automation.schedule_config.misfire_policy
            == WorkflowScheduleMisfirePolicy::Skip
            && attempt == 1
            && now
                > scheduled_at
                    + Duration::seconds(i64::from(
                        due_run.automation.schedule_config.misfire_grace_seconds,
                    ));
        if misfire_expired {
            tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'skipped', retry_at = NULL, lease_token = NULL,
                     lease_expires_at = NULL, updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&occurrence_id],
            )?;
            tx.execute(
                "UPDATE workflow_automations
                 SET next_run_at = ?2, status = 'ready', updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![&due_run.automation.id, &next_run_at],
            )?;
            let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
            tx.commit()?;
            drop(conn);
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: Some(occurrence),
                run: None,
                skip_reason: Some("misfire_grace_exceeded".into()),
            });
        }
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'claimed', attempt_count = ?2, retry_at = NULL,
                 lease_token = ?3, lease_expires_at = ?4, updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![
                &occurrence_id,
                i64::from(attempt),
                &lease_token,
                &lease_expires_at
            ],
        )?;
        let run_id = new_id();
        tx.execute(
            "INSERT INTO workflow_automation_runs
                 (id, automation_id, task_run_id, status, summary, occurrence_id,
                  scheduled_for, definition_revision, attempt)
             VALUES (?1, ?2, NULL, 'queued', ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &run_id,
                &due_run.automation.id,
                summary.or(Some(due_run.due_reason.as_str())),
                &occurrence_id,
                &scheduled_for,
                definition_revision,
                i64::from(attempt)
            ],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET status = 'queued', updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&due_run.automation.id],
        )?;
        let run = fetch_workflow_run(&tx, &run_id)?;
        let occurrence = fetch_workflow_occurrence(&tx, &occurrence_id)?;
        tx.commit()?;
        drop(conn);
        Ok(WorkflowAutomationDueRunClaim {
            due_run,
            occurrence: Some(occurrence),
            run: Some(run),
            skip_reason: None,
        })
    }

    pub fn claim_due_workflow_automation_run(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        let due_run = self
            .list_due_workflow_automations(now_rfc3339)?
            .into_iter()
            .find(|due| due.automation.id == automation_id)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "Workflow automation '{automation_id}' is not currently due."
                ))
            })?;
        self.claim_workflow_automation_due_run_at(due_run, now_rfc3339, summary)
    }

    pub fn preview_workflow_automation_prompt(&self, id: &str) -> Result<String, CoreError> {
        let automation = self.get_workflow_automation(id)?;
        Ok(automation_prompt(&automation))
    }

    pub fn record_workflow_automation_run(
        &self,
        automation_id: &str,
        task_run_id: Option<&str>,
        status: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        self.get_workflow_automation(automation_id)?;
        let status = WorkflowAutomationRunStatus::parse(status)?;
        let status = status.as_str();
        let id = new_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO workflow_automation_runs
             (id, automation_id, task_run_id, status, summary, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, CASE WHEN ?4 IN ('completed', 'failed', 'cancelled', 'timed_out', 'disabled') THEN datetime('now') ELSE NULL END)",
            rusqlite::params![&id, automation_id, task_run_id, status, summary],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET last_run_at = datetime('now'),
                  status = ?2,
                  updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![automation_id, status],
        )?;
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(&id)
    }

    pub fn start_workflow_automation_run(
        &self,
        run_id: &str,
        task_run_id: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        self.start_workflow_automation_run_at(
            run_id,
            task_run_id,
            summary,
            &Utc::now().to_rfc3339(),
        )
    }

    pub fn start_workflow_automation_run_at(
        &self,
        run_id: &str,
        task_run_id: &str,
        summary: Option<&str>,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow start time '{now_rfc3339}'"))
        })?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = tx
            .query_row(
                &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
                rusqlite::params![run_id],
                workflow_automation_run_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation run {run_id}"))
                }
                other => CoreError::Database(other),
            })?;
        let current_state = crate::task_orchestrator::project_task_status(run.status.as_str())
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        crate::task_orchestrator::validate_task_transition(
            current_state,
            crate::task_orchestrator::TaskOrchestratorState::Running,
        )
        .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        if let Some(existing_task_run_id) = run.task_run_id.as_deref() {
            if existing_task_run_id != task_run_id {
                return Err(CoreError::InvalidInput(format!(
                    "Workflow automation run {run_id} is already bound to task run {existing_task_run_id}"
                )));
            }
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let (occurrence_status, current_attempt, lease_token, lease_expires_at): (
                String,
                i64,
                Option<String>,
                Option<String>,
            ) = tx
                .query_row(
                    "SELECT status, attempt_count, lease_token, lease_expires_at
                     FROM workflow_automation_occurrences WHERE id = ?1",
                    rusqlite::params![occurrence_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!(
                        "Workflow automation occurrence {occurrence_id}"
                    )),
                    other => CoreError::Database(other),
                })?;
            let lease_is_live = lease_expires_at
                .as_deref()
                .and_then(parse_utc_timestamp)
                .is_some_and(|expires_at| expires_at > now);
            let attempt_is_authoritative = occurrence_status == "claimed"
                && current_attempt == i64::from(run.attempt)
                && lease_token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty())
                && lease_is_live;
            if !attempt_is_authoritative {
                tx.execute(
                    "UPDATE workflow_automation_runs
                     SET status = 'cancelled',
                         summary = COALESCE(summary, 'Occurrence claim was superseded or expired'),
                         finished_at = COALESCE(finished_at, datetime('now'))
                     WHERE id = ?1 AND status = 'queued'",
                    rusqlite::params![run_id],
                )?;
                tx.commit()?;
                return Err(CoreError::InvalidInput(format!(
                    "Workflow automation run {run_id} no longer owns the occurrence claim"
                )));
            }
        }
        let task_run_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_task_runs WHERE id = ?1)",
            rusqlite::params![task_run_id],
            |row| row.get(0),
        )?;
        if !task_run_exists {
            return Err(CoreError::NotFound(format!("Agent task run {task_run_id}")));
        }
        let next_run_at = if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let (origin, resume_next_run_at): (String, Option<String>) = tx.query_row(
                "SELECT origin, resume_next_run_at
                 FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
                rusqlite::params![occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if WorkflowAutomationOccurrenceOrigin::parse(&origin)?
                == WorkflowAutomationOccurrenceOrigin::ManualRunNow
            {
                resume_next_run_at
            } else {
                let (trigger_json, enabled, schedule_config_json): (String, i64, Option<String>) =
                    tx.query_row(
                        "SELECT automation.trigger_json, automation.enabled, schedule.config_json
                     FROM workflow_automations automation
                     LEFT JOIN workflow_automation_schedule_configs schedule
                       ON schedule.automation_id = automation.id
                     WHERE automation.id = ?1",
                        rusqlite::params![&run.automation_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(|error| match error {
                        rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!(
                            "Workflow automation {}",
                            run.automation_id
                        )),
                        other => CoreError::Database(other),
                    })?;
                let trigger = serde_json::from_str(&trigger_json)?;
                let schedule_config = schedule_config_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or_default();
                next_run_for_trigger(&trigger, &schedule_config, enabled != 0, now)?
            }
        } else {
            None
        };

        let affected = tx.execute(
            "UPDATE workflow_automation_runs
             SET task_run_id = ?2,
                 status = 'running',
                 summary = COALESCE(?3, summary),
                 finished_at = NULL
             WHERE id = ?1",
            rusqlite::params![run_id, task_run_id, summary],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation run {run_id}"
            )));
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let affected = tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'running', retry_at = NULL, lease_token = NULL,
                     lease_expires_at = NULL, updated_at = datetime('now')
                 WHERE id = ?1 AND status = 'claimed' AND attempt_count = ?2",
                rusqlite::params![occurrence_id, i64::from(run.attempt)],
            )?;
            if affected == 0 {
                return Err(CoreError::NotFound(format!(
                    "Workflow automation occurrence {occurrence_id}"
                )));
            }
        }
        let affected = tx.execute(
            "UPDATE workflow_automations
             SET status = 'running',
                  last_run_at = datetime('now'),
                  next_run_at = COALESCE(?2, next_run_at),
                  updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, &next_run_at],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation {}",
                run.automation_id
            )));
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn mark_workflow_automation_launch_failed_for_retry(
        &self,
        run_id: &str,
        error: &str,
        now_rfc3339: &str,
    ) -> Result<Option<WorkflowAutomationOccurrence>, CoreError> {
        let run = self.get_workflow_automation_run(run_id)?;
        let Some(occurrence_id) = run.occurrence_id.as_deref() else {
            self.transition_workflow_automation_run(
                run_id,
                "cancelled",
                Some("Task Orchestrator launch failed before agent start"),
            )?;
            return Ok(None);
        };
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid workflow launch failure time '{now_rfc3339}'"
            ))
        })?;
        let error = normalize_optional(error, 2_000)?;
        let exhausted = run.attempt as usize >= SCHEDULER_RETRY_MAX_ATTEMPTS;
        let retry_at = (!exhausted).then(|| {
            let seconds = scheduler_retry_backoff_seconds(run.attempt as usize).unwrap_or(300);
            (now + Duration::seconds(seconds)).to_rfc3339()
        });
        let automation = self.get_workflow_automation(&run.automation_id)?;
        let (origin, resume_next_run_at): (String, Option<String>) = self.conn().query_row(
            "SELECT origin, resume_next_run_at
             FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
            rusqlite::params![occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let next_run_at = if WorkflowAutomationOccurrenceOrigin::parse(&origin)?
            == WorkflowAutomationOccurrenceOrigin::ManualRunNow
        {
            exhausted.then_some(resume_next_run_at).flatten()
        } else if exhausted {
            next_run_for_trigger(
                &automation.trigger,
                &automation.schedule_config,
                automation.enabled,
                now,
            )?
        } else {
            None
        };
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE workflow_automation_runs
             SET status = 'cancelled', summary = ?2, finished_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![run_id, &error],
        )?;
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = ?2, retry_at = ?3, last_error = ?4,
                 lease_token = NULL, lease_expires_at = NULL,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![
                occurrence_id,
                if exhausted { "failed" } else { "retry_wait" },
                &retry_at,
                &error
            ],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET status = 'ready',
                 next_run_at = COALESCE(?2, next_run_at),
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, &next_run_at],
        )?;
        let occurrence = fetch_workflow_occurrence(&tx, occurrence_id)?;
        tx.commit()?;
        Ok(Some(occurrence))
    }

    pub fn workflow_automation_has_active_run(
        &self,
        automation_id: &str,
    ) -> Result<bool, CoreError> {
        self.conn()
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM workflow_automation_runs
                     WHERE automation_id = ?1
                       AND task_run_id IS NOT NULL
                       AND status IN ('running', 'initializing', 'in_progress',
                                      'waiting_approval', 'paused', 'resuming', 'cancelling')
                 )",
                rusqlite::params![automation_id],
                |row| row.get(0),
            )
            .map_err(CoreError::Database)
    }

    pub fn get_workflow_automation_occurrence(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationOccurrence, CoreError> {
        self.conn()
            .query_row(
                &format!("{WORKFLOW_OCCURRENCE_SELECT} WHERE id = ?1"),
                rusqlite::params![id],
                workflow_automation_occurrence_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation occurrence {id}"))
                }
                other => CoreError::Database(other),
            })
    }

    pub fn transition_workflow_automation_run(
        &self,
        run_id: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let target_status = WorkflowAutomationRunStatus::parse(status)?;
        let status = target_status.as_str();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = tx
            .query_row(
                &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
                rusqlite::params![run_id],
                workflow_automation_run_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation run {run_id}"))
                }
                other => CoreError::Database(other),
            })?;
        let current_state = crate::task_orchestrator::project_task_status(run.status.as_str())
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        let target_state = crate::task_orchestrator::project_task_status(status)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        if current_state != target_state {
            crate::task_orchestrator::validate_task_transition(current_state, target_state)
                .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        }

        let affected = tx.execute(
            "UPDATE workflow_automation_runs
             SET status = ?2,
                 summary = COALESCE(?3, summary),
                 finished_at = CASE
                     WHEN ?2 IN ('completed', 'failed', 'cancelled', 'timed_out', 'disabled')
                     THEN COALESCE(finished_at, datetime('now'))
                     ELSE NULL
                 END
             WHERE id = ?1",
            rusqlite::params![run_id, status, summary],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation run {run_id}"
            )));
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let affected = tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = ?2,
                     lease_token = NULL,
                     lease_expires_at = NULL,
                     updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![occurrence_id, status],
            )?;
            if affected == 0 {
                return Err(CoreError::NotFound(format!(
                    "Workflow automation occurrence {occurrence_id}"
                )));
            }
        }
        let affected = tx.execute(
            "UPDATE workflow_automations
             SET status = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, status],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation {}",
                run.automation_id
            )));
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn get_workflow_automation_run(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
            rusqlite::params![id],
            workflow_automation_run_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation run {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn get_workflow_automation_run_for_task_run(
        &self,
        task_run_id: &str,
    ) -> Result<Option<WorkflowAutomationRun>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!(
                "{WORKFLOW_RUN_SELECT} WHERE task_run_id = ?1 ORDER BY datetime(created_at) DESC, id DESC LIMIT 1"
            ),
            rusqlite::params![task_run_id],
            workflow_automation_run_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
    }

    pub fn record_workflow_automation_scheduler_event(
        &self,
        automation_id: Option<&str>,
        run_id: Option<&str>,
        event_type: WorkflowSchedulerEventType,
        status: Option<&str>,
        summary: &str,
        payload: Option<&Value>,
    ) -> Result<WorkflowAutomationSchedulerEvent, CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id,
                run_id,
                event_type,
                status,
                summary,
                payload,
            },
        )?;
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_scheduler_event(&id)
    }

    pub fn get_workflow_automation_scheduler_event(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationSchedulerEvent, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
             FROM workflow_automation_scheduler_events WHERE id = ?1",
            rusqlite::params![id],
            workflow_scheduler_event_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation scheduler event {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_workflow_automation_scheduler_events(
        &self,
        automation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut out = Vec::new();
        if let Some(automation_id) = automation_id {
            let mut stmt = conn.prepare(
                "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
                 FROM workflow_automation_scheduler_events
                 WHERE automation_id = ?1
                 ORDER BY datetime(created_at) DESC, id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![automation_id, limit],
                workflow_scheduler_event_from_row,
            )?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
                 FROM workflow_automation_scheduler_events
                 ORDER BY datetime(created_at) DESC, id DESC
                 LIMIT ?1",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![limit], workflow_scheduler_event_from_row)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn workflow_automation_scheduler_retry_decision(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationSchedulerRetryDecision, CoreError> {
        let automation_id = automation_id.trim();
        if automation_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "Workflow automation id is required for scheduler retry decision".to_string(),
            ));
        }
        let events = self.list_workflow_automation_scheduler_events(
            Some(automation_id),
            SCHEDULER_RETRY_EVENT_LOOKBACK_LIMIT,
        )?;
        workflow_automation_scheduler_retry_decision_from_events(&events, now_rfc3339)
    }

    pub fn list_workflow_automation_scheduler_events_for_run(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
             FROM workflow_automation_scheduler_events
             WHERE run_id = ?1
             ORDER BY datetime(created_at) ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![run_id, limit],
            workflow_scheduler_event_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_workflow_automation_scheduler_events_for_task_run(
        &self,
        task_run_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let task_run_id = task_run_id.trim();
        if task_run_id.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.automation_id, e.run_id, e.event_type, e.status, e.summary, e.payload_json, e.created_at
             FROM workflow_automation_scheduler_events e
             INNER JOIN workflow_automation_runs r ON r.id = e.run_id
             WHERE r.task_run_id = ?1
             ORDER BY datetime(e.created_at) ASC, e.id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![task_run_id, limit],
            workflow_scheduler_event_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Atomically append the exact checkpoint prompt and re-queue its original
    /// turn/run. The checkpoint is a one-shot launch boundary: retries with
    /// the same key return the first response message, while stale checkpoints
    /// and changed launch input fail closed.
    pub fn resume_agent_turn_from_checkpoint(
        &self,
        message: &ConversationMessage,
        provider: Option<&str>,
        model: Option<&str>,
        idempotency_key: &str,
        checkpoint_id: &str,
    ) -> Result<AgentTurnLaunchRecord, CoreError> {
        let idempotency_key =
            normalize_required(idempotency_key, "Checkpoint launch idempotency key", 256)?;
        let checkpoint_id = normalize_required(checkpoint_id, "Task resume checkpoint id", 256)?;
        if message.role != Role::User {
            return Err(CoreError::InvalidInput(
                "Checkpoint response message must have the user role".to_string(),
            ));
        }
        if message.conversation_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "Checkpoint response conversation id cannot be empty".to_string(),
            ));
        }

        let tool_calls_json = if message.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&message.tool_calls)?)
        };
        let artifacts_json = Some(serde_json::to_string(&serde_json::json!({
            "kind": "checkpointContinuation",
            "version": 1,
            "checkpointId": &checkpoint_id,
        }))?);
        let image_attachments_json = message
            .image_attachments
            .as_ref()
            .filter(|attachments| !attachments.is_empty())
            .map(serde_json::to_string)
            .transpose()?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checkpoint = tx
            .query_row(
                "SELECT checkpoint.run_id, checkpoint.resume_prompt,
                        checkpoint.launch_idempotency_key,
                        checkpoint.response_message_id,
                        run.conversation_id, run.turn_id, run.status
                 FROM task_resume_checkpoints checkpoint
                 JOIN agent_task_runs run ON run.id = checkpoint.run_id
                 WHERE checkpoint.id = ?1",
                [&checkpoint_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::NotFound(format!("Task resume checkpoint {checkpoint_id}"))
            })?;
        let (
            run_id,
            resume_prompt,
            persisted_launch_key,
            response_message_id,
            conversation_id,
            turn_id,
            run_status,
        ) = checkpoint;

        if conversation_id != message.conversation_id {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint belongs to a different conversation".to_string(),
            ));
        }
        if message.content != resume_prompt {
            return Err(CoreError::InvalidInput(
                "Checkpoint response must exactly match the durable resume prompt".to_string(),
            ));
        }

        let latest_checkpoint_id: String = tx.query_row(
            "SELECT id FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, rowid DESC
             LIMIT 1",
            [&run_id],
            |row| row.get(0),
        )?;
        if latest_checkpoint_id != checkpoint_id {
            return Err(CoreError::InvalidInput(format!(
                "Task resume checkpoint {checkpoint_id} is stale"
            )));
        }

        // A committed response is the idempotency record. If startup restored
        // the durable pause because the original launch never committed its
        // started marker, the same key may atomically queue that response one
        // more time without appending another transcript message.
        if let Some(response_message_id) = response_message_id {
            if persisted_launch_key.as_deref() != Some(idempotency_key.as_str()) {
                return Err(CoreError::InvalidInput(
                    "Task resume checkpoint was already launched with a different idempotency key"
                        .to_string(),
                ));
            }
            let persisted_response = tx
                .query_row(
                    "SELECT sort_order, content
                     FROM messages
                     WHERE id = ?1 AND conversation_id = ?2 AND role = 'user'",
                    rusqlite::params![&response_message_id, &conversation_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .ok_or_else(|| {
                    CoreError::Internal(format!(
                        "Checkpoint {checkpoint_id} references a missing response message"
                    ))
                })?;
            if persisted_response.1 != resume_prompt {
                return Err(CoreError::Internal(format!(
                    "Checkpoint {checkpoint_id} response message no longer matches its prompt"
                )));
            }
            let replayed_after_restart = if run_status == "paused" {
                let run_updated = tx.execute(
                    "UPDATE agent_task_runs
                     SET status = 'queued', phase = 'queued',
                         summary = 'Resuming from checkpoint', error_message = NULL,
                         finished_at = NULL, updated_at = datetime('now')
                     WHERE id = ?1 AND status = 'paused'",
                    [&run_id],
                )?;
                if run_updated != 1 {
                    return Err(CoreError::InvalidInput(
                        "Task changed while its checkpoint response was being replayed".to_string(),
                    ));
                }
                let turn_updated = tx.execute(
                    "UPDATE conversation_turns
                     SET status = 'running', finished_at = NULL, updated_at = datetime('now')
                     WHERE id = ?1 AND conversation_id = ?2 AND status = 'paused'",
                    rusqlite::params![&turn_id, &conversation_id],
                )?;
                if turn_updated != 1 {
                    return Err(CoreError::InvalidInput(
                        "Conversation turn changed while its checkpoint response was being replayed"
                            .to_string(),
                    ));
                }
                tx.execute(
                    "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
                    [&conversation_id],
                )?;
                true
            } else {
                false
            };
            tx.commit()?;
            return Ok(AgentTurnLaunchRecord {
                conversation_id,
                user_message_id: response_message_id,
                user_message_sort_order: persisted_response.0,
                turn_id,
                run_id,
                status: if replayed_after_restart {
                    "queued".to_string()
                } else {
                    run_status
                },
                reused: !replayed_after_restart,
            });
        }

        if persisted_launch_key
            .as_deref()
            .is_some_and(|key| key != idempotency_key.as_str())
        {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint was already claimed by a different idempotency key"
                    .to_string(),
            ));
        }
        if run_status != "paused" {
            return Err(CoreError::InvalidInput(format!(
                "Task resume checkpoint {checkpoint_id} cannot resume task from status {run_status}"
            )));
        }

        let user_message_sort_order = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1
             FROM messages WHERE conversation_id = ?1",
            [&conversation_id],
            |row| row.get::<_, i64>(0),
        )?;
        tx.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_call_id,
             tool_calls_json, artifacts_json, token_count, sort_order, thinking,
             image_attachments_json)
             VALUES (?1, ?2, 'user', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                &message.id,
                &conversation_id,
                &message.content,
                &message.tool_call_id,
                &tool_calls_json,
                &artifacts_json,
                message.token_count,
                user_message_sort_order,
                &message.thinking,
                &image_attachments_json,
            ],
        )?;
        let checkpoint_updated = tx.execute(
            "UPDATE task_resume_checkpoints
             SET launch_idempotency_key = ?2, response_message_id = ?3
             WHERE id = ?1
               AND response_message_id IS NULL
               AND (launch_idempotency_key IS NULL OR launch_idempotency_key = ?2)",
            rusqlite::params![&checkpoint_id, &idempotency_key, &message.id],
        )?;
        if checkpoint_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint changed while it was being launched".to_string(),
            ));
        }
        let run_updated = tx.execute(
            "UPDATE agent_task_runs
             SET status = 'queued', phase = 'queued',
                 summary = 'Resuming from checkpoint', error_message = NULL,
                 provider = COALESCE(?2, provider), model = COALESCE(?3, model),
                 finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'paused'",
            rusqlite::params![&run_id, provider, model],
        )?;
        if run_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Task changed while its checkpoint was being resumed".to_string(),
            ));
        }
        let turn_updated = tx.execute(
            "UPDATE conversation_turns
             SET status = 'running', finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND conversation_id = ?2",
            rusqlite::params![&turn_id, &conversation_id],
        )?;
        if turn_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Conversation turn changed while its checkpoint was being resumed".to_string(),
            ));
        }
        tx.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            [&conversation_id],
        )?;
        tx.commit()?;

        Ok(AgentTurnLaunchRecord {
            conversation_id,
            user_message_id: message.id.clone(),
            user_message_sort_order,
            turn_id,
            run_id,
            status: "queued".to_string(),
            reused: false,
        })
    }

    pub fn create_task_resume_checkpoint(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        self.create_task_resume_checkpoint_with_state(run_id, reason, None)
    }

    pub fn create_task_resume_checkpoint_with_state(
        &self,
        run_id: &str,
        reason: &str,
        live_state: Option<&Value>,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let checkpoint = self.prepare_task_resume_checkpoint(run_id, reason, live_state)?;
        let conn = self.conn();
        Self::insert_task_resume_checkpoint_on_connection(&conn, &checkpoint)
    }

    /// Build a checkpoint without making it durable. The Run Event outbox uses
    /// this draft so the checkpoint row can share the pause event transaction.
    pub(crate) fn prepare_task_resume_checkpoint(
        &self,
        run_id: &str,
        reason: &str,
        live_state: Option<&Value>,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let events = self
            .get_agent_task_run_events(run_id)?
            .into_iter()
            .rev()
            .take(20)
            .collect::<Vec<_>>();
        let mut events = events;
        events.reverse();
        let artifacts = self
            .list_agent_task_artifacts(run_id)
            .unwrap_or_else(|_| Vec::new());
        let mut state = serde_json::json!({
            "run": run,
            "recentEvents": events,
            "artifacts": artifacts,
            "checkpointedAt": Utc::now().to_rfc3339(),
        });
        if let Some(partial_output) = partial_assistant_output(&self.list_agent_run_events(run_id)?)
        {
            if let Some(map) = state.as_object_mut() {
                map.insert("partialAssistantOutput".to_string(), partial_output);
            }
        }
        if let Some(live_state) = live_state {
            if let Some(map) = state.as_object_mut() {
                map.insert("liveTurnState".to_string(), live_state.clone());
            }
        }
        let run = self.get_agent_task_run(run_id)?;
        let checkpoint_id = new_id();
        let resume_prompt = build_resume_prompt(&run, &checkpoint_id, reason, &state);
        Ok(TaskResumeCheckpoint {
            id: checkpoint_id,
            run_id: run_id.to_string(),
            reason: reason.trim().to_string(),
            status: run.status,
            phase: run.phase,
            state,
            resume_prompt,
            created_at: String::new(),
        })
    }

    pub(crate) fn insert_task_resume_checkpoint_on_connection(
        connection: &rusqlite::Connection,
        checkpoint: &TaskResumeCheckpoint,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let state_json = serde_json::to_string(&checkpoint.state)?;
        connection.execute(
            "INSERT INTO task_resume_checkpoints
             (id, run_id, reason, status, phase, state_json, resume_prompt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &checkpoint.id,
                &checkpoint.run_id,
                &checkpoint.reason,
                &checkpoint.status,
                &checkpoint.phase,
                &state_json,
                &checkpoint.resume_prompt,
            ],
        )?;
        Self::get_task_resume_checkpoint_on_connection(connection, &checkpoint.id)
    }

    pub(crate) fn get_task_resume_checkpoint_on_connection(
        connection: &rusqlite::Connection,
        checkpoint_id: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        connection
            .query_row(
                "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
                 FROM task_resume_checkpoints WHERE id = ?1",
                rusqlite::params![checkpoint_id],
                task_resume_checkpoint_from_row,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Task resume checkpoint {checkpoint_id}"))
                }
                other => CoreError::Database(other),
            })
    }

    pub fn get_task_resume_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let conn = self.conn();
        Self::get_task_resume_checkpoint_on_connection(&conn, checkpoint_id)
    }

    pub fn latest_task_resume_checkpoint(
        &self,
        run_id: &str,
    ) -> Result<Option<TaskResumeCheckpoint>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
             FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, id DESC
             LIMIT 1",
            rusqlite::params![run_id],
            task_resume_checkpoint_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
    }

    pub fn list_task_resume_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<TaskResumeCheckpoint>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
             FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], task_resume_checkpoint_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn build_task_resume_prompt(&self, run_id: &str) -> Result<TaskResumePrompt, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let checkpoint = self
            .latest_task_resume_checkpoint(run_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Resume checkpoint for task {run_id}")))?;
        Ok(TaskResumePrompt {
            run,
            prompt: checkpoint.resume_prompt.clone(),
            checkpoint,
        })
    }

    pub fn record_skill_usage_event(&self, input: &RecordSkillUsageInput) -> Result<(), CoreError> {
        let skill_id = normalize_required(&input.skill_id, "Skill id", 160)?;
        let outcome = normalize_required(&input.outcome, "Skill usage outcome", 40)?;
        let evidence_json = serde_json::to_string(&input.evidence)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO skill_usage_events
             (id, skill_id, conversation_id, task_run_id, outcome, evidence_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_id(),
                &skill_id,
                input.conversation_id,
                input.task_run_id,
                &outcome,
                evidence_json
            ],
        )?;
        if matches!(outcome.as_str(), "failed" | "failure" | "error") {
            let (failure_count, success_count): (i64, i64) = conn.query_row(
                "SELECT
                    SUM(CASE WHEN outcome IN ('failed', 'failure', 'error') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END)
                 FROM skill_usage_events
                 WHERE skill_id = ?1",
                rusqlite::params![&skill_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    ))
                },
            )?;
            if failure_count >= 3 && success_count == 0 {
                conn.execute(
                    "UPDATE skills
                     SET enabled = 0, updated_at = datetime('now')
                     WHERE id = ?1 AND enabled = 1",
                    rusqlite::params![&skill_id],
                )?;
            }
        }
        Ok(())
    }

    pub fn learning_governance_snapshot(&self) -> Result<LearningGovernanceSnapshot, CoreError> {
        let conn = self.conn();
        let mut stats_by_id: HashMap<String, SkillUsageStats> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, name, enabled
                 FROM skills
                 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            })?;
            for row in rows {
                let (skill_id, name, enabled) = row?;
                stats_by_id.insert(
                    skill_id.clone(),
                    SkillUsageStats {
                        skill_id,
                        name,
                        enabled,
                        usage_count: 0,
                        success_count: 0,
                        failure_count: 0,
                        last_used_at: None,
                        recent_failure_evidence: None,
                        disable_recommended: false,
                    },
                );
            }
        }

        {
            let mut stmt = conn.prepare(
                "SELECT skill_id,
                        COUNT(id) AS usage_count,
                        SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END) AS success_count,
                        SUM(CASE WHEN outcome IN ('failed', 'failure', 'error') THEN 1 ELSE 0 END) AS failure_count,
                        MAX(created_at) AS last_used_at
                 FROM skill_usage_events
                 GROUP BY skill_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u32,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u32,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u32,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            for row in rows {
                let (skill_id, usage_count, success_count, failure_count, last_used_at) = row?;
                let entry =
                    stats_by_id
                        .entry(skill_id.clone())
                        .or_insert_with(|| SkillUsageStats {
                            skill_id: skill_id.clone(),
                            name: skill_id,
                            enabled: true,
                            usage_count: 0,
                            success_count: 0,
                            failure_count: 0,
                            last_used_at: None,
                            recent_failure_evidence: None,
                            disable_recommended: false,
                        });
                entry.usage_count = usage_count;
                entry.success_count = success_count;
                entry.failure_count = failure_count;
                entry.last_used_at = last_used_at;
                entry.disable_recommended = failure_count >= 3 && success_count == 0;
            }
        }
        let mut failure_evidence = HashMap::new();
        {
            let mut evidence_stmt = conn.prepare(
                "SELECT skill_id, evidence_json
                 FROM skill_usage_events
                 WHERE outcome IN ('failed', 'failure', 'error')
                 ORDER BY datetime(created_at) DESC",
            )?;
            let rows = evidence_stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (skill_id, evidence_json) = row?;
                failure_evidence.entry(skill_id).or_insert_with(|| {
                    serde_json::from_str::<Value>(&evidence_json)
                        .unwrap_or_else(|_| serde_json::json!({}))
                });
            }
        }
        let mut stats = stats_by_id.into_values().collect::<Vec<_>>();
        for stat in &mut stats {
            stat.recent_failure_evidence = failure_evidence.get(&stat.skill_id).cloned();
            stat.disable_recommended = stat.failure_count >= 3 && stat.success_count == 0;
        }
        stats.sort_by(|left, right| {
            right
                .usage_count
                .cmp(&left.usage_count)
                .then_with(|| right.last_used_at.cmp(&left.last_used_at))
                .then_with(|| left.name.cmp(&right.name))
        });

        let pending_proposals = conn.query_row(
            "SELECT COUNT(*) FROM skill_change_proposals WHERE status = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        )? as u32;
        let procedural_memory_count = conn.query_row(
            "SELECT COUNT(*) FROM agent_procedural_memories",
            [],
            |row| row.get::<_, i64>(0),
        )? as u32;
        let memory_injection_count =
            conn.query_row("SELECT COUNT(*) FROM memory_injection_events", [], |row| {
                row.get::<_, i64>(0)
            })? as u32;

        let mut recommendations = Vec::new();
        let failed_skills = stats.iter().filter(|item| item.failure_count > 0).count();
        if failed_skills > 0 {
            recommendations.push(format!(
                "Review {failed_skills} skill(s) with recent failure evidence before broad reuse."
            ));
        }
        let stale_skills = stats
            .iter()
            .filter(|item| item.usage_count == 0 && item.enabled)
            .count();
        if stale_skills > 0 {
            recommendations.push(format!(
                "Consider disabling or rewriting {stale_skills} enabled skill(s) with no recorded usage."
            ));
        }
        if pending_proposals > 0 {
            recommendations.push(format!(
                "Review {pending_proposals} pending skill proposal(s) before they affect future tasks."
            ));
        }

        Ok(LearningGovernanceSnapshot {
            skill_stats: stats,
            pending_proposals,
            procedural_memory_count,
            memory_injection_count,
            recommendations,
        })
    }

    pub fn record_memory_injection_event(
        &self,
        memory_id: &str,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        query: &str,
        reason: &str,
        score: Option<f32>,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO memory_injection_events
             (id, memory_id, conversation_id, turn_id, query, reason, score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                new_id(),
                memory_id,
                conversation_id,
                turn_id,
                query.trim(),
                reason.trim(),
                score,
            ],
        )?;
        Ok(())
    }

    pub fn build_investigation_graph(&self, run_id: &str) -> Result<InvestigationGraph, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let events = self.get_agent_task_run_events(run_id)?;
        let artifacts = self.list_agent_task_artifacts(run_id)?;
        let persisted_artifacts = self
            .list_persisted_agent_task_artifacts(run_id)
            .unwrap_or_else(|_| Vec::new());
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut citations = BTreeSet::new();
        let mut open_questions = BTreeSet::new();

        nodes.push(InvestigationGraphNode {
            id: "question".to_string(),
            node_type: "question".to_string(),
            label: run.title.clone(),
            summary: run.summary.clone(),
            status: Some(run.status.clone()),
            source_url: None,
            created_at: Some(run.created_at.clone()),
        });

        if let Some(plan) = &run.plan {
            nodes.push(InvestigationGraphNode {
                id: "plan".to_string(),
                node_type: "plan".to_string(),
                label: "Task plan".to_string(),
                summary: Some(
                    "Route, source scope, evidence policy, and planned steps.".to_string(),
                ),
                status: Some(run.phase.clone()),
                source_url: None,
                created_at: Some(run.updated_at.clone()),
            });
            edges.push(InvestigationGraphEdge {
                from: "question".to_string(),
                to: "plan".to_string(),
                label: "planned as".to_string(),
            });
            collect_open_questions(plan, &mut open_questions);
        }

        for (index, event) in events.iter().enumerate() {
            if let Some(payload) = &event.payload {
                collect_string_field(payload, "citation", &mut citations);
                collect_string_field(payload, "cite", &mut citations);
                collect_open_questions(payload, &mut open_questions);
                if let Some(url) = payload
                    .get("url")
                    .and_then(|value| value.as_str())
                    .or_else(|| payload.get("finalUrl").and_then(|value| value.as_str()))
                {
                    let id = format!("source:{index}");
                    nodes.push(InvestigationGraphNode {
                        id: id.clone(),
                        node_type: "source".to_string(),
                        label: event.label.clone(),
                        summary: event.status.clone(),
                        status: event.status.clone(),
                        source_url: Some(url.to_string()),
                        created_at: Some(event.created_at.clone()),
                    });
                    edges.push(InvestigationGraphEdge {
                        from: "plan".to_string(),
                        to: id,
                        label: "gathered".to_string(),
                    });
                    continue;
                }
            }
            if matches!(
                event.event_type.as_str(),
                "tool" | "subtask" | "verification"
            ) {
                let id = format!("event:{index}");
                nodes.push(InvestigationGraphNode {
                    id: id.clone(),
                    node_type: event.event_type.clone(),
                    label: event.label.clone(),
                    summary: event.status.clone(),
                    status: event.status.clone(),
                    source_url: None,
                    created_at: Some(event.created_at.clone()),
                });
                edges.push(InvestigationGraphEdge {
                    from: "plan".to_string(),
                    to: id,
                    label: "recorded".to_string(),
                });
            }
        }

        for artifact in artifacts {
            collect_open_questions(&artifact.payload, &mut open_questions);
            collect_citations_from_text(&artifact.payload.to_string(), &mut citations);
            let node = artifact_to_node(&artifact);
            edges.push(InvestigationGraphEdge {
                from: "plan".to_string(),
                to: node.id.clone(),
                label: "produced".to_string(),
            });
            nodes.push(node);
        }

        for artifact in persisted_artifacts {
            if let Some(payload) = &artifact.payload {
                collect_open_questions(payload, &mut open_questions);
                collect_citations_from_text(&payload.to_string(), &mut citations);
            }
            collect_citations_from_text(&artifact.content, &mut citations);
            let node = persisted_artifact_to_node(&artifact);
            edges.push(InvestigationGraphEdge {
                from: "plan".to_string(),
                to: node.id.clone(),
                label: "saved".to_string(),
            });
            nodes.push(node);
        }

        Ok(InvestigationGraph {
            run_id: run.id,
            nodes,
            edges,
            citations: citations.into_iter().collect(),
            open_questions: open_questions.into_iter().collect(),
        })
    }

    pub fn record_browser_evidence_capture(
        &self,
        url: &str,
        final_url: &str,
        title: &str,
        excerpt: &str,
        method: &str,
    ) -> Result<BrowserEvidenceCapture, CoreError> {
        let payload = browser_evidence_payload(url, final_url, title, excerpt, method);
        let id = new_id();
        let payload_json = serde_json::to_string(&payload)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO browser_evidence_captures
             (id, url, final_url, title, excerpt, method, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![&id, url, final_url, title, excerpt, method, payload_json],
        )?;
        drop(conn);
        self.get_browser_evidence_capture(&id)
    }

    pub fn get_browser_evidence_capture(
        &self,
        id: &str,
    ) -> Result<BrowserEvidenceCapture, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, url, final_url, title, excerpt, method, payload_json, created_at
             FROM browser_evidence_captures WHERE id = ?1",
            rusqlite::params![id],
            browser_evidence_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Browser evidence capture {id}"))
            }
            other => CoreError::Database(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::WorkflowSchedulerEventType;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::agent::StreamBlockChannel;
    use crate::agent_run::AgentRunEvent;
    use crate::conversation::{AgentTaskRun, ConversationMessage, CreateConversationInput};
    use crate::db::Database;
    use crate::error::CoreError;
    use crate::llm::Role;
    use crate::workflow_automation::{
        workflow_automation_scheduler_retry_decision_from_events, SaveWorkflowAutomationInput,
        TaskResumeCheckpoint, WorkflowAutomation, WorkflowAutomationApprovalPolicy,
        WorkflowAutomationOccurrenceOrigin, WorkflowAutomationOccurrenceStatus,
        WorkflowAutomationRunStatus, WorkflowAutomationSchedulerEvent, WorkflowAutomationTrigger,
    };
    use crate::workflow_scheduler::{
        WorkflowAutomationScheduleConfig, WorkflowScheduleMisfirePolicy,
        WorkflowScheduleOverlapPolicy, WorkflowScheduleWorkspacePolicy,
    };

    fn add_user_message(
        db: &Database,
        conversation_id: &str,
        content: &str,
    ) -> ConversationMessage {
        let message = unpersisted_user_message(conversation_id, content);
        db.add_message(&message).unwrap();
        message
    }

    fn scheduled_automation(
        db: &Database,
        name: &str,
        cron: &str,
        schedule_config: WorkflowAutomationScheduleConfig,
    ) -> WorkflowAutomation {
        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: None,
                name: name.into(),
                description: "scheduler contract test".into(),
                workflow_template_id: "report_brief".into(),
                prompt: "Run the scheduled contract test.".into(),
                trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
                source_scope: Vec::new(),
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: false,
                    allowed_tools: Vec::new(),
                    risk_level: "low".into(),
                },
                enabled: true,
            },
            &schedule_config,
        )
        .unwrap()
    }

    fn scheduled_automation_requiring_approval(
        db: &Database,
        name: &str,
        cron: &str,
    ) -> WorkflowAutomation {
        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: None,
                name: name.into(),
                description: "scheduler approval contract test".into(),
                workflow_template_id: "report_brief".into(),
                prompt: "Run only after approval.".into(),
                trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
                source_scope: Vec::new(),
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: Vec::new(),
                    risk_level: "medium".into(),
                },
                enabled: true,
            },
            &WorkflowAutomationScheduleConfig::default(),
        )
        .unwrap()
    }

    fn create_test_agent_run(db: &Database, label: &str) -> AgentTaskRun {
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-test".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(db, &conversation.id, label);
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
            .unwrap();
        db.create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            label,
            Some("openai"),
            Some("gpt-test"),
        )
        .unwrap()
    }

    fn unpersisted_user_message(conversation_id: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role: Role::User,
            content: content.to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 8,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        }
    }

    fn pause_task_run_with_checkpoint(
        db: &Database,
        run_id: &str,
        reason: &str,
    ) -> TaskResumeCheckpoint {
        let checkpoint = db
            .create_task_resume_checkpoint(run_id, reason)
            .expect("create test resume checkpoint");
        db.update_agent_task_run_progress(
            run_id,
            Some("paused"),
            Some("paused"),
            None,
            Some("Paused with a resumable checkpoint"),
            None,
            Some(&serde_json::json!({
                "kind": "resumeCheckpoint",
                "checkpointId": checkpoint.id,
                "resumePrompt": checkpoint.resume_prompt,
            })),
        )
        .expect("pause test task run");
        checkpoint
    }

    fn scheduler_event(
        id: &str,
        event_type: &str,
        created_at: &str,
    ) -> WorkflowAutomationSchedulerEvent {
        WorkflowAutomationSchedulerEvent {
            id: id.to_string(),
            automation_id: Some("automation-1".to_string()),
            run_id: None,
            event_type: event_type.to_string(),
            status: None,
            summary: event_type.to_string(),
            payload: serde_json::json!({}),
            created_at: created_at.to_string(),
        }
    }

    #[test]
    fn workflow_scheduler_retry_decision_backs_off_consecutive_retryable_failures() {
        let events = vec![
            scheduler_event("event-3", "skipped_backoff", "2026-06-04T09:03:00Z"),
            scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
            scheduler_event("event-1", "claim_failed", "2026-06-04T08:40:00Z"),
        ];

        let decision = workflow_automation_scheduler_retry_decision_from_events(
            &events,
            "2026-06-04T09:10:00Z",
        )
        .unwrap();

        assert!(!decision.allowed);
        assert_eq!(decision.max_attempts, 4);
        assert!(!decision.attempts_exhausted);
        assert_eq!(decision.retryable_failure_count, 2);
        assert_eq!(
            decision.last_retryable_event_type.as_deref(),
            Some("launch_failed")
        );
        assert_eq!(decision.backoff_seconds, Some(900));
        assert_eq!(
            decision.backoff_until.as_deref(),
            Some("2026-06-04T09:15:00+00:00")
        );
        assert_eq!(decision.retry_after_seconds, Some(300));

        let elapsed = workflow_automation_scheduler_retry_decision_from_events(
            &events,
            "2026-06-04T09:15:00Z",
        )
        .unwrap();
        assert!(elapsed.allowed);
        assert!(!elapsed.attempts_exhausted);
        assert_eq!(elapsed.retryable_failure_count, 2);
        assert_eq!(elapsed.retry_after_seconds, None);
    }

    #[test]
    fn workflow_scheduler_retry_decision_blocks_after_max_attempts() {
        let events = vec![
            scheduler_event("event-5", "skipped_retry_limit", "2026-06-04T10:00:00Z"),
            scheduler_event("event-4", "launch_failed", "2026-06-04T09:00:00Z"),
            scheduler_event("event-3", "claim_failed", "2026-06-04T08:00:00Z"),
            scheduler_event("event-2", "launch_failed", "2026-06-04T07:00:00Z"),
            scheduler_event("event-1", "claim_failed", "2026-06-04T06:00:00Z"),
        ];

        let decision = workflow_automation_scheduler_retry_decision_from_events(
            &events,
            "2026-06-04T20:00:00Z",
        )
        .unwrap();

        assert!(!decision.allowed);
        assert_eq!(decision.max_attempts, 4);
        assert!(decision.attempts_exhausted);
        assert_eq!(decision.retryable_failure_count, 4);
        assert_eq!(
            decision.last_retryable_event_type.as_deref(),
            Some("launch_failed")
        );
        assert_eq!(decision.backoff_seconds, Some(14_400));
        assert_eq!(decision.backoff_until, None);
        assert_eq!(decision.retry_after_seconds, None);
    }

    #[test]
    fn workflow_scheduler_retry_decision_resets_after_progress_or_non_retry_gate() {
        let after_progress = workflow_automation_scheduler_retry_decision_from_events(
            &[
                scheduler_event("event-3", "launch_succeeded", "2026-06-04T09:05:00Z"),
                scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
                scheduler_event("event-1", "claim_failed", "2026-06-04T08:55:00Z"),
            ],
            "2026-06-04T09:06:00Z",
        )
        .unwrap();
        assert!(after_progress.allowed);
        assert!(!after_progress.attempts_exhausted);
        assert_eq!(after_progress.retryable_failure_count, 0);
        assert_eq!(after_progress.backoff_until, None);

        let after_approval_gate = workflow_automation_scheduler_retry_decision_from_events(
            &[
                scheduler_event(
                    "event-3",
                    "skipped_pre_run_approval",
                    "2026-06-04T09:05:00Z",
                ),
                scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
            ],
            "2026-06-04T09:06:00Z",
        )
        .unwrap();
        assert!(after_approval_gate.allowed);
        assert!(!after_approval_gate.attempts_exhausted);
        assert_eq!(after_approval_gate.retryable_failure_count, 0);
    }

    #[test]
    fn automation_lifecycle_computes_due_runs_and_audits_policy() {
        let db = Database::open_memory().unwrap();
        let saved = db
            .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
                id: None,
                name: "Morning inbox brief".into(),
                description: "Summarize new source material every morning.".into(),
                workflow_template_id: "report_brief".into(),
                prompt: "Summarize new documents in this source scope.".into(),
                trigger: crate::workflow_automation::WorkflowAutomationTrigger::Schedule {
                    cron: "0 9 * * *".into(),
                },
                source_scope: vec!["source-a".into()],
                approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: vec!["search_knowledge_base".into()],
                    risk_level: "medium".into(),
                },
                enabled: true,
            })
            .unwrap();

        assert_eq!(saved.trigger_kind, "schedule");
        assert!(saved.next_run_at.is_some());
        assert!(saved.approval_policy.require_before_run);

        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].automation.id, saved.id);
        assert!(due[0].prompt.contains("Morning inbox brief"));
    }

    #[test]
    fn invalid_cron_or_timezone_is_rejected_before_automation_is_saved() {
        let db = Database::open_memory().unwrap();
        for enabled in [true, false] {
            for (cron, timezone) in [
                ("61 9 * * *", "UTC"),
                ("0 9 * *", "UTC"),
                ("0 9 * * *", "Mars/Olympus"),
            ] {
                let mut config = WorkflowAutomationScheduleConfig::default();
                config.timezone = timezone.into();
                let result = db.save_workflow_automation_with_schedule_config(
                    &SaveWorkflowAutomationInput {
                        id: None,
                        name: format!("invalid-{enabled}-{cron}-{timezone}"),
                        description: String::new(),
                        workflow_template_id: "report_brief".into(),
                        prompt: "must not persist".into(),
                        trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
                        source_scope: Vec::new(),
                        approval_policy: WorkflowAutomationApprovalPolicy::default(),
                        enabled,
                    },
                    &config,
                );
                assert!(result.is_err(), "enabled={enabled} {cron} {timezone}");
            }
        }
        assert!(db.list_workflow_automations().unwrap().is_empty());
    }

    #[test]
    fn approval_occurrence_is_durable_actionable_and_single_winner() {
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation_with_schedule_config(
                &SaveWorkflowAutomationInput {
                    id: None,
                    name: "approval-cas".into(),
                    description: String::new(),
                    workflow_template_id: "report_brief".into(),
                    prompt: "Wait for approval.".into(),
                    trigger: WorkflowAutomationTrigger::Schedule {
                        cron: "0 9 * * *".into(),
                    },
                    source_scope: Vec::new(),
                    approval_policy: WorkflowAutomationApprovalPolicy {
                        require_before_run: true,
                        allowed_tools: Vec::new(),
                        risk_level: "medium".into(),
                    },
                    enabled: true,
                },
                &WorkflowAutomationScheduleConfig::default(),
            )
            .unwrap();
        let expected_next = automation.next_run_at.clone().unwrap();
        let due = db
            .list_due_workflow_automations(&expected_next)
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, &expected_next, None)
            .unwrap();
        let run = claim.run.as_ref().unwrap();

        assert!(db
            .mark_workflow_automation_run_waiting_approval(&run.id)
            .unwrap());
        assert!(!db
            .mark_workflow_automation_run_waiting_approval(&run.id)
            .unwrap());

        let paused = db.get_workflow_automation(&automation.id).unwrap();
        assert!(paused.enabled);
        assert_eq!(paused.status, "waiting_approval");
        assert_eq!(paused.next_run_at.as_deref(), Some(expected_next.as_str()));
        let waiting = db.list_workflow_automation_runs_waiting_approval().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].id, run.id);
        assert!(db
            .list_due_workflow_automations(&expected_next)
            .unwrap()
            .is_empty());
        let events = db
            .list_workflow_automation_scheduler_events(Some(&automation.id), 10)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "approval_requested");
        assert_eq!(events[0].status.as_deref(), Some("waiting_approval"));
        let approved = db
            .approve_workflow_automation_run_at(&run.id, &expected_next)
            .unwrap();
        assert_eq!(
            approved.run.as_ref().unwrap().status,
            WorkflowAutomationRunStatus::Queued
        );
        assert_eq!(
            approved.occurrence.as_ref().unwrap().status,
            WorkflowAutomationOccurrenceStatus::Claimed
        );
        assert_eq!(
            db.workflow_automation_occurrence_approval_state(
                approved.occurrence.as_ref().unwrap().id.as_str()
            )
            .unwrap(),
            super::WorkflowAutomationApprovalState::Approved
        );
        assert!(db
            .list_workflow_automation_runs_waiting_approval()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn denied_approval_consumes_occurrence_and_advances_schedule() {
        let db = Database::open_memory().unwrap();
        let automation =
            scheduled_automation_requiring_approval(&db, "approval-denied", "0 9 * * *");
        let scheduled_for = automation.next_run_at.clone().unwrap();
        let due = db
            .list_due_workflow_automations(&scheduled_for)
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, &scheduled_for, None)
            .unwrap();
        let run = claim.run.as_ref().unwrap();
        db.mark_workflow_automation_run_waiting_approval(&run.id)
            .unwrap();

        let denied = db
            .deny_workflow_automation_run_at(&run.id, &scheduled_for)
            .unwrap();
        assert_eq!(denied.status, WorkflowAutomationRunStatus::Cancelled);
        let occurrence = db
            .get_workflow_automation_occurrence(
                denied.occurrence_id.as_deref().expect("occurrence id"),
            )
            .unwrap();
        assert_eq!(
            occurrence.status,
            WorkflowAutomationOccurrenceStatus::Skipped
        );
        assert_eq!(
            occurrence.last_error.as_deref(),
            Some("pre_run_approval_denied")
        );
        assert_eq!(
            db.workflow_automation_occurrence_approval_state(&occurrence.id)
                .unwrap(),
            super::WorkflowAutomationApprovalState::Denied
        );
        let advanced = db.get_workflow_automation(&automation.id).unwrap();
        assert_eq!(advanced.status, "ready");
        assert!(
            super::parse_utc_timestamp(advanced.next_run_at.as_deref().unwrap()).unwrap()
                > super::parse_utc_timestamp(&scheduled_for).unwrap()
        );
    }

    #[test]
    fn manual_run_now_uses_durable_approval_without_consuming_cron_cursor() {
        let db = Database::open_memory().unwrap();
        let automation =
            scheduled_automation_requiring_approval(&db, "manual-run-now", "0 9 * * *");
        let recurring_cursor = automation.next_run_at.clone().unwrap();
        let now = Utc::now().to_rfc3339();
        let due = db
            .workflow_automation_run_now_due_at(&automation.id, &now)
            .unwrap();
        assert_eq!(due.origin, WorkflowAutomationOccurrenceOrigin::ManualRunNow);
        let claim = db
            .claim_workflow_automation_due_run_at(due, &now, Some("run now"))
            .unwrap();
        let run = claim.run.as_ref().unwrap();
        assert!(db
            .mark_workflow_automation_run_waiting_approval(&run.id)
            .unwrap());
        assert_eq!(
            db.get_workflow_automation(&automation.id)
                .unwrap()
                .next_run_at
                .as_deref(),
            Some(recurring_cursor.as_str())
        );

        db.deny_workflow_automation_run_at(&run.id, &now).unwrap();
        let restored = db.get_workflow_automation(&automation.id).unwrap();
        assert_eq!(restored.status, "ready");
        assert_eq!(
            restored.next_run_at.as_deref(),
            Some(recurring_cursor.as_str())
        );
    }

    #[test]
    fn starting_manual_run_now_preserves_recurring_cursor() {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            "manual-run-now-start",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let recurring_cursor = automation.next_run_at.clone().unwrap();
        let now = Utc::now().to_rfc3339();
        let due = db
            .workflow_automation_run_now_due_at(&automation.id, &now)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, &now, None)
            .unwrap();
        let task = create_test_agent_run(&db, "manual run now task");
        db.start_workflow_automation_run_at(&claim.run.as_ref().unwrap().id, &task.id, None, &now)
            .unwrap();
        assert_eq!(
            db.get_workflow_automation(&automation.id)
                .unwrap()
                .next_run_at
                .as_deref(),
            Some(recurring_cursor.as_str())
        );
    }

    #[test]
    fn schedule_revision_remains_monotonic_across_trigger_kind_round_trip() {
        let db = Database::open_memory().unwrap();
        let scheduled = scheduled_automation(
            &db,
            "schedule-round-trip",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let base = SaveWorkflowAutomationInput {
            id: Some(scheduled.id.clone()),
            name: scheduled.name.clone(),
            description: scheduled.description.clone(),
            workflow_template_id: scheduled.workflow_template_id.clone(),
            prompt: scheduled.prompt.clone(),
            trigger: WorkflowAutomationTrigger::Manual,
            source_scope: scheduled.source_scope.clone(),
            approval_policy: scheduled.approval_policy.clone(),
            enabled: true,
        };
        db.save_workflow_automation_with_schedule_config(
            &base,
            &WorkflowAutomationScheduleConfig::default(),
        )
        .unwrap();
        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                trigger: WorkflowAutomationTrigger::Schedule {
                    cron: "30 9 * * *".into(),
                },
                ..base
            },
            &WorkflowAutomationScheduleConfig::default(),
        )
        .unwrap();
        let revisions: Vec<i64> = {
            let conn = db.conn();
            let mut stmt = conn
                .prepare(
                    "SELECT revision FROM workflow_automation_definition_revisions
                     WHERE automation_id = ?1 ORDER BY revision",
                )
                .unwrap();
            stmt.query_map([&scheduled.id], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(revisions, vec![1, 2]);
    }

    #[test]
    fn editing_definition_snapshots_revision_and_explicitly_cancels_waiting_occurrence() {
        let db = Database::open_memory().unwrap();
        let automation =
            scheduled_automation_requiring_approval(&db, "revision-cancel", "0 9 * * *");
        let scheduled_for = automation.next_run_at.clone().unwrap();
        let due = db
            .list_due_workflow_automations(&scheduled_for)
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, &scheduled_for, None)
            .unwrap();
        let run = claim.run.as_ref().unwrap().clone();
        db.mark_workflow_automation_run_waiting_approval(&run.id)
            .unwrap();

        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: Some(automation.id.clone()),
                name: automation.name.clone(),
                description: automation.description.clone(),
                workflow_template_id: automation.workflow_template_id.clone(),
                prompt: "Run the revised definition only.".into(),
                trigger: automation.trigger.clone(),
                source_scope: automation.source_scope.clone(),
                approval_policy: automation.approval_policy.clone(),
                enabled: true,
            },
            &WorkflowAutomationScheduleConfig::default(),
        )
        .unwrap();

        assert_eq!(
            db.get_workflow_automation_run(&run.id).unwrap().status,
            WorkflowAutomationRunStatus::Cancelled
        );
        let occurrence = db
            .get_workflow_automation_occurrence(run.occurrence_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            occurrence.status,
            WorkflowAutomationOccurrenceStatus::Cancelled
        );
        assert_eq!(
            occurrence.last_error.as_deref(),
            Some("definition_superseded")
        );
        assert!(db
            .list_workflow_automation_runs_waiting_approval()
            .unwrap()
            .is_empty());
        let snapshot_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM workflow_automation_definition_revisions
                 WHERE automation_id = ?1",
                [&automation.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(snapshot_count, 2);
    }

    #[test]
    fn damaged_unknown_or_missing_schedule_config_is_projected_fail_closed() {
        for scenario in ["damaged", "unknown-version", "missing"] {
            let db = Database::open_memory().unwrap();
            let automation = scheduled_automation(
                &db,
                scenario,
                "0 9 * * *",
                WorkflowAutomationScheduleConfig::default(),
            );
            match scenario {
                "damaged" => {
                    db.conn()
                        .execute(
                            "UPDATE workflow_automation_schedule_configs
                             SET config_json = '{' WHERE automation_id = ?1",
                            [&automation.id],
                        )
                        .unwrap();
                }
                "unknown-version" => {
                    db.conn()
                        .execute(
                            "UPDATE workflow_automation_schedule_configs
                             SET config_json = '{\"version\":99}' WHERE automation_id = ?1",
                            [&automation.id],
                        )
                        .unwrap();
                }
                "missing" => {
                    db.conn()
                        .execute(
                            "DELETE FROM workflow_automation_schedule_configs
                             WHERE automation_id = ?1",
                            [&automation.id],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }

            let loaded = db.get_workflow_automation(&automation.id).unwrap();
            assert!(!loaded.enabled, "{scenario}");
            assert_eq!(loaded.status, "needs_review", "{scenario}");
            assert!(loaded.next_run_at.is_none(), "{scenario}");
            assert!(loaded.schedule_config.legacy_needs_review, "{scenario}");
            assert!(
                db.list_due_workflow_automations("2099-01-01T09:00:00Z")
                    .unwrap()
                    .is_empty(),
                "{scenario}"
            );
        }
    }

    #[test]
    fn live_occurrence_claim_is_idempotent_and_expired_lease_creates_new_attempt() {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            "lease",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let first = db
            .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
            .unwrap();
        let duplicate = db
            .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:30Z", None)
            .unwrap();
        assert!(duplicate.run.is_none());
        assert_eq!(
            duplicate.skip_reason.as_deref(),
            Some("already_claimed_live")
        );
        assert_eq!(
            duplicate.occurrence.as_ref().unwrap().id,
            first.occurrence.as_ref().unwrap().id
        );
        let run_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM workflow_automation_runs WHERE automation_id = ?1",
                [&automation.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 1);

        let reclaimed = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:03:00Z", None)
            .unwrap();
        assert_eq!(reclaimed.run.as_ref().unwrap().attempt, 2);
        assert_eq!(
            reclaimed.occurrence.as_ref().unwrap().id,
            first.occurrence.as_ref().unwrap().id
        );
    }

    #[test]
    fn superseded_occurrence_attempt_cannot_start_after_lease_reclaim() {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            "lease-fence",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let first = db
            .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
            .unwrap();
        let reclaimed = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:03:00Z", None)
            .unwrap();
        let first_run = first.run.as_ref().unwrap();
        let reclaimed_run = reclaimed.run.as_ref().unwrap();
        let stale_task = create_test_agent_run(&db, "stale scheduled start");

        let stale_start = db.start_workflow_automation_run_at(
            &first_run.id,
            &stale_task.id,
            None,
            "2099-01-01T09:03:01Z",
        );
        assert!(stale_start.is_err());
        assert_eq!(
            db.get_workflow_automation_run(&first_run.id)
                .unwrap()
                .status,
            WorkflowAutomationRunStatus::Cancelled
        );

        let current_task = create_test_agent_run(&db, "current scheduled start");
        db.start_workflow_automation_run_at(
            &reclaimed_run.id,
            &current_task.id,
            None,
            "2099-01-01T09:03:01Z",
        )
        .unwrap();
        assert_eq!(
            db.get_workflow_automation_occurrence(
                reclaimed.occurrence.as_ref().unwrap().id.as_str()
            )
            .unwrap()
            .status,
            WorkflowAutomationOccurrenceStatus::Running
        );
    }

    #[test]
    fn claim_reloads_the_authoritative_definition_snapshot_and_revision() {
        let db = Database::open_memory().unwrap();
        let mut first_config = WorkflowAutomationScheduleConfig::default();
        first_config.execution_policy.model = Some("old-model".into());
        let automation =
            scheduled_automation(&db, "definition-snapshot", "0 0 1 1 *", first_config);
        let stale_due = db
            .list_due_workflow_automations("2099-01-01T00:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();

        let mut current_config = WorkflowAutomationScheduleConfig::default();
        current_config.execution_policy.model = Some("current-model".into());
        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: Some(automation.id.clone()),
                name: automation.name.clone(),
                description: automation.description.clone(),
                workflow_template_id: automation.workflow_template_id.clone(),
                prompt: "Run the current definition snapshot.".into(),
                trigger: automation.trigger.clone(),
                source_scope: automation.source_scope.clone(),
                approval_policy: automation.approval_policy.clone(),
                enabled: true,
            },
            &current_config,
        )
        .unwrap();

        let claim = db
            .claim_workflow_automation_due_run_at(stale_due, "2099-01-01T00:00:00Z", None)
            .unwrap();
        assert_eq!(claim.occurrence.as_ref().unwrap().definition_revision, 2);
        assert_eq!(
            claim
                .due_run
                .automation
                .schedule_config
                .execution_policy
                .model
                .as_deref(),
            Some("current-model")
        );
        assert!(claim
            .due_run
            .prompt
            .contains("Run the current definition snapshot."));
        assert!(!claim.due_run.prompt.contains("old-model"));
    }

    #[test]
    fn overlap_policy_is_enforced_inside_atomic_claim() {
        for (policy, expects_run, reason) in [
            (
                WorkflowScheduleOverlapPolicy::Skip,
                false,
                Some("overlap_active"),
            ),
            (WorkflowScheduleOverlapPolicy::Allow, true, None),
        ] {
            let db = Database::open_memory().unwrap();
            let mut config = WorkflowAutomationScheduleConfig::default();
            config.overlap_policy = policy;
            let automation = scheduled_automation(&db, "overlap", "0 9 * * *", config);
            let existing = db
                .record_workflow_automation_run(&automation.id, None, "queued", Some("existing"))
                .unwrap();
            let task = create_test_agent_run(&db, "existing active run");
            db.start_workflow_automation_run(&existing.id, &task.id, None)
                .unwrap();
            let due = db
                .list_due_workflow_automations("2099-01-01T09:00:00Z")
                .unwrap()
                .into_iter()
                .find(|item| item.automation.id == automation.id)
                .unwrap();
            let claim = db
                .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
                .unwrap();
            assert_eq!(claim.run.is_some(), expects_run);
            assert_eq!(claim.skip_reason.as_deref(), reason);
        }
    }

    #[test]
    fn isolated_schedules_lock_one_source_across_automation_definitions() {
        let db = Database::open_memory().unwrap();
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.workspace_policy = WorkflowScheduleWorkspacePolicy::IsolatedPatch;
        config.execution_policy.orchestration_profile = "codeUltra".into();
        config.execution_policy.source_root_fingerprint = Some("blake3:test-source".into());
        let save = |name: &str, source_id: &str| {
            db.save_workflow_automation_with_schedule_config(
                &SaveWorkflowAutomationInput {
                    id: None,
                    name: name.into(),
                    description: String::new(),
                    workflow_template_id: "report_brief".into(),
                    prompt: "Apply one isolated patch.".into(),
                    trigger: WorkflowAutomationTrigger::Schedule {
                        cron: "0 9 * * *".into(),
                    },
                    source_scope: vec![source_id.into()],
                    approval_policy: WorkflowAutomationApprovalPolicy {
                        require_before_run: false,
                        allowed_tools: vec!["edit_file".into()],
                        risk_level: "high".into(),
                    },
                    enabled: true,
                },
                &config,
            )
            .unwrap()
        };
        let first = save("isolated-lock-first", "source-canonical");
        let second = save("isolated-lock-second", "source-alias");
        let at = [
            first.next_run_at.clone().unwrap(),
            second.next_run_at.clone().unwrap(),
        ]
        .into_iter()
        .max()
        .unwrap();
        let due = db.list_due_workflow_automations(&at).unwrap();
        let first_due = due
            .iter()
            .find(|item| item.automation.id == first.id)
            .unwrap()
            .clone();
        let second_due = due
            .iter()
            .find(|item| item.automation.id == second.id)
            .unwrap()
            .clone();

        assert!(db
            .claim_workflow_automation_due_run_at(first_due, &at, None)
            .unwrap()
            .run
            .is_some());
        let blocked = db
            .claim_workflow_automation_due_run_at(second_due, &at, None)
            .unwrap();
        assert!(blocked.run.is_none());
        assert_eq!(
            blocked.skip_reason.as_deref(),
            Some("source_workspace_locked")
        );
        assert_eq!(
            blocked.occurrence.as_ref().unwrap().status,
            WorkflowAutomationOccurrenceStatus::Planned
        );
    }

    #[test]
    fn misfire_policy_skips_or_runs_latest_occurrence() {
        for (policy, expects_run) in [
            (WorkflowScheduleMisfirePolicy::Skip, false),
            (WorkflowScheduleMisfirePolicy::RunLatest, true),
        ] {
            let db = Database::open_memory().unwrap();
            let mut config = WorkflowAutomationScheduleConfig::default();
            config.misfire_policy = policy;
            config.misfire_grace_seconds = 60;
            let automation = scheduled_automation(&db, "misfire", "0 9 * * *", config);
            let due = db
                .list_due_workflow_automations("2099-01-01T09:00:00Z")
                .unwrap()
                .into_iter()
                .find(|item| item.automation.id == automation.id)
                .unwrap();
            let claim = db
                .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
                .unwrap();
            assert_eq!(claim.run.is_some(), expects_run);
            if policy == WorkflowScheduleMisfirePolicy::Skip {
                assert_eq!(claim.skip_reason.as_deref(), Some("misfire_grace_exceeded"));
                assert_eq!(
                    claim.occurrence.unwrap().status,
                    WorkflowAutomationOccurrenceStatus::Skipped
                );
            }
        }
    }

    #[test]
    fn run_latest_materializes_the_last_missed_occurrence_at_or_before_now() {
        let db = Database::open_memory().unwrap();
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.misfire_policy = WorkflowScheduleMisfirePolicy::RunLatest;
        let automation = scheduled_automation(&db, "latest-misfire", "0 0 1 1 *", config);
        let due = db
            .list_due_workflow_automations("2099-08-27T12:34:56Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();

        let claim = db
            .claim_workflow_automation_due_run_at(due, "2099-08-27T12:34:56Z", None)
            .unwrap();
        assert_eq!(
            claim.occurrence.as_ref().unwrap().scheduled_for,
            "2099-01-01T00:00:00+00:00"
        );
        assert_eq!(
            claim.run.as_ref().unwrap().scheduled_for.as_deref(),
            Some("2099-01-01T00:00:00+00:00")
        );
        assert_eq!(
            claim.due_run.scheduled_for.as_deref(),
            Some("2099-01-01T00:00:00+00:00")
        );
    }

    #[test]
    fn launch_failure_retries_same_occurrence_with_a_new_attempt_run() {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            "retry",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let first = db
            .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
            .unwrap();
        let first_run = first.run.as_ref().unwrap();
        db.mark_workflow_automation_launch_failed_for_retry(
            &first_run.id,
            "provider unavailable",
            "2099-01-01T09:00:00Z",
        )
        .unwrap();
        let second = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:05:01Z", None)
            .unwrap();
        let second_run = second.run.as_ref().unwrap();
        assert_ne!(first_run.id, second_run.id);
        assert_eq!(second_run.attempt, 2);
        assert_eq!(first_run.occurrence_id, second_run.occurrence_id);
        assert_eq!(
            db.get_workflow_automation_run(&first_run.id)
                .unwrap()
                .status,
            WorkflowAutomationRunStatus::Cancelled
        );
    }

    #[test]
    fn retry_wait_occurrence_is_not_reclassified_as_a_misfire() {
        let db = Database::open_memory().unwrap();
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.misfire_policy = WorkflowScheduleMisfirePolicy::Skip;
        config.misfire_grace_seconds = 60;
        let automation = scheduled_automation(&db, "retry-misfire", "0 9 * * *", config);
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let scheduled_for = due.scheduled_for.clone().unwrap();
        let first = db
            .claim_workflow_automation_due_run_at(due.clone(), &scheduled_for, None)
            .unwrap();
        let first_run = first.run.as_ref().unwrap();
        db.mark_workflow_automation_launch_failed_for_retry(
            &first_run.id,
            "provider unavailable",
            &scheduled_for,
        )
        .unwrap();
        let retry_now = (super::parse_utc_timestamp(&scheduled_for).unwrap()
            + chrono::Duration::minutes(10))
        .to_rfc3339();

        let retry = db
            .claim_workflow_automation_due_run_at(due, &retry_now, None)
            .unwrap();
        assert!(retry.skip_reason.is_none());
        assert_eq!(retry.run.as_ref().unwrap().attempt, 2);
        assert_eq!(first_run.occurrence_id, retry.run.unwrap().occurrence_id);
    }

    #[test]
    fn starting_occurrence_advances_schedule_and_completion_updates_occurrence() {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            "advance",
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
            .unwrap();
        let run = claim.run.as_ref().unwrap();
        let task = create_test_agent_run(&db, "scheduled start");
        db.start_workflow_automation_run_at(&run.id, &task.id, None, "2099-01-01T09:00:00Z")
            .unwrap();
        let advanced = db.get_workflow_automation(&automation.id).unwrap();
        assert!(
            super::parse_utc_timestamp(advanced.next_run_at.as_deref().unwrap()).unwrap()
                > super::parse_utc_timestamp("2099-01-01T09:00:00Z").unwrap()
        );
        db.transition_workflow_automation_run(&run.id, "completed", Some("done"))
            .unwrap();
        assert_eq!(
            db.get_workflow_automation_occurrence(
                claim
                    .occurrence
                    .as_ref()
                    .map(|item| item.id.as_str())
                    .unwrap()
            )
            .unwrap()
            .status,
            WorkflowAutomationOccurrenceStatus::Completed
        );
    }

    #[test]
    fn folder_trigger_detects_matching_files_and_advances_after_run() {
        let db = Database::open_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let saved = db
            .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
                id: None,
                name: "PDF actions".into(),
                description: "Extract actions when PDFs appear.".into(),
                workflow_template_id: "document_compare".into(),
                prompt: "Extract action items from new PDFs.".into(),
                trigger: crate::workflow_automation::WorkflowAutomationTrigger::Folder {
                    path: dir.path().display().to_string(),
                    pattern: "*.pdf".into(),
                },
                source_scope: vec![],
                approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: vec!["read_file".into()],
                    risk_level: "medium".into(),
                },
                enabled: true,
            })
            .unwrap();

        assert!(db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .is_empty());
        std::fs::write(dir.path().join("incoming.pdf"), b"%PDF-1.4").unwrap();
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].automation.id, saved.id);
        assert!(due[0].due_reason.contains("folder trigger"));

        db.record_workflow_automation_run(&saved.id, None, "completed", Some("done"))
            .unwrap();
        assert!(db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn due_workflow_claim_creates_queued_run_and_advances_folder_trigger() {
        let db = Database::open_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let saved = db
            .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
                id: None,
                name: "Claim PDFs".into(),
                description: "Claim new PDFs for processing.".into(),
                workflow_template_id: "document_compare".into(),
                prompt: "Review new PDFs.".into(),
                trigger: crate::workflow_automation::WorkflowAutomationTrigger::Folder {
                    path: dir.path().display().to_string(),
                    pattern: "*.pdf".into(),
                },
                source_scope: vec!["source-a".into()],
                approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: vec!["read_file".into()],
                    risk_level: "medium".into(),
                },
                enabled: true,
            })
            .unwrap();
        std::fs::write(dir.path().join("incoming.pdf"), b"%PDF-1.4").unwrap();

        let claim = db
            .claim_due_workflow_automation_run(&saved.id, "2099-01-01T09:00:00Z", None)
            .unwrap();
        let run = claim.run.as_ref().expect("folder claim creates a run");

        assert_eq!(claim.due_run.automation.id, saved.id);
        assert_eq!(run.automation_id, saved.id);
        assert_eq!(run.status, WorkflowAutomationRunStatus::Queued);
        assert_eq!(
            run.summary.as_deref(),
            Some(claim.due_run.due_reason.as_str())
        );
        assert_eq!(
            db.get_workflow_automation(&saved.id).unwrap().status,
            "queued"
        );
        assert!(db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn workflow_automation_run_binds_to_agent_task_run_and_transitions() {
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
                id: None,
                name: "Manual brief".into(),
                description: "Run a scoped brief on demand.".into(),
                workflow_template_id: "report_brief".into(),
                prompt: "Summarize this source scope.".into(),
                trigger: crate::workflow_automation::WorkflowAutomationTrigger::Manual,
                source_scope: vec!["source-a".into()],
                approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: false,
                    allowed_tools: vec![],
                    risk_level: "low".into(),
                },
                enabled: true,
            })
            .unwrap();
        let queued_run = db
            .record_workflow_automation_run(&automation.id, None, "queued", Some("queued"))
            .unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Run the brief");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
            .unwrap();
        let task_run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "Manual brief",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();

        let running = db
            .start_workflow_automation_run(
                &queued_run.id,
                &task_run.id,
                Some("Agent session started"),
            )
            .unwrap();

        assert_eq!(running.status, WorkflowAutomationRunStatus::Running);
        assert_eq!(running.task_run_id.as_deref(), Some(task_run.id.as_str()));
        assert_eq!(running.summary.as_deref(), Some("Agent session started"));
        assert_eq!(
            db.get_workflow_automation(&automation.id).unwrap().status,
            "running"
        );

        let completed = db
            .transition_workflow_automation_run(&queued_run.id, "completed", Some("done"))
            .unwrap();

        assert_eq!(completed.status, WorkflowAutomationRunStatus::Completed);
        assert_eq!(completed.summary.as_deref(), Some("done"));
        assert!(completed.finished_at.is_some());
        assert_eq!(
            db.get_workflow_automation(&automation.id).unwrap().status,
            "completed"
        );

        let restart_err = db
            .start_workflow_automation_run(&queued_run.id, &task_run.id, None)
            .unwrap_err();
        assert!(restart_err.to_string().contains("terminal task state"));
    }

    #[test]
    fn workflow_scheduler_events_persist_payload_and_filter_by_automation() {
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
                id: None,
                name: "Scheduler audit".into(),
                description: "Audit scheduler decisions.".into(),
                workflow_template_id: "report_brief".into(),
                prompt: "Summarize due work.".into(),
                trigger: crate::workflow_automation::WorkflowAutomationTrigger::Manual,
                source_scope: vec![],
                approval_policy: Default::default(),
                enabled: true,
            })
            .unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Run the scheduler audit");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
            .unwrap();
        let task_run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "Scheduler audit",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let run = db
            .record_workflow_automation_run(
                &automation.id,
                Some(&task_run.id),
                "queued",
                Some("queued"),
            )
            .unwrap();

        let event = db
            .record_workflow_automation_scheduler_event(
                Some(&automation.id),
                Some(&run.id),
                WorkflowSchedulerEventType::LaunchSucceeded,
                Some("running"),
                "Scheduler launched workflow",
                Some(&serde_json::json!({
                    "queueId": format!("workflow_due:{}", automation.id),
                    "delivery": "scheduler"
                })),
            )
            .unwrap();

        assert_eq!(event.automation_id.as_deref(), Some(automation.id.as_str()));
        assert_eq!(event.run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(event.event_type, "launch_succeeded");
        assert_eq!(event.status.as_deref(), Some("running"));
        assert_eq!(event.payload["delivery"], "scheduler");

        let events = db
            .list_workflow_automation_scheduler_events(Some(&automation.id), 10)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event.id);

        let all_events = db
            .list_workflow_automation_scheduler_events(None, 10)
            .unwrap();
        assert_eq!(all_events.len(), 1);

        let run_events = db
            .list_workflow_automation_scheduler_events_for_run(&run.id, 10)
            .unwrap();
        assert_eq!(run_events.len(), 1);
        assert_eq!(run_events[0].id, event.id);

        let unrelated_run_events = db
            .list_workflow_automation_scheduler_events_for_run("missing-run", 10)
            .unwrap();
        assert!(unrelated_run_events.is_empty());

        let task_run_events = db
            .list_workflow_automation_scheduler_events_for_task_run(&task_run.id, 10)
            .unwrap();
        assert_eq!(task_run_events.len(), 1);
        assert_eq!(task_run_events[0].id, event.id);

        let unrelated_task_run_events = db
            .list_workflow_automation_scheduler_events_for_task_run("missing-task-run", 10)
            .unwrap();
        assert!(unrelated_task_run_events.is_empty());
    }

    #[test]
    fn task_resume_checkpoint_builds_a_resume_prompt() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Compare these documents");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("knowledge"))
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "Document comparison",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let plan = serde_json::json!({ "steps": [{ "id": "map", "status": "completed" }] });
        db.update_agent_task_run_progress(
            &run.id,
            Some("running"),
            Some("compare"),
            Some("knowledge"),
            Some("Mapped the input documents"),
            Some(&plan),
            None,
        )
        .unwrap();

        let checkpoint = db
            .create_task_resume_checkpoint(&run.id, "user_pause")
            .unwrap();
        assert!(checkpoint.resume_prompt.contains("Document comparison"));
        assert!(checkpoint
            .resume_prompt
            .contains("Do not redo completed tool work"));
        assert!(checkpoint
            .resume_prompt
            .contains("Start by naming the resumed checkpoint"));
        assert!(checkpoint.state.get("run").is_some());

        let prompt = db.build_task_resume_prompt(&run.id).unwrap();
        assert!(prompt.prompt.contains("Resume this Nexa task"));
    }

    #[test]
    fn task_resume_checkpoint_can_embed_live_turn_state() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Research for a while");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("web"))
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "Long research task",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let live_state = serde_json::json!({
            "kind": "longTaskLiveState",
            "iteration": 3,
            "taskPlan": {
                "objective": "Research for a while",
                "steps": []
            }
        });

        let checkpoint = db
            .create_task_resume_checkpoint_with_state(
                &run.id,
                "auto_tool_round_3",
                Some(&live_state),
            )
            .unwrap();

        assert_eq!(
            checkpoint.state["liveTurnState"]["kind"].as_str(),
            Some("longTaskLiveState")
        );
        assert_eq!(
            checkpoint.state["liveTurnState"]["taskPlan"]["objective"].as_str(),
            Some("Research for a while")
        );
        assert!(checkpoint
            .resume_prompt
            .contains("Prefer liveTurnState.taskPlan"));
    }

    #[test]
    fn task_resume_checkpoint_carries_partial_assistant_output_forward() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Explain the result");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("chat"))
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &user.id,
                "Partial response",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        db.save_agent_run_event(&AgentRunEvent::output_delta(
            &run.id,
            Some(&turn.id),
            1,
            "answer-block",
            StreamBlockChannel::Answer,
            0,
            "Partial ",
        ))
        .unwrap();
        db.save_agent_run_event(&AgentRunEvent::output_delta(
            &run.id,
            Some(&turn.id),
            2,
            "answer-block",
            StreamBlockChannel::Answer,
            8,
            "answer",
        ))
        .unwrap();

        let checkpoint = db
            .create_task_resume_checkpoint(&run.id, "user_stop")
            .unwrap();

        assert_eq!(
            checkpoint.state["partialAssistantOutput"]["text"].as_str(),
            Some("Partial answer")
        );
        assert!(checkpoint.resume_prompt.contains("Partial answer"));
        assert!(checkpoint
            .resume_prompt
            .contains("continue after it without repeating it"));
    }

    #[test]
    fn task_checkpoint_resume_requeues_the_original_turn_and_run_atomically() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let original_message = add_user_message(&db, &conversation.id, "Continue this work");
        let turn = db
            .create_conversation_turn(&conversation.id, &original_message.id, Some("chat"))
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &original_message.id,
                "Checkpoint resume",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let checkpoint = pause_task_run_with_checkpoint(&db, &run.id, "user_pause");
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE conversation_turns SET status = 'paused', finished_at = datetime('now') WHERE id = ?1",
                [&turn.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_task_runs SET finished_at = datetime('now') WHERE id = ?1",
                [&run.id],
            )
            .unwrap();
        }
        let response = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);

        let launch = db
            .resume_agent_turn_from_checkpoint(
                &response,
                Some("anthropic"),
                Some("claude-sonnet-4"),
                "checkpoint-launch-1",
                &checkpoint.id,
            )
            .unwrap();

        assert_eq!(launch.conversation_id, conversation.id);
        assert_eq!(launch.turn_id, turn.id);
        assert_eq!(launch.run_id, run.id);
        assert_eq!(launch.user_message_id, response.id);
        assert_eq!(launch.status, "queued");
        assert!(!launch.reused);
        let resumed_run = db.get_agent_task_run(&run.id).unwrap();
        assert_eq!(resumed_run.status, "queued");
        assert_eq!(resumed_run.phase, "queued");
        assert_eq!(resumed_run.provider.as_deref(), Some("anthropic"));
        assert_eq!(resumed_run.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(resumed_run.user_message_id, original_message.id);
        assert!(resumed_run.finished_at.is_none());
        let resumed_turn = db.get_conversation_turn(&turn.id).unwrap();
        assert_eq!(resumed_turn.status, "running");
        assert!(resumed_turn.finished_at.is_none());
        let messages = db.get_messages(&conversation.id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].id, response.id);
        assert_eq!(messages[1].content, checkpoint.resume_prompt);
        assert_eq!(
            messages[1]
                .artifacts
                .as_ref()
                .and_then(|artifacts| artifacts.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("checkpointContinuation")
        );
        let (launch_key, response_message_id): (Option<String>, Option<String>) = db
            .conn()
            .query_row(
                "SELECT launch_idempotency_key, response_message_id
                 FROM task_resume_checkpoints WHERE id = ?1",
                [&checkpoint.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(launch_key.as_deref(), Some("checkpoint-launch-1"));
        assert_eq!(response_message_id.as_deref(), Some(response.id.as_str()));
    }

    #[test]
    fn task_checkpoint_resume_replays_one_message_only_for_the_same_key_and_prompt() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let original_message = add_user_message(&db, &conversation.id, "Resume safely");
        let turn = db
            .create_conversation_turn(&conversation.id, &original_message.id, None)
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &original_message.id,
                "Idempotent resume",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let checkpoint = pause_task_run_with_checkpoint(&db, &run.id, "user_pause");
        let first = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);
        let launched = db
            .resume_agent_turn_from_checkpoint(
                &first,
                None,
                None,
                "stable-resume-key",
                &checkpoint.id,
            )
            .unwrap();
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE conversation_turns
                 SET status = 'paused', finished_at = NULL
                 WHERE id = ?1",
                [&turn.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_task_runs
                 SET status = 'paused', phase = 'paused', finished_at = NULL
                 WHERE id = ?1",
                [&run.id],
            )
            .unwrap();
        }
        let retry = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);
        let recovered = db
            .resume_agent_turn_from_checkpoint(
                &retry,
                None,
                None,
                "stable-resume-key",
                &checkpoint.id,
            )
            .unwrap();
        assert!(!recovered.reused);
        assert_eq!(recovered.status, "queued");
        assert_eq!(recovered.user_message_id, launched.user_message_id);
        let replayed = db
            .resume_agent_turn_from_checkpoint(
                &retry,
                None,
                None,
                "stable-resume-key",
                &checkpoint.id,
            )
            .unwrap();
        assert!(replayed.reused);
        assert_eq!(replayed.user_message_id, launched.user_message_id);
        assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);

        let different_key = db.resume_agent_turn_from_checkpoint(
            &retry,
            None,
            None,
            "different-resume-key",
            &checkpoint.id,
        );
        assert!(matches!(different_key, Err(CoreError::InvalidInput(_))));
        let different_prompt =
            unpersisted_user_message(&conversation.id, "not the checkpoint prompt");
        let different_prompt = db.resume_agent_turn_from_checkpoint(
            &different_prompt,
            None,
            None,
            "stable-resume-key",
            &checkpoint.id,
        );
        assert!(matches!(different_prompt, Err(CoreError::InvalidInput(_))));
        assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);
    }

    #[test]
    fn task_checkpoint_resume_rejects_stale_checkpoints_and_terminal_runs() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let original_message = add_user_message(&db, &conversation.id, "Resume latest only");
        let turn = db
            .create_conversation_turn(&conversation.id, &original_message.id, None)
            .unwrap();
        let run = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &original_message.id,
                "Stale resume",
                Some("openai"),
                Some("gpt-5"),
            )
            .unwrap();
        let stale = pause_task_run_with_checkpoint(&db, &run.id, "first_pause");
        let stale_message = unpersisted_user_message(&conversation.id, &stale.resume_prompt);
        db.resume_agent_turn_from_checkpoint(
            &stale_message,
            None,
            None,
            "stale-resume-key",
            &stale.id,
        )
        .unwrap();
        let latest = db
            .create_task_resume_checkpoint(&run.id, "second_pause")
            .unwrap();
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE conversation_turns SET status = 'paused' WHERE id = ?1",
                [&turn.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_task_runs SET status = 'paused', phase = 'paused' WHERE id = ?1",
                [&run.id],
            )
            .unwrap();
        }
        let stale_result = db.resume_agent_turn_from_checkpoint(
            &stale_message,
            None,
            None,
            "stale-resume-key",
            &stale.id,
        );
        assert!(matches!(stale_result, Err(CoreError::InvalidInput(_))));

        db.finish_agent_task_run(&run.id, "completed", Some("done"), None, None)
            .unwrap();
        let terminal_message = unpersisted_user_message(&conversation.id, &latest.resume_prompt);
        let terminal_result = db.resume_agent_turn_from_checkpoint(
            &terminal_message,
            None,
            None,
            "terminal-resume-key",
            &latest.id,
        );
        assert!(matches!(terminal_result, Err(CoreError::InvalidInput(_))));
        assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);
    }

    #[test]
    fn skill_governance_snapshot_surfaces_usage_and_stale_candidates() {
        let db = Database::open_memory().unwrap();
        let skill = db
            .save_skill(&crate::skills::SaveSkillInput {
                id: None,
                name: "Evidence Review".into(),
                description: "Verify claims against cited sources.".into(),
                content: "## Trigger\nUse for evidence review.\n\n## Workflow\nCheck citations."
                    .to_string(),
                enabled: true,
                resource_bundle: Vec::new(),
            })
            .unwrap();
        db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
            skill_id: skill.id.clone(),
            conversation_id: None,
            task_run_id: None,
            outcome: "failed".into(),
            evidence: serde_json::json!({ "reason": "missing citations" }),
        })
        .unwrap();
        for reason in ["bad source", "unverifiable"] {
            db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
                skill_id: skill.id.clone(),
                conversation_id: None,
                task_run_id: None,
                outcome: "failed".into(),
                evidence: serde_json::json!({ "reason": reason }),
            })
            .unwrap();
        }
        db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
            skill_id: "builtin-research-synthesis".into(),
            conversation_id: None,
            task_run_id: None,
            outcome: "success".into(),
            evidence: serde_json::json!({ "name": "Research Synthesis" }),
        })
        .unwrap();

        let snapshot = db.learning_governance_snapshot().unwrap();
        assert_eq!(snapshot.skill_stats.len(), 2);
        let failed = snapshot
            .skill_stats
            .iter()
            .find(|item| item.skill_id == skill.id)
            .unwrap();
        assert_eq!(failed.failure_count, 3);
        assert!(!failed.enabled);
        assert!(failed.disable_recommended);
        assert!(snapshot
            .skill_stats
            .iter()
            .any(|item| item.skill_id == "builtin-research-synthesis" && item.success_count == 1));
        assert!(snapshot
            .recommendations
            .iter()
            .any(|item| item.contains("Review")));
    }

    #[test]
    fn investigation_graph_uses_events_artifacts_and_evidence_nodes() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let user = add_user_message(&db, &conversation.id, "Research source-backed answer");
        let turn = db
            .create_conversation_turn(&conversation.id, &user.id, Some("knowledge"))
            .unwrap();
        let run = db
            .create_agent_task_run(&conversation.id, &turn.id, &user.id, "Research", None, None)
            .unwrap();
        let plan = serde_json::json!({ "steps": [{ "id": "research", "status": "in_progress" }] });
        db.update_agent_task_run_progress(
            &run.id,
            Some("running"),
            Some("research"),
            Some("knowledge"),
            None,
            Some(&plan),
            None,
        )
        .unwrap();
        db.record_agent_task_run_event(
            &run.id,
            "tool",
            "fetch_url completed",
            Some("completed"),
            Some(&serde_json::json!({
                "tool": "fetch_url",
                "url": "https://example.com/report",
                "citation": "[cite:web:1]"
            })),
        )
        .unwrap();
        db.create_agent_task_artifact(
            &run.id,
            &crate::conversation::CreateAgentTaskArtifactInput {
                kind: "report".into(),
                title: "Brief".into(),
                summary: Some("Evidence-backed brief".into()),
                content: "Claim with [cite:web:1]".into(),
                paths: vec![],
                payload: Some(serde_json::json!({ "openQuestions": ["freshness"] })),
                source: Some("agent".into()),
            },
        )
        .unwrap();

        let graph = db.build_investigation_graph(&run.id).unwrap();
        assert!(graph.nodes.iter().any(|node| node.node_type == "source"));
        assert!(graph.nodes.iter().any(|node| node.node_type == "artifact"));
        assert!(graph.open_questions.iter().any(|item| item == "freshness"));
    }

    #[test]
    fn browser_evidence_payload_is_source_scoped_and_auditable() {
        let payload = crate::workflow_automation::browser_evidence_payload(
            "https://example.com/report",
            "https://example.com/report",
            "Example Report",
            "Readable excerpt",
            "readable_text",
        );
        assert_eq!(payload["kind"], "browserEvidence");
        assert_eq!(payload["source"]["url"], "https://example.com/report");
        assert_eq!(payload["capture"]["method"], "readable_text");
    }
}
