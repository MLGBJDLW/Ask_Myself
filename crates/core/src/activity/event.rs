use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityEventKind {
    Started,
    StdoutChunk,
    StderrChunk,
    Progress,
    ReadyUrl,
    CwdChanged,
    CommandStarted,
    CommandFinished,
    PromptDetected,
    InputRequested,
    BrowserObservation,
    DesktopObservation,
    StateChanged,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub activity_id: String,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: ActivityEventKind,
    pub payload: serde_json::Value,
}
