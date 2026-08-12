//! Typed routing and tool-visibility policy.
//!
//! Route selection and dynamic tool visibility both consume the same decision
//! so prompt routing and offered tools cannot drift independently.

use serde::{Deserialize, Serialize};

use crate::tools::ToolCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolVisibilityRouteKind {
    DirectResponse,
    KnowledgeRetrieval,
    CollectionFocused,
    ConversationRecall,
    CodebaseOperation,
    FileOperation,
    WebLookup,
    SourceManagement,
}

impl ToolVisibilityRouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectResponse => "DirectResponse",
            Self::KnowledgeRetrieval => "KnowledgeRetrieval",
            Self::CollectionFocused => "CollectionFocused",
            Self::ConversationRecall => "ConversationRecall",
            Self::CodebaseOperation => "CodebaseOperation",
            Self::FileOperation => "FileOperation",
            Self::WebLookup => "WebLookup",
            Self::SourceManagement => "SourceManagement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolVisibilitySignalKind {
    CollectionContext,
    Question,
    CodeOrToolOperation,
    FileOperation,
    FileWorkspace,
    SourceManagement,
    WebLookup,
    ConversationRecall,
    KnowledgeWork,
    Automation,
    Process,
    Terminal,
    Browser,
    Desktop,
    DocumentAnalysis,
    LinkedSources,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilitySignal {
    pub kind: ToolVisibilitySignalKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolVisibilityEffect {
    MatchedSignal { signal: ToolVisibilitySignalKind },
    ActivatedCategory { category: ToolCategory },
    SelectedRoute { route: ToolVisibilityRouteKind },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilityDecisionLogEntry {
    pub rule_id: String,
    pub effect: ToolVisibilityEffect,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilityDecision {
    pub route: ToolVisibilityRouteKind,
    pub active_categories: Vec<ToolCategory>,
    pub route_categories: Vec<ToolCategory>,
    pub signals: Vec<ToolVisibilitySignal>,
    pub log: Vec<ToolVisibilityDecisionLogEntry>,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolVisibilityInput<'a> {
    pub query: &'a str,
    pub system_prompt: &'a str,
    pub has_sources: bool,
}

pub fn decide_tool_visibility(input: ToolVisibilityInput<'_>) -> ToolVisibilityDecision {
    let query = input.query.to_lowercase();
    let collection_context = system_prompt_has_collection_context(input.system_prompt);
    let mut signals = Vec::new();
    let mut log = Vec::new();

    if collection_context {
        push_signal(
            &mut signals,
            &mut log,
            "signal.collection_context",
            ToolVisibilitySignalKind::CollectionContext,
            Vec::new(),
            "system prompt contains an active collection context block",
        );
    }
    push_term_signal(
        &query,
        QUESTION_TERMS,
        &mut signals,
        &mut log,
        "signal.question",
        ToolVisibilitySignalKind::Question,
        "query asks for explanation, analysis, comparison, or summary",
    );
    push_term_signal(
        &query,
        CODE_OR_TOOL_TERMS,
        &mut signals,
        &mut log,
        "signal.code_or_tool",
        ToolVisibilitySignalKind::CodeOrToolOperation,
        "query mentions code, tooling, shell, diagnostics, or agent operation",
    );
    push_term_signal(
        &query,
        FILE_ROUTE_TERMS,
        &mut signals,
        &mut log,
        "signal.file_operation",
        ToolVisibilitySignalKind::FileOperation,
        "query mentions file, directory, document, or edit operations",
    );
    push_term_signal(
        &query,
        FILE_WORKSPACE_TERMS,
        &mut signals,
        &mut log,
        "signal.file_workspace",
        ToolVisibilitySignalKind::FileWorkspace,
        "query mentions file-workspace tools such as search, notes, replacement, or grep",
    );
    push_term_signal(
        &query,
        SOURCE_TERMS,
        &mut signals,
        &mut log,
        "signal.source_management",
        ToolVisibilitySignalKind::SourceManagement,
        "query mentions source indexing or reindexing",
    );
    push_term_signal(
        &query,
        WEB_ROUTE_TERMS,
        &mut signals,
        &mut log,
        "signal.web_lookup",
        ToolVisibilitySignalKind::WebLookup,
        "query mentions URL or web inspection",
    );
    push_term_signal(
        &query,
        CONVERSATION_RECALL_TERMS,
        &mut signals,
        &mut log,
        "signal.conversation_recall",
        ToolVisibilitySignalKind::ConversationRecall,
        "query asks about earlier conversation context",
    );
    push_term_signal(
        &query,
        KNOWLEDGE_TERMS,
        &mut signals,
        &mut log,
        "signal.knowledge",
        ToolVisibilitySignalKind::KnowledgeWork,
        "query mentions memory, evidence, playbooks, collections, skills, or knowledge artifacts",
    );
    push_term_signal(
        &query,
        AUTOMATION_TERMS,
        &mut signals,
        &mut log,
        "signal.automation",
        ToolVisibilitySignalKind::Automation,
        "query explicitly requests opening or revealing a local path",
    );
    push_term_signal(
        &query,
        PROCESS_TERMS,
        &mut signals,
        &mut log,
        "signal.process",
        ToolVisibilitySignalKind::Process,
        "query mentions command execution, builds, tests, or local services",
    );
    push_term_signal(
        &query,
        TERMINAL_TERMS,
        &mut signals,
        &mut log,
        "signal.terminal",
        ToolVisibilitySignalKind::Terminal,
        "query refers to the user-visible terminal or an interactive shell",
    );
    push_term_signal(
        &query,
        BROWSER_TERMS,
        &mut signals,
        &mut log,
        "signal.browser",
        ToolVisibilitySignalKind::Browser,
        "query refers to browser inspection, interaction, or a local web app",
    );
    if !has_signal(&signals, ToolVisibilitySignalKind::Browser)
        && query_has_web_navigation_handoff(&query)
    {
        push_signal(
            &mut signals,
            &mut log,
            "signal.browser_navigation_handoff",
            ToolVisibilitySignalKind::Browser,
            vec!["navigation intent + web target".to_string()],
            "explicit web navigation belongs to the shared browser session",
        );
    }
    if !has_signal(&signals, ToolVisibilitySignalKind::Automation)
        && query_has_local_path_handoff(&query)
    {
        push_signal(
            &mut signals,
            &mut log,
            "signal.local_path_handoff",
            ToolVisibilitySignalKind::Automation,
            vec!["open/reveal intent + local path".to_string()],
            "an explicit local path handoff needs the visible desktop opener",
        );
    }
    push_term_signal(
        &query,
        DESKTOP_TERMS,
        &mut signals,
        &mut log,
        "signal.desktop",
        ToolVisibilitySignalKind::Desktop,
        "query refers to a native desktop window or input action",
    );
    push_term_signal(
        &query,
        DOCUMENT_ANALYSIS_TERMS,
        &mut signals,
        &mut log,
        "signal.document_analysis",
        ToolVisibilitySignalKind::DocumentAnalysis,
        "query mentions document analysis, comparison, statistics, citations, or summaries",
    );
    if input.has_sources {
        push_signal(
            &mut signals,
            &mut log,
            "signal.linked_sources",
            ToolVisibilitySignalKind::LinkedSources,
            Vec::new(),
            "turn has an active source scope",
        );
    }

    let route = select_route(input.has_sources, &signals);
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: "route.select".to_string(),
        effect: ToolVisibilityEffect::SelectedRoute { route },
        reason: route_reason(route).to_string(),
        matched_terms: Vec::new(),
    });

    let mut active_categories = Vec::new();
    activate_category(
        &mut active_categories,
        &mut log,
        "category.always_core",
        ToolCategory::Core,
        "core tools are always visible",
    );
    if has_signal(&signals, ToolVisibilitySignalKind::CodeOrToolOperation)
        || has_signal(&signals, ToolVisibilitySignalKind::FileOperation)
        || has_signal(&signals, ToolVisibilitySignalKind::FileWorkspace)
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.filesystem",
            ToolCategory::FileSystem,
            "code/tool/file signals need filesystem and shell-capable tools",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::SourceManagement) || input.has_sources {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.source_management",
            ToolCategory::SourceManagement,
            if input.has_sources {
                "linked sources make source management tools relevant"
            } else {
                "source-management signal matched"
            },
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::KnowledgeWork)
        || (input.has_sources && has_signal(&signals, ToolVisibilitySignalKind::Question))
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.knowledge",
            ToolCategory::Knowledge,
            "knowledge or sourced-question signals need retrieval and evidence tools",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::WebLookup) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.web",
            ToolCategory::Web,
            "web lookup signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Automation) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.automation",
            ToolCategory::Automation,
            "local path handoff signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Process) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.process",
            ToolCategory::Process,
            "process signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Terminal) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.terminal",
            ToolCategory::Terminal,
            "terminal signal matched",
        );
        activate_category(
            &mut active_categories,
            &mut log,
            "category.terminal_process",
            ToolCategory::Process,
            "terminal work may require starting or inspecting a process",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Browser) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.browser_read",
            ToolCategory::BrowserRead,
            "browser signal needs rendered-page observation",
        );
        activate_category(
            &mut active_categories,
            &mut log,
            "category.browser_interact",
            ToolCategory::BrowserInteract,
            "browser signal may require stateful page interaction",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Desktop) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.desktop_interact",
            ToolCategory::DesktopInteract,
            "desktop signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::DocumentAnalysis)
        || (input.has_sources && has_signal(&signals, ToolVisibilitySignalKind::Question))
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.document_analysis",
            ToolCategory::DocumentAnalysis,
            "document-analysis or sourced-question signals need comparison and summary tools",
        );
    }
    if has_subagent_relevance_signal(&signals) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.subagent",
            ToolCategory::SubAgent,
            "complex, agent, or workflow signals can require delegated work tools",
        );
    }

    let route_categories = route_categories(route);
    for category in &route_categories {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.route_requirement",
            *category,
            "selected route requires this tool category",
        );
    }

    ToolVisibilityDecision {
        route,
        active_categories,
        route_categories,
        signals,
        log,
    }
}

