use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

#[cfg(test)]
use crate::agent_stream::{
    compact_agent_event_for_frontend, split_text_by_utf8_bytes, MAX_FRONTEND_ARTIFACT_STRING_CHARS,
    MAX_FRONTEND_TOOL_CONTENT_CHARS,
};
use crate::agent_task_events::{
    emit_agent_task_run_update, record_internal_agent_run_status_event,
};
use crate::app_events::emit_app_event;
use crate::background_work_governor::{
    BackgroundWorkGovernor, BackgroundWorkPermit, BackgroundWorkReceiver, SourceChangeJob,
};
use nexa_core::agent::power_mode::AgentPowerMode;
#[cfg(test)]
use nexa_core::agent::AgentEvent;
use nexa_core::agent::{AgentExecutionMode, AgentSteeringMessage};
use nexa_core::agent_run::{AgentRunEvent, AgentRunPhase};
#[cfg(test)]
use nexa_core::app_settings::ShellAccessMode;
use nexa_core::app_settings::{AppConfig, TextToSpeechConfig, WizardState};
#[cfg(test)]
use nexa_core::approval::ToolApprovalMode;
use nexa_core::approval::{ApprovalDecision, SessionApprovalStore, ToolPermissionKey};
use nexa_core::capability_registry::{
    CapabilityRegistryProjection, RegistryActivationRecord, RegistryReadMode, RegistryScope,
};
use nexa_core::companion::CompanionProjection;
use nexa_core::conversation::memory::estimate_tokens;
use nexa_core::conversation::{
    conversation_message_llm_context_content, conversation_message_provider_turn,
    conversation_message_reasoning_replay, validate_agent_config_credential_contract,
    AgentConfig as DbAgentConfig, AgentExecutionGraph, AgentSubtaskRun, AgentTaskArtifact,
    AgentTaskArtifactSummary, AgentTaskArtifactVersion, AgentTaskRun, AgentTaskRunEvent,
    AgentTaskRunListItem, AgentTaskRunPageCursor, AgentTaskRunSummaryPage, CheckpointBranch,
    CollectionContext, Conversation, ConversationMessage, ConversationStats, ConversationTurn,
    CreateAgentTaskArtifactInput, CreateConversationInput, ImageAttachment, SaveAgentConfigInput,
    UpdateAgentTaskArtifactInput,
};
use nexa_core::db::Database;
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::embed::{EmbedderConfig, LocalEmbeddingModel};
use nexa_core::error::CoreError;
use nexa_core::event_claim_graph::{
    CreateKnowledgeClaimInput, KnowledgeClaim, NarrativeEvidencePlan,
};
use nexa_core::evolution::{
    AgentProceduralMemory, AppliedSkillChange, SkillChangeProposal, SkillProposalStatus,
};
use nexa_core::feedback::{Feedback, FeedbackAction};
use nexa_core::index::IndexStats;
use nexa_core::ingest::{self, EmbedResult, IngestResult};
use nexa_core::interaction::{
    InteractionAnswers, InteractionRequest, InteractionResponse, InteractionStatus,
    SubmitInteractionResponse,
};
use nexa_core::llm::{
    create_provider, CompletionRequest, Message, ProviderConfig, ProviderType, Role,
};
use nexa_core::mcp::{McpServer, McpToolInfo, SaveMcpServerInput};
use nexa_core::persona::{PersonaProfile, SavePersonaInput};

use base64::Engine;
use chrono::{SecondsFormat, Utc};
use log::{info, warn};
use nexa_core::models::{
    EvidenceCard, Playbook, PlaybookCitation, SearchFilters, SearchQuery, Source,
};
use nexa_core::playbook::QueryLog;
use nexa_core::privacy::PrivacyConfig;
use nexa_core::project::{CreateProjectInput, Project, UpdateProjectInput};
use nexa_core::project_memory::{
    CreateProjectMemoryInput, ProjectMemory, UpdateProjectMemoryInput,
};
use nexa_core::project_runtime::ProjectWorkspaceSnapshot;
use nexa_core::provider_catalog::{
    build_effective_model_catalog, load_provider_presets, ProviderModelCatalogSnapshot,
    ProviderPreset,
};
use nexa_core::provider_registry::provider_type_for_parts;
use nexa_core::run_event_outbox::AgentRunEventOutboxes;
use nexa_core::runtime::AgentRunEventOutbox;
use nexa_core::search::{self, SearchResult};
use nexa_core::settings_schema_v2::{
    CapabilityBindingV2, SettingsMigrationReportV2, SettingsProfileV2, SettingsSchemaStateV2,
    SettingsScopeV2,
};
use nexa_core::skills::{DiscoveredSkillBundle, SaveSkillInput, Skill};
use nexa_core::source_tree::SourceTree;
use nexa_core::sources::{CreateSourceInput, UpdateSourceInput};
use nexa_core::tts_provider_catalog::{
    build_tts_voice_catalog, discover_tts_voices, supports_dynamic_tts_voice_catalog,
    TtsVoiceCatalogSnapshot,
};
use nexa_core::watcher::{FileWatcher, WatcherEventKind};
use nexa_core::workflow_catalog::{workflow_catalog, WorkflowCatalogTemplate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

mod agent_chat;
mod app_config;
mod approval;
mod companion;
mod conversation;
mod knowledge;
mod media;
mod media_generation;
mod personas;
mod preview;
mod realtime_transcription;
mod skills_mcp;
mod sources;
mod terminal;
mod update;
mod watcher;
mod workflows;

pub use agent_chat::*;
pub use app_config::*;
pub use approval::*;
pub use companion::*;
pub use conversation::*;
pub use knowledge::*;
pub use media::*;
pub use media_generation::*;
pub use personas::*;
pub use preview::*;
pub use realtime_transcription::*;
pub use skills_mcp::*;
pub use sources::*;
pub use terminal::*;
pub use update::*;
pub use watcher::*;
pub use workflows::*;

const DEFAULT_MCP_CALL_TIMEOUT_SECS: u64 = 300;

/// Application state holding the database connection.
pub struct AppState {
    pub db: Arc<Database>,
    pub db_executor: DatabaseExecutor,
    pub run_event_outboxes: AgentRunEventOutboxes,
    pub subagent_lifecycle: crate::subagent_lifecycle::SubagentLifecycleRuntime,
    pub context_compaction: nexa_core::context_maintenance::ContextCompactionService,
    pub media_generation: nexa_core::media_generation::MediaGenerationRuntime,
    /// Guard: true while whisper transcription is in progress.
    #[cfg(feature = "video")]
    pub whisper_busy: Arc<AtomicBool>,
    /// Native, bounded microphone audio spool. Renderer callers only receive
    /// opaque session IDs; filesystem paths remain inside Rust.
    #[cfg(feature = "video")]
    pub voice_audio_spool: Arc<nexa_core::voice_audio_spool::VoiceAudioSpool>,
    /// Rejects excess raw append work before cloning or entering Tokio's
    /// blocking pool, preventing direct IPC callers from bypassing JS bounds.
    #[cfg(feature = "video")]
    pub voice_spool_append_permits: Arc<tokio::sync::Semaphore>,
    /// Lock to serialize scan operations and prevent duplicate document inserts.
    pub scan_lock: Arc<Mutex<()>>,
    /// Coordinates resource-intensive background work with foreground turns.
    pub background_work: BackgroundWorkGovernor,
}

pub struct AgentState {
    pub sessions: nexa_core::runtime::AgentSessionManager,
}

struct TerminalAgentError<'a> {
    conversation_id: &'a str,
    task_run_id: &'a str,
    turn_id: &'a str,
    message: &'a str,
    status: &'a str,
    payload: Option<&'a serde_json::Value>,
}

