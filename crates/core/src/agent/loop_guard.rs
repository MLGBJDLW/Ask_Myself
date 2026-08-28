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
    last_text_shape: Option<String>,
    repeated_text_fingerprint_count: u32,
    repeated_text_intervention_used: bool,
    consecutive_tool_errors: u32,
    tool_error_intervention_used: bool,
    consecutive_non_action_tool_steps: u32,
    bookkeeping_intervention_used: bool,
}

impl AgentLoopGuard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn observe_model_step(
        &mut self,
        assistant_text: &str,
        tool_calls: &[ToolCallRequest],
    ) -> Option<LoopGuardIntervention> {
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

    pub(crate) fn observe_tool_result(&mut self, is_error: bool) -> Option<LoopGuardIntervention> {
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
    tool_calls
        .iter()
        .map(|call| {
            format!(
                "{}:{}",
                call.name,
                canonical_json_text(&call.arguments).unwrap_or_else(|| call.arguments.clone())
            )
        })
        .collect::<Vec<_>>()
        .join("|")
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
    fn detects_consecutive_tool_errors() {
        let mut guard = AgentLoopGuard::new();
        assert!(guard.observe_tool_result(true).is_none());
        assert!(guard.observe_tool_result(true).is_none());
        assert!(guard.observe_tool_result(true).is_none());
        assert_eq!(
            guard.observe_tool_result(true).unwrap().action,
            LoopGuardAction::ChangeStrategy
        );
        assert_eq!(
            guard.observe_tool_result(true).unwrap().action,
            LoopGuardAction::StopLoop
        );
        assert!(guard.observe_tool_result(false).is_none());
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
}