pub fn system_prompt_has_collection_context(system_prompt: &str) -> bool {
    system_prompt
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("## Collection Context"))
}

fn select_route(has_sources: bool, signals: &[ToolVisibilitySignal]) -> ToolVisibilityRouteKind {
    if has_signal(signals, ToolVisibilitySignalKind::CollectionContext) {
        return ToolVisibilityRouteKind::CollectionFocused;
    }
    if has_signal(signals, ToolVisibilitySignalKind::SourceManagement) {
        return ToolVisibilityRouteKind::SourceManagement;
    }
    if has_signal(signals, ToolVisibilitySignalKind::CodeOrToolOperation)
        || has_signal(signals, ToolVisibilitySignalKind::Process)
        || has_signal(signals, ToolVisibilitySignalKind::Terminal)
    {
        return ToolVisibilityRouteKind::CodebaseOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::FileOperation) {
        return ToolVisibilityRouteKind::FileOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::ConversationRecall) {
        return ToolVisibilityRouteKind::ConversationRecall;
    }
    if has_signal(signals, ToolVisibilitySignalKind::Browser) {
        return ToolVisibilityRouteKind::WebLookup;
    }
    if has_signal(signals, ToolVisibilitySignalKind::WebLookup) {
        return ToolVisibilityRouteKind::WebLookup;
    }
    if has_sources && has_signal(signals, ToolVisibilitySignalKind::Question) {
        return ToolVisibilityRouteKind::KnowledgeRetrieval;
    }
    ToolVisibilityRouteKind::DirectResponse
}

