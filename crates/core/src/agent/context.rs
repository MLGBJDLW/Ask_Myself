//! Context management — prepare and trim messages for LLM requests.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::prompt_ir::{
    AgentPrompt, PromptBlock, PromptCompileOptions, PromptLayer, RuntimePlacement, ToolSurface,
};
use crate::conversation::memory::{
    context_safety_buffer, estimate_message_tokens_for_model, estimate_tokens_for_model,
    model_context_window, trim_to_context_window,
};
use crate::llm::{ContentPart, Message, Role, ToolDefinition};
use crate::skills::Skill;

const CHARS_PER_TOKEN_ESTIMATE: usize = 4;
const SYSTEM_PROMPT_CONTEXT_FRACTION: usize = 4;
const MIN_SYSTEM_PROMPT_CHARS: usize = 16_000;
const MAX_SYSTEM_PROMPT_CHARS: usize = 256_000;
const MIN_SKILL_PROMPT_CHARS: usize = 8_000;
const MAX_SKILL_PROMPT_CHARS: usize = 64_000;

/// Build a complete message list for an LLM request, trimmed to fit the
/// model's context window.
///
/// 1. Prepend the system prompt.
/// 2. Append conversation history.
/// 3. Append the new user message.
/// 4. Trim from the oldest non-system message to stay within the context window
///    minus `max_tokens_response` (reserved for the model's answer).
///
/// If `context_window_override` is `Some`, it takes priority over auto-detection.
#[allow(clippy::too_many_arguments)]
pub fn prepare_messages(
    system_prompt: &str,
    history: &[Message],
    user_parts: &[ContentPart],
    model: &str,
    max_tokens_response: u32,
    context_window_override: Option<u32>,
    skills: &[Skill],
    loaded_skills: &[Skill],
    tool_definitions: &[ToolDefinition],
) -> Vec<Message> {
    prepare_messages_with_options(
        system_prompt,
        history,
        user_parts,
        model,
        max_tokens_response,
        context_window_override,
        skills,
        loaded_skills,
        tool_definitions,
        PrepareMessagesOptions::default(),
    )
}

#[derive(Debug, Clone, Copy)]
pub struct PrepareMessagesOptions<'a> {
    pub include_skill_system_prompt: bool,
    pub volatile_system_sections: &'a [&'a str],
    pub evidence_sections: &'a [&'a str],
    pub controller_state_sections: &'a [&'a str],
    pub append_volatile_system_prompt_to_tail: bool,
}

impl Default for PrepareMessagesOptions<'static> {
    fn default() -> Self {
        Self {
            include_skill_system_prompt: true,
            volatile_system_sections: &[],
            evidence_sections: &[],
            controller_state_sections: &[],
            append_volatile_system_prompt_to_tail: false,
        }
    }
}

