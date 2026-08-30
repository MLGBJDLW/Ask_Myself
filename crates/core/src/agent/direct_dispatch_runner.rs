//! Direct-dispatch fast path for simple unambiguous commands.

use super::*;

pub(super) enum DirectDispatchOutcome {
    NotMatched,
    ExecutedButFailed,
    Completed(Message),
}

impl AgentExecutor {
    // -----------------------------------------------------------------------
    // Direct dispatch — skip LLM for simple commands
    // -----------------------------------------------------------------------

    /// Attempt to handle the query without an LLM call by detecting simple,
    /// unambiguous command patterns. The result distinguishes a query that did
    /// not match from a tool batch that entered execution and failed, because
    /// only the latter consumes verified tool-round authority before falling
    /// through to the normal ReAct loop.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn try_direct_dispatch(
        &self,
        user_text: &str,
        db: &Database,
        source_scope: &[String],
        tx: &mpsc::Sender<AgentEvent>,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        sort_order: i64,
    ) -> DirectDispatchOutcome {
        if user_text.is_empty() {
            return DirectDispatchOutcome::NotMatched;
        }
        let model = self.config.model.as_deref().unwrap_or(DEFAULT_MODEL);

        let Some(dispatch) = direct_dispatch::match_direct_pattern(user_text, db) else {
            return DirectDispatchOutcome::NotMatched;
        };

        debug!(
            "Direct dispatch: tool={}, args={}",
            dispatch.tool_name, dispatch.arguments
        );

        let call_id = format!("direct_{}", Uuid::new_v4());

        let started_at = std::time::Instant::now();
        let _ = tx
            .send(AgentEvent::ToolRunStarted {
                run: build_tool_run_item(
                    &self.tools,
                    &call_id,
                    &dispatch.tool_name,
                    ToolRunStatus::Running,
                    Some(&dispatch.arguments),
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
            })
            .await;

        // Execute the tool directly.
        let result = self
            .tools
            .execute(
                &dispatch.tool_name,
                crate::tools::ToolExecutionContext {
                    call_id: &call_id,
                    arguments: &dispatch.arguments,
                    db,
                    source_scope,
                    conversation_id,
                    turn_id,
                    tool_registry: Some(&self.tools),
                    cancel_token: Some(&self.cancel_token),
                    activity_runtime: Some(&self.activity_runtime),
                    event_tx: Some(tx),
                },
            )
            .await;

        match result {
            Ok(tool_result) => {
                let direct_run_status = if tool_result.is_error {
                    ToolRunStatus::Failed
                } else {
                    ToolRunStatus::Completed
                };
                let _ = tx
                    .send(AgentEvent::ToolRunCompleted {
                        run: build_tool_run_item(
                            &self.tools,
                            &tool_result.call_id,
                            &dispatch.tool_name,
                            direct_run_status,
                            Some(&dispatch.arguments),
                            Some(tool_result.content.clone()),
                            Some(tool_result.is_error),
                            tool_result.artifacts.clone(),
                            None,
                            Some(started_at.elapsed().as_millis() as u64),
                        ),
                    })
                    .await;

                if tool_result.is_error {
                    // Tool returned an error — fall through to LLM for
                    // a better user-facing response.
                    return DirectDispatchOutcome::ExecutedButFailed;
                }

                // Emit the content as text so streaming listeners see it.
                let _ = tx
                    .send(AgentEvent::TextDelta {
                        delta: tool_result.content.clone(),
                    })
                    .await;

                let msg = Message::text(Role::Assistant, tool_result.content);

                // Persist the assistant message.
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
                            "routeKind": "DirectResponse",
                            "items": [{
                                "kind": "status",
                                "text": "Handled via direct dispatch without a full agent loop.",
                                "tone": "success"
                            }]
                        });
                        let _ = db.finalize_conversation_turn(
                            tid,
                            "success",
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
                        cached: false,
                        finish_reason: Some("stop".to_string()),
                    })
                    .await;

                DirectDispatchOutcome::Completed(msg)
            }
            Err(e) => {
                warn!("Direct dispatch failed ({}): {}", dispatch.tool_name, e);
                let content = format!("{} failed: {e}", dispatch.tool_name);
                let _ = tx
                    .send(AgentEvent::ToolRunCompleted {
                        run: build_tool_run_item(
                            &self.tools,
                            &call_id,
                            &dispatch.tool_name,
                            ToolRunStatus::Failed,
                            Some(&dispatch.arguments),
                            Some(content),
                            Some(true),
                            None,
                            None,
                            Some(started_at.elapsed().as_millis() as u64),
                        ),
                    })
                    .await;
                DirectDispatchOutcome::ExecutedButFailed
            }
        }
    }
}
