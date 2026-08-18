//! Tool system — trait, registry, and built-in tools for the agent framework.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::activity::ActivityRuntime;
use crate::app_settings::ShellAccessMode;
use crate::approval::{ApprovalRisk, ToolApprovalMode};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolDefinition;
use crate::models::Source;
use crate::plugins::CapabilityOwner;
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
    // Closed top-level schemas give providers a much stronger contract and
    // prevent invented bookkeeping fields from reaching individual tools.
    // Preserve an explicit setting for runtime/MCP definitions that truly
    // accept arbitrary properties.
    schema
        .entry("additionalProperties")
        .or_insert_with(|| serde_json::Value::Bool(false));
    parameters
}

pub mod activity_tool;
pub mod agent_memory_tool;
pub mod appearance_tool;
pub mod archive_output_tool;
pub mod browser_evidence_tool;
pub mod browser_session_tool;
pub mod chunk_context_tool;
pub mod code_intelligence_tool;
pub mod compare_tool;
pub mod compile_tool;
pub mod computer_use_tool;
pub mod conversation_goal_tool;
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
pub mod request_user_input_tool;
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
    pub owner: CapabilityOwner,
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
    pub turn_id: Option<&'a str>,
    pub tool_registry: Option<&'a ToolRegistry>,
    pub cancel_token: Option<&'a tokio_util::sync::CancellationToken>,
    pub activity_runtime: Option<&'a ActivityRuntime>,
    /// Parent-turn event sink for tools that own work beyond their initial
    /// invocation. Detached runtimes must clone the sender before returning.
    pub event_tx: Option<&'a tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
}

impl<'a> ToolExecutionContext<'a> {
    /// Build the minimal context used by isolated tools and focused tests.
    pub fn new(
        call_id: &'a str,
        arguments: &'a str,
        db: &'a Database,
        source_scope: &'a [String],
    ) -> Self {
        Self {
            call_id,
            arguments,
            db,
            source_scope,
            conversation_id: None,
            turn_id: None,
            tool_registry: None,
            cancel_token: None,
            activity_runtime: None,
            event_tx: None,
        }
    }

    pub fn with_conversation_id(mut self, conversation_id: Option<&'a str>) -> Self {
        self.conversation_id = conversation_id;
        self
    }

    pub fn with_turn_id(mut self, turn_id: Option<&'a str>) -> Self {
        self.turn_id = turn_id;
        self
    }

    pub fn with_activity_runtime(mut self, activity_runtime: &'a ActivityRuntime) -> Self {
        self.activity_runtime = Some(activity_runtime);
        self
    }

    pub fn with_event_tx(
        mut self,
        event_tx: &'a tokio::sync::mpsc::Sender<crate::agent::AgentEvent>,
    ) -> Self {
        self.event_tx = Some(event_tx);
        self
    }
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

