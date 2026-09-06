//! Built-in capability package declarations and UI projections.

pub(crate) mod image_generation;
pub(crate) mod office_documents;
pub(crate) mod text_to_speech;

use crate::app_settings::AppConfig;
use crate::capability_package::{CapabilityPackageManifest, CapabilityPackagePermissions};
use crate::ecosystem::EcosystemSurfaceKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityOwner {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProviderCatalog {
    pub id: String,
    pub label: String,
    pub item_kind: String,
    pub items: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySettingsSchema {
    pub config_key: String,
    pub fields: Vec<CapabilitySettingsField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySettingsField {
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
pub enum CapabilityRuntimeStatus {
    Pass,
    Warning,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityCheckSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRuntimeCheck {
    pub id: String,
    pub label: String,
    pub status: CapabilityRuntimeStatus,
    pub severity: CapabilityCheckSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackageView {
    pub id: String,
    pub name: String,
    pub capability: String,
    pub description: String,
    pub built_in: bool,
    pub surface: EcosystemSurfaceKind,
    pub version: u32,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub workflows: Vec<String>,
    pub settings_surfaces: Vec<String>,
    pub permissions: CapabilityPackagePermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_schema: Option<CapabilitySettingsSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_catalogs: Vec<CapabilityProviderCatalog>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_checks: Vec<CapabilityRuntimeCheck>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CapabilityPackageViewContext<'a> {
    pub app_config: Option<&'a AppConfig>,
    pub office_runtime: Option<&'a crate::office_runtime::OfficeRuntimeReadiness>,
}

#[derive(Debug, Clone, Copy)]
struct BuiltinCapabilityDeclaration {
    id: &'static str,
    name: &'static str,
    capability: &'static str,
    description: &'static str,
    surface: EcosystemSurfaceKind,
    tools: &'static [&'static str],
    settings_surfaces: &'static [&'static str],
    workflows: &'static [&'static str],
}

impl BuiltinCapabilityDeclaration {
    fn info(self) -> CapabilityOwner {
        CapabilityOwner {
            id: self.id.to_string(),
            name: self.name.to_string(),
            capability: self.capability.to_string(),
            description: self.description.to_string(),
        }
    }

    fn manifest(self) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            id: self.id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            surface: self.surface,
            version: 1,
            tools: self.tools.iter().map(|tool| (*tool).to_string()).collect(),
            skills: Vec::new(),
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
            runtime_checks: Vec::new(),
            permissions: CapabilityPackagePermissions::default(),
        }
    }

    fn view(self, context: CapabilityPackageViewContext<'_>) -> CapabilityPackageView {
        let manifest = self.manifest();
        let view = CapabilityPackageView {
            id: manifest.id,
            name: manifest.name,
            capability: self.capability.to_string(),
            description: manifest.description,
            built_in: true,
            surface: manifest.surface,
            version: manifest.version,
            tools: manifest.tools,
            skills: manifest.skills,
            workflows: manifest.workflows,
            settings_surfaces: manifest.settings_surfaces,
            permissions: manifest.permissions,
            settings_schema: None,
            provider_catalogs: Vec::new(),
            runtime_checks: Vec::new(),
        };
        if self.id == IMAGE_PACKAGE.id {
            image_generation::enrich_manifest(
                view,
                context.app_config.map(|config| &config.image_generation),
            )
        } else if self.id == TTS_PACKAGE.id {
            text_to_speech::enrich_manifest(
                view,
                context.app_config.map(|config| &config.text_to_speech),
            )
        } else if self.id == OFFICE_PACKAGE.id {
            office_documents::enrich_manifest(view, context.office_runtime)
        } else {
            view
        }
    }

    fn owns_tool(self, name: &str) -> bool {
        self.tools.contains(&name)
    }
}

const CORE_AGENT_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "core-agent",
    name: "Core Agent",
    capability: "Run orchestration",
    description: "Routes tasks, coordinates user input, tracks plans, records verification, and manages the shared declarative appearance.",
    surface: EcosystemSurfaceKind::CorePlatform,
    tools: &[
        "tool_search",
        "update_plan",
        "get_goal",
        "update_goal",
        "request_user_input",
        "record_verification",
        "appearance",
    ],
    settings_surfaces: &["agent-quality", "tool-approvals", "appearance"],
    workflows: &["task-planning", "verification", "customize-appearance"],
};

const KNOWLEDGE_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "knowledge-base",
    name: "Knowledge Base",
    capability: "Local evidence retrieval",
    description: "Searches, indexes, and inspects local knowledge sources with evidence metadata.",
    surface: EcosystemSurfaceKind::CorePlatform,
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

