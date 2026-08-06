use super::*;
use crate::browser::BrowserState;
use crate::desktop_agent_session::{
    annotate_user_artifacts_with_execution_mode, build_desktop_agent_initial_task_artifacts,
    build_desktop_agent_session_config, build_desktop_agent_session_dependencies,
    build_desktop_agent_turn_config, build_desktop_agent_vision_user_content,
    finalize_desktop_agent_turn, provider_config_egress_id, provider_config_is_local,
    request_desktop_running_agent_stop, resolve_desktop_summarization_provider_config,
    run_desktop_agent_post_success_learning,
    run_desktop_agent_turn, DesktopAgentApprovalRuntime, DesktopAgentPostSuccessLearningRequest,
    DesktopAgentSessionConfigInput, DesktopAgentSessionDependencyRequest,
    DesktopAgentTurnConfigRequest, DesktopAgentTurnFinalization, DesktopAgentTurnRequest,
    DesktopAgentTurnRuntime, DesktopAgentTurnStream, DesktopAgentVisionUserContentRequest,
    DesktopRunningAgentStopRequest,
};
use nexa_core::llm::ReasoningEffort;
use nexa_core::mixture_of_agents::{
    AgentCollaborationMode, MoaAdvisor, MoaPreset, MoaPresetId, MoaProvider,
};
use nexa_core::quality_profile::{CustomOrchestrationOptions, OrchestrationProfile};
use nexa_core::runtime::{
    ActiveAgentTurn, AgentRunEventSequencer, AgentTurnHandle, AgentTurnState,
    RuntimeTerminalStatus, StartTurnRequest, TurnLaunchStage, RUNTIME_PROTOCOL_VERSION,
};
use nexa_core::vision_router::VisionTurnOverride;

fn normalize_turn_attachments(
    attachments: Vec<ImageAttachment>,
) -> Result<Vec<ImageAttachment>, String> {
    attachments
        .into_iter()
        .enumerate()
        .map(|(index, mut attachment)| {
            attachment.vision_analysis = None;
            if !attachment.media_type.starts_with("image/") {
                return Ok(attachment);
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&attachment.base64_data)
                .map_err(|error| {
                    format!(
                        "Failed to decode image attachment '{}': {error}",
                        attachment.original_name
                    )
                })?;
            let computed_hash = nexa_core::vision_router::attachment_hash(&bytes);
            if attachment
                .attachment_hash
                .as_deref()
                .is_some_and(|provided| !provided.eq_ignore_ascii_case(&computed_hash))
            {
                return Err(format!(
                    "Image attachment '{}' changed after preparation",
                    attachment.original_name
                ));
            }
            attachment.attachment_hash = Some(computed_hash.clone());
            if attachment
                .attachment_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
            {
                attachment.attachment_id =
                    Some(format!("attachment-{index}-{}", &computed_hash[..24]));
            }
            Ok(attachment)
        })
        .collect()
}

fn build_moa_provider(
    db: &Database,
    aggregator_config: &DbAgentConfig,
    aggregator: Box<dyn nexa_core::llm::LlmProvider>,
    preset_id: MoaPresetId,
) -> Result<Box<dyn nexa_core::llm::LlmProvider>, String> {
    let mut candidates = db.list_agent_configs().map_err(|error| error.to_string())?;
    candidates.sort_by_key(|config| config.id == aggregator_config.id);
    if candidates.is_empty() {
        candidates.push(aggregator_config.clone());
    }

    let mut preset = MoaPreset::builtin(
        preset_id,
        &aggregator_config.provider,
        &aggregator_config.model,
    );
    let mut advisors = Vec::new();
    for (index, template_slot) in preset.references.iter().cloned().enumerate() {
        let config = &candidates[index % candidates.len()];
        let Ok(provider) = create_provider(db_config_to_provider_config(config, None)) else {
            continue;
        };
        let mut slot = template_slot;
        slot.provider = config.provider.clone();
        slot.model = config.model.clone();
        slot.reasoning_effort = config
            .reasoning_effort
            .as_deref()
            .and_then(|effort| match effort.trim().to_ascii_lowercase().as_str() {
                "none" => Some(ReasoningEffort::None),
                "minimal" => Some(ReasoningEffort::Minimal),
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                "max" => Some(ReasoningEffort::Max),
                "xhigh" => Some(ReasoningEffort::XHigh),
                _ => None,
            });
        advisors.push(MoaAdvisor {
            slot,
            provider: Arc::from(provider),
        });
    }
    preset.references = advisors
        .iter()
        .map(|advisor| advisor.slot.clone())
        .collect();
    let provider = MoaProvider::new(Arc::from(aggregator), preset, advisors)
        .map_err(|error| error.to_string())?;
    Ok(Box::new(provider))
}

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
    pub browser_state: BrowserState,
    pub app_handle: AppHandle,
    pub conversation_id: String,
    pub message: String,
    pub attachments: Option<Vec<ImageAttachment>>,
    pub agent_config_id: Option<String>,
    pub persona_id: Option<String>,
    pub skill_ids: Option<Vec<String>>,
    pub execution_mode: Option<String>,
    pub power_mode: Option<String>,
    pub collaboration_mode: Option<String>,
    pub moa_preset: Option<String>,
    pub orchestration_profile: Option<String>,
    pub custom_orchestration: Option<CustomOrchestrationOptions>,
    pub vision_turn_override: Option<VisionTurnOverride>,
    pub user_artifacts: Option<serde_json::Value>,
    pub task_orchestrator_run_id: Option<String>,
    pub idempotency_key: String,
}

