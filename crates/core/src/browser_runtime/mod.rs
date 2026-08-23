//! Engine-neutral contract for Nexa's shared user/Agent browser workspace.
//!
//! Desktop backends own their native browser surfaces, while tools and UI
//! clients share these lifecycle, observation, action, and control semantics.

mod control_lease;
mod events;
mod native_pointer;
mod policy;
mod runtime;
mod types;

pub use control_lease::ControlLease;
pub use events::{BrowserRuntimeEvent, BrowserRuntimeEventKind};
pub use native_pointer::{
    acquire_desktop_input_permit, desktop_input_arbiter, move_native_pointer,
    try_acquire_cross_process_input, CrossProcessDesktopInputGuard,
};
pub use policy::{classify_action_risk, BrowserActionRisk};
pub use runtime::{BrowserRuntime, BrowserRuntimeError, BrowserRuntimeResult};
pub use types::*;
