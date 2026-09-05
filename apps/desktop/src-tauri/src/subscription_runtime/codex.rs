//! The official app-server owns inference; Nexa owns effects and durable events.
use super::projection::Projection;
use super::*;
use futures::{stream::FuturesUnordered, StreamExt};
use nexa_core::agent::StreamBlockChannel;
use nexa_core::llm::ToolCallRequest;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// One reader owns stdout. A bounded mailbox prevents output from exhausting
/// memory while a tool is awaiting approval; it never blocks clock responses.
struct Wire {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<Result<Value, CoreError>>,
    reader: tokio::task::JoinHandle<()>,
    queued: VecDeque<Value>,
    next_id: u64,
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.child.start_kill();
    }
}

impl Wire {
    async fn start() -> Result<Self, CoreError> {
        let binary = tokio::task::spawn_blocking(
            crate::commands::subscription_accounts::resolve_codex_binary,
        )
        .await
        .map_err(protocol_error)?
        .map_err(protocol_error)?;
        let mut command = tokio::process::Command::new(binary.program);
        command.args(["app-server", "--stdio", "--strict-config"]);
        for (key, value) in static_config() {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(protocol_error)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| protocol_error("missing Codex stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol_error("missing Codex stdout"))?;
        let (tx, messages) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut frame = Vec::new();
            loop {
                // fill_buf/consume bounds even a malformed unterminated frame.
                let chunk = match reader.fill_buf().await {
                    Ok(chunk) if chunk.is_empty() => break,
                    Ok(chunk) => chunk,
                    Err(error) => {
                        let _ = tx.send(Err(protocol_error(error))).await;
                        break;
                    }
                };
                let length = chunk
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|index| index + 1)
                    .unwrap_or(chunk.len());
                if frame.len() + length > MAX_FRAME {
                    let _ = tx
                        .send(Err(protocol_error("Codex protocol frame too large")))
                        .await;
                    break;
                }
                let complete = chunk[length - 1] == b'\n';
                frame.extend_from_slice(&chunk[..length]);
                reader.consume(length);
                if complete {
                    if frame.iter().any(|byte| !byte.is_ascii_whitespace()) {
                        let parsed = serde_json::from_slice(&frame)
                            .map_err(|_| protocol_error("invalid Codex protocol JSON"));
                        let invalid = parsed.is_err();
                        if tx.send(parsed).await.is_err() || invalid {
                            break;
                        }
                    }
                    frame.clear();
                }
            }
        });
        let mut wire = Self {
            child,
            stdin,
            messages,
            reader,
            queued: VecDeque::new(),
            next_id: 1,
        };
        wire.request("initialize", json!({"clientInfo":{"name":"nexa-desktop","title":"Nexa","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        wire.write(json!({"method":"initialized"})).await?;
        Ok(wire)
    }

    async fn write(&mut self, message: Value) -> Result<(), CoreError> {
        let mut bytes = serde_json::to_vec(&message).map_err(protocol_error)?;
        bytes.push(b'\n');
        tokio::time::timeout(RPC_TIMEOUT, self.stdin.write_all(&bytes))
            .await
            .map_err(|_| protocol_error("Codex write timed out"))?
            .map_err(protocol_error)
    }

    async fn send(&mut self, method: &str, params: Value) -> Result<u64, CoreError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }

    /// Bootstrap only. During inference the event pump handles responses and
    /// server requests concurrently with tool futures instead of blocking here.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, CoreError> {
        let id = self.send(method, params).await?;
        tokio::time::timeout(RPC_TIMEOUT, async {
            loop {
                let message = self
                    .messages
                    .recv()
                    .await
                    .ok_or_else(|| protocol_error("Codex exited during startup"))??;
                if message.get("method").is_none() && message["id"].as_u64() == Some(id) {
                    return rpc_result(message, method);
                } else {
                    if self.queued.len() >= 1024 {
                        return Err(protocol_error("Codex startup event budget exceeded"));
                    }
                    self.queued.push_back(message);
                }
            }
        })
        .await
        .map_err(|_| protocol_error(format!("Codex {method} timed out")))?
    }

    async fn reject(&mut self, message: &Value) -> Result<(), CoreError> {
        let result = match message["method"].as_str().unwrap_or_default() {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                Some(json!({"decision":"decline"}))
            }
            "item/permissions/requestApproval" => Some(json!({"permissions":{},"scope":"turn"})),
            "mcpServer/elicitation/request" => {
                Some(json!({"action":"decline","content":null,"_meta":null}))
            }
            _ => None,
        };
        self.write(match result { Some(result) => json!({"id":message["id"],"result":result}), None => json!({"id":message["id"],"error":{"code":-32601,"message":"Use the provided Nexa tools; this native request is unavailable"}}) }).await
    }

