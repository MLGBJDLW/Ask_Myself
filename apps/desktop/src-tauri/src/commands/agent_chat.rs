use super::*;
use crate::browser::BrowserState;
use crate::desktop_agent_session::{
    annotate_user_artifacts_with_execution_mode, build_desktop_agent_initial_task_artifacts,
    build_desktop_agent_session_config, build_desktop_agent_session_dependencies,
    build_desktop_agent_turn_config, build_desktop_agent_vision_user_content,
    finalize_desktop_agent_stop, finalize_desktop_agent_turn, provider_config_egress_id,
    provider_config_is_local, reconcile_authoritative_run_event_outbox_failure,
    request_desktop_running_agent_stop, resolve_desktop_summarization_provider_config,
    run_desktop_agent_post_success_learning, run_desktop_agent_turn, DesktopAgentApprovalRuntime,
    DesktopAgentPostSuccessLearningRequest, DesktopAgentSessionConfigInput,
    DesktopAgentSessionDependencyRequest, DesktopAgentStopFinalization,
    DesktopAgentTurnConfigRequest, DesktopAgentTurnFinalization, DesktopAgentTurnRequest,
    DesktopAgentTurnRuntime, DesktopAgentTurnStream, DesktopAgentVisionUserContentRequest,
    DesktopRunningAgentStopRequest,
};
use nexa_core::approval::ToolApprovalMode;
use nexa_core::llm::ReasoningEffort;
use nexa_core::mixture_of_agents::{
    AgentCollaborationMode, MoaAdvisor, MoaPreset, MoaPresetId, MoaProvider,
};
use nexa_core::quality_profile::{CustomOrchestrationOptions, OrchestrationProfile};
use nexa_core::run_event_outbox::AgentRunEventSubmitError;
use nexa_core::runtime::{
    ActiveAgentTurn, AgentRunEventOutbox, AgentTurnHandle, AgentTurnState, RuntimeTerminalStatus,
    StartTurnRequest, TurnLaunchStage, RUNTIME_PROTOCOL_VERSION,
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
    /// Host-validated per-turn config projection (for example, a scheduled
    /// execution policy). It is never persisted back to the saved config.
    pub agent_config_override: Option<DbAgentConfig>,
    /// When true, the override is the immutable route authority for this run;
    /// Capability Registry may not silently replace its provider or model.
    pub agent_config_override_is_authoritative: bool,
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
    pub root_allowed_tools: Option<Vec<String>>,
    /// Host-authorized per-run grant. Scheduled workflows use AllowAll only
    /// after the registry has been narrowed to their immutable saved allowlist;
    /// hard screen/computer/browser confirmations remain non-bypassable.
    pub tool_approval_mode_override: Option<ToolApprovalMode>,
    pub force_workspace_isolation: bool,
    pub task_orchestrator_run_id: Option<String>,
    pub resume_checkpoint_id: Option<String>,
    pub retry_from_message_id: Option<String>,
    pub idempotency_key: String,
}

