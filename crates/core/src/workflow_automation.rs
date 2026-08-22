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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationDueRunClaim {
    pub due_run: WorkflowAutomationDueRun,
    pub run: WorkflowAutomationRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationRun {
    pub id: String,
    pub automation_id: String,
    pub task_run_id: Option<String>,
    pub status: String,
    pub summary: Option<String>,
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
    let source_scope_json: String = row.get(7)?;
    let approval_policy_json: String = row.get(8)?;
    Ok(WorkflowAutomation {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        workflow_template_id: row.get(3)?,
        prompt: row.get(4)?,
        trigger_kind: row.get(6)?,
        trigger: serde_json::from_str::<WorkflowAutomationTrigger>(&trigger_json)
            .unwrap_or(WorkflowAutomationTrigger::Manual),
        source_scope: parse_json_or_default::<Vec<String>>(source_scope_json),
        approval_policy: parse_json_or_default::<WorkflowAutomationApprovalPolicy>(
            approval_policy_json,
        ),
        enabled: row.get::<_, i64>(9)? != 0,
        status: row.get(10)?,
        last_run_at: row.get(11)?,
        next_run_at: row.get(12)?,
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

fn next_run_for_trigger(trigger: &WorkflowAutomationTrigger, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    let WorkflowAutomationTrigger::Schedule { cron } = trigger else {
        return None;
    };
    next_run_for_simple_cron(cron, Utc::now()).map(|value| value.to_rfc3339())
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

fn scheduler_event_is_retryable_failure(event_type: &str) -> bool {
    matches!(event_type, "claim_failed" | "launch_failed")
}

fn scheduler_event_is_retry_audit_only(event_type: &str) -> bool {
    matches!(event_type, "skipped_backoff" | "skipped_retry_limit")
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
        let event_type = event.event_type.trim();
        if scheduler_event_is_retry_audit_only(event_type) {
            continue;
        }
        if !scheduler_event_is_retryable_failure(event_type) {
            break;
        }
        retryable_failure_count += 1;
        if last_retryable_event_type.is_none() {
            last_retryable_event_type = Some(event_type.to_string());
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

fn next_run_for_simple_cron(cron: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let parts = cron.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return None;
    }
    let minute = parse_cron_number(parts[0], 0, 59)?;
    let hour = parse_cron_number(parts[1], 0, 23)?;
    let mut candidate = DateTime::<Utc>::from_naive_utc_and_offset(
        now.date_naive().and_hms_opt(hour, minute, 0)?,
        Utc,
    );
    if candidate <= now {
        candidate += Duration::days(1);
    }
    Some(candidate)
}

fn parse_cron_number(value: &str, min: u32, max: u32) -> Option<u32> {
    if value == "*" {
        return Some(min);
    }
    let parsed = value.parse::<u32>().ok()?;
    (min..=max).contains(&parsed).then_some(parsed)
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

fn build_resume_prompt(run: &AgentTaskRun, checkpoint_id: &str, state: &Value) -> String {
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
    format!(
        "Resume this Nexa task from a durable checkpoint.\n\nTask: {}\nRun ID: {}\nCheckpoint ID: {}\nPrevious status: {}\nPrevious phase: {}\nRoute: {}\nSummary: {}{}\n\nInstructions:\n- Start by naming the resumed checkpoint and the next unfinished phase.\n- Prefer liveTurnState.taskPlan when present; it is the freshest in-memory execution state captured at the checkpoint boundary.\n- Continue after partialAssistantOutput exactly where it stopped; do not repeat text already shown.\n- Continue from the checkpoint state instead of restarting completed work.\n- Do not redo completed tool work unless the checkpoint shows stale, failed, missing, or contradictory evidence.\n- Treat recentEvents and artifacts as durable pointers; inspect only the files, sources, or records needed for the next decision.\n- Reuse existing evidence and artifacts when they are still valid.\n- Re-check stale or missing evidence before making final claims.\n- Preserve the user's source scope and approval boundaries.\n- Run verification before the final answer, then say what was resumed and what still needs verification.\n\nCheckpoint state:\n{}",
        run.title,
        run.id,
        checkpoint_id,
        run.status,
        run.phase,
        run.route_kind.as_deref().unwrap_or("unknown"),
        run.summary.as_deref().unwrap_or("No summary yet."),
        partial_output,
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
        let next_run_at = next_run_for_trigger(&input.trigger, input.enabled);
        let enabled = if input.enabled { 1 } else { 0 };

        let id = input.id.clone().unwrap_or_else(new_id);
        let conn = self.conn();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_automations WHERE id = ?1)",
            rusqlite::params![&id],
            |row| row.get(0),
        )?;
        if exists {
            conn.execute(
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
            conn.execute(
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
        drop(conn);
        self.get_workflow_automation(&id)
    }

    pub fn get_workflow_automation(&self, id: &str) -> Result<WorkflowAutomation, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, description, workflow_template_id, prompt, trigger_json,
                    trigger_kind, source_scope_json, approval_policy_json, enabled,
                    status, last_run_at, next_run_at, created_at, updated_at
             FROM workflow_automations WHERE id = ?1",
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
        let mut stmt = conn.prepare(
            "SELECT id, name, description, workflow_template_id, prompt, trigger_json,
                    trigger_kind, source_scope_json, approval_policy_json, enabled,
                    status, last_run_at, next_run_at, created_at, updated_at
             FROM workflow_automations
             ORDER BY enabled DESC, updated_at DESC, name ASC",
        )?;
        let rows = stmt.query_map([], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
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

    pub fn list_due_workflow_automations(
        &self,
        now_rfc3339: &str,
    ) -> Result<Vec<WorkflowAutomationDueRun>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, workflow_template_id, prompt, trigger_json,
                    trigger_kind, source_scope_json, approval_policy_json, enabled,
                    status, last_run_at, next_run_at, created_at, updated_at
             FROM workflow_automations
             WHERE enabled = 1
               AND trigger_kind IN ('schedule', 'folder')
               AND (
                    trigger_kind = 'folder'
                    OR (next_run_at IS NOT NULL AND next_run_at <= ?1)
               )
             ORDER BY COALESCE(next_run_at, updated_at) ASC, name ASC
             LIMIT 100",
        )?;
        let rows = stmt.query_map(rusqlite::params![now_rfc3339], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let automation = row?;
            let due_reason = match &automation.trigger {
                WorkflowAutomationTrigger::Schedule { .. } => automation.trigger.label(),
                WorkflowAutomationTrigger::Folder { .. } => {
                    if !folder_trigger_due(&automation.trigger, automation.last_run_at.as_deref())?
                    {
                        continue;
                    }
                    "folder trigger matched a new or updated file".to_string()
                }
                WorkflowAutomationTrigger::Manual => continue,
            };
            out.push(WorkflowAutomationDueRun {
                prompt: automation_prompt(&automation),
                due_reason,
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
        let run = self.record_workflow_automation_run(
            &due_run.automation.id,
            None,
            "queued",
            summary.or(Some(due_run.due_reason.as_str())),
        )?;
        Ok(WorkflowAutomationDueRunClaim { due_run, run })
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
        self.claim_workflow_automation_due_run(due_run, summary)
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
        let automation = self.get_workflow_automation(automation_id)?;
        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO workflow_automation_runs
             (id, automation_id, task_run_id, status, summary, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, CASE WHEN ?4 IN ('completed', 'failed', 'cancelled', 'timed_out', 'disabled') THEN datetime('now') ELSE NULL END)",
            rusqlite::params![&id, automation_id, task_run_id, status, summary],
        )?;
        let next_run_at = next_run_for_trigger(&automation.trigger, automation.enabled);
        conn.execute(
            "UPDATE workflow_automations
             SET last_run_at = datetime('now'),
                 next_run_at = ?2,
                 status = ?3,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![automation_id, next_run_at, status],
        )?;
        drop(conn);
        self.get_workflow_automation_run(&id)
    }

    pub fn start_workflow_automation_run(
        &self,
        run_id: &str,
        task_run_id: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let run = self.get_workflow_automation_run(run_id)?;
        let current_state = crate::task_orchestrator::project_task_status(&run.status)
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
        self.get_agent_task_run(task_run_id)?;

        let conn = self.conn();
        let affected = conn.execute(
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
        conn.execute(
            "UPDATE workflow_automations
             SET status = 'running',
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![run.automation_id],
        )?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn transition_workflow_automation_run(
        &self,
        run_id: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let run = self.get_workflow_automation_run(run_id)?;
        let current_state = crate::task_orchestrator::project_task_status(&run.status)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        let target_state = crate::task_orchestrator::project_task_status(status)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        if current_state != target_state {
            crate::task_orchestrator::validate_task_transition(current_state, target_state)
                .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        }

        let conn = self.conn();
        let affected = conn.execute(
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
        conn.execute(
            "UPDATE workflow_automations
             SET status = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![run.automation_id, status],
        )?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn get_workflow_automation_run(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, automation_id, task_run_id, status, summary, created_at, finished_at
             FROM workflow_automation_runs WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(WorkflowAutomationRun {
                    id: row.get(0)?,
                    automation_id: row.get(1)?,
                    task_run_id: row.get(2)?,
                    status: row.get(3)?,
                    summary: row.get(4)?,
                    created_at: row.get(5)?,
                    finished_at: row.get(6)?,
                })
            },
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
            "SELECT id, automation_id, task_run_id, status, summary, created_at, finished_at
             FROM workflow_automation_runs
             WHERE task_run_id = ?1
             ORDER BY datetime(created_at) DESC, id DESC
             LIMIT 1",
            rusqlite::params![task_run_id],
            |row| {
                Ok(WorkflowAutomationRun {
                    id: row.get(0)?,
                    automation_id: row.get(1)?,
                    task_run_id: row.get(2)?,
                    status: row.get(3)?,
                    summary: row.get(4)?,
                    created_at: row.get(5)?,
                    finished_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(CoreError::Database)
    }

    pub fn record_workflow_automation_scheduler_event(
        &self,
        automation_id: Option<&str>,
        run_id: Option<&str>,
        event_type: &str,
        status: Option<&str>,
        summary: &str,
        payload: Option<&Value>,
    ) -> Result<WorkflowAutomationSchedulerEvent, CoreError> {
        let event_type = normalize_required(event_type, "Scheduler event type", 120)?;
        let summary = normalize_optional(summary, 2_000)?;
        let status = status
            .map(|value| normalize_optional(value, 120))
            .transpose()?
            .filter(|value| !value.is_empty());
        let payload = payload.cloned().unwrap_or_else(|| serde_json::json!({}));
        let payload_json = serde_json::to_string(&payload)?;
        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO workflow_automation_scheduler_events
             (id, automation_id, run_id, event_type, status, summary, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &id,
                automation_id,
                run_id,
                &event_type,
                status.as_deref(),
                &summary,
                &payload_json
            ],
        )?;
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
        let resume_prompt = build_resume_prompt(&run, &checkpoint_id, &state);
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
    use uuid::Uuid;

    use crate::agent::StreamBlockChannel;
    use crate::agent_run::AgentRunEvent;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::db::Database;
    use crate::error::CoreError;
    use crate::llm::Role;
    use crate::workflow_automation::{
        workflow_automation_scheduler_retry_decision_from_events, TaskResumeCheckpoint,
        WorkflowAutomationSchedulerEvent,
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

        assert_eq!(claim.due_run.automation.id, saved.id);
        assert_eq!(claim.run.automation_id, saved.id);
        assert_eq!(claim.run.status, "queued");
        assert_eq!(
            claim.run.summary.as_deref(),
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

        assert_eq!(running.status, "running");
        assert_eq!(running.task_run_id.as_deref(), Some(task_run.id.as_str()));
        assert_eq!(running.summary.as_deref(), Some("Agent session started"));
        assert_eq!(
            db.get_workflow_automation(&automation.id).unwrap().status,
            "running"
        );

        let completed = db
            .transition_workflow_automation_run(&queued_run.id, "completed", Some("done"))
            .unwrap();

        assert_eq!(completed.status, "completed");
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
                "launch_succeeded",
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
