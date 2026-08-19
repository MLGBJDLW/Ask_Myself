use crate::error::CoreError;

#[cfg(target_os = "windows")]
pub fn move_native_pointer(x: i32, y: i32) -> Result<(), CoreError> {
    use enigo::{Coordinate, Enigo, Mouse, Settings};

    let mut input = Enigo::new(&Settings::default()).map_err(|error| {
        CoreError::Internal(format!("initialize browser pointer injection: {error}"))
    })?;
    input
        .move_mouse(x, y, Coordinate::Abs)
        .map_err(|error| CoreError::Internal(format!("move browser pointer: {error}")))
}

#[cfg(not(target_os = "windows"))]
pub fn move_native_pointer(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::InvalidInput(
        "Native Browser Workspace pointer movement is unavailable on this platform".to_string(),
    ))
}