fn submit_terminal_agent_error(
    stream_event_seq: &AgentRunEventOutbox,
    error: TerminalAgentError<'_>,
) {
    let run_event = AgentRunEvent::terminal_error(
        error.task_run_id,
        Some(error.turn_id),
        0,
        error.message,
        error.status,
        error.payload,
    );
    if let Err(submit_error) = stream_event_seq.submit(run_event) {
        warn!(
            "Failed to submit terminal RunEvent for {}: {submit_error}",
            error.conversation_id
        );
    }
}

/// State for the MCP server manager.
pub struct McpManagerState {
    pub manager: Arc<TokioMutex<nexa_core::mcp::McpManager>>,
}

/// State for tracking active model download cancellation.
pub struct DownloadCancelFlag(pub Arc<AtomicBool>);

/// State for the per-call tool approval flow.
///
/// `pending` maps an approval-request id → a oneshot `Sender` that the
/// Tauri `approve_tool_call_cmd` resolves once the user clicks a button
/// in the GUI. `session_store` holds "allow for this session" grants that
/// persist until the app is closed.
#[derive(Default)]
pub struct ApprovalState {
    pub pending: PendingToolApprovals,
    pub session_store: SessionApprovalStore,
}

/// A pending desktop approval remains associated with its owning durable run.
/// Stop/pause paths use that ownership to resolve the prompt before committing
/// a resumable checkpoint.
pub struct PendingToolApproval {
    pub task_run_id: String,
    pub sender: tokio::sync::oneshot::Sender<ApprovalDecision>,
}

pub type PendingToolApprovals = Arc<TokioMutex<HashMap<String, PendingToolApproval>>>;

#[tauri::command]
pub fn list_workflow_templates_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WorkflowCatalogTemplate>, String> {
    filter_desktop_workflow_templates_by_package_host(state.db.as_ref(), workflow_catalog())
}

pub(crate) fn filter_desktop_workflow_templates_by_package_host(
    db: &Database,
    templates: Vec<WorkflowCatalogTemplate>,
) -> Result<Vec<WorkflowCatalogTemplate>, String> {
    let snapshot = conversation::desktop_package_host_snapshot(db)?;
    let visible_workflow_ids = snapshot
        .runtime_components()
        .into_iter()
        .filter(|component| component.kind == nexa_core::package_host::PackageSurfaceKind::Workflow)
        .map(|component| component.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    Ok(templates
        .into_iter()
        .filter(|template| visible_workflow_ids.contains(template.id.as_str()))
        .collect())
}

async fn sync_enabled_mcp_servers(
    db: &Database,
    manager: &mut nexa_core::mcp::McpManager,
) -> Result<HashMap<String, String>, String> {
    let enabled_servers = db.get_enabled_mcp_servers().map_err(|e| e.to_string())?;
    Ok(manager
        .sync_servers(&enabled_servers, Some(DEFAULT_MCP_CALL_TIMEOUT_SECS))
        .await)
}

/// State for the file watcher.
pub struct WatcherState {
    pub watcher: Mutex<FileWatcher>,
    /// Map of source_id → root_path for actively watched sources.
    pub watched: Mutex<HashMap<String, String>>,
    /// Incremented after every foreground database mutation so background
    /// startup registration cannot apply a stale database snapshot.
    pub revision: AtomicU64,
}

/// Info about a watched source, returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchedSourceInfo {
    pub source_id: String,
    pub root_path: String,
}

/// Validates that `path` is within a registered source directory.
/// Returns the canonicalized path on success.
#[cfg(feature = "video")]
fn validate_path_in_scope(db: &Database, path: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path).map_err(|e| format!("Invalid path: {e}"))?;
    let sources = db.list_sources().map_err(|e| format!("DB error: {e}"))?;
    let in_scope = sources.iter().any(|s| {
        if let Ok(source_canonical) = std::fs::canonicalize(&s.root_path) {
            canonical.starts_with(&source_canonical)
        } else {
            false
        }
    });
    if !in_scope {
        return Err("File is not within a registered source directory".into());
    }
    Ok(canonical)
}

/// Progress for batch operations spanning multiple sources.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    pub operation: String,
    pub source_index: usize,
    pub source_count: usize,
    pub source_id: String,
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub current_file: Option<String>,
}

/// Progress for FTS index operations.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsProgress {
    pub operation: String,
    pub phase: String,
}

fn task_title_from_message(message: &str) -> String {
    let single_line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= 80 {
        return single_line;
    }
    let mut out = single_line.chars().take(77).collect::<String>();
    out.push_str("...");
    out
}

/// Initialise the file watcher and spawn a background thread that registers
/// sources and processes file-change events. Recursive registration can be
/// expensive on large trees, so it must never block Tauri's setup callback.
pub fn init_watcher(
    app_handle: tauri::AppHandle,
    background_work_receiver: BackgroundWorkReceiver,
) {
    let (file_watcher, rx) = match FileWatcher::new() {
        Ok(pair) => pair,
        Err(e) => {
            warn!("Failed to initialise file watcher: {e}");
            return;
        }
    };

    let watcher_state = WatcherState {
        watcher: Mutex::new(file_watcher),
        watched: Mutex::new(HashMap::new()),
        revision: AtomicU64::new(0),
    };
    app_handle.manage(watcher_state);

    let worker_handle = app_handle.clone();
    thread::spawn(move || {
        while let Some(permit) = background_work_receiver.recv() {
            process_source_change_job(&worker_handle, permit);
        }
    });

    // Clone what we need for the background thread.
    let handle = app_handle.clone();

    thread::spawn(move || {
        if let Some(app_state) = handle.try_state::<AppState>() {
            match app_state.db.list_sources() {
                Ok(sources) => {
                    if let Some(state) = handle.try_state::<WatcherState>() {
                        for snapshot in sources.into_iter().filter(|source| source.watch_enabled) {
                            loop {
                                let revision = state.revision.load(Ordering::Acquire);
                                let current = match app_state.db.get_source(&snapshot.id) {
                                    Ok(source) => source,
                                    Err(error) => {
                                        warn!(
                                            "Failed to refresh watched source {}: {error}",
                                            snapshot.id
                                        );
                                        break;
                                    }
                                };
                                if !current.watch_enabled || !Path::new(&current.root_path).exists()
                                {
                                    break;
                                }
                                let mut watcher = state.watcher.lock().unwrap();
                                let mut watched = state.watched.lock().unwrap();
                                if state.revision.load(Ordering::Acquire) != revision {
                                    drop(watched);
                                    drop(watcher);
                                    continue;
                                }
                                if watched.get(&current.id) == Some(&current.root_path) {
                                    break;
                                }
                                if let Err(e) = watcher.watch(Path::new(&current.root_path)) {
                                    warn!("Failed to watch {}: {e}", current.root_path);
                                } else {
                                    watched.insert(current.id, current.root_path);
                                }
                                break;
                            }
                        }
                    }
                }
                Err(e) => warn!("Failed to list watched sources: {e}"),
            }
        }

        // Debounce: collect events for 2 seconds before acting.
        let debounce = Duration::from_secs(2);
        // source_id → (last_event_time, changed_paths, removed_paths)
        let mut pending: HashMap<String, (Instant, HashSet<PathBuf>, HashSet<PathBuf>)> =
            HashMap::new();

        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(event) => {
                    // Find which watched source owns this path.
                    let ws = match handle.try_state::<WatcherState>() {
                        Some(s) => s,
                        None => continue,
                    };
                    let watched = ws.watched.lock().unwrap();
                    let matched: Option<&String> = watched
                        .iter()
                        .find(|(_, root)| event.path.starts_with(root.as_str()))
                        .map(|(sid, _)| sid);
                    if let Some(sid) = matched {
                        let sid = sid.clone();
                        drop(watched);
                        let entry = pending
                            .entry(sid)
                            .or_insert_with(|| (Instant::now(), HashSet::new(), HashSet::new()));
                        entry.0 = Instant::now();
                        record_debounced_watcher_path(
                            &mut entry.1,
                            &mut entry.2,
                            event.path,
                            event.kind,
                        );
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check if any pending source has been quiet for `debounce`.
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Watcher channel disconnected, stopping watcher thread");
                    break;
                }
            }

            // Process debounced sources.
            let now = Instant::now();
            let ready: Vec<String> = pending
                .iter()
                .filter(|(_, (ts, _, _))| now.duration_since(*ts) >= debounce)
                .map(|(sid, _)| sid.clone())
                .collect();

            for source_id in ready {
                let (_ts, changed_paths, removed_paths) = pending.remove(&source_id).unwrap();
                let app_state = match handle.try_state::<AppState>() {
                    Some(s) => s,
                    None => continue,
                };
                app_state.background_work.submit_source_changes(
                    source_id,
                    changed_paths,
                    removed_paths,
                );
            }
        }
    });
}

