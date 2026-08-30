//! Typed routing and tool-visibility policy.
//!
//! Route selection and dynamic tool visibility both consume the same decision
//! so prompt routing and offered tools cannot drift independently.

use serde::{Deserialize, Serialize};

use crate::tools::ToolCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolVisibilityRouteKind {
    DirectResponse,
    KnowledgeRetrieval,
    CollectionFocused,
    ConversationRecall,
    CodebaseOperation,
    FileOperation,
    WebLookup,
    InteractionOperation,
    SourceManagement,
}

impl ToolVisibilityRouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectResponse => "DirectResponse",
            Self::KnowledgeRetrieval => "KnowledgeRetrieval",
            Self::CollectionFocused => "CollectionFocused",
            Self::ConversationRecall => "ConversationRecall",
            Self::CodebaseOperation => "CodebaseOperation",
            Self::FileOperation => "FileOperation",
            Self::WebLookup => "WebLookup",
            Self::InteractionOperation => "InteractionOperation",
            Self::SourceManagement => "SourceManagement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolVisibilitySignalKind {
    CollectionContext,
    Question,
    CodeOrToolOperation,
    FileOperation,
    FileWorkspace,
    SourceManagement,
    WebLookup,
    ConversationRecall,
    KnowledgeWork,
    Automation,
    Process,
    Terminal,
    Browser,
    WebArtifactAuthoring,
    BrowserInteraction,
    Desktop,
    DesktopInteraction,
    DocumentAnalysis,
    LinkedSources,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilitySignal {
    pub kind: ToolVisibilitySignalKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolVisibilityEffect {
    MatchedSignal { signal: ToolVisibilitySignalKind },
    ActivatedCategory { category: ToolCategory },
    SelectedRoute { route: ToolVisibilityRouteKind },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilityDecisionLogEntry {
    pub rule_id: String,
    pub effect: ToolVisibilityEffect,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCapabilityRequirements {
    pub route: ToolVisibilityRouteKind,
    pub active_categories: Vec<ToolCategory>,
    pub route_categories: Vec<ToolCategory>,
    pub signals: Vec<ToolVisibilitySignal>,
    pub log: Vec<ToolVisibilityDecisionLogEntry>,
    #[serde(default)]
    pub interaction: TurnInteractionRequirements,
}

/// Backwards-compatible trace vocabulary. Tool visibility is now one
/// projection of the authoritative per-turn capability requirements.
pub type ToolVisibilityDecision = TurnCapabilityRequirements;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VisualObservationRequirement {
    #[default]
    NotRequired,
    AfterLastMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DesktopObservationRequirement {
    #[default]
    NotRequired,
    BeforeControlAndAfterLastControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BrowserTerminalClosureRequirement {
    #[default]
    NotRequired,
    Tab,
    AllTabs,
    Session,
}

impl BrowserTerminalClosureRequirement {
    pub fn is_required(self) -> bool {
        self != Self::NotRequired
    }

    pub(crate) fn accepts_tab_receipt(self, remaining_tab_count: u64) -> bool {
        match self {
            Self::Tab => true,
            Self::AllTabs => remaining_tab_count == 0,
            Self::NotRequired | Self::Session => false,
        }
    }

    pub(crate) fn allows_session(self) -> bool {
        self == Self::Session
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnInteractionRequirements {
    pub visual_observation: VisualObservationRequirement,
    pub browser_observation: bool,
    pub browser_interaction: bool,
    /// The user's requested browser end state may legitimately remove the
    /// renderable session/tab, so a bound terminal closure receipt can replace
    /// an otherwise impossible post-action screenshot.
    #[serde(default)]
    pub browser_terminal_closure: BrowserTerminalClosureRequirement,
    pub desktop_observation: DesktopObservationRequirement,
    pub desktop_interaction: bool,
}

impl TurnCapabilityRequirements {
    pub fn requires_visual_observation_after_mutation(&self) -> bool {
        self.interaction.visual_observation == VisualObservationRequirement::AfterLastMutation
    }

    pub fn requires_completion_gate(&self) -> bool {
        self.interaction.requires_completion_gate()
    }

    pub fn for_route(route: ToolVisibilityRouteKind) -> Self {
        let route_categories = route_categories(route);
        let mut active_categories = vec![ToolCategory::Core];
        for category in &route_categories {
            if !active_categories.contains(category) {
                active_categories.push(*category);
            }
        }
        Self {
            route,
            active_categories,
            route_categories,
            signals: Vec::new(),
            log: Vec::new(),
            interaction: TurnInteractionRequirements::default(),
        }
    }

    pub fn for_route_name(route: &str) -> Self {
        let route = match route {
            "CollectionFocused" => ToolVisibilityRouteKind::CollectionFocused,
            "KnowledgeRetrieval" => ToolVisibilityRouteKind::KnowledgeRetrieval,
            "ConversationRecall" => ToolVisibilityRouteKind::ConversationRecall,
            "CodebaseOperation" => ToolVisibilityRouteKind::CodebaseOperation,
            "FileOperation" => ToolVisibilityRouteKind::FileOperation,
            "WebLookup" => ToolVisibilityRouteKind::WebLookup,
            "InteractionOperation" => ToolVisibilityRouteKind::InteractionOperation,
            "SourceManagement" => ToolVisibilityRouteKind::SourceManagement,
            _ => ToolVisibilityRouteKind::DirectResponse,
        };
        Self::for_route(route)
    }
}

impl TurnInteractionRequirements {
    pub fn requires_visual_observation_after_mutation(self) -> bool {
        self.visual_observation == VisualObservationRequirement::AfterLastMutation
    }

    pub fn requires_desktop_observation(self) -> bool {
        self.desktop_observation == DesktopObservationRequirement::BeforeControlAndAfterLastControl
    }

    pub fn requires_completion_gate(self) -> bool {
        self.requires_visual_observation_after_mutation() || self.requires_desktop_observation()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolVisibilityInput<'a> {
    pub query: &'a str,
    pub system_prompt: &'a str,
    pub has_sources: bool,
}

pub fn decide_tool_visibility(input: ToolVisibilityInput<'_>) -> ToolVisibilityDecision {
    resolve_turn_capability_requirements(input)
}

pub fn resolve_turn_capability_requirements(
    input: ToolVisibilityInput<'_>,
) -> TurnCapabilityRequirements {
    let query = input.query.to_lowercase();
    let collection_context = system_prompt_has_collection_context(input.system_prompt);
    let mut signals = Vec::new();
    let mut log = Vec::new();

    if collection_context {
        push_signal(
            &mut signals,
            &mut log,
            "signal.collection_context",
            ToolVisibilitySignalKind::CollectionContext,
            Vec::new(),
            "system prompt contains an active collection context block",
        );
    }
    push_term_signal(
        &query,
        QUESTION_TERMS,
        &mut signals,
        &mut log,
        "signal.question",
        ToolVisibilitySignalKind::Question,
        "query asks for explanation, analysis, comparison, or summary",
    );
    push_term_signal(
        &query,
        CODE_OR_TOOL_TERMS,
        &mut signals,
        &mut log,
        "signal.code_or_tool",
        ToolVisibilitySignalKind::CodeOrToolOperation,
        "query mentions code, tooling, shell, diagnostics, or agent operation",
    );
    push_term_signal(
        &query,
        FILE_ROUTE_TERMS,
        &mut signals,
        &mut log,
        "signal.file_operation",
        ToolVisibilitySignalKind::FileOperation,
        "query mentions file, directory, document, or edit operations",
    );
    push_term_signal(
        &query,
        FILE_WORKSPACE_TERMS,
        &mut signals,
        &mut log,
        "signal.file_workspace",
        ToolVisibilitySignalKind::FileWorkspace,
        "query mentions file-workspace tools such as search, notes, replacement, or grep",
    );
    push_term_signal(
        &query,
        SOURCE_TERMS,
        &mut signals,
        &mut log,
        "signal.source_management",
        ToolVisibilitySignalKind::SourceManagement,
        "query mentions source indexing or reindexing",
    );
    push_term_signal(
        &query,
        WEB_ROUTE_TERMS,
        &mut signals,
        &mut log,
        "signal.web_lookup",
        ToolVisibilitySignalKind::WebLookup,
        "query mentions URL or web inspection",
    );
    push_term_signal(
        &query,
        CONVERSATION_RECALL_TERMS,
        &mut signals,
        &mut log,
        "signal.conversation_recall",
        ToolVisibilitySignalKind::ConversationRecall,
        "query asks about earlier conversation context",
    );
    push_term_signal(
        &query,
        KNOWLEDGE_TERMS,
        &mut signals,
        &mut log,
        "signal.knowledge",
        ToolVisibilitySignalKind::KnowledgeWork,
        "query mentions memory, evidence, playbooks, collections, skills, or knowledge artifacts",
    );
    push_term_signal(
        &query,
        AUTOMATION_TERMS,
        &mut signals,
        &mut log,
        "signal.automation",
        ToolVisibilitySignalKind::Automation,
        "query explicitly requests opening or revealing a local path",
    );
    push_term_signal(
        &query,
        PROCESS_TERMS,
        &mut signals,
        &mut log,
        "signal.process",
        ToolVisibilitySignalKind::Process,
        "query mentions command execution, builds, tests, or local services",
    );
    push_term_signal(
        &query,
        TERMINAL_TERMS,
        &mut signals,
        &mut log,
        "signal.terminal",
        ToolVisibilitySignalKind::Terminal,
        "query refers to the user-visible terminal or an interactive shell",
    );
    if query_requests_browser_operation(&query) {
        push_signal(
            &mut signals,
            &mut log,
            "signal.browser",
            ToolVisibilitySignalKind::Browser,
            BROWSER_TERMS
                .iter()
                .filter(|term| query.contains(**term))
                .map(|term| (*term).to_string())
                .collect(),
            "query explicitly requests browser observation or interaction",
        );
    }
    if query_requests_web_artifact_authoring(&query) {
        push_signal(
            &mut signals,
            &mut log,
            "signal.web_artifact_authoring",
            ToolVisibilitySignalKind::WebArtifactAuthoring,
            vec!["web artifact medium + authoring intent".to_string()],
            "a runnable web artifact requires a process plus rendered visual observation",
        );
    }
    if query_requests_browser_interaction(&query) {
        push_signal(
            &mut signals,
            &mut log,
            "signal.browser_interaction",
            ToolVisibilitySignalKind::BrowserInteraction,
            vec!["browser interaction intent".to_string()],
            "the requested rendered experience includes explicit user interaction",
        );
    }
    if !has_signal(&signals, ToolVisibilitySignalKind::Browser)
        && query_has_web_navigation_handoff(&query)
    {
        push_signal(
            &mut signals,
            &mut log,
            "signal.browser_navigation_handoff",
            ToolVisibilitySignalKind::Browser,
            vec!["navigation intent + web target".to_string()],
            "explicit web navigation belongs to the shared browser session",
        );
    }
    if !has_signal(&signals, ToolVisibilitySignalKind::Automation)
        && query_has_local_path_handoff(&query)
    {
        push_signal(
            &mut signals,
            &mut log,
            "signal.local_path_handoff",
            ToolVisibilitySignalKind::Automation,
            vec!["open/reveal intent + local path".to_string()],
            "an explicit local path handoff needs the visible desktop opener",
        );
    }
    if query_requests_desktop_operation(&query) {
        push_signal(
            &mut signals,
            &mut log,
            "signal.desktop",
            ToolVisibilitySignalKind::Desktop,
            DESKTOP_TERMS
                .iter()
                .chain(NATIVE_DESKTOP_APP_TERMS.iter())
                .filter(|term| query.contains(**term))
                .map(|term| (*term).to_string())
                .collect(),
            "query explicitly requests native desktop observation or input",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Desktop)
        && query_requests_desktop_interaction(&query)
    {
        push_signal(
            &mut signals,
            &mut log,
            "signal.desktop_interaction",
            ToolVisibilitySignalKind::DesktopInteraction,
            vec!["desktop input intent".to_string()],
            "the request requires native desktop input after an observation",
        );
    }
    push_term_signal(
        &query,
        DOCUMENT_ANALYSIS_TERMS,
        &mut signals,
        &mut log,
        "signal.document_analysis",
        ToolVisibilitySignalKind::DocumentAnalysis,
        "query mentions document analysis, comparison, statistics, citations, or summaries",
    );
    if input.has_sources {
        push_signal(
            &mut signals,
            &mut log,
            "signal.linked_sources",
            ToolVisibilitySignalKind::LinkedSources,
            Vec::new(),
            "turn has an active source scope",
        );
    }

    let route = select_route(input.has_sources, &signals);
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: "route.select".to_string(),
        effect: ToolVisibilityEffect::SelectedRoute { route },
        reason: route_reason(route).to_string(),
        matched_terms: Vec::new(),
    });

    let mut active_categories = Vec::new();
    activate_category(
        &mut active_categories,
        &mut log,
        "category.always_core",
        ToolCategory::Core,
        "core tools are always visible",
    );
    if has_signal(&signals, ToolVisibilitySignalKind::CodeOrToolOperation)
        || has_signal(&signals, ToolVisibilitySignalKind::FileOperation)
        || has_signal(&signals, ToolVisibilitySignalKind::FileWorkspace)
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.filesystem",
            ToolCategory::FileSystem,
            "code/tool/file signals need filesystem and shell-capable tools",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::SourceManagement) || input.has_sources {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.source_management",
            ToolCategory::SourceManagement,
            if input.has_sources {
                "linked sources make source management tools relevant"
            } else {
                "source-management signal matched"
            },
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::KnowledgeWork)
        || (input.has_sources && has_signal(&signals, ToolVisibilitySignalKind::Question))
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.knowledge",
            ToolCategory::Knowledge,
            "knowledge or sourced-question signals need retrieval and evidence tools",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::WebLookup) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.web",
            ToolCategory::Web,
            "web lookup signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Automation) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.automation",
            ToolCategory::Automation,
            "local path handoff signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Process) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.process",
            ToolCategory::Process,
            "process signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Terminal) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.terminal",
            ToolCategory::Terminal,
            "terminal signal matched",
        );
        activate_category(
            &mut active_categories,
            &mut log,
            "category.terminal_process",
            ToolCategory::Process,
            "terminal work may require starting or inspecting a process",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Browser) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.browser_read",
            ToolCategory::BrowserRead,
            "browser signal needs rendered-page observation",
        );
        activate_category(
            &mut active_categories,
            &mut log,
            "category.browser_visual_observation",
            ToolCategory::VisualObservation,
            "browser work needs rendered-state evidence",
        );
        if !has_signal(&signals, ToolVisibilitySignalKind::WebArtifactAuthoring)
            || has_signal(&signals, ToolVisibilitySignalKind::BrowserInteraction)
        {
            activate_category(
                &mut active_categories,
                &mut log,
                "category.browser_interact",
                ToolCategory::BrowserInteract,
                "the browser request needs stateful page interaction",
            );
        }
    }
    if has_signal(&signals, ToolVisibilitySignalKind::WebArtifactAuthoring) {
        for (rule_id, category, reason) in [
            (
                "category.web_artifact_process",
                ToolCategory::Process,
                "a generated web artifact must be served or rendered",
            ),
            (
                "category.web_artifact_browser_read",
                ToolCategory::BrowserRead,
                "a generated web artifact must be inspected in a browser",
            ),
            (
                "category.web_artifact_visual_observation",
                ToolCategory::VisualObservation,
                "a generated web artifact needs pixel-bearing visual evidence",
            ),
        ] {
            activate_category(&mut active_categories, &mut log, rule_id, category, reason);
        }
    }
    if has_signal(&signals, ToolVisibilitySignalKind::BrowserInteraction) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.explicit_browser_interaction",
            ToolCategory::BrowserInteract,
            "the rendered experience includes an explicit interaction contract",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::Desktop) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.desktop_interact",
            ToolCategory::DesktopInteract,
            "desktop signal matched",
        );
    }
    if has_signal(&signals, ToolVisibilitySignalKind::DocumentAnalysis)
        || (input.has_sources && has_signal(&signals, ToolVisibilitySignalKind::Question))
    {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.document_analysis",
            ToolCategory::DocumentAnalysis,
            "document-analysis or sourced-question signals need comparison and summary tools",
        );
    }
    if has_subagent_relevance_signal(&signals) {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.subagent",
            ToolCategory::SubAgent,
            "complex, agent, or workflow signals can require delegated work tools",
        );
    }

    let route_categories = route_categories(route);
    for category in &route_categories {
        activate_category(
            &mut active_categories,
            &mut log,
            "category.route_requirement",
            *category,
            "selected route requires this tool category",
        );
    }

    let visual_observation = if has_signal(&signals, ToolVisibilitySignalKind::WebArtifactAuthoring)
        || has_signal(&signals, ToolVisibilitySignalKind::Browser)
    {
        VisualObservationRequirement::AfterLastMutation
    } else {
        VisualObservationRequirement::default()
    };
    let desktop_observation = if has_signal(&signals, ToolVisibilitySignalKind::Desktop) {
        DesktopObservationRequirement::BeforeControlAndAfterLastControl
    } else {
        DesktopObservationRequirement::default()
    };
    let interaction = TurnInteractionRequirements {
        visual_observation,
        browser_observation: has_signal(&signals, ToolVisibilitySignalKind::Browser),
        browser_interaction: has_signal(&signals, ToolVisibilitySignalKind::BrowserInteraction),
        browser_terminal_closure: browser_terminal_closure_requirement(&query),
        desktop_observation,
        desktop_interaction: has_signal(&signals, ToolVisibilitySignalKind::DesktopInteraction),
    };

    TurnCapabilityRequirements {
        route,
        active_categories,
        route_categories,
        signals,
        log,
        interaction,
    }
}