    async fn receive(&mut self) -> Result<Value, CoreError> {
        if let Some(message) = self.queued.pop_front() {
            return Ok(message);
        }
        self.messages
            .recv()
            .await
            .ok_or_else(|| protocol_error("Codex process exited before turn completion"))?
    }
}

fn rpc_result(message: Value, method: &str) -> Result<Value, CoreError> {
    if message.get("error").is_some() {
        // Config responses can contain credentials; never echo raw RPC values.
        return Err(protocol_error(format!(
            "Codex {method} failed ({})",
            message["error"]["code"]
        )));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| protocol_error(format!("Codex {method} returned no result")))
}

fn static_config() -> serde_json::Map<String, Value> {
    let mut config = serde_json::Map::new();
    config.insert("web_search".into(), json!("disabled"));
    for key in [
        "tools.update_plan.enabled",
        "tools.experimental_request_user_input.enabled",
        "agents.enabled",
        "orchestrator.mcp.enabled",
        "orchestrator.skills.enabled",
        "skills.include_instructions",
        "skills.bundled.enabled",
    ] {
        config.insert(key.into(), json!(false));
    }
    for feature in [
        "multi_agent_v2",
        "shell_tool",
        "view_image",
        "apps",
        "plugins",
        "hooks",
        "browser_use",
        "computer_use",
        "in_app_local_automation",
        "image_generation",
        "sleep_tool",
        "goals",
        "memories",
        "skill_search",
        "code_mode",
        "code_mode_only",
        "code_mode_host",
        "deferred_executor",
        "token_budget",
    ] {
        config.insert(format!("features.{feature}"), json!(false));
    }
    config
}

fn disable_ambient(
    config: &Value,
    skills: &Value,
    cwd: &str,
) -> Result<serde_json::Map<String, Value>, CoreError> {
    let servers = config
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object)
        .ok_or_else(|| protocol_error("cannot inventory Codex MCP configuration"))?;
    let entries = skills["data"]
        .as_array()
        .ok_or_else(|| protocol_error("cannot inventory Codex skills"))?;
    if entries.len() != 1 || entries[0]["cwd"].as_str() != Some(cwd) {
        return Err(protocol_error(
            "Codex skill inventory did not match the requested workspace",
        ));
    }
    let entry = &entries[0];
    if !entry["errors"].as_array().is_some_and(Vec::is_empty) {
        return Err(protocol_error(
            "Codex reported a skill configuration error; repair it before using this runtime",
        ));
    }
    let skills = entry["skills"]
        .as_array()
        .ok_or_else(|| protocol_error("invalid Codex skill inventory"))?;
    let mut paths = HashSet::new();
    for skill in skills {
        let path = skill["path"]
            .as_str()
            .filter(|path| std::path::Path::new(path).is_absolute())
            .ok_or_else(|| protocol_error("invalid native skill path"))?;
        paths.insert(path.to_string());
    }
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    let mut overrides = static_config();
    overrides.insert(
        "mcp_servers".into(),
        Value::Object(
            servers
                .keys()
                .map(|name| (name.clone(), json!({"enabled":false})))
                .collect(),
        ),
    );
    overrides.insert(
        "skills.config".into(),
        json!(paths
            .into_iter()
            .map(|path| json!({"path":path,"enabled":false}))
            .collect::<Vec<_>>()),
    );
    Ok(overrides)
}

