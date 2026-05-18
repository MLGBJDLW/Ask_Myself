//! Turn finalization, verification, persistence, and terminal events.

use super::*;

pub(super) struct TurnFinalizationContext<'a> {
    pub(super) db: &'a Database,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) model: &'a str,
    pub(super) route_kind: AgentRouteKind,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) task_plan: &'a mut AgentTaskPlan,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) sort_order: i64,
}

pub(super) struct CancellationFinalizationContext<'a> {
    pub(super) db: &'a Database,
    pub(super) tx: &'a mpsc::Sender<AgentEvent>,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) turn_id: Option<&'a str>,
    pub(super) model: &'a str,
    pub(super) route_kind: AgentRouteKind,
    pub(super) persisted_trace_items: &'a mut Vec<PersistedTraceItem>,
    pub(super) loop_recorder: &'a mut TurnLoopRecorder,
    pub(super) trace: &'a mut Option<AgentTrace>,
    pub(super) sort_order: &'a mut i64,
}

impl AgentExecutor {
    pub(super) async fn finish_cancelled_turn(
        &self,
        ctx: CancellationFinalizationContext<'_>,
        pending_tool_calls: Option<&[ToolCallRequest]>,
        accumulated_content: &mut String,
        total_usage: Usage,
        last_prompt_tokens: u32,
    ) -> Message {
        let CancellationFinalizationContext {
            db,
            tx,
            conversation_id,
            turn_id,
            model,
            route_kind,
            persisted_trace_items,
            loop_recorder,
            trace,
            sort_order,
        } = ctx;

        if let Some(cid) = conversation_id {
            if let Some(pending) = pending_tool_calls {
                for tc in pending {
                    let synthetic = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: cid.to_string(),
                        role: Role::Tool,
                        content: format!(
                            "Error: tool '{}' was interrupted (cancelled by user).",
                            tc.name
                        ),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                        artifacts: None,
                        token_count: 15,
                        created_at: String::new(),
                        sort_order: *sort_order,
                        thinking: None,
                        image_attachments: None,
                    };
                    if let Err(e) = db.add_message(&synthetic) {
                        warn!("Failed to insert synthetic tool response on cancel: {e}");
                    }
                    *sort_order += 1;
                }
            }
        }

        if !accumulated_content.is_empty() {
            let note = "\n\n*[Request cancelled by user]*";
            let _ = tx
                .send(AgentEvent::TextDelta {
                    delta: note.to_string(),
                })
                .await;
            accumulated_content.push_str(note);
        }
        let cancel_text = if accumulated_content.is_empty() {
            "Request cancelled by user.".to_string()
        } else {
            accumulated_content.clone()
        };
        let final_msg = Message::text(Role::Assistant, cancel_text);
        append_persisted_trace_status(persisted_trace_items, "Request cancelled by user.", "error");
        let finished = TurnLoopEvent::TurnFinished {
            outcome: "cancelled".to_string(),
        };
        loop_recorder.record(finished.clone());
        append_persisted_trace_loop_event(persisted_trace_items, finished);

