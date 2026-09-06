//! Runtime guardrails for unproductive agent loops.

use serde_json::Value;
use std::collections::HashMap;

use crate::llm::ToolCallRequest;

const REPEATED_TOOL_SIGNATURE_THRESHOLD: u32 = 3;
const REPEATED_TEXT_FINGERPRINT_THRESHOLD: u32 = 3;
const CONSECUTIVE_TOOL_ERROR_THRESHOLD: u32 = 4;
const NON_ACTION_TOOL_STEP_THRESHOLD: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopGuardAction {
    BlockToolCalls,
    ChangeStrategy,
    StopLoop,
}

impl LoopGuardAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LoopGuardAction::BlockToolCalls => "blockToolCalls",
            LoopGuardAction::ChangeStrategy => "changeStrategy",
            LoopGuardAction::StopLoop => "stopLoop",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LoopGuardIntervention {
    pub(crate) reason: String,
    pub(crate) action: LoopGuardAction,
    pub(crate) prompt: String,
}

#[derive(Debug, Default)]
pub(crate) struct AgentLoopGuard {
    last_tool_signature: Option<String>,
    repeated_tool_signature_count: u32,
    repeated_tool_intervention_used: bool,
    observation_fingerprints: HashMap<String, blake3::Hash>,
    discovery_fingerprints: std::collections::HashSet<(String, blake3::Hash)>,
    repeated_discovery_results: u32,
    discovery_intervention_used: bool,
    last_text_shape: Option<String>,
    repeated_text_fingerprint_count: u32,
    repeated_text_intervention_used: bool,
    consecutive_tool_errors: u32,
    tool_error_intervention_used: bool,
    consecutive_non_action_tool_steps: u32,
    bookkeeping_intervention_used: bool,
    last_protocol_fault_signature: Option<String>,
    consecutive_protocol_fault_count: u32,
    protocol_fault_intervention_used: bool,
}

