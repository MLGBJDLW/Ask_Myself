use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{Manager, Webview, WebviewUrl};
use tokio::sync::oneshot;
use url::Url;

use super::policy::navigation_preapproved;
use super::scripts::{browser_init_script, browser_takeover_script};
use super::state::{BrowserBounds, BrowserState};

const SCREENSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
// Keep one capture small enough for both the current-turn visual model path
// and the ephemeral tool-card preview. A capture that cannot reach every
// advertised consumer fails explicitly instead of being silently discarded.
const MAX_SCREENSHOT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCREENSHOT_BASE64_BYTES: usize = MAX_SCREENSHOT_BYTES.div_ceil(3) * 4;
const TRUSTED_INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_TRUSTED_TEXT_BYTES: usize = 256 * 1024;

fn trusted_input_modifiers(modifiers: &[String]) -> Result<u8, String> {
    let mut encoded = 0_u8;
    for modifier in modifiers {
        encoded |= match modifier.as_str() {
            "Alt" => 1,
            "Control" => 2,
            "Meta" => 4,
            "Shift" => 8,
            _ => return Err(format!("Unsupported trusted input modifier '{modifier}'")),
        };
    }
    Ok(encoded)
}

fn trusted_pointer_payloads(
    x: f64,
    y: f64,
    button: &str,
    modifiers: &[String],
    click_count: u8,
) -> Result<Vec<serde_json::Value>, String> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err("Trusted pointer coordinates must be finite viewport positions".to_string());
    }
    if !matches!(click_count, 1 | 2) {
        return Err("Trusted pointer click_count must be 1 or 2".to_string());
    }
    let (button, pressed_buttons) = match button {
        "left" => ("left", 1),
        "middle" => ("middle", 4),
        "right" => ("right", 2),
        _ => return Err("Trusted pointer button must be left, middle, or right".to_string()),
    };
    let modifiers = trusted_input_modifiers(modifiers)?;
    let mut payloads = vec![serde_json::json!({
        "type": "mouseMoved",
        "x": x,
        "y": y,
        "button": "none",
        "buttons": 0,
        "modifiers": modifiers,
        "pointerType": "mouse",
    })];
    for current_count in 1..=click_count {
        payloads.push(serde_json::json!({
            "type": "mousePressed",
            "x": x,
            "y": y,
            "button": button,
            "buttons": pressed_buttons,
            "clickCount": current_count,
            "modifiers": modifiers,
            "pointerType": "mouse",
        }));
        payloads.push(serde_json::json!({
            "type": "mouseReleased",
            "x": x,
            "y": y,
            "button": button,
            "buttons": 0,
            "clickCount": current_count,
            "modifiers": modifiers,
            "pointerType": "mouse",
        }));
    }
    Ok(payloads)
}

struct TrustedKeySpec {
    key: &'static str,
    code: &'static str,
    virtual_key_code: u16,
    text: Option<&'static str>,
}

fn trusted_key_spec(key: &str) -> Option<TrustedKeySpec> {
    let spec = match key {
        "Enter" => ("Enter", "Enter", 13, Some("\r")),
        "Tab" => ("Tab", "Tab", 9, None),
        "Escape" | "Esc" => ("Escape", "Escape", 27, None),
        " " | "Space" | "Spacebar" => (" ", "Space", 32, Some(" ")),
        "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowUp" => ("ArrowUp", "ArrowUp", 38, None),
        "ArrowRight" => ("ArrowRight", "ArrowRight", 39, None),
        "ArrowDown" => ("ArrowDown", "ArrowDown", 40, None),
        "Home" => ("Home", "Home", 36, None),
        "End" => ("End", "End", 35, None),
        "PageUp" => ("PageUp", "PageUp", 33, None),
        "PageDown" => ("PageDown", "PageDown", 34, None),
        "Backspace" => ("Backspace", "Backspace", 8, None),
        "Delete" => ("Delete", "Delete", 46, None),
        _ => return None,
    };
    Some(TrustedKeySpec {
        key: spec.0,
        code: spec.1,
        virtual_key_code: spec.2,
        text: spec.3,
    })
}

