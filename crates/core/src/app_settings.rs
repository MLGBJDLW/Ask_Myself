use crate::db::Database;
use crate::error::CoreError;
use crate::web_search::{WebSearchProviderProfile, WebSearchReranker};
use rusqlite::params;
use serde::{Deserialize, Serialize};

const APP_CONFIG_KEY: &str = "app_config";
const WIZARD_STATE_KEY: &str = "wizard_state";
const CURRENT_TIMEOUT_DEFAULTS_VERSION: u32 = 1;
const CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION: u32 = 3;
const LEGACY_DEFAULT_TOOL_TIMEOUT_SECS: i64 = 30;
const LEGACY_DEFAULT_AGENT_TIMEOUT_SECS: i64 = 180;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationConfig {
    #[serde(default = "default_image_provider")]
    pub provider: String,
    #[serde(default = "default_image_api_style")]
    pub api_style: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_image_base_url_option")]
    pub base_url: Option<String>,
    #[serde(default = "default_image_model")]
    pub model: String,
    #[serde(default = "default_image_size_option")]
    pub size: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default = "default_image_output_format_option")]
    pub output_format: Option<String>,
}

impl Default for ImageGenerationConfig {
    fn default() -> Self {
        Self {
            provider: default_image_provider(),
            api_style: default_image_api_style(),
            api_key: String::new(),
            base_url: default_image_base_url_option(),
            model: default_image_model(),
            size: default_image_size_option(),
            quality: None,
            output_format: default_image_output_format_option(),
        }
    }
}

