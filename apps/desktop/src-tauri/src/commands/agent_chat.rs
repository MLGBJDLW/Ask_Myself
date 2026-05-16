use super::*;

// ── Agent Chat Command (streaming) ──────────────────────────────────────

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
) -> Result<(), String> {
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
    let provider_config = db_config_to_provider_config(&db_config, Some(app_cfg.llm_timeout_secs));
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
    let user_msg = ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: message.clone(),
        tool_call_id: None,
        tool_calls: vec![],
        artifacts: None,
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
    let memory_section = if conv.project_id.is_some() {
        String::new()
    } else {
        nexa_core::personalization::build_memory_summary_for_query(&state.db, Some(&message))
            .unwrap_or_default()
    };
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
    let selected_skills = if persona_default_skill_ids.is_empty() {
        nexa_core::skills::get_active_skills_for_query(&state.db, &message, 5)
    } else {
        nexa_core::skills::get_active_skills_for_query_with_pinned(
            &state.db,
            &message,
            8,
            &persona_default_skill_ids,
        )
    }
    .unwrap_or_else(|err| {
        warn!(
            "Failed to select skills for task run {}: {err}",
            task_run.id
        );
        Vec::new()
    });
    let initial_task_artifacts = serde_json::json!({
        "kind": "agentTaskArtifacts",
        "version": 1,
        "selectedSkills": build_selected_skills_artifact(&selected_skills),
    });
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
    let persona_section =
        nexa_core::persona::build_persona_prompt_section(persona_profile.as_ref());
    let current_turn_time_section = build_current_turn_time_section();
    let system_prompt = build_system_prompt(
        Some(&conv.system_prompt),
        &[
            &current_turn_time_section,
            &persona_section,
            &collection_context_section,
            &source_scope_section,
            &memory_section,
            &project_memory_section,
            &agent_memory_section,
            &preference_section,
            &learned_section,
            &scratchpad_section,
        ],
    );

    // 6. Build executor config from DB config.
    let executor_config = ExecutorConfig {
        max_iterations: db_config.max_iterations.map(|v| v as u32).unwrap_or(25),
        system_prompt,
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
        tool_timeout_secs: Some(config_timeout_secs(
            db_config.tool_timeout_secs,
            app_cfg.tool_timeout_secs,
            30,
        )),
        agent_timeout_secs: Some(config_timeout_secs(
            db_config.agent_timeout_secs,
            app_cfg.agent_timeout_secs,
            180,
        )),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: app_cfg.dynamic_tool_visibility,
        trace_enabled: app_cfg.trace_enabled,
        require_tool_confirmation: app_cfg.confirm_destructive,
        shell_access_mode: app_cfg.shell_access_mode,
    };

    // 6b. Build confirmation callback if enabled.
    let confirmation_cb: Option<ConfirmationCallback> =
        if app_cfg.confirm_destructive || app_cfg.shell_access_mode.requires_confirmation() {
            let dialog_handle = app_handle.clone();
            Some(Arc::new(move |message: String| {
                let handle = dialog_handle.clone();
                Box::pin(async move {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    handle
                        .dialog()
                        .message(&message)
                        .title("Confirm Tool Execution")
                        .kind(MessageDialogKind::Warning)
                        .buttons(MessageDialogButtons::OkCancelCustom(
                            "Allow".into(),
                            "Deny".into(),
                        ))
                        .show(move |confirmed| {
                            let _ = tx.send(confirmed);
                        });
                    match tokio::time::timeout(Duration::from_secs(30), rx).await {
                        Ok(Ok(confirmed)) => confirmed,
                        _ => !message.starts_with("Run:"), // deny run_shell on timeout; allow others
                    }
                })
            }))
        } else {
            None
        };

    // 6c. Build per-call approval callback (new GUI flow).
    //
    // Always wired — the callback itself checks the global
    // `tool_approval_mode` and short-circuits for AllowAll/DenyAll. In
    // `Ask` mode it consults persisted `never` policies, then the
    // session allow-list, and finally emits an `ApprovalRequested`
    // event and blocks on a oneshot from `approve_tool_call_cmd`.
    let approval_cb: Option<ApprovalCallback> = {
        let db_handle = state.db.clone();
        let app_handle_cb = app_handle.clone();
        let approval_event_seq = Arc::clone(&stream_event_seq);
        let approval_task_run_id = task_run.id.clone();
        let approval_turn_id = turn.id.clone();
        let pending = approval_state.pending.clone();
        let session_store = approval_state.session_store.clone();
        let stream_conv_id = conversation_id.clone();
        let approval_mode = app_cfg.tool_approval_mode;
        Some(Arc::new(move |req: ApprovalRequest| {
            let db = db_handle.clone();
            let handle = app_handle_cb.clone();
            let pending = pending.clone();
            let store = session_store.clone();
            let conv = stream_conv_id.clone();
            let event_seq = Arc::clone(&approval_event_seq);
            let task_run_id = approval_task_run_id.clone();
            let turn_id = approval_turn_id.clone();
            Box::pin(async move {
                // 0. Global mode short-circuit.
                if let Some(d) = approval_mode.short_circuit() {
                    return d;
                }
                // 1. Persistent "never" policy. Prefer targeted rules and
                // fall back to legacy per-tool policies created before the
                // permission engine gained target keys.
                if let Ok(Some(pol)) = db.get_tool_permission_policy(&req.permission_key) {
                    if pol == "never" {
                        return ApprovalDecision::Deny;
                    }
                }
                let allow_legacy_tool_policy = req.tool_name != "project_tool";
                if allow_legacy_tool_policy {
                    if let Ok(Some(pol)) = db.get_tool_approval_policy(&req.tool_name) {
                        if pol == "never" {
                            return ApprovalDecision::Deny;
                        }
                    }
                }
                // 2. Session allow.
                if matches!(
                    store.get(&req.permission_key),
                    Some(ApprovalDecision::AllowSession)
                ) || (allow_legacy_tool_policy
                    && matches!(
                        store.get(&req.tool_name),
                        Some(ApprovalDecision::AllowSession)
                    ))
                {
                    return ApprovalDecision::AllowOnce;
                }
                // 3. Ask the UI — emit a synthetic frontend event
                //    (the executor also emits one, but routing through
                //    conversation_id makes the UI dispatcher simpler).
                let (tx, rx) = tokio::sync::oneshot::channel();
                pending.lock().await.insert(req.id.clone(), tx);
                emit_agent_frontend_event(
                    &handle,
                    &event_seq,
                    &conv,
                    &task_run_id,
                    Some(&turn_id),
                    AgentEvent::ApprovalRequested {
                        request: req.clone(),
                    },
                );
                // 4. Wait up to 60s for a decision; otherwise deny.
                let decision = match tokio::time::timeout(Duration::from_secs(60), rx).await {
                    Ok(Ok(d)) => d,
                    _ => {
                        pending.lock().await.remove(&req.id);
                        ApprovalDecision::Deny
                    }
                };
                // 5. Persist the decision according to scope.
                match decision {
                    ApprovalDecision::AllowSession => {
                        store.set(&req.permission_key, ApprovalDecision::AllowSession);
                    }
                    ApprovalDecision::Never => {
                        let key = ToolPermissionKey::from_request(&req);
                        let _ = db.save_tool_permission_policy(&key, "never");
                    }
                    _ => {}
                }
                decision
            })
        }))
    };

    // 6d. Create a separate summarization provider if configured.
    let summarization_provider: Option<Box<dyn nexa_core::llm::LlmProvider>> =
        if let Some(ref summ_provider_name) = db_config.summarization_provider {
            let summ_config = ProviderConfig {
                provider_type: provider_type_for_parts(summ_provider_name, None),
                api_key: Some(db_config.api_key.clone()),
                base_url: db_config.base_url.clone(),
                org_id: None,
                timeout_secs: Some(app_cfg.llm_timeout_secs),
            };
            create_provider(summ_config).ok()
        } else if db_config.summarization_model.is_some() {
            // Same provider, different model — reuse the main provider config.
            None
        } else {
            None
        };

    // 7. Create tool registry with built-in + MCP tools.
    let mut tools = default_tool_registry();

    // Register MCP tools from currently enabled servers.
    emit_agent_frontend_event(
        &app_handle,
        &stream_event_seq,
        &conversation_id,
        &task_run.id,
        Some(&turn.id),
        AgentEvent::Status {
            content: "Loading tools and MCP servers".to_string(),
            tone: None,
        },
    );
    {
        let mut mcp_manager = mcp_state.manager.lock().await;
        match sync_enabled_mcp_servers(&state.db, &mut mcp_manager).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id}: {error}");
                }
            }
            Err(error) => warn!("Failed to load enabled MCP servers: {error}"),
        }
        if let Err(e) = mcp_manager.register_tools(&mut tools).await {
            warn!("Failed to register MCP tools: {e}");
        }
    }

    let cancel_token = CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();
    let (steering_tx, steering_rx) = tokio::sync::mpsc::unbounded_channel::<AgentSteeringMessage>();

    let delegation_runtime = DelegationRuntime::new(
        provider_config.clone(),
        executor_config.clone(),
        db_config.subagent_allowed_tools.clone(),
        db_config.subagent_allowed_skill_ids.clone(),
        cancel_token.clone(),
        Some(task_run.id.clone()),
    );
    tools.register(Box::new(SubagentTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(SubagentBatchTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(JudgeSubagentResultsTool::from_runtime(
        delegation_runtime.clone(),
    )));
    delegation_runtime.set_tool_registry(tools.clone());

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
                        if text.trim().is_empty() {
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
                                text.len()
                            );
                            user_parts.push(ContentPart::Text {
                                text: format!("[Attached file: {}]\n\n{}", att.original_name, text),
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
    let forwarder_event_seq = Arc::clone(&stream_event_seq);
    let forwarder_terminal_emitted = Arc::clone(&terminal_emitted);
    let command_stream_event_seq = Arc::clone(&stream_event_seq);

    let turn_timeout_secs = executor_config.agent_timeout_secs.unwrap_or(180) as u64;

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
        let cancel_token = cancel_token_clone;
        let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);

        // Forward events to the frontend in a separate task.
        let event_forwarder = tokio::spawn(
            AgentStreamForwarder::new(
                handle.clone(),
                db.clone(),
                conv_id.clone(),
                task_run_id.clone(),
                turn_id.clone(),
                forwarder_event_seq,
                forwarder_terminal_emitted,
            )
            .run(rx),
        );

        // Run the agent.  The executor now saves ALL messages (intermediate
        // tool-call assistants, tool results, and the final answer) to the DB
        // using incrementing sort_order starting at `assistant_sort_order`.
        let executor_cancel_token = cancel_token.clone();
        let mut executor = AgentExecutor::new(provider, tools, executor_config)
            .with_cancel_token(executor_cancel_token)
            .with_steering_receiver(steering_rx);
        if let Some(cb) = confirmation_cb {
            executor = executor.with_confirmation_callback(cb);
        }
        if let Some(cb) = approval_cb {
            executor = executor.with_approval_callback(cb);
        }
        if let Some(summ_provider) = summarization_provider {
            executor = executor.with_summarization_provider(summ_provider);
        }
        executor = executor.with_skills_override(selected_skills);
        let run_future = executor.run(
            history,
            user_parts,
            &db,
            Some(&conv_id),
            Some(&turn_id),
            tx,
            assistant_sort_order,
        );

        // Keep the frontend stream alive while the agent is still running but
        // the upstream provider is temporarily silent (reasoning, tool work,
        // or SSE gaps). A timeout of 0 disables the hard turn stop; users can
        // still stop the run manually.
        let mut run_future = Box::pin(run_future);
        let mut turn_timeout = (turn_timeout_secs > 0)
            .then(|| Box::pin(tokio::time::sleep(Duration::from_secs(turn_timeout_secs))));
        let mut keepalive =
            tokio::time::interval(Duration::from_secs(STREAM_KEEPALIVE_INTERVAL_SECS));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;

        let (result, timed_out) = loop {
            tokio::select! {
                run_result = &mut run_future => break (Some(run_result), false),
                _ = async {
                    if let Some(timeout) = turn_timeout.as_mut() {
                        timeout.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => break (None, true),
                _ = keepalive.tick() => {
                    emit_agent_frontend_event(
                        &handle,
                        &stream_event_seq,
                        &conv_id,
                        &task_run_id,
                        Some(&turn_id),
                        AgentEvent::Thinking {
                            content: String::new(),
                        },
                    );
                }
            }
        };

        if timed_out {
            cancel_token.cancel();
        }

        drop(run_future);
        drop(turn_timeout);
        drop(executor);

        // Wait for event forwarder to finish.
        let _ = event_forwarder.await;

        match &result {
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

        let turn_snapshot = db.get_conversation_turn(&turn_id).ok();
        let trace_artifacts = serde_json::json!({
            "turnId": &turn_id,
            "turnStatus": turn_snapshot.as_ref().map(|turn| turn.status.clone()),
            "routeKind": turn_snapshot.as_ref().and_then(|turn| turn.route_kind.clone()),
            "trace": turn_snapshot.as_ref().and_then(|turn| turn.trace.clone()),
        });
        let previous_task_artifacts = db
            .get_agent_task_run(&task_run_id)
            .ok()
            .and_then(|run| run.artifacts);
        let subtask_runs = db
            .list_agent_subtask_runs(&task_run_id)
            .unwrap_or_else(|err| {
                warn!("Failed to load subtask runs for {task_run_id}: {err}");
                Vec::new()
            });
        let task_artifacts =
            build_final_task_artifacts(previous_task_artifacts, trace_artifacts, &subtask_runs);
        let (task_status, task_summary, task_error): (&str, &str, Option<String>) = if timed_out {
            (
                "timed_out",
                "Agent execution timed out",
                Some("Agent execution timed out.".to_string()),
            )
        } else if let Some(Err(CoreError::Cancelled(message))) = &result {
            (
                "cancelled",
                "Agent execution cancelled",
                Some(message.clone()),
            )
        } else if let Some(Err(err)) = &result {
            ("failed", "Agent execution failed", Some(err.to_string()))
        } else {
            match turn_snapshot.as_ref().map(|turn| turn.status.as_str()) {
                Some("cancelled") => ("cancelled", "Stopped by user", None),
                Some("error") => ("failed", "Agent execution failed", None),
                Some("cached") => ("completed", "Answered from cache", None),
                _ => ("completed", "Task completed", None),
            }
        };
        let _ = db.finish_agent_task_run(
            &task_run_id,
            task_status,
            Some(task_summary),
            task_error.as_deref(),
            Some(&task_artifacts),
        );
        record_agent_run_status_task_event(
            &db,
            &handle,
            &conv_id,
            &task_run_id,
            Some(&turn_id),
            &stream_event_seq,
            AgentRunPhase::Done,
            task_summary,
            Some(task_status),
            Some(&task_artifacts),
        );
        emit_agent_task_run_update(&db, &handle, &conv_id, &task_run_id);

        // Repair orphaned tool_calls in DB after timeout or error.
        if !matches!(&result, Some(Ok(_))) {
            repair_orphaned_tool_calls(&db, &conv_id);
        }

        // Auto memory extraction (background, best-effort).
        if matches!(&result, Some(Ok(_))) {
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
                            timeout_secs: Some(app_cfg.llm_timeout_secs),
                        }
                    } else {
                        ProviderConfig {
                            provider_type: provider_type_for_config(&db_config_for_extraction),
                            api_key: Some(db_config_for_extraction.api_key.clone()),
                            base_url: db_config_for_extraction.base_url.clone(),
                            org_id: None,
                            timeout_secs: Some(app_cfg.llm_timeout_secs),
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
                let artifacts = serde_json::json!({
                    "reason": "aborted_after_cancel_timeout"
                });
                let _ = db.finish_agent_task_run(
                    &task_run_id,
                    "cancelled",
                    Some("Stopped by user"),
                    None,
                    Some(&artifacts),
                );
                record_agent_run_status_task_event(
                    &db,
                    &handle,
                    &conv_id,
                    &task_run_id,
                    Some(&turn_id),
                    stream_event_seq.as_ref(),
                    AgentRunPhase::Done,
                    "Stopped by user",
                    Some("cancelled"),
                    Some(&artifacts),
                );
                emit_agent_task_run_update(&db, &handle, &conv_id, &task_run_id);
            }
        });
    }
    Ok(())
}
