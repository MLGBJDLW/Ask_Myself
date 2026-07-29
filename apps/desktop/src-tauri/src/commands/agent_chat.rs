use super::*;
use crate::desktop_agent_session::{
    annotate_user_artifacts_with_execution_mode, build_desktop_agent_initial_task_artifacts,
    build_desktop_agent_session_config, build_desktop_agent_session_dependencies,
    build_desktop_agent_turn_config, build_desktop_agent_user_content_parts,
    build_desktop_running_agent_task, build_desktop_summarization_provider,
    finalize_desktop_agent_turn, replace_desktop_running_agent_task,
    request_desktop_running_agent_stop, run_desktop_agent_post_success_learning,
    run_desktop_agent_turn, steer_desktop_running_agent_task, DesktopAgentApprovalRuntime,
    DesktopAgentPostSuccessLearningRequest, DesktopAgentSessionConfigInput,
    DesktopAgentSessionDependencyRequest, DesktopAgentTurnConfigRequest,
    DesktopAgentTurnFinalization, DesktopAgentTurnRequest, DesktopAgentTurnRuntime,
    DesktopAgentTurnStream, DesktopAgentUserContentRequest, DesktopRunningAgentStopRequest,
    DesktopRunningAgentTaskRequest,
};
use nexa_core::runtime::{
    AgentTurnHandle, AgentTurnState, RuntimeTerminalStatus, StartTurnRequest,
    RUNTIME_PROTOCOL_VERSION,
};

// ── Agent Chat Command (streaming) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAgentChatLaunch {
    pub conversation_id: String,
    pub task_run_id: String,
    pub task_orchestrator_run_id: Option<String>,
    pub handle: AgentTurnHandle,
}

pub(super) struct DesktopAgentChatLaunchRequest<'a> {
    pub state: &'a AppState,
    pub agent_state: &'a AgentState,
    pub mcp_state: &'a McpManagerState,
    pub approval_state: &'a ApprovalState,
    pub terminal_state: Option<TerminalState>,
    pub app_handle: AppHandle,
    pub conversation_id: String,
    pub message: String,
    pub attachments: Option<Vec<ImageAttachment>>,
    pub agent_config_id: Option<String>,
    pub persona_id: Option<String>,
    pub skill_ids: Option<Vec<String>>,
    pub execution_mode: Option<String>,
    pub power_mode: Option<String>,
    pub user_artifacts: Option<serde_json::Value>,
    pub task_orchestrator_run_id: Option<String>,
    pub idempotency_key: String,
}

#[tauri::command]
pub async fn agent_chat_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    terminal_state: tauri::State<'_, TerminalState>,
    app_handle: AppHandle,
    mut request: StartTurnRequest,
) -> Result<AgentTurnHandle, String> {
    request.apply_protocol_defaults();
    if request.version != RUNTIME_PROTOCOL_VERSION {
        return Err(format!(
            "Unsupported runtime protocol version {}; expected {}.",
            request.version, RUNTIME_PROTOCOL_VERSION
        ));
    }

    launch_desktop_agent_chat_turn(DesktopAgentChatLaunchRequest {
        state: state.inner(),
        agent_state: agent_state.inner(),
        mcp_state: mcp_state.inner(),
        approval_state: approval_state.inner(),
        terminal_state: Some(terminal_state.inner().clone()),
        app_handle,
        conversation_id: request.conversation_id,
        message: request.message,
        attachments: Some(request.attachments),
        agent_config_id: request.agent_config_id,
        persona_id: request.persona_id,
        skill_ids: Some(request.skill_ids),
        execution_mode: Some(request.execution_mode.as_str().to_string()),
        power_mode: Some(request.power_mode.as_str().to_string()),
        user_artifacts: request.user_artifacts,
        task_orchestrator_run_id: request.task_orchestrator_run_id,
        idempotency_key: request.idempotency_key,
    })
    .await
    .map(|launch| launch.handle)
}

