use std::sync::Mutex;

use nexa_core::app_settings::{
    AppConfig, CompanionAnchor, CompanionDisplayMode, CompanionInteractionMode,
    CompanionLogicalPosition, CompanionSettings,
};
use serde::Serialize;
use tauri::{
    App, AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalRect, PhysicalSize,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::commands::AppState;

pub const COMPANION_WINDOW_LABEL: &str = "companion";
// This viewport is the single size authority. The renderer fills it and never
// applies the user scale a second time.
const COMPANION_WIDTH: f64 = 144.0;
const COMPANION_HEIGHT: f64 = 168.0;
const WORK_AREA_MARGIN: i32 = 12;

#[derive(Debug, Default)]
pub struct CompanionWindowState {
    inner: Mutex<CompanionWindowRuntime>,
}

#[derive(Debug, Default)]
struct CompanionWindowRuntime {
    renderer_ready: bool,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionWindowDiagnostics {
    pub platform: &'static str,
    pub window_available: bool,
    pub renderer_ready: bool,
    pub visible: bool,
    pub click_through: bool,
    pub last_error: Option<String>,
    pub limitations: Vec<&'static str>,
}

fn record_error(app: &AppHandle, error: impl Into<String>) {
    let error = error.into();
    log::warn!("[companion-window] {error}");
    if let Some(state) = app.try_state::<CompanionWindowState>() {
        if let Ok(mut runtime) = state.inner.lock() {
            runtime.last_error = Some(error);
        }
    }
}

fn work_area_bounds(
    area: &PhysicalRect<i32, u32>,
    window_size: PhysicalSize<u32>,
) -> (i32, i32, i32, i32) {
    let min_x = area.position.x.saturating_add(WORK_AREA_MARGIN);
    let min_y = area.position.y.saturating_add(WORK_AREA_MARGIN);
    let max_x = area
        .position
        .x
        .saturating_add(area.size.width.saturating_sub(window_size.width) as i32)
        .saturating_sub(WORK_AREA_MARGIN)
        .max(min_x);
    let max_y = area
        .position
        .y
        .saturating_add(area.size.height.saturating_sub(window_size.height) as i32)
        .saturating_sub(WORK_AREA_MARGIN)
        .max(min_y);
    (min_x, min_y, max_x, max_y)
}

fn clamp_position_to_work_area(
    area: &PhysicalRect<i32, u32>,
    window_size: PhysicalSize<u32>,
    position: PhysicalPosition<i32>,
) -> PhysicalPosition<i32> {
    let (min_x, min_y, max_x, max_y) = work_area_bounds(area, window_size);
    PhysicalPosition::new(
        position.x.clamp(min_x, max_x),
        position.y.clamp(min_y, max_y),
    )
}

fn monitor_identity(monitor: &tauri::Monitor) -> String {
    format!(
        "{}@{}:{}:{}x{}",
        monitor.name().map(String::as_str).unwrap_or("unnamed"),
        monitor.position().x,
        monitor.position().y,
        monitor.size().width,
        monitor.size().height
    )
}

fn target_monitor(window: &WebviewWindow, settings: &CompanionSettings) -> Option<tauri::Monitor> {
    let monitors = window.available_monitors().ok()?;
    settings
        .monitor_id
        .as_deref()
        .and_then(|id| {
            monitors
                .iter()
                .find(|monitor| monitor_identity(monitor) == id)
                .cloned()
        })
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| monitors.into_iter().next())
}

fn clamped_position(
    monitor: &tauri::Monitor,
    window_size: PhysicalSize<u32>,
    settings: &CompanionSettings,
) -> PhysicalPosition<i32> {
    let (min_x, _, max_x, max_y) = work_area_bounds(monitor.work_area(), window_size);
    let scale_factor = monitor.scale_factor();
    let (x, y) = match settings.position {
        Some(position) => (
            monitor
                .position()
                .x
                .saturating_add((position.x * scale_factor).round() as i32),
            monitor
                .position()
                .y
                .saturating_add((position.y * scale_factor).round() as i32),
        ),
        None => match settings.anchor {
            CompanionAnchor::BottomLeft => (min_x, max_y),
            CompanionAnchor::BottomRight | CompanionAnchor::Free => (max_x, max_y),
        },
    };
    clamp_position_to_work_area(
        monitor.work_area(),
        window_size,
        PhysicalPosition::new(x, y),
    )
}

fn place_inside_work_area(
    window: &WebviewWindow,
    settings: &CompanionSettings,
) -> Result<(), String> {
    let monitor = target_monitor(window, settings)
        .ok_or_else(|| "No monitor is available for the Companion window".to_string())?;
    let window_size = window
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new(COMPANION_WIDTH as u32, COMPANION_HEIGHT as u32));
    window
        .set_position(clamped_position(&monitor, window_size, settings))
        .map_err(|error| format!("Failed to position Companion window: {error}"))
}

