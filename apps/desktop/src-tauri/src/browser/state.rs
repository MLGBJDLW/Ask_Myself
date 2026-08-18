use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub use nexa_core::browser_runtime::{
    BrowserBounds, BrowserControlOwner, BrowserElement, ControlLease,
};
use nexa_core::browser_runtime::{
    BrowserObservation as CoreBrowserObservation, BrowserSession as CoreBrowserSession,
    BrowserTab as CoreBrowserTab,
};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Webview};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use super::network_proxy::BrowserNetworkProxy;
use super::policy::{
    classify_agent_action, form_navigation_approval_key, normalize_browser_url,
    validate_agent_network_url, BrowserActionRisk, NavigationActor,
};
use super::scripts::OBSERVE_EXPRESSION;
use super::webview_host::{
    create_child_webview, dispatch_eval_json, eval_json, BrowserChildWebview, PendingEvalJson,
};

pub const BROWSER_EVENT: &str = "browser:event";
const MAX_OBSERVATIONS: usize = 64;

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

pub type BrowserObservationPayload = CoreBrowserObservation;

#[derive(Clone)]
struct StoredObservation {
    created_at: Instant,
    tab_id: String,
    url: String,
    dom_fingerprint: String,
    user_epoch: u64,
    lease_generation: u64,
    elements: Vec<BrowserElement>,
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
    approved_agent_urls: Arc<Mutex<HashSet<String>>>,
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
    agent_restricted: Arc<AtomicBool>,
    network_proxy: Arc<BrowserNetworkProxy>,
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

