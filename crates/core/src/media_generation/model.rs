use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

        }
    };
}

string_enum!(MediaOperation {
    TextToVideo => "text_to_video",
    ImageToVideo => "image_to_video",
    VideoToVideo => "video_to_video",
    Extend => "extend",
    Edit => "edit",
    FirstLastFrame => "first_last_frame",
    MotionTransfer => "motion_transfer",
    LipSync => "lip_sync",
    Upscale => "upscale",
    AudioGeneration => "audio_generation",
});

string_enum!(MediaObservationMode {
    Polling => "polling",
    Webhook => "webhook",
    Hybrid => "hybrid",
});

string_enum!(MediaJobState {
    Draft => "draft",
    Validating => "validating",
    UploadingAssets => "uploading_assets",
    Submitting => "submitting",
    Queued => "queued",
    Running => "running",
    PostProcessing => "post_processing",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
    Expired => "expired",
    ProviderUnknown => "provider_unknown",
});

impl MediaJobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    pub fn needs_provider_recovery(self) -> bool {
        matches!(
            self,
            Self::Submitting
                | Self::Queued
                | Self::Running
                | Self::PostProcessing
                | Self::ProviderUnknown
        )
    }

    pub(crate) fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Draft => matches!(next, Self::Validating | Self::Cancelled),
            Self::Validating => {
                matches!(next, Self::UploadingAssets | Self::Failed | Self::Cancelled)
            }
            Self::UploadingAssets => {
                matches!(next, Self::Submitting | Self::Failed | Self::Cancelled)
            }
            Self::Submitting => matches!(
                next,
                Self::Queued
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::Queued => matches!(
                next,
                Self::Submitting
                    | Self::Running
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::Running => matches!(
                next,
                Self::Submitting
                    | Self::PostProcessing
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::PostProcessing => matches!(
                next,
                Self::Completed
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::ProviderUnknown => matches!(
                next,
                Self::Queued
                    | Self::Running
                    | Self::PostProcessing
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
            ),
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired => false,
        }
    }
}

string_enum!(MediaJobAttemptState {
    Created => "created",
    Submitting => "submitting",
    Accepted => "accepted",
    Observing => "observing",
    Succeeded => "succeeded",
    Failed => "failed",
    Cancelled => "cancelled",
    Expired => "expired",
    ProviderUnknown => "provider_unknown",
});

impl MediaJobAttemptState {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    pub(crate) fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Created => matches!(
                next,
                Self::Submitting
                    | Self::Accepted
                    | Self::Observing
                    | Self::Succeeded
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::Submitting => matches!(
                next,
                Self::Accepted
                    | Self::Observing
                    | Self::Succeeded
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::Accepted => matches!(
                next,
                Self::Observing
                    | Self::Succeeded
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::Observing => matches!(
                next,
                Self::Succeeded
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
                    | Self::ProviderUnknown
            ),
            Self::ProviderUnknown => matches!(
                next,
                Self::Submitting
                    | Self::Accepted
                    | Self::Observing
                    | Self::Succeeded
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Expired
            ),
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired => false,
        }
    }
}

string_enum!(MediaAssetStorageKind {
    ManagedLocal => "managed_local",
    ProviderRemote => "provider_remote",
    External => "external",
});

string_enum!(MediaAssetLocalState {
    Available => "available",
    DeletionRequested => "deletion_requested",
    Deleted => "deleted",
});

string_enum!(MediaAssetLocalRetentionPolicy {
    RetainUntilDeleted => "retain_until_deleted",
    DeleteAfterExpiry => "delete_after_expiry",
});

string_enum!(MediaRemoteDeletionStatus {
    NotRequested => "not_requested",
    Requested => "requested",
    Confirmed => "confirmed",
    Unsupported => "unsupported",
    Failed => "failed",
});

string_enum!(MediaRecoveryAction {
    ValidateLocally => "validate_locally",
    UploadInputs => "upload_inputs",
    BeginSubmissionAttempt => "begin_submission_attempt",
    LookupByIdempotencyKey => "lookup_by_idempotency_key",
    ObserveProviderTask => "observe_provider_task",
    ResumePostProcessing => "resume_post_processing",
    ReconcileCancellation => "reconcile_cancellation",
    RequestRemoteDeletion => "request_remote_deletion",
});

string_enum!(MediaAssetRelationType {
    Input => "input",
    Output => "output",
    DerivedFrom => "derived_from",
    VariantOf => "variant_of",
    Extends => "extends",
    Edits => "edits",
    AudioTrack => "audio_track",
    Export => "export",
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMediaJobRequest {
    pub idempotency_key: String,
    pub project_id: Option<String>,
    pub conversation_id: Option<String>,
    pub provider_id: String,
    /// URI-like endpoint/account/region identity. It must not contain raw
    /// credentials and scopes provider task IDs more narrowly than provider ID.
    pub provider_source: String,
    pub model_id: String,
    pub api_version: Option<String>,
    pub operation: MediaOperation,
    /// Ordered content IDs are part of request identity. Changing their order
    /// or bytes changes the idempotency fingerprint.
    #[serde(default)]
    pub input_asset_ids: Vec<String>,
    pub raw_parameters: Value,
    pub normalized_parameters: Value,
    #[serde(default = "empty_object")]
    pub provider_extras: Value,
    pub observation_mode: MediaObservationMode,
    pub estimated_cost_micros: Option<i64>,
    pub currency: Option<String>,
    pub data_region: Option<String>,
    pub remote_retention_expires_at: Option<String>,
    #[serde(default)]
    pub allow_cross_provider_fallback: bool,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

const fn default_max_attempts() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionMediaJobRequest {
    pub job_id: String,
    pub expected_revision: u64,
    pub next_state: MediaJobState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginMediaJobAttemptRequest {
    pub job_id: String,
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub provider_id: String,
    pub provider_source: String,
    pub model_id: String,
    pub api_version: Option<String>,
    pub data_region: Option<String>,
    /// Provider-specific retention deadline captured for this exact attempt.
    /// `None` means the provider contract did not expose a deadline.
    pub remote_retention_expires_at: Option<String>,
    /// Durable proof required before creating a new attempt from
    /// `provider_unknown`. A caller assertion is deliberately insufficient.
    pub provider_unknown_reconciliation: Option<ProviderUnknownReconciliation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUnknownReconciliation {
    pub observed_at: String,
    pub lookup_source: String,
    pub lookup_idempotency_key: String,
    pub lookup_evidence: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordMediaProviderEventRequest {
    pub job_id: String,
    pub expected_revision: u64,
    pub attempt_id: String,
    pub provider_id: String,
    /// Stable provider endpoint/account/region scope. Together with
    /// `deduplication_key`, this follows the CloudEvents `(source, id)` rule.
    pub event_source: String,
    pub deduplication_key: String,
    pub event_kind: String,
    pub payload: Value,
    pub provider_created_at: Option<String>,
    pub provider_task_id: Option<String>,
    pub attempt_state: Option<MediaJobAttemptState>,
    pub next_job_state: Option<MediaJobState>,
    pub error: Option<Value>,
    pub retry_classification: Option<String>,
    pub next_eligible_at: Option<String>,
    pub cancellation_result: Option<Value>,
    pub final_cost_micros: Option<i64>,
    pub watermark_present: Option<bool>,
    pub provenance: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RegisterMediaAssetRequest {
    /// SHA-256 verified by Nexa while reading exactly `byte_length` bytes.
    /// Remote locators that have not been downloaded and verified are provider
    /// event data, not content-addressed assets.
    pub content_hash_sha256: String,
    pub content_verified_at: String,
    pub media_type: String,
    pub byte_length: u64,
    pub storage_kind: MediaAssetStorageKind,
    pub storage_key: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMediaJobCancellation {
    pub job_id: String,
    pub expected_revision: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMediaJobRemoteDeletion {
    pub job_id: String,
    pub expected_revision: u64,
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordMediaJobRemoteDeletionResult {
    pub job_id: String,
    pub expected_revision: u64,
    pub attempt_id: String,
    pub event_source: String,
    pub deduplication_key: String,
    pub status: MediaRemoteDeletionStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMediaAssetDeletion {
    pub asset_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMediaAssetOccurrenceRequest {
    pub job_id: String,
    pub expected_revision: u64,
    pub relation_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkMediaAssetRequest {
    pub job_id: String,
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub attempt_id: String,
    pub asset_id: String,
    pub parent_asset_id: Option<String>,
    pub relation_type: MediaAssetRelationType,
    #[serde(default)]
    pub ordinal: u32,
    pub local_retention_policy: MediaAssetLocalRetentionPolicy,
    pub local_retention_expires_at: Option<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobRecord {
    pub id: String,
    pub idempotency_key: String,
    pub project_id: Option<String>,
    pub conversation_id: Option<String>,
    pub provider_id: String,
    pub provider_source: String,
    pub model_id: String,
    pub api_version: Option<String>,
    pub operation: MediaOperation,
    pub input_asset_ids: Vec<String>,
    pub state: MediaJobState,
    pub revision: u64,
    pub raw_parameters: Value,
    pub normalized_parameters: Value,
    pub provider_extras: Value,
    pub observation_mode: MediaObservationMode,
    pub current_attempt_id: Option<String>,
    pub current_provider_task_id: Option<String>,
    pub retry_count: u32,
    pub max_attempts: u32,
    pub estimated_cost_micros: Option<i64>,
    pub final_cost_micros: Option<i64>,
    pub currency: Option<String>,
    pub data_region: Option<String>,
    pub remote_retention_expires_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub cancellation_reason: Option<String>,
    pub allow_cross_provider_fallback: bool,
    pub watermark_present: Option<bool>,
    pub provenance: Value,
    pub last_provider_observed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobAttemptRecord {
    pub id: String,
    pub job_id: String,
    pub attempt_number: u32,
    pub idempotency_key: String,
    pub provider_id: String,
    pub provider_source: String,
    pub model_id: String,
    pub api_version: Option<String>,
    pub data_region: Option<String>,
    pub remote_retention_expires_at: Option<String>,
    pub cross_provider_fallback_authorized: bool,
    pub provider_task_id: Option<String>,
    pub state: MediaJobAttemptState,
    pub error: Option<Value>,
    pub retry_classification: Option<String>,
    pub next_eligible_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub cancellation_result: Option<Value>,
    pub remote_deletion_requested_at: Option<String>,
    pub remote_deletion_status: MediaRemoteDeletionStatus,
    pub remote_deletion_completed_at: Option<String>,
    pub remote_deletion_error: Option<Value>,
    pub submitted_at: Option<String>,
    pub last_observed_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaAssetRecord {
    pub id: String,
    pub content_hash_sha256: String,
    pub content_verified_at: String,
    pub media_type: String,
    pub byte_length: u64,
    pub storage_kind: MediaAssetStorageKind,
    pub storage_key: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub local_state: MediaAssetLocalState,
    pub local_deletion_requested_at: Option<String>,
    pub local_deletion_completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaAssetRelationRecord {
    pub id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub asset_id: String,
    pub parent_asset_id: Option<String>,
    pub relation_type: MediaAssetRelationType,
    pub ordinal: u32,
    pub local_retention_policy: MediaAssetLocalRetentionPolicy,
    pub local_retention_expires_at: Option<String>,
    pub metadata: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProviderEventRecord {
    pub id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub sequence: u64,
    pub provider_id: String,
    pub event_source: String,
    pub deduplication_key: String,
    pub event_kind: String,
    pub payload: Value,
    pub provider_created_at: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobSnapshot {
    pub job: MediaJobRecord,
    pub attempts: Vec<MediaJobAttemptRecord>,
    pub assets: Vec<MediaAssetRecord>,
    pub asset_relations: Vec<MediaAssetRelationRecord>,
    pub provider_event_count: u64,
    pub provider_events: Vec<MediaProviderEventRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRecoveryPlanItem {
    pub job_id: String,
    pub attempt_id: Option<String>,
    pub revision: u64,
    pub action: MediaRecoveryAction,
    pub provider_source: String,
    pub provider_task_id: Option<String>,
    pub cancellation_requested: bool,
}