impl AgentLoopGuard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record_protocol_progress(&mut self) {
        self.last_protocol_fault_signature = None;
        self.consecutive_protocol_fault_count = 0;
        self.protocol_fault_intervention_used = false;
    }

    pub(crate) fn observe_model_step(
        &mut self,
        assistant_text: &str,
        tool_calls: &[ToolCallRequest],
    ) -> Option<LoopGuardIntervention> {
        self.record_protocol_progress();
        if !tool_calls.is_empty() {
            let has_action_progress = tool_calls
                .iter()
                .any(|call| tool_call_is_action_progress(&call.name));
            if !has_action_progress {
                self.consecutive_non_action_tool_steps =
                    self.consecutive_non_action_tool_steps.saturating_add(1);
                if self.consecutive_non_action_tool_steps >= NON_ACTION_TOOL_STEP_THRESHOLD {
                    if self.bookkeeping_intervention_used {
                        return Some(LoopGuardIntervention {
                            reason: "The model continued emitting only plan or goal bookkeeping after the controller blocked that loop.".to_string(),
                            action: LoopGuardAction::StopLoop,
                            prompt: String::new(),
                        });
                    }
                    self.bookkeeping_intervention_used = true;
                    return Some(LoopGuardIntervention {
                        reason: "The model emitted consecutive plan or goal bookkeeping steps without a concrete task action.".to_string(),
                        action: LoopGuardAction::BlockToolCalls,
                        prompt: "## Loop Guard\nPlan and goal tools are optional bookkeeping, not prerequisites for task execution. Stop updating them now. Either call the concrete evidence or action tool, provide a truthful final answer, or ask one focused question.".to_string(),
                    });
                }
            }
            let signature = tool_call_batch_signature(tool_calls);
            if self.last_tool_signature.as_deref() == Some(signature.as_str()) {
                self.repeated_tool_signature_count =
                    self.repeated_tool_signature_count.saturating_add(1);
            } else {
                self.last_tool_signature = Some(signature);
                self.repeated_tool_signature_count = 1;
                self.repeated_tool_intervention_used = false;
                self.observation_fingerprints.clear();
            }

            if self.repeated_tool_signature_count >= REPEATED_TOOL_SIGNATURE_THRESHOLD {
                if self.repeated_tool_intervention_used {
                    return Some(LoopGuardIntervention {
                        reason: "The model repeated the same tool-call batch after the bounded loop-guard intervention."
                            .to_string(),
                        action: LoopGuardAction::StopLoop,
                        prompt: String::new(),
                    });
                }
                self.repeated_tool_intervention_used = true;
                return Some(LoopGuardIntervention {
                    reason: format!(
                        "The model requested the same tool call batch {} times without visible progress.",
                        self.repeated_tool_signature_count
                    ),
                    action: LoopGuardAction::BlockToolCalls,
                    prompt: "## Loop Guard\nThe previous tool-call batch was blocked because it repeated the same tool names and arguments without progress. Do not retry the same calls. Summarize what is known, choose a different tool/argument strategy, or ask the user for the missing information.".to_string(),
                });
            }

            // Only a non-blocked executable action satisfies the boundary.
            // Plan/goal bookkeeping is structured, but it is not evidence that
            // the requested project action occurred.
            if has_action_progress {
                self.consecutive_non_action_tool_steps = 0;
                self.bookkeeping_intervention_used = false;
            }
            self.last_text_shape = None;
            self.repeated_text_fingerprint_count = 0;
            self.repeated_text_intervention_used = false;
            return None;
        }

        let shape = normalize_text_shape(assistant_text)?;
        if self
            .last_text_shape
            .as_deref()
            .is_some_and(|previous| text_shapes_match(previous, &shape))
        {
            self.repeated_text_fingerprint_count =
                self.repeated_text_fingerprint_count.saturating_add(1);
        } else {
            self.last_text_shape = Some(shape);
            self.repeated_text_fingerprint_count = 1;
            self.repeated_text_intervention_used = false;
        }

        if self.repeated_text_fingerprint_count >= REPEATED_TEXT_FINGERPRINT_THRESHOLD {
            if self.repeated_text_intervention_used {
                return Some(LoopGuardIntervention {
                    reason: "The model repeated the same answer shape after the bounded loop-guard intervention."
                        .to_string(),
                    action: LoopGuardAction::StopLoop,
                    prompt: String::new(),
                });
            }
            self.repeated_text_intervention_used = true;
            return Some(LoopGuardIntervention {
                reason: "The model produced the same final-text shape repeatedly without taking a new action."
                    .to_string(),
                action: LoopGuardAction::ChangeStrategy,
                prompt: "## Loop Guard\nThe last draft repeated an earlier answer shape without new progress. Make a concrete decision now: either provide the final answer with explicit uncertainty, use a different tool strategy, or ask one focused question.".to_string(),
            });
        }

        None
    }

    /// Observe a provider sample that was rejected before dispatch. The guard
    /// tracks consecutive rejected protocol samples and resets on the next
    /// accepted model step. Payload changes do not count as committed progress,
    /// so this remains a no-progress safety valve rather than a turn-wide retry
    /// maximum.
    pub(crate) fn observe_protocol_rejection(
        &mut self,
        fault_code: &str,
        tool_calls: &[ToolCallRequest],
    ) -> Option<LoopGuardIntervention> {
        let call_signature = if tool_calls.is_empty() {
            "no-call-envelope".to_string()
        } else {
            tool_call_batch_signature(tool_calls)
        };
        let signature = format!("{fault_code}:{call_signature}");
        let repeated_equivalent =
            self.last_protocol_fault_signature.as_deref() == Some(signature.as_str());
        self.last_protocol_fault_signature = Some(signature);
        self.consecutive_protocol_fault_count =
            self.consecutive_protocol_fault_count.saturating_add(1);

        if self.consecutive_protocol_fault_count < REPEATED_TOOL_SIGNATURE_THRESHOLD {
            return None;
        }
        if self.protocol_fault_intervention_used {
            return Some(LoopGuardIntervention {
                reason: format!(
                    "The provider returned {} consecutive rejected tool protocol envelopes after a strategy-change instruction; the latest envelope was {}.",
                    self.consecutive_protocol_fault_count,
                    if repeated_equivalent { "equivalent to the prior draft" } else { "different but still uncommitted" },
                ),
                action: LoopGuardAction::StopLoop,
                prompt: String::new(),
            });
        }
        self.protocol_fault_intervention_used = true;
        Some(LoopGuardIntervention {
            reason: format!(
                "The provider returned {} consecutive rejected tool protocol envelopes without a committed tool round; the latest envelope was {}.",
                self.consecutive_protocol_fault_count,
                if repeated_equivalent { "equivalent to the prior draft" } else { "different but still uncommitted" },
            ),
            action: LoopGuardAction::ChangeStrategy,
            prompt: "## Tool Protocol Recovery\nThe same incomplete or output-truncated tool envelope has repeated without any committed progress. Do not resend that payload shape. Reduce the payload, split long writes into create plus bounded append calls, or choose a different safe tool strategy.".to_string(),
        })
    }

    pub(crate) fn observe_tool_result(
        &mut self,
        call: &ToolCallRequest,
        is_error: bool,
        content: &str,
        artifacts: Option<&Value>,
    ) -> Option<LoopGuardIntervention> {
        if !is_error && tool_call_is_discovery(&call.name) {
            // Compare actual results across alternating discovery tools and
            // changing queries. A new page remains progress; new call IDs do not.
            let fingerprint = (
                call.name.clone(),
                discovery_result_fingerprint(call, content, artifacts),
            );
            if self.discovery_fingerprints.insert(fingerprint) {
                self.repeated_discovery_results = 0;
                self.discovery_intervention_used = false;
            } else {
                self.repeated_discovery_results += 1;
                if self.repeated_discovery_results >= 3 {
                    let stop = self.discovery_intervention_used;
                    self.discovery_intervention_used = true;
                    return Some(LoopGuardIntervention {
                        reason: "Discovery keeps returning already-seen entries without reading evidence or executing the task.".into(),
                        action: if stop { LoopGuardAction::StopLoop } else { LoopGuardAction::ChangeStrategy },
                        prompt: "## Loop Guard\nThe discovery results are already available. Use a returned exact file path with read_file/read_files, or call the discovered tool now. Do not list or search again unless a new scope, page, or changed result is needed. If an advertised tool is unavailable, report that concrete limitation instead of repeating discovery.".into(),
                    });
                }
            }
        } else if !is_error && tool_call_is_action_progress(&call.name) {
            self.discovery_fingerprints.clear();
            self.repeated_discovery_results = 0;
            self.discovery_intervention_used = false;
        }
        // Only a read-only observation with a changed content receipt proves
        // progress here. New IDs/timestamps and action successes do not.
        if !is_error && call.name == "browser_session" {
            let action = serde_json::from_str::<Value>(&call.arguments).ok();
            if action
                .as_ref()
                .and_then(|args| args.get("action"))
                .and_then(Value::as_str)
                == Some("observe")
            {
                if let Some(data) = artifacts.and_then(|value| value.get("data")) {
                    if let Some(content_hash) = data
                        .get("contentHash")
                        .and_then(Value::as_str)
                        .filter(|hash| !hash.is_empty())
                    {
                        let fingerprint = blake3::hash(serde_json::json!({
                            "url": data.get("url"),
                            "contentHash": content_hash,
                            "screenshotHash": data.get("screenshotHash").or_else(|| data.pointer("/screenshot/contentHash")),
                        }).to_string().as_bytes());
                        let invocation = tool_call_batch_signature(std::slice::from_ref(call));
                        if self
                            .observation_fingerprints
                            .insert(invocation, fingerprint)
                            .is_some_and(|previous| previous != fingerprint)
                        {
                            self.repeated_tool_signature_count = 0;
                            self.repeated_tool_intervention_used = false;
                        }
                    }
                }
            }
        }
        if is_error {
            self.consecutive_tool_errors = self.consecutive_tool_errors.saturating_add(1);
        } else {
            self.consecutive_tool_errors = 0;
            self.tool_error_intervention_used = false;
        }

        if self.consecutive_tool_errors >= CONSECUTIVE_TOOL_ERROR_THRESHOLD {
            if self.tool_error_intervention_used {
                return Some(LoopGuardIntervention {
                    reason: format!(
                        "Observed {} consecutive tool errors after the bounded loop-guard intervention.",
                        self.consecutive_tool_errors
                    ),
                    action: LoopGuardAction::StopLoop,
                    prompt: String::new(),
                });
            }
            self.tool_error_intervention_used = true;
            return Some(LoopGuardIntervention {
                reason: format!(
                    "Observed {} consecutive tool errors.",
                    self.consecutive_tool_errors
                ),
                action: LoopGuardAction::ChangeStrategy,
                prompt: "## Loop Guard\nSeveral tool calls failed in a row. Stop retrying the same approach. Inspect the latest error, reduce scope, change arguments, or answer with the recoverable limitation if the task cannot proceed.".to_string(),
            });
        }

        None
    }
}

