use super::*;
use crate::agent_stream_bridge::{event_has_visible_token, event_marks_provider_response_byte};

pub(super) struct SubagentEventPumpConfig {
    pub(super) cancel_token: CancellationToken,
    pub(super) worker_actual_token_limit: Option<u32>,
    pub(super) telemetry_db: Database,
    pub(super) telemetry_identity: Option<(String, String)>,
    pub(super) telemetry_call_label: String,
    pub(super) lifecycle: Option<SubagentEventBridge>,
    pub(super) launch_started: Instant,
    pub(super) non_streaming_completion: bool,
}

pub(super) struct SubagentEventPumpHandle {
    pub(super) task: tokio::task::JoinHandle<EventCapture>,
    pub(super) fatal_error_rx: mpsc::UnboundedReceiver<String>,
    pub(super) provider_connected_rx: mpsc::Receiver<()>,
    pub(super) first_response_rx: mpsc::Receiver<()>,
}

pub(super) struct SubagentEventPump;

impl SubagentEventPump {
    pub(super) fn spawn(
        event_rx: mpsc::Receiver<AgentEvent>,
        config: SubagentEventPumpConfig,
    ) -> SubagentEventPumpHandle {
        let (fatal_error_tx, fatal_error_rx) = mpsc::unbounded_channel::<String>();
        let (provider_connected_tx, provider_connected_rx) = mpsc::channel::<()>(1);
        let (first_response_tx, first_response_rx) = mpsc::channel::<()>(1);
        let task = tokio::spawn(Self::run(
            event_rx,
            fatal_error_tx,
            provider_connected_tx,
            first_response_tx,
            config,
        ));
        SubagentEventPumpHandle {
            task,
            fatal_error_rx,
            provider_connected_rx,
            first_response_rx,
        }
    }

