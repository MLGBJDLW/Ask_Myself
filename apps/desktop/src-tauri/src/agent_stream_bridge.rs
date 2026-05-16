use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nexa_core::agent::AgentEvent;
use nexa_core::db::Database;
use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::agent_stream::{
    agent_event_rotates_stream_blocks, compact_agent_event_for_frontend, PendingStreamDelta,
    StreamBlockEmitter,
};
use crate::agent_task_events::{emit_agent_task_run_update, record_task_progress_for_agent_event};

const STREAM_FLUSH_INTERVAL_MS: u64 = 16;

pub(crate) struct AgentStreamForwarder {
    app_handle: AppHandle,
    db: Arc<Database>,
    conversation_id: String,
    task_run_id: String,
    turn_id: String,
    event_seq: Arc<AtomicU64>,
    terminal_emitted: Arc<AtomicBool>,
}

impl AgentStreamForwarder {
    pub(crate) fn new(
        app_handle: AppHandle,
        db: Arc<Database>,
        conversation_id: String,
        task_run_id: String,
        turn_id: String,
        event_seq: Arc<AtomicU64>,
        terminal_emitted: Arc<AtomicBool>,
    ) -> Self {
        Self {
            app_handle,
            db,
            conversation_id,
            task_run_id,
            turn_id,
            event_seq,
            terminal_emitted,
        }
    }

    pub(crate) async fn run(self, mut rx: mpsc::Receiver<AgentEvent>) {
        let mut pending_delta: Option<PendingStreamDelta> = None;
        let mut stream_emitter = StreamBlockEmitter::new(Arc::clone(&self.event_seq));
        let mut reasoning_phase_recorded = false;
        let mut generating_phase_recorded = false;
        let mut tick = tokio::time::interval(Duration::from_millis(STREAM_FLUSH_INTERVAL_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;

        loop {
            tokio::select! {
                biased;
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(AgentEvent::TextDelta { delta }) => {
                            if self.terminal_emitted.load(Ordering::SeqCst) {
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
                        Some(AgentEvent::Thinking { content }) => {
                            if self.terminal_emitted.load(Ordering::SeqCst) {
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
                        Some(other) => {
                            let frontend_event = compact_agent_event_for_frontend(other);
                            self.flush_pending(&mut stream_emitter, &mut pending_delta);
                            let rotates_blocks = agent_event_rotates_stream_blocks(&frontend_event);
                            let run_event = stream_emitter.next_run_event(
                                &self.task_run_id,
                                Some(&self.turn_id),
                                &frontend_event,
                            );
                            if run_event.is_terminal() {
                                if self.terminal_emitted.swap(true, Ordering::SeqCst) {
                                    continue;
                                }
                            } else if self.terminal_emitted.load(Ordering::SeqCst) {
                                continue;
                            }
                            record_task_progress_for_agent_event(
                                &self.db,
                                &self.app_handle,
                                &self.conversation_id,
                                &self.task_run_id,
                                &frontend_event,
                                &run_event,
                            );
                            stream_emitter.emit_event(
                                &self.app_handle,
                                &self.conversation_id,
                                run_event,
                            );
                            if rotates_blocks {
                                stream_emitter.rotate_blocks();
                            }
                        }
                        None => {
                            if !self.terminal_emitted.load(Ordering::SeqCst) {
                                self.flush_pending(&mut stream_emitter, &mut pending_delta);
                            }
                            break;
                        }
                    }
                }
                _ = tick.tick() => {
                    if !self.terminal_emitted.load(Ordering::SeqCst) {
                        self.flush_pending(&mut stream_emitter, &mut pending_delta);
                    }
                }
            }
        }
    }

    fn record_progress_phase(&self, phase: &str, label: &str) {
        let _ = self.db.update_agent_task_run_progress(
            &self.task_run_id,
            Some("running"),
            Some(phase),
            None,
            Some(label),
            None,
            None,
        );
        emit_agent_task_run_update(
            &self.db,
            &self.app_handle,
            &self.conversation_id,
            &self.task_run_id,
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
            &self.app_handle,
            &self.db,
            &self.task_run_id,
            Some(&self.turn_id),
        );
    }
}
