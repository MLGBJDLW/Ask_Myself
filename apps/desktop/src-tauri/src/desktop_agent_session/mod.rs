//! Desktop Agent Session Adapter over the core agent executor.
//!
//! This Module keeps Desktop-specific executor wiring behind one Interface so
//! chat commands can focus on Host Surface concerns such as task events and UI
//! persistence.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::{Local, SecondsFormat, Utc};
use log::{info, warn};
use nexa_core::agent::power_mode::{
    resolve_agent_power_policy, AgentPowerMode, AgentPowerPolicyInput,
};
use nexa_core::agent::{
    build_system_prompt, AgentConfig, AgentEvent, AgentExecutionMode, AgentExecutor,
    AgentRequestKind, AgentSteeringMessage, CancellationToken,
};
use nexa_core::agent_run::{
    AgentRunDisplayKind, AgentRunEvent, AgentRunEventImportance, AgentRunEventKind,
    AgentRunEventVisibility,
};
use nexa_core::app_settings::AppConfig;
use nexa_core::approval::{
    ApprovalCallback, ApprovalDecision, ApprovalRequest, SessionApprovalStore, ToolApprovalMode,
    ToolPermissionKey,
};
use nexa_core::capability_registry::RuntimeCapabilityResolution;
use nexa_core::context_pack::{
    ContextAssembler, ContextItemRole, ContextItemStability, ContextPack, ContextPackItem,
    ContextTrustLevel,
};
use nexa_core::conversation::memory::ResolvedContextWindow;
use nexa_core::conversation::{
    AgentConfig as DbAgentConfig, AgentSubtaskRun, Conversation, ConversationMessage,
    ImageAttachment,
};
use nexa_core::db::Database;
use nexa_core::error::CoreError;
use nexa_core::llm::{
    create_provider, model_declares_vision_support, model_supports_vision, ContentPart,
    LlmProvider, Message, ProviderConfig, ProviderType, ReasoningEffort, Role,
};
use nexa_core::mcp::{McpManager, McpServer};
use nexa_core::mixture_of_agents::{AgentCollaborationMode, MoaPresetId};
use nexa_core::ocr::extract_text_from_image;
use nexa_core::package_host::{BuiltinPackageHost, PackageRuntimeAssembler};
#[cfg(test)]
use nexa_core::project::{CreateProjectInput, UpdateProjectInput};
use nexa_core::provider_catalog::resolve_endpoint_model_context_window;
use nexa_core::provider_registry::provider_type_for_parts;
use nexa_core::quality_profile::{
    resolve_orchestration_profile, CustomOrchestrationOptions, OrchestrationProfile,
    OrchestrationProfileInput,
};
use nexa_core::run_event_outbox::{AgentRunEventOutboxFailure, AgentRunEventSubmitError};
use nexa_core::runtime::AgentRunEventOutbox;
use nexa_core::skills::Skill;
use nexa_core::tools::ToolRegistry;
use nexa_core::vision_router::{
    attachment_hash, classify_vision_route, execute_vision_observation, observation_prompt_text,
    VisionAttachmentAnalysis, VisionAttachmentStatus, VisionClassificationInput,
    VisionExecutionInput, VisionOcrProfile, VisionProfileV1, VisionProviderInput,
    VisionRouterPolicy, VisionTargetProfile, VisionTurnOverride, VISION_CLASSIFIER_VERSION,
    VISION_OBSERVATION_SCHEMA_VERSION,
};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as TokioMutex};
use uuid::Uuid;

use crate::agent_stream::{emit_agent_frontend_event, emit_agent_frontend_event_with_presentation};
use crate::agent_stream_bridge::AgentStreamForwarder;
use crate::agent_task_events::emit_agent_task_run_update;
use crate::app_events::{emit_app_event, emit_main_window_event};
use crate::browser::agent_tool::NativeBrowserSessionTool;
use crate::browser::BrowserState;
use crate::commands::{PendingToolApproval, PendingToolApprovals, TerminalState};
use crate::subagent_lifecycle::SubagentLifecycleRuntime;
use crate::subagent_tool::{
    DelegationRuntime, JudgeSubagentResultsTool, ObserveSubagentBatchTool, SubagentBatchTool,
    SubagentLifecycleTool, SubagentTool,
};
use crate::terminal_agent_tool::TerminalAgentTool;

const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const REQUIRED_ACTIVITY_RUNTIME_TOOLS: &[&str] = &[
    "activity_observe",
    "browser_session",
    "run_shell",
    "tool_search",
];

