use super::projection::Projection;
use super::*;
use github_copilot_sdk::types::{ToolBinaryResult, ToolInvocation, ToolResult, ToolResultExpanded};
use github_copilot_sdk::{
    Attachment, CliProgram, Client, ClientMode, ClientOptions, MessageOptions, SessionConfig,
    SessionEvent, SystemMessageConfig, Tool, ToolSet,
};
use nexa_core::agent::StreamBlockChannel;
use nexa_core::llm::ToolCallRequest;
use std::collections::{HashSet, VecDeque};
use std::time::Duration;

struct ToolBridge {
    tools: Arc<ExternalToolSession>,
    fatal: mpsc::Sender<CoreError>,
}

fn client_options(binary: std::path::PathBuf) -> Result<ClientOptions, CoreError> {
    // Empty mode requires an explicit persistence owner. Use the same official
    // home as Copilot login; never copy credentials into Nexa or a temp home.
    let directory = std::env::var_os("COPILOT_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|home| std::path::PathBuf::from(home).join(".copilot"))
        })
        .filter(|path| path.is_absolute())
        .ok_or_else(|| protocol_error("Copilot home must be an absolute directory"))?;
    Ok(with_login_credential_backend(
        ClientOptions::default()
            .with_program(CliProgram::Path(binary))
            .with_mode(ClientMode::Empty)
            .with_base_directory(directory),
        std::env::var_os("COPILOT_DISABLE_KEYTAR"),
    ))
}

fn with_login_credential_backend(
    options: ClientOptions,
    inherited: Option<std::ffi::OsString>,
) -> ClientOptions {
    // SDK 1.0.11 injects DISABLE_KEYTAR=1 in Empty mode, then applies caller
    // env/env_remove. Tool isolation remains Empty; authentication must use
    // precisely the same keychain setting as the ordinary login/account CLI.
    // Neither path extracts or copies a token into Nexa.
    match inherited {
        Some(value) => {
            options.with_env([(std::ffi::OsString::from("COPILOT_DISABLE_KEYTAR"), value)])
        }
        None => options.with_env_remove(["COPILOT_DISABLE_KEYTAR"]),
    }
}

#[async_trait::async_trait]
impl github_copilot_sdk::tool::ToolHandler for ToolBridge {
    async fn call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<ToolResult, github_copilot_sdk::Error> {
        let result = self
            .tools
            .execute(ToolCallRequest {
                id: invocation.tool_call_id,
                name: invocation.tool_name,
                arguments: invocation.arguments.to_string(),
                thought_signature: None,
            })
            .await;
        Ok(match result {
            Ok(output) => {
                let mut result = ToolResultExpanded::new(
                    output.result.content,
                    if output.result.is_error {
                        "failure"
                    } else {
                        "success"
                    },
                );
                let mut images = Vec::new();
                for part in output.visual_parts {
                    match part {
                        ContentPart::Image { media_type, data } => images.push(ToolBinaryResult {
                            data,
                            mime_type: media_type,
                            r#type: "image".into(),
                            description: Some("Current Nexa tool observation".into()),
                        }),
                        ContentPart::Text { text } => {
                            result.text_result_for_llm.push('\n');
                            result.text_result_for_llm.push_str(&text);
                        }
                        _ => {}
                    }
                }
                if !images.is_empty() {
                    result.binary_results_for_llm = Some(images);
                }
                ToolResult::Expanded(result)
            }
            Err(error) => {
                let message = error.to_string();
                let _ = self.fatal.try_send(error);
                ToolResult::Expanded(ToolResultExpanded::new(message, "failure"))
            }
        })
    }
}

