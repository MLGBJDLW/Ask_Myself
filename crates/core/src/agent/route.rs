//! Turn routing strategy for the agent runtime.

use crate::tools::ToolCategory;

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

#[derive(Debug, Clone)]
pub(crate) struct AgentRoutePlan {
    pub(crate) kind: AgentRouteKind,
    pub(crate) prompt_section: String,
    pub(crate) extra_categories: Vec<ToolCategory>,
}

fn query_looks_like_question(query: &str) -> bool {
    let q = query.to_lowercase();
    q.contains('?')
        || q.contains("what")
        || q.contains("why")
        || q.contains("how")
        || q.contains("which")
        || q.contains("where")
        || q.contains("when")
        || q.contains("who")
        || q.contains("tell me")
        || q.contains("explain")
        || q.contains("analyze")
        || q.contains("analysis")
        || q.contains("summarize")
        || q.contains("compare")
        || q.contains("分析")
        || q.contains("总结")
        || q.contains("为什么")
        || q.contains("如何")
        || q.contains("怎么")
        || q.contains("哪些")
        || q.contains("什么")
}

pub(crate) fn system_prompt_has_collection_context(system_prompt: &str) -> bool {
    system_prompt
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("## Collection Context"))
}

pub(crate) fn route_user_turn(
    query: &str,
    system_prompt: &str,
    has_sources: bool,
) -> AgentRoutePlan {
    let q = query.to_lowercase();
    let collection_context = system_prompt_has_collection_context(system_prompt);

    let code_or_tool_operation = q.contains("run_shell")
        || q.contains("run shell")
        || q.contains("shell")
        || q.contains("terminal")
        || q.contains("command")
        || q.contains("powershell")
        || q.contains("cmd")
        || q.contains("cargo")
        || q.contains("npm")
        || q.contains("pnpm")
        || q.contains("node")
        || q.contains("python")
        || q.contains("git")
        || q.contains("tool")
        || q.contains("tools")
        || q.contains("agent")
        || q.contains("subagent")
        || q.contains("unavailable")
        || q.contains("available")
        || q.contains("fix")
        || q.contains("debug")
        || q.contains("bug")
        || q.contains("test")
        || q.contains("build")
        || q.contains("compile")
        || q.contains("运行")
        || q.contains("命令")
        || q.contains("终端")
        || q.contains("调用")
        || q.contains("工具")
        || q.contains("不可用")
        || q.contains("修复")
        || q.contains("排查")
        || q.contains("测试")
        || q.contains("构建")
        || q.contains("编译")
        || q.contains("代码")
        || q.contains("项目")
        || q.contains("仓库")
        || q.contains("主agent")
        || q.contains("子agent");

    let file_operation = q.contains("file")
        || q.contains("read")
        || q.contains("edit")
        || q.contains("write")
        || q.contains("create")
        || q.contains("move")
        || q.contains("rename")
        || q.contains("copy")
        || q.contains("delete")
        || q.contains("folder")
        || q.contains("directory")
        || q.contains("document")
        || q.contains("word")
        || q.contains("docx")
        || q.contains("excel")
        || q.contains("xlsx")
        || q.contains("ppt")
        || q.contains("pptx")
        || q.contains("office")
        || q.contains("文档")
        || q.contains("文件")
        || q.contains("移动")
        || q.contains("重命名")
        || q.contains("复制")
        || q.contains("删除")
        || q.contains("幻灯片")
        || q.contains("表格")
        || code_or_tool_operation;

    let source_management = q.contains("source")
        || q.contains("index")
        || q.contains("reindex")
        || q.contains("数据源")
        || q.contains("索引");

    let web_lookup = q.contains("http")
        || q.contains("url")
        || q.contains("website")
        || q.contains("web ")
        || q.contains("网页")
        || q.contains("链接");

    let conversation_recall = q.contains("earlier")
        || q.contains("previous")
        || q.contains("before")
        || q.contains("this conversation")
        || q.contains("chat history")
        || q.contains("we discussed")
        || q.contains("刚才")
        || q.contains("之前")
        || q.contains("上面")
        || q.contains("这段对话");

    if collection_context {
        return AgentRoutePlan {
            kind: AgentRouteKind::CollectionFocused,
            prompt_section: "## Active Routing Plan\nUse the current collection and its saved evidence as your primary working set. Stay anchored to that collection first, and only widen beyond it if the collection is clearly insufficient. If you widen scope, explain why.".to_string(),
            extra_categories: vec![ToolCategory::Knowledge, ToolCategory::DocumentAnalysis],
        };
    }

    if source_management {
        return AgentRoutePlan {
            kind: AgentRouteKind::SourceManagement,
            prompt_section: "## Active Routing Plan\nThis is a source/index management request. Prefer direct, operational handling over exploratory retrieval, and avoid unnecessary long-form analysis.".to_string(),
            extra_categories: vec![ToolCategory::SourceManagement],
        };
    }

    if code_or_tool_operation {
        return AgentRoutePlan {
            kind: AgentRouteKind::CodebaseOperation,
            prompt_section: "## Active Routing Plan\nThis is a codebase or tooling request. Start with code_intelligence for named functions, types, tools, agents, or call/reference questions before broad text search. Use project_tool list/describe before ad hoc run_shell when the repository may define local lint, test, codegen, diagnostics, or validation workflows; project_tool run must include the current manifestHash from list/describe. Inspect with glob_files/search_files/read_file as needed, then modify with text-edit tools, and verify with project_tool run or focused run_shell commands when appropriate.".to_string(),
            extra_categories: vec![ToolCategory::FileSystem, ToolCategory::DocumentAnalysis],
        };
    }

    if file_operation {
        return AgentRoutePlan {
            kind: AgentRouteKind::FileOperation,
            prompt_section: "## Active Routing Plan\nThis request is file-centric. Prefer reading, comparing, generating, or editing the relevant files directly before broad knowledge-base search. For requested DOCX/XLSX/PPTX/PDF work, use run_shell + the doc-script-editor skill for Python-backed creation, validation, conversion, rendering, extraction, redaction, formula QA, template preservation, and OOXML edits. Pair Office work with docx-document-design, pptx-presentation-design, or xlsx-workbook-design as appropriate.".to_string(),
            extra_categories: vec![ToolCategory::FileSystem, ToolCategory::DocumentAnalysis],
        };
    }

    if conversation_recall {
        return AgentRoutePlan {
            kind: AgentRouteKind::ConversationRecall,
            prompt_section: "## Active Routing Plan\nThe user is asking about the current conversation context. Check the conversation history and already-available evidence first before widening to new retrieval.".to_string(),
            extra_categories: vec![ToolCategory::Knowledge, ToolCategory::DocumentAnalysis],
        };
    }

    if web_lookup {
        return AgentRoutePlan {
            kind: AgentRouteKind::WebLookup,
            prompt_section: "## Active Routing Plan\nThis request likely needs web or URL inspection. Prefer targeted fetch or MCP/web tools instead of broad local retrieval.".to_string(),
            extra_categories: vec![ToolCategory::Web],
        };
    }

    if has_sources && query_looks_like_question(query) {
        return AgentRoutePlan {
            kind: AgentRouteKind::KnowledgeRetrieval,
            prompt_section: "## Active Routing Plan\nThis is a knowledge retrieval turn. Prefer grounded retrieval, comparison, and evidence synthesis before answering. Stop once the evidence is sufficient instead of over-searching.".to_string(),
            extra_categories: vec![ToolCategory::Knowledge, ToolCategory::DocumentAnalysis],
        };
    }

    AgentRoutePlan {
        kind: AgentRouteKind::DirectResponse,
        prompt_section: "## Active Routing Plan\nAnswer the user's question directly when no specialized route applies. For factual questions about the user's indexed documents, notes, projects, memories, or knowledge base, search first using search_knowledge_base. For codebase, file, shell/tool, current-conversation, URL, or web inspection tasks, use the route-appropriate tools instead of forcing knowledge-base retrieval. Use tools whenever they would improve answer accuracy or completeness.".to_string(),
        extra_categories: Vec::new(),
    }
}
