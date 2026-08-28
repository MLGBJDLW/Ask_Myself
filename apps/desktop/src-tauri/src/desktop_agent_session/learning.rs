use super::*;

pub fn resolve_desktop_summarization_provider_config(
    database: &Database,
    db_config: &DbAgentConfig,
) -> Result<Option<(ProviderConfig, String, String)>, String> {
    let Some(provider_name) = db_config
        .summarization_provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    else {
        return Ok(None);
    };

    let requested_type = provider_type_for_parts(provider_name, None);
    if requested_type == provider_type_for_config(db_config) {
        return Ok(Some((
            desktop_provider_config(db_config),
            db_config.name.clone(),
            db_config
                .summarization_model
                .clone()
                .unwrap_or_else(|| db_config.model.clone()),
        )));
    }

    let mut candidates = database
        .list_agent_configs()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|candidate| provider_type_for_config(candidate) == requested_type)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(format!(
            "Summarization provider '{provider_name}' needs its own saved provider configuration; main-agent credentials are never reused across providers"
        ));
    }
    if let Some(model) = db_config
        .summarization_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let model_matches = candidates
            .iter()
            .filter(|candidate| candidate.model == model)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        if model_matches.len() == 1 {
            candidates.retain(|candidate| candidate.id == model_matches[0]);
        }
    }
    if candidates.len() > 1 {
        let defaults = candidates
            .iter()
            .filter(|candidate| candidate.is_default)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        if defaults.len() == 1 {
            candidates.retain(|candidate| candidate.id == defaults[0]);
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "Summarization provider '{provider_name}' matches multiple saved configurations; select a unique summarization model or keep summarization on the main provider"
        ));
    }
    let selected = candidates.pop().expect("one summarization candidate");
    let summary_model = db_config
        .summarization_model
        .clone()
        .unwrap_or_else(|| selected.model.clone());
    Ok(Some((
        desktop_provider_config(&selected),
        selected.name,
        summary_model,
    )))
}

pub fn desktop_memory_extraction_model(db_config: &DbAgentConfig) -> &str {
    db_config
        .summarization_model
        .as_deref()
        .unwrap_or(&db_config.model)
}

pub fn desktop_memory_extraction_provider_config(db_config: &DbAgentConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: provider_type_for_config(db_config),
        api_key: Some(db_config.api_key.clone()),
        base_url: db_config.base_url.clone(),
        org_id: None,
        timeout_secs: None,
        streaming: db_config.provider_streaming,
    }
}

pub async fn run_desktop_agent_post_success_learning(
    request: DesktopAgentPostSuccessLearningRequest,
) {
    let DesktopAgentPostSuccessLearningRequest {
        db,
        conversation_id,
        db_config,
    } = request;

    let app_cfg = db.load_app_config().unwrap_or_default();
    if app_cfg.auto_memory_extraction {
        let extraction_route = match resolve_desktop_summarization_provider_config(&db, &db_config)
        {
            Ok(Some((config, _, model))) => Some((config, model)),
            Ok(None) => Some((
                desktop_memory_extraction_provider_config(&db_config),
                desktop_memory_extraction_model(&db_config).to_string(),
            )),
            Err(error) => {
                warn!("Auto memory extraction skipped for {conversation_id}: {error}");
                None
            }
        };
        if let Some((extract_provider_config, extract_model)) = extraction_route {
            let extract_provider_type = extract_provider_config.provider_type;
            match create_provider(extract_provider_config) {
                Ok(extract_llm) => {
                    match nexa_core::personalization::auto_extract_and_save(
                        &db,
                        &conversation_id,
                        extract_llm.as_ref(),
                        &extract_model,
                        Some(extract_provider_type),
                    )
                    .await
                    {
                        Ok(n) if n > 0 => {
                            info!(
                                "Auto-extracted {n} memories from conversation {conversation_id}"
                            );
                        }
                        Err(error) => {
                            warn!("Auto memory extraction failed for {conversation_id}: {error}");
                        }
                        _ => {}
                    }
                }
                Err(error) => {
                    warn!("Auto memory extraction provider failed for {conversation_id}: {error}");
                }
            }
        }
    }

    if app_cfg.auto_skill_learning {
        match nexa_core::evolution::review_recent_traces_for_evolution(&db, 5) {
            Ok(review) if review.events_created > 0 => {
                info!(
                    "Agent evolution review created {} event(s) for conversation {}",
                    review.events_created, conversation_id
                );
            }
            Err(e) => warn!("Agent evolution review failed for {conversation_id}: {e}"),
            _ => {}
        }
    }

    if app_cfg.dreaming.enabled && app_cfg.dreaming.after_successful_turn {
        if desktop_background_dream_budget_available(&db, &app_cfg) {
            match db.start_dream_run(nexa_core::dreaming::StartDreamInput {
                trigger_kind: Some("after_turn".to_string()),
                scope_json: Some(nexa_core::dreaming_scope::merge_configured_dream_scope(
                    &app_cfg.dreaming,
                    serde_json::json!({
                        "conversationId": conversation_id,
                        "surface": "desktop_agent_post_success_learning"
                    }),
                )),
                max_artifacts: Some(app_cfg.dreaming.max_artifacts_per_run),
            }) {
                Ok(run) => {
                    info!(
                        "Dreaming consolidation run {} completed after successful conversation {}",
                        run.id, conversation_id
                    );
                }
                Err(e) => warn!("Dreaming consolidation failed for {conversation_id}: {e}"),
            }
        } else {
            info!("Dreaming consolidation skipped for {conversation_id}: daily background budget reached");
        }
    }
}

pub(crate) fn desktop_background_dream_budget_available(
    db: &Database,
    app_cfg: &AppConfig,
) -> bool {
    let max_runs = app_cfg.dreaming.max_runs_per_day;
    if max_runs == 0 {
        return false;
    }
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let Ok(runs) = db.list_dream_runs(200) else {
        return false;
    };
    let used = runs
        .iter()
        .filter(|run| run.trigger_kind != "manual" && run.created_at.starts_with(&today))
        .count();
    used < max_runs
}
