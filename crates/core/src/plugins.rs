//! Built-in plugin metadata.
//!
//! This is the first plugin-host seam: capabilities are grouped as coherent
//! packages before their implementations are moved behind package-specific
//! modules.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolPluginInfo {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
    pub built_in: bool,
    pub tools: Vec<String>,
    pub settings_surfaces: Vec<String>,
    pub workflows: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct BuiltinPlugin {
    id: &'static str,
    name: &'static str,
    capability: &'static str,
    description: &'static str,
    tools: &'static [&'static str],
    settings_surfaces: &'static [&'static str],
    workflows: &'static [&'static str],
}

impl BuiltinPlugin {
    fn info(self) -> ToolPluginInfo {
        ToolPluginInfo {
            id: self.id.to_string(),
            name: self.name.to_string(),
            capability: self.capability.to_string(),
            description: self.description.to_string(),
        }
    }

    fn manifest(self) -> PluginManifest {
        PluginManifest {
            id: self.id.to_string(),
            name: self.name.to_string(),
            capability: self.capability.to_string(),
            description: self.description.to_string(),
            built_in: true,
            tools: self.tools.iter().map(|tool| (*tool).to_string()).collect(),
            settings_surfaces: self
                .settings_surfaces
                .iter()
                .map(|surface| (*surface).to_string())
                .collect(),
            workflows: self
                .workflows
                .iter()
                .map(|workflow| (*workflow).to_string())
                .collect(),
        }
    }

    fn owns_tool(self, name: &str) -> bool {
        self.tools.iter().any(|tool| *tool == name)
    }
}

const CORE_AGENT_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "core-agent",
    name: "Core Agent",
    capability: "Run orchestration",
    description:
        "Routes tasks, tracks plans, and records verification without owning domain tools.",
    tools: &["tool_search", "update_plan", "record_verification"],
    settings_surfaces: &["agent-quality", "tool-approvals"],
    workflows: &["task-planning", "verification"],
};

const KNOWLEDGE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "knowledge-base",
    name: "Knowledge Base",
    capability: "Local evidence retrieval",
    description: "Searches, indexes, and inspects local knowledge sources with evidence metadata.",
    tools: &[
        "search_knowledge_base",
        "retrieve_evidence",
        "list_sources",
        "list_documents",
        "manage_source",
        "reindex_document",
        "get_statistics",
        "search_by_date",
        "get_chunk_context",
        "query_knowledge_graph",
        "get_related_concepts",
        "run_health_check",
    ],
    settings_surfaces: &["sources", "data-privacy"],
    workflows: &["ask-knowledge-base", "manage-sources"],
};

const OFFICE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "office-documents",
    name: "Office Documents",
    capability: "Document generation and analysis",
    description:
        "Prepares document runtimes and works with PPT, DOCX, XLSX, PDF, and HTML document flows.",
    tools: &[
        "prepare_document_tools",
        "get_document_info",
        "compare_documents",
        "summarize_document",
        "compile_document",
    ],
    settings_surfaces: &["office-runtime"],
    workflows: &[
        "generate-presentation",
        "analyze-document",
        "compare-documents",
    ],
};

const IMAGE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "image-generation",
    name: "Image Generation",
    capability: "Image creation",
    description:
        "Routes image requests through provider-specific adapters and stores generated assets.",
    tools: &["generate_image"],
    settings_surfaces: &["image-generation"],
    workflows: &["generate-image"],
};

const WEB_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "web-research",
    name: "Web Research",
    capability: "Network research",
    description:
        "Fetches remote pages and web search results with explicit network trust metadata.",
    tools: &["fetch_url", "mcp__web_search__search"],
    settings_surfaces: &["mcp", "network"],
    workflows: &["research-web"],
};

const FILE_WORKSPACE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "file-workspace",
    name: "File Workspace",
    capability: "Scoped file work",
    description:
        "Reads, searches, edits, and archives files through source-scoped workspace tools.",
    tools: &[
        "read_file",
        "read_files",
        "list_dir",
        "glob_files",
        "search_files",
        "grep_files",
        "code_intelligence",
        "edit_file",
        "multi_edit",
        "create_file",
        "write_note",
        "archive_output",
        "project_tool",
    ],
    settings_surfaces: &["data-privacy", "project-tools"],
    workflows: &["inspect-files", "edit-files", "run-project-tool"],
};