    /// Execute this tool with the complete runtime identity, scope, and services.
    async fn execute(&self, context: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError>;
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

    pub fn plugin_info(&self, name: &str) -> CapabilityOwner {
        crate::plugins::capability_owner_for_tool(name)
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
        let owner = descriptor.owner;
        ToolInvocation {
            call_id: call_id.into(),
            tool_name,
            owner,
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

        let definitions = stable_tool_definitions(definitions);
        if tracing::enabled!(target: "nexa::tool_visibility", tracing::Level::DEBUG) {
            let offered_tools: Vec<&str> = definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect();
            let hidden_tools: Vec<serde_json::Value> = self
                .tools
                .iter()
                .filter(|tool| !selected_names.contains(tool.name()))
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name(),
                        "categories": tool.categories(),
                        "reason": "none of the tool categories are active for this visibility decision",
                    })
                })
                .collect();
            tracing::debug!(
                target: "nexa::tool_visibility",
                route = decision.route.as_str(),
                active_categories = ?decision.active_categories,
                offered_tools = ?offered_tools,
                hidden_tools = ?hidden_tools,
                decision_log = ?decision.log,
                "resolved dynamic tool visibility"
            );
        }
        definitions
    }

    /// Execute a tool by name through the single context-based runtime entry point.
    pub async fn execute(
        &self,
        name: &str,
        ctx: ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let (tool, arguments) = match self.prepare_execution(name, ctx.call_id, ctx.arguments) {
            Ok(prepared) => prepared,
            Err(result) => return Ok(result),
        };
        let call_id = ctx.call_id;
        let result = tool
            .execute(ToolExecutionContext {
                call_id: ctx.call_id,
                arguments: &arguments,
                db: ctx.db,
                source_scope: ctx.source_scope,
                conversation_id: ctx.conversation_id,
                turn_id: ctx.turn_id,
                tool_registry: ctx.tool_registry,
                cancel_token: ctx.cancel_token,
                activity_runtime: ctx.activity_runtime,
                event_tx: ctx.event_tx,
            })
            .await;
        Ok(normalize_tool_execution_result(
            call_id,
            name,
            tool.parameters_schema(),
            result,
        ))
    }

    pub(crate) fn normalized_arguments_for_scheduling(
        &self,
        name: &str,
        arguments: &str,
    ) -> serde_json::Value {
        let mut value = parse_tool_arguments_value(arguments).unwrap_or_default();
        if let Some(tool) = self.get(name) {
            normalize_property_aliases(&mut value, &tool.definition().parameters);
        }
        value
    }

    fn prepare_execution<'a>(
        &'a self,
        name: &str,
        call_id: &str,
        arguments: &str,
    ) -> Result<(&'a dyn Tool, String), ToolResult> {
        let Some(tool) = self.get(name) else {
            let suggestions = nearest_tool_names(name, &self.tool_names());
            let suffix = if suggestions.is_empty() {
                String::new()
            } else {
                format!(" Did you mean: {}?", suggestions.join(", "))
            };
            return Err(structured_tool_error_result(
                call_id,
                "unknown_tool",
                format!("Unknown tool '{name}'.{suffix}"),
                serde_json::json!({
                    "tool": suggestions.first(),
                    "availableSuggestions": suggestions,
                    "recovery": "use an exact registered tool name; use tool_search when the needed capability is not currently visible"
                }),
                true,
            ));
        };

        if let Err(error) = enforce_tool_arg_limit(name, arguments) {
            return Err(structured_tool_error_result(
                call_id,
                "tool_arguments_too_large",
                error.to_string(),
                serde_json::json!({
                    "tool": name,
                    "arguments": tool.definition().parameters,
                    "recovery": "split the request into smaller resumable calls or move large generated input to a declared stdin/file resource channel"
                }),
                true,
            ));
        }

        let schema = tool.definition().parameters;
        match normalize_tool_arguments(name, arguments, &schema) {
            Ok(arguments) => Ok((tool, arguments)),
            Err((code, message)) => Err(structured_tool_error_result(
                call_id, code, message, schema, true,
            )),
        }
    }
}

fn strip_json_code_fence(arguments: &str) -> Cow<'_, str> {
    let trimmed = arguments.trim();
    let Some(header_end) = trimmed.find('\n') else {
        return Cow::Borrowed(trimmed);
    };
    let header = trimmed[..header_end].trim_end_matches('\r').trim();
    if !matches!(header, "```" | "```json" | "```JSON") || !trimmed.ends_with("```") {
        return Cow::Borrowed(trimmed);
    }
    Cow::Borrowed(trimmed[header_end + 1..trimmed.len() - 3].trim())
}

pub(crate) fn parse_tool_arguments_value(
    arguments: &str,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(strip_json_code_fence(arguments).as_ref())
}

fn camel_to_snake_case(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len() + 4);
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                normalized.push('_');
            }
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(ch);
        }
    }
    normalized
}

fn snake_to_camel_case(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut uppercase_next = false;
    for ch in value.chars() {
        if ch == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            normalized.push(ch.to_ascii_uppercase());
            uppercase_next = false;
        } else {
            normalized.push(ch);
        }
    }
    normalized
}

