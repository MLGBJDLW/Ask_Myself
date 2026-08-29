use super::*;

fn stable_instruction(
    id: &'static str,
    source: &'static str,
    reason: &'static str,
    trust: ContextTrustLevel,
    priority: i32,
    text: String,
) -> ContextPackItem {
    ContextPackItem::text(
        id,
        ContextItemRole::Instruction,
        source,
        reason,
        trust,
        priority,
        ContextItemStability::StablePrefix,
        text,
    )
}

fn volatile_instruction(
    id: &'static str,
    source: &'static str,
    reason: &'static str,
    trust: ContextTrustLevel,
    priority: i32,
    text: String,
) -> ContextPackItem {
    ContextPackItem::text(
        id,
        ContextItemRole::Instruction,
        source,
        reason,
        trust,
        priority,
        ContextItemStability::VolatileSuffix,
        text,
    )
}

fn source_context_section(
    id: &'static str,
    source: &'static str,
    reason: &'static str,
    priority: i32,
    text: String,
) -> ContextPackItem {
    ContextPackItem::text(
        id,
        ContextItemRole::SourceScope,
        source,
        reason,
        ContextTrustLevel::UserSelected,
        priority,
        ContextItemStability::VolatileSuffix,
        text,
    )
}

fn evidence_context_section(
    id: &'static str,
    source: &'static str,
    reason: &'static str,
    priority: i32,
    text: String,
) -> ContextPackItem {
    ContextPackItem::text(
        id,
        ContextItemRole::Evidence,
        source,
        reason,
        ContextTrustLevel::RetrievedEvidence,
        priority,
        ContextItemStability::VolatileSuffix,
        text,
    )
}

fn memory_context_section(
    id: &'static str,
    source: &'static str,
    reason: &'static str,
    priority: i32,
    text: String,
) -> ContextPackItem {
    ContextPackItem::text(
        id,
        ContextItemRole::Memory,
        source,
        reason,
        ContextTrustLevel::AgentMemory,
        priority,
        ContextItemStability::VolatileSuffix,
        text,
    )
}

