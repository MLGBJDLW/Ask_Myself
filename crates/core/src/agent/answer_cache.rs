//! Answer-cache fast path for agent turns.

use super::*;

impl AgentExecutor {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn try_cached_answer(
        &self,
        user_query_text: &str,
        cache_source_filter: Option<&str>,
        db: &Database,
        tx: &mpsc::Sender<AgentEvent>,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        model: &str,
        sort_order: i64,
        route_kind: AgentRouteKind,
        trace: &mut Option<AgentTrace>,
    ) -> Option<Message> {
        if user_query_text.is_empty() {
            return None;
        }

        let cached = db
            .find_cached_answer(
                user_query_text,
                cache_source_filter,
                self.config.cache_ttl_hours.map(|h| h as i64),
            )
            .ok()
            .flatten()?;

        let _ = db.increment_cache_hit(&cached.id);
        debug!("Cache hit for query: {}", user_query_text);
        let _ = tx
            .send(AgentEvent::TextDelta {
                delta: cached.answer_text.clone(),
            })
            .await;
        let msg = Message::text(Role::Assistant, cached.answer_text);

        if let Some(cid) = conversation_id {
            let assistant_message_id = Uuid::new_v4().to_string();
            let conv_msg = ConversationMessage {
                id: assistant_message_id.clone(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: None,
                token_count: estimate_message_tokens_for_model(model, &msg),
                created_at: String::new(),
                sort_order,
                thinking: None,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                error!("Failed to persist message: {e}");
                let _ = tx
                    .send(AgentEvent::Status {
                        content: format!("Warning: message was not saved to history: {e}"),
                        tone: Some("warning".to_string()),
                    })
                    .await;
            }
            if let Some(tid) = turn_id {
                let trace = serde_json::json!({
                    "kind": "turnTrace",
                    "routeKind": format!("{:?}", route_kind),
                    "items": [{
                        "kind": "status",
                        "text": "Answered from cache.",
                        "tone": "success"
                    }]
                });
                let _ = db.finalize_conversation_turn(
                    tid,
                    "cached",
                    Some(&assistant_message_id),
                    Some(&trace),
                );
            }
        }

        let _ = tx
            .send(AgentEvent::Done {
                message: msg.clone(),
                usage_total: Usage::default(),
                last_prompt_tokens: 0,
                context_breakdown: None,
                cached: true,
                finish_reason: Some("stop".to_string()),
            })
            .await;

        if let Some(ref mut t) = trace {
            t.cache_hit = true;
            t.finish(TraceOutcome::Success, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        Some(msg)
    }
}
