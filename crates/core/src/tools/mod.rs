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

use crate::app_settings::ShellAccessMode;
use crate::approval::{ApprovalRisk, ToolApprovalMode};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolDefinition;
use crate::models::Source;
use crate::plugins::ToolPluginInfo;

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

fn with_scheduler_control_parameters(mut parameters: serde_json::Value) -> serde_json::Value {
    let Some(schema) = parameters.as_object_mut() else {
        return parameters;
    };
    if schema.get("type").and_then(|value| value.as_str()) != Some("object") {
        return parameters;
    }
    let properties = schema
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    let Some(properties) = properties.as_object_mut() else {
        return parameters;
    };
    properties
        .entry("wait_for_previous".to_string())
        .or_insert_with(|| {
            serde_json::json!({
                "type": "boolean",
                "description": "If true, this call waits for earlier tool calls in the same assistant turn to finish before it starts. Use this when the call depends on files, artifacts, or command output produced by a previous tool call.",
                "default": false
            })
        });
    parameters
}

pub mod agent_memory_tool;
pub mod archive_output_tool;
pub mod chunk_context_tool;
pub mod code_intelligence_tool;
pub mod compare_tool;
pub mod compile_tool;
pub mod create_file_tool;
pub mod date_search_tool;
pub mod desktop_automation_tool;
pub(crate) mod diff_stats;
pub mod document_info_tool;
pub mod document_utils;
pub mod edit_file_tool;
pub mod fetch_url_tool;
pub mod file_tool;
pub mod glob_files_tool;
pub mod harness_dry_run_tool;
pub mod health_check_tool;
pub mod image_generation_tool;
pub mod knowledge_graph_tool;
pub mod list_dir_tool;
pub mod list_documents_tool;
pub mod list_sources_tool;
pub mod manage_skill_tool;
pub mod manage_source_tool;
pub mod mcp_tool;
pub mod multi_edit_tool;
pub mod path_utils;
pub mod playbook_tool;
pub mod prepare_document_tools_tool;
pub mod project_tool;
pub mod read_files_tool;
pub mod record_verification_tool;
pub mod reindex_tool;
pub mod related_concepts_tool;
pub mod run_shell_tool;
pub mod scratchpad_tool;
pub mod search_files_tool;
pub mod search_playbooks_tool;
pub mod search_tool;
pub mod session_search_tool;
pub mod statistics_tool;
pub mod submit_feedback_tool;
pub mod summarize_tool;
pub mod tool_search_tool;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputAttachment {
    pub name: String,
    pub mime_type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub llm_content: String,
    pub display_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolOutputAttachment>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            llm_content: content.clone(),
            display_content: content,
            data: None,
            artifacts: None,
            attachments: Vec::new(),
        }
    }
}

impl ToolResult {
    const OUTPUT_ARTIFACT_KEY: &'static str = "toolOutput";

    pub fn from_output(call_id: impl Into<String>, is_error: bool, output: ToolOutput) -> Self {
        let mut artifacts = serde_json::Map::new();
        artifacts.insert(
            Self::OUTPUT_ARTIFACT_KEY.to_string(),
            serde_json::to_value(&output).unwrap_or(serde_json::Value::Null),
        );
        if let Some(data) = output.data.clone() {
            artifacts.insert("data".to_string(), data);
        }
        if let Some(nested_artifacts) = output.artifacts.clone() {
            artifacts.insert("artifacts".to_string(), nested_artifacts);
        }
        Self {
            call_id: call_id.into(),
            content: output.display_content,
            is_error,
            artifacts: Some(serde_json::Value::Object(artifacts)),
        }
    }