        if let Some(cid) = conversation_id {
            let assistant_message_id = Uuid::new_v4().to_string();
            let conv_msg = ConversationMessage {
                id: assistant_message_id.clone(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: build_trace_artifacts(persisted_trace_items),
                token_count: estimate_message_tokens_for_model(model, &final_msg),
                created_at: String::new(),
                sort_order: *sort_order,
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
                let trace_payload = build_turn_trace(route_kind, persisted_trace_items);
                let _ = db.finalize_conversation_turn(
                    tid,
                    "cancelled",
                    Some(&assistant_message_id),
                    Some(&trace_payload),
                );
            }
        }

        let _ = tx
            .send(AgentEvent::Done {
                message: final_msg.clone(),
                usage_total: total_usage,
                last_prompt_tokens,
                cached: false,
                finish_reason: Some("cancelled".to_string()),
            })
            .await;

        if let Some(ref mut t) = trace {
            t.finish(TraceOutcome::Cancelled, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        final_msg
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finish_successful_turn(
        &self,
        ctx: TurnFinalizationContext<'_>,
        assistant_msg: Message,
        assistant_reasoning_content: Option<String>,
        user_query_text: &str,
        cache_source_filter: Option<&str>,
        total_usage: Usage,
        last_prompt_tokens: u32,
        last_finish_reason: Option<String>,
    ) -> Message {
        let TurnFinalizationContext {
            db,
            tx,
            conversation_id,
            turn_id,
            model,
            route_kind,
            persisted_trace_items,
            task_plan,
            loop_recorder,
            trace,
            sort_order,
        } = ctx;

        let final_text = assistant_msg.text_content();
        let evidence_audit = audit_final_answer(
            task_plan,
            &final_text,
            evidence_signals_from_trace(persisted_trace_items),
        );
        let verification_artifact = evidence_audit.to_artifact();
        let verification_passed = verification_artifact["overallStatus"].as_str() != Some("failed");
        if finalize_task_plan(task_plan, verification_passed) {
            emit_task_plan_update(
                tx,
                task_plan,
                "finalizing",
                if verification_passed {
                    "Execution plan completed"
                } else {
                    "Execution plan stopped with a verification gap"
                },
            )
            .await;
        }
        append_persisted_trace_status(
            persisted_trace_items,
            &format!(
                "Evidence audit: {}.",
                verification_artifact["overallStatus"]
                    .as_str()
                    .unwrap_or("pending")
            ),
            if verification_artifact["overallStatus"].as_str() == Some("failed") {
                "error"
            } else {
                "info"
            },
        );

        if let Some(cid) = conversation_id {
            let assistant_message_id = Uuid::new_v4().to_string();
            let conv_msg = ConversationMessage {
                id: assistant_message_id.clone(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_text.clone(),
                tool_call_id: None,
                tool_calls: assistant_msg.tool_calls.clone().unwrap_or_default(),
                artifacts: build_trace_artifacts(persisted_trace_items),
                token_count: estimate_message_tokens_for_model(model, &assistant_msg),
                created_at: String::new(),
                sort_order,
                thinking: assistant_reasoning_content,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save final assistant message: {e}");
            }
            if let Some(tid) = turn_id {
                let trace_payload = build_turn_trace_with_verification(
                    route_kind,
                    persisted_trace_items,
                    Some(&verification_artifact),
                );
                let _ = db.finalize_conversation_turn(
                    tid,
                    "success",
                    Some(&assistant_message_id),
                    Some(&trace_payload),
                );
                if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                    let task_artifacts = build_task_run_artifacts(&verification_artifact);
                    let _ = db.update_agent_task_run_progress(
                        &task_run.id,
                        Some("running"),
                        Some("finalizing"),
                        Some(route_kind.as_str()),
                        Some("Final evidence audit completed"),
                        None,
                        Some(&task_artifacts),
                    );
                    let timeline_event = TaskTimelineEvent::verification(
                        "Evidence audit completed",
                        verification_artifact["overallStatus"].as_str(),
                        Some(&verification_artifact),
                    );
                    let _ = AgentTaskRuntime::new(db)
                        .record_timeline_event(&task_run.id, &timeline_event);
                }
            }
        }

        if !final_text.is_empty() && !user_query_text.is_empty() {
            let citations = crate::cache::extract_citations(&final_text);
            if !citations.is_empty() {
                let _ = db.cache_answer(
                    user_query_text,
                    &final_text,
                    &citations,
                    cache_source_filter,
                );
            }
        }

        let finished = TurnLoopEvent::TurnFinished {
            outcome: "success".to_string(),
        };
        loop_recorder.record(finished.clone());
        append_persisted_trace_loop_event(persisted_trace_items, finished);

        let _ = tx
            .send(AgentEvent::Done {
                message: assistant_msg.clone(),
                usage_total: total_usage,
                last_prompt_tokens,
                cached: false,
                finish_reason: last_finish_reason,
            })
            .await;

        if let Some(ref mut t) = trace {
            t.finish(TraceOutcome::Success, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        assistant_msg
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finish_max_iterations(
        &self,
        ctx: TurnFinalizationContext<'_>,
        mut final_content: String,
        total_usage: Usage,
        last_prompt_tokens: u32,
        last_finish_reason: Option<String>,
    ) -> Message {
        let TurnFinalizationContext {
            db,
            tx,
            conversation_id,
            turn_id,
            model,
            route_kind,
            persisted_trace_items,
            task_plan,
            loop_recorder,
            trace,
            sort_order,
        } = ctx;

        if !final_content.is_empty() {
            let note = "\n\n*[Note: I used all available tool calls. The answer above may be incomplete.]*";
            let _ = tx
                .send(AgentEvent::TextDelta {
                    delta: note.to_string(),
                })
                .await;
            final_content.push_str(note);
        }

        let final_msg = Message::text(Role::Assistant, final_content);
        append_persisted_trace_status(
            persisted_trace_items,
            "Reached maximum iterations before producing a final answer.",
            "error",
        );
        if finalize_task_plan(task_plan, false) {
            emit_task_plan_update(
                tx,
                task_plan,
                "finalizing",
                "Execution plan stopped at max iterations",
            )
            .await;
        }
        let finished = TurnLoopEvent::TurnFinished {
            outcome: "max_iterations".to_string(),
        };
        loop_recorder.record(finished.clone());
        append_persisted_trace_loop_event(persisted_trace_items, finished);

        if let Some(cid) = conversation_id {
            let assistant_message_id = Uuid::new_v4().to_string();
            let conv_msg = ConversationMessage {
                id: assistant_message_id.clone(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_msg.text_content(),
                tool_call_id: None,
                tool_calls: vec![],
                artifacts: build_trace_artifacts(persisted_trace_items),
                token_count: estimate_message_tokens_for_model(model, &final_msg),
                created_at: String::new(),
                sort_order,
                thinking: None,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save final assistant message: {e}");
            }
            if let Some(tid) = turn_id {
                let trace_payload = build_turn_trace(route_kind, persisted_trace_items);
                let _ = db.finalize_conversation_turn(
                    tid,
                    "max_iterations",
                    Some(&assistant_message_id),
                    Some(&trace_payload),
                );
            }
        }

        let _ = tx
            .send(AgentEvent::Done {
                message: final_msg.clone(),
                usage_total: total_usage,
                last_prompt_tokens,
                cached: false,
                finish_reason: last_finish_reason,
            })
            .await;

        if let Some(ref mut t) = trace {
            t.finish(TraceOutcome::MaxIterations, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        final_msg
    }
}