fn record_debounced_watcher_path(
    changed_paths: &mut HashSet<PathBuf>,
    removed_paths: &mut HashSet<PathBuf>,
    path: PathBuf,
    kind: WatcherEventKind,
) {
    if kind == WatcherEventKind::Removed {
        changed_paths.remove(&path);
        removed_paths.insert(path);
    } else {
        // Atomic saves commonly emit Removed followed by Created/Modified for
        // the same path. Preserve only the latest observed state so the
        // recreated file is ingested instead of being deleted from search.
        removed_paths.remove(&path);
        changed_paths.insert(path);
    }
}

fn process_source_change_job(app_handle: &tauri::AppHandle, permit: BackgroundWorkPermit) {
    let app_state = match app_handle.try_state::<AppState>() {
        Some(state) => state,
        None => return,
    };
    let job = permit.job().clone();
    let _scan_guard = app_state
        .scan_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    for removed in &job.removed_paths {
        if let Err(error) = permit.wait_for_foreground() {
            warn!("Background source cleanup stopped: {error}");
            return;
        }
        let path_str = removed.to_string_lossy();
        match app_state.db.delete_document_by_path(&path_str) {
            Ok(true) => info!("Removed document for deleted file: {path_str}"),
            Ok(false) => {}
            Err(error) => warn!("Failed to remove document for {path_str}: {error}"),
        }
    }

    let mut files_added = 0usize;
    let mut files_updated = 0usize;
    for path in &job.changed_paths {
        if let Err(error) = permit.wait_for_foreground() {
            warn!("Background source ingestion stopped: {error}");
            return;
        }
        match ingest::ingest_single_file(&app_state.db, &job.source_id, path) {
            Ok(ingest::IngestFileResult::Added) => files_added += 1,
            Ok(ingest::IngestFileResult::Updated) => files_updated += 1,
            Ok(ingest::IngestFileResult::Unchanged) => {}
            Err(error) => warn!("Incremental ingest failed for {}: {error}", path.display()),
        }
    }

    // Reconcile the source even when duplicate notifications ingest as
    // `Unchanged` or this generation only removes a file. A newer generation
    // may have cancelled the previous bounded embedding pass after ingestion,
    // leaving legitimate missing vectors for this source to resume.
    if source_change_job_requires_embedding_reconciliation(&job) {
        info!(
            "Reconciling source-scoped embeddings after watcher generation for source {}",
            job.source_id
        );
        match nexa_core::embedding_job::run_source(
            &app_state.db,
            &job.source_id,
            nexa_core::embedding_job::EmbeddingJobLimits::default(),
            &permit,
        ) {
            Ok(_) => {}
            Err(CoreError::Cancelled(reason)) => info!(
                "Background embedding yielded for source {}: {reason}",
                job.source_id
            ),
            Err(error) => warn!("Auto-embed failed for source {}: {error}", job.source_id),
        }
    }

    let payload = serde_json::json!({
        "sourceId": job.source_id,
        "filesAdded": files_added,
        "filesUpdated": files_updated,
        "filesRemoved": job.removed_paths.len(),
    });
    emit_app_event(app_handle, "file-changed", &payload);
}

fn source_change_job_requires_embedding_reconciliation(job: &SourceChangeJob) -> bool {
    !job.changed_paths.is_empty() || !job.removed_paths.is_empty()
}

// ── Agent Helpers ───────────────────────────────────────────────────────

fn normalize_optional_base_url(base_url: Option<String>) -> Option<String> {
    base_url.and_then(|value| {
        let trimmed = value.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn provider_type_for_config(config: &DbAgentConfig) -> ProviderType {
    provider_type_for_parts(&config.provider, config.base_url.as_deref())
}

fn provider_type_for_input(config: &SaveAgentConfigInput) -> ProviderType {
    provider_type_for_parts(&config.provider, config.base_url.as_deref())
}

/// Convert a DB [`DbAgentConfig`] to a [`ProviderConfig`] suitable for
/// [`create_provider`].
fn db_config_to_provider_config(
    config: &DbAgentConfig,
    timeout_secs: Option<u64>,
) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider_type_for_config(config),
        api_key: Some(config.api_key.clone()),
        base_url: normalize_optional_base_url(config.base_url.clone()),
        org_id: None,
        timeout_secs,
    }
}

fn select_agent_config_for_conversation(
    db: &Database,
    conv: &Conversation,
    requested_config_id: Option<&str>,
) -> Result<DbAgentConfig, String> {
    let configs = db.list_agent_configs().map_err(|e| e.to_string())?;
    if let Some(id) = requested_config_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if let Some(config) = configs.iter().find(|cfg| cfg.id == id).cloned() {
            return Ok(config);
        }
        warn!(
            "Requested agent config id '{}' was not found; falling back to conversation provider/model",
            id
        );
    }
    let default_config = configs.iter().find(|cfg| cfg.is_default).cloned();
    let matching_config = configs
        .iter()
        .filter(|cfg| cfg.provider == conv.provider && cfg.model == conv.model)
        .find(|cfg| cfg.is_default)
        .cloned()
        .or_else(|| {
            configs
                .iter()
                .find(|cfg| cfg.provider == conv.provider && cfg.model == conv.model)
                .cloned()
        });

    matching_config
        .or(default_config)
        .ok_or_else(|| "No agent config set. Please configure an LLM provider first.".to_string())
}

fn build_connection_probe_request(config: &SaveAgentConfigInput) -> CompletionRequest {
    CompletionRequest {
        model: config.model.trim().to_string(),
        messages: vec![Message::text(Role::User, "Reply with exactly: OK")],
        temperature: Some(0.0),
        max_tokens: Some(8),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_enabled: None,
        reasoning_effort: None,
        provider_type: Some(provider_type_for_input(config)),
        routing_session_id: None,
        parallel_tool_calls: true,
    }
}

/// Convert a DB [`ConversationMessage`] to an LLM [`Message`].
fn conv_message_to_llm(msg: &ConversationMessage) -> Message {
    let mut m = Message::text(
        msg.role.clone(),
        conversation_message_llm_context_content(msg),
    );
    m.name = msg.tool_call_id.clone();
    m.tool_calls = if msg.tool_calls.is_empty() {
        None
    } else {
        Some(msg.tool_calls.clone())
    };
    if msg.role == Role::Assistant {
        m.reasoning_content = conversation_message_reasoning_replay(msg);
        if let Some(envelope) = conversation_message_provider_turn(msg) {
            m.set_provider_turn(envelope);
        }
    }
    m
}

/// Repair persisted assistant/tool replay units before they enter a live agent
/// request. The core integrity policy owns completeness, uniqueness, adjacency,
/// and atomic removal; this desktop seam only records privacy-safe diagnostics.
fn sanitize_tool_call_history(
    messages: Vec<Message>,
    conversation_id: Option<&str>,
) -> Vec<Message> {
    let report = nexa_core::llm::message_validation::repair_persisted_message_history(
        messages,
        conversation_id,
    );
    for diagnostic in &report.repairs {
        warn!(
            "Repaired legacy-invalid conversation history before agent dispatch: conversation_id={}, message_index={}, role={}, reason={}",
            diagnostic.conversation_id.as_deref().unwrap_or("unknown"),
            diagnostic.message_index,
            diagnostic.role,
            diagnostic.reason,
        );
    }
    report.messages
}