pub(super) async fn run(request: SubscriptionTurnRequest) -> Result<Message, CoreError> {
    let cancellation = request.cancellation.clone();
    let model_id = request
        .config
        .model
        .clone()
        .ok_or_else(|| protocol_error("select a Copilot model first"))?;
    let connect = async {
        let binary = tokio::task::spawn_blocking(
            crate::commands::subscription_accounts::resolve_copilot_binary,
        )
        .await
        .map_err(protocol_error)?
        .map_err(protocol_error)?;
        let client = Client::start(client_options(binary)?)
            .await
            .map_err(protocol_error)?;
        let models = client.list_models().await.map_err(protocol_error)?;
        let model = models.into_iter().find(|model| model.id == model_id).ok_or_else(|| protocol_error("the selected model is not available to this Copilot account; refresh its model list"))?;
        Ok::<_, CoreError>((client, model))
    };
    let (client, model) = tokio::select! {
        _ = cancellation.cancelled() => return Err(CoreError::Cancelled("Stopped during Copilot connection".into())),
        result = tokio::time::timeout(Duration::from_secs(45), connect) => result.map_err(|_| protocol_error("Copilot connection timed out"))??,
    };
    let native_vision = model
        .capabilities
        .supports
        .as_ref()
        .and_then(|supports| supports.vision)
        .unwrap_or(false);
    let mut turn = request.prepare(native_vision)?;
    let (fatal_tx, mut fatal_rx) = mpsc::channel(1);
    let bridge = Arc::new(ToolBridge {
        tools: turn.tools.clone(),
        fatal: fatal_tx,
    });
    let tools = turn
        .tools
        .definitions()
        .into_iter()
        .map(|definition| {
            Tool::new(definition.name)
                .with_description(definition.description)
                .with_parameters(definition.parameters)
                // Only this custom callback skips CLI permission prompts. Nexa's
                // shared dispatcher performs the actual policy and user approval.
                .with_skip_permission(true)
                .with_handler(bridge.clone())
        })
        .collect::<Vec<_>>();
    let mut config = SessionConfig::default()
        .with_model(&model_id)
        .with_streaming(true)
        .with_tools(tools)
        .with_available_tools(
            ToolSet::new()
                .add_custom("*")
                .map_err(protocol_error)?
                .to_vec(),
        )
        .with_system_message(
            SystemMessageConfig::new()
                .with_mode("replace")
                .with_content(&turn.system_prompt),
        )
        .deny_all_permissions();
    if let Some(effort) = turn.config.reasoning_effort.as_ref() {
        let effort = serde_json::to_value(effort)
            .map_err(protocol_error)?
            .as_str()
            .ok_or_else(|| protocol_error("invalid reasoning effort"))?
            .to_string();
        if !model
            .supported_reasoning_efforts
            .as_ref()
            .is_some_and(|levels| levels.contains(&effort))
        {
            return Err(protocol_error(
                "the selected Copilot model does not support this reasoning effort",
            ));
        }
        config = config.with_reasoning_effort(effort);
    }
    let session = tokio::select! {
        _ = turn.cancellation.cancelled() => return Err(CoreError::Cancelled("Stopped during Copilot session creation".into())),
        result = tokio::time::timeout(Duration::from_secs(30), client.create_session(config)) => result.map_err(|_| protocol_error("Copilot session creation timed out"))?.map_err(protocol_error)?,
    };
    let mut events = session.subscribe();
    let initial = MessageOptions::new(&turn.prompt).with_attachments(
        turn.images
            .iter()
            .map(|(mime_type, data)| Attachment::Blob {
                data: data.clone(),
                mime_type: mime_type.clone(),
                display_name: None,
            })
            .collect(),
    );
    let mut projection = Projection::default();
    let mut seen = HashSet::new();
    let mut steering = VecDeque::new();
    let mut steering_closed = false;
    let run = async {
        session.send(initial).await.map_err(protocol_error)?;
        loop {
            tokio::select! {
                biased;
                error = fatal_rx.recv() => if let Some(error) = error { return Err(error); },
                _ = turn.cancellation.cancelled() => return Err(CoreError::Cancelled("Stopped by user".into())),
                message = turn.steering.recv(), if !steering_closed => match message {
                    Some(message) => {
                        if steering.len() >= 64 || message.content.len() > 256 * 1024 { return Err(protocol_error("Copilot steering input budget exceeded")); }
                        steering.push_back(message);
                    },
                    None => steering_closed = true,
                },
                event = events.recv() => {
                    let batch = match event {
                        Ok(event) => vec![event],
                        Err(error) if matches!(error.kind(), github_copilot_sdk::subscription::RecvErrorKind::Lagged(_)) => {
                            session.get_events().await.map_err(protocol_error)?
                        }
                        Err(error) => return Err(protocol_error(error)),
                    };
                    let mut idle = false;
                    for event in batch {
                        if event.agent_id.is_none() && !seen.contains(&event.id) {
                            if event.event_type == "session.idle" { idle = true; }
                            else if matches!(event.event_type.as_str(), "assistant.turn_start" | "assistant.message_delta" | "tool.execution_start") { idle = false; }
                        }
                        project_event(&mut projection, &turn.events, &mut seen, &event).await?;
                    }
                    if idle {
                        if let Some(message) = steering.pop_front() {
                            if message.recovery_control.is_some() { return Err(protocol_error("Copilot manages its own recovery. Stop and start a new turn to change reasoning.")); }
                            let attachments = message.parts.iter().filter_map(|part| match part { ContentPart::Image {media_type,data} => Some(Attachment::Blob{data:data.clone(),mime_type:media_type.clone(),display_name:None}),_=>None }).collect::<Vec<_>>();
                            if !native_vision && !attachments.is_empty() { return Err(protocol_error("the selected Copilot model does not accept steering images")); }
                            projection.persist_completed_answer(&turn).await?;
                            turn.tools.persist_steering(&message).await?;
                            if turn.cancellation.is_cancelled() { return Err(CoreError::Cancelled("Stopped by user".into())); }
                            turn.events.send(AgentEvent::Steering { content:message.content.clone() }).await.map_err(protocol_error)?;
                            session.send(MessageOptions::new(redact_user_text(&message.content,&turn.privacy)).with_attachments(attachments)).await.map_err(protocol_error)?;
                        } else { return Ok(()); }
                    }
                }
            }
        }
    }.await;
    turn.cancellation.cancel();
    if run.is_err() {
        projection.persist_partial(&turn).await?;
        for message in steering {
            if message.recovery_control.is_none() {
                turn.tools.persist_steering(&message).await?;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), session.abort()).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), session.disconnect()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), client.stop()).await;
    match run {
        Ok(()) => projection.finish(&turn).await,
        Err(error) => Err(error),
    }
}