pub fn build_desktop_agent_turn_config(
    request: DesktopAgentTurnConfigRequest<'_>,
) -> DesktopAgentTurnConfig {
    let DesktopAgentTurnConfigRequest {
        db,
        conversation,
        turn_id,
        message,
        persona_id,
        explicit_skill_ids,
        db_config,
        app_cfg,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
        custom_orchestration,
    } = request;

    let source_scope_ids = db
        .get_effective_conversation_source_scope(&conversation.id)
        .unwrap_or_default();
    let source_scope_section =
        nexa_core::conversation::build_source_scope_prompt_section(db, &source_scope_ids)
            .unwrap_or_default();
    let collection_context_section =
        nexa_core::conversation::build_collection_context_prompt_section(
            conversation.collection_context.as_ref(),
        );
    let memory_section =
        nexa_core::personalization::build_memory_summary_for_query(db, Some(message))
            .unwrap_or_default();
    let project_memory_section = nexa_core::project_memory::build_project_memory_summary_for_query(
        db,
        conversation.project_id.as_deref(),
        Some(message),
    )
    .unwrap_or_default();
    let project_workspace = conversation.project_id.as_deref().and_then(|project_id| {
        db.get_project_workspace_snapshot(project_id, Some(message))
            .ok()
    });
    let project_instruction_section = project_workspace
        .as_ref()
        .map(nexa_core::project_runtime::build_project_instruction_section)
        .unwrap_or_default();
    let project_evidence_section = project_workspace
        .as_ref()
        .map(nexa_core::project_runtime::build_project_evidence_section)
        .unwrap_or_default();
    let narrative_evidence_section = conversation
        .project_id
        .as_deref()
        .and_then(|project_id| db.build_project_narrative_plan(project_id, message, 8).ok())
        .map(|plan| nexa_core::event_claim_graph::build_narrative_evidence_section(&plan))
        .unwrap_or_default();
    let agent_memory_section =
        nexa_core::evolution::build_agent_procedural_memory_summary_for_query(db, Some(message))
            .unwrap_or_default();
    let memory_injections = if agent_memory_section.is_empty() {
        Vec::new()
    } else {
        db.search_agent_procedural_memories(message, 3)
            .or_else(|_| db.list_agent_procedural_memories(2))
            .unwrap_or_default()
            .into_iter()
            .map(|memory| DesktopMemoryInjectionEffect {
                memory_id: memory.id,
                conversation_id: conversation.id.clone(),
                turn_id: turn_id.to_string(),
                query: message.to_string(),
                confidence: memory.confidence,
            })
            .collect()
    };
    let preference_section =
        nexa_core::personalization::build_preference_summary_for_query(db, Some(message))
            .unwrap_or_default();
    let learned_section = {
        let cfg = db.get_embedder_config().ok();
        let embedding = cfg.and_then(|c| match nexa_core::embed::create_embedder(&c) {
            Ok(embedder) if embedder.dimensions() > 0 => embedder.embed(message).ok(),
            _ => None,
        });
        match embedding {
            Some(vec) if !vec.iter().all(|&v| v == 0.0) => {
                match nexa_core::learning::retrieve_similar_successes(db, &vec, 3) {
                    Ok(hits) => nexa_core::learning::build_learned_successes_section(&hits),
                    Err(_) => String::new(),
                }
            }
            _ => String::new(),
        }
    };
    let scratchpad_section = nexa_core::agent::scratchpad::build_agent_scratchpad_prompt_section(
        db,
        Some(&conversation.id),
    );
    let requested_persona_id = persona_id
        .or(conversation.persona_id.as_deref())
        .unwrap_or("default");
    let persona_profile = match nexa_core::persona::enabled_persona_by_id(db, requested_persona_id)
    {
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
    let persona_update = (conversation.persona_id.as_deref().unwrap_or("default")
        != effective_persona_id)
        .then(|| DesktopPersonaUpdateEffect {
            conversation_id: conversation.id.clone(),
            persona_id: (effective_persona_id != "default")
                .then(|| effective_persona_id.to_string()),
        });
    let persona_default_skill_ids = persona_profile
        .as_ref()
        .map(|persona| persona.default_skill_ids.clone())
        .unwrap_or_default();
    let mut pinned_skill_ids = persona_default_skill_ids;
    for id in explicit_skill_ids {
        let trimmed = id.trim();
        if !trimmed.is_empty() && !pinned_skill_ids.iter().any(|existing| existing == trimmed) {
            pinned_skill_ids.push(trimmed.to_string());
        }
    }
    let persona_section =
        nexa_core::persona::build_persona_prompt_section(persona_profile.as_ref());
    let current_turn_time_section = build_current_turn_time_section();
    let plan_mode_section = if execution_mode.is_plan() {
        nexa_core::agent::plan_mode_prompt_section()
    } else {
        ""
    };
    let provider_type = provider_type_for_config(db_config);
    let context_window_resolution = resolve_desktop_context_window(db_config);
    let catalog_limits_authoritative =
        nexa_core::provider_catalog::endpoint_model_catalog_limits_are_authoritative(
            &db_config.provider,
            db_config.base_url.as_deref(),
            &db_config.model,
        );
    let configured_reasoning_effort =
        db_config
            .reasoning_effort
            .as_ref()
            .and_then(|effort| match effort.as_str() {
                "none" => Some(ReasoningEffort::None),
                "minimal" => Some(ReasoningEffort::Minimal),
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                "max" => Some(ReasoningEffort::Max),
                "xhigh" => Some(ReasoningEffort::XHigh),
                _ => None,
            });
    let goal_section = nexa_core::conversation::goal::build_conversation_goal_prompt_section(
        db,
        &conversation.id,
        !execution_mode.is_plan(),
    );
    // A durable goal can span turns, but it cannot silently erase a tool-round
    // cap the user explicitly saved for each turn.
    let configured_max_iterations = db_config
        .max_iterations
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(u32::MAX);
    let power_policy = resolve_agent_power_policy(AgentPowerPolicyInput {
        mode: power_mode,
        provider_type,
        model: &db_config.model,
        max_iterations: configured_max_iterations,
        reasoning_enabled: db_config.reasoning_enabled,
        thinking_budget: db_config
            .thinking_budget
            .and_then(|value| u32::try_from(value).ok()),
        reasoning_effort: configured_reasoning_effort,
        subagent_max_parallel: db_config
            .subagent_max_parallel
            .and_then(|value| u32::try_from(value).ok()),
        subagent_max_calls_per_turn: db_config
            .subagent_max_calls_per_turn
            .and_then(|value| u32::try_from(value).ok()),
        subagent_token_budget: db_config
            .subagent_token_budget
            .and_then(|value| u32::try_from(value).ok()),
    });
    let power_mode_section = power_policy.prompt_section().to_string();
    let orchestration_policy = resolve_orchestration_profile(OrchestrationProfileInput {
        profile: orchestration_profile,
        custom: custom_orchestration.clone(),
        max_iterations: power_policy.max_iterations,
        max_parallel: power_policy.subagent_max_parallel,
        max_calls_per_turn: power_policy.subagent_max_calls_per_turn,
        delegated_token_budget: power_policy.subagent_token_budget,
        verification_reserve_percent: power_policy.verification_reserve_percent,
    });
    let profile_overrides_persisted_delegation =
        orchestration_profile != OrchestrationProfile::Balanced || power_mode.is_nexus();
    let subagent_max_parallel = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_max_parallel
    } else {
        Some(orchestration_policy.max_parallel)
    };
    let subagent_max_calls_per_turn = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_max_calls_per_turn
    } else {
        Some(orchestration_policy.max_calls_per_turn)
    };
    let subagent_token_budget = if orchestration_profile == OrchestrationProfile::Balanced {
        power_policy.subagent_token_budget
    } else {
        Some(orchestration_policy.delegated_token_budget)
    };
    let had_saved_v2_limits = db_config.delegation_limits_v2.is_some();
    let delegation_limits_v2 = {
        let mut limits = db_config.delegation_limits_v2.clone().unwrap_or_default();
        if profile_overrides_persisted_delegation || !had_saved_v2_limits {
            limits.max_parallel = subagent_max_parallel;
            limits.max_calls_per_turn = subagent_max_calls_per_turn;
            limits.total_actual_tokens_soft_limit = subagent_token_budget.map(u64::from);
        }
        let automatic_worker_cap = limits
            .total_actual_tokens_soft_limit
            .and_then(|total| u32::try_from(total).ok())
            .zip(limits.max_parallel)
            .map(|(total, parallel)| automatic_delegated_worker_cap(total, parallel));
        if limits.max_actual_tokens_per_worker.is_none() {
            limits.max_actual_tokens_per_worker = automatic_worker_cap;
        }
        // Every runtime uses the independent V2 contract. Auto context resolves
        // the selected model's real catalog capacity. Auto per-step output uses
        // a verified catalog ceiling (or safe 8K fallback) and is then bounded
        // by the cumulative worker allocation.
        Some(limits)
    };
    let orchestration_profile_section = orchestration_policy.prompt_section();
    let collaboration_mode_section = if collaboration_mode.is_moa() {
        format!(
            "## Mixture-of-Agents Collaboration\n\nThe user explicitly selected the `{}` virtual-provider preset for this turn. Private tool-free advisors may inform model calls, but only the aggregator may answer or call tools. MoA remains independent from Nexus; do not recursively enable MoA inside delegated workers.",
            moa_preset.as_str()
        )
    } else {
        String::new()
    };
    let conversation_system_prompt = db
        .get_effective_conversation_system_prompt(conversation)
        .unwrap_or_else(|error| {
            warn!(
                "Failed to resolve conversation instruction provenance for {}: {}",
                conversation.id, error
            );
            if conversation.project_id.is_some() {
                String::new()
            } else {
                conversation.system_prompt.clone()
            }
        });
    let base_system_prompt = build_system_prompt(Some(&conversation_system_prompt), &[]);
    let context_budget = context_window_resolution
        .capacity_tokens
        .map(|window| window.saturating_mul(3) / 5);
    let mut context_assembler = ContextAssembler::new("agent_turn", context_budget);
    let context_items = vec![
        stable_instruction(
            "system-instructions",
            "runtime",
            "stable runtime and conversation instructions",
            ContextTrustLevel::System,
            1_000,
            base_system_prompt,
        ),
        volatile_instruction(
            "current-turn-time",
            "runtime.clock",
            "current turn time",
            ContextTrustLevel::System,
            130,
            current_turn_time_section,
        ),
        stable_instruction(
            "project-instructions",
            "project.instructions",
            "live user-maintained project instructions",
            ContextTrustLevel::UserSelected,
            950,
            project_instruction_section,
        ),
        volatile_instruction(
            "execution-mode",
            "runtime.execution_mode",
            "selected execution mode",
            ContextTrustLevel::System,
            120,
            plan_mode_section.to_string(),
        ),
        volatile_instruction(
            "power-policy",
            "runtime.power_policy",
            "resolved power policy",
            ContextTrustLevel::System,
            110,
            power_mode_section,
        ),
        volatile_instruction(
            "quality-policy",
            "runtime.orchestration_profile",
            "resolved orchestration quality profile",
            ContextTrustLevel::System,
            108,
            orchestration_profile_section,
        ),
        volatile_instruction(
            "collaboration-policy",
            "runtime.collaboration_mode",
            "selected LLM collaboration policy",
            ContextTrustLevel::System,
            106,
            collaboration_mode_section,
        ),
        volatile_instruction(
            "active-goal",
            "conversation.goal",
            "active user goal",
            ContextTrustLevel::UserSelected,
            100,
            goal_section,
        ),
        volatile_instruction(
            "persona",
            "persona",
            "selected persona",
            ContextTrustLevel::UserSelected,
            90,
            persona_section,
        ),
        source_context_section(
            "collection-context",
            "conversation.collection",
            "selected collection context",
            80,
            collection_context_section,
        ),
        source_context_section(
            "source-scope",
            "conversation.sources",
            "effective source scope",
            70,
            source_scope_section,
        ),
        memory_context_section(
            "user-memory",
            "memory.user",
            "query-relevant user memory",
            60,
            memory_section,
        ),
        memory_context_section(
            "project-memory",
            "memory.project",
            "query-relevant project memory",
            50,
            project_memory_section,
        ),
        evidence_context_section(
            "project-workspace-evidence",
            "project.workspace",
            "query-relevant project episodes and observed events",
            55,
            project_evidence_section,
        ),
        evidence_context_section(
            "event-claim-narrative",
            "knowledge.event_claim_graph",
            "query-classified event and claim narrative with review state",
            54,
            narrative_evidence_section,
        ),
        memory_context_section(
            "procedural-memory",
            "memory.procedural",
            "query-relevant procedural memory",
            40,
            agent_memory_section,
        ),
        memory_context_section(
            "preferences",
            "memory.preferences",
            "query-relevant preferences",
            30,
            preference_section,
        ),
        memory_context_section(
            "learned-successes",
            "memory.learned_successes",
            "similar successful trajectories",
            20,
            learned_section,
        ),
        memory_context_section(
            "scratchpad",
            "memory.scratchpad",
            "conversation scratchpad",
            10,
            scratchpad_section,
        ),
    ];
    for item in context_items {
        context_assembler
            .add(item)
            .expect("desktop context contributors use stable unique ids");
    }
    let context_pack = context_assembler.assemble();
    let system_prompt = context_pack
        .prompt_sections_for_stability(ContextItemStability::StablePrefix)
        .join("\n\n");
    let volatile_system_sections =
        context_pack.prompt_sections_for_stability(ContextItemStability::VolatileSuffix);

    let executor_config = AgentConfig {
        max_iterations: orchestration_policy.max_iterations,
        system_prompt,
        volatile_system_sections,
        model: Some(db_config.model.clone()),
        temperature: db_config.temperature.map(|t| t as f32),
        max_tokens: db_config
            .max_tokens
            .and_then(|value| u32::try_from(value).ok()),
        max_actual_tokens_per_run: None,
        context_window: db_config
            .context_window
            .and_then(|value| u32::try_from(value).ok()),
        context_window_resolution: Some(context_window_resolution),
        catalog_limits_authoritative: Some(catalog_limits_authoritative),
        reasoning_enabled: power_policy.reasoning_enabled,
        thinking_budget: power_policy.thinking_budget,
        reasoning_effort: power_policy.reasoning_effort,
        provider_type: Some(provider_type),
        native_search_plan: nexa_core::llm::native_search::NativeSearchPlan::resolve(
            app_cfg.web_search.execution_mode,
            provider_type,
            db_config.base_url.as_deref(),
            &db_config.model,
        ),
        request_kind: AgentRequestKind::MainAgentStep,
        summarization_model: db_config.summarization_model.clone(),
        summarization_provider_type: db_config
            .summarization_provider
            .as_deref()
            .map(|provider| provider_type_for_parts(provider, None)),
        subagent_max_parallel,
        subagent_max_calls_per_turn,
        subagent_token_budget,
        subagent_verification_reserve_percent: if orchestration_profile
            == OrchestrationProfile::Balanced
        {
            power_policy.verification_reserve_percent
        } else {
            Some(orchestration_policy.verification_reserve_percent)
        },
        delegation_limits_v2,
        // Preserve the saved liveness policy. `0` remains an explicit user
        // choice for unlimited execution; missing or invalid values fall back
        // to the core runtime defaults instead of silently forcing unlimited.
        tool_timeout_secs: db_config
            .tool_timeout_secs
            .and_then(|value| u32::try_from(value).ok()),
        agent_timeout_secs: db_config
            .agent_timeout_secs
            .and_then(|value| u32::try_from(value).ok()),
        cache_ttl_hours: Some(app_cfg.cache_ttl_hours),
        dynamic_tool_visibility: app_cfg.dynamic_tool_visibility,
        trace_enabled: app_cfg.trace_enabled,
        require_tool_confirmation: app_cfg.confirm_destructive,
        shell_access_mode: app_cfg.shell_access_mode,
        tool_approval_mode: app_cfg.tool_approval_mode,
        execution_mode,
        power_mode,
        collaboration_mode,
        moa_preset,
        orchestration_profile,
        custom_orchestration,
    };

    DesktopAgentTurnConfig {
        executor_config,
        context_window_resolution,
        source_scope_ids,
        pinned_skill_ids,
        context_pack,
        effects: DesktopAgentTurnConfigEffects {
            memory_injections,
            persona_update,
        },
    }
}

