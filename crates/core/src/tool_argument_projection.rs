//! Mandatory, non-configurable projection for sensitive tool arguments.
//!
//! Execution receives the original in-memory JSON. UI events, approvals and
//! durable traces must use this projection so typed text and key sequences do
//! not become an accidental credential store.

use serde_json::{json, Value};

pub fn is_sensitive_computer_control_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("computer_control")
}

pub fn is_browser_session_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("browser_session")
}

fn browser_session_arguments_contain_sensitive_input(arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return true;
    };
    object.contains_key("text")
        || object.contains_key("value")
        || object.contains_key("key")
        || object
            .get("condition")
            .and_then(Value::as_object)
            .is_some_and(|condition| {
                ["text", "name", "pattern"]
                    .iter()
                    .any(|field| condition.contains_key(*field))
            })
}

pub fn tool_arguments_contain_sensitive_input(tool_name: &str, arguments: &Value) -> bool {
    is_sensitive_computer_control_name(tool_name)
        || (is_browser_session_name(tool_name)
            && browser_session_arguments_contain_sensitive_input(arguments))
}

pub fn tool_call_contains_sensitive_input(tool_name: &str, arguments: &str) -> bool {
    match serde_json::from_str::<Value>(arguments) {
        Ok(arguments) => tool_arguments_contain_sensitive_input(tool_name, &arguments),
        Err(_) => {
            is_sensitive_computer_control_name(tool_name) || is_browser_session_name(tool_name)
        }
    }
}

fn text_summary(value: &str) -> Value {
    json!({
        "redacted": true,
        "kind": "text",
        "charCount": value.chars().count(),
        "lineCount": value.lines().count().max(1),
        "utf8Bytes": value.len()
    })
}

fn key_sequence_summary(value: &str) -> Value {
    let keys = value
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let modifier_count = keys
        .iter()
        .filter(|key| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "ctrl"
                    | "control"
                    | "alt"
                    | "option"
                    | "shift"
                    | "meta"
                    | "win"
                    | "windows"
                    | "command"
                    | "cmd"
            )
        })
        .count();
    json!({
        "redacted": true,
        "kind": "keySequence",
        "keyCount": keys.len(),
        "modifierCount": modifier_count
    })
}

fn redacted_invalid_value(kind: &str, value: &Value) -> Value {
    let value_type = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    json!({
        "redacted": true,
        "kind": kind,
        "valueType": value_type,
        "invalidShape": true
    })
}

/// Return the audit-safe form of a parsed tool argument object.
pub fn audit_safe_arguments(tool_name: &str, arguments: &Value) -> Value {
    if is_sensitive_computer_control_name(tool_name) {
        return audit_safe_computer_control_arguments(arguments);
    }
    if is_browser_session_name(tool_name) {
        return audit_safe_browser_session_arguments(arguments);
    }
    arguments.clone()
}

