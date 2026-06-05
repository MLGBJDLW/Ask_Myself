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
use crate::agent_stream::{emit_agent_frontend_event, emit_agent_run_frontend_event};
use crate::agent_task_events::{
    emit_agent_task_run_update, record_agent_run_status_task_event, record_agent_run_task_event,
};
use crate::app_events::emit_app_event;
use crate::desktop_agent_session::DesktopRunningAgentTask;
use nexa_core::agent::{
    build_system_prompt, AgentConfig as ExecutorConfig, AgentEvent, AgentExecutionMode,
    AgentExecutor, AgentRequestKind, AgentSteeringMessage, CancellationToken,
};
use nexa_core::agent_run::{AgentRunEvent, AgentRunPhase};
use nexa_core::app_settings::{AppConfig, ShellAccessMode, WizardState};
use nexa_core::approval::{
    ApprovalCallback, ApprovalDecision, ApprovalRequest, SessionApprovalStore, ToolApprovalMode,
    ToolApprovalPolicy, ToolPermissionKey,
};
use nexa_core::conversation::memory::estimate_tokens;
use nexa_core::conversation::{
    conversation_message_llm_context_content, AgentConfig as DbAgentConfig, AgentExecutionGraph,
    AgentSubtaskRun, AgentTaskArtifact, AgentTaskArtifactSummary, AgentTaskArtifactVersion,
    AgentTaskRun, AgentTaskRunEvent, AgentTaskRunListItem, CheckpointBranch, CollectionContext,
    Conversation, ConversationMessage, ConversationStats, ConversationTurn,
    CreateAgentTaskArtifactInput, CreateConversationInput, ImageAttachment, SaveAgentConfigInput,
    UpdateAgentTaskArtifactInput,
};
use nexa_core::db::Database;
use nexa_core::embed::{EmbedderConfig, LocalEmbeddingModel};
use nexa_core::error::CoreError;
use nexa_core::evolution::{
    AgentProceduralMemory, AppliedSkillChange, SkillChangeProposal, SkillProposalStatus,
};
use nexa_core::feedback::{Feedback, FeedbackAction};
use nexa_core::index::IndexStats;
use nexa_core::ingest::{self, EmbedResult, IngestResult};
use nexa_core::llm::{
    create_provider, CompletionRequest, ContentPart, Message, ProviderConfig, ProviderType, Role,
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
use nexa_core::provider_catalog::{load_provider_presets, preset_model_ids, ProviderPreset};
use nexa_core::provider_registry::provider_type_for_parts;
use nexa_core::search::{self, SearchResult};
use nexa_core::skills::{DiscoveredSkillBundle, SaveSkillInput, Skill};
use nexa_core::source_tree::SourceTree;
use nexa_core::sources::{CreateSourceInput, UpdateSourceInput};
use nexa_core::tools::default_tool_registry;
use nexa_core::watcher::{FileWatcher, WatcherEventKind};
use nexa_core::workflow_catalog::{workflow_catalog, WorkflowCatalogTemplate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

mod agent_chat;
mod app_config;
mod approval;
mod conversation;
mod knowledge;
mod media;
mod personas;
mod preview;
mod skills_mcp;
mod sources;
mod watcher;
mod workflows;

pub use agent_chat::*;
pub use app_config::*;
pub use approval::*;
pub use conversation::*;
pub use knowledge::*;
pub use media::*;
pub use personas::*;
pub use preview::*;
pub use skills_mcp::*;
pub use sources::*;
pub use watcher::*;
pub use workflows::*;

const DEFAULT_MCP_CALL_TIMEOUT_SECS: u64 = 300;
const UNLIMITED_EXECUTOR_TIMEOUT_SECS: u32 = 0;

/// Application state holding the database connection.
pub struct AppState {
    pub db: Arc<Database>,
    /// Guard: true while whisper transcription is in progress.
    #[cfg(feature = "video")]
    pub whisper_busy: Arc<AtomicBool>,
    /// Lock to serialize scan operations and prevent duplicate document inserts.
    pub scan_lock: Arc<Mutex<()>>,
}

pub struct AgentState {
    /// Map of conversation_id → running agent task state.
    pub running: TokioMutex<HashMap<String, DesktopRunningAgentTask>>,
}

struct TerminalAgentError<'a> {
    conversation_id: &'a str,
    task_run_id: &'a str,
    turn_id: &'a str,
    message: &'a str,
    status: &'a str,
    payload: Option<&'a serde_json::Value>,
}

fn emit_terminal_agent_error_once(
    terminal_emitted: &AtomicBool,
    db: &Database,
    app_handle: &AppHandle,
    stream_event_seq: &AtomicU64,
    error: TerminalAgentError<'_>,
) {
    if terminal_emitted.swap(true, Ordering::SeqCst) {
        return;
    }

    let event_seq = stream_event_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let run_event = AgentRunEvent::terminal_error(
        error.task_run_id,
        Some(error.turn_id),
        event_seq,
        error.message,
        error.status,
        error.payload,
    );
    emit_agent_run_frontend_event(app_handle, error.conversation_id, &run_event);
    record_agent_run_task_event(
        db,
        app_handle,
        error.conversation_id,
        error.task_run_id,
        &run_event,
        "error",
        error.message,
        Some(error.status),
        error.payload,
    );
}

/// State for the MCP server manager.
pub struct McpManagerState {
    pub manager: TokioMutex<nexa_core::mcp::McpManager>,
}

/// State for tracking active model download cancellation.
pub struct DownloadCancelFlag(pub Arc<AtomicBool>);

/// State for the per-call tool approval flow.
///
/// `pending` maps an [`ApprovalRequest`] id → a oneshot `Sender` that the
/// Tauri `approve_tool_call_cmd` resolves once the user clicks a button
/// in the GUI. `session_store` holds "allow for this session" grants that
/// persist until the app is closed.
#[derive(Default)]
pub struct ApprovalState {
    pub pending: Arc<TokioMutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalDecision>>>>,
    pub session_store: SessionApprovalStore,
}

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

/// Initialise the file watcher, start watching all sources with
/// `watch_enabled = true`, and spawn a background thread that processes
/// file-change events (debounced, auto-scan, emit to frontend).
pub fn init_watcher(app_handle: tauri::AppHandle, db: &Database) {
    let (file_watcher, rx) = match FileWatcher::new() {
        Ok(pair) => pair,
        Err(e) => {
            warn!("Failed to initialise file watcher: {e}");
            return;
        }
    };

    let mut watcher_guard = file_watcher;
    let mut watched_map: HashMap<String, String> = HashMap::new();

    // Watch all sources where watch_enabled = true.
    if let Ok(sources) = db.list_sources() {
        for source in &sources {
            if source.watch_enabled {
                let path = Path::new(&source.root_path);
                if path.exists() {
                    if let Err(e) = watcher_guard.watch(path) {
                        warn!("Failed to watch {}: {e}", source.root_path);
                    } else {
                        watched_map.insert(source.id.clone(), source.root_path.clone());
                    }
                }
            }
        }
    }

    // Split watcher_guard back into WatcherState so we can share it.
    // We need a temporary trick: FileWatcher doesn't derive Clone, but
    // we can wrap it in Mutex after setup.
    let watcher_state = WatcherState {
        watcher: Mutex::new(watcher_guard),
        watched: Mutex::new(watched_map),
    };
    app_handle.manage(watcher_state);

    // Clone what we need for the background thread.
    let handle = app_handle.clone();

    thread::spawn(move || {
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
                        if event.kind == WatcherEventKind::Removed {
                            entry.2.insert(event.path.clone());
                        } else {
                            // Created or Modified
                            entry.1.insert(event.path.clone());
                        }
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

                // Handle removed files: delete their documents from the DB.
                for removed in &removed_paths {
                    let path_str = removed.to_string_lossy();
                    match app_state.db.delete_document_by_path(&path_str) {
                        Ok(true) => info!("Removed document for deleted file: {path_str}"),
                        Ok(false) => { /* file wasn't indexed, nothing to do */ }
                        Err(e) => warn!("Failed to remove document for {path_str}: {e}"),
                    }
                }

                // Incrementally ingest only the changed files instead of
                // re-scanning the entire source directory.
                let mut files_added = 0usize;
                let mut files_updated = 0usize;
                for path in &changed_paths {
                    match ingest::ingest_single_file(&app_state.db, &source_id, path) {
                        Ok(ingest::IngestFileResult::Added) => files_added += 1,
                        Ok(ingest::IngestFileResult::Updated) => files_updated += 1,
                        Ok(ingest::IngestFileResult::Unchanged) => {}
                        Err(e) => warn!("Incremental ingest failed for {}: {e}", path.display()),
                    }
                }

                // Embed any new un-embedded chunks.
                if files_added > 0 || files_updated > 0 {
                    info!("Auto-embedding after incremental ingest for source {source_id}");
                    if let Err(e) = ingest::embed_source(&app_state.db, &source_id) {
                        warn!("Auto-embed failed for source {source_id}: {e}");
                    }
                }

                let payload = serde_json::json!({
                    "sourceId": source_id,
                    "filesAdded": files_added,
                    "filesUpdated": files_updated,
                    "filesRemoved": removed_paths.len(),
                });
                emit_app_event(&handle, "file-changed", &payload);
            }
        }
    });
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
        reasoning_effort: None,
        provider_type: Some(provider_type_for_input(config)),
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
        m.reasoning_content = msg.thinking.clone();
    }
    m
}