pub fn system_prompt_has_collection_context(system_prompt: &str) -> bool {
    system_prompt
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("## Collection Context"))
}

fn select_route(has_sources: bool, signals: &[ToolVisibilitySignal]) -> ToolVisibilityRouteKind {
    if has_signal(signals, ToolVisibilitySignalKind::CollectionContext) {
        return ToolVisibilityRouteKind::CollectionFocused;
    }
    if has_signal(signals, ToolVisibilitySignalKind::SourceManagement) {
        return ToolVisibilityRouteKind::SourceManagement;
    }
    if has_signal(signals, ToolVisibilitySignalKind::Desktop) {
        return ToolVisibilityRouteKind::InteractionOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::WebArtifactAuthoring) {
        return ToolVisibilityRouteKind::CodebaseOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::CodeOrToolOperation)
        || has_signal(signals, ToolVisibilitySignalKind::Process)
        || has_signal(signals, ToolVisibilitySignalKind::Terminal)
    {
        return ToolVisibilityRouteKind::CodebaseOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::FileOperation) {
        return ToolVisibilityRouteKind::FileOperation;
    }
    if has_signal(signals, ToolVisibilitySignalKind::ConversationRecall) {
        return ToolVisibilityRouteKind::ConversationRecall;
    }
    if has_signal(signals, ToolVisibilitySignalKind::Browser) {
        return ToolVisibilityRouteKind::WebLookup;
    }
    if has_signal(signals, ToolVisibilitySignalKind::WebLookup) {
        return ToolVisibilityRouteKind::WebLookup;
    }
    if has_sources && has_signal(signals, ToolVisibilitySignalKind::Question) {
        return ToolVisibilityRouteKind::KnowledgeRetrieval;
    }
    ToolVisibilityRouteKind::DirectResponse
}