struct InteractionSubmissionArtifact {
    interaction_id: String,
    answers: InteractionAnswers,
}

fn interaction_submission_from_artifacts(
    artifacts: Option<&serde_json::Value>,
) -> Result<Option<InteractionSubmissionArtifact>, String> {
    let Some(artifact) = artifacts.and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    if artifact.get("kind").and_then(serde_json::Value::as_str) != Some("questionResponse") {
        return Ok(None);
    }
    let Some(interaction_id) = artifact
        .get("interactionId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        // Version 1 question responses intentionally remain on the legacy path.
        return Ok(None);
    };
    let raw_answers = artifact
        .get("answers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Durable question response is missing its answers.".to_string())?;
    let mut answers = InteractionAnswers::new();
    for raw_answer in raw_answers {
        let answer = raw_answer
            .as_object()
            .ok_or_else(|| "Durable question response contains an invalid answer.".to_string())?;
        let id = answer
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "Durable question response contains an answer without an id.".to_string()
            })?;
        let values = answer
            .get("answers")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("Durable question response `{id}` has invalid values."))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("Durable question response `{id}` must contain text."))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if answers.insert(id.to_string(), values).is_some() {
            return Err(format!(
                "Durable question response contains duplicate answer id `{id}`."
            ));
        }
    }
    Ok(Some(InteractionSubmissionArtifact {
        interaction_id: interaction_id.to_string(),
        answers,
    }))
}

fn interaction_response_from_user_artifacts(
    db: &Database,
    conversation_id: &str,
    artifacts: Option<&serde_json::Value>,
) -> Result<Option<SubmitInteractionResponse>, String> {
    let Some(submission) = interaction_submission_from_artifacts(artifacts)? else {
        return Ok(None);
    };
    let mut request = db
        .get_interaction_request(&submission.interaction_id)
        .map_err(|error| error.to_string())?;
    if request.conversation_id != conversation_id {
        return Err("Interaction response belongs to a different conversation.".to_string());
    }
    if request.status == InteractionStatus::Pending {
        request = db
            .mark_interaction_presented(&request.interaction_id)
            .map_err(|error| error.to_string())?;
    }
    Ok(Some(SubmitInteractionResponse {
        interaction_id: submission.interaction_id,
        resume_token: request.resume_token,
        answers: submission.answers,
    }))
}

#[tauri::command]
pub async fn agent_chat_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    terminal_state: tauri::State<'_, TerminalState>,
    browser_state: tauri::State<'_, BrowserState>,
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
        browser_state: browser_state.inner().clone(),
        app_handle,
        conversation_id: request.conversation_id,
        message: request.message,
        attachments: Some(request.attachments),
        agent_config_id: request.agent_config_id,
        persona_id: request.persona_id,
        skill_ids: Some(request.skill_ids),
        execution_mode: Some(request.execution_mode.as_str().to_string()),
        power_mode: Some(request.power_mode.as_str().to_string()),
        collaboration_mode: Some(request.collaboration_mode.as_str().to_string()),
        moa_preset: Some(request.moa_preset.as_str().to_string()),
        orchestration_profile: Some(request.orchestration_profile.as_str().to_string()),
        custom_orchestration: request.custom_orchestration,
        vision_turn_override: request.vision_turn_override,
        user_artifacts: request.user_artifacts,
        task_orchestrator_run_id: request.task_orchestrator_run_id,
        idempotency_key: request.idempotency_key,
    })
    .await
    .map(|launch| launch.handle)
}

