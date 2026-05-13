//! Inspectable context pack for prompts, evidence, memory, and tool guidance.

use serde::{Deserialize, Serialize};

use crate::rag::RagContextPack;

pub const CONTEXT_PACK_VERSION: u16 = 1;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackItem {
    pub id: String,
    pub role: ContextItemRole,
    pub source: String,
    pub reason: String,
    pub trust_level: ContextTrustLevel,
    pub token_estimate: u32,
    pub payload: serde_json::Value,
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
}

impl ContextPack {
    pub fn new(purpose: impl Into<String>, token_budget: Option<u32>) -> Self {
        Self {
            version: CONTEXT_PACK_VERSION,
            purpose: purpose.into(),
            token_budget,
            total_token_estimate: 0,
            items: Vec::new(),
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
}
