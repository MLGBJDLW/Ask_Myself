//! Stable payload for non-stream task timeline events.
//!
//! `AgentRunEvent` owns live stream replay ordering. `TaskTimelineEvent` is for
//! Task Center lifecycle facts that should be durable and visible, but must not
//! consume run `eventSeq` or replay as assistant output.

use serde::{Deserialize, Serialize};

pub const TASK_TIMELINE_EVENT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskTimelineEventKind {
    Subtask,
    Verification,
}

impl TaskTimelineEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subtask => "subtask",
            Self::Verification => "verification",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTimelineEvent {
    pub version: u16,
    pub kind: TaskTimelineEventKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub payload: serde_json::Value,
}

impl TaskTimelineEvent {
    pub fn new(
        kind: TaskTimelineEventKind,
        label: &str,
        status: Option<&str>,
        payload: Option<&serde_json::Value>,
    ) -> Self {
        Self {
            version: TASK_TIMELINE_EVENT_VERSION,
            kind,
            label: label.to_string(),
            status: status.map(str::to_string),
            payload: payload.cloned().unwrap_or_else(|| serde_json::json!({})),
        }
    }

    pub fn subtask(label: &str, status: &str, payload: Option<&serde_json::Value>) -> Self {
        Self::new(TaskTimelineEventKind::Subtask, label, Some(status), payload)
    }

    pub fn verification(
        label: &str,
        status: Option<&str>,
        payload: Option<&serde_json::Value>,
    ) -> Self {
        Self::new(TaskTimelineEventKind::Verification, label, status, payload)
    }

    pub fn event_type(&self) -> &'static str {
        self.kind.as_str()
    }

    pub fn task_event_payload(&self) -> serde_json::Value {
        serde_json::json!({ "taskTimeline": self })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_timeline_payload_without_agent_run_protocol() {
        let event = TaskTimelineEvent::subtask(
            "Collect evidence",
            "running",
            Some(&serde_json::json!({ "subtaskRunId": "subtask-1" })),
        );

        let payload = event.task_event_payload();

        assert_eq!(event.event_type(), "subtask");
        assert_eq!(payload["taskTimeline"]["version"], 1);
        assert_eq!(payload["taskTimeline"]["kind"], "subtask");
        assert!(payload.get("agentRun").is_none());
    }
}
