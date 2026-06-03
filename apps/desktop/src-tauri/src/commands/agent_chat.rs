use super::*;
use crate::desktop_agent_session::{
    build_desktop_agent_session_dependencies, finalize_desktop_agent_stop,
    finalize_desktop_agent_turn, run_desktop_agent_turn, DesktopAgentApprovalRuntime,
    DesktopAgentSessionDependencyRequest, DesktopAgentStopFinalization,
    DesktopAgentTurnFinalization, DesktopAgentTurnRequest, DesktopAgentTurnRuntime,
    DesktopAgentTurnStream,
};

// ── Agent Chat Command (streaming) ──────────────────────────────────────

fn execution_mode_artifact(execution_mode: AgentExecutionMode) -> serde_json::Value {
    serde_json::json!({
        "kind": "executionMode",
        "version": 1,
        "mode": execution_mode.as_str(),
    })
}

fn annotate_user_artifacts_with_execution_mode(
    artifacts: Option<serde_json::Value>,
    execution_mode: AgentExecutionMode,
) -> Option<serde_json::Value> {
    if !execution_mode.is_plan() {
        return artifacts;
    }

    let marker = execution_mode_artifact(execution_mode);
    match artifacts {
        None => Some(marker),
        Some(serde_json::Value::Object(mut map)) => {
            map.insert("executionMode".to_string(), marker);
            Some(serde_json::Value::Object(map))
        }
        Some(value) => Some(serde_json::json!({
            "kind": "chatSendContext",
            "userArtifacts": value,
            "executionMode": marker,
        })),
    }
}

pub(super) struct DesktopAgentSessionConfigInput<'a> {
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub db_config: &'a DbAgentConfig,
    pub app_cfg: &'a AppConfig,
    pub source_scope_ids: &'a [String],
    pub selected_skills: &'a [Skill],
    pub auto_loaded_skills: &'a [Skill],
    pub execution_mode: AgentExecutionMode,
}

pub(super) fn build_desktop_agent_session_config(
    input: DesktopAgentSessionConfigInput<'_>,
) -> nexa_core::runtime::AgentSessionConfig {
    let mut config = nexa_core::runtime::AgentSessionConfig {
        session_id: input.conversation_id.to_string(),
        conversation_id: Some(input.conversation_id.to_string()),
        task_run_id: Some(input.task_run_id.to_string()),
        host_surface: nexa_core::runtime::RuntimeHostSurface::Desktop,
        provider: Some(input.db_config.provider.clone()),
        model: Some(input.db_config.model.clone()),
        reasoning_enabled: input.db_config.reasoning_enabled,
        thinking_budget: input.db_config.thinking_budget.map(|value| value as u32),
        reasoning_effort: input.db_config.reasoning_effort.clone(),
        source_scope: nexa_core::runtime::RuntimeSourceScope {
            source_ids: input.source_scope_ids.to_vec(),
            collection_id: None,
            working_directory: None,
        },
        approval_mode: input.app_cfg.tool_approval_mode,
        shell_access_mode: input.app_cfg.shell_access_mode,
        execution_mode: input.execution_mode,
        trace_enabled: input.app_cfg.trace_enabled,
        skill_context: nexa_core::runtime::RuntimeSkillContext {
            available_skill_ids: input
                .selected_skills
                .iter()
                .map(|skill| skill.id.clone())
                .collect(),
            loaded_skill_ids: input
                .auto_loaded_skills
                .iter()
                .map(|skill| skill.id.clone())
                .collect(),
            trust_state: None,
        },
        package_context: desktop_runtime_package_context(),
        metadata: serde_json::json!({
            "kind": "desktopAgentSessionConfig",
            "agentConfigId": input.db_config.id,
            "agentConfigName": input.db_config.name,
        }),
        ..Default::default()
    };
    config.apply_protocol_defaults();
    config
}

pub(super) fn desktop_runtime_package_context() -> nexa_core::runtime::RuntimePackageContext {
    let manifests = nexa_core::ecosystem::builtin_ecosystem_manifests();
    let snapshot = nexa_core::package_host::package_host_snapshot_from_manifests(&manifests);
    nexa_core::runtime::RuntimePackageContext::from_package_host_snapshot(&snapshot)
}

