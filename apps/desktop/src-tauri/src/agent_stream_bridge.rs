use std::time::{Duration, Instant};

use nexa_core::agent::AgentEvent;
use nexa_core::agent_run::{AgentRunEventPersistence, AgentRunPhase};
use nexa_core::runtime::{AgentRunEventOutbox, TurnLaunchStage};
use tokio::sync::mpsc;

use crate::agent_stream::{
    agent_event_rotates_stream_blocks, prepare_agent_run_event_for_frontend, PendingStreamDelta,
    StreamBlockEmitter,
};
use crate::agent_task_events::record_internal_agent_run_status_event;
use crate::tool_preview_journal::ToolPreviewJournal;

// Text rendering stays frame-responsive. Cumulative tool-input previews are
// independently sampled by ToolPreviewJournal and never enter SQLite.
const STREAM_FLUSH_INTERVAL_MS: u64 = 50;

pub(crate) struct AgentStreamForwarder {
    conversation_id: String,
    task_run_id: String,
    turn_id: String,
    event_outbox: AgentRunEventOutbox,
    launch_started: Instant,
}

impl AgentStreamForwarder {
    pub(crate) fn new(
        conversation_id: String,
        task_run_id: String,
        turn_id: String,
        event_outbox: AgentRunEventOutbox,
        launch_started: Instant,
    ) -> Self {
        Self {
            conversation_id,
            task_run_id,
            turn_id,
            event_outbox,
            launch_started,
        }
    }

    pub(crate) async fn run(self, mut rx: mpsc::Receiver<AgentEvent>) {
        let mut pending_delta: Option<PendingStreamDelta> = None;
        let mut pending_tool_updates = ToolPreviewJournal::default();
        let mut stream_emitter = StreamBlockEmitter::new(self.event_outbox.clone());
        let mut reasoning_phase_recorded = false;
        let mut generating_phase_recorded = false;
        let mut provider_connected_recorded = false;
        let mut first_sse_byte_recorded = false;
        let mut first_visible_token_recorded = false;
        let mut tick = tokio::time::interval(Duration::from_millis(STREAM_FLUSH_INTERVAL_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;

        loop {
            tokio::select! {
                biased;
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            if matches!(
                                &event,
                                AgentEvent::ControllerStatus { code, .. }
                                    if code == "provider_connected"
                            ) && !provider_connected_recorded {
                                provider_connected_recorded = true;
                                self.record_launch_metric(TurnLaunchStage::ProviderConnectMs);
                            } else if provider_connected_recorded
                                && !first_sse_byte_recorded
                                && event_marks_provider_response_byte(&event)
                            {
                                first_sse_byte_recorded = true;
                                self.record_launch_metric(TurnLaunchStage::FirstSseByteMs);
                            }

                            if provider_connected_recorded
                                && !first_visible_token_recorded
                                && event_has_visible_token(&event)
                            {
                                first_visible_token_recorded = true;
                                self.record_launch_metric(TurnLaunchStage::FirstVisibleTokenMs);
                            }

                            match event {
                                AgentEvent::TextDelta { delta } => {
                                    if self.event_outbox.is_closed_for_submission() {
                                        continue;
                                    }
                                    if !generating_phase_recorded {
                                        generating_phase_recorded = true;
                                        self.record_progress_phase("generating", "Generating answer");
                                    }
                                    match &mut pending_delta {
                                        Some(PendingStreamDelta::Text(text)) => text.push_str(&delta),
                                        Some(PendingStreamDelta::Thinking(_)) => {
                                            self.flush_pending(&mut stream_emitter, &mut pending_delta);
                                            pending_delta = Some(PendingStreamDelta::Text(delta));
                                        }
                                        None => pending_delta = Some(PendingStreamDelta::Text(delta)),
                                    }
                                }
                                AgentEvent::Thinking { content } => {
                                    if self.event_outbox.is_closed_for_submission() {
                                        continue;
                                    }
                                    if !reasoning_phase_recorded {
                                        reasoning_phase_recorded = true;
                                        self.record_progress_phase("reasoning", "Reasoning");
                                    }
                                    match &mut pending_delta {
                                        Some(PendingStreamDelta::Thinking(thinking)) => {
                                            thinking.push_str(&content)
                                        }
                                        Some(PendingStreamDelta::Text(_)) => {
                                            self.flush_pending(&mut stream_emitter, &mut pending_delta);
                                            pending_delta = Some(PendingStreamDelta::Thinking(content));
                                        }
                                        None => pending_delta = Some(PendingStreamDelta::Thinking(content)),
                                    }
                                }
                                AgentEvent::ToolCallPreparing { .. }
                                | AgentEvent::ToolCallArgsDelta { .. }
                                | AgentEvent::ToolCallStart { .. }
                                | AgentEvent::ToolCallProgress { .. }
                                | AgentEvent::ToolCallResult { .. } => {
                                    // ToolRun is the only public tool lifecycle.
                                }
                                event @ AgentEvent::ToolRunUpdated { .. } => {
                                    if self.event_outbox.is_closed_for_submission() {
                                        continue;
                                    }
                                    self.flush_pending(&mut stream_emitter, &mut pending_delta);
                                    pending_tool_updates.queue(event, Instant::now());
                                }
                                other => {
                                    self.flush_pending(&mut stream_emitter, &mut pending_delta);
                                    match &other {
                                        AgentEvent::StreamReset { .. } => {
                                            pending_tool_updates.reset();
                                        }
                                        AgentEvent::ToolRunCompleted { run } => {
                                            pending_tool_updates.retire(&run.call_id);
                                        }
                                        _ => self.flush_tool_preview_events(
                                            &stream_emitter,
                                            pending_tool_updates.drain_due(Instant::now()),
                                        ),
                                    }
                                    let (frontend_event, run_event) =
                                        prepare_agent_run_event_for_frontend(
                                            &self.task_run_id,
                                            Some(&self.turn_id),
                                            other,
                                        );
                                    let rotates_blocks = agent_event_rotates_stream_blocks(&frontend_event);
                                    if !run_event.is_terminal()
                                        && self.event_outbox.is_closed_for_submission()
                                    {
                                        continue;
                                    }
                                    stream_emitter.emit_event(
                                        &self.conversation_id,
                                        run_event,
                                    );
                                    if rotates_blocks {
                                        stream_emitter.rotate_blocks();
                                    }
                                }
                            }
                        }
                        None => {
                            if !self.event_outbox.is_closed_for_submission() {
                                self.flush_pending(&mut stream_emitter, &mut pending_delta);
                                self.flush_tool_preview_events(
                                    &stream_emitter,
                                    pending_tool_updates.drain_all(),
                                );
                            }
                            break;
                        }
                    }
                }
                _ = tick.tick() => {
                    if !self.event_outbox.is_closed_for_submission() {
                        self.flush_pending(&mut stream_emitter, &mut pending_delta);
                        self.flush_tool_preview_events(
                            &stream_emitter,
                            pending_tool_updates.drain_due(Instant::now()),
                        );
                    }
                }
            }
        }
    }

    fn record_progress_phase(&self, phase: &str, label: &str) {
        let payload = serde_json::json!({ "phase": phase });
        record_internal_agent_run_status_event(
            &self.conversation_id,
            &self.task_run_id,
            Some(&self.turn_id),
            &self.event_outbox,
            AgentRunPhase::Responding,
            label,
            Some("running"),
            Some(&payload),
        );
    }

    fn record_launch_metric(&self, stage: TurnLaunchStage) {
        let elapsed_ms = self
            .launch_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let payload = serde_json::json!({
            "kind": "turnLaunchMetric",
            "stage": stage.as_str(),
            "elapsedMs": elapsed_ms,
        });
        record_internal_agent_run_status_event(
            &self.conversation_id,
            &self.task_run_id,
            Some(&self.turn_id),
            &self.event_outbox,
            AgentRunPhase::Routing,
            stage.as_str(),
            None,
            Some(&payload),
        );
    }

    fn flush_pending(
        &self,
        stream_emitter: &mut StreamBlockEmitter,
        pending_delta: &mut Option<PendingStreamDelta>,
    ) {
        stream_emitter.flush_pending(
            pending_delta,
            &self.conversation_id,
            &self.task_run_id,
            Some(&self.turn_id),
        );
    }

    fn flush_tool_preview_events(
        &self,
        stream_emitter: &StreamBlockEmitter,
        events: Vec<AgentEvent>,
    ) {
        for event in events {
            let (_, mut run_event) =
                prepare_agent_run_event_for_frontend(&self.task_run_id, Some(&self.turn_id), event);
            run_event.persistence = AgentRunEventPersistence::Ephemeral;
            stream_emitter.emit_event(&self.conversation_id, run_event);
        }
    }
}