fn trusted_key_payloads(key: &str, modifiers: &[String]) -> Result<Vec<serde_json::Value>, String> {
    let spec =
        trusted_key_spec(key).ok_or_else(|| format!("Unsupported trusted non-text key '{key}'"))?;
    let modifiers = trusted_input_modifiers(modifiers)?;
    let text = (modifiers & (1 | 2 | 4) == 0)
        .then_some(spec.text)
        .flatten();
    let mut key_down = serde_json::json!({
        "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": spec.key,
        "code": spec.code,
        "windowsVirtualKeyCode": spec.virtual_key_code,
        "nativeVirtualKeyCode": spec.virtual_key_code,
        "modifiers": modifiers,
        "autoRepeat": false,
        "isKeypad": false,
    });
    if let Some(text) = text {
        key_down["text"] = serde_json::Value::String(text.to_string());
        key_down["unmodifiedText"] = serde_json::Value::String(text.to_string());
    }
    Ok(vec![
        key_down,
        serde_json::json!({
                "type": "keyUp",
                "key": spec.key,
                "code": spec.code,
                "windowsVirtualKeyCode": spec.virtual_key_code,
                "nativeVirtualKeyCode": spec.virtual_key_code,
                "modifiers": modifiers,
                "autoRepeat": false,
                "isKeypad": false,
        }),
    ])
}

