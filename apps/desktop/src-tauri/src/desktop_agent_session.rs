//! Desktop Agent Session Adapter over the core agent executor.
//!
//! This Module keeps Desktop-specific executor wiring behind one Interface so
//! chat commands can focus on Host Surface concerns such as task events and UI
//! persistence.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use nexa_core::agent::{
    AgentConfig, AgentEvent, AgentExecutor, AgentSteeringMessage, CancellationToken,
    ConfirmationCallback,
};
use nexa_core::agent_run::AgentRunPhase;
use nexa_core::approval::{
    ApprovalCallback, ApprovalDecision, ApprovalRequest, SessionApprovalStore, ToolApprovalMode,
    ToolPermissionKey,
};
use nexa_core::conversation::{AgentSubtaskRun, ConversationMessage};
use nexa_core::db::Database;
use nexa_core::error::CoreError;
use nexa_core::llm::{ContentPart, LlmProvider, Message, ProviderConfig, Role};
use nexa_core::mcp::McpManager;
use nexa_core::skills::Skill;
use nexa_core::tools::{default_tool_registry, ToolRegistry};
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent_stream::emit_agent_frontend_event;
use crate::agent_stream_bridge::AgentStreamForwarder;
use crate::agent_task_events::{emit_agent_task_run_update, record_agent_run_status_task_event};
use crate::subagent_tool::{
    DelegationRuntime, JudgeSubagentResultsTool, SubagentBatchTool, SubagentTool,
};

pub struct DesktopAgentTurnRuntime {
    pub timeout_secs: u64,
    pub keepalive_interval_secs: u64,
}

pub struct DesktopAgentTurnStream {
    pub app_handle: AppHandle,
    pub task_run_id: String,
    pub event_seq: Arc<AtomicU64>,
    pub terminal_emitted: Arc<AtomicBool>,
}

pub struct DesktopAgentApprovalRuntime {
    pub pending:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<ApprovalDecision>>>>,
    pub session_store: SessionApprovalStore,
    pub approval_mode: ToolApprovalMode,
}

pub struct DesktopAgentSessionDependencies {
    pub tools: ToolRegistry,
    pub selected_skills: Vec<Skill>,
    pub auto_loaded_skills: Vec<Skill>,
}

pub struct DesktopAgentSessionDependencyRequest<'a> {
    pub db: &'a Database,
    pub mcp_manager: &'a tokio::sync::Mutex<McpManager>,
    pub app_handle: &'a AppHandle,
    pub event_seq: &'a AtomicU64,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub turn_id: &'a str,
    pub message: &'a str,
    pub pinned_skill_ids: &'a [String],
    pub provider_config: ProviderConfig,
    pub executor_config: AgentConfig,
    pub subagent_allowed_tools: Option<Vec<String>>,
    pub subagent_allowed_skill_ids: Option<Vec<String>>,
    pub cancel_token: CancellationToken,
    pub plan_mode: bool,
    pub mcp_call_timeout_secs: u64,
}

pub struct DesktopAgentTurnOutcome {
    pub result: Option<Result<Message, CoreError>>,
    pub timed_out: bool,
}

pub struct DesktopAgentTurnFinalization<'a> {
    pub db: &'a Database,
    pub app_handle: &'a AppHandle,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub turn_id: &'a str,
    pub event_seq: &'a AtomicU64,
    pub outcome: &'a DesktopAgentTurnOutcome,
}

pub struct DesktopAgentStopFinalization<'a> {
    pub db: &'a Database,
    pub app_handle: &'a AppHandle,
    pub conversation_id: &'a str,
    pub task_run_id: &'a str,
    pub turn_id: &'a str,
    pub event_seq: &'a AtomicU64,
    pub reason: &'a str,
    pub summary: &'a str,
}

pub struct DesktopAgentTurnRequest {
    pub provider: Box<dyn LlmProvider>,
    pub dependencies: DesktopAgentSessionDependencies,
    pub executor_config: AgentConfig,
    pub cancel_token: CancellationToken,
    pub steering_rx: mpsc::UnboundedReceiver<AgentSteeringMessage>,
    pub approval_runtime: DesktopAgentApprovalRuntime,
    pub summarization_provider: Option<Box<dyn LlmProvider>>,
    pub history: Vec<Message>,
    pub user_parts: Vec<ContentPart>,
    pub db: Arc<Database>,
    pub conversation_id: String,
    pub turn_id: String,
    pub assistant_sort_order: i64,
    pub runtime: DesktopAgentTurnRuntime,
    pub stream: DesktopAgentTurnStream,
}

