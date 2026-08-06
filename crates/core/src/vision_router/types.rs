use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::settings_schema_v2::{CapabilityBindingConstraintsV2, CapabilityFallbackModeV2};

pub const VISION_OBSERVATION_SCHEMA_VERSION: u16 = 1;
pub const VISION_CLASSIFIER_VERSION: u16 = 1;
pub const DEFAULT_VISION_CACHE_RETENTION_DAYS: u16 = 30;

const MAX_SUMMARY_CHARS: usize = 16_000;
const MAX_OCR_TEXT_CHARS: usize = 64_000;
const MAX_REGION_COUNT: usize = 512;
const MAX_TABLE_COUNT: usize = 32;
const MAX_ENTITY_COUNT: usize = 256;
const MAX_CHART_COUNT: usize = 32;
const MAX_TABLE_ROWS: usize = 256;
const MAX_TABLE_COLUMNS: usize = 64;
const MAX_CELL_CHARS: usize = 4_096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionMode {
    Off,
    Ask,
    #[default]
    Auto,
    AlwaysAuxiliary,
}

impl VisionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::AlwaysAuxiliary => "always_auxiliary",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "always_auxiliary" => Some(Self::AlwaysAuxiliary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionTurnOverride {
    Auto,
    OcrOnly,
    VisionOnly,
}

impl VisionTurnOverride {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OcrOnly => "ocr_only",
            Self::VisionOnly => "vision_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionRouterPolicy {
    pub mode: VisionMode,
    pub prefer_local_processing: bool,
    pub local_only: bool,
    pub cache_enabled: bool,
    pub cache_retention_days: u16,
}

impl Default for VisionRouterPolicy {
    fn default() -> Self {
        Self {
            mode: VisionMode::Auto,
            prefer_local_processing: true,
            local_only: false,
            cache_enabled: true,
            cache_retention_days: DEFAULT_VISION_CACHE_RETENTION_DAYS,
        }
    }
}

impl VisionRouterPolicy {
    pub fn from_binding_options(options: &BTreeMap<String, Value>) -> Result<Self, CoreError> {
        let mut policy = Self::default();
        if let Some(value) = options.get("mode") {
            let mode = value.as_str().ok_or_else(|| {
                CoreError::InvalidInput("Vision option mode must be a string".to_string())
            })?;
            policy.mode = VisionMode::parse(mode)
                .ok_or_else(|| CoreError::InvalidInput(format!("Unknown Vision mode {mode}")))?;
        }
        if let Some(value) = options.get("preferLocalProcessing") {
            policy.prefer_local_processing = value.as_bool().ok_or_else(|| {
                CoreError::InvalidInput(
                    "Vision option preferLocalProcessing must be boolean".to_string(),
                )
            })?;
        }
        if let Some(value) = options.get("localOnly") {
            policy.local_only = value.as_bool().ok_or_else(|| {
                CoreError::InvalidInput("Vision option localOnly must be boolean".to_string())
            })?;
        }
        if let Some(value) = options.get("cacheEnabled") {
            policy.cache_enabled = value.as_bool().ok_or_else(|| {
                CoreError::InvalidInput("Vision option cacheEnabled must be boolean".to_string())
            })?;
        }
        if let Some(value) = options.get("cacheRetentionDays") {
            let days = value.as_u64().ok_or_else(|| {
                CoreError::InvalidInput(
                    "Vision option cacheRetentionDays must be an integer".to_string(),
                )
            })?;
            if !(1..=3650).contains(&days) {
                return Err(CoreError::InvalidInput(
                    "Vision cacheRetentionDays must be between 1 and 3650".to_string(),
                ));
            }
            policy.cache_retention_days = days as u16;
        }
        if policy.local_only {
            policy.prefer_local_processing = true;
        }
        Ok(policy)
    }

    pub fn to_binding_options(&self) -> BTreeMap<String, Value> {
        BTreeMap::from([
            (
                "mode".to_string(),
                Value::String(self.mode.as_str().to_string()),
            ),
            (
                "preferLocalProcessing".to_string(),
                Value::Bool(self.prefer_local_processing),
            ),
            ("localOnly".to_string(), Value::Bool(self.local_only)),
            ("cacheEnabled".to_string(), Value::Bool(self.cache_enabled)),
            (
                "cacheRetentionDays".to_string(),
                Value::Number(self.cache_retention_days.into()),
            ),
            (
                "selectionSource".to_string(),
                Value::String("explicit_user".to_string()),
            ),
        ])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionIntent {
    DenseText,
    VisualReasoning,
    Mixed,
    Unknown,
}

impl VisionIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DenseText => "dense_text",
            Self::VisualReasoning => "visual_reasoning",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionRoutePlan {
    MetadataOnly,
    NativeDirect,
    OcrOnly,
    VisionOnly,
    OcrThenVision,
    VisionThenOcr,
}

impl VisionRoutePlan {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetadataOnly => "metadata_only",
            Self::NativeDirect => "native_direct",
            Self::OcrOnly => "ocr_only",
            Self::VisionOnly => "vision_only",
            Self::OcrThenVision => "ocr_then_vision",
            Self::VisionThenOcr => "vision_then_ocr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionRouteDecision {
    pub intent: VisionIntent,
    pub plan: VisionRoutePlan,
    pub classification_confidence: f32,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionPrivacyScope {
    Local,
    SingleProvider,
    MultiProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionConfidenceKind {
    OcrRecognitionMean,
    ProviderReported,
    RouteClassification,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisionRegion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub bbox: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtractedTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtractedEntity {
    pub kind: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChartSeriesObservation {
    pub name: String,
    #[serde(default)]
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChartObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_axis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_axis: Option<String>,
    #[serde(default)]
    pub series: Vec<ChartSeriesObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionObservationSourceKind {
    LocalOcr,
    VisionModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionObservationSource {
    pub kind: VisionObservationSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_index: Option<usize>,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionAttemptStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionRouteAttempt {
    pub processor: String,
    pub status: VisionAttemptStatus,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionRouteTrace {
    pub classifier_version: u16,
    pub intent: VisionIntent,
    pub plan: VisionRoutePlan,
    pub classification_confidence: f32,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<VisionRouteAttempt>,
}

impl From<VisionRouteDecision> for VisionRouteTrace {
    fn from(value: VisionRouteDecision) -> Self {
        Self {
            classifier_version: VISION_CLASSIFIER_VERSION,
            intent: value.intent,
            plan: value.plan,
            classification_confidence: value.classification_confidence,
            reason_codes: value.reason_codes,
            attempts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionObservationV1 {
    pub schema_version: u16,
    pub attachment_id: String,
    pub attachment_hash: String,
    pub profile_hash: String,
    pub intent: VisionIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(default)]
    pub regions: Vec<VisionRegion>,
    #[serde(default)]
    pub tables: Vec<ExtractedTable>,
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub chart_data: Vec<ChartObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_kind: Option<VisionConfidenceKind>,
    #[serde(default)]
    pub sources: Vec<VisionObservationSource>,
    pub fallback_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub privacy_scope: VisionPrivacyScope,
    pub route: VisionRouteTrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionAttachmentStatus {
    Pending,
    Cached,
    Observed,
    MetadataOnly,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionAttachmentAnalysis {
    pub status: VisionAttachmentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<VisionObservationV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl VisionObservationV1 {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.schema_version != VISION_OBSERVATION_SCHEMA_VERSION {
            return Err(invalid_observation("unsupported schema version"));
        }
        if self.attachment_id.trim().is_empty()
            || !is_hex_hash(&self.attachment_hash, 64)
            || !is_hex_hash(&self.profile_hash, 64)
        {
            return Err(invalid_observation(
                "invalid attachment or profile identity",
            ));
        }
        validate_optional_string(self.summary.as_deref(), MAX_SUMMARY_CHARS, "summary")?;
        validate_optional_string(self.ocr_text.as_deref(), MAX_OCR_TEXT_CHARS, "ocrText")?;
        if self.regions.len() > MAX_REGION_COUNT
            || self.tables.len() > MAX_TABLE_COUNT
            || self.entities.len() > MAX_ENTITY_COUNT
            || self.chart_data.len() > MAX_CHART_COUNT
        {
            return Err(invalid_observation("collection limit exceeded"));
        }
        validate_confidence(self.confidence, "confidence")?;
        if self.confidence.is_some() != self.confidence_kind.is_some() {
            return Err(invalid_observation(
                "confidence and confidenceKind must be provided together",
            ));
        }
        for region in &self.regions {
            if region.bbox.iter().any(|value| !value.is_finite())
                || region.bbox.iter().any(|value| !(0.0..=1.0).contains(value))
                || region.bbox[0] + region.bbox[2] > 1.0001
                || region.bbox[1] + region.bbox[3] > 1.0001
            {
                return Err(invalid_observation(
                    "region bbox is outside normalized bounds",
                ));
            }
            validate_confidence(region.confidence, "region confidence")?;
            validate_optional_string(region.kind.as_deref(), 128, "region kind")?;
            validate_optional_string(region.text.as_deref(), MAX_CELL_CHARS, "region text")?;
        }
        for table in &self.tables {
            if table.rows.len() > MAX_TABLE_ROWS || table.headers.len() > MAX_TABLE_COLUMNS {
                return Err(invalid_observation("table limit exceeded"));
            }
            validate_optional_string(table.title.as_deref(), 512, "table title")?;
            for cell in table.headers.iter().chain(table.rows.iter().flatten()) {
                if cell.chars().count() > MAX_CELL_CHARS {
                    return Err(invalid_observation("table cell limit exceeded"));
                }
            }
            if table.rows.iter().any(|row| row.len() > MAX_TABLE_COLUMNS) {
                return Err(invalid_observation("table column limit exceeded"));
            }
        }
        for entity in &self.entities {
            if entity.kind.trim().is_empty()
                || entity.kind.chars().count() > 128
                || entity.value.chars().count() > MAX_CELL_CHARS
                || entity
                    .region_index
                    .is_some_and(|index| index >= self.regions.len())
            {
                return Err(invalid_observation("invalid entity"));
            }
        }
        for chart in &self.chart_data {
            if chart.series.len() > 64
                || chart
                    .series
                    .iter()
                    .any(|series| series.name.chars().count() > 512 || series.values.len() > 1_024)
            {
                return Err(invalid_observation("chart limit exceeded"));
            }
        }
        if self.sources.is_empty() {
            return Err(invalid_observation("missing observation source"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionOcrProfile {
    pub enabled: bool,
    pub confidence_threshold_millis: u16,
    pub det_limit_side_len: u32,
    pub use_cls: bool,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionTargetProfile {
    pub binding_revision: u64,
    pub target_id: String,
    pub target_revision: u64,
    pub connection_id: String,
    pub connection_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionProfileV1 {
    pub observation_schema_version: u16,
    pub classifier_version: u16,
    pub intent: VisionIntent,
    pub mode: VisionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_override: Option<VisionTurnOverride>,
    pub prefer_local_processing: bool,
    pub local_only: bool,
    pub primary_egress_id: String,
    pub primary_is_local: bool,
    pub fallback_mode: CapabilityFallbackModeV2,
    pub constraints: CapabilityBindingConstraintsV2,
    pub ocr: VisionOcrProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<VisionTargetProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_targets: Vec<VisionTargetProfile>,
}

impl VisionProfileV1 {
    pub fn profile_hash(&self) -> Result<String, CoreError> {
        let bytes = serde_json::to_vec(self)?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }
}

pub fn attachment_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn invalid_observation(message: &str) -> CoreError {
    CoreError::InvalidInput(format!("invalid_observation: {message}"))
}

fn validate_optional_string(
    value: Option<&str>,
    max_chars: usize,
    field: &str,
) -> Result<(), CoreError> {
    if value.is_some_and(|value| value.chars().count() > max_chars) {
        return Err(invalid_observation(&format!("{field} limit exceeded")));
    }
    Ok(())
}

fn validate_confidence(value: Option<f32>, field: &str) -> Result<(), CoreError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(invalid_observation(&format!("{field} is outside 0..=1")));
    }
    Ok(())
}

fn is_hex_hash(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_options_are_validated_and_local_only_implies_preference() {
        let options = BTreeMap::from([
            ("mode".into(), Value::String("always_auxiliary".into())),
            ("preferLocalProcessing".into(), Value::Bool(false)),
            ("localOnly".into(), Value::Bool(true)),
            ("cacheEnabled".into(), Value::Bool(false)),
            ("cacheRetentionDays".into(), Value::Number(7.into())),
        ]);
        let policy = VisionRouterPolicy::from_binding_options(&options).unwrap();
        assert_eq!(policy.mode, VisionMode::AlwaysAuxiliary);
        assert!(policy.prefer_local_processing);
        assert!(policy.local_only);
        assert!(!policy.cache_enabled);
        assert_eq!(policy.cache_retention_days, 7);
    }

    #[test]
    fn profile_hash_is_stable_and_revision_sensitive() {
        let mut profile = VisionProfileV1 {
            observation_schema_version: VISION_OBSERVATION_SCHEMA_VERSION,
            classifier_version: VISION_CLASSIFIER_VERSION,
            intent: VisionIntent::Mixed,
            mode: VisionMode::Auto,
            turn_override: None,
            prefer_local_processing: true,
            local_only: false,
            primary_egress_id: "registry:primary".into(),
            primary_is_local: false,
            fallback_mode: CapabilityFallbackModeV2::Automatic,
            constraints: CapabilityBindingConstraintsV2 {
                require_same_connection: false,
                allow_cross_provider: true,
                ..CapabilityBindingConstraintsV2::default()
            },
            ocr: VisionOcrProfile {
                enabled: true,
                confidence_threshold_millis: 600,
                det_limit_side_len: 960,
                use_cls: true,
                languages: vec!["en".into(), "zh".into()],
            },
            target: Some(VisionTargetProfile {
                binding_revision: 1,
                target_id: "target:one".into(),
                target_revision: 2,
                connection_id: "connection:one".into(),
                connection_revision: 3,
                descriptor_hash: Some("descriptor".into()),
            }),
            fallback_targets: vec![VisionTargetProfile {
                binding_revision: 1,
                target_id: "target:two".into(),
                target_revision: 4,
                connection_id: "connection:two".into(),
                connection_revision: 5,
                descriptor_hash: Some("fallback-descriptor".into()),
            }],
        };
        let first = profile.profile_hash().unwrap();
        assert_eq!(first, profile.profile_hash().unwrap());
        profile.target.as_mut().unwrap().target_revision += 1;
        assert_ne!(first, profile.profile_hash().unwrap());
        let target_revision_hash = profile.profile_hash().unwrap();
        profile.fallback_targets[0].target_revision += 1;
        assert_ne!(target_revision_hash, profile.profile_hash().unwrap());
        let fallback_hash = profile.profile_hash().unwrap();
        profile.constraints.allow_cross_provider = false;
        assert_ne!(fallback_hash, profile.profile_hash().unwrap());
    }

    #[test]
    fn observation_rejects_unbounded_or_fabricated_values() {
        let mut observation = VisionObservationV1 {
            schema_version: VISION_OBSERVATION_SCHEMA_VERSION,
            attachment_id: "attachment".into(),
            attachment_hash: "a".repeat(64),
            profile_hash: "b".repeat(64),
            intent: VisionIntent::DenseText,
            summary: None,
            ocr_text: Some("text".into()),
            regions: vec![VisionRegion {
                kind: Some("text".into()),
                text: Some("text".into()),
                bbox: [0.0, 0.0, 1.0, 1.0],
                confidence: Some(0.9),
            }],
            tables: vec![],
            entities: vec![],
            chart_data: vec![],
            confidence: Some(0.9),
            confidence_kind: Some(VisionConfidenceKind::OcrRecognitionMean),
            sources: vec![VisionObservationSource {
                kind: VisionObservationSourceKind::LocalOcr,
                provider_id: None,
                model_id: None,
                target_id: None,
                target_revision: None,
                fallback_index: None,
                local: true,
            }],
            fallback_used: false,
            fallback_reason: None,
            privacy_scope: VisionPrivacyScope::Local,
            route: VisionRouteTrace {
                classifier_version: VISION_CLASSIFIER_VERSION,
                intent: VisionIntent::DenseText,
                plan: VisionRoutePlan::OcrOnly,
                classification_confidence: 0.9,
                reason_codes: vec![],
                attempts: vec![],
            },
        };
        observation.validate().unwrap();
        observation.regions[0].bbox = [0.8, 0.0, 0.3, 1.0];
        assert!(observation.validate().is_err());
    }
}
