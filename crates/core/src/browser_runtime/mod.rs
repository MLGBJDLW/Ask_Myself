//! Shared browser-domain types and safety primitives.
//!
//! The desktop Browser Workspace owns its native WebView lifecycle directly.
//! Keep only contracts with multiple real consumers here.

mod control_lease;
mod native_pointer;
mod policy;
mod types;

pub use control_lease::ControlLease;
pub use native_pointer::{
    acquire_desktop_input_permit, desktop_input_arbiter, move_native_pointer,
    try_acquire_cross_process_input, CrossProcessDesktopInputGuard,
};
pub use policy::{classify_action_risk, BrowserActionRisk};
pub use types::*;
