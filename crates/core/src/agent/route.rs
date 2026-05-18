//! Turn routing strategy for the agent runtime.

use crate::tool_visibility_policy::{
    decide_tool_visibility, ToolVisibilityDecision, ToolVisibilityInput, ToolVisibilityRouteKind,
};
use crate::tools::run_shell_contract;

pub(crate) use crate::tool_visibility_policy::system_prompt_has_collection_context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRouteKind {
    DirectResponse,
    KnowledgeRetrieval,
    CollectionFocused,
    ConversationRecall,
    CodebaseOperation,
    FileOperation,
    WebLookup,
    SourceManagement,
}

impl AgentRouteKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            AgentRouteKind::DirectResponse => "DirectResponse",
            AgentRouteKind::KnowledgeRetrieval => "KnowledgeRetrieval",
            AgentRouteKind::CollectionFocused => "CollectionFocused",
            AgentRouteKind::FileOperation => "FileOperation",
            AgentRouteKind::SourceManagement => "SourceManagement",
            AgentRouteKind::ConversationRecall => "ConversationRecall",
            AgentRouteKind::CodebaseOperation => "CodebaseOperation",
            AgentRouteKind::WebLookup => "WebLookup",
        }
    }
}

impl From<ToolVisibilityRouteKind> for AgentRouteKind {
    fn from(kind: ToolVisibilityRouteKind) -> Self {
        match kind {
            ToolVisibilityRouteKind::DirectResponse => Self::DirectResponse,
            ToolVisibilityRouteKind::KnowledgeRetrieval => Self::KnowledgeRetrieval,
            ToolVisibilityRouteKind::CollectionFocused => Self::CollectionFocused,
            ToolVisibilityRouteKind::ConversationRecall => Self::ConversationRecall,
            ToolVisibilityRouteKind::CodebaseOperation => Self::CodebaseOperation,
            ToolVisibilityRouteKind::FileOperation => Self::FileOperation,
            ToolVisibilityRouteKind::WebLookup => Self::WebLookup,
            ToolVisibilityRouteKind::SourceManagement => Self::SourceManagement,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentRoutePlan {
    pub(crate) kind: AgentRouteKind,
    pub(crate) prompt_section: String,
    pub(crate) visibility_decision: ToolVisibilityDecision,
}

pub(crate) fn route_user_turn(
    query: &str,
    system_prompt: &str,
    has_sources: bool,
) -> AgentRoutePlan {
    let visibility_decision = decide_tool_visibility(ToolVisibilityInput {
        query,
        system_prompt,
        has_sources,
    });
    let kind = AgentRouteKind::from(visibility_decision.route);
    let prompt_section = prompt_section_for_route(kind);

    AgentRoutePlan {
        kind,
        prompt_section,
        visibility_decision,
    }
}

fn prompt_section_for_route(kind: AgentRouteKind) -> String {
    match kind {
        AgentRouteKind::CollectionFocused => "## Active Routing Plan\nUse the current collection and its saved evidence as your primary working set. Stay anchored to that collection first, and only widen beyond it if the collection is clearly insufficient. If you widen scope, explain why.".to_string(),
        AgentRouteKind::SourceManagement => "## Active Routing Plan\nThis is a source/index management request. Prefer direct, operational handling over exploratory retrieval, and avoid unnecessary long-form analysis.".to_string(),
        AgentRouteKind::CodebaseOperation => format!(
            "## Active Routing Plan\nThis is a codebase or tooling request. Start with code_intelligence for named functions, types, tools, agents, or call/reference questions before broad text search. {} Inspect with glob_files/search_files/read_file as needed, then modify with text-edit tools, and verify with project_tool run or focused run_shell commands when appropriate.",
            run_shell_contract::route_guidance()
        ),
        AgentRouteKind::FileOperation => "## Active Routing Plan\nThis request is file-centric. Prefer reading, comparing, generating, or editing the relevant files directly before broad knowledge-base search. For requested DOCX/XLSX/PPTX/PDF work, use run_shell + the doc-script-editor skill for Python-backed creation, validation, conversion, rendering, extraction, redaction, formula QA, template preservation, and OOXML edits. Pair Office work with docx-document-design, pptx-presentation-design, or xlsx-workbook-design as appropriate.".to_string(),
        AgentRouteKind::ConversationRecall => "## Active Routing Plan\nThe user is asking about the current conversation context. Check the conversation history and already-available evidence first before widening to new retrieval.".to_string(),
        AgentRouteKind::WebLookup => "## Active Routing Plan\nThis request likely needs web or URL inspection. Prefer targeted fetch or MCP/web tools instead of broad local retrieval.".to_string(),
        AgentRouteKind::KnowledgeRetrieval => "## Active Routing Plan\nThis is a knowledge retrieval turn. Prefer grounded retrieval, comparison, and evidence synthesis before answering. Stop once the evidence is sufficient instead of over-searching.".to_string(),
        AgentRouteKind::DirectResponse => "## Active Routing Plan\nAnswer the user's question directly when no specialized route applies. For factual questions about the user's indexed documents, notes, projects, memories, or knowledge base, search first using search_knowledge_base. For codebase, file, shell/tool, current-conversation, URL, or web inspection tasks, use the route-appropriate tools instead of forcing knowledge-base retrieval. Use tools whenever they would improve answer accuracy or completeness.".to_string(),
    }
}
