use super::*;

pub(crate) fn build_current_turn_time_section() -> String {
    let now = Local::now();
    let utc_now = now.with_timezone(&Utc);
    format!(
        "## Current Turn Time\n\n\
         The current time at the start of this user turn is:\n\
         - Local timestamp: {}\n\
         - UTC timestamp: {}\n\
         - Local date: {}\n\
         - Local time: {}\n\
         - Weekday: {}\n\
         - UTC offset: {}\n\n\
         Use this as the reference point for relative dates such as today, yesterday, tomorrow, last week, and latest. For time-sensitive facts, schedules, prices, laws, releases, or other information that may have changed, prefer fresh retrieval/tool evidence instead of relying only on memory.",
        now.to_rfc3339_opts(SecondsFormat::Secs, false),
        utc_now.to_rfc3339_opts(SecondsFormat::Secs, true),
        now.format("%Y-%m-%d"),
        now.format("%H:%M:%S"),
        now.format("%A"),
        now.format("%:z"),
    )
}

pub(crate) fn provider_type_for_config(config: &DbAgentConfig) -> ProviderType {
    provider_type_for_parts(&config.provider, config.base_url.as_deref())
}

pub(crate) fn desktop_provider_config(config: &DbAgentConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider_type_for_config(config),
        api_key: Some(config.api_key.clone()),
        base_url: config.base_url.as_deref().and_then(|value| {
            let normalized = value.trim().trim_end_matches('/');
            (!normalized.is_empty()).then(|| normalized.to_string())
        }),
        org_id: None,
        timeout_secs: None,
        streaming: config.provider_streaming,
    }
}