fn route_categories(route: ToolVisibilityRouteKind) -> Vec<ToolCategory> {
    match route {
        ToolVisibilityRouteKind::CollectionFocused
        | ToolVisibilityRouteKind::ConversationRecall
        | ToolVisibilityRouteKind::KnowledgeRetrieval => {
            vec![ToolCategory::Knowledge, ToolCategory::DocumentAnalysis]
        }
        ToolVisibilityRouteKind::SourceManagement => vec![ToolCategory::SourceManagement],
        ToolVisibilityRouteKind::CodebaseOperation | ToolVisibilityRouteKind::FileOperation => {
            vec![
                ToolCategory::FileSystem,
                ToolCategory::Process,
                ToolCategory::DocumentAnalysis,
            ]
        }
        ToolVisibilityRouteKind::WebLookup => vec![ToolCategory::Web],
        ToolVisibilityRouteKind::InteractionOperation => vec![ToolCategory::DesktopInteract],
        ToolVisibilityRouteKind::DirectResponse => Vec::new(),
    }
}

fn route_reason(route: ToolVisibilityRouteKind) -> &'static str {
    match route {
        ToolVisibilityRouteKind::CollectionFocused => {
            "collection context has highest priority over query text"
        }
        ToolVisibilityRouteKind::SourceManagement => {
            "source/index operations should be handled directly"
        }
        ToolVisibilityRouteKind::CodebaseOperation => {
            "code or tool operations need codebase tooling and verification"
        }
        ToolVisibilityRouteKind::FileOperation => {
            "file/document operations need filesystem-oriented tools"
        }
        ToolVisibilityRouteKind::ConversationRecall => {
            "conversation recall should inspect current conversation context first"
        }
        ToolVisibilityRouteKind::WebLookup => "URL or web requests need web tools",
        ToolVisibilityRouteKind::InteractionOperation => {
            "native desktop work requires an observe-control-observe interaction contract"
        }
        ToolVisibilityRouteKind::KnowledgeRetrieval => {
            "sourced question needs grounded retrieval and evidence synthesis"
        }
        ToolVisibilityRouteKind::DirectResponse => {
            "no specialized policy signal requires a route-specific tool set"
        }
    }
}

fn push_term_signal(
    query: &str,
    terms: &[&str],
    signals: &mut Vec<ToolVisibilitySignal>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    kind: ToolVisibilitySignalKind,
    reason: &str,
) {
    let matched_terms = terms
        .iter()
        .filter(|term| query_contains_signal_term(query, term))
        .map(|term| (*term).to_string())
        .collect::<Vec<_>>();
    if matched_terms.is_empty() {
        return;
    }
    push_signal(signals, log, rule_id, kind, matched_terms, reason);
}

fn query_contains_signal_term(query: &str, term: &str) -> bool {
    if term.len() > 3 || !term.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return query.contains(term);
    }
    query.match_indices(term).any(|(start, _)| {
        let before = query[..start].chars().next_back();
        let end = start + term.len();
        let after = query[end..].chars().next();
        before.is_none_or(|character| !is_signal_word_character(character))
            && after.is_none_or(|character| !is_signal_word_character(character))
    })
}

fn is_signal_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn push_signal(
    signals: &mut Vec<ToolVisibilitySignal>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    kind: ToolVisibilitySignalKind,
    matched_terms: Vec<String>,
    reason: &str,
) {
    signals.push(ToolVisibilitySignal {
        kind,
        matched_terms: matched_terms.clone(),
    });
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: rule_id.to_string(),
        effect: ToolVisibilityEffect::MatchedSignal { signal: kind },
        reason: reason.to_string(),
        matched_terms,
    });
}

fn activate_category(
    categories: &mut Vec<ToolCategory>,
    log: &mut Vec<ToolVisibilityDecisionLogEntry>,
    rule_id: &str,
    category: ToolCategory,
    reason: &str,
) {
    if categories.contains(&category) {
        return;
    }
    categories.push(category);
    log.push(ToolVisibilityDecisionLogEntry {
        rule_id: rule_id.to_string(),
        effect: ToolVisibilityEffect::ActivatedCategory { category },
        reason: reason.to_string(),
        matched_terms: Vec::new(),
    });
}

fn has_signal(signals: &[ToolVisibilitySignal], kind: ToolVisibilitySignalKind) -> bool {
    signals.iter().any(|signal| signal.kind == kind)
}

fn query_has_web_navigation_handoff(query: &str) -> bool {
    contains_any(query, NAVIGATION_INTENT_TERMS)
        && !query_has_local_path_target(query)
        && (contains_any(query, WEB_NAVIGATION_TARGET_TERMS) || query_has_explicit_url(query))
}

fn query_has_local_path_handoff(query: &str) -> bool {
    contains_any(query, LOCAL_PATH_HANDOFF_INTENT_TERMS) && query_has_local_path_target(query)
}

fn query_requests_web_artifact_authoring(query: &str) -> bool {
    contains_any(query, WEB_ARTIFACT_MEDIA_TERMS)
        && contains_any(query, ARTIFACT_AUTHORING_INTENT_TERMS)
}

fn query_requests_browser_operation(query: &str) -> bool {
    query_requests_browser_terminal_closure(query)
        || (contains_any(query, BROWSER_TERMS)
            && (contains_any(query, BROWSER_OPERATION_INTENT_TERMS)
                || query_has_explicit_url(query)))
}

fn query_requests_browser_interaction(query: &str) -> bool {
    (query_requests_web_artifact_authoring(query)
        || query_requests_browser_operation(query)
        || query_has_web_navigation_handoff(query))
        && (contains_any(query, BROWSER_INTERACTION_TERMS)
            || query_requests_browser_terminal_closure(query)
            || contains_any(query, NAVIGATION_INTENT_TERMS))
}

