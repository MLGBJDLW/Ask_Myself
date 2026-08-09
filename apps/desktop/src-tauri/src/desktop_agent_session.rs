//! Desktop Agent Session Adapter over the core agent executor.
//!
//! This Module keeps Desktop-specific executor wiring behind one Interface so
//! chat commands can focus on Host Surface concerns such as task events and UI
//! persistence.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
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
    AgentRunDisplayKind, AgentRunEvent, AgentRunEventImportance, AgentRunEventVisibility,
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
use nexa_core::provider_registry::provider_type_for_parts;
use nexa_core::quality_profile::{
    resolve_orchestration_profile, CustomOrchestrationOptions, OrchestrationProfile,
    OrchestrationProfileInput,
};
use nexa_core::runtime::AgentRunEventSequencer;
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

use crate::agent_stream::{
    emit_agent_frontend_event, emit_agent_frontend_event_with_presentation,
    emit_agent_run_frontend_event,
};
use crate::agent_stream_bridge::AgentStreamForwarder;
use crate::agent_task_events::{emit_agent_task_run_update, persist_durable_run_event};
use crate::app_events::emit_app_event;
use crate::browser::agent_tool::NativeBrowserSessionTool;
use crate::browser::BrowserState;
use crate::commands::TerminalState;
use crate::subagent_tool::{
    DelegationRuntime, JudgeSubagentResultsTool, ObserveSubagentBatchTool, SubagentBatchTool,
    SubagentLifecycleTool, SubagentTool,
};
use crate::terminal_agent_tool::TerminalAgentTool;

const UNLIMITED_EXECUTOR_TIMEOUT_SECS: u32 = 0;
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

pub struct DesktopAgentTurnRuntime {
    pub timeout_secs: u64,
    pub keepalive_interval_secs: u64,
}

pub struct DesktopAgentTurnStream {
    pub app_handle: AppHandle,
    pub task_run_id: String,
    pub event_seq: Arc<AgentRunEventSequencer>,
    pub terminal_emitted: Arc<AtomicBool>,
    pub launch_started: Instant,
}

pub struct DesktopAgentApprovalRuntime {
    pub pending:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalDecision>>>>,
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
    pub app_handle: &'a AppHandle,
    pub event_seq: &'a AgentRunEventSequencer,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub turn_id: &'a str,
    pub message: &'a str,
    pub pinned_skill_ids: &'a [String],
    pub provider_config: ProviderConfig,
    pub executor_config: AgentConfig,
    pub subagent_allowed_tools: Option<Vec<String>>,
    pub subagent_allowed_skill_ids: Option<Vec<String>>,
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
    pub outcome: &'a DesktopAgentTurnOutcome,
}

pub struct DesktopAgentStopFinalization<'a> {
    pub db: &'a Database,
    pub app_handle: &'a AppHandle,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub task_orchestrator_run_id: Option<&'a str>,
    pub turn_id: &'a str,
    pub event_seq: &'a AgentRunEventSequencer,
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
}

struct DesktopApprovalCallbackInput {
    db: Arc<Database>,
    app_handle: AppHandle,
    conversation_id: String,
    task_run_id: String,
    turn_id: String,
    event_seq: Arc<AgentRunEventSequencer>,
    approval_runtime: DesktopAgentApprovalRuntime,
}

pub fn execution_mode_artifact(execution_mode: AgentExecutionMode) -> serde_json::Value {
    serde_json::json!({
        "kind": "executionMode",
        "version": 1,
        "mode": execution_mode.as_str(),
    })
}

pub fn power_mode_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentPowerMode",
        "version": 1,
        "mode": config.power_mode.as_str(),
        "policy": {
            "orchestration": if config.power_mode.is_nexus() { "proactiveParallelSubagents" } else { "standard" },
            "reasoningEnabled": config.reasoning_enabled,
            "reasoningEffort": config.reasoning_effort.as_ref().map(ToString::to_string),
            "thinkingBudget": config.thinking_budget,
            "maxParallel": config.subagent_max_parallel,
            "maxCallsPerTurn": config.subagent_max_calls_per_turn,
            "delegatedTokenBudget": config.subagent_token_budget,
            "verificationReservePercent": config.subagent_verification_reserve_percent,
        },
    })
}

pub fn collaboration_mode_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentCollaborationMode",
        "version": 1,
        "mode": config.collaboration_mode.as_str(),
        "preset": config.moa_preset.as_str(),
        "contract": {
            "advisorsReceiveTools": false,
            "aggregatorRetainsTools": true,
            "privateCacheSafeTail": true,
            "independentFromNexus": true,
        },
    })
}

pub fn orchestration_profile_artifact(config: &AgentConfig) -> serde_json::Value {
    serde_json::json!({
        "kind": "orchestrationProfile",
        "version": 1,
        "profile": config.orchestration_profile.as_str(),
        "providerReasoningEffort": config.reasoning_effort.as_ref().map(ToString::to_string),
        "policy": {
            "maxIterations": config.max_iterations,
            "maxParallel": config.subagent_max_parallel,
            "maxCallsPerTurn": config.subagent_max_calls_per_turn,
            "delegatedTokenBudget": config.subagent_token_budget,
            "verificationReservePercent": config.subagent_verification_reserve_percent,
        },
    })
}

pub fn request_desktop_running_agent_stop(
    task_state: nexa_core::runtime::ActiveAgentTurn,
    request: DesktopRunningAgentStopRequest,
) {
    let DesktopRunningAgentStopRequest {
        db,
        app_handle,
        conversation_id,
    } = request;
    let task_run_id = task_state.handle.run_id.clone();
    let task_orchestrator_run_id = task_state.orchestrator_run_id.clone();
    let turn_id = task_state.handle.turn_id.clone();
    let stream_event_seq = Arc::clone(&task_state.event_sequencer);
    if let Err(error) = db.cancel_interactions_for_stopped_run(&task_run_id) {
        warn!("Failed to cancel interactions for stopped run {task_run_id}: {error}");
    }
    let _ = db.update_agent_task_run_progress(
        &task_run_id,
        Some("cancelling"),
        Some("cancelling"),
        None,
        Some("Stop requested"),
        None,
        None,
    );
    let run_event = emit_agent_frontend_event(
        &app_handle,
        stream_event_seq.as_ref(),
        &conversation_id,
        &task_run_id,
        Some(&turn_id),
        AgentEvent::Status {
            content: "Stop requested".to_string(),
            tone: Some("muted".to_string()),
        },
    );
    persist_durable_run_event(&db, &run_event);
    emit_agent_task_run_update(&db, &app_handle, &conversation_id, &task_run_id);

    task_state.cancel_token.cancel();
    let abort_task = task_state.task;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if !abort_task.is_finished() {
            abort_task.abort();
            finalize_desktop_agent_stop(DesktopAgentStopFinalization {
                db: &db,
                app_handle: &app_handle,
                conversation_id: &conversation_id,
                task_run_id: &task_run_id,
                task_orchestrator_run_id: task_orchestrator_run_id.as_deref(),
                turn_id: &turn_id,
                event_seq: stream_event_seq.as_ref(),
                reason: "aborted_after_cancel_timeout",
                summary: "Stopped by user",
            });
        }
    });
}

pub fn annotate_user_artifacts_with_execution_mode(
    artifacts: Option<serde_json::Value>,
    execution_mode: AgentExecutionMode,
    power_mode: AgentPowerMode,
    collaboration_mode: AgentCollaborationMode,
    moa_preset: MoaPresetId,
    orchestration_profile: OrchestrationProfile,
) -> Option<serde_json::Value> {
    if !execution_mode.is_plan()
        && !power_mode.is_nexus()
        && !collaboration_mode.is_moa()
        && orchestration_profile == OrchestrationProfile::Balanced
    {
        return artifacts;
    }

    let insert_markers = |map: &mut serde_json::Map<String, serde_json::Value>| {
        if execution_mode.is_plan() {
            map.insert(
                "executionMode".to_string(),
                execution_mode_artifact(execution_mode),
            );
        }
        if power_mode.is_nexus() {
            map.insert(
                "powerMode".to_string(),
                serde_json::json!({
                    "kind": "agentPowerMode",
                    "version": 1,
                    "mode": power_mode.as_str(),
                }),
            );
        }
        if collaboration_mode.is_moa() {
            map.insert(
                "collaborationMode".to_string(),
                serde_json::json!({
                    "kind": "agentCollaborationMode",
                    "version": 1,
                    "mode": collaboration_mode.as_str(),
                    "preset": moa_preset.as_str(),
                }),
            );
        }
        if orchestration_profile != OrchestrationProfile::Balanced {
            map.insert(
                "orchestrationProfile".to_string(),
                serde_json::json!({
                    "kind": "orchestrationProfile",
                    "version": 1,
                    "profile": orchestration_profile.as_str(),
                }),
            );
        }
    };
    match artifacts {
        None => {
            let mut map = serde_json::Map::new();
            map.insert(
                "kind".to_string(),
                serde_json::Value::String("chatSendContext".to_string()),
            );
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
        Some(serde_json::Value::Object(mut map)) => {
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert(
                "kind".to_string(),
                serde_json::Value::String("chatSendContext".to_string()),
            );
            map.insert("userArtifacts".to_string(), value);
            insert_markers(&mut map);
            Some(serde_json::Value::Object(map))
        }
    }
}

fn build_current_turn_time_section() -> String {
    let now = Local::now();
    let utc_now = now.with_timezone(&Utc);
    format!(
        "## Current Turn Time\n\n\
         The current time at the start of this user turn is:\n\
         - Local timestamp: {}\n\
         - UTC timestamp: {}\n\
         - Local date: {}\n\
         - Local time: {}\n\
         - Weekday: {}\n\
         - UTC offset: {}\n\n\
         Use this as the reference point for relative dates such as today, yesterday, tomorrow, last week, and latest. For time-sensitive facts, schedules, prices, laws, releases, or other information that may have changed, prefer fresh retrieval/tool evidence instead of relying only on memory.",
        now.to_rfc3339_opts(SecondsFormat::Secs, false),
        utc_now.to_rfc3339_opts(SecondsFormat::Secs, true),
        now.format("%Y-%m-%d"),
        now.format("%H:%M:%S"),
        now.format("%A"),
        now.format("%:z"),
    )
}

fn provider_type_for_config(config: &DbAgentConfig) -> ProviderType {
    provider_type_for_parts(&config.provider, config.base_url.as_deref())
}

fn desktop_provider_config(config: &DbAgentConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider_type_for_config(config),
        api_key: Some(config.api_key.clone()),
        base_url: config.base_url.as_deref().and_then(|value| {
            let normalized = value.trim().trim_end_matches('/');
            (!normalized.is_empty()).then(|| normalized.to_string())
        }),
        org_id: None,
        timeout_secs: None,
    }
}

