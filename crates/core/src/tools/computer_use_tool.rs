//! Native, observation-scoped computer use for the Windows desktop.
//!
//! Observation and input are intentionally separate tools. Screenshots are
//! ephemeral visual evidence, while every input-producing call passes through
//! the normal high-risk approval gate and must reference a fresh observation.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolOutput, ToolOutputAttachment, ToolResult};

static OBSERVE_DEF: OnceLock<ToolDef> = OnceLock::new();
static CONTROL_DEF: OnceLock<ToolDef> = OnceLock::new();
const OBSERVE_DEF_JSON: &str = include_str!("../../prompts/tools/computer_observe.json");
const CONTROL_DEF_JSON: &str = include_str!("../../prompts/tools/computer_control.json");
const OBSERVATION_TTL: Duration = Duration::from_secs(120);
const MAX_OBSERVATIONS: usize = 64;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct WindowSnapshot {
    id: u64,
    pid: u32,
    app_name: String,
    title: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    minimized: bool,
    maximized: bool,
    focused: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct ObservedWindow {
    snapshot: WindowSnapshot,
    image_width: Option<u32>,
    image_height: Option<u32>,
    screenshot_hash: Option<String>,
}

#[derive(Debug)]
struct ObservationRecord {
    id: String,
    created_at: Instant,
    conversation_id: Option<String>,
    windows: Vec<ObservedWindow>,
}

fn observation_store() -> &'static Mutex<VecDeque<ObservationRecord>> {
    static STORE: OnceLock<Mutex<VecDeque<ObservationRecord>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn remember_observation(
    conversation_id: Option<&str>,
    windows: Vec<ObservedWindow>,
) -> Result<String, CoreError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut store = observation_store()
        .lock()
        .map_err(|_| CoreError::Internal("Computer observation store is unavailable.".into()))?;
    while store
        .front()
        .is_some_and(|record| now.duration_since(record.created_at) > OBSERVATION_TTL)
    {
        store.pop_front();
    }
    while store.len() >= MAX_OBSERVATIONS {
        store.pop_front();
    }
    store.push_back(ObservationRecord {
        id: id.clone(),
        created_at: now,
        conversation_id: conversation_id.map(str::to_string),
        windows,
    });
    Ok(id)
}

fn observed_window(
    conversation_id: Option<&str>,
    observation_id: &str,
    window_id: u64,
) -> Result<ObservedWindow, CoreError> {
    let now = Instant::now();
    let store = observation_store()
        .lock()
        .map_err(|_| CoreError::Internal("Computer observation store is unavailable.".into()))?;
    let record = store
        .iter()
        .find(|record| record.id == observation_id)
        .ok_or_else(|| {
            CoreError::InvalidInput(
                "Unknown computer observation. Run computer_observe list_windows again."
                    .to_string(),
            )
        })?;
    if now.duration_since(record.created_at) > OBSERVATION_TTL {
        return Err(CoreError::InvalidInput(
            "Computer observation expired. Run computer_observe again before acting.".to_string(),
        ));
    }
    if record.conversation_id.as_deref() != conversation_id {
        return Err(CoreError::InvalidInput(
            "Computer observation belongs to a different conversation. Observe the window again before acting."
                .to_string(),
        ));
    }
    record
        .windows
        .iter()
        .find(|window| window.snapshot.id == window_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Window {window_id} was not present in observation {observation_id}."
            ))
        })
}