fn query_requests_browser_terminal_closure(query: &str) -> bool {
    browser_terminal_closure_requirement(query).is_required()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserTerminalClosureClause {
    Unrelated,
    Affirmative(BrowserTerminalClosureRequirement),
    Negated(BrowserTerminalClosureNegationScope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserTerminalClosureNegationScope {
    Any,
    Tab,
    Session,
}

impl BrowserTerminalClosureNegationScope {
    fn cancels(self, requirement: BrowserTerminalClosureRequirement) -> bool {
        match self {
            Self::Any => requirement.is_required(),
            // Closing a whole session necessarily closes its tabs, so a later
            // explicit instruction to preserve a tab also cancels that wider
            // destructive action.
            Self::Tab => matches!(
                requirement,
                BrowserTerminalClosureRequirement::Tab
                    | BrowserTerminalClosureRequirement::AllTabs
                    | BrowserTerminalClosureRequirement::Session
            ),
            Self::Session => requirement == BrowserTerminalClosureRequirement::Session,
        }
    }
}

fn browser_terminal_closure_requirement(query: &str) -> BrowserTerminalClosureRequirement {
    let query = query.trim();
    let clauses = browser_terminal_closure_clauses(query);
    if browser_terminal_query_is_question(query)
        && browser_terminal_query_is_global_discussion(query)
        && !clauses.iter().any(|clause| {
            matches!(
                browser_terminal_closure_clause(clause),
                BrowserTerminalClosureClause::Affirmative(_)
            )
        })
    {
        return BrowserTerminalClosureRequirement::NotRequired;
    }
    let mut requirement = BrowserTerminalClosureRequirement::NotRequired;
    for clause in clauses {
        match browser_terminal_closure_clause(clause) {
            BrowserTerminalClosureClause::Affirmative(clause_requirement) => {
                requirement = clause_requirement;
            }
            BrowserTerminalClosureClause::Negated(scope) if scope.cancels(requirement) => {
                requirement = BrowserTerminalClosureRequirement::NotRequired;
            }
            BrowserTerminalClosureClause::Unrelated | BrowserTerminalClosureClause::Negated(_) => {}
        }
    }
    requirement
}

fn browser_terminal_query_is_question(query: &str) -> bool {
    let query = query.trim();
    query.ends_with(['?', '？', '吗', '呢'])
        || BROWSER_TERMINAL_QUESTION_PREFIXES
            .iter()
            .any(|prefix| query.starts_with(prefix))
        || BROWSER_TERMINAL_QUESTION_INFIXES
            .iter()
            .any(|infix| query.contains(infix))
        || BROWSER_TERMINAL_CHINESE_QUESTION_MARKERS
            .iter()
            .any(|marker| query.contains(marker))
}

fn browser_terminal_query_is_global_discussion(query: &str) -> bool {
    BROWSER_TERMINAL_GLOBAL_DISCUSSION_PREFIXES
        .iter()
        .any(|prefix| query.trim_start().starts_with(prefix))
}

fn browser_terminal_closure_clauses(query: &str) -> Vec<&str> {
    let mut separators = Vec::new();
    for (index, character) in query.char_indices() {
        if matches!(
            character,
            ',' | ';' | '.' | '!' | '?' | '，' | '；' | '。' | '！' | '？'
        ) {
            let end = index + character.len_utf8();
            if browser_terminal_clause_separator_is_supported(query, index, end, false, true) {
                separators.push((index, end));
            }
        }
    }
    for marker in BROWSER_TERMINAL_CLAUSE_SEPARATORS {
        separators.extend(query.match_indices(marker).filter_map(|(index, _)| {
            let end = index + marker.len();
            browser_terminal_clause_separator_is_supported(
                query,
                index,
                end,
                BROWSER_TERMINAL_PRIOR_IMPERATIVE_SEPARATORS.contains(marker),
                BROWSER_TERMINAL_PRIOR_IMPERATIVE_SEPARATORS.contains(marker),
            )
            .then_some((index, end))
        }));
    }
    for marker in BROWSER_TERMINAL_SINGLE_CHARACTER_CLAUSE_SEPARATORS {
        separators.extend(query.match_indices(marker).filter_map(|(index, _)| {
            let end = index + marker.len();
            browser_terminal_clause_separator_is_supported(
                query,
                index,
                end,
                BROWSER_TERMINAL_PRIOR_IMPERATIVE_SEPARATORS.contains(marker),
                BROWSER_TERMINAL_PRIOR_IMPERATIVE_SEPARATORS.contains(marker),
            )
            .then_some((index, end))
        }));
    }
    separators.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));

    let mut clauses = Vec::new();
    let mut cursor = 0;
    for (start, end) in separators {
        if start < cursor {
            continue;
        }
        if start > cursor {
            clauses.push(query[cursor..start].trim());
        }
        cursor = end;
    }
    if cursor < query.len() {
        clauses.push(query[cursor..].trim());
    }
    if clauses.is_empty() {
        clauses.push(query.trim());
    }
    clauses
        .into_iter()
        .filter(|clause| !clause.is_empty())
        .collect()
}

fn browser_terminal_clause_separator_is_supported(
    query: &str,
    start: usize,
    end: usize,
    allow_prior_imperative: bool,
    allow_temporal_precondition: bool,
) -> bool {
    let left = query[..start].trim();
    let right = query[end..]
        .trim_start_matches([',', ';', '，', '；'])
        .trim();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let right_clause = browser_terminal_closure_clause(right);
    if right_clause != BrowserTerminalClosureClause::Unrelated {
        return !matches!(right_clause, BrowserTerminalClosureClause::Affirmative(_))
            || matches!(
                browser_terminal_closure_clause(left),
                BrowserTerminalClosureClause::Affirmative(_)
            )
            || (allow_temporal_precondition
                && browser_terminal_prior_clause_is_temporal_precondition(left))
            || (allow_prior_imperative && browser_terminal_prior_clause_is_imperative(left));
    }
    matches!(
        browser_terminal_closure_clause(left),
        BrowserTerminalClosureClause::Affirmative(_)
    ) && browser_terminal_followup_is_command(right)
}

fn browser_terminal_prior_clause_is_temporal_precondition(query: &str) -> bool {
    let command = browser_terminal_closure_command(query);
    if BROWSER_TERMINAL_DECISION_QUESTION_MARKERS
        .iter()
        .chain(BROWSER_TERMINAL_PRIOR_DISCUSSION_MARKERS.iter())
        .chain(BROWSER_TERMINAL_PRIOR_INTERROGATIVE_MARKERS.iter())
        .any(|marker| command.contains(marker))
    {
        return false;
    }
    command.starts_with("after ")
        || command.starts_with("once ")
        || matches!(command, "when finished" | "when done")
        || command.starts_with("when finished ")
        || command.starts_with("when done ")
        || command.ends_with('后')
        || command.ends_with("之后")
        || command.ends_with("以后")
}

fn browser_terminal_prior_clause_is_imperative(query: &str) -> bool {
    let command = browser_terminal_closure_command(query);
    let decision_or_discussion = BROWSER_TERMINAL_DECISION_QUESTION_MARKERS
        .iter()
        .any(|marker| command.contains(marker))
        || BROWSER_TERMINAL_PRIOR_DISCUSSION_MARKERS
            .iter()
            .any(|marker| command.contains(marker));
    if decision_or_discussion {
        return false;
    }
    if BROWSER_TERMINAL_REPORTING_FOLLOWUP_PREFIXES
        .iter()
        .any(|prefix| command.starts_with(prefix))
    {
        return true;
    }
    if BROWSER_TERMINAL_PRIOR_INTERROGATIVE_MARKERS
        .iter()
        .any(|marker| command.contains(marker))
    {
        return false;
    }
    BROWSER_TERMINAL_PRIOR_IMPERATIVE_PREFIXES
        .iter()
        .any(|prefix| command.starts_with(prefix))
}

fn browser_terminal_followup_is_command(query: &str) -> bool {
    let command = browser_terminal_closure_command(query);
    if BROWSER_TERMINAL_REPORTING_FOLLOWUP_PREFIXES
        .iter()
        .any(|prefix| command.starts_with(prefix))
    {
        return !BROWSER_TERMINAL_DECISION_QUESTION_MARKERS
            .iter()
            .any(|marker| command.contains(marker));
    }
    if browser_terminal_query_is_question(query) {
        return false;
    }
    BROWSER_TERMINAL_CLAUSE_FOLLOWUP_PREFIXES
        .iter()
        .any(|prefix| command.starts_with(prefix))
}

fn browser_terminal_closure_clause(query: &str) -> BrowserTerminalClosureClause {
    let temporal_command = BROWSER_TERMINAL_LEADING_TEMPORAL_CLOSURE_PREFIXES
        .iter()
        .find_map(|prefix| query.trim_start().strip_prefix(prefix))
        .map(browser_terminal_closure_command);
    if temporal_command.is_some() && browser_terminal_query_is_question(query) {
        return BrowserTerminalClosureClause::Unrelated;
    }
    let command = temporal_command.unwrap_or_else(|| browser_terminal_closure_command(query));
    if browser_terminal_closure_is_discussion(command) {
        return BrowserTerminalClosureClause::Unrelated;
    }
    if let Some(scope) = browser_terminal_closure_negation_scope(command) {
        return BrowserTerminalClosureClause::Negated(scope);
    }
    if command_matches_browser_terminal_closure(command, BROWSER_TERMINAL_ALL_TABS_CLOSURE_COMMANDS)
    {
        return BrowserTerminalClosureClause::Affirmative(
            BrowserTerminalClosureRequirement::AllTabs,
        );
    }
    if command_matches_browser_terminal_closure(command, BROWSER_TERMINAL_TAB_CLOSURE_COMMANDS) {
        return BrowserTerminalClosureClause::Affirmative(BrowserTerminalClosureRequirement::Tab);
    }
    if command_matches_browser_terminal_closure(command, BROWSER_TERMINAL_SESSION_CLOSURE_COMMANDS)
    {
        return BrowserTerminalClosureClause::Affirmative(
            BrowserTerminalClosureRequirement::Session,
        );
    }
    BrowserTerminalClosureClause::Unrelated
}

fn browser_terminal_closure_command(query: &str) -> &str {
    let mut command = query.trim();
    while let Some(rest) = BROWSER_TERMINAL_CLOSURE_COMMAND_PREFIXES
        .iter()
        .find_map(|prefix| command.strip_prefix(prefix))
    {
        command = rest.trim_start();
    }
    loop {
        let trimmed = command.trim_end_matches([
            ',', ';', ':', '.', '!', '?', '，', '；', '：', '。', '！', '？',
        ]);
        let Some(rest) = BROWSER_TERMINAL_CLOSURE_COMMAND_SUFFIXES
            .iter()
            .find_map(|suffix| trimmed.strip_suffix(suffix))
        else {
            return trimmed;
        };
        command = rest.trim_end();
    }
}

fn browser_terminal_closure_negation_scope(
    command: &str,
) -> Option<BrowserTerminalClosureNegationScope> {
    let mut negated = BROWSER_TERMINAL_CLOSURE_NEGATION_PREFIXES
        .iter()
        .find_map(|prefix| command.strip_prefix(prefix))?
        .trim_start();
    while let Some(rest) = BROWSER_TERMINAL_CLOSURE_NEGATION_FILLERS
        .iter()
        .find_map(|filler| negated.strip_prefix(filler))
    {
        negated = rest.trim_start();
    }
    let negated = browser_terminal_closure_command(negated);
    if command_matches_browser_terminal_closure(negated, BROWSER_TERMINAL_ALL_TABS_CLOSURE_COMMANDS)
    {
        return Some(BrowserTerminalClosureNegationScope::Tab);
    }
    if command_matches_browser_terminal_closure(negated, BROWSER_TERMINAL_TAB_CLOSURE_COMMANDS) {
        return Some(BrowserTerminalClosureNegationScope::Tab);
    }
    if command_matches_browser_terminal_closure(negated, BROWSER_TERMINAL_SESSION_CLOSURE_COMMANDS)
    {
        return Some(BrowserTerminalClosureNegationScope::Session);
    }
    command_matches_browser_terminal_closure(negated, BROWSER_TERMINAL_GENERIC_CLOSURE_COMMANDS)
        .then_some(BrowserTerminalClosureNegationScope::Any)
}

fn command_matches_browser_terminal_closure(command: &str, intents: &[&str]) -> bool {
    intents.contains(&command)
}

fn browser_terminal_closure_is_discussion(query: &str) -> bool {
    let query = browser_terminal_closure_command(query);
    BROWSER_TERMINAL_CLOSURE_DISCUSSION_PREFIXES
        .iter()
        .any(|prefix| query.starts_with(prefix))
}

fn query_requests_desktop_operation(query: &str) -> bool {
    (contains_any(query, DESKTOP_TERMS) && contains_any(query, DESKTOP_OPERATION_INTENT_TERMS))
        || (contains_any(query, NATIVE_DESKTOP_APP_TERMS)
            && contains_any(query, DESKTOP_OPERATION_INTENT_TERMS))
}

fn query_requests_desktop_interaction(query: &str) -> bool {
    contains_any(query, DESKTOP_INTERACTION_TERMS)
        || (contains_any(query, NATIVE_DESKTOP_APP_TERMS)
            && contains_any(query, DESKTOP_OPERATION_INTENT_TERMS)
            && contains_any(query, DESKTOP_APP_ACTIVATION_TERMS))
}

fn query_has_local_path_target(query: &str) -> bool {
    query
        .split_whitespace()
        .map(trim_handoff_token)
        .any(looks_like_local_path)
}

fn contains_any(query: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| query.contains(*term))
}