fn route_categories(route: ToolVisibilityRouteKind) -> Vec<ToolCategory> {
    match route {
        ToolVisibilityRouteKind::CollectionFocused
        | ToolVisibilityRouteKind::ConversationRecall
        | ToolVisibilityRouteKind::KnowledgeRetrieval => {
            vec![ToolCategory::Knowledge, ToolCategory::DocumentAnalysis]
        }
        ToolVisibilityRouteKind::SourceManagement => vec![ToolCategory::SourceManagement],
        ToolVisibilityRouteKind::CodebaseOperation | ToolVisibilityRouteKind::FileOperation => {
            vec![
                ToolCategory::FileSystem,
                ToolCategory::Process,
                ToolCategory::DocumentAnalysis,
            ]
        }
        ToolVisibilityRouteKind::WebLookup => vec![ToolCategory::Web],
        ToolVisibilityRouteKind::DirectResponse => Vec::new(),
    }
}

fn route_reason(route: ToolVisibilityRouteKind) -> &'static str {
    match route {
        ToolVisibilityRouteKind::CollectionFocused => {
            "collection context has highest priority over query text"
        }
        ToolVisibilityRouteKind::SourceManagement => {
            "source/index operations should be handled directly"
        }
        ToolVisibilityRouteKind::CodebaseOperation => {
            "code or tool operations need codebase tooling and verification"
        }
        ToolVisibilityRouteKind::FileOperation => {
            "file/document operations need filesystem-oriented tools"
        }
        ToolVisibilityRouteKind::ConversationRecall => {
            "conversation recall should inspect current conversation context first"
        }
        ToolVisibilityRouteKind::WebLookup => "URL or web requests need web tools",
        ToolVisibilityRouteKind::KnowledgeRetrieval => {
            "sourced question needs grounded retrieval and evidence synthesis"
        }
        ToolVisibilityRouteKind::DirectResponse => {
            "no specialized policy signal requires a route-specific tool set"
        }
    }
}