/// Build a complete message list with provider-aware prompt layout options.
#[allow(clippy::too_many_arguments)]
pub fn prepare_messages_with_options(
    system_prompt: &str,
    history: &[Message],
    user_parts: &[ContentPart],
    model: &str,
    max_tokens_response: u32,
    context_window_override: Option<u32>,
    skills: &[Skill],
    loaded_skills: &[Skill],
    tool_definitions: &[ToolDefinition],
    options: PrepareMessagesOptions<'_>,
) -> Vec<Message> {
    let user_query = user_parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Budget prompt layers from the model context instead of using a small
    // fixed cap. This preserves mandatory prompt and skill-index layers on
    // modern long-context models while still protecting small-context models.
    let max_context = context_window_override.unwrap_or_else(|| model_context_window(model));
    let tool_overhead = estimate_tool_tokens_for_model(model, tool_definitions);
    let effective_context = max_context
        .saturating_sub(tool_overhead)
        .saturating_sub(context_safety_buffer(max_context));
    let system_prompt_budget = system_prompt_char_budget(effective_context, max_tokens_response);
    let skill_budget = skill_prompt_char_budget(max_context, system_prompt_budget);
    let loaded_skills_section = if options.include_skill_system_prompt {
        crate::skills::build_loaded_skills_section_with_budget(
            loaded_skills,
            loaded_skill_prompt_char_budget(skill_budget, !skills.is_empty()),
        )
    } else {
        String::new()
    };
    let remaining_skill_budget = skill_budget.saturating_sub(loaded_skills_section.len());
    let available_skills_section = if options.include_skill_system_prompt {
        crate::skills::build_skills_section_for_query_with_budget(
            skills,
            &user_query,
            remaining_skill_budget,
        )
    } else {
        String::new()
    };
    let volatile_skill_budget = if !options.include_skill_system_prompt
        || (skills.is_empty() && loaded_skills.is_empty())
    {
        0
    } else {
        skill_budget
    };
    let stable_system_prompt = cap_text_to_chars(
        system_prompt.to_string(),
        stable_system_prompt_char_budget(system_prompt_budget, volatile_skill_budget),
        "\n...[truncated]",
    );
    let runtime_section = format!(
        "## Runtime Context\nCurrent date: {} (UTC)",
        Utc::now().format("%Y-%m-%d")
    );
    let volatile_skills_section =
        combine_prompt_sections(available_skills_section, loaded_skills_section);
    let volatile_system_prompt = combine_prompt_section_parts(
        std::iter::once(runtime_section.as_str())
            .chain(options.volatile_system_sections.iter().copied())
            .chain(std::iter::once(volatile_skills_section.as_str())),
    );
    let current_user = Message {
        role: Role::User,
        parts: user_parts.to_vec(),
        name: None,
        tool_calls: None,
        reasoning_content: None,
        prompt_cache_hint: None,
    };
    let prompt = AgentPrompt {
        policy: PromptBlock::new(PromptLayer::Policy, stable_system_prompt)
            .into_iter()
            .collect(),
        runtime: PromptBlock::new(PromptLayer::Runtime, volatile_system_prompt)
            .into_iter()
            .collect(),
        evidence: options
            .evidence_sections
            .iter()
            .filter_map(|section| PromptBlock::new(PromptLayer::Evidence, *section))
            .collect(),
        transcript: history.to_vec(),
        current_user: Some(current_user),
        controller_state: options
            .controller_state_sections
            .iter()
            .filter_map(|section| PromptBlock::new(PromptLayer::ControllerState, *section))
            .collect(),
        tools: ToolSurface {
            definitions: tool_definitions.to_vec(),
        },
        ..AgentPrompt::default()
    };
    let messages = prompt.compile_to_messages(PromptCompileOptions {
        runtime_placement: if options.append_volatile_system_prompt_to_tail {
            RuntimePlacement::Tail
        } else {
            RuntimePlacement::AfterPolicy
        },
    });

    // Trim to fit context window, accounting for tool definition overhead.
    let mut trimmed = trim_to_context_window(&messages, effective_context, max_tokens_response);

    // If messages were evicted, inject an extractive recap into the system prompt
    // so the LLM retains awareness of earlier conversation topics.
    let original_non_system = messages.iter().filter(|m| m.role != Role::System).count();
    let kept_non_system = trimmed.iter().filter(|m| m.role != Role::System).count();
    let evicted_count = original_non_system.saturating_sub(kept_non_system);

    if evicted_count > 0 {
        let evicted: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .take(evicted_count)
            .collect();

        let recap = build_evicted_recap(&evicted);
        if !recap.is_empty() {
            if let Some(sys) = trimmed.iter_mut().rev().find(|m| m.role == Role::System) {
                if let Some(ContentPart::Text { text }) = sys.parts.first_mut() {
                    *text = cap_text_to_chars(
                        format!("{}\n\n{}", text, recap),
                        system_prompt_budget,
                        "\n...[truncated]",
                    );
                }
            }
        }
    }

    trimmed
}

/// Build an extractive recap of evicted conversation messages.
///
/// Only includes `User` and `Assistant` messages (skips tool-call
/// intermediaries). The output is capped at ~800 characters (~200 tokens)
/// to avoid eating too much context budget.
fn build_evicted_recap(evicted: &[&Message]) -> String {
    const MAX_RECAP_CHARS: usize = 800;
    let mut parts: Vec<String> = Vec::new();
    let mut total_chars: usize = 0;

    for msg in evicted {
        if total_chars >= MAX_RECAP_CHARS {
            break;
        }

        match msg.role {
            Role::User => {
                let text = msg.text_content();
                let summary = if text.trim().is_empty() {
                    if msg.has_images() {
                        "[image]".to_string()
                    } else {
                        continue;
                    }
                } else {
                    let label = if msg.has_images() { "[image] " } else { "" };
                    format!("{}{}", label, truncate_text(&text, 100))
                };
                let line = format!("- User asked: {}", summary);
                total_chars += line.len();
                parts.push(line);
            }
            Role::Assistant => {
                // Skip tool-call intermediary messages.
                if msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
                    continue;
                }
                let text = msg.text_content();
                let summary = if text.trim().is_empty() {
                    continue;
                } else {
                    truncate_text(&text, 80)
                };
                let line = format!("- You answered: {}", summary);
                total_chars += line.len();
                parts.push(line);
            }
            _ => {} // Skip Tool and System messages.
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    format!(
        "## Earlier conversation context (summarized)\n\
         These topics were discussed earlier but trimmed for context space:\n{}",
        parts.join("\n")
    )
}

/// Public entry-point for building an extractive recap from owned messages.
///
/// This is used by `AgentExecutor::summarize_if_needed` as the extractive
/// fallback string that gets passed to the LLM summariser.
pub fn build_evicted_recap_from_messages(evicted: &[Message]) -> String {
    let refs: Vec<&Message> = evicted.iter().collect();
    build_evicted_recap(&refs)
}

/// Estimate tokens occupied by tool definitions in the LLM request.
pub fn estimate_tool_tokens(tools: &[ToolDefinition]) -> u32 {
    estimate_tool_tokens_for_model("gpt-4o", tools)
}

pub fn estimate_tool_tokens_for_model(model: &str, tools: &[ToolDefinition]) -> u32 {
    let mut total = 0u32;
    for tool in tools {
        total += estimate_tool_definition_tokens_for_model(model, tool);
    }
    total
}

/// Token contribution of a context segment in the current model request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageSegment {
    pub kind: String,
    pub tokens: u32,
}

