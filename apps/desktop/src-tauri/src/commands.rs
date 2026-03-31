use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(feature = "video")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::subagent_tool::{
    DelegationRuntime, JudgeSubagentResultsTool, SubagentBatchTool, SubagentTool,
};
use ask_core::agent::{
    build_system_prompt, AgentConfig as ExecutorConfig, AgentEvent, AgentExecutor,
    CancellationToken, ConfirmationCallback,
};
use ask_core::app_settings::AppConfig;
use ask_core::conversation::memory::estimate_tokens;
use ask_core::conversation::{
    AgentConfig as DbAgentConfig, Conversation, ConversationMessage, ConversationStats,
    CreateConversationInput, SaveAgentConfigInput,
};
use ask_core::db::Database;
use ask_core::embed::{EmbedderConfig, LocalEmbeddingModel};
use ask_core::feedback::{Feedback, FeedbackAction};
use ask_core::index::IndexStats;
use ask_core::ingest::{self, EmbedResult, IngestResult};
use ask_core::llm::{
    create_provider, model_supports_vision, ContentPart, Message, ProviderConfig, ProviderType,
    ReasoningEffort, Role,
};
use ask_core::mcp::{McpServer, McpToolInfo, SaveMcpServerInput};
use ask_core::models::{
    EvidenceCard, Playbook, PlaybookCitation, SearchFilters, SearchQuery, Source,
};
use ask_core::ocr::extract_text_from_image;
use ask_core::playbook::QueryLog;
use ask_core::privacy::PrivacyConfig;
use ask_core::search::{self, SearchResult};
use ask_core::skills::{SaveSkillInput, Skill};
use ask_core::sources::{CreateSourceInput, UpdateSourceInput};
use ask_core::tools::default_tool_registry;
use ask_core::watcher::{FileWatcher, WatcherEventKind};
use base64::Engine;
use log::{info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

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
pub struct AgentState {
    /// Map of conversation_id → (cancellation token, task handle).
    pub running: TokioMutex<HashMap<String, (CancellationToken, tokio::task::JoinHandle<()>)>>,
}

/// State for the MCP server manager.
pub struct McpManagerState {
    pub manager: TokioMutex<ask_core::mcp::McpManager>,
}

async fn sync_enabled_mcp_servers(
    db: &Database,
    manager: &mut ask_core::mcp::McpManager,
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

/// Envelope for agent stream events sent to frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentFrontendEvent {
    conversation_id: String,
    #[serde(flatten)]
    event: AgentEvent,
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
                let _ = handle.emit("file-changed", payload);
            }
        }
    });
}