fn push_term_signal(
    query: &str,
    terms: &[&str],
    signals: &mut Vec<ToolVisibilitySignal>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    kind: ToolVisibilitySignalKind,
    reason: &str,
) {
    let matched_terms = terms
        .iter()
        .filter(|term| query.contains(**term))
        .map(|term| (*term).to_string())
        .collect::<Vec<_>>();
    if matched_terms.is_empty() {
        return;
    }
    push_signal(signals, log, rule_id, kind, matched_terms, reason);
}

fn push_signal(
    signals: &mut Vec<ToolVisibilitySignal>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    kind: ToolVisibilitySignalKind,
    matched_terms: Vec<String>,
    reason: &str,
) {
    signals.push(ToolVisibilitySignal {
        kind,
        matched_terms: matched_terms.clone(),
    });
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: rule_id.to_string(),
        effect: ToolVisibilityEffect::MatchedSignal { signal: kind },
        reason: reason.to_string(),
        matched_terms,
    });
}

fn activate_category(
    categories: &mut Vec<ToolCategory>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    category: ToolCategory,
    reason: &str,
) {
    if categories.contains(&category) {
        return;
    }
    categories.push(category);
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: rule_id.to_string(),
        effect: ToolVisibilityEffect::ActivatedCategory { category },
        reason: reason.to_string(),
        matched_terms: Vec::new(),
    });
}

fn has_signal(signals: &[ToolVisibilitySignal], kind: ToolVisibilitySignalKind) -> bool {
    signals.iter().any(|signal| signal.kind == kind)
}

fn query_has_web_navigation_handoff(query: &str) -> bool {
    contains_any(query, NAVIGATION_INTENT_TERMS)
        && !query_has_local_path_target(query)
        && (contains_any(query, WEB_NAVIGATION_TARGET_TERMS) || query_has_explicit_url(query))
}

fn query_has_local_path_handoff(query: &str) -> bool {
    contains_any(query, LOCAL_PATH_HANDOFF_INTENT_TERMS) && query_has_local_path_target(query)
}

fn query_has_local_path_target(query: &str) -> bool {
    query
        .split_whitespace()
        .map(trim_handoff_token)
        .any(looks_like_local_path)
}

fn contains_any(query: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| query.contains(*term))
}

fn query_has_explicit_url(query: &str) -> bool {
    query
        .split_whitespace()
        .map(trim_handoff_token)
        .any(|token| token.starts_with("http://") || token.starts_with("https://"))
}

fn trim_handoff_token(token: &str) -> &str {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
    });
    token.trim_end_matches(|character: char| matches!(character, '.' | '?' | '!'))
}

fn looks_like_local_path(token: &str) -> bool {
    if token.is_empty()
        || token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("www.")
    {
        return false;
    }
    let bytes = token.as_bytes();
    let has_drive_prefix = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if has_drive_prefix
        || token.starts_with('/')
        || token.starts_with('\\')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.starts_with("~\\")
        || token.contains('/')
        || token.contains('\\')
    {
        return true;
    }
    token
        .rsplit_once('.')
        .map(|(_, extension)| LOCAL_FILE_EXTENSIONS.contains(&extension))
        .unwrap_or(false)
}

