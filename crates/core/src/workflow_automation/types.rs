use super::*;

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

    pub(super) fn label(&self) -> String {
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::ManualRunNow => "manual_run_now",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, CoreError> {
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
    pub(super) fn parse(value: &str) -> Result<Self, CoreError> {
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }

    pub(super) fn from_str(value: &str) -> Result<Self, CoreError> {
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

    pub(super) const fn is_retryable_failure(self) -> bool {
        matches!(self, Self::ClaimFailed | Self::LaunchFailed)
    }

    pub(super) const fn is_retry_audit_only(self) -> bool {
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