pub fn apply_desktop_agent_turn_config_effects(
    db: &Database,
    effects: &DesktopAgentTurnConfigEffects,
) {
    for effect in &effects.memory_injections {
        if let Err(error) = db.record_memory_injection_event(
            &effect.memory_id,
            Some(&effect.conversation_id),
            Some(&effect.turn_id),
            &effect.query,
            "agent_procedural_memory_prompt",
            Some(effect.confidence),
        ) {
            warn!("Failed to record procedural memory injection: {error}");
        }
    }
    if let Some(effect) = &effects.persona_update {
        if let Err(error) =
            db.update_conversation_persona(&effect.conversation_id, effect.persona_id.as_deref())
        {
            warn!("Failed to persist the resolved conversation persona: {error}");
        }
    }
}

pub fn build_desktop_agent_session_config(
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
        thinking_budget: input
            .db_config
            .thinking_budget
            .and_then(|value| u32::try_from(value).ok()),
        reasoning_effort: input.db_config.reasoning_effort.clone(),
        source_scope: nexa_core::runtime::RuntimeSourceScope {
            source_ids: input.source_scope_ids.to_vec(),
            collection_id: None,
            working_directory: None,
        },
        approval_mode: input.app_cfg.tool_approval_mode,
        shell_access_mode: input.app_cfg.shell_access_mode,
        execution_mode: input.execution_mode,
        collaboration_mode: input.collaboration_mode,
        moa_preset: input.moa_preset,
        orchestration_profile: input.orchestration_profile,
        custom_orchestration: input.custom_orchestration.clone(),
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
        package_context: desktop_runtime_package_context(input.db),
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

pub fn desktop_runtime_package_context(db: &Database) -> nexa_core::runtime::RuntimePackageContext {
    nexa_core::package_host::database_backed_builtin_runtime_package_context(db)
        .expect("database-backed builtin Package Host snapshot is valid")
}

pub fn filter_desktop_tool_names_by_package_host(
    db: &Database,
    names: Vec<String>,
) -> Result<Vec<String>, String> {
    PackageRuntimeAssembler::database_builtin(db)
        .and_then(|assembler| assembler.visible_tool_names(names))
        .map_err(|error| error.to_string())
}

pub fn runtime_session_config_artifact(
    config: &nexa_core::runtime::AgentSessionConfig,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "agentSessionConfig",
        "version": 1,
        "config": config,
    })
}

pub fn build_desktop_agent_initial_task_artifacts(
    selected_skills: &[Skill],
    runtime_session_config: &nexa_core::runtime::AgentSessionConfig,
    context_pack: &ContextPack,
    execution_mode: AgentExecutionMode,
    executor_config: &AgentConfig,
) -> serde_json::Value {
    let mut artifacts = serde_json::json!({
        "kind": "agentTaskArtifacts",
        "version": 1,
        "selectedSkills": build_selected_skills_artifact(selected_skills),
        "runtimeSession": runtime_session_config_artifact(runtime_session_config),
        "contextPack": context_pack,
    });
    if execution_mode.is_plan() {
        artifacts["executionMode"] = execution_mode_artifact(execution_mode);
    }
    if executor_config.power_mode.is_nexus() {
        artifacts["powerMode"] = power_mode_artifact(executor_config);
    }
    if executor_config.collaboration_mode.is_moa() {
        artifacts["collaborationMode"] = collaboration_mode_artifact(executor_config);
    }
    artifacts["orchestrationProfile"] = orchestration_profile_artifact(executor_config);
    artifacts
}