/// Best-effort breakdown of the prompt tokens used by the latest model request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageBreakdown {
    pub total_tokens: u32,
    pub segments: Vec<ContextUsageSegment>,
}

pub fn estimate_context_usage_breakdown_for_model(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    actual_prompt_tokens: Option<u32>,
) -> ContextUsageBreakdown {
    const ORDER: [&str; 24] = [
        "systemCore",
        "runtime",
        "instructions",
        "persona",
        "routePlan",
        "taskPlan",
        "availableSkills",
        "loadedSkills",
        "userMemory",
        "projectMemory",
        "agentMemory",
        "preferences",
        "learnedSuccesses",
        "scratchpad",
        "sourceScope",
        "collectionContext",
        "conversationSummary",
        "conversation",
        "thinking",
        "toolCalls",
        "toolResults",
        "tools",
        "mcp",
        "overhead",
    ];

    let mut segments: BTreeMap<&'static str, u32> = BTreeMap::new();
    for message in messages {
        add_message_context_tokens(&mut segments, model, message);
    }
    for tool in tools {
        let kind = if is_mcp_tool_definition(tool) {
            "mcp"
        } else {
            "tools"
        };
        add_tokens(
            &mut segments,
            kind,
            estimate_tool_definition_tokens_for_model(model, tool),
        );
    }

    let estimated_total = segments.values().copied().sum::<u32>();
    let actual_total = actual_prompt_tokens.unwrap_or(0);
    let total_tokens = if actual_total > 0 {
        actual_total
    } else {
        estimated_total
    };

    if actual_total > 0 && estimated_total > actual_total {
        scale_segments_to_total(&mut segments, estimated_total, actual_total);
    } else if actual_total > estimated_total {
        add_tokens(&mut segments, "overhead", actual_total - estimated_total);
    }

    let segments = ORDER
        .iter()
        .filter_map(|kind| {
            let tokens = segments.get(*kind).copied().unwrap_or(0);
            (tokens > 0).then(|| ContextUsageSegment {
                kind: (*kind).to_string(),
                tokens,
            })
        })
        .collect();

    ContextUsageBreakdown {
        total_tokens,
        segments,
    }
}

fn add_message_context_tokens(
    segments: &mut BTreeMap<&'static str, u32>,
    model: &str,
    message: &Message,
) {
    match message.role {
        Role::System => add_system_prompt_tokens(segments, model, &message.text_content()),
        Role::Tool => add_tokens(
            segments,
            "toolResults",
            estimate_message_tokens_for_model(model, message),
        ),
        Role::User => add_tokens(
            segments,
            "conversation",
            estimate_message_body_tokens_for_model(model, message),
        ),
        Role::Assistant => {
            add_tokens(
                segments,
                "conversation",
                estimate_message_body_tokens_for_model(model, message),
            );
            if let Some(reasoning) = message.reasoning_content.as_deref() {
                add_tokens(
                    segments,
                    "thinking",
                    estimate_tokens_for_model(model, reasoning),
                );
            }
            if let Some(tool_calls) = message.tool_calls.as_ref() {
                let mut tokens = 0u32;
                for call in tool_calls {
                    tokens = tokens
                        .saturating_add(estimate_tokens_for_model(model, &call.id))
                        .saturating_add(estimate_tokens_for_model(model, &call.name))
                        .saturating_add(estimate_tokens_for_model(model, &call.arguments))
                        .saturating_add(4);
                }
                add_tokens(segments, "toolCalls", tokens);
            }
        }
    }
}

fn estimate_message_body_tokens_for_model(model: &str, message: &Message) -> u32 {
    let mut tokens = estimate_tokens_for_model(model, &message.text_content());
    for part in &message.parts {
        if let ContentPart::Image { data, .. } = part {
            let estimated = (data.len() / 1500) as u32;
            tokens = tokens.saturating_add(estimated.max(258));
        }
    }
    tokens
}

fn add_system_prompt_tokens(segments: &mut BTreeMap<&'static str, u32>, model: &str, prompt: &str) {
    for section in split_markdown_h2_sections(prompt) {
        add_tokens(
            segments,
            context_kind_for_system_heading(section.heading),
            estimate_tokens_for_model(model, section.text),
        );
    }
}

struct PromptSection<'a> {
    heading: &'a str,
    text: &'a str,
}