fn build_selected_skills_artifact(skills: &[Skill]) -> serde_json::Value {
    serde_json::json!({
        "kind": "selectedSkills",
        "version": 1,
        "skills": skills
            .iter()
            .map(|skill| {
                serde_json::json!({
                    "id": &skill.id,
                    "name": &skill.name,
                    "description": &skill.description,
                    "shortDescription": &skill.interface.short_description,
                    "enabled": skill.enabled,
                    "builtin": skill.builtin,
                    "sourcePath": &skill.source_path,
                    "implicit": skill.policy.allow_implicit_invocation,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// Map a MIME type to a file extension for temp-file parsing.
fn mime_to_extension(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.ms-powerpoint" => "ppt",
        "text/plain" => "txt",
        "text/markdown" | "text/x-markdown" => "md",
        "text/csv" => "csv",
        "text/html" => "html",
        "application/json" => "json",
        "application/epub+zip" => "epub",
        _ if mime.starts_with("text/") => "txt",
        _ => "bin",
    }
}

fn emit_user_content_event(
    app_handle: Option<&AppHandle>,
    event_name: &str,
    payload: &serde_json::Value,
) {
    if let Some(app_handle) = app_handle {
        emit_app_event(app_handle, event_name, payload);
    }
}

pub fn resolve_desktop_summarization_provider_config(
    database: &Database,
    db_config: &DbAgentConfig,
) -> Result<Option<(ProviderConfig, String, String)>, String> {
    let Some(provider_name) = db_config
        .summarization_provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    else {
        return Ok(None);
    };

    let requested_type = provider_type_for_parts(provider_name, None);
    if requested_type == provider_type_for_config(db_config) {
        return Ok(Some((
            desktop_provider_config(db_config),
            db_config.name.clone(),
            db_config
                .summarization_model
                .clone()
                .unwrap_or_else(|| db_config.model.clone()),
        )));
    }

    let mut candidates = database
        .list_agent_configs()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|candidate| provider_type_for_config(candidate) == requested_type)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(format!(
            "Summarization provider '{provider_name}' needs its own saved provider configuration; main-agent credentials are never reused across providers"
        ));
    }
    if let Some(model) = db_config
        .summarization_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let model_matches = candidates
            .iter()
            .filter(|candidate| candidate.model == model)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        if model_matches.len() == 1 {
            candidates.retain(|candidate| candidate.id == model_matches[0]);
        }
    }
    if candidates.len() > 1 {
        let defaults = candidates
            .iter()
            .filter(|candidate| candidate.is_default)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        if defaults.len() == 1 {
            candidates.retain(|candidate| candidate.id == defaults[0]);
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "Summarization provider '{provider_name}' matches multiple saved configurations; select a unique summarization model or keep summarization on the main provider"
        ));
    }
    let selected = candidates.pop().expect("one summarization candidate");
    let summary_model = db_config
        .summarization_model
        .clone()
        .unwrap_or_else(|| selected.model.clone());
    Ok(Some((
        desktop_provider_config(&selected),
        selected.name,
        summary_model,
    )))
}

pub fn desktop_memory_extraction_model(db_config: &DbAgentConfig) -> &str {
    db_config
        .summarization_model
        .as_deref()
        .unwrap_or(&db_config.model)
}

pub fn desktop_memory_extraction_provider_config(db_config: &DbAgentConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider_type_for_config(db_config),
        api_key: Some(db_config.api_key.clone()),
        base_url: db_config.base_url.clone(),
        org_id: None,
        timeout_secs: None,
    }
}

pub async fn run_desktop_agent_post_success_learning(
    request: DesktopAgentPostSuccessLearningRequest,
) {
    let DesktopAgentPostSuccessLearningRequest {
        db,
        conversation_id,
        db_config,
    } = request;

    let app_cfg = db.load_app_config().unwrap_or_default();
    if app_cfg.auto_memory_extraction {
        let extraction_route = match resolve_desktop_summarization_provider_config(&db, &db_config)
        {
            Ok(Some((config, _, model))) => Some((config, model)),
            Ok(None) => Some((
                desktop_memory_extraction_provider_config(&db_config),
                desktop_memory_extraction_model(&db_config).to_string(),
            )),
            Err(error) => {
                warn!("Auto memory extraction skipped for {conversation_id}: {error}");
                None
            }
        };
        if let Some((extract_provider_config, extract_model)) = extraction_route {
            let extract_provider_type = extract_provider_config.provider_type;
            match create_provider(extract_provider_config) {
                Ok(extract_llm) => {
                    match nexa_core::personalization::auto_extract_and_save(
                        &db,
                        &conversation_id,
                        extract_llm.as_ref(),
                        &extract_model,
                        Some(extract_provider_type),
                    )
                    .await
                    {
                        Ok(n) if n > 0 => {
                            info!(
                                "Auto-extracted {n} memories from conversation {conversation_id}"
                            );
                        }
                        Err(error) => {
                            warn!("Auto memory extraction failed for {conversation_id}: {error}");
                        }
                        _ => {}
                    }
                }
                Err(error) => {
                    warn!("Auto memory extraction provider failed for {conversation_id}: {error}");
                }
            }
        }
    }

    if app_cfg.auto_skill_learning {
        match nexa_core::evolution::review_recent_traces_for_evolution(&db, 5) {
            Ok(review) if review.events_created > 0 => {
                info!(
                    "Agent evolution review created {} event(s) for conversation {}",
                    review.events_created, conversation_id
                );
            }
            Err(e) => warn!("Agent evolution review failed for {conversation_id}: {e}"),
            _ => {}
        }
    }

    if app_cfg.dreaming.enabled && app_cfg.dreaming.after_successful_turn {
        if desktop_background_dream_budget_available(&db, &app_cfg) {
            match db.start_dream_run(nexa_core::dreaming::StartDreamInput {
                trigger_kind: Some("after_turn".to_string()),
                scope_json: Some(nexa_core::dreaming_scope::merge_configured_dream_scope(
                    &app_cfg.dreaming,
                    serde_json::json!({
                        "conversationId": conversation_id,
                        "surface": "desktop_agent_post_success_learning"
                    }),
                )),
                max_artifacts: Some(app_cfg.dreaming.max_artifacts_per_run),
            }) {
                Ok(run) => {
                    info!(
                        "Dreaming consolidation run {} completed after successful conversation {}",
                        run.id, conversation_id
                    );
                }
                Err(e) => warn!("Dreaming consolidation failed for {conversation_id}: {e}"),
            }
        } else {
            info!("Dreaming consolidation skipped for {conversation_id}: daily background budget reached");
        }
    }
}

fn desktop_background_dream_budget_available(db: &Database, app_cfg: &AppConfig) -> bool {
    let max_runs = app_cfg.dreaming.max_runs_per_day;
    if max_runs == 0 {
        return false;
    }
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let Ok(runs) = db.list_dream_runs(200) else {
        return false;
    };
    let used = runs
        .iter()
        .filter(|run| run.trigger_kind != "manual" && run.created_at.starts_with(&today))
        .count();
    used < max_runs
}

pub fn build_desktop_agent_user_content_parts(
    request: DesktopAgentUserContentRequest<'_>,
) -> Result<Vec<ContentPart>, String> {
    let DesktopAgentUserContentRequest {
        db,
        app_handle,
        provider_config,
        db_config,
        message,
        attachments,
    } = request;

    let vision_supported = model_supports_vision(&provider_config.provider_type, &db_config.model);
    info!(
        "Attachment check: provider={}, model={}, provider_type={:?}, vision_supported={}, has_attachments={}",
        db_config.provider,
        db_config.model,
        provider_config.provider_type,
        vision_supported,
        attachments.is_some_and(|items| !items.is_empty())
    );

    let mut user_parts = vec![ContentPart::Text {
        text: message.to_string(),
    }];
    let Some(attachments) = attachments else {
        return Ok(user_parts);
    };

    for attachment in attachments {
        if attachment.media_type.starts_with("image/") {
            if vision_supported {
                user_parts.push(ContentPart::Image {
                    media_type: attachment.media_type.clone(),
                    data: attachment.base64_data.clone(),
                });
            } else {
                warn!(
                    "Model '{}' (provider {:?}) does not support vision. Using OCR fallback for image '{}'.",
                    db_config.model, provider_config.provider_type, attachment.original_name
                );
                emit_user_content_event(
                    app_handle,
                    "image:ocr-fallback",
                    &serde_json::json!({
                        "image_name": attachment.original_name,
                        "model": db_config.model,
                        "reason": "Model does not support native image inputs"
                    }),
                );
                let ocr_config = db.load_ocr_config().unwrap_or_default();
                let image_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&attachment.base64_data)
                    .map_err(|e| format!("Failed to decode image: {e}"))?;
                let ocr_result = extract_text_from_image(
                    &image_bytes,
                    &attachment.media_type,
                    &ocr_config,
                    None,
                );
                info!(
                    "OCR fallback result for non-vision model: success={}, text_len={}",
                    ocr_result.is_ok(),
                    ocr_result.as_ref().map(|r| r.full_text.len()).unwrap_or(0)
                );
                match ocr_result {
                    Ok(result) if !result.full_text.is_empty() => {
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Image \"{}\" — processed via OCR (model does not support native vision)]:\n{}",
                                attachment.original_name, result.full_text
                            ),
                        });
                    }
                    _ => {
                        warn!(
                            "OCR fallback also failed for image '{}'. Install OCR model or use a vision-capable model.",
                            attachment.original_name
                        );
                        emit_user_content_event(
                            app_handle,
                            "image:ocr-failed",
                            &serde_json::json!({
                                "image_name": attachment.original_name,
                                "model": db_config.model,
                                "hint": "Install OCR model in Settings or switch to a vision-capable model"
                            }),
                        );
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Image \"{}\" attached but could not be processed — this model does not support image inputs and OCR is not available. Install the OCR model in Settings or use a vision-capable model.]",
                                attachment.original_name
                            ),
                        });
                    }
                }
            }
            continue;
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&attachment.base64_data)
            .map_err(|e| format!("Failed to decode attachment: {e}"))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            warn!(
                "Attachment '{}' is too large ({} bytes, limit {}). Skipping.",
                attachment.original_name,
                bytes.len(),
                MAX_ATTACHMENT_BYTES
            );
            user_parts.push(ContentPart::Text {
                text: format!(
                    "[Attached file \"{}\" skipped — file too large ({:.1} MB, limit 10 MB)]",
                    attachment.original_name,
                    bytes.len() as f64 / (1024.0 * 1024.0)
                ),
            });
            continue;
        }

        let ext = mime_to_extension(&attachment.media_type);
        let temp_path =
            std::env::temp_dir().join(format!("nexa-attach-{}.{}", Uuid::new_v4(), ext));
        if let Err(e) = std::fs::write(&temp_path, &bytes) {
            warn!(
                "Failed to write temp file for attachment '{}': {}",
                attachment.original_name, e
            );
            user_parts.push(ContentPart::Text {
                text: format!(
                    "[Attached file \"{}\" — could not process: {}]",
                    attachment.original_name, e
                ),
            });
            continue;
        }

        let parse_result = nexa_core::parse::parse_file(
            &temp_path,
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        );
        let _ = std::fs::remove_file(&temp_path);
        match parse_result {
            Ok(parsed) => {
                let text: String = parsed
                    .chunks
                    .iter()
                    .map(|c| c.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let visual_text = parsed
                    .visual_artifacts
                    .iter()
                    .map(|artifact| artifact.to_chunk_content())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let combined_text = [text.as_str(), visual_text.as_str()]
                    .into_iter()
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if combined_text.trim().is_empty() {
                    user_parts.push(ContentPart::Text {
                        text: format!(
                            "[Attached file \"{}\" — no text content could be extracted]",
                            attachment.original_name
                        ),
                    });
                } else {
                    info!(
                        "Parsed document attachment '{}': {} chars",
                        attachment.original_name,
                        combined_text.len()
                    );
                    user_parts.push(ContentPart::Text {
                        text: format!(
                            "[Attached file: {}]\n\n{}",
                            attachment.original_name, combined_text
                        ),
                    });
                }
            }
            Err(e) => {
                warn!(
                    "Failed to parse attachment '{}': {}",
                    attachment.original_name, e
                );
                user_parts.push(ContentPart::Text {
                    text: format!(
                        "[Attached file \"{}\" — could not extract content: {}]",
                        attachment.original_name, e
                    ),
                });
            }
        }
    }

    Ok(user_parts)
}