struct DesktopApprovalCallbackInput {
    db: Arc<Database>,
    app_handle: AppHandle,
    conversation_id: String,
    task_run_id: String,
    turn_id: String,
    event_seq: Arc<AtomicU64>,
    approval_runtime: DesktopAgentApprovalRuntime,
}

async fn sync_enabled_desktop_mcp_servers(
    db: &Database,
    manager: &mut McpManager,
    timeout_secs: u64,
) -> Result<HashMap<String, String>, String> {
    let enabled_servers = db.get_enabled_mcp_servers().map_err(|e| e.to_string())?;
    Ok(manager
        .sync_servers(&enabled_servers, Some(timeout_secs))
        .await)
}

pub async fn build_desktop_agent_session_dependencies(
    request: DesktopAgentSessionDependencyRequest<'_>,
) -> DesktopAgentSessionDependencies {
    let DesktopAgentSessionDependencyRequest {
        db,
        mcp_manager,
        app_handle,
        event_seq,
        conversation_id,
        task_run_id,
        turn_id,
        message,
        pinned_skill_ids,
        provider_config,
        executor_config,
        subagent_allowed_tools,
        subagent_allowed_skill_ids,
        cancel_token,
        plan_mode,
        mcp_call_timeout_secs,
    } = request;

    let selected_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_available_skills_for_query(db, message)
    } else {
        nexa_core::skills::get_available_skills_for_query_with_pinned(db, message, pinned_skill_ids)
    }
    .unwrap_or_else(|err| {
        warn!("Failed to select skills for task run {task_run_id}: {err}");
        Vec::new()
    });

    let max_loaded_skills = 3usize.max(pinned_skill_ids.len());
    let auto_loaded_skills = if pinned_skill_ids.is_empty() {
        nexa_core::skills::get_active_skills_for_query(db, message, max_loaded_skills)
    } else {
        nexa_core::skills::get_active_skills_for_query_with_pinned(
            db,
            message,
            max_loaded_skills,
            pinned_skill_ids,
        )
    }
    .unwrap_or_else(|err| {
        warn!("Failed to auto-load skills for task run {task_run_id}: {err}");
        Vec::new()
    });

    let mut tools = default_tool_registry();
    emit_agent_frontend_event(
        app_handle,
        event_seq,
        conversation_id,
        task_run_id,
        Some(turn_id),
        AgentEvent::Status {
            content: "Loading tools and MCP servers".to_string(),
            tone: None,
        },
    );
    {
        let mut manager = mcp_manager.lock().await;
        match sync_enabled_desktop_mcp_servers(db, &mut manager, mcp_call_timeout_secs).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id}: {error}");
                }
            }
            Err(error) => warn!("Failed to load enabled MCP servers: {error}"),
        }
        if let Err(error) = manager.register_tools(&mut tools).await {
            warn!("Failed to register MCP tools: {error}");
        }
    }

    let delegation_runtime = DelegationRuntime::new(
        provider_config,
        executor_config,
        subagent_allowed_tools,
        subagent_allowed_skill_ids,
        cancel_token,
        Some(task_run_id.to_string()),
    );
    tools.register(Box::new(SubagentTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(SubagentBatchTool::from_runtime(
        delegation_runtime.clone(),
    )));
    tools.register(Box::new(JudgeSubagentResultsTool::from_runtime(
        delegation_runtime.clone(),
    )));
    delegation_runtime.set_tool_registry(tools.clone());

    if plan_mode {
        let before_count = tools.tool_names().len();
        tools = tools.plan_mode_filtered();
        let after_count = tools.tool_names().len();
        info!(
            "Plan mode tool registry filtered from {before_count} to {after_count} read-only tools"
        );
        emit_agent_frontend_event(
            app_handle,
            event_seq,
            conversation_id,
            task_run_id,
            Some(turn_id),
            AgentEvent::Status {
                content: "Plan mode active: write, execution, MCP, automation, and delegation tools are disabled."
                    .to_string(),
                tone: Some("info".to_string()),
            },
        );
    }

    DesktopAgentSessionDependencies {
        tools,
        selected_skills,
        auto_loaded_skills,
    }
}

