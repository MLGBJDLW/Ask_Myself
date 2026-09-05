use super::{protocol_error, PreparedTurn};
use nexa_core::agent::{AgentEvent, StreamBlockChannel};
use nexa_core::error::CoreError;
use nexa_core::llm::Usage;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::mpsc;

#[derive(Default)]
pub(super) struct Projection {
    offsets: HashMap<String, usize>,
    completed: HashSet<String>,
    drafts: HashMap<String, String>,
    draft_order: Vec<String>,
    draft_bytes: usize,
    async_messages: VecDeque<(String, String)>,
    pub(super) answer: String,
    pub(super) usage: Usage,
    pub(super) last_prompt_tokens: u32,
}

impl Projection {
    pub(super) async fn delta(
        &mut self,
        tx: &mpsc::Sender<AgentEvent>,
        id: &str,
        channel: StreamBlockChannel,
        delta: &str,
    ) -> Result<(), CoreError> {
        if delta.is_empty() || self.completed.contains(id) {
            return Ok(());
        }
        if self.offsets.len() > 2048 {
            return Err(protocol_error(
                "upstream output exceeded the bounded event protocol",
            ));
        }
        let offset = self.offsets.entry(id.to_string()).or_default();
        if *offset + delta.len() > 4 * 1024 * 1024 {
            return Err(protocol_error(
                "upstream output exceeded the bounded event protocol",
            ));
        }
        if channel == StreamBlockChannel::Answer && self.draft_bytes + delta.len() > 4 * 1024 * 1024
        {
            return Err(protocol_error(
                "subscription answer history exceeded its byte budget",
            ));
        }
        tx.send(AgentEvent::StreamBlockDelta {
            block_id: id.to_string(),
            channel,
            offset: *offset,
            delta: delta.to_string(),
        })
        .await
        .map_err(protocol_error)?;
        *offset += delta.len();
        if channel == StreamBlockChannel::Answer {
            if !self.drafts.contains_key(id) {
                self.draft_order.push(id.to_string());
            }
            self.drafts
                .entry(id.to_string())
                .or_default()
                .push_str(delta);
            self.draft_bytes += delta.len();
        }
        Ok(())
    }

    pub(super) async fn complete(
        &mut self,
        tx: &mpsc::Sender<AgentEvent>,
        id: &str,
        text: &str,
    ) -> Result<(), CoreError> {
        if !self.completed.contains(id) && self.completed.len() >= 2048 {
            return Err(protocol_error(
                "subscription completed-block budget exceeded",
            ));
        }
        if text.len() > 4 * 1024 * 1024 {
            return Err(protocol_error(
                "upstream answer exceeded the bounded event protocol",
            ));
        }
        if !self.completed.contains(id) {
            let offset = self.offsets.get(id).copied().unwrap_or(0);
            // Full records restore missing ephemeral deltas after subscriber
            // lag. Done remains the authority if the upstream revised text.
            if let Some(suffix) = text.get(offset..) {
                self.delta(tx, id, StreamBlockChannel::Answer, suffix)
                    .await?;
            }
            self.completed.insert(id.to_string());
        }
        self.answer = text.to_string();
        let previous = self.drafts.get(id).map_or(0, String::len);
        if self.draft_bytes - previous + text.len() > 4 * 1024 * 1024 {
            return Err(protocol_error(
                "subscription answer history exceeded its byte budget",
            ));
        }
        if !self.drafts.contains_key(id) {
            self.draft_order.push(id.to_string());
        }
        self.draft_bytes = self.draft_bytes - previous + text.len();
        self.drafts.insert(id.to_string(), text.to_string());
        Ok(())
    }

    pub(super) fn mark_persisted(&mut self, id: &str) {
        if let Some(text) = self.drafts.remove(id) {
            self.draft_bytes -= text.len();
        }
        self.draft_order.retain(|key| key != id);
    }

    pub(super) fn queue_async_message(&mut self, id: String, text: String) {
        self.async_messages.push_back((id, text));
    }

    /// Called only at a tool boundary; the app-server reader must remain free
    /// to answer native clock requests while a tool waits for approval.
    pub(super) async fn flush_async_messages(
        &mut self,
        turn: &PreparedTurn,
    ) -> Result<(), CoreError> {
        while let Some((id, text)) = self.async_messages.front() {
            turn.tools.persist_answer(text).await?;
            let id = id.clone();
            self.async_messages.pop_front();
            self.mark_persisted(&id);
        }
        Ok(())
    }

    pub(super) async fn persist_partial(&mut self, turn: &PreparedTurn) -> Result<(), CoreError> {
        self.flush_async_messages(turn).await?;
        let text = self
            .draft_order
            .iter()
            .filter_map(|id| self.drafts.get(id))
            .filter(|text| !text.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        if !text.is_empty() {
            turn.tools.persist_answer(&text).await?;
        }
        Ok(())
    }

    pub(super) async fn finish(
        mut self,
        turn: &PreparedTurn,
    ) -> Result<nexa_core::llm::Message, CoreError> {
        self.flush_async_messages(turn).await?;
        if self.answer.trim().is_empty() {
            return Err(protocol_error("upstream completed without a final answer"));
        }
        let message = turn.tools.persist_answer(&self.answer).await?;
        turn.events
            .send(AgentEvent::Done {
                message: message.clone(),
                last_prompt_tokens: self.last_prompt_tokens,
                usage_total: self.usage,
                context_breakdown: None,
                cached: false,
                finish_reason: Some("stop".into()),
            })
            .await
            .map_err(protocol_error)?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn disconnected_answer_deltas_survive_reload_without_reasoning_or_duplicate_questions() {
        let (request, mut rx, _, _) =
            super::super::tests::fixture(super::super::SubscriptionRuntimeKind::Codex, "test");
        let db = request.db.clone();
        let conversation = request.conversation_id.clone();
        let turn = request.prepare(false).unwrap();
        let mut projection = Projection::default();
        projection
            .delta(
                &turn.events,
                "thought",
                StreamBlockChannel::Thinking,
                "private reasoning",
            )
            .await
            .unwrap();
        projection
            .delta(
                &turn.events,
                "question",
                StreamBlockChannel::Answer,
                "already stored question",
            )
            .await
            .unwrap();
        projection.mark_persisted("question");
        projection
            .delta(&turn.events, "answer", StreamBlockChannel::Answer, "半个")
            .await
            .unwrap();
        projection
            .delta(&turn.events, "answer", StreamBlockChannel::Answer, "回答")
            .await
            .unwrap();
        assert!(
            projection.answer.is_empty(),
            "there was no full-message event"
        );
        projection.persist_partial(&turn).await.unwrap();
        assert_eq!(
            db.get_messages(&conversation)
                .unwrap()
                .last()
                .unwrap()
                .content,
            "半个回答"
        );
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::Done { .. }));
        }
    }
}
