//! Provider-aware prompt layout decisions for model requests.

use super::*;

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
        if uses_implicit_prefix_cache(provider_type, model) {
            return Self {
                include_skill_system_prompt: true,
                include_turn_scaffolding_system_prompts: true,
                allow_dynamic_tool_visibility: true,
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

fn uses_implicit_prefix_cache(provider_type: Option<ProviderType>, model: Option<&str>) -> bool {
    matches!(
        provider_type,
        Some(
            ProviderType::OpenAi
                | ProviderType::AzureOpenAi
                | ProviderType::Qwen
                | ProviderType::OpenRouter
                | ProviderType::DeepSeek
        )
    ) || model
        .map(|model| model.to_ascii_lowercase().contains("deepseek"))
        .unwrap_or(false)
}

pub(super) fn turn_scaffolding_sections(
    route_prompt_section: &str,
    task_plan: &AgentTaskPlan,
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

    sections.push(task_plan.to_prompt_section());
    if include_dynamic_tool_discovery {
        sections.push(tool_discovery::dynamic_tool_visibility_prompt().to_string());
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::{build_task_plan, TaskPlanningInput};

    fn plan() -> AgentTaskPlan {
        build_task_plan(TaskPlanningInput {
            user_query: "audit cache behavior",
            route_kind: "CodebaseOperation",
            has_sources: false,
            source_scope_count: 0,
            collection_context: false,
        })
    }

    #[test]
    fn deepseek_layout_keeps_replayable_tail_context() {
        let layout = PromptLayout::for_provider(Some(ProviderType::DeepSeek));

        assert!(layout.include_skill_system_prompt);
        assert!(layout.include_turn_scaffolding_system_prompts);
        assert!(layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
        assert!(layout.effective_dynamic_tool_visibility(true));
    }

    #[test]
    fn exact_prefix_cache_layout_keeps_replayable_tail_context() {
        for provider_type in [
            ProviderType::OpenAi,
            ProviderType::AzureOpenAi,
            ProviderType::Qwen,
            ProviderType::OpenRouter,
        ] {
            let layout = PromptLayout::for_provider(Some(provider_type));

            assert!(layout.include_skill_system_prompt);
            assert!(layout.include_turn_scaffolding_system_prompts);
            assert!(layout.allow_dynamic_tool_visibility);
            assert!(layout.append_volatile_system_prompt_to_tail);
            assert!(layout.effective_dynamic_tool_visibility(true));
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
        assert!(layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
    }

    #[test]
    fn deepseek_scaffolding_sections_are_controller_state() {
        let sections = turn_scaffolding_sections(
            "## Active Routing Plan\nroute",
            &plan(),
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
            &plan(),
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
            turn_scaffolding_sections("## Active Routing Plan\nroute", &plan(), true, layout);

        assert!(sections.is_empty());
    }
}