pub(crate) fn build_selected_skills_artifact(skills: &[Skill]) -> serde_json::Value {
    serde_json::json!({
        "kind": "selectedSkills",
        "version": 1,
        "skills": skills
            .iter()
            .map(|skill| {
                serde_json::json!({
                    "id": &skill.id,
                    "name": &skill.name,
                    "description": &skill.description,
                    "shortDescription": &skill.interface.short_description,
                    "enabled": skill.enabled,
                    "builtin": skill.builtin,
                    "sourcePath": &skill.source_path,
                    "implicit": skill.policy.allow_implicit_invocation,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// Map a MIME type to a file extension for temp-file parsing.
pub(crate) fn mime_to_extension(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.ms-powerpoint" => "ppt",
        "text/plain" => "txt",
        "text/markdown" | "text/x-markdown" => "md",
        "text/csv" => "csv",
        "text/html" => "html",
        "application/json" => "json",
        "application/epub+zip" => "epub",
        _ if mime.starts_with("text/") => "txt",
        _ => "bin",
    }
}

pub(crate) fn emit_user_content_event(
    app_handle: Option<&AppHandle>,
    event_name: &str,
    payload: &serde_json::Value,
) {
    if let Some(app_handle) = app_handle {
        emit_app_event(app_handle, event_name, payload);
    }
}

pub fn build_desktop_agent_user_content_parts(
    request: DesktopAgentUserContentRequest<'_>,
) -> Result<Vec<ContentPart>, String> {
    let DesktopAgentUserContentRequest {
        db,
        app_handle,
        provider_config,
        db_config,
        message,
        attachments,
    } = request;

    let vision_supported = model_supports_vision(&provider_config.provider_type, &db_config.model);
    info!(
        "Attachment check: provider={}, model={}, provider_type={:?}, vision_supported={}, has_attachments={}",
        db_config.provider,
        db_config.model,
        provider_config.provider_type,
        vision_supported,
        attachments.is_some_and(|items| !items.is_empty())
    );

    let mut user_parts = vec![ContentPart::Text {
        text: message.to_string(),
    }];
    let Some(attachments) = attachments else {
        return Ok(user_parts);
    };

    for attachment in attachments {
        if attachment.media_type.starts_with("image/") {
            if vision_supported {
                user_parts.push(ContentPart::Image {
                    media_type: attachment.media_type.clone(),
                    data: attachment.base64_data.clone(),
                });
            } else {
                warn!(
                    "Model '{}' (provider {:?}) does not support vision. Using OCR fallback for image '{}'.",
                    db_config.model, provider_config.provider_type, attachment.original_name
                );
                emit_user_content_event(
                    app_handle,
                    "image:ocr-fallback",
                    &serde_json::json!({
                        "image_name": attachment.original_name,
                        "model": db_config.model,
                        "reason": "Model does not support native image inputs"
                    }),
                );
                let ocr_config = db.load_ocr_config().unwrap_or_default();
                let image_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&attachment.base64_data)
                    .map_err(|e| format!("Failed to decode image: {e}"))?;
                let ocr_result = extract_text_from_image(
                    &image_bytes,
                    &attachment.media_type,
                    &ocr_config,
                    None,
                );
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
                                attachment.original_name, result.full_text
                            ),
                        });
                    }
                    _ => {
                        warn!(
                            "OCR fallback also failed for image '{}'. Install OCR model or use a vision-capable model.",
                            attachment.original_name
                        );
                        emit_user_content_event(
                            app_handle,
                            "image:ocr-failed",
                            &serde_json::json!({
                                "image_name": attachment.original_name,
                                "model": db_config.model,
                                "hint": "Install OCR model in Settings or switch to a vision-capable model"
                            }),
                        );
                        user_parts.push(ContentPart::Text {
                            text: format!(
                                "[Image \"{}\" attached but could not be processed — this model does not support image inputs and OCR is not available. Install the OCR model in Settings or use a vision-capable model.]",
                                attachment.original_name
                            ),
                        });
                    }
                }
            }
            continue;
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&attachment.base64_data)
            .map_err(|e| format!("Failed to decode attachment: {e}"))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            warn!(
                "Attachment '{}' is too large ({} bytes, limit {}). Skipping.",
                attachment.original_name,
                bytes.len(),
                MAX_ATTACHMENT_BYTES
            );
            user_parts.push(ContentPart::Text {
                text: format!(
                    "[Attached file \"{}\" skipped — file too large ({:.1} MB, limit 10 MB)]",
                    attachment.original_name,
                    bytes.len() as f64 / (1024.0 * 1024.0)
                ),
            });
            continue;
        }

        let ext = mime_to_extension(&attachment.media_type);
        let temp_path =
            std::env::temp_dir().join(format!("nexa-attach-{}.{}", Uuid::new_v4(), ext));
        if let Err(e) = std::fs::write(&temp_path, &bytes) {
            warn!(
                "Failed to write temp file for attachment '{}': {}",
                attachment.original_name, e
            );
            user_parts.push(ContentPart::Text {
                text: format!(
                    "[Attached file \"{}\" — could not process: {}]",
                    attachment.original_name, e
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
                            attachment.original_name
                        ),
                    });
                } else {
                    info!(
                        "Parsed document attachment '{}': {} chars",
                        attachment.original_name,
                        combined_text.len()
                    );
                    user_parts.push(ContentPart::Text {
                        text: format!(
                            "[Attached file: {}]\n\n{}",
                            attachment.original_name, combined_text
                        ),
                    });
                }
            }
            Err(e) => {
                warn!(
                    "Failed to parse attachment '{}': {}",
                    attachment.original_name, e
                );
                user_parts.push(ContentPart::Text {
                    text: format!(
                        "[Attached file \"{}\" — could not extract content: {}]",
                        attachment.original_name, e
                    ),
                });
            }
        }
    }

    Ok(user_parts)
}

