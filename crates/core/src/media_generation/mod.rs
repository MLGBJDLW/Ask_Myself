//! Durable media-generation jobs and content-addressed asset lineage.
//!
//! The external seam persists provider-neutral jobs, attempts, observations,
//! and asset relationships. Provider transports and workflow UI remain
//! adapters outside this module; state-machine, idempotency, optimistic
//! concurrency, and transaction rules stay behind the runtime interface.

mod asset_store;
mod model;
mod runtime;
mod store;

pub use asset_store::{ImportMediaAssetRequest, MediaGenerationAssetStore};
pub use model::{
    BeginMediaJobAttemptRequest, CreateMediaJobRequest, DeleteMediaAssetOccurrenceRequest,
    LinkMediaAssetRequest, MediaAssetLocalRetentionPolicy, MediaAssetLocalState, MediaAssetRecord,
    MediaAssetRelationRecord, MediaAssetRelationType, MediaAssetStorageKind, MediaJobAttemptRecord,
    MediaJobAttemptState, MediaJobRecord, MediaJobSnapshot, MediaJobState, MediaObservationMode,
    MediaOperation, MediaProviderEventRecord, MediaRecoveryAction, MediaRecoveryPlanItem,
    MediaRemoteDeletionStatus, ProviderUnknownReconciliation, RecordMediaJobRemoteDeletionResult,
    RecordMediaProviderEventRequest, RequestMediaAssetDeletion, RequestMediaJobCancellation,
    RequestMediaJobRemoteDeletion, TransitionMediaJobRequest,
};
pub use runtime::MediaGenerationRuntime;