fn tool_call_batch_signature(tool_calls: &[ToolCallRequest]) -> String {
    let mut signatures = tool_calls
        .iter()
        .map(|call| {
            format!(
                "{}:{}",
                call.name,
                canonical_json_text(&call.arguments).unwrap_or_else(|| call.arguments.clone())
            )
        })
        .collect::<Vec<_>>();
    signatures.sort();
    signatures.join("|")
}

fn tool_call_is_discovery(name: &str) -> bool {
    matches!(
        name,
        "tool_search"
            | "list_dir"
            | "list_sources"
            | "list_documents"
            | "glob_files"
            | "search_files"
            | "grep_files"
            | "list_subagent_models"
    )
}

fn discovery_result_fingerprint(
    call: &ToolCallRequest,
    content: &str,
    artifacts: Option<&Value>,
) -> blake3::Hash {
    if call.name == "tool_search" {
        if let Some(matches) = artifacts
            .and_then(|value| {
                value
                    .get("matches")
                    .or_else(|| value.pointer("/artifacts/matches"))
            })
            .and_then(Value::as_array)
        {
            // Query echo, relevance ranking, and scores do not make the same
            // advertised capabilities new evidence.
            let mut entries: Vec<_> = matches.iter().map(|item| {
                serde_json::json!({"name": item.get("name"), "description": item.get("description")}).to_string()
            }).collect();
            entries.sort();
            return blake3::hash(entries.join("\n").as_bytes());
        }
    }
    blake3::hash(content.as_bytes())
}

