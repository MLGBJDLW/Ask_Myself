//! Provider-aware prompt layout decisions for model requests.

use super::*;
const MAX_CACHE_STABLE_TOOL_DEFINITIONS: usize = 24;
const MAX_CACHE_STABLE_TOOL_TOKENS: u32 = 12_000;
const CACHE_STABLE_TOOL_BUDGET_DIVISOR: u32 = 4;
const RESIDENT_DISCOVERY_TOOL_NAME: &str = "tool_search";
const CACHE_STABLE_RESIDENT_TOOL_NAMES: &[&str] = &[
    "activity_observe",
    "browser_evidence_capture",
    "browser_session",
    "code_intelligence",
    "computer_control",
    "computer_observe",
    "create_file",
    "edit_file",
    "glob_files",
    "grep_files",
    "list_dir",
    "manage_skill",
    "multi_edit",
    "read_file",
    "read_files",
    "request_user_input",
    "run_shell",
    "search_files",
    RESIDENT_DISCOVERY_TOOL_NAME,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheStableToolSurfaceMode {
    FullPinned,
    StableResidentDynamic,
}

#[derive(Debug, Clone)]
pub(super) struct CacheStableToolSurface {
    pub(super) mode: CacheStableToolSurfaceMode,
    pub(super) definitions: Vec<ToolDefinition>,
}

impl CacheStableToolSurface {
    pub(super) fn uses_dynamic_discovery(&self) -> bool {
        self.mode == CacheStableToolSurfaceMode::StableResidentDynamic
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromptLayout {
    pub(super) include_skill_system_prompt: bool,
    pub(super) include_turn_scaffolding_system_prompts: bool,
    pub(super) allow_dynamic_tool_visibility: bool,
    pub(super) append_volatile_system_prompt_to_tail: bool,
}

impl PromptLayout {
    #[cfg(test)]
    pub(super) fn for_provider(provider_type: Option<ProviderType>) -> Self {
        Self::for_request(provider_type, None)
    }

    pub(super) fn for_request(provider_type: Option<ProviderType>, model: Option<&str>) -> Self {
        if uses_stable_prefix_cache(provider_type, model) {
            return Self {
                include_skill_system_prompt: true,
                include_turn_scaffolding_system_prompts: true,
                // Prefix-cached providers serialize tool definitions into the
                // reusable request prefix. Route-specific filtering or
                // activating tools after `tool_search` changes that prefix and
                // can invalidate the entire warm cache. Keep the complete,
                // deterministically sorted registry pinned for the request
                // loop; volatile route/skill state remains append-only below.
                allow_dynamic_tool_visibility: false,
                append_volatile_system_prompt_to_tail: true,
            };
        }

        Self {
            include_skill_system_prompt: true,
            include_turn_scaffolding_system_prompts: true,
            allow_dynamic_tool_visibility: true,
            append_volatile_system_prompt_to_tail: false,
        }
    }

    pub(super) fn effective_dynamic_tool_visibility(self, configured: bool) -> bool {
        configured && self.allow_dynamic_tool_visibility
    }
}

pub(super) fn cache_stable_tool_surface_limits(
    _model: &str,
    context_window: Option<u32>,
    max_response_tokens: u32,
) -> (usize, u32) {
    let context_relative_tool_tokens = context_window
        .map(|max_context| {
            max_context
                .saturating_sub(max_response_tokens)
                .saturating_sub(context_safety_buffer(max_context))
                / CACHE_STABLE_TOOL_BUDGET_DIVISOR
        })
        // Provider-managed capacity is unknown, not a synthetic 32K window.
        // Definition count remains bounded independently below.
        .unwrap_or(MAX_CACHE_STABLE_TOOL_TOKENS);
    (
        MAX_CACHE_STABLE_TOOL_DEFINITIONS,
        context_relative_tool_tokens.min(MAX_CACHE_STABLE_TOOL_TOKENS),
    )
}

pub(super) fn tool_surface_fits_cache_stable_limits(
    model: &str,
    definitions: &[ToolDefinition],
    max_definitions: usize,
    max_tool_tokens: u32,
) -> bool {
    definitions.len() <= max_definitions
        && context::estimate_tool_tokens_for_model(model, definitions) <= max_tool_tokens
}

/// Choose a query-independent tool surface for providers whose prompt caches
/// require an exact, stable prefix. The complete registry is preferred, but it
/// is never allowed to consume an unbounded share of the request context.
pub(super) fn select_cache_stable_tool_surface(
    registry: &ToolRegistry,
    model: &str,
    context_window: Option<u32>,
    max_response_tokens: u32,
) -> Result<CacheStableToolSurface, CoreError> {
    let (max_definitions, max_tool_tokens) =
        cache_stable_tool_surface_limits(model, context_window, max_response_tokens);
    let full = registry.definitions();
    if tool_surface_fits_cache_stable_limits(model, &full, max_definitions, max_tool_tokens) {
        return Ok(CacheStableToolSurface {
            mode: CacheStableToolSurfaceMode::FullPinned,
            definitions: full,
        });
    }

    let resident_names = CACHE_STABLE_RESIDENT_TOOL_NAMES
        .iter()
        .filter(|name| registry.contains(name))
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    let resident = registry.filtered(&resident_names).definitions();
    let has_discovery = resident
        .iter()
        .any(|definition| definition.name == RESIDENT_DISCOVERY_TOOL_NAME);
    if has_discovery
        && tool_surface_fits_cache_stable_limits(model, &resident, max_definitions, max_tool_tokens)
    {
        return Ok(CacheStableToolSurface {
            mode: CacheStableToolSurfaceMode::StableResidentDynamic,
            definitions: resident,
        });
    }

    let discovery_names = vec![RESIDENT_DISCOVERY_TOOL_NAME.to_string()];
    let discovery = registry.filtered(&discovery_names).definitions();
    if !discovery.is_empty()
        && tool_surface_fits_cache_stable_limits(
            model,
            &discovery,
            max_definitions,
            max_tool_tokens,
        )
    {
        return Ok(CacheStableToolSurface {
            mode: CacheStableToolSurfaceMode::StableResidentDynamic,
            definitions: discovery,
        });
    }

    Err(CoreError::InvalidInput(format!(
        "The enabled tool registry exceeds the model prompt budget, and the resident tool_search surface cannot fit safely (limit: {max_definitions} tools / {max_tool_tokens} estimated tokens)."
    )))
}

fn uses_stable_prefix_cache(provider_type: Option<ProviderType>, model: Option<&str>) -> bool {
    let is_alibaba_qwen = provider_type == Some(ProviderType::AlibabaModelStudio)
        && model.is_some_and(|model| {
            let model_lower = model.to_ascii_lowercase();
            model_lower.starts_with("qwen") || model_lower.starts_with("qwq")
        });
    matches!(
        provider_type,
        Some(
            ProviderType::Anthropic
                | ProviderType::OpenAi
                | ProviderType::AzureOpenAi
                | ProviderType::Qwen
                | ProviderType::OpenRouter
                | ProviderType::DeepSeek
                | ProviderType::Zhipu
        )
    ) || is_alibaba_qwen
        || model
            .map(|model| {
                let model = model.to_ascii_lowercase();
                model.contains("deepseek") || model.contains("glm-")
            })
            .unwrap_or(false)
}

pub(super) fn turn_scaffolding_sections(
    route_prompt_section: &str,
    task_plan: Option<&AgentTaskPlan>,
    include_dynamic_tool_discovery: bool,
    layout: PromptLayout,
) -> Vec<String> {
    if !layout.include_turn_scaffolding_system_prompts {
        return Vec::new();
    }

    let mut sections = Vec::new();
    if !route_prompt_section.trim().is_empty() {
        sections.push(route_prompt_section.to_string());
    }

    if let Some(task_plan) = task_plan {
        sections.push(task_plan.to_prompt_section());
    }
    if include_dynamic_tool_discovery {
        sections.push(tool_discovery::dynamic_tool_visibility_prompt().to_string());
    }
    sections
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    use crate::intelligence::{build_task_plan, TaskPlanningInput};
    use crate::tools::{tool_search_tool::ToolSearchTool, Tool, ToolResult};

    struct OversizedMcpTool;

    #[async_trait]
    impl Tool for OversizedMcpTool {
        fn name(&self) -> &str {
            "mcp__cache_test__oversized"
        }

        fn description(&self) -> &str {
            "Oversized schema used to exercise cache-stable surface fallback."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "payload": {
                        "type": "string",
                        "description": "cache schema field description ".repeat(3_000)
                    }
                }
            })
        }

        fn categories(&self) -> &'static [ToolCategory] {
            &[ToolCategory::Mcp]
        }

        async fn execute(
            &self,
            context: crate::tools::ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            let crate::tools::ToolExecutionContext {
                call_id,
                arguments: _arguments,
                db: _db,
                source_scope: _source_scope,
                ..
            } = context;
            Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ok".to_string(),
                is_error: false,
                artifacts: None,
            })
        }
    }

    fn plan() -> AgentTaskPlan {
        build_task_plan(TaskPlanningInput::for_route(
            "audit cache behavior",
            "CodebaseOperation",
            false,
            0,
        ))
    }

    #[test]
    fn deepseek_layout_keeps_replayable_tail_context() {
        let layout = PromptLayout::for_provider(Some(ProviderType::DeepSeek));

        assert!(layout.include_skill_system_prompt);
        assert!(layout.include_turn_scaffolding_system_prompts);
        assert!(!layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
        assert!(!layout.effective_dynamic_tool_visibility(true));
    }

    #[test]
    fn exact_prefix_cache_layout_keeps_replayable_tail_context() {
        for provider_type in [
            ProviderType::Anthropic,
            ProviderType::OpenAi,
            ProviderType::AzureOpenAi,
            ProviderType::Qwen,
            ProviderType::OpenRouter,
        ] {
            let layout = PromptLayout::for_provider(Some(provider_type));

            assert!(layout.include_skill_system_prompt);
            assert!(layout.include_turn_scaffolding_system_prompts);
            assert!(!layout.allow_dynamic_tool_visibility);
            assert!(layout.append_volatile_system_prompt_to_tail);
            assert!(!layout.effective_dynamic_tool_visibility(true));
        }
    }

    #[test]
    fn default_layout_keeps_turn_scaffolding_for_custom_providers() {
        let layout = PromptLayout::for_provider(Some(ProviderType::Custom));

        assert!(layout.include_skill_system_prompt);
        assert!(layout.include_turn_scaffolding_system_prompts);
        assert!(layout.allow_dynamic_tool_visibility);
        assert!(!layout.append_volatile_system_prompt_to_tail);
        assert!(layout.effective_dynamic_tool_visibility(true));
    }

    #[test]
    fn deepseek_model_name_uses_prefix_cache_layout_for_compatible_routes() {
        let layout = PromptLayout::for_request(Some(ProviderType::OpenRouter), Some("deepseek/v3"));

        assert!(layout.include_skill_system_prompt);
        assert!(!layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
    }

    #[test]
    fn alibaba_qwen_models_keep_prefix_cache_without_affecting_router_models() {
        let qwen =
            PromptLayout::for_request(Some(ProviderType::AlibabaModelStudio), Some("qwen3.7-max"));
        let third_party = PromptLayout::for_request(
            Some(ProviderType::AlibabaModelStudio),
            Some("kimi-k2.7-code"),
        );

        assert!(!qwen.allow_dynamic_tool_visibility);
        assert!(qwen.append_volatile_system_prompt_to_tail);
        assert!(third_party.allow_dynamic_tool_visibility);
        assert!(!third_party.append_volatile_system_prompt_to_tail);
    }

    #[test]
    fn deepseek_scaffolding_sections_are_controller_state() {
        let sections = turn_scaffolding_sections(
            "## Active Routing Plan\nroute",
            Some(&plan()),
            true,
            PromptLayout::for_provider(Some(ProviderType::DeepSeek)),
        );

        assert_eq!(sections.len(), 3);
        assert!(sections[0].contains("Active Routing Plan"));
        assert!(sections[1].contains("Active Task Plan"));
        assert!(sections[2].contains("Dynamic Tool Discovery"));
    }

    #[test]
    fn default_scaffolding_sections_are_controller_state() {
        let sections = turn_scaffolding_sections(
            "## Active Routing Plan\nroute",
            Some(&plan()),
            true,
            PromptLayout::for_provider(Some(ProviderType::Custom)),
        );

        assert_eq!(sections.len(), 3);
        assert!(sections[0].contains("Active Routing Plan"));
        assert!(sections[1].contains("Active Task Plan"));
        assert!(sections[2].contains("Dynamic Tool Discovery"));
    }

    #[test]
    fn turn_scaffolding_sections_can_be_disabled_by_layout() {
        let mut layout = PromptLayout::for_provider(Some(ProviderType::Custom));
        layout.include_turn_scaffolding_system_prompts = false;

        let sections =
            turn_scaffolding_sections("## Active Routing Plan\nroute", Some(&plan()), true, layout);

        assert!(sections.is_empty());
    }

    #[test]
    fn default_execution_can_omit_model_facing_task_plan_scaffolding() {
        let sections = turn_scaffolding_sections(
            "## Active Routing Plan\nroute",
            None,
            true,
            PromptLayout::for_provider(Some(ProviderType::Custom)),
        );

        assert_eq!(sections.len(), 2);
        assert!(sections[0].contains("Active Routing Plan"));
        assert!(sections[1].contains("Dynamic Tool Discovery"));
        assert!(!sections
            .iter()
            .any(|section| section.contains("Active Task Plan")));
    }

    #[test]
    fn cache_stable_surface_pins_a_registry_that_fits_the_budget() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolSearchTool));

        let surface =
            select_cache_stable_tool_surface(&registry, "deepseek-chat", Some(128_000), 4_096)
                .expect("small registry should fit");

        assert_eq!(surface.mode, CacheStableToolSurfaceMode::FullPinned);
        assert_eq!(surface.definitions.len(), 1);
        assert_eq!(surface.definitions[0].name, "tool_search");
    }

    #[test]
    fn provider_managed_context_does_not_invent_a_small_tool_budget() {
        let (max_definitions, max_tool_tokens) =
            cache_stable_tool_surface_limits("private-model", None, 4_096);
        assert_eq!(max_definitions, MAX_CACHE_STABLE_TOOL_DEFINITIONS);
        assert_eq!(max_tool_tokens, MAX_CACHE_STABLE_TOOL_TOKENS);
    }

    #[test]
    fn cache_stable_surface_falls_back_to_query_independent_core_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolSearchTool));
        registry.register(Box::new(OversizedMcpTool));

        let surface =
            select_cache_stable_tool_surface(&registry, "deepseek-chat", Some(8_192), 4_096)
                .expect("resident discovery surface should fit");

        assert_eq!(
            surface.mode,
            CacheStableToolSurfaceMode::StableResidentDynamic
        );
        assert_eq!(
            surface
                .definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tool_search"]
        );
    }

    #[test]
    fn huge_context_does_not_justify_pinning_a_distracting_tool_surface() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ToolSearchTool));
        registry.register(Box::new(OversizedMcpTool));

        let surface =
            select_cache_stable_tool_surface(&registry, "deepseek-v4-pro", Some(1_000_000), 8_192)
                .expect("resident discovery should fit independently of model context size");

        assert_eq!(
            surface.mode,
            CacheStableToolSurfaceMode::StableResidentDynamic,
            "large context capacity must not turn tool-schema bloat into the default prompt"
        );
        assert_eq!(
            surface
                .definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tool_search"]
        );
    }

    #[test]
    fn default_registry_uses_a_small_stable_coding_surface() {
        let registry = crate::tools::default_tool_registry();
        let surface =
            select_cache_stable_tool_surface(&registry, "deepseek-v4-pro", Some(1_000_000), 16_384)
                .expect("the stable resident coding surface should fit");
        let names = surface
            .definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            surface.mode,
            CacheStableToolSurfaceMode::StableResidentDynamic
        );
        for required in [
            "tool_search",
            "read_file",
            "create_file",
            "edit_file",
            "run_shell",
            "manage_skill",
        ] {
            assert!(
                names.contains(&required),
                "missing resident tool {required}"
            );
        }
        assert!(surface.definitions.len() <= MAX_CACHE_STABLE_TOOL_DEFINITIONS);
        assert!(
            context::estimate_tool_tokens_for_model("deepseek-v4-pro", &surface.definitions)
                <= MAX_CACHE_STABLE_TOOL_TOKENS
        );
    }

    #[test]
    fn deepseek_and_glm_cache_stable_surfaces_include_platform_available_interaction_tools() {
        let registry = crate::tools::default_tool_registry();
        for (provider, model) in [
            (ProviderType::DeepSeek, "deepseek-chat"),
            (ProviderType::Zhipu, "glm-5.3"),
        ] {
            let layout = PromptLayout::for_request(Some(provider), Some(model));
            assert!(
                !layout.allow_dynamic_tool_visibility,
                "{provider:?}/{model} must use the production cache-stable surface"
            );
            let surface =
                select_cache_stable_tool_surface(&registry, model, Some(1_000_000), 16_384)
                    .expect("interaction-capable stable surface should fit");
            let names = surface
                .definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>();
            for required in ["run_shell", "browser_evidence_capture", "browser_session"] {
                assert!(
                    names.contains(&required),
                    "{provider:?}/{model} is missing first-request tool {required}"
                );
            }

            #[cfg(target_os = "windows")]
            for required in ["computer_observe", "computer_control"] {
                assert!(
                    registry.contains(required),
                    "the Windows default registry is missing executable tool {required}"
                );
                assert!(
                    names.contains(&required),
                    "{provider:?}/{model} is missing first-request tool {required}"
                );
            }

            #[cfg(not(target_os = "windows"))]
            for unavailable in ["computer_observe", "computer_control"] {
                assert!(
                    !registry.contains(unavailable),
                    "the non-Windows default registry must not expose unavailable tool {unavailable}"
                );
                assert!(
                    !names.contains(&unavailable),
                    "{provider:?}/{model} leaked unavailable tool {unavailable} into the first-request surface"
                );
            }
        }
    }
}
