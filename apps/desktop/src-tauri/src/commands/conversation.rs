use super::*;
use crate::desktop_agent_session::{
    filter_desktop_tool_names_by_package_host, resolve_desktop_summarization_provider_config,
};
use nexa_core::package_host::{
    database_backed_builtin_package_host_snapshot, PackageHealthState, PackageHostSnapshot,
};

// ── Project Commands ────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_project_cmd(
    state: tauri::State<'_, AppState>,
    input: CreateProjectInput,
) -> Result<Project, String> {
    state.db.create_project(&input).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_projects_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<Project>, String> {
    state.db.list_projects().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_project_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Project, String> {
    state.db.get_project(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_project_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    input: UpdateProjectInput,
) -> Result<Project, String> {
    state
        .db
        .update_project(&id, &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_project_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_project(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_project_memories_cmd(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ProjectMemory>, String> {
    state
        .db
        .list_project_memories(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_project_workspace_cmd(
    state: tauri::State<'_, AppState>,
    project_id: String,
    query: Option<String>,
) -> Result<ProjectWorkspaceSnapshot, String> {
    let mut snapshot = state
        .db
        .get_project_workspace_snapshot(&project_id, query.as_deref())
        .map_err(|e| e.to_string())?;
    snapshot.episodes = state
        .db
        .list_project_episodes(&project_id, 100)
        .map_err(|e| e.to_string())?;
    snapshot.events = state
        .db
        .list_project_events(&project_id, 100)
        .map_err(|e| e.to_string())?;
    Ok(snapshot)
}

#[tauri::command]
pub async fn get_project_narrative_cmd(
    state: tauri::State<'_, AppState>,
    project_id: String,
    query: String,
) -> Result<NarrativeEvidencePlan, String> {
    state
        .db
        .build_project_narrative_plan(&project_id, &query, 30)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_companion_projection_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<CompanionProjection, String> {
    state
        .db
        .get_companion_projection(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_project_knowledge_claim_cmd(
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: CreateKnowledgeClaimInput,
) -> Result<KnowledgeClaim, String> {
    state
        .db
        .create_knowledge_claim(Some(&project_id), &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn review_project_knowledge_claim_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    review_state: String,
) -> Result<KnowledgeClaim, String> {
    state
        .db
        .review_knowledge_claim(&id, &review_state)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_project_memory_cmd(
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: CreateProjectMemoryInput,
) -> Result<ProjectMemory, String> {
    state
        .db
        .create_project_memory(&project_id, &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_project_memory_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    input: UpdateProjectMemoryInput,
) -> Result<ProjectMemory, String> {
    state
        .db
        .update_project_memory(&id, &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_project_memory_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .delete_project_memory(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn move_conversation_to_project_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    project_id: String,
) -> Result<(), String> {
    state
        .db
        .move_conversation_to_project(&conversation_id, &project_id)
        .map_err(|e| e.to_string())?;
    if let Ok(project) = state.db.get_project(&project_id) {
        if let Some(source_scope) = project.source_scope {
            if !source_scope.is_empty() {
                let _ = state
                    .db
                    .set_conversation_sources(&conversation_id, &source_scope);
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_conversation_from_project_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    state
        .db
        .remove_conversation_from_project(&conversation_id)
        .map_err(|e| e.to_string())
}

// ── Conversation Commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn create_conversation_cmd(
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    system_prompt: Option<String>,
    collection_context: Option<CollectionContext>,
    project_id: Option<String>,
    persona_id: Option<String>,
) -> Result<Conversation, String> {
    let input = CreateConversationInput {
        provider,
        model,
        system_prompt,
        collection_context,
        project_id,
        persona_id,
    };
    let conversation = state
        .db
        .create_conversation(&input)
        .map_err(|e| e.to_string())?;

    if let Some(project_id) = conversation.project_id.as_deref() {
        if let Ok(project) = state.db.get_project(project_id) {
            if let Some(source_scope) = project.source_scope {
                if !source_scope.is_empty() {
                    let _ = state
                        .db
                        .set_conversation_sources(&conversation.id, &source_scope);
                }
            }
        }
    }

    Ok(conversation)
}

#[tauri::command]
pub async fn list_conversations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Conversation>, String> {
    state.db.list_conversations().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_archived_conversations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Conversation>, String> {
    state
        .db
        .list_archived_conversations()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(Conversation, Vec<ConversationMessage>), String> {
    let conv = state.db.get_conversation(&id).map_err(|e| e.to_string())?;
    let traced_assistant_ids = state
        .db
        .get_conversation_turns(&id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|turn| turn.assistant_message_id)
        .collect::<std::collections::HashSet<_>>();
    let msgs = state
        .db
        .get_messages(&id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|message| {
            let has_canonical_turn_trace = traced_assistant_ids.contains(&message.id);
            nexa_core::conversation::conversation_message_for_display_with_turn_trace(
                message,
                has_canonical_turn_trace,
            )
        })
        .collect();
    Ok((conv, msgs))
}

#[tauri::command]
pub async fn get_conversation_turns_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<ConversationTurn>, String> {
    state
        .db
        .get_conversation_turns(&conversation_id)
        .map(|turns| {
            turns
                .into_iter()
                .map(nexa_core::conversation::conversation_turn_for_display)
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_interaction_requests_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: Option<String>,
    include_terminal: Option<bool>,
) -> Result<Vec<InteractionRequest>, String> {
    state
        .db
        .list_interaction_requests(
            conversation_id.as_deref(),
            include_terminal.unwrap_or(false),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_interaction_request_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .get_interaction_request(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn mark_interaction_presented_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .mark_interaction_presented(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn mark_interaction_partially_answered_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .mark_interaction_partially_answered(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn append_interaction_supplement_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
    content: String,
) -> Result<ConversationMessage, String> {
    state
        .db
        .append_interaction_supplement(&interaction_id, &content)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn submit_interaction_response_cmd(
    state: tauri::State<'_, AppState>,
    input: SubmitInteractionResponse,
) -> Result<InteractionResponse, String> {
    state
        .db
        .submit_interaction_response(&input)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_interaction_response_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionResponse, String> {
    state
        .db
        .get_interaction_response(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn acknowledge_interaction_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .acknowledge_interaction(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn cancel_interaction_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .cancel_interaction(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn supersede_interaction_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .supersede_interaction(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn fail_interaction_cmd(
    state: tauri::State<'_, AppState>,
    interaction_id: String,
) -> Result<InteractionRequest, String> {
    state
        .db
        .fail_interaction(&interaction_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_agent_task_runs_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<AgentTaskRun>, String> {
    state
        .db_executor
        .read(move |db| db.get_agent_task_runs_for_conversation(&conversation_id))
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_recent_agent_task_runs_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<AgentTaskRunListItem>, String> {
    state
        .db
        .list_recent_agent_task_runs(limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_agent_task_run_summaries_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    cursor: Option<AgentTaskRunPageCursor>,
    status: Option<String>,
    project_id: Option<String>,
) -> Result<AgentTaskRunSummaryPage, String> {
    state
        .db
        .list_agent_task_run_summaries(
            limit.unwrap_or(25),
            cursor.as_ref(),
            status.as_deref(),
            project_id.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_agent_task_run_events_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<AgentTaskRunEvent>, String> {
    state
        .db_executor
        .read(move |db| db.get_agent_task_run_events(&run_id))
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_agent_run_events_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
    after_event_seq: Option<u64>,
) -> Result<Vec<AgentRunEvent>, String> {
    state
        .db_executor
        .read(move |db| match after_event_seq {
            Some(after_event_seq) => {
                db.list_agent_run_events_after(&run_id, after_event_seq, 2_048)
            }
            None => db.list_agent_run_events(&run_id),
        })
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_agent_run_event_page_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
    after_event_seq: u64,
    durable_high_water: Option<u64>,
) -> Result<nexa_core::agent_run::AgentRunEventPage, String> {
    state
        .db_executor
        .read(move |db| {
            db.list_agent_run_event_page(&run_id, after_event_seq, durable_high_water, 2_048)
        })
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_run_usage_snapshot_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Option<nexa_core::usage_snapshot::UsageSnapshot>, String> {
    state
        .db
        .get_run_usage_snapshot(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_conversation_usage_snapshot_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<nexa_core::usage_snapshot::UsageSnapshot>, String> {
    state
        .db
        .get_conversation_usage_snapshot(&conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_ai_usage_analytics_cmd(
    state: tauri::State<'_, AppState>,
    filter: nexa_core::usage_analytics::UsageAnalyticsFilter,
) -> Result<nexa_core::usage_analytics::UsageAnalytics, String> {
    state
        .db
        .get_usage_analytics(&filter)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_ai_usage_records_cmd(
    state: tauri::State<'_, AppState>,
    filter: nexa_core::usage_analytics::UsageAnalyticsFilter,
) -> Result<u64, String> {
    state
        .db
        .delete_usage_records(&filter)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn export_ai_usage_cmd(
    state: tauri::State<'_, AppState>,
    filter: nexa_core::usage_analytics::UsageAnalyticsFilter,
    format: String,
    path: String,
) -> Result<(), String> {
    let analytics = state
        .db
        .get_usage_analytics(&filter)
        .map_err(|error| error.to_string())?;
    let content = match format.trim().to_ascii_lowercase().as_str() {
        "json" => serde_json::to_string_pretty(&analytics).map_err(|error| error.to_string())?,
        "csv" => usage_analytics_csv(&analytics),
        other => return Err(format!("Unsupported AI usage export format: {other}")),
    };
    std::fs::write(Path::new(path.trim()), content).map_err(|error| error.to_string())
}

fn usage_analytics_csv(analytics: &nexa_core::usage_analytics::UsageAnalytics) -> String {
    let mut lines = vec![
        "dimension,key,provider,model,requests,agent_runs,turns,successes,prompt_tokens,completion_tokens,thinking_tokens,total_tokens,cache_read_tokens,cache_miss_tokens,estimated_cost_micros".to_string(),
    ];
    for (dimension, rows) in [
        ("model", &analytics.by_model),
        ("operation", &analytics.by_operation),
    ] {
        for row in rows {
            lines.push(format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                dimension,
                csv_cell(&row.key),
                csv_cell(row.provider_id.as_deref().unwrap_or("")),
                csv_cell(row.model_id.as_deref().unwrap_or("")),
                row.request_count,
                row.agent_run_count,
                row.turn_count,
                row.success_count,
                row.prompt_tokens,
                row.completion_tokens,
                row.thinking_tokens,
                row.total_tokens,
                row.cache_read_tokens,
                row.cache_miss_tokens,
                row.estimated_cost_micros
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            ));
        }
    }
    lines.join("\n") + "\n"
}

fn csv_cell(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[tauri::command]
pub async fn get_agent_subtask_runs_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<AgentSubtaskRun>, String> {
    state
        .db
        .list_agent_subtask_runs(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_agent_execution_graph_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<AgentExecutionGraph, String> {
    state
        .db
        .get_agent_execution_graph(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_agent_task_artifacts_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<AgentTaskArtifactSummary>, String> {
    state
        .db
        .list_agent_task_artifacts(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_persisted_agent_task_artifacts_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<AgentTaskArtifact>, String> {
    state
        .db
        .list_persisted_agent_task_artifacts(&run_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_agent_task_artifact_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
    input: CreateAgentTaskArtifactInput,
) -> Result<AgentTaskArtifact, String> {
    state
        .db
        .create_agent_task_artifact(&run_id, &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_agent_task_artifact_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
    input: UpdateAgentTaskArtifactInput,
) -> Result<AgentTaskArtifact, String> {
    state
        .db
        .update_agent_task_artifact(&artifact_id, &input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_agent_task_artifact_versions_cmd(
    state: tauri::State<'_, AppState>,
    artifact_id: String,
) -> Result<Vec<AgentTaskArtifactVersion>, String> {
    state
        .db
        .list_agent_task_artifact_versions(&artifact_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_tool_access_map_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
) -> Result<Vec<nexa_core::tool_access::ToolAccessInfo>, String> {
    let package_assembler =
        nexa_core::package_host::PackageRuntimeAssembler::database_builtin(state.db.as_ref())
            .map_err(|error| error.to_string())?;
    let mut registry = package_assembler.builtin_tool_registry();
    {
        let mut mcp_manager = mcp_state.manager.lock().await;
        match sync_enabled_mcp_servers(&state.db, &mut mcp_manager).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id} for tool access map: {error}");
                }
            }
            Err(error) => {
                warn!("Failed to refresh enabled MCP servers for tool access map: {error}")
            }
        }
        if let Err(error) = mcp_manager
            .register_tools_with_recovery(&mut registry, Arc::downgrade(&mcp_state.manager))
            .await
        {
            warn!("Failed to register MCP tools for tool access map: {error}");
        }
    }

    let mut names = registry.tool_names();
    names.extend([
        "spawn_subagent".to_string(),
        "spawn_subagent_batch".to_string(),
        "judge_subagent_results".to_string(),
    ]);
    let names = filter_desktop_tool_names_by_package_host(state.db.as_ref(), names)?;
    Ok(nexa_core::tool_access::tool_access_map_for_names(names))
}

#[tauri::command]
pub fn list_capability_packages_cmd(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    include_runtime_checks: Option<bool>,
) -> Result<Vec<nexa_core::plugins::CapabilityPackageView>, String> {
    let config = state.db.load_app_config().map_err(|e| e.to_string())?;
    let office_runtime = if include_runtime_checks.unwrap_or(false) {
        match app_handle.path().app_data_dir() {
            Ok(data_dir) => Some(nexa_core::office_runtime::check_office_runtime(&data_dir)),
            Err(error) => {
                warn!(
                    "Failed to resolve app data directory for capability runtime checks: {error}"
                );
                None
            }
        }
    } else {
        None
    };

    Ok(nexa_core::plugins::builtin_capability_views_with_context(
        nexa_core::plugins::CapabilityPackageViewContext {
            app_config: Some(&config),
            office_runtime: office_runtime.as_ref(),
        },
    ))
    .and_then(|manifests| {
        filter_desktop_capability_views_by_package_host(state.db.as_ref(), manifests)
    })
}

pub(crate) fn filter_desktop_capability_views_by_package_host(
    db: &Database,
    manifests: Vec<nexa_core::plugins::CapabilityPackageView>,
) -> Result<Vec<nexa_core::plugins::CapabilityPackageView>, String> {
    let snapshot = desktop_package_host_snapshot(db)?;
    let visible_package_ids = snapshot
        .records
        .iter()
        .filter(|record| record.is_runtime_visible())
        .map(|record| record.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    Ok(manifests
        .into_iter()
        .filter(|manifest| visible_package_ids.contains(manifest.id.as_str()))
        .collect())
}

pub(crate) fn desktop_package_host_snapshot(db: &Database) -> Result<PackageHostSnapshot, String> {
    database_backed_builtin_package_host_snapshot(db).map_err(|e| e.to_string())
}

fn normalize_desktop_package_id(package_id: &str) -> Result<String, String> {
    let package_id = package_id.trim();
    if package_id.is_empty() {
        return Err("package_id must not be empty".to_string());
    }
    Ok(package_id.to_string())
}

fn ensure_desktop_package_host_package(db: &Database, package_id: &str) -> Result<(), String> {
    let package_id = normalize_desktop_package_id(package_id)?;
    let snapshot = desktop_package_host_snapshot(db)?;
    if snapshot
        .records
        .iter()
        .any(|record| record.id == package_id)
    {
        Ok(())
    } else {
        Err(format!("Unknown package id {package_id}"))
    }
}

pub(crate) fn set_desktop_package_host_package_enabled(
    db: &Database,
    package_id: &str,
    enabled: bool,
) -> Result<PackageHostSnapshot, String> {
    let package_id = normalize_desktop_package_id(package_id)?;
    ensure_desktop_package_host_package(db, &package_id)?;
    db.set_package_host_package_enabled(&package_id, enabled)
        .map_err(|e| e.to_string())?;
    desktop_package_host_snapshot(db)
}

pub(crate) fn set_desktop_package_host_package_health(
    db: &Database,
    package_id: &str,
    health_state: PackageHealthState,
) -> Result<PackageHostSnapshot, String> {
    let package_id = normalize_desktop_package_id(package_id)?;
    ensure_desktop_package_host_package(db, &package_id)?;
    db.set_package_host_package_health(&package_id, health_state)
        .map_err(|e| e.to_string())?;
    desktop_package_host_snapshot(db)
}

#[tauri::command]
pub fn get_package_host_snapshot_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<PackageHostSnapshot, String> {
    desktop_package_host_snapshot(state.db.as_ref())
}

#[tauri::command]
pub fn set_package_host_package_enabled_cmd(
    state: tauri::State<'_, AppState>,
    package_id: String,
    enabled: bool,
) -> Result<PackageHostSnapshot, String> {
    set_desktop_package_host_package_enabled(state.db.as_ref(), &package_id, enabled)
}

#[tauri::command]
pub fn set_package_host_package_health_cmd(
    state: tauri::State<'_, AppState>,
    package_id: String,
    health_state: PackageHealthState,
) -> Result<PackageHostSnapshot, String> {
    set_desktop_package_host_package_health(state.db.as_ref(), &package_id, health_state)
}

#[tauri::command]
pub fn list_project_tools_cmd(
    state: tauri::State<'_, AppState>,
    source_scope: Option<Vec<String>>,
) -> Result<nexa_core::tools::project_tool::ProjectToolCatalog, String> {
    let scope = source_scope.unwrap_or_default();
    nexa_core::tools::project_tool::list_project_tool_catalog(state.db.as_ref(), &scope)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_conversation_collection_context_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    collection_context: Option<CollectionContext>,
) -> Result<(), String> {
    state
        .db
        .update_conversation_collection_context(&id, collection_context.as_ref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_conversation_persona_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    persona_id: Option<String>,
) -> Result<Conversation, String> {
    let normalized = persona_id.as_deref();
    state
        .db
        .update_conversation_persona(&id, normalized)
        .map_err(|e| e.to_string())?;
    state.db.get_conversation(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_conversation_model_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    provider: String,
    model: String,
) -> Result<Conversation, String> {
    state
        .db
        .update_conversation_model(&id, &provider, &model)
        .map_err(|e| e.to_string())?;
    state.db.get_conversation(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db_executor
        .write(move |db| db.delete_conversation(&id))
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn archive_conversation_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    id: String,
) -> Result<Conversation, String> {
    if agent_state.sessions.contains(&id).await {
        return Err("Stop the running agent before archiving this conversation.".to_string());
    }
    state
        .db
        .archive_conversation(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn unarchive_conversation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Conversation, String> {
    state
        .db
        .unarchive_conversation(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_conversations_batch_cmd(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, String> {
    state
        .db_executor
        .write(move |db| db.delete_conversations_batch(&ids))
        .await
        .map(|execution| execution.value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_all_conversations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    state
        .db_executor
        .write(|db| db.delete_all_conversations())
        .await
        .map(|execution| execution.value)
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
        .rename_conversation_by_user(&id, &title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_title_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<String, String> {
    // 0. Respect user-initiated renames: if the user already set a title,
    //    skip auto regeneration.
    let conversation = state
        .db
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;
    if !conversation.initial_auto_title_pending || !conversation.title.trim().is_empty() {
        return Ok(conversation.title);
    }

    // 1. Load a bounded excerpt containing both the opening and latest turns.
    let messages = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    let title_context = nexa_core::conversation::build_title_generation_context(&messages)
        .ok_or_else(|| "No user message found for title generation.".to_string())?;
    let fallback_title = nexa_core::conversation::fallback_title(&title_context.fallback_message);

    // 2. Prefer the config that actually owns this conversation. A global
    //    default may point at another provider after the user switches models.
    let configs = state.db.list_agent_configs().map_err(|e| e.to_string())?;
    let db_config = configs
        .iter()
        .find(|config| {
            config.provider.eq_ignore_ascii_case(&conversation.provider)
                && config.model == conversation.model
        })
        .or_else(|| {
            configs
                .iter()
                .find(|config| config.provider.eq_ignore_ascii_case(&conversation.provider))
        })
        .or_else(|| configs.iter().find(|config| config.is_default))
        .cloned();

    // 3. Build the provider + model pair. Provider/configuration failures are
    //    not fatal to auto-title: persist the deterministic fallback instead.
    let title = if let Some(db_config) = db_config {
        let fallback_model = db_config.model.clone();
        let provider_bundle = if let Some(summ_provider_name) =
            db_config.summarization_provider.as_deref()
        {
            let summ_provider_type =
                provider_type_for_parts(summ_provider_name, db_config.base_url.as_deref());
            let summ_config = ProviderConfig {
                provider_type: summ_provider_type,
                api_key: Some(db_config.api_key.clone()),
                base_url: db_config.base_url.clone(),
                org_id: None,
                timeout_secs: None,
                streaming: db_config.provider_streaming,
            };
            match create_provider(summ_config) {
                Ok(provider) => Ok((
                    provider,
                    db_config
                        .summarization_model
                        .clone()
                        .unwrap_or_else(|| fallback_model.clone()),
                    summ_provider_type,
                )),
                Err(error) => {
                    warn!(
                        "summarization provider '{summ_provider_name}' unavailable ({error}); falling back to the conversation provider"
                    );
                    let provider_type = provider_type_for_config(&db_config);
                    create_provider(db_config_to_provider_config(&db_config, None)).map(
                        |provider| {
                            (
                                provider,
                                db_config
                                    .summarization_model
                                    .clone()
                                    .unwrap_or_else(|| fallback_model.clone()),
                                provider_type,
                            )
                        },
                    )
                }
            }
        } else {
            let provider_type = provider_type_for_config(&db_config);
            create_provider(db_config_to_provider_config(&db_config, None)).map(|provider| {
                (
                    provider,
                    db_config
                        .summarization_model
                        .clone()
                        .unwrap_or(fallback_model),
                    provider_type,
                )
            })
        };

        match provider_bundle {
            Ok((provider, title_model, title_provider_type)) => {
                let generated = nexa_core::conversation::generate_title_with_usage(
                    provider.as_ref(),
                    &title_model,
                    Some(title_provider_type),
                    &title_context,
                )
                .await;
                if let Some(usage) = generated.usage.as_ref() {
                    let provider_id =
                        nexa_core::usage_analytics::provider_type_id(Some(title_provider_type));
                    let (estimated_cost_micros, currency, pricing_version) =
                        nexa_core::usage_analytics::usage_cost_metadata(Some(title_provider_type));
                    let raw = serde_json::to_value(usage).unwrap_or_else(|_| serde_json::json!({}));
                    let invocation_id = format!(
                        "title:{}:{}",
                        conversation_id,
                        blake3::hash(title_context.excerpt.as_bytes()).to_hex()
                    );
                    let _ =
                        state
                            .db
                            .record_ai_usage(&nexa_core::usage_analytics::AiUsageRecordInput {
                                invocation_id: &invocation_id,
                                occurred_at: None,
                                provider_id,
                                provider_type: provider_id,
                                model_id: &title_model,
                                raw_model_id: Some(&title_model),
                                modality: "language_model",
                                operation_kind: "conversation_title",
                                conversation_id: Some(&conversation_id),
                                turn_id: None,
                                run_id: None,
                                subtask_run_id: None,
                                project_id: None,
                                prompt_tokens: u64::from(usage.prompt_tokens),
                                completion_tokens: u64::from(usage.completion_tokens),
                                thinking_tokens: u64::from(usage.thinking_tokens.unwrap_or(0)),
                                total_tokens: u64::from(usage.total_tokens.max(
                                    usage.prompt_tokens.saturating_add(usage.completion_tokens),
                                )),
                                cache_read_tokens: u64::from(usage.cache_read_tokens.unwrap_or(0)),
                                cache_miss_tokens: u64::from(usage.cache_miss_tokens.unwrap_or(0)),
                                cache_creation_tokens: u64::from(
                                    usage.cache_creation_tokens.unwrap_or(0),
                                ),
                                usage_source: "provider",
                                request_status: "success",
                                latency_ms: None,
                                time_to_first_token_ms: None,
                                upstream_provider_id: None,
                                cache_outcome_reason: None,
                                estimated_cost_micros,
                                currency,
                                pricing_version,
                                provider_raw: &raw,
                            });
                }
                generated.title
            }
            Err(error) => {
                warn!("title provider unavailable ({error}); using fallback title");
                fallback_title.clone()
            }
        }
    } else {
        warn!(
            "no agent config available for conversation {}; using fallback title",
            conversation_id
        );
        fallback_title
    };

    // 4. The DB update is guarded by title_is_auto. Re-read the row so a
    //    concurrent user rename wins both in persistence and the returned UI.
    state
        .db
        .update_conversation_title(&conversation_id, &title)
        .map_err(|e| e.to_string())?;
    state
        .db
        .get_conversation(&conversation_id)
        .map(|conversation| conversation.title)
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

// ── Conversation Maintenance Commands ────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactConversationResult {
    pub conversation_id: String,
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub evicted_messages: usize,
}

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
pub async fn start_context_compaction_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::context_maintenance::StartContextCompactionRequest,
) -> Result<nexa_core::context_maintenance::ContextCompactionHandle, String> {
    let conversation_id = request.conversation_id.clone();
    let prepared = state
        .db_executor
        .read(move |database| {
            let conversation = database.get_conversation(&conversation_id)?;
            let config = select_agent_config_for_conversation(database, &conversation, None)
                .map_err(nexa_core::error::CoreError::InvalidInput)?;
            ensure_manual_compaction_supported(&config.provider)?;
            let summary_provider = resolve_desktop_summarization_provider_config(database, &config)
                .map_err(nexa_core::error::CoreError::InvalidInput)?;
            Ok((conversation, config, summary_provider))
        })
        .await
        .map_err(|error| error.to_string())?
        .value;
    let (conversation, config, summary_provider) = prepared;
    let (provider_config, provider_label, model) = summary_provider.unwrap_or_else(|| {
        (
            db_config_to_provider_config(&config, None),
            config.name.clone(),
            config
                .summarization_model
                .clone()
                .unwrap_or_else(|| config.model.clone()),
        )
    });
    let provider_type = provider_config.provider_type;
    let provider = create_provider(provider_config).map_err(|error| error.to_string())?;
    let job = nexa_core::context_maintenance::ContextCompactionJob {
        request,
        snapshot_version: conversation.updated_at,
        model,
        context_window: config
            .context_window
            .and_then(|value| u32::try_from(value).ok()),
        max_response_tokens: config
            .max_tokens
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(4_096),
        provider_type: Some(provider_type),
        provider_label,
        summarizer: Arc::from(provider),
    };
    state
        .context_compaction
        .start(job)
        .await
        .map_err(|error| error.to_string())
}

fn ensure_manual_compaction_supported(provider: &str) -> Result<(), nexa_core::error::CoreError> {
    if crate::subscription_runtime::SubscriptionRuntimeKind::from_provider(provider).is_some() {
        return Err(nexa_core::error::CoreError::InvalidInput("Manual context compaction is unavailable for subscription conversations. Their official runtime manages context during each active turn.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod subscription_compaction_tests {
    #[test]
    fn native_routes_are_rejected_before_constructing_an_http_summarizer() {
        for provider in ["github_copilot", "openai_codex"] {
            assert!(super::ensure_manual_compaction_supported(provider).is_err());
        }
        assert!(super::ensure_manual_compaction_supported("open_ai").is_ok());
    }
}

#[tauri::command]
pub async fn observe_context_compaction_cmd(
    state: tauri::State<'_, AppState>,
    operation_id: String,
    after_seq: u64,
) -> Result<nexa_core::activity::ActivityObservation, String> {
    state
        .context_compaction
        .observe(&operation_id, after_seq, Duration::from_millis(2_000))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn cancel_context_compaction_cmd(
    state: tauri::State<'_, AppState>,
    operation_id: String,
) -> Result<(), String> {
    state
        .context_compaction
        .cancel(&operation_id, "user_requested")
        .await
        .map_err(|error| error.to_string())
}

/// Compatibility adapter for older clients. Product UI uses the immediate
/// start/observe/cancel protocol above; no compaction implementation lives in
/// this command.
#[tauri::command]
pub async fn compact_conversation_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<CompactConversationResult, String> {
    let handle = start_context_compaction_cmd(
        state.clone(),
        nexa_core::context_maintenance::StartContextCompactionRequest {
            conversation_id,
            idempotency_key: Uuid::new_v4().to_string(),
            policy: Default::default(),
        },
    )
    .await?;
    let mut cursor = 0;
    loop {
        let observation = state
            .context_compaction
            .observe(&handle.operation_id, cursor, Duration::from_millis(2_500))
            .await
            .map_err(|error| error.to_string())?;
        cursor = observation.cursor;
        if !observation.record.state.is_terminal() {
            continue;
        }
        let detail = observation
            .events
            .last()
            .and_then(|event| event.payload.get("detail"));
        if observation.record.state == nexa_core::activity::ActivityState::Completed {
            let result = detail
                .and_then(|detail| detail.get("result"))
                .cloned()
                .ok_or_else(|| "Context compaction completed without a result".to_string())?;
            let result: nexa_core::context_maintenance::ContextCompactionResult =
                serde_json::from_value(result).map_err(|error| error.to_string())?;
            return Ok(CompactConversationResult {
                conversation_id: result.conversation_id,
                messages_before: result.messages_before,
                messages_after: result.messages_after,
                tokens_before: result.tokens_before,
                tokens_after: result.tokens_after,
                evicted_messages: result.evicted_messages,
            });
        }
        return Err(detail
            .and_then(|detail| detail.get("error").or_else(|| detail.get("reason")))
            .and_then(|value| value.as_str())
            .unwrap_or("Context compaction did not complete")
            .to_string());
    }
}

#[tauri::command]
pub async fn search_conversations_cmd(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<nexa_core::conversation::ConversationSearchResult>, String> {
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
) -> Result<Vec<nexa_core::conversation::Checkpoint>, String> {
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
        .restore_checkpoint_into_conversation(&checkpoint_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn branch_checkpoint_cmd(
    state: tauri::State<'_, AppState>,
    checkpoint_id: String,
) -> Result<CheckpointBranch, String> {
    state
        .db
        .branch_checkpoint(&checkpoint_id)
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

// ── File Checkpoint Commands ───────────────────────────────────────────────

#[tauri::command]
pub fn list_file_checkpoints_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: Option<String>,
) -> Result<Vec<nexa_core::file_checkpoint::FileCheckpoint>, String> {
    state
        .db
        .list_file_checkpoints(conversation_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_file_checkpoint_cmd(
    state: tauri::State<'_, AppState>,
    checkpoint_id: String,
) -> Result<nexa_core::file_checkpoint::FileCheckpointRestore, String> {
    state
        .db
        .restore_file_checkpoint(&checkpoint_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_file_checkpoint_cmd(
    state: tauri::State<'_, AppState>,
    checkpoint_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_file_checkpoint(&checkpoint_id)
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
) -> Result<Vec<nexa_core::personalization::UserMemory>, String> {
    state.db.list_user_memories().map_err(|e| e.to_string())
}

/// Debug / inspection endpoint for the per-conversation agent scratchpad.
#[tauri::command]
pub async fn get_agent_scratchpad_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<nexa_core::agent::scratchpad::AgentScratchpad>, String> {
    state
        .db
        .get_agent_scratchpad(&conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_user_memory_cmd(
    state: tauri::State<'_, AppState>,
    content: String,
) -> Result<nexa_core::personalization::UserMemory, String> {
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
) -> Result<nexa_core::personalization::UserMemory, String> {
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

#[tauri::command]
pub async fn list_agent_procedural_memories_cmd(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<AgentProceduralMemory>, String> {
    state
        .db
        .list_agent_procedural_memories(limit.unwrap_or(20).min(100) as usize)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_agent_procedural_memory_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .delete_agent_procedural_memory(&id)
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

// Settings Schema V2 remains compatible with AgentConfig during PR 8. These
// commands expose migration state and explicit rollback without coupling the
// future Settings UI to the legacy provider form.

#[tauri::command]
pub async fn get_settings_schema_state_v2_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<SettingsSchemaStateV2, String> {
    state
        .db
        .settings_schema_state_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_settings_profiles_v2_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SettingsProfileV2>, String> {
    state
        .db
        .list_settings_profiles_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_settings_profile_v2_cmd(
    state: tauri::State<'_, AppState>,
    profile: SettingsProfileV2,
    expected_revision: Option<u64>,
) -> Result<SettingsProfileV2, String> {
    state
        .db
        .save_settings_profile_v2(&profile, expected_revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_capability_binding_v2_cmd(
    state: tauri::State<'_, AppState>,
    scope: SettingsScopeV2,
    capability_id: String,
    binding: CapabilityBindingV2,
    expected_profile_revision: u64,
) -> Result<SettingsProfileV2, String> {
    state
        .db
        .save_capability_binding_v2(&scope, &capability_id, &binding, expected_profile_revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_vision_observation_cache_cmd(
    state: tauri::State<'_, AppState>,
    attachment_hash: String,
    profile_hash: Option<String>,
) -> Result<usize, String> {
    state
        .db
        .delete_vision_observation_cache(&attachment_hash, profile_hash.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clear_vision_observation_cache_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    state
        .db
        .clear_vision_observation_cache()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn migrate_settings_schema_v2_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<SettingsMigrationReportV2, String> {
    state
        .db
        .migrate_settings_schema_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn rollback_settings_schema_v2_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    state
        .db
        .rollback_settings_schema_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_capability_registry_projection_cmd(
    state: tauri::State<'_, AppState>,
    scope: RegistryScope,
) -> Result<CapabilityRegistryProjection, String> {
    state
        .db
        .capability_registry_projection(&scope)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_capability_registry_read_mode_cmd(
    state: tauri::State<'_, AppState>,
    capability_id: String,
    scope: SettingsScopeV2,
    mode: RegistryReadMode,
    expected_revision: u64,
) -> Result<RegistryActivationRecord, String> {
    state
        .db
        .set_registry_read_mode(&capability_id, &scope, mode, expected_revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn test_agent_connection_cmd(
    config: SaveAgentConfigInput,
) -> Result<ProviderModelCatalogSnapshot, String> {
    validate_agent_config_credential_contract(&config).map_err(|error| error.to_string())?;
    let provider_config = ProviderConfig {
        provider_type: provider_type_for_input(&config),
        api_key: Some(config.api_key.clone()),
        base_url: normalize_optional_base_url(config.base_url.clone()),
        org_id: None,
        timeout_secs: None,
        streaming: config.provider_streaming,
    };
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;

    provider
        .complete(&build_connection_probe_request(&config))
        .await
        .map_err(|e| e.to_string())?;

    let refreshed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let verified_model_id = config
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|model_id| !model_id.is_empty())
        .unwrap_or_else(|| config.model.trim());
    match provider.list_models().await {
        Ok(models) => Ok(build_effective_model_catalog(
            &config.provider,
            config.base_url.as_deref(),
            Some(models),
            Some(verified_model_id),
            refreshed_at,
        )),
        Err(error) => {
            warn!(
                "Connection probe succeeded but model listing failed for provider {}: {}",
                config.provider, error
            );
            Ok(build_effective_model_catalog(
                &config.provider,
                config.base_url.as_deref(),
                None,
                Some(verified_model_id),
                refreshed_at,
            ))
        }
    }
}

#[tauri::command]
pub async fn refresh_provider_model_catalog_cmd(
    config: SaveAgentConfigInput,
) -> Result<ProviderModelCatalogSnapshot, String> {
    validate_agent_config_credential_contract(&config).map_err(|error| error.to_string())?;
    let provider_config = ProviderConfig {
        provider_type: provider_type_for_input(&config),
        api_key: Some(config.api_key.clone()),
        base_url: normalize_optional_base_url(config.base_url.clone()),
        org_id: None,
        timeout_secs: None,
        streaming: config.provider_streaming,
    };
    let provider = create_provider(provider_config).map_err(|e| e.to_string())?;
    let refreshed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    match provider.list_models().await {
        Ok(models) => Ok(build_effective_model_catalog(
            &config.provider,
            config.base_url.as_deref(),
            Some(models),
            None,
            refreshed_at,
        )),
        Err(error) => {
            warn!(
                "Model catalog refresh failed for provider {}: {}",
                config.provider, error
            );
            Ok(build_effective_model_catalog(
                &config.provider,
                config.base_url.as_deref(),
                None,
                None,
                refreshed_at,
            ))
        }
    }
}

#[tauri::command]
pub async fn list_provider_presets_cmd() -> Result<Vec<ProviderPreset>, String> {
    load_provider_presets().map_err(|e| e.to_string())
}
