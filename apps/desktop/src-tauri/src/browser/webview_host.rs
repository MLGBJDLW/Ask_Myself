use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{Manager, Webview, WebviewUrl};
use tokio::sync::oneshot;
use url::Url;

use super::policy::navigation_preapproved;
use super::scripts::BROWSER_INIT_SCRIPT;
use super::state::{BrowserBounds, BrowserState};

pub fn create_child_webview(
    state: &BrowserState,
    session_id: &str,
    tab_id: &str,
    url: Url,
    profile_dir: PathBuf,
    agent_restricted_initially: bool,
    bounds: Option<BrowserBounds>,
) -> Result<(Webview, Arc<AtomicBool>, Arc<Mutex<HashSet<String>>>), String> {
    let window = state
        .app()
        .get_window("main")
        .ok_or_else(|| "Main application window is unavailable".to_string())?;
    let label = format!("browser-{}", tab_id.trim_start_matches("tab_"));
    let agent_restricted = Arc::new(AtomicBool::new(agent_restricted_initially));
    let approved_agent_urls = Arc::new(Mutex::new(HashSet::from([url.to_string()])));

    let navigation_restriction = Arc::clone(&agent_restricted);
    let approved_for_navigation = Arc::clone(&approved_agent_urls);
    let state_for_load = state.clone();
    let session_for_load = session_id.to_string();
    let tab_for_load = tab_id.to_string();
    let state_for_title = state.clone();
    let session_for_title = session_id.to_string();
    let tab_for_title = tab_id.to_string();
    let state_for_popup = state.clone();
    let session_for_popup = session_id.to_string();
    let tab_for_popup = tab_id.to_string();
    let state_for_download = state.clone();
    let session_for_download = session_id.to_string();
    let tab_for_download = tab_id.to_string();

    let builder = WebviewBuilder::new(label, WebviewUrl::External(url))
        .data_directory(profile_dir)
        .disable_drag_drop_handler()
        .initialization_script(BROWSER_INIT_SCRIPT)
        .on_navigation(move |target| {
            let agent_restricted =
                navigation_restriction.load(std::sync::atomic::Ordering::Relaxed);
            approved_for_navigation.lock().is_ok_and(|mut approved| {
                navigation_preapproved(target, agent_restricted, &mut approved)
            })
        })
        .on_page_load(move |_webview, payload| {
            state_for_load.update_page_load(
                &session_for_load,
                &tab_for_load,
                payload.url(),
                payload.event() == PageLoadEvent::Started,
            );
        })
        .on_document_title_changed(move |_webview, title| {
            state_for_title.handle_document_title(&session_for_title, &tab_for_title, title);
        })
        .on_new_window(move |url, _features| {
            state_for_popup.emit(
                "newWindowRequested",
                serde_json::json!({
                    "sessionId": session_for_popup,
                    "tabId": tab_for_popup,
                    "url": url,
                }),
            );
            NewWindowResponse::Deny
        })
        .on_download(move |_webview, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                state_for_download.emit(
                    "downloadRequested",
                    serde_json::json!({
                        "sessionId": session_for_download,
                        "tabId": tab_for_download,
                        "url": url,
                        "blocked": true,
                    }),
                );
            }
            false
        });
    let initial = bounds.unwrap_or(BrowserBounds {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    });
    let visible = bounds.is_some();
    let webview = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(initial.x, initial.y),
            tauri::LogicalSize::new(initial.width, initial.height),
        )
        .map_err(|error| format!("Could not create browser WebView: {error}"))?;
    if !visible {
        let _ = webview.hide();
    }
    Ok((webview, agent_restricted, approved_agent_urls))
}

#[derive(Deserialize)]
struct EvalEnvelope {
    ok: bool,
    value: Option<serde_json::Value>,
    error: Option<String>,
}

pub async fn eval_json(webview: &Webview, expression: &str) -> Result<serde_json::Value, String> {
    let script = format!(
        "(() => {{ try {{ return {{ ok: true, value: ({expression}) }}; }} catch (error) {{ return {{ ok: false, error: String(error && error.message || error) }}; }} }})()"
    );
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(std::sync::Mutex::new(Some(sender)));
    let sender_for_callback = Arc::clone(&sender);
    webview
        .eval_with_callback(script, move |result| {
            if let Ok(mut sender) = sender_for_callback.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(result);
                }
            }
        })
        .map_err(|error| format!("Could not evaluate browser script: {error}"))?;
    let raw = tokio::time::timeout(std::time::Duration::from_secs(10), receiver)
        .await
        .map_err(|_| "Browser script timed out".to_string())?
        .map_err(|_| "Browser script response was dropped".to_string())?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("Could not decode browser script result: {error}"))?;
    let value = if let Some(encoded) = value.as_str() {
        serde_json::from_str(encoded).unwrap_or(value)
    } else {
        value
    };
    let envelope: EvalEnvelope = serde_json::from_value(value)
        .map_err(|error| format!("Could not decode browser script envelope: {error}"))?;
    if envelope.ok {
        Ok(envelope.value.unwrap_or(serde_json::Value::Null))
    } else {
        Err(envelope
            .error
            .unwrap_or_else(|| "Browser script failed".to_string()))
    }
}