pub(super) fn runtime_session_config_artifact(
    config: &nexa_core::runtime::AgentSessionConfig,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentSessionConfig",
        "version": 1,
        "config": config,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn agent_chat_cmd(
    state: tauri::State<'_, AppState>,
    agent_state: tauri::State<'_, AgentState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    approval_state: tauri::State<'_, ApprovalState>,
    app_handle: AppHandle,
    conversation_id: String,
    message: String,
    attachments: Option<Vec<ImageAttachment>>,
    agent_config_id: Option<String>,
    persona_id: Option<String>,
    skill_ids: Option<Vec<String>>,
    execution_mode: Option<String>,
    user_artifacts: Option<serde_json::Value>,
) -> Result<(), String> {
    let execution_mode = AgentExecutionMode::from_wire(execution_mode.as_deref())?;
    let plan_mode = execution_mode.is_plan();

    // 1. Load the conversation first so provider/model selection follows the
    // active chat, not whatever global default happened to be selected later.
    let mut conv = state
        .db
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;

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

    // 5. Save user message to DB.
    let persisted_user_artifacts =
        annotate_user_artifacts_with_execution_mode(user_artifacts, execution_mode);
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
    state.db.add_message(&user_msg).map_err(|e| e.to_string())?;
    let turn = state
        .db
        .create_conversation_turn(&conversation_id, &user_msg.id, None)
        .map_err(|e| e.to_string())?;
    let task_run = state
        .db
        .create_agent_task_run(
            &conversation_id,
            &turn.id,
            &user_msg.id,
            &task_title_from_message(&message),
            Some(&db_config.provider),
            Some(&db_config.model),
        )
        .map_err(|e| e.to_string())?;
    let task_run_id_for_command = task_run.id.clone();
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

    // 6. Build prompt sections from conversation context.
    let source_scope_ids = state
        .db
        .get_effective_conversation_source_scope(&conversation_id)
        .unwrap_or_default();
    let source_scope_section =
        nexa_core::conversation::build_source_scope_prompt_section(&state.db, &source_scope_ids)
            .unwrap_or_default();
    let collection_context_section =
        nexa_core::conversation::build_collection_context_prompt_section(
            conv.collection_context.as_ref(),
        );
    let memory_section =
        nexa_core::personalization::build_memory_summary_for_query(&state.db, Some(&message))
            .unwrap_or_default();
    let project_memory_section = nexa_core::project_memory::build_project_memory_summary_for_query(
        &state.db,
        conv.project_id.as_deref(),
        Some(&message),
    )
    .unwrap_or_default();
    let agent_memory_section =
        nexa_core::evolution::build_agent_procedural_memory_summary_for_query(
            &state.db,
            Some(&message),
        )
        .unwrap_or_default();
    if !agent_memory_section.is_empty() {
        let memory_hits = state
            .db
            .search_agent_procedural_memories(&message, 3)
            .or_else(|_| state.db.list_agent_procedural_memories(2))
            .unwrap_or_default();
        for memory in memory_hits {
            let _ = state.db.record_memory_injection_event(
                &memory.id,
                Some(&conversation_id),
                Some(&turn.id),
                &message,
                "agent_procedural_memory_prompt",
                Some(memory.confidence),
            );
        }
    }
    let preference_section =
        nexa_core::personalization::build_preference_summary_for_query(&state.db, Some(&message))
            .unwrap_or_default();
    // Retrieve similar past upvoted turns and inject them as a few-shot
    // "Learned Successes" section. Embedding failures are non-fatal —
    // we just skip retrieval silently rather than blocking the turn.
    let learned_section = {
        let cfg = state.db.get_embedder_config().ok();
        let embedding = cfg.and_then(|c| match nexa_core::embed::create_embedder(&c) {
            Ok(embedder) if embedder.dimensions() > 0 => embedder.embed(&message).ok(),
            _ => None,
        });
        match embedding {
            Some(vec) if !vec.iter().all(|&v| v == 0.0) => {
                match nexa_core::learning::retrieve_similar_successes(&state.db, &vec, 3) {
                    Ok(hits) => nexa_core::learning::build_learned_successes_section(&hits),
                    Err(_) => String::new(),
                }
            }
            _ => String::new(),
        }
    };
    let scratchpad_section = nexa_core::agent::scratchpad::build_agent_scratchpad_prompt_section(
        &state.db,
        Some(&conversation_id),
    );
    let requested_persona_id = persona_id
        .as_deref()
        .or(conv.persona_id.as_deref())
        .unwrap_or("default");
    let persona_profile =
        match nexa_core::persona::enabled_persona_by_id(&state.db, requested_persona_id) {
            Ok(persona) => persona,
            Err(err) => {
                warn!("Failed to load persona '{requested_persona_id}': {err}");
                None
            }
        };
    let effective_persona_id = persona_profile
        .as_ref()
        .map(|persona| persona.id.as_str())
        .unwrap_or("default");
    if conv.persona_id.as_deref().unwrap_or("default") != effective_persona_id {
        let _ = state.db.update_conversation_persona(
            &conversation_id,
            if effective_persona_id == "default" {
                None
            } else {
                Some(effective_persona_id)
            },
        );
    }
    let persona_default_skill_ids = persona_profile
        .as_ref()
        .map(|persona| persona.default_skill_ids.clone())
        .unwrap_or_default();
    let mut pinned_skill_ids = persona_default_skill_ids;
    if let Some(explicit_skill_ids) = skill_ids {
        for id in explicit_skill_ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() && !pinned_skill_ids.iter().any(|existing| existing == trimmed) {
                pinned_skill_ids.push(trimmed.to_string());
            }
        }
    }
    let persona_section =
        nexa_core::persona::build_persona_prompt_section(persona_profile.as_ref());
    let current_turn_time_section = build_current_turn_time_section();
    let plan_mode_section = if plan_mode {
        nexa_core::agent::plan_mode_prompt_section()
    } else {
        ""
    };
    let system_prompt = build_system_prompt(Some(&conv.system_prompt), &[]);
    let volatile_system_sections = vec![
        current_turn_time_section,
        plan_mode_section.to_string(),
        persona_section,
        collection_context_section,
        source_scope_section,
        memory_section,
        project_memory_section,
        agent_memory_section,
        preference_section,
        learned_section,
        scratchpad_section,
    ];

    // 6. Build executor config from DB config.
    let executor_config = ExecutorConfig {
        max_iterations: db_config.max_iterations.map(|v| v as u32).unwrap_or(25),
        system_prompt,
        volatile_system_sections,
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config.max_tokens.map(|t| t as u32),
        context_window: db_config.context_window.map(|w| w as u32),
        reasoning_enabled: db_config.reasoning_enabled,
        thinking_budget: db_config.thinking_budget.map(|v| v as u32),
        reasoning_effort: db_config
            .reasoning_effort
            .as_ref()
            .and_then(|s| match s.as_str() {
                "none" => Some(ReasoningEffort::None),
                "minimal" => Some(ReasoningEffort::Minimal),
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                "max" => Some(ReasoningEffort::Max),
                "xhigh" => Some(ReasoningEffort::XHigh),
                _ => None,
            }),
        provider_type: Some(provider_type_for_config(&db_config)),
        summarization_model: db_config.summarization_model.clone(),
        subagent_max_parallel: db_config.subagent_max_parallel.map(|v| v as u32),
        subagent_max_calls_per_turn: db_config.subagent_max_calls_per_turn.map(|v| v as u32),
        subagent_token_budget: db_config.subagent_token_budget.map(|v| v as u32),
        tool_timeout_secs: Some(UNLIMITED_EXECUTOR_TIMEOUT_SECS),
        agent_timeout_secs: Some(UNLIMITED_EXECUTOR_TIMEOUT_SECS),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: app_cfg.dynamic_tool_visibility,
        trace_enabled: app_cfg.trace_enabled,
        require_tool_confirmation: app_cfg.confirm_destructive,
        shell_access_mode: app_cfg.shell_access_mode,
        tool_approval_mode: app_cfg.tool_approval_mode,
        execution_mode,
    };

    // 6b. Create a separate summarization provider if configured.
    let summarization_provider: Option<Box<dyn nexa_core::llm::LlmProvider>> =
        if let Some(ref summ_provider_name) = db_config.summarization_provider {
            let summ_config = ProviderConfig {
                provider_type: provider_type_for_parts(summ_provider_name, None),
                api_key: Some(db_config.api_key.clone()),
                base_url: db_config.base_url.clone(),
                org_id: None,
                timeout_secs: None,
            };
            create_provider(summ_config).ok()
        } else if db_config.summarization_model.is_some() {
            // Same provider, different model — reuse the main provider config.
            None
        } else {
            None
        };

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
        })
        .await;
    let runtime_session_config =
        build_desktop_agent_session_config(DesktopAgentSessionConfigInput {
            conversation_id: &conversation_id,
            task_run_id: &task_run.id,
            db_config: &db_config,
            app_cfg: &app_cfg,
            source_scope_ids: &source_scope_ids,
            selected_skills: &session_dependencies.selected_skills,
            auto_loaded_skills: &session_dependencies.auto_loaded_skills,
            execution_mode,
        });
    let mut initial_task_artifacts = serde_json::json!({
        "kind": "agentTaskArtifacts",
        "version": 1,
        "selectedSkills": build_selected_skills_artifact(&session_dependencies.selected_skills),
        "runtimeSession": runtime_session_config_artifact(&runtime_session_config),
    });
    if plan_mode {
        initial_task_artifacts["executionMode"] = execution_mode_artifact(execution_mode);
    }
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
    let vision_supported = model_supports_vision(&provider_config.provider_type, &db_config.model);
    info!(
        "Attachment check: provider={}, model={}, provider_type={:?}, vision_supported={}, has_attachments={}",
        db_config.provider, db_config.model, provider_config.provider_type, vision_supported, attachments.is_some()
    );
    let mut user_parts = vec![ContentPart::Text {
        text: message.clone(),
    }];
    if let Some(atts) = &attachments {
        for att in atts {
            if att.media_type.starts_with("image/") {
                // ── Image attachment ──
                if vision_supported {
                    user_parts.push(ContentPart::Image {
                        media_type: att.media_type.clone(),
                        data: att.base64_data.clone(),
                    });
                } else {
                    // Model doesn't support vision — OCR fallback
                    warn!(
                        "Model '{}' (provider {:?}) does not support vision. Using OCR fallback for image '{}'.",
                        db_config.model, provider_config.provider_type, att.original_name
                    );
                    emit_app_event(
                        &app_handle,
                        "image:ocr-fallback",
                        &serde_json::json!({
                            "image_name": att.original_name,
                            "model": db_config.model,
                            "reason": "Model does not support native image inputs"
                        }),
                    );
                    let ocr_config = state.db.load_ocr_config().unwrap_or_default();
                    let image_bytes = base64::engine::general_purpose::STANDARD
                        .decode(&att.base64_data)
                        .map_err(|e| format!("Failed to decode image: {}", e))?;
                    let ocr_result =
                        extract_text_from_image(&image_bytes, &att.media_type, &ocr_config, None);
                    info!(
                        "OCR fallback result for non-vision model: success={}, text_len={}",
                        ocr_result.is_ok(),
                        ocr_result.as_ref().map(|r| r.full_text.len()).unwrap_or(0)
                    );
                    match ocr_result {
                        Ok(result) if !result.full_text.is_empty() => {
                            user_parts.push(ContentPart::Text {
                                text: format!(
                                    "[Image \"{}\" — processed via OCR (model does not support native vision)]:\n{}",
                                    att.original_name, result.full_text
                                ),
                            });
                        }
                        _ => {
                            warn!(
                                "OCR fallback also failed for image '{}'. Install OCR model or use a vision-capable model.",
                                att.original_name
                            );
                            emit_app_event(
                                &app_handle,
                                "image:ocr-failed",
                                &serde_json::json!({
                                    "image_name": att.original_name,
                                    "model": db_config.model,
                                    "hint": "Install OCR model in Settings or switch to a vision-capable model"
                                }),
                            );
                            user_parts.push(ContentPart::Text {
                                text: format!(
                                    "[Image \"{}\" attached but could not be processed — this model does not support image inputs and OCR is not available. Install the OCR model in Settings or use a vision-capable model.]",
                                    att.original_name
                                ),
                            });
                        }
                    }
                }
            } else {
                // ── Document attachment — parse to text ──
                const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&att.base64_data)
                    .map_err(|e| format!("Failed to decode attachment: {}", e))?;
                if bytes.len() > MAX_ATTACHMENT_BYTES {
                    warn!(
                        "Attachment '{}' is too large ({} bytes, limit {}). Skipping.",
                        att.original_name,
                        bytes.len(),
                        MAX_ATTACHMENT_BYTES
                    );
                    user_parts.push(ContentPart::Text {
                        text: format!(
                            "[Attached file \"{}\" skipped — file too large ({:.1} MB, limit 10 MB)]",
                            att.original_name,
                            bytes.len() as f64 / (1024.0 * 1024.0)
                        ),
                    });
                    continue;
                }
                let ext = mime_to_extension(&att.media_type);
                let temp_path =
                    std::env::temp_dir().join(format!("nexa-attach-{}.{}", Uuid::new_v4(), ext));
                if let Err(e) = std::fs::write(&temp_path, &bytes) {
                    warn!(
                        "Failed to write temp file for attachment '{}': {}",
                        att.original_name, e
                    );
                    user_parts.push(ContentPart::Text {
                        text: format!(
                            "[Attached file \"{}\" — could not process: {}]",
                            att.original_name, e
                        ),
                    });
                    continue;
                }
                let parse_result = nexa_core::parse::parse_file(
                    &temp_path,
                    None,
                    #[cfg(feature = "video")]
                    None,
                    None,
                    None,
                    None,
                );
                let _ = std::fs::remove_file(&temp_path);
                match parse_result {
                    Ok(parsed) => {
                        let text: String = parsed
                            .chunks
                            .iter()
                            .map(|c| c.content.as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let visual_text = parsed
                            .visual_artifacts
                            .iter()
                            .map(|artifact| artifact.to_chunk_content())
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let combined_text = [text.as_str(), visual_text.as_str()]
                            .into_iter()
                            .map(str::trim)
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if combined_text.trim().is_empty() {
                            user_parts.push(ContentPart::Text {
                                text: format!(
                                    "[Attached file \"{}\" — no text content could be extracted]",
                                    att.original_name
                                ),
                            });
                        } else {
                            info!(
                                "Parsed document attachment '{}': {} chars",
                                att.original_name,
                                combined_text.len()
                            );
                            user_parts.push(ContentPart::Text {
                                text: format!(
                                    "[Attached file: {}]\n\n{}",
                                    att.original_name, combined_text
                                ),
                            });
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse attachment '{}': {}", att.original_name, e);
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Attached file \"{}\" — could not extract content: {}]",
                                att.original_name, e
                            ),
                        });
                    }
                }
            }
        }
    }

    // 8. Spawn the agent loop in a background task.
    let db = state.db.clone();
    let conv_id = conversation_id.clone();
    let turn_id = turn.id.clone();
    let task_run_id = task_run.id.clone();
    let handle = app_handle.clone();
    let assistant_sort_order = next_sort_order + 1;
    let db_config_for_extraction = db_config.clone();
    let command_stream_event_seq = Arc::clone(&stream_event_seq);
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
                event_seq: Arc::clone(&stream_event_seq),
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
                    stream_event_seq.as_ref(),
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
                    stream_event_seq.as_ref(),
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
                    stream_event_seq.as_ref(),
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
            turn_id: &turn_id,
            event_seq: stream_event_seq.as_ref(),
            outcome: &outcome,
        });

        // Auto memory extraction (background, best-effort).
        if matches!(result, Some(Ok(_))) {
            let app_cfg = db.load_app_config().unwrap_or_default();
            if app_cfg.auto_memory_extraction {
                // Determine the model: prefer summarization model, fall back to main.
                let extract_model = db_config_for_extraction
                    .summarization_model
                    .as_deref()
                    .unwrap_or(&db_config_for_extraction.model);
                // Build a provider for extraction (reuse summarization provider config or main).
                let extract_provider_config =
                    if let Some(ref sp) = db_config_for_extraction.summarization_provider {
                        ProviderConfig {
                            provider_type: provider_type_for_parts(sp, None),
                            api_key: Some(db_config_for_extraction.api_key.clone()),
                            base_url: db_config_for_extraction.base_url.clone(),
                            org_id: None,
                            timeout_secs: None,
                        }
                    } else {
                        ProviderConfig {
                            provider_type: provider_type_for_config(&db_config_for_extraction),
                            api_key: Some(db_config_for_extraction.api_key.clone()),
                            base_url: db_config_for_extraction.base_url.clone(),
                            org_id: None,
                            timeout_secs: None,
                        }
                    };
                if let Ok(extract_llm) = create_provider(extract_provider_config) {
                    match nexa_core::personalization::auto_extract_and_save(
                        &db,
                        &conv_id,
                        extract_llm.as_ref(),
                        extract_model,
                    )
                    .await
                    {
                        Ok(n) if n > 0 => {
                            info!("Auto-extracted {n} memories from conversation {conv_id}");
                        }
                        Err(e) => {
                            warn!("Auto memory extraction failed for {conv_id}: {e}");
                        }
                        _ => {}
                    }
                }
            }

            if app_cfg.auto_skill_learning {
                match nexa_core::evolution::review_recent_traces_for_evolution(&db, 5) {
                    Ok(review) if review.events_created > 0 => {
                        info!(
                            "Agent evolution review created {} event(s) for conversation {}",
                            review.events_created, conv_id
                        );
                    }
                    Err(e) => warn!("Agent evolution review failed for {conv_id}: {e}"),
                    _ => {}
                }
            }
        }
    });

    // 8. Track the running task for potential cancellation.
    {
        let mut running = agent_state.running.lock().await;
        // Cancel any existing task for this conversation.
        if let Some(prev) = running.remove(&conversation_id) {
            prev.cancel_token.cancel();
            prev.task.abort();
        }
        running.insert(
            conversation_id,
            RunningAgentTask {
                cancel_token,
                task,
                steering_tx,
                task_run_id: task_run_id_for_command,
                turn_id: turn.id.clone(),
                stream_event_seq: Arc::clone(&command_stream_event_seq),
            },
        );
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
    let Some(task) = running.get(&conversation_id) else {
        return Err("No running agent for this conversation.".to_string());
    };

    if task.task.is_finished() {
        running.remove(&conversation_id);
        return Err("No running agent for this conversation.".to_string());
    }

    task.steering_tx
        .send(AgentSteeringMessage::text(trimmed.to_string()))
        .map_err(|_| "Running agent is no longer accepting steering messages.".to_string())
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
    if let Some(task_state) = running.remove(&conversation_id) {
        let task_run_id = task_state.task_run_id.clone();
        let turn_id = task_state.turn_id.clone();
        let stream_event_seq = Arc::clone(&task_state.stream_event_seq);
        let _ = state.db.update_agent_task_run_progress(
            &task_run_id,
            Some("cancelling"),
            Some("cancelling"),
            None,
            Some("Stop requested"),
            None,
            None,
        );
        let run_event = emit_agent_frontend_event(
            &app_handle,
            stream_event_seq.as_ref(),
            &conversation_id,
            &task_run_id,
            Some(&turn_id),
            AgentEvent::Status {
                content: "Stop requested".to_string(),
                tone: Some("muted".to_string()),
            },
        );
        record_agent_run_task_event(
            &state.db,
            &app_handle,
            &conversation_id,
            &task_run_id,
            &run_event,
            run_event.task_event_type(),
            "Stop requested",
            Some("cancelling"),
            None,
        );
        emit_agent_task_run_update(&state.db, &app_handle, &conversation_id, &task_run_id);

        // Signal cooperative cancellation first so the agent can save
        // partial work, then abort the task as a fallback.
        task_state.cancel_token.cancel();
        // Give cooperative cancellation 2 seconds to save partial state
        // before forcibly aborting the task.
        let abort_task = task_state.task;
        let db = state.db.clone();
        let handle = app_handle.clone();
        let conv_id = conversation_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if !abort_task.is_finished() {
                abort_task.abort();
                finalize_desktop_agent_stop(DesktopAgentStopFinalization {
                    db: &db,
                    app_handle: &handle,
                    conversation_id: &conv_id,
                    task_run_id: &task_run_id,
                    turn_id: &turn_id,
                    event_seq: stream_event_seq.as_ref(),
                    reason: "aborted_after_cancel_timeout",
                    summary: "Stopped by user",
                });
            }
        });
    }
    Ok(())
}