fn query_has_explicit_url(query: &str) -> bool {
    query
        .split_whitespace()
        .map(trim_handoff_token)
        .any(|token| token.starts_with("http://") || token.starts_with("https://"))
}

fn trim_handoff_token(token: &str) -> &str {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
    });
    token.trim_end_matches(['.', '?', '!'])
}

fn looks_like_local_path(token: &str) -> bool {
    if token.is_empty()
        || token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("www.")
    {
        return false;
    }
    let bytes = token.as_bytes();
    let has_drive_prefix = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if has_drive_prefix
        || token.starts_with('/')
        || token.starts_with('\\')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.starts_with("~\\")
        || token.contains('/')
        || token.contains('\\')
    {
        return true;
    }
    token
        .rsplit_once('.')
        .map(|(_, extension)| LOCAL_FILE_EXTENSIONS.contains(&extension))
        .unwrap_or(false)
}

fn has_subagent_relevance_signal(signals: &[ToolVisibilitySignal]) -> bool {
    const SUBAGENT_RELEVANCE_TERMS: &[&str] = &[
        "agent",
        "agents",
        "agentic",
        "subagent",
        "workflow",
        "complex",
        "multi-step",
        "multi step",
        "architecture",
        "review",
        "compare",
        "comparison",
        "research",
        "verify",
        "verification",
        "critique",
        "并行",
        "复杂",
        "研究",
        "验证",
        "审查",
        "评审",
        "对比",
        "比较",
        "架构",
        "主agent",
        "子agent",
    ];

    signals.iter().any(|signal| {
        matches!(
            signal.kind,
            ToolVisibilitySignalKind::CodeOrToolOperation
                | ToolVisibilitySignalKind::KnowledgeWork
                | ToolVisibilitySignalKind::DocumentAnalysis
                | ToolVisibilitySignalKind::WebLookup
        ) && signal
            .matched_terms
            .iter()
            .any(|term| SUBAGENT_RELEVANCE_TERMS.contains(&term.as_str()))
    })
}

const QUESTION_TERMS: &[&str] = &[
    "?",
    "what",
    "why",
    "how",
    "which",
    "where",
    "when",
    "who",
    "tell me",
    "explain",
    "analyze",
    "analysis",
    "summarize",
    "compare",
    "review",
    "分析",
    "总结",
    "为什么",
    "如何",
    "怎么",
    "哪些",
    "什么",
    "解释",
    "帮我看",
    "看一下",
    "到底",
    "还有哪些",
    "还有多少",
];

const CODE_OR_TOOL_TERMS: &[&str] = &[
    "run_shell",
    "run shell",
    "shell",
    "terminal",
    "command",
    "powershell",
    "cmd",
    "cargo",
    "npm",
    "pnpm",
    "node",
    "python",
    "git",
    "tool",
    "tools",
    "agent",
    "agents",
    "subagent",
    "claude",
    "deepseek",
    "top_agents",
    "top agents",
    "cli",
    "ide",
    "extension",
    "eval",
    "evaluation",
    "benchmark",
    "accuracy",
    "hitrate",
    "hit rate",
    "unavailable",
    "available",
    "fix",
    "debug",
    "bug",
    "test",
    "build",
    "compile",
    "运行",
    "命令",
    "终端",
    "调用",
    "工具",
    "不可用",
    "修复",
    "排查",
    "测试",
    "构建",
    "编译",
    "代码",
    "代码细节",
    "架构",
    "架构设计",
    "模型",
    "路由",
    "技能",
    "命中",
    "命中率",
    "命中效率",
    "准确率",
    "召回率",
    "评测",
    "基准",
    "项目",
    "仓库",
    "主agent",
    "子agent",
];

const FILE_ROUTE_TERMS: &[&str] = &[
    "file",
    "read",
    "edit",
    "write",
    "create",
    "move",
    "rename",
    "copy",
    "delete",
    "directory",
    "folder",
    "document",
    "word",
    "docx",
    "excel",
    "xlsx",
    "ppt",
    "pptx",
    "office",
    "文件",
    "读取",
    "编辑",
    "写",
    "写入",
    "新建",
    "创建",
    "保存",
    "修改",
    "移动",
    "重命名",
    "复制",
    "删除",
    "目录",
    "文档",
    "幻灯片",
    "表格",
];

const FILE_WORKSPACE_TERMS: &[&str] = &[
    "file",
    "read",
    "edit",
    "replace",
    "write",
    "create",
    "find",
    "grep",
    "rg",
    "move",
    "rename",
    "copy",
    "delete",
    "directory",
    "folder",
    "note",
    "document",
    "word",
    "docx",
    "excel",
    "xlsx",
    "ppt",
    "pptx",
    "office",
    "文件",
    "读取",
    "编辑",
    "写",
    "写入",
    "新建",
    "创建",
    "保存",
    "修改",
    "改写",
    "润色",
    "替换",
    "查找",
    "搜索文件",
    "移动",
    "重命名",
    "复制",
    "删除",
    "目录",
    "笔记",
    "文档",
    "幻灯片",
    "表格",
];

const SOURCE_TERMS: &[&str] = &["source", "index", "reindex", "数据源", "索引"];

const WEB_ROUTE_TERMS: &[&str] = &[
    "url",
    "http",
    "website",
    "web ",
    "web search",
    "search online",
    "internet",
    "fetch",
    "link",
    "网页",
    "网页搜索",
    "搜索网页",
    "联网",
    "网上",
    "链接",
];

const CONVERSATION_RECALL_TERMS: &[&str] = &[
    "earlier",
    "previous",
    "before",
    "this conversation",
    "chat history",
    "we discussed",
    "刚才",
    "之前",
    "上面",
    "这段对话",
];

const KNOWLEDGE_TERMS: &[&str] = &[
    "remember",
    "memory",
    "session",
    "history",
    "harness",
    "evolution",
    "evolve",
    "playbook",
    "collection",
    "collections",
    "citation",
    "citations",
    "evidence",
    "saved",
    "bookmark",
    "skill",
    "agent",
    "agentic",
    "workflow",
    "complex",
    "multi-step",
    "multi step",
    "research",
    "compile",
    "compilation",
    "entity",
    "entities",
    "graph",
    "knowledge",
    "health",
    "archive",
    "wiki",
    "concept",
    "concepts",
    "收藏",
    "引用",
    "证据",
    "记住",
    "记忆",
    "会话",
    "历史",
    "进化",
    "自我",
    "编译",
    "实体",
    "图谱",
    "知识",
    "健康",
    "归档",
    "概念",
    "技能",
    "研究",
    "命中",
    "命中率",
    "命中效率",
    "准确率",
    "评测",
    "基准",
];

const AUTOMATION_TERMS: &[&str] = &[
    "open path",
    "open file",
    "open folder",
    "open this path",
    "open this file",
    "open this folder",
    "reveal path",
    "reveal file",
    "reveal folder",
    "reveal this path",
    "reveal this file",
    "reveal this folder",
    "show in explorer",
    "show in finder",
    "打开路径",
    "打开文件",
    "打开文件夹",
    "打开这个路径",
    "打开这个文件",
    "打开这个文件夹",
    "定位路径",
    "定位文件",
    "定位文件夹",
    "定位这个路径",
    "定位这个文件",
    "定位这个文件夹",
];

const PROCESS_TERMS: &[&str] = &[
    "run_shell",
    "run shell",
    "shell",
    "command",
    "powershell",
    "pwsh",
    "cmd",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "python",
    "build",
    "compile",
    "test",
    "dev server",
    "local server",
    "运行",
    "命令",
    "构建",
    "编译",
    "测试",
    "本地服务",
];

const TERMINAL_TERMS: &[&str] = &[
    "terminal",
    "powershell",
    "pwsh",
    "command prompt",
    "cmd",
    "interactive shell",
    "终端",
    "命令行",
];

const BROWSER_TERMS: &[&str] = &[
    "browser",
    "browser session",
    "localhost",
    "local app",
    "local page",
    "dev server",
    "浏览器",
    "本地页面",
    "本地网页",
];