fn should_reapply_configured_anchor(anchor: CompanionAnchor, has_persisted_position: bool) -> bool {
    !has_persisted_position && !matches!(anchor, CompanionAnchor::Free)
}

fn keep_current_position_inside_work_area(
    window: &WebviewWindow,
    settings: &CompanionSettings,
) -> Result<(), String> {
    if should_reapply_configured_anchor(settings.anchor, settings.position.is_some()) {
        return place_inside_work_area(window, settings);
    }
    let monitor = target_monitor(window, settings)
        .ok_or_else(|| "No monitor is available for the Companion window".to_string())?;
    let window_size = window
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new(COMPANION_WIDTH as u32, COMPANION_HEIGHT as u32));
    let current = window
        .outer_position()
        .map_err(|error| format!("Failed to read Companion window position: {error}"))?;
    let clamped = clamp_position_to_work_area(monitor.work_area(), window_size, current);
    if clamped == current {
        return Ok(());
    }
    window
        .set_position(clamped)
        .map_err(|error| format!("Failed to keep Companion window inside the work area: {error}"))
}

fn apply_window_attributes(
    window: &WebviewWindow,
    settings: &CompanionSettings,
) -> Result<(), String> {
    if let Err(error) = window.set_always_on_top(settings.always_on_top) {
        log::warn!("[companion-window] topmost state is unsupported: {error}");
    }
    if let Err(error) = window.set_visible_on_all_workspaces(settings.visible_on_all_workspaces) {
        log::warn!("[companion-window] all-workspaces visibility is unsupported: {error}");
    }
    if let Err(error) = window.set_ignore_cursor_events(
        settings.interaction_mode == CompanionInteractionMode::ClickThrough,
    ) {
        log::warn!("[companion-window] click-through is unsupported: {error}");
    }
    window
        .set_size(LogicalSize::new(
            COMPANION_WIDTH * f64::from(settings.scale),
            COMPANION_HEIGHT * f64::from(settings.scale),
        ))
        .map_err(|error| format!("Failed to scale Companion window: {error}"))?;
    place_inside_work_area(window, settings)
}

pub fn apply_companion_settings(app: &AppHandle, settings: &CompanionSettings, show: bool) {
    let Some(window) = app.get_webview_window(COMPANION_WINDOW_LABEL) else {
        record_error(app, "Companion window is unavailable on this platform");
        return;
    };
    if let Err(error) = apply_window_attributes(&window, settings) {
        record_error(app, error);
    }
    if !settings.enabled {
        let _ = window.hide();
        let _ = app.emit("companion://visibility", false);
        return;
    }
    let ready = app
        .try_state::<CompanionWindowState>()
        .and_then(|state| {
            state
                .inner
                .lock()
                .ok()
                .map(|runtime| runtime.renderer_ready)
        })
        .unwrap_or(false);
    if show && ready && settings.display_mode != CompanionDisplayMode::Manual {
        if let Err(error) = window.show() {
            record_error(app, format!("Failed to show Companion window: {error}"));
        } else {
            let _ = app.emit("companion://visibility", true);
        }
    }
}

