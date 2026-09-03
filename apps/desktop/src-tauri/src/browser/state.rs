use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub use nexa_core::browser_runtime::{
    BrowserBounds, BrowserControlOwner, BrowserElement, BrowserElementBounds, ControlLease,
};
use nexa_core::browser_runtime::{
    BrowserObservation as CoreBrowserObservation, BrowserScreenshot,
    BrowserSession as CoreBrowserSession, BrowserTab as CoreBrowserTab,
};
use nexa_core::tools::run_shell_tool::{managed_loopback_permits, ManagedLoopbackPermit};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, Webview};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use super::network_proxy::BrowserNetworkProxy;
use super::policy::{
    classify_agent_action, form_navigation_approval_key, managed_permit_matches_url,
    normalize_browser_url, normalize_browser_url_candidate, validate_agent_network_url_with_permit,
    BrowserActionRisk, NavigationActor,
};
use super::scripts::OBSERVE_EXPRESSION;
use super::webview_host::{
    capture_webview_image, create_child_webview, dispatch_eval_json, dispatch_trusted_key,
    dispatch_trusted_pointer_click, eval_json, insert_trusted_text, trusted_key_input_match,
    BrowserCapturePlan, BrowserChildWebview, BrowserSurfaceFlight, BrowserSurfaceGate,
    BrowserTrustedInputGuard, PendingEvalJson, TrustedInputEventBudget, TrustedInputMatch,
};

pub const BROWSER_EVENT: &str = "browser:event";
const MAX_OBSERVATIONS: usize = 64;
const MAX_BROWSER_TABS_PER_SESSION: usize = 16;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserPageSnapshot {
    url: String,
    title: String,
    text: String,
    viewport: serde_json::Value,
    history_length: usize,
    user_epoch: u64,
    dom_fingerprint: String,
    elements: Vec<BrowserElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserHistoryTarget {
    key: String,
    url: String,
}

#[derive(Clone, Copy)]
pub(super) enum BrowserHistoryDirection {
    Back,
    Forward,
}

impl BrowserHistoryDirection {
    fn offset(self) -> i8 {
        match self {
            Self::Back => -1,
            Self::Forward => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Forward => "forward",
        }
    }
}

pub type BrowserObservationPayload = CoreBrowserObservation;

pub struct BrowserActOutcome {
    pub observation: BrowserObservationPayload,
    pub effect_observed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserActFailurePhase {
    PreCommit,
    EffectMayHaveOccurred,
}

#[derive(Debug)]
pub struct BrowserActFailure {
    pub phase: BrowserActFailurePhase,
    pub observation_consumed: bool,
}

impl BrowserActFailure {
    pub fn effect_may_have_occurred(&self) -> bool {
        self.phase == BrowserActFailurePhase::EffectMayHaveOccurred
    }
}

#[derive(Debug, Default)]
struct BrowserActCommitState {
    committed: AtomicBool,
    observation_consumed: AtomicBool,
}

#[derive(Debug, Clone, Default)]
pub struct BrowserActCommitTracker(Arc<BrowserActCommitState>);

impl BrowserActCommitTracker {
    pub(super) fn mark_observation_consumed(&self) {
        self.0.observation_consumed.store(true, Ordering::Release);
    }

    pub(super) fn mark_committed(&self) {
        self.0.committed.store(true, Ordering::Release);
    }

    pub(super) fn effect_may_have_occurred(&self) -> bool {
        self.0.committed.load(Ordering::Acquire)
    }

    pub(super) fn observation_consumed(&self) -> bool {
        self.0.observation_consumed.load(Ordering::Acquire)
    }

    pub fn failure(&self, _message: String) -> BrowserActFailure {
        BrowserActFailure {
            phase: if self.effect_may_have_occurred() {
                BrowserActFailurePhase::EffectMayHaveOccurred
            } else {
                BrowserActFailurePhase::PreCommit
            },
            observation_consumed: self.observation_consumed(),
        }
    }
}

#[derive(Clone)]
struct StoredObservation {
    created_at: Instant,
    tab_id: String,
    url: String,
    dom_fingerprint: String,
    user_epoch: u64,
    lease_generation: u64,
    elements: Vec<BrowserElement>,
    claimed_for_action: bool,
}

/// Page signature captured after preparation side effects and before the
/// agent's committed input. Settle must never compare against the older
/// claimed observation when preparation can scroll or focus the page.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionVerificationBaseline {
    url: String,
    dom_fingerprint: String,
    user_epoch: u64,
}

impl StoredObservation {
    fn verification_baseline(&self) -> ActionVerificationBaseline {
        ActionVerificationBaseline {
            url: self.url.clone(),
            dom_fingerprint: self.dom_fingerprint.clone(),
            user_epoch: self.user_epoch,
        }
    }
}

pub type BrowserTabInfo = CoreBrowserTab;
pub type BrowserSessionInfo = CoreBrowserSession;

struct BrowserTab {
    id: String,
    webview: Webview,
    url: String,
    title: String,
    loading: bool,
    status: String,
    bounds: BrowserBounds,
    approved_agent_urls: Arc<Mutex<HashSet<String>>>,
    agent_restricted: Arc<AtomicBool>,
    network_proxy: Arc<BrowserNetworkProxy>,
    trusted_input_guard: BrowserTrustedInputGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserSessionPhase {
    Active,
    Closing,
    CleanupPending,
}

impl BrowserSessionPhase {
    pub(super) fn accepts_new_tabs(self) -> bool {
        self == Self::Active
    }

    pub(super) fn begin_close(self, opening_tabs: usize) -> Result<Self, String> {
        if opening_tabs != 0 {
            return Err(
                "A browser tab is still opening; wait for it to finish before closing the session"
                    .to_string(),
            );
        }
        Ok(Self::Closing)
    }
}

struct BrowserSession {
    id: String,
    conversation_id: Option<String>,
    profile_id: String,
    temporary_profile: bool,
    active_tab_id: Option<String>,
    tabs: HashMap<String, BrowserTab>,
    control_lease: ControlLease,
    observations: HashMap<String, StoredObservation>,
    initializing: bool,
    workspace_visible: bool,
    visibility_revision: u64,
    visibility_requested: bool,
    visibility_request_revision: Option<u64>,
    opening_tabs: usize,
    phase: BrowserSessionPhase,
    surface_gate: BrowserSurfaceGate,
}

struct BrowserRuntimeState {
    sessions: HashMap<String, BrowserSession>,
}

#[derive(Clone)]
pub struct BrowserState {
    app: AppHandle,
    profile_root: Arc<PathBuf>,
    inner: Arc<Mutex<BrowserRuntimeState>>,
    creation_locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
}

struct AgentNavigationPermitGuard {
    state: BrowserState,
    session_id: String,
    tab_id: String,
    url: Url,
    form_navigation: bool,
}

impl Drop for AgentNavigationPermitGuard {
    fn drop(&mut self) {
        self.state.revoke_agent_action_url(
            &self.session_id,
            &self.tab_id,
            &self.url,
            self.form_navigation,
        );
    }
}

struct InitializingSessionGuard {
    state: BrowserState,
    session_id: String,
    armed: bool,
}

struct OpeningTabGuard {
    state: BrowserState,
    session_id: String,
    armed: bool,
}

impl OpeningTabGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OpeningTabGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut runtime) = self.state.inner.lock() {
            if let Some(session) = runtime.sessions.get_mut(&self.session_id) {
                session.opening_tabs = session.opening_tabs.saturating_sub(1);
            }
        }
    }
}

impl InitializingSessionGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InitializingSessionGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut runtime) = self.state.inner.lock() {
                runtime.sessions.remove(&self.session_id);
            }
        }
    }
}

