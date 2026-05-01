//! Tool system — trait, registry, and built-in tools for the agent framework.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool categories for dynamic visibility
// ---------------------------------------------------------------------------

/// Logical category for grouping tools. Used by [`ToolRegistry::select_tools`]
/// to decide which tool definitions are sent to the LLM on a given turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    /// Always available: search, done, list_sources, etc.
    Core,
    /// File operations: read_file, edit_file, list_dir, write_note
    FileSystem,
    /// Source management: manage_source, reindex_document
    SourceManagement,
    /// Knowledge / playbook / memory tools
    Knowledge,
    /// URL fetching
    Web,
    /// Detailed document inspection & comparison
    DocumentAnalysis,
    /// Subagent / multi-agent tools
    SubAgent,
    /// MCP: dynamically added MCP tools
    Mcp,
    /// Controlled browser/desktop handoff actions
    Automation,
}

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolDefinition;
use crate::models::Source;

// ---------------------------------------------------------------------------
// Shared tool-definition helper (parsed from JSON once via OnceLock)
// ---------------------------------------------------------------------------

/// Cached tool definition loaded from a JSON file at compile time.
pub(crate) struct ToolDef {
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDef {
    /// Parse a tool-definition JSON blob (`include_str!` output) exactly once.
    pub fn from_json<'a>(lock: &'a OnceLock<ToolDef>, json_str: &str) -> &'a ToolDef {
        lock.get_or_init(|| {
            let v: serde_json::Value =
                serde_json::from_str(json_str).expect("invalid tool definition JSON");
            ToolDef {
                description: v["description"]
                    .as_str()
                    .expect("tool JSON missing 'description'")
                    .to_string(),
                parameters: v["parameters"].clone(),
            }
        })
    }
}

pub mod agent_memory_tool;
pub mod archive_output_tool;
pub mod chunk_context_tool;
pub mod compare_tool;
pub mod compile_tool;
pub mod create_file_tool;
pub mod date_search_tool;
pub mod desktop_automation_tool;
pub mod document_info_tool;
pub mod document_utils;
pub mod edit_document_tool;
pub mod edit_file_tool;
pub mod fetch_url_tool;
pub mod file_tool;
pub mod harness_dry_run_tool;
pub mod health_check_tool;
pub mod knowledge_graph_tool;
pub mod list_dir_tool;
pub mod list_documents_tool;
pub mod list_sources_tool;
pub mod manage_skill_tool;
pub mod manage_source_tool;
pub mod mcp_tool;
pub mod path_utils;
pub mod playbook_tool;
pub mod prepare_document_tools_tool;
pub mod read_files_tool;
pub mod record_verification_tool;
pub mod reindex_tool;
pub mod related_concepts_tool;
pub mod run_shell_tool;
pub mod scratchpad_tool;
pub mod search_playbooks_tool;
pub mod search_tool;
pub mod session_search_tool;
pub mod statistics_tool;
pub mod submit_feedback_tool;
pub mod summarize_tool;
pub mod update_plan_tool;
pub mod write_note_tool;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Result returned by a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    pub artifacts: Option<serde_json::Value>,
}

/// Trust metadata attached to tool artifacts that may be injected into model
/// context or shown in the UI. Retrieved content is normally evidence, not
/// instruction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustBoundary {
    pub origin: String,
    pub authority: String,
    pub visibility: String,
    pub mutability: String,
    pub externality: String,
    pub can_instruct: bool,
}

impl TrustBoundary {
    pub fn local_source_evidence(scope_active: bool) -> Self {
        Self {
            origin: "local_source".to_string(),
            authority: "evidence".to_string(),
            visibility: if scope_active {
                "source_scope".to_string()
            } else {
                "workspace".to_string()
            },
            mutability: "read_only".to_string(),
            externality: "local".to_string(),
            can_instruct: false,
        }
    }

    pub fn tool_error() -> Self {
        Self {
            origin: "tool".to_string(),
            authority: "observation".to_string(),
            visibility: "current_chat".to_string(),
            mutability: "read_only".to_string(),
            externality: "local".to_string(),
            can_instruct: false,
        }
    }
}

/// Structured, retryable error payload for model-facing tool failures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolContractError {
    pub kind: String,
    pub code: String,
    pub message: String,
    pub expected_format: serde_json::Value,
    pub retryable: bool,
    pub trust_boundary: TrustBoundary,
}