fn audit_safe_computer_control_arguments(arguments: &Value) -> Value {
    let Some(object) = arguments.as_object() else {
        return json!({
            "redacted": true,
            "kind": "invalidComputerControlArgumentsRoot",
            "invalidShape": true
        });
    };
    let mut projected = serde_json::Map::new();

    if let Some(action) = object.get("action") {
        let canonical = action.as_str().map(str::trim).map(str::to_ascii_lowercase);
        let safe = canonical.filter(|action| {
            matches!(
                action.as_str(),
                "focus_window"
                    | "move_mouse"
                    | "click"
                    | "drag"
                    | "scroll"
                    | "type_text"
                    | "key"
                    | "invoke"
                    | "set_value"
            )
        });
        projected.insert(
            "action".to_string(),
            safe.map(Value::String)
                .unwrap_or_else(|| redacted_invalid_value("action", action)),
        );
    }
    if object.contains_key("observation_id") || object.contains_key("observationId") {
        projected.insert(
            "observation_id".to_string(),
            Value::String("<observation-token-redacted>".to_string()),
        );
    }
    for key in [
        "window_id",
        "windowId",
        "x",
        "y",
        "to_x",
        "to_y",
        "scroll_x",
        "scroll_y",
        "click_count",
        "max_elements",
    ] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                match value {
                    Value::Number(_) => value.clone(),
                    _ => redacted_invalid_value(key, value),
                },
            );
        }
    }
    for key in ["include_elements", "wait_for_previous"] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                match value {
                    Value::Bool(_) => value.clone(),
                    _ => redacted_invalid_value(key, value),
                },
            );
        }
    }
    for (key, allowed) in [
        (
            "coordinate_space",
            &["captured_image_pixels", "normalized_0_1"][..],
        ),
        ("button", &["left", "right", "middle"][..]),
        ("capture_mode", &["raw", "som"][..]),
    ] {
        if let Some(value) = object.get(key) {
            let safe = value
                .as_str()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .filter(|candidate| allowed.contains(&candidate.as_str()));
            projected.insert(
                key.to_string(),
                safe.map(Value::String)
                    .unwrap_or_else(|| redacted_invalid_value(key, value)),
            );
        }
    }
    for key in ["element_id", "to_element_id"] {
        if let Some(value) = object.get(key) {
            let safe = value.as_str().filter(|candidate| {
                candidate.strip_prefix('e').is_some_and(|digits| {
                    !digits.is_empty()
                        && digits.len() <= 4
                        && digits.bytes().all(|byte| byte.is_ascii_digit())
                        && !digits.starts_with('0')
                })
            });
            projected.insert(
                key.to_string(),
                safe.map(|value| Value::String(value.to_string()))
                    .unwrap_or_else(|| redacted_invalid_value(key, value)),
            );
        }
    }
    if let Some(raw) = object.get("text") {
        let value = raw
            .as_str()
            .map(text_summary)
            .unwrap_or_else(|| redacted_invalid_value("text", raw));
        projected.insert("text".to_string(), value);
    }
    for key in ["key_sequence", "keySequence"] {
        if let Some(raw) = object.get(key) {
            let value = raw
                .as_str()
                .map(key_sequence_summary)
                .unwrap_or_else(|| redacted_invalid_value("keySequence", raw));
            projected.insert(key.to_string(), value);
        }
    }
    if let Some(reason) = object.get("reason") {
        projected.insert(
            "reason".to_string(),
            reason
                .as_str()
                .map(|value| {
                    let mut summary = text_summary(value);
                    summary["kind"] = Value::String("reason".to_string());
                    summary
                })
                .unwrap_or_else(|| redacted_invalid_value("reason", reason)),
        );
    }
    let known = [
        "action",
        "observation_id",
        "observationId",
        "window_id",
        "windowId",
        "element_id",
        "to_element_id",
        "coordinate_space",
        "x",
        "y",
        "to_x",
        "to_y",
        "button",
        "click_count",
        "scroll_x",
        "scroll_y",
        "text",
        "key_sequence",
        "keySequence",
        "reason",
        "include_elements",
        "max_elements",
        "capture_mode",
        "wait_for_previous",
    ];
    let unknown_field_count = object
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .count();
    if unknown_field_count > 0 {
        projected.insert("unknownFieldCount".to_string(), unknown_field_count.into());
    }
    Value::Object(projected)
}

fn audit_safe_browser_condition(condition: &Value) -> Value {
    let Some(object) = condition.as_object() else {
        return redacted_invalid_value("condition", condition);
    };
    let mut projected = serde_json::Map::new();
    if let Some(condition_type) = object.get("type") {
        let safe = condition_type
            .as_str()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|candidate| {
                matches!(
                    candidate.as_str(),
                    "page_loaded"
                        | "text_present"
                        | "text_absent"
                        | "url_matches"
                        | "element_present"
                        | "element_absent"
                )
            });
        projected.insert(
            "type".to_string(),
            safe.map(Value::String)
                .unwrap_or_else(|| redacted_invalid_value("conditionType", condition_type)),
        );
    }
    for key in ["ref", "targetRef"] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                value
                    .as_str()
                    .map(|value| Value::String(value.to_string()))
                    .unwrap_or_else(|| redacted_invalid_value(key, value)),
            );
        }
    }
    for key in ["text", "name", "pattern"] {
        if let Some(value) = object.get(key) {
            let mut summary = value
                .as_str()
                .map(text_summary)
                .unwrap_or_else(|| redacted_invalid_value(key, value));
            if summary.is_object() {
                summary["kind"] = Value::String(format!("condition{key}"));
            }
            projected.insert(key.to_string(), summary);
        }
    }
    if let Some(role) = object.get("role") {
        projected.insert(
            "role".to_string(),
            role.as_str()
                .map(|value| Value::String(value.to_string()))
                .unwrap_or_else(|| redacted_invalid_value("role", role)),
        );
    }
    let known = [
        "type",
        "ref",
        "targetRef",
        "text",
        "name",
        "pattern",
        "role",
    ];
    let unknown_field_count = object
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .count();
    if unknown_field_count > 0 {
        projected.insert("unknownFieldCount".to_string(), unknown_field_count.into());
    }
    Value::Object(projected)
}