pub async fn build_desktop_agent_vision_user_content(
    request: DesktopAgentVisionUserContentRequest<'_>,
) -> Result<DesktopAgentVisionUserContentResult, String> {
    let DesktopAgentVisionUserContentRequest {
        db,
        app_handle,
        provider_config,
        db_config,
        message,
        attachments,
        vision_resolution,
        task_run_id,
        primary_egress_id,
        primary_routes_local,
        primary_native_vision_allowed,
        turn_override,
        cancellation,
        allow_observation_cache,
    } = request;
    let mut parts = vec![ContentPart::Text {
        text: message.to_string(),
    }];
    let Some(input_attachments) = attachments else {
        return Ok(DesktopAgentVisionUserContentResult {
            parts,
            attachments: Vec::new(),
            llm_context_content: message.to_string(),
        });
    };
    let policy = vision_resolution
        .map(|resolution| VisionRouterPolicy::from_binding_options(&resolution.snapshot.options))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let ocr_config = db.load_ocr_config().unwrap_or_default();
    let primary_supports_vision = primary_native_vision_allowed
        && model_declares_vision_support(&provider_config.provider_type, &db_config.model);
    let primary_is_local = primary_routes_local;
    let auxiliary_is_local = vision_resolution
        .is_some_and(|resolution| provider_config_is_local(&resolution.provider_config));
    let mut persisted_attachments = Vec::with_capacity(input_attachments.len());
    let mut llm_context_fragments = vec![message.to_string()];
    let mut non_image_attachments = Vec::new();
    let mut provider_routes: Option<Vec<DesktopVisionProviderRoute>> = None;
    let mut selected_fallback_index = vision_resolution
        .map(|resolution| resolution.snapshot.fallback_index)
        .unwrap_or_default();

    for original in input_attachments {
        if !original.media_type.starts_with("image/") {
            let mut attachment = original.clone();
            attachment.vision_analysis = None;
            non_image_attachments.push(attachment.clone());
            persisted_attachments.push(attachment);
            continue;
        }
        if cancellation.is_cancelled() {
            return Err("Agent execution cancelled during image understanding".to_string());
        }
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode(&original.base64_data)
            .map_err(|error| format!("Failed to decode image: {error}"))?;
        if image_bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "Image attachment '{}' exceeds the {} byte limit",
                original.original_name, MAX_ATTACHMENT_BYTES
            ));
        }
        let computed_hash = attachment_hash(&image_bytes);
        if original
            .attachment_hash
            .as_deref()
            .is_some_and(|provided| !provided.eq_ignore_ascii_case(&computed_hash))
        {
            return Err(format!(
                "Image attachment '{}' changed after preparation",
                original.original_name
            ));
        }
        let attachment_id = original
            .attachment_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let decision = classify_vision_route(VisionClassificationInput {
            original_name: &original.original_name,
            mime_type: &original.media_type,
            user_prompt: message,
            policy: &policy,
            turn_override,
            primary_supports_vision,
            primary_is_local,
            auxiliary_available: vision_resolution.is_some(),
            auxiliary_is_local,
            ocr_available: ocr_config.enabled,
        })
        .map_err(|error| error.to_string())?;
        let target = vision_resolution.map(|resolution| VisionTargetProfile {
            binding_revision: resolution.snapshot.binding_revision,
            target_id: resolution.snapshot.target_id.clone(),
            target_revision: resolution.snapshot.target_revision,
            connection_id: resolution.snapshot.connection_id.clone(),
            connection_revision: resolution.snapshot.connection_revision,
            descriptor_hash: resolution.snapshot.descriptor_hash.clone(),
        });
        let fallback_targets = vision_resolution
            .map(|resolution| {
                resolution
                    .snapshot
                    .fallback_targets
                    .iter()
                    .filter(|candidate| {
                        candidate.fallback_index > resolution.snapshot.fallback_index
                    })
                    .map(|candidate| VisionTargetProfile {
                        binding_revision: resolution.snapshot.binding_revision,
                        target_id: candidate.target_id.clone(),
                        target_revision: candidate.target_revision,
                        connection_id: candidate.connection_id.clone(),
                        connection_revision: candidate.connection_revision,
                        descriptor_hash: candidate.descriptor_hash.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let profile = VisionProfileV1 {
            observation_schema_version: VISION_OBSERVATION_SCHEMA_VERSION,
            classifier_version: VISION_CLASSIFIER_VERSION,
            intent: decision.intent,
            mode: policy.mode,
            turn_override,
            prefer_local_processing: policy.prefer_local_processing,
            local_only: policy.local_only,
            primary_egress_id: primary_egress_id.to_string(),
            primary_is_local,
            fallback_mode: vision_resolution
                .map(|resolution| resolution.snapshot.fallback_mode)
                .unwrap_or_default(),
            constraints: vision_resolution
                .map(|resolution| resolution.snapshot.constraints.clone())
                .unwrap_or_default(),
            ocr: VisionOcrProfile {
                enabled: ocr_config.enabled,
                confidence_threshold_millis: (ocr_config.confidence_threshold * 1_000.0)
                    .round()
                    .clamp(0.0, 1_000.0) as u16,
                det_limit_side_len: ocr_config.det_limit_side_len,
                use_cls: ocr_config.use_cls,
                languages: ocr_config.languages.clone(),
            },
            target,
            fallback_targets,
        };
        let profile_hash = profile.profile_hash().map_err(|error| error.to_string())?;
        let mut attachment = original.clone();
        attachment.attachment_id = Some(attachment_id.clone());
        attachment.attachment_hash = Some(computed_hash.clone());
        attachment.vision_analysis = None;

        match decision.plan {
            nexa_core::vision_router::VisionRoutePlan::NativeDirect => {
                parts.push(ContentPart::Image {
                    media_type: original.media_type.clone(),
                    data: original.base64_data.clone(),
                });
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status: VisionAttachmentStatus::MetadataOnly,
                    profile_hash: Some(profile_hash),
                    observation: None,
                    reason_code: Some("native_direct_unstructured".to_string()),
                });
                llm_context_fragments.push(format!(
                    "[Image: {} — processed directly by the pinned native vision model]",
                    original.original_name
                ));
            }
            nexa_core::vision_router::VisionRoutePlan::MetadataOnly => {
                let metadata = format!(
                    "[Image: {} — image understanding disabled; no pixels were sent to a model]",
                    original.original_name
                );
                parts.push(ContentPart::Text {
                    text: metadata.clone(),
                });
                llm_context_fragments.push(metadata);
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status: VisionAttachmentStatus::MetadataOnly,
                    profile_hash: Some(profile_hash),
                    observation: None,
                    reason_code: Some("vision_disabled".to_string()),
                });
            }
            _ => {
                let now_epoch = Utc::now().timestamp();
                let cached = if allow_observation_cache && policy.cache_enabled {
                    db.get_vision_observation_cache(&computed_hash, &profile_hash, now_epoch)
                        .map_err(|error| error.to_string())?
                } else {
                    None
                };
                let (observation, status) = if let Some(cached) = cached {
                    let mut observation = cached.observation;
                    observation.attachment_id = attachment_id.clone();
                    observation.validate().map_err(|error| error.to_string())?;
                    (observation, VisionAttachmentStatus::Cached)
                } else {
                    let requires_vision = matches!(
                        decision.plan,
                        nexa_core::vision_router::VisionRoutePlan::VisionOnly
                            | nexa_core::vision_router::VisionRoutePlan::OcrThenVision
                            | nexa_core::vision_router::VisionRoutePlan::VisionThenOcr
                    );
                    if requires_vision && provider_routes.is_none() {
                        provider_routes = vision_resolution
                            .map(build_desktop_vision_provider_routes)
                            .transpose()?;
                    }
                    let vision = provider_routes
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .filter(|route| route.fallback_index >= selected_fallback_index)
                        .filter(|route| !policy.local_only || route.local)
                        .map(|route| VisionProviderInput {
                            provider: route.provider.as_ref(),
                            provider_type: route.provider_type,
                            provider_id: &route.provider_id,
                            egress_id: &route.egress_id,
                            model_id: &route.model_id,
                            target_id: &route.target_id,
                            target_revision: route.target_revision,
                            fallback_index: route.fallback_index,
                            local: route.local,
                        })
                        .collect::<Vec<_>>();
                    let observation = execute_vision_observation(VisionExecutionInput {
                        attachment_id: &attachment_id,
                        attachment_hash: &computed_hash,
                        profile_hash: &profile_hash,
                        image_bytes: &image_bytes,
                        mime_type: &original.media_type,
                        decision,
                        ocr_config: &ocr_config,
                        vision: &vision,
                        route_primary_fallback_index: vision_resolution
                            .map(|resolution| resolution.snapshot.fallback_index)
                            .unwrap_or_default(),
                        primary_egress_id,
                        primary_is_local,
                        cancellation,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                    (observation, VisionAttachmentStatus::Observed)
                };
                if let Some(next_fallback_index) = observation
                    .sources
                    .iter()
                    .filter_map(|source| source.fallback_index)
                    .max()
                    .filter(|index| *index > selected_fallback_index)
                {
                    db.advance_task_runtime_fallback(
                        task_run_id,
                        "vision",
                        selected_fallback_index,
                        next_fallback_index,
                        "vision_invocation_failed_automatic_fallback",
                    )
                    .map_err(|error| error.to_string())?;
                    selected_fallback_index = next_fallback_index;
                }
                if allow_observation_cache
                    && policy.cache_enabled
                    && status == VisionAttachmentStatus::Observed
                {
                    let expires_at_epoch =
                        now_epoch + i64::from(policy.cache_retention_days) * 24 * 60 * 60;
                    db.save_vision_observation_cache(&observation, now_epoch, expires_at_epoch)
                        .map_err(|error| error.to_string())?;
                }
                let prompt = observation_prompt_text(&original.original_name, &observation)
                    .map_err(|error| error.to_string())?;
                parts.push(ContentPart::Text {
                    text: prompt.clone(),
                });
                llm_context_fragments.push(prompt);
                attachment.vision_analysis = Some(VisionAttachmentAnalysis {
                    status,
                    profile_hash: Some(profile_hash),
                    observation: Some(observation),
                    reason_code: None,
                });
            }
        }
        persisted_attachments.push(attachment);
    }

    if !non_image_attachments.is_empty() {
        let mut document_parts =
            build_desktop_agent_user_content_parts(DesktopAgentUserContentRequest {
                db,
                app_handle,
                provider_config,
                db_config,
                message: "",
                attachments: Some(&non_image_attachments),
            })?;
        if document_parts
            .first()
            .is_some_and(|part| matches!(part, ContentPart::Text { text } if text.is_empty()))
        {
            document_parts.remove(0);
        }
        for part in &document_parts {
            if let ContentPart::Text { text } = part {
                llm_context_fragments.push(text.clone());
            }
        }
        parts.extend(document_parts);
    }

    Ok(DesktopAgentVisionUserContentResult {
        parts,
        attachments: persisted_attachments,
        llm_context_content: llm_context_fragments
            .into_iter()
            .filter(|fragment| !fragment.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    })
}

pub fn build_desktop_tool_visual_interpreter(
    request: DesktopToolVisualInterpreterRequest,
) -> ToolVisualInterpreter {
    Arc::new(move |visual_request: ToolVisualInterpretationRequest| {
        let request = request.clone();
        Box::pin(async move { interpret_desktop_tool_visuals(request, visual_request).await })
    })
}

async fn interpret_desktop_tool_visuals(
    request: DesktopToolVisualInterpreterRequest,
    visual_request: ToolVisualInterpretationRequest,
) -> ToolVisualObservation {
    let tool_name = visual_request.tool_name;
    let attachments = visual_request
        .attachments
        .into_iter()
        .filter_map(|attachment| {
            let base64_data = attachment.data.get("base64")?.as_str()?.to_string();
            Some(ImageAttachment {
                base64_data,
                media_type: attachment.mime_type,
                original_name: attachment.name,
                attachment_id: None,
                attachment_hash: None,
                vision_analysis: None,
            })
        })
        .collect::<Vec<_>>();
    if attachments.is_empty() {
        return ToolVisualObservation::unavailable(
            "desktop-vision-router",
            "tool_visual_image_missing",
            "The tool did not return a valid current-turn image attachment.",
        );
    }
    let vision_resolution = match request.db.resolve_or_pin_task_runtime_capability(
        &request.registry_scope,
        "vision",
        &request.task_run_id,
    ) {
        Ok(resolution) => resolution,
        Err(error) => {
            warn!("Failed to resolve auxiliary vision for tool '{tool_name}': {error}");
            return ToolVisualObservation::failed(
                "desktop-vision-router",
                "vision_capability_resolution_failed",
                "The configured auxiliary visual capability could not be resolved. The current-turn pixels were not persisted.",
            );
        }
    };
    let result = build_desktop_agent_vision_user_content(DesktopAgentVisionUserContentRequest {
        db: request.db.as_ref(),
        app_handle: None,
        provider_config: &request.provider_config,
        db_config: &request.db_config,
        message: &request.user_prompt,
        attachments: Some(&attachments),
        vision_resolution: vision_resolution.as_ref(),
        task_run_id: &request.task_run_id,
        primary_egress_id: &request.primary_egress_id,
        primary_routes_local: request.primary_routes_local,
        // Core invokes this adapter only for a text-only primary. Keeping
        // this false also prevents accidental image replay into that model.
        primary_native_vision_allowed: false,
        turn_override: request.turn_override,
        cancellation: &request.cancellation,
        allow_observation_cache: false,
    })
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            warn!("Auxiliary visual interpretation failed for tool '{tool_name}': {error}");
            return ToolVisualObservation::failed(
                "desktop-vision-router",
                "vision_processing_failed",
                "The auxiliary Vision Router and OCR fallback could not produce a structured observation. The current-turn pixels were not persisted.",
            );
        }
    };
    let mut reason_code = None;
    let mut observed = false;
    let mut failed = false;
    for analysis in result
        .attachments
        .iter()
        .filter_map(|attachment| attachment.vision_analysis.as_ref())
    {
        observed |= matches!(
            analysis.status,
            VisionAttachmentStatus::Observed | VisionAttachmentStatus::Cached
        );
        failed |= analysis.status == VisionAttachmentStatus::Failed;
        reason_code = reason_code.or_else(|| analysis.reason_code.clone());
    }
    let text = result
        .parts
        .into_iter()
        .skip(1)
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text),
            ContentPart::Image { .. } | ContentPart::ProviderTurn { .. } => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if failed {
        return ToolVisualObservation::failed(
            "desktop-vision-router",
            reason_code.unwrap_or_else(|| "vision_processing_failed".to_string()),
            if text.is_empty() {
                "The auxiliary visual interpreter failed without a usable observation.".to_string()
            } else {
                text
            },
        );
    }
    if observed {
        return ToolVisualObservation::interpreted(
            "desktop-vision-router",
            if text.is_empty() {
                "The auxiliary visual interpreter completed without textual details.".to_string()
            } else {
                text
            },
        );
    }
    ToolVisualObservation::unavailable(
        "desktop-vision-router",
        reason_code.unwrap_or_else(|| "no_image_processor_available".to_string()),
        if text.is_empty() {
            "No configured auxiliary Vision Router or OCR processor could interpret the current-turn pixels. The pixels were not persisted.".to_string()
        } else {
            text
        },
    )
}

