use super::conversation::desktop_package_host_snapshot;
use super::*;
use nexa_core::package_host::PackageSurfaceKind;

// ── Skills Commands ─────────────────────────────────────────────────

fn app_data_dir_for_skills(app_handle: &AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data directory: {e}"))
}

fn materialize_user_skill_resource(app_handle: &AppHandle, skill: &Skill) -> Result<(), String> {
    let data_dir = app_data_dir_for_skills(app_handle)?;
    nexa_core::skills::materialize_user_skill_to_disk(&data_dir, skill)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn materialize_user_skill_resources(
    app_handle: &AppHandle,
    skills: &[Skill],
) -> Result<(), String> {
    let data_dir = app_data_dir_for_skills(app_handle)?;
    nexa_core::skills::materialize_user_skills_to_disk(&data_dir, skills).map_err(|e| e.to_string())
}

fn remove_user_skill_resource(app_handle: &AppHandle, skill_id: &str) -> Result<(), String> {
    let data_dir = app_data_dir_for_skills(app_handle)?;
    nexa_core::skills::remove_materialized_user_skill(&data_dir, skill_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_skills_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<Skill>, String> {
    state.db.list_skills().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_skill_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    input: SaveSkillInput,
) -> Result<Skill, String> {
    let skill = state.db.save_skill(&input).map_err(|e| e.to_string())?;
    if skill.enabled {
        materialize_user_skill_resource(&app_handle, &skill)?;
    } else {
        remove_user_skill_resource(&app_handle, &skill.id)?;
    }
    Ok(skill)
}

#[tauri::command]
pub async fn delete_skill_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_skill(&id).map_err(|e| e.to_string())?;
    remove_user_skill_resource(&app_handle, &id)
}

#[tauri::command]
pub async fn toggle_skill_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_skill(&id, enabled)
        .map_err(|e| e.to_string())?;
    if enabled {
        if let Some(skill) = state
            .db
            .list_skills()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|skill| skill.id == id)
        {
            materialize_user_skill_resource(&app_handle, &skill)?;
        }
        Ok(())
    } else {
        remove_user_skill_resource(&app_handle, &id)
    }
}

pub(crate) fn filter_desktop_builtin_skills_by_package_host(
    db: &Database,
    skills: Vec<Skill>,
) -> Result<Vec<Skill>, String> {
    let snapshot = desktop_package_host_snapshot(db)?;
    let visible_skill_ids = snapshot
        .runtime_components()
        .into_iter()
        .filter(|component| component.kind == PackageSurfaceKind::Skill)
        .map(|component| component.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    Ok(skills
        .into_iter()
        .filter(|skill| {
            visible_skill_ids.contains(skill.id.as_str())
                || skill
                    .id
                    .strip_prefix("builtin-")
                    .is_some_and(|slug| visible_skill_ids.contains(slug))
        })
        .collect())
}

#[tauri::command]
pub async fn list_builtin_skills_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Skill>, String> {
    filter_desktop_builtin_skills_by_package_host(
        state.db.as_ref(),
        nexa_core::skills::load_builtin_skills(),
    )
}

#[tauri::command]
pub async fn import_skill_from_md_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    content: String,
) -> Result<Skill, String> {
    let (fm, body) = nexa_core::skills::parse_skill_file(&content).map_err(|e| e.to_string())?;
    let input = SaveSkillInput {
        id: None,
        name: fm.name,
        description: fm.description,
        content: body,
        enabled: true,
        resource_bundle: Vec::new(),
    };
    let skill = state.db.save_skill(&input).map_err(|e| e.to_string())?;
    if skill.enabled {
        materialize_user_skill_resource(&app_handle, &skill)?;
    }
    Ok(skill)
}

/// Parse an editor import without persisting it. This keeps file selection and
/// the explicit Save action as one database write instead of creating a hidden
/// duplicate skill first.
#[tauri::command]
pub async fn parse_skill_markdown_cmd(content: String) -> Result<SaveSkillInput, String> {
    let (fm, body) = nexa_core::skills::parse_skill_file(&content).map_err(|e| e.to_string())?;
    Ok(SaveSkillInput {
        id: None,
        name: fm.name,
        description: fm.description,
        content: body,
        enabled: true,
        resource_bundle: Vec::new(),
    })
}

