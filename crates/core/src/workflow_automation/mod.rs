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
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
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

mod types;
pub use types::*;

struct PreparedWorkflowOccurrenceClaim {
    due_run: WorkflowAutomationDueRun,
    definition_revision: i64,
    scheduled_for: String,
    scheduled_at: DateTime<Utc>,
    next_run_at: Option<String>,
    occurrence_id: String,
    existing: Option<WorkflowAutomationOccurrence>,
}

enum WorkflowOccurrenceClaimDecision {
    Skip(&'static str),
    Queue { attempt: u32 },
}

#[derive(Clone, Copy)]
enum WorkflowApprovalDecision {
    Approve,
    Deny,
}

enum WorkflowApprovalResolution {
    Approved(WorkflowAutomationDueRunClaim),
    Denied(WorkflowAutomationRun),
}

struct PendingWorkflowApproval {
    automation: WorkflowAutomation,
    run: WorkflowAutomationRun,
    occurrence_id: String,
    origin: WorkflowAutomationOccurrenceOrigin,
    resume_next_run_at: Option<String>,
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

fn fetch_pending_workflow_approval(
    tx: &Transaction<'_>,
    run_id: &str,
) -> Result<PendingWorkflowApproval, CoreError> {
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
    let run = fetch_workflow_run(tx, run_id)?;
    let occurrence_id = run
        .occurrence_id
        .clone()
        .ok_or_else(|| CoreError::Internal(format!("Workflow run {run_id} lost its occurrence")))?;
    let (origin, resume_next_run_at): (String, Option<String>) = tx.query_row(
        "SELECT origin, resume_next_run_at
         FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
        rusqlite::params![&occurrence_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(PendingWorkflowApproval {
        automation,
        run,
        occurrence_id,
        origin: WorkflowAutomationOccurrenceOrigin::parse(&origin)?,
        resume_next_run_at,
    })
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

mod approvals;
mod events;
mod governance;
mod investigation;
mod resume;
mod scheduling;

#[cfg(test)]
mod tests;
