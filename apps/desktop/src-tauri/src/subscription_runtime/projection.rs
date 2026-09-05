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
    answer_block_ids: Vec<String>,
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
        self.complete_block(tx, id, text).await?;
        self.select_answer_blocks(vec![id.to_string()])
    }

    pub(super) fn select_answer_blocks(&mut self, ids: Vec<String>) -> Result<(), CoreError> {
        let mut parts = Vec::new();
        for id in &ids {
            if !self.completed.contains(id) {
                return Err(protocol_error(
                    "answer response contains an incomplete block",
                ));
            }
            let text = self
                .drafts
                .get(id)
                .ok_or_else(|| protocol_error("answer response block is missing"))?;
            if !text.is_empty() {
                parts.push(text.as_str());
            }
        }
        let answer = parts.join("\n\n");
        if answer.len() > 4 * 1024 * 1024 {
            return Err(protocol_error("assembled response exceeds its byte budget"));
        }
        self.answer = answer;
        self.answer_block_ids = ids;
        Ok(())
    }

    pub(super) async fn complete_block(
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
        let draft = self.drafts.get(id).map(String::as_str).unwrap_or_default();
        let previous = draft.len();
        if self.draft_bytes - previous + text.len() > 4 * 1024 * 1024 {
            return Err(protocol_error(
                "subscription answer history exceeded its byte budget",
            ));
        }
        // A byte count alone cannot prove that the full record extends the
        // observed deltas: a middle delta may have been lost or text revised.
        if !self.completed.contains(id) && text.starts_with(draft) {
            self.delta(tx, id, StreamBlockChannel::Answer, &text[previous..])
                .await?;
            // delta already charged the suffix to the draft byte budget.
        } else if draft != text {
            tx.send(AgentEvent::StreamBlockSnapshot {
                block_id: id.to_string(),
                channel: StreamBlockChannel::Answer,
                text: text.to_string(),
            })
            .await
            .map_err(protocol_error)?;
        }
        self.completed.insert(id.to_string());
        self.offsets.insert(id.to_string(), text.len());
        let previous = self.drafts.get(id).map_or(0, String::len);
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

    pub(super) fn clear_answer(&mut self) {
        self.answer.clear();
        self.answer_block_ids.clear();
    }

    /// Commit a completed response before a new user input changes the turn.
    /// It is then excluded from failure recovery to avoid a duplicate reply.
    pub(super) async fn persist_completed_answer(
        &mut self,
        turn: &PreparedTurn,
    ) -> Result<Option<nexa_core::agent::PersistedAssistantMessage>, CoreError> {
        self.flush_async_messages(turn).await?;
        if self.answer.trim().is_empty() {
            return Ok(None);
        }
        let message = turn.tools.persist_answer(&self.answer).await?;
        for id in std::mem::take(&mut self.answer_block_ids) {
            self.mark_persisted(&id);
        }
        self.clear_answer();
        Ok(Some(message))
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
        let message = self
            .persist_completed_answer(turn)
            .await?
            .ok_or_else(|| protocol_error("upstream completed without a final answer"))?;
        turn.events
            .send(AgentEvent::Done {
                message: message.message.clone(),
                last_prompt_tokens: self.last_prompt_tokens,
                usage_total: self.usage,
                context_breakdown: None,
                assistant_message_id: Some(message.id),
                cached: false,
                finish_reason: Some("stop".into()),
            })
            .await
            .map_err(protocol_error)?;
        Ok(message.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn completed_response_precedes_steering_and_is_not_duplicated_on_failure() {
        let (request, mut rx, _, _) =
            super::super::tests::fixture(super::super::SubscriptionRuntimeKind::Copilot, "test");
        let db = request.db.clone();
        let conversation = request.conversation_id.clone();
        let turn = request.prepare(false).unwrap();
        let mut projection = Projection::default();
        projection
            .complete(&turn.events, "first", "first response")
            .await
            .unwrap();
        assert!(projection
            .persist_completed_answer(&turn)
            .await
            .unwrap()
            .is_some());
        assert!(projection
            .persist_completed_answer(&turn)
            .await
            .unwrap()
            .is_none());
        turn.tools
            .persist_steering(&nexa_core::agent::AgentSteeringMessage::text("follow up"))
            .await
            .unwrap();
        projection
            .delta(
                &turn.events,
                "second",
                StreamBlockChannel::Answer,
                "partial follow-up",
            )
            .await
            .unwrap();
        projection.persist_partial(&turn).await.unwrap();
        let history = db.get_messages(&conversation).unwrap();
        assert_eq!(
            history[1..]
                .iter()
                .map(|message| (message.role.clone(), message.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (nexa_core::llm::Role::Assistant, "first response"),
                (nexa_core::llm::Role::User, "follow up"),
                (nexa_core::llm::Role::Assistant, "partial follow-up"),
            ]
        );
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, AgentEvent::Done { .. }),
                "checkpointing is not a terminal event"
            );
        }
    }

    #[tokio::test]
    async fn full_records_replace_corrupt_prefixes_and_repeated_revisions() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut projection = Projection::default();
        projection
            .delta(&tx, "a", StreamBlockChannel::Answer, "你错")
            .await
            .unwrap();
        projection.complete(&tx, "a", "你好🙂").await.unwrap();
        projection.complete(&tx, "a", "修正🙂").await.unwrap();
        assert_eq!(projection.drafts["a"], "修正🙂");
        assert_eq!(projection.draft_bytes, "修正🙂".len());
        assert!(matches!(
            rx.recv().await.unwrap(),
            AgentEvent::StreamBlockDelta { .. }
        ));
        for expected in ["你好🙂", "修正🙂"] {
            let event = rx.recv().await.unwrap();
            assert!(
                matches!(&event, AgentEvent::StreamBlockSnapshot { text, .. } if text == expected)
            );
            let wire = nexa_core::agent_run::AgentRunEvent::from_agent_event(&event);
            assert_eq!(
                wire.kind,
                nexa_core::agent_run::AgentRunEventKind::OutputSnapshot
            );
            assert_eq!(wire.payload["text"], expected);
        }
        projection.complete(&tx, "a", "修正🙂").await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "identical full records are idempotent"
        );
    }

    #[tokio::test]
    async fn full_record_appends_only_a_verified_prefix_suffix() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut projection = Projection::default();
        projection
            .delta(&tx, "a", StreamBlockChannel::Answer, "你")
            .await
            .unwrap();
        projection.complete(&tx, "a", "你好🙂").await.unwrap();
        rx.recv().await.unwrap();
        assert!(
            matches!(rx.recv().await.unwrap(), AgentEvent::StreamBlockDelta { offset: 3, delta, .. } if delta == "好🙂")
        );
        assert_eq!(projection.draft_bytes, "你好🙂".len());
    }

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