pub(crate) fn event_marks_provider_response_byte(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TextDelta { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::ToolCallPreparing { .. }
            | AgentEvent::ToolCallArgsDelta { .. }
            | AgentEvent::ToolCallStart { .. }
            | AgentEvent::ToolRunStarted { .. }
            | AgentEvent::ToolRunUpdated { .. }
            | AgentEvent::ToolRunCompleted { .. }
            | AgentEvent::UsageUpdate { .. }
            | AgentEvent::Done { .. }
    )
}

pub(crate) fn event_has_visible_token(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::TextDelta { delta } => !delta.is_empty(),
        AgentEvent::Thinking { content } => !content.is_empty(),
        AgentEvent::ToolCallStart { .. } => true,
        AgentEvent::ToolRunStarted { .. } | AgentEvent::ToolRunCompleted { .. } => true,
        AgentEvent::Done { message, .. } => !message.text_content().is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::llm::{Message, Role, Usage};

    #[test]
    fn provider_byte_and_visible_output_contracts_are_distinct() {
        let preparing = AgentEvent::ToolCallPreparing {
            call_id: "call-1".into(),
            tool_name: "search".into(),
            args_bytes: 1,
            index: 0,
        };
        assert!(event_marks_provider_response_byte(&preparing));
        assert!(!event_has_visible_token(&preparing));

        let tool_start = AgentEvent::ToolCallStart {
            call_id: "call-1".into(),
            tool_name: "search".into(),
            arguments: "{}".into(),
        };
        assert!(event_marks_provider_response_byte(&tool_start));
        assert!(event_has_visible_token(&tool_start));

        let done = AgentEvent::Done {
            message: Message::text(Role::Assistant, "cached answer"),
            usage_total: Usage::default(),
            last_prompt_tokens: 0,
            context_breakdown: None,
            cached: true,
            finish_reason: Some("stop".into()),
        };
        assert!(event_has_visible_token(&done));
    }
}
