//! Inspectable context pack for prompts, evidence, memory, and tool guidance.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::rag::RagContextPack;

pub const CONTEXT_PACK_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemRole {
    Instruction,
    Evidence,
    ToolGuidance,
    Memory,
    Conversation,
    SourceScope,
}

impl ContextItemRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instruction => "instruction",
            Self::Evidence => "evidence",
            Self::ToolGuidance => "tool_guidance",
            Self::Memory => "memory",
            Self::Conversation => "conversation",
            Self::SourceScope => "source_scope",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTrustLevel {
    System,
    UserSelected,
    RetrievedEvidence,
    AgentMemory,
    External,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemStability {
    StablePrefix,
    #[default]
    VolatileSuffix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackItem {
    pub id: String,
    pub role: ContextItemRole,
    pub source: String,
    pub reason: String,
    pub trust_level: ContextTrustLevel,
    pub token_estimate: u32,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub stability: ContextItemStability,
    pub payload: serde_json::Value,
}

impl ContextPackItem {
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        id: impl Into<String>,
        role: ContextItemRole,
        source: impl Into<String>,
        reason: impl Into<String>,
        trust_level: ContextTrustLevel,
        priority: i32,
        stability: ContextItemStability,
        text: impl Into<String>,
    ) -> Self {
        let text = text.into();
        Self {
            id: id.into(),
            role,
            source: source.into(),
            reason: reason.into(),
            trust_level,
            token_estimate: estimate_tokens(&text),
            priority,
            stability,
            payload: serde_json::json!({ "text": text }),
        }
    }

    pub fn prompt_text(&self) -> Option<&str> {
        self.payload.get("text").and_then(serde_json::Value::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPack {
    pub version: u16,
    pub purpose: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
    pub total_token_estimate: u32,
    pub items: Vec<ContextPackItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_item_ids: Vec<String>,
}

impl ContextPack {
    pub fn new(purpose: impl Into<String>, token_budget: Option<u32>) -> Self {
        Self {
            version: CONTEXT_PACK_VERSION,
            purpose: purpose.into(),
            token_budget,
            total_token_estimate: 0,
            items: Vec::new(),
            omitted_item_ids: Vec::new(),
        }
    }

    pub fn push(&mut self, item: ContextPackItem) {
        self.total_token_estimate = self
            .total_token_estimate
            .saturating_add(item.token_estimate);
        self.items.push(item);
    }

    pub fn from_rag_context_pack(
        rag_pack: &RagContextPack,
        purpose: impl Into<String>,
        token_budget: Option<u32>,
    ) -> Self {
        let mut pack = Self::new(purpose, token_budget);
        pack.push(ContextPackItem {
            id: "rag-ordering-policy".to_string(),
            role: ContextItemRole::ToolGuidance,
            source: "rag".to_string(),
            reason: "Explain how retrieved evidence is ordered before prompt packing.".to_string(),
            trust_level: ContextTrustLevel::System,
            token_estimate: estimate_tokens(&rag_pack.ordering_policy),
            priority: 0,
            stability: ContextItemStability::StablePrefix,
            payload: serde_json::json!({
                "orderingPolicy": rag_pack.ordering_policy,
                "recommendedContextChunks": rag_pack.recommended_context_chunks,
                "primaryChunkIds": rag_pack.primary_chunk_ids,
            }),
        });

        for group in &rag_pack.groups {
            let chunk_ids = group.chunk_ids.clone();
            let role = if group.role == "primary_source" {
                "primary retrieved source"
            } else if group.role == "supporting_source" {
                "supporting retrieved source"
            } else {
                "secondary retrieved source"
            };
            let token_estimate = group
                .chunks
                .iter()
                .map(|chunk| 32u32.saturating_add(estimate_tokens(&chunk.chunk_kind)))
                .sum::<u32>()
                .max(32);
            pack.push(ContextPackItem {
                id: format!("rag-document-{}", group.document_id),
                role: ContextItemRole::Evidence,
                source: group.document_path.clone(),
                reason: role.to_string(),
                trust_level: ContextTrustLevel::RetrievedEvidence,
                token_estimate,
                priority: 0,
                stability: ContextItemStability::VolatileSuffix,
                payload: serde_json::json!({
                    "sourceId": group.source_id,
                    "sourceName": group.source_name,
                    "documentId": group.document_id,
                    "documentTitle": group.document_title,
                    "documentPath": group.document_path,
                    "chunkIds": chunk_ids,
                    "chunks": group.chunks,
                }),
            });
        }

        pack
    }

    pub fn items_by_role(&self, role: ContextItemRole) -> Vec<&ContextPackItem> {
        self.items.iter().filter(|item| item.role == role).collect()
    }

    pub fn prompt_sections(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(ContextPackItem::prompt_text)
            .map(str::to_string)
            .filter(|text| !text.trim().is_empty())
            .collect()
    }

    pub fn prompt_sections_for_stability(&self, stability: ContextItemStability) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.stability == stability)
            .filter_map(ContextPackItem::prompt_text)
            .map(str::to_string)
            .filter(|text| !text.trim().is_empty())
            .collect()
    }
}

/// Canonical context assembly gateway shared by every runtime host.
pub struct ContextAssembler {
    pack: ContextPack,
    seen_ids: HashSet<String>,
}

impl ContextAssembler {
    pub fn new(purpose: impl Into<String>, token_budget: Option<u32>) -> Self {
        Self {
            pack: ContextPack::new(purpose, token_budget),
            seen_ids: HashSet::new(),
        }
    }

    pub fn add(&mut self, item: ContextPackItem) -> Result<(), ContextAssemblyError> {
        if !self.seen_ids.insert(item.id.clone()) {
            return Err(ContextAssemblyError::DuplicateItemId { item_id: item.id });
        }
        if item
            .prompt_text()
            .is_some_and(|text| text.trim().is_empty())
        {
            return Ok(());
        }
        self.pack.items.push(item);
        Ok(())
    }

    pub fn assemble(mut self) -> ContextPack {
        self.pack.items.sort_by(|left, right| {
            left.stability
                .cmp(&right.stability)
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut selected = Vec::new();
        let mut total = 0u32;
        for item in self.pack.items {
            let next_total = total.saturating_add(item.token_estimate);
            if self
                .pack
                .token_budget
                .is_some_and(|budget| next_total > budget)
            {
                self.pack.omitted_item_ids.push(item.id);
                continue;
            }
            total = next_total;
            selected.push(item);
        }
        self.pack.items = selected;
        self.pack.total_token_estimate = total;
        self.pack
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContextAssemblyError {
    #[error("duplicate context item id {item_id}")]
    DuplicateItemId { item_id: String },
}

fn estimate_tokens(text: &str) -> u32 {
    text.split_whitespace().count().max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::{RagContextChunk, RagContextGroup};

    #[test]
    fn converts_rag_pack_into_inspectable_context_pack() {
        let rag_pack = RagContextPack {
            ordering_policy: "primary first".to_string(),
            recommended_context_chunks: 2,
            primary_chunk_ids: vec!["chunk-1".to_string()],
            context_window_chunk_ids: vec!["chunk-1".to_string()],
            supporting_chunk_ids: Vec::new(),
            groups: vec![RagContextGroup {
                role: "primary_source".to_string(),
                source_id: "source-1".to_string(),
                source_name: "Notes".to_string(),
                document_id: "doc-1".to_string(),
                document_title: "Retry notes".to_string(),
                document_path: "notes/retry.md".to_string(),
                chunk_ids: vec!["chunk-1".to_string()],
                chunks: vec![RagContextChunk {
                    chunk_id: "chunk-1".to_string(),
                    chunk_index: 0,
                    chunk_kind: "text".to_string(),
                    role: "primary_direct".to_string(),
                    score: 0.92,
                }],
            }],
        };

        let pack = ContextPack::from_rag_context_pack(&rag_pack, "answer question", Some(1200));

        assert_eq!(pack.version, CONTEXT_PACK_VERSION);
        assert_eq!(pack.items_by_role(ContextItemRole::Evidence).len(), 1);
        assert!(pack.total_token_estimate > 0);
        assert_eq!(pack.items[1].payload["documentTitle"], "Retry notes");
    }

    #[test]
    fn assembler_deduplicates_orders_and_budgets_context() {
        let mut assembler = ContextAssembler::new("agent turn", Some(4));
        assembler
            .add(ContextPackItem::text(
                "volatile-memory",
                ContextItemRole::Memory,
                "memory",
                "query memory",
                ContextTrustLevel::AgentMemory,
                10,
                ContextItemStability::VolatileSuffix,
                "one two three",
            ))
            .unwrap();
        assembler
            .add(ContextPackItem::text(
                "stable-persona",
                ContextItemRole::Instruction,
                "persona",
                "selected persona",
                ContextTrustLevel::System,
                100,
                ContextItemStability::StablePrefix,
                "system rule",
            ))
            .unwrap();

        let pack = assembler.assemble();

        assert_eq!(pack.prompt_sections(), vec!["system rule"]);
        assert_eq!(pack.omitted_item_ids, vec!["volatile-memory"]);
        assert_eq!(pack.total_token_estimate, 2);
    }

    #[test]
    fn assembler_rejects_duplicate_contributor_ids() {
        let mut assembler = ContextAssembler::new("agent turn", None);
        for _ in 0..2 {
            let result = assembler.add(ContextPackItem::text(
                "goal",
                ContextItemRole::Instruction,
                "goal",
                "active goal",
                ContextTrustLevel::UserSelected,
                80,
                ContextItemStability::VolatileSuffix,
                "finish the migration",
            ));
            if result.is_err() {
                assert_eq!(
                    result.unwrap_err(),
                    ContextAssemblyError::DuplicateItemId {
                        item_id: "goal".to_string()
                    }
                );
                return;
            }
        }
        panic!("duplicate context contributor should be rejected");
    }
}
