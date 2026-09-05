use super::*;

pub async fn run_desktop_agent_turn(request: DesktopAgentTurnRequest) -> DesktopAgentTurnOutcome {
    let DesktopAgentTurnRequest {
        backend,
        dependencies,
        executor_config,
        cancel_token,
        steering_rx,
        approval_runtime,
        summarization_provider,
        tool_visual_interpreter,
        history,
        user_parts,
        db,
        conversation_id,
        turn_id,
        assistant_sort_order,
        runtime,
        stream,
    } = request;

    let approval_cb = build_desktop_approval_callback(DesktopApprovalCallbackInput {
        db: Arc::clone(&db),
        task_run_id: stream.task_run_id.clone(),
        approval_runtime,
        cancellation: cancel_token.clone(),
    });

    let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(64);
    let event_forwarder = AgentStreamForwarder::new(
        conversation_id.clone(),
        stream.task_run_id.clone(),
        turn_id.clone(),
        stream.event_seq.as_ref().clone(),
        stream.launch_started,
    )
    .run(events_rx);

    let run_cancel = cancel_token.clone();
    let run_conversation = conversation_id.clone();
    let run_turn = turn_id.clone();
    let run_future: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Message, CoreError>> + Send>,
    > = match backend {
        DesktopAgentBackend::Nexa(provider) => Box::pin(async move {
            let activity = nexa_core::activity::ActivityRuntime::with_database((*db).clone())?;
            let mut executor = AgentExecutor::new(provider, dependencies.tools, executor_config)
                .with_activity_runtime(activity)
                .with_cancel_token(run_cancel)
                .with_steering_receiver(steering_rx)
                .with_approval_callback(approval_cb)
                .with_tool_visual_interpreter(tool_visual_interpreter)
                .with_skills_override(dependencies.selected_skills)
                .with_auto_loaded_skills_override(dependencies.auto_loaded_skills);
            if let Some(provider) = summarization_provider {
                executor = executor.with_summarization_provider(provider);
            }
            executor
                .run(
                    history,
                    user_parts,
                    db.as_ref(),
                    Some(&run_conversation),
                    Some(&run_turn),
                    events_tx,
                    assistant_sort_order,
                )
                .await
        }),
        DesktopAgentBackend::Subscription(kind) => Box::pin(crate::subscription_runtime::run(
            crate::subscription_runtime::SubscriptionTurnRequest {
                kind,
                config: executor_config,
                dependencies,
                db,
                conversation_id: run_conversation,
                turn_id: run_turn,
                next_sort_order: assistant_sort_order,
                history,
                user_parts,
                events: events_tx,
                cancellation: run_cancel,
                steering: steering_rx,
                approval: approval_cb,
                visual_interpreter: tool_visual_interpreter,
            },
        )),
    };

    // The same run owns its driver and ordered publication consumer. Renderer
    // navigation cannot replace either backend or replay the user request.
    let run_driver = async {
        let mut run_future = run_future;
        let mut turn_timeout = (runtime.timeout_secs > 0).then(|| {
            Box::pin(tokio::time::sleep(Duration::from_secs(
                runtime.timeout_secs,
            )))
        });
        let mut keepalive =
            tokio::time::interval(Duration::from_secs(runtime.keepalive_interval_secs.max(1)));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;

        let (result, timed_out) = loop {
            tokio::select! {
                biased;
                run_result = &mut run_future => break (Some(run_result), false),
                _ = async {
                    if let Some(timeout) = turn_timeout.as_mut() {
                        timeout.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => break (None, true),
                _ = keepalive.tick() => {
                    emit_main_window_event(
                        &stream.app_handle,
                        "agent://heartbeat",
                        &serde_json::json!({
                            "conversationId": conversation_id,
                            "runId": stream.task_run_id,
                            "turnId": turn_id,
                            "durableHighWater": stream.event_seq.durable_high_water(),
                        }),
                    );
                }
            }
        };

        if timed_out {
            let _ = stream.event_seq.submit(AgentRunEvent::terminal_error(
                &stream.task_run_id,
                Some(&turn_id),
                0,
                "Agent execution timed out.",
                "timed_out",
                Some(&serde_json::json!({ "reason": "agent_timeout" })),
            ));
            cancel_token.cancel();
            // Drive cooperative cancellation to its tool/model boundary before
            // dropping the executor. Browser receipt guards then persist an
            // uncertain terminal, while computer workers retain ownership and
            // finish their worker-owned receipts independently.
            let _ = tokio::time::timeout(Duration::from_secs(5), &mut run_future).await;
        }

        drop(run_future);
        drop(turn_timeout);
        (result, timed_out)
    };

    let ((result, timed_out), ()) = tokio::join!(run_driver, event_forwarder);

    DesktopAgentTurnOutcome { result, timed_out }
}
