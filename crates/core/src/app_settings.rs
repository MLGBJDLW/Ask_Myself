use crate::db::Database;
use crate::error::CoreError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

const APP_CONFIG_KEY: &str = "app_config";
const WIZARD_STATE_KEY: &str = "wizard_state";

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
    /// Maximum time for a single tool call in seconds. 0 disables the hard tool timeout.
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: i64,
    /// Maximum time for an agent turn in seconds. 0 disables the hard turn timeout.
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_secs: i64,

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

    /// LLM HTTP request timeout in seconds. Default: 300
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,

    /// MCP tool call timeout in seconds. Default: 60
    #[serde(default = "default_mcp_call_timeout_secs")]
    pub mcp_call_timeout_secs: u64,

    /// Whether to send only context-selected tools to the main agent. Default: false.
    #[serde(default = "default_dynamic_tool_visibility")]
    pub dynamic_tool_visibility: bool,

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
}

fn default_tool_timeout() -> i64 {
    30
}
fn default_agent_timeout() -> i64 {
    180
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
    60
}
fn default_dynamic_tool_visibility() -> bool {
    false
}
fn default_trace_enabled() -> bool {
    true
}
fn default_auto_memory_extraction() -> bool {
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            tool_timeout_secs: default_tool_timeout(),
            agent_timeout_secs: default_agent_timeout(),
            cache_ttl_hours: default_cache_ttl_hours(),
            default_search_limit: default_search_limit(),
            min_search_similarity: default_min_search_similarity(),
            max_text_file_size: default_max_text_file_size(),
            max_video_file_size: default_max_video_file_size(),
            max_audio_file_size: default_max_audio_file_size(),
            llm_timeout_secs: default_llm_timeout_secs(),
            mcp_call_timeout_secs: default_mcp_call_timeout_secs(),
            dynamic_tool_visibility: default_dynamic_tool_visibility(),
            trace_enabled: default_trace_enabled(),
            confirm_destructive: false,
            shell_access_mode: ShellAccessMode::Restricted,
            tool_approval_mode: crate::approval::ToolApprovalMode::default(),
            auto_memory_extraction: true,
            hf_mirror_base_url: default_hf_mirror_base_url(),
            ghproxy_base_url: default_ghproxy_base_url(),
            image_generation: ImageGenerationConfig::default(),
        }
    }
}

fn encrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.image_generation.api_key =
        crate::crypto::encrypt_api_key(&config.image_generation.api_key)?;
    Ok(config)
}

fn decrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.image_generation.api_key =
        crate::crypto::decrypt_api_key(&config.image_generation.api_key)?;
    Ok(config)
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
                let config: AppConfig = serde_json::from_str(&json)?;
                decrypt_app_config_secrets(config)
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

        assert!(!config.dynamic_tool_visibility);
        assert!(config.trace_enabled);
    }
}