fn has_subagent_relevance_signal(signals: &[ToolVisibilitySignal]) -> bool {
    const SUBAGENT_RELEVANCE_TERMS: &[&str] = &[
        "agent",
        "agents",
        "agentic",
        "subagent",
        "workflow",
        "complex",
        "multi-step",
        "multi step",
        "architecture",
        "review",
        "compare",
        "comparison",
        "research",
        "verify",
        "verification",
        "critique",
        "并行",
        "复杂",
        "研究",
        "验证",
        "审查",
        "评审",
        "对比",
        "比较",
        "架构",
        "主agent",
        "子agent",
    ];

    signals.iter().any(|signal| {
        matches!(
            signal.kind,
            ToolVisibilitySignalKind::CodeOrToolOperation
                | ToolVisibilitySignalKind::KnowledgeWork
                | ToolVisibilitySignalKind::DocumentAnalysis
                | ToolVisibilitySignalKind::WebLookup
        ) && signal
            .matched_terms
            .iter()
            .any(|term| SUBAGENT_RELEVANCE_TERMS.contains(&term.as_str()))
    })
}

const QUESTION_TERMS: &[&str] = &[
    "?",
    "what",
    "why",
    "how",
    "which",
    "where",
    "when",
    "who",
    "tell me",
    "explain",
    "analyze",
    "analysis",
    "summarize",
    "compare",
    "review",
    "分析",
    "总结",
    "为什么",
    "如何",
    "怎么",
    "哪些",
    "什么",
    "解释",
    "帮我看",
    "看一下",
    "到底",
    "还有哪些",
    "还有多少",
];

const CODE_OR_TOOL_TERMS: &[&str] = &[
    "run_shell",
    "run shell",
    "shell",
    "terminal",
    "command",
    "powershell",
    "cmd",
    "cargo",
    "npm",
    "pnpm",
    "node",
    "python",
    "git",
    "tool",
    "tools",
    "agent",
    "agents",
    "subagent",
    "claude",
    "deepseek",
    "top_agents",
    "top agents",
    "cli",
    "ide",
    "extension",
    "eval",
    "evaluation",
    "benchmark",
    "accuracy",
    "hitrate",
    "hit rate",
    "unavailable",
    "available",
    "fix",
    "debug",
    "bug",
    "test",
    "build",
    "compile",
    "运行",
    "命令",
    "终端",
    "调用",
    "工具",
    "不可用",
    "修复",
    "排查",
    "测试",
    "构建",
    "编译",
    "代码",
    "代码细节",
    "架构",
    "架构设计",
    "模型",
    "路由",
    "技能",
    "命中",
    "命中率",
    "命中效率",
    "准确率",
    "召回率",
    "评测",
    "基准",
    "项目",
    "仓库",
    "主agent",
    "子agent",
];

const FILE_ROUTE_TERMS: &[&str] = &[
    "file",
    "read",
    "edit",
    "write",
    "create",
    "move",
    "rename",
    "copy",
    "delete",
    "directory",
    "folder",
    "document",
    "word",
    "docx",
    "excel",
    "xlsx",
    "ppt",
    "pptx",
    "office",
    "文件",
    "读取",
    "编辑",
    "写",
    "写入",
    "新建",
    "创建",
    "保存",
    "修改",
    "移动",
    "重命名",
    "复制",
    "删除",
    "目录",
    "文档",
    "幻灯片",
    "表格",
];

const FILE_WORKSPACE_TERMS: &[&str] = &[
    "file",
    "read",
    "edit",
    "replace",
    "write",
    "create",
    "find",
    "grep",
    "rg",
    "move",
    "rename",
    "copy",
    "delete",
    "directory",
    "folder",
    "note",
    "document",
    "word",
    "docx",
    "excel",
    "xlsx",
    "ppt",
    "pptx",
    "office",
    "文件",
    "读取",
    "编辑",
    "写",
    "写入",
    "新建",
    "创建",
    "保存",
    "修改",
    "改写",
    "润色",
    "替换",
    "查找",
    "搜索文件",
    "移动",
    "重命名",
    "复制",
    "删除",
    "目录",
    "笔记",
    "文档",
    "幻灯片",
    "表格",
];

const SOURCE_TERMS: &[&str] = &["source", "index", "reindex", "数据源", "索引"];

const WEB_ROUTE_TERMS: &[&str] = &[
    "url",
    "http",
    "website",
    "web ",
    "web search",
    "search online",
    "internet",
    "fetch",
    "link",
    "网页",
    "网页搜索",
    "搜索网页",
    "联网",
    "网上",
    "链接",
];