pub async fn build_desktop_agent_vision_user_content(
    request: DesktopAgentVisionUserContentRequest<'_>,
) -> Result<DesktopAgentVisionUserContentResult, String> {
    let DesktopAgentVisionUserContentRequest {
        db,
        app_handle,
        provider_config,
        db_config,
        message,
        attachments,
        vision_resolution,
        task_run_id,
        primary_egress_id,
        primary_routes_local,
        primary_native_vision_allowed,
        turn_override,
        cancellation,
    } = request;
    let mut parts = vec![ContentPart::Text {
        text: message.to_string(),
    }];
    let Some(input_attachments) = attachments else {
        return Ok(DesktopAgentVisionUserContentResult {
            parts,
            attachments: Vec::new(),
            llm_context_content: message.to_string(),
        });
    };
    let policy = vision_resolution
        .map(|resolution| VisionRouterPolicy::from_binding_options(&resolution.snapshot.options))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let ocr_config = db.load_ocr_config().unwrap_or_default();
    let primary_supports_vision = primary_native_vision_allowed
        && model_declares_vision_support(&provider_config.provider_type, &db_config.model);
    let primary_is_local = primary_routes_local;
    let auxiliary_is_local = vision_resolution
        .is_some_and(|resolution| provider_config_is_local(&resolution.provider_config));
    let mut persisted_attachments = Vec::with_capacity(input_attachments.len());
    let mut llm_context_fragments = vec![message.to_string()];
    let mut non_image_attachments = Vec::new();
    let mut provider_routes: Option<Vec<DesktopVisionProviderRoute>> = None;
    let mut selected_fallback_index = vision_resolution
        .map(|resolution| resolution.snapshot.fallback_index)
        .unwrap_or_default();

    for original in input_attachments {
        if !original.media_type.starts_with("image/") {
            let mut attachment = original.clone();
            attachment.vision_analysis = None;
            non_image_attachments.push(attachment.clone());
            persisted_attachments.push(attachment);
            continue;
        }
        if cancellation.is_cancelled() {
            return Err("Agent execution cancelled during image understanding".to_string());
        }
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode(&original.base64_data)
            .map_err(|error| format!("Failed to decode image: {error}"))?;
        if image_bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "Image attachment '{}' exceeds the {} byte limit",
                original.original_name, MAX_ATTACHMENT_BYTES
            ));
        }
        let computed_hash = attachment_hash(&image_bytes);
        if original
            .attachment_hash
            .as_deref()
            .is_some_and(|provided| !provided.eq_ignore_ascii_case(&computed_hash))
        {
            return Err(format!(
                "Image attachment '{}' changed after preparation",
                original.original_name
            ));
        }
        let attachment_id = original
            .attachment_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let decision = classify_vision_route(VisionClassificationInput {
            original_name: &original.original_name,
            mime_type: &original.media_type,
            user_prompt: message,
            policy: &policy,
            turn_override,
            primary_supports_vision,
            primary_is_local,
            auxiliary_available: vision_resolution.is_some(),
            auxiliary_is_local,
            ocr_available: ocr_config.enabled,
        })
        .map_err(|error| error.to_string())?;
        let target = vision_resolution.map(|resolution| VisionTargetProfile {
            binding_revision: resolution.snapshot.binding_revision,
            target_id: resolution.snapshot.target_id.clone(),
            target_revision: resolution.snapshot.target_revision,
            connection_id: resolution.snapshot.connection_id.clone(),
            connection_revision: resolution.snapshot.connection_revision,
            descriptor_hash: resolution.snapshot.descriptor_hash.clone(),
        });
        let fallback_targets = vision_resolution
            .map(|resolution| {
                resolution
                    .snapshot
                    .fallback_targets
                    .iter()
                    .filter(|candidate| {
                        candidate.fallback_index > resolution.snapshot.fallback_index
                    })
                    .map(|candidate| VisionTargetProfile {
                        binding_revision: resolution.snapshot.binding_revision,
                        target_id: candidate.target_id.clone(),
                        target_revision: candidate.target_revision,
                        connection_id: candidate.connection_id.clone(),
                        connection_revision: candidate.connection_revision,
                        descriptor_hash: candidate.descriptor_hash.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let profile = VisionProfileV1 {
            observation_schema_version: VISION_OBSERVATION_SCHEMA_VERSION,
            classifier_version: VISION_CLASSIFIER_VERSION,
            intent: decision.intent,
            mode: policy.mode,
            turn_override,
            prefer_local_processing: policy.prefer_local_processing,
            local_only: policy.local_only,
            primary_egress_id: primary_egress_id.to_string(),
            primary_is_local,
            fallback_mode: vision_resolution
                .map(|resolution| resolution.snapshot.fallback_mode)
                .unwrap_or_default(),
            constraints: vision_resolution
                .map(|resolution| resolution.snapshot.constraints.clone())
                .unwrap_or_default(),
            ocr: VisionOcrProfile {
                enabled: ocr_config.enabled,
                confidence_threshold_millis: (ocr_config.confidence_threshold * 1_000.0)
                    .round()
                    .clamp(0.0, 1_000.0) as u16,
                det_limit_side_len: ocr_config.det_limit_side_len,
                use_cls: ocr_config.use_cls,
                languages: ocr_config.languages.clone(),
            },
            target,
            fallback_targets,
        };
        let profile_hash = profile.profile_hash().map_err(|error| error.to_string())?;
        let mut attachment = original.clone();
        attachment.attachment_id = Some(attachment_id.clone());
        attachment.attachment_hash = Some(computed_hash.clone());
        attachment.vision_analysis = None;

        match decision.plan {
            nexa_core::vision_router::VisionRoutePlan::NativeDirect => {
                parts.push(ContentPart::Image {
                    media_type: original.media_type.clone(),
                    data: original.base64_data.clone(),
                });
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status: VisionAttachmentStatus::MetadataOnly,
                    profile_hash: Some(profile_hash),
                    observation: None,
                    reason_code: Some("native_direct_unstructured".to_string()),
                });
                llm_context_fragments.push(format!(
                    "[Image: {} — processed directly by the pinned native vision model]",
                    original.original_name
                ));
            }
            nexa_core::vision_router::VisionRoutePlan::MetadataOnly => {
                let metadata = format!(
                    "[Image: {} — image understanding disabled; no pixels were sent to a model]",
                    original.original_name
                );
                parts.push(ContentPart::Text {
                    text: metadata.clone(),
                });
                llm_context_fragments.push(metadata);
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status: VisionAttachmentStatus::MetadataOnly,
                    profile_hash: Some(profile_hash),
                    observation: None,
                    reason_code: Some("vision_disabled".to_string()),
                });
            }
            _ => {
                let now_epoch = Utc::now().timestamp();
                let cached = if policy.cache_enabled {
                    db.get_vision_observation_cache(&computed_hash, &profile_hash, now_epoch)
                        .map_err(|error| error.to_string())?
                } else {
                    None
                };
                let (observation, status) = if let Some(cached) = cached {
                    let mut observation = cached.observation;
                    observation.attachment_id = attachment_id.clone();
                    observation.validate().map_err(|error| error.to_string())?;
                    (observation, VisionAttachmentStatus::Cached)
                } else {
                    let requires_vision = matches!(
                        decision.plan,
                        nexa_core::vision_router::VisionRoutePlan::VisionOnly
                            | nexa_core::vision_router::VisionRoutePlan::OcrThenVision
                            | nexa_core::vision_router::VisionRoutePlan::VisionThenOcr
                    );
                    if requires_vision && provider_routes.is_none() {
                        provider_routes = vision_resolution
                            .map(build_desktop_vision_provider_routes)
                            .transpose()?;
                    }
                    let vision = provider_routes
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .filter(|route| route.fallback_index >= selected_fallback_index)
                        .filter(|route| !policy.local_only || route.local)
                        .map(|route| VisionProviderInput {
                            provider: route.provider.as_ref(),
                            provider_type: route.provider_type,
                            provider_id: &route.provider_id,
                            egress_id: &route.egress_id,
                            model_id: &route.model_id,
                            target_id: &route.target_id,
                            target_revision: route.target_revision,
                            fallback_index: route.fallback_index,
                            local: route.local,
                        })
                        .collect::<Vec<_>>();
                    let observation = execute_vision_observation(VisionExecutionInput {
                        attachment_id: &attachment_id,
                        attachment_hash: &computed_hash,
                        profile_hash: &profile_hash,
                        image_bytes: &image_bytes,
                        mime_type: &original.media_type,
                        decision,
                        ocr_config: &ocr_config,
                        vision: &vision,
                        route_primary_fallback_index: vision_resolution
                            .map(|resolution| resolution.snapshot.fallback_index)
                            .unwrap_or_default(),
                        primary_egress_id,
                        primary_is_local,
                        cancellation,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                    (observation, VisionAttachmentStatus::Observed)
                };
                if let Some(next_fallback_index) = observation
                    .sources
                    .iter()
                    .filter_map(|source| source.fallback_index)
                    .max()
                    .filter(|index| *index > selected_fallback_index)
                {
                    db.advance_task_runtime_fallback(
                        task_run_id,
                        "vision",
                        selected_fallback_index,
                        next_fallback_index,
                        "vision_invocation_failed_automatic_fallback",
                    )
                    .map_err(|error| error.to_string())?;
                    selected_fallback_index = next_fallback_index;
                }
                if policy.cache_enabled && status == VisionAttachmentStatus::Observed {
                    let expires_at_epoch =
                        now_epoch + i64::from(policy.cache_retention_days) * 24 * 60 * 60;
                    db.save_vision_observation_cache(&observation, now_epoch, expires_at_epoch)
                        .map_err(|error| error.to_string())?;
                }
                let prompt = observation_prompt_text(&original.original_name, &observation)
                    .map_err(|error| error.to_string())?;
                parts.push(ContentPart::Text {
                    text: prompt.clone(),
                });
                llm_context_fragments.push(prompt);
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status,
                    profile_hash: Some(profile_hash),
                    observation: Some(observation),
                    reason_code: None,
                });
            }
        }
        persisted_attachments.push(attachment);
    }

    if !non_image_attachments.is_empty() {
        let mut document_parts =
            build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
                db,
                app_handle,
                provider_config,
                db_config,
                message: "",
                attachments: Some(&non_image_attachments),
            })?;
        if document_parts
            .first()
            .is_some_and(|part| matches!(part, ContentPart::Text { text } if text.is_empty()))
        {
            document_parts.remove(0);
        }
        for part in &document_parts {
            if let ContentPart::Text { text } = part {
                llm_context_fragments.push(text.clone());
            }
        }
        parts.extend(document_parts);
    }

    Ok(DesktopAgentVisionUserContentResult {
        parts,
        attachments: persisted_attachments,
        llm_context_content: llm_context_fragments
            .into_iter()
            .filter(|fragment| !fragment.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    })
}

fn build_desktop_vision_provider_routes(
    resolution: &RuntimeCapabilityResolution,
) -> Result<Vec<DesktopVisionProviderRoute>, String> {
    let mut routes = vec![DesktopVisionProviderRoute {
        fallback_index: resolution.snapshot.fallback_index,
        target_id: resolution.snapshot.target_id.clone(),
        target_revision: resolution.snapshot.target_revision,
        provider_id: resolution.snapshot.provider_id.clone(),
        egress_id: format!("registry:{}", resolution.snapshot.connection_id),
        model_id: resolution.model_id.clone(),
        local: provider_config_is_local(&resolution.provider_config),
        provider_type: resolution.provider_config.provider_type,
        provider: create_provider(resolution.provider_config.clone()).map_err(|_| {
            "vision_provider_initialization_failed: provider details were redacted".to_string()
        })?,
    }];
    for fallback in &resolution.fallbacks {
        let snapshot = resolution
            .snapshot
            .fallback_targets
            .iter()
            .find(|candidate| candidate.fallback_index == fallback.fallback_index)
            .ok_or_else(|| {
                "vision_fallback_snapshot_missing: frozen fallback metadata is incomplete"
                    .to_string()
            })?;
        routes.push(DesktopVisionProviderRoute {
            fallback_index: fallback.fallback_index,
            target_id: fallback.target_id.clone(),
            target_revision: fallback.target_revision,
            provider_id: snapshot.provider_id.clone(),
            egress_id: format!("registry:{}", fallback.connection_id),
            model_id: fallback.model_id.clone(),
            local: provider_config_is_local(&fallback.provider_config),
            provider_type: fallback.provider_config.provider_type,
            provider: create_provider(fallback.provider_config.clone()).map_err(|_| {
                "vision_fallback_initialization_failed: provider details were redacted".to_string()
            })?,
        });
    }
    routes.sort_by_key(|route| route.fallback_index);
    Ok(routes)
}

