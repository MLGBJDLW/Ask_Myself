//! Steering-message integration for active agent turns.

use super::*;

pub(super) struct SteeringDrainContext<'a> {
    pub(super) db: &'a Database,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) model: &'a str,
    pub(super) sort_order: &'a mut i64,
    pub(super) privacy_cfg: &'a privacy::PrivacyConfig,
}

fn steering_status_preview(text: &str) -> String {
    const MAX_CHARS: usize = 280;

    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_CHARS {
        return compact;
    }

    let mut preview = compact.chars().take(MAX_CHARS).collect::<String>();
    preview.push('…');
    preview
}

fn steering_status_text(text: &str, has_parts: bool) -> String {
    let preview = steering_status_preview(text);
    if preview.is_empty() {
        if has_parts {
            "Attached context received.".to_string()
        } else {
            "Steering received.".to_string()
        }
    } else {
        preview
    }
}

impl AgentExecutor {
    pub(super) async fn drain_steering_messages(
        &self,
        messages: &mut Vec<Message>,
        ctx: &mut SteeringDrainContext<'_>,
    ) -> Vec<String> {
        self.drain_steering_messages_from(messages, ctx, None).await
    }

    pub(super) async fn drain_steering_messages_from(
        &self,
        messages: &mut Vec<Message>,
        ctx: &mut SteeringDrainContext<'_>,
        initial: Option<AgentSteeringMessage>,
    ) -> Vec<String> {
        let drained = self.collect_steering_messages(initial).await;
        self.apply_steering_messages(messages, ctx, drained).await
    }

    pub(super) async fn collect_steering_messages(
        &self,
        initial: Option<AgentSteeringMessage>,
    ) -> Vec<AgentSteeringMessage> {
        let mut drained = Vec::new();
        if let Some(message) = initial {
            drained.push(message);
        }

        let Some(rx) = &self.steering_rx else {
            return drained;
        };

        {
            let mut rx = rx.lock().await;
            while let Ok(message) = rx.try_recv() {
                drained.push(message);
            }
        }

        drained
    }

    pub(super) fn steering_message_has_effective_content(message: &AgentSteeringMessage) -> bool {
        !message.content.trim().is_empty() || !message.parts.is_empty()
    }

    pub(super) async fn wait_for_steering_message(&self) -> Option<AgentSteeringMessage> {
        let Some(rx) = &self.steering_rx else {
            return std::future::pending::<Option<AgentSteeringMessage>>().await;
        };

        let mut rx = rx.lock().await;
        rx.recv().await
    }

    pub(super) async fn apply_steering_messages(
        &self,
        messages: &mut Vec<Message>,
        ctx: &mut SteeringDrainContext<'_>,
        drained: Vec<AgentSteeringMessage>,
    ) -> Vec<String> {
        if drained.is_empty() {
            return Vec::new();
        }

        if drained.len() > 1 {
            let _ = ctx
                .tx
                .send(AgentEvent::Status {
                    content: format!(
                        "{} steering messages received; applying them to the next agent step.",
                        drained.len()
                    ),
                    tone: Some("muted".to_string()),
                })
                .await;
        }

        let mut steering_texts = Vec::with_capacity(drained.len());
        for steering in drained {
            if steering.recovery_control.is_some() {
                continue;
            }
            let text = steering.content.trim().to_string();
            if text.is_empty() && steering.parts.is_empty() {
                continue;
            }

            let _ = ctx
                .tx
                .send(AgentEvent::Steering {
                    content: steering_status_text(&text, !steering.parts.is_empty()),
                })
                .await;

            if let Some(cid) = ctx.conversation_id {
                let conv_msg = ConversationMessage {
                    id: Uuid::new_v4().to_string(),
                    conversation_id: cid.to_string(),
                    role: Role::User,
                    content: text.clone(),
                    tool_call_id: None,
                    tool_calls: vec![],
                    artifacts: Some(serde_json::json!({ "kind": "steering" })),
                    token_count: estimate_tokens_for_model(ctx.model, &text),
                    created_at: String::new(),
                    sort_order: *ctx.sort_order,
                    thinking: None,
                    image_attachments: steering.image_attachments.clone(),
                };
                if let Err(e) = ctx.db.add_message(&conv_msg) {
                    warn!("Failed to save steering message: {e}");
                } else {
                    *ctx.sort_order += 1;
                }
            }

            let mut parts = if steering.parts.is_empty() {
                vec![ContentPart::Text { text: text.clone() }]
            } else {
                steering.parts
            };
            if ctx.privacy_cfg.enabled {
                for part in &mut parts {
                    if let ContentPart::Text { text } = part {
                        *text = privacy::redact_content(text, &ctx.privacy_cfg.redact_patterns);
                    }
                }
            }
            messages.push(Message {
                role: Role::User,
                parts,
                name: None,
                tool_calls: None,
                reasoning_content: None,
                prompt_cache_hint: None,
            });
            steering_texts.push(text);
        }

        steering_texts
    }

    pub(super) fn expand_tool_defs_for_steering(
        &self,
        tool_defs: &mut Vec<ToolDefinition>,
        steering_texts: &[String],
        has_sources: bool,
    ) {
        let layout = prompt_layout::PromptLayout::for_request(
            self.config.provider_type,
            self.config.model.as_deref(),
        );
        if !layout.effective_dynamic_tool_visibility(self.config.dynamic_tool_visibility) {
            return;
        }

        for text in steering_texts {
            if text.trim().is_empty() {
                continue;
            }
            let selected = self.tools.select_tools(text, has_sources);
            if selected.is_empty() {
                continue;
            }
            *tool_defs = merge_tool_definitions(std::mem::take(tool_defs), selected);
        }
    }

    pub(super) fn reasoning_content_for_iteration(
        &self,
        iteration_thinking: &str,
        _has_tool_calls: bool,
    ) -> Option<String> {
        crate::llm::reasoning_replay::sanitize_reasoning_text(Some(iteration_thinking))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_status_preview_compacts_whitespace() {
        assert_eq!(
            steering_status_preview("  please\n\tchange   direction  "),
            "please change direction"
        );
    }

    #[test]
    fn steering_status_preview_truncates_long_text() {
        let long = "a".repeat(400);
        let preview = steering_status_preview(&long);
        assert_eq!(preview.chars().count(), 281);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn steering_status_text_mentions_attached_context() {
        assert_eq!(steering_status_text("", true), "Attached context received.");
    }
}
