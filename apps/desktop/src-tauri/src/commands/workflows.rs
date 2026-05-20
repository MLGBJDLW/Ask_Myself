use super::*;
use nexa_core::tools::{browser_evidence_tool::BrowserEvidenceCaptureTool, Tool};
use nexa_core::workflow_automation::{
    BrowserEvidenceCapture, InvestigationGraph, LearningGovernanceSnapshot,
    SaveWorkflowAutomationInput, TaskResumeCheckpoint, TaskResumePrompt, WorkflowAutomation,
    WorkflowAutomationApprovalPolicy, WorkflowAutomationDueRun, WorkflowAutomationRun,
};

#[tauri::command]
pub async fn save_workflow_automation_cmd(
    state: tauri::State<'_, AppState>,
    input: SaveWorkflowAutomationInput,
) -> Result<WorkflowAutomation, String> {
    state
        .db
        .save_workflow_automation(&input)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_workflow_automations_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WorkflowAutomation>, String> {
    state
        .db
        .list_workflow_automations()
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn delete_workflow_automation_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .delete_workflow_automation(&id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn set_workflow_automation_enabled_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<WorkflowAutomation, String> {
    let existing = state
        .db
        .get_workflow_automation(&id)
        .map_err(|err| err.to_string())?;
    state
        .db
        .save_workflow_automation(&SaveWorkflowAutomationInput {
            id: Some(existing.id),
            name: existing.name,
            description: existing.description,
            workflow_template_id: existing.workflow_template_id,
            prompt: existing.prompt,
            trigger: existing.trigger,
            source_scope: existing.source_scope,
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: existing.approval_policy.require_before_run,
                allowed_tools: existing.approval_policy.allowed_tools,
                risk_level: existing.approval_policy.risk_level,
            },
            enabled,
        })
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_due_workflow_automations_cmd(
    state: tauri::State<'_, AppState>,
    now: Option<String>,
) -> Result<Vec<WorkflowAutomationDueRun>, String> {
    let now = now.unwrap_or_else(|| Utc::now().to_rfc3339());
    state
        .db
        .list_due_workflow_automations(&now)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn preview_workflow_automation_prompt_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    state
        .db
        .preview_workflow_automation_prompt(&id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn record_workflow_automation_run_cmd(
    state: tauri::State<'_, AppState>,
    automation_id: String,
    task_run_id: Option<String>,
    status: String,
    summary: Option<String>,
) -> Result<WorkflowAutomationRun, String> {
    state
        .db
        .record_workflow_automation_run(
            &automation_id,
            task_run_id.as_deref(),
            &status,
            summary.as_deref(),
        )
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_task_resume_checkpoints_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<TaskResumeCheckpoint>, String> {
    state
        .db
        .list_task_resume_checkpoints(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_task_resume_prompt_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<TaskResumePrompt, String> {
    state
        .db
        .build_task_resume_prompt(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn pause_agent_task_run_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    app_handle: AppHandle,
    run_id: String,
) -> Result<TaskResumeCheckpoint, String> {
    let run = state
        .db
        .get_agent_task_run(&run_id)
        .map_err(|err| err.to_string())?;
    let checkpoint = state
        .db
        .pause_task_run_with_checkpoint(&run_id, "user_pause")
        .map_err(|err| err.to_string())?;

    let mut running = agent_state.running.lock().await;
    if let Some(task_state) = running.remove(&run.conversation_id) {
        if task_state.task_run_id == run_id {
            let stream_event_seq = Arc::clone(&task_state.stream_event_seq);
            let run_event = emit_agent_frontend_event(
                &app_handle,
                stream_event_seq.as_ref(),
                &run.conversation_id,
                &run_id,
                Some(&task_state.turn_id),
                AgentEvent::Status {
                    content: "Pause checkpoint saved".to_string(),
                    tone: Some("muted".to_string()),
                },
            );
            record_agent_run_task_event(
                &state.db,
                &app_handle,
                &run.conversation_id,
                &run_id,
                &run_event,
                run_event.task_event_type(),
                "Pause checkpoint saved",
                Some("paused"),
                Some(&serde_json::json!({
                    "checkpointId": checkpoint.id,
                    "reason": "user_pause"
                })),
            );
            emit_agent_task_run_update(&state.db, &app_handle, &run.conversation_id, &run_id);
            task_state.cancel_token.cancel();
            let abort_task = task_state.task;
            let db = state.db.clone();
            let handle = app_handle.clone();
            let conv_id = run.conversation_id.clone();
            let turn_id = task_state.turn_id.clone();
            let checkpoint_id = checkpoint.id.clone();
            let resume_prompt = checkpoint.resume_prompt.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                if !abort_task.is_finished() {
                    abort_task.abort();
                    let artifacts = serde_json::json!({
                        "kind": "resumeCheckpoint",
                        "checkpointId": checkpoint_id,
                        "resumePrompt": resume_prompt,
                    });
                    let _ = db.finish_agent_task_run(
                        &run_id,
                        "paused",
                        Some("Paused with a resumable checkpoint"),
                        None,
                        Some(&artifacts),
                    );
                    record_agent_run_status_task_event(
                        &db,
                        &handle,
                        &conv_id,
                        &run_id,
                        Some(&turn_id),
                        stream_event_seq.as_ref(),
                        AgentRunPhase::Done,
                        "Paused with a resumable checkpoint",
                        Some("paused"),
                        Some(&artifacts),
                    );
                    emit_agent_task_run_update(&db, &handle, &conv_id, &run_id);
                }
            });
        } else {
            running.insert(run.conversation_id.clone(), task_state);
        }
    }

    Ok(checkpoint)
}

#[tauri::command]
pub async fn get_investigation_graph_cmd(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<InvestigationGraph, String> {
    state
        .db
        .build_investigation_graph(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn get_learning_governance_snapshot_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<LearningGovernanceSnapshot, String> {
    state
        .db
        .learning_governance_snapshot()
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn capture_browser_evidence_cmd(
    state: tauri::State<'_, AppState>,
    url: String,
    max_length: Option<usize>,
    mode: Option<String>,
) -> Result<BrowserEvidenceCapture, String> {
    let args = serde_json::json!({
        "url": url,
        "max_length": max_length.unwrap_or(6000),
        "mode": mode.unwrap_or_else(|| "auto".to_string()),
    })
    .to_string();
    let tool = BrowserEvidenceCaptureTool;
    let result = tool
        .execute("manual-browser-evidence-capture", &args, &state.db, &[])
        .await
        .map_err(|err| err.to_string())?;
    if result.is_error {
        return Err(result.content);
    }
    let output = result.output_channels();
    output
        .data
        .and_then(|value| serde_json::from_value::<BrowserEvidenceCapture>(value).ok())
        .ok_or_else(|| "Browser evidence capture did not return structured data.".to_string())
}
