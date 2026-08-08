use serde::{Deserialize, Serialize};

pub const MODEL_DESCRIPTOR_SCHEMA_VERSION: u16 = 2;

const fn descriptor_schema_version() -> u16 {
    MODEL_DESCRIPTOR_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelCatalogSource {
    Official,
    Discovered,
    #[default]
    Curated,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycle {
    #[default]
    Active,
    Preview,
    Gated,
    Legacy,
    Deprecated,
    Removed,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelAccess {
    #[default]
    Public,
    AccountEnablement,
    Application,
    PrivatePreview,
}

#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ProductReadiness {
    #[default]
    Known,
    Discoverable,
    Callable,
    ProductReady,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelModality {
    Text,
    Image,
    Audio,
    Video,
    File,
    Embedding,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetCapability {
    pub enabled: bool,
    #[serde(default)]
    pub default_tokens: Option<u32>,
    #[serde(default)]
    pub min_tokens: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub step: Option<u32>,
    #[serde(default)]
    pub allow_zero: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningCapability {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub effort_levels: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub effort_budget_exclusive: bool,
    #[serde(default)]
    pub thinking_budget: Option<ThinkingBudgetCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub audio_input: bool,
    #[serde(default)]
    pub audio_output: bool,
    #[serde(default)]
    pub video_input: bool,
    #[serde(default)]
    pub video_output: bool,
    #[serde(default)]
    pub tool_calling: bool,
    #[serde(default)]
    pub parallel_tool_calling: bool,
    #[serde(default)]
    pub structured_output: bool,
    #[serde(default)]
    pub image_generation: bool,
    #[serde(default)]
    pub image_editing: bool,
    #[serde(default)]
    pub multi_reference_editing: bool,
    #[serde(default)]
    pub realtime: bool,
    #[serde(default)]
    pub prompt_cache: bool,
    #[serde(default)]
    pub async_jobs: bool,
    #[serde(default)]
    pub batch: bool,
    #[serde(default)]
    pub dimension_override: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_web_search: Option<crate::model_catalog::NativeWebSearchCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub max_images: Option<u32>,
    #[serde(default)]
    pub max_input_bytes: Option<u64>,
    #[serde(default)]
    pub max_video_seconds: Option<u64>,
    #[serde(default)]
    pub max_audio_seconds: Option<u64>,
    #[serde(default)]
    pub embedding_dimensions: Option<usize>,
    #[serde(default)]
    pub supported_sizes: Vec<String>,
    #[serde(default)]
    pub output_formats: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelDescriptor {
    #[serde(default = "descriptor_schema_version")]
    pub schema_version: u16,
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub display_name: String,
    pub provider_id: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub lifecycle: ModelLifecycle,
    #[serde(default)]
    pub access: ModelAccess,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub endpoint_ids: Vec<String>,
    #[serde(default)]
    pub endpoint_kinds: Vec<String>,
    #[serde(default)]
    pub input_modalities: Vec<ModelModality>,
    #[serde(default)]
    pub output_modalities: Vec<ModelModality>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub limits: ModelLimits,
    #[serde(default)]
    pub pricing_ref: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub deprecation_date: Option<String>,
    #[serde(default)]
    pub replacement_model_id: Option<String>,
    #[serde(default)]
    pub source: ModelCatalogSource,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub product_readiness: ProductReadiness,
    #[serde(default)]
    pub available_to_credential: Option<bool>,
    /// Curated ordering hint. Eligibility rules always take precedence.
    #[serde(default)]
    pub recommended: bool,
}

impl ModelDescriptor {
    pub fn new(
        id: impl Into<String>,
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        let id = id.into();
        Self {
            schema_version: MODEL_DESCRIPTOR_SCHEMA_VERSION,
            family: id.clone(),
            id,
            aliases: Vec::new(),
            display_name: display_name.into(),
            provider_id: provider_id.into(),
            version: None,
            lifecycle: ModelLifecycle::Active,
            access: ModelAccess::Public,
            regions: Vec::new(),
            endpoint_ids: Vec::new(),
            endpoint_kinds: Vec::new(),
            input_modalities: vec![ModelModality::Text],
            output_modalities: vec![ModelModality::Text],
            capabilities: ModelCapabilities::default(),
            limits: ModelLimits::default(),
            pricing_ref: None,
            release_date: None,
            deprecation_date: None,
            replacement_model_id: None,
            source: ModelCatalogSource::Curated,
            last_verified_at: None,
            product_readiness: ProductReadiness::Known,
            available_to_credential: None,
            recommended: false,
        }
    }

    pub fn is_implicit_default_eligible(&self) -> bool {
        self.lifecycle == ModelLifecycle::Active
            && self.access == ModelAccess::Public
            && self.product_readiness == ProductReadiness::ProductReady
            && self.available_to_credential != Some(false)
    }

    pub fn is_explicitly_selectable(&self) -> bool {
        self.lifecycle != ModelLifecycle::Removed && self.available_to_credential != Some(false)
    }

    pub(crate) fn matches_id_or_alias(&self, value: &str) -> bool {
        let value = normalize_id(value);
        normalize_id(&self.id) == value
            || self
                .aliases
                .iter()
                .any(|candidate| normalize_id(candidate) == value)
    }
}

pub(crate) fn normalize_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}