pub(crate) fn structured_tool_error_result(
    call_id: &str,
    code: impl Into<String>,
    message: impl Into<String>,
    expected_format: serde_json::Value,
    retryable: bool,
) -> ToolResult {
    let code = code.into();
    let message = message.into();
    let error = ToolContractError {
        kind: "toolContractError".to_string(),
        code: code.clone(),
        message: message.clone(),
        expected_format,
        retryable,
        trust_boundary: TrustBoundary::tool_error(),
    };
    let content = format!(
        "Error: {message}\n\nCode: {code}\nRetryable: {retryable}\nUse the expected JSON shape shown in artifacts.expectedFormat before calling the tool again."
    );

    ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: true,
        artifacts: serde_json::to_value(error).ok(),
    }
}

pub(crate) fn tool_contract_error_result(
    call_id: &str,
    code: impl Into<String>,
    message: impl Into<String>,
    expected_format: serde_json::Value,
) -> ToolResult {
    structured_tool_error_result(call_id, code, message, expected_format, true)
}

pub(crate) fn scope_is_active(source_scope: &[String]) -> bool {
    !source_scope.is_empty()
}

pub(crate) fn source_in_scope(source_id: &str, source_scope: &[String]) -> bool {
    !scope_is_active(source_scope) || source_scope.iter().any(|id| id == source_id)
}

pub(crate) fn scoped_sources(
    db: &Database,
    source_scope: &[String],
) -> Result<Vec<Source>, CoreError> {
    let mut sources = db.list_sources()?;
    if scope_is_active(source_scope) {
        let allowed: HashSet<&str> = source_scope.iter().map(String::as_str).collect();
        sources.retain(|source| allowed.contains(source.id.as_str()));
    }
    Ok(sources)
}

pub(crate) fn ensure_source_in_scope(
    source_id: &str,
    source_scope: &[String],
) -> Result<(), String> {
    if source_in_scope(source_id, source_scope) {
        Ok(())
    } else {
        Err(format!(
            "Source '{source_id}' is outside the current source scope."
        ))
    }
}

