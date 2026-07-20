use super::*;

// ── Agent Trace Analytics ──────────────────────────────────────────────

#[tauri::command]
pub fn get_trace_summary(
    state: tauri::State<'_, AppState>,
) -> Result<nexa_core::trace::TraceSummary, String> {
    state.db.get_trace_summary().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recent_traces(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<nexa_core::trace::AgentTrace>, String> {
    state
        .db
        .get_recent_traces(limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_agent_task_trajectory_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
    redaction_profile: Option<nexa_core::trajectory::TrajectoryRedactionProfile>,
) -> Result<nexa_core::trajectory::Trajectory, String> {
    nexa_core::trajectory::export_agent_task_run_trajectory(
        state.db.as_ref(),
        &run_id,
        redaction_profile
            .unwrap_or(nexa_core::trajectory::TrajectoryRedactionProfile::FullLocalPrivate),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_agent_trajectory_cmd(
    state: tauri::State<'_, AppState>,
    trajectory: nexa_core::trajectory::Trajectory,
) -> Result<nexa_core::trajectory::TrajectoryStoreSummary, String> {
    state
        .db
        .save_agent_trajectory(&trajectory)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_agent_trajectory_cmd(
    state: tauri::State<'_, AppState>,
    trajectory_id: String,
) -> Result<nexa_core::trajectory::Trajectory, String> {
    state
        .db
        .load_agent_trajectory(&trajectory_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_agent_trajectories_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<nexa_core::trajectory::TrajectoryStoreSummary>, String> {
    state
        .db
        .list_agent_trajectory_summaries(limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_trajectory_eval_pack_cmd(
    state: tauri::State<'_, AppState>,
    pack: nexa_core::eval_harness::EvalPack,
) -> Result<nexa_core::eval_harness::EvalReport, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::evaluate_pack_from_store(db.as_ref(), &pack)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn compare_trajectory_replay_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::eval_harness::TrajectoryReplayRequest,
) -> Result<nexa_core::eval_harness::TrajectoryReplayReport, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::evaluate_replay_from_store(db.as_ref(), &request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn replay_trajectory_session_cmd(
    state: tauri::State<'_, AppState>,
    trajectory_id: String,
    runtime_mode: Option<nexa_core::eval_harness::TrajectoryReplayRuntimeMode>,
) -> Result<nexa_core::eval_harness::TrajectoryReplayExecution, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::replay_trajectory_from_store_with_runtime_mode(
        db.as_ref(),
        &trajectory_id,
        runtime_mode
            .unwrap_or(nexa_core::eval_harness::TrajectoryReplayRuntimeMode::RecordedEvents),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_stored_trajectory_smoke_eval_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<nexa_core::eval_harness::StoredTrajectoryEvalReport, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::run_stored_trajectory_smoke_eval(db.as_ref(), limit.unwrap_or(50))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_developer_eval_smoke_workflow_cmd(
    state: tauri::State<'_, AppState>,
    trajectory_limit: Option<usize>,
) -> Result<nexa_core::eval_harness::DeveloperEvalSmokeReport, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::run_developer_eval_smoke_workflow(
        db.as_ref(),
        trajectory_limit.unwrap_or(50),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_developer_eval_nightly_workflow_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<nexa_core::eval_harness::DeveloperEvalSmokeReport, String> {
    let db = state.db.clone();
    nexa_core::eval_harness::run_developer_eval_nightly_workflow(db.as_ref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_agent_quality_eval_cmd() -> nexa_core::quality_eval::QualityEvalReport {
    nexa_core::quality_eval::run_agent_quality_eval()
}

// ── Knowledge Compilation Commands ─────────────────────────────────────

#[tauri::command]
pub async fn compile_document_cmd(
    state: tauri::State<'_, AppState>,
    doc_id: String,
) -> Result<serde_json::Value, String> {
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set.".to_string())?;
    let provider_config = db_config_to_provider_config(&db_config, None);
    let provider_type = provider_config.provider_type;
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let result = nexa_core::compile::compile_document(
        &state.db,
        &doc_id,
        provider.as_ref(),
        &db_config.model,
        Some(provider_type),
    )
    .await
    .map_err(|e| e.to_string())?;

    serde_json::to_value(&result).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn compile_pending_documents_cmd(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set.".to_string())?;
    let provider_config = db_config_to_provider_config(&db_config, None);
    let provider_type = provider_config.provider_type;
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let results = nexa_core::compile::compile_pending_with_progress(
        &state.db,
        provider.as_ref(),
        &db_config.model,
        Some(provider_type),
        limit.unwrap_or(10),
        |progress| {
            emit_app_event(&app_handle, "compile:progress", progress);
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    serde_json::to_value(&results).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_compile_stats_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let stats = state.db.get_compile_stats().map_err(|e| e.to_string())?;
    serde_json::to_value(&stats).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_knowledge_map_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let map = state
        .db
        .get_knowledge_map(limit.unwrap_or(50))
        .map_err(|e| e.to_string())?;
    serde_json::to_value(&map).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_knowledge_graph_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
    source_id: Option<String>,
    path_prefix: Option<String>,
    entity_types: Option<Vec<String>>,
    relation_types: Option<Vec<String>>,
    min_strength: Option<f64>,
) -> Result<serde_json::Value, String> {
    let graph = state
        .db
        .get_knowledge_graph(nexa_core::knowledge_graph::KnowledgeGraphQuery {
            limit: limit.unwrap_or(80),
            source_id: source_id.filter(|value| !value.trim().is_empty()),
            source_ids: Vec::new(),
            path_prefix: path_prefix.filter(|value| !value.trim().is_empty()),
            entity_types: entity_types.unwrap_or_default(),
            relation_types: relation_types.unwrap_or_default(),
            min_strength,
        })
        .map_err(|e| e.to_string())?;
    serde_json::to_value(&graph).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_knowledge_health_check_cmd(
    state: tauri::State<'_, AppState>,
    stale_days: Option<u32>,
) -> Result<serde_json::Value, String> {
    let report = state
        .db
        .run_health_check(stale_days.unwrap_or(90))
        .map_err(|e| e.to_string())?;
    serde_json::to_value(&report).map_err(|e| e.to_string())
}

/// Compile pending documents after a scan/embed cycle.
/// This is an opt-in command the frontend can call after ingestion completes.
#[tauri::command]
pub async fn compile_after_scan_cmd(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let db_config = state
        .db
        .get_default_agent_config()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No default agent config set.".to_string())?;
    let provider_config = db_config_to_provider_config(&db_config, None);
    let provider_type = provider_config.provider_type;
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let cap = limit.unwrap_or(10);
    let results = nexa_core::compile::compile_pending(
        &state.db,
        provider.as_ref(),
        &db_config.model,
        Some(provider_type),
        cap,
    )
    .await
    .map_err(|e| e.to_string())?;

    // Notify frontend of compilation progress
    emit_app_event(
        &app_handle,
        "compile:complete",
        &serde_json::json!({
            "compiled": results.len(),
            "limit": cap,
        }),
    );

    if let Ok(app_config) = state.db.load_app_config() {
        if app_config.dreaming.enabled && app_config.dreaming.after_scan {
            match start_dream_run_with_config(
                state.db.as_ref(),
                &app_config,
                "after_scan",
                serde_json::json!({
                        "surface": "compile_after_scan",
                        "compiled": results.len(),
                        "limit": cap
                }),
                None,
            ) {
                Ok(run) => emit_app_event(&app_handle, "dreaming:complete", &run),
                Err(err) => emit_app_event(
                    &app_handle,
                    "dreaming:error",
                    &serde_json::json!({ "triggerKind": "after_scan", "error": err }),
                ),
            }
        }
    }

    serde_json::to_value(&results).map_err(|e| e.to_string())
}

// ── Scan Error Commands ─────────────────────────────────────────────────

#[tauri::command]
pub fn get_scan_errors_cmd(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<Vec<nexa_core::models::ScanError>, String> {
    state
        .db
        .get_scan_errors(&source_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_scan_errors_cmd(
    state: tauri::State<'_, AppState>,
    source_id: String,
) -> Result<usize, String> {
    state
        .db
        .clear_scan_errors(&source_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_scan_error_cmd(
    state: tauri::State<'_, AppState>,
    source_id: String,
    path: String,
) -> Result<bool, String> {
    state
        .db
        .clear_scan_error(&source_id, &path)
        .map_err(|e| e.to_string())
}

// ── Knowledge Loop ──────────────────────────────────────────────────

#[tauri::command]
pub fn get_knowledge_gaps_cmd(
    state: tauri::State<'_, AppState>,
    min_queries: Option<i64>,
) -> Result<serde_json::Value, String> {
    let gaps = state
        .db
        .get_knowledge_gaps(min_queries.unwrap_or(2))
        .map_err(|e| e.to_string())?;
    serde_json::to_value(&gaps).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn suggest_explorations_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let suggestions = state
        .db
        .suggest_explorations(limit.unwrap_or(10))
        .map_err(|e| e.to_string())?;
    serde_json::to_value(&suggestions).map_err(|e| e.to_string())
}

// ── Dreaming / Insights ─────────────────────────────────────────────

fn start_dream_run_with_config(
    db: &Database,
    app_config: &AppConfig,
    trigger_kind: &str,
    base_scope: serde_json::Value,
    max_artifacts: Option<usize>,
) -> Result<nexa_core::dreaming::DreamRun, String> {
    if trigger_kind != "manual" && !background_dream_budget_available(db, app_config)? {
        return Err("Dreaming daily background budget has been reached.".to_string());
    }
    db.start_dream_run(nexa_core::dreaming::StartDreamInput {
        trigger_kind: Some(trigger_kind.to_string()),
        scope_json: Some(nexa_core::dreaming_scope::merge_configured_dream_scope(
            &app_config.dreaming,
            base_scope,
        )),
        max_artifacts: Some(max_artifacts.unwrap_or(app_config.dreaming.max_artifacts_per_run)),
    })
    .map_err(|e| e.to_string())
}

fn background_dream_budget_available(
    db: &Database,
    app_config: &AppConfig,
) -> Result<bool, String> {
    let max_runs = app_config.dreaming.max_runs_per_day;
    if max_runs == 0 {
        return Ok(false);
    }
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let runs = db.list_dream_runs(200).map_err(|e| e.to_string())?;
    let used = runs
        .iter()
        .filter(|run| run.trigger_kind != "manual" && run.created_at.starts_with(&today))
        .count();
    Ok(used < max_runs)
}

pub struct DreamingSchedulerState {
    pub shutdown: Arc<AtomicBool>,
    pub tick_lock: TokioMutex<()>,
    pub poll_interval_secs: u64,
    pub last_idle_run_secs: AtomicU64,
    pub last_scheduled_run_secs: AtomicU64,
}

pub fn init_dreaming_scheduler(app_handle: AppHandle) {
    if app_handle.try_state::<DreamingSchedulerState>().is_some() {
        return;
    }

    let scheduler_state = DreamingSchedulerState {
        shutdown: Arc::new(AtomicBool::new(false)),
        tick_lock: TokioMutex::new(()),
        poll_interval_secs: 60,
        last_idle_run_secs: AtomicU64::new(0),
        last_scheduled_run_secs: AtomicU64::new(0),
    };
    let shutdown = Arc::clone(&scheduler_state.shutdown);
    let poll_interval_secs = scheduler_state.poll_interval_secs;
    app_handle.manage(scheduler_state);

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(poll_interval_secs.max(5)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            if let Err(err) = run_dreaming_scheduler_tick(handle.clone()).await {
                warn!("Dreaming scheduler tick failed: {err}");
            }
            interval.tick().await;
        }
    });
}

pub fn shutdown_dreaming_scheduler(state: &DreamingSchedulerState) {
    state.shutdown.store(true, Ordering::SeqCst);
}

pub async fn run_dreaming_scheduler_tick(
    app_handle: AppHandle,
) -> Result<Vec<nexa_core::dreaming::DreamRun>, String> {
    let scheduler_state = app_handle
        .try_state::<DreamingSchedulerState>()
        .ok_or_else(|| "Dreaming scheduler state is not initialized.".to_string())?;
    if scheduler_state.shutdown.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let _tick_guard = match scheduler_state.tick_lock.try_lock() {
        Ok(guard) => guard,
        Err(_) => return Ok(Vec::new()),
    };

    let app_state = app_handle
        .try_state::<AppState>()
        .ok_or_else(|| "App state is not initialized.".to_string())?;
    let app_config = app_state.db.load_app_config().map_err(|e| e.to_string())?;
    if !app_config.dreaming.enabled {
        return Ok(Vec::new());
    }

    let now_secs = current_unix_secs();
    let mut runs = Vec::new();
    if app_config.dreaming.schedule
        && interval_due(
            &scheduler_state.last_scheduled_run_secs,
            now_secs,
            app_config.dreaming.schedule_interval_minutes,
        )
    {
        match start_dream_run_with_config(
            app_state.db.as_ref(),
            &app_config,
            "schedule",
            serde_json::json!({
                    "surface": "dreaming_scheduler",
                    "intervalMinutes": app_config.dreaming.schedule_interval_minutes
            }),
            None,
        ) {
            Ok(run) => {
                scheduler_state
                    .last_scheduled_run_secs
                    .store(now_secs, Ordering::SeqCst);
                emit_app_event(&app_handle, "dreaming:complete", &run);
                runs.push(run);
            }
            Err(err) => {
                scheduler_state
                    .last_scheduled_run_secs
                    .store(now_secs, Ordering::SeqCst);
                emit_app_event(
                    &app_handle,
                    "dreaming:error",
                    &serde_json::json!({ "triggerKind": "schedule", "error": err }),
                );
            }
        }
    }

    if app_config.dreaming.idle
        && app_is_idle(&app_handle)
        && interval_due(
            &scheduler_state.last_idle_run_secs,
            now_secs,
            app_config.dreaming.idle_interval_minutes,
        )
    {
        match start_dream_run_with_config(
            app_state.db.as_ref(),
            &app_config,
            "idle",
            serde_json::json!({
                    "surface": "dreaming_scheduler",
                    "intervalMinutes": app_config.dreaming.idle_interval_minutes
            }),
            None,
        ) {
            Ok(run) => {
                scheduler_state
                    .last_idle_run_secs
                    .store(now_secs, Ordering::SeqCst);
                emit_app_event(&app_handle, "dreaming:complete", &run);
                runs.push(run);
            }
            Err(err) => {
                scheduler_state
                    .last_idle_run_secs
                    .store(now_secs, Ordering::SeqCst);
                emit_app_event(
                    &app_handle,
                    "dreaming:error",
                    &serde_json::json!({ "triggerKind": "idle", "error": err }),
                );
            }
        }
    }

    Ok(runs)
}

fn app_is_idle(app_handle: &AppHandle) -> bool {
    let Some(app_state) = app_handle.try_state::<AppState>() else {
        return false;
    };
    if app_state.scan_lock.try_lock().is_err() {
        return false;
    }
    let Some(agent_state) = app_handle.try_state::<AgentState>() else {
        return true;
    };
    let is_idle = match agent_state.running.try_lock() {
        Ok(running) => running.is_empty(),
        Err(_) => false,
    };
    is_idle
}

fn interval_due(last_run_secs: &AtomicU64, now_secs: u64, interval_minutes: usize) -> bool {
    let last = last_run_secs.load(Ordering::SeqCst);
    let interval_secs = (interval_minutes.max(1) as u64).saturating_mul(60);
    last == 0 || now_secs.saturating_sub(last) >= interval_secs
}

fn current_unix_secs() -> u64 {
    UNIX_EPOCH
        .elapsed()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[tauri::command]
pub fn start_dream_cmd(
    state: tauri::State<'_, AppState>,
    input: Option<nexa_core::dreaming::StartDreamInput>,
) -> Result<nexa_core::dreaming::DreamRun, String> {
    let app_config = state.db.load_app_config().map_err(|e| e.to_string())?;
    let input = input.unwrap_or_default();
    let trigger_kind = input
        .trigger_kind
        .as_deref()
        .unwrap_or("manual")
        .trim()
        .to_ascii_lowercase();
    if trigger_kind != "manual" && !app_config.dreaming.enabled {
        return Err("Dreaming background consolidation is disabled.".to_string());
    }
    start_dream_run_with_config(
        state.db.as_ref(),
        &app_config,
        &trigger_kind,
        input.scope_json.unwrap_or_else(|| serde_json::json!({})),
        input.max_artifacts,
    )
}

#[tauri::command]
pub fn list_dream_runs_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<nexa_core::dreaming::DreamRun>, String> {
    state
        .db
        .list_dream_runs(limit.unwrap_or(20))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_dream_run_events_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<nexa_core::dreaming::DreamRunEvent>, String> {
    state
        .db
        .list_dream_run_events(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_dream_artifacts_cmd(
    state: tauri::State<'_, AppState>,
    status: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<nexa_core::dreaming::DreamArtifact>, String> {
    state
        .db
        .list_dream_artifacts(status.as_deref(), kind.as_deref(), limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_dream_artifact_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
) -> Result<nexa_core::dreaming::DreamArtifact, String> {
    state
        .db
        .apply_dream_artifact(&artifact_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_dream_artifact_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
    input: nexa_core::dreaming::UpdateDreamArtifactInput,
) -> Result<nexa_core::dreaming::DreamArtifact, String> {
    state
        .db
        .update_dream_artifact(&artifact_id, input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reject_dream_artifact_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
) -> Result<nexa_core::dreaming::DreamArtifact, String> {
    state
        .db
        .reject_dream_artifact(&artifact_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn undo_dream_artifact_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
) -> Result<nexa_core::dreaming::DreamArtifact, String> {
    state
        .db
        .undo_dream_artifact(&artifact_id)
        .map_err(|e| e.to_string())
}