const OFFICE_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "office-documents",
    name: "Office Documents",
    capability: "Document generation and analysis",
    description:
        "Prepares document runtimes and works with PPT, DOCX, XLSX, PDF, HTML, and OCR image flows.",
    surface: EcosystemSurfaceKind::CapabilityPackage,
    tools: &[
        "prepare_document_tools",
        "office_artifact",
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

const IMAGE_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "image-generation",
    name: "Image Generation",
    capability: "Image creation",
    description:
        "Routes image requests through provider-specific adapters and stores generated assets.",
    surface: EcosystemSurfaceKind::Adapter,
    tools: &["generate_image"],
    settings_surfaces: &["image-generation"],
    workflows: &["generate-image"],
};

const TTS_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "text-to-speech",
    name: "Text to Speech",
    capability: "Speech synthesis",
    description: "Synthesizes speech through cloud providers and returns a transient audio asset.",
    surface: EcosystemSurfaceKind::Adapter,
    tools: &["synthesize_speech"],
    settings_surfaces: &["text-to-speech"],
    workflows: &["synthesize-speech"],
};

const WEB_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "web-research",
    name: "Web Research",
    capability: "Network research",
    description:
        "Fetches remote pages and web search results with explicit network trust metadata.",
    surface: EcosystemSurfaceKind::Adapter,
    tools: &[
        "web_search",
        "web_research_context",
        "fetch_url",
        "browser_evidence_capture",
        "browser_session",
        "download_asset",
    ],
    settings_surfaces: &["network", "web-search"],
    workflows: &["research-web", "web-research-context"],
};

const FILE_WORKSPACE_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "file-workspace",
    name: "File Workspace",
    capability: "Scoped file work",
    description:
        "Reads, searches, edits, and archives files through source-scoped workspace tools.",
    surface: EcosystemSurfaceKind::CapabilityPackage,
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

const DESKTOP_AUTOMATION_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "desktop-automation",
    name: "Desktop Automation",
    capability: "System and desktop actions",
    description: "Runs local commands and controlled desktop actions behind approval gates.",
    surface: EcosystemSurfaceKind::HostSurface,
    tools: &[
        "run_shell",
        "activity_observe",
        "desktop_automation",
        "computer_observe",
        "computer_control",
        "terminal_session",
    ],
    settings_surfaces: &["tool-approvals"],
    workflows: &["run-command", "control-desktop"],
};

const MEMORY_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "agent-memory",
    name: "Agent Memory",
    capability: "Reusable working context",
    description:
        "Manages playbooks, sessions, skills, scratchpads, feedback, and durable agent memory.",
    surface: EcosystemSurfaceKind::CapabilityPackage,
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

const EVALUATION_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "agent-evaluation",
    name: "Agent Evaluation",
    capability: "Quality checks",
    description:
        "Runs readiness previews and quality checks for agent configuration and workflows.",
    surface: EcosystemSurfaceKind::CorePlatform,
    tools: &["agent_harness_dry_run"],
    settings_surfaces: &["agent-quality"],
    workflows: &["dry-run-agent"],
};

const DELEGATION_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "delegation",
    name: "Delegation",
    capability: "Subagent work",
    description: "Delegates bounded work to subagents and adjudicates their outputs.",
    surface: EcosystemSurfaceKind::CapabilityPackage,
    tools: &[
        "list_subagent_models",
        "spawn_subagent",
        "spawn_subagent_batch",
        "judge_subagent_results",
        "observe_subagent_batch",
        "observe_subagent",
        "wait_subagent",
        "send_subagent_input",
        "cancel_subagent",
        "close_subagent",
    ],
    settings_surfaces: &["agent-quality"],
    workflows: &["parallel-agent-work"],
};

const MCP_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "mcp-connectors",
    name: "MCP Connectors",
    capability: "External connectors",
    description:
        "Exposes server-defined tools from configured MCP connectors with explicit approval policy.",
    surface: EcosystemSurfaceKind::Connector,
    tools: &["mcp_tool"],
    settings_surfaces: &["mcp"],
    workflows: &["connector-tool-call"],
};

const COMPUTER_USE_CONNECTOR_PACKAGE: BuiltinCapabilityDeclaration = BuiltinCapabilityDeclaration {
    id: "computer-use-connector",
    name: "Computer Use Connector",
    capability: "Vision-guided UI automation",
    description:
        "Classifies tools from an isolated computer-use MCP service so observation and actions stay behind Nexa's connector approval boundary.",
    surface: EcosystemSurfaceKind::Connector,
    tools: &["mcp__computer_use__*"],
    settings_surfaces: &["mcp", "tool-approvals"],
    workflows: &["observe-decide-act"],
};

const BUILTIN_PACKAGES: &[BuiltinCapabilityDeclaration] = &[
    CORE_AGENT_PACKAGE,
    KNOWLEDGE_PACKAGE,
    OFFICE_PACKAGE,
    IMAGE_PACKAGE,
    TTS_PACKAGE,
    WEB_PACKAGE,
    FILE_WORKSPACE_PACKAGE,
    DESKTOP_AUTOMATION_PACKAGE,
    MEMORY_PACKAGE,
    EVALUATION_PACKAGE,
    DELEGATION_PACKAGE,
    COMPUTER_USE_CONNECTOR_PACKAGE,
    MCP_PACKAGE,
];