impl BrowserState {
    pub fn new(app: AppHandle, profile_root: PathBuf) -> Self {
        Self {
            app,
            profile_root: Arc::new(profile_root),
            inner: Arc::new(Mutex::new(BrowserRuntimeState {
                sessions: HashMap::new(),
            })),
            creation_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn emit(&self, kind: &str, payload: serde_json::Value) {
        let _ = self.app.emit(
            BROWSER_EVENT,
            serde_json::json!({ "kind": kind, "payload": payload }),
        );
    }

    fn require_visible_focused_host_window(&self) -> Result<(), String> {
        let window = self
            .app
            .get_window("main")
            .ok_or_else(|| "Main application window is unavailable".to_string())?;
        let visible = window
            .is_visible()
            .map_err(|error| format!("Could not read main window visibility: {error}"))?;
        let minimized = window
            .is_minimized()
            .map_err(|error| format!("Could not read main window state: {error}"))?;
        let focused = window
            .is_focused()
            .map_err(|error| format!("Could not read main window focus: {error}"))?;
        if !browser_host_window_allows_agent_action(visible, minimized, focused) {
            return Err(
                "Browser action requires the Nexa window to be visible, restored, and focused"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn update_page_load(&self, session_id: &str, tab_id: &str, url: &Url, loading: bool) {
        if let Ok(mut runtime) = self.inner.lock() {
            if let Some(session) = runtime.sessions.get_mut(session_id) {
                if loading {
                    session.observations.clear();
                    session.control_lease.invalidate();
                }
                if let Some(tab) = session.tabs.get_mut(tab_id) {
                    if loading {
                        tab.network_proxy.retain_agent_loopback_permit_for_url(url);
                    }
                    tab.url = url.to_string();
                    tab.loading = loading;
                    tab.status = if loading { "loading" } else { "idle" }.to_string();
                }
            }
        }
        self.emit(
            if loading {
                "pageLoadStarted"
            } else {
                "pageLoadFinished"
            },
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id, "url": url }),
        );
    }

    pub fn handle_document_title(&self, session_id: &str, tab_id: &str, title: String) {
        if let Ok(mut runtime) = self.inner.lock() {
            if let Some(tab) = runtime
                .sessions
                .get_mut(session_id)
                .and_then(|session| session.tabs.get_mut(tab_id))
            {
                tab.title = title.clone();
            }
        }
        self.emit(
            "titleChanged",
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id, "title": title }),
        );
    }

    pub(super) fn record_user_takeover(&self, session_id: &str, tab_id: &str) {
        let (owner, webviews) = if let Ok(mut runtime) = self.inner.lock() {
            let Some(session) = runtime.sessions.get_mut(session_id) else {
                return;
            };
            if matches!(session.control_lease.owner(), BrowserControlOwner::User) {
                return;
            }
            session.control_lease.acquire(BrowserControlOwner::User);
            session.observations.clear();
            for tab in session.tabs.values() {
                tab.network_proxy.revoke_agent_network_access();
                tab.network_proxy.set_agent_restricted(false);
                if let Ok(mut approved) = tab.approved_agent_urls.lock() {
                    approved.clear();
                }
            }
            (
                session.control_lease.owner().clone(),
                session
                    .tabs
                    .values()
                    .map(|tab| tab.webview.clone())
                    .collect::<Vec<_>>(),
            )
        } else {
            return;
        };
        invalidate_for_user_takeover(&webviews);
        self.emit(
            "controlChanged",
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id, "owner": owner, "reason": "directUserInput" }),
        );
    }

    pub fn session_info(&self, session_id: &str) -> Result<BrowserSessionInfo, String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        Ok(session_info(session))
    }

    pub fn list_sessions(&self) -> Result<Vec<BrowserSessionInfo>, String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        Ok(runtime
            .sessions
            .values()
            .filter(|session| !session.initializing)
            .map(session_info)
            .collect())
    }

    pub fn active_session(
        &self,
        conversation_id: &str,
    ) -> Result<Option<BrowserSessionInfo>, String> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .find(|session| session.conversation_id.as_deref() == Some(conversation_id)))
    }

    pub async fn create_session(
        &self,
        conversation_id: Option<String>,
        profile_id: Option<String>,
        initial_url: Option<&str>,
        open_initial_url_on_reuse: bool,
        actor: NavigationActor,
        bounds: Option<BrowserBounds>,
    ) -> Result<BrowserSessionInfo, String> {
        let creation_lock = conversation_id
            .as_deref()
            .map(|conversation_id| self.conversation_creation_lock(conversation_id))
            .transpose()?;
        let _creation_guard = if let Some(lock) = creation_lock.as_ref() {
            Some(lock.lock().await)
        } else {
            None
        };
        if let Some(conversation_id) = conversation_id.as_deref() {
            if let Some(existing) = self.active_session(conversation_id)? {
                let accepts_new_tabs = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?
                    .sessions
                    .get(&existing.id)
                    .is_some_and(|session| session.phase.accepts_new_tabs());
                if !accepts_new_tabs {
                    return Err(
                        "Browser session is closing or awaiting cleanup; retry close_session before creating or opening another tab"
                            .to_string(),
                    );
                }
                if open_initial_url_on_reuse {
                    if let Some(initial_url) = initial_url {
                        self.open_tab(&existing.id, initial_url, actor, bounds)
                            .await?;
                    }
                }
                self.emit(
                    "sessionCreated",
                    serde_json::json!({
                        "sessionId": &existing.id,
                        "conversationId": conversation_id,
                        "requestVisible": actor == NavigationActor::Agent,
                    }),
                );
                return self.session_info(&existing.id);
            }
        }
        let event_conversation_id = conversation_id.clone();
        let session_id = format!("browser_{}", uuid::Uuid::new_v4().simple());
        let temporary_profile = profile_id.is_none();
        let profile_id = profile_id
            .as_deref()
            .map(safe_identifier)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("temporary-{session_id}"));
        {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            if runtime
                .sessions
                .values()
                .any(|session| session.profile_id == profile_id)
            {
                return Err(format!(
                    "Browser profile '{profile_id}' is already active in another workspace"
                ));
            }
            runtime.sessions.insert(
                session_id.clone(),
                BrowserSession {
                    id: session_id.clone(),
                    conversation_id,
                    profile_id,
                    temporary_profile,
                    active_tab_id: None,
                    tabs: HashMap::new(),
                    control_lease: ControlLease::default(),
                    observations: HashMap::new(),
                    initializing: true,
                    workspace_visible: bounds.is_some(),
                    visibility_revision: 0,
                    visibility_requested: false,
                    visibility_request_revision: None,
                    opening_tabs: 0,
                    phase: BrowserSessionPhase::Active,
                    surface_gate: BrowserSurfaceGate::default(),
                },
            );
        }
        let mut initialization_guard = InitializingSessionGuard {
            state: self.clone(),
            session_id: session_id.clone(),
            armed: true,
        };
        let target = initial_url.unwrap_or("about:blank");
        if let Err(error) = self.open_tab(&session_id, target, actor, bounds).await {
            if let Ok(mut runtime) = self.inner.lock() {
                runtime.sessions.remove(&session_id);
            }
            return Err(error);
        }
        {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            session.initializing = false;
        }
        initialization_guard.disarm();
        self.emit(
            "sessionCreated",
            serde_json::json!({
                "sessionId": session_id,
                "conversationId": event_conversation_id,
                "requestVisible": actor == NavigationActor::Agent,
            }),
        );
        self.session_info(&session_id)
    }

    pub async fn open_tab(
        &self,
        session_id: &str,
        input: &str,
        actor: NavigationActor,
        bounds: Option<BrowserBounds>,
    ) -> Result<BrowserTabInfo, String> {
        let url = if actor == NavigationActor::Agent {
            normalize_browser_url_candidate(input)?
        } else {
            normalize_browser_url(input, actor)?
        };
        {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if !session.phase.accepts_new_tabs() {
                return Err(
                    "Browser session is closing; retry close_session before opening another tab"
                        .to_string(),
                );
            }
        }
        if actor != NavigationActor::Agent {
            self.acquire_control(session_id, BrowserControlOwner::User)?;
        }
        let (profile_id, tab_id, conversation_id, effective_bounds, agent_open_fence) = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if !session.phase.accepts_new_tabs() {
                return Err(
                    "Browser session is closing; retry close_session before opening another tab"
                        .to_string(),
                );
            }
            if actor == NavigationActor::Agent
                && matches!(session.control_lease.owner(), BrowserControlOwner::User)
            {
                return Err(
                    "Browser control belongs to the user; wait until they hand it back".to_string(),
                );
            }
            let agent_open_fence = if actor == NavigationActor::Agent && !session.initializing {
                require_agent_tab_surface(
                    session,
                    session.active_tab_id.as_deref().ok_or_else(|| {
                        "Browser Workspace has no active tab to anchor a new tab".to_string()
                    })?,
                )?;
                let BrowserControlOwner::Agent { call_id } = session.control_lease.owner() else {
                    return Err(
                        "Browser control changed before the Agent could open a tab".to_string()
                    );
                };
                Some((
                    session.visibility_revision,
                    session.control_lease.generation(),
                    call_id.clone(),
                    session.active_tab_id.clone(),
                ))
            } else {
                None
            };
            if !browser_tab_open_allowed(
                session.tabs.len(),
                session.opening_tabs,
                session.initializing,
                session.workspace_visible,
            ) {
                return Err(
                    if session.tabs.len() + session.opening_tabs >= MAX_BROWSER_TABS_PER_SESSION {
                        format!(
                        "Browser session reached the authoritative {MAX_BROWSER_TABS_PER_SESSION}-tab limit"
                    )
                    } else {
                        "Browser Workspace must be visible before opening another tab or popup"
                            .to_string()
                    },
                );
            }
            session.opening_tabs = session.opening_tabs.saturating_add(1);
            let inherited_bounds = if bounds.is_none() && session.workspace_visible {
                session
                    .active_tab_id
                    .as_deref()
                    .and_then(|tab_id| session.tabs.get(tab_id))
                    .map(|tab| tab.bounds)
            } else {
                None
            };
            (
                session.profile_id.clone(),
                format!("tab_{}", uuid::Uuid::new_v4().simple()),
                session.conversation_id.clone(),
                bounds.or(inherited_bounds),
                agent_open_fence,
            )
        };
        let mut opening_guard = OpeningTabGuard {
            state: self.clone(),
            session_id: session_id.to_string(),
            armed: true,
        };
        let agent_restricted = Arc::new(AtomicBool::new(actor == NavigationActor::Agent));
        let network_proxy = Arc::new(BrowserNetworkProxy::start(Arc::clone(&agent_restricted))?);
        if actor == NavigationActor::Agent && matches!(url.scheme(), "http" | "https") {
            Self::prepare_proxy_network_access(conversation_id.as_deref(), &network_proxy, &url)
                .await?;
        }
        let network_proxy_url = network_proxy.url().clone();
        let profile_dir = self.profile_root.join(&profile_id);
        std::fs::create_dir_all(&profile_dir)
            .map_err(|error| format!("Could not create browser profile: {error}"))?;
        let BrowserChildWebview {
            webview,
            approved_agent_urls,
            trusted_input_guard,
        } = create_child_webview(
            self,
            session_id,
            &tab_id,
            url.clone(),
            profile_dir,
            &profile_id,
            Arc::clone(&agent_restricted),
            network_proxy_url,
            effective_bounds,
        )?;
        let initial_bounds = effective_bounds
            .unwrap_or(BrowserBounds {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            })
            .sanitized();
        let info = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            {
                let session = runtime
                    .sessions
                    .get_mut(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                session.opening_tabs = session.opening_tabs.saturating_sub(1);
                opening_guard.disarm();
                if let Some((
                    expected_visibility_revision,
                    expected_generation,
                    expected_call_id,
                    expected_active_tab_id,
                )) = agent_open_fence.as_ref()
                {
                    let fence_is_current = session.visibility_revision
                        == *expected_visibility_revision
                        && session.control_lease.generation() == *expected_generation
                        && session.active_tab_id.as_deref() == expected_active_tab_id.as_deref()
                        && matches!(
                            session.control_lease.owner(),
                            BrowserControlOwner::Agent { call_id } if call_id == expected_call_id
                        )
                        && expected_active_tab_id
                            .as_deref()
                            .is_some_and(|active_tab_id| {
                                require_agent_tab_surface(session, active_tab_id).is_ok()
                            });
                    if !fence_is_current {
                        drop(runtime);
                        network_proxy.shutdown();
                        let _ = webview.close();
                        return Err(
                            "Browser Workspace changed while the Agent was opening the tab; retry while it is visible"
                                .to_string(),
                        );
                    }
                }
                if actor == NavigationActor::Agent {
                    if matches!(session.control_lease.owner(), BrowserControlOwner::User) {
                        drop(runtime);
                        network_proxy.shutdown();
                        let _ = webview.close();
                        return Err(
                            "Browser control belongs to the user; wait until they hand it back"
                                .to_string(),
                        );
                    }
                    if matches!(session.control_lease.owner(), BrowserControlOwner::None) {
                        session.control_lease.acquire(BrowserControlOwner::Agent {
                            call_id: "open_tab".to_string(),
                        });
                    }
                }
            }
            if effective_bounds.is_some() {
                for (other_session_id, other) in &mut runtime.sessions {
                    if other_session_id == session_id || !other.workspace_visible {
                        continue;
                    }
                    other.workspace_visible = false;
                    other.observations.clear();
                    other.control_lease.invalidate();
                    for tab in other.tabs.values() {
                        tab.network_proxy.revoke_agent_network_access();
                        let _ = tab.webview.hide();
                    }
                }
            }
            let session = runtime
                .sessions
                .get_mut(session_id)
                .expect("validated browser session must remain present");
            for tab in session.tabs.values() {
                let _ = tab.webview.hide();
                tab.network_proxy.revoke_agent_network_access();
            }
            if effective_bounds.is_some() {
                session.workspace_visible = true;
            }
            session.active_tab_id = Some(tab_id.clone());
            session.tabs.insert(
                tab_id.clone(),
                BrowserTab {
                    id: tab_id.clone(),
                    webview,
                    url: url.to_string(),
                    title: url.host_str().unwrap_or("New tab").to_string(),
                    loading: true,
                    status: "loading".to_string(),
                    bounds: initial_bounds,
                    approved_agent_urls,
                    agent_restricted,
                    network_proxy,
                    trusted_input_guard,
                },
            );
            session_info(session)
                .tabs
                .into_iter()
                .find(|tab| tab.id == tab_id)
                .expect("inserted browser tab must be visible")
        };
        self.emit(
            "tabOpened",
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id }),
        );
        Ok(info)
    }

    pub async fn navigate(
        &self,
        session_id: &str,
        tab_id: &str,
        input: &str,
        actor: NavigationActor,
        commit_tracker: Option<&BrowserActCommitTracker>,
    ) -> Result<BrowserTabInfo, String> {
        if actor == NavigationActor::Agent && commit_tracker.is_none() {
            return Err(
                "Agent browser navigation requires durable commit tracking before dispatch"
                    .to_string(),
            );
        }
        let url = if actor == NavigationActor::Agent {
            normalize_browser_url_candidate(input)?
        } else {
            normalize_browser_url(input, actor)?
        };
        if actor == NavigationActor::Agent {
            {
                let runtime = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?;
                let session = runtime
                    .sessions
                    .get(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                require_agent_tab_surface(session, tab_id)?;
            }
            if matches!(url.scheme(), "http" | "https") {
                self.prepare_agent_network_access(session_id, tab_id, &url)
                    .await?;
            }
        } else {
            self.acquire_control(session_id, BrowserControlOwner::User)?;
        }
        {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if actor == NavigationActor::Agent {
                if matches!(session.control_lease.owner(), BrowserControlOwner::User) {
                    return Err(
                        "Browser control belongs to the user; wait until they hand it back"
                            .to_string(),
                    );
                }
                require_agent_tab_surface(session, tab_id)?;
                if matches!(session.control_lease.owner(), BrowserControlOwner::None) {
                    session.control_lease.acquire(BrowserControlOwner::Agent {
                        call_id: "navigate".to_string(),
                    });
                }
            }
            session.observations.clear();
            let tab = session
                .tabs
                .get_mut(tab_id)
                .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
            tab.network_proxy
                .set_agent_restricted(actor == NavigationActor::Agent);
            if actor != NavigationActor::Agent || !matches!(url.scheme(), "http" | "https") {
                tab.network_proxy.revoke_agent_network_access();
            }
            if actor == NavigationActor::Agent {
                tab.approved_agent_urls
                    .lock()
                    .map_err(|_| "Browser navigation policy is unavailable".to_string())?
                    .insert(url.to_string());
            }
            if let Err(error) = dispatch_browser_navigation(commit_tracker, || {
                tab.webview
                    .navigate(url.clone())
                    .map_err(|error| error.to_string())
            }) {
                if actor == NavigationActor::Agent {
                    if let Ok(mut approved) = tab.approved_agent_urls.lock() {
                        approved.remove(url.as_str());
                    }
                }
                return Err(format!("Browser navigation failed: {error}"));
            }
            tab.url = url.to_string();
            tab.loading = true;
            tab.status = "loading".to_string();
        }
        self.tab_info(session_id, tab_id)
    }

    pub async fn open_popup(
        &self,
        session_id: &str,
        source_tab_id: &str,
        input: &str,
        bounds: Option<BrowserBounds>,
    ) -> Result<BrowserTabInfo, String> {
        let actor = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            let source_tab = session
                .tabs
                .get(source_tab_id)
                .ok_or_else(|| format!("Unknown browser tab '{source_tab_id}'"))?;
            if source_tab.agent_restricted.load(Ordering::Relaxed) {
                NavigationActor::Agent
            } else {
                NavigationActor::User
            }
        };
        self.open_tab(session_id, input, actor, bounds).await
    }

    pub fn tab_info(&self, session_id: &str, tab_id: &str) -> Result<BrowserTabInfo, String> {
        self.session_info(session_id)?
            .tabs
            .into_iter()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))
    }

    pub fn activate_tab(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> Result<BrowserSessionInfo, String> {
        self.activate_tab_checked(session_id, tab_id, None)
    }

    pub fn activate_tab_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
    ) -> Result<BrowserSessionInfo, String> {
        self.activate_tab_checked(session_id, tab_id, Some(call_id))
    }

    fn activate_tab_checked(
        &self,
        session_id: &str,
        tab_id: &str,
        agent_call_id: Option<&str>,
    ) -> Result<BrowserSessionInfo, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if agent_call_id.is_some_and(|call_id| {
            !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        }) {
            return Err(
                "Browser control changed before the Agent could activate the tab".to_string(),
            );
        }
        let target_tab = session
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
        if agent_call_id.is_some()
            && (!session.workspace_visible
                || target_tab.bounds.width < 64.0
                || target_tab.bounds.height < 64.0)
        {
            return Err(
                "Browser Workspace must be visible with valid bounds before the Agent activates a tab"
                    .to_string(),
            );
        }
        if session.active_tab_id.as_deref() != Some(tab_id) {
            session.control_lease.invalidate();
            session.observations.clear();
        }
        for (id, tab) in &session.tabs {
            if id == tab_id && session.workspace_visible {
                tab.webview.show().map_err(|error| error.to_string())?;
                let _ = tab.webview.set_focus();
            } else {
                let _ = tab.webview.hide();
                tab.network_proxy.revoke_agent_network_access();
            }
        }
        session.active_tab_id = Some(tab_id.to_string());
        let info = session_info(session);
        drop(runtime);
        self.emit(
            "tabActivated",
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id }),
        );
        Ok(info)
    }

    pub async fn set_bounds(
        &self,
        session_id: &str,
        bounds: BrowserBounds,
        visible: bool,
        visibility_revision: u64,
    ) -> Result<(), String> {
        let bounds = bounds.sanitized();
        let surface_gate = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?
                .surface_gate
                .clone()
        };
        let _surface_flight = surface_gate.acquire().await?;
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        {
            let session = runtime
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            accept_visibility_revision(&mut session.visibility_revision, visibility_revision)?;
        }
        if visible {
            for (other_session_id, other) in &mut runtime.sessions {
                if other_session_id == session_id || !other.workspace_visible {
                    continue;
                }
                other.workspace_visible = false;
                other.observations.clear();
                other.control_lease.invalidate();
                for tab in other.tabs.values() {
                    tab.network_proxy.revoke_agent_network_access();
                    let _ = tab.webview.hide();
                }
            }
        }
        let session = runtime
            .sessions
            .get_mut(session_id)
            .expect("validated browser session must remain present");
        if visibility_request_is_satisfied(
            session.visibility_request_revision,
            visible,
            visibility_revision,
        ) {
            session.visibility_requested = false;
            session.visibility_request_revision = None;
        }
        session.observations.clear();
        session.control_lease.invalidate();
        session.workspace_visible = visible;
        for (tab_id, tab) in &mut session.tabs {
            tab.bounds = bounds;
            tab.webview
                .set_bounds(tauri::Rect {
                    position: tauri::LogicalPosition::new(bounds.x, bounds.y).into(),
                    size: tauri::LogicalSize::new(bounds.width, bounds.height).into(),
                })
                .map_err(|error| format!("Could not resize browser: {error}"))?;
            if visible && session.active_tab_id.as_deref() == Some(tab_id) {
                tab.webview.show().map_err(|error| error.to_string())?;
            } else {
                tab.network_proxy.revoke_agent_network_access();
                tab.webview.hide().map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub async fn eval_action(
        &self,
        session_id: &str,
        tab_id: &str,
        expression: &str,
    ) -> Result<serde_json::Value, String> {
        let webview = self.webview(session_id, tab_id)?;
        eval_json(&webview, expression).await
    }

    pub fn reload(&self, session_id: &str, tab_id: &str) -> Result<(), String> {
        self.webview(session_id, tab_id)?
            .reload()
            .map_err(|error| error.to_string())
    }

    pub fn stop(&self, session_id: &str, tab_id: &str) -> Result<(), String> {
        self.webview(session_id, tab_id)?
            .eval("window.stop()")
            .map_err(|error| error.to_string())
    }

    pub fn go_back(&self, session_id: &str, tab_id: &str) -> Result<(), String> {
        self.webview(session_id, tab_id)?
            .eval("history.back()")
            .map_err(|error| error.to_string())
    }

    pub fn go_forward(&self, session_id: &str, tab_id: &str) -> Result<(), String> {
        self.webview(session_id, tab_id)?
            .eval("history.forward()")
            .map_err(|error| error.to_string())
    }

    pub async fn go_back_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        self.traverse_history_as_agent(
            session_id,
            tab_id,
            call_id,
            BrowserHistoryDirection::Back,
            commit_tracker,
        )
        .await
    }

    pub async fn go_forward_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        self.traverse_history_as_agent(
            session_id,
            tab_id,
            call_id,
            BrowserHistoryDirection::Forward,
            commit_tracker,
        )
        .await
    }

    async fn traverse_history_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        direction: BrowserHistoryDirection,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        let (webview, lease_generation) = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            ) {
                return Err(format!(
                    "Browser control changed before the Agent could go {}",
                    direction.label()
                ));
            }
            let tab = require_agent_tab_surface(session, tab_id)?;
            (tab.webview.clone(), session.control_lease.generation())
        };

        let expression = browser_history_target_expression(direction);
        let value = eval_json(&webview, &expression).await?;
        if value.is_null() {
            return Err(format!(
                "No validated browser history entry is available to go {}",
                direction.label()
            ));
        }
        let target: BrowserHistoryTarget = serde_json::from_value(value)
            .map_err(|error| format!("Browser history target was invalid: {error}"))?;
        let target_url = Url::parse(&target.url)
            .map_err(|error| format!("Browser history URL was invalid: {error}"))?;
        self.prepare_agent_network_access(session_id, tab_id, &target_url)
            .await?;

        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if session.control_lease.generation() != lease_generation
            || !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        {
            return Err(format!(
                "Browser control changed before the Agent could go {}",
                direction.label()
            ));
        }
        let tab = require_agent_tab_surface(session, tab_id)?;
        let approval = target_url.to_string();
        let target_key = serde_json::to_string(&target.key)
            .map_err(|error| format!("Browser history key was invalid: {error}"))?;
        with_agent_navigation_approval(&tab.approved_agent_urls, approval, || {
            dispatch_browser_navigation(Some(commit_tracker), || {
                tab.webview
                    .eval(format!("window.navigation.traverseTo({target_key})"))
                    .map_err(|error| error.to_string())
            })
        })
    }

    pub fn acquire_control(
        &self,
        session_id: &str,
        owner: BrowserControlOwner,
    ) -> Result<BrowserSessionInfo, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        session.control_lease.acquire(owner);
        session.observations.clear();
        let agent_owned = matches!(
            session.control_lease.owner(),
            BrowserControlOwner::Agent { .. }
        );
        for tab in session.tabs.values() {
            if !agent_owned {
                tab.network_proxy.revoke_agent_network_access();
            }
            tab.network_proxy.set_agent_restricted(agent_owned);
            if matches!(session.control_lease.owner(), BrowserControlOwner::User) {
                if let Ok(mut approved) = tab.approved_agent_urls.lock() {
                    approved.clear();
                }
            }
        }
        let webviews = matches!(session.control_lease.owner(), BrowserControlOwner::User)
            .then(|| {
                session
                    .tabs
                    .values()
                    .map(|tab| tab.webview.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let info = session_info(session);
        drop(runtime);
        invalidate_for_user_takeover(&webviews);
        self.emit(
            "controlChanged",
            serde_json::json!({ "sessionId": session_id, "owner": info.control_owner }),
        );
        Ok(info)
    }

    pub fn acquire_agent_control(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> Result<BrowserSessionInfo, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if !session.control_lease.try_acquire_agent(call_id.to_string()) {
            return Err(
                "Browser control belongs to the user; wait until they hand it back".to_string(),
            );
        }
        session.observations.clear();
        for tab in session.tabs.values() {
            tab.network_proxy.set_agent_restricted(true);
        }
        let info = session_info(session);
        drop(runtime);
        self.emit(
            "controlChanged",
            serde_json::json!({ "sessionId": session_id, "owner": info.control_owner }),
        );
        Ok(info)
    }

    pub fn release_control(&self, session_id: &str) -> Result<BrowserSessionInfo, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        session.control_lease.release();
        session.observations.clear();
        for tab in session.tabs.values() {
            tab.network_proxy.revoke_agent_network_access();
            tab.network_proxy.set_agent_restricted(false);
        }
        let info = session_info(session);
        drop(runtime);
        self.emit(
            "controlChanged",
            serde_json::json!({ "sessionId": session_id, "owner": info.control_owner }),
        );
        Ok(info)
    }

    async fn prepare_agent_network_access(
        &self,
        session_id: &str,
        tab_id: &str,
        url: &Url,
    ) -> Result<Option<ManagedLoopbackPermit>, String> {
        let (conversation_id, network_proxy, lease_generation, owner_call_id) = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            let tab = require_agent_tab_surface(session, tab_id)?;
            let BrowserControlOwner::Agent { call_id } = session.control_lease.owner() else {
                return Err(
                    "Browser control changed before network access was authorized".to_string(),
                );
            };
            (
                session.conversation_id.clone(),
                Arc::clone(&tab.network_proxy),
                session.control_lease.generation(),
                call_id.clone(),
            )
        };
        let permit = Self::validated_agent_network_permit(conversation_id.as_deref(), url).await?;
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        let tab = require_agent_tab_surface(session, tab_id)?;
        if session.control_lease.generation() != lease_generation
            || !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id } if call_id == &owner_call_id
            )
            || !Arc::ptr_eq(&network_proxy, &tab.network_proxy)
        {
            return Err(
                "Browser control or active surface changed before network access was authorized"
                    .to_string(),
            );
        }
        network_proxy.replace_agent_loopback_permits(permit.iter().cloned().collect());
        Ok(permit)
    }

    async fn prepare_proxy_network_access(
        conversation_id: Option<&str>,
        network_proxy: &BrowserNetworkProxy,
        url: &Url,
    ) -> Result<Option<ManagedLoopbackPermit>, String> {
        let permit = Self::validated_agent_network_permit(conversation_id, url).await?;
        network_proxy.replace_agent_loopback_permits(permit.iter().cloned().collect());
        Ok(permit)
    }

    async fn validated_agent_network_permit(
        conversation_id: Option<&str>,
        url: &Url,
    ) -> Result<Option<ManagedLoopbackPermit>, String> {
        let permit = if let Some(conversation_id) = conversation_id {
            managed_loopback_permits(conversation_id)
                .await
                .into_iter()
                .find(|permit| managed_permit_matches_url(permit, url))
        } else {
            None
        };
        validate_agent_network_url_with_permit(url, permit.as_ref()).await?;
        Ok(permit)
    }

    fn request_workspace_visibility(&self, session_id: &str) -> Result<(), String> {
        let (conversation_id, minimum_visibility_revision) = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            let minimum = next_visibility_request_revision(
                session.visibility_revision,
                session.visibility_request_revision,
            );
            session.visibility_requested = true;
            session.visibility_request_revision = Some(minimum);
            (session.conversation_id.clone(), minimum)
        };
        self.emit(
            "workspaceVisibilityRequested",
            serde_json::json!({
                "sessionId": session_id,
                "conversationId": conversation_id,
                "requestVisible": true,
                "minimumVisibilityRevision": minimum_visibility_revision,
            }),
        );
        Ok(())
    }

    async fn wait_until_workspace_visible(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> Result<(), String> {
        let already_visible = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            require_agent_tab_surface(session, tab_id).is_ok()
        };
        if already_visible {
            return Ok(());
        }
        self.request_workspace_visibility(session_id)?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let ready = {
                let runtime = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?;
                let session = runtime
                    .sessions
                    .get(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                if session.active_tab_id.as_deref() != Some(tab_id) {
                    return Err(
                        "The requested browser tab is not visible. Activate it before observing."
                            .to_string(),
                    );
                }
                require_agent_tab_surface(session, tab_id).is_ok()
            };
            if ready {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "Browser Workspace did not become visible. Keep the conversation open and retry the observation."
                        .to_string(),
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn observe(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
    ) -> Result<BrowserObservationPayload, String> {
        self.acquire_agent_control(session_id, call_id)?;
        self.wait_until_workspace_visible(session_id, tab_id)
            .await?;
        self.require_visible_focused_host_window()?;
        let lease_generation = self.agent_lease_generation(session_id, tab_id, call_id)?;
        let webview = self.webview(session_id, tab_id)?;
        let current_url = webview
            .url()
            .map_err(|error| format!("Could not read browser address: {error}"))?;
        self.prepare_agent_network_access(session_id, tab_id, &current_url)
            .await?;
        self.revalidate_agent_lease(session_id, tab_id, call_id, lease_generation)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let value = loop {
            self.revalidate_agent_lease(session_id, tab_id, call_id, lease_generation)?;
            let loading = self.tab_info(session_id, tab_id)?.loading;
            if !loading {
                if let Ok(value) = eval_json(&webview, OBSERVE_EXPRESSION).await {
                    if value.is_object() {
                        break value;
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err("Browser page did not become observable within 20 seconds".to_string());
            }
            tokio::time::sleep(Duration::from_millis(75)).await;
        };
        let snapshot: BrowserPageSnapshot = serde_json::from_value(value)
            .map_err(|error| format!("Could not decode browser observation: {error}"))?;
        let snapshot_url = Url::parse(&snapshot.url)
            .map_err(|_| "Browser observation returned an invalid URL".to_string())?;
        self.prepare_agent_network_access(session_id, tab_id, &snapshot_url)
            .await?;
        self.revalidate_agent_lease(session_id, tab_id, call_id, lease_generation)?;
        self.require_visible_focused_host_window()?;
        let (capture_plan, surface_flight) = self.capture_context(session_id, tab_id)?;
        let capture = capture_webview_image(&webview, capture_plan, surface_flight).await?;
        self.require_visible_focused_host_window()?;
        let screenshot = BrowserScreenshot {
            mime_type: capture.mime_type,
            content_hash: blake3::hash(&capture.image_bytes).to_hex().to_string(),
            width: capture.width,
            height: capture.height,
            byte_length: capture.image_bytes.len(),
            image_bytes: capture.image_bytes,
        };
        self.revalidate_agent_lease(session_id, tab_id, call_id, lease_generation)?;
        let confirmation: BrowserPageSnapshot =
            serde_json::from_value(eval_json(&webview, OBSERVE_EXPRESSION).await.map_err(
                |error| format!("Could not confirm browser visual observation: {error}"),
            )?)
            .map_err(|error| format!("Could not decode browser visual confirmation: {error}"))?;
        if confirmation.url != snapshot.url
            || confirmation.dom_fingerprint != snapshot.dom_fingerprint
            || confirmation.user_epoch != snapshot.user_epoch
        {
            return Err(
                "stale observation: page changed while its visual evidence was captured; observe again"
                    .to_string(),
            );
        }
        self.require_visible_focused_host_window()?;
        self.revalidate_agent_lease(session_id, tab_id, call_id, lease_generation)?;
        let content_hash = blake3::hash(snapshot.dom_fingerprint.as_bytes())
            .to_hex()
            .to_string();
        let observation_id = format!("obs_{}", uuid::Uuid::new_v4().simple());
        let owner = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if session.control_lease.generation() != lease_generation
                || !matches!(
                    session.control_lease.owner(),
                    BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
                )
            {
                return Err("stale observation: browser control owner changed".to_string());
            }
            require_agent_tab_surface(session, tab_id)?;
            let tab = session
                .tabs
                .get_mut(tab_id)
                .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
            let dispatched_url = tab
                .webview
                .url()
                .map_err(|error| format!("Could not read browser address: {error}"))?;
            if dispatched_url != snapshot_url {
                return Err("stale observation: page navigated during observation".to_string());
            }
            tab.url = snapshot.url.clone();
            tab.title = snapshot.title.clone();
            let stored = StoredObservation {
                created_at: Instant::now(),
                tab_id: tab_id.to_string(),
                url: snapshot.url.clone(),
                dom_fingerprint: snapshot.dom_fingerprint.clone(),
                user_epoch: snapshot.user_epoch,
                lease_generation,
                elements: snapshot.elements.clone(),
                claimed_for_action: false,
            };
            session.observations.insert(observation_id.clone(), stored);
            if session.observations.len() > MAX_OBSERVATIONS {
                if let Some(oldest) = session
                    .observations
                    .iter()
                    .min_by_key(|(_, observation)| observation.created_at)
                    .map(|(id, _)| id.clone())
                {
                    session.observations.remove(&oldest);
                }
            }
            session.control_lease.owner().clone()
        };
        let _ = snapshot.history_length;
        Ok(BrowserObservationPayload {
            observation_id,
            session_id: session_id.to_string(),
            tab_id: tab_id.to_string(),
            url: snapshot.url,
            title: snapshot.title,
            text: snapshot.text,
            viewport: snapshot.viewport,
            content_hash,
            elements: snapshot.elements.clone(),
            accessibility_tree: snapshot.elements,
            control_owner: owner,
            screenshot: Some(screenshot),
        })
    }

    pub async fn act(&self, request: BrowserActRequest<'_>) -> Result<BrowserActOutcome, String> {
        self.require_visible_focused_host_window()?;
        let (observation, expected, expected_end) = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(request.session_id)
                .ok_or_else(|| format!("Unknown browser session '{}'", request.session_id))?;
            require_agent_tab_surface(session, request.tab_id)?;
            if matches!(session.control_lease.owner(), BrowserControlOwner::User) {
                return Err(
                    "Browser control belongs to the user; wait until they hand it back".to_string(),
                );
            }
            session.control_lease.acquire(BrowserControlOwner::Agent {
                call_id: request.call_id.to_string(),
            });
            let observation = session
                .observations
                .get(request.observation_id)
                .cloned()
                .ok_or_else(|| "stale observation: observe the tab again".to_string())?;
            if observation.claimed_for_action {
                return Err(
                    "stale observation: this browser observation was already consumed by an action"
                        .to_string(),
                );
            }
            if observation.created_at.elapsed() > Duration::from_secs(120)
                || observation.tab_id != request.tab_id
                || observation.lease_generation != session.control_lease.generation()
            {
                return Err("stale observation: browser state or control owner changed".to_string());
            }
            let current_url = session
                .tabs
                .get(request.tab_id)
                .and_then(|tab| tab.webview.url().ok())
                .map(|url| url.to_string())
                .unwrap_or_default();
            if current_url != observation.url {
                return Err("stale observation: page navigated".to_string());
            }
            let expected = request.target_ref.and_then(|target_ref| {
                observation
                    .elements
                    .iter()
                    .find(|element| element.element_ref == target_ref)
                    .cloned()
            });
            if request.target_ref.is_some() && expected.is_none() {
                return Err("stale observation: target is not part of this observation".to_string());
            }
            let expected_end = request.end_ref.and_then(|end_ref| {
                observation
                    .elements
                    .iter()
                    .find(|element| element.element_ref == end_ref)
                    .cloned()
            });
            if request.end_ref.is_some() && expected_end.is_none() {
                return Err(
                    "stale observation: drag destination is not part of this observation"
                        .to_string(),
                );
            }
            // Claim while holding the session mutex, before the first await or
            // WebView/native side effect. A dropped callback or aborted turn
            // can no longer reuse the same pre-action state.
            session
                .observations
                .get_mut(request.observation_id)
                .expect("observation remained present under the session lock")
                .claimed_for_action = true;
            request.commit_tracker.mark_observation_consumed();
            (observation, expected, expected_end)
        };
        let is_form_submitter = expected.as_ref().is_some_and(|element| {
            element.tag == "button"
                || (element.tag == "input"
                    && element
                        .input_type
                        .as_deref()
                        .is_some_and(|kind| matches!(kind, "submit" | "image")))
        });
        let presses_activation_key = request.action == "press"
            && request
                .key
                .is_some_and(|key| matches!(key, "Enter" | " " | "Space" | "Spacebar"));
        let implicit_form_submit = request.action == "press"
            && request.key == Some("Enter")
            && expected.as_ref().is_some_and(|element| {
                element.tag == "input"
                    && !element.input_type.as_deref().is_some_and(|kind| {
                        matches!(kind, "button" | "reset" | "file" | "checkbox" | "radio")
                    })
            });
        let click_may_navigate = matches!(request.action, "click" | "double_click")
            && expected
                .as_ref()
                .is_some_and(|element| element.tag == "a" || is_form_submitter);
        let mut navigation_approval = None;
        if click_may_navigate || presses_activation_key {
            if let Some(href) = expected
                .as_ref()
                .and_then(|element| element.href.as_deref())
            {
                let target =
                    Url::parse(href).map_err(|_| "Browser link target is invalid".to_string())?;
                self.prepare_agent_network_access(request.session_id, request.tab_id, &target)
                    .await?;
                navigation_approval = Some((target, is_form_submitter || implicit_form_submit));
            }
        }
        let action_input = serde_json::to_string(&serde_json::json!({
            "action": request.action,
            "targetRef": request.target_ref,
            "endRef": request.end_ref,
            "text": request.text,
            "value": request.value,
            "key": request.key,
            "button": request.button.unwrap_or("left"),
            "modifiers": request.modifiers,
            "scrollX": request.scroll_x,
            "scrollY": request.scroll_y,
            "userEpoch": observation.user_epoch,
            "domFingerprint": observation.dom_fingerprint,
            "expected": expected,
            "expectedEnd": expected_end,
        }))
        .map_err(|error| error.to_string())?;
        let preview_expression = format!(
            "(() => {{ const bridge = window.__NEXA_BROWSER_RUNTIME__; if (!bridge) throw new Error('Browser interaction runtime is unavailable'); return bridge.previewAction({action_input}); }})()"
        );
        self.emit(
            "agentAction",
            serde_json::json!({
                "sessionId": request.session_id,
                "tabId": request.tab_id,
                "action": request.action,
                "phase": "moving",
                "targetRef": request.target_ref,
                "endRef": request.end_ref,
            }),
        );
        let preview = self.dispatch_agent_action(
            request.session_id,
            request.tab_id,
            request.observation_id,
            request.call_id,
            &preview_expression,
        )?;
        let preview = preview.resolve().await?;
        let duration_ms = preview
            .get("durationMs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(600);
        if duration_ms > 0 {
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        }
        if matches!(request.action, "move" | "hover") {
            // Acquire before the native preparation so a queued computer
            // action cannot make the returned element bounds stale while this
            // pointer commit waits for desktop ownership.
            let _desktop_input_guard = nexa_core::browser_runtime::acquire_desktop_input_permit()
                .await
                .map_err(|error| error.to_string())?;
            let _cross_process_guard =
                nexa_core::browser_runtime::try_acquire_cross_process_input()
                    .map_err(|error| error.to_string())?;
            let prepare_expression = format!(
                "(() => {{ const bridge = window.__NEXA_BROWSER_RUNTIME__; if (!bridge) throw new Error('Browser interaction runtime is unavailable'); return bridge.prepareNativePointer({action_input}); }})()"
            );
            self.emit(
                "agentAction",
                serde_json::json!({
                    "sessionId": request.session_id,
                    "tabId": request.tab_id,
                    "action": request.action,
                    "phase": "committing",
                    "targetRef": request.target_ref,
                }),
            );
            let preparation = self.dispatch_agent_action(
                request.session_id,
                request.tab_id,
                request.observation_id,
                request.call_id,
                &prepare_expression,
            )?;
            let prepared = preparation.resolve().await?;
            let target_bounds = prepared.get("bounds").cloned().ok_or_else(|| {
                "Browser pointer preparation returned no target bounds".to_string()
            })?;
            let target_bounds: BrowserElementBounds = serde_json::from_value(target_bounds)
                .map_err(|error| {
                    format!("Browser pointer preparation returned invalid bounds: {error}")
                })?;
            let verification_baseline =
                action_verification_baseline_from_preparation(&prepared, "pointer")?;
            self.require_visible_focused_host_window()?;
            self.move_native_pointer_to_target(
                request.session_id,
                request.tab_id,
                request.observation_id,
                request.call_id,
                observation.lease_generation,
                &target_bounds,
                &request.commit_tracker,
            )?;
            drop(_desktop_input_guard);
            let effect_observed = self
                .settle_after_agent_action(
                    request.session_id,
                    request.tab_id,
                    request.call_id,
                    &verification_baseline,
                )
                .await?;
            let fresh_observation = self
                .observe(request.session_id, request.tab_id, request.call_id)
                .await?;
            self.emit(
                "agentAction",
                serde_json::json!({
                    "sessionId": request.session_id,
                    "tabId": request.tab_id,
                    "action": request.action,
                    "phase": if effect_observed { "verified" } else { "observedUnchanged" },
                    "effectObserved": effect_observed,
                    "observationId": fresh_observation.observation_id,
                }),
            );
            return Ok(BrowserActOutcome {
                observation: fresh_observation,
                effect_observed,
            });
        }
        let mut navigation_permit_guard = None;
        if let Some((target, form_navigation)) = navigation_approval.as_ref() {
            self.approve_agent_action_url(
                request.session_id,
                request.tab_id,
                request.observation_id,
                request.call_id,
                target,
                *form_navigation,
            )?;
            navigation_permit_guard = Some(AgentNavigationPermitGuard {
                state: self.clone(),
                session_id: request.session_id.to_string(),
                tab_id: request.tab_id.to_string(),
                url: target.clone(),
                form_navigation: *form_navigation,
            });
        }
        let expression = format!(
            "(() => {{ const bridge = window.__NEXA_BROWSER_RUNTIME__; if (!bridge) throw new Error('Browser interaction runtime is unavailable'); return bridge.act({action_input}); }})()"
        );
        self.emit(
            "agentAction",
            serde_json::json!({
                "sessionId": request.session_id,
                "tabId": request.tab_id,
                "action": request.action,
                "phase": "committing",
                "targetRef": request.target_ref,
                "endRef": request.end_ref,
            }),
        );
        #[cfg(windows)]
        if matches!(request.action, "click" | "double_click" | "type" | "press") {
            let verification_baseline = match self
                .commit_trusted_webview_action(
                    &request,
                    &observation,
                    expected.as_ref(),
                    &action_input,
                    &request.commit_tracker,
                )
                .await
            {
                Ok(verification_baseline) => verification_baseline,
                Err(error) => {
                    if let Some((target, form_navigation)) = navigation_approval.as_ref() {
                        self.revoke_agent_action_url(
                            request.session_id,
                            request.tab_id,
                            target,
                            *form_navigation,
                        );
                    }
                    return Err(error);
                }
            };
            let effect_observed = self
                .settle_after_agent_action(
                    request.session_id,
                    request.tab_id,
                    request.call_id,
                    &verification_baseline,
                )
                .await?;
            let fresh_observation = self
                .observe(request.session_id, request.tab_id, request.call_id)
                .await?;
            drop(navigation_permit_guard);
            self.emit(
                "agentAction",
                serde_json::json!({
                    "sessionId": request.session_id,
                    "tabId": request.tab_id,
                    "action": request.action,
                    "phase": if effect_observed { "verified" } else { "observedUnchanged" },
                    "effectObserved": effect_observed,
                    "observationId": fresh_observation.observation_id,
                }),
            );
            return Ok(BrowserActOutcome {
                observation: fresh_observation,
                effect_observed,
            });
        }
        self.require_visible_focused_host_window()?;
        request.commit_tracker.mark_committed();
        let pending = match self.dispatch_agent_action(
            request.session_id,
            request.tab_id,
            request.observation_id,
            request.call_id,
            &expression,
        ) {
            Ok(pending) => pending,
            Err(error) => {
                if let Some((target, form_navigation)) = navigation_approval.as_ref() {
                    self.revoke_agent_action_url(
                        request.session_id,
                        request.tab_id,
                        target,
                        *form_navigation,
                    );
                }
                return Err(error);
            }
        };
        if let Err(error) = pending.resolve().await {
            if let Some((target, form_navigation)) = navigation_approval.as_ref() {
                self.revoke_agent_action_url(
                    request.session_id,
                    request.tab_id,
                    target,
                    *form_navigation,
                );
            }
            return Err(error);
        }
        let effect_observed = self
            .settle_after_agent_action(
                request.session_id,
                request.tab_id,
                request.call_id,
                &observation.verification_baseline(),
            )
            .await?;
        let fresh_observation = self
            .observe(request.session_id, request.tab_id, request.call_id)
            .await?;
        drop(navigation_permit_guard);
        self.emit(
            "agentAction",
            serde_json::json!({
                "sessionId": request.session_id,
                "tabId": request.tab_id,
                "action": request.action,
                "phase": if effect_observed { "verified" } else { "observedUnchanged" },
                "effectObserved": effect_observed,
                "observationId": fresh_observation.observation_id,
            }),
        );
        Ok(BrowserActOutcome {
            observation: fresh_observation,
            effect_observed,
        })
    }

    #[cfg(windows)]
    async fn commit_trusted_webview_action(
        &self,
        request: &BrowserActRequest<'_>,
        observation: &StoredObservation,
        expected: Option<&BrowserElement>,
        action_input: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<ActionVerificationBaseline, String> {
        let (preparation_method, preparation_label) = match request.action {
            "click" | "double_click" => ("prepareNativePointer", "pointer"),
            "type" => ("prepareTrustedText", "text"),
            "press" => ("prepareTrustedKey", "key"),
            action => return Err(format!("Unsupported trusted browser action '{action}'")),
        };
        let budget = trusted_action_budget(request.action, expected, request.key)?;
        let prepare_expression = format!(
            "(() => {{ const bridge = window.__NEXA_BROWSER_RUNTIME__; if (!bridge) throw new Error('Browser interaction runtime is unavailable'); return bridge.{preparation_method}({action_input}); }})()"
        );
        self.require_visible_focused_host_window()?;
        commit_tracker.mark_committed();
        let preparation = self.dispatch_agent_action(
            request.session_id,
            request.tab_id,
            request.observation_id,
            request.call_id,
            &prepare_expression,
        )?;
        let prepared = preparation.resolve().await.map_err(|error| {
            format!("Trusted browser {preparation_label} preparation failed: {error}")
        })?;
        let verification_baseline =
            action_verification_baseline_from_preparation(&prepared, preparation_label)?;
        let pointer_bounds = if matches!(request.action, "click" | "double_click") {
            Some(
                serde_json::from_value::<BrowserElementBounds>(
                    prepared.get("bounds").cloned().ok_or_else(|| {
                        "Trusted browser pointer preparation returned no target bounds".to_string()
                    })?,
                )
                .map_err(|error| {
                    format!("Trusted browser pointer preparation returned invalid bounds: {error}")
                })?,
            )
        } else {
            if prepared.get("focused").and_then(serde_json::Value::as_bool) != Some(true) {
                return Err(format!(
                    "Trusted browser {preparation_label} preparation could not focus the target"
                ));
            }
            None
        };
        let expected_input = match request.action {
            "click" | "double_click" => {
                let bounds = pointer_bounds
                    .as_ref()
                    .expect("pointer actions always parse prepared target bounds");
                TrustedInputMatch::Pointer {
                    x: bounds.x + bounds.width / 2.0,
                    y: bounds.y + bounds.height / 2.0,
                    button: request.button.unwrap_or("left").to_string(),
                }
            }
            "type" => TrustedInputMatch::Text {
                data: request.text.unwrap_or_default().to_string(),
            },
            "press" => trusted_key_input_match(
                request
                    .key
                    .expect("trusted action budget validates a press key"),
            )?,
            _ => unreachable!("trusted action kind was validated before preparation"),
        };

        self.require_visible_focused_host_window()?;

        // Validate the exact claimed observation, surface and lease immediately
        // before arming. The returned guard is cloned out of the runtime lock so
        // no synchronous mutex is held across a WebView await.
        let trusted_guard = self.trusted_input_guard_for_action(
            request.session_id,
            request.tab_id,
            request.observation_id,
            request.call_id,
            observation.lease_generation,
        )?;
        let armed_guard = trusted_guard
            .arm(budget, expected_input)
            .await
            .map_err(|error| {
                format!(
                    "Trusted browser input could not be armed; no input was dispatched: {error}"
                )
            })?;

        self.require_visible_focused_host_window()?;

        // Arming itself crosses the WebView boundary. Re-run the complete fence
        // before dispatch so a hide, resize, tab switch, takeover or navigation
        // that won that race cancels the action without a synthetic fallback.
        if let Err(fence_error) = self.trusted_input_guard_for_action(
            request.session_id,
            request.tab_id,
            request.observation_id,
            request.call_id,
            observation.lease_generation,
        ) {
            let disarm_result = armed_guard.disarm().await;
            return Err(match disarm_result {
                Ok(()) => format!(
                    "Trusted browser input was cancelled before dispatch because its state changed: {fence_error}"
                ),
                Err(disarm_error) => format!(
                    "Trusted browser input was cancelled before dispatch, but disarm failed and control state is uncertain: {fence_error}; {disarm_error}"
                ),
            });
        }

        let dispatch_result = match request.action {
            "click" | "double_click" => {
                let bounds = pointer_bounds
                    .as_ref()
                    .expect("pointer actions always parse prepared target bounds");
                dispatch_trusted_pointer_click(
                    &armed_guard,
                    bounds.x + bounds.width / 2.0,
                    bounds.y + bounds.height / 2.0,
                    request.button.unwrap_or("left"),
                    request.modifiers,
                    if request.action == "double_click" {
                        2
                    } else {
                        1
                    },
                )
                .await
            }
            "type" => insert_trusted_text(&armed_guard, request.text.unwrap_or_default()).await,
            "press" => {
                dispatch_trusted_key(
                    &armed_guard,
                    request
                        .key
                        .expect("trusted action budget validates a press key"),
                    request.modifiers,
                )
                .await
            }
            _ => unreachable!("trusted action kind was validated before preparation"),
        };
        let disarm_result = armed_guard.disarm().await;
        match (dispatch_result, disarm_result) {
            (Ok(()), Ok(())) => Ok(verification_baseline),
            (Err(dispatch_error), Ok(())) => Err(format!(
                "Trusted browser {} dispatch failed at its commit boundary; effect is uncertain and a fresh observation is required: {dispatch_error}",
                request.action
            )),
            (Ok(()), Err(disarm_error)) => Err(format!(
                "Trusted browser {} was dispatched, but disarm failed; effect and control state are uncertain and a fresh observation is required: {disarm_error}",
                request.action
            )),
            (Err(dispatch_error), Err(disarm_error)) => Err(format!(
                "Trusted browser {} dispatch and disarm both failed; effect and control state are uncertain and a fresh observation is required: {dispatch_error}; {disarm_error}",
                request.action
            )),
        }
    }

    async fn settle_after_agent_action(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        before: &ActionVerificationBaseline,
    ) -> Result<bool, String> {
        const SETTLE_LIMIT: Duration = Duration::from_millis(1_500);
        const CHANGE_QUIET_WINDOW: Duration = Duration::from_millis(150);
        const UNCHANGED_OBSERVATION_WINDOW: Duration = Duration::from_millis(400);
        const POLL_INTERVAL: Duration = Duration::from_millis(50);

        let started = Instant::now();
        let deadline = started + SETTLE_LIMIT;
        let mut effect_observed = false;
        let mut last_signature: Option<(String, String, u64)> = None;
        let mut stable_since = started;
        loop {
            let _ = self.agent_lease_generation(session_id, tab_id, call_id)?;
            let (webview, loading) = {
                let runtime = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?;
                let session = runtime
                    .sessions
                    .get(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                let tab = require_agent_tab_surface(session, tab_id)?;
                (tab.webview.clone(), tab.loading)
            };
            let now = Instant::now();
            if loading {
                effect_observed = true;
                last_signature = None;
                stable_since = now;
            } else if let Ok(snapshot) =
                eval_json(&webview, OBSERVE_EXPRESSION)
                    .await
                    .and_then(|value| {
                        serde_json::from_value::<BrowserPageSnapshot>(value).map_err(|error| {
                            format!("Could not decode browser settle state: {error}")
                        })
                    })
            {
                let signature = (
                    snapshot.url.clone(),
                    snapshot.dom_fingerprint.clone(),
                    snapshot.user_epoch,
                );
                if last_signature.as_ref() != Some(&signature) {
                    stable_since = now;
                    last_signature = Some(signature);
                }
                effect_observed |= action_snapshot_changed(
                    &before.url,
                    &before.dom_fingerprint,
                    before.user_epoch,
                    &snapshot.url,
                    &snapshot.dom_fingerprint,
                    snapshot.user_epoch,
                );
                if (effect_observed && stable_since.elapsed() >= CHANGE_QUIET_WINDOW)
                    || (!effect_observed && started.elapsed() >= UNCHANGED_OBSERVATION_WINDOW)
                {
                    return Ok(effect_observed);
                }
            }
            if now >= deadline {
                return Ok(effect_observed);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub fn action_risk(&self, args: &serde_json::Value) -> BrowserActionRisk {
        let action = args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let target_ref = args.get("targetRef").and_then(serde_json::Value::as_str);
        let element = args
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .zip(
                args.get("observationId")
                    .and_then(serde_json::Value::as_str),
            )
            .and_then(|(session_id, observation_id)| {
                self.inner.lock().ok().and_then(|runtime| {
                    runtime
                        .sessions
                        .get(session_id)
                        .and_then(|session| session.observations.get(observation_id))
                        .and_then(|observation| {
                            target_ref.and_then(|target_ref| {
                                observation
                                    .elements
                                    .iter()
                                    .find(|element| element.element_ref == target_ref)
                                    .cloned()
                            })
                        })
                })
            });
        classify_agent_action(
            action,
            element.as_ref().map(|element| element.role.as_str()),
            element.as_ref().map(|element| element.name.as_str()),
            element.as_ref().and_then(|element| element.href.as_deref()),
            element
                .as_ref()
                .and_then(|element| element.input_type.as_deref()),
        )
    }

    pub async fn reload_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        let (current_url, lease_generation) = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            ) {
                return Err("Browser control changed before the Agent could reload".to_string());
            }
            let current_url = require_agent_tab_surface(session, tab_id)?
                .webview
                .url()
                .map_err(|error| format!("Could not read browser address: {error}"))?;
            (current_url, session.control_lease.generation())
        };
        self.prepare_agent_network_access(session_id, tab_id, &current_url)
            .await?;
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if session.control_lease.generation() != lease_generation
            || !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        {
            return Err("Browser control changed before the Agent could reload".to_string());
        }
        let tab = require_agent_tab_surface(session, tab_id)?;
        let approval = current_url.to_string();
        with_agent_navigation_approval(&tab.approved_agent_urls, approval, || {
            dispatch_browser_navigation(Some(commit_tracker), || {
                tab.webview.reload().map_err(|error| error.to_string())
            })
        })
    }

    pub fn close_tab(&self, session_id: &str, tab_id: &str) -> Result<BrowserSessionInfo, String> {
        self.close_tab_checked(session_id, tab_id, None, None)
    }

    pub fn close_tab_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<BrowserSessionInfo, String> {
        self.close_tab_checked(session_id, tab_id, Some(call_id), Some(commit_tracker))
    }

    fn close_tab_checked(
        &self,
        session_id: &str,
        tab_id: &str,
        agent_call_id: Option<&str>,
        commit_tracker: Option<&BrowserActCommitTracker>,
    ) -> Result<BrowserSessionInfo, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if agent_call_id.is_some_and(|call_id| {
            !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        }) {
            return Err("Browser control changed before the Agent could close the tab".to_string());
        }
        if agent_call_id.is_some() {
            require_agent_tab_surface(session, tab_id)?;
        }
        let tab = session
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
        dispatch_terminal_browser_mutation(commit_tracker, || {
            tab.webview
                .close()
                .map_err(|error| format!("Could not close browser tab: {error}"))
        })?;
        let tab = session
            .tabs
            .remove(tab_id)
            .expect("successfully closed browser tab must remain registered until commit");
        tab.network_proxy.shutdown();
        let mut show_result = Ok(());
        if session.active_tab_id.as_deref() == Some(tab_id) {
            session.active_tab_id = session.tabs.keys().next().cloned();
            if let Some(active) = session
                .active_tab_id
                .as_ref()
                .and_then(|id| session.tabs.get(id))
            {
                if session.workspace_visible {
                    show_result = active
                        .webview
                        .show()
                        .map_err(|error| format!("Could not show the next browser tab: {error}"));
                }
            }
        }
        session
            .observations
            .retain(|_, observation| observation.tab_id != tab_id);
        let info = session_info(session);
        drop(runtime);
        self.emit(
            "tabClosed",
            serde_json::json!({ "sessionId": session_id, "tabId": tab_id }),
        );
        show_result?;
        Ok(info)
    }

    pub fn close_session(&self, session_id: &str) -> Result<(), String> {
        self.close_session_checked(session_id, None)
    }

    pub fn close_session_as_agent(
        &self,
        session_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        self.close_session_checked(session_id, Some((call_id, commit_tracker)))
    }

    fn close_session_checked(
        &self,
        session_id: &str,
        agent_context: Option<(&str, &BrowserActCommitTracker)>,
    ) -> Result<(), String> {
        if let Some((call_id, commit_tracker)) = agent_context {
            return self.close_session_for_agent(session_id, call_id, commit_tracker);
        }
        self.close_session_for_user(session_id)
    }

    fn close_session_for_user(&self, session_id: &str) -> Result<(), String> {
        self.begin_session_close(session_id, None, None)?;
        loop {
            let active_tab_id = {
                let runtime = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?;
                let session = runtime
                    .sessions
                    .get(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                let tab_ids = session.tabs.keys().cloned().collect::<Vec<_>>();
                let active_tab_id =
                    next_active_tab_for_terminal_close(session.active_tab_id.as_deref(), &tab_ids)?;
                if session.temporary_profile {
                    if let Some(active_tab_id) = active_tab_id.as_deref() {
                        let tab = session
                            .tabs
                            .get(active_tab_id)
                            .expect("validated active browser tab must remain registered");
                        tab.webview.clear_all_browsing_data().map_err(|error| {
                            format!(
                                "Could not clear temporary browser data before closing: {error}"
                            )
                        })?;
                    }
                }
                active_tab_id
            };
            let Some(tab_id) = active_tab_id else {
                break;
            };
            self.close_tab_checked(session_id, &tab_id, None, None)?;
        }
        self.finalize_empty_session_close(session_id, None, None)
    }

    fn close_session_for_agent(
        &self,
        session_id: &str,
        call_id: &str,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        let temporary_profile =
            self.begin_session_close(session_id, Some(call_id), Some(commit_tracker))?;

        loop {
            let active_tab_id = {
                let runtime = self
                    .inner
                    .lock()
                    .map_err(|_| "Browser runtime is unavailable".to_string())?;
                let session = runtime
                    .sessions
                    .get(session_id)
                    .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
                if !matches!(
                    session.control_lease.owner(),
                    BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
                ) {
                    return Err(
                        "Browser control changed before temporary browser data could be cleared"
                            .to_string(),
                    );
                }
                let tab_ids = session.tabs.keys().cloned().collect::<Vec<_>>();
                let active_tab_id =
                    next_active_tab_for_terminal_close(session.active_tab_id.as_deref(), &tab_ids)?;
                if let Some(active_tab_id) = active_tab_id.as_deref() {
                    let tab = require_agent_tab_surface(session, active_tab_id)?;
                    if temporary_profile {
                        dispatch_terminal_browser_mutation(Some(commit_tracker), || {
                            tab.webview.clear_all_browsing_data().map_err(|error| {
                                format!(
                                    "Could not clear temporary browser data before closing: {error}"
                                )
                            })
                        })?;
                    }
                }
                active_tab_id
            };
            let Some(tab_id) = active_tab_id else {
                break;
            };
            self.close_tab_checked(session_id, &tab_id, Some(call_id), Some(commit_tracker))?;
        }

        self.finalize_empty_session_close(session_id, Some(call_id), Some(commit_tracker))
    }

    fn begin_session_close(
        &self,
        session_id: &str,
        agent_call_id: Option<&str>,
        commit_tracker: Option<&BrowserActCommitTracker>,
    ) -> Result<bool, String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if agent_call_id.is_some_and(|call_id| {
            !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        }) {
            return Err(
                "Browser control changed before the Agent could close the session".to_string(),
            );
        }
        session.phase = session.phase.begin_close(session.opening_tabs)?;
        if let Some(commit_tracker) = commit_tracker {
            commit_tracker.mark_committed();
        }
        Ok(session.temporary_profile)
    }

    fn finalize_empty_session_close(
        &self,
        session_id: &str,
        agent_call_id: Option<&str>,
        commit_tracker: Option<&BrowserActCommitTracker>,
    ) -> Result<(), String> {
        let mut runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        if agent_call_id.is_some_and(|call_id| {
            !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        }) {
            return Err(
                "Browser control changed before the session close could be finalized".to_string(),
            );
        }
        if !session.tabs.is_empty() {
            return Err(
                "Browser tabs changed while the session was closing; inspect the remaining tabs before retrying"
                    .to_string(),
            );
        }
        if session.opening_tabs != 0 {
            return Err(
                "A browser tab began opening while the session was closing; retry after it finishes"
                    .to_string(),
            );
        }
        if session.phase != BrowserSessionPhase::Closing {
            return Err(
                "Browser session close lost its terminal state before cleanup; retry close_session"
                    .to_string(),
            );
        }
        let temporary_profile = session.temporary_profile;
        let profile_id = session.profile_id.clone();
        session.surface_gate.close();
        if temporary_profile {
            runtime
                .sessions
                .get_mut(session_id)
                .expect("validated empty browser session must remain registered")
                .phase = BrowserSessionPhase::CleanupPending;
            let profile_dir =
                validated_temporary_profile_dir(self.profile_root.as_path(), profile_id.as_str())?;
            dispatch_terminal_browser_mutation(commit_tracker, || {
                match std::fs::remove_dir_all(&profile_dir) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(format!(
                        "Temporary browser profile cleanup failed for '{}': {error}; the empty session remains available for retry",
                        profile_dir.display()
                    )),
                }
            })?;
        }
        if let Some(commit_tracker) = commit_tracker {
            commit_tracker.mark_committed();
        }
        runtime
            .sessions
            .remove(session_id)
            .expect("validated empty browser session must remain registered until commit");
        drop(runtime);
        self.emit(
            "sessionClosed",
            serde_json::json!({ "sessionId": session_id }),
        );
        Ok(())
    }

    pub fn close_all_sessions(&self) {
        let session_ids = self
            .inner
            .lock()
            .map(|runtime| runtime.sessions.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for session_id in session_ids {
            let _ = self.close_session(&session_id);
        }
    }

    fn dispatch_agent_action(
        &self,
        session_id: &str,
        tab_id: &str,
        observation_id: &str,
        call_id: &str,
        expression: &str,
    ) -> Result<PendingEvalJson, String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        let tab = require_agent_tab_surface(session, tab_id)?;
        if !matches!(
            session.control_lease.owner(),
            BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
        ) {
            return Err("stale observation: browser control owner changed".to_string());
        }
        let observation = session
            .observations
            .get(observation_id)
            .ok_or_else(|| "stale observation: observe the tab again".to_string())?;
        if observation.tab_id != tab_id
            || observation.lease_generation != session.control_lease.generation()
        {
            return Err("stale observation: browser state or control owner changed".to_string());
        }
        dispatch_eval_json(&tab.webview, expression)
    }

    #[cfg(windows)]
    fn trusted_input_guard_for_action(
        &self,
        session_id: &str,
        tab_id: &str,
        observation_id: &str,
        call_id: &str,
        lease_generation: u64,
    ) -> Result<BrowserTrustedInputGuard, String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        let tab = require_agent_tab_surface(session, tab_id)?;
        if session.control_lease.generation() != lease_generation
            || !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        {
            return Err(
                "Trusted browser input was cancelled because control or the active tab changed"
                    .to_string(),
            );
        }
        let observation = session
            .observations
            .get(observation_id)
            .filter(|observation| {
                observation.claimed_for_action
                    && observation.created_at.elapsed() <= Duration::from_secs(120)
                    && observation.tab_id == tab_id
                    && observation.lease_generation == lease_generation
            })
            .ok_or_else(|| {
                "Trusted browser input lost its exact claimed observation".to_string()
            })?;
        let current_url = tab
            .webview
            .url()
            .map_err(|error| format!("Could not read browser address: {error}"))?;
        if current_url.as_str() != observation.url {
            return Err(
                "Trusted browser input was cancelled because the observed page changed".to_string(),
            );
        }
        Ok(tab.trusted_input_guard.clone())
    }

    fn agent_lease_generation(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
    ) -> Result<u64, String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        require_agent_tab_surface(session, tab_id)?;
        if !matches!(
            session.control_lease.owner(),
            BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
        ) {
            return Err("Browser control changed before the Agent operation began".to_string());
        }
        Ok(session.control_lease.generation())
    }

    fn revalidate_agent_lease(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
        lease_generation: u64,
    ) -> Result<(), String> {
        let current_generation = self.agent_lease_generation(session_id, tab_id, call_id)?;
        if current_generation != lease_generation {
            return Err("Browser control changed during the Agent operation".to_string());
        }
        Ok(())
    }

    fn approve_agent_action_url(
        &self,
        session_id: &str,
        tab_id: &str,
        observation_id: &str,
        call_id: &str,
        url: &Url,
        form_navigation: bool,
    ) -> Result<(), String> {
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        let tab = require_agent_tab_surface(session, tab_id)?;
        if !matches!(
            session.control_lease.owner(),
            BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
        ) {
            return Err("stale observation: browser control owner changed".to_string());
        }
        let observation = session
            .observations
            .get(observation_id)
            .ok_or_else(|| "stale observation: observe the tab again".to_string())?;
        if observation.tab_id != tab_id
            || observation.lease_generation != session.control_lease.generation()
        {
            return Err("stale observation: browser state or control owner changed".to_string());
        }
        let approval = if form_navigation {
            form_navigation_approval_key(url)
        } else {
            url.to_string()
        };
        tab.approved_agent_urls
            .lock()
            .map_err(|_| "Browser navigation policy is unavailable".to_string())?
            .insert(approval);
        Ok(())
    }

    fn revoke_agent_action_url(
        &self,
        session_id: &str,
        tab_id: &str,
        url: &Url,
        form_navigation: bool,
    ) {
        let approval = if form_navigation {
            form_navigation_approval_key(url)
        } else {
            url.to_string()
        };
        if let Ok(runtime) = self.inner.lock() {
            if let Some(tab) = runtime
                .sessions
                .get(session_id)
                .and_then(|session| session.tabs.get(tab_id))
            {
                if let Ok(mut approved) = tab.approved_agent_urls.lock() {
                    approved.remove(&approval);
                }
            }
        }
    }

    fn move_native_pointer_to_target(
        &self,
        session_id: &str,
        tab_id: &str,
        observation_id: &str,
        call_id: &str,
        lease_generation: u64,
        target: &BrowserElementBounds,
        commit_tracker: &BrowserActCommitTracker,
    ) -> Result<(), String> {
        // Keep the runtime mutex through the native pointer commit. A hide,
        // tab switch or takeover therefore either wins before this validation
        // or waits until after the single OS side effect has completed.
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        let tab = require_agent_tab_surface(session, tab_id)?;
        if session.control_lease.generation() != lease_generation
            || !matches!(
                session.control_lease.owner(),
                BrowserControlOwner::Agent { call_id: owner_call_id } if owner_call_id == call_id
            )
        {
            return Err(
                "Browser pointer commit was cancelled because control or the active tab changed"
                    .to_string(),
            );
        }
        let observation = session
            .observations
            .get(observation_id)
            .filter(|observation| {
                observation.claimed_for_action
                    && observation.created_at.elapsed() <= Duration::from_secs(120)
                    && observation.tab_id == tab_id
                    && observation.lease_generation == lease_generation
            })
            .ok_or_else(|| "Browser pointer commit lost its claimed observation".to_string())?;
        let current_url = tab
            .webview
            .url()
            .map_err(|error| format!("Could not read browser address: {error}"))?;
        if current_url.as_str() != observation.url {
            return Err(
                "Browser pointer commit was cancelled because the page changed".to_string(),
            );
        }
        let bounds = tab.bounds;
        let window = self
            .app
            .get_window("main")
            .ok_or_else(|| "Main application window is unavailable".to_string())?;
        if !window
            .is_visible()
            .map_err(|error| format!("Could not read main window visibility: {error}"))?
            || window
                .is_minimized()
                .map_err(|error| format!("Could not read main window state: {error}"))?
        {
            return Err(
                "Browser pointer movement requires the visible, restored Nexa window".to_string(),
            );
        }
        if !window
            .is_focused()
            .map_err(|error| format!("Could not read main window focus: {error}"))?
        {
            return Err(
                "Browser pointer movement was cancelled because the user focused another application"
                    .to_string(),
            );
        }
        let origin = window
            .outer_position()
            .map_err(|error| format!("Could not read main window position: {error}"))?;
        let scale_factor = window
            .scale_factor()
            .map_err(|error| format!("Could not read main window scale: {error}"))?;
        let (x, y) =
            browser_target_screen_point((origin.x, origin.y), scale_factor, bounds, target)?;
        commit_tracker.mark_committed();
        let result = nexa_core::browser_runtime::move_native_pointer(x, y)
            .map_err(|error| error.to_string());
        drop(runtime);
        result
    }

    fn webview(&self, session_id: &str, tab_id: &str) -> Result<Webview, String> {
        self.inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?
            .sessions
            .get(session_id)
            .and_then(|session| session.tabs.get(tab_id))
            .map(|tab| tab.webview.clone())
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))
    }

    fn capture_context(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> Result<(BrowserCapturePlan, BrowserSurfaceFlight), String> {
        let (bounds, surface_flight) = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            let tab = require_agent_tab_surface(session, tab_id)?;
            let surface_flight = session.surface_gate.try_acquire()?;
            (tab.bounds, surface_flight)
        };
        let scale_factor = self
            .app
            .get_window("main")
            .ok_or_else(|| "Main application window is unavailable".to_string())?
            .scale_factor()
            .map_err(|error| format!("Could not read main window scale: {error}"))?;
        let plan = BrowserCapturePlan::new(bounds, scale_factor)?;
        Ok((plan, surface_flight))
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    fn conversation_creation_lock(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<AsyncMutex<()>>, String> {
        let mut locks = self
            .creation_locks
            .lock()
            .map_err(|_| "Browser session creation coordinator is unavailable".to_string())?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(conversation_id).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(conversation_id.to_string(), Arc::downgrade(&lock));
        Ok(lock)
    }
}

pub(super) fn accept_visibility_revision(current: &mut u64, incoming: u64) -> Result<(), String> {
    if incoming <= *current {
        return Err(format!(
            "Stale Browser Workspace visibility revision {incoming}; current revision is {current}"
        ));
    }
    *current = incoming;
    Ok(())
}

pub(super) fn next_visibility_request_revision(
    current_revision: u64,
    outstanding_request: Option<u64>,
) -> u64 {
    outstanding_request
        .filter(|revision| *revision > current_revision)
        .unwrap_or_else(|| current_revision.saturating_add(1))
}

pub(super) fn visibility_request_is_satisfied(
    outstanding_request: Option<u64>,
    visible: bool,
    incoming_revision: u64,
) -> bool {
    visible && outstanding_request.is_some_and(|required| incoming_revision >= required)
}

fn action_verification_baseline_from_preparation(
    prepared: &serde_json::Value,
    preparation_label: &str,
) -> Result<ActionVerificationBaseline, String> {
    serde_json::from_value(
        prepared
            .get("verificationBaseline")
            .cloned()
            .ok_or_else(|| {
                format!("Browser {preparation_label} preparation returned no verification baseline")
            })?,
    )
    .map_err(|error| {
        format!("Browser {preparation_label} preparation returned an invalid baseline: {error}")
    })
}

pub(super) fn action_snapshot_changed(
    before_url: &str,
    before_dom_fingerprint: &str,
    before_user_epoch: u64,
    after_url: &str,
    after_dom_fingerprint: &str,
    after_user_epoch: u64,
) -> bool {
    before_url != after_url
        || before_dom_fingerprint != after_dom_fingerprint
        || before_user_epoch != after_user_epoch
}

pub(super) fn trusted_action_budget(
    action: &str,
    target: Option<&BrowserElement>,
    key: Option<&str>,
) -> Result<TrustedInputEventBudget, String> {
    let target = target
        .ok_or_else(|| format!("Trusted browser {action} requires an observation-scoped target"))?;
    match action {
        "click" | "double_click" => {
            let click_count = if action == "double_click" { 2 } else { 1 };
            let expected_input_events = u8::from(
                target.tag.eq_ignore_ascii_case("input")
                    && target.input_type.as_deref().is_some_and(|input_type| {
                        input_type.eq_ignore_ascii_case("checkbox")
                            || input_type.eq_ignore_ascii_case("radio")
                    }),
            ) * click_count;
            TrustedInputEventBudget::pointer_click(click_count, expected_input_events)
        }
        "type" => Ok(TrustedInputEventBudget::text_insert()),
        "press" => {
            let key = key.ok_or_else(|| "Trusted browser press requires a key".to_string())?;
            if !matches!(
                key,
                "Enter"
                    | "Tab"
                    | "Escape"
                    | "Esc"
                    | " "
                    | "Space"
                    | "Spacebar"
                    | "ArrowLeft"
                    | "ArrowUp"
                    | "ArrowRight"
                    | "ArrowDown"
                    | "Home"
                    | "End"
                    | "PageUp"
                    | "PageDown"
                    | "Backspace"
                    | "Delete"
            ) {
                return Err(format!("Unsupported trusted browser key '{key}'"));
            }
            let tag = target.tag.as_str();
            let input_type = target.input_type.as_deref().unwrap_or_default();
            let is_checkable = tag.eq_ignore_ascii_case("input")
                && (input_type.eq_ignore_ascii_case("checkbox")
                    || input_type.eq_ignore_ascii_case("radio"));
            let is_editable_input = tag.eq_ignore_ascii_case("input")
                && !matches!(
                    input_type.to_ascii_lowercase().as_str(),
                    "button"
                        | "checkbox"
                        | "color"
                        | "file"
                        | "hidden"
                        | "image"
                        | "radio"
                        | "range"
                        | "reset"
                        | "submit"
                );
            let is_editable = is_editable_input
                || tag.eq_ignore_ascii_case("textarea")
                || matches!(target.role.as_str(), "textbox" | "searchbox");
            let is_select =
                tag.eq_ignore_ascii_case("select") || target.role.eq_ignore_ascii_case("combobox");
            let is_value_stepper = tag.eq_ignore_ascii_case("input")
                && matches!(input_type.to_ascii_lowercase().as_str(), "number" | "range");
            let expected_input_events = u8::from(
                (matches!(key, " " | "Space" | "Spacebar") && (is_checkable || is_editable))
                    || (matches!(key, "ArrowUp" | "ArrowDown") && (is_select || is_value_stepper))
                    || (matches!(key, "Home" | "End") && is_value_stepper)
                    || (matches!(key, "Backspace" | "Delete") && is_editable)
                    || (key == "Enter"
                        && (tag.eq_ignore_ascii_case("textarea")
                            || (!tag.eq_ignore_ascii_case("input")
                                && target.role.eq_ignore_ascii_case("textbox")))),
            );
            TrustedInputEventBudget::key_press(expected_input_events)
        }
        _ => Err(format!("Unsupported trusted browser action '{action}'")),
    }
}

pub(super) fn agent_tab_surface_is_valid(
    workspace_visible: bool,
    active: bool,
    bounds: BrowserBounds,
) -> bool {
    workspace_visible && active && bounds.width >= 64.0 && bounds.height >= 64.0
}

pub(super) fn browser_tab_open_allowed(
    existing_tabs: usize,
    opening_tabs: usize,
    initializing: bool,
    workspace_visible: bool,
) -> bool {
    let below_limit = existing_tabs.saturating_add(opening_tabs) < MAX_BROWSER_TABS_PER_SESSION;
    let initial_tab = initializing && existing_tabs == 0 && opening_tabs == 0;
    below_limit && (workspace_visible || initial_tab)
}

pub(super) fn next_active_tab_for_terminal_close(
    active_tab_id: Option<&str>,
    tab_ids: &[String],
) -> Result<Option<String>, String> {
    if tab_ids.is_empty() {
        return if active_tab_id.is_none() {
            Ok(None)
        } else {
            Err("Browser session retained an active tab id after all tabs were closed".to_string())
        };
    }
    let active_tab_id = active_tab_id
        .ok_or_else(|| "Browser session has tabs but no active tab to close safely".to_string())?;
    if !tab_ids.iter().any(|tab_id| tab_id == active_tab_id) {
        return Err("Browser session active tab is not registered".to_string());
    }
    Ok(Some(active_tab_id.to_string()))
}

pub(super) fn validated_temporary_profile_dir(
    profile_root: &Path,
    profile_id: &str,
) -> Result<PathBuf, String> {
    if !profile_root.is_absolute() {
        return Err("Browser profile root must be absolute".to_string());
    }
    let mut components = Path::new(profile_id).components();
    let Some(Component::Normal(profile_name)) = components.next() else {
        return Err("Temporary browser profile id is not a safe path segment".to_string());
    };
    if components.next().is_some() {
        return Err("Temporary browser profile id must be one safe path segment".to_string());
    }
    let profile_dir = profile_root.join(profile_name);
    if profile_dir == profile_root || !profile_dir.starts_with(profile_root) {
        return Err("Temporary browser profile escaped its configured root".to_string());
    }
    Ok(profile_dir)
}

fn require_agent_tab_surface<'a>(
    session: &'a BrowserSession,
    tab_id: &str,
) -> Result<&'a BrowserTab, String> {
    let tab = session
        .tabs
        .get(tab_id)
        .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
    if !agent_tab_surface_is_valid(
        session.workspace_visible,
        session.active_tab_id.as_deref() == Some(tab_id),
        tab.bounds,
    ) {
        return Err(
            "Browser Workspace must be visible with the target tab active and valid bounds"
                .to_string(),
        );
    }
    Ok(tab)
}