    pub fn output_channels(&self) -> ToolOutput {
        self.artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.get(Self::OUTPUT_ARTIFACT_KEY))
            .and_then(|value| serde_json::from_value::<ToolOutput>(value.clone()).ok())
            .unwrap_or_else(|| ToolOutput {
                llm_content: self.content.clone(),
                display_content: self.content.clone(),
                data: None,
                artifacts: self.artifacts.clone(),
                attachments: Vec::new(),
            })
    }

    pub fn llm_context_content(&self) -> String {
        self.output_channels().llm_content
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolRenderKind {
    Generic,
    CommandExecution,
    FileChange,
    Search,
    Subagent,
    Image,
    Plan,
    Verification,
    Mcp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolInputStreamingMode {
    None,
    UiPreview,
    ToolConsumesPartial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolInterruptBehavior {
    Block,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRunCapabilities {
    pub input_streaming: ToolInputStreamingMode,
    pub render_kind: ToolRenderKind,
    pub read_only: bool,
    pub destructive: bool,
    pub concurrency_safe: bool,
    pub interrupt_behavior: ToolInterruptBehavior,
    pub resource_keys: Vec<String>,
}

/// Canonical policy and permission profile for a tool invocation.
///
/// This is intentionally separate from [`ToolRunCapabilities`]. Capabilities
/// describe how a call should run; the access profile describes what kind of
/// user data or system surface the call may touch, and how it should be shown
/// in permission UIs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccessProfile {
    pub category: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_execute: bool,
    pub can_access_network: bool,
    pub needs_approval: bool,
    pub risk_level: ApprovalRisk,
    pub risk_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocation {
    pub call_id: String,
    pub tool_name: String,
    pub plugin: ToolPluginInfo,
    pub arguments: serde_json::Value,
    pub capabilities: ToolRunCapabilities,
    pub access_profile: ToolAccessProfile,
    pub wait_for_previous: bool,
}

pub struct ToolExecutionContext<'a> {
    pub call_id: &'a str,
    pub arguments: &'a str,
    pub db: &'a Database,
    pub source_scope: &'a [String],
    pub conversation_id: Option<&'a str>,
    pub cancel_token: Option<&'a tokio_util::sync::CancellationToken>,
}

fn infer_render_kind(name: &str) -> ToolRenderKind {
    match name {
        "run_shell" => ToolRenderKind::CommandExecution,
        "edit_file" | "multi_edit" | "create_file" | "write_note" => ToolRenderKind::FileChange,
        "search_knowledge_base"
        | "search_files"
        | "search_playbooks"
        | "glob_files"
        | "list_dir"
        | "list_documents"
        | "list_sources"
        | "session_search"
        | "tool_search"
        | "code_intelligence" => ToolRenderKind::Search,
        "spawn_subagent" | "spawn_subagent_batch" => ToolRenderKind::Subagent,
        "generate_image" => ToolRenderKind::Image,
        "update_plan" => ToolRenderKind::Plan,
        "record_verification" => ToolRenderKind::Verification,
        name if name.starts_with("mcp__") => ToolRenderKind::Mcp,
        _ => ToolRenderKind::Generic,
    }
}

fn infer_input_streaming(name: &str) -> ToolInputStreamingMode {
    match name {
        "generate_image"
        | "run_shell"
        | "search_knowledge_base"
        | "search_files"
        | "spawn_subagent"
        | "spawn_subagent_batch"
        | "tool_search"
        | "code_intelligence" => ToolInputStreamingMode::UiPreview,
        _ => ToolInputStreamingMode::None,
    }
}

fn push_string_resource_key(keys: &mut Vec<String>, prefix: &str, value: &str) {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty() {
        return;
    }
    let key = format!("{prefix}:{normalized}");
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}

fn collect_string_or_array_resource(
    args: &serde_json::Value,
    field: &str,
    prefix: &str,
    keys: &mut Vec<String>,
) {
    let Some(value) = args.get(field) else {
        return;
    };
    match value {
        serde_json::Value::String(text) => push_string_resource_key(keys, prefix, text),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    push_string_resource_key(keys, prefix, text);
                }
            }
        }
        _ => {}
    }
}

