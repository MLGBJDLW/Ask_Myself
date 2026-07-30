use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserRuntimeEventKind {
    SessionCreated,
    SessionClosed,
    TabOpened,
    TabClosed,
    TabActivated,
    PageLoadStarted,
    PageLoadFinished,
    TitleChanged,
    ControlChanged,
    NewWindowRequested,
    DownloadRequested,
    PermissionRequested,
    Crashed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRuntimeEvent {
    pub kind: BrowserRuntimeEventKind,
    pub session_id: String,
    pub tab_id: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}