fn invalidate_for_user_takeover(webviews: &[Webview]) {
    for webview in webviews {
        let _ = webview.eval("window.__NEXA_BROWSER_RUNTIME__?.invalidateForUserTakeover()");
    }
}

pub struct BrowserActRequest<'a> {
    pub call_id: &'a str,
    pub session_id: &'a str,
    pub tab_id: &'a str,
    pub observation_id: &'a str,
    pub action: &'a str,
    pub target_ref: Option<&'a str>,
    pub end_ref: Option<&'a str>,
    pub text: Option<&'a str>,
    pub value: Option<&'a str>,
    pub key: Option<&'a str>,
    pub button: Option<&'a str>,
    pub modifiers: &'a [String],
    pub scroll_x: i64,
    pub scroll_y: i64,
    pub commit_tracker: BrowserActCommitTracker,
}

pub(super) fn browser_host_window_allows_agent_action(
    visible: bool,
    minimized: bool,
    focused: bool,
) -> bool {
    visible && !minimized && focused
}

pub(super) fn browser_target_screen_point(
    window_origin: (i32, i32),
    scale_factor: f64,
    webview_bounds: BrowserBounds,
    target_bounds: &BrowserElementBounds,
) -> Result<(i32, i32), String> {
    let logical_x = webview_bounds.x + target_bounds.x + target_bounds.width / 2.0;
    let logical_y = webview_bounds.y + target_bounds.y + target_bounds.height / 2.0;
    if !scale_factor.is_finite()
        || scale_factor <= 0.0
        || !logical_x.is_finite()
        || !logical_y.is_finite()
    {
        return Err("Browser pointer target has invalid screen coordinates".to_string());
    }
    let physical_x = f64::from(window_origin.0) + logical_x * scale_factor;
    let physical_y = f64::from(window_origin.1) + logical_y * scale_factor;
    if physical_x < f64::from(i32::MIN)
        || physical_x > f64::from(i32::MAX)
        || physical_y < f64::from(i32::MIN)
        || physical_y > f64::from(i32::MAX)
    {
        return Err("Browser pointer target is outside the supported desktop range".to_string());
    }
    Ok((physical_x.round() as i32, physical_y.round() as i32))
}