    async fn run(
        event_rx: mpsc::Receiver<AgentEvent>,
        fatal_error_tx: mpsc::UnboundedSender<String>,
        provider_connected_tx: mpsc::Sender<()>,
        first_response_tx: mpsc::Sender<()>,
        config: SubagentEventPumpConfig,
    ) -> EventCapture {
        let SubagentEventPumpConfig {
            cancel_token: capture_cancel_token,
            worker_actual_token_limit,
            telemetry_db,
            telemetry_identity,
            telemetry_call_label,
            lifecycle: lifecycle_capture,
            launch_started,
            non_streaming_completion,
        } = config;
        let mut event_rx = event_rx;
        let mut capture = EventCapture::default();
        let mut provider_invocation_index = 0_u32;
        let mut active_provider_invocation_id: Option<String> = None;
        let mut first_provider_byte_recorded = false;
        let mut first_visible_token_recorded = false;
        let mut pending_thinking = String::new();
        let mut pending_output = String::new();
        let mut last_delta_flush = Instant::now();
        let mut worker_token_limit_exceeded = false;
        loop {
            let event =
                match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => {
                        flush_subagent_deltas(
                            lifecycle_capture.as_ref(),
                            &mut pending_thinking,
                            &mut pending_output,
                        )
                        .await;
                        break;
                    }
                    Err(_) => {
                        flush_subagent_deltas(
                            lifecycle_capture.as_ref(),
                            &mut pending_thinking,
                            &mut pending_output,
                        )
                        .await;
                        last_delta_flush = Instant::now();
                        continue;
                    }
                };
            let provider_connected = matches!(
                &event,
                AgentEvent::ControllerStatus { code, .. } if code == "provider_connected"
            );
            if provider_connected {
                emit_subagent_lifecycle_event(
                    lifecycle_capture.as_ref(),
                    SubagentLifecycleEventKind::Connected,
                    serde_json::json!({ "providerConnected": true }),
                )
                .await;
                provider_invocation_index = provider_invocation_index.saturating_add(1);
                let invocation_id = format!(
                    "subagent-provider:{}:{}",
                    telemetry_identity
                        .as_ref()
                        .map(|(_, subtask_id)| subtask_id.as_str())
                        .unwrap_or("detached"),
                    provider_invocation_index
                );
                active_provider_invocation_id = Some(invocation_id.clone());
                first_provider_byte_recorded = false;
                first_visible_token_recorded = false;
                if let Some((parent_run_id, subtask_run_id)) = telemetry_identity.as_ref() {
                    record_subagent_launch_metric(
                        &telemetry_db,
                        parent_run_id,
                        subtask_run_id,
                        &telemetry_call_label,
                        "provider_connect_ms",
                        Some(instant_elapsed_ms(launch_started)),
                        Some(&invocation_id),
                        if non_streaming_completion {
                            "completion_boundary"
                        } else {
                            "measured"
                        },
                    );
                }
                signal_progress_latch(&provider_connected_tx);
            }
            if event_marks_provider_response_byte(&event) && active_provider_invocation_id.is_some()
            {
                signal_progress_latch(&first_response_tx);
                if !first_provider_byte_recorded {
                    first_provider_byte_recorded = true;
                    if let (Some((parent_run_id, subtask_run_id)), Some(provider_invocation_id)) = (
                        telemetry_identity.as_ref(),
                        active_provider_invocation_id.as_deref(),
                    ) {
                        let elapsed_ms = instant_elapsed_ms(launch_started);
                        record_subagent_launch_metric(
                            &telemetry_db,
                            parent_run_id,
                            subtask_run_id,
                            &telemetry_call_label,
                            "first_sse_byte_ms",
                            (!non_streaming_completion).then_some(elapsed_ms),
                            Some(provider_invocation_id),
                            if non_streaming_completion {
                                "not_applicable_completion_mode"
                            } else {
                                "measured"
                            },
                        );
                    }
                }
            }
            if event_has_visible_token(&event)
                && active_provider_invocation_id.is_some()
                && !first_visible_token_recorded
            {
                first_visible_token_recorded = true;
                if let (Some((parent_run_id, subtask_run_id)), Some(provider_invocation_id)) = (
                    telemetry_identity.as_ref(),
                    active_provider_invocation_id.as_deref(),
                ) {
                    let elapsed_ms = instant_elapsed_ms(launch_started);
                    record_subagent_launch_metric(
                        &telemetry_db,
                        parent_run_id,
                        subtask_run_id,
                        &telemetry_call_label,
                        "first_visible_token_ms",
                        Some(elapsed_ms),
                        Some(provider_invocation_id),
                        "measured",
                    );
                    record_subagent_launch_metric(
                        &telemetry_db,
                        parent_run_id,
                        subtask_run_id,
                        &telemetry_call_label,
                        "frontend_first_paint_ms",
                        None,
                        Some(provider_invocation_id),
                        "not_applicable_background_worker",
                    );
                }
            }
            let should_flush_before_event = match &event {
                AgentEvent::Thinking { .. } => !pending_output.is_empty(),
                AgentEvent::TextDelta { .. } => !pending_thinking.is_empty(),
                _ => !pending_thinking.is_empty() || !pending_output.is_empty(),
            };
            if should_flush_before_event {
                flush_subagent_deltas(
                    lifecycle_capture.as_ref(),
                    &mut pending_thinking,
                    &mut pending_output,
                )
                .await;
                last_delta_flush = Instant::now();
            }
            match event {
                AgentEvent::ToolCallStart {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    let capture_detail = serde_json::json!({
                        "phase": "start",
                        "callId": &call_id,
                        "toolName": &tool_name,
                        "arguments": &arguments,
                    });
                    capture.tool_events.push(capture_detail);
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::ToolStarted,
                        serde_json::json!({
                            "phase": "start",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": arguments.len(),
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallResult {
                    call_id,
                    tool_name,
                    content,
                    is_error,
                    artifacts,
                } => {
                    let capture_detail = serde_json::json!({
                        "phase": "result",
                        "callId": &call_id,
                        "toolName": &tool_name,
                        "content": &content,
                        "isError": is_error,
                        "artifacts": &artifacts,
                    });
                    capture.tool_events.push(capture_detail);
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "result",
                            "callId": call_id,
                            "toolName": tool_name,
                            "contentBytes": content.len(),
                            "isError": is_error,
                            "hasArtifacts": artifacts.is_some(),
                        }),
                    )
                    .await;
                }
                AgentEvent::Thinking { content } => {
                    if !content.trim().is_empty() {
                        pending_thinking.push_str(&content);
                        capture.thinking.push(content);
                    }
                }
                AgentEvent::Status { content, tone } => {
                    if !content.trim().is_empty() {
                        let detail = serde_json::json!({
                            "phase": "status",
                            "content": content,
                            "tone": tone,
                        });
                        capture.tool_events.push(detail.clone());
                        emit_subagent_lifecycle_event(
                            lifecycle_capture.as_ref(),
                            SubagentLifecycleEventKind::Progress,
                            detail,
                        )
                        .await;
                    }
                }
                AgentEvent::ConnectionState { state } => {
                    let detail = serde_json::json!({
                        "phase": "connection",
                        "state": state,
                    });
                    capture.tool_events.push(detail.clone());
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        detail,
                    )
                    .await;
                }
                AgentEvent::Steering { content } => {
                    if !content.trim().is_empty() {
                        capture.tool_events.push(serde_json::json!({
                            "phase": "steering",
                            "content": content,
                        }));
                        emit_subagent_lifecycle_event(
                            lifecycle_capture.as_ref(),
                            SubagentLifecycleEventKind::InputApplied,
                            serde_json::json!({
                                "bytes": content.len(),
                                "state": "applied_at_model_boundary",
                            }),
                        )
                        .await;
                    }
                }
                AgentEvent::UsageUpdate { usage_total, .. } => {
                    capture.usage_total = usage_total;
                    if !worker_token_limit_exceeded
                        && worker_actual_token_limit
                            .is_some_and(|limit| capture.usage_total.total_tokens > limit)
                    {
                        worker_token_limit_exceeded = true;
                        capture_cancel_token.cancel();
                        let _ = fatal_error_tx.send(format!(
                            "worker actual token limit exceeded: {} > {}",
                            capture.usage_total.total_tokens,
                            worker_actual_token_limit.unwrap_or_default(),
                        ));
                    }
                }
                AgentEvent::Done {
                    usage_total,
                    finish_reason,
                    ..
                } => {
                    flush_subagent_deltas(
                        lifecycle_capture.as_ref(),
                        &mut pending_thinking,
                        &mut pending_output,
                    )
                    .await;
                    capture.usage_total = usage_total;
                    if !worker_token_limit_exceeded
                        && worker_actual_token_limit
                            .is_some_and(|limit| capture.usage_total.total_tokens > limit)
                    {
                        worker_token_limit_exceeded = true;
                        capture_cancel_token.cancel();
                        let _ = fatal_error_tx.send(format!(
                            "worker actual token limit exceeded: {} > {}",
                            capture.usage_total.total_tokens,
                            worker_actual_token_limit.unwrap_or_default(),
                        ));
                    }
                    capture.finish_reason = finish_reason;
                }
                AgentEvent::Error { message } => {
                    flush_subagent_deltas(
                        lifecycle_capture.as_ref(),
                        &mut pending_thinking,
                        &mut pending_output,
                    )
                    .await;
                    capture.error_message = Some(message.clone());
                    capture.tool_events.push(serde_json::json!({
                        "phase": "error",
                        "message": &message,
                    }));
                    let _ = fatal_error_tx.send(message);
                    capture_cancel_token.cancel();
                    break;
                }
                AgentEvent::TextDelta { delta } => {
                    pending_output.push_str(&delta);
                }
                AgentEvent::ToolRunStarted { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::ToolStarted,
                        serde_json::json!({ "phase": "runStarted", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolRunUpdated { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "phase": "runUpdated", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolRunCompleted { run } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({ "phase": "runCompleted", "run": run }),
                    )
                    .await;
                }
                AgentEvent::ToolCallPreparing {
                    call_id,
                    tool_name,
                    args_bytes,
                    index,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolPreparing",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": args_bytes,
                            "index": index,
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallArgsDelta {
                    call_id,
                    tool_name,
                    arguments_delta,
                    index,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolArgsDelta",
                            "callId": call_id,
                            "toolName": tool_name,
                            "argsBytes": arguments_delta.len(),
                            "index": index,
                        }),
                    )
                    .await;
                }
                AgentEvent::ToolCallProgress {
                    call_id,
                    tool_name,
                    note,
                    activity,
                } => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "toolProgress",
                            "callId": call_id,
                            "toolName": tool_name,
                            "note": note,
                            "activity": activity,
                        }),
                    )
                    .await;
                }
                AgentEvent::ControllerStatus {
                    code,
                    content,
                    tone,
                } if code != "provider_connected" => {
                    emit_subagent_lifecycle_event(
                        lifecycle_capture.as_ref(),
                        SubagentLifecycleEventKind::Progress,
                        serde_json::json!({
                            "phase": "controller",
                            "code": code,
                            "content": content,
                            "tone": tone,
                        }),
                    )
                    .await;
                }
                AgentEvent::StreamBlockDelta { .. }
                | AgentEvent::StreamReset { .. }
                | AgentEvent::AutoCompacted { .. }
                | AgentEvent::ApprovalRequested { .. }
                | AgentEvent::ApprovalResolved { .. }
                | AgentEvent::ControllerStatus { .. }
                | AgentEvent::PlanUpdated { .. } => {}
            }
            if last_delta_flush.elapsed() >= Duration::from_millis(100)
                && (!pending_thinking.is_empty() || !pending_output.is_empty())
            {
                flush_subagent_deltas(
                    lifecycle_capture.as_ref(),
                    &mut pending_thinking,
                    &mut pending_output,
                )
                .await;
                last_delta_flush = Instant::now();
            }
        }
        capture
    }
}