async fn project_event(
    projection: &mut Projection,
    tx: &mpsc::Sender<AgentEvent>,
    seen: &mut HashSet<String>,
    event: &SessionEvent,
) -> Result<(), CoreError> {
    if event.agent_id.is_some() {
        return Ok(());
    }
    if event.ephemeral != Some(true) && !seen.insert(event.id.clone()) {
        return Ok(());
    }
    let data = &event.data;
    match event.event_type.as_str() {
        "assistant.message_delta" => {
            projection
                .delta(
                    tx,
                    data.get("messageId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&event.id),
                    StreamBlockChannel::Answer,
                    data.get("deltaContent")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                )
                .await?
        }
        "assistant.reasoning_delta" => {
            projection
                .delta(
                    tx,
                    data.get("reasoningId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&event.id),
                    StreamBlockChannel::Thinking,
                    data.get("deltaContent")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                )
                .await?
        }
        "assistant.message" => {
            let text = data
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let id = data
                .get("messageId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&event.id);
            projection.complete(tx, id, text).await?;
            if data
                .get("toolRequests")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|calls| !calls.is_empty())
            {
                projection.clear_answer();
            }
        }
        "tool.execution_start" => projection.clear_answer(),
        "assistant.usage" => {
            let tokens = |key: &str| {
                data.get(key)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32
            };
            projection.usage.prompt_tokens = projection
                .usage
                .prompt_tokens
                .saturating_add(tokens("inputTokens"));
            projection.last_prompt_tokens = tokens("inputTokens");
            projection.usage.completion_tokens = projection
                .usage
                .completion_tokens
                .saturating_add(tokens("outputTokens"));
            projection.usage.total_tokens = projection
                .usage
                .prompt_tokens
                .saturating_add(projection.usage.completion_tokens);
        }
        "session.error" => {
            return Err(protocol_error(
                data.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Copilot session failed"),
            ))
        }
        _ => {}
    }
    if seen.len() > 8192 {
        return Err(protocol_error("Copilot exceeded the turn event budget"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_tool_mode_preserves_normal_system_keychain_login() {
        let options = with_login_credential_backend(
            ClientOptions::default().with_mode(ClientMode::Empty),
            None,
        );
        assert_eq!(options.mode, ClientMode::Empty);
        assert!(options
            .env_remove
            .contains(&std::ffi::OsString::from("COPILOT_DISABLE_KEYTAR")));
        assert!(options.github_token.is_none());
    }
    #[test]
    fn explicit_login_keychain_setting_is_preserved_without_copying_credentials() {
        for value in ["0", "1", ""] {
            let options = with_login_credential_backend(
                ClientOptions::default().with_mode(ClientMode::Empty),
                Some(value.into()),
            );
            assert_eq!(options.mode, ClientMode::Empty);
            assert!(options
                .env
                .iter()
                .any(|(key, current)| key == "COPILOT_DISABLE_KEYTAR" && current == value));
            assert!(!options
                .env_remove
                .contains(&std::ffi::OsString::from("COPILOT_DISABLE_KEYTAR")));
            assert!(options.github_token.is_none());
        }
    }
    #[tokio::test]
    #[ignore = "uses the official Copilot subscription for one read-only tool inference"]
    async fn native_copilot_executes_nexa_tool_and_streams_persisted_answer() {
        let binary = crate::commands::subscription_accounts::resolve_copilot_binary().unwrap();
        let client = Client::start(client_options(binary).unwrap())
            .await
            .unwrap();
        let models = client.list_models().await.unwrap();
        let model = models
            .iter()
            .find(|model| model.id == "gpt-5-mini")
            .or_else(|| models.first())
            .unwrap()
            .id
            .clone();
        client.stop().await.unwrap();
        super::super::tests::run_live(SubscriptionRuntimeKind::Copilot, &model).await;
    }
}