pub(crate) fn current_scope_miss_message() -> &'static str {
    "I could not find that in the current source scope."
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// A tool that can be invoked by the agent during a conversation.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Machine-readable name used in LLM tool-call requests.
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &str;

    /// JSON Schema describing the parameters the tool accepts.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Build a [`ToolDefinition`] suitable for an LLM completion request.
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    /// Categories this tool belongs to. Used for dynamic tool visibility.
    /// Defaults to [`ToolCategory::Core`] so newly added tools are always visible.
    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core]
    }

    /// Whether this tool requires user confirmation before execution.
    /// Override for destructive tools. Receives parsed arguments so
    /// confirmation can be conditional (e.g. only on "remove" actions).
    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        false
    }

    /// Human-readable description of what this tool will do, for the
    /// confirmation dialog. Called with the tool's parsed arguments so it
    /// can describe the specific action.
    fn confirmation_message(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Execute the tool with the given JSON-encoded arguments.
    ///
    /// `source_scope` restricts results to the given source IDs when non-empty
    /// (used for per-conversation source scoping).
    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError>;

    /// Context-aware variant of [`Tool::execute`] used by the registry.
    ///
    /// Conversation-scoped tools (e.g. `update_scratchpad`) override this to
    /// receive the active `conversation_id`. The default impl falls back to
    /// [`Tool::execute`] so existing tools need no changes.
    async fn execute_with_context(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
        _conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        self.execute(call_id, arguments, db, source_scope).await
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// A collection of tools available to the agent.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(Arc::from(tool));
    }

    /// Register a shared tool instance.
    pub fn register_shared(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Return [`ToolDefinition`]s for every registered tool.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Check whether a tool name is already registered.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name() == name)
    }

    /// Return registered tool names in registry order.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    /// Build a filtered registry preserving the original tool order.
    pub fn filtered(&self, allowed_names: &[String]) -> ToolRegistry {
        let allowed: HashSet<&str> = allowed_names.iter().map(String::as_str).collect();
        let mut registry = ToolRegistry::new();
        for tool in &self.tools {
            if allowed.contains(tool.name()) {
                registry.register_shared(Arc::clone(tool));
            }
        }
        registry
    }

    /// Build a filtered registry excluding the provided tool names.
    pub fn without_names(&self, blocked_names: &[&str]) -> ToolRegistry {
        let blocked: HashSet<&str> = blocked_names.iter().copied().collect();
        let mut registry = ToolRegistry::new();
        for tool in &self.tools {
            if !blocked.contains(tool.name()) {
                registry.register_shared(Arc::clone(tool));
            }
        }
        registry
    }

    /// Check if a tool requires confirmation for the given arguments.
    pub fn requires_confirmation(&self, name: &str, args: &serde_json::Value) -> bool {
        self.get(name)
            .is_some_and(|t| t.requires_confirmation(args))
    }

    /// Get the confirmation message for a tool with the given arguments.
    pub fn confirmation_message(&self, name: &str, args: &serde_json::Value) -> Option<String> {
        self.get(name).and_then(|t| t.confirmation_message(args))
    }

    /// Return definitions for tools whose categories overlap with `active`.
    pub fn definitions_for_categories(
        &self,
        active: &HashSet<ToolCategory>,
    ) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| t.categories().iter().any(|c| active.contains(c)))
            .map(|t| t.definition())
            .collect()
    }

    /// Select tool definitions relevant to the current user message.
    ///
    /// Core and MCP tools are always included. Other categories are activated
    /// when keywords in the user message suggest they may be needed.
    ///
    /// Even when a tool is not advertised, it remains callable via
    /// [`execute`](Self::execute) — this is an optimisation, not a restriction.
    pub fn select_tools(&self, user_message: &str, has_sources: bool) -> Vec<ToolDefinition> {
        let mut categories: HashSet<ToolCategory> = HashSet::new();

        // Always-on categories
        categories.insert(ToolCategory::Core);
        categories.insert(ToolCategory::Mcp);

        let msg = user_message.to_lowercase();
        let looks_like_question = msg.contains('?')
            || msg.contains("what")
            || msg.contains("why")
            || msg.contains("how")
            || msg.contains("which")
            || msg.contains("where")
            || msg.contains("when")
            || msg.contains("who")
            || msg.contains("tell me")
            || msg.contains("explain")
            || msg.contains("analyze")
            || msg.contains("analysis")
            || msg.contains("总结")
            || msg.contains("分析")
            || msg.contains("为什么")
            || msg.contains("如何")
            || msg.contains("怎么")
            || msg.contains("哪些")
            || msg.contains("什么")
            || msg.contains("解释");

        // File operations
        if msg.contains("file")
            || msg.contains("read")
            || msg.contains("edit")
            || msg.contains("write")
            || msg.contains("create")
            || msg.contains("move")
            || msg.contains("rename")
            || msg.contains("copy")
            || msg.contains("delete")
            || msg.contains("directory")
            || msg.contains("folder")
            || msg.contains("note")
            || msg.contains("文件")
            || msg.contains("读取")
            || msg.contains("编辑")
            || msg.contains("移动")
            || msg.contains("重命名")
            || msg.contains("复制")
            || msg.contains("删除")
            || msg.contains("目录")
            || msg.contains("笔记")
            || msg.contains("document")
            || msg.contains("文档")
            || msg.contains("word")
            || msg.contains("docx")
            || msg.contains("excel")
            || msg.contains("xlsx")
            || msg.contains("ppt")
            || msg.contains("pptx")
            || msg.contains("office")
            || msg.contains("幻灯片")
            || msg.contains("表格")
        {
            categories.insert(ToolCategory::FileSystem);
        }

        // Source management
        if msg.contains("source")
            || msg.contains("index")
            || msg.contains("reindex")
            || msg.contains("数据源")
            || msg.contains("索引")
        {
            categories.insert(ToolCategory::SourceManagement);
        }

        // Knowledge / playbook
        if msg.contains("remember")
            || msg.contains("memory")
            || msg.contains("session")
            || msg.contains("history")
            || msg.contains("harness")
            || msg.contains("evolution")
            || msg.contains("evolve")
            || msg.contains("playbook")
            || msg.contains("collection")
            || msg.contains("collections")
            || msg.contains("citation")
            || msg.contains("citations")
            || msg.contains("evidence")
            || msg.contains("saved")
            || msg.contains("bookmark")
            || msg.contains("skill")
            || msg.contains("workflow")
            || msg.contains("compile")
            || msg.contains("compilation")
            || msg.contains("entity")
            || msg.contains("entities")
            || msg.contains("graph")
            || msg.contains("knowledge")
            || msg.contains("health")
            || msg.contains("archive")
            || msg.contains("wiki")
            || msg.contains("concept")
            || msg.contains("concepts")
            || msg.contains("收藏")
            || msg.contains("引用")
            || msg.contains("证据")
            || msg.contains("记住")
            || msg.contains("记忆")
            || msg.contains("会话")
            || msg.contains("历史")
            || msg.contains("进化")
            || msg.contains("自我")
            || msg.contains("编译")
            || msg.contains("实体")
            || msg.contains("图谱")
            || msg.contains("知识")
            || msg.contains("健康")
            || msg.contains("归档")
            || msg.contains("概念")
        {
            categories.insert(ToolCategory::Knowledge);
        }

        // Web / URL fetching
        if msg.contains("url")
            || msg.contains("http")
            || msg.contains("website")
            || msg.contains("web")
            || msg.contains("fetch")
            || msg.contains("link")
            || msg.contains("网页")
            || msg.contains("链接")
        {
            categories.insert(ToolCategory::Web);
        }

        // Controlled browser / desktop automation
        if msg.contains("browser")
            || msg.contains("desktop")
            || msg.contains("automate")
            || msg.contains("automation")
            || msg.contains("open url")
            || msg.contains("open website")
            || msg.contains("open file")
            || msg.contains("reveal file")
            || msg.contains("launch")
            || msg.contains("http://")
            || msg.contains("https://")
            || msg.contains("浏览器")
            || msg.contains("桌面")
            || msg.contains("自动化")
            || msg.contains("打开网页")
            || msg.contains("打开网站")
            || msg.contains("打开文件")
            || msg.contains("定位文件")
        {
            categories.insert(ToolCategory::Automation);
        }

        // Document analysis / comparison
        if msg.contains("compare")
            || msg.contains("document")
            || msg.contains("summarize")
            || msg.contains("summary")
            || msg.contains("analyze")
            || msg.contains("analysis")
            || msg.contains("evidence")
            || msg.contains("citation")
            || msg.contains("statistics")
            || msg.contains("stats")
            || msg.contains("info")
            || msg.contains("分析")
            || msg.contains("总结")
            || msg.contains("引用")
            || msg.contains("文档")
            || msg.contains("比较")
            || msg.contains("统计")
        {
            categories.insert(ToolCategory::DocumentAnalysis);
        }

        // If the conversation has linked sources, source management is likely useful
        if has_sources {
            categories.insert(ToolCategory::SourceManagement);
            if looks_like_question {
                categories.insert(ToolCategory::Knowledge);
                categories.insert(ToolCategory::DocumentAnalysis);
            }
        }

        self.definitions_for_categories(&categories)
    }

    /// Execute a tool by name, returning an error if the tool is not found.
    pub async fn execute(
        &self,
        name: &str,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        enforce_tool_arg_limit(name, arguments)?;
        let tool = self
            .get(name)
            .ok_or_else(|| CoreError::InvalidInput(format!("Unknown tool: {name}")))?;
        tool.execute(call_id, arguments, db, source_scope).await
    }

    /// Conversation-aware variant of [`ToolRegistry::execute`].
    ///
    /// Passes the active `conversation_id` to the tool so conversation-scoped
    /// tools (e.g. `update_scratchpad`) can look up or mutate their state.
    pub async fn execute_with_context(
        &self,
        name: &str,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        enforce_tool_arg_limit(name, arguments)?;
        let tool = self
            .get(name)
            .ok_or_else(|| CoreError::InvalidInput(format!("Unknown tool: {name}")))?;
        tool.execute_with_context(call_id, arguments, db, source_scope, conversation_id)
            .await
    }
}

