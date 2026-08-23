use crate::error::CoreError;

/// Process-wide owner for foreground desktop input. Browser native-pointer
/// commits and general computer control must never interleave.
pub fn desktop_input_arbiter() -> &'static std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
        std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
}

pub async fn acquire_desktop_input_permit() -> Result<tokio::sync::OwnedMutexGuard<()>, CoreError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        std::sync::Arc::clone(desktop_input_arbiter()).lock_owned(),
    )
    .await
    .map_err(|_| {
        CoreError::InvalidInput(
            "Desktop input is still owned by another action after 5 seconds. No new input was sent; inspect the existing action receipt or restart the desktop input broker before retrying."
                .to_string(),
        )
    })
}

#[cfg(target_os = "windows")]
pub struct CrossProcessDesktopInputGuard {
    handle: windows::Win32::Foundation::HANDLE,
    owned: bool,
}

// The guard owns a Win32 semaphore, not a thread-affine mutex. Windows permits
// ReleaseSemaphore from a different thread, so moving it across Tokio awaits
// is sound.
#[cfg(target_os = "windows")]
unsafe impl Send for CrossProcessDesktopInputGuard {}

#[cfg(target_os = "windows")]
pub fn try_acquire_cross_process_input() -> Result<CrossProcessDesktopInputGuard, CoreError> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{CreateSemaphoreW, WaitForSingleObject};

    let handle = unsafe {
        CreateSemaphoreW(
            None,
            1,
            1,
            windows::core::w!("Local\\NexaDesktopInputSemaphoreV3"),
        )
    }
    .map_err(|error| CoreError::Internal(format!("create desktop-input semaphore: {error}")))?;
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    if wait == WAIT_OBJECT_0 {
        return Ok(CrossProcessDesktopInputGuard {
            handle,
            owned: true,
        });
    }
    let _ = unsafe { CloseHandle(handle) };
    if wait == WAIT_TIMEOUT {
        return Err(CoreError::InvalidInput(
            "Another Nexa process is currently using global desktop input. Re-observe after that action finishes."
                .to_string(),
        ));
    }
    Err(CoreError::Internal(
        "Could not acquire the cross-process desktop-input semaphore".to_string(),
    ))
}

#[cfg(target_os = "windows")]
impl Drop for CrossProcessDesktopInputGuard {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::ReleaseSemaphore;

        if self.owned {
            let _ = unsafe { ReleaseSemaphore(self.handle, 1, None) };
            self.owned = false;
        }
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(not(target_os = "windows"))]
pub struct CrossProcessDesktopInputGuard;

#[cfg(not(target_os = "windows"))]
pub fn try_acquire_cross_process_input() -> Result<CrossProcessDesktopInputGuard, CoreError> {
    Err(CoreError::InvalidInput(
        "Cross-process desktop input is unavailable on this platform".to_string(),
    ))
}

#[cfg(target_os = "windows")]
pub fn move_native_pointer(x: i32, y: i32) -> Result<(), CoreError> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    unsafe { SetCursorPos(x, y) }
        .map_err(|error| CoreError::Internal(format!("move browser pointer: {error}")))?;
    let mut actual = POINT::default();
    unsafe { GetCursorPos(&mut actual) }
        .map_err(|error| CoreError::Internal(format!("verify browser pointer: {error}")))?;
    if actual.x.abs_diff(x) > 1 || actual.y.abs_diff(y) > 1 {
        return Err(CoreError::InvalidInput(format!(
            "Browser pointer moved to ({}, {}) instead of ({x}, {y}); refusing uncertain multi-display input.",
            actual.x, actual.y
        )));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn move_native_pointer(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::InvalidInput(
        "Native Browser Workspace pointer movement is unavailable on this platform".to_string(),
    ))
}