pub fn builtin_capability_views() -> Vec<CapabilityPackageView> {
    builtin_capability_views_for_config(None)
}

pub fn builtin_capability_views_for_config(
    app_config: Option<&AppConfig>,
) -> Vec<CapabilityPackageView> {
    builtin_capability_views_with_context(CapabilityPackageViewContext {
        app_config,
        office_runtime: None,
    })
}

pub fn builtin_capability_views_with_context(
    context: CapabilityPackageViewContext<'_>,
) -> Vec<CapabilityPackageView> {
    BUILTIN_PACKAGES
        .iter()
        .map(|package| package.view(context))
        .collect()
}

pub fn builtin_capability_manifests() -> Vec<CapabilityPackageManifest> {
    BUILTIN_PACKAGES
        .iter()
        .map(|package| package.manifest())
        .collect()
}

pub fn capability_owner_for_tool(name: &str) -> CapabilityOwner {
    capability_for_tool_name(name).info()
}

fn capability_for_tool_name(name: &str) -> BuiltinCapabilityDeclaration {
    if is_computer_use_connector_tool(name) {
        return COMPUTER_USE_CONNECTOR_PACKAGE;
    }
    if name == "mcp_tool" || name.starts_with("mcp__") {
        return MCP_PACKAGE;
    }
    BUILTIN_PACKAGES
        .iter()
        .copied()
        .find(|package| package.owns_tool(name))
        .unwrap_or(CORE_AGENT_PACKAGE)
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
        assert_eq!(
            capability_owner_for_tool("generate_image").id,
            "image-generation"
        );
        assert_eq!(
            capability_owner_for_tool("synthesize_speech").id,
            "text-to-speech"
        );
        assert_eq!(
            capability_owner_for_tool("compile_document").id,
            "office-documents"
        );
        assert_eq!(
            capability_owner_for_tool("run_shell").id,
            "desktop-automation"
        );
        assert_eq!(capability_owner_for_tool("web_search").id, "web-research");
        assert_eq!(
            capability_owner_for_tool("web_research_context").id,
            "web-research"
        );
        assert_eq!(
            capability_owner_for_tool("download_asset").id,
            "web-research"
        );
        assert_eq!(
            capability_owner_for_tool("mcp__custom__dangerous").id,
            "mcp-connectors"
        );
        assert_eq!(
            capability_owner_for_tool("mcp__computer_use__computer").id,
            "computer-use-connector"
        );
        assert_eq!(
            capability_owner_for_tool("mcp__computer-use__screenshot").id,
            "computer-use-connector"
        );
    }

    #[test]
    fn question_tool_belongs_to_the_core_agent_package() {
        assert_eq!(
            capability_owner_for_tool("request_user_input").id,
            "core-agent"
        );
    }

    #[test]
    fn appearance_tool_shares_the_core_agent_and_settings_surface() {
        assert_eq!(capability_owner_for_tool("appearance").id, "core-agent");
        let core = builtin_capability_views()
            .into_iter()
            .find(|package| package.id == "core-agent")
            .expect("missing core agent package");
        assert!(core
            .settings_surfaces
            .iter()
            .any(|surface| surface == "appearance"));
    }

    #[test]
    fn builtin_manifests_expose_package_metadata() {
        let manifests = builtin_capability_views();
        let office = manifests
            .iter()
            .find(|package| package.id == "office-documents")
            .expect("missing office capability package");

        assert!(office.tools.iter().any(|tool| tool == "compile_document"));
        assert!(office
            .settings_surfaces
            .iter()
            .any(|surface| surface == "office-runtime"));
    }

    #[test]
    fn builtin_manifests_classify_ecosystem_surfaces() {
        let manifests = builtin_capability_views();

        let kind_for = |id: &str| {
            manifests
                .iter()
                .find(|manifest| manifest.id == id)
                .map(|manifest| manifest.surface)
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
    fn builtin_views_project_canonical_capability_manifests() {
        let views = builtin_capability_views();
        let capability_manifests = builtin_capability_manifests();

        assert_eq!(capability_manifests.len(), views.len());
        for capability_manifest in &capability_manifests {
            crate::capability_package::validate_capability_manifest(capability_manifest)
                .expect("builtin capability manifest should validate");
            let view = views
                .iter()
                .find(|manifest| manifest.id == capability_manifest.id)
                .expect("capability manifest should project to a view");

            assert_eq!(capability_manifest.name, view.name);
            assert_eq!(capability_manifest.surface, view.surface);
            assert_eq!(capability_manifest.description, view.description);
            assert_eq!(capability_manifest.tools, view.tools);
            assert_eq!(
                capability_manifest.settings_surfaces,
                view.settings_surfaces
            );
            assert_eq!(capability_manifest.workflows, view.workflows);
        }
    }

    #[test]
    fn default_registry_tools_are_declared_by_one_builtin_manifest() {
        let manifests = builtin_capability_views();
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