const WEB_ARTIFACT_MEDIA_TERMS: &[&str] = &[
    "html",
    "canvas",
    "webgl",
    "three.js",
    "threejs",
    "react app",
    "vue app",
    "svelte app",
    "spa",
    "single-page app",
    "single page app",
    "网页应用",
    "单页应用",
    "前端页面",
];

const ARTIFACT_AUTHORING_INTENT_TERMS: &[&str] = &[
    "build",
    "create",
    "make",
    "implement",
    "write",
    "generate",
    "design",
    "制作",
    "创建",
    "新建",
    "实现",
    "写",
    "生成",
    "设计",
    "做一个",
];

const BROWSER_INTERACTION_TERMS: &[&str] = &[
    "interactive",
    "interaction",
    "click",
    "drag",
    "drop",
    "type into",
    "keyboard",
    "mouse",
    "scroll",
    "交互",
    "点击",
    "拖拽",
    "拖动",
    "输入",
    "键盘",
    "鼠标",
    "滚动",
];

const BROWSER_OPERATION_INTENT_TERMS: &[&str] = &[
    "open",
    "visit",
    "navigate",
    "inspect",
    "check",
    "verify",
    "capture",
    "debug",
    "click",
    "drag",
    "type into",
    "scroll",
    "打开",
    "访问",
    "导航",
    "检查",
    "验证",
    "截取",
    "捕获",
    "调试",
    "点击",
    "拖拽",
    "输入",
    "滚动",
];

const BROWSER_TERMINAL_CLOSURE_COMMAND_PREFIXES: &[&str] = &[
    "please ",
    "can you ",
    "could you ",
    "would you ",
    "first ",
    "请你",
    "请",
    "帮我",
    "麻烦你",
    "麻烦",
    "先",
    "再",
];

// A dependent clause can still assign the close to the Agent, for example
// "After you close the browser, tell me the result". Keep this subject-bound
// instead of accepting generic hypothetical "after close" prose.
const BROWSER_TERMINAL_LEADING_TEMPORAL_CLOSURE_PREFIXES: &[&str] = &["after you ", "once you "];

const BROWSER_TERMINAL_CLOSURE_COMMAND_SUFFIXES: &[&str] = &[
    " please",
    " now",
    " immediately",
    " for me",
    "吧",
    "现在",
    "立即",
    "马上",
    "一下",
];

const BROWSER_TERMINAL_QUESTION_PREFIXES: &[&str] = &[
    "how ", "what ", "why ", "when ", "where ", "who ", "which ", "should ", "is ", "are ",
    "does ", "will ", "would ", "could ", "can ", "do ",
];

const BROWSER_TERMINAL_QUESTION_INFIXES: &[&str] = &[
    " is ", " are ", " does ", " will ", " would ", " could ", " can ",
];

const BROWSER_TERMINAL_CHINESE_QUESTION_MARKERS: &[&str] = &[
    "如何",
    "什么",
    "多少",
    "是否",
    "会不会",
    "为什么",
    "为何",
    "怎样",
    "怎么",
    "多久",
    "多大",
];

const BROWSER_TERMINAL_GLOBAL_DISCUSSION_PREFIXES: &[&str] = &[
    "how ",
    "what ",
    "why ",
    "should i ",
    "should we ",
    "is it ",
    "we need a policy ",
    "policy ",
    "please explain how ",
    "please explain whether ",
    "could you explain how ",
    "could you explain whether ",
    "can you tell me if ",
    "can you tell me whether ",
    "如何",
    "怎么",
    "怎样",
    "为什么",
    "为何",
    "是否",
    "请问",
    "请告诉我",
    "请说明",
    "告诉我是否",
    "解释如何",
    "解释为什么",
];

const BROWSER_TERMINAL_CLAUSE_SEPARATORS: &[&str] = &[
    " and then ",
    " and ",
    " then ",
    " but ",
    " however ",
    " after ",
    " before ",
    "然后",
    "之后",
    "以后",
    "最后",
    "但是",
    "不过",
    "并且",
];

const BROWSER_TERMINAL_SINGLE_CHARACTER_CLAUSE_SEPARATORS: &[&str] = &["再", "后", "但", "并"];

const BROWSER_TERMINAL_PRIOR_IMPERATIVE_SEPARATORS: &[&str] = &[
    " and then ",
    " then ",
    "然后",
    "最后",
    "之后",
    "以后",
    "再",
    "后",
];

const BROWSER_TERMINAL_CLAUSE_FOLLOWUP_PREFIXES: &[&str] = &[
    "open ",
    "launch ",
    "start ",
    "save ",
    "continue ",
    "send ",
    "write ",
    "create ",
    "focus ",
    "check ",
    "verify ",
    "inspect ",
    "run ",
    "opening ",
    "launching ",
    "starting ",
    "checking ",
    "verifying ",
    "saving ",
    "finishing ",
    "sending ",
    "writing ",
    "creating ",
    "running ",
    "打开文件",
    "打开应用",
    "打开另一个应用",
    "打开窗口",
    "打开页面",
    "启动应用",
    "启动程序",
    "启动服务",
    "保存结果",
    "保存文件",
    "保存更改",
    "继续操作",
    "继续任务",
    "发送结果",
    "写入文件",
    "创建文件",
    "新建任务",
    "聚焦窗口",
    "检查结果",
    "验证结果",
    "查看结果",
    "运行命令",
];

const BROWSER_TERMINAL_PRIOR_IMPERATIVE_PREFIXES: &[&str] = &[
    "inspect ",
    "check ",
    "verify ",
    "open ",
    "visit ",
    "navigate ",
    "look ",
    "read ",
    "save ",
    "finish ",
    "complete ",
    "tell ",
    "report ",
    "summarize ",
    "return ",
    "检查",
    "查看",
    "验证",
    "打开",
    "访问",
    "导航",
    "读取",
    "保存",
    "完成",
    "处理",
    "告诉",
    "汇报",
    "总结",
    "说明",
];

const BROWSER_TERMINAL_REPORTING_FOLLOWUP_PREFIXES: &[&str] = &[
    "tell ",
    "report ",
    "summarize ",
    "return ",
    "explain ",
    "告诉",
    "汇报",
    "总结",
    "说明",
    "解释",
];

const BROWSER_TERMINAL_DECISION_QUESTION_MARKERS: &[&str] = &[
    "whether ",
    " if ",
    "should ",
    "是否",
    "要不要",
    "应不应该",
    "该不该",
    "需不需要",
];

const BROWSER_TERMINAL_PRIOR_DISCUSSION_MARKERS: &[&str] = &[
    "that means",
    "do you mean",
    "means ",
    "instruction",
    "guide ",
    "document ",
    "says ",
    "sequence",
    "steps",
    "words",
    "text ",
    "content",
    ":",
    "\"",
    "'",
    "`",
    "意味着",
    "意思是",
    "你的意思",
    "是指",
    "这条说明",
    "指南",
    "文档",
    "写着",
    "内容",
    "步骤",
    "文字",
    "这段",
    "这句话",
    "以下",
    "：",
    "“",
    "”",
    "‘",
    "’",
];

const BROWSER_TERMINAL_PRIOR_INTERROGATIVE_MARKERS: &[&str] = &[
    " how ",
    " what ",
    " why ",
    " when ",
    " where ",
    " who ",
    " which ",
    " do i ",
    "如何",
    "怎么",
    "怎样",
    "为什么",
    "何时",
    "什么时候",
    "哪里",
    "哪个",
    "谁",
];

const BROWSER_TERMINAL_CLOSURE_NEGATION_PREFIXES: &[&str] = &[
    "do not ",
    "don't ",
    "don’t ",
    "dont ",
    "never ",
    "不要再",
    "别再",
    "不要",
    "别",
    "无需",
    "不用",
    "不必",
];

const BROWSER_TERMINAL_CLOSURE_NEGATION_FILLERS: &[&str] =
    &["actually ", "really ", "truly ", "再", "真的", "实际"];

const BROWSER_TERMINAL_CLOSURE_DISCUSSION_PREFIXES: &[&str] = &[
    "how ",
    "what ",
    "why ",
    "should i ",
    "should we ",
    "is it ",
    "tell me ",
    "explain ",
    "we need a policy ",
    "policy ",
    "如何",
    "怎么",
    "怎样",
    "为什么",
    "是否",
    "告诉我",
    "解释",
];

const BROWSER_TERMINAL_TAB_CLOSURE_COMMANDS: &[&str] = &[
    "close tab",
    "close tabs",
    "close the tab",
    "close current tab",
    "close this tab",
    "close a browser tab",
    "close browser tab",
    "close browser tabs",
    "close the browser tab",
    "close current browser tab",
    "close the current browser tab",
    "close this browser tab",
    "close the current tab",
    "关闭当前标签页",
    "关闭这个标签页",
    "关闭标签页",
    "关闭浏览器标签页",
    "关掉当前标签页",
    "关掉这个标签页",
];

const BROWSER_TERMINAL_ALL_TABS_CLOSURE_COMMANDS: &[&str] = &[
    "close all tabs",
    "close every tab",
    "close each tab",
    "close all browser tabs",
    "close every browser tab",
    "close each browser tab",
    "关闭所有标签页",
    "关闭全部标签页",
    "关闭每个标签页",
    "关闭所有浏览器标签页",
    "关闭全部浏览器标签页",
    "关闭每个浏览器标签页",
    "关掉所有标签页",
    "关掉全部标签页",
];

