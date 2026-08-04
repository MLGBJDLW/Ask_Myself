//! OS process options for commands whose stdio is captured or discarded.
//!
//! On Windows, launching a console-subsystem executable such as `python.exe`,
//! `git.exe`, or `ffmpeg.exe` from the desktop app can briefly allocate a
//! visible console. Background helpers must opt out explicitly. User-visible
//! launchers such as Explorer and the platform `open` command intentionally do
//! not use these helpers.

#[cfg(any(windows, test))]
const WINDOWS_CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(any(windows, test))]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(any(windows, test))]
fn windows_creation_flags(own_process_group: bool) -> u32 {
    WINDOWS_CREATE_NO_WINDOW
        | if own_process_group {
            WINDOWS_CREATE_NEW_PROCESS_GROUP
        } else {
            0
        }
}

#[cfg(windows)]
pub(crate) fn configure_std_background(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(windows_creation_flags(false));
}

#[cfg(not(windows))]
pub(crate) fn configure_std_background(_command: &mut std::process::Command) {}

#[cfg(windows)]
pub(crate) fn configure_std_background_process_group(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(windows_creation_flags(true));
}

#[cfg(not(windows))]
pub(crate) fn configure_std_background_process_group(_command: &mut std::process::Command) {}

#[cfg(windows)]
pub(crate) fn configure_tokio_background(command: &mut tokio::process::Command) {
    command.creation_flags(windows_creation_flags(false));
}

#[cfg(not(windows))]
pub(crate) fn configure_tokio_background(_command: &mut tokio::process::Command) {}

#[cfg(windows)]
pub(crate) fn configure_tokio_background_process_group(command: &mut tokio::process::Command) {
    command.creation_flags(windows_creation_flags(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_flags_hide_windows_without_forcing_a_process_group() {
        assert_eq!(windows_creation_flags(false), WINDOWS_CREATE_NO_WINDOW);
        assert_eq!(
            windows_creation_flags(true),
            WINDOWS_CREATE_NO_WINDOW | WINDOWS_CREATE_NEW_PROCESS_GROUP
        );
    }
}
