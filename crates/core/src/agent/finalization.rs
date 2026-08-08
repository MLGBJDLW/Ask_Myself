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
        context_breakdown: Option<context::ContextUsageBreakdown>,
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
                context_breakdown,
                cached: false,
                finish_reason: Some("cancelled".to_string()),
            })
            .await;

        if let Some(ref mut t) = trace {
            t.provider_runtime = self.provider.runtime_metadata().await;
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
        answer_delta_seen: bool,
        user_query_text: &str,
        cache_source_filter: Option<&str>,
        total_usage: Usage,
        last_prompt_tokens: u32,
        context_breakdown: Option<context::ContextUsageBreakdown>,
        last_finish_reason: Option<String>,
    ) -> Result<Message, CoreError> {
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
        let reasoning_was_promoted = !answer_delta_seen
            && assistant_reasoning_content
                .as_deref()
                .is_some_and(|reasoning| {
                    let normalized_answer = final_text.trim();
                    let normalized_reasoning = reasoning.trim();
                    !normalized_reasoning.is_empty() && normalized_answer == normalized_reasoning
                });
        if reasoning_was_promoted {
            let frontend_message = "The model finished without producing a final answer. Its reasoning was kept separate; retry the response.";
            let trace_message =
                "finalization rejected reasoning content promoted into the answer channel";
            append_persisted_trace_status(persisted_trace_items, frontend_message, "error");
            emit_error_and_finalize_turn(
                tx,
                db,
                trace,
                turn_id,
                route_kind,
                persisted_trace_items,
                TurnErrorMessages {
                    frontend_message: frontend_message.to_string(),
                    trace_message: trace_message.to_string(),
                },
            )
            .await;
            return Err(CoreError::Agent(trace_message.to_string()));
        }
        let proposed_plan_artifact = self
            .config
            .execution_mode
            .is_plan()
            .then(|| extract_proposed_plan_artifact(&final_text))
            .flatten();
        let verification_artifact = if self.config.execution_mode.is_plan() {
            serde_json::json!({
                "kind": "verification",
                "version": 1,
                "overallStatus": "passed",
                "mode": "plan",
                "summary": "Plan Mode completed without executing write tools.",
                "checks": [
                    {
                        "id": "plan-mode-read-only",
                        "label": "Read-only planning turn",
                        "status": "passed"
                    }
                ]
            })
        } else {
            audit_final_answer(
                task_plan,
                &final_text,
                evidence_signals_from_trace(persisted_trace_items),
            )
            .to_artifact()
        };
        let verification_passed = verification_artifact_passed(&verification_artifact);
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
        let verification_trace_status = if self.config.execution_mode.is_plan() {
            "Plan mode audit: read-only planning turn passed.".to_string()
        } else {
            format!(
                "Evidence audit: {}.",
                verification_artifact["overallStatus"]
                    .as_str()
                    .unwrap_or("pending")
            )
        };
        append_developer_persisted_trace_status(
            persisted_trace_items,
            &verification_trace_status,
            verification_artifact_tone(&verification_artifact),
        );

        if let Some(cid) = conversation_id {
            let assistant_message_id = Uuid::new_v4().to_string();
            let reasoning_envelope = self.reasoning_envelope_for_persistence(
                model,
                assistant_reasoning_content.as_deref(),
                assistant_reasoning_content.as_deref(),
                assistant_msg
                    .tool_calls
                    .as_ref()
                    .is_some_and(|tool_calls| !tool_calls.is_empty()),
            );
            let display_thinking = reasoning_envelope
                .as_ref()
                .and_then(|envelope| envelope.display_text.clone());
            let artifacts = merge_reasoning_envelope_artifact(
                build_assistant_artifacts(persisted_trace_items, proposed_plan_artifact.as_ref()),
                reasoning_envelope,
            );
            let conv_msg = ConversationMessage {
                id: assistant_message_id.clone(),
                conversation_id: cid.to_string(),
                role: Role::Assistant,
                content: final_text.clone(),
                tool_call_id: None,
                tool_calls: assistant_msg.tool_calls.clone().unwrap_or_default(),
                artifacts,
                token_count: estimate_message_tokens_for_model(model, &assistant_msg),
                created_at: String::new(),
                sort_order,
                thinking: display_thinking,
                image_attachments: None,
            };
            if let Err(e) = db.add_message(&conv_msg) {
                warn!("Failed to save final assistant message: {e}");
            }
            if let Some(tid) = turn_id {
                let mut trace_payload = build_turn_trace_with_verification(
                    route_kind,
                    persisted_trace_items,
                    Some(&verification_artifact),
                );
                if let Some(plan) = proposed_plan_artifact.as_ref() {
                    trace_payload["proposedPlan"] = plan.clone();
                }
                let _ = db.finalize_conversation_turn(
                    tid,
                    "success",
                    Some(&assistant_message_id),
                    Some(&trace_payload),
                );
                if let Ok(Some(task_run)) = db.get_agent_task_run_by_turn(tid) {
                    let previous_task_artifacts = db
                        .get_agent_task_run(&task_run.id)
                        .ok()
                        .and_then(|run| run.artifacts);
                    let mut task_artifacts =
                        build_task_run_artifacts(previous_task_artifacts, &verification_artifact);
                    if let Some(plan) = proposed_plan_artifact.as_ref() {
                        task_artifacts["proposedPlan"] = plan.clone();
                    }
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

        if !self.config.execution_mode.is_plan()
            && !final_text.is_empty()
            && !user_query_text.is_empty()
        {
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
                context_breakdown,
                cached: false,
                finish_reason: last_finish_reason,
            })
            .await;

        if let Some(ref mut t) = trace {
            t.provider_runtime = self.provider.runtime_metadata().await;
            t.finish(TraceOutcome::Success, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        Ok(assistant_msg)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finish_max_iterations(
        &self,
        ctx: TurnFinalizationContext<'_>,
        mut final_content: String,
        total_usage: Usage,
        last_prompt_tokens: u32,
        context_breakdown: Option<context::ContextUsageBreakdown>,
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
                context_breakdown,
                cached: false,
                finish_reason: last_finish_reason,
            })
            .await;

        if let Some(ref mut t) = trace {
            t.provider_runtime = self.provider.runtime_metadata().await;
            t.finish(TraceOutcome::MaxIterations, None);
            if let Err(e) = db.save_agent_trace(t) {
                warn!("Failed to save agent trace: {e}");
            }
        }

        final_msg
    }
}

fn verification_artifact_passed(artifact: &serde_json::Value) -> bool {
    artifact["overallStatus"].as_str() == Some("passed")
}

fn verification_artifact_tone(artifact: &serde_json::Value) -> &'static str {
    match artifact["overallStatus"].as_str() {
        Some("failed") => "error",
        Some("passed") => "info",
        _ => "warning",
    }
}

fn build_assistant_artifacts(
    trace_items: &[PersistedTraceItem],
    proposed_plan: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let trace_artifacts = build_trace_artifacts(trace_items);
    match (trace_artifacts, proposed_plan) {
        (None, None) => None,
        (Some(trace), None) => Some(trace),
        (None, Some(plan)) => Some(serde_json::json!({
            "kind": "assistantArtifacts",
            "version": 1,
            "proposedPlan": plan,
        })),
        (Some(serde_json::Value::Object(mut map)), Some(plan)) => {
            map.insert("proposedPlan".to_string(), plan.clone());
            Some(serde_json::Value::Object(map))
        }
        (Some(trace), Some(plan)) => Some(serde_json::json!({
            "kind": "assistantArtifacts",
            "version": 1,
            "trace": trace,
            "proposedPlan": plan,
        })),
    }
}

fn extract_proposed_plan_artifact(final_text: &str) -> Option<serde_json::Value> {
    let start_tag = "<proposed_plan>";
    let end_tag = "</proposed_plan>";
    let start = find_ascii_case_insensitive(final_text, start_tag)? + start_tag.len();
    let end = find_ascii_case_insensitive(&final_text[start..], end_tag)? + start;
    let markdown = final_text[start..end].trim();
    if markdown.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "kind": "proposedPlan",
        "version": 1,
        "mode": "plan",
        "title": proposed_plan_title(markdown),
        "markdown": markdown,
    }))
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn proposed_plan_title(markdown: &str) -> String {
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let without_heading = trimmed.trim_start_matches('#').trim();
        let without_number = without_heading
            .trim_start_matches(|ch: char| {
                ch.is_ascii_digit() || ch == '.' || ch == ')' || ch.is_whitespace()
            })
            .trim();
        let title = without_number.trim_matches(['*', '`', ':']);
        if !title.is_empty() {
            return title.chars().take(96).collect();
        }
    }
    "Proposed plan".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_completion_requires_passed_status() {
        assert!(verification_artifact_passed(&serde_json::json!({
            "overallStatus": "passed"
        })));
        for status in ["partial", "pending", "failed"] {
            assert!(!verification_artifact_passed(&serde_json::json!({
                "overallStatus": status
            })));
        }
    }

    #[test]
    fn extracts_proposed_plan_artifact_from_final_text() {
        let artifact = extract_proposed_plan_artifact(
            "Context.\n\n<proposed_plan>\n# Implement Plan Mode\n\n- Add readonly tools.\n</proposed_plan>",
        )
        .expect("plan artifact");

        assert_eq!(artifact["kind"].as_str(), Some("proposedPlan"));
        assert_eq!(artifact["title"].as_str(), Some("Implement Plan Mode"));
        assert!(artifact["markdown"]
            .as_str()
            .is_some_and(|text| text.contains("readonly tools")));
    }
}
