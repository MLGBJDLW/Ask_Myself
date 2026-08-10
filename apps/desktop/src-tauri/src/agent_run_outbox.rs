//! Tauri delivery adapter for committed Run Events and task projections.

use nexa_core::agent_run::AgentRunEvent;
use nexa_core::conversation::AgentTaskRun;
use nexa_core::run_event_outbox::AgentRunEventDelivery;
use tauri::AppHandle;

use crate::agent_stream::emit_agent_run_frontend_event;
use crate::agent_task_events::emit_agent_task_run_snapshot;

pub(crate) struct DesktopAgentRunEventDelivery {
    app_handle: AppHandle,
}

impl DesktopAgentRunEventDelivery {
    pub(crate) fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }
}

impl AgentRunEventDelivery for DesktopAgentRunEventDelivery {
    fn deliver_run_event(&self, conversation_id: &str, event: &AgentRunEvent) {
        emit_agent_run_frontend_event(&self.app_handle, conversation_id, event);
    }

    fn deliver_task_run_snapshot(&self, conversation_id: &str, snapshot: AgentTaskRun) {
        emit_agent_task_run_snapshot(&self.app_handle, conversation_id, snapshot);
    }
}