const BROWSER_TERMINAL_SESSION_CLOSURE_COMMANDS: &[&str] = &[
    "close browser",
    "close the browser",
    "close this browser",
    "close browser session",
    "close the browser session",
    "close this browser session",
    "close browser window",
    "close the browser window",
    "quit browser",
    "quit the browser",
    "exit browser",
    "exit the browser",
    "关闭浏览器",
    "关闭这个浏览器",
    "关闭浏览器会话",
    "关闭这个浏览器会话",
    "关闭浏览器窗口",
    "关掉浏览器",
    "关掉这个浏览器",
    "退出浏览器",
];

const BROWSER_TERMINAL_GENERIC_CLOSURE_COMMANDS: &[&str] = &[
    "close",
    "close it",
    "close them",
    "close anything",
    "close anything else",
    "关闭",
    "关闭它",
    "关闭它们",
    "关掉",
    "关掉它",
    "关掉它们",
];

const NAVIGATION_INTENT_TERMS: &[&str] = &[
    "open",
    "visit",
    "navigate",
    "go to",
    "browse to",
    "launch",
    "打开",
    "访问",
    "前往",
    "转到",
    "导航到",
];

const WEB_NAVIGATION_TARGET_TERMS: &[&str] = &[
    "url", "website", "web site", "webpage", "web page", "link", "网址", "网站", "网页", "链接",
];

const LOCAL_PATH_HANDOFF_INTENT_TERMS: &[&str] = &[
    "open",
    "reveal",
    "show in explorer",
    "show in finder",
    "locate",
    "打开",
    "显示",
    "定位",
];

const LOCAL_FILE_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "jsonl", "yaml", "yml", "toml", "xml", "csv", "tsv", "log", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "rtf", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs",
    "py", "go", "java", "kt", "kts", "c", "h", "cpp", "hpp", "cs", "swift", "sh", "ps1", "bat",
    "cmd", "html", "htm", "css", "scss", "sql", "db", "sqlite", "png", "jpg", "jpeg", "gif",
    "webp", "svg", "mp3", "wav", "m4a", "mp4", "mov", "avi", "zip", "tar", "gz", "7z", "rar",
];

const DESKTOP_TERMS: &[&str] = &[
    "desktop",
    "computer use",
    "window",
    "screenshot",
    "mouse",
    "keyboard",
    "桌面",
    "电脑操作",
    "窗口",
    "截图",
    "鼠标",
    "键盘",
];

const NATIVE_DESKTOP_APP_TERMS: &[&str] = &[
    "excel",
    "microsoft word",
    "powerpoint",
    "outlook",
    "teams",
    "slack",
    "discord",
    "wechat",
    "notepad",
    "calculator",
    "file explorer",
    "微信",
    "企业微信",
    "钉钉",
    "飞书",
    "记事本",
    "计算器",
    "文件资源管理器",
];

const DESKTOP_INTERACTION_TERMS: &[&str] = &[
    "click",
    "drag",
    "drop",
    "type into",
    "keyboard",
    "mouse",
    "scroll",
    "press",
    "select",
    "点击",
    "拖拽",
    "拖动",
    "输入",
    "键盘",
    "鼠标",
    "滚动",
    "按下",
    "选择",
];

const DESKTOP_APP_ACTIVATION_TERMS: &[&str] =
    &["open", "focus", "launch", "start", "打开", "聚焦", "启动"];

const DESKTOP_OPERATION_INTENT_TERMS: &[&str] = &[
    "take a screenshot",
    "capture",
    "observe",
    "inspect",
    "open",
    "focus",
    "launch",
    "start",
    "click",
    "drag",
    "type into",
    "scroll",
    "press",
    "select",
    "computer use",
    "截图一下",
    "截取",
    "捕获",
    "观察",
    "检查",
    "打开",
    "聚焦",
    "启动",
    "点击",
    "拖拽",
    "输入",
    "滚动",
    "按下",
    "选择",
    "电脑操作",
];

