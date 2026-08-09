use log::warn;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) fn emit_window_event<T: Serialize + ?Sized>(
    app_handle: &AppHandle,
    window_label: &str,
    event: &str,
    payload: &T,
) {
    let Some(window) = app_handle.get_webview_window(window_label) else {
        return;
    };
    if let Err(err) = window.emit(event, payload) {
        let msg = err.to_string();
        let lower = msg.to_ascii_lowercase();
        if lower.contains("0x80070578")
            || lower.contains("invalid window handle")
            || lower.contains("invalid window")
        {
            return;
        }
        warn!("Failed to emit event '{event}' to window '{window_label}': {msg}");
    }
}

pub(crate) fn emit_main_window_event<T: Serialize + ?Sized>(
    app_handle: &AppHandle,
    event: &str,
    payload: &T,
) {
    emit_window_event(app_handle, "main", event, payload);
}

pub(crate) fn emit_app_event<T: Serialize + ?Sized>(
    app_handle: &AppHandle,
    event: &str,
    payload: &T,
) {
    let windows = app_handle.webview_windows();
    if windows.is_empty() {
        return;
    }

    for (label, window) in windows {
        if let Err(err) = window.emit(event, payload) {
            let msg = err.to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("0x80070578")
                || lower.contains("invalid window handle")
                || lower.contains("invalid window")
            {
                continue;
            }
            warn!("Failed to emit event '{event}' to window '{label}': {msg}");
        }
    }
}