/// Sanitize conversation history to ensure every assistant message with
/// `tool_calls` is followed by matching tool response messages.
///
/// If an assistant message has orphaned tool_calls (no matching tool responses),
/// the tool_calls field is stripped to prevent API errors like:
/// "An assistant message with 'tool_calls' must be followed by tool messages
/// responding to each 'tool_call_id'."
fn sanitize_tool_call_history(mut messages: Vec<Message>) -> Vec<Message> {
    let mut indices_to_remove: HashSet<usize> = HashSet::new();

    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Assistant {
            if let Some(ref tool_calls) = messages[i].tool_calls {
                if !tool_calls.is_empty() {
                    // Collect expected tool_call_ids
                    let expected_ids: HashSet<&str> =
                        tool_calls.iter().map(|tc| tc.id.as_str()).collect();

                    // Check following messages for matching tool responses
                    let mut found_ids = HashSet::new();
                    let mut j = i + 1;
                    while j < messages.len() && messages[j].role == Role::Tool {
                        if let Some(ref name) = messages[j].name {
                            found_ids.insert(name.as_str());
                        }
                        j += 1;
                    }

                    // If any tool_call_id is missing a response, strip everything
                    if !expected_ids.is_subset(&found_ids) {
                        warn!(
                            "Sanitizing orphaned tool_calls in conversation history: \
                             expected {:?}, found {:?}",
                            expected_ids, found_ids
                        );
                        messages[i].tool_calls = None;

                        // Add placeholder if content is empty
                        if messages[i].text_content().trim().is_empty() {
                            messages[i].parts = vec![ContentPart::Text {
                                text: "[Tool calls interrupted before completion]".to_string(),
                            }];
                        }

                        // Mark ALL following Tool messages for removal
                        // (they're orphaned since we stripped the tool_calls)
                        let mut k = i + 1;
                        while k < messages.len() && messages[k].role == Role::Tool {
                            indices_to_remove.insert(k);
                            k += 1;
                        }
                    }
                }
            }
        }
        i += 1;
    }

    // Additional pass: find any Tool messages whose tool_call_id doesn't
    // match any preceding assistant's tool_calls
    for i in 0..messages.len() {
        if messages[i].role == Role::Tool && !indices_to_remove.contains(&i) {
            let tool_id = messages[i].name.as_deref().unwrap_or("");
            let has_match = messages[..i].iter().any(|m| {
                m.role == Role::Assistant
                    && m.tool_calls
                        .as_ref()
                        .is_some_and(|tcs| tcs.iter().any(|tc| tc.id == tool_id))
            });
            if !has_match {
                indices_to_remove.insert(i);
            }
        }
    }

    // Remove orphaned tool messages
    if !indices_to_remove.is_empty() {
        messages = messages
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| !indices_to_remove.contains(idx))
            .map(|(_, msg)| msg)
            .collect();
    }

    // Final pass: fix any assistant messages with neither content nor tool_calls
    for msg in &mut messages {
        if msg.role == Role::Assistant
            && msg.tool_calls.as_ref().map_or(true, |tc| tc.is_empty())
            && msg.text_content().trim().is_empty()
        {
            msg.parts = vec![ContentPart::Text {
                text: "[Empty assistant message]".to_string(),
            }];
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::conversation::{
        desktop_package_host_snapshot, filter_desktop_builtin_plugins_by_package_host,
        set_desktop_package_host_package_enabled, set_desktop_package_host_package_health,
    };
    use super::preview::{
        append_preview_warning, build_file_preview, default_app_launch_command,
        file_explorer_launch_command, resolve_source_file,
    };
    use super::skills_mcp::filter_desktop_builtin_skills_by_package_host;
    use super::workflows::{
        due_workflow_run_is_scheduler_eligible, ensure_workflow_template_runtime_visible,
        filter_due_workflow_runs_by_package_host, queue_due_workflow_automation_execution_ticket,
        select_task_orchestrator_launch_agent_config, task_orchestrator_scheduler_due_runs,
        task_orchestrator_scheduler_retry_skip_event, task_orchestrator_scheduler_status_is_active,
        workflow_due_runs_to_queue_items,
    };
    use super::*;
    use crate::desktop_agent_session::{
        build_desktop_agent_session_config, desktop_runtime_package_context,
        filter_desktop_tool_names_by_package_host, filter_desktop_tool_registry_by_package_host,
        runtime_session_config_artifact, DesktopAgentSessionConfigInput,
    };

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
            max_iterations: Some(25),
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
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
            max_iterations: Some(25),
            summarization_model: None,
            summarization_provider: None,
            image_generation_model: None,
            subagent_allowed_tools: None,
            subagent_allowed_skill_ids: None,
            subagent_max_parallel: None,
            subagent_max_calls_per_turn: None,
            subagent_token_budget: None,
            tool_timeout_secs: None,
            agent_timeout_secs: None,
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
                enabled: true,
                status: "ready".to_string(),
                last_run_at: None,
                next_run_at: Some("2099-01-01T09:00:00Z".to_string()),
                created_at: String::new(),
                updated_at: String::new(),
            },
            prompt: "Run the saved workflow.".to_string(),
            due_reason: "schedule 0 9 * * *".to_string(),
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
    fn desktop_builtin_plugins_are_filtered_by_package_host_state() {
        let db = Database::open_memory().unwrap();
        db.set_package_host_package_enabled("office-documents", false)
            .unwrap();
        db.set_package_host_package_health(
            "mcp-connectors",
            nexa_core::package_host::PackageHealthState::Unhealthy,
        )
        .unwrap();

        let manifests = filter_desktop_builtin_plugins_by_package_host(
            &db,
            nexa_core::plugins::builtin_plugin_manifests(),
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

        let tools = filter_desktop_tool_registry_by_package_host(&db, default_tool_registry())
            .expect("filter tool registry");

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
        db.conn()
            .execute(
                "UPDATE workflow_automation_scheduler_events SET created_at = ?2 WHERE id = ?1",
                rusqlite::params![failed_event.id, "2099-01-01T08:59:00Z"],
            )
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
}
