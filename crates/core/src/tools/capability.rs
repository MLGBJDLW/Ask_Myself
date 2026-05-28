//! Capability descriptors for tool runtime, permissions, UI projection, and
//! resource scheduling.
//!
//! Tool implementations stay in their domain modules. This module owns the
//! manifest-like metadata that the runtime, approval UI, and scheduler need to
//! interpret a tool call consistently.

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::plugins::{plugin_for_tool, ToolPluginInfo};

use super::{
    ToolAccessProfile, ToolInputStreamingMode, ToolInterruptBehavior, ToolRenderKind,
    ToolRunCapabilities,
};

/// Logical category for grouping tools. Used by [`super::ToolRegistry::select_tools`]
/// to decide which tool definitions are sent to the LLM on a given turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolUiDescriptor {
    pub render_kind: ToolRenderKind,
    pub display_category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResourceDescriptor {
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilityDescriptor {
    pub name: String,
    pub package: ToolPluginInfo,
    pub categories: Vec<String>,
    pub ui: ToolUiDescriptor,
    pub resources: ToolResourceDescriptor,
    pub capabilities: ToolRunCapabilities,
    pub access_profile: ToolAccessProfile,
}

pub fn category_label(category: ToolCategory) -> &'static str {
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

pub fn capability_render_kind(name: &str) -> ToolRenderKind {
    match name {
        "run_shell" => ToolRenderKind::CommandExecution,
        "edit_file" | "multi_edit" | "create_file" | "write_note" | "download_asset" => {
            ToolRenderKind::FileChange
        }
        "fetch_url"
        | "browser_evidence_capture"
        | "web_search"
        | "web_research_context"
        | "read_file"
        | "read_files"
        | "extract_image_text"
        | "get_document_info"
        | "compare_documents"
        | "summarize_document"
        | "compile_document"
        | "search_knowledge_base"
        | "retrieve_evidence"
        | "search_files"
        | "search_playbooks"
        | "search_sessions"
        | "search_by_date"
        | "get_chunk_context"
        | "query_knowledge_graph"
        | "get_related_concepts"
        | "grep_files"
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

pub fn capability_input_streaming(name: &str) -> ToolInputStreamingMode {
    match name {
        "generate_image"
        | "download_asset"
        | "browser_evidence_capture"
        | "fetch_url"
        | "web_search"
        | "web_research_context"
        | "read_file"
        | "read_files"
        | "list_dir"
        | "glob_files"
        | "grep_files"
        | "run_shell"
        | "edit_file"
        | "multi_edit"
        | "create_file"
        | "write_note"
        | "extract_image_text"
        | "get_document_info"
        | "compare_documents"
        | "summarize_document"
        | "compile_document"
        | "search_knowledge_base"
        | "retrieve_evidence"
        | "search_files"
        | "search_playbooks"
        | "search_sessions"
        | "search_by_date"
        | "get_chunk_context"
        | "query_knowledge_graph"
        | "get_related_concepts"
        | "list_documents"
        | "list_sources"
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

pub fn capability_resource_keys(name: &str, args: &serde_json::Value) -> Vec<String> {
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
        "output_dir",
        "outputDir",
        "filename",
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
    for field in ["url", "finalUrl", "final_url"] {
        collect_string_or_array_resource(args, field, "web", &mut keys);
    }
    if name.starts_with("mcp__") {
        push_string_resource_key(&mut keys, "mcp", name);
    }
    keys
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
        "fetch_url" | "web_search" | "web_research_context" => (
            "web",
            true,
            false,
            false,
            true,
            false,
            ApprovalRisk::Low,
            "Reads remote web content or search results and crosses the local trust boundary.",
        ),
        "browser_evidence_capture" => (
            "web",
            true,
            false,
            false,
            true,
            true,
            ApprovalRisk::Medium,
            "Captures read-only browser/page evidence with provenance and crosses the local trust boundary.",
        ),
        "download_asset" => (
            "web",
            true,
            true,
            false,
            true,
            true,
            ApprovalRisk::Medium,
            "Downloads a remote image asset into the workspace after URL, content-type, size, and output-path validation.",
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
            "Reads the enabled tool catalog and can activate hidden matches for later model steps.",
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

pub fn fallback_registry_run_capabilities(
    name: &str,
    args: &serde_json::Value,
) -> ToolRunCapabilities {
    ToolRunCapabilities {
        input_streaming: capability_input_streaming(name),
        render_kind: capability_render_kind(name),
        read_only: !matches!(name, "mcp_tool") && !name.starts_with("mcp__"),
        destructive: false,
        concurrency_safe: true,
        interrupt_behavior: ToolInterruptBehavior::Block,
        resource_keys: capability_resource_keys(name, args),
    }
}

pub fn fallback_tool_access_profile(name: &str, args: &serde_json::Value) -> ToolAccessProfile {
    let capabilities = ToolRunCapabilities {
        input_streaming: capability_input_streaming(name),
        render_kind: capability_render_kind(name),
        read_only: !matches!(name, "mcp_tool") && !name.starts_with("mcp__"),
        destructive: false,
        concurrency_safe: true,
        interrupt_behavior: ToolInterruptBehavior::Block,
        resource_keys: capability_resource_keys(name, args),
    };
    infer_tool_access_profile(name, &[ToolCategory::Core], &capabilities, args)
}

pub fn capability_descriptor_for_tool(
    name: &str,
    categories: &[ToolCategory],
    capabilities: ToolRunCapabilities,
    args: &serde_json::Value,
) -> ToolCapabilityDescriptor {
    let access_profile = infer_tool_access_profile(name, categories, &capabilities, args);
    let display_category = access_profile.category.clone();
    ToolCapabilityDescriptor {
        name: name.to_string(),
        package: plugin_for_tool(name),
        categories: categories
            .iter()
            .map(|category| category_label(*category).to_string())
            .collect(),
        ui: ToolUiDescriptor {
            render_kind: capabilities.render_kind,
            display_category,
        },
        resources: ToolResourceDescriptor {
            keys: capabilities.resource_keys.clone(),
        },
        capabilities,
        access_profile,
    }
}