#[cfg(test)]
mod tests {
    use super::conversation::{
        desktop_package_host_snapshot, filter_desktop_capability_views_by_package_host,
        set_desktop_package_host_package_enabled, set_desktop_package_host_package_health,
    };
    use super::preview::{
        append_preview_warning, build_file_preview, default_app_launch_command,
        file_explorer_launch_command, resolve_source_file,
    };
    use super::skills_mcp::filter_desktop_builtin_skills_by_package_host;
    use super::workflows::{
        apply_scheduled_execution_policy, due_workflow_run_is_scheduler_eligible,
        ensure_workflow_template_runtime_visible, filter_due_workflow_runs_by_package_host,
        queue_due_workflow_automation_execution_ticket,
        select_task_orchestrator_launch_agent_config, task_orchestrator_scheduler_due_runs,
        task_orchestrator_scheduler_retry_skip_event, task_orchestrator_scheduler_status_is_active,
        workflow_due_runs_to_queue_items,
    };
    use super::*;
    use crate::desktop_agent_session::{
        build_desktop_agent_session_config, desktop_runtime_package_context,
        filter_desktop_tool_names_by_package_host, runtime_session_config_artifact,
        DesktopAgentSessionConfigInput,
    };

    #[test]
    fn duplicate_or_removal_only_watcher_generation_resumes_missing_embeddings() {
        let duplicate_notification = SourceChangeJob {
            source_id: "source".to_string(),
            changed_paths: vec![PathBuf::from("unchanged.md")],
            removed_paths: Vec::new(),
        };
        let removal_only = SourceChangeJob {
            source_id: "source".to_string(),
            changed_paths: Vec::new(),
            removed_paths: vec![PathBuf::from("removed.md")],
        };

        assert!(source_change_job_requires_embedding_reconciliation(
            &duplicate_notification
        ));
        assert!(source_change_job_requires_embedding_reconciliation(
            &removal_only
        ));
    }