const CONVERSATION_RECALL_TERMS: &[&str] = &[
    "earlier",
    "previous",
    "before",
    "this conversation",
    "chat history",
    "we discussed",
    "刚才",
    "之前",
    "上面",
    "这段对话",
];

const KNOWLEDGE_TERMS: &[&str] = &[
    "remember",
    "memory",
    "session",
    "history",
    "harness",
    "evolution",
    "evolve",
    "playbook",
    "collection",
    "collections",
    "citation",
    "citations",
    "evidence",
    "saved",
    "bookmark",
    "skill",
    "agent",
    "agentic",
    "workflow",
    "complex",
    "multi-step",
    "multi step",
    "research",
    "compile",
    "compilation",
    "entity",
    "entities",
    "graph",
    "knowledge",
    "health",
    "archive",
    "wiki",
    "concept",
    "concepts",
    "收藏",
    "引用",
    "证据",
    "记住",
    "记忆",
    "会话",
    "历史",
    "进化",
    "自我",
    "编译",
    "实体",
    "图谱",
    "知识",
    "健康",
    "归档",
    "概念",
    "技能",
    "研究",
    "命中",
    "命中率",
    "命中效率",
    "准确率",
    "评测",
    "基准",
];

const AUTOMATION_TERMS: &[&str] = &[
    "open path",
    "open file",
    "open folder",
    "open this path",
    "open this file",
    "open this folder",
    "reveal path",
    "reveal file",
    "reveal folder",
    "reveal this path",
    "reveal this file",
    "reveal this folder",
    "show in explorer",
    "show in finder",
    "打开路径",
    "打开文件",
    "打开文件夹",
    "打开这个路径",
    "打开这个文件",
    "打开这个文件夹",
    "定位路径",
    "定位文件",
    "定位文件夹",
    "定位这个路径",
    "定位这个文件",
    "定位这个文件夹",
];

const PROCESS_TERMS: &[&str] = &[
    "run_shell",
    "run shell",
    "shell",
    "command",
    "powershell",
    "pwsh",
    "cmd",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "python",
    "build",
    "compile",
    "test",
    "dev server",
    "local server",
    "运行",
    "命令",
    "构建",
    "编译",
    "测试",
    "本地服务",
];

const TERMINAL_TERMS: &[&str] = &[
    "terminal",
    "powershell",
    "pwsh",
    "command prompt",
    "cmd",
    "interactive shell",
    "终端",
    "命令行",
];

const BROWSER_TERMS: &[&str] = &[
    "browser",
    "browser session",
    "localhost",
    "local app",
    "local page",
    "dev server",
    "浏览器",
    "本地页面",
    "本地网页",
];

const NAVIGATION_INTENT_TERMS: &[&str] = &[
    "open",
    "visit",
    "navigate",
    "go to",
    "browse to",
    "launch",
    "打开",
    "访问",
    "前往",
    "转到",
    "导航到",
];

const WEB_NAVIGATION_TARGET_TERMS: &[&str] = &[
    "url", "website", "web site", "webpage", "web page", "link", "网址", "网站", "网页", "链接",
];

const LOCAL_PATH_HANDOFF_INTENT_TERMS: &[&str] = &[
    "open",
    "reveal",
    "show in explorer",
    "show in finder",
    "locate",
    "打开",
    "显示",
    "定位",
];

const LOCAL_FILE_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "jsonl", "yaml", "yml", "toml", "xml", "csv", "tsv", "log", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "rtf", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs",
    "py", "go", "java", "kt", "kts", "c", "h", "cpp", "hpp", "cs", "swift", "sh", "ps1", "bat",
    "cmd", "html", "htm", "css", "scss", "sql", "db", "sqlite", "png", "jpg", "jpeg", "gif",
    "webp", "svg", "mp3", "wav", "m4a", "mp4", "mov", "avi", "zip", "tar", "gz", "7z", "rar",
];

const DESKTOP_TERMS: &[&str] = &[
    "desktop",
    "computer use",
    "window",
    "screenshot",
    "mouse",
    "keyboard",
    "桌面",
    "电脑操作",
    "窗口",
    "截图",
    "鼠标",
    "键盘",
];

