use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ask_core::agent::{AgentConfig as ExecutorConfig, AgentEvent, AgentExecutor};
use ask_core::conversation::{
    AgentConfig as DbAgentConfig, Conversation, ConversationMessage,
    CreateConversationInput, SaveAgentConfigInput,
};
use ask_core::db::Database;
use ask_core::embed::EmbedderConfig;
use ask_core::feedback::{Feedback, FeedbackAction};
use ask_core::index::IndexStats;
use ask_core::ingest::{self, EmbedResult, IngestResult};
use ask_core::llm::{create_provider, Message, ProviderConfig, ProviderType, Role};
use ask_core::models::{
    EvidenceCard, Playbook, PlaybookCitation, SearchFilters, SearchQuery, Source,
};
use ask_core::playbook::QueryLog;
use ask_core::privacy::PrivacyConfig;
use ask_core::search::{self, SearchResult};
use ask_core::sources::{CreateSourceInput, UpdateSourceInput};
use ask_core::tools::default_tool_registry;
use ask_core::watcher::{FileWatcher, WatcherEventKind};
use log::{info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

/// Application state holding the database connection.
pub struct AppState {
    pub db: Arc<Database>,
}

/// State for tracking running agent tasks (for cancellation).
pub struct AgentState {
    /// Map of conversation_id → running agent task handle.
    pub running: TokioMutex<HashMap<String, tokio::task::JoinHandle<()>>>,
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

/// Initialise the file watcher, start watching all sources with
/// `watch_enabled = true`, and spawn a background thread that processes
/// file-change events (debounced, auto-scan, emit to frontend).
pub fn init_watcher(
    app_handle: tauri::AppHandle,
    db: &Database,
) {
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
        // source_id → (last_event_time, paths_removed)
        let mut pending: HashMap<String, (Instant, HashSet<PathBuf>)> = HashMap::new();

        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(event) => {
                    // Find which watched source owns this path.
                    let ws = match handle.try_state::<WatcherState>() {
                        Some(s) => s,
                        None => continue,
                    };
                    let watched = ws.watched.lock().unwrap();
                    let matched: Option<&String> = watched.iter()
                        .find(|(_, root)| event.path.starts_with(root.as_str()))
                        .map(|(sid, _)| sid);
                    if let Some(sid) = matched {
                        let sid = sid.clone();
                        drop(watched);
                        let entry = pending
                            .entry(sid)
                            .or_insert_with(|| (Instant::now(), HashSet::new()));
                        entry.0 = Instant::now();
                        if event.kind == WatcherEventKind::Removed {
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
                .filter(|(_, (ts, _))| now.duration_since(*ts) >= debounce)
                .map(|(sid, _)| sid.clone())
                .collect();

            for source_id in ready {
                let (_ts, removed_paths) = pending.remove(&source_id).unwrap();
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

                // Re-scan the source to pick up created/modified files.
                info!("Auto-scanning source {source_id} due to file changes");
                match ingest::scan_source(&app_state.db, &source_id) {
                    Ok(result) => {
                        let payload = serde_json::json!({
                            "sourceId": source_id,
                            "filesScanned": result.files_scanned,
                            "filesAdded": result.files_added,
                            "filesUpdated": result.files_updated,
                            "filesRemoved": removed_paths.len(),
                        });
                        let _ = handle.emit("file-changed", payload);
                    }
                    Err(e) => {
                        warn!("Auto-scan failed for source {source_id}: {e}");
                    }
                }
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
pub fn get_source(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<Source, String> {
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
pub fn delete_source(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_source(&source_id)
        .map_err(|e| e.to_string())
}

// ── Ingest Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub fn scan_source(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<IngestResult, String> {
    ingest::scan_source(&state.db, &source_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn scan_all_sources(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<IngestResult>, String> {
    let sources = state.db.list_sources().map_err(|e| e.to_string())?;
    let mut results = Vec::with_capacity(sources.len());
    for source in &sources {
        let result =
            ingest::scan_source(&state.db, &source.id).map_err(|e| e.to_string())?;
        results.push(result);
    }
    Ok(results)
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
pub fn get_index_stats(
    state: tauri::State<'_, AppState>,
) -> Result<IndexStats, String> {
    state.db.get_index_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rebuild_index(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.db.rebuild_fts_index().map_err(|e| e.to_string())
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
pub fn list_playbooks(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Playbook>, String> {
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

// ── Hybrid Search Commands ──────────────────────────────────────────────

#[tauri::command]
pub fn hybrid_search(
    state: tauri::State<'_, AppState>,
    query_text: String,
    filters: Option<SearchFilters>,
) -> Result<SearchResult, String> {
    let query = SearchQuery {
        text: query_text,
        filters: filters.unwrap_or_default(),
        limit: 20,
        offset: 0,
    };
    search::hybrid_search(&state.db, &query).map_err(|e| e.to_string())
}

// ── Embedding Commands ──────────────────────────────────────────────────

#[tauri::command]
pub fn embed_source(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<EmbedResult, String> {
    ingest::embed_source(&state.db, &source_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rebuild_embeddings(
    state: tauri::State<'_, AppState>,
) -> Result<EmbedResult, String> {
    ingest::rebuild_embeddings(&state.db).map_err(|e| e.to_string())
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
pub fn get_privacy_config(
    state: tauri::State<'_, AppState>,
) -> Result<PrivacyConfig, String> {
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
pub fn optimize_fts_index(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.db.optimize_fts_index().map_err(|e| e.to_string())
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
pub fn test_api_connection_cmd(
    api_key: String,
    base_url: String,
) -> Result<bool, String> {
    ask_core::embed::test_api_connection(&api_key, &base_url).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_local_model_cmd() -> Result<bool, String> {
    Ok(ask_core::embed::check_local_model_exists(None))
}

#[tauri::command]
pub fn download_local_model_cmd() -> Result<(), String> {
    ask_core::embed::download_local_model(None).map_err(|e| e.to_string())
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
        let parent = p
            .parent()
            .unwrap_or(p)
            .to_str()
            .unwrap_or(&path);
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
    let source = app_state.db.get_source(&source_id).map_err(|e| e.to_string())?;
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
        _ => ProviderType::Custom,
    }
}

/// Convert a DB [`DbAgentConfig`] to a [`ProviderConfig`] suitable for
/// [`create_provider`].
fn db_config_to_provider_config(config: &DbAgentConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: parse_provider_type(&config.provider),
        api_key: Some(config.api_key.clone()),
        base_url: config.base_url.clone(),
        org_id: None,
    }
}

/// Convert a DB [`ConversationMessage`] to an LLM [`Message`].
fn conv_message_to_llm(msg: &ConversationMessage) -> Message {
    Message {
        role: msg.role.clone(),
        content: msg.content.clone(),
        name: msg.tool_call_id.clone(),
        tool_calls: if msg.tool_calls.is_empty() {
            None
        } else {
            Some(msg.tool_calls.clone())
        },
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
    state
        .db
        .delete_conversation(&id)
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
    state
        .db
        .delete_agent_config(&id)
        .map_err(|e| e.to_string())
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
    };
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;
    provider
        .health_check()
        .await
        .map_err(|e| e.to_string())?;
    let models = provider
        .list_models()
        .await
        .map_err(|e| e.to_string())?;
    Ok(models)
}

// ── Agent Chat Command (streaming) ──────────────────────────────────────

#[tauri::command]
pub async fn agent_chat_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    app_handle: AppHandle,
    conversation_id: String,
    message: String,
) -> Result<(), String> {
    // 1. Get default agent config from DB.
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set. Please configure an LLM provider first.".to_string())?;

    // 2. Create LLM provider.
    let provider_config = db_config_to_provider_config(&db_config);
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    // 3. Load conversation history and convert to LLM messages.
    let existing_msgs = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    let history: Vec<Message> = existing_msgs.iter().map(conv_message_to_llm).collect();
    let next_sort_order = existing_msgs.len() as i64;

    // 4. Save user message to DB.
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: message.clone(),
        tool_call_id: None,
        tool_calls: vec![],
        token_count: (message.len() / 4) as u32,
        created_at: String::new(),
        sort_order: next_sort_order,
    };
    state
        .db
        .add_message(&user_msg)
        .map_err(|e| e.to_string())?;

    // 5. Build executor config from DB config.
    let executor_config = ExecutorConfig {
        max_iterations: 10,
        system_prompt: ExecutorConfig::default().system_prompt,
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config.max_tokens.map(|t| t as u32),
        context_window: db_config.context_window.map(|w| w as u32),
    };

    // 6. Create tool registry.
    let tools = default_tool_registry();

    // 7. Spawn the agent loop in a background task.
    let db = state.db.clone();
    let conv_id = conversation_id.clone();
    let handle = app_handle.clone();
    let assistant_sort_order = next_sort_order + 1;

    let task = tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);

        // Forward events to the frontend in a separate task.
        let event_handle = handle.clone();
        let event_forwarder = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let _ = event_handle.emit("agent:event", &event);
            }
        });

        // Run the agent.
        let executor = AgentExecutor::new(provider, tools, executor_config);
        let result = executor.run(history, message, &db, Some(&conv_id), tx).await;

        // Wait for event forwarder to finish.
        let _ = event_forwarder.await;

        // Save assistant message to DB.
        match result {
            Ok(assistant_msg) => {
                let conv_msg = ConversationMessage {
                    id: Uuid::new_v4().to_string(),
                    conversation_id: conv_id,
                    role: Role::Assistant,
                    content: assistant_msg.content.clone(),
                    tool_call_id: None,
                    tool_calls: assistant_msg
                        .tool_calls
                        .unwrap_or_default(),
                    token_count: (assistant_msg.content.len() / 4) as u32,
                    created_at: String::new(),
                    sort_order: assistant_sort_order,
                };
                if let Err(e) = db.add_message(&conv_msg) {
                    warn!("Failed to save assistant message: {e}");
                }
            }
            Err(e) => {
                warn!("Agent execution failed for conversation {conv_id}: {e}");
            }
        }
    });

    // 8. Track the running task for potential cancellation.
    {
        let mut running = agent_state.running.lock().await;
        // Cancel any existing task for this conversation.
        if let Some(prev) = running.remove(&conversation_id) {
            prev.abort();
        }
        running.insert(conversation_id, task);
    }

    Ok(())
}

// ── Agent Stop Command ──────────────────────────────────────────────────

#[tauri::command]
pub async fn agent_stop_cmd(
    agent_state: tauri::State<'_, AgentState>,
    conversation_id: String,
) -> Result<(), String> {
    let mut running = agent_state.running.lock().await;
    if let Some(task) = running.remove(&conversation_id) {
        task.abort();
    }
    Ok(())
}