fn audit_safe_browser_session_arguments(arguments: &Value) -> Value {
    let Some(object) = arguments.as_object() else {
        return json!({
            "redacted": true,
            "kind": "invalidBrowserSessionArgumentsRoot",
            "invalidShape": true
        });
    };
    let mut projected = serde_json::Map::new();
    if let Some(action) = object.get("action") {
        let safe = action
            .as_str()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|candidate| {
                matches!(
                    candidate.as_str(),
                    "create_session"
                        | "list_sessions"
                        | "list_tabs"
                        | "open_tab"
                        | "activate_tab"
                        | "navigate"
                        | "go_back"
                        | "go_forward"
                        | "reload"
                        | "observe"
                        | "move"
                        | "hover"
                        | "click"
                        | "double_click"
                        | "drag"
                        | "type"
                        | "select"
                        | "press"
                        | "scroll"
                        | "wait_for"
                        | "close_tab"
                        | "close_session"
                )
            });
        projected.insert(
            "action".to_string(),
            safe.map(Value::String)
                .unwrap_or_else(|| redacted_invalid_value("action", action)),
        );
    }
    for key in [
        "sessionId",
        "session_id",
        "tabId",
        "tab_id",
        "observationId",
        "observation_id",
        "targetRef",
        "target_ref",
        "endRef",
        "end_ref",
        "url",
    ] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                value
                    .as_str()
                    .map(|value| Value::String(value.to_string()))
                    .unwrap_or_else(|| redacted_invalid_value(key, value)),
            );
        }
    }
    for key in ["text", "value"] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                value
                    .as_str()
                    .map(text_summary)
                    .unwrap_or_else(|| redacted_invalid_value(key, value)),
            );
        }
    }
    if let Some(value) = object.get("key") {
        projected.insert(
            "key".to_string(),
            value
                .as_str()
                .map(key_sequence_summary)
                .unwrap_or_else(|| redacted_invalid_value("key", value)),
        );
    }
    if let Some(value) = object.get("button") {
        let safe = value
            .as_str()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|candidate| matches!(candidate.as_str(), "left" | "middle" | "right"));
        projected.insert(
            "button".to_string(),
            safe.map(Value::String)
                .unwrap_or_else(|| redacted_invalid_value("button", value)),
        );
    }
    if let Some(value) = object.get("modifiers") {
        let safe = value.as_array().and_then(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|candidate| {
                            matches!(*candidate, "Alt" | "Control" | "Meta" | "Shift")
                        })
                        .map(|value| Value::String(value.to_string()))
                })
                .collect::<Option<Vec<_>>>()
        });
        projected.insert(
            "modifiers".to_string(),
            safe.map(Value::Array)
                .unwrap_or_else(|| redacted_invalid_value("modifiers", value)),
        );
    }
    for key in [
        "scrollX",
        "scroll_x",
        "scrollY",
        "scroll_y",
        "timeoutMs",
        "timeout_ms",
    ] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                match value {
                    Value::Number(_) => value.clone(),
                    _ => redacted_invalid_value(key, value),
                },
            );
        }
    }
    for key in ["wait_for_previous", "waitForPrevious"] {
        if let Some(value) = object.get(key) {
            projected.insert(
                key.to_string(),
                match value {
                    Value::Bool(_) => value.clone(),
                    _ => redacted_invalid_value(key, value),
                },
            );
        }
    }
    if let Some(condition) = object.get("condition") {
        projected.insert(
            "condition".to_string(),
            audit_safe_browser_condition(condition),
        );
    }
    let known = [
        "action",
        "sessionId",
        "session_id",
        "tabId",
        "tab_id",
        "url",
        "observationId",
        "observation_id",
        "targetRef",
        "target_ref",
        "endRef",
        "end_ref",
        "text",
        "value",
        "key",
        "button",
        "modifiers",
        "scrollX",
        "scroll_x",
        "scrollY",
        "scroll_y",
        "condition",
        "timeoutMs",
        "timeout_ms",
        "wait_for_previous",
        "waitForPrevious",
    ];
    let unknown_field_count = object
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .count();
    if unknown_field_count > 0 {
        projected.insert("unknownFieldCount".to_string(), unknown_field_count.into());
    }
    Value::Object(projected)
}

/// Parse and project a raw argument string. Invalid partial JSON is hidden for
/// sensitive tools rather than copied into UI or persistence verbatim.
pub fn audit_safe_arguments_string(tool_name: &str, arguments: &str) -> String {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => serde_json::to_string(&audit_safe_arguments(tool_name, &value))
            .unwrap_or_else(|_| "{\"redacted\":true}".to_string()),
        Err(_)
            if is_sensitive_computer_control_name(tool_name)
                || is_browser_session_name(tool_name) =>
        {
            "{\"redacted\":true,\"kind\":\"incompleteSensitiveInteractionArguments\"}".to_string()
        }
        Err(_) => arguments.to_string(),
    }
}