async fn model(wire: &mut Wire, id: &str) -> Result<Value, CoreError> {
    let account = wire
        .request("account/read", json!({"refreshToken":false}))
        .await?;
    if account.pointer("/account/type").and_then(Value::as_str) != Some("chatgpt") {
        return Err(protocol_error(
            "Sign in with a ChatGPT subscription in provider settings first",
        ));
    }
    let mut cursor = Value::Null;
    let mut cursors = HashSet::new();
    for _ in 0..20 {
        let result = wire
            .request(
                "model/list",
                json!({"limit":100,"cursor":cursor,"includeHidden":false}),
            )
            .await?;
        let models = result["data"]
            .as_array()
            .ok_or_else(|| protocol_error("invalid Codex model catalog"))?;
        if let Some(model) = models
            .iter()
            .find(|model| model["model"].as_str() == Some(id))
        {
            return Ok(model.clone());
        }
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
        if !cursors.insert(cursor.to_string()) {
            return Err(protocol_error("Codex model catalog repeated its cursor"));
        }
    }
    Err(protocol_error(
        "the selected model is unavailable to this ChatGPT account; refresh its model list",
    ))
}

fn user_input(text: &str, images: &[(String, String)]) -> Vec<Value> {
    let mut input = vec![json!({"type":"text","text":text,"text_elements":[]})];
    input.extend(
        images
            .iter()
            .map(|(mime, data)| json!({"type":"image","url":format!("data:{mime};base64,{data}")})),
    );
    input
}

