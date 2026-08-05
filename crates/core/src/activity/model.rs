use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivitySurface {
    Process,
    Terminal,
    Browser,
    Desktop,
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Queued,
    Starting,
    Running,
    Ready,
    WaitingInput,
    Quiet,
    Suspended,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
    Superseded,
    TimedOut,
}

impl ActivityState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed
                | Self::Cancelled
                | Self::Orphaned
                | Self::Superseded
                | Self::TimedOut
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySpec {
    pub activity_id: String,
    pub session_id: Option<String>,
    pub surface: ActivitySurface,
    pub conversation_id: Option<String>,
    pub turn_id: Option<String>,
    pub task_run_id: Option<String>,
    pub parent_activity_id: Option<String>,
    pub owner_tool: String,
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
}

impl ActivitySpec {
    pub fn new(surface: ActivitySurface, owner_tool: impl Into<String>) -> Self {
        Self {
            activity_id: format!("act_{}", Uuid::new_v4()),
            session_id: None,
            surface,
            conversation_id: None,
            turn_id: None,
            task_run_id: None,
            parent_activity_id: None,
            owner_tool: owner_tool.into(),
            workspace_id: None,
            cwd: None,
        }
    }

    pub fn with_activity_id(mut self, activity_id: impl Into<String>) -> Self {
        self.activity_id = activity_id.into();
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_conversation_id(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    pub fn with_task_run_id(mut self, task_run_id: impl Into<String>) -> Self {
        self.task_run_id = Some(task_run_id.into());
        self
    }

    pub fn with_parent_activity_id(mut self, parent_activity_id: impl Into<String>) -> Self {
        self.parent_activity_id = Some(parent_activity_id.into());
        self
    }

    pub fn with_workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecord {
    pub activity_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub surface: ActivitySurface,
    pub state: ActivityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_activity_id: Option<String>,
    pub owner_tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub last_event_seq: u64,
}

impl ActivityRecord {
    pub(crate) fn from_spec(spec: ActivitySpec, now: DateTime<Utc>) -> Self {
        Self {
            activity_id: spec.activity_id,
            session_id: spec.session_id,
            surface: spec.surface,
            state: ActivityState::Running,
            conversation_id: spec.conversation_id,
            turn_id: spec.turn_id,
            task_run_id: spec.task_run_id,
            parent_activity_id: spec.parent_activity_id,
            owner_tool: spec.owner_tool,
            workspace_id: spec.workspace_id,
            cwd: spec.cwd,
            started_at: now,
            updated_at: now,
            completed_at: None,
            last_event_seq: 0,
        }
    }
}