fn build_desktop_confirmation_callback(
    app_handle: &AppHandle,
    executor_config: &AgentConfig,
) -> Option<ConfirmationCallback> {
    if !executor_config.require_tool_confirmation
        && !executor_config.shell_access_mode.requires_confirmation()
    {
        return None;
    }

    let dialog_handle = app_handle.clone();
    Some(Arc::new(move |message: String| {
        let handle = dialog_handle.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            handle
                .dialog()
                .message(&message)
                .title("Confirm Tool Execution")
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Allow".into(),
                    "Deny".into(),
                ))
                .show(move |confirmed| {
                    let _ = tx.send(confirmed);
                });
            match tokio::time::timeout(Duration::from_secs(30), rx).await {
                Ok(Ok(confirmed)) => confirmed,
                _ => !message.starts_with("Run:"),
            }
        })
    }))
}

fn build_desktop_approval_callback(input: DesktopApprovalCallbackInput) -> ApprovalCallback {
    let DesktopApprovalCallbackInput {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        event_seq,
        approval_runtime,
    } = input;
    let pending = approval_runtime.pending;
    let session_store = approval_runtime.session_store;
    let approval_mode = approval_runtime.approval_mode;

    Arc::new(move |req: ApprovalRequest| {
        let db = Arc::clone(&db);
        let handle = app_handle.clone();
        let pending = Arc::clone(&pending);
        let store = session_store.clone();
        let conv = conversation_id.clone();
        let event_seq = Arc::clone(&event_seq);
        let task_run_id = task_run_id.clone();
        let turn_id = turn_id.clone();
        Box::pin(async move {
            if let Some(decision) = approval_mode.short_circuit() {
                return decision;
            }

            if let Ok(Some(policy)) = db.get_tool_permission_policy(&req.permission_key) {
                if policy == "never" {
                    return ApprovalDecision::Deny;
                }
            }
            let allow_legacy_tool_policy = req.tool_name != "project_tool";
            if allow_legacy_tool_policy {
                if let Ok(Some(policy)) = db.get_tool_approval_policy(&req.tool_name) {
                    if policy == "never" {
                        return ApprovalDecision::Deny;
                    }
                }
            }

            if matches!(
                store.get(&req.permission_key),
                Some(ApprovalDecision::AllowSession)
            ) || (allow_legacy_tool_policy
                && matches!(
                    store.get(&req.tool_name),
                    Some(ApprovalDecision::AllowSession)
                ))
            {
                return ApprovalDecision::AllowOnce;
            }

            let (tx, rx) = tokio::sync::oneshot::channel();
            pending.lock().await.insert(req.id.clone(), tx);
            emit_agent_frontend_event(
                &handle,
                event_seq.as_ref(),
                &conv,
                &task_run_id,
                Some(&turn_id),
                AgentEvent::ApprovalRequested {
                    request: req.clone(),
                },
            );

            let decision = match tokio::time::timeout(Duration::from_secs(60), rx).await {
                Ok(Ok(decision)) => decision,
                _ => {
                    pending.lock().await.remove(&req.id);
                    ApprovalDecision::Deny
                }
            };
            match decision {
                ApprovalDecision::AllowSession => {
                    store.set(&req.permission_key, ApprovalDecision::AllowSession);
                }
                ApprovalDecision::Never => {
                    let key = ToolPermissionKey::from_request(&req);
                    let _ = db.save_tool_permission_policy(&key, "never");
                }
                _ => {}
            }
            decision
        })
    })
}