fn infer_resource_keys(name: &str, args: &serde_json::Value) -> Vec<String> {
    let mut keys = Vec::new();
    for field in [
        "path",
        "paths",
        "file_path",
        "filePath",
        "file_paths",
        "filePaths",
        "target_path",
        "targetPath",
        "target_paths",
        "targetPaths",
        "absolute_path",
        "absolutePath",
        "source_path",
        "sourcePath",
        "destination_path",
        "destinationPath",
        "dest_path",
        "destPath",
        "new_path",
        "newPath",
        "old_path",
        "oldPath",
    ] {
        collect_string_or_array_resource(args, field, "file", &mut keys);
    }
    for field in ["source_id", "sourceId", "source_ids", "sourceIds"] {
        collect_string_or_array_resource(args, field, "source", &mut keys);
    }
    if name.starts_with("mcp__") {
        push_string_resource_key(&mut keys, "mcp", name);
    }
    keys
}

fn is_builtin_web_search_mcp_tool(name: &str) -> bool {
    name == "mcp__web_search__search" || name.starts_with("mcp__web_search__")
}

pub fn invocation_waits_for_previous(args: &serde_json::Value) -> bool {
    args.get("wait_for_previous")
        .or_else(|| args.get("waitForPrevious"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn category_label(category: ToolCategory) -> &'static str {
    match category {
        ToolCategory::Core => "core",
        ToolCategory::FileSystem => "filesystem",
        ToolCategory::SourceManagement => "source_management",
        ToolCategory::Knowledge => "knowledge",
        ToolCategory::Web => "web",
        ToolCategory::DocumentAnalysis => "document_analysis",
        ToolCategory::SubAgent => "delegation",
        ToolCategory::Mcp => "mcp",
        ToolCategory::Automation => "automation",
    }
}

fn first_non_core_category(categories: &[ToolCategory]) -> ToolCategory {
    categories
        .iter()
        .copied()
        .find(|category| *category != ToolCategory::Core)
        .or_else(|| categories.first().copied())
        .unwrap_or(ToolCategory::Core)
}

fn generic_access_profile(
    name: &str,
    categories: &[ToolCategory],
    capabilities: &ToolRunCapabilities,
) -> ToolAccessProfile {
    let category = if name.starts_with("mcp__") {
        "mcp"
    } else {
        category_label(first_non_core_category(categories))
    };
    let can_execute = matches!(
        capabilities.render_kind,
        ToolRenderKind::CommandExecution | ToolRenderKind::Subagent
    ) || categories
        .iter()
        .any(|category| matches!(category, ToolCategory::Automation | ToolCategory::SubAgent));
    let can_access_network = categories
        .iter()
        .any(|category| matches!(category, ToolCategory::Web | ToolCategory::Mcp))
        || name.starts_with("mcp__");
    let can_write = !capabilities.read_only || capabilities.destructive;
    let risk_level = if can_execute && (can_write || can_access_network) {
        ApprovalRisk::High
    } else if can_write || capabilities.destructive {
        ApprovalRisk::Medium
    } else {
        ApprovalRisk::Low
    };

    ToolAccessProfile {
        category: category.to_string(),
        can_read: !matches!(
            capabilities.render_kind,
            ToolRenderKind::Plan | ToolRenderKind::Verification
        ),
        can_write,
        can_execute,
        can_access_network,
        needs_approval: capabilities.destructive,
        risk_level,
        risk_reason: if risk_level == ApprovalRisk::Low {
            "Read-only or low-risk local agent helper.".to_string()
        } else {
            "Tool capabilities indicate this invocation can mutate state, execute work, or cross a trust boundary.".to_string()
        },
    }
}

pub fn infer_tool_access_profile(
    name: &str,
    categories: &[ToolCategory],
    capabilities: &ToolRunCapabilities,
    args: &serde_json::Value,
) -> ToolAccessProfile {
    let (
        category,
        can_read,
        can_write,
        can_execute,
        can_access_network,
        needs_approval,
        risk_level,
        reason,
    ) = match name {
        "run_shell" => (
            "system",
            true,
            true,
            true,
            true,
            true,
            ApprovalRisk::High,
            "Executes local commands and can affect files, processes, and network.",
        ),
        "edit_file" | "multi_edit" => (
            "filesystem",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::High,
            "Modifies existing text files and should pass through the write approval gate.",
        ),
        "create_file" => (
            "filesystem",
            false,
            true,
            false,
            false,
            true,
            if args
                .get("overwrite")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "Creates or overwrites local files.",
        ),
        "write_note" => (
            "filesystem",
            false,
            true,
            false,
            false,
            true,
            if args
                .get("mode")
                .and_then(|value| value.as_str())
                .is_some_and(|mode| mode != "create")
            {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "Creates or updates local notes.",
        ),
        "archive_output" => (
            "artifact",
            false,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
            "Persists agent output as a reusable local artifact.",
        ),
        "prepare_document_tools" => (
            "document_tooling",
            true,
            true,
            true,
            true,
            true,
            ApprovalRisk::Medium,
            "Prepares required Python document-processing helpers.",
        ),
        "manage_source" => (
            "source_management",
            true,
            true,
            false,
            false,
            true,
            if args.get("action").and_then(|value| value.as_str()) == Some("remove") {
                ApprovalRisk::High
            } else {
                ApprovalRisk::Medium
            },
            "Adds, updates, or removes knowledge sources.",
        ),
        "reindex_document" => (
            "source_management",
            true,
            true,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Refreshes derived knowledge indexes without directly editing user files.",
        ),
        "compile_document" => (
            "document_analysis",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads document compilation status and diagnostics.",
        ),
        "fetch_url" => (
            "web",
            true,
            false,
            false,
            true,
            false,
            ApprovalRisk::Low,
            "Reads remote URLs and crosses the local trust boundary.",
        ),
        "desktop_automation" => (
            "automation",
            true,
            true,
            true,
            true,
            true,
            ApprovalRisk::High,
            "Can operate desktop or browser surfaces through automation.",
        ),
        "get_document_info" | "compare_documents" | "summarize_document" => (
            "document_analysis",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads local Office/PDF/document content for inspection and comparison.",
        ),
        "read_file" | "read_files" | "list_dir" | "glob_files" | "search_files"
        | "grep_files" | "code_intelligence" => (
            "filesystem",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads local files or directories for source-scoped inspection.",
        ),
        "project_tool" => {
            if args.get("action").and_then(|value| value.as_str()) == Some("run") {
                (
                    "project_tool",
                    true,
                    true,
                    true,
                    true,
                    true,
                    ApprovalRisk::High,
                    "Runs a command declared by a source-scoped project tool manifest.",
                )
            } else {
                (
                    "project_tool_catalog",
                    true,
                    false,
                    false,
                    false,
                    false,
                    ApprovalRisk::Low,
                    "Reads source-scoped project tool manifests.",
                )
            }
        }
        "run_health_check" | "get_statistics" => (
            "knowledge_health",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads knowledge-base diagnostics, coverage, and storage statistics.",
        ),
        "agent_harness_dry_run" => (
            "agent_harness",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Runs a read-only readiness preview of local agent configuration and tool availability.",
        ),
        "search_knowledge_base"
        | "retrieve_evidence"
        | "list_sources"
        | "list_documents"
        | "search_by_date"
        | "get_chunk_context"
        | "query_knowledge_graph"
        | "get_related_concepts" => (
            "knowledge",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads indexed local knowledge as evidence.",
        ),
        "search_playbooks" | "search_sessions" => (
            "memory",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads saved sessions, playbooks, or reusable local working context.",
        ),
        "manage_playbook" | "submit_feedback" => (
            "memory",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
            "Changes reusable playbooks, feedback, or knowledge-workflow records.",
        ),
        "manage_agent_memory" | "update_scratchpad" | "manage_skill" => (
            "memory",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
            "Changes persistent agent memory, skills, or working notes.",
        ),
        "spawn_subagent" | "spawn_subagent_batch" => (
            "delegation",
            true,
            true,
            true,
            true,
            true,
            ApprovalRisk::Medium,
            "Delegates bounded work to another agent with narrowed tool and source access.",
        ),
        "judge_subagent_results" => (
            "delegation",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads and adjudicates subagent outputs without directly changing user data.",
        ),
        tool if is_builtin_web_search_mcp_tool(tool) => (
            "web",
            true,
            false,
            false,
            true,
            false,
            ApprovalRisk::Low,
            "Reads web search results through the built-in web search MCP server.",
        ),
        tool if tool == "mcp_tool" || tool.starts_with("mcp__") => (
            "mcp",
            true,
            true,
            true,
            true,
            true,
            ApprovalRisk::High,
            "Delegates to an external MCP server with server-defined capabilities.",
        ),
        "update_plan" | "record_verification" => (
            "artifact",
            false,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Records structured task progress or verification artifacts.",
        ),
        "tool_search" => (
            "tool_catalog",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads the built-in tool catalog to choose an appropriate tool.",
        ),
        _ => {
            return generic_access_profile(name, categories, capabilities);
        }
    };

    ToolAccessProfile {
        category: category.to_string(),
        can_read,
        can_write,
        can_execute,
        can_access_network,
        needs_approval,
        risk_level,
        risk_reason: reason.to_string(),
    }
}

pub fn fallback_tool_access_profile(name: &str, args: &serde_json::Value) -> ToolAccessProfile {
    let capabilities = ToolRunCapabilities {
        input_streaming: infer_input_streaming(name),
        render_kind: infer_render_kind(name),
        read_only: !matches!(name, "mcp_tool") && !name.starts_with("mcp__"),
        destructive: false,
        concurrency_safe: true,
        interrupt_behavior: ToolInterruptBehavior::Block,
        resource_keys: infer_resource_keys(name, args),
    };
    infer_tool_access_profile(name, &[ToolCategory::Core], &capabilities, args)
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

#[derive(Debug, Clone)]
pub(crate) struct FileAccessPolicy {
    pub sources: Vec<Source>,
    pub allow_unregistered_absolute_paths: bool,
}

pub(crate) fn file_access_policy(
    db: &Database,
    source_scope: &[String],
) -> Result<FileAccessPolicy, CoreError> {
    let config = db.load_app_config().unwrap_or_default();
    let use_all_sources = !config.shell_access_mode.is_restricted()
        || config.tool_approval_mode == ToolApprovalMode::AllowAll;
    let sources = if use_all_sources {
        scoped_sources(db, &[])?
    } else {
        scoped_sources(db, source_scope)?
    };

    Ok(FileAccessPolicy {
        sources,
        allow_unregistered_absolute_paths: matches!(
            config.shell_access_mode,
            ShellAccessMode::Open
        ),
    })
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
            parameters: with_scheduler_control_parameters(self.parameters_schema()),
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

    /// Preferred frontend renderer family for this tool.
    fn render_kind(&self) -> ToolRenderKind {
        infer_render_kind(self.name())
    }

    /// Whether this tool can safely receive arguments before the final JSON is
    /// complete. Default is no streaming; tools can opt into UI preview or true
    /// partial-input consumption once their implementation supports it.
    fn input_streaming(&self) -> ToolInputStreamingMode {
        infer_input_streaming(self.name())
    }

    /// Whether this invocation is read-only after parsing its arguments.
    fn is_read_only(&self, args: &serde_json::Value) -> bool {
        !self.requires_confirmation(args)
    }

    /// Whether multiple invocations of this tool can safely run concurrently.
    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
        !self.requires_confirmation(_args)
    }

    /// What should happen if the user interrupts while this tool is running.
    fn interrupt_behavior(&self) -> ToolInterruptBehavior {
        ToolInterruptBehavior::Cancel
    }

    /// Resource identity touched by this invocation.
    ///
    /// The scheduler uses these keys to allow independent writes to run in
    /// parallel while still isolating calls that touch the same file/source.
    fn resource_keys(&self, args: &serde_json::Value) -> Vec<String> {
        infer_resource_keys(self.name(), args)
    }

    /// Canonical capability descriptor used by the ToolRun lifecycle.
    fn run_capabilities(&self, args: &serde_json::Value) -> ToolRunCapabilities {
        let destructive = self.requires_confirmation(args);
        let resource_keys = self.resource_keys(args);
        ToolRunCapabilities {
            input_streaming: self.input_streaming(),
            render_kind: self.render_kind(),
            read_only: self.is_read_only(args),
            destructive,
            concurrency_safe: self.is_concurrency_safe(args),
            interrupt_behavior: if destructive {
                ToolInterruptBehavior::Block
            } else {
                self.interrupt_behavior()
            },
            resource_keys,
        }
    }

    /// Canonical permission and risk descriptor for this invocation.
    fn access_profile(&self, args: &serde_json::Value) -> ToolAccessProfile {
        let capabilities = self.run_capabilities(args);
        infer_tool_access_profile(self.name(), self.categories(), &capabilities, args)
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

    /// Deep execution interface used by the agent runtime.
    ///
    /// The default bridges to the legacy argument list, while newer tools can
    /// use cancellation and other execution context without widening every call
    /// site again.
    async fn execute_with_run_context(
        &self,
        ctx: ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        self.execute_with_context(
            ctx.call_id,
            ctx.arguments,
            ctx.db,
            ctx.source_scope,
            ctx.conversation_id,
        )
        .await
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

    pub fn run_capabilities(&self, name: &str, args: &serde_json::Value) -> ToolRunCapabilities {
        self.get(name)
            .map(|tool| tool.run_capabilities(args))
            .unwrap_or(ToolRunCapabilities {
                input_streaming: ToolInputStreamingMode::None,
                render_kind: infer_render_kind(name),
                read_only: false,
                destructive: false,
                concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Block,
                resource_keys: infer_resource_keys(name, args),
            })
    }

    pub fn access_profile(&self, name: &str, args: &serde_json::Value) -> ToolAccessProfile {
        self.get(name)
            .map(|tool| tool.access_profile(args))
            .unwrap_or_else(|| fallback_tool_access_profile(name, args))
    }

    pub fn plugin_info(&self, name: &str) -> ToolPluginInfo {
        crate::plugins::plugin_for_tool(name)
    }

    pub fn build_invocation(
        &self,
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> ToolInvocation {
        let tool_name = name.into();
        let capabilities = self.run_capabilities(&tool_name, &arguments);
        let access_profile = self.access_profile(&tool_name, &arguments);
        let plugin = self.plugin_info(&tool_name);
        ToolInvocation {
            call_id: call_id.into(),
            tool_name,
            plugin,
            wait_for_previous: invocation_waits_for_previous(&arguments),
            arguments,
            capabilities,
            access_profile,
        }
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
    /// Dynamic visibility is an opt-in prompt compaction mode. The main agent
    /// should normally receive the full registry; this selector exists for
    /// constrained runs and behavioral evaluation.
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

        let looks_like_code_or_tool_work = msg.contains("run_shell")
            || msg.contains("run shell")
            || msg.contains("shell")
            || msg.contains("terminal")
            || msg.contains("command")
            || msg.contains("powershell")
            || msg.contains("cmd")
            || msg.contains("cargo")
            || msg.contains("npm")
            || msg.contains("pnpm")
            || msg.contains("node")
            || msg.contains("python")
            || msg.contains("git")
            || msg.contains("tool")
            || msg.contains("tools")
            || msg.contains("agent")
            || msg.contains("subagent")
            || msg.contains("unavailable")
            || msg.contains("available")
            || msg.contains("fix")
            || msg.contains("debug")
            || msg.contains("bug")
            || msg.contains("test")
            || msg.contains("build")
            || msg.contains("compile")
            || msg.contains("运行")
            || msg.contains("命令")
            || msg.contains("终端")
            || msg.contains("调用")
            || msg.contains("工具")
            || msg.contains("不可用")
            || msg.contains("修复")
            || msg.contains("排查")
            || msg.contains("测试")
            || msg.contains("构建")
            || msg.contains("编译")
            || msg.contains("代码")
            || msg.contains("项目")
            || msg.contains("仓库")
            || msg.contains("主agent")
            || msg.contains("子agent");

        // File operations
        if msg.contains("file")
            || msg.contains("read")
            || msg.contains("edit")
            || msg.contains("replace")
            || msg.contains("write")
            || msg.contains("create")
            || msg.contains("find")
            || msg.contains("grep")
            || msg.contains("rg")
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
            || msg.contains("替换")
            || msg.contains("查找")
            || msg.contains("搜索文件")
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
            || looks_like_code_or_tool_work
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

    pub async fn execute_with_run_context(
        &self,
        name: &str,
        ctx: ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        enforce_tool_arg_limit(name, ctx.arguments)?;
        let tool = self
            .get(name)
            .ok_or_else(|| CoreError::InvalidInput(format!("Unknown tool: {name}")))?;
        tool.execute_with_run_context(ctx).await
    }
}

/// Generic argument-size guard shared by both execute paths.
///
/// `run_shell` has its own stricter per-arg + total limits, so it's skipped
/// here. Other tools should never need more than 32 KB of JSON input; if an
/// LLM tries to stuff document bytes into tool arguments, we reject early with
/// a message pointing at the `doc-script-editor` skill.
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
    registry.register(Box::new(tool_search_tool::ToolSearchTool));
    registry.register(Box::new(glob_files_tool::GlobFilesTool));
    registry.register(Box::new(search_files_tool::SearchFilesTool));
    registry.register(Box::new(search_files_tool::GrepFilesTool));
    registry.register(Box::new(code_intelligence_tool::CodeIntelligenceTool));
    registry.register(Box::new(project_tool::ProjectTool));
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
    registry.register(Box::new(multi_edit_tool::MultiEditTool));
    registry.register(Box::new(create_file_tool::CreateFileTool));
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
    registry.register(Box::new(image_generation_tool::GenerateImageTool));
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
    use crate::approval::ApprovalRisk;

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
        assert_eq!(properties["queries"]["maxItems"], serde_json::json!(2));
        assert_eq!(def.parameters["required"], serde_json::json!([]));
        assert!(def.description.contains("queries"));
        assert!(def.description.contains("at most 1-2 query variants"));
    }

    #[test]
    fn default_registry_does_not_offer_legacy_office_generators() {
        let registry = default_tool_registry();
        let names = registry.tool_names();

        assert!(!names.iter().any(|name| name == "generate_docx"));
        assert!(!names.iter().any(|name| name == "generate_xlsx"));
        assert!(!names.iter().any(|name| name == "ppt_generate"));
        assert!(!names.iter().any(|name| name == "edit_document"));
        assert!(names.iter().any(|name| name == "prepare_document_tools"));
    }

    #[test]
    fn select_tools_includes_desktop_automation_for_browser_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Open this website in my browser", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "desktop_automation"));
    }

    #[test]
    fn select_tools_includes_shell_for_tool_repair_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools(
            "为什么主agent没有办法调用run_shell？请仔细排查并全面修复。",
            false,
        );
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "run_shell"));
        assert!(names.iter().any(|name| name == "edit_file"));
        assert!(names.iter().any(|name| name == "code_intelligence"));
        assert!(names.iter().any(|name| name == "project_tool"));
    }

    #[test]
    fn select_tools_includes_code_navigation_for_codebase_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools(
            "debug the agent routing bug and run the repository diagnostics",
            false,
        );
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "code_intelligence"));
        assert!(names.iter().any(|name| name == "project_tool"));
        assert!(names.iter().any(|name| name == "search_files"));
        assert!(names.iter().any(|name| name == "run_shell"));
    }

    #[test]
    fn access_profile_marks_run_shell_as_high_risk_platform_capability() {
        let registry = default_tool_registry();
        let args = serde_json::json!({
            "program": "git",
            "args": ["status", "--short"],
            "cwd": "."
        });

        let profile = registry.access_profile("run_shell", &args);

        assert_eq!(profile.category, "system");
        assert!(profile.can_read);
        assert!(profile.can_write);
        assert!(profile.can_execute);
        assert!(profile.can_access_network);
        assert!(profile.needs_approval);
        assert_eq!(profile.risk_level, ApprovalRisk::High);

        let capabilities = registry.run_capabilities("run_shell", &args);
        assert!(!capabilities.destructive);
    }

    #[test]
    fn access_profile_reflects_argument_sensitive_write_risk() {
        let registry = default_tool_registry();
        let create = registry.access_profile(
            "create_file",
            &serde_json::json!({
                "path": "notes/example.md",
                "content": "hello",
                "overwrite": false
            }),
        );
        let overwrite = registry.access_profile(
            "create_file",
            &serde_json::json!({
                "path": "notes/example.md",
                "content": "hello",
                "overwrite": true
            }),
        );

        assert!(create.can_write);
        assert_eq!(create.risk_level, ApprovalRisk::Medium);
        assert_eq!(overwrite.risk_level, ApprovalRisk::High);
        assert!(overwrite.needs_approval);
    }

    #[test]
    fn tool_definitions_expose_scheduler_wait_hint() {
        let registry = default_tool_registry();
        let def = registry
            .get("run_shell")
            .expect("run_shell should be registered")
            .definition();

        let properties = def.parameters["properties"]
            .as_object()
            .expect("parameters should expose properties");
        assert!(properties.contains_key("wait_for_previous"));
        assert!(!def.parameters["required"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|value| value.as_str() == Some("wait_for_previous")));
    }

    #[test]
    fn default_tool_contracts_have_policy_and_scheduler_metadata() {
        let registry = default_tool_registry();

        for name in registry.tool_names() {
            let tool = registry.get(&name).expect("registered tool should resolve");
            let def = tool.definition();
            if def.parameters["type"].as_str() == Some("object") {
                assert!(
                    def.parameters["properties"]
                        .as_object()
                        .is_some_and(|properties| properties.contains_key("wait_for_previous")),
                    "{name} should expose wait_for_previous"
                );
            }

            let profile = registry.access_profile(&name, &serde_json::Value::Null);
            assert!(
                !profile.category.is_empty(),
                "{name} should declare an access category"
            );
            assert!(
                !profile.risk_reason.is_empty(),
                "{name} should declare a risk reason"
            );
            if profile.needs_approval {
                assert_ne!(
                    profile.risk_level,
                    ApprovalRisk::Low,
                    "{name} should not be low-risk when approval is needed"
                );
            }
        }
    }

    #[test]
    fn invocation_combines_capabilities_policy_and_scheduler_hints() {
        let registry = default_tool_registry();
        let invocation = registry.build_invocation(
            "call-1",
            "create_file",
            serde_json::json!({
                "path": "notes/example.md",
                "content": "hello",
                "overwrite": true,
                "wait_for_previous": true
            }),
        );

        assert_eq!(invocation.call_id, "call-1");
        assert_eq!(invocation.tool_name, "create_file");
        assert_eq!(invocation.plugin.id, "file-workspace");
        assert!(invocation.wait_for_previous);
        assert!(invocation.capabilities.destructive);
        assert!(invocation.access_profile.can_write);
        assert_eq!(invocation.access_profile.risk_level, ApprovalRisk::High);
    }

    #[test]
    fn tool_result_can_split_display_data_and_llm_context() {
        let output = ToolOutput {
            llm_content: "context-only summary".to_string(),
            display_content: "full display output".to_string(),
            data: Some(serde_json::json!({ "rows": 2 })),
            artifacts: Some(serde_json::json!({ "kind": "table" })),
            attachments: Vec::new(),
        };

        let result = ToolResult::from_output("call-1", false, output.clone());

        assert_eq!(result.content, "full display output");
        assert_eq!(result.llm_context_content(), "context-only summary");
        assert_eq!(result.output_channels(), output);
    }
}
