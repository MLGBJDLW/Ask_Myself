//! Built-in ecosystem surface metadata.
//!
//! The public type names still use `Plugin` for compatibility with existing
//! desktop calls, but this module classifies built-in capabilities by their
//! ecosystem surface so "plugin" does not become the umbrella term.

pub(crate) mod image_generation;
pub(crate) mod office_documents;
pub(crate) mod text_to_speech;

use crate::app_settings::AppConfig;
use crate::capability_package::{CapabilityPackageManifest, CapabilityPackagePermissions};
use crate::ecosystem::EcosystemSurfaceKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolPluginInfo {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginProviderCatalog {
    pub id: String,
    pub label: String,
    pub item_kind: String,
    pub items: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsSchema {
    pub config_key: String,
    pub fields: Vec<PluginSettingsField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsField {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PluginRuntimeStatus {
    Pass,
    Warning,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PluginCheckSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeCheck {
    pub id: String,
    pub label: String,
    pub status: PluginRuntimeStatus,
    pub severity: PluginCheckSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
    pub built_in: bool,
    pub ecosystem_surface: EcosystemSurfaceKind,
    pub tools: Vec<String>,
    pub settings_surfaces: Vec<String>,
    pub workflows: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_schema: Option<PluginSettingsSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_catalogs: Vec<PluginProviderCatalog>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_checks: Vec<PluginRuntimeCheck>,
}

impl PluginManifest {
    pub fn to_capability_manifest(&self) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            id: self.id.clone(),
            name: self.name.clone(),
            surface: self.ecosystem_surface,
            description: self.description.clone(),
            version: 1,
            tools: self.tools.clone(),
            skills: Vec::new(),
            workflows: self.workflows.clone(),
            settings_surfaces: self.settings_surfaces.clone(),
            runtime_checks: self
                .runtime_checks
                .iter()
                .map(|check| check.id.clone())
                .collect(),
            permissions: CapabilityPackagePermissions::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PluginManifestContext<'a> {
    pub app_config: Option<&'a AppConfig>,
    pub office_runtime: Option<&'a crate::office_runtime::OfficeRuntimeReadiness>,
}

#[derive(Debug, Clone, Copy)]
struct BuiltinPlugin {
    id: &'static str,
    name: &'static str,
    capability: &'static str,
    description: &'static str,
    ecosystem_surface: EcosystemSurfaceKind,
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

    fn base_manifest(self) -> PluginManifest {
        PluginManifest {
            id: self.id.to_string(),
            name: self.name.to_string(),
            capability: self.capability.to_string(),
            description: self.description.to_string(),
            built_in: true,
            ecosystem_surface: self.ecosystem_surface,
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
            settings_schema: None,
            provider_catalogs: Vec::new(),
            runtime_checks: Vec::new(),
        }
    }

    fn manifest(self, context: PluginManifestContext<'_>) -> PluginManifest {
        let manifest = self.base_manifest();
        if self.id == IMAGE_PLUGIN.id {
            image_generation::enrich_manifest(
                manifest,
                context.app_config.map(|config| &config.image_generation),
            )
        } else if self.id == TTS_PLUGIN.id {
            text_to_speech::enrich_manifest(
                manifest,
                context.app_config.map(|config| &config.text_to_speech),
            )
        } else if self.id == OFFICE_PLUGIN.id {
            office_documents::enrich_manifest(manifest, context.office_runtime)
        } else {
            manifest
        }
    }

    fn owns_tool(self, name: &str) -> bool {
        self.tools.contains(&name)
    }
}

const CORE_AGENT_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "core-agent",
    name: "Core Agent",
    capability: "Run orchestration",
    description:
        "Routes tasks, tracks plans, and records verification without owning domain tools.",
    ecosystem_surface: EcosystemSurfaceKind::CorePlatform,
    tools: &[
        "tool_search",
        "update_plan",
        "get_goal",
        "update_goal",
        "request_user_input",
        "record_verification",
    ],
    settings_surfaces: &["agent-quality", "tool-approvals"],
    workflows: &["task-planning", "verification"],
};

const KNOWLEDGE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "knowledge-base",
    name: "Knowledge Base",
    capability: "Local evidence retrieval",
    description: "Searches, indexes, and inspects local knowledge sources with evidence metadata.",
    ecosystem_surface: EcosystemSurfaceKind::CorePlatform,
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
        "Prepares document runtimes and works with PPT, DOCX, XLSX, PDF, HTML, and OCR image flows.",
    ecosystem_surface: EcosystemSurfaceKind::CapabilityPackage,
    tools: &[
        "prepare_document_tools",
        "get_document_info",
        #[cfg(feature = "ocr")]
        "extract_image_text",
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
    ecosystem_surface: EcosystemSurfaceKind::Adapter,
    tools: &["generate_image"],
    settings_surfaces: &["image-generation"],
    workflows: &["generate-image"],
};

const TTS_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "text-to-speech",
    name: "Text to Speech",
    capability: "Speech synthesis",
    description: "Synthesizes speech through cloud providers and returns a transient audio asset.",
    ecosystem_surface: EcosystemSurfaceKind::Adapter,
    tools: &["synthesize_speech"],
    settings_surfaces: &["text-to-speech"],
    workflows: &["synthesize-speech"],
};

const WEB_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "web-research",
    name: "Web Research",
    capability: "Network research",
    description:
        "Fetches remote pages and web search results with explicit network trust metadata.",
    ecosystem_surface: EcosystemSurfaceKind::Adapter,
    tools: &[
        "web_search",
        "web_research_context",
        "fetch_url",
        "browser_evidence_capture",
        "download_asset",
    ],
    settings_surfaces: &["network", "web-search"],
    workflows: &["research-web", "web-research-context"],
};

const FILE_WORKSPACE_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "file-workspace",
    name: "File Workspace",
    capability: "Scoped file work",
    description:
        "Reads, searches, edits, and archives files through source-scoped workspace tools.",
    ecosystem_surface: EcosystemSurfaceKind::CapabilityPackage,
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
    ecosystem_surface: EcosystemSurfaceKind::HostSurface,
    tools: &["run_shell", "desktop_automation", "terminal_session"],
    settings_surfaces: &["tool-approvals"],
    workflows: &["run-command", "control-desktop"],
};

const MEMORY_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "agent-memory",
    name: "Agent Memory",
    capability: "Reusable working context",
    description:
        "Manages playbooks, sessions, skills, scratchpads, feedback, and durable agent memory.",
    ecosystem_surface: EcosystemSurfaceKind::CapabilityPackage,
    tools: &[
        "search_playbooks",
        "manage_playbook",
        "search_sessions",
        "manage_persona",
        "manage_user_memory",
        "manage_project_memory",
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
    ecosystem_surface: EcosystemSurfaceKind::CorePlatform,
    tools: &["agent_harness_dry_run"],
    settings_surfaces: &["agent-quality"],
    workflows: &["dry-run-agent"],
};

const DELEGATION_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "delegation",
    name: "Delegation",
    capability: "Subagent work",
    description: "Delegates bounded work to subagents and adjudicates their outputs.",
    ecosystem_surface: EcosystemSurfaceKind::CapabilityPackage,
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
    ecosystem_surface: EcosystemSurfaceKind::Connector,
    tools: &["mcp_tool"],
    settings_surfaces: &["mcp"],
    workflows: &["connector-tool-call"],
};

