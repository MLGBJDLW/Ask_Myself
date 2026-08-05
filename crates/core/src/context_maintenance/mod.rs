//! Durable, cancellable context-maintenance operations.
//!
//! The external seam is intentionally small: start, observe, cancel, and load
//! the active context projection. Provider transport, planning, fallback,
//! persistence, leases, and terminal-event rules stay behind it.

mod model;
mod planner;
mod service;
mod store;

pub use model::{
    ContextCompactionHandle, ContextCompactionJob, ContextCompactionPhase, ContextCompactionPolicy,
    ContextCompactionResult, ContextProjection, StartContextCompactionRequest,
};
pub use service::ContextCompactionService;
pub use store::load_context_projection;
