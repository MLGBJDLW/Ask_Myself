use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::tools::default_tool_registry;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccessInfo {
    pub name: String,
    pub category: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_execute: bool,
    pub can_access_network: bool,
    pub needs_approval: bool,
    pub risk_level: ApprovalRisk,
    pub risk_reason: String,
}

pub fn tool_access_map() -> Vec<ToolAccessInfo> {
    tool_access_map_for_names(default_tool_registry().tool_names())
}

pub fn tool_access_map_with_extra_names<I, S>(extra_names: I) -> Vec<ToolAccessInfo>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut names = default_tool_registry().tool_names();
    names.extend(
        extra_names
            .into_iter()
            .map(|name| name.as_ref().to_string()),
    );
    tool_access_map_for_names(names)
}

pub fn tool_access_map_for_names<I, S>(names: I) -> Vec<ToolAccessInfo>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = BTreeSet::new();
    let mut tools = names
        .into_iter()
        .filter_map(|name| {
            let name = name.as_ref().to_string();
            if seen.insert(name.clone()) {
                Some(describe_tool_access(&name))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    tools.sort_by(|a, b| {
        risk_rank(b.risk_level)
            .cmp(&risk_rank(a.risk_level))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });
    tools
}

fn risk_rank(risk: ApprovalRisk) -> u8 {
    match risk {
        ApprovalRisk::Low => 0,
        ApprovalRisk::Medium => 1,
        ApprovalRisk::High => 2,
    }
}

pub fn describe_tool_access(name: &str) -> ToolAccessInfo {
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
            "Executes local shell commands and can affect files, processes, and network.",
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
        "create_file" | "write_note" => (
            "filesystem",
            false,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
            "Creates or overwrites local files.",
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
            "Prepares required document-processing helpers; optional Poppler/LibreOffice setup requires explicit selection.",
        ),
        "manage_source" => (
            "source_management",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
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
        | "grep_files" => (
            "filesystem",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads local files or directories.",
        ),
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
        _ => (
            "core",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Read-only or low-risk local agent helper.",
        ),
    };

    ToolAccessInfo {
        name: name.to_string(),
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

fn is_builtin_web_search_mcp_tool(name: &str) -> bool {
    name == "mcp__web_search__search" || name.starts_with("mcp__web_search__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_access_map_marks_high_risk_and_office_tools() {
        let map = tool_access_map();
        let by_name = |name: &str| {
            map.iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"))
        };

        let shell = by_name("run_shell");
        assert!(shell.can_execute);
        assert!(shell.needs_approval);
        assert_eq!(shell.risk_level, ApprovalRisk::High);

        let editor = by_name("edit_file");
        assert!(editor.can_write);
        assert!(editor.needs_approval);

        let fetch = by_name("fetch_url");
        assert!(fetch.can_access_network);
        assert!(!fetch.can_write);

        let search = by_name("search_knowledge_base");
        assert_eq!(search.category, "knowledge");
        assert!(!search.can_write);

        let memory = by_name("manage_agent_memory");
        assert_eq!(memory.category, "memory");
        assert!(memory.can_write);

        let office = by_name("get_document_info");
        assert_eq!(office.category, "document_analysis");
        assert!(office.can_read);
        assert!(!office.can_write);
    }

    #[test]
    fn extra_runtime_tools_are_classified() {
        let map = tool_access_map_with_extra_names([
            "spawn_subagent",
            "judge_subagent_results",
            "mcp__web_search__search",
            "mcp__unknown__dangerous",
        ]);
        let by_name = |name: &str| {
            map.iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"))
        };

        assert_eq!(by_name("spawn_subagent").category, "delegation");
        assert_eq!(
            by_name("judge_subagent_results").risk_level,
            ApprovalRisk::Low
        );

        let web_search = by_name("mcp__web_search__search");
        assert_eq!(web_search.category, "web");
        assert!(web_search.can_access_network);
        assert!(!web_search.can_write);

        let unknown_mcp = by_name("mcp__unknown__dangerous");
        assert_eq!(unknown_mcp.category, "mcp");
        assert_eq!(unknown_mcp.risk_level, ApprovalRisk::High);
    }
}