/// Generic argument-size guard shared by both execute paths.
///
/// `run_shell` has its own stricter per-arg + total limits, so it's skipped
/// here. Other tools should never need more than 32 KB of JSON input; if an
/// LLM tries to stuff file bytes into (for example) `edit_document.replacements`
/// we reject early with a message pointing at the `doc-script-editor` skill.
fn enforce_tool_arg_limit(name: &str, arguments: &str) -> Result<(), CoreError> {
    const MAX_TOOL_ARG_BYTES: usize = 32 * 1024;
    if name == "run_shell" {
        return Ok(());
    }
    let arg_size = arguments.len();
    if arg_size > MAX_TOOL_ARG_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Tool arguments exceed {} KB ({} bytes). For document editing with large content, use the 'run_shell' tool with the 'doc-script-editor' skill instead of passing file bytes in arguments.",
            MAX_TOOL_ARG_BYTES / 1024,
            arg_size
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Default registry builder
// ---------------------------------------------------------------------------

/// Build the default tool registry with all built-in tools.
pub fn default_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(search_tool::SearchTool));
    registry.register(Box::new(playbook_tool::PlaybookTool));
    registry.register(Box::new(
        prepare_document_tools_tool::PrepareDocumentToolsTool,
    ));
    registry.register(Box::new(file_tool::FileTool));
    registry.register(Box::new(read_files_tool::ReadFilesTool));
    registry.register(Box::new(summarize_tool::RetrieveEvidenceTool));
    registry.register(Box::new(list_sources_tool::ListSourcesTool));
    registry.register(Box::new(list_documents_tool::ListDocumentsTool));
    registry.register(Box::new(list_dir_tool::ListDirTool));
    registry.register(Box::new(chunk_context_tool::ChunkContextTool));
    registry.register(Box::new(fetch_url_tool::FetchUrlTool));
    registry.register(Box::new(write_note_tool::WriteNoteTool));
    registry.register(Box::new(search_playbooks_tool::SearchPlaybooksTool));
    registry.register(Box::new(edit_file_tool::EditFileTool));
    registry.register(Box::new(create_file_tool::CreateFileTool));
    registry.register(Box::new(edit_document_tool::EditDocumentTool));
    registry.register(Box::new(submit_feedback_tool::SubmitFeedbackTool));
    registry.register(Box::new(document_info_tool::GetDocumentInfoTool));
    registry.register(Box::new(reindex_tool::ReindexTool));
    registry.register(Box::new(compare_tool::CompareTool));
    registry.register(Box::new(manage_source_tool::ManageSourceTool));
    registry.register(Box::new(statistics_tool::GetStatisticsTool));
    registry.register(Box::new(date_search_tool::DateSearchTool));
    registry.register(Box::new(desktop_automation_tool::DesktopAutomationTool));
    registry.register(Box::new(summarize_tool::SummarizeDocumentTool));
    registry.register(Box::new(update_plan_tool::UpdatePlanTool));
    registry.register(Box::new(record_verification_tool::RecordVerificationTool));
    registry.register(Box::new(compile_tool::CompileTool));
    registry.register(Box::new(knowledge_graph_tool::KnowledgeGraphTool));
    registry.register(Box::new(health_check_tool::HealthCheckTool));
    registry.register(Box::new(archive_output_tool::ArchiveOutputTool));
    registry.register(Box::new(related_concepts_tool::RelatedConceptsTool));
    registry.register(Box::new(run_shell_tool::RunShellTool));
    registry.register(Box::new(scratchpad_tool::UpdateScratchpadTool));
    registry.register(Box::new(session_search_tool::SessionSearchTool));
    registry.register(Box::new(agent_memory_tool::AgentMemoryTool));
    registry.register(Box::new(manage_skill_tool::ManageSkillTool));
    registry.register(Box::new(harness_dry_run_tool::HarnessDryRunTool));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_tools_includes_knowledge_for_collection_queries() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("summarize this collection and its evidence", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "manage_playbook"));
        assert!(names.iter().any(|name| name == "search_playbooks"));
    }

    #[test]
    fn select_tools_includes_document_analysis_for_question_with_sources() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("What changed in my retry notes and why?", true);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "compare_documents"));
        assert!(names.iter().any(|name| name == "summarize_document"));
    }

    #[test]
    fn search_knowledge_base_contract_accepts_single_or_batch_query() {
        let registry = default_tool_registry();
        let def = registry
            .get("search_knowledge_base")
            .expect("search tool should be registered")
            .definition();
        let properties = def.parameters["properties"]
            .as_object()
            .expect("tool parameters should be an object");

        assert!(properties.contains_key("query"));
        assert!(properties.contains_key("queries"));
        assert_eq!(def.parameters["required"], serde_json::json!([]));
        assert!(def.description.contains("queries"));
        assert!(def.description.contains("SINGLE call"));
    }

    #[test]
    fn default_registry_does_not_offer_legacy_office_generators() {
        let registry = default_tool_registry();
        let names = registry.tool_names();

        assert!(!names.iter().any(|name| name == "generate_docx"));
        assert!(!names.iter().any(|name| name == "generate_xlsx"));
        assert!(!names.iter().any(|name| name == "ppt_generate"));
        assert!(names.iter().any(|name| name == "prepare_document_tools"));
    }

    #[test]
    fn select_tools_includes_desktop_automation_for_browser_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Open this website in my browser", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "desktop_automation"));
    }
}
