use super::*;

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
pub async fn list_source_tree_cmd(
    state: tauri::State<'_, AppState>,
    source_id: String,
    relative_path: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
) -> Result<SourceTree, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        nexa_core::source_tree::list_source_tree(
            &db,
            &source_id,
            relative_path.as_deref(),
            depth,
            limit,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn update_source(
    state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
    root_path: Option<String>,
    include_globs: Option<Vec<String>>,
    exclude_globs: Option<Vec<String>>,
    watch_enabled: Option<bool>,
) -> Result<Source, String> {
    let previous = state.db.get_source(&source_id).map_err(|e| e.to_string())?;
    let input = UpdateSourceInput {
        root_path,
        include_globs,
        exclude_globs,
        watch_enabled,
    };
    let updated = state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())?;
    watcher_state.revision.fetch_add(1, Ordering::AcqRel);

    let root_changed = previous.root_path != updated.root_path;
    let watch_changed = previous.watch_enabled != updated.watch_enabled;
    if root_changed || watch_changed {
        let was_watched = {
            let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
            watched.remove(&source_id).is_some()
        };
        if was_watched {
            let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
            let _ = watcher.unwatch(std::path::Path::new(&previous.root_path));
        }
        if updated.watch_enabled {
            {
                let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
                watcher
                    .watch(std::path::Path::new(&updated.root_path))
                    .map_err(|e| e.to_string())?;
            }
            let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
            watched.insert(source_id.clone(), updated.root_path.clone());
        }
    }

    Ok(updated)
}