pub(super) async fn run(request: SubscriptionTurnRequest) -> Result<Message, CoreError> {
    let cancellation = request.cancellation.clone();
    let model_id = request
        .config
        .model
        .clone()
        .ok_or_else(|| protocol_error("select a Codex model first"))?;
    let bootstrap = async {
        let mut wire = Wire::start().await?;
        let model = model(&mut wire, &model_id).await?;
        let native_vision = model["inputModalities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "image"));
        let effort = request
            .config
            .reasoning_effort
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(protocol_error)?;
        if let Some(effort) = &effort {
            if !model["supportedReasoningEfforts"]
                .as_array()
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| &value["reasoningEffort"] == effort)
                })
            {
                return Err(protocol_error(
                    "the selected Codex model does not support this reasoning effort",
                ));
            }
        }
        let cwd = std::env::current_dir()
            .map_err(protocol_error)?
            .to_string_lossy()
            .to_string();
        let config = wire
            .request("config/read", json!({"includeLayers":false,"cwd":cwd}))
            .await?;
        let skills = wire
            .request("skills/list", json!({"cwds":[cwd],"forceReload":true}))
            .await?;
        let overrides = disable_ambient(&config, &skills, &cwd)?;
        drop(config);
        drop(skills);
        let turn = request.prepare(native_vision)?;
        let tools = turn.tools.definitions().into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"inputSchema":tool.parameters})).collect::<Vec<_>>();
        let response = wire.request("thread/start", json!({"model":model_id,"allowProviderModelFallback":false,"cwd":cwd,"config":overrides,"developerInstructions":turn.system_prompt,"dynamicTools":tools,"environments":[],"selectedCapabilityRoots":[],"approvalPolicy":"never","sandbox":"read-only","ephemeral":true})).await?;
        if response["model"].as_str() != Some(&model_id)
            || response["approvalPolicy"] != "never"
            || response.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly")
        {
            return Err(protocol_error(
                "Codex did not accept the requested model and execution policy",
            ));
        }
        let thread_id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol_error("Codex created no thread"))?
            .to_string();
        let response = wire.request("turn/start", json!({"threadId":thread_id,"input":user_input(&turn.prompt,&turn.images),"effort":effort,"environments":[],"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly","networkAccess":false}})).await?;
        let turn_id = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol_error("Codex created no turn"))?
            .to_string();
        Ok::<_, CoreError>((wire, turn, thread_id, turn_id, native_vision))
    };
    let (mut wire, mut turn, thread_id, turn_id, native_vision) = tokio::select! {
        _ = cancellation.cancelled() => return Err(CoreError::Cancelled("Stopped during Codex connection".into())),
        result = tokio::time::timeout(Duration::from_secs(120), bootstrap) => result.map_err(|_| protocol_error("Codex connection timed out"))??,
    };
    let mut projection = Projection::default();
    let mut pending = FuturesUnordered::new();
    let mut replies = HashMap::new();
    let mut steering_closed = false;
    let mut async_items = HashSet::new();
    let mut server_requests = 0u32;
    let result = async {
        loop {
            tokio::select! {
                biased;
                _ = turn.cancellation.cancelled() => return Err(CoreError::Cancelled("Stopped by user".into())),
                Some((id, output)) = pending.next(), if !pending.is_empty() => {
                    let output: nexa_core::agent::ExternalToolOutput = output?;
                    let mut content = vec![json!({"type":"inputText","text":output.result.content})];
                    content.extend(output.visual_parts.into_iter().filter_map(|part| match part {
                        ContentPart::Text {text} => Some(json!({"type":"inputText","text":text})),
                        ContentPart::Image {media_type,data} => Some(json!({"type":"inputImage","imageUrl":format!("data:{media_type};base64,{data}")})), _=>None,
                    }));
                    wire.write(json!({"id":id,"result":{"contentItems":content,"success":!output.result.is_error}})).await?;
                }
                message = turn.steering.recv(), if !steering_closed => match message {
                    Some(message) => {
                        if replies.len() >= 64 { return Err(protocol_error("too many unacknowledged steering messages")); }
                        if message.recovery_control.is_some() { return Err(protocol_error("Codex owns recovery; start a new turn to change its reasoning mode")); }
                        let images = message.parts.into_iter().filter_map(|part| match part { ContentPart::Image {media_type,data}=>Some((media_type,data)),_=>None }).collect::<Vec<_>>();
                        if !native_vision && !images.is_empty() { return Err(protocol_error("this Codex model does not accept images")); }
                        let id = wire.send("turn/steer",json!({"threadId":thread_id,"expectedTurnId":turn_id,"input":user_input(&message.content,&images)})).await?;
                        replies.insert(id,message.content);
                    }
                    None => steering_closed = true,
                },
                message = wire.receive() => {
                    let message = message?;
                    if let Some(id) = message["id"].as_u64() {
                        if message.get("method").is_none() {
                            if let Some(content) = replies.remove(&id) {
                                rpc_result(message,"turn/steer")?;
                                turn.events.send(AgentEvent::Steering {content}).await.map_err(protocol_error)?;
                            }
                            continue;
                        }
                    }
                    let method = message["method"].as_str().unwrap_or_default();
                    let params = &message["params"];
                    if message.get("id").is_some() {
                        server_requests += 1;
                        if server_requests > 256 { wire.reject(&message).await?; return Err(protocol_error("Codex exceeded the per-turn native request budget")); }
                        if params["threadId"].as_str() != Some(&thread_id) { wire.reject(&message).await?; return Err(protocol_error("Codex request belongs to another thread")); }
                        match method {
                            "currentTime/read" => {
                                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(protocol_error)?.as_secs();
                                wire.write(json!({"id":message["id"],"result":{"currentTimeAt":now}})).await?;
                            }
                            "item/tool/call" => {
                                if params["turnId"].as_str() != Some(&turn_id) || pending.len() >= 16 { wire.reject(&message).await?; return Err(protocol_error("invalid Codex tool dispatch boundary")); }
                                let call = ToolCallRequest {id:required_string(params,"callId")?,name:required_string(params,"tool")?,arguments:params["arguments"].to_string(),thought_signature:None};
                                let tools = turn.tools.clone(); let id = message["id"].clone();
                                projection.answer.clear();
                                pending.push(async move { (id,tools.execute(call).await) });
                            }
                            _ => { wire.reject(&message).await?; return Err(protocol_error(format!("Codex requested unsupported native capability {method}; use Nexa tools"))); }
                        }
                        continue;
                    }
                    if params["threadId"].as_str().is_some_and(|id| id != thread_id) { continue; }
                    if params["turnId"].as_str().is_some_and(|id| id != turn_id) { continue; }
                    if project_event(&mut projection,&turn,&mut async_items,method,params).await? {
                        if !pending.is_empty() || !replies.is_empty() { return Err(protocol_error("Codex completed with unacknowledged work")); }
                        return Ok(());
                    }
                }
            }
        }
    }.await;
    turn.cancellation.cancel();
    // Poll cancellation through shared dispatch before dropping pending futures.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while pending.next().await.is_some() {}
    })
    .await;
    if result.is_err() {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            wire.send(
                "turn/interrupt",
                json!({"threadId":thread_id,"turnId":turn_id}),
            ),
        )
        .await;
    }
    let _ = wire.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), wire.child.wait()).await;
    match result {
        Ok(()) => projection.finish(&turn).await,
        Err(error) => {
            if !projection.answer.is_empty() {
                turn.tools.persist_answer(&projection.answer).await?;
            }
            Err(error)
        }
    }
}