fn split_markdown_h2_sections(prompt: &str) -> Vec<PromptSection<'_>> {
    if prompt.trim().is_empty() {
        return Vec::new();
    }

    let mut sections = Vec::new();
    let mut current_heading = "";
    let mut current_start = 0usize;
    let mut cursor = 0usize;

    for line in prompt.split_inclusive('\n') {
        let line_start = cursor;
        let line_end = cursor + line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") && !trimmed.starts_with("### ") {
            if line_start > current_start {
                sections.push(PromptSection {
                    heading: current_heading,
                    text: &prompt[current_start..line_start],
                });
            }
            current_heading = trimmed
                .trim_start_matches('#')
                .trim()
                .trim_end_matches('\n')
                .trim();
            current_start = line_start;
        }
        cursor = line_end;
    }

    if current_start < prompt.len() {
        sections.push(PromptSection {
            heading: current_heading,
            text: &prompt[current_start..],
        });
    }

    sections
}

fn context_kind_for_system_heading(heading: &str) -> &'static str {
    let normalized = heading.to_ascii_lowercase();
    if normalized.contains("runtime context") || normalized.contains("current turn time") {
        "runtime"
    } else if normalized.contains("conversation-specific instructions") {
        "instructions"
    } else if normalized.contains("active persona") {
        "persona"
    } else if normalized.contains("active routing plan") {
        "routePlan"
    } else if normalized.contains("active task plan") {
        "taskPlan"
    } else if normalized.contains("available skills") {
        "availableSkills"
    } else if normalized.contains("loaded skills") {
        "loadedSkills"
    } else if normalized.contains("user long-term memory") {
        "userMemory"
    } else if normalized.contains("project memory") {
        "projectMemory"
    } else if normalized.contains("agent procedural memory") {
        "agentMemory"
    } else if normalized.contains("user preferences") {
        "preferences"
    } else if normalized.contains("learned successes") {
        "learnedSuccesses"
    } else if normalized.contains("agent scratchpad") {
        "scratchpad"
    } else if normalized.contains("active source scope") {
        "sourceScope"
    } else if normalized.contains("collection context") {
        "collectionContext"
    } else if normalized.contains("earlier conversation context") {
        "conversationSummary"
    } else {
        "systemCore"
    }
}

fn estimate_tool_definition_tokens_for_model(model: &str, tool: &ToolDefinition) -> u32 {
    let tool_text = format!("{} {} {}", tool.name, tool.description, tool.parameters);
    estimate_tokens_for_model(model, &tool_text) + 10
}

fn is_mcp_tool_definition(tool: &ToolDefinition) -> bool {
    tool.name == "mcp_tool" || tool.name.starts_with("mcp__")
}

fn add_tokens(segments: &mut BTreeMap<&'static str, u32>, kind: &'static str, tokens: u32) {
    if tokens == 0 {
        return;
    }
    *segments.entry(kind).or_insert(0) += tokens;
}

fn scale_segments_to_total(
    segments: &mut BTreeMap<&'static str, u32>,
    estimated_total: u32,
    actual_total: u32,
) {
    if estimated_total == 0 {
        return;
    }

    let largest_kind = segments
        .iter()
        .max_by_key(|(_, tokens)| *tokens)
        .map(|(kind, _)| *kind);
    let mut scaled_total = 0u32;

    for tokens in segments.values_mut() {
        let scaled = ((*tokens as u64) * (actual_total as u64) / (estimated_total as u64)) as u32;
        *tokens = scaled;
        scaled_total = scaled_total.saturating_add(scaled);
    }

    if let Some(kind) = largest_kind {
        let remainder = actual_total.saturating_sub(scaled_total);
        if remainder > 0 {
            *segments.entry(kind).or_insert(0) += remainder;
        }
    }
}

fn system_prompt_char_budget(effective_context_tokens: u32, reserved_for_response: u32) -> usize {
    let available_tokens = effective_context_tokens.saturating_sub(reserved_for_response) as usize;
    if available_tokens == 0 {
        return MIN_SYSTEM_PROMPT_CHARS
            .min((effective_context_tokens as usize).saturating_mul(CHARS_PER_TOKEN_ESTIMATE));
    }
    let available_chars = available_tokens.saturating_mul(CHARS_PER_TOKEN_ESTIMATE);
    let target = (available_chars / SYSTEM_PROMPT_CONTEXT_FRACTION)
        .clamp(MIN_SYSTEM_PROMPT_CHARS, MAX_SYSTEM_PROMPT_CHARS);
    target.min(available_chars)
}

fn skill_prompt_char_budget(context_window_tokens: u32, system_prompt_budget: usize) -> usize {
    let one_percent_context_chars =
        (context_window_tokens as usize).saturating_mul(CHARS_PER_TOKEN_ESTIMATE) / 100;
    one_percent_context_chars
        .clamp(MIN_SKILL_PROMPT_CHARS, MAX_SKILL_PROMPT_CHARS)
        .min(system_prompt_budget / 2)
}

