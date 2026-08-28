use std::collections::HashMap;
use std::time::{Duration, Instant};

use nexa_core::agent::{AgentEvent, ToolRunStatus};

const MIN_PREVIEW_GROWTH_BYTES: usize = 2 * 1024;
const MAX_PREVIEW_SILENCE: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct PreviewCursor {
    first_observed_at: Instant,
    last_emitted_at: Option<Instant>,
    last_emitted_bytes: usize,
}

/// Coalesces cumulative tool-input snapshots behind a small interface.
///
/// Provider chunks may arrive token-by-token, but callers only need a
/// meaningful diff checkpoint or a slow-stream heartbeat. The final tool
/// lifecycle remains authoritative and is not handled by this module.
#[derive(Default)]
pub(crate) struct ToolPreviewJournal {
    pending: Vec<AgentEvent>,
    cursors: HashMap<String, PreviewCursor>,
}

impl ToolPreviewJournal {
    pub(crate) fn queue(&mut self, event: AgentEvent, now: Instant) {
        let Some(call_id) = preview_call_id(&event) else {
            return;
        };
        self.cursors
            .entry(call_id.to_string())
            .or_insert(PreviewCursor {
                first_observed_at: now,
                last_emitted_at: None,
                last_emitted_bytes: 0,
            });
        if let Some(index) = self
            .pending
            .iter()
            .position(|candidate| preview_call_id(candidate) == Some(call_id))
        {
            self.pending[index] = event;
        } else {
            self.pending.push(event);
        }
    }

    pub(crate) fn drain_due(&mut self, now: Instant) -> Vec<AgentEvent> {
        let mut ready = Vec::new();
        let mut waiting = Vec::with_capacity(self.pending.len());
        for event in self.pending.drain(..) {
            let Some(call_id) = preview_call_id(&event).map(str::to_string) else {
                continue;
            };
            let received_bytes = preview_received_bytes(&event);
            let cursor = self.cursors.entry(call_id).or_insert(PreviewCursor {
                first_observed_at: now,
                last_emitted_at: None,
                last_emitted_bytes: 0,
            });
            let last_activity = cursor.last_emitted_at.unwrap_or(cursor.first_observed_at);
            let due = received_bytes.saturating_sub(cursor.last_emitted_bytes)
                >= MIN_PREVIEW_GROWTH_BYTES
                || now.saturating_duration_since(last_activity) >= MAX_PREVIEW_SILENCE;
            if due {
                cursor.last_emitted_at = Some(now);
                cursor.last_emitted_bytes = received_bytes;
                ready.push(event);
            } else {
                waiting.push(event);
            }
        }
        self.pending = waiting;
        ready
    }

    pub(crate) fn retire(&mut self, call_id: &str) {
        self.pending
            .retain(|event| preview_call_id(event) != Some(call_id));
        self.cursors.remove(call_id);
    }

    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        self.cursors.clear();
    }

    pub(crate) fn drain_all(&mut self) -> Vec<AgentEvent> {
        self.cursors.clear();
        std::mem::take(&mut self.pending)
    }
}

fn preview_call_id(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::ToolRunUpdated { run } if run.status == ToolRunStatus::Preparing => {
            Some(run.call_id.as_str())
        }
        _ => None,
    }
}

fn preview_received_bytes(event: &AgentEvent) -> usize {
    let AgentEvent::ToolRunUpdated { run } = event else {
        return 0;
    };
    run.artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.pointer("/inputProgress/receivedBytes"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_else(|| run.arguments.as_deref().map(str::len).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexa_core::agent::{ToolRunItem, ToolRunStatus};
    use nexa_core::tools::{
        ToolInputStreamingMode, ToolInterruptBehavior, ToolRenderKind, ToolRunCapabilities,
    };

    fn preview(bytes: usize) -> AgentEvent {
        AgentEvent::ToolRunUpdated {
            run: ToolRunItem {
                call_id: "call-file".to_string(),
                tool_name: "create_file".to_string(),
                owner: nexa_core::plugins::capability_owner_for_tool("create_file"),
                provider_executed: false,
                status: ToolRunStatus::Preparing,
                arguments: Some("x".repeat(bytes)),
                render_kind: ToolRenderKind::FileChange,
                capabilities: ToolRunCapabilities {
                    input_streaming: ToolInputStreamingMode::UiPreview,
                    render_kind: ToolRenderKind::FileChange,
                    read_only: false,
                    destructive: true,
                    concurrency_safe: false,
                    interrupt_behavior: ToolInterruptBehavior::Block,
                    resource_keys: Vec::new(),
                },
                content: None,
                is_error: None,
                artifacts: Some(serde_json::json!({
                    "inputProgress": { "receivedBytes": bytes }
                })),
                progress_note: None,
                duration_ms: None,
            },
        }
    }

    #[test]
    fn token_chunks_collapse_into_bounded_semantic_preview_updates() {
        let started = Instant::now();
        let mut journal = ToolPreviewJournal::default();
        let mut emitted = Vec::new();
        for index in 1..=5_000usize {
            let now = started + Duration::from_millis(index as u64 * 10);
            journal.queue(preview(index * 16), now);
            emitted.extend(journal.drain_due(now));
        }
        emitted.extend(journal.drain_all());

        assert!(emitted.len() <= 45, "emitted {} previews", emitted.len());
        assert_eq!(preview_received_bytes(emitted.last().unwrap()), 80_000);
    }

    #[test]
    fn slow_small_preview_emits_a_heartbeat_after_two_seconds() {
        let started = Instant::now();
        let mut journal = ToolPreviewJournal::default();
        journal.queue(preview(32), started);
        assert!(journal
            .drain_due(started + Duration::from_millis(1_999))
            .is_empty());
        assert_eq!(journal.drain_due(started + Duration::from_secs(2)).len(), 1);
    }

    #[test]
    fn execution_lifecycle_updates_never_enter_the_preview_journal() {
        let started = Instant::now();
        let mut journal = ToolPreviewJournal::default();
        let mut running = preview(32);
        let AgentEvent::ToolRunUpdated { run } = &mut running else {
            unreachable!("preview fixture must be a ToolRunUpdated event");
        };
        run.status = ToolRunStatus::Running;

        journal.queue(running, started);

        assert!(journal.drain_all().is_empty());
    }
}
