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
                include_skill_system_prompt: false,
                include_turn_scaffolding_system_prompts: false,
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

pub(super) fn insert_turn_scaffolding_system_prompts(
    messages: &mut Vec<Message>,
    route_prompt_section: &str,
    task_plan: &AgentTaskPlan,
    include_dynamic_tool_discovery: bool,
    layout: PromptLayout,
) {
    if !layout.include_turn_scaffolding_system_prompts {
        return;
    }

    if !route_prompt_section.trim().is_empty() {
        let insert_at = messages.len().min(1);
        messages.insert(
            insert_at,
            Message::text(Role::System, route_prompt_section.to_string()),
        );
    }

    let plan_insert_at = messages.len().min(2);
    messages.insert(
        plan_insert_at,
        Message::text(Role::System, task_plan.to_prompt_section()),
    );

    if include_dynamic_tool_discovery {
        let discovery_insert_at = messages.len().min(3);
        messages.insert(
            discovery_insert_at,
            Message::text(
                Role::System,
                tool_discovery::dynamic_tool_visibility_prompt().to_string(),
            ),
        );
    }
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
    fn deepseek_layout_omits_non_persistent_turn_scaffolding() {
        let layout = PromptLayout::for_provider(Some(ProviderType::DeepSeek));

        assert!(!layout.include_skill_system_prompt);
        assert!(!layout.include_turn_scaffolding_system_prompts);
        assert!(!layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
        assert!(!layout.effective_dynamic_tool_visibility(true));
    }

    #[test]
    fn exact_prefix_cache_layout_omits_non_persistent_turn_scaffolding() {
        for provider_type in [
            ProviderType::OpenAi,
            ProviderType::AzureOpenAi,
            ProviderType::Qwen,
            ProviderType::OpenRouter,
        ] {
            let layout = PromptLayout::for_provider(Some(provider_type));

            assert!(!layout.include_skill_system_prompt);
            assert!(!layout.include_turn_scaffolding_system_prompts);
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

        assert!(!layout.include_skill_system_prompt);
        assert!(!layout.allow_dynamic_tool_visibility);
        assert!(layout.append_volatile_system_prompt_to_tail);
    }

    #[test]
    fn deepseek_scaffolding_insertion_is_noop_for_prefix_stability() {
        let mut messages = vec![
            Message::text(Role::System, "stable system"),
            Message::text(Role::User, "user"),
        ];

        insert_turn_scaffolding_system_prompts(
            &mut messages,
            "## Active Routing Plan\nroute",
            &plan(),
            true,
            PromptLayout::for_provider(Some(ProviderType::DeepSeek)),
        );

        assert_eq!(messages.len(), 2);
        assert!(!messages
            .iter()
            .any(|message| message.text_content().contains("Active Task Plan")));
        assert!(!messages
            .iter()
            .any(|message| message.text_content().contains("Active Routing Plan")));
    }

    #[test]
    fn default_scaffolding_insertion_preserves_existing_prompt_order() {
        let mut messages = vec![
            Message::text(Role::System, "stable system"),
            Message::text(Role::System, "runtime"),
            Message::text(Role::User, "user"),
        ];

        insert_turn_scaffolding_system_prompts(
            &mut messages,
            "## Active Routing Plan\nroute",
            &plan(),
            true,
            PromptLayout::for_provider(Some(ProviderType::Custom)),
        );

        assert!(messages[1].text_content().contains("Active Routing Plan"));
        assert!(messages[2].text_content().contains("Active Task Plan"));
        assert!(messages[3]
            .text_content()
            .contains("Dynamic Tool Discovery"));
        assert_eq!(messages[4].text_content(), "runtime");
    }

    #[test]
    fn deepseek_second_turn_replays_first_turn_prefix_without_scaffolding() {
        let layout = PromptLayout::for_provider(Some(ProviderType::DeepSeek));
        let mut first_turn = vec![
            Message::text(Role::System, "stable system"),
            Message::text(
                Role::System,
                "## Runtime Context\nCurrent date: 2026-06-02 (UTC)",
            ),
            Message::text(Role::User, "first question"),
        ];
        insert_turn_scaffolding_system_prompts(
            &mut first_turn,
            "## Active Routing Plan\nfirst route",
            &plan(),
            false,
            layout,
        );

        let mut second_turn = vec![
            Message::text(Role::System, "stable system"),
            Message::text(
                Role::System,
                "## Runtime Context\nCurrent date: 2026-06-02 (UTC)",
            ),
            Message::text(Role::User, "first question"),
            Message::text(Role::Assistant, "first answer"),
            Message::text(Role::User, "second question"),
        ];
        insert_turn_scaffolding_system_prompts(
            &mut second_turn,
            "## Active Routing Plan\nsecond route",
            &plan(),
            false,
            layout,
        );

        let first_prefix = first_turn
            .iter()
            .map(|message| (message.role.clone(), message.text_content()))
            .collect::<Vec<_>>();
        let second_prefix = second_turn
            .iter()
            .take(first_turn.len())
            .map(|message| (message.role.clone(), message.text_content()))
            .collect::<Vec<_>>();

        assert_eq!(first_prefix, second_prefix);
    }
}