/// Clone a tool call for durable storage. Provider-native thought signatures
/// can themselves embed the original function-call arguments, so sensitive
/// interaction calls retain replay metadata only through the separately
/// projected provider envelope.
pub fn audit_safe_tool_call(call: &crate::llm::ToolCallRequest) -> crate::llm::ToolCallRequest {
    let mut projected = call.clone();
    projected.arguments = audit_safe_arguments_string(&call.name, &call.arguments);
    if tool_call_contains_sensitive_input(&call.name, &call.arguments) {
        projected.thought_signature = None;
    }
    projected
}

pub fn audit_safe_tool_calls(
    calls: &[crate::llm::ToolCallRequest],
) -> Vec<crate::llm::ToolCallRequest> {
    let sensitive_batch = calls
        .iter()
        .any(|call| tool_call_contains_sensitive_input(&call.name, &call.arguments));
    calls
        .iter()
        .map(|call| {
            let mut projected = audit_safe_tool_call(call);
            if sensitive_batch {
                projected.thought_signature = None;
            }
            projected
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computer_control_projection_never_contains_typed_text_or_key_sequence() {
        let sentinel = "sentinel-password-4f19";
        let projected = audit_safe_arguments(
            "computer_control",
            &json!({
                "action": "set_value",
                "text": sentinel,
                "key_sequence": "ctrl+sentinel-key",
                "window_id": 42
            }),
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains(sentinel));
        assert!(!serialized.contains("sentinel-key"));
        assert_eq!(projected["text"]["charCount"], sentinel.chars().count());
        assert_eq!(projected["key_sequence"]["keyCount"], 2);
    }

    #[test]
    fn browser_session_projection_never_contains_typed_text_selected_values_or_keys() {
        let sentinel = "browser-input-secret-719d";
        let projected = audit_safe_arguments(
            "browser_session",
            &json!({
                "action": "type",
                "sessionId": "browser-a",
                "observationId": "observation-a",
                "targetRef": "e7",
                "text": sentinel,
                "value": format!("selected-{sentinel}"),
                "key": format!("Control+{sentinel}"),
            }),
        );
        let serialized = serde_json::to_string(&projected).unwrap();

        assert!(!serialized.contains(sentinel));
        assert_eq!(projected["text"]["charCount"], sentinel.chars().count());
        assert_eq!(
            projected["value"]["charCount"],
            format!("selected-{sentinel}").chars().count()
        );
        assert_eq!(projected["key"]["keyCount"], 2);
        assert_eq!(projected["action"], "type");
        assert_eq!(projected["targetRef"], "e7");
    }

    #[test]
    fn malformed_streaming_browser_arguments_fail_closed() {
        let projected = audit_safe_arguments_string(
            "browser_session",
            r#"{"action":"type","text":"browser-stream-secret"#,
        );

        assert!(!projected.contains("browser-stream-secret"));
        assert!(projected.contains("redacted"));
    }

    #[test]
    fn malformed_streaming_computer_arguments_fail_closed() {
        let projected = audit_safe_arguments_string(
            "computer_control",
            r#"{"action":"type_text","text":"secret"#,
        );
        assert!(!projected.contains("secret"));
        assert!(projected.contains("redacted"));
    }

    #[test]
    fn invalid_sensitive_argument_shapes_are_redacted_before_validation() {
        let sentinel = "nested-invalid-secret-3c22";
        let projected = audit_safe_arguments(
            "computer_control",
            &json!({
                "action": "type_text",
                "text": { "nested": sentinel },
                "key_sequence": ["ctrl", sentinel]
            }),
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains(sentinel));
        assert_eq!(projected["text"]["invalidShape"], true);
        assert_eq!(projected["key_sequence"]["invalidShape"], true);
    }

    #[test]
    fn encoded_object_root_reason_and_unknown_fields_cannot_smuggle_text() {
        let sentinel = "root-or-reason-secret-5e81";
        let encoded_root =
            Value::String(format!(r#"{{"action":"type_text","text":"{sentinel}"}}"#));
        let root_projection = audit_safe_arguments("computer_control", &encoded_root);
        assert!(!root_projection.to_string().contains(sentinel));
        assert_eq!(root_projection["invalidShape"], true);
        let raw_projection = audit_safe_arguments_string(
            " computer_control ",
            &serde_json::to_string(&encoded_root).unwrap(),
        );
        assert!(!raw_projection.contains(sentinel));

        let object_projection = audit_safe_arguments(
            "computer_control",
            &json!({
                "action": "type_text",
                "text": "safe-length-only",
                "reason": format!("type {sentinel}"),
                "metadata": { "duplicate": sentinel }
            }),
        );
        let serialized = object_projection.to_string();
        assert!(!serialized.contains(sentinel));
        assert_eq!(object_projection["reason"]["kind"], "reason");
        assert_eq!(object_projection["unknownFieldCount"], 1);
    }
}
