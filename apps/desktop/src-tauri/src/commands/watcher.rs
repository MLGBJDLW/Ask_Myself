use super::*;

// ── Watcher Commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn start_watching(
    app_state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
) -> Result<(), String> {
    let source = app_state
        .db
        .get_source(&source_id)
        .map_err(|e| e.to_string())?;
    let path = std::path::Path::new(&source.root_path);
    if !path.exists() {
        return Err(format!("Path does not exist: {}", source.root_path));
    }
    let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
    watcher.watch(path).map_err(|e| e.to_string())?;
    let mut watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
    watched.insert(source_id.clone(), source.root_path.clone());

    // Persist watch_enabled = true in the database.
    let input = UpdateSourceInput {
        root_path: None,
        include_globs: None,
        exclude_globs: None,
        watch_enabled: Some(true),
    };
    app_state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())?;
    watcher_state.revision.fetch_add(1, Ordering::AcqRel);

    Ok(())
}

#[tauri::command]
pub fn stop_watching(
    app_state: tauri::State<'_, AppState>,
    watcher_state: tauri::State<'_, WatcherState>,
    source_id: String,
) -> Result<(), String> {
    // Persist first, then advance the revision. A startup registration that
    // sampled the old database value must observe the revision change before
    // it can install that stale watch.
    let input = UpdateSourceInput {
        root_path: None,
        include_globs: None,
        exclude_globs: None,
        watch_enabled: Some(false),
    };
    app_state
        .db
        .update_source(&source_id, input)
        .map_err(|e| e.to_string())?;
    watcher_state.revision.fetch_add(1, Ordering::AcqRel);

    let root_path = watcher_state
        .watched
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&source_id);
    if let Some(root_path) = root_path {
        let path = std::path::Path::new(&root_path);
        let mut watcher = watcher_state.watcher.lock().map_err(|e| e.to_string())?;
        let _ = watcher.unwatch(path); // best-effort
    }

    Ok(())
}

#[tauri::command]
pub fn get_watcher_status(
    watcher_state: tauri::State<'_, WatcherState>,
) -> Result<Vec<WatchedSourceInfo>, String> {
    let watched = watcher_state.watched.lock().map_err(|e| e.to_string())?;
    Ok(watched
        .iter()
        .map(|(id, path)| WatchedSourceInfo {
            source_id: id.clone(),
            root_path: path.clone(),
        })
        .collect())
}