fn stable_system_prompt_char_budget(system_prompt_budget: usize, skill_budget: usize) -> usize {
    system_prompt_budget.saturating_sub(skill_budget)
}

fn loaded_skill_prompt_char_budget(total_skill_budget: usize, keep_available_index: bool) -> usize {
    if total_skill_budget == 0 {
        return 0;
    }
    if !keep_available_index {
        return total_skill_budget;
    }
    total_skill_budget.saturating_mul(3) / 4
}

fn combine_prompt_sections(first: String, second: String) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, true) => String::new(),
        (false, true) => first,
        (true, false) => second,
        (false, false) => format!("{first}{second}"),
    }
}

fn combine_prompt_section_parts<'a>(sections: impl IntoIterator<Item = &'a str>) -> String {
    let mut combined = String::new();
    for section in sections {
        let section = section.trim();
        if section.is_empty() {
            continue;
        }
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }
        combined.push_str(section);
    }
    combined
}

/// Enforce `MAX_SYSTEM_PROMPT_CHARS` on the system prompt.
///
/// If the prompt exceeds the limit it is truncated on a word boundary and
/// a `...[truncated]` marker is appended so the LLM can see signalling.
#[cfg(test)]
fn cap_system_prompt(text: String) -> String {
    cap_text_to_chars(text, MAX_SYSTEM_PROMPT_CHARS, "\n...[truncated]")
}

fn cap_text_to_chars(text: String, max_chars: usize, marker: &str) -> String {
    if text.len() <= max_chars {
        return text;
    }
    if max_chars == 0 {
        return String::new();
    }
    let marker_budget = marker.len().min(max_chars);
    let content_limit = max_chars.saturating_sub(marker_budget);
    if content_limit == 0 {
        return marker.chars().take(max_chars).collect();
    }
    let safe_limit = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= content_limit)
        .last()
        .unwrap_or(0);
    let truncated = &text[..safe_limit];
    let cut = truncated
        .rfind('\n')
        .or_else(|| truncated.rfind(' '))
        .unwrap_or(safe_limit);
    format!("{}{}", &text[..cut], marker)
}