const COMPUTER_USE_CONNECTOR_PLUGIN: BuiltinPlugin = BuiltinPlugin {
    id: "computer-use-connector",
    name: "Computer Use Connector",
    capability: "Vision-guided UI automation",
    description:
        "Classifies tools from an isolated computer-use MCP service so observation and actions stay behind Nexa's connector approval boundary.",
    ecosystem_surface: EcosystemSurfaceKind::Connector,
    tools: &["mcp__computer_use__*"],
    settings_surfaces: &["mcp", "tool-approvals"],
    workflows: &["observe-decide-act"],
};

const BUILTIN_PLUGINS: &[BuiltinPlugin] = &[
    CORE_AGENT_PLUGIN,
    KNOWLEDGE_PLUGIN,
    OFFICE_PLUGIN,
    IMAGE_PLUGIN,
    TTS_PLUGIN,
    WEB_PLUGIN,
    FILE_WORKSPACE_PLUGIN,
    DESKTOP_AUTOMATION_PLUGIN,
    MEMORY_PLUGIN,
    EVALUATION_PLUGIN,
    DELEGATION_PLUGIN,
    COMPUTER_USE_CONNECTOR_PLUGIN,
    MCP_PLUGIN,
];

pub fn builtin_plugin_manifests() -> Vec<PluginManifest> {
    builtin_plugin_manifests_for_config(None)
}

pub fn builtin_plugin_manifests_for_config(app_config: Option<&AppConfig>) -> Vec<PluginManifest> {
    builtin_plugin_manifests_with_context(PluginManifestContext {
        app_config,
        office_runtime: None,
    })
}

pub fn builtin_plugin_manifests_with_context(
    context: PluginManifestContext<'_>,
) -> Vec<PluginManifest> {
    BUILTIN_PLUGINS
        .iter()
        .map(|plugin| plugin.manifest(context))
        .collect()
}

pub fn builtin_capability_manifests() -> Vec<CapabilityPackageManifest> {
    builtin_capability_manifests_with_context(PluginManifestContext::default())
}

