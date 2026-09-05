use super::{protocol_error, PreparedTurn};
use nexa_core::agent::{AgentEvent, StreamBlockChannel};
use nexa_core::error::CoreError;
use nexa_core::llm::Usage;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

#[derive(Default)]
pub(super) struct Projection {
    offsets: HashMap<String, usize>,
    completed: HashSet<String>,
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
        tx.send(AgentEvent::StreamBlockDelta {
            block_id: id.to_string(),
            channel,
            offset: *offset,
            delta: delta.to_string(),
        })
        .await
        .map_err(protocol_error)?;
        *offset += delta.len();
        Ok(())
    }

    pub(super) async fn complete(
        &mut self,
        tx: &mpsc::Sender<AgentEvent>,
        id: &str,
        text: &str,
    ) -> Result<(), CoreError> {
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
        Ok(())
    }

    pub(super) async fn finish(
        self,
        turn: &PreparedTurn,
    ) -> Result<nexa_core::llm::Message, CoreError> {
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
