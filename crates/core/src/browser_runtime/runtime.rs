use async_trait::async_trait;

use super::{
    ActInBrowserTab, BrowserControlOwner, BrowserObservation, BrowserSession, BrowserTab,
    CreateBrowserSession, NavigateBrowserTab, ObserveBrowserTab, OpenBrowserTab,
};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BrowserRuntimeError(pub String);

impl From<String> for BrowserRuntimeError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

pub type BrowserRuntimeResult<T> = Result<T, BrowserRuntimeError>;

/// Backend-neutral browser lifecycle used by both the visible dock and tools.
#[async_trait]
pub trait BrowserRuntime: Send + Sync {
    async fn create_session(
        &self,
        request: CreateBrowserSession,
    ) -> BrowserRuntimeResult<BrowserSession>;
    async fn list_sessions(&self) -> BrowserRuntimeResult<Vec<BrowserSession>>;
    async fn bind_session(
        &self,
        conversation_id: &str,
    ) -> BrowserRuntimeResult<Option<BrowserSession>>;
    async fn open_tab(&self, request: OpenBrowserTab) -> BrowserRuntimeResult<BrowserTab>;
    async fn activate_tab(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<BrowserSession>;
    async fn navigate(&self, request: NavigateBrowserTab) -> BrowserRuntimeResult<BrowserTab>;
    async fn go_back(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()>;
    async fn go_forward(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()>;
    async fn reload(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()>;
    async fn observe(&self, request: ObserveBrowserTab)
        -> BrowserRuntimeResult<BrowserObservation>;
    async fn act(&self, request: ActInBrowserTab) -> BrowserRuntimeResult<BrowserObservation>;
    async fn begin_element_pick(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()>;
    async fn end_element_pick(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<Option<serde_json::Value>>;
    async fn acquire_control(
        &self,
        session_id: &str,
        owner: BrowserControlOwner,
    ) -> BrowserRuntimeResult<BrowserSession>;
    async fn release_control(&self, session_id: &str) -> BrowserRuntimeResult<BrowserSession>;
    async fn close_tab(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<BrowserSession>;
    async fn close_session(&self, session_id: &str) -> BrowserRuntimeResult<()>;
}