pub(super) fn provider_config_is_local(config: &ProviderConfig) -> bool {
    let Some(base_url) = config.base_url.as_deref() else {
        return matches!(
            config.provider_type,
            ProviderType::Ollama | ProviderType::LmStudio
        );
    };
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(super) fn provider_config_egress_id(config: &ProviderConfig) -> String {
    let endpoint = config
        .base_url
        .as_deref()
        .and_then(|base_url| reqwest::Url::parse(base_url).ok())
        .and_then(|url| {
            let host = url.host_str()?.to_ascii_lowercase();
            let port = url.port_or_known_default()?;
            Some(format!("{}://{host}:{port}", url.scheme()))
        })
        .unwrap_or_else(|| "default-endpoint".to_string());
    format!("legacy:{:?}:{endpoint}", config.provider_type)
}

pub fn build_desktop_agent_turn_config(
    request: DesktopAgentTurnConfigRequest<'_>,
) -> DesktopAgentTurnConfig {
    let DesktopAgentTurnConfigRequest {
        db,
        conversation,
        turn_id,
        message,
        persona_id,
        explicit_skill_ids,
        db_config,
        app_cfg,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
        custom_orchestration,
    } = request;

    let source_scope_ids = db
        .get_effective_conversation_source_scope(&conversation.id)
        .unwrap_or_default();
    let source_scope_section =
        nexa_core::conversation::build_source_scope_prompt_section(db, &source_scope_ids)
            .unwrap_or_default();
    let collection_context_section =
        nexa_core::conversation::build_collection_context_prompt_section(
            conversation.collection_context.as_ref(),
        );
    let memory_section =
        nexa_core::personalization::build_memory_summary_for_query(db, Some(message))
            .unwrap_or_default();
    let project_memory_section = nexa_core::project_memory::build_project_memory_summary_for_query(
        db,
        conversation.project_id.as_deref(),
        Some(message),
    )
    .unwrap_or_default();
    let project_workspace = conversation.project_id.as_deref().and_then(|project_id| {
        db.get_project_workspace_snapshot(project_id, Some(message))
            .ok()
    });
    let project_instruction_section = project_workspace
        .as_ref()
        .map(nexa_core::project_runtime::build_project_instruction_section)
        .unwrap_or_default();
    let project_evidence_section = project_workspace
        .as_ref()
        .map(nexa_core::project_runtime::build_project_evidence_section)
        .unwrap_or_default();
    let narrative_evidence_section = conversation
        .project_id
        .as_deref()
        .and_then(|project_id| db.build_project_narrative_plan(project_id, message, 8).ok())
        .map(|plan| nexa_core::event_claim_graph::build_narrative_evidence_section(&plan))
        .unwrap_or_default();
    let agent_memory_section =
        nexa_core::evolution::build_agent_procedural_memory_summary_for_query(db, Some(message))
            .unwrap_or_default();
    if !agent_memory_section.is_empty() {
        let memory_hits = db
            .search_agent_procedural_memories(message, 3)
            .or_else(|_| db.list_agent_procedural_memories(2))
            .unwrap_or_default();
        for memory in memory_hits {
            let _ = db.record_memory_injection_event(
                &memory.id,
                Some(&conversation.id),
                Some(turn_id),
                message,
                "agent_procedural_memory_prompt",
                Some(memory.confidence),
            );
        }
    }
    let preference_section =
        nexa_core::personalization::build_preference_summary_for_query(db, Some(message))
            .unwrap_or_default();
    let learned_section = {
        let cfg = db.get_embedder_config().ok();
        let embedding = cfg.and_then(|c| match nexa_core::embed::create_embedder(&c) {
            Ok(embedder) if embedder.dimensions() > 0 => embedder.embed(message).ok(),
            _ => None,
        });
        match embedding {
            Some(vec) if !vec.iter().all(|&v| v == 0.0) => {
                match nexa_core::learning::retrieve_similar_successes(db, &vec, 3) {
                    Ok(hits) => nexa_core::learning::build_learned_successes_section(&hits),
                    Err(_) => String::new(),
                }
            }
            _ => String::new(),
        }
    };
    let scratchpad_section = nexa_core::agent::scratchpad::build_agent_scratchpad_prompt_section(
        db,
        Some(&conversation.id),
    );
    let requested_persona_id = persona_id
        .or(conversation.persona_id.as_deref())
        .unwrap_or("default");
    let persona_profile = match nexa_core::persona::enabled_persona_by_id(db, requested_persona_id)
    {
        Ok(persona) => persona,
        Err(err) => {
            warn!("Failed to load persona '{requested_persona_id}': {err}");
            None
        }
    };
    let effective_persona_id = persona_profile
        .as_ref()
        .map(|persona| persona.id.as_str())
        .unwrap_or("default");
    if conversation.persona_id.as_deref().unwrap_or("default") != effective_persona_id {
        let _ = db.update_conversation_persona(
            &conversation.id,
            if effective_persona_id == "default" {
                None
            } else {
                Some(effective_persona_id)
            },
        );
    }
    let persona_default_skill_ids = persona_profile
        .as_ref()
        .map(|persona| persona.default_skill_ids.clone())
        .unwrap_or_default();
    let mut pinned_skill_ids = persona_default_skill_ids;
    for id in explicit_skill_ids {
        let trimmed = id.trim();
        if !trimmed.is_empty() && !pinned_skill_ids.iter().any(|existing| existing == trimmed) {
            pinned_skill_ids.push(trimmed.to_string());
        }
    }
    let persona_section =
        nexa_core::persona::build_persona_prompt_section(persona_profile.as_ref());
    let current_turn_time_section = build_current_turn_time_section();
    let plan_mode_section = if execution_mode.is_plan() {
        nexa_core::agent::plan_mode_prompt_section()
    } else {
        ""
    };
    let provider_type = provider_type_for_config(db_config);
    let configured_reasoning_effort =
        db_config
            .reasoning_effort
            .as_ref()
            .and_then(|effort| match effort.as_str() {
                "none" => Some(ReasoningEffort::None),
                "minimal" => Some(ReasoningEffort::Minimal),
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                "max" => Some(ReasoningEffort::Max),
                "xhigh" => Some(ReasoningEffort::XHigh),
                _ => None,
            });
    let active_goal = db
        .get_conversation_goal(&conversation.id)
        .ok()
        .flatten()
        .filter(|goal| goal.status == nexa_core::conversation::ConversationGoalStatus::Active);
    let goal_section = nexa_core::conversation::goal::build_conversation_goal_prompt_section(
        db,
        &conversation.id,
        !execution_mode.is_plan(),
    );
    let configured_max_iterations = if active_goal.is_some() && !execution_mode.is_plan() {
        u32::MAX
    } else {
        db_config
            .max_iterations
            .map(|value| value as u32)
            .unwrap_or(u32::MAX)
    };
    let power_policy = resolve_agent_power_policy(AgentPowerPolicyInput {
        mode: power_mode,
        provider_type,
        model: &db_config.model,
        max_iterations: configured_max_iterations,
        reasoning_enabled: db_config.reasoning_enabled,
        thinking_budget: db_config.thinking_budget.map(|value| value as u32),
        reasoning_effort: configured_reasoning_effort,
        subagent_max_parallel: db_config.subagent_max_parallel.map(|value| value as u32),
        subagent_max_calls_per_turn: db_config
            .subagent_max_calls_per_turn
            .map(|value| value as u32),
        subagent_token_budget: db_config.subagent_token_budget.map(|value| value as u32),
    });
    let power_mode_section = power_policy.prompt_section().to_string();
    let orchestration_policy = resolve_orchestration_profile(OrchestrationProfileInput {
        profile: orchestration_profile,
        custom: custom_orchestration.clone(),
        max_iterations: power_policy.max_iterations,
        max_parallel: power_policy.subagent_max_parallel,
        max_calls_per_turn: power_policy.subagent_max_calls_per_turn,
        delegated_token_budget: power_policy.subagent_token_budget,
        verification_reserve_percent: power_policy.verification_reserve_percent,
    });
    let profile_overrides_persisted_delegation =
        orchestration_profile != OrchestrationProfile::Balanced || power_mode.is_nexus();
    let subagent_max_parallel = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_max_parallel
    } else {
        Some(orchestration_policy.max_parallel)
    };
    let subagent_max_calls_per_turn = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_max_calls_per_turn
    } else {
        Some(orchestration_policy.max_calls_per_turn)
    };
    let subagent_token_budget = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_token_budget
    } else {
        Some(orchestration_policy.delegated_token_budget)
    };
    let delegation_limits_v2 = db_config.delegation_limits_v2.clone().map(|mut limits| {
        if profile_overrides_persisted_delegation {
            limits.max_parallel = subagent_max_parallel;
            limits.max_calls_per_turn = subagent_max_calls_per_turn;
            limits.total_actual_tokens_soft_limit = subagent_token_budget.map(u64::from);
        }
        limits
    });
    let orchestration_profile_section = orchestration_policy.prompt_section();
    let collaboration_mode_section = if collaboration_mode.is_moa() {
        format!(
            "## Mixture-of-Agents Collaboration\n\nThe user explicitly selected the `{}` virtual-provider preset for this turn. Private tool-free advisors may inform model calls, but only the aggregator may answer or call tools. MoA remains independent from Nexus; do not recursively enable MoA inside delegated workers.",
            moa_preset.as_str()
        )
    } else {
        String::new()
    };
    let conversation_system_prompt = db
        .get_effective_conversation_system_prompt(conversation)
        .unwrap_or_else(|error| {
            warn!(
                "Failed to resolve conversation instruction provenance for {}: {}",
                conversation.id, error
            );
            if conversation.project_id.is_some() {
                String::new()
            } else {
                conversation.system_prompt.clone()
            }
        });
    let base_system_prompt = build_system_prompt(Some(&conversation_system_prompt), &[]);
    let context_budget = db_config
        .context_window
        .and_then(|window| u32::try_from(window).ok())
        .map(|window| window.saturating_mul(3) / 5);
    let mut context_assembler = ContextAssembler::new("agent_turn", context_budget);
    let context_items = [
        (
            "system-instructions",
            ContextItemRole::Instruction,
            "runtime",
            "stable runtime and conversation instructions",
            ContextTrustLevel::System,
            1_000,
            ContextItemStability::StablePrefix,
            base_system_prompt,
        ),
        (
            "current-turn-time",
            ContextItemRole::Instruction,
            "runtime.clock",
            "current turn time",
            ContextTrustLevel::System,
            130,
            ContextItemStability::VolatileSuffix,
            current_turn_time_section,
        ),
        (
            "project-instructions",
            ContextItemRole::Instruction,
            "project.instructions",
            "live user-maintained project instructions",
            ContextTrustLevel::UserSelected,
            950,
            ContextItemStability::StablePrefix,
            project_instruction_section,
        ),
        (
            "execution-mode",
            ContextItemRole::Instruction,
            "runtime.execution_mode",
            "selected execution mode",
            ContextTrustLevel::System,
            120,
            ContextItemStability::VolatileSuffix,
            plan_mode_section.to_string(),
        ),
        (
            "power-policy",
            ContextItemRole::Instruction,
            "runtime.power_policy",
            "resolved power policy",
            ContextTrustLevel::System,
            110,
            ContextItemStability::VolatileSuffix,
            power_mode_section,
        ),
        (
            "quality-policy",
            ContextItemRole::Instruction,
            "runtime.orchestration_profile",
            "resolved orchestration quality profile",
            ContextTrustLevel::System,
            108,
            ContextItemStability::VolatileSuffix,
            orchestration_profile_section,
        ),
        (
            "collaboration-policy",
            ContextItemRole::Instruction,
            "runtime.collaboration_mode",
            "selected LLM collaboration policy",
            ContextTrustLevel::System,
            106,
            ContextItemStability::VolatileSuffix,
            collaboration_mode_section,
        ),
        (
            "active-goal",
            ContextItemRole::Instruction,
            "conversation.goal",
            "active user goal",
            ContextTrustLevel::UserSelected,
            100,
            ContextItemStability::VolatileSuffix,
            goal_section,
        ),
        (
            "persona",
            ContextItemRole::Instruction,
            "persona",
            "selected persona",
            ContextTrustLevel::UserSelected,
            90,
            ContextItemStability::VolatileSuffix,
            persona_section,
        ),
        (
            "collection-context",
            ContextItemRole::SourceScope,
            "conversation.collection",
            "selected collection context",
            ContextTrustLevel::UserSelected,
            80,
            ContextItemStability::VolatileSuffix,
            collection_context_section,
        ),
        (
            "source-scope",
            ContextItemRole::SourceScope,
            "conversation.sources",
            "effective source scope",
            ContextTrustLevel::UserSelected,
            70,
            ContextItemStability::VolatileSuffix,
            source_scope_section,
        ),
        (
            "user-memory",
            ContextItemRole::Memory,
            "memory.user",
            "query-relevant user memory",
            ContextTrustLevel::AgentMemory,
            60,
            ContextItemStability::VolatileSuffix,
            memory_section,
        ),
        (
            "project-memory",
            ContextItemRole::Memory,
            "memory.project",
            "query-relevant project memory",
            ContextTrustLevel::AgentMemory,
            50,
            ContextItemStability::VolatileSuffix,
            project_memory_section,
        ),
        (
            "project-workspace-evidence",
            ContextItemRole::Evidence,
            "project.workspace",
            "query-relevant project episodes and observed events",
            ContextTrustLevel::RetrievedEvidence,
            55,
            ContextItemStability::VolatileSuffix,
            project_evidence_section,
        ),
        (
            "event-claim-narrative",
            ContextItemRole::Evidence,
            "knowledge.event_claim_graph",
            "query-classified event and claim narrative with review state",
            ContextTrustLevel::RetrievedEvidence,
            54,
            ContextItemStability::VolatileSuffix,
            narrative_evidence_section,
        ),
        (
            "procedural-memory",
            ContextItemRole::Memory,
            "memory.procedural",
            "query-relevant procedural memory",
            ContextTrustLevel::AgentMemory,
            40,
            ContextItemStability::VolatileSuffix,
            agent_memory_section,
        ),
        (
            "preferences",
            ContextItemRole::Memory,
            "memory.preferences",
            "query-relevant preferences",
            ContextTrustLevel::AgentMemory,
            30,
            ContextItemStability::VolatileSuffix,
            preference_section,
        ),
        (
            "learned-successes",
            ContextItemRole::Memory,
            "memory.learned_successes",
            "similar successful trajectories",
            ContextTrustLevel::AgentMemory,
            20,
            ContextItemStability::VolatileSuffix,
            learned_section,
        ),
        (
            "scratchpad",
            ContextItemRole::Memory,
            "memory.scratchpad",
            "conversation scratchpad",
            ContextTrustLevel::AgentMemory,
            10,
            ContextItemStability::VolatileSuffix,
            scratchpad_section,
        ),
    ];
    for (id, role, source, reason, trust, priority, stability, text) in context_items {
        context_assembler
            .add(ContextPackItem::text(
                id, role, source, reason, trust, priority, stability, text,
            ))
            .expect("desktop context contributors use stable unique ids");
    }
    let context_pack = context_assembler.assemble();
    let system_prompt = context_pack
        .prompt_sections_for_stability(ContextItemStability::StablePrefix)
        .join("\n\n");
    let volatile_system_sections =
        context_pack.prompt_sections_for_stability(ContextItemStability::VolatileSuffix);

    let executor_config = AgentConfig {
        max_iterations: orchestration_policy.max_iterations,
        system_prompt,
        volatile_system_sections,
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config.max_tokens.map(|t| t as u32),
        context_window: db_config.context_window.map(|w| w as u32),
        reasoning_enabled: power_policy.reasoning_enabled,
        thinking_budget: power_policy.thinking_budget,
        reasoning_effort: power_policy.reasoning_effort,
        provider_type: Some(provider_type),
        native_search_plan: nexa_core::llm::native_search::NativeSearchPlan::resolve(
            app_cfg.web_search.execution_mode,
            provider_type,
            db_config.base_url.as_deref(),
            &db_config.model,
        ),
        request_kind: AgentRequestKind::MainAgentStep,
        summarization_model: db_config.summarization_model.clone(),
        summarization_provider_type: db_config
            .summarization_provider
            .as_deref()
            .map(|provider| provider_type_for_parts(provider, None)),
        subagent_max_parallel,
        subagent_max_calls_per_turn,
        subagent_token_budget,
        subagent_verification_reserve_percent: if orchestration_profile
            == OrchestrationProfile::Balanced
        {
            power_policy.verification_reserve_percent
        } else {
            Some(orchestration_policy.verification_reserve_percent)
        },
        delegation_limits_v2,
        tool_timeout_secs: Some(UNLIMITED_EXECUTOR_TIMEOUT_SECS),
        agent_timeout_secs: Some(UNLIMITED_EXECUTOR_TIMEOUT_SECS),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: app_cfg.dynamic_tool_visibility,
        trace_enabled: app_cfg.trace_enabled,
        require_tool_confirmation: app_cfg.confirm_destructive,
        shell_access_mode: app_cfg.shell_access_mode,
        tool_approval_mode: app_cfg.tool_approval_mode,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
        custom_orchestration,
    };

    DesktopAgentTurnConfig {
        executor_config,
        source_scope_ids,
        pinned_skill_ids,
        context_pack,
    }
}