pub fn builtin_capability_manifests_with_context(
    context: PluginManifestContext<'_>,
) -> Vec<CapabilityPackageManifest> {
    builtin_plugin_manifests_with_context(context)
        .into_iter()
        .map(|manifest| manifest.to_capability_manifest())
        .collect()
}

pub fn plugin_for_tool(name: &str) -> ToolPluginInfo {
    plugin_for_tool_name(name).info()
}

fn plugin_for_tool_name(name: &str) -> BuiltinPlugin {
    if is_computer_use_connector_tool(name) {
        return COMPUTER_USE_CONNECTOR_PLUGIN;
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

fn is_computer_use_connector_tool(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    normalized.starts_with("mcp__computer_use__")
        || normalized.starts_with("mcp__windows_computer_use__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::EcosystemSurfaceKind;

    #[test]
    fn maps_tools_to_capability_packages() {
        assert_eq!(plugin_for_tool("request_user_input").id, "core-agent");
        assert_eq!(plugin_for_tool("generate_image").id, "image-generation");
        assert_eq!(plugin_for_tool("synthesize_speech").id, "text-to-speech");
        assert_eq!(plugin_for_tool("compile_document").id, "office-documents");
        assert_eq!(plugin_for_tool("run_shell").id, "desktop-automation");
        assert_eq!(plugin_for_tool("web_search").id, "web-research");
        assert_eq!(plugin_for_tool("web_research_context").id, "web-research");
        assert_eq!(plugin_for_tool("download_asset").id, "web-research");
        assert_eq!(
            plugin_for_tool("mcp__custom__dangerous").id,
            "mcp-connectors"
        );
        assert_eq!(
            plugin_for_tool("mcp__computer_use__computer").id,
            "computer-use-connector"
        );
        assert_eq!(
            plugin_for_tool("mcp__computer-use__screenshot").id,
            "computer-use-connector"
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

    #[test]
    fn builtin_manifests_classify_ecosystem_surfaces() {
        let manifests = builtin_plugin_manifests();

        let kind_for = |id: &str| {
            manifests
                .iter()
                .find(|manifest| manifest.id == id)
                .map(|manifest| manifest.ecosystem_surface)
        };

        assert_eq!(
            kind_for("core-agent"),
            Some(EcosystemSurfaceKind::CorePlatform)
        );
        assert_eq!(
            kind_for("knowledge-base"),
            Some(EcosystemSurfaceKind::CorePlatform)
        );
        assert_eq!(
            kind_for("office-documents"),
            Some(EcosystemSurfaceKind::CapabilityPackage)
        );
        assert_eq!(
            kind_for("image-generation"),
            Some(EcosystemSurfaceKind::Adapter)
        );
        assert_eq!(
            kind_for("web-research"),
            Some(EcosystemSurfaceKind::Adapter)
        );
        assert_eq!(
            kind_for("desktop-automation"),
            Some(EcosystemSurfaceKind::HostSurface)
        );
        assert_eq!(
            kind_for("mcp-connectors"),
            Some(EcosystemSurfaceKind::Connector)
        );
        assert_eq!(
            kind_for("computer-use-connector"),
            Some(EcosystemSurfaceKind::Connector)
        );
    }

    #[test]
    fn builtin_capability_manifests_derive_from_compat_manifests() {
        let compat_manifests = builtin_plugin_manifests();
        let capability_manifests = builtin_capability_manifests();

        assert_eq!(capability_manifests.len(), compat_manifests.len());
        for capability_manifest in &capability_manifests {
            crate::capability_package::validate_capability_manifest(capability_manifest)
                .expect("builtin capability manifest should validate");
            let compat = compat_manifests
                .iter()
                .find(|manifest| manifest.id == capability_manifest.id)
                .expect("capability manifest should come from compat manifest");

            assert_eq!(capability_manifest.name, compat.name);
            assert_eq!(capability_manifest.surface, compat.ecosystem_surface);
            assert_eq!(capability_manifest.description, compat.description);
            assert_eq!(capability_manifest.tools, compat.tools);
            assert_eq!(
                capability_manifest.settings_surfaces,
                compat.settings_surfaces
            );
            assert_eq!(capability_manifest.workflows, compat.workflows);
        }
    }

    #[test]
    fn default_registry_tools_are_declared_by_one_builtin_manifest() {
        let manifests = builtin_plugin_manifests();
        let registry = crate::tools::default_tool_registry();

        for tool_name in registry.tool_names() {
            let owners = manifests
                .iter()
                .filter(|manifest| manifest.tools.iter().any(|tool| tool == &tool_name))
                .map(|manifest| manifest.id.as_str())
                .collect::<Vec<_>>();

            assert_eq!(
                owners.len(),
                1,
                "{tool_name} should be declared by exactly one built-in ecosystem manifest; owners: {owners:?}"
            );
        }
    }
}