impl ImageGenerationConfig {
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.model.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextToSpeechConfig {
    #[serde(default = "default_tts_provider")]
    pub provider: String,
    #[serde(default = "default_tts_api_style")]
    pub api_style: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_tts_base_url_option")]
    pub base_url: Option<String>,
    #[serde(default = "default_tts_model")]
    pub model: String,
    #[serde(default = "default_tts_voice")]
    pub voice: String,
    #[serde(default = "default_tts_output_format")]
    pub output_format: String,
    #[serde(default = "default_tts_speed")]
    pub speed: f32,
}

impl Default for TextToSpeechConfig {
    fn default() -> Self {
        Self {
            provider: default_tts_provider(),
            api_style: default_tts_api_style(),
            api_key: String::new(),
            base_url: default_tts_base_url_option(),
            model: default_tts_model(),
            voice: default_tts_voice(),
            output_format: default_tts_output_format(),
            speed: default_tts_speed(),
        }
    }
}

impl TextToSpeechConfig {
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.voice.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    #[serde(default)]
    pub provider_profile: WebSearchProviderProfile,
    #[serde(default)]
    pub reranker: WebSearchReranker,
    #[serde(default)]
    pub provider_mode: WebSearchProviderMode,
    #[serde(default = "default_web_search_custom_providers")]
    pub custom_providers: Vec<WebSearchCustomProviderConfig>,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::Auto,
            provider_mode: WebSearchProviderMode::BuiltInFirst,
            custom_providers: default_web_search_custom_providers(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamingConfig {
    /// Enables background consolidation triggers. Manual runs remain available.
    #[serde(default = "default_dreaming_enabled")]
    pub enabled: bool,
    /// Queue a consolidation pass when the app is idle for the configured interval.
    #[serde(default)]
    pub idle: bool,
    /// Queue a consolidation pass after scan/compile completes.
    #[serde(default)]
    pub after_scan: bool,
    /// Queue a consolidation pass after a successful agent turn.
    #[serde(default)]
    pub after_successful_turn: bool,
    /// Queue a scheduled consolidation pass at a fixed interval.
    #[serde(default)]
    pub schedule: bool,
    /// Minimum minutes between idle-triggered consolidation runs.
    #[serde(default = "default_dreaming_idle_interval_minutes")]
    pub idle_interval_minutes: usize,
    /// Minimum minutes between schedule-triggered consolidation runs.
    #[serde(default = "default_dreaming_schedule_interval_minutes")]
    pub schedule_interval_minutes: usize,
    /// Soft cap for created review artifacts per run.
    #[serde(default = "default_dreaming_max_artifacts_per_run")]
    pub max_artifacts_per_run: usize,
    /// Maximum background consolidation runs allowed per day. Manual runs are not blocked.
    #[serde(default = "default_dreaming_max_runs_per_day")]
    pub max_runs_per_day: usize,
    /// Keep dreaming consolidation local-first. Reserved for future LLM-backed planners.
    #[serde(default = "default_dreaming_local_only")]
    pub local_only: bool,
    /// Optional source opt-in list. Empty means all sources are eligible.
    #[serde(default)]
    pub source_ids: Vec<String>,
    /// Optional project opt-in list. Empty means all projects are eligible.
    #[serde(default)]
    pub project_ids: Vec<String>,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: default_dreaming_enabled(),
            idle: false,
            after_scan: false,
            after_successful_turn: false,
            schedule: false,
            idle_interval_minutes: default_dreaming_idle_interval_minutes(),
            schedule_interval_minutes: default_dreaming_schedule_interval_minutes(),
            max_artifacts_per_run: default_dreaming_max_artifacts_per_run(),
            max_runs_per_day: default_dreaming_max_runs_per_day(),
            local_only: default_dreaming_local_only(),
            source_ids: Vec::new(),
            project_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderMode {
    #[default]
    BuiltInFirst,
    CustomFirst,
    CustomOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchCustomProviderPreset {
    Brave,
    Tavily,
    #[serde(rename = "anysearch", alias = "any_search")]
    AnySearch,
    #[serde(rename = "serpapi_google", alias = "serp_api_google")]
    SerpApiGoogle,
    Searxng,
}

impl WebSearchCustomProviderPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::AnySearch => "anysearch",
            Self::SerpApiGoogle => "serpapi_google",
            Self::Searxng => "searxng",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Brave => "Brave Search API",
            Self::Tavily => "Tavily Search",
            Self::AnySearch => "AnySearch",
            Self::SerpApiGoogle => "SerpAPI Google",
            Self::Searxng => "SearXNG",
        }
    }

    pub fn default_base_url(self) -> Option<String> {
        match self {
            Self::Brave => Some("https://api.search.brave.com/res/v1/web/search".to_string()),
            Self::Tavily => Some("https://api.tavily.com/search".to_string()),
            Self::AnySearch => Some("https://api.anysearch.com/v1/search".to_string()),
            Self::SerpApiGoogle => Some("https://serpapi.com/search.json".to_string()),
            Self::Searxng => None,
        }
    }

    pub fn requires_api_key(self) -> bool {
        !matches!(self, Self::AnySearch | Self::Searxng)
    }

    pub fn requires_base_url(self) -> bool {
        matches!(self, Self::Searxng)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchCustomProviderConfig {
    pub id: String,
    pub preset: WebSearchCustomProviderPreset,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub priority: u32,
}

impl WebSearchCustomProviderConfig {
    pub fn is_configured(&self) -> bool {
        (!self.preset.requires_api_key() || !self.api_key.trim().is_empty())
            && (!self.preset.requires_base_url()
                || self
                    .base_url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()))
    }

    pub fn effective_base_url(&self) -> Option<String> {
        self.base_url
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| self.preset.default_base_url())
    }
}

fn default_true() -> bool {
    true
}

fn default_web_search_custom_providers() -> Vec<WebSearchCustomProviderConfig> {
    [
        (WebSearchCustomProviderPreset::Brave, 10u32),
        (WebSearchCustomProviderPreset::Tavily, 20u32),
        (WebSearchCustomProviderPreset::AnySearch, 25u32),
        (WebSearchCustomProviderPreset::SerpApiGoogle, 30u32),
        (WebSearchCustomProviderPreset::Searxng, 40u32),
    ]
    .into_iter()
    .map(|(preset, priority)| WebSearchCustomProviderConfig {
        id: preset.as_str().to_string(),
        preset,
        name: preset.display_name().to_string(),
        enabled: false,
        api_key: String::new(),
        base_url: preset.default_base_url(),
        priority,
    })
    .collect()
}

/// First-run setup wizard state. Persisted in the `app_config` table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WizardState {
    /// Whether the user has completed (or explicitly finished) the wizard.
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShellAccessMode {
    #[default]
    Restricted,
    ConfirmAll,
    Open,
}

impl ShellAccessMode {
    pub fn requires_confirmation(self) -> bool {
        matches!(self, Self::ConfirmAll)
    }