pub async fn run_desktop_agent_turn(request: DesktopAgentTurnRequest) -> DesktopAgentTurnOutcome {
    let DesktopAgentTurnRequest {
        provider,
        dependencies,
        executor_config,
        cancel_token,
        steering_rx,
        approval_runtime,
        summarization_provider,
        history,
        user_parts,
        db,
        conversation_id,
        turn_id,
        assistant_sort_order,
        runtime,
        stream,
    } = request;

    let confirmation_cb = build_desktop_confirmation_callback(&stream.app_handle, &executor_config);
    let approval_cb = build_desktop_approval_callback(DesktopApprovalCallbackInput {
        db: Arc::clone(&db),
        app_handle: stream.app_handle.clone(),
        conversation_id: conversation_id.clone(),
        task_run_id: stream.task_run_id.clone(),
        turn_id: turn_id.clone(),
        event_seq: Arc::clone(&stream.event_seq),
        approval_runtime,
    });

    let executor_cancel_token = cancel_token.clone();
    let mut executor = AgentExecutor::new(provider, dependencies.tools, executor_config)
        .with_cancel_token(executor_cancel_token)
        .with_steering_receiver(steering_rx);
    if let Some(cb) = confirmation_cb {
        executor = executor.with_confirmation_callback(cb);
    }
    executor = executor.with_approval_callback(approval_cb);
    if let Some(provider) = summarization_provider {
        executor = executor.with_summarization_provider(provider);
    }
    executor = executor
        .with_skills_override(dependencies.selected_skills)
        .with_auto_loaded_skills_override(dependencies.auto_loaded_skills);

    let (events_tx, events_rx) = mpsc::channel::<AgentEvent>(64);
    let event_forwarder = tokio::spawn(
        AgentStreamForwarder::new(
            stream.app_handle.clone(),
            db.clone(),
            conversation_id.clone(),
            stream.task_run_id.clone(),
            turn_id.clone(),
            Arc::clone(&stream.event_seq),
            Arc::clone(&stream.terminal_emitted),
        )
        .run(events_rx),
    );

    let run_future = executor.run(
        history,
        user_parts,
        db.as_ref(),
        Some(&conversation_id),
        Some(&turn_id),
        events_tx,
        assistant_sort_order,
    );

    let mut run_future = Box::pin(run_future);
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
            run_result = &mut run_future => break (Some(run_result), false),
            _ = async {
                if let Some(timeout) = turn_timeout.as_mut() {
                    timeout.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break (None, true),
            _ = keepalive.tick() => {
                emit_agent_frontend_event(
                    &stream.app_handle,
                    stream.event_seq.as_ref(),
                    &conversation_id,
                    &stream.task_run_id,
                    Some(&turn_id),
                    AgentEvent::Thinking {
                        content: String::new(),
                    },
                );
            }
        }
    };

    if timed_out {
        cancel_token.cancel();
    }

    drop(run_future);
    drop(turn_timeout);
    drop(executor);

    let _ = event_forwarder.await;

    DesktopAgentTurnOutcome { result, timed_out }
}

pub fn finalize_desktop_agent_turn(finalization: DesktopAgentTurnFinalization<'_>) {
    let DesktopAgentTurnFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        event_seq,
        outcome,
    } = finalization;

    let turn_snapshot = db.get_conversation_turn(turn_id).ok();
    let trace_artifacts = serde_json::json!({
        "turnId": turn_id,
        "turnStatus": turn_snapshot.as_ref().map(|turn| turn.status.clone()),
        "routeKind": turn_snapshot.as_ref().and_then(|turn| turn.route_kind.clone()),
        "trace": turn_snapshot.as_ref().and_then(|turn| turn.trace.clone()),
    });
    let previous_task_artifacts = db
        .get_agent_task_run(task_run_id)
        .ok()
        .and_then(|run| run.artifacts);
    let subtask_runs = db
        .list_agent_subtask_runs(task_run_id)
        .unwrap_or_else(|err| {
            warn!("Failed to load subtask runs for {task_run_id}: {err}");
            Vec::new()
        });
    let task_artifacts =
        build_final_task_artifacts(previous_task_artifacts, trace_artifacts, &subtask_runs);
    let verification_status = task_artifacts
        .get("verification")
        .and_then(|verification| verification.get("overallStatus"))
        .and_then(|status| status.as_str());
    let current_task_status = db
        .get_agent_task_run(task_run_id)
        .ok()
        .map(|run| run.status);
    let (task_status, task_summary, task_error): (&str, &str, Option<String>) =
        if current_task_status.as_deref() == Some("paused") {
            ("paused", "Paused with a resumable checkpoint", None)
        } else if outcome.timed_out {
            (
                "timed_out",
                "Agent execution timed out",
                Some("Agent execution timed out.".to_string()),
            )
        } else if let Some(Err(CoreError::Cancelled(message))) = &outcome.result {
            (
                "cancelled",
                "Agent execution cancelled",
                Some(message.clone()),
            )
        } else if let Some(Err(err)) = &outcome.result {
            ("failed", "Agent execution failed", Some(err.to_string()))
        } else {
            match turn_snapshot.as_ref().map(|turn| turn.status.as_str()) {
                Some("cancelled") => ("cancelled", "Stopped by user", None),
                Some("error") => ("failed", "Agent execution failed", None),
                Some("cached") => ("completed", "Answered from cache", None),
                _ if verification_status.is_some_and(|status| status != "passed") => {
                    ("completed", "Task completed with verification gap", None)
                }
                _ => ("completed", "Task completed", None),
            }
        };

    let _ = db.finish_agent_task_run(
        task_run_id,
        task_status,
        Some(task_summary),
        task_error.as_deref(),
        Some(&task_artifacts),
    );
    record_agent_run_status_task_event(
        db,
        app_handle,
        conversation_id,
        task_run_id,
        Some(turn_id),
        event_seq,
        AgentRunPhase::Done,
        task_summary,
        Some(task_status),
        Some(&task_artifacts),
    );
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);

    if !matches!(&outcome.result, Some(Ok(_))) {
        repair_orphaned_tool_calls(db, conversation_id);
    }
}