pub fn create_companion_window(app: &mut App, settings: &CompanionSettings) {
    app.manage(CompanionWindowState::default());
    let builder = WebviewWindowBuilder::new(
        app,
        COMPANION_WINDOW_LABEL,
        WebviewUrl::App("companion".into()),
    )
    .title("Nexa Companion")
    .inner_size(
        COMPANION_WIDTH * f64::from(settings.scale),
        COMPANION_HEIGHT * f64::from(settings.scale),
    )
    .resizable(false)
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .always_on_top(settings.always_on_top)
    .visible_on_all_workspaces(settings.visible_on_all_workspaces)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .visible(false);

    match builder.build() {
        Ok(window) => {
            if let Err(error) = apply_window_attributes(&window, settings) {
                record_error(app.handle(), error);
            }
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
                loop {
                    interval.tick().await;
                    let Ok(config) = load_config(&app_handle) else {
                        continue;
                    };
                    let Some(window) = app_handle.get_webview_window(COMPANION_WINDOW_LABEL) else {
                        break;
                    };
                    if config.companion.enabled && window.is_visible().unwrap_or(false) {
                        // Declarative anchors follow work-area changes. Free or
                        // persisted placement uses the live native position so
                        // automatic roaming is not reset every three seconds.
                        if let Err(error) =
                            keep_current_position_inside_work_area(&window, &config.companion)
                        {
                            record_error(&app_handle, error);
                        }
                    }
                }
            });
        }
        Err(error) => record_error(
            app.handle(),
            format!("Companion window creation is unsupported or failed: {error}"),
        ),
    }
}

fn load_config(app: &AppHandle) -> Result<AppConfig, String> {
    app.try_state::<AppState>()
        .ok_or_else(|| "Application state is unavailable".to_string())?
        .db
        .load_app_config()
        .map_err(|error| error.to_string())
}

fn save_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    app.try_state::<AppState>()
        .ok_or_else(|| "Application state is unavailable".to_string())?
        .db
        .save_app_config(config)
        .map_err(|error| error.to_string())
}

pub fn show_companion(app: &AppHandle) -> Result<(), String> {
    let config = load_config(app)?;
    if !config.companion.enabled {
        return Err("Desktop Companion is disabled in Appearance settings".to_string());
    }
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    apply_window_attributes(&window, &config.companion)?;
    window.show().map_err(|error| error.to_string())?;
    let _ = app.emit("companion://visibility", true);
    Ok(())
}

pub fn hide_companion(app: &AppHandle) -> Result<(), String> {
    app.get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())?;
    let _ = app.emit("companion://visibility", false);
    Ok(())
}

pub fn unlock_companion(app: &AppHandle) -> Result<(), String> {
    let mut config = load_config(app)?;
    config.companion.interaction_mode = CompanionInteractionMode::Smart;
    config.companion.lock_position = false;
    save_config(app, &config)?;
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    window
        .set_ignore_cursor_events(false)
        .map_err(|error| error.to_string())
}

pub fn lock_companion(app: &AppHandle) -> Result<(), String> {
    let mut config = load_config(app)?;
    config.companion.interaction_mode = CompanionInteractionMode::Locked;
    config.companion.lock_position = true;
    save_config(app, &config)?;
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    window
        .set_ignore_cursor_events(false)
        .map_err(|error| error.to_string())
}

pub fn reset_companion_position(app: &AppHandle) -> Result<(), String> {
    let mut config = load_config(app)?;
    config.companion.position = None;
    config.companion.monitor_id = None;
    save_config(app, &config)?;
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    place_inside_work_area(&window, &config.companion)
}

#[tauri::command]
pub fn companion_renderer_ready_cmd(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<CompanionWindowState>()
        .ok_or_else(|| "Companion runtime is unavailable".to_string())?;
    state
        .inner
        .lock()
        .map_err(|_| "Companion runtime lock is poisoned".to_string())?
        .renderer_ready = true;
    let config = load_config(&app)?;
    apply_companion_settings(&app, &config.companion, config.companion.auto_show_on_start);
    Ok(())
}

#[tauri::command]
pub fn show_companion_cmd(app: AppHandle) -> Result<(), String> {
    show_companion(&app)
}

#[tauri::command]
pub fn hide_companion_cmd(app: AppHandle) -> Result<(), String> {
    hide_companion(&app)
}

#[tauri::command]
pub fn toggle_companion_cmd(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    if window.is_visible().unwrap_or(false) {
        hide_companion(&app)
    } else {
        show_companion(&app)
    }
}