// ── Source Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn add_source(
    state: tauri::State<'_, AppState>,
    kind: String,
    root_path: String,
    include_globs: Vec<String>,
    exclude_globs: Vec<String>,
) -> Result<Source, String> {
    // `kind` is accepted for API compatibility; the core crate currently
    // hardcodes "local_folder" for all sources.
    let _ = kind;
    let input = CreateSourceInput {
        root_path,
        include_globs,
        exclude_globs,
        watch_enabled: false,
    };
    state.db.add_source(input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_sources(state: tauri::State<'_, AppState>) -> Result<Vec<Source>, String> {
    state.db.list_sources().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_source(state: tauri::State<'_, AppState>, source_id: String) -> Result<Source, String> {
    state.db.get_source(&source_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_source(
    state: tauri::State<'_, AppState>,
    source_id: String,
    include_globs: Option<Vec<String>>,
    exclude_globs: Option<Vec<String>>,
    watch_enabled: Option<bool>,
) -> Result<Source, String> {
    let input = UpdateSourceInput {
        include_globs,
        exclude_globs,
        watch_enabled,
    };
    state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_source(state: tauri::State<'_, AppState>, source_id: String) -> Result<(), String> {
    state
        .db
        .delete_source(&source_id)
        .map_err(|e| e.to_string())
}

// ── Ingest Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn scan_source(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    source_id: String,
) -> Result<IngestResult, String> {
    let db = state.db.clone();
    let scan_lock = state.scan_lock.clone();
    let sid = source_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _lock = scan_lock.lock().map_err(|e| format!("scan lock: {e}"))?;
        ingest::scan_source_with_progress(&db, &sid, |progress| {
            let _ = app_handle.emit("source:scan-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    // Invalidate cached answers that may reference this source.
    let _ = state.db.invalidate_cache_for_source(&source_id);

    Ok(result)
}

#[tauri::command]
pub async fn scan_all_sources(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<Vec<IngestResult>, String> {
    let db = state.db.clone();
    let scan_lock = state.scan_lock.clone();
    let results = tokio::task::spawn_blocking(move || {
        let _lock = scan_lock.lock().map_err(|e| format!("scan lock: {e}"))?;
        let sources = db.list_sources().map_err(|e| e.to_string())?;
        let source_count = sources.len();
        let mut results = Vec::with_capacity(source_count);
        for (i, source) in sources.iter().enumerate() {
            let ah = app_handle.clone();
            let sid = source.id.clone();
            let result = ingest::scan_source_with_progress(&db, &source.id, move |progress| {
                let _ = ah.emit(
                    "batch:scan-progress",
                    &BatchProgress {
                        operation: "scan-all".to_string(),
                        source_index: i + 1,
                        source_count,
                        source_id: sid.clone(),
                        phase: progress.phase.clone(),
                        current: progress.current,
                        total: progress.total,
                        current_file: progress.current_file.clone(),
                    },
                );
            })
            .map_err(|e| e.to_string())?;
            results.push(result);
        }
        Ok::<_, String>(results)
    })
    .await
    .map_err(|e| e.to_string())?;

    // Invalidate all cached answers after re-scanning all sources.
    let _ = state.db.clear_answer_cache();

    results
}

// ── Search Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub fn search(
    state: tauri::State<'_, AppState>,
    query_text: String,
    filters: Option<SearchFilters>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<SearchResult, String> {
    let query = SearchQuery {
        text: query_text,
        filters: filters.unwrap_or_default(),
        limit: limit.unwrap_or(20),
        offset: offset.unwrap_or(0),
    };
    let result = search::search(&state.db, &query).map_err(|e| e.to_string())?;

    // Log the query for analytics (best-effort; ignore errors).
    let _ = state.db.log_query(
        &query.text,
        result.total_matches as i32,
        result.search_time_ms as i64,
    );

    Ok(result)
}

#[tauri::command]
pub fn get_evidence_card(
    state: tauri::State<'_, AppState>,
    chunk_id: String,
) -> Result<EvidenceCard, String> {
    search::get_evidence_card(&state.db, &chunk_id).map_err(|e| e.to_string())
}

// ── Index Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_index_stats(state: tauri::State<'_, AppState>) -> Result<IndexStats, String> {
    state.db.get_index_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rebuild_index(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = app_handle.emit(
            "batch:fts-progress",
            &FtsProgress {
                operation: "rebuild-fts".to_string(),
                phase: "running".to_string(),
            },
        );
        let result = db.rebuild_fts_index().map_err(|e| e.to_string());
        let _ = app_handle.emit(
            "batch:fts-progress",
            &FtsProgress {
                operation: "rebuild-fts".to_string(),
                phase: "complete".to_string(),
            },
        );
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Playbook Commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn create_playbook(
    state: tauri::State<'_, AppState>,
    title: String,
    description: String,
    query_text: String,
) -> Result<Playbook, String> {
    state
        .db
        .create_playbook(&title, &description, &query_text)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_playbooks(state: tauri::State<'_, AppState>) -> Result<Vec<Playbook>, String> {
    state.db.list_playbooks().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_playbook(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
) -> Result<Playbook, String> {
    state
        .db
        .get_playbook(&playbook_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_playbook(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
    title: String,
    description: String,
) -> Result<Playbook, String> {
    state
        .db
        .update_playbook(&playbook_id, &title, &description)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_playbook(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_playbook(&playbook_id)
        .map_err(|e| e.to_string())
}

// ── Citation Commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn add_citation(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
    chunk_id: String,
    note: String,
    sort_order: u32,
) -> Result<PlaybookCitation, String> {
    state
        .db
        .add_citation(&playbook_id, &chunk_id, &note, sort_order)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_citations(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
) -> Result<Vec<PlaybookCitation>, String> {
    state
        .db
        .list_citations(&playbook_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_citation(
    state: tauri::State<'_, AppState>,
    citation_id: String,
) -> Result<(), String> {
    state
        .db
        .remove_citation(&citation_id)
        .map_err(|e| e.to_string())
}

// ── Query Log Commands ──────────────────────────────────────────────────

#[tauri::command]
pub fn get_recent_queries(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<QueryLog>, String> {
    state
        .db
        .get_recent_queries(limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_recent_queries(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.db.clear_query_logs().map_err(|e| e.to_string())
}

// ── Answer Cache Commands ───────────────────────────────────────────────

#[tauri::command]
pub fn clear_answer_cache(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    state.db.clear_answer_cache().map_err(|e| e.to_string())
}

// ── Hybrid Search Commands ──────────────────────────────────────────────

#[tauri::command]
pub fn hybrid_search(
    state: tauri::State<'_, AppState>,
    query_text: String,
    filters: Option<SearchFilters>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<SearchResult, String> {
    let query = SearchQuery {
        text: query_text,
        filters: filters.unwrap_or_default(),
        limit: limit.unwrap_or(20),
        offset: offset.unwrap_or(0),
    };
    search::hybrid_search(&state.db, &query).map_err(|e| e.to_string())
}

// ── Embedding Commands ──────────────────────────────────────────────────

#[tauri::command]
pub async fn embed_source(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    source_id: String,
) -> Result<EmbedResult, String> {
    let db = state.db.clone();
    let sid = source_id.clone();
    tokio::task::spawn_blocking(move || {
        ingest::embed_source_with_progress(&db, &sid, |progress| {
            let _ = app_handle.emit("source:scan-progress", &progress);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rebuild_embeddings(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<EmbedResult, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        ingest::rebuild_embeddings_with_progress(&db, |progress| {
            let _ = app_handle.emit("batch:rebuild-progress", &progress);
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

// ── Feedback Commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn add_feedback(
    state: tauri::State<'_, AppState>,
    chunk_id: String,
    query_text: String,
    action: String,
) -> Result<Feedback, String> {
    let feedback_action = match action.as_str() {
        "upvote" => FeedbackAction::Upvote,
        "downvote" => FeedbackAction::Downvote,
        "pin" => FeedbackAction::Pin,
        other => return Err(format!("Invalid feedback action: {other}")),
    };
    state
        .db
        .add_feedback(&chunk_id, &query_text, feedback_action)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_feedback_for_query(
    state: tauri::State<'_, AppState>,
    query_text: String,
) -> Result<Vec<Feedback>, String> {
    state
        .db
        .get_feedback_for_query(&query_text)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_feedback(
    state: tauri::State<'_, AppState>,
    feedback_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_feedback(&feedback_id)
        .map_err(|e| e.to_string())
}

// ── Privacy Commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn get_privacy_config(state: tauri::State<'_, AppState>) -> Result<PrivacyConfig, String> {
    state.db.load_privacy_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_privacy_config(
    state: tauri::State<'_, AppState>,
    config: PrivacyConfig,
) -> Result<(), String> {
    state
        .db
        .save_privacy_config(&config)
        .map_err(|e| e.to_string())
}

// ── Index Commands (extra) ──────────────────────────────────────────────

#[tauri::command]
pub async fn optimize_fts_index(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let _ = app_handle.emit(
            "batch:fts-progress",
            &FtsProgress {
                operation: "optimize-fts".to_string(),
                phase: "running".to_string(),
            },
        );
        let result = db.optimize_fts_index().map_err(|e| e.to_string());
        let _ = app_handle.emit(
            "batch:fts-progress",
            &FtsProgress {
                operation: "optimize-fts".to_string(),
                phase: "complete".to_string(),
            },
        );
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Citation Commands (extra) ───────────────────────────────────────────

#[tauri::command]
pub fn update_citation_note(
    state: tauri::State<'_, AppState>,
    citation_id: String,
    note: String,
) -> Result<(), String> {
    state
        .db
        .update_citation_note(&citation_id, &note)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reorder_citations(
    state: tauri::State<'_, AppState>,
    playbook_id: String,
    citation_ids: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .reorder_citations(&playbook_id, &citation_ids)
        .map_err(|e| e.to_string())
}

// ── Embedder Config Commands ───────────────────────────────────────────

#[tauri::command]
pub fn get_embedder_config_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<EmbedderConfig, String> {
    state.db.get_embedder_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_embedder_config_cmd(
    state: tauri::State<'_, AppState>,
    config: EmbedderConfig,
) -> Result<(), String> {
    state
        .db
        .save_embedder_config(&config)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn test_api_connection_cmd(api_key: String, base_url: String) -> Result<bool, String> {
    ask_core::embed::test_api_connection(&api_key, &base_url).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_local_model_cmd(local_model: Option<String>) -> Result<bool, String> {
    let model = local_model
        .map(|s| LocalEmbeddingModel::from_config_str(&s))
        .unwrap_or_default();
    Ok(ask_core::embed::check_local_model_exists_for(None, &model))
}

#[tauri::command]
pub async fn download_local_model_cmd(
    app_handle: AppHandle,
    local_model: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let model = local_model
            .map(|s| LocalEmbeddingModel::from_config_str(&s))
            .unwrap_or_default();
        ask_core::embed::download_local_model_for_with_progress(None, &model, |progress| {
            let _ = app_handle.emit("model:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ── Image Attachment Commands ───────────────────────────────────────────

/// An image attachment prepared for LLM submission.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub base64_data: String,
    pub media_type: String,
    pub original_name: String,
}

#[tauri::command]
pub async fn prepare_image_attachment(path: String) -> Result<ImageAttachment, String> {
    let file_path = std::path::Path::new(&path);
    if !file_path.exists() {
        return Err(format!("File not found: {path}"));
    }

    let bytes = std::fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let mime = ask_core::parse::detect_mime_type(file_path);

    if !ask_core::media::is_supported_image(&mime) {
        return Err(format!("Unsupported image type: {mime}"));
    }

    let (base64_data, media_type) =
        ask_core::media::prepare_image_for_llm(&bytes, &mime).map_err(|e| e.to_string())?;

    let original_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());

    Ok(ImageAttachment {
        base64_data,
        media_type,
        original_name,
    })
}

// ── File Commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn open_file_in_default_app(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File not found: {path}"));
    }

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn show_in_file_explorer(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File not found: {path}"));
    }

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .args(["/select,", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .args(["-R", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    {
        let parent = p.parent().unwrap_or(p).to_str().unwrap_or(&path);
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ── Watcher Commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn start_watching(
    app_state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
) -> Result<(), String> {
    let source = app_state
        .db
        .get_source(&source_id)
        .map_err(|e| e.to_string())?;
    let path = std::path::Path::new(&source.root_path);
    if !path.exists() {
        return Err(format!("Path does not exist: {}", source.root_path));
    }
    let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
    watcher.watch(path).map_err(|e| e.to_string())?;
    let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
    watched.insert(source_id.clone(), source.root_path.clone());

    // Persist watch_enabled = true in the database.
    let input = UpdateSourceInput {
        include_globs: None,
        exclude_globs: None,
        watch_enabled: Some(true),
    };
    app_state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn stop_watching(
    app_state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
) -> Result<(), String> {
    let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
    if let Some(root_path) = watched.remove(&source_id) {
        let path = std::path::Path::new(&root_path);
        let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
        let _ = watcher.unwatch(path); // best-effort
    }

    // Persist watch_enabled = false in the database.
    let input = UpdateSourceInput {
        include_globs: None,
        exclude_globs: None,
        watch_enabled: Some(false),
    };
    app_state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn get_watcher_status(
    watcher_state: tauri::State<'_, WatcherState>,
) -> Result<Vec<WatchedSourceInfo>, String> {
    let watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
    Ok(watched
        .iter()
        .map(|(id, path)| WatchedSourceInfo {
            source_id: id.clone(),
            root_path: path.clone(),
        })
        .collect())
}

// ── Agent Helpers ───────────────────────────────────────────────────────

/// Map a provider string from the DB to a [`ProviderType`] enum variant.
fn parse_provider_type(s: &str) -> ProviderType {
    match s.to_lowercase().as_str() {
        "openai" | "open_ai" => ProviderType::OpenAi,
        "anthropic" => ProviderType::Anthropic,
        "google" | "gemini" => ProviderType::Google,
        "deepseek" | "deep_seek" => ProviderType::DeepSeek,
        "ollama" => ProviderType::Ollama,
        "lmstudio" | "lm_studio" => ProviderType::LmStudio,
        "azure" | "azure_openai" | "azure_open_ai" | "azureopenai" => ProviderType::AzureOpenAi,
        "zhipu" | "glm" => ProviderType::Zhipu,
        "moonshot" | "kimi" => ProviderType::Moonshot,
        "qwen" | "tongyi" => ProviderType::Qwen,
        "doubao" => ProviderType::Doubao,
        "yi" | "lingyiwanwu" => ProviderType::Yi,
        "baichuan" => ProviderType::Baichuan,
        _ => ProviderType::Custom,
    }
}

/// Convert a DB [`DbAgentConfig`] to a [`ProviderConfig`] suitable for
/// [`create_provider`].
fn db_config_to_provider_config(
    config: &DbAgentConfig,
    timeout_secs: Option<u64>,
) -> ProviderConfig {
    ProviderConfig {
        provider_type: parse_provider_type(&config.provider),
        api_key: Some(config.api_key.clone()),
        base_url: config.base_url.clone(),
        org_id: None,
        timeout_secs,
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
                        .map_or(false, |tcs| tcs.iter().any(|tc| tc.id == tool_id))
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

// ── Conversation Commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn create_conversation_cmd(
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    system_prompt: Option<String>,
) -> Result<Conversation, String> {
    let input = CreateConversationInput {
        provider,
        model,
        system_prompt,
    };
    state
        .db
        .create_conversation(&input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_conversations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Conversation>, String> {
    state.db.list_conversations().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(Conversation, Vec<ConversationMessage>), String> {
    let conv = state.db.get_conversation(&id).map_err(|e| e.to_string())?;
    let msgs = state.db.get_messages(&id).map_err(|e| e.to_string())?;
    Ok((conv, msgs))
}

#[tauri::command]
pub async fn delete_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_conversation(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_conversations_batch_cmd(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, String> {
    state
        .db
        .delete_conversations_batch(&ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_all_conversations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    state
        .db
        .delete_all_conversations()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rename_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    state
        .db
        .update_conversation_title(&id, &title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_title_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<String, String> {
    // 1. Load default agent config for LLM access.
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set.".to_string())?;

    // 2. Load conversation messages.
    let messages = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;

    let first_user = messages.iter().find(|m| m.role == Role::User);
    let first_assistant = messages.iter().find(|m| m.role == Role::Assistant);

    let user_content = match first_user {
        Some(m) => m.content.clone(),
        None => return Err("No user message found.".to_string()),
    };
    let assistant_content = first_assistant.map(|m| m.content.as_str());

    // 3. Create provider and generate title.
    let app_cfg = state.db.load_app_config().unwrap_or_default();
    let provider_config = db_config_to_provider_config(&db_config, Some(app_cfg.llm_timeout_secs));
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let title = ask_core::conversation::generate_title(
        provider.as_ref(),
        &db_config.model,
        &user_content,
        assistant_content,
    )
    .await;

    // 4. Update DB.
    state
        .db
        .update_conversation_title(&conversation_id, &title)
        .map_err(|e| e.to_string())?;

    Ok(title)
}

#[tauri::command]
pub async fn update_conversation_system_prompt_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    system_prompt: String,
) -> Result<(), String> {
    state
        .db
        .update_conversation_system_prompt(&id, &system_prompt)
        .map_err(|e| e.to_string())
}

// ── Conversation Maintenance Commands ────────────────────────────────────

#[tauri::command]
pub async fn get_conversation_stats_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<ConversationStats, String> {
    state.db.get_conversation_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cleanup_empty_conversations_cmd(
    state: tauri::State<'_, AppState>,
    days_old: u32,
) -> Result<usize, String> {
    state
        .db
        .cleanup_empty_conversations(days_old)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn compact_conversation_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    // 1. Load conversation and its messages.
    let conv = state
        .db
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;
    let messages = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    if messages.is_empty() {
        return Ok(());
    }

    // 2. Load default agent config for provider / model.
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set.".to_string())?;

    let app_cfg = state.db.load_app_config().unwrap_or_default();
    let provider_config = db_config_to_provider_config(&db_config, Some(app_cfg.llm_timeout_secs));
    let provider = create_provider(provider_config.clone()).map_err(|e| e.to_string())?;

    let executor_config = ExecutorConfig {
        max_iterations: 1,
        system_prompt: build_system_prompt(Some(&conv.system_prompt), &[]),
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config.max_tokens.map(|t| t as u32),
        context_window: db_config.context_window.map(|w| w as u32),
        reasoning_enabled: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: Some(parse_provider_type(&db_config.provider)),
        summarization_model: db_config.summarization_model.clone(),
        subagent_max_parallel: db_config.subagent_max_parallel.map(|v| v as u32),
        subagent_max_calls_per_turn: db_config.subagent_max_calls_per_turn.map(|v| v as u32),
        subagent_token_budget: db_config.subagent_token_budget.map(|v| v as u32),
        tool_timeout_secs: Some(
            db_config
                .tool_timeout_secs
                .map(|v| v as u32)
                .unwrap_or(app_cfg.tool_timeout_secs as u32),
        ),
        agent_timeout_secs: Some(
            db_config
                .agent_timeout_secs
                .map(|v| v as u32)
                .unwrap_or(app_cfg.agent_timeout_secs as u32),
        ),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: true,
        trace_enabled: true,
        require_tool_confirmation: false,
    };

    let summarization_provider: Option<Box<dyn ask_core::llm::LlmProvider>> =
        if let Some(ref summ_provider_name) = db_config.summarization_provider {
            let summ_config = ProviderConfig {
                provider_type: parse_provider_type(summ_provider_name),
                api_key: Some(db_config.api_key.clone()),
                base_url: db_config.base_url.clone(),
                org_id: None,
                timeout_secs: Some(app_cfg.llm_timeout_secs),
            };
            create_provider(summ_config).ok()
        } else {
            None
        };

    let tools = default_tool_registry();
    let mut executor = AgentExecutor::new(provider, tools, executor_config);
    if let Some(summ_provider) = summarization_provider {
        executor = executor.with_summarization_provider(summ_provider);
    }

    // 3. Run compaction (creates a checkpoint before evicting).
    let compacted = executor
        .compact_conversation(&conversation_id, messages, Some(&state.db), "manual")
        .await
        .map_err(|e| e.to_string())?;

    // 4. Replace messages in DB: delete old, insert compacted.
    state
        .db
        .delete_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    for msg in &compacted {
        state.db.add_message(msg).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn search_conversations_cmd(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ask_core::conversation::ConversationSearchResult>, String> {
    state
        .db
        .search_conversations(&query, limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

// ── Checkpoint Commands ─────────────────────────────────────────────────

#[tauri::command]
pub fn list_checkpoints_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<ask_core::conversation::Checkpoint>, String> {
    state
        .db
        .list_checkpoints(&conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_checkpoint_cmd(
    state: tauri::State<'_, AppState>,
    checkpoint_id: String,
) -> Result<Vec<ConversationMessage>, String> {
    state
        .db
        .restore_checkpoint(&checkpoint_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_checkpoint_cmd(
    state: tauri::State<'_, AppState>,
    checkpoint_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_checkpoint(&checkpoint_id)
        .map_err(|e| e.to_string())
}

// ── Agent Config Commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn set_conversation_sources_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    source_ids: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .set_conversation_sources(&conversation_id, &source_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_conversation_sources_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<String>, String> {
    state
        .db
        .get_linked_sources(&conversation_id)
        .map_err(|e| e.to_string())
}

// ── User Memory Commands ────────────────────────────────────────────────

#[tauri::command]
pub async fn list_user_memories_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ask_core::personalization::UserMemory>, String> {
    state.db.list_user_memories().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_user_memory_cmd(
    state: tauri::State<'_, AppState>,
    content: String,
) -> Result<ask_core::personalization::UserMemory, String> {
    state
        .db
        .create_user_memory(&content)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_user_memory_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    content: String,
) -> Result<ask_core::personalization::UserMemory, String> {
    state
        .db
        .update_user_memory(&id, &content)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_user_memory_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_user_memory(&id).map_err(|e| e.to_string())
}

// ── Agent Config Commands (LLM providers) ───────────────────────────────

#[tauri::command]
pub async fn list_agent_configs_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DbAgentConfig>, String> {
    state.db.list_agent_configs().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_agent_config_cmd(
    state: tauri::State<'_, AppState>,
    config: SaveAgentConfigInput,
) -> Result<DbAgentConfig, String> {
    state
        .db
        .save_agent_config(&config)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_agent_config_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_agent_config(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_default_agent_config_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .set_default_agent_config(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_agent_connection_cmd(
    config: SaveAgentConfigInput,
) -> Result<Vec<String>, String> {
    let provider_config = ProviderConfig {
        provider_type: parse_provider_type(&config.provider),
        api_key: Some(config.api_key.clone()),
        base_url: config.base_url.clone(),
        org_id: None,
        timeout_secs: None,
    };
    let provider = create_provider(provider_config.clone()).map_err(|e| e.to_string())?;
    provider.health_check().await.map_err(|e| e.to_string())?;
    let models = provider.list_models().await.map_err(|e| e.to_string())?;
    Ok(models)
}

// ── Agent Chat Command (streaming) ──────────────────────────────────────

#[tauri::command]
pub async fn agent_chat_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    app_handle: AppHandle,
    conversation_id: String,
    message: String,
    attachments: Option<Vec<ImageAttachment>>,
) -> Result<(), String> {
    // 1. Get default agent config from DB.
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "No default agent config set. Please configure an LLM provider first.".to_string()
        })?;

    // 2. Create LLM provider.
    let app_cfg = state.db.load_app_config().unwrap_or_default();
    let provider_config = db_config_to_provider_config(&db_config, Some(app_cfg.llm_timeout_secs));
    let provider = create_provider(provider_config.clone()).map_err(|e| e.to_string())?;

    // 3. Load conversation history and convert to LLM messages.
    let existing_msgs = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    let history: Vec<Message> = existing_msgs.iter().map(conv_message_to_llm).collect();
    let history = sanitize_tool_call_history(history);
    let next_sort_order = existing_msgs.len() as i64;

    // 4. Save user message to DB.
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: message.clone(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
        token_count: estimate_tokens(&message),
        created_at: String::new(),
        sort_order: next_sort_order,
        thinking: None,
    };
    state.db.add_message(&user_msg).map_err(|e| e.to_string())?;

    // 5. Load conversation to check for custom system prompt.
    let conv = state
        .db
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;
    let source_scope_ids = state
        .db
        .get_linked_sources(&conversation_id)
        .unwrap_or_default();
    let source_scope_section =
        ask_core::conversation::build_source_scope_prompt_section(&state.db, &source_scope_ids)
            .unwrap_or_default();
    let memory_section =
        ask_core::personalization::build_memory_summary_for_query(&state.db, Some(&message))
            .unwrap_or_default();
    let preference_section =
        ask_core::personalization::build_preference_summary_for_query(&state.db, Some(&message))
            .unwrap_or_default();
    let system_prompt = build_system_prompt(
        Some(&conv.system_prompt),
        &[&source_scope_section, &memory_section, &preference_section],
    );

    // 6. Build executor config from DB config.
    let executor_config = ExecutorConfig {
        max_iterations: db_config.max_iterations.map(|v| v as u32).unwrap_or(10),
        system_prompt,
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config.max_tokens.map(|t| t as u32),
        context_window: db_config.context_window.map(|w| w as u32),
        reasoning_enabled: db_config.reasoning_enabled,
        thinking_budget: db_config.thinking_budget.map(|v| v as u32),
        reasoning_effort: db_config
            .reasoning_effort
            .as_ref()
            .and_then(|s| match s.as_str() {
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                _ => None,
            }),
        provider_type: Some(parse_provider_type(&db_config.provider)),
        summarization_model: db_config.summarization_model.clone(),
        subagent_max_parallel: db_config.subagent_max_parallel.map(|v| v as u32),
        subagent_max_calls_per_turn: db_config.subagent_max_calls_per_turn.map(|v| v as u32),
        subagent_token_budget: db_config.subagent_token_budget.map(|v| v as u32),
        tool_timeout_secs: Some(
            db_config
                .tool_timeout_secs
                .map(|v| v as u32)
                .unwrap_or(app_cfg.tool_timeout_secs as u32),
        ),
        agent_timeout_secs: Some(
            db_config
                .agent_timeout_secs
                .map(|v| v as u32)
                .unwrap_or(app_cfg.agent_timeout_secs as u32),
        ),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: true,
        trace_enabled: true,
        require_tool_confirmation: app_cfg.confirm_destructive,
    };

    // 6b. Build confirmation callback if enabled.
    let confirmation_cb: Option<ConfirmationCallback> = if app_cfg.confirm_destructive {
        let dialog_handle = app_handle.clone();
        Some(Arc::new(move |message: String| {
            let handle = dialog_handle.clone();
            Box::pin(async move {
                let (tx, rx) = tokio::sync::oneshot::channel();
                handle
                    .dialog()
                    .message(&message)
                    .title("Confirm Tool Execution")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Allow".into(),
                        "Deny".into(),
                    ))
                    .show(move |confirmed| {
                        let _ = tx.send(confirmed);
                    });
                match tokio::time::timeout(Duration::from_secs(30), rx).await {
                    Ok(Ok(confirmed)) => confirmed,
                    _ => true, // auto-approve on timeout / channel error
                }
            })
        }))
    } else {
        None
    };

    // 6c. Create a separate summarization provider if configured.
    let summarization_provider: Option<Box<dyn ask_core::llm::LlmProvider>> =
        if let Some(ref summ_provider_name) = db_config.summarization_provider {
            let summ_config = ProviderConfig {
                provider_type: parse_provider_type(summ_provider_name),
                api_key: Some(db_config.api_key.clone()),
                base_url: db_config.base_url.clone(),
                org_id: None,
                timeout_secs: Some(app_cfg.llm_timeout_secs),
            };
            create_provider(summ_config).ok()
        } else if db_config.summarization_model.is_some() {
            // Same provider, different model — reuse the main provider config.
            None
        } else {
            None
        };

    // 7. Create tool registry with built-in + MCP tools.
    let mut tools = default_tool_registry();

    // Register MCP tools from currently enabled servers.
    {
        let mut mcp_manager = mcp_state.manager.lock().await;
        match sync_enabled_mcp_servers(&state.db, &mut mcp_manager).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id}: {error}");
                }
            }
            Err(error) => warn!("Failed to load enabled MCP servers: {error}"),
        }
        if let Err(e) = mcp_manager.register_tools(&mut tools).await {
            warn!("Failed to register MCP tools: {e}");
        }
    }

    let delegation_runtime = DelegationRuntime::new(
        provider_config.clone(),
        executor_config.clone(),
        db_config.subagent_allowed_tools.clone(),
        db_config.subagent_allowed_skill_ids.clone(),
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
    delegation_runtime.set_tool_registry(tools.clone());

    // 7b. Build user content parts (text + optional image attachments).
    let vision_supported = model_supports_vision(&provider_config.provider_type, &db_config.model);
    info!(
        "Image attachment vision check: provider={}, model={}, provider_type={:?}, vision_supported={}, has_attachments={}",
        db_config.provider, db_config.model, provider_config.provider_type, vision_supported, attachments.is_some()
    );
    let mut user_parts = vec![ContentPart::Text {
        text: message.clone(),
    }];
    if let Some(atts) = &attachments {
        for att in atts {
            if vision_supported {
                user_parts.push(ContentPart::Image {
                    media_type: att.media_type.clone(),
                    data: att.base64_data.clone(),
                });
            } else {
                // Model doesn't support vision — OCR fallback
                let ocr_config = state.db.load_ocr_config().unwrap_or_default();
                let image_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&att.base64_data)
                    .map_err(|e| format!("Failed to decode image: {}", e))?;
                let ocr_result =
                    extract_text_from_image(&image_bytes, &att.media_type, &ocr_config, None);
                info!(
                    "OCR fallback result for non-vision model: success={}, text_len={}",
                    ocr_result.is_ok(),
                    ocr_result.as_ref().map(|r| r.full_text.len()).unwrap_or(0)
                );
                match ocr_result {
                    Ok(result) if !result.full_text.is_empty() => {
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Image OCR - {}]:\n{}",
                                att.original_name, result.full_text
                            ),
                        });
                    }
                    _ => {
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Image \"{}\" attached but could not be processed — this model does not support image inputs]",
                                att.original_name
                            ),
                        });
                    }
                }
            }
        }
    }

    // 8. Spawn the agent loop in a background task.
    let db = state.db.clone();
    let conv_id = conversation_id.clone();
    let handle = app_handle.clone();
    let assistant_sort_order = next_sort_order + 1;
    let db_config_for_extraction = db_config.clone();

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();
    let turn_timeout_secs = executor_config.agent_timeout_secs.unwrap_or(180) as u64;

    const STREAM_KEEPALIVE_INTERVAL_SECS: u64 = 10;

    let task = tokio::spawn(async move {
        let cancel_token = cancel_token_clone;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);

        // Forward events to the frontend in a separate task.
        let event_handle = handle.clone();
        let stream_conv_id = conv_id.clone();
        let event_forwarder = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let payload = AgentFrontendEvent {
                    conversation_id: stream_conv_id.clone(),
                    event,
                };
                let _ = event_handle.emit("agent:event", &payload);
            }
        });

        // Run the agent.  The executor now saves ALL messages (intermediate
        // tool-call assistants, tool results, and the final answer) to the DB
        // using incrementing sort_order starting at `assistant_sort_order`.
        let executor_cancel_token = cancel_token.clone();
        let mut executor = AgentExecutor::new(provider, tools, executor_config)
            .with_cancel_token(executor_cancel_token);
        if let Some(cb) = confirmation_cb {
            executor = executor.with_confirmation_callback(cb);
        }
        if let Some(summ_provider) = summarization_provider {
            executor = executor.with_summarization_provider(summ_provider);
        }
        let run_future = executor.run(
            history,
            user_parts,
            &db,
            Some(&conv_id),
            tx,
            assistant_sort_order,
        );

        // Keep the frontend stream alive while the agent is still running but
        // the upstream provider is temporarily silent (reasoning, tool work,
        // or SSE gaps). The actual hard stop remains `turn_timeout_secs`.
        let mut run_future = Box::pin(run_future);
        let mut turn_timeout = Box::pin(tokio::time::sleep(Duration::from_secs(turn_timeout_secs)));
        let mut keepalive =
            tokio::time::interval(Duration::from_secs(STREAM_KEEPALIVE_INTERVAL_SECS));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;

        let (result, timed_out) = loop {
            tokio::select! {
                run_result = &mut run_future => break (Some(run_result), false),
                _ = &mut turn_timeout => break (None, true),
                _ = keepalive.tick() => {
                    let payload = AgentFrontendEvent {
                        conversation_id: conv_id.clone(),
                        event: AgentEvent::Thinking {
                            content: String::new(),
                        },
                    };
                    let _ = handle.emit("agent:event", &payload);
                }
            }
        };

        if timed_out {
            cancel_token.cancel();
        }

        drop(run_future);
        drop(turn_timeout);

        // Wait for event forwarder to finish.
        let _ = event_forwarder.await;

        match result {
            Some(Ok(_)) => {}
            Some(Err(ref e)) => {
                warn!("Agent execution failed for conversation {conv_id}: {e}");
                let payload = AgentFrontendEvent {
                    conversation_id: conv_id.clone(),
                    event: AgentEvent::Error {
                        message: "Agent execution failed unexpectedly.".to_string(),
                    },
                };
                let _ = handle.emit("agent:event", payload);
            }
            None => {
                warn!("Agent execution timed out for conversation {conv_id}");
                let payload = AgentFrontendEvent {
                    conversation_id: conv_id.clone(),
                    event: AgentEvent::Error {
                        message: "Agent execution timed out.".to_string(),
                    },
                };
                let _ = handle.emit("agent:event", payload);
            }
        }

        // Repair orphaned tool_calls in DB after timeout or error.
        if !matches!(result, Some(Ok(_))) {
            repair_orphaned_tool_calls(&db, &conv_id);
        }

        // Auto memory extraction (background, best-effort).
        if matches!(result, Some(Ok(_))) {
            let app_cfg = db.load_app_config().unwrap_or_default();
            if app_cfg.auto_memory_extraction {
                // Determine the model: prefer summarization model, fall back to main.
                let extract_model = db_config_for_extraction
                    .summarization_model
                    .as_deref()
                    .unwrap_or(&db_config_for_extraction.model);
                // Build a provider for extraction (reuse summarization provider config or main).
                let extract_provider_config =
                    if let Some(ref sp) = db_config_for_extraction.summarization_provider {
                        ProviderConfig {
                            provider_type: parse_provider_type(sp),
                            api_key: Some(db_config_for_extraction.api_key.clone()),
                            base_url: db_config_for_extraction.base_url.clone(),
                            org_id: None,
                            timeout_secs: Some(app_cfg.llm_timeout_secs),
                        }
                    } else {
                        ProviderConfig {
                            provider_type: parse_provider_type(&db_config_for_extraction.provider),
                            api_key: Some(db_config_for_extraction.api_key.clone()),
                            base_url: db_config_for_extraction.base_url.clone(),
                            org_id: None,
                            timeout_secs: Some(app_cfg.llm_timeout_secs),
                        }
                    };
                if let Ok(extract_llm) = create_provider(extract_provider_config) {
                    match ask_core::personalization::auto_extract_and_save(
                        &db,
                        &conv_id,
                        extract_llm.as_ref(),
                        extract_model,
                    )
                    .await
                    {
                        Ok(n) if n > 0 => {
                            info!("Auto-extracted {n} memories from conversation {conv_id}");
                        }
                        Err(e) => {
                            warn!("Auto memory extraction failed for {conv_id}: {e}");
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // 8. Track the running task for potential cancellation.
    {
        let mut running = agent_state.running.lock().await;
        // Cancel any existing task for this conversation.
        if let Some((prev_token, prev_task)) = running.remove(&conversation_id) {
            prev_token.cancel();
            prev_task.abort();
        }
        running.insert(conversation_id, (cancel_token, task));
    }

    Ok(())
}

// ── Model Context Window ─────────────────────────────────────────────────

#[tauri::command]
pub fn get_model_context_window(model: String) -> u32 {
    ask_core::conversation::memory::model_context_window(&model)
}

// ── Agent Stop Command ──────────────────────────────────────────────────

#[tauri::command]
pub async fn agent_stop_cmd(
    agent_state: tauri::State<'_, AgentState>,
    conversation_id: String,
) -> Result<(), String> {
    let mut running = agent_state.running.lock().await;
    if let Some((token, task)) = running.remove(&conversation_id) {
        // Signal cooperative cancellation first so the agent can save
        // partial work, then abort the task as a fallback.
        token.cancel();
        // Give cooperative cancellation 2 seconds to save partial state
        // before forcibly aborting the task.
        let abort_task = task;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            abort_task.abort();
        });
    }
    Ok(())
}

// ── App Config ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_app_config_cmd(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    state.db.load_app_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_app_config_cmd(
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    state.db.save_app_config(&config).map_err(|e| e.to_string())
}

// ── OCR ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_ocr_config_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<ask_core::ocr::OcrConfig, String> {
    state.db.load_ocr_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_ocr_config_cmd(
    state: tauri::State<'_, AppState>,
    config: ask_core::ocr::OcrConfig,
) -> Result<(), String> {
    state.db.save_ocr_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_ocr_models_cmd(config: ask_core::ocr::OcrConfig) -> bool {
    ask_core::ocr::check_ocr_models_exist(&config)
}

#[tauri::command]
pub async fn download_ocr_models_cmd(
    app_handle: AppHandle,
    config: ask_core::ocr::OcrConfig,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ask_core::ocr::download_ocr_models(&config, |progress| {
            let _ = app_handle.emit("ocr:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ── Video ───────────────────────────────────────────────────────────

#[cfg(feature = "video")]
#[tauri::command]
pub fn get_video_config_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<ask_core::video::VideoConfig, String> {
    state.db.load_video_config().map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn save_video_config_cmd(
    state: tauri::State<'_, AppState>,
    config: ask_core::video::VideoConfig,
) -> Result<(), String> {
    state
        .db
        .save_video_config(&config)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn check_whisper_model_cmd(config: ask_core::video::VideoConfig) -> bool {
    ask_core::video::check_whisper_model_exists(&config)
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn download_whisper_model_cmd(
    app_handle: AppHandle,
    config: ask_core::video::VideoConfig,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ask_core::video::download_whisper_model(&config, |progress| {
            let _ = app_handle.emit("video:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn check_ffmpeg_cmd(config: ask_core::video::VideoConfig) -> Result<bool, String> {
    ask_core::video::check_ffmpeg(&config).map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn download_ffmpeg_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let data_dir = app_handle
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to get data dir: {e}"))?;
    let db = state.db.clone();

    let path = tokio::task::spawn_blocking(move || {
        ask_core::video::download_ffmpeg(&data_dir, |progress| {
            let _ = app_handle.emit("ffmpeg:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;

    let path_str = path.to_string_lossy().to_string();

    // Auto-save ffmpeg path to config
    let mut config = db.load_video_config().map_err(|e| e.to_string())?;
    config.ffmpeg_path = Some(path_str.clone());
    db.save_video_config(&config).map_err(|e| e.to_string())?;

    Ok(path_str)
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn delete_whisper_model_cmd(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if state.whisper_busy.load(Ordering::SeqCst) {
        return Err("Cannot delete model while transcription is in progress".into());
    }
    let config = state.db.load_video_config().map_err(|e| e.to_string())?;
    ask_core::video::delete_whisper_model(&config).map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn transcribe_audio_buffer_cmd(
    audio_data: Vec<u8>,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let db = state.db.clone();
    let whisper_busy = state.whisper_busy.clone();

    tokio::task::spawn_blocking(move || {
        if whisper_busy.load(Ordering::SeqCst) {
            return Err("Transcription already in progress".into());
        }

        let config = db.load_video_config().map_err(|e| e.to_string())?;

        let temp_dir = std::env::temp_dir().join("ask-myself-voice");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        let wav_path = temp_dir.join(format!("voice-{}.wav", Uuid::new_v4()));
        std::fs::write(&wav_path, &audio_data).map_err(|e| e.to_string())?;

        whisper_busy.store(true, Ordering::SeqCst);
        struct Guard(Arc<AtomicBool>, PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
                let _ = std::fs::remove_file(&self.1);
            }
        }
        let _guard = Guard(whisper_busy, wav_path.clone());

        let segments =
            ask_core::video::transcribe_audio(&wav_path, &config).map_err(|e| e.to_string())?;

        let text = segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(text)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptChunk {
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub chunk_type: String,
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn analyze_video_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();
    let whisper_busy = state.whisper_busy.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &path)?;

    tokio::task::spawn_blocking(move || {
        let config = db.load_video_config().map_err(|e| e.to_string())?;
        let file_path = std::path::Path::new(&path);
        if !file_path.is_file() {
            return Err(format!("File not found: {path}"));
        }

        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Set whisper_busy guard; ensure it resets even on panic.
        whisper_busy.store(true, Ordering::SeqCst);
        struct WhisperGuard(Arc<AtomicBool>);
        impl Drop for WhisperGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = WhisperGuard(whisper_busy);

        let ah = app_handle.clone();
        let fname = file_name.clone();
        let result = ask_core::video::analyze_video(file_path, &config, move |progress| {
            let _ = ah.emit(
                "video:processing-progress",
                serde_json::json!({
                    "progress": progress.progress_pct,
                    "phase": progress.phase,
                    "detail": progress.detail,
                    "fileName": &fname,
                }),
            );
        })
        .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "transcript": result.full_transcript,
            "segmentCount": result.transcript_segments.len(),
            "durationSecs": result.duration_secs,
            "frameTextsCount": result.frame_texts.len(),
            "thumbnailPath": result.thumbnail_path.map(|p| p.to_string_lossy().to_string()),
            "metadata": result.metadata,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn get_video_transcript_cmd(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> Result<Vec<TranscriptChunk>, String> {
    let db = state.db.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &file_path)?;

    tokio::task::spawn_blocking(move || {
        let conn = db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT c.content, c.start_offset, c.end_offset, c.metadata_json
                 FROM chunks c
                 JOIN documents d ON d.id = c.document_id
                 WHERE d.path = ?1
                 ORDER BY c.chunk_index",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(rusqlite::params![&file_path], |row| {
                let content: String = row.get(0)?;
                let start: i64 = row.get(1)?;
                let end: i64 = row.get(2)?;
                let meta_json: String = row.get(3)?;
                Ok((content, start, end, meta_json))
            })
            .map_err(|e| e.to_string())?;

        let mut chunks = Vec::new();
        for row in rows {
            let (text, start_ms, end_ms, meta_json) = row.map_err(|e| e.to_string())?;
            let heading: Option<String> = serde_json::from_str::<serde_json::Value>(&meta_json)
                .ok()
                .and_then(|v| {
                    v.get("heading_context")
                        .and_then(|h| h.as_str().map(String::from))
                });
            let chunk_type = if heading
                .as_deref()
                .map_or(false, |h| h.starts_with("[Frame OCR"))
            {
                "frame_ocr"
            } else {
                "transcript"
            };
            chunks.push(TranscriptChunk {
                text,
                start_ms: Some(start_ms),
                end_ms: Some(end_ms),
                chunk_type: chunk_type.to_string(),
            });
        }

        Ok(chunks)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn get_video_metadata_cmd(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &file_path)?;

    tokio::task::spawn_blocking(move || {
        let conn = db.conn();
        let result: Result<(String, String), _> = conn.query_row(
            "SELECT mime_type, metadata FROM documents WHERE path = ?1",
            rusqlite::params![&file_path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((mime_type, metadata_json)) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&metadata_json).unwrap_or(serde_json::json!({}));
                Ok(serde_json::json!({
                    "mimeType": mime_type,
                    "durationSecs": meta.get("duration_secs").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))),
                    "width": meta.get("video_width").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))),
                    "height": meta.get("video_height").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))),
                    "codec": meta.get("video_codec").and_then(|v| v.as_str()),
                    "framerate": meta.get("video_framerate").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))),
                    "thumbnailPath": meta.get("thumbnail_path").and_then(|v| v.as_str()),
                    "creationTime": meta.get("video_creation_time").and_then(|v| v.as_str()),
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(format!("No document found for path: {file_path}"))
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ── Skills Commands ─────────────────────────────────────────────────

#[tauri::command]
pub async fn list_skills_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<Skill>, String> {
    state.db.list_skills().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_skill_cmd(
    state: tauri::State<'_, AppState>,
    input: SaveSkillInput,
) -> Result<Skill, String> {
    state.db.save_skill(&input).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_skill_cmd(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_skill(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_skill_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_skill(&id, enabled)
        .map_err(|e| e.to_string())
}

// ── MCP Commands ────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_mcp_servers_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<McpServer>, String> {
    state.db.list_mcp_servers().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    input: SaveMcpServerInput,
) -> Result<McpServer, String> {
    let saved = state
        .db
        .save_mcp_server(&input)
        .map_err(|e| e.to_string())?;
    let mut manager = mcp_state.manager.lock().await;
    match sync_enabled_mcp_servers(&state.db, &mut manager).await {
        Ok(errors) => {
            for (server_id, error) in errors {
                warn!("Failed to sync MCP server {server_id} after save: {error}");
            }
        }
        Err(error) => warn!("Failed to refresh enabled MCP servers after save: {error}"),
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_mcp_server(&id).map_err(|e| e.to_string())?;
    let mut manager = mcp_state.manager.lock().await;
    manager.disconnect_server(&id).await.ok();
    Ok(())
}

#[tauri::command]
pub async fn toggle_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_mcp_server(&id, enabled)
        .map_err(|e| e.to_string())?;

    let mut manager = mcp_state.manager.lock().await;
    if enabled {
        match sync_enabled_mcp_servers(&state.db, &mut manager).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id} after enable: {error}");
                }
            }
            Err(error) => warn!("Failed to refresh enabled MCP servers after enable: {error}"),
        }
    } else {
        manager.disconnect_server(&id).await.ok();
    }

    Ok(())
}

#[tauri::command]
pub async fn test_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
) -> Result<Vec<McpToolInfo>, String> {
    let servers = state.db.list_mcp_servers().map_err(|e| e.to_string())?;
    let server = servers
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("MCP server {id} not found"))?;
    let mut manager = mcp_state.manager.lock().await;
    // connect_server stores the client so list_mcp_tools_cmd can reuse it.
    let app_cfg = state.db.load_app_config().unwrap_or_default();
    let tools = manager
        .connect_server(&server, Some(app_cfg.mcp_call_timeout_secs))
        .await
        .map_err(|e| e.to_string())?;
    // For built-in managed servers that aren't enabled, disconnect after
    // testing to stop the managed process.
    if server.builtin_id.is_some() && !server.enabled {
        let _ = manager.disconnect_server(&server.id).await;
    }
    Ok(tools)
}

#[tauri::command]
pub async fn test_mcp_server_direct_cmd(
    mcp_state: tauri::State<'_, McpManagerState>,
    name: String,
    transport: String,
    command: Option<String>,
    args: Option<String>,
    url: Option<String>,
    env_json: Option<String>,
    headers_json: Option<String>,
) -> Result<Vec<McpToolInfo>, String> {
    let server = McpServer {
        id: "__test__".to_string(),
        name,
        transport,
        command,
        args,
        url,
        env_json,
        headers_json,
        enabled: true,
        created_at: String::new(),
        updated_at: String::new(),
        builtin_id: None,
    };
    let mut manager = mcp_state.manager.lock().await;
    let tools = manager
        .connect_server(&server, None)
        .await
        .map_err(|e| e.to_string())?;
    manager.disconnect_server("__test__").await.ok();
    Ok(tools)
}

#[tauri::command]
pub async fn list_mcp_tools_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    server_id: String,
) -> Result<Vec<McpToolInfo>, String> {
    let mut manager = mcp_state.manager.lock().await;
    // If already connected, list tools from existing client.
    if let Some(client) = manager.get_client(&server_id) {
        let mut guard = client.lock().await;
        return guard.list_tools().await.map_err(|e| e.to_string());
    }
    // Otherwise, connect first.
    let servers = state.db.list_mcp_servers().map_err(|e| e.to_string())?;
    let server = servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("MCP server {server_id} not found"))?;
    let app_cfg = state.db.load_app_config().unwrap_or_default();
    manager
        .connect_server(&server, Some(app_cfg.mcp_call_timeout_secs))
        .await
        .map_err(|e| e.to_string())
}

// ── Agent Trace Analytics ──────────────────────────────────────────────

#[tauri::command]
pub fn get_trace_summary(
    state: tauri::State<'_, AppState>,
) -> Result<ask_core::trace::TraceSummary, String> {
    state.db.get_trace_summary().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recent_traces(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<ask_core::trace::AgentTrace>, String> {
    state
        .db
        .get_recent_traces(limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}