    pub fn is_restricted(self) -> bool {
        matches!(self, Self::Restricted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Legacy UI setting retained for config compatibility.
    /// Runtime now ignores this; tool calls use local tool-specific guards.
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: i64,
    /// Legacy UI setting retained for config compatibility.
    /// Runtime now ignores this; agent turns are not capped by an outer wall-clock timeout.
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_secs: i64,
    /// Internal migration marker for timeout defaults. Missing means legacy defaults.
    #[serde(default)]
    pub timeout_defaults_version: u32,

    /// Answer cache TTL in hours. 0 = disabled. Default: 24
    #[serde(default = "default_cache_ttl_hours")]
    pub cache_ttl_hours: u32,

    /// Default search result limit. Default: 20
    #[serde(default = "default_search_limit")]
    pub default_search_limit: usize,

    /// Minimum vector similarity threshold for search. Default: 0.2
    #[serde(default = "default_min_search_similarity")]
    pub min_search_similarity: f32,

    /// Maximum file size for text ingestion in bytes. Default: 100 MB
    #[serde(default = "default_max_text_file_size")]
    pub max_text_file_size: u64,

    /// Maximum file size for video ingestion in bytes. Default: 2 GB
    #[serde(default = "default_max_video_file_size")]
    pub max_video_file_size: u64,

    /// Maximum file size for audio ingestion in bytes. Default: 500 MB
    #[serde(default = "default_max_audio_file_size")]
    pub max_audio_file_size: u64,

    /// Legacy UI setting retained for config compatibility.
    /// Runtime now ignores this; providers use internal request-start and stream-idle guards.
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,

    /// Legacy UI setting retained for config compatibility.
    /// Runtime now ignores this; MCP calls use an internal 300s request watchdog.
    #[serde(default = "default_mcp_call_timeout_secs")]
    pub mcp_call_timeout_secs: u64,

    /// Whether to send only context-selected tools to the main agent. Default: false.
    #[serde(default = "default_dynamic_tool_visibility")]
    pub dynamic_tool_visibility: bool,

    /// Version marker for agent tool-visibility defaults. Older configs used
    /// dynamic visibility by default, which hurts prompt-cache stability.
    #[serde(default)]
    pub tool_visibility_defaults_version: u32,

    /// Whether to collect detailed agent traces. Default: true.
    #[serde(default = "default_trace_enabled")]
    pub trace_enabled: bool,

    /// Whether destructive tool calls require user confirmation. Default: false
    #[serde(default)]
    pub confirm_destructive: bool,

    /// Shell command access mode for run_shell. Default: restricted.
    #[serde(default)]
    pub shell_access_mode: ShellAccessMode,

    /// Global tool-approval mode. Default: `Ask` (per-call GUI dialog for
    /// high-risk tools). `AllowAll` bypasses the gate entirely; `DenyAll`
    /// rejects every gated call without prompting.
    #[serde(default)]
    pub tool_approval_mode: crate::approval::ToolApprovalMode,

    /// Whether to automatically extract memories from conversations. Default: true
    #[serde(default = "default_auto_memory_extraction")]
    pub auto_memory_extraction: bool,

    /// Whether successful complex turns should create pending skill proposals.
    /// Default: true. Proposals remain reviewed drafts until applied.
    #[serde(default = "default_auto_skill_learning")]
    pub auto_skill_learning: bool,

    /// HuggingFace mirror base URL used as fallback when `huggingface.co` is blocked.
    /// Empty string disables the fallback. Default: `https://hf-mirror.com`.
    #[serde(default = "default_hf_mirror_base_url")]
    pub hf_mirror_base_url: String,

    /// GitHub reverse-proxy base URL used for FFmpeg binary downloads.
    /// Empty string disables the fallback. Default: `https://mirror.ghproxy.com`.
    #[serde(default = "default_ghproxy_base_url")]
    pub ghproxy_base_url: String,

    /// Dedicated image generation provider settings used by the generate_image tool.
    #[serde(default)]
    pub image_generation: ImageGenerationConfig,

    /// Dedicated cloud speech provider settings used by the synthesize_speech tool.
    #[serde(default)]
    pub text_to_speech: TextToSpeechConfig,

    /// Defaults for native no-key public web search tools.
    #[serde(default)]
    pub web_search: WebSearchConfig,

    /// Background knowledge consolidation and review queue settings.
    #[serde(default)]
    pub dreaming: DreamingConfig,
}

fn default_tool_timeout() -> i64 {
    0
}
fn default_agent_timeout() -> i64 {
    0
}
fn default_cache_ttl_hours() -> u32 {
    24
}
fn default_search_limit() -> usize {
    20
}
fn default_min_search_similarity() -> f32 {
    0.2
}
fn default_max_text_file_size() -> u64 {
    100 * 1024 * 1024
}
fn default_max_video_file_size() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_max_audio_file_size() -> u64 {
    500 * 1024 * 1024
}
fn default_llm_timeout_secs() -> u64 {
    300
}
fn default_mcp_call_timeout_secs() -> u64 {
    300
}
fn default_dynamic_tool_visibility() -> bool {
    true
}
fn default_trace_enabled() -> bool {
    true
}
fn default_auto_memory_extraction() -> bool {
    true
}
fn default_auto_skill_learning() -> bool {
    true
}
fn default_dreaming_enabled() -> bool {
    true
}
fn default_dreaming_idle_interval_minutes() -> usize {
    180
}
fn default_dreaming_schedule_interval_minutes() -> usize {
    720
}
fn default_dreaming_max_artifacts_per_run() -> usize {
    24
}
fn default_dreaming_max_runs_per_day() -> usize {
    12
}
fn default_dreaming_local_only() -> bool {
    true
}
fn default_hf_mirror_base_url() -> String {
    "https://hf-mirror.com".to_string()
}
fn default_ghproxy_base_url() -> String {
    "https://mirror.ghproxy.com".to_string()
}
fn default_image_provider() -> String {
    "open_ai".to_string()
}
fn default_image_api_style() -> String {
    "openai_images".to_string()
}
fn default_image_base_url_option() -> Option<String> {
    Some("https://api.openai.com/v1".to_string())
}
fn default_image_model() -> String {
    "gpt-image-2".to_string()
}
fn default_image_size_option() -> Option<String> {
    Some("1024x1024".to_string())
}
fn default_image_output_format_option() -> Option<String> {
    Some("png".to_string())
}
fn default_tts_provider() -> String {
    "open_ai".to_string()
}
fn default_tts_api_style() -> String {
    "openai_speech".to_string()
}
fn default_tts_base_url_option() -> Option<String> {
    Some("https://api.openai.com/v1".to_string())
}
fn default_tts_model() -> String {
    "gpt-4o-mini-tts".to_string()
}
fn default_tts_voice() -> String {
    "coral".to_string()
}
fn default_tts_output_format() -> String {
    "wav".to_string()
}
fn default_tts_speed() -> f32 {
    1.0
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            tool_timeout_secs: default_tool_timeout(),
            agent_timeout_secs: default_agent_timeout(),
            timeout_defaults_version: CURRENT_TIMEOUT_DEFAULTS_VERSION,
            cache_ttl_hours: default_cache_ttl_hours(),
            default_search_limit: default_search_limit(),
            min_search_similarity: default_min_search_similarity(),
            max_text_file_size: default_max_text_file_size(),
            max_video_file_size: default_max_video_file_size(),
            max_audio_file_size: default_max_audio_file_size(),
            llm_timeout_secs: default_llm_timeout_secs(),
            mcp_call_timeout_secs: default_mcp_call_timeout_secs(),
            dynamic_tool_visibility: default_dynamic_tool_visibility(),
            tool_visibility_defaults_version: CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION,
            trace_enabled: default_trace_enabled(),
            confirm_destructive: false,
            shell_access_mode: ShellAccessMode::Restricted,
            tool_approval_mode: crate::approval::ToolApprovalMode::default(),
            auto_memory_extraction: true,
            auto_skill_learning: true,
            hf_mirror_base_url: default_hf_mirror_base_url(),
            ghproxy_base_url: default_ghproxy_base_url(),
            image_generation: ImageGenerationConfig::default(),
            text_to_speech: TextToSpeechConfig::default(),
            web_search: WebSearchConfig::default(),
            dreaming: DreamingConfig::default(),
        }
    }
}

fn encrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.image_generation.api_key =
        crate::crypto::encrypt_api_key(&config.image_generation.api_key)?;
    config.text_to_speech.api_key = crate::crypto::encrypt_api_key(&config.text_to_speech.api_key)?;
    for provider in &mut config.web_search.custom_providers {
        provider.api_key = crate::crypto::encrypt_api_key(&provider.api_key)?;
    }
    Ok(config)
}

fn decrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.image_generation.api_key =
        crate::crypto::decrypt_api_key(&config.image_generation.api_key)?;
    config.text_to_speech.api_key = crate::crypto::decrypt_api_key(&config.text_to_speech.api_key)?;
    for provider in &mut config.web_search.custom_providers {
        provider.api_key = crate::crypto::decrypt_api_key(&provider.api_key)?;
    }
    Ok(config)
}

