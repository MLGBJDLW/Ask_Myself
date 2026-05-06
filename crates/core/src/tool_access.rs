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
    let registry = default_tool_registry();
    let mut tools = registry
        .tool_names()
        .into_iter()
        .map(|name| describe_tool_access(&name))
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

fn describe_tool_access(name: &str) -> ToolAccessInfo {
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
        "edit_file" => (
            "filesystem",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::High,
            "Modifies existing files and should pass through the write approval gate.",
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
        "reindex_document" | "compile_document" => (
            "source_management",
            true,
            true,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Refreshes derived knowledge indexes without directly editing user files.",
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
        "read_file" | "read_files" | "list_dir" => (
            "filesystem",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads local files or directories.",
        ),
        "search" | "retrieve_evidence" | "list_sources" | "list_documents" | "date_search" => (
            "knowledge",
            true,
            false,
            false,
            false,
            false,
            ApprovalRisk::Low,
            "Reads indexed local knowledge as evidence.",
        ),
        "agent_memory" | "update_scratchpad" | "manage_skill" => (
            "memory",
            true,
            true,
            false,
            false,
            true,
            ApprovalRisk::Medium,
            "Changes persistent agent memory, skills, or working notes.",
        ),
        "mcp_tool" => (
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

        let office = by_name("get_document_info");
        assert_eq!(office.category, "document_analysis");
        assert!(office.can_read);
        assert!(!office.can_write);
    }
}
