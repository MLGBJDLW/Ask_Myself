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
use crate::agent_stream_bridge::AgentStreamForwarder;
use crate::agent_task_events::{
    emit_agent_task_run_update, record_agent_run_status_task_event, record_agent_run_task_event,
};
use crate::app_events::emit_app_event;
use crate::subagent_tool::{
    DelegationRuntime, JudgeSubagentResultsTool, SubagentBatchTool, SubagentTool,
};
use nexa_core::agent::{
    build_system_prompt, AgentConfig as ExecutorConfig, AgentEvent, AgentExecutor,
    AgentSteeringMessage, CancellationToken, ConfirmationCallback,
};
use nexa_core::agent_run::{AgentRunEvent, AgentRunPhase};
use nexa_core::app_settings::{AppConfig, ShellAccessMode, WizardState};
use nexa_core::approval::{
    ApprovalCallback, ApprovalDecision, ApprovalRequest, SessionApprovalStore, ToolApprovalMode,
    ToolApprovalPolicy, ToolPermissionKey,
};
use nexa_core::conversation::memory::estimate_tokens;
use nexa_core::conversation::{
    AgentConfig as DbAgentConfig, AgentExecutionGraph, AgentSubtaskRun, AgentTaskArtifact,
    AgentTaskArtifactSummary, AgentTaskArtifactVersion, AgentTaskRun, AgentTaskRunEvent,
    AgentTaskRunListItem, CheckpointBranch, CollectionContext, Conversation, ConversationMessage,
    ConversationStats, ConversationTurn, CreateAgentTaskArtifactInput, CreateConversationInput,
    ImageAttachment, SaveAgentConfigInput, UpdateAgentTaskArtifactInput,
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
    create_provider, model_supports_vision, CompletionRequest, ContentPart, Message,
    ProviderConfig, ProviderType, ReasoningEffort, Role,
};
use nexa_core::mcp::{McpServer, McpToolInfo, SaveMcpServerInput};
use nexa_core::persona::{PersonaProfile, SavePersonaInput};

use base64::Engine;
use chrono::{Local, SecondsFormat, Utc};
use log::{info, warn};
use nexa_core::models::{
    EvidenceCard, Playbook, PlaybookCitation, SearchFilters, SearchQuery, Source,
};
use nexa_core::ocr::extract_text_from_image;
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
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
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

/// Application state holding the database connection.
pub struct AppState {
    pub db: Arc<Database>,
    /// Guard: true while whisper transcription is in progress.
    #[cfg(feature = "video")]
    pub whisper_busy: Arc<AtomicBool>,
    /// Lock to serialize scan operations and prevent duplicate document inserts.
    pub scan_lock: Arc<Mutex<()>>,
}

/// State for tracking running agent tasks (for cancellation).
pub struct RunningAgentTask {
    pub cancel_token: CancellationToken,
    pub task: tokio::task::JoinHandle<()>,
    pub steering_tx: tokio::sync::mpsc::UnboundedSender<AgentSteeringMessage>,
    pub task_run_id: String,
    pub turn_id: String,
    pub stream_event_seq: Arc<AtomicU64>,
}

pub struct AgentState {
    /// Map of conversation_id → running agent task state.
    pub running: TokioMutex<HashMap<String, RunningAgentTask>>,
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
pub fn list_workflow_templates_cmd() -> Vec<WorkflowCatalogTemplate> {
    workflow_catalog()
}

async fn sync_enabled_mcp_servers(
    db: &Database,
    manager: &mut nexa_core::mcp::McpManager,
) -> Result<HashMap<String, String>, String> {
    let enabled_servers = db.get_enabled_mcp_servers().map_err(|e| e.to_string())?;
    let app_cfg = db.load_app_config().unwrap_or_default();
    Ok(manager
        .sync_servers(&enabled_servers, Some(app_cfg.mcp_call_timeout_secs))
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
    let mut m = Message::text(msg.role.clone(), &msg.content);
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

fn config_timeout_secs(value: Option<i64>, app_value: i64, default_value: u32) -> u32 {
    let secs = value.unwrap_or(app_value);
    if secs < 0 {
        default_value
    } else {
        secs.min(u32::MAX as i64) as u32
    }
}

/// After an interrupted agent execution, check for assistant messages with
/// `tool_calls` that lack corresponding tool response messages, and insert
/// synthetic error responses so the conversation history remains valid.
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
            let mut found_ids = HashSet::new();
            let mut j = i + 1;
            while j < msgs.len() && msgs[j].role == Role::Tool {
                if let Some(ref tc_id) = msgs[j].tool_call_id {
                    found_ids.insert(tc_id.as_str());
                }
                j += 1;
            }

            // Find the max sort_order among existing tool responses (or the assistant msg)
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
    use super::preview::{append_preview_warning, build_file_preview, resolve_source_file};
    use super::*;

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
}