fn migrate_timeout_defaults(mut config: AppConfig) -> (AppConfig, bool) {
    if config.timeout_defaults_version >= CURRENT_TIMEOUT_DEFAULTS_VERSION {
        return (config, false);
    }

    if config.tool_timeout_secs == LEGACY_DEFAULT_TOOL_TIMEOUT_SECS {
        config.tool_timeout_secs = default_tool_timeout();
    }
    if config.agent_timeout_secs == LEGACY_DEFAULT_AGENT_TIMEOUT_SECS {
        config.agent_timeout_secs = default_agent_timeout();
    }

    config.timeout_defaults_version = CURRENT_TIMEOUT_DEFAULTS_VERSION;
    (config, true)
}

fn migrate_tool_visibility_defaults(mut config: AppConfig) -> (AppConfig, bool) {
    if config.tool_visibility_defaults_version >= CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION {
        return (config, false);
    }

    // Keep the default tool surface task-local. Exact-prefix providers now
    // persist replayable turn scaffolding, so the old full-registry default is
    // no longer needed for prompt-cache continuity and bloats most requests.
    config.dynamic_tool_visibility = true;
    config.tool_visibility_defaults_version = CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION;
    (config, true)
}

impl Database {
    pub fn load_app_config(&self) -> Result<AppConfig, CoreError> {
        let conn = self.conn();
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_config')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(AppConfig::default());
        }
        let result = conn.query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![APP_CONFIG_KEY],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => {
                drop(conn);
                let config: AppConfig = serde_json::from_str(&json)?;
                let config = decrypt_app_config_secrets(config)?;
                let (config, timeout_migrated) = migrate_timeout_defaults(config);
                let (config, visibility_migrated) = migrate_tool_visibility_defaults(config);
                let migrated = timeout_migrated || visibility_migrated;
                if migrated {
                    self.save_app_config(&config)?;
                }
                Ok(config)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AppConfig::default()),
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    pub fn save_app_config(&self, config: &AppConfig) -> Result<(), CoreError> {
        let json = serde_json::to_string(&encrypt_app_config_secrets(config.clone())?)?;
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;
        conn.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![APP_CONFIG_KEY, &json],
        )?;
        Ok(())
    }

    /// Load the first-run wizard state. Returns `WizardState::default()` (not
    /// completed) when no record exists or the `app_config` table hasn't been
    /// created yet.
    pub fn load_wizard_state(&self) -> Result<WizardState, CoreError> {
        let conn = self.conn();
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_config')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(WizardState::default());
        }
        let result = conn.query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![WIZARD_STATE_KEY],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(WizardState::default()),
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    /// Persist the wizard state (creates the table lazily if needed).
    pub fn save_wizard_state(&self, state: &WizardState) -> Result<(), CoreError> {
        let json = serde_json::to_string(state)?;
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;
        conn.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![WIZARD_STATE_KEY, &json],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_state_roundtrip() {
        let db = Database::open_memory().expect("open_memory");

        // No record → not completed.
        let initial = db.load_wizard_state().expect("load initial");
        assert!(!initial.completed);

        // Save completed=true.
        db.save_wizard_state(&WizardState { completed: true })
            .expect("save completed");
        let loaded = db.load_wizard_state().expect("load completed");
        assert!(loaded.completed);

        // Reset.
        db.save_wizard_state(&WizardState { completed: false })
            .expect("save reset");
        let reset = db.load_wizard_state().expect("load reset");
        assert!(!reset.completed);
    }

    #[test]
    fn app_config_defaults_agent_behavior() {
        let config = AppConfig::default();

        assert_eq!(config.tool_timeout_secs, 0);
        assert_eq!(config.agent_timeout_secs, 0);
        assert_eq!(
            config.timeout_defaults_version,
            CURRENT_TIMEOUT_DEFAULTS_VERSION
        );
        assert!(config.dynamic_tool_visibility);
        assert_eq!(
            config.tool_visibility_defaults_version,
            CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION
        );
        assert!(config.trace_enabled);
        assert_eq!(config.web_search.custom_providers.len(), 5);
        assert!(config
            .web_search
            .custom_providers
            .iter()
            .any(|provider| provider.preset == WebSearchCustomProviderPreset::Brave));
        assert!(config
            .web_search
            .custom_providers
            .iter()
            .any(|provider| provider.preset == WebSearchCustomProviderPreset::AnySearch));
    }

    #[test]
    fn app_config_migrates_legacy_timeout_defaults_once() {
        let db = Database::open_memory().expect("open_memory");
        {
            let conn = db.conn();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS app_config (
                     key TEXT PRIMARY KEY NOT NULL,
                     value TEXT NOT NULL,
                     updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                 )",
            )
            .expect("create app_config table");
            let legacy = serde_json::json!({
                "toolTimeoutSecs": LEGACY_DEFAULT_TOOL_TIMEOUT_SECS,
                "agentTimeoutSecs": LEGACY_DEFAULT_AGENT_TIMEOUT_SECS
            });
            conn.execute(
                "INSERT INTO app_config (key, value, updated_at)
                 VALUES (?1, ?2, datetime('now'))",
                params![APP_CONFIG_KEY, legacy.to_string()],
            )
            .expect("insert legacy app config");
        }

        let migrated = db.load_app_config().expect("load migrated app config");
        assert_eq!(migrated.tool_timeout_secs, 0);
        assert_eq!(migrated.agent_timeout_secs, 0);
        assert_eq!(
            migrated.timeout_defaults_version,
            CURRENT_TIMEOUT_DEFAULTS_VERSION
        );
        assert!(migrated.dynamic_tool_visibility);
        assert_eq!(
            migrated.tool_visibility_defaults_version,
            CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION
        );

        let mut explicit = migrated;
        explicit.tool_timeout_secs = LEGACY_DEFAULT_TOOL_TIMEOUT_SECS;
        explicit.agent_timeout_secs = LEGACY_DEFAULT_AGENT_TIMEOUT_SECS;
        db.save_app_config(&explicit)
            .expect("save explicit bounded timeouts");

        let reloaded = db.load_app_config().expect("reload explicit app config");
        assert_eq!(reloaded.tool_timeout_secs, LEGACY_DEFAULT_TOOL_TIMEOUT_SECS);
        assert_eq!(
            reloaded.agent_timeout_secs,
            LEGACY_DEFAULT_AGENT_TIMEOUT_SECS
        );
        assert_eq!(
            reloaded.timeout_defaults_version,
            CURRENT_TIMEOUT_DEFAULTS_VERSION
        );
    }

    #[test]
    fn web_search_provider_wire_names_match_frontend() {
        let config: WebSearchConfig = serde_json::from_value(serde_json::json!({
            "providerProfile": "default",
            "reranker": "auto",
            "providerMode": "built_in_first",
            "customProviders": [
                {
                    "id": "tavily",
                    "preset": "tavily",
                    "name": "Tavily Search",
                    "enabled": true,
                    "apiKey": "tvly-dev-example",
                    "baseUrl": "https://api.tavily.com/search",
                    "priority": 20
                },
                {
                    "id": "anysearch",
                    "preset": "anysearch",
                    "name": "AnySearch",
                    "enabled": true,
                    "apiKey": "",
                    "baseUrl": "https://api.anysearch.com/v1/search",
                    "priority": 25
                },
                {
                    "id": "serpapi_google",
                    "preset": "serpapi_google",
                    "name": "SerpAPI Google",
                    "enabled": false,
                    "apiKey": "",
                    "baseUrl": "https://serpapi.com/search.json",
                    "priority": 30
                }
            ]
        }))
        .expect("frontend web search config should deserialize");

        assert_eq!(
            config.custom_providers[1].preset,
            WebSearchCustomProviderPreset::AnySearch
        );
        assert_eq!(
            config.custom_providers[2].preset,
            WebSearchCustomProviderPreset::SerpApiGoogle
        );

        let json = serde_json::to_string(&config).expect("serialize web search config");
        assert!(json.contains("\"preset\":\"anysearch\""));
        assert!(json.contains("\"preset\":\"serpapi_google\""));
    }

    #[test]
    fn app_config_encrypts_web_search_provider_keys() {
        let db = Database::open_memory().expect("open_memory");
        let mut config = AppConfig::default();
        let tavily = config
            .web_search
            .custom_providers
            .iter_mut()
            .find(|provider| provider.preset == WebSearchCustomProviderPreset::Tavily)
            .expect("tavily provider");
        tavily.enabled = true;
        tavily.api_key = "tvly-dev-example".to_string();

        db.save_app_config(&config).expect("save app config");
        let loaded = db.load_app_config().expect("load app config");

        let loaded_tavily = loaded
            .web_search
            .custom_providers
            .iter()
            .find(|provider| provider.preset == WebSearchCustomProviderPreset::Tavily)
            .expect("loaded tavily provider");
        assert_eq!(loaded_tavily.api_key, "tvly-dev-example");

        let raw: String = db
            .conn()
            .query_row(
                "SELECT value FROM app_config WHERE key = ?1",
                params![APP_CONFIG_KEY],
                |row| row.get(0),
            )
            .expect("raw app_config");
        assert!(!raw.contains("tvly-dev-example"));
    }
}