const DESKTOP_AUTOMATION_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "desktop-automation",
    name: "Desktop Automation",
    capability: "System and desktop actions",
    description: "Runs local commands and controlled desktop actions behind approval gates.",
    tools: &["run_shell", "desktop_automation"],
    settings_surfaces: &["tool-approvals"],
    workflows: &["run-command", "control-desktop"],
};

const MEMORY_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "agent-memory",
    name: "Agent Memory",
    capability: "Reusable working context",
    description:
        "Manages playbooks, sessions, skills, scratchpads, feedback, and durable agent memory.",
    tools: &[
        "search_playbooks",
        "manage_playbook",
        "search_sessions",
        "manage_agent_memory",
        "update_scratchpad",
        "manage_skill",
        "submit_feedback",
    ],
    settings_surfaces: &["extensions"],
    workflows: &["reuse-playbooks", "manage-memory", "manage-skills"],
};

const EVALUATION_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "agent-evaluation",
    name: "Agent Evaluation",
    capability: "Quality checks",
    description:
        "Runs readiness previews and quality checks for agent configuration and workflows.",
    tools: &["agent_harness_dry_run"],
    settings_surfaces: &["agent-quality"],
    workflows: &["dry-run-agent"],
};

const DELEGATION_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "delegation",
    name: "Delegation",
    capability: "Subagent work",
    description: "Delegates bounded work to subagents and adjudicates their outputs.",
    tools: &[
        "spawn_subagent",
        "spawn_subagent_batch",
        "judge_subagent_results",
    ],
    settings_surfaces: &["agent-quality"],
    workflows: &["parallel-agent-work"],
};

const MCP_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "mcp-connectors",
    name: "MCP Connectors",
    capability: "External connectors",
    description:
        "Exposes server-defined tools from configured MCP connectors with explicit approval policy.",
    tools: &["mcp_tool"],
    settings_surfaces: &["mcp"],
    workflows: &["connector-tool-call"],
};

const BUILTIN_PLUGINS: &[BuiltinPlugin] = &[
    CORE_AGENT_PLUGIN,
    KNOWLEDGE_PLUGIN,
    OFFICE_PLUGIN,
    IMAGE_PLUGIN,
    WEB_PLUGIN,
    FILE_WORKSPACE_PLUGIN,
    DESKTOP_AUTOMATION_PLUGIN,
    MEMORY_PLUGIN,
    EVALUATION_PLUGIN,
    DELEGATION_PLUGIN,
    MCP_PLUGIN,
];

pub fn builtin_plugin_manifests() -> Vec<PluginManifest> {
    BUILTIN_PLUGINS
        .iter()
        .map(|plugin| plugin.manifest())
        .collect()
}

pub fn plugin_for_tool(name: &str) -> ToolPluginInfo {
    plugin_for_tool_name(name).info()
}

fn plugin_for_tool_name(name: &str) -> BuiltinPlugin {
    if name == "mcp__web_search__search" || name.starts_with("mcp__web_search__") {
        return WEB_PLUGIN;
    }
    if name == "mcp_tool" || name.starts_with("mcp__") {
        return MCP_PLUGIN;
    }
    BUILTIN_PLUGINS
        .iter()
        .copied()
        .find(|plugin| plugin.owns_tool(name))
        .unwrap_or(CORE_AGENT_PLUGIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tools_to_capability_packages() {
        assert_eq!(plugin_for_tool("generate_image").id, "image-generation");
        assert_eq!(plugin_for_tool("compile_document").id, "office-documents");
        assert_eq!(plugin_for_tool("run_shell").id, "desktop-automation");
        assert_eq!(
            plugin_for_tool("mcp__web_search__search").id,
            "web-research"
        );
        assert_eq!(
            plugin_for_tool("mcp__custom__dangerous").id,
            "mcp-connectors"
        );
    }

    #[test]
    fn builtin_manifests_expose_package_metadata() {
        let manifests = builtin_plugin_manifests();
        let office = manifests
            .iter()
            .find(|plugin| plugin.id == "office-documents")
            .expect("missing office plugin");

        assert!(office.tools.iter().any(|tool| tool == "compile_document"));
        assert!(office
            .settings_surfaces
            .iter()
            .any(|surface| surface == "office-runtime"));
    }
}