#[derive(Debug, Deserialize)]
struct ObserveArgs {
    action: String,
    #[serde(default)]
    observation_id: Option<String>,
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct ControlArgs {
    action: String,
    observation_id: String,
    window_id: u64,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    to_x: Option<f64>,
    #[serde(default)]
    to_y: Option<f64>,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    click_count: Option<u8>,
    #[serde(default)]
    scroll_x: Option<i32>,
    #[serde(default)]
    scroll_y: Option<i32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key_sequence: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlAction {
    FocusWindow,
    MoveMouse,
    Click,
    Drag,
    Scroll,
    TypeText,
    Key,
}

impl ControlAction {
    fn parse(value: &str) -> Result<Self, CoreError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "focus_window" => Ok(Self::FocusWindow),
            "move_mouse" => Ok(Self::MoveMouse),
            "click" => Ok(Self::Click),
            "drag" => Ok(Self::Drag),
            "scroll" => Ok(Self::Scroll),
            "type_text" => Ok(Self::TypeText),
            "key" => Ok(Self::Key),
            other => Err(CoreError::InvalidInput(format!(
                "Unsupported computer_control action '{other}'."
            ))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FocusWindow => "focus_window",
            Self::MoveMouse => "move_mouse",
            Self::Click => "click",
            Self::Drag => "drag",
            Self::Scroll => "scroll",
            Self::TypeText => "type_text",
            Self::Key => "key",
        }
    }
}

#[derive(Debug)]
struct CapturedWindow {
    snapshot: WindowSnapshot,
    png: Vec<u8>,
    image_width: u32,
    image_height: u32,
    native_image_width: u32,
    native_image_height: u32,
}

#[derive(Debug)]
struct ControlOutcome {
    summary: String,
    capture: Option<CapturedWindow>,
    observation_error: Option<String>,
    cursor_position: Option<(i32, i32)>,
    target_verified: bool,
    state_changed: bool,
}

fn local_desktop_trust_boundary() -> serde_json::Value {
    serde_json::json!({
        "origin": "local_desktop",
        "authority": "observation",
        "visibility": "current_chat",
        "mutability": "read_only",
        "externality": "local",
        "canInstruct": false
    })
}

fn screenshot_attachment(capture: &CapturedWindow) -> ToolOutputAttachment {
    ToolOutputAttachment {
        name: format!("window-{}.png", capture.snapshot.id),
        mime_type: "image/png".to_string(),
        data: serde_json::json!({ "base64": STANDARD.encode(&capture.png) }),
    }
}

fn capture_data(observation_id: &str, capture: &CapturedWindow) -> serde_json::Value {
    serde_json::json!({
        "observationId": observation_id,
        "window": capture.snapshot,
        "imageWidth": capture.image_width,
        "imageHeight": capture.image_height,
        "nativeImageWidth": capture.native_image_width,
        "nativeImageHeight": capture.native_image_height,
        "coordinateSpace": "captured_image_pixels",
        "screenshotHash": blake3::hash(&capture.png).to_hex().to_string(),
        "expiresInSeconds": OBSERVATION_TTL.as_secs()
    })
}

async fn blocking<T, F>(operation: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CoreError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| CoreError::Internal(format!("Computer use worker failed: {error}")))?
}

pub struct ComputerObserveTool;

#[async_trait]
impl Tool for ComputerObserveTool {
    fn name(&self) -> &str {
        "computer_observe"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&OBSERVE_DEF, OBSERVE_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&OBSERVE_DEF, OBSERVE_DEF_JSON)
            .parameters
            .clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::DesktopInteract]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db: _db,
            source_scope: _source_scope,
            conversation_id,
            ..
        } = context;
        let args: ObserveArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid computer_observe arguments: {error}"))
        })?;

        match args.action.trim().to_ascii_lowercase().as_str() {
            "list_windows" => {
                let max_results = args.max_results.unwrap_or(50).clamp(1, 100);
                let windows = blocking(platform::list_windows).await?;
                let windows: Vec<WindowSnapshot> = windows.into_iter().take(max_results).collect();
                let observation_id = remember_observation(
                    conversation_id,
                    windows
                        .iter()
                        .cloned()
                        .map(|snapshot| ObservedWindow {
                            snapshot,
                            image_width: None,
                            image_height: None,
                            screenshot_hash: None,
                        })
                        .collect(),
                )?;
                let data = serde_json::json!({
                    "observationId": observation_id,
                    "windows": windows,
                    "expiresInSeconds": OBSERVATION_TTL.as_secs()
                });
                let content = format!(
                    "Observed {} capturable Windows windows. Use observationId {} with capture_window before coordinate-based input.\n{}",
                    windows.len(),
                    observation_id,
                    serde_json::to_string_pretty(&data).unwrap_or_default()
                );
                Ok(ToolResult::from_output(
                    call_id,
                    false,
                    ToolOutput {
                        llm_content: content.clone(),
                        display_content: content,
                        data: Some(data),
                        artifacts: Some(serde_json::json!({
                            "kind": "computerObservation",
                            "trustBoundary": local_desktop_trust_boundary()
                        })),
                        attachments: Vec::new(),
                    },
                ))
            }
            "capture_window" => {
                let observation_id = args.observation_id.as_deref().ok_or_else(|| {
                    CoreError::InvalidInput(
                        "capture_window requires observation_id from list_windows.".to_string(),
                    )
                })?;
                let window_id = args.window_id.ok_or_else(|| {
                    CoreError::InvalidInput("capture_window requires window_id.".to_string())
                })?;
                let observed = observed_window(conversation_id, observation_id, window_id)?;
                let capture =
                    blocking(move || platform::capture_window(&observed.snapshot)).await?;
                let fresh_observation_id = remember_observation(
                    conversation_id,
                    vec![ObservedWindow {
                        snapshot: capture.snapshot.clone(),
                        image_width: Some(capture.image_width),
                        image_height: Some(capture.image_height),
                        screenshot_hash: Some(blake3::hash(&capture.png).to_hex().to_string()),
                    }],
                )?;
                let data = capture_data(&fresh_observation_id, &capture);
                let content = format!(
                    "Captured window {} ('{}') at {}x{} image pixels. Use observationId {} and these image coordinates for the next computer_control action.",
                    capture.snapshot.id,
                    capture.snapshot.title,
                    capture.image_width,
                    capture.image_height,
                    fresh_observation_id
                );
                Ok(ToolResult::from_output(
                    call_id,
                    false,
                    ToolOutput {
                        llm_content: content.clone(),
                        display_content: content,
                        data: Some(data),
                        artifacts: Some(serde_json::json!({
                            "kind": "computerObservation",
                            "trustBoundary": local_desktop_trust_boundary()
                        })),
                        attachments: vec![screenshot_attachment(&capture)],
                    },
                ))
            }
            "cursor_position" => {
                let (x, y) = blocking(platform::cursor_position).await?;
                let data = serde_json::json!({ "x": x, "y": y, "coordinateSpace": "screen_physical_pixels" });
                let content = format!("Cursor is at screen position ({x}, {y}).");
                Ok(ToolResult::from_output(
                    call_id,
                    false,
                    ToolOutput {
                        llm_content: content.clone(),
                        display_content: content,
                        data: Some(data),
                        artifacts: Some(serde_json::json!({
                            "kind": "computerObservation",
                            "trustBoundary": local_desktop_trust_boundary()
                        })),
                        attachments: Vec::new(),
                    },
                ))
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unsupported computer_observe action '{other}'."
            ))),
        }
    }
}