pub fn finalize_desktop_agent_stop(finalization: DesktopAgentStopFinalization<'_>) {
    let DesktopAgentStopFinalization {
        db,
        app_handle,
        conversation_id,
        task_run_id,
        turn_id,
        event_seq,
        reason,
        summary,
    } = finalization;
    let artifacts = serde_json::json!({ "reason": reason });

    let _ = db.finish_agent_task_run(
        task_run_id,
        "cancelled",
        Some(summary),
        None,
        Some(&artifacts),
    );
    record_agent_run_status_task_event(
        db,
        app_handle,
        conversation_id,
        task_run_id,
        Some(turn_id),
        event_seq,
        AgentRunPhase::Done,
        summary,
        Some("cancelled"),
        Some(&artifacts),
    );
    emit_agent_task_run_update(db, app_handle, conversation_id, task_run_id);
}

fn build_final_task_artifacts(
    previous_artifacts: Option<serde_json::Value>,
    trace_artifacts: serde_json::Value,
    subtask_runs: &[AgentSubtaskRun],
) -> serde_json::Value {
    let mut merged = match previous_artifacts {
        Some(serde_json::Value::Object(map)) => map,
        Some(previous) => {
            let mut map = serde_json::Map::new();
            map.insert("previous".to_string(), previous);
            map
        }
        None => serde_json::Map::new(),
    };
    merged.insert(
        "kind".to_string(),
        serde_json::Value::String("agentTaskArtifacts".to_string()),
    );
    merged.insert(
        "version".to_string(),
        serde_json::Value::Number(serde_json::Number::from(1)),
    );
    merged.insert("trace".to_string(), trace_artifacts);
    merged.insert(
        "subtasks".to_string(),
        serde_json::to_value(subtask_runs).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
    );
    serde_json::Value::Object(merged)
}

fn repair_orphaned_tool_calls(db: &Database, conversation_id: &str) {
    let msgs = match db.get_messages(conversation_id) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to load messages for orphan repair: {e}");
            return;
        }
    };

    let mut i = 0;
    while i < msgs.len() {
        if msgs[i].role == Role::Assistant && !msgs[i].tool_calls.is_empty() {
            let mut found_ids = std::collections::HashSet::new();
            let mut j = i + 1;
            while j < msgs.len() && msgs[j].role == Role::Tool {
                if let Some(ref tc_id) = msgs[j].tool_call_id {
                    found_ids.insert(tc_id.as_str());
                }
                j += 1;
            }

            let base_sort = if j > i + 1 {
                msgs[j - 1].sort_order
            } else {
                msgs[i].sort_order
            };

            let mut extra_sort = 1;
            for tc in &msgs[i].tool_calls {
                if !found_ids.contains(tc.id.as_str()) {
                    warn!(
                        "Inserting synthetic error response for orphaned tool_call {}",
                        tc.id
                    );
                    let synthetic = ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        conversation_id: conversation_id.to_string(),
                        role: Role::Tool,
                        content: format!(
                            "Error: tool '{}' was interrupted before completing (agent timeout or cancellation).",
                            tc.name
                        ),
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                        artifacts: None,
                        token_count: 20,
                        created_at: String::new(),
                        sort_order: base_sort + extra_sort,
                        thinking: None,
                        image_attachments: None,
                    };
                    if let Err(e) = db.add_message(&synthetic) {
                        warn!("Failed to insert synthetic tool response: {e}");
                    }
                    extra_sort += 1;
                }
            }
        }
        i += 1;
    }
}