fn trusted_text_payload(text: &str) -> Result<serde_json::Value, String> {
    if text.len() > MAX_TRUSTED_TEXT_BYTES {
        return Err(format!(
            "Trusted text input exceeds the {MAX_TRUSTED_TEXT_BYTES}-byte limit"
        ));
    }
    Ok(serde_json::json!({ "text": text }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedInputEventBudget {
    pointer_down: u8,
    key_down: u8,
    input: u8,
}

impl TrustedInputEventBudget {
    pub fn new(pointer_down: u8, key_down: u8, input: u8) -> Result<Self, String> {
        if pointer_down > 2 || key_down > 1 || input > 2 {
            return Err(
                "Trusted input budget exceeds pointerDown=2, keyDown=1, or input=2".to_string(),
            );
        }
        if pointer_down + key_down + input == 0 {
            return Err("Trusted input budget must expect at least one event".to_string());
        }
        Ok(Self {
            pointer_down,
            key_down,
            input,
        })
    }

    pub fn pointer_click(click_count: u8, expected_input_events: u8) -> Result<Self, String> {
        if expected_input_events > click_count {
            return Err("Trusted pointer input events cannot exceed the click count".to_string());
        }
        Self::new(click_count, 0, expected_input_events)
    }

    pub fn text_insert() -> Self {
        Self {
            pointer_down: 0,
            key_down: 0,
            input: 1,
        }
    }

    pub fn key_press(expected_input_events: u8) -> Result<Self, String> {
        if expected_input_events > 1 {
            return Err("Trusted key input events cannot exceed one".to_string());
        }
        Self::new(0, 1, expected_input_events)
    }

    fn as_json(self) -> serde_json::Value {
        serde_json::json!({
            "pointerDown": self.pointer_down,
            "keyDown": self.key_down,
            "input": self.input,
        })
    }
}

#[derive(Debug, Clone)]
pub enum TrustedInputMatch {
    Pointer { x: f64, y: f64, button: String },
    Text { data: String },
    Key { key: String },
}

impl TrustedInputMatch {
    fn as_json(&self) -> serde_json::Value {
        match self {
            Self::Pointer { x, y, button } => serde_json::json!({
                "kind": "pointer",
                "x": x,
                "y": y,
                "button": button,
            }),
            Self::Text { data } => serde_json::json!({
                "kind": "text",
                "data": data,
            }),
            Self::Key { key } => serde_json::json!({
                "kind": "key",
                "key": key,
            }),
        }
    }
}

pub fn trusted_key_input_match(key: &str) -> Result<TrustedInputMatch, String> {
    let spec =
        trusted_key_spec(key).ok_or_else(|| format!("Unsupported trusted non-text key '{key}'"))?;
    Ok(TrustedInputMatch::Key {
        key: spec.key.to_string(),
    })
}

#[derive(Clone)]
pub struct BrowserTrustedInputGuard {
    webview: Webview,
    token: Arc<str>,
}

impl BrowserTrustedInputGuard {
    pub async fn arm(
        &self,
        budget: TrustedInputEventBudget,
        expected: TrustedInputMatch,
    ) -> Result<ArmedTrustedInputGuard, String> {
        let baseline_physical_input_epoch = physical_input_epoch()?;
        let operation_id = uuid::Uuid::new_v4().simple().to_string();
        let expression = trusted_input_guard_expression(
            "arm",
            &self.token,
            &operation_id,
            Some((budget, &expected)),
        );
        let armed = eval_json(&self.webview, &expression)
            .await?
            .as_bool()
            .unwrap_or(false);
        if !armed {
            return Err(
                "Browser trusted-input guard rejected the arm request; no input was dispatched"
                    .to_string(),
            );
        }
        if physical_input_epoch()? != baseline_physical_input_epoch {
            let disarm = trusted_input_guard_expression("disarm", &self.token, &operation_id, None);
            let _ = eval_json(&self.webview, &disarm).await;
            return Err(
                "Physical user input occurred while the trusted-input guard was arming; no Agent input was dispatched"
                    .to_string(),
            );
        }
        Ok(ArmedTrustedInputGuard {
            guard: self.clone(),
            operation_id,
            budget,
            physical_input_epoch: baseline_physical_input_epoch,
            armed: true,
        })
    }
}

#[must_use = "dropping the armed guard disarms the trusted-input allowance"]
pub struct ArmedTrustedInputGuard {
    guard: BrowserTrustedInputGuard,
    operation_id: String,
    budget: TrustedInputEventBudget,
    physical_input_epoch: u32,
    armed: bool,
}

impl ArmedTrustedInputGuard {
    fn webview(&self) -> &Webview {
        &self.guard.webview
    }

    pub async fn disarm(mut self) -> Result<(), String> {
        let input_changed_before_disarm = self.physical_input_changed()?;
        let expression =
            trusted_input_guard_expression("disarm", &self.guard.token, &self.operation_id, None);
        let disarmed = eval_json(&self.guard.webview, &expression)
            .await?
            .as_bool()
            .unwrap_or(false);
        if !disarmed {
            return Err(
                "Browser trusted-input guard rejected the disarm request; control state is uncertain"
                    .to_string(),
            );
        }
        self.armed = false;
        if input_changed_before_disarm || self.physical_input_changed()? {
            return Err(
                "Physical user input overlapped the trusted WebView action; control state is uncertain"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn physical_input_changed(&self) -> Result<bool, String> {
        Ok(physical_input_epoch()? != self.physical_input_epoch)
    }
}

impl Drop for ArmedTrustedInputGuard {
    fn drop(&mut self) {
        if self.armed {
            let expression = trusted_input_guard_expression(
                "disarm",
                &self.guard.token,
                &self.operation_id,
                None,
            );
            let _ = self.guard.webview.eval(expression);
        }
    }
}

fn trusted_input_guard_expression(
    action: &str,
    token: &str,
    operation_id: &str,
    guarded: Option<(TrustedInputEventBudget, &TrustedInputMatch)>,
) -> String {
    let token = serde_json::to_string(token).expect("trusted input token is serializable");
    let operation_id =
        serde_json::to_string(operation_id).expect("trusted input operation id is serializable");
    match guarded {
        Some((budget, expected)) => format!(
            "(() => {{ const guard = window.__NEXA_TRUSTED_INPUT_GUARD__; return Boolean(guard && guard.{action}({token}, {operation_id}, {}, {})); }})()",
            budget.as_json(),
            expected.as_json(),
        ),
        None => format!(
            "(() => {{ const guard = window.__NEXA_TRUSTED_INPUT_GUARD__; return Boolean(guard && guard.{action}({token}, {operation_id})); }})()"
        ),
    }
}

#[cfg(windows)]
fn physical_input_epoch() -> Result<u32, String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return Err(format!(
            "Could not read the Windows physical-input epoch: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(info.dwTime)
}

#[cfg(not(windows))]
fn physical_input_epoch() -> Result<u32, String> {
    Ok(0)
}

/// Dispatch a trusted WebView pointer click through the DevTools input domain.
/// `click_count` is deliberately limited to a single or double click.
/// The caller must first validate an observation-scoped target, active visible
/// tab, current control/visibility generations and an in-viewport hit point.
/// An error can follow a partially delivered sequence and must not be retried
/// without a fresh observation.
pub async fn dispatch_trusted_pointer_click(
    guard: &ArmedTrustedInputGuard,
    x: f64,
    y: f64,
    button: &str,
    modifiers: &[String],
    click_count: u8,
) -> Result<(), String> {
    if guard.budget.pointer_down != click_count
        || guard.budget.key_down != 0
        || guard.budget.input > click_count
    {
        return Err("Trusted pointer guard budget does not match this click sequence".to_string());
    }
    let payloads = trusted_pointer_payloads(x, y, button, modifiers, click_count)?;
    dispatch_cdp_sequence(guard, "Input.dispatchMouseEvent", payloads).await
}

/// Insert bounded text into the already validated, selected and focused
/// WebView target. The caller owns replacement/selection semantics and must
/// not retry an uncertain error without a fresh observation.
pub async fn insert_trusted_text(guard: &ArmedTrustedInputGuard, text: &str) -> Result<(), String> {
    if guard.budget != TrustedInputEventBudget::text_insert() {
        return Err("Trusted text guard must expect exactly one input event".to_string());
    }
    let payload = trusted_text_payload(text)?;
    dispatch_cdp_sequence(guard, "Input.insertText", vec![payload]).await
}

/// Dispatch one allowlisted, non-text key press to the validated and focused
/// WebView target. The caller must hold the current control/visibility lease.
pub async fn dispatch_trusted_key(
    guard: &ArmedTrustedInputGuard,
    key: &str,
    modifiers: &[String],
) -> Result<(), String> {
    if guard.budget.pointer_down != 0 || guard.budget.key_down != 1 || guard.budget.input > 1 {
        return Err("Trusted key guard budget does not match one key sequence".to_string());
    }
    let payloads = trusted_key_payloads(key, modifiers)?;
    dispatch_cdp_sequence(guard, "Input.dispatchKeyEvent", payloads).await
}

pub struct CapturedWebviewScreenshot {
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct BrowserChildWebview {
    pub webview: Webview,
    pub approved_agent_urls: Arc<Mutex<HashSet<String>>>,
    pub trusted_input_guard: BrowserTrustedInputGuard,
}

pub fn create_child_webview(
    state: &BrowserState,
    session_id: &str,
    tab_id: &str,
    url: Url,
    profile_dir: PathBuf,
    profile_id: &str,
    agent_restricted: Arc<AtomicBool>,
    network_proxy_url: Url,
    bounds: Option<BrowserBounds>,
) -> Result<BrowserChildWebview, String> {
    let window = state
        .app()
        .get_window("main")
        .ok_or_else(|| "Main application window is unavailable".to_string())?;
    let label = format!("browser-{}", tab_id.trim_start_matches("tab_"));
    let approved_agent_urls = Arc::new(Mutex::new(HashSet::from([url.to_string()])));
    let takeover_token = uuid::Uuid::new_v4().simple().to_string();
    let pick_token = uuid::Uuid::new_v4().simple().to_string();
    let takeover_url = Url::parse(&format!("nexa-user-input://{takeover_token}"))
        .map_err(|error| format!("Could not create browser takeover signal: {error}"))?;

    let navigation_restriction = Arc::clone(&agent_restricted);
    let approved_for_navigation = Arc::clone(&approved_agent_urls);
    let takeover_for_navigation = takeover_url.clone();
    let state_for_takeover = state.clone();
    let session_for_takeover = session_id.to_string();
    let tab_for_takeover = tab_id.to_string();
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
        .data_store_identifier(profile_data_store_identifier(profile_id))
        .disable_drag_drop_handler()
        .proxy_url(network_proxy_url.clone())
        .initialization_script_for_all_frames(browser_init_script(&pick_token))
        .initialization_script_for_all_frames(browser_takeover_script(&takeover_token))
        .on_navigation(move |target| {
            if target == &takeover_for_navigation {
                state_for_takeover.record_user_takeover(&session_for_takeover, &tab_for_takeover);
                return false;
            }
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
    #[cfg(windows)]
    let builder = builder.additional_browser_args(&format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-quic --proxy-server={} --proxy-bypass-list=<-loopback>",
        network_proxy_url
    ));
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
    let trusted_input_guard = BrowserTrustedInputGuard {
        webview: webview.clone(),
        token: Arc::from(takeover_token),
    };
    Ok(BrowserChildWebview {
        webview,
        approved_agent_urls,
        trusted_input_guard,
    })
}

fn profile_data_store_identifier(profile_id: &str) -> [u8; 16] {
    let hash = blake3::hash(format!("nexa-browser-profile:{profile_id}").as_bytes());
    let mut identifier = [0_u8; 16];
    identifier.copy_from_slice(&hash.as_bytes()[..16]);
    identifier
}

#[derive(Deserialize)]
struct EvalEnvelope {
    ok: bool,
    value: Option<serde_json::Value>,
    error: Option<String>,
}

pub struct PendingEvalJson {
    receiver: oneshot::Receiver<String>,
}

impl PendingEvalJson {
    pub async fn resolve(self) -> Result<serde_json::Value, String> {
        let raw = tokio::time::timeout(std::time::Duration::from_secs(10), self.receiver)
            .await
            .map_err(|_| "Browser script timed out".to_string())?
            .map_err(|_| "Browser script response was dropped".to_string())?;
        decode_eval_json(&raw)
    }
}

pub fn dispatch_eval_json(webview: &Webview, expression: &str) -> Result<PendingEvalJson, String> {
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
    Ok(PendingEvalJson { receiver })
}

pub async fn eval_json(webview: &Webview, expression: &str) -> Result<serde_json::Value, String> {
    dispatch_eval_json(webview, expression)?.resolve().await
}

#[cfg(windows)]
async fn call_devtools_protocol_method(
    webview: &Webview,
    method: &str,
    parameters: serde_json::Value,
    timeout: std::time::Duration,
) -> Result<String, String> {
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR,
        Microsoft::Web::WebView2::Win32::ICoreWebView2,
    };

    let method = method.to_string();
    let parameters = serde_json::to_string(&parameters)
        .map_err(|error| format!("Could not encode DevTools parameters for {method}: {error}"))?;
    let method_for_dispatch = method.clone();
    let (sender, receiver) = oneshot::channel::<Result<String, String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let sender_for_dispatch = Arc::clone(&sender);
    webview
        .with_webview(move |platform| {
            let dispatch = (|| -> Result<(), String> {
                let core: ICoreWebView2 =
                    unsafe { platform.controller().CoreWebView2() }.map_err(|error| {
                        format!("Could not access Browser Workspace WebView2: {error}")
                    })?;
                let sender_for_callback = Arc::clone(&sender_for_dispatch);
                let method_for_callback = method_for_dispatch.clone();
                let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                    move |status, payload| {
                        let result = status.map(|_| payload).map_err(|error| {
                            format!("DevTools method {method_for_callback} failed: {error}")
                        });
                        send_devtools_reply(&sender_for_callback, result);
                        Ok(())
                    },
                ));
                let method = CoTaskMemPWSTR::from(method_for_dispatch.as_str());
                let parameters = CoTaskMemPWSTR::from(parameters.as_str());
                unsafe {
                    core.CallDevToolsProtocolMethod(
                        *method.as_ref().as_pcwstr(),
                        *parameters.as_ref().as_pcwstr(),
                        &handler,
                    )
                }
                .map_err(|error| {
                    format!("Could not dispatch DevTools method {method_for_dispatch}: {error}")
                })
            })();
            if let Err(error) = dispatch {
                send_devtools_reply(&sender_for_dispatch, Err(error));
            }
        })
        .map_err(|error| format!("Could not access Browser Workspace WebView: {error}"))?;

    tokio::time::timeout(timeout, receiver)
        .await
        .map_err(|_| format!("DevTools method {method} timed out"))?
        .map_err(|_| format!("DevTools method {method} response was dropped"))?
}

#[cfg(windows)]
pub async fn capture_webview_png(
    webview: &Webview,
) -> Result<Option<CapturedWebviewScreenshot>, String> {
    let payload = call_devtools_protocol_method(
        webview,
        "Page.captureScreenshot",
        serde_json::json!({
            "format": "png",
            "fromSurface": true,
            "captureBeyondViewport": false,
        }),
        SCREENSHOT_TIMEOUT,
    )
    .await
    .map_err(|error| format!("Browser screenshot capture failed: {error}"))?;
    decode_cdp_screenshot(&payload).map(Some)
}

#[cfg(not(windows))]
pub async fn capture_webview_png(
    _webview: &Webview,
) -> Result<Option<CapturedWebviewScreenshot>, String> {
    Ok(None)
}

#[cfg(windows)]
fn send_devtools_reply(
    sender: &Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>,
    result: Result<String, String>,
) {
    if let Ok(mut sender) = sender.lock() {
        if let Some(sender) = sender.take() {
            let _ = sender.send(result);
        }
    }
}

#[cfg(windows)]
async fn dispatch_cdp_sequence(
    guard: &ArmedTrustedInputGuard,
    method: &str,
    payloads: Vec<serde_json::Value>,
) -> Result<(), String> {
    tokio::time::timeout(TRUSTED_INPUT_TIMEOUT, async {
        let event_count = payloads.len();
        for (index, payload) in payloads.into_iter().enumerate() {
            if guard.physical_input_changed()? {
                return Err(format!(
                    "Physical user input appeared before trusted WebView event {}/{}; Agent dispatch was stopped",
                    index + 1,
                    event_count,
                ));
            }
            call_devtools_protocol_method(guard.webview(), method, payload, TRUSTED_INPUT_TIMEOUT)
                .await
                .map_err(|error| {
                    format!(
                        "Trusted WebView input via {method} failed at event {}/{}; earlier events may already have been delivered: {error}",
                        index + 1,
                        event_count,
                    )
                })?;
            if guard.physical_input_changed()? {
                return Err(format!(
                    "Physical user input overlapped trusted WebView event {}/{}; effect is uncertain",
                    index + 1,
                    event_count,
                ));
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| {
        format!(
            "Trusted WebView input via {method} timed out; earlier events may already have been delivered"
        )
    })?
}

#[cfg(not(windows))]
async fn dispatch_cdp_sequence(
    _guard: &ArmedTrustedInputGuard,
    method: &str,
    _payloads: Vec<serde_json::Value>,
) -> Result<(), String> {
    Err(format!(
        "Trusted WebView input via {method} is unsupported on this platform"
    ))
}

#[cfg(windows)]
fn decode_cdp_screenshot(payload: &str) -> Result<CapturedWebviewScreenshot, String> {
    #[derive(Deserialize)]
    struct CaptureEnvelope {
        data: String,
    }

    let envelope: CaptureEnvelope = serde_json::from_str(payload)
        .map_err(|error| format!("Could not decode browser screenshot response: {error}"))?;
    if envelope.data.len() > MAX_SCREENSHOT_BASE64_BYTES {
        return Err("Browser screenshot exceeded the safe capture limit".to_string());
    }
    let png_bytes = STANDARD
        .decode(envelope.data)
        .map_err(|error| format!("Browser screenshot returned invalid base64: {error}"))?;
    if png_bytes.len() > MAX_SCREENSHOT_BYTES {
        return Err("Browser screenshot exceeded the safe capture limit".to_string());
    }
    let (width, height) = png_dimensions(&png_bytes)?;
    Ok(CapturedWebviewScreenshot {
        png_bytes,
        width,
        height,
    })
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != PNG_SIGNATURE || &png[12..16] != b"IHDR" {
        return Err("Browser screenshot was not a valid PNG image".to_string());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().expect("validated PNG width"));
    let height = u32::from_be_bytes(png[20..24].try_into().expect("validated PNG height"));
    if width == 0 || height == 0 {
        return Err("Browser screenshot had empty dimensions".to_string());
    }
    Ok((width, height))
}

fn decode_eval_json(raw: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
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

#[cfg(test)]
mod tests {
    use super::{
        browser_takeover_script, png_dimensions, trusted_input_guard_expression,
        trusted_key_input_match, trusted_key_payloads, trusted_pointer_payloads,
        trusted_text_payload, TrustedInputEventBudget, TrustedInputMatch, MAX_TRUSTED_TEXT_BYTES,
    };

    #[test]
    fn png_dimensions_reject_non_png_and_read_ihdr() {
        assert!(png_dimensions(b"not a png").is_err());
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640_u32.to_be_bytes());
        png.extend_from_slice(&480_u32.to_be_bytes());
        assert_eq!(png_dimensions(&png).unwrap(), (640, 480));
    }

    #[test]
    fn trusted_pointer_payloads_encode_a_bounded_double_click_sequence() {
        let payloads = trusted_pointer_payloads(
            120.5,
            64.25,
            "left",
            &["Control".to_string(), "Shift".to_string()],
            2,
        )
        .expect("valid trusted double click");

        assert_eq!(payloads.len(), 5);
        assert_eq!(payloads[0]["type"], "mouseMoved");
        assert_eq!(payloads[1]["type"], "mousePressed");
        assert_eq!(payloads[2]["type"], "mouseReleased");
        assert_eq!(payloads[3]["clickCount"], 2);
        assert_eq!(payloads[4]["clickCount"], 2);
        assert_eq!(payloads[1]["buttons"], 1);
        assert_eq!(payloads[2]["buttons"], 0);
        assert_eq!(payloads[1]["modifiers"], 10);
        assert_eq!(payloads[1]["x"], 120.5);
        assert_eq!(payloads[1]["y"], 64.25);

        assert!(trusted_pointer_payloads(f64::NAN, 1.0, "left", &[], 1).is_err());
        assert!(trusted_pointer_payloads(-1.0, 1.0, "left", &[], 1).is_err());
        assert!(trusted_pointer_payloads(1.0, 1.0, "back", &[], 1).is_err());
        assert!(trusted_pointer_payloads(1.0, 1.0, "left", &[], 3).is_err());
        assert!(trusted_pointer_payloads(1.0, 1.0, "left", &["CapsLock".to_string()], 1,).is_err());
    }

    #[test]
    fn trusted_key_payloads_allow_only_named_non_text_keys() {
        let payloads = trusted_key_payloads("Enter", &["Shift".to_string()])
            .expect("Enter should be supported");
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0]["type"], "keyDown");
        assert_eq!(payloads[1]["type"], "keyUp");
        assert_eq!(payloads[0]["key"], "Enter");
        assert_eq!(payloads[0]["code"], "Enter");
        assert_eq!(payloads[0]["windowsVirtualKeyCode"], 13);
        assert_eq!(payloads[0]["modifiers"], 8);
        assert_eq!(payloads[0]["text"], "\r");
        assert_eq!(
            trusted_key_payloads("Tab", &[]).unwrap()[0]["type"],
            "rawKeyDown"
        );
        assert!(trusted_key_payloads("A", &[]).is_err());
        assert!(trusted_key_payloads("F12", &[]).is_err());
        assert!(matches!(
            trusted_key_input_match("Spacebar").unwrap(),
            TrustedInputMatch::Key { key } if key == " "
        ));
        assert!(matches!(
            trusted_key_input_match("Esc").unwrap(),
            TrustedInputMatch::Key { key } if key == "Escape"
        ));
    }

    #[test]
    fn trusted_text_payload_preserves_text_but_rejects_oversized_input() {
        let payload = trusted_text_payload("hello 世界").expect("bounded text should be accepted");
        assert_eq!(payload["text"], "hello 世界");
        assert!(trusted_text_payload(&"x".repeat(MAX_TRUSTED_TEXT_BYTES + 1)).is_err());
    }

    #[test]
    fn trusted_input_budget_and_guard_expression_are_exact_and_bounded() {
        let budget = TrustedInputEventBudget::new(2, 0, 2).unwrap();
        let expected = TrustedInputMatch::Pointer {
            x: 123.5,
            y: 45.0,
            button: "left".to_string(),
        };
        let expression = trusted_input_guard_expression(
            "arm",
            "secret-token",
            "operation-1",
            Some((budget, &expected)),
        );
        assert!(expression.contains("guard.arm(\"secret-token\", \"operation-1\""));
        assert!(expression.contains(r#""pointerDown":2"#));
        assert!(expression.contains(r#""keyDown":0"#));
        assert!(expression.contains(r#""input":2"#));
        assert!(expression.contains(r#""kind":"pointer""#));
        assert!(expression.contains(r#""x":123.5"#));
        assert!(TrustedInputEventBudget::new(0, 0, 0).is_err());
        assert!(TrustedInputEventBudget::new(3, 0, 0).is_err());
        assert!(TrustedInputEventBudget::new(0, 2, 0).is_err());
        assert!(TrustedInputEventBudget::new(0, 0, 3).is_err());
        assert!(TrustedInputEventBudget::pointer_click(1, 2).is_err());
        assert!(TrustedInputEventBudget::key_press(2).is_err());
        assert_eq!(
            TrustedInputEventBudget::text_insert(),
            TrustedInputEventBudget::new(0, 0, 1).unwrap()
        );
    }

    #[test]
    fn takeover_script_requires_token_authenticated_exact_single_use_events() {
        let script = browser_takeover_script("takeover-secret");
        assert!(script.contains("Object.defineProperty(window, '__NEXA_TRUSTED_INPUT_GUARD__'"));
        assert!(script.contains("providedToken !== trustedInputToken"));
        assert!(script.contains("trustedInputGuard[budgetKey] -= 1"));
        assert!(script.contains("matchesTrustedInput(type, event)"));
        assert!(script.contains("Math.abs(event.clientX - expected.x) <= 2"));
        assert!(script.contains("eventTargetsArmedElement(event)"));
        assert!(script.contains("event.isTrusted && !consumeTrustedInput(type, event)"));
        assert!(script.contains("event.source !== window.parent"));
        assert!(script.contains("configurable: false"));
        assert!(script.contains("writable: false"));
        assert!(script.contains("\"takeover-secret\""));
        assert!(!script.contains("trustedInputBypass = true"));
    }
}