fn normalize_property_aliases(value: &mut serde_json::Value, schema: &serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return;
    };
    let keys = object.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if properties.contains_key(&key) {
            continue;
        }
        let snake = camel_to_snake_case(&key);
        let camel = snake_to_camel_case(&key);
        let canonical = if properties.contains_key(&snake) {
            Some(snake)
        } else if properties.contains_key(&camel) {
            Some(camel)
        } else {
            None
        };
        let Some(canonical) = canonical else {
            continue;
        };
        if object.contains_key(&canonical) {
            continue;
        }
        if let Some(value) = object.remove(&key) {
            object.insert(canonical, value);
        }
    }
}

fn json_value_matches_type(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "string" => value.is_string(),
        _ => true,
    }
}

fn top_level_argument_issue(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Option<(&'static str, String)> {
    let object = value.as_object()?;
    if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
        let missing = required
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter(|field| !object.contains_key(*field))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Some((
                "missing_required_arguments",
                format!("Missing required tool argument(s): {}", missing.join(", ")),
            ));
        }
    }

    let properties = schema.get("properties")?.as_object()?;
    for (field, field_value) in object {
        let Some(field_schema) = properties.get(field) else {
            continue;
        };
        if let Some(expected) = field_schema.get("type").and_then(serde_json::Value::as_str) {
            if !json_value_matches_type(field_value, expected) {
                return Some((
                    "invalid_argument_type",
                    format!("Tool argument '{field}' must be {expected}"),
                ));
            }
        }
        if let Some(allowed) = field_schema
            .get("enum")
            .and_then(serde_json::Value::as_array)
        {
            if !allowed.contains(field_value) {
                return Some((
                    "invalid_argument_value",
                    format!(
                        "Tool argument '{field}' must be one of: {}",
                        allowed
                            .iter()
                            .map(serde_json::Value::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
    }
    None
}

fn normalize_tool_arguments(
    tool_name: &str,
    arguments: &str,
    schema: &serde_json::Value,
) -> Result<String, (&'static str, String)> {
    let payload = strip_json_code_fence(arguments);
    let mut value = match serde_json::from_str::<serde_json::Value>(payload.as_ref()) {
        Ok(value) => value,
        // run_shell has an additional Windows-path escape repair lane.
        Err(_) if tool_name == "run_shell" => return Ok(payload.into_owned()),
        Err(error) => {
            return Err((
                "invalid_arguments_json",
                format!(
                    "Tool arguments must be one JSON object: {error}. Remove prose/code fences and retry with only schema fields."
                ),
            ));
        }
    };
    if !value.is_object() {
        return Err((
            "invalid_arguments_shape",
            "Tool arguments must be a JSON object.".to_string(),
        ));
    }
    normalize_property_aliases(&mut value, schema);
    if let Some(issue) = top_level_argument_issue(&value, schema) {
        return Err(issue);
    }
    serde_json::to_string(&value).map_err(|error| {
        (
            "invalid_arguments_json",
            format!("Failed to normalize tool arguments: {error}"),
        )
    })
}

fn nearest_tool_names(requested: &str, available: &[String]) -> Vec<String> {
    let mut scored = available
        .iter()
        .map(|name| (levenshtein(requested, name), name.clone()))
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_name), (right_score, right_name)| {
        left_score.cmp(right_score).then(left_name.cmp(right_name))
    });
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right_chars.iter().enumerate() {
            current.push(
                (current[right_index] + 1)
                    .min(previous[right_index + 1] + 1)
                    .min(previous[right_index] + usize::from(left_char != *right_char)),
            );
        }
        previous = current;
    }
    previous[right_chars.len()]
}

fn classify_tool_result_error(message: &str) -> (&'static str, bool) {
    let lower = message.to_ascii_lowercase();
    if lower.contains("cancel") || lower.contains("denied") || lower.contains("permission") {
        ("tool_permission_or_cancellation", false)
    } else if lower.contains("timed out") || lower.contains("timeout") {
        ("tool_timeout", true)
    } else if lower.contains("not found") || lower.contains("cannot find") {
        ("resource_not_found", true)
    } else if lower.contains("found ") && lower.contains(" times")
        || lower.contains("multiple occurrences")
        || lower.contains("ambiguous")
    {
        ("ambiguous_match", true)
    } else if lower.contains("invalid")
        || lower.contains("requires")
        || lower.contains("must ")
        || lower.contains("missing")
    {
        ("invalid_tool_request", true)
    } else {
        ("tool_execution_failed", true)
    }
}

fn core_error_contract(error: &CoreError) -> (&'static str, bool) {
    match error {
        CoreError::InvalidInput(_) | CoreError::Serialization(_) | CoreError::Parse(_) => {
            ("invalid_tool_request", true)
        }
        CoreError::NotFound(_) => ("resource_not_found", true),
        CoreError::RateLimited { .. }
        | CoreError::TransientLlm(_)
        | CoreError::StreamIncomplete(_) => ("transient_tool_failure", true),
        CoreError::Cancelled(_) => ("tool_cancelled", false),
        CoreError::Io(_) | CoreError::Database(_) | CoreError::Internal(_) => {
            ("tool_runtime_failure", true)
        }
        _ => ("tool_execution_failed", true),
    }
}

fn normalize_tool_execution_result(
    call_id: &str,
    tool_name: &str,
    schema: serde_json::Value,
    result: Result<ToolResult, CoreError>,
) -> ToolResult {
    match result {
        Ok(result) if result.is_error && result.artifacts.is_none() => {
            let (code, retryable) = classify_tool_result_error(&result.content);
            structured_tool_error_result(
                call_id,
                code,
                result.content,
                serde_json::json!({
                    "tool": tool_name,
                    "arguments": schema,
                    "recovery": if retryable {
                        "correct the smallest failing field or precondition, then retry once if the operation is still needed"
                    } else {
                        "do not retry until permission, cancellation, or user intent changes"
                    }
                }),
                retryable,
            )
        }
        Ok(result) => result,
        Err(error) => {
            let (code, retryable) = core_error_contract(&error);
            structured_tool_error_result(
                call_id,
                code,
                format!("{tool_name} failed: {error}"),
                serde_json::json!({
                    "tool": tool_name,
                    "arguments": schema,
                    "recovery": if retryable {
                        "inspect the structured code, repair arguments or runtime preconditions, and retry once with a materially corrected call"
                    } else {
                        "stop and wait for user intent or permissions to change"
                    }
                }),
                retryable,
            )
        }
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
            | "computer_control"
            | "desktop_automation"
            | "browser_evidence_capture"
            | "download_asset"
            | "generate_image"
            | "synthesize_speech"
            | "prepare_document_tools"
            | "update_plan"
            | "update_goal"
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
        let lower = name.to_ascii_lowercase();
        let guidance = if lower == "manage_skill" || lower.contains("manage_skill") {
            "Keep SKILL.md and its declared resource bundle within the owner safety envelope; use the filesystem importer for exceptionally large binary assets."
        } else if max_bytes == MAX_FILE_MUTATION_ARG_BYTES {
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
    const MAX_SKILL_PROPOSAL_ARG_BYTES: usize = 16 * 1024 * 1024;

    let lower = name.to_ascii_lowercase();
    if lower == "run_shell" {
        return None;
    }
    if lower == "manage_skill" || lower.contains("manage_skill") {
        // A reviewed proposal may contain a large SKILL.md plus JSON escaping
        // overhead and bundled resources. Retain a transport guard, but do not
        // let the generic search-tool cap block valid skill packages.
        return Some(MAX_SKILL_PROPOSAL_ARG_BYTES);
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
    registry.register(Box::new(activity_tool::ActivityObserveTool));
    registry.register(Box::new(appearance_tool::AppearanceTool));
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
    registry.register(Box::new(browser_session_tool::BrowserSessionTool::default()));
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
    #[cfg(target_os = "windows")]
    {
        registry.register(Box::new(computer_use_tool::ComputerObserveTool));
        registry.register(Box::new(computer_use_tool::ComputerControlTool));
    }
    registry.register(Box::new(desktop_automation_tool::DesktopAutomationTool));
    registry.register(Box::new(summarize_tool::SummarizeDocumentTool));
    registry.register(Box::new(update_plan_tool::UpdatePlanTool));
    registry.register(Box::new(conversation_goal_tool::GetGoalTool));
    registry.register(Box::new(conversation_goal_tool::UpdateGoalTool));
    registry.register(Box::new(record_verification_tool::RecordVerificationTool));
    registry.register(Box::new(request_user_input_tool::RequestUserInputTool));
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

    struct EchoArgumentsTool;

    #[async_trait]
    impl Tool for EchoArgumentsTool {
        fn name(&self) -> &str {
            "echo_arguments"
        }

        fn description(&self) -> &str {
            "Echo normalized arguments for contract tests."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "start_line": { "type": "integer" }
                },
                "required": ["start_line"]
            })
        }

        async fn execute(
            &self,
            context: crate::tools::ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            let crate::tools::ToolExecutionContext {
                call_id,
                arguments,
                db: _db,
                source_scope: _source_scope,
                ..
            } = context;
            Ok(ToolResult {
                call_id: call_id.to_string(),
                content: arguments.to_string(),
                is_error: false,
                artifacts: None,
            })
        }
    }

    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "failing_tool"
        }

        fn description(&self) -> &str {
            "Always fails for contract tests."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }

        async fn execute(
            &self,
            context: crate::tools::ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            let crate::tools::ToolExecutionContext {
                call_id: _call_id,
                arguments: _arguments,
                db: _db,
                source_scope: _source_scope,
                ..
            } = context;
            Err(CoreError::InvalidInput(
                "the requested field is stale".to_string(),
            ))
        }
    }

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
            context: crate::tools::ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            let crate::tools::ToolExecutionContext {
                call_id,
                arguments: _arguments,
                db: _db,
                source_scope: _source_scope,
                ..
            } = context;
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
            "update_goal",
            "record_verification",
            "project_tool",
            "computer_control",
            "spawn_subagent",
        ] {
            assert!(
                !names.iter().any(|name| name == blocked),
                "{blocked} should be blocked"
            );
        }

        let allowed = [
            "read_file",
            "grep_files",
            "search_files",
            "web_search",
            "tool_search",
        ];
        for allowed in allowed {
            assert!(
                names.iter().any(|name| name == allowed),
                "{allowed} should remain available"
            );
        }
        #[cfg(target_os = "windows")]
        assert!(names.iter().any(|name| name == "computer_observe"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_computer_tools_are_hidden_on_unsupported_platforms() {
        let names = default_tool_registry().tool_names();

        assert!(!names.iter().any(|name| name == "computer_observe"));
        assert!(!names.iter().any(|name| name == "computer_control"));
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
    fn select_tools_uses_browser_session_exclusively_for_browser_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Open this website in my browser", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "browser_session"));
        assert!(!names.iter().any(|name| name == "desktop_automation"));
    }

    #[test]
    fn select_tools_keeps_desktop_automation_for_local_path_handoffs() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Reveal this file in Explorer", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "desktop_automation"));
        assert!(!names.iter().any(|name| name == "browser_session"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn select_tools_includes_native_computer_use_for_desktop_tasks() {
        let registry = default_tool_registry();
        let defs = registry.select_tools(
            "Capture this app window, then click the Save button with the mouse.",
            false,
        );
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "computer_observe"));
        assert!(names.iter().any(|name| name == "computer_control"));
    }

    #[test]
    fn select_tools_keeps_manage_skill_available_for_direct_turns() {
        let registry = default_tool_registry();
        let defs = registry.select_tools("Say hello briefly.", false);
        let names: Vec<String> = defs.into_iter().map(|def| def.name).collect();

        assert!(names.iter().any(|name| name == "manage_skill"));
    }

    #[test]
    fn default_registry_exposes_agent_only_user_question_tool() {
        let registry = default_tool_registry();
        let tool = registry
            .get("request_user_input")
            .expect("request_user_input should be registered");
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["questions"]["minItems"], 1);
        assert_eq!(schema["properties"]["questions"]["maxItems"], 6);
        assert_eq!(
            schema["properties"]["kind"]["enum"],
            serde_json::json!(["user_input", "high_risk_confirmation"])
        );
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
        assert_eq!(descriptor.owner.id, "file-workspace");
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
    fn skill_proposals_are_not_bound_by_generic_search_payload_limit() {
        let content = "x".repeat(512 * 1024);
        let args = serde_json::json!({
            "action": "propose_create",
            "name": "Large skill",
            "content": content
        })
        .to_string();

        assert!(enforce_tool_arg_limit("manage_skill", &args).is_ok());
        assert!(enforce_tool_arg_limit("search_knowledge_base", &args).is_err());
    }

    #[tokio::test]
    async fn registry_executes_large_skill_proposals_end_to_end() {
        let db = Database::open_memory().unwrap();
        let registry = default_tool_registry();
        let args = serde_json::json!({
            "action": "propose_create",
            "name": "Large registry skill",
            "description": "Verifies the reviewed proposal transport path.",
            "content": format!("# Workflow\n\n{}", "Run the verified step.\n".repeat(16_000))
        })
        .to_string();

        let result = registry
            .execute(
                "manage_skill",
                crate::tools::ToolExecutionContext::new("call-large-skill", &args, &db, &[]),
            )
            .await
            .unwrap();

        assert!(!result.is_error, "proposal failed: {}", result.content);
        assert_eq!(db.list_skill_change_proposals(None, 10).unwrap().len(), 1);
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
                !descriptor.owner.id.is_empty(),
                "{} should belong to a capability owner",
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

        assert_eq!(descriptor.owner.id, "web-research");
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

        assert_eq!(descriptor.owner.id, "web-research");
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
    fn default_tool_definitions_close_top_level_object_schemas() {
        let registry = default_tool_registry();

        for definition in registry.definitions() {
            if definition.parameters["type"].as_str() == Some("object") {
                assert_eq!(
                    definition.parameters["additionalProperties"],
                    serde_json::Value::Bool(false),
                    "{} should reject invented top-level fields at the provider boundary",
                    definition.name
                );
            }
        }
    }

    #[tokio::test]
    async fn registry_normalizes_fenced_json_and_case_aliases() {
        let db = Database::open_memory().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoArgumentsTool));

        let result = registry
            .execute(
                "echo_arguments",
                crate::tools::ToolExecutionContext::new(
                    "call-normalize",
                    "```json\n{\"startLine\": 7}\n```",
                    &db,
                    &[],
                ),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.call_id, "call-normalize");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.content).unwrap()["start_line"],
            7
        );
    }

    #[tokio::test]
    async fn registry_returns_structured_contract_errors() {
        let db = Database::open_memory().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoArgumentsTool));
        registry.register(Box::new(FailingTool));

        let invalid = registry
            .execute(
                "echo_arguments",
                crate::tools::ToolExecutionContext::new(
                    "call-invalid",
                    r#"{"startLine":"seven"}"#,
                    &db,
                    &[],
                ),
            )
            .await
            .unwrap();
        assert!(invalid.is_error);
        assert_eq!(invalid.call_id, "call-invalid");
        assert_eq!(
            invalid.artifacts.as_ref().unwrap()["code"],
            "invalid_argument_type"
        );

        let failed = registry
            .execute(
                "failing_tool",
                crate::tools::ToolExecutionContext::new("call-failed", "{}", &db, &[]),
            )
            .await
            .unwrap();
        assert!(failed.is_error);
        assert_eq!(failed.call_id, "call-failed");
        assert_eq!(
            failed.artifacts.as_ref().unwrap()["code"],
            "invalid_tool_request"
        );

        let unknown = registry
            .execute(
                "echo_argument",
                crate::tools::ToolExecutionContext::new("call-unknown", "{}", &db, &[]),
            )
            .await
            .unwrap();
        assert!(unknown.is_error);
        assert_eq!(unknown.call_id, "call-unknown");
        assert_eq!(unknown.artifacts.as_ref().unwrap()["code"], "unknown_tool");
        assert!(unknown.content.contains("echo_arguments"));
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
        assert_eq!(invocation.owner.id, "file-workspace");
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
