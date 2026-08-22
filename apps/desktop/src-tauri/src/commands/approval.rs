use super::*;

// ── Tool Approval ───────────────────────────────────────────────────

/// Resolve a pending [`ApprovalRequest`] emitted by the agent executor.
///
/// The frontend calls this after the user clicks a button in the approval
/// dialog. Decision strings: `allow_once`, `allow_session`, `deny`, `never`.
#[tauri::command]
pub async fn approve_tool_call_cmd(
    approval_state: tauri::State<'_, ApprovalState>,
    request_id: String,
    decision: String,
) -> Result<(), String> {
    let decision = ApprovalDecision::parse(&decision)
        .ok_or_else(|| format!("Unknown approval decision: {decision}"))?;
    let sender = {
        let mut pending = approval_state.pending.lock().await;
        pending.remove(&request_id)
    };
    match sender {
        Some(pending) => {
            pending
                .sender
                .send(decision)
                .map_err(|_| "Approval request already resolved or expired".to_string())?;
            Ok(())
        }
        None => Err(format!("Unknown approval request id: {request_id}")),
    }
}

#[tauri::command]
pub fn list_tool_permission_policies_cmd(
    state: tauri::State<'_, AppState>,
    approval_state: tauri::State<'_, ApprovalState>,
) -> Result<serde_json::Value, String> {
    let persisted = state
        .db
        .list_tool_permission_policies()
        .map_err(|e| e.to_string())?;
    let session: Vec<serde_json::Value> = approval_state
        .session_store
        .list()
        .into_iter()
        .filter_map(|(permission_key, decision)| {
            let key = ToolPermissionKey::parse(&permission_key)?;
            Some(serde_json::json!({
                "toolName": key.tool_name,
                "permissionKey": permission_key,
                "targetKind": key.target_kind,
                "targetValue": key.target_value,
                "decision": decision.as_str(),
            }))
        })
        .collect();
    Ok(serde_json::json!({
        "persisted": persisted,
        "session": session,
    }))
}

#[tauri::command]
pub fn delete_tool_permission_policy_cmd(
    state: tauri::State<'_, AppState>,
    approval_state: tauri::State<'_, ApprovalState>,
    scope: Option<String>,
    permission_key: String,
) -> Result<(), String> {
    match scope.as_deref() {
        Some("session") => approval_state.session_store.remove(&permission_key),
        Some("forever") | None => {
            state
                .db
                .delete_tool_permission_policy(&permission_key)
                .map_err(|e| e.to_string())?;
        }
        Some(other) => return Err(format!("Unknown scope: {other}")),
    }
    Ok(())
}

#[tauri::command]
pub fn clear_tool_permission_policies_cmd(
    state: tauri::State<'_, AppState>,
    approval_state: tauri::State<'_, ApprovalState>,
) -> Result<(), String> {
    approval_state.session_store.clear();
    state
        .db
        .clear_tool_permission_policies()
        .map_err(|e| e.to_string())?;
    Ok(())
}
