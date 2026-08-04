//! Provider-neutral prompt model for agent requests.
//!
//! The current provider adapters still consume [`Message`] lists, but agent
//! assembly should not be expressed as "just append more system messages".
//! This IR keeps policy, runtime context, evidence, transcript, controller
//! state, and tool surface separate until the final provider-compatibility
//! compile step.

use serde::{Deserialize, Serialize};

pub use crate::llm::{CacheBoundaryHint, PromptStability};
use crate::llm::{Message, Role, ToolDefinition};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum PromptLayer {
    Policy,
    Developer,
    Runtime,
    Evidence,
    Transcript,
    ControllerState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptBlock {
    pub layer: PromptLayer,
    pub content: String,
    pub stability: PromptStability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_boundary_hint: Option<CacheBoundaryHint>,
}

impl PromptBlock {
    pub fn new(layer: PromptLayer, content: impl Into<String>) -> Option<Self> {
        let content = content.into();
        if content.trim().is_empty() {
            return None;
        }
        let (stability, cache_boundary_hint) = cache_semantics_for_layer(layer);
        Some(Self {
            layer,
            content,
            stability,
            cache_boundary_hint,
        })
    }

    pub fn with_cache_semantics(
        mut self,
        stability: PromptStability,
        cache_boundary_hint: Option<CacheBoundaryHint>,
    ) -> Self {
        self.stability = stability;
        self.cache_boundary_hint = cache_boundary_hint;
        self
    }
}

fn cache_semantics_for_layer(layer: PromptLayer) -> (PromptStability, Option<CacheBoundaryHint>) {
    match layer {
        PromptLayer::Policy | PromptLayer::Developer => {
            (PromptStability::Stable, Some(CacheBoundaryHint::PolicyEnd))
        }
        PromptLayer::Evidence => (
            PromptStability::Replayable,
            Some(CacheBoundaryHint::StableEvidenceEnd),
        ),
        PromptLayer::Runtime | PromptLayer::Transcript => (
            PromptStability::Replayable,
            Some(CacheBoundaryHint::ReplayableTurnTail),
        ),
        PromptLayer::ControllerState => (
            PromptStability::Volatile,
            Some(CacheBoundaryHint::ReplayableTurnTail),
        ),
    }
}

pub fn controller_state_message(content: impl Into<String>) -> Option<Message> {
    PromptBlock::new(PromptLayer::ControllerState, content)
        .and_then(|block| message_from_blocks(Role::System, std::iter::once(&block)))
}

pub fn evidence_message(content: impl Into<String>) -> Option<Message> {
    PromptBlock::new(PromptLayer::Evidence, content)
        .and_then(|block| message_from_blocks(Role::System, std::iter::once(&block)))
}

#[derive(Debug, Clone, Default)]
pub struct ToolSurface {
    pub definitions: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentPrompt {
    pub policy: Vec<PromptBlock>,
    pub developer: Vec<PromptBlock>,
    pub runtime: Vec<PromptBlock>,
    pub evidence: Vec<PromptBlock>,
    pub transcript: Vec<Message>,
    pub current_user: Option<Message>,
    pub controller_state: Vec<PromptBlock>,
    pub tools: ToolSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePlacement {
    AfterPolicy,
    Tail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCompileOptions {
    pub runtime_placement: RuntimePlacement,
}

impl Default for PromptCompileOptions {
    fn default() -> Self {
        Self {
            runtime_placement: RuntimePlacement::AfterPolicy,
        }
    }
}

impl AgentPrompt {
    pub fn compile_to_messages(&self, options: PromptCompileOptions) -> Vec<Message> {
        let mut messages = Vec::with_capacity(
            self.transcript.len()
                + usize::from(self.current_user.is_some())
                + usize::from(!self.policy.is_empty() || !self.developer.is_empty())
                + usize::from(self.has_context_tail()),
        );

        if let Some(message) = message_from_blocks(
            Role::System,
            self.policy.iter().chain(self.developer.iter()),
        ) {
            messages.push(message);
        }

        if options.runtime_placement == RuntimePlacement::AfterPolicy {
            self.push_context_messages(&mut messages);
        }

        messages.extend(self.transcript.iter().cloned());

        if let Some(current_user) = &self.current_user {
            messages.push(current_user.clone().with_prompt_cache_hint(
                PromptStability::Replayable,
                CacheBoundaryHint::ReplayableTurnTail,
            ));
        }

        if options.runtime_placement == RuntimePlacement::Tail {
            self.push_context_messages(&mut messages);
        }

        messages
    }

    fn push_context_messages(&self, messages: &mut Vec<Message>) {
        for blocks in [
            self.runtime.as_slice(),
            self.evidence.as_slice(),
            self.controller_state.as_slice(),
        ] {
            if let Some(message) = message_from_blocks(Role::System, blocks.iter()) {
                messages.push(message);
            }
        }
    }

    fn has_context_tail(&self) -> bool {
        !self.runtime.is_empty() || !self.evidence.is_empty() || !self.controller_state.is_empty()
    }
}

fn joined_blocks<'a>(blocks: impl Iterator<Item = &'a PromptBlock>) -> Option<String> {
    let parts = blocks
        .filter_map(|block| {
            let trimmed = block.content.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn message_from_blocks<'a>(
    role: Role,
    blocks: impl Iterator<Item = &'a PromptBlock>,
) -> Option<Message> {
    let blocks = blocks
        .filter(|block| !block.content.trim().is_empty())
        .collect::<Vec<_>>();
    let content = joined_blocks(blocks.iter().copied())?;
    let cache_semantics = blocks.iter().rev().find_map(|block| {
        block
            .cache_boundary_hint
            .map(|boundary| (block.stability, boundary))
    });
    let message = Message::text(role, content);
    Some(match cache_semantics {
        Some((stability, boundary)) => message.with_prompt_cache_hint(stability, boundary),
        None => message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Message {
        Message::text(Role::User, text)
    }

    #[test]
    fn compiler_keeps_runtime_separate_from_transcript_at_tail() {
        let prompt = AgentPrompt {
            policy: vec![PromptBlock::new(PromptLayer::Policy, "policy").unwrap()],
            runtime: vec![PromptBlock::new(PromptLayer::Runtime, "runtime").unwrap()],
            transcript: vec![user("first")],
            current_user: Some(user("second")),
            ..AgentPrompt::default()
        };

        let messages = prompt.compile_to_messages(PromptCompileOptions {
            runtime_placement: RuntimePlacement::Tail,
        });

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), "policy");
        assert_eq!(messages[1].text_content(), "first");
        assert_eq!(messages[2].text_content(), "second");
        assert_eq!(messages[3].role, Role::System);
        assert_eq!(messages[3].text_content(), "runtime");
    }

    #[test]
    fn compiler_places_runtime_after_policy_when_requested() {
        let prompt = AgentPrompt {
            policy: vec![PromptBlock::new(PromptLayer::Policy, "policy").unwrap()],
            runtime: vec![PromptBlock::new(PromptLayer::Runtime, "runtime").unwrap()],
            current_user: Some(user("question")),
            ..AgentPrompt::default()
        };

        let messages = prompt.compile_to_messages(PromptCompileOptions::default());

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].text_content(), "policy");
        assert_eq!(messages[1].text_content(), "runtime");
        assert_eq!(messages[2].text_content(), "question");
    }

    #[test]
    fn compiler_keeps_evidence_and_controller_state_out_of_policy() {
        let prompt = AgentPrompt {
            policy: vec![PromptBlock::new(PromptLayer::Policy, "policy").unwrap()],
            evidence: vec![PromptBlock::new(PromptLayer::Evidence, "evidence").unwrap()],
            controller_state: vec![PromptBlock::new(PromptLayer::ControllerState, "plan").unwrap()],
            current_user: Some(user("question")),
            ..AgentPrompt::default()
        };

        let messages = prompt.compile_to_messages(PromptCompileOptions::default());

        assert_eq!(messages[0].text_content(), "policy");
        assert_eq!(messages[1].text_content(), "evidence");
        assert_eq!(messages[2].text_content(), "plan");
        assert_eq!(messages[3].text_content(), "question");
        assert!(!messages[0].text_content().contains("evidence"));
        assert!(!messages[0].text_content().contains("plan"));
        assert_eq!(
            messages[0].prompt_cache_hint(),
            Some((PromptStability::Stable, CacheBoundaryHint::PolicyEnd))
        );
        assert_eq!(
            messages[1].prompt_cache_hint(),
            Some((
                PromptStability::Replayable,
                CacheBoundaryHint::StableEvidenceEnd
            ))
        );
        assert_eq!(
            messages[2].prompt_cache_hint(),
            Some((
                PromptStability::Volatile,
                CacheBoundaryHint::ReplayableTurnTail
            ))
        );
    }

    #[test]
    fn controller_state_message_ignores_blank_content() {
        assert!(controller_state_message("   ").is_none());
        let message = controller_state_message("## Loop Guard\nChange strategy").unwrap();
        assert_eq!(message.role, Role::System);
        assert_eq!(message.text_content(), "## Loop Guard\nChange strategy");
    }

    #[test]
    fn evidence_message_ignores_blank_content() {
        assert!(evidence_message("   ").is_none());
        let message = evidence_message("## Retrieved Evidence\nfacts").unwrap();
        assert_eq!(message.role, Role::System);
        assert_eq!(message.text_content(), "## Retrieved Evidence\nfacts");
    }

    #[test]
    fn prompt_blocks_express_cache_stability_without_vendor_fields() {
        let policy = PromptBlock::new(PromptLayer::Policy, "policy").unwrap();
        let evidence = PromptBlock::new(PromptLayer::Evidence, "evidence").unwrap();
        let controller = PromptBlock::new(PromptLayer::ControllerState, "plan").unwrap();

        assert_eq!(policy.stability, PromptStability::Stable);
        assert_eq!(
            policy.cache_boundary_hint,
            Some(CacheBoundaryHint::PolicyEnd)
        );
        assert_eq!(evidence.stability, PromptStability::Replayable);
        assert_eq!(
            evidence.cache_boundary_hint,
            Some(CacheBoundaryHint::StableEvidenceEnd)
        );
        assert_eq!(controller.stability, PromptStability::Volatile);
    }
}