fn missing_core_runtime_tools(registry: &ToolRegistry) -> Vec<&'static str> {
    let tool_names = registry.tool_names();
    REQUIRED_ACTIVITY_RUNTIME_TOOLS
        .iter()
        .copied()
        .filter(|required| !tool_names.iter().any(|name| name == required))
        .collect()
}

fn canonical_builtin_tool_registry() -> ToolRegistry {
    PackageRuntimeAssembler::from_host(&BuiltinPackageHost)
        .expect("built-in package manifests must remain valid")
        .builtin_tool_registry()
}

fn resolve_desktop_context_window(db_config: &DbAgentConfig) -> ResolvedContextWindow {
    resolve_endpoint_model_context_window(
        &db_config.provider,
        db_config.base_url.as_deref(),
        &db_config.model,
        db_config
            .context_window
            .and_then(|value| u32::try_from(value).ok()),
    )
}

pub struct DesktopAgentTurnRuntime {
    pub timeout_secs: u64,
    pub keepalive_interval_secs: u64,
}

pub struct DesktopAgentTurnStream {
    pub app_handle: AppHandle,
    pub task_run_id: String,
    pub event_seq: Arc<AgentRunEventOutbox>,
    pub launch_started: Instant,
}

pub struct DesktopAgentApprovalRuntime {
    pub pending: PendingToolApprovals,
    pub session_store: SessionApprovalStore,
    pub approval_mode: ToolApprovalMode,
}

pub struct DesktopAgentSessionDependencies {
    pub tools: ToolRegistry,
    pub selected_skills: Vec<Skill>,
    pub auto_loaded_skills: Vec<Skill>,
    pub metrics: DesktopAgentDependencyMetrics,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DesktopAgentDependencyMetrics {
    pub skill_select_ms: u64,
    pub mcp_sync_ms: u64,
    pub tool_registry_ms: u64,
}

pub struct DesktopAgentTurnConfigRequest<'a> {
    pub db: &'a Database,
    pub conversation: &'a Conversation,
    pub turn_id: &'a str,
    pub message: &'a str,
    pub persona_id: Option<&'a str>,
    pub explicit_skill_ids: &'a [String],
    pub db_config: &'a DbAgentConfig,
    pub app_cfg: &'a AppConfig,
    pub execution_mode: AgentExecutionMode,
    pub power_mode: AgentPowerMode,
    pub collaboration_mode: AgentCollaborationMode,
    pub moa_preset: MoaPresetId,
    pub orchestration_profile: OrchestrationProfile,
    pub custom_orchestration: Option<CustomOrchestrationOptions>,
}

pub struct DesktopAgentTurnConfig {
    pub executor_config: AgentConfig,
    pub context_window_resolution: ResolvedContextWindow,
    pub source_scope_ids: Vec<String>,
    pub pinned_skill_ids: Vec<String>,
    pub context_pack: ContextPack,
}

pub struct DesktopAgentUserContentRequest<'a> {
    pub db: &'a Database,
    pub app_handle: Option<&'a AppHandle>,
    pub provider_config: &'a ProviderConfig,
    pub db_config: &'a DbAgentConfig,
    pub message: &'a str,
    pub attachments: Option<&'a [ImageAttachment]>,
}

pub struct DesktopAgentVisionUserContentRequest<'a> {
    pub db: &'a Database,
    pub app_handle: Option<&'a AppHandle>,
    pub provider_config: &'a ProviderConfig,
    pub db_config: &'a DbAgentConfig,
    pub message: &'a str,
    pub attachments: Option<&'a [ImageAttachment]>,
    pub vision_resolution: Option<&'a RuntimeCapabilityResolution>,
    pub task_run_id: &'a str,
    pub primary_egress_id: &'a str,
    pub primary_routes_local: bool,
    pub primary_native_vision_allowed: bool,
    pub turn_override: Option<VisionTurnOverride>,
    pub cancellation: &'a CancellationToken,
}

pub struct DesktopAgentVisionUserContentResult {
    pub parts: Vec<ContentPart>,
    pub attachments: Vec<ImageAttachment>,
    pub llm_context_content: String,
}

struct DesktopVisionProviderRoute {
    fallback_index: usize,
    target_id: String,
    target_revision: u64,
    provider_id: String,
    egress_id: String,
    model_id: String,
    local: bool,
    provider_type: ProviderType,
    provider: Box<dyn LlmProvider>,
}