fn required_string(value: &Value, key: &str) -> Result<String, CoreError> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| protocol_error(format!("missing Codex {key}")))
}

async fn project_event(
    projection: &mut Projection,
    turn: &PreparedTurn,
    async_items: &mut HashSet<String>,
    method: &str,
    params: &Value,
) -> Result<bool, CoreError> {
    match method {
        "item/agentMessage/delta" => {
            projection
                .delta(
                    &turn.events,
                    &required_string(params, "itemId")?,
                    StreamBlockChannel::Answer,
                    params["delta"].as_str().unwrap_or_default(),
                )
                .await?
        }
        "item/reasoning/summaryTextDelta" => {
            projection
                .delta(
                    &turn.events,
                    &format!(
                        "reasoning:{}:{}",
                        required_string(params, "itemId")?,
                        params["summaryIndex"]
                    ),
                    StreamBlockChannel::Thinking,
                    params["delta"].as_str().unwrap_or_default(),
                )
                .await?
        }
        "item/completed" => {
            let item = &params["item"];
            if item["type"] == "agentMessage" {
                let id = required_string(item, "id")?;
                let text = item["text"].as_str().unwrap_or_default();
                if item["delivery"] == "async" {
                    if async_items.len() >= 256 {
                        return Err(protocol_error("Codex async message budget exceeded"));
                    }
                    if async_items.insert(id.clone()) {
                        let mut text = text.to_string();
                        if let Some(questions) = item["questions"].as_array() {
                            for question in questions {
                                text.push_str(&format!(
                                    "\n\n{}",
                                    question["title"].as_str().unwrap_or_default()
                                ));
                                if let Some(options) = question["options"].as_array() {
                                    for option in options {
                                        if let Some(option) = option.as_str() {
                                            text.push_str(&format!("\n- {option}"));
                                        }
                                    }
                                }
                            }
                        }
                        // This durable intermediate message is not a Done event.
                        turn.tools.persist_answer(&text).await?;
                        projection
                            .delta(&turn.events, &id, StreamBlockChannel::Answer, &text)
                            .await?;
                    }
                } else {
                    projection.complete(&turn.events, &id, text).await?;
                    if item["phase"] == "commentary" {
                        projection.answer.clear();
                    }
                }
            }
        }
        "thread/tokenUsage/updated" => {
            let usage = &params["tokenUsage"]["total"];
            let tokens = |key: &str| usage[key].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
            projection.usage.prompt_tokens = tokens("inputTokens");
            projection.last_prompt_tokens = params["tokenUsage"]["last"]["inputTokens"]
                .as_u64()
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32;
            projection.usage.completion_tokens = tokens("outputTokens");
            projection.usage.total_tokens = tokens("totalTokens");
        }
        "turn/completed" => match params["turn"]["status"].as_str() {
            Some("completed") => return Ok(true),
            Some("interrupted") => {
                return Err(CoreError::Cancelled("Codex turn interrupted".into()))
            }
            _ => {
                return Err(protocol_error(
                    params["turn"]["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex turn failed"),
                ))
            }
        },
        "error" if params["willRetry"] != true => {
            return Err(protocol_error(
                params["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex runtime failed"),
            ))
        }
        _ => {}
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn async_question_is_durable_and_never_becomes_final_answer() {
        let (request, mut rx, _, _) =
            super::super::tests::fixture(SubscriptionRuntimeKind::Codex, "test");
        let db = request.db.clone();
        let conversation = request.conversation_id.clone();
        let turn = request.prepare(false).unwrap();
        let mut projection = Projection::default();
        let mut seen = HashSet::new();
        let params = json!({"item":{"type":"agentMessage","id":"q1","phase":"final_answer","delivery":"async","text":"Choose a region","questions":[{"title":"Region","options":["Beijing","Singapore"]}]}});
        assert!(
            !project_event(&mut projection, &turn, &mut seen, "item/completed", &params)
                .await
                .unwrap()
        );
        assert!(
            !project_event(&mut projection, &turn, &mut seen, "item/completed", &params)
                .await
                .unwrap()
        );
        assert!(projection.answer.is_empty());
        let history = db.get_messages(&conversation).unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == nexa_core::llm::Role::Assistant)
                .count(),
            1
        );
        assert!(history.last().unwrap().content.contains("Singapore"));
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::Done { .. }));
        }
    }
    #[tokio::test]
    #[ignore = "uses the official ChatGPT subscription for one read-only tool inference"]
    async fn native_codex_executes_nexa_tool_and_streams_persisted_answer() {
        let mut wire = Wire::start().await.unwrap();
        let catalog = wire
            .request("model/list", json!({"limit":100,"includeHidden":false}))
            .await
            .unwrap();
        let models = catalog["data"].as_array().unwrap();
        let model = models
            .iter()
            .find(|model| model["model"] == "gpt-5.4-mini")
            .or_else(|| models.iter().find(|model| model["isDefault"] == true))
            .unwrap()["model"]
            .as_str()
            .unwrap()
            .to_string();
        drop(wire);
        super::super::tests::run_live(SubscriptionRuntimeKind::Codex, &model).await;
    }
    #[tokio::test]
    #[ignore = "requires the user's official Codex CLI and login; no model inference"]
    async fn native_codex_subscription_preflight() {
        let mut wire = Wire::start().await.unwrap();
        let account = wire
            .request("account/read", json!({"refreshToken":false}))
            .await
            .unwrap();
        assert_eq!(
            account.pointer("/account/type").and_then(Value::as_str),
            Some("chatgpt"),
            "ChatGPT login required"
        );
        let models = wire
            .request("model/list", json!({"limit":100,"includeHidden":false}))
            .await
            .unwrap();
        let model = models["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["isDefault"] == true)
            .unwrap();
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let config = wire
            .request("config/read", json!({"includeLayers":false,"cwd":cwd}))
            .await
            .unwrap();
        let skills = wire
            .request("skills/list", json!({"cwds":[cwd],"forceReload":true}))
            .await
            .unwrap();
        let overrides = disable_ambient(&config, &skills, &cwd).unwrap();
        drop(config);
        drop(skills);
        let result = wire.request("thread/start",json!({"model":model["model"],"allowProviderModelFallback":false,"cwd":cwd,"config":overrides,"developerInstructions":"Nexa integration preflight. No model turn will be requested.","dynamicTools":[],"environments":[],"selectedCapabilityRoots":[],"approvalPolicy":"never","sandbox":"read-only","ephemeral":true})).await.unwrap();
        assert_eq!(result["model"], model["model"]);
        assert_eq!(result["approvalPolicy"], "never");
        assert_eq!(result["sandbox"]["type"], "readOnly");
    }
    #[test]
    fn ambient_inventory_preserves_literal_server_names_and_rejects_scan_failure() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let config =
            json!({"config":{"mcp_servers":{"team.tools":{"env":{"SECRET":"never-copy"}}}}});
        let skills = json!({"data":[{"cwd":cwd,"skills":[],"errors":[]}]});
        let disabled = disable_ambient(&config, &skills, &cwd).unwrap();
        assert_eq!(
            disabled["mcp_servers"],
            json!({"team.tools":{"enabled":false}})
        );
        assert!(!serde_json::to_string(&disabled).unwrap().contains("SECRET"));
        let broken = json!({"data":[{"cwd":cwd,"skills":[],"errors":[{"message":"load failed"}]}]});
        assert!(disable_ambient(&config, &broken, &cwd).is_err());
        assert!(disable_ambient(&json!({}), &skills, &cwd).is_err());
    }
}
