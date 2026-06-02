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
        let mut drained = Vec::new();
        if let Some(message) = initial {
            drained.push(message);
        }

        let Some(rx) = &self.steering_rx else {
            return self.apply_steering_messages(messages, ctx, drained).await;
        };

        {
            let mut rx = rx.lock().await;
            while let Ok(message) = rx.try_recv() {
                drained.push(message);
            }
        }

        self.apply_steering_messages(messages, ctx, drained).await
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

        let _ = ctx
            .tx
            .send(AgentEvent::Status {
                content: if drained.len() == 1 {
                    "Steering message received; applying it to the next agent step.".to_string()
                } else {
                    format!(
                        "{} steering messages received; applying them to the next agent step.",
                        drained.len()
                    )
                },
                tone: Some("muted".to_string()),
            })
            .await;

        let mut steering_texts = Vec::with_capacity(drained.len());
        for steering in drained {
            let text = steering.content.trim().to_string();
            if text.is_empty() && steering.parts.is_empty() {
                continue;
            }

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
        let layout = prompt_layout::PromptLayout::for_provider(self.config.provider_type);
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
        has_tool_calls: bool,
    ) -> Option<String> {
        if !iteration_thinking.is_empty() {
            return Some(iteration_thinking.to_string());
        }
        if has_tool_calls
            && self.config.reasoning_enabled.unwrap_or(false)
            && matches!(self.config.provider_type, Some(ProviderType::DeepSeek))
        {
            return Some(MISSING_REASONING_CONTENT_PLACEHOLDER.to_string());
        }
        None
    }
}
