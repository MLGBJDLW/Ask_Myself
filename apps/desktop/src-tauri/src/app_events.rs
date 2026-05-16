use log::warn;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

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