fn safe_identifier(input: &str) -> String {
    input
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(80)
        .collect()
}

pub(super) fn browser_history_target_expression(direction: BrowserHistoryDirection) -> String {
    format!(
        "(() => {{ const api = window.navigation; if (!api || typeof api.entries !== 'function' || typeof api.traverseTo !== 'function' || !api.currentEntry) return null; const targetIndex = api.currentEntry.index + ({}); const target = api.entries().find((entry) => entry.index === targetIndex); return target && typeof target.url === 'string' && typeof target.key === 'string' ? {{ key: target.key, url: target.url }} : null; }})()",
        direction.offset()
    )
}

pub(super) fn with_agent_navigation_approval(
    approved_urls: &Mutex<HashSet<String>>,
    approval: String,
    navigate: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    approved_urls
        .lock()
        .map_err(|_| "Browser navigation policy is unavailable".to_string())?
        .insert(approval.clone());
    if let Err(error) = navigate() {
        if let Ok(mut approved) = approved_urls.lock() {
            approved.remove(&approval);
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn dispatch_browser_navigation<T>(
    commit_tracker: Option<&BrowserActCommitTracker>,
    navigate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let result = navigate();
    if result.is_ok() {
        if let Some(commit_tracker) = commit_tracker {
            commit_tracker.mark_committed();
        }
    }
    result
}

pub(super) fn dispatch_terminal_browser_mutation<T>(
    commit_tracker: Option<&BrowserActCommitTracker>,
    mutate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if let Some(commit_tracker) = commit_tracker {
        commit_tracker.mark_committed();
    }
    mutate()
}

fn session_info(session: &BrowserSession) -> BrowserSessionInfo {
    let mut tabs: Vec<_> = session
        .tabs
        .values()
        .map(|tab| BrowserTabInfo {
            id: tab.id.clone(),
            session_id: session.id.clone(),
            url: tab.url.clone(),
            title: tab.title.clone(),
            active: session.active_tab_id.as_deref() == Some(tab.id.as_str()),
            loading: tab.loading,
            status: tab.status.clone(),
        })
        .collect();
    tabs.sort_by(|left, right| left.id.cmp(&right.id));
    BrowserSessionInfo {
        id: session.id.clone(),
        conversation_id: session.conversation_id.clone(),
        profile_id: session.profile_id.clone(),
        active_tab_id: session.active_tab_id.clone(),
        tabs,
        control_owner: session.control_lease.owner().clone(),
        workspace_visible: session.workspace_visible,
        cleanup_pending: session.phase == BrowserSessionPhase::CleanupPending,
        visibility_revision: session.visibility_revision,
        visibility_requested: session.visibility_requested,
        visibility_request_revision: session.visibility_request_revision,
    }
}