pub(crate) fn tool_call_is_action_progress(name: &str) -> bool {
    name != "update_plan"
}

fn canonical_json_text(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    serde_json::to_string(&canonicalize_value(value)).ok()
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_value).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_value(value)))
                    .collect(),
            )
        }
        other => other,
    }
}

fn normalize_text_shape(text: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut in_digit_run = false;
    for character in text.chars().take(512) {
        if character.is_numeric() {
            if !in_digit_run {
                normalized.push('#');
                in_digit_run = true;
            }
        } else if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
            in_digit_run = false;
        } else {
            in_digit_run = false;
        }
    }
    if normalized.chars().count() < 12 {
        return None;
    }
    Some(normalized)
}

fn text_shapes_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left = character_ngrams(left, 2);
    let right = character_ngrams(right, 2);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let intersection = left
        .iter()
        .map(|(gram, count)| count.min(right.get(gram).unwrap_or(&0)))
        .sum::<usize>();
    let left_count = left.values().sum::<usize>();
    let right_count = right.values().sum::<usize>();
    intersection.saturating_mul(2).saturating_mul(100)
        >= (left_count + right_count).saturating_mul(50)
}

fn character_ngrams(value: &str, width: usize) -> HashMap<String, usize> {
    let characters = value.chars().collect::<Vec<_>>();
    let mut grams = HashMap::new();
    for window in characters.windows(width) {
        *grams.entry(window.iter().collect()).or_insert(0) += 1;
    }
    grams
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "call".to_string(),
            name: "read_file".to_string(),
            arguments: arguments.to_string(),
            thought_signature: None,
        }
    }

    #[test]
    fn detects_repeated_tool_signature_with_reordered_json() {
        let mut guard = AgentLoopGuard::new();
        assert!(guard
            .observe_model_step("", &[call(r#"{"b":2,"a":1}"#)])
            .is_none());
        assert!(guard
            .observe_model_step("", &[call(r#"{"a":1,"b":2}"#)])
            .is_none());
        let intervention = guard
            .observe_model_step("", &[call(r#"{"a":1,"b":2}"#)])
            .expect("third identical call should be blocked");

        assert_eq!(intervention.action, LoopGuardAction::BlockToolCalls);
        assert_eq!(
            guard
                .observe_model_step("", &[call(r#"{"a":1,"b":2}"#)])
                .unwrap()
                .action,
            LoopGuardAction::StopLoop
        );
    }

    #[test]
    fn alternating_discovery_without_new_results_changes_strategy_then_stops() {
        let mut guard = AgentLoopGuard::new();
        let mut list = call(r#"{"path":"src"}"#);
        list.name = "list_dir".into();
        let mut search = call(r#"{"query":"read files"}"#);
        search.name = "tool_search".into();
        let calls = [list, search];
        let mut actions = Vec::new();
        for index in 0..8 {
            let call = &calls[index % 2];
            assert!(guard
                .observe_model_step("", std::slice::from_ref(call))
                .is_none());
            if let Some(intervention) =
                guard.observe_tool_result(call, false, "same discovered entries", None)
            {
                actions.push(intervention.action);
            }
        }
        assert_eq!(actions.first(), Some(&LoopGuardAction::ChangeStrategy));
        assert!(actions.contains(&LoopGuardAction::StopLoop));
    }

    #[test]
    fn discovery_pagination_and_reading_evidence_are_progress() {
        let mut guard = AgentLoopGuard::new();
        for index in 0..12 {
            let mut list = call(&format!(r#"{{"cursor":{index}}}"#));
            list.name = "list_documents".into();
            assert!(guard.observe_model_step("", &[list.clone()]).is_none());
            assert!(guard
                .observe_tool_result(&list, false, &format!("page {index}"), None)
                .is_none());
        }
        assert!(guard
            .observe_tool_result(&call("{}"), false, "actual file content", None)
            .is_none());
        assert!(guard.discovery_fingerprints.is_empty());
    }

    #[test]
    fn rephrasing_tool_search_does_not_make_the_same_matches_new_evidence() {
        let mut guard = AgentLoopGuard::new();
        let mut intervention = None;
        for index in 0..4 {
            let mut search = call(&format!(r#"{{"query":"read files {index}"}}"#));
            search.name = "tool_search".into();
            assert!(guard.observe_model_step("", &[search.clone()]).is_none());
            intervention = guard.observe_tool_result(
                &search,
                false,
                &format!("query echo {index}"),
                Some(&serde_json::json!({
                    "kind": "toolSearchResults", "query": index.to_string(),
                    "matches": [{"name":"read_file", "description":"Read a file", "score":index}],
                })),
            );
        }
        assert_eq!(
            intervention.unwrap().action,
            LoopGuardAction::ChangeStrategy
        );
    }

    #[test]
    fn changed_browser_observations_are_progress_but_random_ids_are_not() {
        for screenshot_field in ["screenshotHash", "screenshot"] {
            let mut guard = AgentLoopGuard::new();
            let mut observe = call(r#"{"action":"observe","tabId":"tab"}"#);
            observe.name = "browser_session".into();
            for index in 0..8 {
                assert!(guard
                    .observe_model_step("", std::slice::from_ref(&observe))
                    .is_none());
                let mut data = serde_json::json!({"contentHash":"same-dom", "url":"https://example.com", "observationId":format!("random-{index}")});
                data[screenshot_field] = if screenshot_field == "screenshot" {
                    serde_json::json!({"contentHash":index.to_string()})
                } else {
                    serde_json::json!(index.to_string())
                };
                assert!(guard
                    .observe_tool_result(
                        &observe,
                        false,
                        "",
                        Some(&serde_json::json!({"data": data}))
                    )
                    .is_none());
            }
        }
        let mut guard = AgentLoopGuard::new();
        let mut observe = call(r#"{"action":"observe"}"#);
        observe.name = "browser_session".into();
        for index in 0..2 {
            assert!(guard
                .observe_model_step("", std::slice::from_ref(&observe))
                .is_none());
            guard.observe_tool_result(&observe, false, "", Some(&serde_json::json!({"data":{"contentHash":"same", "observationId":index, "timestamp":index}})));
        }
        assert_eq!(
            guard.observe_model_step("", &[observe]).unwrap().action,
            LoopGuardAction::BlockToolCalls
        );
    }

    #[test]
    fn changed_browser_receipts_do_not_authorize_repeated_actions_or_errors() {
        for (action, is_error) in [("click", false), ("observe", true)] {
            let mut guard = AgentLoopGuard::new();
            let mut request = call(&serde_json::json!({"action": action}).to_string());
            request.name = "browser_session".into();
            for index in 0..2 {
                assert!(guard
                    .observe_model_step("", std::slice::from_ref(&request))
                    .is_none());
                guard.observe_tool_result(
                    &request,
                    is_error,
                    "",
                    Some(&serde_json::json!({"data":{"contentHash":index.to_string()}})),
                );
            }
            assert_eq!(
                guard.observe_model_step("", &[request]).unwrap().action,
                LoopGuardAction::BlockToolCalls
            );
        }
    }

    #[test]
    fn detects_consecutive_tool_errors() {
        let mut guard = AgentLoopGuard::new();
        assert!(guard
            .observe_tool_result(&call("{}"), true, "", None)
            .is_none());
        assert!(guard
            .observe_tool_result(&call("{}"), true, "", None)
            .is_none());
        assert!(guard
            .observe_tool_result(&call("{}"), true, "", None)
            .is_none());
        assert_eq!(
            guard
                .observe_tool_result(&call("{}"), true, "", None)
                .unwrap()
                .action,
            LoopGuardAction::ChangeStrategy
        );
        assert_eq!(
            guard
                .observe_tool_result(&call("{}"), true, "", None)
                .unwrap()
                .action,
            LoopGuardAction::StopLoop
        );
        assert!(guard
            .observe_tool_result(&call("{}"), false, "", None)
            .is_none());
    }

    #[test]
    fn repeated_plan_bookkeeping_is_blocked_then_stopped() {
        let mut guard = AgentLoopGuard::new();
        let plan_call = ToolCallRequest {
            id: "plan".to_string(),
            name: "update_plan".to_string(),
            arguments: r#"{"steps":[{"step":"write","status":"in_progress"}]}"#.to_string(),
            thought_signature: None,
        };

        assert!(guard
            .observe_model_step("", std::slice::from_ref(&plan_call))
            .is_none());
        assert_eq!(
            guard
                .observe_model_step("", std::slice::from_ref(&plan_call))
                .unwrap()
                .action,
            LoopGuardAction::BlockToolCalls
        );
        assert_eq!(
            guard.observe_model_step("", &[plan_call]).unwrap().action,
            LoopGuardAction::StopLoop
        );
    }

    #[test]
    fn concrete_action_resets_the_bookkeeping_loop_window() {
        let mut guard = AgentLoopGuard::new();
        let plan_call = ToolCallRequest {
            id: "plan".to_string(),
            name: "update_plan".to_string(),
            arguments: r#"{"steps":[{"step":"write","status":"in_progress"}]}"#.to_string(),
            thought_signature: None,
        };

        assert!(guard
            .observe_model_step("", std::slice::from_ref(&plan_call))
            .is_none());
        assert_eq!(
            guard
                .observe_model_step("", std::slice::from_ref(&plan_call))
                .unwrap()
                .action,
            LoopGuardAction::BlockToolCalls
        );
        assert!(guard
            .observe_model_step("", &[call(r#"{"path":"shader.html"}"#)])
            .is_none());
        assert!(guard.observe_model_step("", &[plan_call]).is_none());
    }

    #[test]
    fn short_multilingual_progress_promises_are_nudged_once_then_stopped() {
        let mut guard = AgentLoopGuard::new();
        let samples = [
            "直接追加第 3 块——物理着色器与 WebGL 初始化。",
            "继续写入文件第 3 块——着色器与 WebGL 初始化。",
            "不再更新计划，直接追加第 3 块着色器和 WebGL 初始化。",
            "停止更新计划，直接写入第 3 块 WebGL 着色器初始化。",
        ];

        assert!(guard.observe_model_step(samples[0], &[]).is_none());
        assert!(guard.observe_model_step(samples[1], &[]).is_none());
        assert_eq!(
            guard.observe_model_step(samples[2], &[]).unwrap().action,
            LoopGuardAction::ChangeStrategy
        );
        assert_eq!(
            guard.observe_model_step(samples[3], &[]).unwrap().action,
            LoopGuardAction::StopLoop
        );
    }

    #[test]
    fn goal_state_changes_count_as_concrete_actions() {
        let mut guard = AgentLoopGuard::new();
        for name in ["create_goal", "update_goal"] {
            let goal_call = ToolCallRequest {
                id: name.to_string(),
                name: name.to_string(),
                arguments: r#"{"status":"complete"}"#.to_string(),
                thought_signature: None,
            };
            assert!(guard.observe_model_step("", &[goal_call]).is_none());
        }
    }

    #[test]
    fn repeated_protocol_rejections_nudge_once_then_stop_and_reset_on_progress() {
        let mut guard = AgentLoopGuard::new();
        let rejected = call(r#"{"path":"large.html","content":"partial"}"#);

        assert!(guard
            .observe_protocol_rejection("output_limit", std::slice::from_ref(&rejected))
            .is_none());
        assert!(guard
            .observe_protocol_rejection("output_limit", std::slice::from_ref(&rejected))
            .is_none());
        assert_eq!(
            guard
                .observe_protocol_rejection("output_limit", std::slice::from_ref(&rejected))
                .unwrap()
                .action,
            LoopGuardAction::ChangeStrategy
        );
        assert_eq!(
            guard
                .observe_protocol_rejection("output_limit", std::slice::from_ref(&rejected))
                .unwrap()
                .action,
            LoopGuardAction::StopLoop
        );

        assert!(guard.observe_model_step("done", &[]).is_none());
        assert!(guard
            .observe_protocol_rejection("output_limit", &[rejected])
            .is_none());
    }

    #[test]
    fn changing_rejected_payloads_do_not_evade_the_no_progress_guard() {
        let mut guard = AgentLoopGuard::new();
        for index in 0..2 {
            let rejected = call(&format!(r#"{{"attempt":{index}}}"#));
            assert!(guard
                .observe_protocol_rejection("output_limit", &[rejected])
                .is_none());
        }
        let changed_again = call(r#"{"attempt":2}"#);
        assert_eq!(
            guard
                .observe_protocol_rejection("output_limit", &[changed_again])
                .unwrap()
                .action,
            LoopGuardAction::ChangeStrategy
        );
    }

    #[test]
    fn provider_native_progress_resets_rejected_protocol_streak() {
        let mut guard = AgentLoopGuard::new();
        let rejected = call(r#"{"draft":"uncommitted"}"#);
        for _ in 0..2 {
            assert!(guard
                .observe_protocol_rejection("provider_pause", std::slice::from_ref(&rejected))
                .is_none());
            guard.record_protocol_progress();
        }

        for _ in 0..2 {
            assert!(guard
                .observe_protocol_rejection("provider_pause", std::slice::from_ref(&rejected))
                .is_none());
        }
        assert_eq!(
            guard
                .observe_protocol_rejection("provider_pause", &[rejected])
                .unwrap()
                .action,
            LoopGuardAction::ChangeStrategy
        );
    }
}