    pub fn update_page_load(&self, session_id: &str, tab_id: &str, url: &Url, loading: bool) {
        if let Ok(mut runtime) = self.inner.lock() {
            if let Some(tab) = runtime
                .sessions
                .get_mut(session_id)
                .and_then(|session| session.tabs.get_mut(tab_id))
            {
                tab.url = url.to_string();
                tab.loading = loading;
                tab.status = if loading { "loading" } else { "idle" }.to_string();
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
            session.network_proxy.set_agent_restricted(false);
            for tab in session.tabs.values() {
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
                if open_initial_url_on_reuse {
                    if let Some(initial_url) = initial_url {
                        self.open_tab(&existing.id, initial_url, actor, bounds)
                            .await?;
                    }
                }
                return self.session_info(&existing.id);
            }
        }
        let session_id = format!("browser_{}", uuid::Uuid::new_v4().simple());
        let temporary_profile = profile_id.is_none();
        let profile_id = profile_id
            .as_deref()
            .map(safe_identifier)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("temporary-{session_id}"));
        let agent_restricted = Arc::new(AtomicBool::new(actor == NavigationActor::Agent));
        let network_proxy = Arc::new(BrowserNetworkProxy::start(Arc::clone(&agent_restricted))?);
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
                    agent_restricted,
                    network_proxy,
                },
            );
        }
        let target = initial_url.unwrap_or("https://www.google.com");
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
        self.emit(
            "sessionCreated",
            serde_json::json!({ "sessionId": session_id }),
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
        let url = normalize_browser_url(input, actor)?;
        if actor == NavigationActor::Agent {
            validate_agent_network_url(&url).await?;
        } else {
            self.acquire_control(session_id, BrowserControlOwner::User)?;
        }
        let (profile_id, tab_id, agent_restricted, network_proxy_url) = {
            let runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get(session_id)
                .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
            if actor == NavigationActor::Agent
                && matches!(session.control_lease.owner(), BrowserControlOwner::User)
            {
                return Err(
                    "Browser control belongs to the user; wait until they hand it back".to_string(),
                );
            }
            if actor == NavigationActor::Agent {
                session.network_proxy.set_agent_restricted(true);
            }
            (
                session.profile_id.clone(),
                format!("tab_{}", uuid::Uuid::new_v4().simple()),
                Arc::clone(&session.agent_restricted),
                session.network_proxy.url().clone(),
            )
        };
        let profile_dir = self.profile_root.join(&profile_id);
        std::fs::create_dir_all(&profile_dir)
            .map_err(|error| format!("Could not create browser profile: {error}"))?;
        let BrowserChildWebview {
            webview,
            approved_agent_urls,
        } = create_child_webview(
            self,
            session_id,
            &tab_id,
            url.clone(),
            profile_dir,
            &profile_id,
            agent_restricted,
            network_proxy_url,
            bounds,
        )?;
        let info = {
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
                    drop(runtime);
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
            for tab in session.tabs.values() {
                let _ = tab.webview.hide();
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
                    approved_agent_urls,
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
    ) -> Result<BrowserTabInfo, String> {
        let url = normalize_browser_url(input, actor)?;
        if actor == NavigationActor::Agent {
            validate_agent_network_url(&url).await?;
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
                if matches!(session.control_lease.owner(), BrowserControlOwner::None) {
                    session.control_lease.acquire(BrowserControlOwner::Agent {
                        call_id: "navigate".to_string(),
                    });
                }
            }
            session.observations.clear();
            session
                .network_proxy
                .set_agent_restricted(actor == NavigationActor::Agent);
            let tab = session
                .tabs
                .get_mut(tab_id)
                .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
            if actor == NavigationActor::Agent {
                tab.approved_agent_urls
                    .lock()
                    .map_err(|_| "Browser navigation policy is unavailable".to_string())?
                    .insert(url.to_string());
            }
            if let Err(error) = tab.webview.navigate(url.clone()) {
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
            if !session.tabs.contains_key(source_tab_id) {
                return Err(format!("Unknown browser tab '{source_tab_id}'"));
            }
            if session.agent_restricted.load(Ordering::Relaxed) {
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
        if !session.tabs.contains_key(tab_id) {
            return Err(format!("Unknown browser tab '{tab_id}'"));
        }
        for (id, tab) in &session.tabs {
            if id == tab_id {
                tab.webview.show().map_err(|error| error.to_string())?;
                let _ = tab.webview.set_focus();
            } else {
                let _ = tab.webview.hide();
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

    pub fn set_bounds(
        &self,
        session_id: &str,
        bounds: BrowserBounds,
        visible: bool,
    ) -> Result<(), String> {
        let bounds = bounds.sanitized();
        let runtime = self
            .inner
            .lock()
            .map_err(|_| "Browser runtime is unavailable".to_string())?;
        let session = runtime
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("Unknown browser session '{session_id}'"))?;
        for (tab_id, tab) in &session.tabs {
            tab.webview
                .set_bounds(tauri::Rect {
                    position: tauri::LogicalPosition::new(bounds.x, bounds.y).into(),
                    size: tauri::LogicalSize::new(bounds.width, bounds.height).into(),
                })
                .map_err(|error| format!("Could not resize browser: {error}"))?;
            if visible && session.active_tab_id.as_deref() == Some(tab_id) {
                tab.webview.show().map_err(|error| error.to_string())?;
            } else {
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
        session.network_proxy.set_agent_restricted(matches!(
            session.control_lease.owner(),
            BrowserControlOwner::Agent { .. }
        ));
        for tab in session.tabs.values() {
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
        session.network_proxy.set_agent_restricted(true);
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
        let info = session_info(session);
        drop(runtime);
        self.emit(
            "controlChanged",
            serde_json::json!({ "sessionId": session_id, "owner": info.control_owner }),
        );
        Ok(info)
    }

    pub async fn observe(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
    ) -> Result<BrowserObservationPayload, String> {
        self.acquire_agent_control(session_id, call_id)?;
        let lease_generation = self.agent_lease_generation(session_id, call_id)?;
        let webview = self.webview(session_id, tab_id)?;
        let current_url = webview
            .url()
            .map_err(|error| format!("Could not read browser address: {error}"))?;
        validate_agent_network_url(&current_url).await?;
        self.revalidate_agent_lease(session_id, call_id, lease_generation)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let value = loop {
            self.revalidate_agent_lease(session_id, call_id, lease_generation)?;
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
        validate_agent_network_url(&snapshot_url).await?;
        self.revalidate_agent_lease(session_id, call_id, lease_generation)?;
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
        })
    }

    pub async fn act(
        &self,
        request: BrowserActRequest<'_>,
    ) -> Result<BrowserObservationPayload, String> {
        let (observation, expected, expected_end) = {
            let mut runtime = self
                .inner
                .lock()
                .map_err(|_| "Browser runtime is unavailable".to_string())?;
            let session = runtime
                .sessions
                .get_mut(request.session_id)
                .ok_or_else(|| format!("Unknown browser session '{}'", request.session_id))?;
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
                validate_agent_network_url(&target).await?;
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
        if let Some((target, form_navigation)) = navigation_approval.as_ref() {
            self.approve_agent_action_url(
                request.session_id,
                request.tab_id,
                request.observation_id,
                request.call_id,
                target,
                *form_navigation,
            )?;
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
        let observation = self
            .observe(request.session_id, request.tab_id, request.call_id)
            .await?;
        self.emit(
            "agentAction",
            serde_json::json!({
                "sessionId": request.session_id,
                "tabId": request.tab_id,
                "action": request.action,
                "phase": "verified",
                "observationId": observation.observation_id,
            }),
        );
        Ok(observation)
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
            let current_url = session
                .tabs
                .get(tab_id)
                .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?
                .webview
                .url()
                .map_err(|error| format!("Could not read browser address: {error}"))?;
            (current_url, session.control_lease.generation())
        };
        validate_agent_network_url(&current_url).await?;
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
        let tab = session
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
        let approval = current_url.to_string();
        tab.approved_agent_urls
            .lock()
            .map_err(|_| "Browser navigation policy is unavailable".to_string())?
            .insert(approval.clone());
        if let Err(error) = tab.webview.reload() {
            if let Ok(mut approved) = tab.approved_agent_urls.lock() {
                approved.remove(&approval);
            }
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn close_tab(&self, session_id: &str, tab_id: &str) -> Result<BrowserSessionInfo, String> {
        self.close_tab_checked(session_id, tab_id, None)
    }

    pub fn close_tab_as_agent(
        &self,
        session_id: &str,
        tab_id: &str,
        call_id: &str,
    ) -> Result<BrowserSessionInfo, String> {
        self.close_tab_checked(session_id, tab_id, Some(call_id))
    }

    fn close_tab_checked(
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
            return Err("Browser control changed before the Agent could close the tab".to_string());
        }
        let tab = session
            .tabs
            .remove(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
        let _ = tab.webview.close();
        if session.active_tab_id.as_deref() == Some(tab_id) {
            session.active_tab_id = session.tabs.keys().next().cloned();
            if let Some(active) = session
                .active_tab_id
                .as_ref()
                .and_then(|id| session.tabs.get(id))
            {
                let _ = active.webview.show();
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
        Ok(info)
    }

    pub fn close_session(&self, session_id: &str) -> Result<(), String> {
        self.close_session_checked(session_id, None)
    }

    pub fn close_session_as_agent(&self, session_id: &str, call_id: &str) -> Result<(), String> {
        self.close_session_checked(session_id, Some(call_id))
    }

    fn close_session_checked(
        &self,
        session_id: &str,
        agent_call_id: Option<&str>,
    ) -> Result<(), String> {
        let session = {
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
                    "Browser control changed before the Agent could close the session".to_string(),
                );
            }
            runtime
                .sessions
                .remove(session_id)
                .expect("validated browser session must still exist")
        };
        session.network_proxy.shutdown();
        for tab in session.tabs.into_values() {
            if session.temporary_profile {
                let _ = tab.webview.clear_all_browsing_data();
            }
            let _ = tab.webview.close();
        }
        if session.temporary_profile {
            let profile_dir = self.profile_root.join(&session.profile_id);
            if profile_dir.starts_with(self.profile_root.as_path()) {
                let _ = std::fs::remove_dir_all(profile_dir);
            }
        }
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
        let tab = session
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
        dispatch_eval_json(&tab.webview, expression)
    }

    fn agent_lease_generation(&self, session_id: &str, call_id: &str) -> Result<u64, String> {
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
            return Err("Browser control changed before the Agent operation began".to_string());
        }
        Ok(session.control_lease.generation())
    }

    fn revalidate_agent_lease(
        &self,
        session_id: &str,
        call_id: &str,
        lease_generation: u64,
    ) -> Result<(), String> {
        let current_generation = self.agent_lease_generation(session_id, call_id)?;
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
        let tab = session
            .tabs
            .get(tab_id)
            .ok_or_else(|| format!("Unknown browser tab '{tab_id}'"))?;
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
}

fn safe_identifier(input: &str) -> String {
    input
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(80)
        .collect()
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
    }
}
