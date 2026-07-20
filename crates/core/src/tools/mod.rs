//! Tool system — trait, registry, and built-in tools for the agent framework.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::app_settings::ShellAccessMode;
use crate::approval::{ApprovalRisk, ToolApprovalMode};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolDefinition;
use crate::models::Source;
use crate::plugins::ToolPluginInfo;
use crate::tool_visibility_policy::{
    decide_tool_visibility, ToolVisibilityDecision, ToolVisibilityInput,
};

pub mod capability;
pub use capability::{
    capability_descriptor_for_tool, fallback_tool_access_profile, infer_tool_access_profile,
    ToolCapabilityDescriptor, ToolCategory, ToolResourceDescriptor, ToolUiDescriptor,
};
use capability::{
    capability_input_streaming, capability_render_kind, capability_resource_keys,
    fallback_registry_run_capabilities,
};

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
pub mod browser_evidence_tool;
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
pub mod download_asset_tool;
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
#[cfg(feature = "ocr")]
pub mod ocr_tool;
pub mod path_utils;
pub mod persona_tool;
pub mod playbook_tool;
pub mod prepare_document_tools_tool;
pub mod project_memory_tool;
pub mod project_tool;
pub mod read_files_tool;
pub mod record_verification_tool;
pub mod reindex_tool;
pub mod related_concepts_tool;
pub(crate) mod run_shell_contract;
pub mod run_shell_tool;
pub mod scratchpad_tool;
pub mod search_files_tool;
pub mod search_playbooks_tool;
pub mod search_tool;
pub mod session_search_tool;
pub mod statistics_tool;
pub mod submit_feedback_tool;
pub mod summarize_tool;
pub(crate) mod text_match;
pub mod text_to_speech_tool;
pub mod tool_search_tool;
pub mod update_plan_tool;
pub mod user_memory_tool;
pub mod web_research_context_tool;
pub mod web_search_tool;
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
    pub tool_registry: Option<&'a ToolRegistry>,
    pub cancel_token: Option<&'a tokio_util::sync::CancellationToken>,
}