pub fn build_desktop_agent_session_config(
    input: DesktopAgentSessionConfigInput<'_>,
) -> nexa_core::runtime::AgentSessionConfig {
    let mut config = nexa_core::runtime::AgentSessionConfig {
        session_id: input.conversation_id.to_string(),
        conversation_id: Some(input.conversation_id.to_string()),
        task_run_id: Some(input.task_run_id.to_string()),
        host_surface: nexa_core::runtime::RuntimeHostSurface::Desktop,
        provider: Some(input.db_config.provider.clone()),
        model: Some(input.db_config.model.clone()),
        reasoning_enabled: input.db_config.reasoning_enabled,
        thinking_budget: input.db_config.thinking_budget.map(|value| value as u32),
        reasoning_effort: input.db_config.reasoning_effort.clone(),
        source_scope: nexa_core::runtime::RuntimeSourceScope {
            source_ids: input.source_scope_ids.to_vec(),
            collection_id: None,
            working_directory: None,
        },
        approval_mode: input.app_cfg.tool_approval_mode,
        shell_access_mode: input.app_cfg.shell_access_mode,
        execution_mode: input.execution_mode,
        collaboration_mode: input.collaboration_mode,
        moa_preset: input.moa_preset,
        orchestration_profile: input.orchestration_profile,
        custom_orchestration: input.custom_orchestration.clone(),
        trace_enabled: input.app_cfg.trace_enabled,
        skill_context: nexa_core::runtime::RuntimeSkillContext {
            available_skill_ids: input
                .selected_skills
                .iter()
                .map(|skill| skill.id.clone())
                .collect(),
            loaded_skill_ids: input
                .auto_loaded_skills
                .iter()
                .map(|skill| skill.id.clone())
                .collect(),
            trust_state: None,
        },
        package_context: desktop_runtime_package_context(input.db),
        metadata: serde_json::json!({
            "kind": "desktopAgentSessionConfig",
            "agentConfigId": input.db_config.id,
            "agentConfigName": input.db_config.name,
        }),
        ..Default::default()
    };
    config.apply_protocol_defaults();
    config
}

pub fn desktop_runtime_package_context(db: &Database) -> nexa_core::runtime::RuntimePackageContext {
    nexa_core::package_host::database_backed_builtin_runtime_package_context(db)
        .expect("database-backed builtin Package Host snapshot is valid")
}

pub fn filter_desktop_tool_names_by_package_host(
    db: &Database,
    names: Vec<String>,
) -> Result<Vec<String>, String> {
    PackageRuntimeAssembler::database_builtin(db)
        .and_then(|assembler| assembler.visible_tool_names(names))
        .map_err(|error| error.to_string())
}

pub fn runtime_session_config_artifact(
    config: &nexa_core::runtime::AgentSessionConfig,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentSessionConfig",
        "version": 1,
        "config": config,
    })
}

pub fn build_desktop_agent_initial_task_artifacts(
    selected_skills: &[Skill],
    runtime_session_config: &nexa_core::runtime::AgentSessionConfig,
    context_pack: &ContextPack,
    execution_mode: AgentExecutionMode,
    executor_config: &AgentConfig,
) -> serde_json::Value {
    let mut artifacts = serde_json::json!({
        "kind": "agentTaskArtifacts",
        "version": 1,
        "selectedSkills": build_selected_skills_artifact(selected_skills),
        "runtimeSession": runtime_session_config_artifact(runtime_session_config),
        "contextPack": context_pack,
    });
    if execution_mode.is_plan() {
        artifacts["executionMode"] = execution_mode_artifact(execution_mode);
    }
    if executor_config.power_mode.is_nexus() {
        artifacts["powerMode"] = power_mode_artifact(executor_config);
    }
    if executor_config.collaboration_mode.is_moa() {
        artifacts["collaborationMode"] = collaboration_mode_artifact(executor_config);
    }
    artifacts["orchestrationProfile"] = orchestration_profile_artifact(executor_config);
    artifacts
}

async fn sync_enabled_desktop_mcp_servers(
    manager: &mut McpManager,
    enabled_servers: &[McpServer],
    timeout_secs: u64,
) -> Result<HashMap<String, String>, String> {
    Ok(manager
        .sync_servers(enabled_servers, Some(timeout_secs))
        .await)
}

#[derive(Clone)]
struct DesktopToolRegistrySnapshot {
    generation: String,
    tools: ToolRegistry,
}

static DESKTOP_TOOL_REGISTRY_SNAPSHOT: OnceLock<TokioMutex<Option<DesktopToolRegistrySnapshot>>> =
    OnceLock::new();

fn desktop_tool_registry_generation(
    assembler: &PackageRuntimeAssembler,
    enabled_servers: &[McpServer],
) -> Result<String, String> {
    let snapshot = serde_json::to_vec(&(assembler.snapshot(), enabled_servers))
        .map_err(|error| format!("Failed to serialize tool registry generation: {error}"))?;
    Ok(blake3::hash(&snapshot).to_hex().to_string())
}

