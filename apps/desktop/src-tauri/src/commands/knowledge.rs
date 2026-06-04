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
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let result = nexa_core::compile::compile_document(
        &state.db,
        &doc_id,
        provider.as_ref(),
        &db_config.model,
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
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let results = nexa_core::compile::compile_pending_with_progress(
        &state.db,
        provider.as_ref(),
        &db_config.model,
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
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    let cap = limit.unwrap_or(10);
    let results =
        nexa_core::compile::compile_pending(&state.db, provider.as_ref(), &db_config.model, cap)
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

// Silence dead-code warnings for the re-exported type alias used by callers.
#[allow(dead_code)]
type _ApprovalTypeMarkers = (
    ApprovalRequest,
    ApprovalCallback,
    ToolApprovalMode,
    ToolApprovalPolicy,
);