#[tauri::command]
pub async fn record_agent_frontend_paint_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    app_handle: AppHandle,
    conversation_id: String,
    run_id: String,
    turn_id: String,
    elapsed_ms: u64,
) -> Result<(), String> {
    if !agent_state
        .sessions
        .claim_frontend_paint_metric(&conversation_id, &run_id, &turn_id)
        .await
    {
        return Ok(());
    }
    let elapsed_ms = elapsed_ms.min(60 * 60 * 1_000);
    let payload = serde_json::json!({
        "kind": "turnLaunchMetric",
        "stage": TurnLaunchStage::FrontendFirstPaintMs.as_str(),
        "elapsedMs": elapsed_ms,
        "turnId": turn_id,
    });
    state
        .db
        .record_agent_task_run_event(
            &run_id,
            "telemetry",
            TurnLaunchStage::FrontendFirstPaintMs.as_str(),
            Some("recorded"),
            Some(&payload),
        )
        .map_err(|error| error.to_string())?;
    emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &run_id);
    Ok(())
}

pub(super) async fn launch_desktop_agent_chat_turn(
    request: DesktopAgentChatLaunchRequest<'_>,
) -> Result<DesktopAgentChatLaunch, String> {
    let launch_started = Instant::now();
    let DesktopAgentChatLaunchRequest {
        state,
        agent_state,
        mcp_state,
        approval_state,
        terminal_state,
        browser_state,
        app_handle,
        conversation_id,
        message,
        attachments,
        agent_config_id,
        persona_id,
        skill_ids,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
        custom_orchestration,
        vision_turn_override,
        user_artifacts,
        task_orchestrator_run_id,
        idempotency_key,
    } = request;
    let execution_mode = AgentExecutionMode::from_wire(execution_mode.as_deref())?;
    let power_mode = AgentPowerMode::from_wire(power_mode.as_deref())?;
    let collaboration_mode = AgentCollaborationMode::from_wire(collaboration_mode.as_deref())?;
    let moa_preset = MoaPresetId::from_wire(moa_preset.as_deref())?;
    let orchestration_profile = OrchestrationProfile::from_wire(orchestration_profile.as_deref())?;
    let attachments = normalize_turn_attachments(attachments.unwrap_or_default())?;
    let attachments = (!attachments.is_empty()).then_some(attachments);
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
    let vision_attachment_hashes = attachments
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|attachment| attachment.media_type.starts_with("image/"))
        .filter_map(|attachment| attachment.attachment_hash.clone())
        .collect::<Vec<_>>();
    if !vision_attachment_hashes.is_empty() {
        let preflight_scope = nexa_core::capability_registry::RegistryScope {
            workspace_id: conv.project_id.clone(),
            agent_id: Some(db_config.id.clone()),
            task_id: None,
        };
        let projection = state
            .db
            .capability_registry_projection(&preflight_scope)
            .map_err(|error| format!("Vision policy preflight failed: {error}"))?;
        let policy = projection
            .capabilities
            .iter()
            .find(|route| route.capability_id == "vision")
            .map(|route| {
                nexa_core::vision_router::VisionRouterPolicy::from_binding_options(&route.options)
            })
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        if policy.mode == nexa_core::vision_router::VisionMode::Ask
            && vision_turn_override.is_none()
        {
            return Err(
                "decision_required: choose Auto, OCR only, or Vision only before this turn is created"
                    .to_string(),
            );
        }
    }

    // Validate orchestrated launches before allocating their durable run.
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

    let interaction_response = interaction_response_from_user_artifacts(
        state.db.as_ref(),
        &conversation_id,
        user_artifacts.as_ref(),
    )?;
    let resumes_interaction = interaction_response.is_some();

    // Persist only the minimal durable launch tuple before acknowledging. The
    // database allocates sort order inside this same transaction, avoiding a
    // full history read and a concurrent MAX(sort_order) race on the hot path.
    let mut persisted_user_artifacts = annotate_user_artifacts_with_execution_mode(
        user_artifacts,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
    );
    if let Some(turn_override) = vision_turn_override {
        let artifacts = persisted_user_artifacts
            .get_or_insert_with(|| serde_json::json!({ "kind": "messageContextChannels" }));
        if let Some(map) = artifacts.as_object_mut() {
            map.insert(
                "visionTurnOverride".to_string(),
                serde_json::Value::String(turn_override.as_str().to_string()),
            );
            map.insert(
                "visionOverrideAttachmentHashes".to_string(),
                serde_json::Value::Array(
                    vision_attachment_hashes
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
    }
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
        sort_order: 0,
        thinking: None,
        image_attachments: attachments.as_ref().and_then(|atts| {
            if atts.is_empty() {
                None
            } else {
                Some(atts.clone())
            }
        }),
    };
    let launch_record = state
        .db
        .create_agent_turn_and_run_with_interaction_response(
            &user_msg,
            &task_title_from_message(&message),
            Some(&db_config.provider),
            Some(&db_config.model),
            &idempotency_key,
            interaction_response.as_ref(),
        )
        .map_err(|e| e.to_string())?;
    if launch_record.reused {
        return Ok(desktop_agent_chat_launch(
            &launch_record,
            task_orchestrator_run_id,
        ));
    }
    if resumes_interaction {
        // Validate and persist the response before touching a live session.
        // A forged or stale response artifact must never abort unrelated work.
        if let Some(previous) = agent_state.sessions.take(&conversation_id).await {
            previous.cancel_token.cancel();
            previous.task.abort();
            let _ = previous.task.await;
        }
    }
    let task_orchestrator_run_id = if task_orchestrator_run_id.is_some() {
        task_orchestrator_run_id
    } else if resumes_interaction {
        state
            .db
            .get_workflow_automation_run_for_task_run(&launch_record.run_id)
            .map_err(|error| error.to_string())?
            .map(|run| run.id)
    } else {
        None
    };
    let task_run_id_for_command = launch_record.run_id.clone();
    if !resumes_interaction {
        if let Some(run_id) = task_orchestrator_run_id.as_deref() {
            state
                .db
                .start_workflow_automation_run(run_id, &launch_record.run_id, None)
                .map_err(|err| err.to_string())?;
        }
    }
    let last_event_sequence = state
        .db
        .latest_agent_run_event_sequence(&launch_record.run_id)
        .map_err(|error| error.to_string())?;
    let stream_event_seq = Arc::new(AgentRunEventSequencer::new(last_event_sequence));
    let terminal_emitted = Arc::new(AtomicBool::new(false));
    emit_agent_task_run_update(
        &state.db,
        &app_handle,
        &conversation_id,
        &launch_record.run_id,
    );
    record_internal_agent_run_status_event(
        &state.db,
        &app_handle,
        &conversation_id,
        &launch_record.run_id,
        Some(&launch_record.turn_id),
        &stream_event_seq,
        AgentRunPhase::Routing,
        "Task queued",
        Some("queued"),
        None,
    );

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();
    let (steering_tx, steering_rx) = tokio::sync::mpsc::unbounded_channel::<AgentSteeringMessage>();
    let db = state.db.clone();
    let db_executor = state.db_executor.clone();
    let conv_id = conversation_id.clone();
    let turn_id = launch_record.turn_id.clone();
    let task_run_id = launch_record.run_id.clone();
    let user_message_id = launch_record.user_message_id.clone();
    let user_message_sort_order = launch_record.user_message_sort_order;
    let handle = app_handle.clone();
    let db_config_for_post_success = db_config.clone();
    let task_orchestrator_run_id_for_task = task_orchestrator_run_id.clone();
    let resumes_interaction_for_task = resumes_interaction;
    let approval_pending = approval_state.pending.clone();
    let approval_session_store = approval_state.session_store.clone();
    let mcp_manager = Arc::clone(&mcp_state.manager);
    let requested_skill_ids = skill_ids.unwrap_or_default();
    let user_llm_content = conversation_message_llm_context_content(&user_msg).to_string();
    let launch_ack_ms = elapsed_millis(launch_started);
    const STREAM_KEEPALIVE_INTERVAL_SECS: u64 = 10;
    record_turn_launch_metric(
        &state.db,
        &app_handle,
        &conversation_id,
        &launch_record.run_id,
        Some(&turn_id),
        &stream_event_seq,
        TurnLaunchStage::LaunchAckMs,
        launch_ack_ms,
    );

    let stream_event_seq_for_task = Arc::clone(&stream_event_seq);
    let task = tokio::spawn(async move {
        let initialization = async {
            db.mark_agent_task_run_started(&task_run_id, "initializing")
                .map_err(|error| error.to_string())?;
            emit_agent_task_run_update(&db, &handle, &conv_id, &task_run_id);
            record_internal_agent_run_status_event(
                &db,
                &handle,
                &conv_id,
                &task_run_id,
                Some(&turn_id),
                &stream_event_seq_for_task,
                AgentRunPhase::Routing,
                "Agent started",
                Some("running"),
                None,
            );

            if cancel_token_clone.is_cancelled() {
                return Err("Agent execution cancelled during initialization.".to_string());
            }

            let app_cfg = db.load_app_config().unwrap_or_default();
            let registry_scope = nexa_core::capability_registry::RegistryScope {
                workspace_id: conv.project_id.clone(),
                agent_id: Some(db_config.id.clone()),
                task_id: Some(task_run_id.clone()),
            };
            let mut effective_db_config = db_config.clone();
            let registry_resolution = db
                .resolve_or_pin_task_runtime_capability(
                    &registry_scope,
                    "text_generation",
                    &task_run_id,
                )
                .map_err(|error| {
                    format!(
                        "Capability Registry resolution failed for run {task_run_id}; explicitly roll back the durable read mode before retrying: {error}"
                    )
                })?;
            let (
                provider_config,
                registry_fallback_plan,
                primary_egress_id,
                primary_routes_local,
                primary_native_vision_allowed,
            ) = match registry_resolution {
                Some(resolution) => {
                    effective_db_config.provider = resolution.provider_id;
                    effective_db_config.provider_endpoint_id = Some(resolution.endpoint_id);
                    effective_db_config.base_url = resolution.provider_config.base_url.clone();
                    effective_db_config.api_key = resolution
                        .provider_config
                        .api_key
                        .clone()
                        .unwrap_or_default();
                    effective_db_config.model = resolution.model_id.clone();
                    effective_db_config.model_id = Some(resolution.model_id.clone());
                    let primary_routes_local = provider_config_is_local(&resolution.provider_config)
                        && resolution
                            .fallbacks
                            .iter()
                            .all(|fallback| provider_config_is_local(&fallback.provider_config));
                    let primary_native_vision_allowed = resolution.fallbacks.is_empty();
                    let mut egress_connections = vec![resolution.snapshot.connection_id.clone()];
                    egress_connections.extend(
                        resolution
                            .fallbacks
                            .iter()
                            .map(|fallback| fallback.connection_id.clone()),
                    );
                    let egress_id = if egress_connections.len() == 1 {
                        format!("registry:{}", egress_connections[0])
                    } else {
                        format!("registry-plan:{}", egress_connections.join("|"))
                    };
                    let plan = Some((
                        resolution.snapshot.fallback_index,
                        resolution.model_id,
                        resolution.fallbacks,
                    ));
                    (
                        resolution.provider_config,
                        plan,
                        egress_id,
                        primary_routes_local,
                        primary_native_vision_allowed,
                    )
                }
                None => {
                    let provider_config = db_config_to_provider_config(&db_config, None);
                    let egress_id = provider_config_egress_id(&provider_config);
                    let primary_routes_local = provider_config_is_local(&provider_config);
                    (provider_config, None, egress_id, primary_routes_local, true)
                }
            };
            let mut provider = create_provider(provider_config.clone()).map_err(|e| e.to_string())?;
            if let Some((primary_fallback_index, primary_model, fallbacks)) =
                registry_fallback_plan
            {
                if !fallbacks.is_empty() {
                    let candidates = fallbacks
                        .into_iter()
                        .map(|fallback| {
                            let provider_type = fallback.provider_config.provider_type;
                            let provider = create_provider(fallback.provider_config)
                                .map_err(|error| error.to_string())?;
                            Ok(nexa_core::llm::fallback::AutomaticFallbackCandidate {
                                fallback_index: fallback.fallback_index,
                                provider,
                                model: fallback.model_id,
                                provider_type,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let fallback_db = Arc::clone(&db);
                    let fallback_run_id = task_run_id.clone();
                    let on_selected = Arc::new(move |from_index, to_index, reason: &str| {
                        fallback_db
                            .advance_task_runtime_fallback(
                                &fallback_run_id,
                                "text_generation",
                                from_index,
                                to_index,
                                reason,
                            )
                            .map(|_| ())
                    });
                    provider = Box::new(
                        nexa_core::llm::fallback::AutomaticFallbackProvider::new(
                            primary_fallback_index,
                            provider,
                            primary_model,
                            provider_config.provider_type,
                            candidates,
                            on_selected,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
            }
            let provider = if collaboration_mode.is_moa() {
                build_moa_provider(db.as_ref(), &effective_db_config, provider, moa_preset)?
            } else {
                provider
            };
            let vision_resolution = if attachments.as_ref().is_some_and(|attachments| {
                attachments
                    .iter()
                    .any(|attachment| attachment.media_type.starts_with("image/"))
            }) {
                db.resolve_or_pin_task_runtime_capability(
                    &registry_scope,
                    "vision",
                    &task_run_id,
                )
                .map_err(|error| {
                    format!(
                        "Vision capability resolution failed for run {task_run_id}: {error}"
                    )
                })?
            } else {
                None
            };
            let history_started = Instant::now();
            let history_conversation_id = conv_id.clone();
            let projection = db_executor
                .read(move |database| {
                    nexa_core::context_maintenance::load_context_projection(
                        database,
                        &history_conversation_id,
                    )
                })
                .await
                .map_err(|error| error.to_string())?
                .value;
            let history = projection
                .messages
                .into_iter()
                .filter(|entry| {
                    entry.id != user_message_id && entry.sort_order < user_message_sort_order
                })
                .map(|entry| conv_message_to_llm(&entry))
                .collect::<Vec<_>>();
            let history = sanitize_tool_call_history(history);
            record_turn_launch_metric(
                &db,
                &handle,
                &conv_id,
                &task_run_id,
                Some(&turn_id),
                &stream_event_seq_for_task,
                TurnLaunchStage::HistoryLoadMs,
                elapsed_millis(history_started),
            );

            let context_started = Instant::now();
            let desktop_turn_config =
                build_desktop_agent_turn_config(DesktopAgentTurnConfigRequest {
                    db: &db,
                    conversation: &conv,
                    turn_id: &turn_id,
                    message: &message,
                    persona_id: persona_id.as_deref(),
                    explicit_skill_ids: &requested_skill_ids,
                    db_config: &effective_db_config,
                    app_cfg: &app_cfg,
                    execution_mode,
                    power_mode,
                    collaboration_mode,
                    moa_preset,
                    orchestration_profile,
                    custom_orchestration: custom_orchestration.clone(),
                });
            record_turn_launch_metric(
                &db,
                &handle,
                &conv_id,
                &task_run_id,
                Some(&turn_id),
                &stream_event_seq_for_task,
                TurnLaunchStage::ContextBuildMs,
                elapsed_millis(context_started),
            );
            let source_scope_ids = desktop_turn_config.source_scope_ids;
            let pinned_skill_ids = desktop_turn_config.pinned_skill_ids;
            let context_pack = desktop_turn_config.context_pack;
            let mut executor_config = desktop_turn_config.executor_config;
            let summarization_provider = match resolve_desktop_summarization_provider_config(
                db.as_ref(),
                &effective_db_config,
            )? {
                Some((summary_config, _, summary_model)) => {
                    executor_config.summarization_model = Some(summary_model);
                    executor_config.summarization_provider_type =
                        Some(summary_config.provider_type);
                    Some(create_provider(summary_config).map_err(|error| error.to_string())?)
                }
                None => None,
            };

            let session_dependencies =
                build_desktop_agent_session_dependencies(DesktopAgentSessionDependencyRequest {
                    db: &db,
                    mcp_manager: &mcp_manager,
                    app_handle: &handle,
                    event_seq: &stream_event_seq_for_task,
                    conversation_id: &conv_id,
                    task_run_id: &task_run_id,
                    turn_id: &turn_id,
                    message: &message,
                    pinned_skill_ids: &pinned_skill_ids,
                    provider_config: provider_config.clone(),
                    executor_config: executor_config.clone(),
                    subagent_allowed_tools: effective_db_config.subagent_allowed_tools.clone(),
                    subagent_allowed_skill_ids: effective_db_config
                        .subagent_allowed_skill_ids
                        .clone(),
                    cancel_token: cancel_token_clone.clone(),
                    plan_mode: execution_mode.is_plan(),
                    mcp_call_timeout_secs: DEFAULT_MCP_CALL_TIMEOUT_SECS,
                    terminal_state,
                    browser_state,
                })
                .await;
            for (stage, elapsed_ms) in [
                (
                    TurnLaunchStage::SkillSelectMs,
                    session_dependencies.metrics.skill_select_ms,
                ),
                (
                    TurnLaunchStage::McpSyncMs,
                    session_dependencies.metrics.mcp_sync_ms,
                ),
                (
                    TurnLaunchStage::ToolRegistryMs,
                    session_dependencies.metrics.tool_registry_ms,
                ),
            ] {
                record_turn_launch_metric(
                    &db,
                    &handle,
                    &conv_id,
                    &task_run_id,
                    Some(&turn_id),
                    &stream_event_seq_for_task,
                    stage,
                    elapsed_ms,
                );
            }

            if cancel_token_clone.is_cancelled() {
                return Err("Agent execution cancelled during initialization.".to_string());
            }

            let request_build_started = Instant::now();
            let runtime_session_config =
                build_desktop_agent_session_config(DesktopAgentSessionConfigInput {
                    db: db.as_ref(),
                    conversation_id: &conv_id,
                    task_run_id: &task_run_id,
                    db_config: &effective_db_config,
                    app_cfg: &app_cfg,
                    source_scope_ids: &source_scope_ids,
                    selected_skills: &session_dependencies.selected_skills,
                    auto_loaded_skills: &session_dependencies.auto_loaded_skills,
                    execution_mode,
                    collaboration_mode,
                    moa_preset,
                    orchestration_profile,
                    custom_orchestration,
                });
            let initial_task_artifacts = build_desktop_agent_initial_task_artifacts(
                &session_dependencies.selected_skills,
                &runtime_session_config,
                &context_pack,
                execution_mode,
                &executor_config,
            );
            let _ = db.update_agent_task_run_progress(
                &task_run_id,
                None,
                None,
                None,
                None,
                None,
                Some(&initial_task_artifacts),
            );
            emit_agent_task_run_update(&db, &handle, &conv_id, &task_run_id);
            record_turn_launch_metric(
                &db,
                &handle,
                &conv_id,
                &task_run_id,
                Some(&turn_id),
                &stream_event_seq_for_task,
                TurnLaunchStage::RequestBuildMs,
                elapsed_millis(request_build_started),
            );

            let attachment_started = Instant::now();
            let vision_content =
                build_desktop_agent_vision_user_content(DesktopAgentVisionUserContentRequest {
                    db: &db,
                    app_handle: Some(&handle),
                    provider_config: &provider_config,
                    db_config: &effective_db_config,
                    message: &user_llm_content,
                    attachments: attachments.as_deref(),
                    vision_resolution: vision_resolution.as_ref(),
                    task_run_id: &task_run_id,
                    primary_egress_id: &primary_egress_id,
                    primary_routes_local,
                    primary_native_vision_allowed,
                    turn_override: vision_turn_override,
                    cancellation: &cancel_token_clone,
                })
                .await?;
            db.update_message_vision_context(
                &user_message_id,
                &vision_content.attachments,
                &vision_content.llm_context_content,
            )
            .map_err(|error| error.to_string())?;
            let user_parts = vision_content.parts;
            record_turn_launch_metric(
                &db,
                &handle,
                &conv_id,
                &task_run_id,
                Some(&turn_id),
                &stream_event_seq_for_task,
                TurnLaunchStage::AttachmentPrepareMs,
                elapsed_millis(attachment_started),
            );

            let approval_runtime = DesktopAgentApprovalRuntime {
                pending: approval_pending,
                session_store: approval_session_store,
                approval_mode: app_cfg.tool_approval_mode,
            };
            let turn_timeout_secs = executor_config.agent_timeout_secs.unwrap_or(0) as u64;
            Ok::<_, String>((
                provider,
                session_dependencies,
                executor_config,
                approval_runtime,
                summarization_provider,
                history,
                user_parts,
                turn_timeout_secs,
            ))
        }
        .await;

        let (
            provider,
            session_dependencies,
            executor_config,
            approval_runtime,
            summarization_provider,
            history,
            user_parts,
            turn_timeout_secs,
        ) = match initialization {
            Ok(prepared) => prepared,
            Err(error) => {
                finalize_desktop_agent_initialization_failure(
                    &db,
                    &handle,
                    &conv_id,
                    &task_run_id,
                    task_orchestrator_run_id_for_task.as_deref(),
                    &turn_id,
                    &stream_event_seq_for_task,
                    &terminal_emitted,
                    &error,
                );
                return;
            }
        };
        if resumes_interaction_for_task {
            if let Err(error) = db.acknowledge_submitted_interactions_for_run(&task_run_id) {
                finalize_desktop_agent_initialization_failure(
                    &db,
                    &handle,
                    &conv_id,
                    &task_run_id,
                    task_orchestrator_run_id_for_task.as_deref(),
                    &turn_id,
                    &stream_event_seq_for_task,
                    &terminal_emitted,
                    &error.to_string(),
                );
                return;
            }
        }

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
            assistant_sort_order: user_message_sort_order + 1,
            runtime: DesktopAgentTurnRuntime {
                timeout_secs: turn_timeout_secs,
                keepalive_interval_secs: STREAM_KEEPALIVE_INTERVAL_SECS,
            },
            stream: DesktopAgentTurnStream {
                app_handle: handle.clone(),
                task_run_id: task_run_id.clone(),
                event_seq: Arc::clone(&stream_event_seq_for_task),
                terminal_emitted: Arc::clone(&terminal_emitted),
                launch_started,
            },
        })
        .await;
        let result = &outcome.result;

        match result {
            Some(Ok(_)) => {}
            Some(Err(CoreError::AwaitingUserInput { interaction_id })) => {
                log::info!("Agent turn {turn_id} is waiting for interaction {interaction_id}");
            }
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

    // Register the background initializer before acknowledging the launch so
    // cancellation and steering are available immediately.
    let launch = DesktopAgentChatLaunch {
        conversation_id: conversation_id.clone(),
        task_run_id: task_run_id_for_command.clone(),
        task_orchestrator_run_id: task_orchestrator_run_id.clone(),
        handle: AgentTurnHandle {
            session_id: conversation_id.clone(),
            run_id: task_run_id_for_command.clone(),
            turn_id: launch_record.turn_id.clone(),
            state: AgentTurnState::Starting,
        },
    };
    agent_state
        .sessions
        .register(ActiveAgentTurn {
            handle: launch.handle.clone(),
            cancel_token,
            task,
            steering_tx,
            event_sequencer: Arc::clone(&stream_event_seq),
            orchestrator_run_id: task_orchestrator_run_id,
            frontend_paint_recorded: AtomicBool::new(false),
        })
        .await;

    Ok(launch)
}

#[allow(clippy::too_many_arguments)]
fn record_turn_launch_metric(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event_seq: &AgentRunEventSequencer,
    stage: TurnLaunchStage,
    elapsed_ms: u64,
) {
    let payload = serde_json::json!({
        "kind": "turnLaunchMetric",
        "stage": stage.as_str(),
        "elapsedMs": elapsed_ms,
    });
    record_internal_agent_run_status_event(
        db,
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        event_seq,
        AgentRunPhase::Routing,
        stage.as_str(),
        None,
        Some(&payload),
    );
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
fn finalize_desktop_agent_initialization_failure(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    task_orchestrator_run_id: Option<&str>,
    turn_id: &str,
    event_seq: &AgentRunEventSequencer,
    terminal_emitted: &AtomicBool,
    error: &str,
) {
    warn!("Agent initialization failed for conversation {conversation_id}: {error}");
    let cancelled = error.starts_with("Agent execution cancelled");
    let status = if cancelled { "cancelled" } else { "failed" };
    let turn_status = if cancelled { "cancelled" } else { "error" };
    let summary = if cancelled {
        "Agent initialization cancelled"
    } else {
        "Agent initialization failed"
    };
    let trace = serde_json::json!({ "initializationError": error, "status": status });
    let _ = db.finalize_conversation_turn(turn_id, turn_status, None, Some(&trace));
    let _ = db.finish_agent_task_run(
        task_run_id,
        status,
        Some(summary),
        Some(error),
        Some(&trace),
    );
    if let Some(run_id) = task_orchestrator_run_id {
        let _ = db.transition_workflow_automation_run(run_id, status, Some(summary));
    }
    let payload = serde_json::json!({ "stage": "initialization", "reason": error });
    emit_terminal_agent_error_once(
        terminal_emitted,
        db,
        app_handle,
        event_seq,
        TerminalAgentError {
            conversation_id,
            task_run_id,
            turn_id,
            message: if cancelled {
                "Agent execution cancelled during initialization."
            } else {
                "Agent execution failed during initialization."
            },
            status,
            payload: Some(&payload),
        },
    );
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
}

fn desktop_agent_chat_launch(
    record: &nexa_core::conversation::AgentTurnLaunchRecord,
    task_orchestrator_run_id: Option<String>,
) -> DesktopAgentChatLaunch {
    let state = match record.status.as_str() {
        "queued" => AgentTurnState::Starting,
        "running" | "cancelling" => AgentTurnState::Running,
        "waiting_approval" => AgentTurnState::WaitingApproval,
        "awaiting_user_input" => AgentTurnState::AwaitingUserInput,
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

    agent_state
        .sessions
        .steer(&conversation_id, trimmed.to_string())
        .await
        .map_err(|error| error.to_string())
}

// ── Agent Stop Command ──────────────────────────────────────────────────

#[tauri::command]
pub async fn agent_stop_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    app_handle: AppHandle,
    conversation_id: String,
) -> Result<(), String> {
    if let Some(run_id) = state
        .db
        .cancel_awaiting_interactions_for_conversation(&conversation_id)
        .map_err(|error| error.to_string())?
    {
        if let Some(turn) = agent_state.sessions.take(&conversation_id).await {
            turn.cancel_token.cancel();
        }
        emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &run_id);
        return Ok(());
    }

    if let Some(turn) = agent_state.sessions.take(&conversation_id).await {
        request_desktop_running_agent_stop(
            turn,
            DesktopRunningAgentStopRequest {
                db: state.db.clone(),
                app_handle,
                conversation_id,
            },
        );
    }
    Ok(())
}
