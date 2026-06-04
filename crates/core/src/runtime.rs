//! Host-agnostic Agent Session and Runtime Protocol contracts.
//!
//! Desktop, CLI, protocol exits, and task runners should depend on this
//! interface instead of recreating agent turn assembly in each host surface.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::agent::AgentExecutionMode;
use crate::agent_run::{AgentRunEvent, AGENT_RUN_EVENT_VERSION};
use crate::app_settings::ShellAccessMode;
use crate::approval::{ApprovalDecision, ToolApprovalMode};
use crate::error::CoreError;

pub const RUNTIME_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeHostSurface {
    #[default]
    Desktop,
    Cli,
    Ide,
    Mcp,
    Acp,
    Gateway,
    Test,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSourceScope {
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub collection_id: Option<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSkillContext {
    #[serde(default)]
    pub available_skill_ids: Vec<String>,
    #[serde(default)]
    pub loaded_skill_ids: Vec<String>,
    #[serde(default)]
    pub trust_state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePackageContext {
    #[serde(default)]
    pub enabled_package_ids: Vec<String>,
    #[serde(default)]
    pub disabled_package_ids: Vec<String>,
}

impl RuntimePackageContext {
    pub fn from_package_host_snapshot(snapshot: &crate::package_host::PackageHostSnapshot) -> Self {
        let mut enabled_package_ids = Vec::new();
        let mut disabled_package_ids = Vec::new();
        for record in &snapshot.records {
            if record.is_runtime_visible() {
                enabled_package_ids.push(record.id.clone());
            } else {
                disabled_package_ids.push(record.id.clone());
            }
        }
        enabled_package_ids.sort();
        disabled_package_ids.sort();
        Self {
            enabled_package_ids,
            disabled_package_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionConfig {
    #[serde(default = "runtime_protocol_version")]
    pub version: u16,
    #[serde(default = "new_runtime_id")]
    pub session_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub task_run_id: Option<String>,
    #[serde(default)]
    pub host_surface: RuntimeHostSurface,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_enabled: Option<bool>,
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub source_scope: RuntimeSourceScope,
    #[serde(default)]
    pub approval_mode: ToolApprovalMode,
    #[serde(default)]
    pub shell_access_mode: ShellAccessMode,
    #[serde(default)]
    pub execution_mode: AgentExecutionMode,
    #[serde(default = "default_trace_enabled")]
    pub trace_enabled: bool,
    #[serde(default)]
    pub skill_context: RuntimeSkillContext,
    #[serde(default)]
    pub package_context: RuntimePackageContext,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl Default for AgentSessionConfig {
    fn default() -> Self {
        Self {
            version: RUNTIME_PROTOCOL_VERSION,
            session_id: new_runtime_id(),
            conversation_id: None,
            task_run_id: None,
            host_surface: RuntimeHostSurface::Desktop,
            provider: None,
            model: None,
            reasoning_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            source_scope: RuntimeSourceScope::default(),
            approval_mode: ToolApprovalMode::default(),
            shell_access_mode: ShellAccessMode::Restricted,
            execution_mode: AgentExecutionMode::Normal,
            trace_enabled: true,
            skill_context: RuntimeSkillContext::default(),
            package_context: RuntimePackageContext::default(),
            metadata: serde_json::json!({}),
        }
    }
}

impl AgentSessionConfig {
    pub fn from_versioned_json(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let mut config: Self = serde_json::from_value(value)?;
        config.apply_protocol_defaults();
        Ok(config)
    }

    pub fn apply_protocol_defaults(&mut self) {
        if self.version == 0 {
            self.version = RUNTIME_PROTOCOL_VERSION;
        }
        if self.session_id.trim().is_empty() {
            self.session_id = new_runtime_id();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnAttachment {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnInput {
    pub user_text: String,
    #[serde(default)]
    pub attachments: Vec<AgentTurnAttachment>,
    #[serde(default)]
    pub selected_source_ids: Vec<String>,
    #[serde(default)]
    pub selected_collection_id: Option<String>,
    #[serde(default)]
    pub mode: AgentExecutionMode,
    #[serde(default)]
    pub host_metadata: serde_json::Value,
}

impl AgentTurnInput {
    pub fn text(user_text: impl Into<String>) -> Self {
        Self {
            user_text: user_text.into(),
            attachments: Vec::new(),
            selected_source_ids: Vec::new(),
            selected_collection_id: None,
            mode: AgentExecutionMode::Normal,
            host_metadata: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTerminalStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl RuntimeTerminalStatus {
    pub fn from_run_event(event: &AgentRunEvent) -> Option<Self> {
        if !event.is_terminal() {
            return None;
        }
        match event.status.as_deref() {
            Some("cancelled") => Some(Self::Cancelled),
            Some("timed_out") => Some(Self::TimedOut),
            Some("completed") => Some(Self::Completed),
            _ => Some(Self::Failed),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentTurnState {
    Starting,
    Running,
    WaitingApproval,
    Terminal(RuntimeTerminalStatus),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnHandle {
    pub session_id: String,
    pub run_id: String,
    pub turn_id: String,
    pub state: AgentTurnState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum RuntimeOperation {
    ConfigureSession {
        config: AgentSessionConfig,
    },
    StartTurn {
        input: AgentTurnInput,
    },
    SteerTurn {
        turn_id: String,
        text: String,
    },
    InterruptTurn {
        turn_id: String,
        reason: String,
    },
    ResolveApproval {
        request_id: String,
        decision: ApprovalDecision,
    },
    ReadEvents {
        run_id: String,
    },
    ResumeSession {
        session_id: String,
    },
    CloseSession {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResult {
    pub handle: AgentTurnHandle,
    pub final_message: Option<String>,
    pub artifacts: serde_json::Value,
    pub usage: serde_json::Value,
    pub trace_id: Option<String>,
    pub status: RuntimeTerminalStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDependencyContract {
    pub provider_adapter: bool,
    pub tool_registry: bool,
    pub package_host: bool,
    pub skill_activation: bool,
    pub approval_adapter: bool,
    pub persistence_adapter: bool,
    pub execution_environment: bool,
}

impl AgentSessionDependencyContract {
    pub fn complete_runtime() -> Self {
        Self {
            provider_adapter: true,
            tool_registry: true,
            package_host: true,
            skill_activation: true,
            approval_adapter: true,
            persistence_adapter: true,
            execution_environment: true,
        }
    }
}

#[async_trait]
pub trait AgentSession: Send {
    fn config(&self) -> &AgentSessionConfig;

    async fn configure(&mut self, config: AgentSessionConfig) -> Result<(), CoreError>;

    async fn start_turn(&mut self, input: AgentTurnInput) -> Result<AgentTurnHandle, CoreError>;

    async fn steer_turn(&mut self, turn_id: &str, text: String) -> Result<(), CoreError>;

    async fn interrupt_turn(&mut self, turn_id: &str, reason: String) -> Result<(), CoreError>;

    async fn resolve_approval(
        &mut self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), CoreError>;

    async fn read_events(&self, run_id: &str) -> Result<Vec<AgentRunEvent>, CoreError>;

    async fn close(&mut self) -> Result<(), CoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTurnContractReport {
    pub run_id: String,
    pub turn_id: String,
    pub event_count: usize,
    pub terminal_status: RuntimeTerminalStatus,
    pub approval_denied: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeProtocolError {
    #[error("turn emitted no events")]
    Empty,
    #[error("event {event_seq} has unsupported run event version {version}")]
    UnsupportedRunEventVersion { event_seq: u64, version: u16 },
    #[error("event {event_seq} has an empty run id")]
    MissingRunId { event_seq: u64 },
    #[error("event {event_seq} has an empty turn id")]
    MissingTurnId { event_seq: u64 },
    #[error("event sequence is not monotonic: expected {expected}, got {actual}")]
    EventSequenceGap { expected: u64, actual: u64 },
    #[error("events from multiple runs were mixed")]
    MixedRunIds,
    #[error("events from multiple turns were mixed")]
    MixedTurnIds,
    #[error("turn must emit exactly one terminal event, got {count}")]
    TerminalEventCount { count: usize },
    #[error("terminal event must be the final event in a turn")]
    TerminalNotLast,
}

pub fn validate_runtime_turn_events(
    events: &[AgentRunEvent],
) -> Result<RuntimeTurnContractReport, RuntimeProtocolError> {
    let first = events.first().ok_or(RuntimeProtocolError::Empty)?;
    let run_id = first.run_id.clone();
    let turn_id = first.turn_id.clone();

    if run_id.trim().is_empty() {
        return Err(RuntimeProtocolError::MissingRunId {
            event_seq: first.event_seq,
        });
    }
    if turn_id.trim().is_empty() {
        return Err(RuntimeProtocolError::MissingTurnId {
            event_seq: first.event_seq,
        });
    }

    let mut terminal_indexes = Vec::new();
    let mut approval_denied = false;

    for (expected_seq, (index, event)) in (first.event_seq..).zip(events.iter().enumerate()) {
        if event.version != AGENT_RUN_EVENT_VERSION {
            return Err(RuntimeProtocolError::UnsupportedRunEventVersion {
                event_seq: event.event_seq,
                version: event.version,
            });
        }
        if event.run_id != run_id {
            return Err(RuntimeProtocolError::MixedRunIds);
        }
        if event.turn_id != turn_id {
            return Err(RuntimeProtocolError::MixedTurnIds);
        }
        if event.event_seq != expected_seq {
            return Err(RuntimeProtocolError::EventSequenceGap {
                expected: expected_seq,
                actual: event.event_seq,
            });
        }
        if event.is_terminal() {
            terminal_indexes.push(index);
        }
        if event.kind.as_str() == "approvalResolved"
            && matches!(event.status.as_deref(), Some("denied"))
        {
            approval_denied = true;
        }
    }

    if terminal_indexes.len() != 1 {
        return Err(RuntimeProtocolError::TerminalEventCount {
            count: terminal_indexes.len(),
        });
    }
    let terminal_index = terminal_indexes[0];
    if terminal_index + 1 != events.len() {
        return Err(RuntimeProtocolError::TerminalNotLast);
    }

    let terminal_event = &events[terminal_index];
    let terminal_status = RuntimeTerminalStatus::from_run_event(terminal_event)
        .unwrap_or(RuntimeTerminalStatus::Failed);

    Ok(RuntimeTurnContractReport {
        run_id,
        turn_id,
        event_count: events.len(),
        terminal_status,
        approval_denied,
    })
}

fn runtime_protocol_version() -> u16 {
    RUNTIME_PROTOCOL_VERSION
}

fn default_trace_enabled() -> bool {
    true
}

fn new_runtime_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::agent_run::{AgentRunEventKind, AgentRunPhase};

    #[test]
    fn session_config_defaults_are_runtime_protocol_v1() {
        let config = AgentSessionConfig::default();

        assert_eq!(config.version, RUNTIME_PROTOCOL_VERSION);
        assert!(!config.session_id.trim().is_empty());
        assert_eq!(config.host_surface, RuntimeHostSurface::Desktop);
        assert_eq!(config.shell_access_mode, ShellAccessMode::Restricted);
        assert_eq!(config.execution_mode, AgentExecutionMode::Normal);
        assert!(config.trace_enabled);
    }

    #[test]
    fn session_config_migrates_missing_or_zero_version() {
        let raw = serde_json::json!({
            "version": 0,
            "sessionId": "",
            "hostSurface": "cli",
            "model": "gpt-test"
        });

        let config = AgentSessionConfig::from_versioned_json(raw).unwrap();

        assert_eq!(config.version, RUNTIME_PROTOCOL_VERSION);
        assert!(!config.session_id.trim().is_empty());
        assert_eq!(config.host_surface, RuntimeHostSurface::Cli);
        assert_eq!(config.model.as_deref(), Some("gpt-test"));
    }

    #[test]
    fn runtime_package_context_projects_package_host_visibility() {
        let snapshot = crate::package_host::PackageHostSnapshot::new(vec![
            crate::package_host::PackageHostRecord {
                id: "pkg-enabled".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Enabled,
                health: crate::package_host::PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
            crate::package_host::PackageHostRecord {
                id: "pkg-disabled".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Disabled,
                health: crate::package_host::PackageHealthState::Healthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
            crate::package_host::PackageHostRecord {
                id: "pkg-unhealthy".to_string(),
                version: None,
                state: crate::package_host::PackageLifecycleState::Enabled,
                health: crate::package_host::PackageHealthState::Unhealthy,
                dependencies: Vec::new(),
                permissions: Vec::new(),
                components: Vec::new(),
            },
        ]);

        let context = RuntimePackageContext::from_package_host_snapshot(&snapshot);

        assert_eq!(context.enabled_package_ids, vec!["pkg-enabled".to_string()]);
        assert_eq!(
            context.disabled_package_ids,
            vec!["pkg-disabled".to_string(), "pkg-unhealthy".to_string()]
        );
    }

    #[test]
    fn runtime_turn_events_accept_one_terminal_event() {
        let events = vec![
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
                message: crate::llm::Message::text(crate::llm::Role::Assistant, "done"),
                usage_total: crate::llm::Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".to_string()),
            })
            .with_context(Some("run-1"), Some("turn-1"), Some(2)),
        ];

        let report = validate_runtime_turn_events(&events).unwrap();

        assert_eq!(report.run_id, "run-1");
        assert_eq!(report.turn_id, "turn-1");
        assert_eq!(report.terminal_status, RuntimeTerminalStatus::Completed);
        assert_eq!(report.event_count, 2);
    }

    #[test]
    fn runtime_turn_events_reject_sequence_gap() {
        let events = vec![
            AgentRunEvent::status_update(
                "run-1",
                Some("turn-1"),
                1,
                AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ),
            AgentRunEvent::terminal_error("run-1", Some("turn-1"), 3, "failed", "failed", None),
        ];

        assert_eq!(
            validate_runtime_turn_events(&events).unwrap_err(),
            RuntimeProtocolError::EventSequenceGap {
                expected: 2,
                actual: 3
            }
        );
    }

    #[test]
    fn runtime_turn_events_reject_multiple_terminal_events() {
        let events = vec![
            AgentRunEvent::terminal_error("run-1", Some("turn-1"), 1, "failed", "failed", None),
            AgentRunEvent {
                version: AGENT_RUN_EVENT_VERSION,
                run_id: "run-1".to_string(),
                turn_id: "turn-1".to_string(),
                event_seq: 2,
                kind: AgentRunEventKind::Done,
                phase: AgentRunPhase::Done,
                label: "done".to_string(),
                status: Some("completed".to_string()),
                payload: serde_json::json!({ "message": "done" }),
                created_at: None,
            },
        ];

        assert_eq!(
            validate_runtime_turn_events(&events).unwrap_err(),
            RuntimeProtocolError::TerminalEventCount { count: 2 }
        );
    }
}