fn capability_registry_may_select_text_route(agent_config_override_is_authoritative: bool) -> bool {
    !agent_config_override_is_authoritative
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
        agent_config_override: None,
        agent_config_override_is_authoritative: false,
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
        root_allowed_tools: None,
        tool_approval_mode_override: None,
        force_workspace_isolation: false,
        task_orchestrator_run_id: request.task_orchestrator_run_id,
        resume_checkpoint_id: request.resume_checkpoint_id,
        retry_from_message_id: request.retry_from_message_id,
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
        agent_config_override,
        agent_config_override_is_authoritative,
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
        root_allowed_tools,
        tool_approval_mode_override,
        force_workspace_isolation,
        task_orchestrator_run_id,
        resume_checkpoint_id,
        retry_from_message_id,
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
    let resume_checkpoint_id = resume_checkpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let retry_from_message_id = retry_from_message_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    if retry_from_message_id.is_some()
        && (resume_checkpoint_id.is_some() || task_orchestrator_run_id.is_some())
    {
        return Err(
            "Reply retry cannot also resume a task checkpoint or Task Orchestrator run."
                .to_string(),
        );
    }
    if resume_checkpoint_id.is_some() && task_orchestrator_run_id.is_some() {
        return Err(
            "Task checkpoint resumes recover their original Task Orchestrator identity."
                .to_string(),
        );
    }

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
    if agent_config_override_is_authoritative && agent_config_override.is_none() {
        return Err(
            "An authoritative per-turn config requires an agent config override.".to_string(),
        );
    }
    let db_config = if let Some(config_override) = agent_config_override {
        if agent_config_id
            .as_deref()
            .is_some_and(|requested_id| requested_id != config_override.id)
        {
            return Err("Per-turn agent config override does not match agentConfigId.".to_string());
        }
        config_override
    } else {
        select_agent_config_for_conversation(&state.db, &conv, agent_config_id.as_deref())?
    };
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
            nexa_core::task_orchestrator::project_task_status(workflow_run.status.as_str())
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
    if resumes_interaction && resume_checkpoint_id.is_some() {
        return Err(
            "Interaction responses and task checkpoints are mutually exclusive continuations."
                .to_string(),
        );
    }
    if resume_checkpoint_id.is_some() && attachments.is_some() {
        return Err("Task checkpoint resumes cannot add new attachments.".to_string());
    }
    let resumed_run_id = if let Some(response) = interaction_response.as_ref() {
        Some(
            state
                .db
                .get_interaction_request_run_id(&response.interaction_id)
                .map_err(|error| error.to_string())?,
        )
    } else if let Some(checkpoint_id) = resume_checkpoint_id.as_deref() {
        let checkpoint = state
            .db
            .get_task_resume_checkpoint(checkpoint_id)
            .map_err(|error| error.to_string())?;
        let checkpoint_run = state
            .db
            .get_agent_task_run(&checkpoint.run_id)
            .map_err(|error| error.to_string())?;
        if checkpoint_run.conversation_id != conversation_id {
            return Err("Task checkpoint belongs to a different conversation.".to_string());
        }
        Some(checkpoint.run_id)
    } else {
        None
    };
    let resumes_existing_run = resumed_run_id.is_some();
    let run_lifecycle_guard = if let Some(resumed_run_id) = resumed_run_id.as_deref() {
        Some(
            agent_state
                .sessions
                .acquire_run_lifecycle(resumed_run_id)
                .await,
        )
    } else {
        None
    };
    let mut resumed_turn = if let Some(resumed_run_id) = resumed_run_id.as_deref() {
        agent_state
            .sessions
            .take_for_run(&conversation_id, resumed_run_id)
            .await
            .map_err(|error| error.to_string())?
    } else {
        None
    };
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
    let launch_result = if let Some(retry_message_id) = retry_from_message_id.as_deref() {
        state.db.retry_agent_turn_and_run(
            &user_msg,
            retry_message_id,
            &task_title_from_message(&message),
            Some(&db_config.provider),
            Some(&db_config.model),
            &idempotency_key,
        )
    } else if let Some(checkpoint_id) = resume_checkpoint_id.as_deref() {
        state.db.resume_agent_turn_from_checkpoint(
            &user_msg,
            Some(&db_config.provider),
            Some(&db_config.model),
            &idempotency_key,
            checkpoint_id,
        )
    } else {
        state
            .db
            .create_agent_turn_and_run_with_interaction_response(
                &user_msg,
                &task_title_from_message(&message),
                Some(&db_config.provider),
                Some(&db_config.model),
                &idempotency_key,
                interaction_response.as_ref(),
            )
    };
    let mut launch_record = match launch_result {
        Ok(launch_record) => launch_record,
        Err(error) => {
            if let Some(previous) = resumed_turn.take() {
                agent_state.sessions.register(previous).await;
            }
            return Err(error.to_string());
        }
    };
    if let Some(expected_run_id) = resumed_run_id.as_deref() {
        debug_assert_eq!(launch_record.run_id, expected_run_id);
    }
    let _run_lifecycle_guard = match run_lifecycle_guard {
        Some(guard) => guard,
        None => {
            agent_state
                .sessions
                .acquire_run_lifecycle(&launch_record.run_id)
                .await
        }
    };
    let authoritative_task = match state.db.get_agent_task_run(&launch_record.run_id) {
        Ok(task) => task,
        Err(error) => {
            if let Some(previous) = resumed_turn.take() {
                agent_state.sessions.register(previous).await;
            }
            return Err(error.to_string());
        }
    };
    launch_record.status = authoritative_task.status.clone();
    let continuation_is_already_running = resumed_turn
        .as_ref()
        .is_some_and(|previous| !previous.is_finished());
    let task_orchestrator_run_id = if task_orchestrator_run_id.is_some() {
        task_orchestrator_run_id
    } else if resumes_existing_run {
        match state
            .db
            .get_workflow_automation_run_for_task_run(&launch_record.run_id)
        {
            Ok(run) => run.map(|run| run.id),
            Err(error) => {
                if let Some(previous) = resumed_turn.take() {
                    agent_state.sessions.register(previous).await;
                }
                return Err(error.to_string());
            }
        }
    } else {
        None
    };
    if !launch_status_needs_executor(&launch_record.status) {
        if !resumes_existing_run {
            if let Some(run_id) = task_orchestrator_run_id.as_deref() {
                let workflow_run = state
                    .db
                    .get_workflow_automation_run(run_id)
                    .map_err(|error| error.to_string())?;
                match workflow_run.task_run_id.as_deref() {
                    None => {
                        state
                            .db
                            .start_workflow_automation_run(
                                run_id,
                                &launch_record.run_id,
                                Some("Agent launch was superseded before executor spawn"),
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    Some(bound_run_id)
                        if bound_run_id == launch_record.run_id
                            && workflow_run.status
                                == nexa_core::workflow_automation::WorkflowAutomationRunStatus::Queued =>
                    {
                        state
                            .db
                            .start_workflow_automation_run(
                                run_id,
                                &launch_record.run_id,
                                Some("Agent launch was superseded before executor spawn"),
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    Some(bound_run_id) if bound_run_id == launch_record.run_id => {}
                    Some(bound_run_id) => {
                        return Err(format!(
                            "Task Orchestrator run {run_id} is already bound to Agent Run {bound_run_id}"
                        ));
                    }
                }
                if let Err(error) = state.db.transition_workflow_automation_run(
                    run_id,
                    &launch_record.status,
                    authoritative_task
                        .summary
                        .as_deref()
                        .or(Some("Agent launch was superseded before executor spawn")),
                ) {
                    warn!(
                        "Failed to project authoritative Agent Run {} status {} to Task Orchestrator run {run_id}: {error}",
                        launch_record.run_id,
                        launch_record.status
                    );
                }
            }
        }
        if let Some(previous) = resumed_turn.take() {
            agent_state.sessions.register(previous).await;
        }
        return Ok(desktop_agent_chat_launch(
            &launch_record,
            task_orchestrator_run_id,
        ));
    }
    if reused_launch_needs_no_executor(
        &launch_record,
        resumes_existing_run,
        continuation_is_already_running,
    ) {
        if let Some(previous) = resumed_turn.take() {
            agent_state.sessions.register(previous).await;
        }
        return Ok(desktop_agent_chat_launch(
            &launch_record,
            task_orchestrator_run_id,
        ));
    }
    let task_run_id_for_command = launch_record.run_id.clone();
    let stream_event_seq = match state
        .run_event_outboxes
        .open(&conversation_id, &launch_record.run_id)
        .await
    {
        Ok(outbox) => outbox,
        Err(error) => {
            let terminalized = reconcile_pre_executor_launch_failure(
                state.db.as_ref(),
                &launch_record.run_id,
                task_orchestrator_run_id.as_deref(),
                &launch_record.turn_id,
                &error.to_string(),
            );
            if let Some(previous) = resumed_turn.take() {
                if terminalized {
                    previous.cancel_token.cancel();
                    previous.task.abort();
                    let _ = previous.task.await;
                } else {
                    agent_state.sessions.register(previous).await;
                }
            }
            emit_agent_task_run_update(
                &state.db,
                &app_handle,
                &conversation_id,
                &launch_record.run_id,
            );
            return Err(error.to_string());
        }
    };
    if let Err(error) = stream_event_seq.resume_submissions() {
        let message = format!("Could not open the Run Event producer boundary: {error}");
        finalize_desktop_agent_initialization_failure(
            state.db.as_ref(),
            &app_handle,
            &conversation_id,
            &launch_record.run_id,
            task_orchestrator_run_id.as_deref(),
            &launch_record.turn_id,
            stream_event_seq.as_ref(),
            &message,
        )
        .await;
        if let Some(previous) = resumed_turn.take() {
            previous.cancel_token.cancel();
            previous.task.abort();
            let _ = previous.task.await;
        }
        return Err(message);
    }
    if stream_event_seq.is_closed_for_submission() {
        if let Some(previous) = resumed_turn.take() {
            agent_state.sessions.register(previous).await;
        }
        return Err(format!(
            "Agent task run {} is already terminal and cannot resume.",
            launch_record.run_id
        ));
    }
    if let Some(previous) = resumed_turn.take() {
        debug_assert_eq!(previous.handle.run_id, launch_record.run_id);
        previous.cancel_token.cancel();
        previous.task.abort();
        let _ = previous.task.await;
    }
    if !resumes_existing_run {
        if let Some(run_id) = task_orchestrator_run_id.as_deref() {
            state
                .db
                .start_workflow_automation_run(run_id, &launch_record.run_id, None)
                .map_err(|err| err.to_string())?;
        }
    }
    emit_agent_task_run_update(
        &state.db,
        &app_handle,
        &conversation_id,
        &launch_record.run_id,
    );
    record_internal_agent_run_status_event(
        &conversation_id,
        &launch_record.run_id,
        Some(&launch_record.turn_id),
        &stream_event_seq,
        AgentRunPhase::Routing,
        "Task queued",
        Some("queued"),
        None,
    );

    let cancel_token = stream_event_seq.turn_cancellation_token();
    let cancel_token_clone = cancel_token.clone();
    let (steering_tx, steering_rx) = tokio::sync::mpsc::unbounded_channel::<AgentSteeringMessage>();
    let db = state.db.clone();
    let db_executor = state.db_executor.clone();
    let subagent_lifecycle = state.subagent_lifecycle.clone();
    let background_work = state.background_work.clone();
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
        let _foreground_work_lease = background_work.foreground_lease();
        let initialization = async {
            stream_event_seq_for_task
                .submit(AgentRunEvent::status_update(
                    &task_run_id,
                    Some(&turn_id),
                    0,
                    AgentRunPhase::Routing,
                    "Agent started",
                    Some("running"),
                    None,
                ))
                .map_err(|error| error.to_string())?;
            stream_event_seq_for_task
                .flush()
                .await
                .map_err(|error| error.to_string())?;

            if cancel_token_clone.is_cancelled() {
                return Err("Agent execution cancelled during initialization.".to_string());
            }

            let mut app_cfg = db.load_app_config().unwrap_or_default();
            if let Some(approval_mode) = tool_approval_mode_override {
                app_cfg.tool_approval_mode = approval_mode;
            }
            let registry_scope = nexa_core::capability_registry::RegistryScope {
                workspace_id: conv.project_id.clone(),
                agent_id: Some(db_config.id.clone()),
                task_id: Some(task_run_id.clone()),
            };
            let mut effective_db_config = db_config.clone();
            let registry_resolution = if capability_registry_may_select_text_route(
                agent_config_override_is_authoritative,
            ) {
                db.resolve_or_pin_task_runtime_capability(
                    &registry_scope,
                    "text_generation",
                    &task_run_id,
                )
                .map_err(|error| {
                    format!(
                        "Capability Registry resolution failed for run {task_run_id}; explicitly roll back the durable read mode before retrying: {error}"
                    )
                })?
            } else {
                None
            };
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
            let history = sanitize_tool_call_history(history, Some(&conv_id));
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
            let context_resolution_payload = serde_json::json!({
                "contextCapacity": desktop_turn_config.context_window_resolution.capacity_tokens,
                "contextAuthority": desktop_turn_config.context_window_resolution.authority,
            });
            if let Err(error) = db.record_agent_task_run_event(
                &task_run_id,
                "telemetry",
                "context_resolution",
                Some("resolved"),
                Some(&context_resolution_payload),
            ) {
                warn!("Failed to persist context resolution for {task_run_id}: {error}");
            }
            let source_scope_ids = desktop_turn_config.source_scope_ids;
            let pinned_skill_ids = desktop_turn_config.pinned_skill_ids;
            let context_pack = desktop_turn_config.context_pack;
            let mut executor_config = desktop_turn_config.executor_config;
            if force_workspace_isolation {
                executor_config.request_kind =
                    nexa_core::agent::AgentRequestKind::ScheduledIsolatedPatch;
            }
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
                    event_seq: &stream_event_seq_for_task,
                    conversation_id: &conv_id,
                    task_run_id: &task_run_id,
                    turn_id: &turn_id,
                    message: &message,
                    pinned_skill_ids: &pinned_skill_ids,
                    provider_config: provider_config.clone(),
                    executor_config: executor_config.clone(),
                    root_allowed_tools: root_allowed_tools.clone(),
                    subagent_allowed_tools: effective_db_config.subagent_allowed_tools.clone(),
                    subagent_allowed_skill_ids: effective_db_config
                        .subagent_allowed_skill_ids
                        .clone(),
                    subagent_lifecycle,
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
                    &error,
                )
                .await;
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
                    &error.to_string(),
                )
                .await;
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
                submit_terminal_agent_error(
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
                submit_terminal_agent_error(
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
                submit_terminal_agent_error(
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
        })
        .await;

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
            event_outbox: Arc::clone(&stream_event_seq),
            orchestrator_run_id: task_orchestrator_run_id,
            frontend_paint_recorded: AtomicBool::new(false),
        })
        .await;

    Ok(launch)
}

#[allow(clippy::too_many_arguments)]
fn record_turn_launch_metric(
    _db: &Database,
    _app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    turn_id: Option<&str>,
    event_seq: &AgentRunEventOutbox,
    stage: TurnLaunchStage,
    elapsed_ms: u64,
) {
    let payload = serde_json::json!({
        "kind": "turnLaunchMetric",
        "stage": stage.as_str(),
        "elapsedMs": elapsed_ms,
    });
    record_internal_agent_run_status_event(
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

fn reused_launch_needs_no_executor(
    launch: &nexa_core::conversation::AgentTurnLaunchRecord,
    resumes_existing_run: bool,
    continuation_is_already_running: bool,
) -> bool {
    launch.reused
        && (!resumes_existing_run || continuation_is_already_running || launch.status != "queued")
}

fn launch_status_needs_executor(status: &str) -> bool {
    status == "queued"
}

fn initialization_frontend_message(error: &str) -> &'static str {
    if error.contains("stale model-target revision") {
        "The AI provider route changed while Nexa was refreshing it. Retry the message; if it repeats, open Settings > AI Providers and review items marked Needs attention."
    } else if error.starts_with("Capability Registry resolution failed") {
        "Nexa could not resolve the selected AI provider route. Open Settings > AI Providers and review items marked Needs attention."
    } else if error.starts_with("Agent execution cancelled") {
        "Agent execution cancelled during initialization."
    } else {
        "Agent execution failed during initialization. Open Task details for the recorded diagnostic."
    }
}

fn reconcile_pre_executor_launch_failure(
    db: &Database,
    task_run_id: &str,
    task_orchestrator_run_id: Option<&str>,
    turn_id: &str,
    error: &str,
) -> bool {
    let failure_reason = format!("run_event_launch_open_failed: {error}");
    let claimed = match nexa_core::task_run::AgentTaskRuntime::new(db)
        .fail_pre_executor_launch_if_open(task_run_id, &failure_reason)
    {
        Ok(claimed) => claimed,
        Err(failure) => {
            warn!(
                "Failed to terminalize committed launch {task_run_id} before executor start: {failure}"
            );
            return false;
        }
    };
    let trace = serde_json::json!({
        "initializationError": error,
        "status": "failed",
        "stage": "run_event_outbox_open",
    });
    reconcile_initialization_terminal_barrier(
        db,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        false,
        "failed",
        "error",
        "Agent initialization failed",
        &failure_reason,
        &trace,
    );
    claimed
}

#[cfg(test)]
mod initialization_error_tests {
    use super::{
        capability_registry_may_select_text_route, desktop_agent_chat_launch,
        initialization_frontend_message, interaction_run_stop_path, launch_status_needs_executor,
        reconcile_initialization_terminal_barrier, reconcile_pre_executor_launch_failure,
        reused_launch_needs_no_executor, InteractionRunStopPath,
    };
    use nexa_core::conversation::{
        AgentTurnLaunchRecord, ConversationMessage, CreateConversationInput,
    };
    use nexa_core::db::Database;
    use nexa_core::llm::Role;
    use nexa_core::runtime::{AgentSessionManager, AgentTurnState};
    use nexa_core::workflow_automation::{
        SaveWorkflowAutomationInput, WorkflowAutomationApprovalPolicy, WorkflowAutomationTrigger,
    };
    use std::sync::Arc;

    #[test]
    fn authoritative_scheduled_config_cannot_be_replaced_by_registry_resolution() {
        assert!(!capability_registry_may_select_text_route(true));
        assert!(capability_registry_may_select_text_route(false));
    }

    #[tokio::test]
    async fn lifecycle_barrier_observes_pause_before_executor_spawn_decision() {
        let db = Database::open_memory().expect("open memory db");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        let message = ConversationMessage {
            id: "launch-pause-race-message".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Start then pause".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).expect("add user message");
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("create turn");
        let task = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &message.id,
                "Launch/pause race",
                Some("open_ai"),
                Some("gpt-test"),
            )
            .expect("create task run");
        let sessions = Arc::new(AgentSessionManager::new());
        let pause_guard = sessions.acquire_run_lifecycle(&task.id).await;
        db.update_agent_task_run_progress(
            &task.id,
            Some("paused"),
            Some("paused"),
            None,
            Some("Paused before executor spawn"),
            None,
            None,
        )
        .expect("pause task behind lifecycle barrier");

        let (decision_tx, mut decision_rx) = tokio::sync::oneshot::channel();
        let launcher = {
            let db = db.clone();
            let sessions = Arc::clone(&sessions);
            let run_id = task.id.clone();
            tokio::spawn(async move {
                let _launch_guard = sessions.acquire_run_lifecycle(&run_id).await;
                let status = db
                    .get_agent_task_run(&run_id)
                    .expect("authoritative task after lifecycle barrier")
                    .status;
                let _ = decision_tx.send(launch_status_needs_executor(&status));
            })
        };
        tokio::task::yield_now().await;
        assert!(matches!(
            decision_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(pause_guard);
        assert!(!decision_rx.await.expect("spawn decision"));
        launcher.await.expect("launcher task");
    }

    #[test]
    fn initialization_finalizer_preserves_a_competing_terminal_winner() {
        let db = Database::open_memory().expect("open memory db");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        let message = ConversationMessage {
            id: "initialization-race-message".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Start the task".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).expect("add user message");
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .expect("create turn");
        let task = db
            .create_agent_task_run(
                &conversation.id,
                &turn.id,
                &message.id,
                "Initialization race",
                Some("open_ai"),
                Some("gpt-test"),
            )
            .expect("create task run");
        db.mark_agent_task_run_started(&task.id, "initializing")
            .expect("start task run");
        let automation = db
            .save_workflow_automation(&SaveWorkflowAutomationInput {
                id: None,
                name: "Initialization race".to_string(),
                description: "Verify terminal arbitration".to_string(),
                workflow_template_id: "report_brief".to_string(),
                prompt: "Start the task".to_string(),
                trigger: WorkflowAutomationTrigger::Manual,
                source_scope: Vec::new(),
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: false,
                    allowed_tools: Vec::new(),
                    risk_level: "low".to_string(),
                },
                enabled: true,
            })
            .expect("save automation");
        let orchestrator = db
            .record_workflow_automation_run(&automation.id, None, "queued", Some("queued"))
            .expect("queue automation run");
        db.start_workflow_automation_run(&orchestrator.id, &task.id, Some("running"))
            .expect("start automation run");

        let winner_artifacts = serde_json::json!({ "reason": "stop_won" });
        db.finish_agent_task_run(
            &task.id,
            "cancelled",
            Some("Stopped by user"),
            Some("stop_won"),
            Some(&winner_artifacts),
        )
        .expect("project competing terminal winner");
        let initialization_trace = serde_json::json!({ "initializationError": "provider failed" });

        assert!(!reconcile_initialization_terminal_barrier(
            &db,
            &task.id,
            Some(&orchestrator.id),
            &turn.id,
            true,
            "failed",
            "error",
            "Agent initialization failed",
            "provider failed",
            &initialization_trace,
        ));

        let authoritative_task = db.get_agent_task_run(&task.id).expect("task winner");
        assert_eq!(authoritative_task.status, "cancelled");
        assert_eq!(
            authoritative_task.summary.as_deref(),
            Some("Stopped by user")
        );
        assert_eq!(
            authoritative_task.error_message.as_deref(),
            Some("stop_won")
        );
        assert_eq!(authoritative_task.artifacts, Some(winner_artifacts));
        assert_eq!(
            db.get_conversation_turn(&turn.id)
                .expect("turn projection")
                .status,
            "cancelled"
        );
        let authoritative_orchestrator = db
            .get_workflow_automation_run(&orchestrator.id)
            .expect("workflow projection");
        assert_eq!(authoritative_orchestrator.status.as_str(), "cancelled");
        assert_eq!(
            authoritative_orchestrator.summary.as_deref(),
            Some("Stopped by user")
        );
    }

    #[test]
    fn stale_registry_failure_is_actionable_without_exposing_internal_ids() {
        let message = initialization_frontend_message(
            "Capability Registry resolution failed for run secret-run-id: Conflict: Capability target target:internal references a stale model-target revision",
        );
        assert!(message.contains("Retry the message"));
        assert!(message.contains("Settings > AI Providers"));
        assert!(!message.contains("secret-run-id"));
        assert!(!message.contains("target:internal"));
    }

    #[test]
    fn reused_interaction_with_active_siblings_stays_suspended() {
        let launch = AgentTurnLaunchRecord {
            conversation_id: "conversation-1".to_string(),
            user_message_id: "message-1".to_string(),
            user_message_sort_order: 1,
            turn_id: "turn-1".to_string(),
            run_id: "run-1".to_string(),
            status: "awaiting_user_input".to_string(),
            reused: true,
        };

        assert!(reused_launch_needs_no_executor(&launch, true, false));
        assert!(!reused_launch_needs_no_executor(
            &AgentTurnLaunchRecord {
                status: "queued".to_string(),
                ..launch
            },
            true,
            false,
        ));
    }

    #[test]
    fn reused_terminal_checkpoint_never_starts_an_executor() {
        for status in ["completed", "failed", "timed_out", "cancelled"] {
            let launch = AgentTurnLaunchRecord {
                conversation_id: "conversation-1".to_string(),
                user_message_id: "message-1".to_string(),
                user_message_sort_order: 1,
                turn_id: "turn-1".to_string(),
                run_id: "run-1".to_string(),
                status: status.to_string(),
                reused: true,
            };
            assert!(reused_launch_needs_no_executor(&launch, true, false));
        }
    }

    #[test]
    fn paused_launch_is_resumable_nonterminal_state() {
        let launch = desktop_agent_chat_launch(
            &AgentTurnLaunchRecord {
                conversation_id: "conversation-1".to_string(),
                user_message_id: "message-1".to_string(),
                user_message_sort_order: 1,
                turn_id: "turn-1".to_string(),
                run_id: "run-1".to_string(),
                status: "paused".to_string(),
                reused: true,
            },
            None,
        );

        assert_eq!(launch.handle.state, AgentTurnState::Paused);
    }

    #[test]
    fn committed_retry_setup_failure_is_terminal_and_allows_another_retry() {
        let db = Database::open_memory().expect("open memory db");
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        let message = ConversationMessage {
            id: "retry-user".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Retry this answer".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        let original = db
            .create_agent_turn_and_run(
                &message,
                "Original run",
                Some("open_ai"),
                Some("gpt-test"),
                "retry-original",
            )
            .expect("create original run");
        db.finish_agent_task_run(&original.run_id, "completed", None, None, None)
            .expect("finish original run");
        db.finalize_conversation_turn(&original.turn_id, "success", None, None)
            .expect("finish original turn");

        let committed = db
            .retry_agent_turn_and_run(
                &message,
                &message.id,
                "Committed retry",
                Some("open_ai"),
                Some("gpt-test"),
                "retry-committed",
            )
            .expect("commit retry transaction");
        assert!(reconcile_pre_executor_launch_failure(
            &db,
            &committed.run_id,
            None,
            &committed.turn_id,
            "outbox open failed",
        ));
        assert_eq!(
            db.get_agent_task_run(&committed.run_id)
                .expect("failed committed retry")
                .status,
            "failed"
        );

        let next = db.retry_agent_turn_and_run(
            &message,
            &message.id,
            "Retry after setup failure",
            Some("open_ai"),
            Some("gpt-test"),
            "retry-after-setup-failure",
        );
        assert!(
            next.is_ok(),
            "terminalized setup failure must not block retry"
        );
    }

    #[test]
    fn awaiting_input_stop_uses_terminal_cancel_path() {
        assert_eq!(
            interaction_run_stop_path("awaiting_user_input"),
            InteractionRunStopPath::FinalizeCancellation,
        );
        assert_eq!(
            interaction_run_stop_path("paused"),
            InteractionRunStopPath::AlreadyPaused,
        );
        assert_eq!(
            interaction_run_stop_path("running"),
            InteractionRunStopPath::PauseRunning,
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_desktop_agent_initialization_failure(
    db: &Database,
    app_handle: &AppHandle,
    conversation_id: &str,
    task_run_id: &str,
    task_orchestrator_run_id: Option<&str>,
    turn_id: &str,
    event_seq: &AgentRunEventOutbox,
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
    let payload = serde_json::json!({ "stage": "initialization", "reason": error });
    let frontend_message = initialization_frontend_message(error);
    let initialization_candidate = AgentRunEvent::terminal_error(
        task_run_id,
        Some(turn_id),
        0,
        frontend_message,
        status,
        Some(&payload),
    );
    let initialization_candidate_submitted = match event_seq.submit(initialization_candidate) {
        Ok(()) => true,
        Err(AgentRunEventSubmitError::AlreadyClosed) => false,
        Err(submit_error) => {
            warn!(
                "Failed to submit initialization terminal RunEvent for {conversation_id}: {submit_error}"
            );
            false
        }
    };
    if let Err(outbox_error) = event_seq.wait_for_terminal_commit().await {
        warn!(
            "Run Event outbox did not durably terminalize initialization failure for {conversation_id}: {outbox_error}"
        );
        reconcile_authoritative_run_event_outbox_failure(
            db,
            task_run_id,
            task_orchestrator_run_id,
            turn_id,
            &outbox_error,
        );
        emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
        return;
    }

    reconcile_initialization_terminal_barrier(
        db,
        task_run_id,
        task_orchestrator_run_id,
        turn_id,
        initialization_candidate_submitted,
        status,
        turn_status,
        summary,
        error,
        &trace,
    );
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
}

#[allow(clippy::too_many_arguments)]
fn reconcile_initialization_terminal_barrier(
    db: &Database,
    task_run_id: &str,
    task_orchestrator_run_id: Option<&str>,
    turn_id: &str,
    initialization_candidate_submitted: bool,
    initialization_status: &str,
    initialization_turn_status: &str,
    initialization_summary: &str,
    initialization_error: &str,
    initialization_trace: &serde_json::Value,
) -> bool {
    let task = match db.get_agent_task_run(task_run_id) {
        Ok(task) => task,
        Err(error) => {
            warn!("Failed to load initialization terminal projection {task_run_id}: {error}");
            return false;
        }
    };

    if initialization_candidate_submitted && task.status == initialization_status {
        let _ = db.finalize_conversation_turn(
            turn_id,
            initialization_turn_status,
            None,
            Some(initialization_trace),
        );
        let _ = db.finish_agent_task_run(
            task_run_id,
            initialization_status,
            Some(initialization_summary),
            Some(initialization_error),
            Some(initialization_trace),
        );
        if let Some(run_id) = task_orchestrator_run_id {
            if let Err(error) = db.transition_workflow_automation_run(
                run_id,
                initialization_status,
                Some(initialization_summary),
            ) {
                warn!("Failed to finalize Task Orchestrator run {run_id}: {error}");
            }
        }
        return true;
    }

    let authoritative_turn_status = match task.status.as_str() {
        "completed" => "success",
        "failed" | "timed_out" => "error",
        "cancelled" => "cancelled",
        status => {
            warn!(
                "Initialization terminal reconciliation found nonterminal task status {status} for {task_run_id}; preserving it"
            );
            return false;
        }
    };
    if let Err(error) =
        db.finalize_conversation_turn(turn_id, authoritative_turn_status, None, None)
    {
        warn!("Failed to reconcile conversation turn {turn_id}: {error}");
    }
    if let Some(run_id) = task_orchestrator_run_id {
        if let Err(error) =
            db.transition_workflow_automation_run(run_id, &task.status, task.summary.as_deref())
        {
            warn!("Failed to reconcile Task Orchestrator run {run_id}: {error}");
        }
    }
    false
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
        "paused" => AgentTurnState::Paused,
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
pub fn get_model_context_window_resolution(
    provider: String,
    base_url: Option<String>,
    model: String,
) -> nexa_core::conversation::memory::ResolvedContextWindow {
    nexa_core::provider_catalog::resolve_endpoint_model_context_window(
        &provider,
        base_url.as_deref(),
        &model,
        None,
    )
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteractionRunStopPath {
    AlreadyPaused,
    PauseRunning,
    FinalizeCancellation,
}

fn interaction_run_stop_path(status: &str) -> InteractionRunStopPath {
    match status {
        "paused" => InteractionRunStopPath::AlreadyPaused,
        "awaiting_user_input" | "cancelling" => InteractionRunStopPath::FinalizeCancellation,
        _ => InteractionRunStopPath::PauseRunning,
    }
}

#[tauri::command]
pub async fn agent_stop_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    approval_state: tauri::State<'_, ApprovalState>,
    app_handle: AppHandle,
    conversation_id: String,
) -> Result<(), String> {
    if let Some(run_id) = state
        .db
        .stoppable_interaction_run_for_conversation(&conversation_id)
        .map_err(|error| error.to_string())?
    {
        let _run_lifecycle_guard = agent_state.sessions.acquire_run_lifecycle(&run_id).await;
        let run = state
            .db
            .get_agent_task_run(&run_id)
            .map_err(|error| error.to_string())?;
        match interaction_run_stop_path(&run.status) {
            InteractionRunStopPath::AlreadyPaused => return Ok(()),
            InteractionRunStopPath::PauseRunning => {
                if let Ok(Some(turn)) = agent_state
                    .sessions
                    .take_for_run(&conversation_id, &run_id)
                    .await
                {
                    if let Err(error) = request_desktop_running_agent_stop(
                        turn,
                        DesktopRunningAgentStopRequest {
                            db: state.db.clone(),
                            app_handle,
                            conversation_id,
                            pending_approvals: approval_state.pending.clone(),
                        },
                    )
                    .await
                    {
                        return Err(error.message);
                    }
                }
                return Ok(());
            }
            InteractionRunStopPath::FinalizeCancellation => {}
        }
        let mut event_outbox = None;
        let mut task_orchestrator_run_id = None;
        if let Some(turn) = agent_state.sessions.take(&conversation_id).await {
            if turn.handle.run_id == run_id {
                event_outbox = Some(Arc::clone(&turn.event_outbox));
                task_orchestrator_run_id = turn.orchestrator_run_id.clone();
                turn.task.abort();
                turn.cancel_token.cancel();
                let _ = turn.task.await;
            } else {
                agent_state.sessions.register(turn).await;
            }
        }
        let event_outbox = match event_outbox {
            Some(outbox) => outbox,
            None => state
                .run_event_outboxes
                .open(&conversation_id, &run_id)
                .await
                .map_err(|error| error.to_string())?,
        };
        if run.status == "awaiting_user_input" {
            let cancelling = AgentRunEvent::status_update(
                &run_id,
                Some(&run.turn_id),
                0,
                AgentRunPhase::AwaitingUserInput,
                "Stop requested while waiting for user input",
                Some("cancelling"),
                Some(&serde_json::json!({
                    "reason": "cancelled_while_awaiting_user_input"
                })),
            );
            match event_outbox.submit(cancelling) {
                Ok(()) | Err(AgentRunEventSubmitError::AlreadyClosed) => {}
                Err(error) => return Err(error.to_string()),
            }
            event_outbox
                .flush()
                .await
                .map_err(|error| error.to_string())?;
        }
        state
            .db
            .cancel_interactions_for_stopped_run(&run_id)
            .map_err(|error| error.to_string())?;
        if task_orchestrator_run_id.is_none() {
            task_orchestrator_run_id = state
                .db
                .get_workflow_automation_run_for_task_run(&run_id)
                .map_err(|error| error.to_string())?
                .map(|workflow| workflow.id);
        }
        finalize_desktop_agent_stop(DesktopAgentStopFinalization {
            db: state.db.as_ref(),
            app_handle: &app_handle,
            conversation_id: &conversation_id,
            task_run_id: &run_id,
            task_orchestrator_run_id: task_orchestrator_run_id.as_deref(),
            turn_id: &run.turn_id,
            event_seq: event_outbox.as_ref(),
            reason: "cancelled_while_awaiting_user_input",
            summary: "Cancelled while waiting for user input",
        })
        .await;
        return Ok(());
    }

    if let Some(turn) = agent_state.sessions.take(&conversation_id).await {
        if let Err(error) = request_desktop_running_agent_stop(
            turn,
            DesktopRunningAgentStopRequest {
                db: state.db.clone(),
                app_handle,
                conversation_id,
                pending_approvals: approval_state.pending.clone(),
            },
        )
        .await
        {
            return Err(error.message);
        }
    }
    Ok(())
}