#[tauri::command]
pub fn delete_source(
    state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
) -> Result<(), String> {
    let previous = state.db.get_source(&source_id).map_err(|e| e.to_string())?;
    state
        .db
        .delete_source(&source_id)
        .map_err(|e| e.to_string())?;
    watcher_state.revision.fetch_add(1, Ordering::AcqRel);

    let was_watched = {
        let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
        watched.remove(&source_id).is_some()
    };
    if was_watched {
        let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
        let _ = watcher.unwatch(std::path::Path::new(&previous.root_path));
    }
    Ok(())
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
    let progress_handle = app_handle.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _lock = scan_lock.lock().map_err(|e| format!("scan lock: {e}"))?;
        ingest::scan_source_with_progress(&db, &sid, |progress| {
            emit_app_event(&progress_handle, "source:scan-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    emit_file_changed_after_scan(&app_handle, &result);

    Ok(result)
}

#[tauri::command]
pub async fn scan_all_sources(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<Vec<IngestResult>, String> {
    let db = state.db.clone();
    let scan_lock = state.scan_lock.clone();
    let progress_handle = app_handle.clone();
    let results = tokio::task::spawn_blocking(move || {
        let _lock = scan_lock.lock().map_err(|e| format!("scan lock: {e}"))?;
        let sources = db.list_sources().map_err(|e| e.to_string())?;
        let source_count = sources.len();
        let mut results = Vec::with_capacity(source_count);
        for (i, source) in sources.iter().enumerate() {
            let ah = progress_handle.clone();
            let sid = source.id.clone();
            let result = ingest::scan_source_with_progress(&db, &source.id, move |progress| {
                emit_app_event(
                    &ah,
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
    .map_err(|e| e.to_string())??;

    for result in &results {
        emit_file_changed_after_scan(&app_handle, result);
    }

    Ok(results)
}

fn emit_file_changed_after_scan(app_handle: &AppHandle, result: &IngestResult) {
    if result.files_added == 0 && result.files_updated == 0 && result.files_purged == 0 {
        return;
    }

    emit_app_event(
        app_handle,
        "file-changed",
        &serde_json::json!({
            "sourceId": result.source_id,
            "filesAdded": result.files_added,
            "filesUpdated": result.files_updated,
            "filesRemoved": result.files_purged,
        }),
    );
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

#[tauri::command]
pub fn get_evidence_cards(
    state: tauri::State<'_, AppState>,
    chunk_ids: Vec<String>,
) -> Result<Vec<EvidenceCard>, String> {
    search::get_evidence_cards(&state.db, &chunk_ids).map_err(|e| e.to_string())
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
        emit_app_event(
            &app_handle,
            "batch:fts-progress",
            &FtsProgress {
                operation: "rebuild-fts".to_string(),
                phase: "running".to_string(),
            },
        );
        let result = db.rebuild_fts_index().map_err(|e| e.to_string());
        emit_app_event(
            &app_handle,
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
    let scan_lock = state.scan_lock.clone();
    let progress: Arc<dyn Fn(ingest::ScanProgress) + Send + Sync> = Arc::new(move |progress| {
        emit_app_event(&app_handle, "source:scan-progress", &progress);
    });
    let control = state
        .background_work
        .cooperative_embedding_control(Some(progress));
    tokio::task::spawn_blocking(move || {
        let _scan_guard = scan_lock.lock().unwrap_or_else(|error| error.into_inner());
        nexa_core::embedding_job::run_source(
            &db,
            &sid,
            nexa_core::embedding_job::EmbeddingJobLimits::default(),
            &control,
        )
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
    let scan_lock = state.scan_lock.clone();
    let progress: Arc<dyn Fn(ingest::ScanProgress) + Send + Sync> = Arc::new(move |progress| {
        emit_app_event(&app_handle, "batch:rebuild-progress", &progress);
    });
    let control = state
        .background_work
        .cooperative_embedding_control(Some(progress));
    tokio::task::spawn_blocking(move || {
        let _scan_guard = scan_lock.lock().unwrap_or_else(|error| error.into_inner());
        nexa_core::embedding_job::rebuild_all(
            &db,
            nexa_core::embedding_job::EmbeddingJobLimits::default(),
            &control,
        )
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

// ── Message-Level Feedback (learning loop) ─────────────────────────────

/// Persist thumbs up/down on a specific assistant message, and on upvote
/// capture a distilled (user_query → response) "learned success" whose
/// embedding is computed in the background.
///
/// `rating` semantics: `+1` = upvote, `-1` = downvote, `0` = clear.
#[tauri::command]
pub async fn set_message_feedback_cmd(
    state: tauri::State<'_, AppState>,
    message_id: String,
    conversation_id: String,
    rating: i32,
    note: Option<String>,
) -> Result<nexa_core::learning::MessageFeedback, String> {
    let fb = state
        .db
        .set_message_feedback(&message_id, &conversation_id, rating, note.as_deref())
        .map_err(|e| e.to_string())?;

    if rating > 0 {
        // Distill preceding user message + this assistant response into a
        // learned-success row. Failures here are non-fatal — we still
        // return the saved feedback.
        if let Err(err) = spawn_learned_success_capture(state.db.clone(), message_id.clone()) {
            warn!("Failed to capture learned success: {err}");
        }
    }

    Ok(fb)
}

/// Create the learned-success row synchronously, then fire-and-forget a
/// background task that (a) optionally LLM-distills the response via the
/// configured summarization provider/model and (b) populates the embedding.
fn spawn_learned_success_capture(
    db: Arc<Database>,
    assistant_message_id: String,
) -> Result<(), String> {
    // Resolve the (user_query, response) pair on the caller thread so we
    // can fail fast if the message disappeared.
    let assistant = db
        .get_message_role_and_content(&assistant_message_id)
        .map_err(|e| e.to_string())?;
    let Some((role, response_content)) = assistant else {
        return Ok(());
    };
    if role != "assistant" {
        return Ok(());
    }

    let Some((_, user_content)) = db
        .find_preceding_user_message(&assistant_message_id)
        .map_err(|e| e.to_string())?
    else {
        // Orphan assistant message — nothing to learn from.
        return Ok(());
    };

    if user_content.trim().is_empty() || response_content.trim().is_empty() {
        return Ok(());
    }

    // Try to build a dedicated summarization provider from the default
    // agent config. Falls back to char-truncation distillation if the
    // config is missing or the provider can't be constructed.
    let (summ_provider, summ_model, summ_provider_type): (
        Option<Box<dyn nexa_core::llm::LlmProvider>>,
        Option<String>,
        Option<ProviderType>,
    ) = match db.get_default_agent_config() {
        Ok(Some(db_config)) => {
            if let Some(ref summ_provider_name) = db_config.summarization_provider {
                let provider_type =
                    provider_type_for_parts(summ_provider_name, db_config.base_url.as_deref());
                let summ_config = ProviderConfig {
                    provider_type,
                    api_key: Some(db_config.api_key.clone()),
                    base_url: db_config.base_url.clone(),
                    org_id: None,
                    timeout_secs: None,
                    streaming: db_config.provider_streaming,
                };
                match create_provider(summ_config) {
                    Ok(p) => {
                        let model = db_config
                            .summarization_model
                            .clone()
                            .unwrap_or_else(|| db_config.model.clone());
                        (Some(p), Some(model), Some(provider_type))
                    }
                    Err(e) => {
                        warn!(
                            "learned_success: summarization provider unavailable ({e}); using char truncation"
                        );
                        (None, None, None)
                    }
                }
            } else {
                (None, None, None)
            }
        }
        Ok(None) => (None, None, None),
        Err(e) => {
            warn!("learned_success: failed to load agent config ({e}); using char truncation");
            (None, None, None)
        }
    };

    let assistant_message_id_bg = assistant_message_id.clone();
    let user_content_for_embed = nexa_core::learning::distill_text(
        &user_content,
        nexa_core::learning::LEARNED_TEXT_MAX_CHARS,
    );
    let db_bg = db.clone();

    // Fire-and-forget: LLM distillation + insert + embedding computation.
    tokio::spawn(async move {
        let row_id = match nexa_core::learning::insert_learned_success_with_llm(
            &db_bg,
            &user_content,
            &response_content,
            &assistant_message_id_bg,
            summ_provider.as_deref(),
            summ_model.as_deref(),
            summ_provider_type,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("learned_success insert failed: {e}");
                return;
            }
        };

        match db_bg.get_embedder_config() {
            Ok(cfg) => match nexa_core::embed::create_embedder(&cfg) {
                Ok(embedder) => {
                    if embedder.dimensions() == 0 {
                        return;
                    }
                    match embedder.embed(&user_content_for_embed) {
                        Ok(vec) if !vec.iter().all(|&v| v == 0.0) => {
                            if let Err(e) = db_bg.update_learned_success_embedding(&row_id, &vec) {
                                warn!("learned_success embedding update failed: {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => warn!("learned_success embedding compute failed: {e}"),
                    }
                }
                Err(e) => warn!("learned_success embedder create failed: {e}"),
            },
            Err(e) => warn!("learned_success embedder config load failed: {e}"),
        }
    });

    Ok(())
}

#[tauri::command]
pub fn get_message_feedback_cmd(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> Result<Option<nexa_core::learning::MessageFeedback>, String> {
    state
        .db
        .get_message_feedback(&message_id)
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
        emit_app_event(
            &app_handle,
            "batch:fts-progress",
            &FtsProgress {
                operation: "optimize-fts".to_string(),
                phase: "running".to_string(),
            },
        );
        let result = db.optimize_fts_index().map_err(|e| e.to_string());
        emit_app_event(
            &app_handle,
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
pub fn test_api_connection_cmd(
    api_key: String,
    base_url: String,
    model: String,
    dimensions: u32,
) -> Result<bool, String> {
    nexa_core::embed::test_api_connection(&api_key, &base_url, &model, dimensions)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_local_model_cmd(
    local_model: Option<String>,
    model_path: Option<String>,
) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        let model = local_model
            .map(|s| LocalEmbeddingModel::from_config_str(&s))
            .unwrap_or_default();
        nexa_core::embed::check_local_model_exists_for(model_path.as_deref(), &model)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))
}

#[tauri::command]
pub async fn download_local_model_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    local_model: Option<String>,
    model_path: Option<String>,
    cancel_flag: tauri::State<'_, DownloadCancelFlag>,
) -> Result<(), String> {
    let cancel = cancel_flag.0.clone();
    let app_cfg = state.db.load_app_config().map_err(|e| e.to_string())?;
    let hf_mirror_base = app_cfg.hf_mirror_base_url.clone();
    cancel.store(false, Ordering::Relaxed);
    tokio::task::spawn_blocking(move || {
        let model = local_model
            .map(|s| LocalEmbeddingModel::from_config_str(&s))
            .unwrap_or_default();
        nexa_core::embed::download_local_model_for_with_progress(
            model_path.as_deref(),
            &model,
            &hf_mirror_base,
            |progress| {
                emit_app_event(&app_handle, "model:download-progress", &progress);
            },
            &cancel,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[tauri::command]
pub fn cancel_model_download_cmd(
    cancel_flag: tauri::State<'_, DownloadCancelFlag>,
) -> Result<(), String> {
    cancel_flag.0.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn delete_local_model_cmd(
    local_model: Option<String>,
    model_path: Option<String>,
) -> Result<(), String> {
    let model = local_model
        .map(|s| LocalEmbeddingModel::from_config_str(&s))
        .unwrap_or_default();
    let dir = match model_path.filter(|path| !path.trim().is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => nexa_core::embed::default_model_dir_for(&model).map_err(|e| e.to_string())?,
    };
    if dir.exists() {
        for filename in ["model.onnx", "tokenizer.json"] {
            let path = dir.join(filename);
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
        if std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .next()
            .is_none()
        {
            std::fs::remove_dir(&dir).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