pub struct DesktopAgentPostSuccessLearningRequest {
    pub db: Arc<Database>,
    pub conversation_id: String,
    pub db_config: DbAgentConfig,
}

pub struct DesktopAgentSessionConfigInput<'a> {
    pub db: &'a Database,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub db_config: &'a DbAgentConfig,
    pub app_cfg: &'a AppConfig,
    pub source_scope_ids: &'a [String],
    pub selected_skills: &'a [Skill],
    pub auto_loaded_skills: &'a [Skill],
    pub execution_mode: AgentExecutionMode,
    pub collaboration_mode: AgentCollaborationMode,
    pub moa_preset: MoaPresetId,
    pub orchestration_profile: OrchestrationProfile,
    pub custom_orchestration: Option<CustomOrchestrationOptions>,
}

pub struct DesktopAgentSessionDependencyRequest<'a> {
    pub db: &'a Database,
    pub mcp_manager: &'a Arc<tokio::sync::Mutex<McpManager>>,
    pub event_seq: &'a AgentRunEventOutbox,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub turn_id: &'a str,
    pub message: &'a str,
    pub pinned_skill_ids: &'a [String],
    pub provider_config: ProviderConfig,
    pub executor_config: AgentConfig,
    /// Optional root-agent capability ceiling supplied by a host workflow.
    /// `None` preserves the normal interactive-chat registry; `Some` may only
    /// narrow the assembled registry and is also inherited by subagents.
    pub root_allowed_tools: Option<Vec<String>>,
    pub subagent_allowed_tools: Option<Vec<String>>,
    pub subagent_allowed_skill_ids: Option<Vec<String>>,
    pub subagent_lifecycle: SubagentLifecycleRuntime,
    pub cancel_token: CancellationToken,
    pub plan_mode: bool,
    pub mcp_call_timeout_secs: u64,
    pub terminal_state: Option<TerminalState>,
    pub browser_state: BrowserState,
}

pub struct DesktopAgentTurnOutcome {
    pub result: Option<Result<Message, CoreError>>,
    pub timed_out: bool,
}

pub struct DesktopAgentTurnFinalization<'a> {
    pub db: &'a Database,
    pub app_handle: &'a AppHandle,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub task_orchestrator_run_id: Option<&'a str>,
    pub turn_id: &'a str,
    pub event_seq: &'a AgentRunEventOutbox,
    pub outcome: &'a DesktopAgentTurnOutcome,
}

pub struct DesktopAgentStopFinalization<'a> {
    pub db: &'a Database,
    pub app_handle: &'a AppHandle,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub task_orchestrator_run_id: Option<&'a str>,
    pub turn_id: &'a str,
    pub event_seq: &'a AgentRunEventOutbox,
    pub reason: &'a str,
    pub summary: &'a str,
}

pub struct DesktopAgentTurnRequest {
    pub provider: Box<dyn LlmProvider>,
    pub dependencies: DesktopAgentSessionDependencies,
    pub executor_config: AgentConfig,
    pub cancel_token: CancellationToken,
    pub steering_rx: mpsc::UnboundedReceiver<AgentSteeringMessage>,
    pub approval_runtime: DesktopAgentApprovalRuntime,
    pub summarization_provider: Option<Box<dyn LlmProvider>>,
    pub history: Vec<Message>,
    pub user_parts: Vec<ContentPart>,
    pub db: Arc<Database>,
    pub conversation_id: String,
    pub turn_id: String,
    pub assistant_sort_order: i64,
    pub runtime: DesktopAgentTurnRuntime,
    pub stream: DesktopAgentTurnStream,
}

pub struct DesktopRunningAgentStopRequest {
    pub db: Arc<Database>,
    pub app_handle: AppHandle,
    pub conversation_id: String,
    pub pending_approvals: PendingToolApprovals,
}

pub(crate) struct DesktopApprovalCallbackInput {
    db: Arc<Database>,
    task_run_id: String,
    approval_runtime: DesktopAgentApprovalRuntime,
    cancellation: CancellationToken,
}

mod content;
mod dependencies;
mod finalize;
mod learning;
mod run;
mod stop;
mod turn_config;

pub(crate) use content::*;
pub(crate) use dependencies::*;
pub(crate) use finalize::*;
pub(crate) use learning::*;
pub(crate) use run::*;
pub(crate) use stop::*;
pub(crate) use turn_config::*;

#[cfg(test)]
mod tests;