const DOCUMENT_ANALYSIS_TERMS: &[&str] = &[
    "compare",
    "document",
    "image",
    "screenshot",
    "ocr",
    "summarize",
    "summary",
    "analyze",
    "analysis",
    "review",
    "verify",
    "verification",
    "critique",
    "evidence",
    "citation",
    "statistics",
    "stats",
    "info",
    "vision",
    "分析",
    "总结",
    "审查",
    "评审",
    "验证",
    "引用",
    "文档",
    "图片",
    "图像",
    "截图",
    "ocr",
    "文字识别",
    "识别图片",
    "比较",
    "统计",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codebase_policy_selects_route_categories_and_log() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "debug the agent routing bug and run cargo test",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::DocumentAnalysis));
        assert!(decision.log.iter().any(|entry| matches!(
            entry.effect,
            ToolVisibilityEffect::SelectedRoute {
                route: ToolVisibilityRouteKind::CodebaseOperation
            }
        )));
    }

    #[test]
    fn terminal_inspection_activates_terminal_and_process_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "看看 terminal 里面刚才的报错",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::Terminal));
        assert!(decision.active_categories.contains(&ToolCategory::Process));
        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
    }

    #[test]
    fn browser_inspection_activates_read_and_interactive_browser_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "用 browser 检查一下这个页面",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserRead));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert_eq!(decision.route, ToolVisibilityRouteKind::WebLookup);
    }

    #[test]
    fn browser_tasks_do_not_activate_retired_desktop_handoff_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Open this website in my browser and click Sign in",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert!(!decision
            .active_categories
            .contains(&ToolCategory::Automation));
    }

    #[test]
    fn explicit_web_navigation_requests_activate_browser_session_capabilities() {
        for query in [
            "Open https://example.com",
            "Open this website",
            "Go to https://example.com",
        ] {
            let decision = decide_tool_visibility(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(decision.active_categories.contains(&ToolCategory::Web));
            assert!(decision
                .active_categories
                .contains(&ToolCategory::BrowserRead));
            assert!(decision
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
            assert!(!decision
                .active_categories
                .contains(&ToolCategory::Automation));
        }
    }

    #[test]
    fn concrete_local_paths_activate_desktop_automation_capability() {
        for query in [
            "Open notes.txt",
            "Reveal /source/report.pdf",
            "Open link.txt",
        ] {
            let decision = decide_tool_visibility(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(decision
                .active_categories
                .contains(&ToolCategory::Automation));
            assert!(!decision
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
        }
    }

    #[test]
    fn local_path_handoffs_activate_desktop_automation_capability() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Reveal this file in Explorer",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision
            .active_categories
            .contains(&ToolCategory::Automation));
        assert!(!decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
    }

    #[test]
    fn local_dev_server_activates_process_and_interactive_browser_capabilities() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "启动 dev server 并检查 localhost 页面",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::Process));
        assert!(decision
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
    }

    #[test]
    fn direct_response_keeps_mutation_categories_hidden() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Tell me a quick joke.",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::DirectResponse);
        assert_eq!(decision.active_categories, vec![ToolCategory::Core]);
    }

    #[test]
    fn collection_context_is_structured_signal_not_persona_text() {
        let persona = decide_tool_visibility(ToolVisibilityInput {
            query: "Say hello.",
            system_prompt: "## Active Persona\nPrefer saved evidence when it exists.",
            has_sources: false,
        });
        let collection = decide_tool_visibility(ToolVisibilityInput {
            query: "Say hello.",
            system_prompt: "## Collection Context\nSaved evidence: chunk-a",
            has_sources: false,
        });

        assert_eq!(persona.route, ToolVisibilityRouteKind::DirectResponse);
        assert_eq!(collection.route, ToolVisibilityRouteKind::CollectionFocused);
        assert!(collection
            .signals
            .iter()
            .any(|signal| { signal.kind == ToolVisibilitySignalKind::CollectionContext }));
    }

    #[test]
    fn chinese_agent_hit_rate_query_selects_codebase_route() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "使用 DeepSeek 的情况下命中效率只有 85%，参考 Claude 顶级 agents 看看架构设计和代码细节。",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::CodebaseOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
        assert!(decision.signals.iter().any(|signal| {
            signal.kind == ToolVisibilitySignalKind::CodeOrToolOperation
                && signal
                    .matched_terms
                    .iter()
                    .any(|term| term == "deepseek" || term == "命中效率")
        }));
    }

    #[test]
    fn workflow_signal_activates_subagent_category() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "Plan a complex workflow for this research task.",
            system_prompt: "",
            has_sources: false,
        });

        assert!(decision.active_categories.contains(&ToolCategory::SubAgent));
    }

    #[test]
    fn chinese_write_or_modify_query_selects_filesystem_tools() {
        let decision = decide_tool_visibility(ToolVisibilityInput {
            query: "帮我修改这个文件并保存。",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(decision.route, ToolVisibilityRouteKind::FileOperation);
        assert!(decision
            .active_categories
            .contains(&ToolCategory::FileSystem));
    }

    #[test]
    fn implicit_html_canvas_authoring_requires_process_and_visual_browser_observation() {
        let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
            query: "帮我用html写一个黑洞演示图",
            system_prompt: "",
            has_sources: false,
        });

        assert_eq!(
            requirements.route,
            ToolVisibilityRouteKind::CodebaseOperation
        );
        for category in [
            ToolCategory::Process,
            ToolCategory::BrowserRead,
            ToolCategory::VisualObservation,
        ] {
            assert!(
                requirements.active_categories.contains(&category),
                "missing required capability {category:?}"
            );
        }
        assert!(!requirements
            .active_categories
            .contains(&ToolCategory::BrowserInteract));
        assert!(requirements.requires_visual_observation_after_mutation());
    }

    #[test]
    fn explicit_browser_navigation_and_interaction_require_fresh_visual_observation() {
        for query in [
            "Open the browser, visit https://example.com, and click More information",
            "打开浏览器访问 example.com 并点击 More information",
        ] {
            let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(requirements.interaction.browser_observation);
            assert!(requirements.interaction.browser_interaction);
            assert!(requirements.requires_visual_observation_after_mutation());
            assert!(requirements
                .active_categories
                .contains(&ToolCategory::BrowserRead));
            assert!(requirements
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
        }
    }

    #[test]
    fn explicit_browser_closure_allows_only_typed_terminal_postcondition_evidence() {
        assert_eq!(
            browser_terminal_closure_clauses("关闭浏览器后打开文件"),
            vec!["关闭浏览器", "打开文件"],
            "a controlled Chinese follow-up command must form two clauses"
        );
        assert_eq!(
            browser_terminal_closure_requirement("关闭浏览器后打开文件"),
            BrowserTerminalClosureRequirement::Session
        );
        for (query, expected_closure) in [
            ("Close tab", BrowserTerminalClosureRequirement::Tab),
            ("Close browser tab", BrowserTerminalClosureRequirement::Tab),
            ("Close all tabs", BrowserTerminalClosureRequirement::AllTabs),
            (
                "Close every browser tab",
                BrowserTerminalClosureRequirement::AllTabs,
            ),
            (
                "Close the current browser tab",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "Close the browser session",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Could you please close this tab?",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "Open example.com, then close the tab",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "After checking it, close the browser session",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Explain what you found, then close the tab",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "Close the tab, then close the browser session",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Close the browser session, then close the tab",
                BrowserTerminalClosureRequirement::Tab,
            ),
            ("关闭当前标签页", BrowserTerminalClosureRequirement::Tab),
            ("关闭浏览器标签页", BrowserTerminalClosureRequirement::Tab),
            ("关闭所有标签页", BrowserTerminalClosureRequirement::AllTabs),
            (
                "关闭全部浏览器标签页",
                BrowserTerminalClosureRequirement::AllTabs,
            ),
            (
                "请你关闭浏览器吧",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "检查完后关闭当前标签页",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "告诉我结果，然后关闭标签页",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "关闭这个浏览器会话",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭浏览器后告诉我结果",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭浏览器后，请你告诉我结果",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭浏览器后打开文件",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭浏览器并打开另一个应用",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭标签页后保存结果",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "Close the browser, then tell me what happened?",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "关闭浏览器后告诉我发生了什么？",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Could you inspect it, then close the browser?",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Can you tell me what happened, then close the browser?",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "请告诉我结果，然后关闭浏览器？",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "Close the browser after checking it",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "After you close the browser, tell me the result",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "When done, close the browser",
                BrowserTerminalClosureRequirement::Session,
            ),
            (
                "When finished, close the current tab",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "Once you close the current tab, tell me what happened?",
                BrowserTerminalClosureRequirement::Tab,
            ),
            (
                "关闭浏览器以后打开文件",
                BrowserTerminalClosureRequirement::Session,
            ),
        ] {
            let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(
                requirements
                    .active_categories
                    .contains(&ToolCategory::BrowserRead),
                "{query}"
            );
            assert!(
                requirements
                    .active_categories
                    .contains(&ToolCategory::BrowserInteract),
                "{query}"
            );
            assert!(requirements.interaction.browser_observation, "{query}");
            assert!(requirements.interaction.browser_interaction, "{query}");
            assert_eq!(
                requirements.interaction.browser_terminal_closure, expected_closure,
                "{query}"
            );
        }

        let ordinary_interaction = resolve_turn_capability_requirements(ToolVisibilityInput {
            query: "Open the browser and click More information",
            system_prompt: "",
            has_sources: false,
        });
        assert!(!ordinary_interaction
            .interaction
            .browser_terminal_closure
            .is_required());

        for query in [
            "How do I close a browser?",
            "What happens when I close browser?",
            "Do not close the browser",
            "Never close browser tab",
            "Should I close browser?",
            "Is it safe to close this tab?",
            "We need a policy for close browser session",
            "I already closed browser",
            "disclose tab details",
            "close table rows",
            "如何关闭浏览器？",
            "告诉我是否应该关闭浏览器",
            "是否关闭浏览器更好？",
            "不要关闭浏览器",
            "Close the browser, but don't actually close it",
            "Close the tab, then do not close it",
            "关闭浏览器，但不要真的关闭",
            "close browser settings panel",
            "close browser: settings panel",
            "close browser sidebar",
            "close tab groups",
            "关闭浏览器设置面板",
            "关闭标签页分组",
            "关闭浏览器后台进程",
            "关闭浏览器后端服务",
            "关闭标签页并发任务",
            "关闭浏览器后启动时间是多少？",
            "关闭浏览器后运行时是否释放？",
            "关闭浏览器后保存率会变化吗？",
            "Close the browser, then tell me whether I should close it",
            "关闭浏览器后告诉我是否应该关闭它",
            "请问，关闭浏览器，是否安全？",
            "请告诉我，关闭浏览器，会发生什么？",
            "Does that mean, close the browser?",
            "这是否意味着，关闭浏览器？",
            "你的意思是，关闭浏览器？",
            "Could you check whether that means, close the browser?",
            "请检查这是否意味着，关闭浏览器？",
            "Could you check if I should close the browser and close the tab?",
            "请检查我是否应该关闭浏览器并关闭标签页？",
            "Could you check how to close the browser and close the tab?",
            "请检查如何关闭浏览器并关闭标签页？",
            "Can you tell me whether I should close it, then close the browser?",
            "请告诉我是否应该关闭它，然后关闭浏览器？",
            "Please document how to close the browser and close the tab.",
            "The guide says to close the browser and close the tab.",
            "请记录如何关闭浏览器并关闭标签页。",
            "指南写着关闭浏览器并关闭标签页。",
            "Read the instruction: close the browser and close the tab.",
            "查看这条说明：关闭浏览器并关闭标签页。",
            "After what happens, close the browser?",
            "Summarize this sequence: open the page, then close the browser.",
            "Read this: first close the browser, then close the tab.",
            "After you close the browser?",
            "After you close the browser, should I reopen it?",
            "After I close the browser, tell me the result",
            "After you explain how to close the browser, tell me the result",
            "总结以下步骤：打开页面，然后关闭浏览器。",
            "读取这段文字：先关闭浏览器，然后关闭标签页。",
        ] {
            let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });
            assert!(
                !requirements
                    .interaction
                    .browser_terminal_closure
                    .is_required(),
                "knowledge or negated close text must not authorize terminal evidence: {query}"
            );
        }

        let scoped_preservation = resolve_turn_capability_requirements(ToolVisibilityInput {
            query: "Close the current tab, but do not close the browser",
            system_prompt: "",
            has_sources: false,
        });
        assert_eq!(
            scoped_preservation.interaction.browser_terminal_closure,
            BrowserTerminalClosureRequirement::Tab,
            "preserving the wider session must not cancel an explicitly requested tab close"
        );
    }

    #[test]
    fn legacy_interaction_requirements_default_terminal_closure_to_false() {
        let legacy = serde_json::json!({
            "visualObservation": "afterLastMutation",
            "browserObservation": true,
            "browserInteraction": true,
            "desktopObservation": "notRequired",
            "desktopInteraction": false
        });

        let requirements: TurnInteractionRequirements = serde_json::from_value(legacy)
            .expect("legacy interaction requirements must deserialize");

        assert_eq!(
            requirements.browser_terminal_closure,
            BrowserTerminalClosureRequirement::NotRequired
        );
        assert!(requirements.browser_observation);
        assert!(requirements.browser_interaction);
    }

    #[test]
    fn natural_native_app_commands_activate_desktop_observe_control_observe() {
        for query in [
            "帮我在微信里点击发送按钮",
            "请在 Excel 里把 A1 输入为 42",
            "Open Calculator",
            "Open Excel",
            "Focus Microsoft Word",
            "打开计算器",
            "打开 Excel",
            "聚焦微信",
        ] {
            let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert_eq!(
                requirements.route,
                ToolVisibilityRouteKind::InteractionOperation,
                "{query}"
            );
            assert!(requirements.interaction.desktop_interaction, "{query}");
            assert!(
                requirements.interaction.requires_desktop_observation(),
                "{query}"
            );
            assert!(
                requirements
                    .active_categories
                    .contains(&ToolCategory::DesktopInteract),
                "{query}"
            );
            assert!(!requirements.interaction.browser_observation, "{query}");
        }
    }

    #[test]
    fn browser_knowledge_question_does_not_create_an_interaction_gate() {
        for query in [
            "What is a browser?",
            "什么是浏览器？",
            "What is a model context window?",
            "What is Microsoft Excel?",
            "计算器是什么？",
        ] {
            let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
                query,
                system_prompt: "",
                has_sources: false,
            });

            assert!(!requirements.requires_completion_gate());
            assert!(!requirements.interaction.browser_observation);
            assert!(!requirements
                .active_categories
                .contains(&ToolCategory::BrowserInteract));
        }
    }
}