    #[test]
    fn history_sanitization_drops_empty_and_interrupted_assistant_records() {
        let mut interrupted = Message::text(Role::Assistant, "");
        interrupted.reasoning_content = Some("private reasoning".to_string());
        interrupted.tool_calls = Some(vec![nexa_core::llm::ToolCallRequest {
            id: "call-1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
            thought_signature: None,
        }]);
        let reasoning_only = Message {
            role: Role::Assistant,
            parts: Vec::new(),
            name: None,
            tool_calls: None,
            reasoning_content: Some("private reasoning".to_string()),
            prompt_cache_hint: None,
        };

        let sanitized = sanitize_tool_call_history(
            vec![
                Message::text(Role::User, "question"),
                interrupted,
                reasoning_only,
            ],
            None,
        );

        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].role, Role::User);
        assert!(!sanitized
            .iter()
            .any(|message| message.text_content().contains("Empty assistant")));
    }

    #[test]
    fn history_sanitization_preserves_text_but_quarantines_incomplete_tool_units() {
        let mut assistant = Message::text(Role::Assistant, "I started checking the repository.");
        assistant.tool_calls = Some(vec![nexa_core::llm::ToolCallRequest {
            id: "call-incomplete".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"unterminated""#.to_string(),
            thought_signature: None,
        }]);
        let tool_result = Message::text_with_name(
            Role::Tool,
            "The call was rejected before execution.",
            "call-incomplete",
        );

        let sanitized = sanitize_tool_call_history(
            vec![
                Message::text(Role::User, "Inspect this repository"),
                assistant,
                tool_result,
                Message::text(Role::User, "Continue"),
            ],
            Some("conversation-incomplete-tool"),
        );

        assert_eq!(sanitized.len(), 3);
        assert_eq!(
            sanitized[1].text_content(),
            "I started checking the repository."
        );
        assert!(sanitized[1].tool_calls.is_none());
        assert!(sanitized.iter().all(|message| message.role != Role::Tool));
        nexa_core::llm::message_validation::validate_provider_request(
            &sanitized,
            "openai",
            "deepseek-v4-pro",
        )
        .expect("repaired persisted history must cross the provider boundary");
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexa-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_source(root: &Path) -> Database {
        let db = Database::open_memory().expect("open memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("add source");
        db
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
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
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

    #[test]
    fn conv_message_to_llm_prefers_llm_context_content_artifact() {
        let msg = ConversationMessage {
            id: "msg-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            role: Role::User,
            content: "visible user text".to_string(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: Some(serde_json::json!({
                "llmContextContent": "retrieved context\n\nvisible user text"
            })),
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };

        let llm_message = conv_message_to_llm(&msg);

        assert_eq!(
            llm_message.text_content(),
            "retrieved context\n\nvisible user text"
        );
    }

    #[test]
    fn conv_message_to_llm_restores_provider_turn_after_restart() {
        let tool_call = nexa_core::llm::ToolCallRequest {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        };
        let envelope = nexa_core::llm::provider_turn::ProviderTurnEnvelope::capture(
            "turn-item-1",
            "sample-1",
            nexa_core::llm::provider_turn::RouteSnapshot {
                provider_endpoint_id: "deepseek-public".to_string(),
                provider_family: "deepseek".to_string(),
                api_style:
                    nexa_core::llm::reasoning_profile::ReasoningApiStyle::OpenAiChatCompletions,
                model_id: "deepseek-reasoner".to_string(),
                reasoning_profile_id: "deepseek-chat-v1".to_string(),
                reasoning_profile_version: 1,
                replay_policy:
                    nexa_core::llm::reasoning_profile::ReasoningReplayPolicy::RequiredOnToolCall,
            },
            "",
            Some("display reasoning"),
            Some("native replay reasoning"),
            vec![tool_call.clone()],
            true,
        );
        let msg = ConversationMessage {
            id: "msg-provider-turn".to_string(),
            conversation_id: "conversation-1".to_string(),
            role: Role::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![tool_call],
            artifacts: nexa_core::conversation::merge_provider_turn_envelope_artifact(
                None, &envelope,
            ),
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: Some("display reasoning".to_string()),
            image_attachments: None,
        };

        let llm_message = conv_message_to_llm(&msg);

        assert_eq!(llm_message.provider_turn(), Some(&envelope));
        assert_eq!(
            llm_message.reasoning_content.as_deref(),
            Some("native replay reasoning")
        );
    }

    fn save_test_agent_config(
        db: &Database,
        id: &str,
        model: &str,
        is_default: bool,
    ) -> DbAgentConfig {
        db.save_agent_config(&SaveAgentConfigInput {
            id: Some(id.to_string()),
            name: id.to_string(),
            provider: "open_ai".to_string(),
            api_key: "test-key".to_string(),
            base_url: None,
            model: model.to_string(),
            temperature: Some(0.2),
            max_tokens: Some(1024),
            context_window: Some(128_000),
            is_default,
            reasoning_enabled: Some(true),
            thinking_budget: Some(4096),
            reasoning_effort: Some("medium".to_string()),
            max_iterations: None,
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            delegation_limits_v2: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
            provider_endpoint_id: None,
            model_id: None,
        })
        .expect("save agent config")
    }

    fn test_skill(id: &str) -> Skill {
        Skill {
            id: id.to_string(),
            name: id.to_string(),
            description: "Use for tests".to_string(),
            content: "Body".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: Default::default(),
            dependencies: Default::default(),
            policy: Default::default(),
            source_path: None,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }
    }

    fn test_workflow_due_run() -> nexa_core::workflow_automation::WorkflowAutomationDueRun {
        nexa_core::workflow_automation::WorkflowAutomationDueRun {
            automation: nexa_core::workflow_automation::WorkflowAutomation {
                id: "automation-1".to_string(),
                name: "Daily report".to_string(),
                description: "Summarize daily evidence.".to_string(),
                workflow_template_id: "report_brief".to_string(),
                prompt: "Summarize reports.".to_string(),
                trigger_kind: "schedule".to_string(),
                trigger: nexa_core::workflow_automation::WorkflowAutomationTrigger::Schedule {
                    cron: "0 9 * * *".to_string(),
                },
                source_scope: vec!["source-1".to_string()],
                approval_policy: nexa_core::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: vec!["search_knowledge_base".to_string()],
                    risk_level: "medium".to_string(),
                },
                schedule_config: Default::default(),
                enabled: true,
                status: "ready".to_string(),
                last_run_at: None,
                next_run_at: Some("2099-01-01T09:00:00Z".to_string()),
                created_at: String::new(),
                updated_at: String::new(),
            },
            prompt: "Run the saved workflow.".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
            scheduled_for: Some("2099-01-01T09:00:00Z".to_string()),
        }
    }

    fn write_minimal_docx(path: &Path) {
        write_stored_zip(
            path,
            &[
                (
                    "[Content_Types].xml",
                    br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"# as &[u8],
                ),
                (
                    "_rels/.rels",
                    br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"# as &[u8],
                ),
                (
                    "word/document.xml",
                    br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Quarterly Report</w:t></w:r></w:p>
<w:p><w:r><w:t>Revenue increased.</w:t></w:r></w:p>
</w:body>
</w:document>"# as &[u8],
                ),
            ],
        );
    }

    fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
        struct CentralEntry {
            name: String,
            crc32: u32,
            size: u32,
            offset: u32,
        }

        fn push_u16(out: &mut Vec<u8>, value: u16) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        fn push_u32(out: &mut Vec<u8>, value: u32) {
            out.extend_from_slice(&value.to_le_bytes());
        }

        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    let mask = 0u32.wrapping_sub(crc & 1);
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        let mut out = Vec::new();
        let mut central_entries = Vec::new();
        for (name, data) in entries {
            let name_bytes = name.as_bytes();
            let offset = out.len() as u32;
            let crc = crc32(data);
            let size = data.len() as u32;

            push_u32(&mut out, 0x0403_4b50);
            push_u16(&mut out, 20);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u32(&mut out, crc);
            push_u32(&mut out, size);
            push_u32(&mut out, size);
            push_u16(&mut out, name_bytes.len() as u16);
            push_u16(&mut out, 0);
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            central_entries.push(CentralEntry {
                name: (*name).to_string(),
                crc32: crc,
                size,
                offset,
            });
        }

        let central_offset = out.len() as u32;
        for entry in &central_entries {
            let name_bytes = entry.name.as_bytes();
            push_u32(&mut out, 0x0201_4b50);
            push_u16(&mut out, 20);
            push_u16(&mut out, 20);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u32(&mut out, entry.crc32);
            push_u32(&mut out, entry.size);
            push_u32(&mut out, entry.size);
            push_u16(&mut out, name_bytes.len() as u16);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u32(&mut out, 0);
            push_u32(&mut out, entry.offset);
            out.extend_from_slice(name_bytes);
        }
        let central_size = out.len() as u32 - central_offset;

        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, central_entries.len() as u16);
        push_u16(&mut out, central_entries.len() as u16);
        push_u32(&mut out, central_size);
        push_u32(&mut out, central_offset);
        push_u16(&mut out, 0);

        std::fs::write(path, out).expect("write zip");
    }

    #[test]
    fn desktop_agent_session_config_projects_runtime_fields() {
        let db = Database::open_memory().unwrap();
        let db_config = test_agent_config();
        let mut app_cfg = AppConfig::default();
        app_cfg.tool_approval_mode = ToolApprovalMode::DenyAll;
        app_cfg.shell_access_mode = ShellAccessMode::Open;
        app_cfg.trace_enabled = false;
        let selected_skills = vec![test_skill("skill-a")];
        let loaded_skills = vec![test_skill("skill-b")];

        let config = build_desktop_agent_session_config(DesktopAgentSessionConfigInput {
            db: &db,
            conversation_id: "conversation-1",
            task_run_id: "task-run-1",
            db_config: &db_config,
            app_cfg: &app_cfg,
            source_scope_ids: &["source-1".to_string()],
            selected_skills: &selected_skills,
            auto_loaded_skills: &loaded_skills,
            execution_mode: AgentExecutionMode::Plan,
            collaboration_mode: nexa_core::mixture_of_agents::AgentCollaborationMode::Direct,
            moa_preset: nexa_core::mixture_of_agents::MoaPresetId::FastReview,
            orchestration_profile: nexa_core::quality_profile::OrchestrationProfile::Balanced,
            custom_orchestration: None,
        });

        assert_eq!(config.version, nexa_core::runtime::RUNTIME_PROTOCOL_VERSION);
        assert_eq!(config.session_id, "conversation-1");
        assert_eq!(config.conversation_id.as_deref(), Some("conversation-1"));
        assert_eq!(config.task_run_id.as_deref(), Some("task-run-1"));
        assert_eq!(
            config.host_surface,
            nexa_core::runtime::RuntimeHostSurface::Desktop
        );
        assert_eq!(config.provider.as_deref(), Some("open_ai"));
        assert_eq!(config.model.as_deref(), Some("gpt-test"));
        assert_eq!(config.reasoning_enabled, Some(true));
        assert_eq!(config.thinking_budget, Some(4096));
        assert_eq!(config.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(config.source_scope.source_ids, vec!["source-1".to_string()]);
        assert_eq!(config.approval_mode, ToolApprovalMode::DenyAll);
        assert_eq!(config.shell_access_mode, ShellAccessMode::Open);
        assert_eq!(config.execution_mode, AgentExecutionMode::Plan);
        assert!(!config.trace_enabled);
        assert_eq!(
            config.skill_context.available_skill_ids,
            vec!["skill-a".to_string()]
        );
        assert_eq!(
            config.skill_context.loaded_skill_ids,
            vec!["skill-b".to_string()]
        );
        assert_eq!(
            config.metadata["agentConfigId"].as_str(),
            Some("agent-config-1")
        );
        assert!(config
            .package_context
            .enabled_package_ids
            .contains(&"builtin-skills".to_string()));
        assert!(config
            .package_context
            .enabled_package_ids
            .contains(&"builtin-workflows".to_string()));
        assert!(config
            .package_context
            .enabled_package_ids
            .contains(&"mcp-connectors".to_string()));
    }

    #[test]
    fn desktop_runtime_package_context_comes_from_package_host() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();
        let context = desktop_runtime_package_context(&db);

        assert!(context
            .disabled_package_ids
            .contains(&"office-documents".to_string()));
        assert!(!context
            .enabled_package_ids
            .contains(&"office-documents".to_string()));
        assert!(context
            .enabled_package_ids
            .contains(&"desktop-automation".to_string()));
    }

    #[test]
    fn desktop_package_host_snapshot_uses_database_state() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();

        let snapshot = desktop_package_host_snapshot(&db).unwrap();
        let record = snapshot
            .records
            .iter()
            .find(|record| record.id == "office-documents")
            .unwrap();

        assert_eq!(
            record.state,
            nexa_core::package_host::PackageLifecycleState::Disabled
        );
        assert!(snapshot
            .records
            .iter()
            .any(|record| record.id == "desktop-automation"));
    }

    #[test]
    fn desktop_package_host_enable_rejects_unknown_package() {
        let db = Database::open_memory().unwrap();

        let error =
            set_desktop_package_host_package_enabled(&db, "missing-package", false).unwrap_err();

        assert!(error.contains("Unknown package id missing-package"));
        assert!(db
            .get_package_host_state("missing-package")
            .unwrap()
            .is_none());
    }

    #[test]
    fn desktop_package_host_health_update_returns_updated_snapshot() {
        let db = Database::open_memory().unwrap();

        let snapshot = set_desktop_package_host_package_health(
            &db,
            "mcp-connectors",
            nexa_core::package_host::PackageHealthState::Unhealthy,
        )
        .unwrap();
        let record = snapshot
            .records
            .iter()
            .find(|record| record.id == "mcp-connectors")
            .unwrap();

        assert_eq!(
            record.state,
            nexa_core::package_host::PackageLifecycleState::Unhealthy
        );
        assert_eq!(
            record.health,
            nexa_core::package_host::PackageHealthState::Unhealthy
        );
    }

    #[test]
    fn desktop_capability_views_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();
        db.set_package_host_package_health(
            "mcp-connectors",
            nexa_core::package_host::PackageHealthState::Unhealthy,
        )
        .unwrap();

        let manifests = filter_desktop_capability_views_by_package_host(
            &db,
            nexa_core::plugins::builtin_capability_views(),
        )
        .unwrap();
        let ids = manifests
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect::<Vec<_>>();

        assert!(!ids.contains(&"office-documents"));
        assert!(!ids.contains(&"mcp-connectors"));
        assert!(ids.contains(&"desktop-automation"));
    }

    #[test]
    fn desktop_tool_access_names_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        let names = vec![
            "compile_document".to_string(),
            "run_shell".to_string(),
            "mcp__server__tool".to_string(),
            "spawn_subagent".to_string(),
        ];

        let visible = filter_desktop_tool_names_by_package_host(&db, names.clone()).unwrap();
        assert!(visible.contains(&"compile_document".to_string()));
        assert!(visible.contains(&"mcp__server__tool".to_string()));

        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();
        db.set_package_host_package_health(
            "mcp-connectors",
            nexa_core::package_host::PackageHealthState::Unhealthy,
        )
        .unwrap();
        let visible = filter_desktop_tool_names_by_package_host(&db, names).unwrap();

        assert!(!visible.contains(&"compile_document".to_string()));
        assert!(!visible.contains(&"mcp__server__tool".to_string()));
        assert!(visible.contains(&"run_shell".to_string()));
        assert!(visible.contains(&"spawn_subagent".to_string()));
    }

    #[test]
    fn desktop_tool_registry_is_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();

        let tools = nexa_core::package_host::PackageRuntimeAssembler::database_builtin(&db)
            .and_then(|assembler| assembler.assemble_builtin_capabilities())
            .expect("assemble tool registry")
            .tools;

        assert!(!tools.contains("compile_document"));
        assert!(!tools.contains("get_document_info"));
        assert!(tools.contains("run_shell"));
    }

    #[test]
    fn desktop_builtin_skills_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        let skills = filter_desktop_builtin_skills_by_package_host(
            &db,
            nexa_core::skills::load_builtin_skills(),
        )
        .unwrap();
        assert!(!skills.is_empty());

        db.set_package_host_package_enabled("builtin-skills", false)
            .unwrap();
        let skills = filter_desktop_builtin_skills_by_package_host(
            &db,
            nexa_core::skills::load_builtin_skills(),
        )
        .unwrap();

        assert!(skills.is_empty());
    }

    #[test]
    fn desktop_workflow_templates_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        let templates =
            filter_desktop_workflow_templates_by_package_host(&db, workflow_catalog()).unwrap();
        assert!(!templates.is_empty());

        db.set_package_host_package_enabled("builtin-workflows", false)
            .unwrap();
        let templates =
            filter_desktop_workflow_templates_by_package_host(&db, workflow_catalog()).unwrap();

        assert!(templates.is_empty());
    }

    #[test]
    fn workflow_delivery_visibility_rejects_disabled_package() {
        let db = Database::open_memory().unwrap();
        ensure_workflow_template_runtime_visible(&db, "report_brief").unwrap();

        db.set_package_host_package_enabled("builtin-workflows", false)
            .unwrap();
        let error = ensure_workflow_template_runtime_visible(&db, "report_brief").unwrap_err();

        assert!(error.contains("Workflow template 'report_brief' is disabled or unavailable"));
    }

    #[test]
    fn due_workflow_runs_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        let due_runs =
            filter_due_workflow_runs_by_package_host(&db, vec![test_workflow_due_run()]).unwrap();
        assert_eq!(due_runs.len(), 1);

        db.set_package_host_package_enabled("builtin-workflows", false)
            .unwrap();
        let due_runs =
            filter_due_workflow_runs_by_package_host(&db, vec![test_workflow_due_run()]).unwrap();

        assert!(due_runs.is_empty());
    }

    #[test]
    fn due_workflow_adapter_returns_task_orchestrator_queue_items() {
        let items = workflow_due_runs_to_queue_items(&[test_workflow_due_run()]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].queue_id, "workflow_due:automation-1");
        assert_eq!(
            items[0].state,
            nexa_core::task_orchestrator::TaskOrchestratorState::Queued
        );
        assert_eq!(
            items[0].ownership.workflow_id.as_deref(),
            Some("report_brief")
        );
    }

    #[test]
    fn scheduler_due_run_policy_skips_approval_required_or_active_runs() {
        let mut due = test_workflow_due_run();
        due.automation.approval_policy.require_before_run = false;
        due.automation.status = "ready".to_string();

        assert!(due_workflow_run_is_scheduler_eligible(&due));

        due.automation.approval_policy.require_before_run = true;
        assert!(!due_workflow_run_is_scheduler_eligible(&due));

        due.automation.approval_policy.require_before_run = false;
        due.automation.status = "running".to_string();
        assert!(!due_workflow_run_is_scheduler_eligible(&due));

        assert!(task_orchestrator_scheduler_status_is_active("queued"));
        assert!(task_orchestrator_scheduler_status_is_active("cancelling"));
        assert!(!task_orchestrator_scheduler_status_is_active("completed"));
    }

    #[test]
    fn scheduler_retry_skip_event_distinguishes_backoff_from_retry_limit() {
        let backoff = nexa_core::workflow_automation::WorkflowAutomationSchedulerRetryDecision {
            allowed: false,
            max_attempts: 4,
            attempts_exhausted: false,
            retryable_failure_count: 1,
            last_retryable_event_type: Some("launch_failed".to_string()),
            last_retryable_event_at: Some("2099-01-01T08:59:00Z".to_string()),
            backoff_seconds: Some(300),
            backoff_until: Some("2099-01-01T09:04:00Z".to_string()),
            retry_after_seconds: Some(240),
        };
        assert_eq!(
            task_orchestrator_scheduler_retry_skip_event(&backoff),
            (
                "skipped_backoff",
                "backoff",
                "Scheduler skipped due workflow until retry backoff expires"
            )
        );

        let retry_limit =
            nexa_core::workflow_automation::WorkflowAutomationSchedulerRetryDecision {
                attempts_exhausted: true,
                ..backoff
            };
        assert_eq!(
            task_orchestrator_scheduler_retry_skip_event(&retry_limit),
            (
                "skipped_retry_limit",
                "blocked",
                "Scheduler skipped due workflow because retry attempts are exhausted"
            )
        );
    }

    #[test]
    fn scheduler_due_runs_respect_package_host_and_approval_gate() {
        let folder = unique_temp_dir("scheduler-due-policy");
        std::fs::write(folder.join("incoming.pdf"), "evidence").expect("write trigger file");
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation(
                &nexa_core::workflow_automation::SaveWorkflowAutomationInput {
                    id: None,
                    name: "Folder report".to_string(),
                    description: "Summarize folder evidence.".to_string(),
                    workflow_template_id: "report_brief".to_string(),
                    prompt: "Summarize new files.".to_string(),
                    trigger: nexa_core::workflow_automation::WorkflowAutomationTrigger::Folder {
                        path: folder.to_string_lossy().to_string(),
                        pattern: "*.pdf".to_string(),
                    },
                    source_scope: vec!["source-1".to_string()],
                    approval_policy:
                        nexa_core::workflow_automation::WorkflowAutomationApprovalPolicy {
                            require_before_run: true,
                            allowed_tools: Vec::new(),
                            risk_level: "medium".to_string(),
                        },
                    enabled: true,
                },
            )
            .expect("save workflow automation");

        let due_runs = task_orchestrator_scheduler_due_runs(&db, "2099-01-01T09:00:00Z")
            .expect("scheduler due runs");
        assert!(due_runs.is_empty());

        db.save_workflow_automation(
            &nexa_core::workflow_automation::SaveWorkflowAutomationInput {
                id: Some(automation.id.clone()),
                name: automation.name.clone(),
                description: automation.description.clone(),
                workflow_template_id: automation.workflow_template_id.clone(),
                prompt: automation.prompt.clone(),
                trigger: automation.trigger.clone(),
                source_scope: automation.source_scope.clone(),
                approval_policy: nexa_core::workflow_automation::WorkflowAutomationApprovalPolicy {
                    require_before_run: false,
                    allowed_tools: Vec::new(),
                    risk_level: "low".to_string(),
                },
                enabled: true,
            },
        )
        .expect("disable approval gate");

        let due_runs = task_orchestrator_scheduler_due_runs(&db, "2099-01-01T09:00:00Z")
            .expect("scheduler due runs after approval disabled");
        assert_eq!(due_runs.len(), 1);

        let failed_event = db
            .record_workflow_automation_scheduler_event(
                Some(&automation.id),
                None,
                "launch_failed",
                Some("failed"),
                "Scheduler failed to launch due workflow",
                Some(&serde_json::json!({ "retryable": true })),
            )
            .expect("record scheduler failure");
        db.update_workflow_scheduler_event_created_at(&failed_event.id, "2099-01-01T08:59:00Z")
            .expect("set scheduler failure time");

        let due_runs = task_orchestrator_scheduler_due_runs(&db, "2099-01-01T09:00:00Z")
            .expect("scheduler due runs during retry backoff");
        assert!(due_runs.is_empty());

        let due_runs = task_orchestrator_scheduler_due_runs(&db, "2099-01-01T09:05:00Z")
            .expect("scheduler due runs after retry backoff");
        assert_eq!(due_runs.len(), 1);

        db.set_package_host_package_enabled("builtin-workflows", false)
            .unwrap();
        let due_runs = task_orchestrator_scheduler_due_runs(&db, "2099-01-01T09:05:00Z")
            .expect("scheduler due runs after package disabled");
        assert!(due_runs.is_empty());
    }

    #[test]
    fn task_orchestrator_launch_adapter_selects_requested_or_default_agent_config() {
        let db = Database::open_memory().unwrap();
        save_test_agent_config(&db, "cfg-first", "gpt-first", false);
        save_test_agent_config(&db, "cfg-default", "gpt-default", true);

        let default_config =
            select_task_orchestrator_launch_agent_config(&db, None).expect("select default config");
        assert_eq!(default_config.id, "cfg-default");

        let requested_config = select_task_orchestrator_launch_agent_config(&db, Some("cfg-first"))
            .expect("select requested config");
        assert_eq!(requested_config.id, "cfg-first");

        let error =
            select_task_orchestrator_launch_agent_config(&db, Some("missing-config")).unwrap_err();
        assert!(error.contains("Requested agent config 'missing-config' was not found"));
    }

    #[test]
    fn scheduled_execution_policy_preserves_auto_and_fails_closed_on_route_drift() {
        let db = Database::open_memory().unwrap();
        let mut config = save_test_agent_config(&db, "cfg-scheduled", "gpt-original", true);
        config.provider_endpoint_id = Some("text:openai-official".to_string());

        let mut policy = nexa_core::workflow_scheduler::WorkflowAutomationExecutionPolicy {
            agent_config_id: Some(config.id.clone()),
            provider: Some(config.provider.clone()),
            provider_endpoint_id: config.provider_endpoint_id.clone(),
            model: Some("gpt-scheduled".to_string()),
            context_window: None,
            ..Default::default()
        };
        let resolved = apply_scheduled_execution_policy(config.clone(), &policy)
            .expect("matching route should accept scheduled overrides");
        assert_eq!(resolved.model, "gpt-scheduled");
        assert_eq!(resolved.context_window, None, "None keeps provider Auto");
        assert_eq!(resolved.model_id, None);
        assert_eq!(resolved.model_selection_resolution, None);

        policy.context_window = Some(750_000);
        let explicit = apply_scheduled_execution_policy(config.clone(), &policy)
            .expect("explicit context should remain authoritative");
        assert_eq!(explicit.context_window, Some(750_000));

        policy.provider_endpoint_id = Some("text:edited-endpoint".to_string());
        let error = apply_scheduled_execution_policy(config, &policy).unwrap_err();
        assert!(error.contains("endpoint drift"));
    }

    #[test]
    fn due_workflow_execution_ticket_helper_claims_run_and_advances_due_state() {
        let folder = unique_temp_dir("due-workflow-direct-launch");
        std::fs::write(folder.join("incoming.pdf"), "evidence").expect("write trigger file");
        let db = Database::open_memory().unwrap();
        let automation = db
            .save_workflow_automation(
                &nexa_core::workflow_automation::SaveWorkflowAutomationInput {
                    id: None,
                    name: "Folder report".to_string(),
                    description: "Summarize folder evidence.".to_string(),
                    workflow_template_id: "report_brief".to_string(),
                    prompt: "Summarize new files.".to_string(),
                    trigger: nexa_core::workflow_automation::WorkflowAutomationTrigger::Folder {
                        path: folder.to_string_lossy().to_string(),
                        pattern: "*.pdf".to_string(),
                    },
                    source_scope: vec!["source-1".to_string()],
                    approval_policy: Default::default(),
                    enabled: true,
                },
            )
            .expect("save workflow automation");

        let ticket = queue_due_workflow_automation_execution_ticket(
            &db,
            &automation.id,
            "2099-01-01T09:00:00Z",
            None,
        )
        .expect("queue due workflow execution ticket");

        assert_eq!(
            ticket.run.status.state,
            nexa_core::task_orchestrator::TaskOrchestratorState::Queued
        );
        assert_eq!(
            ticket.delivery.queue_item.queue_id,
            format!("workflow_due:{}", automation.id)
        );
        assert_eq!(
            ticket.delivery.queue_item.ownership.source_scope,
            vec!["source-1".to_string()]
        );
        let run = db
            .get_workflow_automation_run(&ticket.run.run_id)
            .expect("persisted workflow run");
        assert_eq!(run.status, "queued");
        assert_eq!(
            run.summary.as_deref(),
            Some(ticket.delivery.queue_item.due_reason.as_str())
        );
        let due_runs = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .expect("list due workflows after claim");
        assert!(!due_runs
            .iter()
            .any(|due| due.automation.id == automation.id));
    }

    #[test]
    fn runtime_session_config_artifact_wraps_protocol_config() {
        let db = Database::open_memory().unwrap();
        let db_config = test_agent_config();
        let app_cfg = AppConfig::default();
        let config = build_desktop_agent_session_config(DesktopAgentSessionConfigInput {
            db: &db,
            conversation_id: "conversation-1",
            task_run_id: "task-run-1",
            db_config: &db_config,
            app_cfg: &app_cfg,
            source_scope_ids: &[],
            selected_skills: &[],
            auto_loaded_skills: &[],
            execution_mode: AgentExecutionMode::Normal,
            collaboration_mode: nexa_core::mixture_of_agents::AgentCollaborationMode::Direct,
            moa_preset: nexa_core::mixture_of_agents::MoaPresetId::FastReview,
            orchestration_profile: nexa_core::quality_profile::OrchestrationProfile::Balanced,
            custom_orchestration: None,
        });

        let artifact = runtime_session_config_artifact(&config);

        assert_eq!(artifact["kind"].as_str(), Some("agentSessionConfig"));
        assert_eq!(artifact["version"].as_u64(), Some(1));
        assert_eq!(
            artifact["config"]["conversationId"].as_str(),
            Some("conversation-1")
        );
        assert_eq!(artifact["config"]["taskRunId"].as_str(), Some("task-run-1"));
    }

    #[test]
    fn preview_default_app_launch_command_uses_platform_opener() {
        let path = Path::new("workspace").join("note.txt");
        let command = default_app_launch_command(&path);

        #[cfg(target_os = "windows")]
        {
            assert_eq!(command.program, "rundll32.exe");
            assert_eq!(
                command.args,
                vec![
                    "url.dll,FileProtocolHandler".to_string(),
                    path.to_string_lossy().to_string()
                ]
            );
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.program, "open");
            assert_eq!(command.args, vec![path.to_string_lossy().to_string()]);
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(command.program, "xdg-open");
            assert_eq!(command.args, vec![path.to_string_lossy().to_string()]);
        }
    }

    #[test]
    fn preview_file_explorer_launch_command_uses_platform_opener() {
        let path = Path::new("workspace").join("note.txt");
        let command = file_explorer_launch_command(&path);

        #[cfg(target_os = "windows")]
        {
            assert_eq!(command.program, "explorer.exe");
            assert_eq!(command.args.len(), 1);
            assert!(command.args[0].starts_with("/select,"));
            assert!(command.args[0].contains("note.txt"));
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.program, "open");
            assert_eq!(
                command.args,
                vec!["-R".to_string(), path.to_string_lossy().to_string()]
            );
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(command.program, "xdg-open");
            assert_eq!(command.args, vec!["workspace".to_string()]);
        }
    }

    #[test]
    fn preview_launch_request_uses_detached_execution_contract() {
        let path = Path::new("workspace").join("note.txt");
        let request = default_app_launch_command(&path).into_execution_request();
        let decision = nexa_core::execution_environment::review_execution_policy(&request);

        assert_eq!(request.caller.tool_name.as_deref(), Some("desktop_preview"));
        assert_eq!(
            request.sandbox.backend,
            nexa_core::execution_environment::ExecutionBackendKind::LocalOpen
        );
        assert_eq!(
            request.sandbox.allowed_programs,
            vec![request.program.clone()]
        );
        assert!(!request.network_intent);
        assert!(!request.sandbox.network_allowed);
        assert!(!request.sandbox.capture_file_changes);
        assert_eq!(
            decision.kind,
            nexa_core::execution_environment::ExecutionDecisionKind::Allowed
        );
    }

    #[test]
    fn preview_resolves_unique_bare_filename_below_source_root() {
        let root = unique_temp_dir("preview-source");
        let nested = root.join("scripts").join("generated");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        let file = nested.join("part2.py");
        std::fs::write(&file, "print('ok')\n").expect("write file");
        let db = db_with_source(&root);

        let resolved = resolve_source_file(&db, "part2.py").expect("resolve bare filename");

        assert_eq!(resolved.canonical, std::fs::canonicalize(&file).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_file_preview_uses_structured_office_preview_without_layout_by_default() {
        let root = unique_temp_dir("preview-docx-source");
        let app_data_dir = unique_temp_dir("preview-docx-app");
        let file = root.join("sample.docx");
        write_minimal_docx(&file);
        let db = db_with_source(&root);

        let preview = build_file_preview(&db, &file.to_string_lossy(), Some(&app_data_dir))
            .expect("build preview");

        assert!(preview.capabilities.can_render_structured);
        assert!(preview.rendered_preview.is_none());
        assert!(matches!(
            preview.structured_preview,
            Some(nexa_core::preview::StructuredPreview::Document { .. })
        ));
        assert!(!preview
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("Rich Office preview"));

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(app_data_dir);
    }

    #[test]
    fn append_preview_warning_keeps_existing_context() {
        let mut warning = Some("Plain text extracted with fallback.".to_string());

        append_preview_warning(&mut warning, "Rich Office preview unavailable.");

        assert_eq!(
            warning.as_deref(),
            Some("Plain text extracted with fallback.\nRich Office preview unavailable.")
        );
    }

    #[test]
    fn split_text_by_utf8_bytes_preserves_cjk_boundaries() {
        let text = "ab中文cd";
        let chunks = split_text_by_utf8_bytes(text, 4);

        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 4));
        assert_eq!(chunks, vec!["ab", "中", "文c", "d"]);
    }

    #[test]
    fn compact_agent_event_caps_tool_payloads_for_frontend() {
        let content = "x".repeat(MAX_FRONTEND_TOOL_CONTENT_CHARS + 100);
        let artifact_text = "y".repeat(MAX_FRONTEND_ARTIFACT_STRING_CHARS + 100);
        let event = AgentEvent::ToolCallResult {
            call_id: "call-1".to_string(),
            tool_name: "read_files".to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "large": artifact_text,
            })),
        };

        let compacted = compact_agent_event_for_frontend(event);

        match compacted {
            AgentEvent::ToolCallResult {
                content,
                artifacts: Some(artifacts),
                ..
            } => {
                assert!(content.contains("[truncated]"));
                assert!(content.chars().count() <= MAX_FRONTEND_TOOL_CONTENT_CHARS);
                let large = artifacts
                    .get("large")
                    .and_then(serde_json::Value::as_str)
                    .expect("large artifact string");
                assert!(large.contains("[truncated]"));
                assert!(large.chars().count() <= MAX_FRONTEND_ARTIFACT_STRING_CHARS);
            }
            other => panic!("unexpected compacted event: {other:?}"),
        }
    }

    #[test]
    fn compact_agent_event_preserves_full_diff_artifact_shape() {
        let lines = (0..400)
            .map(|idx| {
                serde_json::json!({
                    "type": "addition",
                    "oldLine": null,
                    "newLine": idx + 1,
                    "content": format!("line {}", idx + 1),
                })
            })
            .collect::<Vec<_>>();
        let event = AgentEvent::ToolCallResult {
            call_id: "call-1".to_string(),
            tool_name: "edit_file".to_string(),
            content: "ok".to_string(),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "diff": {
                    "path": "src/main.rs",
                    "operation": "str_replace",
                    "hunks": [{
                        "oldStart": 1,
                        "newStart": 1,
                        "oldLines": 0,
                        "newLines": 400,
                        "lines": lines,
                    }],
                },
            })),
        };

        let compacted = compact_agent_event_for_frontend(event);

        match compacted {
            AgentEvent::ToolCallResult {
                artifacts: Some(artifacts),
                ..
            } => {
                let lines = artifacts["diff"]["hunks"][0]["lines"]
                    .as_array()
                    .expect("diff lines array");
                assert_eq!(lines.len(), 400);
                assert_eq!(lines[399]["newLine"].as_u64(), Some(400));
            }
            other => panic!("unexpected compacted event: {other:?}"),
        }
    }

    #[test]
    fn debounced_watcher_path_uses_latest_event_for_atomic_saves() {
        let path = PathBuf::from("atomic-save.md");
        let mut changed_paths = HashSet::new();
        let mut removed_paths = HashSet::new();

        record_debounced_watcher_path(
            &mut changed_paths,
            &mut removed_paths,
            path.clone(),
            WatcherEventKind::Removed,
        );
        record_debounced_watcher_path(
            &mut changed_paths,
            &mut removed_paths,
            path.clone(),
            WatcherEventKind::Created,
        );

        assert_eq!(changed_paths, HashSet::from([path.clone()]));
        assert!(removed_paths.is_empty());

        record_debounced_watcher_path(
            &mut changed_paths,
            &mut removed_paths,
            path.clone(),
            WatcherEventKind::Removed,
        );

        assert!(changed_paths.is_empty());
        assert_eq!(removed_paths, HashSet::from([path]));
    }
}