fn build_desktop_vision_provider_routes(
    resolution: &RuntimeCapabilityResolution,
) -> Result<Vec<DesktopVisionProviderRoute>, String> {
    let mut routes = vec![DesktopVisionProviderRoute {
        fallback_index: resolution.snapshot.fallback_index,
        target_id: resolution.snapshot.target_id.clone(),
        target_revision: resolution.snapshot.target_revision,
        provider_id: resolution.snapshot.provider_id.clone(),
        egress_id: format!("registry:{}", resolution.snapshot.connection_id),
        model_id: resolution.model_id.clone(),
        local: provider_config_is_local(&resolution.provider_config),
        provider_type: resolution.provider_config.provider_type,
        provider: create_provider(resolution.provider_config.clone()).map_err(|_| {
            "vision_provider_initialization_failed: provider details were redacted".to_string()
        })?,
    }];
    for fallback in &resolution.fallbacks {
        let snapshot = resolution
            .snapshot
            .fallback_targets
            .iter()
            .find(|candidate| candidate.fallback_index == fallback.fallback_index)
            .ok_or_else(|| {
                "vision_fallback_snapshot_missing: frozen fallback metadata is incomplete"
                    .to_string()
            })?;
        routes.push(DesktopVisionProviderRoute {
            fallback_index: fallback.fallback_index,
            target_id: fallback.target_id.clone(),
            target_revision: fallback.target_revision,
            provider_id: snapshot.provider_id.clone(),
            egress_id: format!("registry:{}", fallback.connection_id),
            model_id: fallback.model_id.clone(),
            local: provider_config_is_local(&fallback.provider_config),
            provider_type: fallback.provider_config.provider_type,
            provider: create_provider(fallback.provider_config.clone()).map_err(|_| {
                "vision_fallback_initialization_failed: provider details were redacted".to_string()
            })?,
        });
    }
    routes.sort_by_key(|route| route.fallback_index);
    Ok(routes)
}

pub(crate) fn provider_config_is_local(config: &ProviderConfig) -> bool {
    let Some(base_url) = config.base_url.as_deref() else {
        return matches!(
            config.provider_type,
            ProviderType::Ollama | ProviderType::LmStudio
        );
    };
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let normalized_host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(crate) fn provider_config_egress_id(config: &ProviderConfig) -> String {
    let endpoint = config
        .base_url
        .as_deref()
        .and_then(|base_url| reqwest::Url::parse(base_url).ok())
        .and_then(|url| {
            let host = url.host_str()?.to_ascii_lowercase();
            let port = url.port_or_known_default()?;
            Some(format!("{}://{host}:{port}", url.scheme()))
        })
        .unwrap_or_else(|| "default-endpoint".to_string());
    format!("legacy:{:?}:{endpoint}", config.provider_type)
}