pub fn invocation_waits_for_previous(args: &serde_json::Value) -> bool {
    args.get("wait_for_previous")
        .or_else(|| args.get("waitForPrevious"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
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
        capability_render_kind(self.name())
    }

    /// Whether this tool can safely receive arguments before the final JSON is
    /// complete. Default is no streaming; tools can opt into UI preview or true
    /// partial-input consumption once their implementation supports it.
    fn input_streaming(&self) -> ToolInputStreamingMode {
        capability_input_streaming(self.name())
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
        capability_resource_keys(self.name(), args)
    }

    /// Runtime capabilities used by the ToolRun lifecycle.
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

    /// Unified manifest for runtime, permissions, UI projection, and resources.
    fn capability_descriptor(&self, args: &serde_json::Value) -> ToolCapabilityDescriptor {
        capability_descriptor_for_tool(
            self.name(),
            self.categories(),
            self.run_capabilities(args),
            args,
        )
    }

    /// Canonical permission and risk descriptor for this invocation.
    fn access_profile(&self, args: &serde_json::Value) -> ToolAccessProfile {
        self.capability_descriptor(args).access_profile
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

fn stable_tool_definitions(mut definitions: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    definitions.sort_by(|a, b| a.name.cmp(&b.name));
    definitions
}

const RESIDENT_DISCOVERY_TOOL_NAMES: &[&str] = &["tool_search"];

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
        stable_tool_definitions(self.tools.iter().map(|t| t.definition()).collect())
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

    /// Build the tool surface exposed during Plan Mode.
    ///
    /// Plan Mode may inspect local and remote evidence, but it must not mutate
    /// state, execute commands, delegate to other agents, or expose tools whose
    /// schema mixes read-only and write actions.
    pub fn plan_mode_filtered(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        let empty_args = serde_json::json!({});

        for tool in &self.tools {
            if plan_mode_allows_tool(tool.name(), &tool.access_profile(&empty_args)) {
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
            .unwrap_or_else(|| fallback_registry_run_capabilities(name, args))
    }

    pub fn capability_descriptor(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> ToolCapabilityDescriptor {
        self.get(name)
            .map(|tool| tool.capability_descriptor(args))
            .unwrap_or_else(|| {
                capability_descriptor_for_tool(
                    name,
                    &[ToolCategory::Core],
                    fallback_registry_run_capabilities(name, args),
                    args,
                )
            })
    }

    pub fn access_profile(&self, name: &str, args: &serde_json::Value) -> ToolAccessProfile {
        self.capability_descriptor(name, args).access_profile
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
        let descriptor = self.capability_descriptor(&tool_name, &arguments);
        let capabilities = descriptor.capabilities;
        let access_profile = descriptor.access_profile;
        let plugin = descriptor.package;
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
        stable_tool_definitions(
            self.tools
                .iter()
                .filter(|t| t.categories().iter().any(|c| active.contains(c)))
                .map(|t| t.definition())
                .collect(),
        )
    }

    /// Select tool definitions using the shared typed visibility policy.
    ///
    /// Core tools are always included. Other categories come from
    /// [`ToolVisibilityDecision`], the same decision object used by routing.
    /// Runtime MCP tools can be discovered lazily through `tool_search`.
    ///
    /// Dynamic visibility is an opt-in prompt compaction mode. The main agent
    /// should normally receive the full registry; this selector exists for
    /// constrained runs and behavioral evaluation.
    pub fn select_tools(&self, user_message: &str, has_sources: bool) -> Vec<ToolDefinition> {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: user_message,
            system_prompt: "",
            has_sources,
        });
        self.select_tools_for_decision(&decision)
    }

    pub fn select_tools_for_decision(
        &self,
        decision: &ToolVisibilityDecision,
    ) -> Vec<ToolDefinition> {
        let categories: HashSet<ToolCategory> =
            decision.active_categories.iter().copied().collect();
        let mut definitions = self.definitions_for_categories(&categories);
        let mut selected_names: HashSet<String> =
            definitions.iter().map(|tool| tool.name.clone()).collect();

        // Discovery tools are the recovery lane for dynamic visibility. Keep
        // them resident even if future policy changes remove Core from a route.
        for tool in &self.tools {
            if RESIDENT_DISCOVERY_TOOL_NAMES.contains(&tool.name())
                && selected_names.insert(tool.name().to_string())
            {
                definitions.push(tool.definition());
            }
        }

        stable_tool_definitions(definitions)
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

fn plan_mode_allows_tool(name: &str, access: &ToolAccessProfile) -> bool {
    if name == "mcp_tool" || name.starts_with("mcp__") {
        return false;
    }

    if matches!(
        name,
        "run_shell"
            | "project_tool"
            | "desktop_automation"
            | "browser_evidence_capture"
            | "download_asset"
            | "generate_image"
            | "synthesize_speech"
            | "prepare_document_tools"
            | "update_plan"
            | "record_verification"
            | "spawn_subagent"
            | "spawn_subagent_batch"
            | "judge_subagent_results"
    ) {
        return false;
    }

    access.can_read && !access.can_write && !access.can_execute && !access.needs_approval
}

/// Generic argument-size guard shared by all execute paths.
///
/// `run_shell` has its own stricter per-arg + total limits, so it's skipped
/// here. Plain-text mutation tools need room for exact old/new snippets, so
/// they get a much larger cap while read/search tools keep the tighter guard.
fn enforce_tool_arg_limit(name: &str, arguments: &str) -> Result<(), CoreError> {
    const MAX_FILE_MUTATION_ARG_BYTES: usize = 8 * 1024 * 1024;

    let Some(max_bytes) = tool_arg_limit_bytes(name) else {
        return Ok(());
    };
    let arg_size = arguments.len();
    if arg_size > max_bytes {
        let guidance = if max_bytes == MAX_FILE_MUTATION_ARG_BYTES {
            "Split very large rewrites into smaller targeted edits, or use run_shell stdin for bulk generated content."
        } else {
            "For document editing with large content, use a file mutation tool for plain text or run_shell with the doc-script-editor skill for rich document formats."
        };
        return Err(CoreError::InvalidInput(format!(
            "Tool arguments exceed {} KB ({} bytes). {}",
            max_bytes / 1024,
            arg_size,
            guidance,
        )));
    }
    Ok(())
}

fn tool_arg_limit_bytes(name: &str) -> Option<usize> {
    const DEFAULT_MAX_TOOL_ARG_BYTES: usize = 256 * 1024;
    const MAX_FILE_MUTATION_ARG_BYTES: usize = 8 * 1024 * 1024;

    let lower = name.to_ascii_lowercase();
    if lower == "run_shell" {
        return None;
    }
    if matches!(
        lower.as_str(),
        "edit_file" | "multi_edit" | "create_file" | "write_note" | "apply_patch"
    ) || lower.contains("edit_file")
        || lower.contains("multi_edit")
        || lower.contains("create_file")
        || lower.contains("write_note")
        || lower.contains("apply_patch")
    {
        return Some(MAX_FILE_MUTATION_ARG_BYTES);
    }
    Some(DEFAULT_MAX_TOOL_ARG_BYTES)
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
    registry.register(Box::new(web_search_tool::WebSearchTool));
    registry.register(Box::new(web_research_context_tool::WebResearchContextTool));
    registry.register(Box::new(browser_evidence_tool::BrowserEvidenceCaptureTool));
    registry.register(Box::new(download_asset_tool::DownloadAssetTool));
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
    registry.register(Box::new(text_to_speech_tool::SynthesizeSpeechTool));
    #[cfg(feature = "ocr")]
    registry.register(Box::new(ocr_tool::ExtractImageTextTool));
    registry.register(Box::new(archive_output_tool::ArchiveOutputTool));
    registry.register(Box::new(related_concepts_tool::RelatedConceptsTool));
    registry.register(Box::new(run_shell_tool::RunShellTool));
    registry.register(Box::new(scratchpad_tool::UpdateScratchpadTool));
    registry.register(Box::new(session_search_tool::SessionSearchTool));
    registry.register(Box::new(persona_tool::PersonaTool));
    registry.register(Box::new(user_memory_tool::UserMemoryTool));
    registry.register(Box::new(project_memory_tool::ProjectMemoryTool));
    registry.register(Box::new(agent_memory_tool::AgentMemoryTool));
    registry.register(Box::new(manage_skill_tool::ManageSkillTool));
    registry.register(Box::new(harness_dry_run_tool::HarnessDryRunTool));
    registry
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::approval::ApprovalRisk;

    struct RuntimeMcpOnlyTool;

    #[async_trait]
    impl Tool for RuntimeMcpOnlyTool {
        fn name(&self) -> &str {
            "mcp__runtime__expensive_tool"
        }

        fn description(&self) -> &str {
            "Runtime MCP tool that should be discovered lazily."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        fn categories(&self) -> &'static [ToolCategory] {
            &[ToolCategory::Mcp]
        }

        async fn execute(
            &self,
            call_id: &str,
            _arguments: &str,
            _db: &Database,
            _source_scope: &[String],
        ) -> Result<ToolResult, CoreError> {
            Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ok".to_string(),
                is_error: false,
                artifacts: None,
            })
        }
    }

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
    fn plan_mode_registry_excludes_mutating_and_execution_tools() {
        let registry = default_tool_registry();
        let plan_registry = registry.plan_mode_filtered();
        let names = plan_registry.tool_names();

        for blocked in [
            "run_shell",
            "edit_file",
            "multi_edit",
            "create_file",
            "write_note",
            "update_plan",
            "record_verification",
            "project_tool",
            "spawn_subagent",
        ] {
            assert!(
                !names.iter().any(|name| name == blocked),
                "{blocked} should be blocked"
            );
        }

        for allowed in [
            "read_file",
            "grep_files",
            "search_files",
            "web_search",
            "tool_search",
        ] {
            assert!(
                names.iter().any(|name| name == allowed),
                "{allowed} should remain available"
            );
        }
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn select_tools_includes_ocr_for_image_text_requests() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("请 OCR 识别这张截图里的文字", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "extract_image_text"));
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
    fn tool_definitions_are_sorted_for_prompt_cache_stability() {
        let registry = default_tool_registry();
        let names: Vec<String> = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        let mut sorted = names.clone();
        sorted.sort();

        assert_eq!(names, sorted);
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
    fn select_tools_keeps_manage_skill_available_for_direct_turns() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Say hello briefly.", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "manage_skill"));
    }

    #[test]
    fn select_tools_keeps_manage_persona_available_for_direct_turns() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Say hello briefly.", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "manage_persona"));
    }

    #[test]
    fn select_tools_keeps_speech_synthesis_available_for_direct_requests() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Read this paragraph aloud.", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "synthesize_speech"));
    }

    #[test]
    fn select_tools_defers_mcp_tools_for_direct_turns() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(tool_search_tool::ToolSearchTool));
        registry.register(Box::new(RuntimeMcpOnlyTool));
        let defs = registry.select_tools("Say hello briefly.", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "tool_search"));
        assert!(!names
            .iter()
            .any(|name| name == "mcp__runtime__expensive_tool"));
    }

    #[test]
    fn select_tools_keeps_tool_search_resident_even_without_core_category() {
        let registry = default_tool_registry();
        let decision = ToolVisibilityDecision {
            route: crate::tool_visibility_policy::ToolVisibilityRouteKind::DirectResponse,
            active_categories: Vec::new(),
            route_categories: Vec::new(),
            signals: Vec::new(),
            log: Vec::new(),
        };
        let defs = registry.select_tools_for_decision(&decision);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "tool_search"));
        assert!(!names.iter().any(|name| name == "read_file"));
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
        assert!(names.iter().any(|name| name == "grep_files"));
        assert!(names.iter().any(|name| name == "run_shell"));
    }

    #[test]
    fn select_tools_consumes_typed_visibility_decision() {
        let registry = default_tool_registry();
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "debug the agent routing bug and run the repository diagnostics",
            system_prompt: "",
            has_sources: false,
        });
        let via_decision: Vec<String> = registry
            .select_tools_for_decision(&decision)
            .into_iter()
            .map(|def| def.name)
            .collect();
        let via_selector: Vec<String> = registry
            .select_tools(
                "debug the agent routing bug and run the repository diagnostics",
                false,
            )
            .into_iter()
            .map(|def| def.name)
            .collect();

        assert_eq!(via_decision, via_selector);
        assert!(decision.log.iter().any(|entry| matches!(
            entry.effect,
            crate::tool_visibility_policy::ToolVisibilityEffect::SelectedRoute { .. }
        )));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
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
    fn capability_descriptor_is_invocation_source_of_truth() {
        let registry = default_tool_registry();
        let args = serde_json::json!({
            "path": "notes/example.md",
            "content": "hello",
            "overwrite": true
        });

        let descriptor = registry.capability_descriptor("create_file", &args);

        assert_eq!(descriptor.name, "create_file");
        assert_eq!(descriptor.package.id, "file-workspace");
        assert_eq!(descriptor.categories, vec!["filesystem".to_string()]);
        assert_eq!(descriptor.ui.render_kind, ToolRenderKind::FileChange);
        assert_eq!(descriptor.ui.display_category, "filesystem");
        assert_eq!(descriptor.resources.keys, vec!["file:notes/example.md"]);
        assert_eq!(
            descriptor.capabilities,
            registry.run_capabilities("create_file", &args)
        );
        assert_eq!(
            descriptor.access_profile,
            registry.access_profile("create_file", &args)
        );
        assert_eq!(descriptor.access_profile.risk_level, ApprovalRisk::High);
    }

    #[test]
    fn read_and_retrieval_tools_stream_ui_previews() {
        let registry = default_tool_registry();
        let preview_tools = [
            "fetch_url",
            "web_search",
            "web_research_context",
            "read_file",
            "read_files",
            #[cfg(feature = "ocr")]
            "extract_image_text",
            "list_dir",
            "glob_files",
            "grep_files",
            "search_files",
            "get_document_info",
            "compare_documents",
            "summarize_document",
            "compile_document",
            "search_knowledge_base",
            "retrieve_evidence",
            "search_playbooks",
            "search_sessions",
            "search_by_date",
            "get_chunk_context",
            "query_knowledge_graph",
            "get_related_concepts",
            "list_documents",
            "list_sources",
            "tool_search",
            "code_intelligence",
        ];

        for name in preview_tools {
            let capabilities = registry.run_capabilities(name, &serde_json::Value::Null);
            assert_eq!(
                capabilities.input_streaming,
                ToolInputStreamingMode::UiPreview,
                "{name} should expose partial arguments for live UI previews"
            );
            assert_eq!(
                capabilities.render_kind,
                ToolRenderKind::Search,
                "{name} should use the lightweight retrieval renderer"
            );
        }
    }

    #[test]
    fn file_mutation_tools_stream_ui_previews() {
        let registry = default_tool_registry();
        let preview_tools = ["edit_file", "multi_edit", "create_file", "write_note"];

        for name in preview_tools {
            let capabilities = registry.run_capabilities(name, &serde_json::Value::Null);
            assert_eq!(
                capabilities.input_streaming,
                ToolInputStreamingMode::UiPreview,
                "{name} should expose partial arguments for live diff previews"
            );
            assert_eq!(
                capabilities.render_kind,
                ToolRenderKind::FileChange,
                "{name} should use the file change renderer"
            );
        }
    }

    #[test]
    fn file_mutation_tools_allow_large_argument_payloads() {
        let large_replacement = "x".repeat(64 * 1024);
        let args = serde_json::json!({
            "path": "notes.md",
            "old_str": "before",
            "new_str": large_replacement,
        })
        .to_string();

        assert!(enforce_tool_arg_limit("edit_file", &args).is_ok());
        assert!(enforce_tool_arg_limit("mcp__repo__apply_patch", &args).is_ok());
    }

    #[test]
    fn generic_tools_keep_argument_guard_at_transport_scale() {
        let large_query = "x".repeat(257 * 1024);
        let args = serde_json::json!({ "query": large_query }).to_string();
        let err = enforce_tool_arg_limit("search_knowledge_base", &args)
            .expect_err("generic oversized tool arguments should be rejected");

        assert!(err.to_string().contains("256 KB"));
    }

    #[test]
    fn default_registry_tools_expose_capability_descriptors() {
        let registry = default_tool_registry();

        for name in registry.tool_names() {
            let descriptor = registry.capability_descriptor(&name, &serde_json::Value::Null);

            assert_eq!(descriptor.name, name);
            assert!(
                !descriptor.package.id.is_empty(),
                "{} should belong to a capability package",
                descriptor.name
            );
            assert_eq!(
                descriptor.ui.render_kind, descriptor.capabilities.render_kind,
                "{} UI descriptor should mirror runtime renderer",
                descriptor.name
            );
            assert_eq!(
                descriptor.resources.keys, descriptor.capabilities.resource_keys,
                "{} resource descriptor should mirror scheduler keys",
                descriptor.name
            );
            assert_eq!(
                descriptor.access_profile,
                registry.access_profile(&descriptor.name, &serde_json::Value::Null),
                "{} policy should come from the capability descriptor",
                descriptor.name
            );
        }
    }

    #[test]
    fn descriptor_classifies_native_web_search() {
        let registry = default_tool_registry();
        let descriptor =
            registry.capability_descriptor("web_search", &serde_json::json!({ "query": "nexa" }));

        assert_eq!(descriptor.package.id, "web-research");
        assert_eq!(descriptor.ui.render_kind, ToolRenderKind::Search);
        assert!(descriptor.capabilities.read_only);
        assert_eq!(
            descriptor.capabilities.input_streaming,
            ToolInputStreamingMode::UiPreview
        );
        assert_eq!(descriptor.access_profile.category, "web");
        assert!(descriptor.access_profile.can_access_network);
        assert!(!descriptor.access_profile.can_write);
        assert_eq!(descriptor.access_profile.risk_level, ApprovalRisk::Low);
        assert!(descriptor.resources.keys.is_empty());
    }

    #[test]
    fn descriptor_classifies_download_asset_as_web_write() {
        let registry = default_tool_registry();
        let descriptor = registry.capability_descriptor(
            "download_asset",
            &serde_json::json!({ "url": "https://example.com/image.png", "filename": "image.png" }),
        );

        assert_eq!(descriptor.package.id, "web-research");
        assert_eq!(descriptor.ui.render_kind, ToolRenderKind::FileChange);
        assert!(!descriptor.capabilities.read_only);
        assert!(descriptor.access_profile.can_access_network);
        assert!(descriptor.access_profile.can_write);
        assert!(descriptor.access_profile.needs_approval);
        assert_eq!(descriptor.access_profile.risk_level, ApprovalRisk::Medium);
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