const DOCUMENT_ANALYSIS_TERMS: &[&str] = &[
    "compare",
    "document",
    "image",
    "screenshot",
    "ocr",
    "summarize",
    "summary",
    "analyze",
    "analysis",
    "review",
    "verify",
    "verification",
    "critique",
    "evidence",
    "citation",
    "statistics",
    "stats",
    "info",
    "vision",
    "分析",
    "总结",
    "审查",
    "评审",
    "验证",
    "引用",
    "文档",
    "图片",
    "图像",
    "截图",
    "ocr",
    "文字识别",
    "识别图片",
    "比较",
    "统计",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codebase_policy_selects_route_categories_and_log() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "debug the agent routing bug and run cargo test",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::DocumentAnalysis));
        assert!(decision.log.iter().any(|entry| matches!(
            entry.effect,
            ToolVisibilityEffect::SelectedRoute {
                route: ToolVisibilityRouteKind::CodebaseOperation
            }
        )));
    }

    #[test]
    fn terminal_inspection_activates_terminal_and_process_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "看看 terminal 里面刚才的报错",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::Terminal));
        assert!(decision.active_categories.contains(&ToolCategory::Process));
        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
    }

    #[test]
    fn browser_inspection_activates_read_and_interactive_browser_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "用 browser 检查一下这个页面",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserRead));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert_eq!(decision.route, ToolVisibilityRouteKind::WebLookup);
    }

    #[test]
    fn browser_tasks_do_not_activate_retired_desktop_handoff_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Open this website in my browser and click Sign in",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert!(!decision
            .active_categories
            .contains(&ToolCategory::Automation));
    }

    #[test]
    fn explicit_web_navigation_requests_activate_browser_session_capabilities() {
        for query in [
            "Open https://example.com",
            "Open this website",
            "Go to https://example.com",
        ] {
            let decision = decide_tool_visibility(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(decision.active_categories.contains(&ToolCategory::Web));
            assert!(decision
                .active_categories
                .contains(&ToolCategory::BrowserRead));
            assert!(decision
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
            assert!(!decision
                .active_categories
                .contains(&ToolCategory::Automation));
        }
    }

    #[test]
    fn concrete_local_paths_activate_desktop_automation_capability() {
        for query in [
            "Open notes.txt",
            "Reveal /source/report.pdf",
            "Open link.txt",
        ] {
            let decision = decide_tool_visibility(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(decision
                .active_categories
                .contains(&ToolCategory::Automation));
            assert!(!decision
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
        }
    }

    #[test]
    fn local_path_handoffs_activate_desktop_automation_capability() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Reveal this file in Explorer",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::Automation));
        assert!(!decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
    }

    #[test]
    fn local_dev_server_activates_process_and_interactive_browser_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "启动 dev server 并检查 localhost 页面",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::Process));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
    }

    #[test]
    fn direct_response_keeps_mutation_categories_hidden() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Tell me a quick joke.",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::DirectResponse);
        assert_eq!(decision.active_categories, vec![ToolCategory::Core]);
    }

    #[test]
    fn collection_context_is_structured_signal_not_persona_text() {
        let persona = decide_tool_visibility(ToolVisibilityInput {
            query: "Say hello.",
            system_prompt: "## Active Persona\nPrefer saved evidence when it exists.",
            has_sources: false,
        });
        let collection = decide_tool_visibility(ToolVisibilityInput {
            query: "Say hello.",
            system_prompt: "## Collection Context\nSaved evidence: chunk-a",
            has_sources: false,
        });

        assert_eq!(persona.route, ToolVisibilityRouteKind::DirectResponse);
        assert_eq!(collection.route, ToolVisibilityRouteKind::CollectionFocused);
        assert!(collection
            .signals
            .iter()
            .any(|signal| { signal.kind == ToolVisibilitySignalKind::CollectionContext }));
    }

    #[test]
    fn chinese_agent_hit_rate_query_selects_codebase_route() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "使用 DeepSeek 的情况下命中效率只有 85%，参考 Claude 顶级 agents 看看架构设计和代码细节。",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
        assert!(decision.signals.iter().any(|signal| {
            signal.kind == ToolVisibilitySignalKind::CodeOrToolOperation
                && signal
                    .matched_terms
                    .iter()
                    .any(|term| term == "deepseek" || term == "命中效率")
        }));
    }

    #[test]
    fn workflow_signal_activates_subagent_category() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Plan a complex workflow for this research task.",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::SubAgent));
    }

    #[test]
    fn chinese_write_or_modify_query_selects_filesystem_tools() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "帮我修改这个文件并保存。",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::FileOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
    }
}
