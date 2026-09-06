use super::*;

/// A delegated route names saved account configuration, never credentials.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(super) struct SubagentRouteArgs {
    #[serde(default)]
    pub(super) agent_config_id: Option<String>,
    #[serde(default)]
    pub(super) provider: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<ReasoningEffort>,
}

pub(super) fn resolve_subagent_route(
    runtime: &DelegationRuntime,
    db: &Database,
    route: &SubagentRouteArgs,
) -> Result<(AgentConfig, ProviderConfig), CoreError> {
    let requested_provider = route.provider.as_deref();
    if runtime.requires_explicit_route
        && route.agent_config_id.is_none()
        && requested_provider.is_none()
    {
        return Err(CoreError::InvalidInput("The parent uses an official subscription runtime. Select an API worker route with agent_config_id from list_subagent_models; subscription credentials cannot be inherited by the API executor.".into()));
    }
    let selected = if let Some(id) = route.agent_config_id.as_deref() {
        Some(db.get_agent_config(id)?)
    } else if let Some(provider) = requested_provider.filter(|provider| {
        runtime.requires_explicit_route
            || *provider != provider_catalog_key(runtime.provider_config.provider_type)
    }) {
        let mut matches = db
            .list_agent_configs()?
            .into_iter()
            .filter(|config| config.provider == provider);
        let selected = matches.next().ok_or_else(|| CoreError::InvalidInput(format!(
            "No saved provider configuration for '{provider}'. Use list_subagent_models to choose an available route."
        )))?;
        if matches.next().is_some() {
            return Err(CoreError::InvalidInput(format!(
                "Provider '{provider}' has multiple accounts or endpoints. Select agent_config_id from list_subagent_models."
            )));
        }
        Some(selected)
    } else {
        None
    };
    let mut config = runtime.base_config.clone();
    let mut provider_config = runtime.provider_config.clone();
    config.provider_type = Some(provider_config.provider_type);
    if let Some(selected) = selected {
        if requested_provider.is_some_and(|provider| provider != selected.provider) {
            return Err(CoreError::InvalidInput(
                "agent_config_id and provider select different routes".into(),
            ));
        }
        if crate::subscription_runtime::SubscriptionRuntimeKind::from_provider(&selected.provider)
            .is_some()
        {
            return Err(CoreError::InvalidInput("This delegated executor requires an API provider configuration. Subscription accounts must use their official runtime and cannot be used as API credentials.".into()));
        }
        provider_config = crate::desktop_agent_session::desktop_provider_config(&selected);
        config.provider_type = Some(provider_config.provider_type);
        config.model = Some(selected.model);
        config.temperature = selected.temperature.map(|value| value as f32);
        config.max_tokens = selected
            .max_tokens
            .and_then(|value| u32::try_from(value).ok());
        config.context_window = selected
            .context_window
            .and_then(|value| u32::try_from(value).ok());
        config.context_window_resolution = None;
        config.reasoning_enabled = selected.reasoning_enabled;
        config.thinking_budget = selected
            .thinking_budget
            .and_then(|value| u32::try_from(value).ok());
        config.reasoning_effort = selected
            .reasoning_effort
            .as_deref()
            .and_then(ReasoningEffort::from_wire);
        config.summarization_model = selected.summarization_model;
        // Auxiliary models are usable only on their own configured route.
        config.summarization_provider_type = selected
            .summarization_provider
            .as_deref()
            .filter(|provider| *provider == selected.provider)
            .map(|_| provider_config.provider_type);
        if selected
            .summarization_provider
            .as_deref()
            .is_some_and(|provider| provider != selected.provider)
        {
            config.summarization_model = None;
        }
        config.native_search_plan = Default::default();
        config.catalog_limits_authoritative = None;
    }
    if let Some(model) = &route.model {
        if config.model.as_deref() != Some(model) {
            // A model override must not carry the former model's token window
            // or reasoning control family into a different model.
            config.context_window = None;
            config.context_window_resolution = None;
            config.max_tokens = None;
            config.reasoning_enabled = None;
            config.reasoning_effort = None;
            config.thinking_budget = None;
            config.native_search_plan = Default::default();
            config.catalog_limits_authoritative = None;
        }
        config.model = Some(model.clone());
    }
    Ok((config, provider_config))
}

pub(super) fn apply_explicit_worker_reasoning(
    config: &mut AgentConfig,
    route: &SubagentRouteArgs,
) -> Result<(), CoreError> {
    let Some(effort) = route.reasoning_effort.as_ref() else {
        return Ok(());
    };
    if let (Some(provider), Some(model)) = (config.provider_type, config.model.as_deref()) {
        if let Some(capabilities) = model_capabilities_from_catalog(provider, model) {
            let supported = match capabilities.reasoning {
                Some(reasoning) if !reasoning.effort_levels.is_empty() => reasoning
                    .effort_levels
                    .iter()
                    .filter_map(|level| ReasoningEffort::from_wire(level))
                    .any(|supported| supported == *effort),
                Some(reasoning) => {
                    *effort == ReasoningEffort::None && reasoning.mode.as_deref() != Some("always")
                }
                None => *effort == ReasoningEffort::None,
            };
            if !supported {
                return Err(CoreError::InvalidInput(format!(
                    "The selected model '{model}' does not support the requested reasoning_effort"
                )));
            }
        }
    }
    config.reasoning_enabled = Some(*effort != ReasoningEffort::None);
    config.reasoning_effort = Some(effort.clone());
    config.thinking_budget = None;
    Ok(())
}

#[async_trait]
impl Tool for SubagentModelsTool {
    fn name(&self) -> &str {
        "list_subagent_models"
    }
    fn description(&self) -> &str {
        "List configured provider routes and known models for delegation. Use a returned agent_config_id, model and optional reasoning_effort in spawn_subagent or each batch task. API parents inherit their route when omitted; subscription parents must explicitly choose an API account. Call once when choosing a different route; reuse the result."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false})
    }
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::SubAgent]
    }
    async fn execute(
        &self,
        context: nexa_core::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let routes: Vec<_> = context.db.list_agent_configs()?.into_iter().map(|config| {
            let supported = crate::subscription_runtime::SubscriptionRuntimeKind::from_provider(&config.provider).is_none();
            let mut models = if supported { nexa_core::provider_catalog::preset_model_ids(&config.provider, config.base_url.as_deref()) } else { Vec::new() };
            if supported && !models.contains(&config.model) { models.push(config.model.clone()); }
            serde_json::json!({
                "agentConfigId": config.id, "name": config.name, "provider": config.provider,
                "defaultModel": config.model, "models": models, "availableForDelegation": supported,
                "reasoningEffort": config.reasoning_effort,
            })
        }).collect();
        let data = serde_json::json!({"routes": routes, "apiParentRouteInheritedWhenOmitted": true, "subscriptionParentRequiresExplicitRoute": true});
        Ok(ToolResult {
            call_id: context.call_id.into(),
            content: data.to_string(),
            is_error: false,
            artifacts: None,
        })
    }
}