#[tauri::command]
pub async fn inspect_skill_install_source_cmd(
    source: String,
) -> Result<Vec<DiscoveredSkillBundle>, String> {
    nexa_core::skills::inspect_skill_install_source(Path::new(&source)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn install_skills_from_source_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    source: String,
    replace_existing: bool,
    accept_blocked_warnings: bool,
) -> Result<Vec<Skill>, String> {
    let skills = nexa_core::skills::import_skills_from_source(
        &state.db,
        Path::new(&source),
        replace_existing,
        accept_blocked_warnings,
    )
    .map_err(|e| e.to_string())?;
    materialize_user_skill_resources(&app_handle, &skills)?;
    Ok(skills)
}

#[tauri::command]
pub async fn discover_skills_in_directory_cmd(
    directory: String,
) -> Result<Vec<DiscoveredSkillBundle>, String> {
    nexa_core::skills::discover_skills_in_directory(Path::new(&directory))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn import_skills_from_directory_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    directory: String,
) -> Result<Vec<Skill>, String> {
    let skills = nexa_core::skills::import_skills_from_directory(&state.db, Path::new(&directory))
        .map_err(|e| e.to_string())?;
    materialize_user_skill_resources(&app_handle, &skills)?;
    Ok(skills)
}

#[tauri::command]
pub async fn export_skill_to_md_cmd(
    state: tauri::State<'_, AppState>,
    skill_id: String,
) -> Result<String, String> {
    // Check built-ins first.
    if let Some(s) = nexa_core::skills::load_builtin_skills()
        .into_iter()
        .find(|s| s.id == skill_id)
    {
        return Ok(nexa_core::skills::export_skill_to_md(&s));
    }
    let skills = state.db.list_skills().map_err(|e| e.to_string())?;
    let skill = skills
        .into_iter()
        .find(|s| s.id == skill_id)
        .ok_or_else(|| format!("Skill not found: {skill_id}"))?;
    Ok(nexa_core::skills::export_skill_to_md(&skill))
}

#[tauri::command]
pub async fn scan_skill_content_cmd(
    content: String,
) -> Result<Vec<nexa_core::skills::SkillWarning>, String> {
    Ok(nexa_core::skills::scan_skill_content(&content))
}

#[tauri::command]
pub async fn list_skill_change_proposals_cmd(
    state: tauri::State<'_, AppState>,
    status: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<SkillChangeProposal>, String> {
    let parsed_status = status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(SkillProposalStatus::try_from)
        .transpose()
        .map_err(|e| e.to_string())?;
    state
        .db
        .list_skill_change_proposals(parsed_status, limit.unwrap_or(20).min(100) as usize)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn apply_skill_change_proposal_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<AppliedSkillChange, String> {
    state
        .db
        .apply_skill_change_proposal(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reject_skill_change_proposal_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<SkillChangeProposal, String> {
    state
        .db
        .reject_skill_change_proposal(&id)
        .map_err(|e| e.to_string())
}

// ── MCP Commands ────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_mcp_servers_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<McpServer>, String> {
    state.db.list_mcp_servers().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    input: SaveMcpServerInput,
) -> Result<McpServer, String> {
    let saved = state
        .db
        .save_mcp_server(&input)
        .map_err(|e| e.to_string())?;
    let mut manager = mcp_state.manager.lock().await;
    match sync_enabled_mcp_servers(&state.db, &mut manager).await {
        Ok(errors) => {
            for (server_id, error) in errors {
                warn!("Failed to sync MCP server {server_id} after save: {error}");
            }
        }
        Err(error) => warn!("Failed to refresh enabled MCP servers after save: {error}"),
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
) -> Result<(), String> {
    state.db.delete_mcp_server(&id).map_err(|e| e.to_string())?;
    let mut manager = mcp_state.manager.lock().await;
    manager.disconnect_server(&id).await.ok();
    Ok(())
}

#[tauri::command]
pub async fn toggle_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_mcp_server(&id, enabled)
        .map_err(|e| e.to_string())?;

    let mut manager = mcp_state.manager.lock().await;
    if enabled {
        match sync_enabled_mcp_servers(&state.db, &mut manager).await {
            Ok(errors) => {
                for (server_id, error) in errors {
                    warn!("Failed to sync MCP server {server_id} after enable: {error}");
                }
            }
            Err(error) => warn!("Failed to refresh enabled MCP servers after enable: {error}"),
        }
    } else {
        manager.disconnect_server(&id).await.ok();
    }

    Ok(())
}

#[tauri::command]
pub async fn test_mcp_server_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    id: String,
) -> Result<Vec<McpToolInfo>, String> {
    let servers = state.db.list_mcp_servers().map_err(|e| e.to_string())?;
    let server = servers
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("MCP server {id} not found"))?;
    let mut manager = mcp_state.manager.lock().await;
    // connect_server stores the client so list_mcp_tools_cmd can reuse it.
    let tools = manager
        .connect_server(&server, Some(DEFAULT_MCP_CALL_TIMEOUT_SECS))
        .await
        .map_err(|e| e.to_string())?;
    // For built-in managed servers that aren't enabled, disconnect after
    // testing to stop the managed process.
    if server.builtin_id.is_some() && !server.enabled {
        let _ = manager.disconnect_server(&server.id).await;
    }
    Ok(tools)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn test_mcp_server_direct_cmd(
    mcp_state: tauri::State<'_, McpManagerState>,
    name: String,
    transport: String,
    command: Option<String>,
    args: Option<String>,
    url: Option<String>,
    env_json: Option<String>,
    headers_json: Option<String>,
) -> Result<Vec<McpToolInfo>, String> {
    let server = McpServer {
        id: "__test__".to_string(),
        name,
        transport,
        command,
        args,
        url,
        env_json,
        headers_json,
        enabled: true,
        created_at: String::new(),
        updated_at: String::new(),
        builtin_id: None,
    };
    let mut manager = mcp_state.manager.lock().await;
    let tools = manager
        .connect_server(&server, None)
        .await
        .map_err(|e| e.to_string())?;
    manager.disconnect_server("__test__").await.ok();
    Ok(tools)
}

#[tauri::command]
pub async fn list_mcp_tools_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
    server_id: String,
) -> Result<Vec<McpToolInfo>, String> {
    let mut manager = mcp_state.manager.lock().await;
    // If already connected, list tools from existing client.
    if let Some(client) = manager.get_client(&server_id) {
        let mut guard = client.lock().await;
        return guard.list_tools().await.map_err(|e| e.to_string());
    }
    // Otherwise, connect first.
    let servers = state.db.list_mcp_servers().map_err(|e| e.to_string())?;
    let server = servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("MCP server {server_id} not found"))?;
    manager
        .connect_server(&server, Some(DEFAULT_MCP_CALL_TIMEOUT_SECS))
        .await
        .map_err(|e| e.to_string())
}
