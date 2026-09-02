use super::conversation::desktop_package_host_snapshot;
use super::*;
use nexa_core::package_host::PackageSurfaceKind;

// ── Skills Commands ─────────────────────────────────────────────────

fn materialize_user_skill_resource(state: &AppState, skill: &Skill) -> Result<(), String> {
    nexa_core::skills::materialize_user_skill_to_directory(
        &state.user_extensions.skills_dir(),
        skill,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn materialize_user_skill_resources(state: &AppState, skills: &[Skill]) -> Result<(), String> {
    nexa_core::skills::materialize_user_skills_to_directory(
        &state.user_extensions.skills_dir(),
        skills,
    )
    .map_err(|e| e.to_string())
}

fn materialize_user_skill_resources_except(
    state: &AppState,
    skills: &[Skill],
    preserved_skill_ids: &[String],
) -> Result<(), String> {
    nexa_core::skills::materialize_user_skills_to_directory_except(
        &state.user_extensions.skills_dir(),
        skills,
        preserved_skill_ids,
    )
    .map_err(|e| e.to_string())
}

fn remove_user_skill_resource(state: &AppState, skill_id: &str) -> Result<(), String> {
    nexa_core::skills::remove_materialized_user_skill_from_directory(
        &state.user_extensions.skills_dir(),
        skill_id,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_skills_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<Skill>, String> {
    state.db.list_skills().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_skill_cmd(
    state: tauri::State<'_, AppState>,
    input: SaveSkillInput,
) -> Result<Skill, String> {
    let skill = state.db.save_skill(&input).map_err(|e| e.to_string())?;
    materialize_user_skill_resource(&state, &skill)?;
    Ok(skill)
}

#[tauri::command]
pub async fn delete_skill_cmd(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_skill(&id).map_err(|e| e.to_string())?;
    remove_user_skill_resource(&state, &id)
}

#[tauri::command]
pub async fn toggle_skill_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_skill(&id, enabled)
        .map_err(|e| e.to_string())
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
        materialize_user_skill_resource(&state, &skill)?;
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
    materialize_user_skill_resources(&state, &skills)?;
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
    state: tauri::State<'_, AppState>,
    directory: String,
) -> Result<Vec<Skill>, String> {
    let skills = nexa_core::skills::import_skills_from_directory(&state.db, Path::new(&directory))
        .map_err(|e| e.to_string())?;
    materialize_user_skill_resources(&state, &skills)?;
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
    let applied = state
        .db
        .apply_skill_change_proposal(&id)
        .map_err(|e| e.to_string())?;
    materialize_user_skill_resource(&state, &applied.skill)?;
    Ok(applied)
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

fn disconnect_after_mcp_test(server: &McpServer) -> bool {
    !server.enabled
}

fn may_list_mcp_tools(server: &McpServer) -> bool {
    server.enabled
}

#[tauri::command]
pub fn get_user_extension_layout_cmd(
    state: tauri::State<'_, AppState>,
) -> nexa_core::user_extensions::UserExtensionLayoutView {
    state.user_extensions.view()
}

#[tauri::command]
pub async fn reload_user_skill_files_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<nexa_core::skills::RegisteredSkillFileSyncReport, String> {
    let report = nexa_core::skills::sync_registered_user_skills_from_directory(
        &state.db,
        &state.user_extensions.skills_dir(),
    )
    .map_err(|error| error.to_string())?;
    let skills = state.db.list_skills().map_err(|error| error.to_string())?;
    materialize_user_skill_resources_except(&state, &skills, &report.preserved_skill_ids)?;
    Ok(report)
}

#[tauri::command]
pub async fn prepare_mcp_config_file_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let path = state.user_extensions.mcp_config_path();
    nexa_core::mcp::config_file::ensure_user_mcp_config(&path)
        .map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn reload_mcp_config_file_cmd(
    state: tauri::State<'_, AppState>,
    mcp_state: tauri::State<'_, McpManagerState>,
) -> Result<nexa_core::mcp::config_file::McpConfigReloadReport, String> {
    let path = state.user_extensions.mcp_config_path();
    let report = nexa_core::mcp::config_file::reload_user_mcp_config(&state.db, &path)
        .map_err(|error| error.to_string())?;
    let mut manager = mcp_state.manager.lock().await;
    match sync_enabled_mcp_servers(&state.db, &mut manager).await {
        Ok(errors) => {
            for (server_id, error) in errors {
                warn!("Failed to sync MCP connector {server_id} after config reload: {error}");
            }
        }
        Err(error) => warn!("Failed to refresh MCP connectors after config reload: {error}"),
    }
    Ok(report)
}

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
    // Testing is diagnostic only. A disabled connector must not retain a
    // client, background HTTP stream, or stdio child process after discovery.
    if disconnect_after_mcp_test(&server) {
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
    let servers = state.db.list_mcp_servers().map_err(|e| e.to_string())?;
    let server = servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("MCP server {server_id} not found"))?;
    let mut manager = mcp_state.manager.lock().await;
    // Tool enumeration is a runtime action, not a diagnostic test. Re-read
    // durable activation before consulting the client cache so a stale UI
    // snapshot cannot resurrect a connector disabled by a JSON reload.
    if !may_list_mcp_tools(&server) {
        let _ = manager.disconnect_server(&server_id).await;
        return Err(format!("MCP server {server_id} is disabled"));
    }
    // If already connected, list tools from existing client.
    if let Some(client) = manager.get_client(&server_id) {
        let mut guard = client.lock().await;
        return guard.list_tools().await.map_err(|e| e.to_string());
    }
    // Otherwise, connect first.
    manager
        .connect_server(&server, Some(DEFAULT_MCP_CALL_TIMEOUT_SECS))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod mcp_command_tests {
    use super::*;

    fn test_server(enabled: bool, builtin: bool) -> McpServer {
        McpServer {
            id: "test-connector".into(),
            name: "Test Connector".into(),
            transport: "stdio".into(),
            command: Some("test-mcp".into()),
            args: None,
            url: None,
            env_json: None,
            headers_json: None,
            enabled,
            created_at: String::new(),
            updated_at: String::new(),
            builtin_id: builtin.then(|| "builtin-test".into()),
        }
    }

    #[test]
    fn tests_retain_only_explicitly_enabled_connector_connections() {
        assert!(disconnect_after_mcp_test(&test_server(false, false)));
        assert!(disconnect_after_mcp_test(&test_server(false, true)));
        assert!(!disconnect_after_mcp_test(&test_server(true, false)));
    }

    #[test]
    fn tool_enumeration_requires_durable_connector_activation() {
        assert!(!may_list_mcp_tools(&test_server(false, false)));
        assert!(!may_list_mcp_tools(&test_server(false, true)));
        assert!(may_list_mcp_tools(&test_server(true, false)));
    }
}
