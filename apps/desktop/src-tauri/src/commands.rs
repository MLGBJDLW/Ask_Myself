use ask_core::db::Database;
use ask_core::embed::EmbedderConfig;
use ask_core::feedback::{Feedback, FeedbackAction};
use ask_core::index::IndexStats;
use ask_core::ingest::{self, EmbedResult, IngestResult};
use ask_core::models::{
    EvidenceCard, Playbook, PlaybookCitation, SearchFilters, SearchQuery, Source,
};
use ask_core::playbook::QueryLog;
use ask_core::privacy::PrivacyConfig;
use ask_core::search::{self, SearchResult};
use ask_core::sources::{CreateSourceInput, UpdateSourceInput};

/// Application state holding the database connection.
pub struct AppState {
    pub db: Database,
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