/// Truncate text to `max_chars` on a word boundary, appending "..." if truncated.
fn truncate_text(text: &str, max_chars: usize) -> String {
    let clean = text.replace('\n', " ");
    let clean = clean.trim();
    if clean.len() <= max_chars {
        return clean.to_string();
    }
    let safe_limit = clean
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_chars)
        .last()
        .unwrap_or(0);
    let truncated = &clean[..safe_limit];
    let cut = truncated.rfind(' ').unwrap_or(safe_limit);
    format!("{}...", &clean[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> Message {
        Message::text(role, content)
    }

    fn skill(id: &str, name: &str, description: &str) -> Skill {
        Skill {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            content: format!("Follow the {name} workflow."),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: crate::skills::SkillInterfaceMetadata::default(),
            dependencies: crate::skills::SkillDependencies::default(),
            policy: crate::skills::SkillPolicy::default(),
            source_path: None,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }
    }

    #[test]
    fn test_prepare_messages_basic() {
        let history = vec![msg(Role::User, "Hi"), msg(Role::Assistant, "Hello!")];
        let result = prepare_messages(
            "System prompt",
            &history,
            &[ContentPart::Text {
                text: "What's up?".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
            &[],
        );

        // The first system message is stable for prompt caching; runtime state
        // lives in a later system message.
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].text_content(), "System prompt");
        assert_eq!(result[1].role, Role::System);
        assert!(result[1].text_content().starts_with("## Runtime Context"));
        assert!(result[1].text_content().contains("Current date:"));

        // Last message is the new user input.
        assert_eq!(result.last().unwrap().text_content(), "What's up?");
        assert_eq!(result.last().unwrap().role, Role::User);
    }

    #[test]
    fn test_prepare_messages_trims_when_needed() {
        // Build a history that exceeds a small context window.
        // Alternate User/Assistant so the recap has both sides.
        // Use varied words instead of repeated characters so tokenizer-backed
        // counting cannot compress the fixture below the trim threshold.
        let history: Vec<Message> = (0..200)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                let padding = (0..80)
                    .map(|n| format!("token{i}_{n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                msg(role, &format!("Message number {i} {padding}"))
            })
            .collect();
        // Force a small context window (8192) so trimming is guaranteed.
        let result = prepare_messages(
            "Sys",
            &history,
            &[ContentPart::Text {
                text: "New".to_string(),
            }],
            "some-model",
            512,
            Some(8192),
            &[],
            &[],
            &[],
        );

        // System message must survive.
        assert_eq!(result[0].role, Role::System);
        // Total input is 202 messages. With 7680 token budget and ~59 tok/msg, only ~130 fit.
        assert!(
            result.len() < 202,
            "expected trimming, got {} messages",
            result.len()
        );
        assert!(result.len() > 2, "expected more than just sys+user");
        // Last message is the new user input.
        assert_eq!(result.last().unwrap().text_content(), "New");

        // System message should contain the evicted recap.
        let system_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            system_text.contains("Earlier conversation context"),
            "System message should contain evicted recap"
        );
    }

    #[test]
    fn test_no_recap_when_nothing_evicted() {
        let history = vec![msg(Role::User, "Hi"), msg(Role::Assistant, "Hello!")];
        let result = prepare_messages(
            "Sys",
            &history,
            &[ContentPart::Text {
                text: "What's up?".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
            &[],
        );

        // No trimming happened, so no recap.
        let system_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!system_text.contains("Earlier conversation context"));
    }

    #[test]
    fn test_prepare_messages_empty_history() {
        let result = prepare_messages(
            "Sys",
            &[],
            &[ContentPart::Text {
                text: "Hello".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[],
            &[],
            &[],
        );
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[1].role, Role::System);
        assert_eq!(result[2].role, Role::User);
        assert_eq!(result[2].text_content(), "Hello");
    }

    #[test]
    fn test_prepare_messages_with_skills() {
        let skills = vec![skill("1", "Be Concise", "Always favor brevity")];
        let result = prepare_messages(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Hi".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &skills,
            &[],
            &[],
        );
        let sys_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            sys_text.contains("Available Skills"),
            "Skills should be in system prompt"
        );
        assert!(sys_text.contains("Be Concise"));
    }

    #[test]
    fn test_available_skills_do_not_change_stable_system_prompt() {
        let first = prepare_messages(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Write a short scene".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[skill(
                "fiction",
                "Fiction Writing",
                "Use when writing fiction",
            )],
            &[],
            &[],
        );
        let second = prepare_messages(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Audit a spreadsheet".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &[skill(
                "xlsx",
                "Spreadsheet Analysis",
                "Use when reviewing workbooks",
            )],
            &[],
            &[],
        );

        assert_eq!(first[0].text_content(), second[0].text_content());
        assert!(!first[0].text_content().contains("Available Skills"));
        assert!(first[1].text_content().contains("Fiction Writing"));
        assert!(second[1].text_content().contains("Spreadsheet Analysis"));
    }

    #[test]
    fn test_prepare_messages_with_loaded_skills() {
        let loaded_skills = vec![Skill {
            id: "skill-loaded".into(),
            name: "Fiction Writing".into(),
            description: "Use when writing fiction".into(),
            content: "Draft scenes with concrete stakes and natural prose.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: true,
            interface: crate::skills::SkillInterfaceMetadata {
                display_name: "Fiction Writing".into(),
                short_description: "Write fiction".into(),
                icon_small: None,
                icon_large: None,
                default_prompt: None,
            },
            dependencies: crate::skills::SkillDependencies::default(),
            policy: crate::skills::SkillPolicy::default(),
            source_path: None,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }];

        let result = prepare_messages(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Write a chapter".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &loaded_skills,
            &loaded_skills,
            &[],
        );

        let sys_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sys_text.contains("Loaded Skills"));
        assert!(sys_text.contains("skill_id: skill-loaded"));
        assert!(sys_text.contains("Draft scenes with concrete stakes"));
        assert!(sys_text.contains("Available Skills"));
    }

    #[test]
    fn test_prepare_messages_can_omit_skill_sections_for_implicit_prefix_cache() {
        let skills = vec![skill(
            "cache-sensitive",
            "Cache Sensitive Skill",
            "Use when testing cache-sensitive prompt layout",
        )];

        let result = prepare_messages_with_options(
            "System prompt",
            &[],
            &[ContentPart::Text {
                text: "Use a different current task".to_string(),
            }],
            "deepseek-v4-pro",
            4096,
            None,
            &skills,
            &skills,
            &[],
            PrepareMessagesOptions {
                include_skill_system_prompt: false,
                ..PrepareMessagesOptions::default()
            },
        );

        let sys_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sys_text.contains("Runtime Context"));
        assert!(!sys_text.contains("Available Skills"));
        assert!(!sys_text.contains("Loaded Skills"));
        assert!(!sys_text.contains("Cache Sensitive Skill"));
    }

    #[test]
    fn test_prepare_messages_can_append_volatile_system_prompt_to_tail() {
        let volatile_sections = ["## Current Turn Time\nLocal time: 12:34:56"];
        let result = prepare_messages_with_options(
            "Stable system prompt",
            &[],
            &[ContentPart::Text {
                text: "Inspect this repo".to_string(),
            }],
            "deepseek-v4-pro",
            4096,
            None,
            &[],
            &[],
            &[],
            PrepareMessagesOptions {
                include_skill_system_prompt: false,
                volatile_system_sections: &volatile_sections,
                append_volatile_system_prompt_to_tail: true,
                ..PrepareMessagesOptions::default()
            },
        );

        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].text_content(), "Stable system prompt");
        assert_eq!(result[1].role, Role::User);
        assert_eq!(result[1].text_content(), "Inspect this repo");
        assert_eq!(result[2].role, Role::System);
        assert!(result[2].text_content().contains("Runtime Context"));
        assert!(result[2].text_content().contains("Current Turn Time"));
    }

    #[test]
    fn test_prepare_messages_keeps_evidence_and_controller_state_in_context_layer() {
        let evidence_sections = ["## Retrieved Evidence\nSource-backed context"];
        let controller_state_sections = [
            "## Active Routing Plan\nUse codebase route",
            "## Active Task Plan\n1. Inspect\n2. Verify",
        ];
        let result = prepare_messages_with_options(
            "Stable system prompt",
            &[],
            &[ContentPart::Text {
                text: "Inspect this repo".to_string(),
            }],
            "deepseek-v4-pro",
            4096,
            None,
            &[],
            &[],
            &[],
            PrepareMessagesOptions {
                include_skill_system_prompt: false,
                evidence_sections: &evidence_sections,
                controller_state_sections: &controller_state_sections,
                append_volatile_system_prompt_to_tail: true,
                ..PrepareMessagesOptions::default()
            },
        );

        assert_eq!(result.len(), 5);
        assert_eq!(result[0].role, Role::System);
        assert_eq!(result[0].text_content(), "Stable system prompt");
        assert_eq!(result[1].role, Role::User);
        assert_eq!(result[2].role, Role::System);
        assert!(result[2].text_content().contains("Runtime Context"));
        assert_eq!(result[3].role, Role::System);
        assert!(result[3].text_content().contains("Retrieved Evidence"));
        assert_eq!(result[4].role, Role::System);
        assert!(result[4].text_content().contains("Active Routing Plan"));
        assert!(result[4].text_content().contains("Active Task Plan"));
        assert!(!result[0].text_content().contains("Retrieved Evidence"));
        assert!(!result[0].text_content().contains("Active Routing Plan"));
    }

    #[test]
    fn test_tail_volatile_context_keeps_previous_user_turn_as_prefix() {
        let first_volatile = ["## Current Turn Time\nLocal time: 12:00:00"];
        let first = prepare_messages_with_options(
            "Stable system prompt",
            &[],
            &[ContentPart::Text {
                text: "First question".to_string(),
            }],
            "deepseek-v4-pro",
            4096,
            None,
            &[],
            &[],
            &[],
            PrepareMessagesOptions {
                include_skill_system_prompt: false,
                volatile_system_sections: &first_volatile,
                append_volatile_system_prompt_to_tail: true,
                ..PrepareMessagesOptions::default()
            },
        );

        let history = vec![
            Message::text(Role::User, "First question"),
            Message::text(Role::Assistant, "First answer"),
        ];
        let second_volatile = ["## Current Turn Time\nLocal time: 12:01:00"];
        let second = prepare_messages_with_options(
            "Stable system prompt",
            &history,
            &[ContentPart::Text {
                text: "Second question".to_string(),
            }],
            "deepseek-v4-pro",
            4096,
            None,
            &[],
            &[],
            &[],
            PrepareMessagesOptions {
                include_skill_system_prompt: false,
                volatile_system_sections: &second_volatile,
                append_volatile_system_prompt_to_tail: true,
                ..PrepareMessagesOptions::default()
            },
        );

        assert_eq!(first[0].role, second[0].role);
        assert_eq!(first[0].text_content(), second[0].text_content());
        assert_eq!(first[1].role, second[1].role);
        assert_eq!(first[1].text_content(), second[1].text_content());
        assert_eq!(second[2].role, Role::Assistant);
    }

    #[test]
    fn test_prepare_messages_preserves_skills_with_long_system_prompt() {
        let skills = vec![Skill {
            id: "skill-reserve".into(),
            name: "Reserved Skill".into(),
            description: "Use when the base prompt is long".into(),
            content: "Always load this skill before using its workflow.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: crate::skills::SkillInterfaceMetadata::default(),
            dependencies: crate::skills::SkillDependencies::default(),
            policy: crate::skills::SkillPolicy::default(),
            source_path: None,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }];
        let long_prompt = "Core instruction.\n".repeat(24_000);
        let result = prepare_messages(
            &long_prompt,
            &[],
            &[ContentPart::Text {
                text: "Hi".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &skills,
            &[],
            &[],
        );
        let stable_sys_text = result[0].text_content();
        assert!(stable_sys_text.len() <= MAX_SYSTEM_PROMPT_CHARS);
        assert!(stable_sys_text.contains("...[truncated]"));
        assert!(!stable_sys_text.contains("Available Skills"));
        let sys_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            sys_text.contains("Available Skills"),
            "skill index should survive a long base prompt"
        );
        assert!(sys_text.contains("Reserved Skill"));
    }

    #[test]
    fn test_prepare_messages_preserves_skills_with_default_system_prompt() {
        let skills = vec![Skill {
            id: "skill-default-reserve".into(),
            name: "Default Prompt Skill".into(),
            description: "Use with the default prompt".into(),
            content: "Load this skill from the compact index.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
            interface: crate::skills::SkillInterfaceMetadata::default(),
            dependencies: crate::skills::SkillDependencies::default(),
            policy: crate::skills::SkillPolicy::default(),
            source_path: None,
            resources: Vec::new(),
            resource_bundle: Vec::new(),
        }];
        let result = prepare_messages(
            &super::super::default_system_prompt(),
            &[],
            &[ContentPart::Text {
                text: "Hi".to_string(),
            }],
            "gpt-4o",
            4096,
            None,
            &skills,
            &[],
            &[],
        );
        let stable_sys_text = result[0].text_content();
        assert!(stable_sys_text.len() <= MAX_SYSTEM_PROMPT_CHARS);
        assert!(!stable_sys_text.contains("Available Skills"));
        let sys_text = result
            .iter()
            .filter(|msg| msg.role == Role::System)
            .map(Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sys_text.contains("Available Skills"));
        assert!(sys_text.contains("Default Prompt Skill"));
    }

    #[test]
    fn test_estimate_tool_tokens() {
        let tools = vec![ToolDefinition {
            name: "search".into(),
            description: "Search the knowledge base".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let tokens = estimate_tool_tokens(&tools);
        assert!(tokens > 10, "Tool tokens should be non-trivial");
    }

    #[test]
    fn context_usage_breakdown_splits_prompt_conversation_tools_and_mcp() {
        let mut assistant = msg(Role::Assistant, "I'll use a tool.");
        assistant.tool_calls = Some(vec![crate::llm::ToolCallRequest {
            id: "call_1".into(),
            name: "search".into(),
            arguments: r#"{"query":"rust"}"#.into(),
            thought_signature: None,
        }]);
        let messages = vec![
            msg(Role::System, "System prompt"),
            msg(Role::User, "Find Rust docs"),
            assistant,
            Message::text_with_name(Role::Tool, "Search result", "call_1"),
        ];
        let tools = vec![
            ToolDefinition {
                name: "search_knowledge_base".into(),
                description: "Search local knowledge".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
            ToolDefinition {
                name: "mcp__docs__lookup".into(),
                description: "Lookup docs from MCP".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ];

        let breakdown =
            estimate_context_usage_breakdown_for_model("gpt-4o", &messages, &tools, None);
        let kinds = breakdown
            .segments
            .iter()
            .map(|segment| segment.kind.as_str())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"systemCore"));
        assert!(kinds.contains(&"conversation"));
        assert!(kinds.contains(&"toolCalls"));
        assert!(kinds.contains(&"toolResults"));
        assert!(kinds.contains(&"tools"));
        assert!(kinds.contains(&"mcp"));
    }

    #[test]
    fn context_usage_breakdown_splits_skills_memory_and_thinking() {
        let mut assistant = msg(Role::Assistant, "Visible answer");
        assistant.reasoning_content = Some("hidden reasoning tokens".to_string());
        let messages = vec![
            msg(
                Role::System,
                "Core rules\n\n## Available Skills\nskill index\n\n## Loaded Skills\nskill body\n\n## Project Memory\nproject facts",
            ),
            msg(Role::User, "Draft a chapter"),
            assistant,
        ];

        let breakdown = estimate_context_usage_breakdown_for_model("gpt-4o", &messages, &[], None);
        let kinds = breakdown
            .segments
            .iter()
            .map(|segment| segment.kind.as_str())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"systemCore"));
        assert!(kinds.contains(&"availableSkills"));
        assert!(kinds.contains(&"loadedSkills"));
        assert!(kinds.contains(&"projectMemory"));
        assert!(kinds.contains(&"thinking"));
    }

    #[test]
    fn context_usage_breakdown_reconciles_to_actual_prompt_tokens() {
        let messages = vec![msg(Role::System, "System prompt"), msg(Role::User, "Hello")];
        let breakdown =
            estimate_context_usage_breakdown_for_model("gpt-4o", &messages, &[], Some(10));
        let segment_total = breakdown
            .segments
            .iter()
            .map(|segment| segment.tokens)
            .sum::<u32>();

        assert_eq!(breakdown.total_tokens, 10);
        assert_eq!(segment_total, 10);
    }

    #[test]
    fn cap_system_prompt_is_utf8_safe() {
        let text = format!("{}中文", "a".repeat(MAX_SYSTEM_PROMPT_CHARS - 1));
        let capped = cap_system_prompt(text);

        assert!(capped.ends_with("...[truncated]"));
    }

    #[test]
    fn truncate_text_is_utf8_safe() {
        let text = format!("{}中文", "a".repeat(9));
        let truncated = truncate_text(&text, 10);

        assert!(truncated.ends_with("..."));
    }
}