pub struct ComputerControlTool;

#[async_trait]
impl Tool for ComputerControlTool {
    fn name(&self) -> &str {
        "computer_control"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&CONTROL_DEF, CONTROL_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&CONTROL_DEF, CONTROL_DEF_JSON)
            .parameters
            .clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::DesktopInteract]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("desktop input");
        let window_id = args
            .get("window_id")
            .and_then(serde_json::Value::as_u64)
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let reason = args
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| format!(" Reason: {}", reason.trim()))
            .unwrap_or_default();
        Some(format!(
            "Allow computer action '{action}' in observed window {window_id}?{reason}"
        ))
    }

    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
        false
    }

    fn resource_keys(&self, _args: &serde_json::Value) -> Vec<String> {
        vec!["desktop:global-input".to_string()]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db: _db,
            source_scope: _source_scope,
            conversation_id,
            activity_runtime,
            ..
        } = context;
        let args: ControlArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid computer_control arguments: {error}"))
        })?;
        let action = ControlAction::parse(&args.action)?;
        let observed = observed_window(conversation_id, &args.observation_id, args.window_id)?;
        let window_id = args.window_id;
        let reason = args.reason.clone();
        if let Some(runtime) = activity_runtime {
            let mut spec = crate::activity::ActivitySpec::new(
                crate::activity::ActivitySurface::Desktop,
                "computer_control",
            )
            .with_activity_id(call_id);
            if let Some(conversation_id) = conversation_id {
                spec = spec.with_conversation_id(conversation_id);
            }
            runtime.start(spec)?;
        }
        let outcome =
            match blocking(move || platform::control_window(action, &args, &observed)).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    if let Some(runtime) = activity_runtime {
                        let _ = runtime.transition(
                            call_id,
                            crate::activity::ActivityState::Failed,
                            serde_json::json!({ "error": error.to_string() }),
                        );
                    }
                    return Err(error);
                }
            };

        let (fresh_observation_id, capture_data_value, attachments) =
            if let Some(capture) = outcome.capture.as_ref() {
                let observation_id = remember_observation(
                    conversation_id,
                    vec![ObservedWindow {
                        snapshot: capture.snapshot.clone(),
                        image_width: Some(capture.image_width),
                        image_height: Some(capture.image_height),
                        screenshot_hash: Some(blake3::hash(&capture.png).to_hex().to_string()),
                    }],
                )?;
                (
                    Some(observation_id.clone()),
                    Some(capture_data(&observation_id, capture)),
                    vec![screenshot_attachment(capture)],
                )
            } else {
                (None, None, Vec::new())
            };

        let data = serde_json::json!({
            "action": action.label(),
            "actionApplied": true,
            "targetVerified": outcome.target_verified,
            "stateChanged": outcome.state_changed,
            "windowId": window_id,
            "reason": reason,
            "observationId": fresh_observation_id,
            "observation": capture_data_value,
            "observationError": outcome.observation_error,
            "cursorPosition": outcome.cursor_position.map(|(x, y)| serde_json::json!({ "x": x, "y": y }))
        });
        if let Some(runtime) = activity_runtime {
            let _ = runtime.append(
                call_id,
                crate::activity::ActivityEventKind::DesktopObservation,
                data.clone(),
            );
            let _ = runtime.transition(
                call_id,
                crate::activity::ActivityState::Completed,
                serde_json::json!({ "stateChanged": outcome.state_changed }),
            );
        }
        let mut content = outcome.summary;
        if let Some(observation_id) = fresh_observation_id {
            content.push_str(&format!(
                " Fresh post-action observationId: {observation_id}."
            ));
        } else if let Some(error) = outcome.observation_error.as_deref() {
            content.push_str(&format!(" Post-action observation failed: {error}"));
        }

        Ok(ToolResult::from_output(
            call_id,
            false,
            ToolOutput {
                llm_content: content.clone(),
                display_content: content,
                data: Some(data),
                artifacts: Some(serde_json::json!({
                    "kind": "computerControl",
                    "activityId": activity_runtime.map(|_| call_id),
                    "approved": true,
                    "trustBoundary": local_desktop_trust_boundary()
                })),
                attachments,
            },
        ))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::c_void;
    use std::io::Cursor;
    use std::sync::mpsc::{self, SyncSender};
    use std::thread;

    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
    use image::{imageops::FilterType, DynamicImage, ImageFormat, RgbaImage};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetCursorPos, GetForegroundWindow, IsIconic, IsWindow, IsZoomed, SetForegroundWindow,
            ShowWindow, SW_RESTORE,
        },
    };
    use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
    use windows_capture::frame::Frame;
    use windows_capture::graphics_capture_api::InternalCaptureControl;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings as CaptureSettings,
    };
    use windows_capture::window::Window;

    use super::{
        CapturedWindow, ControlAction, ControlArgs, ControlOutcome, CoreError, ObservedWindow,
        WindowSnapshot,
    };

    const MAX_CAPTURE_EDGE: u32 = 1_600;
    const INPUT_SETTLE: Duration = Duration::from_millis(70);
    const POST_ACTION_STATE_BUDGET: Duration = Duration::from_millis(1_200);
    const POST_ACTION_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

    use std::time::Duration;

    fn invalid(message: impl Into<String>) -> CoreError {
        CoreError::InvalidInput(message.into())
    }

    fn platform_error(context: &str, error: impl std::fmt::Display) -> CoreError {
        CoreError::Internal(format!("{context}: {error}"))
    }

    fn snapshot(window: &Window) -> Result<WindowSnapshot, CoreError> {
        let rect = window
            .rect()
            .map_err(|error| platform_error("read window bounds", error))?;
        let handle = window.as_raw_hwnd();
        Ok(WindowSnapshot {
            id: handle as usize as u64,
            pid: window
                .process_id()
                .map_err(|error| platform_error("read window pid", error))?,
            app_name: window.process_name().unwrap_or_default(),
            title: window
                .title()
                .map_err(|error| platform_error("read window title", error))?,
            x: rect.left,
            y: rect.top,
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
            minimized: unsafe { IsIconic(HWND(handle)).as_bool() },
            maximized: unsafe { IsZoomed(HWND(handle)).as_bool() },
            focused: unsafe { GetForegroundWindow() == HWND(handle) },
        })
    }

    fn enumerated_windows() -> Result<Vec<(Window, WindowSnapshot)>, CoreError> {
        let windows =
            Window::enumerate().map_err(|error| platform_error("enumerate windows", error))?;
        let mut result = Vec::new();
        for window in windows {
            let Ok(snapshot) = snapshot(&window) else {
                continue;
            };
            if snapshot.title.trim().is_empty() || snapshot.width == 0 || snapshot.height == 0 {
                continue;
            }
            result.push((window, snapshot));
        }
        Ok(result)
    }

    pub(super) fn list_windows() -> Result<Vec<WindowSnapshot>, CoreError> {
        Ok(enumerated_windows()?
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .collect())
    }

    fn current_window(expected: &WindowSnapshot) -> Result<(Window, WindowSnapshot), CoreError> {
        let (window, current) = enumerated_windows()?
            .into_iter()
            .find(|(_, snapshot)| snapshot.id == expected.id)
            .ok_or_else(|| invalid(format!("Observed window {} no longer exists.", expected.id)))?;
        if current.pid != expected.pid || current.app_name != expected.app_name {
            return Err(invalid(format!(
                "Window {} changed owner since observation; refusing stale desktop access.",
                expected.id
            )));
        }
        Ok((window, current))
    }

    fn resized_png(image: RgbaImage) -> Result<(Vec<u8>, u32, u32, u32, u32), CoreError> {
        let native_width = image.width();
        let native_height = image.height();
        let longest = native_width.max(native_height);
        let (image_width, image_height, image) = if longest > MAX_CAPTURE_EDGE {
            let scale = MAX_CAPTURE_EDGE as f64 / longest as f64;
            let width = ((native_width as f64 * scale).round() as u32).max(1);
            let height = ((native_height as f64 * scale).round() as u32).max(1);
            (
                width,
                height,
                image::imageops::resize(&image, width, height, FilterType::Triangle),
            )
        } else {
            (native_width, native_height, image)
        };
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|error| platform_error("encode window screenshot", error))?;
        Ok((
            png.into_inner(),
            image_width,
            image_height,
            native_width,
            native_height,
        ))
    }

    struct OneFrameCapture {
        sender: SyncSender<Result<RgbaImage, String>>,
    }

    impl GraphicsCaptureApiHandler for OneFrameCapture {
        type Flags = SyncSender<Result<RgbaImage, String>>;
        type Error = String;

        fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                sender: context.flags,
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            let width = frame.width();
            let height = frame.height();
            let result = (|| {
                let buffer = frame
                    .buffer()
                    .map_err(|error| format!("read capture frame: {error}"))?;
                let mut compact = Vec::new();
                let pixels = buffer.as_nopadding_buffer(&mut compact).to_vec();
                RgbaImage::from_raw(width, height, pixels)
                    .ok_or_else(|| "capture frame had an invalid RGBA buffer".to_string())
            })();
            let _ = self.sender.send(result);
            capture_control.stop();
            Ok(())
        }
    }

    fn capture_rgba(window: Window) -> Result<RgbaImage, CoreError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let settings = CaptureSettings::new(
            window,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            sender,
        );
        OneFrameCapture::start(settings).map_err(|error| {
            platform_error("capture window with Windows Graphics Capture", error)
        })?;
        receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| platform_error("receive Windows capture frame", error))?
            .map_err(|error| platform_error("decode Windows capture frame", error))
    }

    pub(super) fn capture_window(expected: &WindowSnapshot) -> Result<CapturedWindow, CoreError> {
        let (window, current) = current_window(expected)?;
        if current.minimized {
            return Err(invalid(format!(
                "Window {} is minimized. Focus or restore it before capture.",
                current.id
            )));
        }
        let image = capture_rgba(window)?;
        let (png, image_width, image_height, native_image_width, native_image_height) =
            resized_png(image)?;
        Ok(CapturedWindow {
            snapshot: current,
            png,
            image_width,
            image_height,
            native_image_width,
            native_image_height,
        })
    }

    fn hwnd(window_id: u64) -> HWND {
        HWND(window_id as usize as *mut c_void)
    }

    fn focus_window(window: &WindowSnapshot) -> Result<(), CoreError> {
        let handle = hwnd(window.id);
        unsafe {
            if !IsWindow(Some(handle)).as_bool() {
                return Err(invalid(format!("Window {} is no longer valid.", window.id)));
            }
            if IsIconic(handle).as_bool() {
                let _ = ShowWindow(handle, SW_RESTORE);
                thread::sleep(INPUT_SETTLE);
            }
            if GetForegroundWindow() != handle && !SetForegroundWindow(handle).as_bool() {
                return Err(invalid(format!(
                    "Windows refused to focus window {}. Bring it to the foreground and retry.",
                    window.id
                )));
            }
        }
        thread::sleep(INPUT_SETTLE);
        if unsafe { GetForegroundWindow() } != handle {
            return Err(invalid(format!(
                "Window {} did not become foreground; no input was sent.",
                window.id
            )));
        }
        Ok(())
    }

    fn enigo() -> Result<Enigo, CoreError> {
        Enigo::new(&Settings::default())
            .map_err(|error| platform_error("initialize Windows input injection", error))
    }

    fn image_point(
        x: Option<f64>,
        y: Option<f64>,
        observed: &ObservedWindow,
        current: &WindowSnapshot,
        label: &str,
    ) -> Result<(i32, i32), CoreError> {
        let x = x.ok_or_else(|| invalid(format!("{label} requires x.")))?;
        let y = y.ok_or_else(|| invalid(format!("{label} requires y.")))?;
        let (image_width, image_height) = observed
            .image_width
            .zip(observed.image_height)
            .ok_or_else(|| {
                invalid(format!(
                    "{label} needs captured-image coordinates. Run computer_observe capture_window first."
                ))
            })?;
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x >= image_width as f64
            || y >= image_height as f64
        {
            return Err(invalid(format!(
                "Point ({x}, {y}) is outside the captured image bounds {image_width}x{image_height}."
            )));
        }
        let local_x = (x * current.width as f64 / image_width as f64)
            .round()
            .clamp(0.0, current.width.saturating_sub(1) as f64) as i32;
        let local_y = (y * current.height as f64 / image_height as f64)
            .round()
            .clamp(0.0, current.height.saturating_sub(1) as f64) as i32;
        Ok((current.x + local_x, current.y + local_y))
    }

    fn mouse_button(value: Option<&str>) -> Result<Button, CoreError> {
        match value.unwrap_or("left").trim().to_ascii_lowercase().as_str() {
            "left" => Ok(Button::Left),
            "right" => Ok(Button::Right),
            "middle" => Ok(Button::Middle),
            other => Err(invalid(format!("Unsupported mouse button '{other}'."))),
        }
    }

    fn named_key(value: &str) -> Result<Key, CoreError> {
        let normalized = value.trim().to_ascii_lowercase();
        let key = match normalized.as_str() {
            "ctrl" | "control" => Key::Control,
            "alt" | "option" => Key::Alt,
            "shift" => Key::Shift,
            "meta" | "win" | "windows" | "command" | "cmd" => Key::Meta,
            "enter" | "return" => Key::Return,
            "tab" => Key::Tab,
            "space" => Key::Space,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "escape" | "esc" => Key::Escape,
            "up" | "arrowup" => Key::UpArrow,
            "down" | "arrowdown" => Key::DownArrow,
            "left" | "arrowleft" => Key::LeftArrow,
            "right" | "arrowright" => Key::RightArrow,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "f1" => Key::F1,
            "f2" => Key::F2,
            "f3" => Key::F3,
            "f4" => Key::F4,
            "f5" => Key::F5,
            "f6" => Key::F6,
            "f7" => Key::F7,
            "f8" => Key::F8,
            "f9" => Key::F9,
            "f10" => Key::F10,
            "f11" => Key::F11,
            "f12" => Key::F12,
            "plus" => Key::Unicode('+'),
            _ => {
                let mut chars = value.chars();
                let Some(character) = chars.next() else {
                    return Err(invalid("Key sequence contains an empty key."));
                };
                if chars.next().is_some() {
                    return Err(invalid(format!("Unsupported key name '{value}'.")));
                }
                Key::Unicode(character)
            }
        };
        Ok(key)
    }

    fn send_key_sequence(enigo: &mut Enigo, sequence: &str) -> Result<(), CoreError> {
        let parts: Vec<&str> = sequence
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() || parts.len() > 8 {
            return Err(invalid(
                "key_sequence must contain between one and eight '+'-separated keys.",
            ));
        }
        let keys: Vec<Key> = parts
            .iter()
            .map(|part| named_key(part))
            .collect::<Result<_, _>>()?;
        if keys.len() == 1 {
            return enigo
                .key(keys[0], Direction::Click)
                .map_err(|error| platform_error("send key", error));
        }

        let mut pressed = Vec::new();
        let mut action_error = None;
        for key in &keys[..keys.len() - 1] {
            match enigo.key(*key, Direction::Press) {
                Ok(()) => pressed.push(*key),
                Err(error) => {
                    action_error = Some(platform_error("press shortcut modifier", error));
                    break;
                }
            }
        }
        if action_error.is_none() {
            if let Err(error) = enigo.key(keys[keys.len() - 1], Direction::Click) {
                action_error = Some(platform_error("send shortcut key", error));
            }
        }
        let mut release_error = None;
        while let Some(key) = pressed.pop() {
            if let Err(error) = enigo.key(key, Direction::Release) {
                release_error
                    .get_or_insert_with(|| platform_error("release shortcut modifier", error));
            }
        }
        if let Some(error) = action_error.or(release_error) {
            return Err(error);
        }
        Ok(())
    }

    fn drag_mouse(enigo: &mut Enigo, from: (i32, i32), to: (i32, i32)) -> Result<(), CoreError> {
        enigo
            .move_mouse(from.0, from.1, Coordinate::Abs)
            .map_err(|error| platform_error("move to drag start", error))?;
        thread::sleep(INPUT_SETTLE);
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|error| platform_error("press drag button", error))?;
        let mut movement_error = None;
        for frame in 1..=12 {
            let progress = frame as f64 / 12.0;
            let eased = 1.0 - (1.0 - progress).powi(3);
            let x = from.0 as f64 + (to.0 - from.0) as f64 * eased;
            let y = from.1 as f64 + (to.1 - from.1) as f64 * eased;
            if let Err(error) =
                enigo.move_mouse(x.round() as i32, y.round() as i32, Coordinate::Abs)
            {
                movement_error = Some(platform_error("drag mouse", error));
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        let release = enigo
            .button(Button::Left, Direction::Release)
            .map_err(|error| platform_error("release drag button", error));
        if let Some(error) = movement_error {
            let _ = release;
            return Err(error);
        }
        release
    }

    pub(super) fn cursor_position() -> Result<(i32, i32), CoreError> {
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe { GetCursorPos(&mut point) }
            .map_err(|error| platform_error("read cursor position", error))?;
        Ok((point.x, point.y))
    }

    pub(super) fn control_window(
        action: ControlAction,
        args: &ControlArgs,
        observed: &ObservedWindow,
    ) -> Result<ControlOutcome, CoreError> {
        let (_, current) = current_window(&observed.snapshot)?;
        let pre_action_capture = capture_window(&current)?;
        let pre_action_hash = blake3::hash(&pre_action_capture.png).to_hex().to_string();
        if observed
            .screenshot_hash
            .as_ref()
            .is_some_and(|expected| expected != &pre_action_hash)
        {
            return Err(invalid(
                "Desktop observation is stale because the captured window changed. Observe it again before acting.",
            ));
        }
        focus_window(&current)?;
        let mut input = enigo()?;

        let summary = match action {
            ControlAction::FocusWindow => {
                format!("Focused window {} ('{}').", current.id, current.title)
            }
            ControlAction::MoveMouse => {
                let point = image_point(args.x, args.y, observed, &current, "move_mouse")?;
                input
                    .move_mouse(point.0, point.1, Coordinate::Abs)
                    .map_err(|error| platform_error("move mouse", error))?;
                format!("Moved the cursor inside window {}.", current.id)
            }
            ControlAction::Click => {
                let point = image_point(args.x, args.y, observed, &current, "click")?;
                let button = mouse_button(args.button.as_deref())?;
                let count = args.click_count.unwrap_or(1).clamp(1, 3);
                input
                    .move_mouse(point.0, point.1, Coordinate::Abs)
                    .map_err(|error| platform_error("move mouse before click", error))?;
                thread::sleep(INPUT_SETTLE);
                for index in 0..count {
                    input
                        .button(button, Direction::Click)
                        .map_err(|error| platform_error("click mouse", error))?;
                    if index + 1 < count {
                        thread::sleep(Duration::from_millis(75));
                    }
                }
                format!("Clicked {count} time(s) inside window {}.", current.id)
            }
            ControlAction::Drag => {
                let from = image_point(args.x, args.y, observed, &current, "drag")?;
                let to = image_point(args.to_x, args.to_y, observed, &current, "drag")?;
                drag_mouse(&mut input, from, to)?;
                format!("Dragged inside window {}.", current.id)
            }
            ControlAction::Scroll => {
                let point = image_point(args.x, args.y, observed, &current, "scroll")?;
                let scroll_x = args.scroll_x.unwrap_or(0).clamp(-20, 20);
                let scroll_y = args.scroll_y.unwrap_or(0).clamp(-20, 20);
                if scroll_x == 0 && scroll_y == 0 {
                    return Err(invalid("scroll requires a non-zero scroll_x or scroll_y."));
                }
                input
                    .move_mouse(point.0, point.1, Coordinate::Abs)
                    .map_err(|error| platform_error("move mouse before scroll", error))?;
                thread::sleep(INPUT_SETTLE);
                if scroll_y != 0 {
                    input
                        .scroll(scroll_y, Axis::Vertical)
                        .map_err(|error| platform_error("scroll vertically", error))?;
                }
                if scroll_x != 0 {
                    input
                        .scroll(scroll_x, Axis::Horizontal)
                        .map_err(|error| platform_error("scroll horizontally", error))?;
                }
                format!("Scrolled inside window {}.", current.id)
            }
            ControlAction::TypeText => {
                let text = args
                    .text
                    .as_deref()
                    .ok_or_else(|| invalid("type_text requires text."))?;
                if text.is_empty() || text.chars().count() > 4_000 || text.contains('\0') {
                    return Err(invalid(
                        "type_text requires 1 to 4000 characters and cannot contain NUL bytes.",
                    ));
                }
                input
                    .text(text)
                    .map_err(|error| platform_error("type text", error))?;
                format!(
                    "Typed {} character(s) into window {}.",
                    text.chars().count(),
                    current.id
                )
            }
            ControlAction::Key => {
                let sequence = args
                    .key_sequence
                    .as_deref()
                    .ok_or_else(|| invalid("key requires key_sequence."))?;
                send_key_sequence(&mut input, sequence)?;
                format!("Sent key sequence '{}' to window {}.", sequence, current.id)
            }
        };

        let cursor_position = input.location().ok();
        let deadline = std::time::Instant::now() + POST_ACTION_STATE_BUDGET;
        let mut latest_capture = None;
        let mut latest_hash = pre_action_hash.clone();
        let mut stable_samples = 0_u8;
        while std::time::Instant::now() < deadline {
            if let Ok(capture) = capture_window(&current) {
                let hash = blake3::hash(&capture.png).to_hex().to_string();
                if hash == latest_hash {
                    stable_samples = stable_samples.saturating_add(1);
                } else {
                    stable_samples = 0;
                    latest_hash = hash;
                }
                latest_capture = Some(capture);
                if latest_hash != pre_action_hash && stable_samples >= 2 {
                    break;
                }
            }
            thread::sleep(POST_ACTION_SAMPLE_INTERVAL);
        }
        match latest_capture.or_else(|| capture_window(&current).ok()) {
            Some(capture) => Ok(ControlOutcome {
                state_changed: latest_hash != pre_action_hash,
                target_verified: true,
                summary,
                capture: Some(capture),
                observation_error: None,
                cursor_position,
            }),
            None => match capture_window(&current) {
                Ok(capture) => Ok(ControlOutcome {
                    summary,
                    capture: Some(capture),
                    observation_error: None,
                    cursor_position,
                    target_verified: true,
                    state_changed: false,
                }),
                Err(error) => Ok(ControlOutcome {
                    summary,
                    capture: None,
                    observation_error: Some(error.to_string()),
                    cursor_position,
                    target_verified: true,
                    state_changed: false,
                }),
            },
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{
        CapturedWindow, ControlAction, ControlArgs, ControlOutcome, CoreError, ObservedWindow,
        WindowSnapshot,
    };

    fn unsupported<T>() -> Result<T, CoreError> {
        Err(CoreError::InvalidInput(
            "Built-in computer use currently requires Windows. Configure a computer-use MCP connector on this platform."
                .to_string(),
        ))
    }

    pub(super) fn list_windows() -> Result<Vec<WindowSnapshot>, CoreError> {
        unsupported()
    }

    pub(super) fn capture_window(_expected: &WindowSnapshot) -> Result<CapturedWindow, CoreError> {
        unsupported()
    }

    pub(super) fn cursor_position() -> Result<(i32, i32), CoreError> {
        unsupported()
    }

    pub(super) fn control_window(
        _action: ControlAction,
        _args: &ControlArgs,
        _observed: &ObservedWindow,
    ) -> Result<ControlOutcome, CoreError> {
        unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_tokens_are_scoped_to_the_listed_window() {
        let snapshot = WindowSnapshot {
            id: 42,
            pid: 7,
            app_name: "Editor".to_string(),
            title: "Document".to_string(),
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            minimized: false,
            maximized: false,
            focused: true,
        };
        let observation_id = remember_observation(
            Some("conversation-1"),
            vec![ObservedWindow {
                snapshot: snapshot.clone(),
                image_width: Some(800),
                image_height: Some(600),
                screenshot_hash: None,
            }],
        )
        .unwrap();

        assert_eq!(
            observed_window(Some("conversation-1"), &observation_id, snapshot.id)
                .unwrap()
                .snapshot,
            snapshot
        );
        assert!(observed_window(Some("conversation-1"), &observation_id, 99).is_err());
        assert!(observed_window(Some("conversation-2"), &observation_id, snapshot.id).is_err());
    }

    #[test]
    fn control_actions_always_require_confirmation() {
        let tool = ComputerControlTool;
        for action in [
            "focus_window",
            "move_mouse",
            "click",
            "drag",
            "scroll",
            "type_text",
            "key",
        ] {
            assert!(tool.requires_confirmation(&serde_json::json!({
                "action": action,
                "observation_id": "observation",
                "window_id": 1
            })));
        }
    }

    #[test]
    fn computer_observation_is_read_only() {
        let tool = ComputerObserveTool;
        assert!(!tool.requires_confirmation(&serde_json::json!({
            "action": "capture_window"
        })));
        assert!(tool.is_read_only(&serde_json::json!({
            "action": "capture_window"
        })));
        let profile = tool.access_profile(&serde_json::json!({
            "action": "capture_window"
        }));
        assert!(!profile.needs_approval);
        assert!(!profile.can_write);
        assert!(!profile.can_execute);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn windows_window_capture_smoke_test() {
        let windows = platform::list_windows().expect("enumerate Windows windows");
        let target = windows
            .into_iter()
            .find(|window| !window.minimized)
            .expect("at least one non-minimized capturable window");
        let capture = platform::capture_window(&target).expect("capture Windows window");
        assert!(!capture.png.is_empty());
        assert!(capture.image_width > 0);
        assert!(capture.image_height > 0);
    }
}