pub(super) async fn launch_desktop_agent_chat_turn(
    request: DesktopAgentChatLaunchRequest<'_>,
) -> Result<DesktopAgentChatLaunch, String> {
    let DesktopAgentChatLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        terminal_state,
        app_handle,
        conversation_id,
        message,
        attachments,
        agent_config_id,
        persona_id,
        skill_ids,
        execution_mode,
        power_mode,
        user_artifacts,
        task_orchestrator_run_id,
        idempotency_key,
    } = request;
    let execution_mode = AgentExecutionMode::from_wire(execution_mode.as_deref())?;
    let power_mode = AgentPowerMode::from_wire(power_mode.as_deref())?;
    let plan_mode = execution_mode.is_plan();
    let task_orchestrator_run_id = task_orchestrator_run_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    // 1. Load the conversation first so provider/model selection follows the
    // active chat, not whatever global default happened to be selected later.
    let mut conv = state
        .db
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;
    if conv.archived_at.is_some() {
        return Err(
            "Archived conversations are read-only. Restore the conversation before continuing."
                .to_string(),
        );
    }
    sync_conversation_goal_from_user_artifacts(
        state.db.as_ref(),
        &conversation_id,
        user_artifacts.as_ref(),
    )?;

    // 2. Resolve the best matching agent config for this conversation.
    let db_config =
        select_agent_config_for_conversation(&state.db, &conv, agent_config_id.as_deref())?;
    if conv.provider != db_config.provider || conv.model != db_config.model {
        state
            .db
            .update_conversation_model(&conversation_id, &db_config.provider, &db_config.model)
            .map_err(|e| e.to_string())?;
        conv.provider = db_config.provider.clone();
        conv.model = db_config.model.clone();
    }

    // 3. Create LLM provider.
    let app_cfg = state.db.load_app_config().unwrap_or_default();
    let provider_config = db_config_to_provider_config(&db_config, None);
    let provider = create_provider(provider_config.clone()).map_err(|e| e.to_string())?;

    // 4. Load conversation history and convert to LLM messages.
    let existing_msgs = state
        .db
        .get_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    let history: Vec<Message> = existing_msgs.iter().map(conv_message_to_llm).collect();
    let history = sanitize_tool_call_history(history);
    let next_sort_order = existing_msgs.len() as i64;

    if let Some(run_id) = task_orchestrator_run_id.as_deref() {
        let workflow_run = state
            .db
            .get_workflow_automation_run(run_id)
            .map_err(|err| err.to_string())?;
        let projected_status =
            nexa_core::task_orchestrator::project_task_status(&workflow_run.status)
                .map_err(|err| err.to_string())?;
        let existing_launch = state
            .db
            .find_agent_turn_by_idempotency_key(&conversation_id, &idempotency_key)
            .map_err(|err| err.to_string())?;
        if projected_status.state != nexa_core::task_orchestrator::TaskOrchestratorState::Queued
            && existing_launch.is_none()
        {
            return Err(format!(
                "Task Orchestrator run {run_id} must be queued to start; got {}.",
                projected_status.raw_status
            ));
        }
        let automation = state
            .db
            .get_workflow_automation(&workflow_run.automation_id)
            .map_err(|err| err.to_string())?;
        super::workflows::ensure_workflow_template_runtime_visible(
            state.db.as_ref(),
            &automation.workflow_template_id,
        )?;
    }

    // 5. Save user message to DB.
    let persisted_user_artifacts =
        annotate_user_artifacts_with_execution_mode(user_artifacts, execution_mode, power_mode);
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: message.clone(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: persisted_user_artifacts,
        token_count: estimate_tokens(&message),
        created_at: String::new(),
        sort_order: next_sort_order,
        thinking: None,
        image_attachments: attachments.as_ref().and_then(|atts| {
            if atts.is_empty() {
                None
            } else {
                Some(atts.clone())
            }
        }),
    };
    let user_llm_content = conversation_message_llm_context_content(&user_msg).to_string();
    let launch_record = state
        .db
        .create_agent_turn_and_run(
            &user_msg,
            &task_title_from_message(&message),
            Some(&db_config.provider),
            Some(&db_config.model),
            &idempotency_key,
        )
        .map_err(|e| e.to_string())?;
    if launch_record.reused {
        return Ok(desktop_agent_chat_launch(
            &launch_record,
            task_orchestrator_run_id,
        ));
    }
    let turn = state
        .db
        .get_conversation_turn(&launch_record.turn_id)
        .map_err(|e| e.to_string())?;
    let task_run = state
        .db
        .get_agent_task_run(&launch_record.run_id)
        .map_err(|e| e.to_string())?;
    let task_run_id_for_command = task_run.id.clone();
    if let Some(run_id) = task_orchestrator_run_id.as_deref() {
        state
            .db
            .start_workflow_automation_run(run_id, &task_run.id, None)
            .map_err(|err| err.to_string())?;
    }
    let stream_event_seq = Arc::new(AtomicU64::new(0));
    let terminal_emitted = Arc::new(AtomicBool::new(false));
    emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &task_run.id);
    record_agent_run_status_task_event(
        &state.db,
        &app_handle,
        &conversation_id,
        &task_run.id,
        Some(&turn.id),
        &stream_event_seq,
        AgentRunPhase::Routing,
        "Task queued",
        Some("queued"),
        None,
    );

    // 6. Build Desktop Agent Session turn config from conversation context.
    let requested_skill_ids = skill_ids.unwrap_or_default();
    let desktop_turn_config = build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
        db: &state.db,
        conversation: &conv,
        turn_id: &turn.id,
        message: &message,
        persona_id: persona_id.as_deref(),
        explicit_skill_ids: &requested_skill_ids,
        db_config: &db_config,
        app_cfg: &app_cfg,
        execution_mode,
        power_mode,
    });
    let source_scope_ids = desktop_turn_config.source_scope_ids;
    let pinned_skill_ids = desktop_turn_config.pinned_skill_ids;
    let executor_config = desktop_turn_config.executor_config;

    let summarization_provider = build_desktop_summarization_provider(&db_config);

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();
    let (steering_tx, steering_rx) = tokio::sync::mpsc::unbounded_channel::<AgentSteeringMessage>();
    let session_dependencies =
        build_desktop_agent_session_dependencies(DesktopAgentSessionDependencyRequest {
            db: &state.db,
            mcp_manager: &mcp_state.manager,
            app_handle: &app_handle,
            event_seq: &stream_event_seq,
            conversation_id: &conversation_id,
            task_run_id: &task_run.id,
            turn_id: &turn.id,
            message: &message,
            pinned_skill_ids: &pinned_skill_ids,
            provider_config: provider_config.clone(),
            executor_config: executor_config.clone(),
            subagent_allowed_tools: db_config.subagent_allowed_tools.clone(),
            subagent_allowed_skill_ids: db_config.subagent_allowed_skill_ids.clone(),
            cancel_token: cancel_token.clone(),
            plan_mode,
            mcp_call_timeout_secs: DEFAULT_MCP_CALL_TIMEOUT_SECS,
            terminal_state,
        })
        .await;
    let runtime_session_config =
        build_desktop_agent_session_config(DesktopAgentSessionConfigInput {
            db: state.db.as_ref(),
            conversation_id: &conversation_id,
            task_run_id: &task_run.id,
            db_config: &db_config,
            app_cfg: &app_cfg,
            source_scope_ids: &source_scope_ids,
            selected_skills: &session_dependencies.selected_skills,
            auto_loaded_skills: &session_dependencies.auto_loaded_skills,
            execution_mode,
        });
    let initial_task_artifacts = build_desktop_agent_initial_task_artifacts(
        &session_dependencies.selected_skills,
        &runtime_session_config,
        execution_mode,
        &executor_config,
    );
    let _ = state.db.update_agent_task_run_progress(
        &task_run.id,
        None,
        None,
        None,
        None,
        None,
        Some(&initial_task_artifacts),
    );
    emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &task_run.id);

    // 7b. Build user content parts (text + optional attachments).
    let user_parts = build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
        db: &state.db,
        app_handle: Some(&app_handle),
        provider_config: &provider_config,
        db_config: &db_config,
        message: &user_llm_content,
        attachments: attachments.as_deref(),
    })?;

    // 8. Spawn the agent loop in a background task.
    let db = state.db.clone();
    let conv_id = conversation_id.clone();
    let turn_id = turn.id.clone();
    let task_run_id = task_run.id.clone();
    let handle = app_handle.clone();
    let assistant_sort_order = next_sort_order + 1;
    let db_config_for_post_success = db_config.clone();
    let task_orchestrator_run_id_for_task = task_orchestrator_run_id.clone();
    let approval_runtime = DesktopAgentApprovalRuntime {
        pending: approval_state.pending.clone(),
        session_store: approval_state.session_store.clone(),
        approval_mode: app_cfg.tool_approval_mode,
    };

    let turn_timeout_secs = executor_config.agent_timeout_secs.unwrap_or(0) as u64;

    const STREAM_KEEPALIVE_INTERVAL_SECS: u64 = 10;

    state
        .db
        .mark_agent_task_run_started(&task_run.id, "initializing")
        .map_err(|e| e.to_string())?;
    emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &task_run.id);
    record_agent_run_status_task_event(
        &state.db,
        &app_handle,
        &conversation_id,
        &task_run.id,
        Some(&turn_id),
        &stream_event_seq,
        AgentRunPhase::Routing,
        "Agent started",
        Some("running"),
        None,
    );

    let stream_event_seq_for_task = Arc::clone(&stream_event_seq);
    let task = tokio::spawn(async move {
        let outcome = run_desktop_agent_turn(DesktopAgentTurnRequest {
            provider,
            dependencies: session_dependencies,
            executor_config,
            cancel_token: cancel_token_clone,
            steering_rx,
            approval_runtime,
            summarization_provider,
            history,
            user_parts,
            db: db.clone(),
            conversation_id: conv_id.clone(),
            turn_id: turn_id.clone(),
            assistant_sort_order,
            runtime: DesktopAgentTurnRuntime {
                timeout_secs: turn_timeout_secs,
                keepalive_interval_secs: STREAM_KEEPALIVE_INTERVAL_SECS,
            },
            stream: DesktopAgentTurnStream {
                app_handle: handle.clone(),
                task_run_id: task_run_id.clone(),
                event_seq: Arc::clone(&stream_event_seq_for_task),
                terminal_emitted: Arc::clone(&terminal_emitted),
            },
        })
        .await;
        let result = &outcome.result;

        match result {
            Some(Ok(_)) => {}
            Some(Err(CoreError::Cancelled(message))) => {
                warn!("Agent execution cancelled for conversation {conv_id}: {message}");
                let payload = serde_json::json!({ "reason": message });
                emit_terminal_agent_error_once(
                    terminal_emitted.as_ref(),
                    &db,
                    &handle,
                    stream_event_seq_for_task.as_ref(),
                    TerminalAgentError {
                        conversation_id: &conv_id,
                        task_run_id: &task_run_id,
                        turn_id: &turn_id,
                        message: "Agent execution cancelled.",
                        status: "cancelled",
                        payload: Some(&payload),
                    },
                );
            }
            Some(Err(e)) => {
                warn!("Agent execution failed for conversation {conv_id}: {e}");
                emit_terminal_agent_error_once(
                    terminal_emitted.as_ref(),
                    &db,
                    &handle,
                    stream_event_seq_for_task.as_ref(),
                    TerminalAgentError {
                        conversation_id: &conv_id,
                        task_run_id: &task_run_id,
                        turn_id: &turn_id,
                        message: "Agent execution failed unexpectedly.",
                        status: "failed",
                        payload: None,
                    },
                );
            }
            None => {
                warn!("Agent execution timed out for conversation {conv_id}");
                let payload = serde_json::json!({ "reason": "timeout" });
                emit_terminal_agent_error_once(
                    terminal_emitted.as_ref(),
                    &db,
                    &handle,
                    stream_event_seq_for_task.as_ref(),
                    TerminalAgentError {
                        conversation_id: &conv_id,
                        task_run_id: &task_run_id,
                        turn_id: &turn_id,
                        message: "Agent execution timed out.",
                        status: "timed_out",
                        payload: Some(&payload),
                    },
                );
            }
        }

        finalize_desktop_agent_turn(DesktopAgentTurnFinalization {
            db: &db,
            app_handle: &handle,
            conversation_id: &conv_id,
            task_run_id: &task_run_id,
            task_orchestrator_run_id: task_orchestrator_run_id_for_task.as_deref(),
            turn_id: &turn_id,
            event_seq: stream_event_seq_for_task.as_ref(),
            outcome: &outcome,
        });

        if matches!(result, Some(Ok(_))) {
            run_desktop_agent_post_success_learning(DesktopAgentPostSuccessLearningRequest {
                db: db.clone(),
                conversation_id: conv_id.clone(),
                db_config: db_config_for_post_success,
            })
            .await;
        }
    });

    // 8. Track the running task for potential cancellation.
    let launch = DesktopAgentChatLaunch {
        conversation_id: conversation_id.clone(),
        task_run_id: task_run_id_for_command.clone(),
        task_orchestrator_run_id: task_orchestrator_run_id.clone(),
        handle: AgentTurnHandle::running(
            conversation_id.clone(),
            task_run_id_for_command.clone(),
            turn.id.clone(),
        ),
    };
    {
        let mut running = agent_state.running.lock().await;
        let running_task = build_desktop_running_agent_task(DesktopRunningAgentTaskRequest {
            cancel_token,
            task,
            steering_tx,
            task_run_id: task_run_id_for_command,
            task_orchestrator_run_id,
            turn_id: turn.id.clone(),
            stream_event_seq: Arc::clone(&stream_event_seq),
        });
        replace_desktop_running_agent_task(&mut running, conversation_id, running_task);
    }

    Ok(launch)
}