pub async fn build_desktop_agent_session_dependencies(
    request: DesktopAgentSessionDependencyRequest<'_>,
) -> DesktopAgentSessionDependencies {
    let DesktopAgentSessionDependencyRequest {
        db,
        mcp_manager,
        app_handle,
        event_seq,
        conversation_id,
        task_run_id,
        turn_id,
        message,
        pinned_skill_ids,
        provider_config,
        executor_config,
        subagent_allowed_tools,
        subagent_allowed_skill_ids,
        cancel_token,
        plan_mode,
        mcp_call_timeout_secs,
        terminal_state,
        browser_state,
    } = request;

    let skill_select_started = Instant::now();
    let selected_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_available_skills_for_query(db, message)
    } else {
        nexa_core::skills::get_available_skills_for_query_with_pinned(db, message, pinned_skill_ids)
    }
    .unwrap_or_else(|err| {
        warn!("Failed to select skills for task run {task_run_id}: {err}");
        Vec::new()
    });

    let max_loaded_skills = 3usize.max(pinned_skill_ids.len());
    let auto_loaded_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_active_skills_for_query(db, message, max_loaded_skills)
    } else {
        nexa_core::skills::get_active_skills_for_query_with_pinned(
            db,
            message,
            max_loaded_skills,
            pinned_skill_ids,
        )
    }
    .unwrap_or_else(|err| {
        warn!("Failed to auto-load skills for task run {task_run_id}: {err}");
        Vec::new()
    });
    let skill_select_ms = elapsed_ms(skill_select_started);

    let tool_registry_started = Instant::now();
    let package_assembler = PackageRuntimeAssembler::database_builtin(db);
    emit_agent_frontend_event_with_presentation(
        app_handle,
        event_seq,
        conversation_id,
        task_run_id,
        Some(turn_id),
        AgentEvent::Status {
            content: "Loading tools and MCP servers".to_string(),
            tone: None,
        },
        AgentRunEventVisibility::Internal,
        AgentRunDisplayKind::Status,
        AgentRunEventImportance::Low,
    );
    let enabled_servers = db.get_enabled_mcp_servers().map_err(|error| {
        warn!("Failed to load enabled MCP servers: {error}");
        error
    });
    let configuration_generation = package_assembler
        .as_ref()
        .ok()
        .zip(enabled_servers.as_ref().ok())
        .and_then(|(assembler, servers)| {
            desktop_tool_registry_generation(assembler, servers)
                .map_err(|error| warn!("{error}"))
                .ok()
        });
    let mut manager = mcp_manager.lock().await;
    let generation = configuration_generation.as_ref().map(|generation| {
        format!(
            "{mcp_manager:p}:{generation}:{}",
            manager.connection_generation()
        )
    });
    let snapshot_cache = DESKTOP_TOOL_REGISTRY_SNAPSHOT.get_or_init(|| TokioMutex::new(None));
    let mut snapshot_guard = snapshot_cache.lock().await;
    let cached_tools = generation.as_ref().and_then(|generation| {
        snapshot_guard
            .as_ref()
            .filter(|snapshot| snapshot.generation == *generation)
            .map(|snapshot| snapshot.tools.clone())
    });
    let (mut tools, mcp_sync_ms) = if let Some(tools) = cached_tools {
        (tools, 0)
    } else {
        let mut tools = package_assembler
            .as_ref()
            .map(PackageRuntimeAssembler::builtin_tool_registry)
            .unwrap_or_else(|error| {
                warn!("Failed to initialize Package Runtime Assembler: {error}");
                canonical_builtin_tool_registry()
            });
        let mcp_sync_started = Instant::now();
        let mut registry_snapshot_complete = enabled_servers.is_ok();
        if let Ok(enabled_servers) = enabled_servers.as_ref() {
            match sync_enabled_desktop_mcp_servers(
                &mut manager,
                enabled_servers,
                mcp_call_timeout_secs,
            )
            .await
            {
                Ok(errors) => {
                    registry_snapshot_complete = errors.is_empty();
                    for (server_id, error) in errors {
                        warn!("Failed to sync MCP server {server_id}: {error}");
                    }
                }
                Err(error) => {
                    registry_snapshot_complete = false;
                    warn!("Failed to sync enabled MCP servers: {error}");
                }
            }
            if let Err(error) = manager
                .register_tools_with_recovery(&mut tools, Arc::downgrade(mcp_manager))
                .await
            {
                registry_snapshot_complete = false;
                warn!("Failed to register MCP tools: {error}");
            }
        }
        let mcp_sync_ms = elapsed_ms(mcp_sync_started);
        if registry_snapshot_complete {
            if let Some(configuration_generation) = configuration_generation {
                let generation = format!(
                    "{mcp_manager:p}:{configuration_generation}:{}",
                    manager.connection_generation()
                );
                *snapshot_guard = Some(DesktopToolRegistrySnapshot {
                    generation,
                    tools: tools.clone(),
                });
            }
        }
        (tools, mcp_sync_ms)
    };
    drop(snapshot_guard);
    drop(manager);

    let delegation_runtime = DelegationRuntime::new(
        provider_config,
        executor_config,
        subagent_allowed_tools,
        subagent_allowed_skill_ids,
        cancel_token,
        Some(task_run_id.to_string()),
        Some(conversation_id.to_string()),
    );
    tools.register(Box::new(SubagentTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(SubagentBatchTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(JudgeSubagentResultsTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(ObserveSubagentBatchTool::from_runtime(
        delegation_runtime.clone(),
    )));
    for lifecycle_tool in SubagentLifecycleTool::all(delegation_runtime.clone()) {
        tools.register(Box::new(lifecycle_tool));
    }
    if let Some(terminal_state) = terminal_state {
        tools.register(Box::new(TerminalAgentTool::new(terminal_state)));
    }
    tools = tools.without_names(&["browser_session"]);
    tools.register(Box::new(NativeBrowserSessionTool::new(browser_state)));
    let before_package_filter_count = tools.tool_names().len();
    let last_known_good_tools = tools.clone();
    tools = match package_assembler.and_then(|assembler| assembler.assemble_tool_registry(tools)) {
        Ok(capabilities) => {
            let after_package_filter_count = capabilities.tools.tool_names().len();
            if before_package_filter_count != after_package_filter_count {
                info!(
                    "Package Runtime Assembler resolved tool registry from {before_package_filter_count} to {after_package_filter_count} tools"
                );
            }
            let missing_core_tools = missing_core_runtime_tools(&capabilities.tools);
            if missing_core_tools.is_empty() {
                info!(
                    "RegistryHealth status=healthy tool_count={after_package_filter_count} missing_core_tools=[]"
                );
                capabilities.tools
            } else {
                warn!(
                    "RegistryHealth status=degraded tool_count={after_package_filter_count} missing_core_tools={missing_core_tools:?}; retaining last-known-good registry"
                );
                last_known_good_tools.clone()
            }
        }
        Err(error) => {
            warn!(
                "Failed to filter tool registry through Package Host for task run {task_run_id}: {error}"
            );
            last_known_good_tools
        }
    };
    delegation_runtime.set_tool_registry(tools.clone());

    if plan_mode {
        let before_count = tools.tool_names().len();
        tools = tools.plan_mode_filtered();
        let after_count = tools.tool_names().len();
        info!(
            "Plan mode tool registry filtered from {before_count} to {after_count} read-only tools"
        );
        emit_agent_frontend_event(
            app_handle,
            event_seq,
            conversation_id,
            task_run_id,
            Some(turn_id),
            AgentEvent::Status {
                content: "Plan mode active: write, execution, MCP, automation, and delegation tools are disabled."
                    .to_string(),
                tone: Some("info".to_string()),
            },
        );
    }

    DesktopAgentSessionDependencies {
        tools,
        selected_skills,
        auto_loaded_skills,
        metrics: DesktopAgentDependencyMetrics {
            skill_select_ms,
            mcp_sync_ms,
            tool_registry_ms: elapsed_ms(tool_registry_started),
        },
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn build_desktop_approval_callback(input: DesktopApprovalCallbackInput) -> ApprovalCallback {
    let DesktopApprovalCallbackInput {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        event_seq,
        approval_runtime,
    } = input;
    let pending = approval_runtime.pending;
    let session_store = approval_runtime.session_store;
    let approval_mode = approval_runtime.approval_mode;

    Arc::new(move |req: ApprovalRequest| {
        let db = Arc::clone(&db);
        let handle = app_handle.clone();
        let pending = Arc::clone(&pending);
        let store = session_store.clone();
        let conv = conversation_id.clone();
        let event_seq = Arc::clone(&event_seq);
        let task_run_id = task_run_id.clone();
        let turn_id = turn_id.clone();
        Box::pin(async move {
            if let Some(decision) = approval_mode.short_circuit() {
                return decision;
            }

            let permission_key = ToolPermissionKey::from_request(&req);
            if let Ok(Some(policy)) = db.resolve_tool_permission_policy(&permission_key) {
                if policy == "never" {
                    return ApprovalDecision::Deny;
                }
            }

            if matches!(
                store.resolve(&permission_key),
                Some(ApprovalDecision::AllowSession)
            ) {
                return ApprovalDecision::AllowOnce;
            }

            let (tx, rx) = tokio::sync::oneshot::channel();
            pending.lock().await.insert(req.id.clone(), tx);
            emit_agent_frontend_event(
                &handle,
                event_seq.as_ref(),
                &conv,
                &task_run_id,
                Some(&turn_id),
                AgentEvent::ApprovalRequested {
                    request: req.clone(),
                },
            );

            let decision = match tokio::time::timeout(Duration::from_secs(60), rx).await {
                Ok(Ok(decision)) => decision,
                _ => {
                    pending.lock().await.remove(&req.id);
                    ApprovalDecision::Deny
                }
            };
            match decision {
                ApprovalDecision::AllowSession => {
                    store.set(&req.permission_key, ApprovalDecision::AllowSession);
                }
                ApprovalDecision::Never => {
                    let key = ToolPermissionKey::from_request(&req);
                    let _ = db.save_tool_permission_policy(&key, "never");
                }
                _ => {}
            }
            decision
        })
    })
}

pub async fn run_desktop_agent_turn(request: DesktopAgentTurnRequest) -> DesktopAgentTurnOutcome {
    let DesktopAgentTurnRequest {
        provider,
        dependencies,
        executor_config,
        cancel_token,
        steering_rx,
        approval_runtime,
        summarization_provider,
        history,
        user_parts,
        db,
        conversation_id,
        turn_id,
        assistant_sort_order,
        runtime,
        stream,
    } = request;

    let approval_cb = build_desktop_approval_callback(DesktopApprovalCallbackInput {
        db: Arc::clone(&db),
        app_handle: stream.app_handle.clone(),
        conversation_id: conversation_id.clone(),
        task_run_id: stream.task_run_id.clone(),
        turn_id: turn_id.clone(),
        event_seq: Arc::clone(&stream.event_seq),
        approval_runtime,
    });

    let executor_cancel_token = cancel_token.clone();
    let activity_runtime = nexa_core::activity::ActivityRuntime::with_database((*db).clone())
        .unwrap_or_else(|error| {
            warn!("Failed to initialize persistent Activity Runtime: {error}");
            nexa_core::activity::ActivityRuntime::new()
        });
    let mut executor = AgentExecutor::new(provider, dependencies.tools, executor_config)
        .with_activity_runtime(activity_runtime)
        .with_cancel_token(executor_cancel_token)
        .with_steering_receiver(steering_rx);
    executor = executor.with_approval_callback(approval_cb);
    if let Some(provider) = summarization_provider {
        executor = executor.with_summarization_provider(provider);
    }
    executor = executor
        .with_skills_override(dependencies.selected_skills)
        .with_auto_loaded_skills_override(dependencies.auto_loaded_skills);

    let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(64);
    let event_forwarder = AgentStreamForwarder::new(
        stream.app_handle.clone(),
        db.clone(),
        conversation_id.clone(),
        stream.task_run_id.clone(),
        turn_id.clone(),
        Arc::clone(&stream.event_seq),
        Arc::clone(&stream.terminal_emitted),
        stream.launch_started,
    )
    .run(events_rx);

    // Keep the forwarder structurally owned by the turn future. Aborting a
    // suspended outer task now drops both the executor and its event consumer;
    // no detached producer can race the resumed launch's event sequencer.
    let run_driver = async {
        let run_future = executor.run(
            history,
            user_parts,
            db.as_ref(),
            Some(&conversation_id),
            Some(&turn_id),
            events_tx,
            assistant_sort_order,
        );

        let mut run_future = Box::pin(run_future);
        let mut turn_timeout = (runtime.timeout_secs > 0).then(|| {
            Box::pin(tokio::time::sleep(Duration::from_secs(
                runtime.timeout_secs,
            )))
        });
        let mut keepalive =
            tokio::time::interval(Duration::from_secs(runtime.keepalive_interval_secs.max(1)));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;

        let (result, timed_out) = loop {
            tokio::select! {
                run_result = &mut run_future => break (Some(run_result), false),
                _ = async {
                    if let Some(timeout) = turn_timeout.as_mut() {
                        timeout.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => break (None, true),
                _ = keepalive.tick() => {
                    emit_agent_frontend_event(
                        &stream.app_handle,
                        stream.event_seq.as_ref(),
                        &conversation_id,
                        &stream.task_run_id,
                        Some(&turn_id),
                        AgentEvent::Thinking {
                            content: String::new(),
                        },
                    );
                }
            }
        };

        if timed_out {
            cancel_token.cancel();
        }

        drop(run_future);
        drop(turn_timeout);
        drop(executor);
        (result, timed_out)
    };

    let ((result, timed_out), ()) = tokio::join!(run_driver, event_forwarder);

    DesktopAgentTurnOutcome { result, timed_out }
}

pub fn finalize_desktop_agent_turn(finalization: DesktopAgentTurnFinalization<'_>) {
    let DesktopAgentTurnFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        outcome,
    } = finalization;

    let turn_snapshot = db.get_conversation_turn(turn_id).ok();
    let trace_artifacts = serde_json::json!({
        "turnId": turn_id,
        "turnStatus": turn_snapshot.as_ref().map(|turn| turn.status.clone()),
        "routeKind": turn_snapshot.as_ref().and_then(|turn| turn.route_kind.clone()),
        "trace": turn_snapshot.as_ref().and_then(|turn| turn.trace.clone()),
    });
    let previous_task_artifacts = db
        .get_agent_task_run(task_run_id)
        .ok()
        .and_then(|run| run.artifacts);
    let subtask_runs = db
        .list_agent_subtask_runs(task_run_id)
        .unwrap_or_else(|err| {
            warn!("Failed to load subtask runs for {task_run_id}: {err}");
            Vec::new()
        });
    let task_artifacts =
        build_final_task_artifacts(previous_task_artifacts, trace_artifacts, &subtask_runs);
    let verification_status = task_artifacts
        .get("verification")
        .and_then(|verification| verification.get("overallStatus"))
        .and_then(|status| status.as_str());
    let current_task_status = db
        .get_agent_task_run(task_run_id)
        .ok()
        .map(|run| run.status);
    if current_task_status.as_deref() == Some("awaiting_user_input")
        || matches!(
            &outcome.result,
            Some(Err(CoreError::AwaitingUserInput { .. }))
        )
    {
        if current_task_status.as_deref() == Some("awaiting_user_input") {
            let _ = db.update_agent_task_run_progress(
                task_run_id,
                Some("awaiting_user_input"),
                Some("awaiting_user_input"),
                None,
                Some("Waiting for user input"),
                None,
                Some(&task_artifacts),
            );
        }
        // A fast response can re-queue this same durable run before the
        // suspended executor reaches finalization. Never let that stale
        // executor overwrite the resumed or explicitly cancelled state.
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        return;
    }
    let (task_status, task_summary, task_error): (&str, &str, Option<String>) =
        if current_task_status.as_deref() == Some("paused") {
            ("paused", "Paused with a resumable checkpoint", None)
        } else if current_task_status.as_deref() == Some("cancelled") {
            ("cancelled", "Stopped by user", None)
        } else if outcome.timed_out {
            (
                "timed_out",
                "Agent execution timed out",
                Some("Agent execution timed out.".to_string()),
            )
        } else if let Some(Err(CoreError::Cancelled(message))) = &outcome.result {
            (
                "cancelled",
                "Agent execution cancelled",
                Some(message.clone()),
            )
        } else if let Some(Err(err)) = &outcome.result {
            ("failed", "Agent execution failed", Some(err.to_string()))
        } else {
            match turn_snapshot.as_ref().map(|turn| turn.status.as_str()) {
                Some("cancelled") => ("cancelled", "Stopped by user", None),
                Some("error") => ("failed", "Agent execution failed", None),
                Some("cached") => ("completed", "Answered from cache", None),
                _ if verification_status.is_some_and(|status| status != "passed") => {
                    ("completed", "Task completed with verification gap", None)
                }
                _ => ("completed", "Task completed", None),
            }
        };

    let _ = db.finish_agent_task_run(
        task_run_id,
        task_status,
        Some(task_summary),
        task_error.as_deref(),
        Some(&task_artifacts),
    );
    if let Some(run_id) = task_orchestrator_run_id {
        if let Err(err) =
            db.transition_workflow_automation_run(run_id, task_status, Some(task_summary))
        {
            warn!("Failed to transition Task Orchestrator run {run_id}: {err}");
        }
    }
    // The executor's Done/Error event is the canonical terminal event. Task
    // finalization updates the materialized snapshot only; appending a status
    // here would make a non-terminal event follow the terminal event.
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);

    if !matches!(&outcome.result, Some(Ok(_))) {
        repair_orphaned_tool_calls(db, conversation_id);
    }
}

pub fn finalize_desktop_agent_stop(finalization: DesktopAgentStopFinalization<'_>) {
    let DesktopAgentStopFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        event_seq,
        reason,
        summary,
    } = finalization;
    let artifacts = serde_json::json!({ "reason": reason });

    let _ = db.finish_agent_task_run(
        task_run_id,
        "cancelled",
        Some(summary),
        None,
        Some(&artifacts),
    );
    if let Some(run_id) = task_orchestrator_run_id {
        if let Err(err) = db.transition_workflow_automation_run(run_id, "cancelled", Some(summary))
        {
            warn!("Failed to cancel Task Orchestrator run {run_id}: {err}");
        }
    }
    let run_event = AgentRunEvent::terminal_status(
        task_run_id,
        Some(turn_id),
        event_seq.next(),
        summary,
        "cancelled",
        Some(&artifacts),
    );
    emit_agent_run_frontend_event(app_handle, conversation_id, &run_event);
    persist_durable_run_event(db, &run_event);
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
}

fn build_final_task_artifacts(
    previous_artifacts: Option<serde_json::Value>,
    trace_artifacts: serde_json::Value,
    subtask_runs: &[AgentSubtaskRun],
) -> serde_json::Value {
    let mut merged = match previous_artifacts {
        Some(serde_json::Value::Object(map)) => map,
        Some(previous) => {
            let mut map = serde_json::Map::new();
            map.insert("previous".to_string(), previous);
            map
        }
        None => serde_json::Map::new(),
    };
    merged.insert(
        "kind".to_string(),
        serde_json::Value::String("agentTaskArtifacts".to_string()),
    );
    merged.insert(
        "version".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    merged.insert("trace".to_string(), trace_artifacts);
    merged.insert(
        "subtasks".to_string(),
        serde_json::to_value(subtask_runs).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
    );
    serde_json::Value::Object(merged)
}

fn repair_orphaned_tool_calls(db: &Database, conversation_id: &str) {
    let msgs = match db.get_messages(conversation_id) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to load messages for orphan repair: {e}");
            return;
        }
    };

    let mut i = 0;
    while i < msgs.len() {
        if msgs[i].role == Role::Assistant && !msgs[i].tool_calls.is_empty() {
            let mut found_ids = std::collections::HashSet::new();
            let mut j = i + 1;
            while j < msgs.len() && msgs[j].role == Role::Tool {
                if let Some(ref tc_id) = msgs[j].tool_call_id {
                    found_ids.insert(tc_id.as_str());
                }
                j += 1;
            }

            let base_sort = if j > i + 1 {
                msgs[j - 1].sort_order
            } else {
                msgs[i].sort_order
            };

            let mut extra_sort = 1;
            for tc in &msgs[i].tool_calls {
                if !found_ids.contains(tc.id.as_str()) {
                    warn!(
                        "Inserting synthetic error response for orphaned tool_call {}",
                        tc.id
                    );
                    let synthetic = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.to_string(),
                        role: Role::Tool,
                        content: format!(
                            "Error: tool '{}' was interrupted before completing (agent timeout or cancellation).",
                            tc.name
                        ),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                        artifacts: None,
                        token_count: 20,
                        created_at: String::new(),
                        sort_order: base_sort + extra_sort,
                        thinking: None,
                        image_attachments: None,
                    };
                    if let Err(e) = db.add_message(&synthetic) {
                        warn!("Failed to insert synthetic tool response: {e}");
                    }
                    extra_sort += 1;
                }
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::app_settings::ShellAccessMode;
    use nexa_core::approval::ToolApprovalMode;
    use nexa_core::conversation::{CollectionContext, CreateConversationInput, ImageAttachment};
    use nexa_core::llm::ProviderType;
    use nexa_core::sources::CreateSourceInput;

    #[test]
    fn registry_health_requires_activity_runtime_core_tools() {
        assert!(missing_core_runtime_tools(&canonical_builtin_tool_registry()).is_empty());
        assert_eq!(
            missing_core_runtime_tools(&ToolRegistry::new()),
            REQUIRED_ACTIVITY_RUNTIME_TOOLS
        );
    }

    #[test]
    fn registry_snapshot_generation_changes_with_mcp_configuration() {
        let assembler = PackageRuntimeAssembler::from_host(&BuiltinPackageHost)
            .expect("built-in package snapshot");
        let mut server = nexa_core::mcp::McpServer {
            id: "mcp-1".to_string(),
            name: "Search".to_string(),
            transport: "streamable_http".to_string(),
            command: None,
            args: None,
            url: Some("https://mcp.example.test".to_string()),
            env_json: None,
            headers_json: Some(r#"{"Authorization":"Bearer first"}"#.to_string()),
            enabled: true,
            created_at: "2026-08-02T00:00:00Z".to_string(),
            updated_at: "2026-08-02T00:00:00Z".to_string(),
            builtin_id: None,
        };

        let first = desktop_tool_registry_generation(&assembler, &[server.clone()])
            .expect("first generation");
        server.headers_json = Some(r#"{"Authorization":"Bearer second"}"#.to_string());
        let second =
            desktop_tool_registry_generation(&assembler, &[server]).expect("second generation");

        assert_ne!(first, second);
    }

    fn test_agent_config() -> DbAgentConfig {
        DbAgentConfig {
            id: "agent-config-1".to_string(),
            name: "Primary".to_string(),
            provider: "open_ai".to_string(),
            api_key: "test-key".to_string(),
            base_url: None,
            model: "gpt-test".to_string(),
            temperature: Some(0.2),
            max_tokens: Some(1024),
            context_window: Some(128_000),
            is_default: true,
            reasoning_enabled: Some(true),
            thinking_budget: Some(4096),
            reasoning_effort: Some("medium".to_string()),
            max_iterations: Some(7),
            summarization_model: Some("gpt-summary".to_string()),
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: Some(2),
            subagent_max_calls_per_turn: Some(3),
            subagent_token_budget: Some(4096),
            delegation_limits_v2: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
            provider_endpoint_id: None,
            model_id: None,
            model_selection_resolution: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn test_provider_config(provider_type: ProviderType) -> ProviderConfig {
        ProviderConfig {
            provider_type,
            api_key: Some("test-key".to_string()),
            base_url: None,
            org_id: None,
            timeout_secs: None,
        }
    }

    fn successful_project_turn(db: &Database, conversation_id: &str) -> String {
        let user_message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role: Role::User,
            content: "Use project context".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 0,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user_message)
            .expect("add project user message");
        let turn = db
            .create_conversation_turn(conversation_id, &user_message.id, None)
            .expect("create project turn");
        db.finalize_conversation_turn(&turn.id, "success", None, None)
            .expect("finish project turn");
        turn.id
    }

    #[test]
    fn local_provider_detection_requires_an_exact_loopback_host() {
        let mut config = test_provider_config(ProviderType::Ollama);
        config.base_url = Some("http://127.example.com:11434".to_string());
        assert!(!provider_config_is_local(&config));

        config.base_url = Some("http://127.0.0.1:11434".to_string());
        assert!(provider_config_is_local(&config));

        config.base_url = Some("http://[::1]:11434".to_string());
        assert!(provider_config_is_local(&config));

        config.base_url = Some("https://remote-ollama.example.com".to_string());
        assert!(!provider_config_is_local(&config));
    }

    #[test]
    fn desktop_agent_user_content_parts_project_attachments() {
        let db = Database::open_memory().expect("open memory db");
        let mut db_config = test_agent_config();
        db_config.model = "gpt-4o".to_string();
        let attachments = vec![
            ImageAttachment {
                base64_data: "image-data".to_string(),
                media_type: "image/png".to_string(),
                original_name: "diagram.png".to_string(),
                attachment_id: None,
                attachment_hash: None,
                vision_analysis: None,
            },
            ImageAttachment {
                base64_data: base64::engine::general_purpose::STANDARD
                    .encode("hello from attachment. ".repeat(8)),
                media_type: "text/plain".to_string(),
                original_name: "notes.txt".to_string(),
                attachment_id: None,
                attachment_hash: None,
                vision_analysis: None,
            },
        ];

        let parts = build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
            db: &db,
            app_handle: None,
            provider_config: &test_provider_config(ProviderType::OpenAi),
            db_config: &db_config,
            message: "Read these",
            attachments: Some(&attachments),
        })
        .expect("build user content parts");

        assert_eq!(parts.len(), 3);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "Read these"),
            other => panic!("unexpected first part: {other:?}"),
        }
        match &parts[1] {
            ContentPart::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "image-data");
            }
            other => panic!("unexpected image part: {other:?}"),
        }
        match &parts[2] {
            ContentPart::Text { text } => {
                assert!(text.contains("[Attached file: notes.txt]"));
                assert!(text.contains("hello from attachment"));
            }
            other => panic!("unexpected attachment part: {other:?}"),
        }
    }

    #[test]
    fn desktop_agent_user_content_parts_return_decode_errors() {
        let db = Database::open_memory().expect("open memory db");
        let attachment = ImageAttachment {
            base64_data: "%not-base64".to_string(),
            media_type: "text/plain".to_string(),
            original_name: "broken.txt".to_string(),
            attachment_id: None,
            attachment_hash: None,
            vision_analysis: None,
        };

        let err = build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
            db: &db,
            app_handle: None,
            provider_config: &test_provider_config(ProviderType::OpenAi),
            db_config: &test_agent_config(),
            message: "Read this",
            attachments: Some(&[attachment]),
        })
        .expect_err("invalid base64 should fail");

        assert!(err.contains("Failed to decode attachment"));
    }

    #[test]
    fn desktop_summarization_provider_config_requires_provider_override() {
        let db = Database::open_memory().expect("open memory db");
        let db_config = test_agent_config();

        assert!(
            resolve_desktop_summarization_provider_config(&db, &db_config)
                .expect("resolve no override")
                .is_none()
        );

        let mut with_provider = db_config;
        with_provider.provider = "openai".to_string();
        with_provider.summarization_provider = Some("open_ai".to_string());
        with_provider.base_url = Some("https://example.test/v1".to_string());

        let (config, name, model) =
            resolve_desktop_summarization_provider_config(&db, &with_provider)
                .expect("resolve provider override")
                .expect("provider override");

        assert_eq!(config.provider_type, ProviderType::OpenAi);
        assert_eq!(config.api_key.as_deref(), Some("test-key"));
        assert_eq!(config.base_url.as_deref(), Some("https://example.test/v1"));
        assert_eq!(config.timeout_secs, None);
        assert_eq!(name, "Primary");
        assert_eq!(model, "gpt-summary");
    }

    #[test]
    fn desktop_summarization_provider_config_rejects_cross_provider_credential_reuse() {
        let db = Database::open_memory().expect("open memory db");
        let mut db_config = test_agent_config();
        db_config.summarization_provider = Some("deepseek".to_string());

        let error = resolve_desktop_summarization_provider_config(&db, &db_config)
            .expect_err("cross-provider summary needs independent saved credentials");

        assert!(error.contains("own saved provider configuration"));
        assert!(error.contains("never reused across providers"));
    }

    #[test]
    fn desktop_memory_extraction_provider_config_uses_summary_overrides() {
        let mut db_config = test_agent_config();
        db_config.provider = "ollama".to_string();
        db_config.model = "llama3".to_string();
        db_config.summarization_model = Some("gpt-summary".to_string());

        let fallback = desktop_memory_extraction_provider_config(&db_config);
        assert_eq!(desktop_memory_extraction_model(&db_config), "gpt-summary");
        assert_eq!(fallback.provider_type, ProviderType::Ollama);

        db_config.summarization_provider = Some("open_ai".to_string());
        let override_config = desktop_memory_extraction_provider_config(&db_config);

        assert_eq!(override_config.provider_type, ProviderType::Ollama);
        assert_eq!(override_config.api_key.as_deref(), Some("test-key"));

        db_config.base_url = Some("https://api.deepseek.com/v1".to_string());
        let sniffed_config = desktop_memory_extraction_provider_config(&db_config);

        assert_eq!(sniffed_config.provider_type, ProviderType::DeepSeek);
    }

    #[test]
    fn project_workspace_instructions_are_live_and_episodes_are_evidence() {
        let db = Database::open_memory().expect("open memory db");
        let project = db
            .create_project(&CreateProjectInput {
                name: "Workspace".to_string(),
                description: Some("Ship an auditable runtime".to_string()),
                icon: None,
                color: None,
                system_prompt: Some("Follow workspace instruction v1".to_string()),
                source_scope: None,
            })
            .expect("create project");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: Some("Conversation-specific instruction".to_string()),
                collection_context: None,
                project_id: Some(project.id.clone()),
                persona_id: None,
            })
            .expect("create conversation");
        let legacy_snapshot = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: Some("Follow workspace instruction v1".to_string()),
                collection_context: None,
                project_id: Some(project.id.clone()),
                persona_id: None,
            })
            .expect("create legacy project conversation");
        db.mark_legacy_project_system_prompt_ambiguous(&legacy_snapshot.id)
            .expect("mark copied legacy project prompt");
        let observed_turn_id = successful_project_turn(&db, &conversation.id);
        db.record_project_turn_completion(
            &conversation.id,
            &observed_turn_id,
            "run-observed",
            "Visible prior result with provenance",
        )
        .expect("record project episode");

        let db_config = test_agent_config();
        let app_cfg = AppConfig::default();
        let request = |conversation: &Conversation| DesktopAgentTurnConfigRequest {
            db: &db,
            conversation,
            turn_id: "turn-current",
            message: "Use the prior result",
            persona_id: None,
            explicit_skill_ids: &[],
            db_config: &db_config,
            app_cfg: &app_cfg,
            execution_mode: AgentExecutionMode::Normal,
            power_mode: AgentPowerMode::Standard,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Balanced,
            custom_orchestration: None,
        };
        let initial = build_desktop_agent_turn_config(request(&conversation)).executor_config;
        assert!(initial
            .system_prompt
            .contains("Follow workspace instruction v1"));
        let volatile = initial.volatile_system_sections.join("\n");
        assert!(volatile.contains("Visible prior result with provenance"));
        assert!(volatile.contains(&format!("turn:{observed_turn_id}")));
        assert!(!initial
            .system_prompt
            .contains("Visible prior result with provenance"));

        db.update_project(
            &project.id,
            &UpdateProjectInput {
                name: None,
                description: None,
                icon: None,
                color: None,
                system_prompt: Some("Follow workspace instruction v2".to_string()),
                source_scope: None,
                archived: None,
            },
        )
        .expect("update live project instruction");
        let refreshed_conversation = db
            .get_conversation(&conversation.id)
            .expect("reload conversation");
        let refreshed =
            build_desktop_agent_turn_config(request(&refreshed_conversation)).executor_config;
        assert!(refreshed
            .system_prompt
            .contains("Follow workspace instruction v2"));
        assert!(!refreshed
            .system_prompt
            .contains("Follow workspace instruction v1"));
        assert!(refreshed
            .system_prompt
            .contains("Conversation-specific instruction"));

        let legacy_after_project_edit = db
            .get_conversation(&legacy_snapshot.id)
            .expect("reload legacy project conversation");
        let migrated =
            build_desktop_agent_turn_config(request(&legacy_after_project_edit)).executor_config;
        assert!(migrated
            .system_prompt
            .contains("Follow workspace instruction v2"));
        assert!(!migrated
            .system_prompt
            .contains("Follow workspace instruction v1"));

        db.update_conversation_system_prompt(&legacy_snapshot.id, "Explicit conversation override")
            .expect("save explicit conversation prompt");
        let explicit = db
            .get_conversation(&legacy_snapshot.id)
            .expect("reload explicit prompt");
        let explicit_config = build_desktop_agent_turn_config(request(&explicit)).executor_config;
        assert!(explicit_config
            .system_prompt
            .contains("Explicit conversation override"));
        assert!(explicit_config
            .system_prompt
            .contains("Follow workspace instruction v2"));
    }

    #[test]
    fn desktop_agent_turn_config_projects_prompt_and_executor_fields() {
        let db = Database::open_memory().expect("open memory db");
        let root = std::env::temp_dir().join(format!("nexa-turn-config-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create source root");
        let source = db
            .add_source(CreateSourceInput {
                root_path: root.to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: false,
            })
            .expect("add source");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: Some("System base".to_string()),
                collection_context: Some(CollectionContext {
                    title: "Research Set".to_string(),
                    description: Some("Scoped notes".to_string()),
                    query_text: Some("agent runtime".to_string()),
                    source_ids: vec![source.id.clone()],
                }),
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        db.set_conversation_sources(&conversation.id, &[source.id.clone()])
            .expect("set source scope");

        let mut app_cfg = AppConfig::default();
        app_cfg.tool_approval_mode = ToolApprovalMode::DenyAll;
        app_cfg.shell_access_mode = ShellAccessMode::Open;
        app_cfg.cache_ttl_hours = 9;
        app_cfg.dynamic_tool_visibility = false;
        app_cfg.trace_enabled = false;
        app_cfg.confirm_destructive = true;

        let explicit_skill_ids = vec![
            "builtin-evidence-first".to_string(),
            "explicit-skill".to_string(),
        ];
        let turn_config = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
            db: &db,
            conversation: &conversation,
            turn_id: "turn-1",
            message: "Summarize runtime evidence",
            persona_id: None,
            explicit_skill_ids: &explicit_skill_ids,
            db_config: &test_agent_config(),
            app_cfg: &app_cfg,
            execution_mode: AgentExecutionMode::Plan,
            power_mode: AgentPowerMode::Standard,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Balanced,
            custom_orchestration: None,
        });

        assert_eq!(turn_config.source_scope_ids, vec![source.id.clone()]);
        assert!(turn_config
            .pinned_skill_ids
            .contains(&"builtin-evidence-first".to_string()));
        assert!(turn_config
            .pinned_skill_ids
            .contains(&"builtin-visual-explanations".to_string()));
        assert!(turn_config
            .pinned_skill_ids
            .contains(&"explicit-skill".to_string()));
        assert_eq!(
            turn_config
                .pinned_skill_ids
                .iter()
                .filter(|id| id.as_str() == "builtin-evidence-first")
                .count(),
            1
        );

        let executor = turn_config.executor_config;
        assert_eq!(executor.max_iterations, 7);
        assert_eq!(executor.model.as_deref(), Some("gpt-test"));
        assert_eq!(executor.temperature, Some(0.2));
        assert_eq!(executor.max_tokens, Some(1024));
        assert_eq!(executor.context_window, Some(128_000));
        assert_eq!(executor.reasoning_enabled, Some(true));
        assert_eq!(executor.thinking_budget, Some(4096));
        assert_eq!(executor.provider_type, Some(ProviderType::OpenAi));
        assert_eq!(executor.summarization_model.as_deref(), Some("gpt-summary"));
        assert_eq!(executor.subagent_max_parallel, Some(2));
        assert_eq!(executor.subagent_max_calls_per_turn, Some(3));
        assert_eq!(executor.subagent_token_budget, Some(4096));
        assert_eq!(executor.subagent_verification_reserve_percent, None);
        assert_eq!(executor.cache_ttl_hours, Some(9));
        assert!(!executor.dynamic_tool_visibility);
        assert!(!executor.trace_enabled);
        assert!(executor.require_tool_confirmation);
        assert_eq!(executor.shell_access_mode, ShellAccessMode::Open);
        assert_eq!(executor.tool_approval_mode, ToolApprovalMode::DenyAll);
        assert_eq!(executor.execution_mode, AgentExecutionMode::Plan);
        assert_eq!(executor.power_mode, AgentPowerMode::Standard);

        let prompt_sections = executor.volatile_system_sections.join("\n");
        assert!(prompt_sections.contains("## Current Turn Time"));
        assert!(prompt_sections.contains("## Active Source Scope"));
        assert!(prompt_sections.contains(root.to_string_lossy().as_ref()));
        assert!(prompt_sections.contains("Research Set"));
        assert!(prompt_sections.contains("Plan Mode"));

        let mut nexus_db_config = test_agent_config();
        nexus_db_config.model = "gpt-5.6".to_string();
        nexus_db_config.delegation_limits_v2 = Some(nexa_core::agent::DelegationLimitsConfig {
            total_actual_tokens_soft_limit: Some(32_000),
            max_parallel: Some(3),
            max_calls_per_turn: Some(6),
            queue_deadline_ms: Some(9_000),
            ..Default::default()
        });
        let nexus = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
            db: &db,
            conversation: &conversation,
            turn_id: "turn-2",
            message: "Verify a difficult cross-module change",
            persona_id: None,
            explicit_skill_ids: &[],
            db_config: &nexus_db_config,
            app_cfg: &app_cfg,
            execution_mode: AgentExecutionMode::Normal,
            power_mode: AgentPowerMode::Nexus,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Balanced,
            custom_orchestration: None,
        })
        .executor_config;
        assert_eq!(nexus.max_iterations, 48);
        assert_eq!(nexus.reasoning_effort, Some(ReasoningEffort::Max));
        assert_eq!(nexus.subagent_max_parallel, Some(6));
        assert_eq!(nexus.subagent_max_calls_per_turn, Some(12));
        assert_eq!(nexus.subagent_token_budget, Some(96_000));
        assert_eq!(nexus.subagent_verification_reserve_percent, Some(25));
        let nexus_limits = nexus
            .delegation_limits_v2
            .as_ref()
            .expect("saved V2 limits remain available");
        assert_eq!(nexus_limits.max_parallel, Some(6));
        assert_eq!(nexus_limits.max_calls_per_turn, Some(12));
        assert_eq!(nexus_limits.total_actual_tokens_soft_limit, Some(96_000));
        assert_eq!(nexus_limits.queue_deadline_ms, Some(9_000));
        assert_eq!(nexus.power_mode, AgentPowerMode::Nexus);
        assert!(nexus
            .volatile_system_sections
            .join("\n")
            .contains("## Nexus Execution Policy"));

        let mut unlimited_db_config = test_agent_config();
        unlimited_db_config.max_iterations = None;
        unlimited_db_config.delegation_limits_v2 = Some(nexa_core::agent::DelegationLimitsConfig {
            total_actual_tokens_soft_limit: Some(32_000),
            max_parallel: Some(3),
            max_calls_per_turn: Some(6),
            run_deadline_ms: Some(240_000),
            ..Default::default()
        });
        let custom = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
            db: &db,
            conversation: &conversation,
            turn_id: "turn-3",
            message: "Apply a bounded custom orchestration policy",
            persona_id: None,
            explicit_skill_ids: &[],
            db_config: &unlimited_db_config,
            app_cfg: &app_cfg,
            execution_mode: AgentExecutionMode::Normal,
            power_mode: AgentPowerMode::Standard,
            collaboration_mode: AgentCollaborationMode::Direct,
            moa_preset: MoaPresetId::FastReview,
            orchestration_profile: OrchestrationProfile::Custom,
            custom_orchestration: Some(CustomOrchestrationOptions {
                max_iterations: Some(48),
                max_parallel: Some(7),
                max_calls_per_turn: Some(15),
                delegated_token_budget: Some(77_000),
                ..Default::default()
            }),
        })
        .executor_config;
        assert_eq!(custom.max_iterations, 48);
        assert_eq!(custom.subagent_max_parallel, Some(7));
        assert_eq!(custom.subagent_max_calls_per_turn, Some(15));
        assert_eq!(custom.subagent_token_budget, Some(77_000));
        let custom_limits = custom
            .delegation_limits_v2
            .expect("custom turn merges into saved V2 limits");
        assert_eq!(custom_limits.max_parallel, Some(7));
        assert_eq!(custom_limits.max_calls_per_turn, Some(15));
        assert_eq!(custom_limits.total_actual_tokens_soft_limit, Some(77_000));
        assert_eq!(custom_limits.run_deadline_ms, Some(240_000));

        let _ = std::fs::remove_dir_all(root);
    }
}