#[tauri::command]
pub fn set_companion_interaction_cmd(
    app: AppHandle,
    mode: CompanionInteractionMode,
) -> Result<(), String> {
    let mut config = load_config(&app)?;
    config.companion.interaction_mode = mode;
    save_config(&app, &config)?;
    apply_companion_settings(&app, &config.companion, false);
    Ok(())
}

#[tauri::command]
pub fn persist_companion_position_cmd(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(COMPANION_WINDOW_LABEL)
        .ok_or_else(|| "Companion window is unavailable".to_string())?;
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "No monitor is available".to_string())?;
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let scale_factor = monitor.scale_factor();
    let mut config = load_config(&app)?;
    config.companion.monitor_id = Some(monitor_identity(&monitor));
    config.companion.anchor = CompanionAnchor::Free;
    config.companion.position = Some(CompanionLogicalPosition {
        x: f64::from(position.x.saturating_sub(monitor.position().x)) / scale_factor,
        y: f64::from(position.y.saturating_sub(monitor.position().y)) / scale_factor,
        scale_factor,
    });
    save_config(&app, &config)
}

#[tauri::command]
pub fn reset_companion_position_cmd(app: AppHandle) -> Result<(), String> {
    reset_companion_position(&app)
}

#[tauri::command]
pub fn get_companion_window_diagnostics_cmd(app: AppHandle) -> CompanionWindowDiagnostics {
    let runtime = app
        .try_state::<CompanionWindowState>()
        .and_then(|state| {
            state
                .inner
                .lock()
                .ok()
                .map(|runtime| (runtime.renderer_ready, runtime.last_error.clone()))
        })
        .unwrap_or((false, Some("Companion runtime is unavailable".to_string())));
    let window = app.get_webview_window(COMPANION_WINDOW_LABEL);
    let config = load_config(&app).ok();
    CompanionWindowDiagnostics {
        platform: std::env::consts::OS,
        window_available: window.is_some(),
        renderer_ready: runtime.0,
        visible: window
            .as_ref()
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false),
        click_through: config.is_some_and(|config| {
            config.companion.interaction_mode == CompanionInteractionMode::ClickThrough
        }),
        last_error: runtime.1,
        limitations: platform_limitations(),
    }
}

fn platform_limitations() -> Vec<&'static str> {
    #[cfg(target_os = "linux")]
    {
        vec!["Transparency, click-through, and always-on-top depend on the active compositor"]
    }
    #[cfg(target_os = "macos")]
    {
        vec!["Skip-taskbar behavior is provided by accessory-window semantics on macOS"]
    }
    #[cfg(target_os = "windows")]
    {
        Vec::new()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        vec!["Desktop Companion is not validated on this platform"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::{PhysicalPosition, PhysicalRect, PhysicalSize};

    #[test]
    fn work_area_clamp_keeps_window_inside_negative_origin_monitor() {
        let work_area = PhysicalRect {
            position: PhysicalPosition::new(-1920, 0),
            size: PhysicalSize::new(1920, 1040),
        };
        let bounds = work_area_bounds(&work_area, PhysicalSize::new(144, 168));
        assert_eq!(bounds, (-1908, 12, -156, 860));
        assert_eq!(
            clamp_position_to_work_area(
                &work_area,
                PhysicalSize::new(144, 168),
                PhysicalPosition::new(-4000, 1200),
            ),
            PhysicalPosition::new(-1908, 860)
        );
        assert_eq!(
            clamp_position_to_work_area(
                &work_area,
                PhysicalSize::new(144, 168),
                PhysicalPosition::new(-1000, 400),
            ),
            PhysicalPosition::new(-1000, 400),
            "an in-bounds live roaming position must not be reset to persisted settings",
        );
        assert!(should_reapply_configured_anchor(
            CompanionAnchor::BottomLeft,
            false
        ));
        assert!(should_reapply_configured_anchor(
            CompanionAnchor::BottomRight,
            false
        ));
        assert!(!should_reapply_configured_anchor(
            CompanionAnchor::Free,
            false
        ));
        assert!(!should_reapply_configured_anchor(
            CompanionAnchor::BottomLeft,
            true
        ));
    }
}