fn desktop_agent_chat_launch(
    record: &nexa_core::conversation::AgentTurnLaunchRecord,
    task_orchestrator_run_id: Option<String>,
) -> DesktopAgentChatLaunch {
    let state = match record.status.as_str() {
        "queued" => AgentTurnState::Starting,
        "running" | "cancelling" => AgentTurnState::Running,
        "waiting_approval" => AgentTurnState::WaitingApproval,
        "completed" | "success" | "cached" => {
            AgentTurnState::Terminal(RuntimeTerminalStatus::Completed)
        }
        "cancelled" => AgentTurnState::Terminal(RuntimeTerminalStatus::Cancelled),
        "timed_out" => AgentTurnState::Terminal(RuntimeTerminalStatus::TimedOut),
        _ => AgentTurnState::Terminal(RuntimeTerminalStatus::Failed),
    };
    DesktopAgentChatLaunch {
        conversation_id: record.conversation_id.clone(),
        task_run_id: record.run_id.clone(),
        task_orchestrator_run_id,
        handle: AgentTurnHandle {
            session_id: record.conversation_id.clone(),
            run_id: record.run_id.clone(),
            turn_id: record.turn_id.clone(),
            state,
        },
    }
}

fn sync_conversation_goal_from_user_artifacts(
    db: &Database,
    conversation_id: &str,
    user_artifacts: Option<&serde_json::Value>,
) -> Result<(), String> {
    let artifact = user_artifacts.and_then(serde_json::Value::as_object);
    let is_goal_command = artifact
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| matches!(kind, "goal" | "agentGoal"));

    if is_goal_command {
        let status = artifact
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("active")
            .trim()
            .to_ascii_lowercase();
        if matches!(
            status.as_str(),
            "clear" | "cleared" | "cancel" | "cancelled"
        ) {
            return db
                .clear_conversation_goal(conversation_id)
                .map_err(|error| error.to_string());
        }

        let objective = artifact
            .and_then(|value| value.get("objective"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Goal objective cannot be empty.".to_string())?;
        db.set_conversation_goal(conversation_id, objective)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    if db
        .get_conversation_goal(conversation_id)
        .map_err(|error| error.to_string())?
        .is_some_and(|goal| goal.status == nexa_core::conversation::ConversationGoalStatus::Blocked)
    {
        db.update_conversation_goal(
            conversation_id,
            nexa_core::conversation::ConversationGoalStatus::Active,
            None,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

// ── Model Context Window ─────────────────────────────────────────────────

#[tauri::command]
pub fn get_model_context_window(model: String) -> u32 {
    nexa_core::conversation::memory::model_context_window(&model)
}

// ── Agent Steering Command ──────────────────────────────────────────────

#[tauri::command]
pub async fn agent_steer_cmd(
    agent_state: tauri::State<'_, AgentState>,
    conversation_id: String,
    message: String,
) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("Steering message cannot be empty.".to_string());
    }

    let mut running = agent_state.running.lock().await;
    steer_desktop_running_agent_task(&mut running, &conversation_id, trimmed)
}

// ── Agent Stop Command ──────────────────────────────────────────────────

#[tauri::command]
pub async fn agent_stop_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    app_handle: AppHandle,
    conversation_id: String,
) -> Result<(), String> {
    let mut running = agent_state.running.lock().await;
    request_desktop_running_agent_stop(
        &mut running,
        DesktopRunningAgentStopRequest {
            db: state.db.clone(),
            app_handle,
            conversation_id,
        },
    );
    Ok(())
}
