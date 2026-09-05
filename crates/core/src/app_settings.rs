use crate::db::Database;
use crate::error::CoreError;
use crate::web_search::{WebSearchProviderProfile, WebSearchReranker};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

const APP_CONFIG_KEY: &str = "app_config";
const UI_LOCALE_KEY: &str = "ui_locale";
const WIZARD_STATE_KEY: &str = "wizard_state";
const CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION: u32 = 3;

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
    /// Local sherpa-onnx executable (a bare PATH command or absolute path).
    #[serde(default)]
    pub executable_path: Option<String>,
    /// Primary ONNX model file for local synthesis.
    #[serde(default)]
    pub model_path: Option<String>,
    /// Model token table used by VITS, Kokoro, and Kitten.
    #[serde(default)]
    pub tokens_path: Option<String>,
    /// Optional voices.bin used by multi-voice Kokoro and Kitten models.
    #[serde(default)]
    pub voices_path: Option<String>,
    /// Optional espeak-ng data directory bundled with a model.
    #[serde(default)]
    pub data_dir: Option<String>,
    /// Optional lexicon file or comma-separated lexicon files.
    #[serde(default)]
    pub lexicon_path: Option<String>,
    /// Local inference thread count.
    #[serde(default = "default_tts_num_threads")]
    pub num_threads: u32,
    /// Automatically read the final answer of each successful turn.
    #[serde(default)]
    pub auto_speak_final_answers: bool,
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
            executable_path: None,
            model_path: None,
            tokens_path: None,
            voices_path: None,
            data_dir: None,
            lexicon_path: None,
            num_threads: default_tts_num_threads(),
            auto_speak_final_answers: false,
        }
    }
}

impl TextToSpeechConfig {
    pub fn is_configured(&self) -> bool {
        if self.api_style == "sherpa_onnx" {
            let family = self.model.trim();
            let has_common_paths = self
                .executable_path
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                && self
                    .model_path
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && self
                    .tokens_path
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
            let has_family_paths = !matches!(family, "kokoro" | "kitten")
                || self
                    .voices_path
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
            return has_common_paths && has_family_paths;
        }

        !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.voice.trim().is_empty()
    }
}

/// Dedicated voice-input transcription settings. Media ingestion can inherit
/// this backend so microphone and file transcription share one provider
/// contract, while an explicit local-Whisper media override remains available.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechToTextConfig {
    #[serde(default = "default_stt_provider")]
    pub provider: String,
    #[serde(default = "default_stt_api_style")]
    pub api_style: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_stt_base_url_option")]
    pub base_url: Option<String>,
    #[serde(default = "default_stt_model")]
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    /// Local sherpa-onnx command line executable.
    #[serde(default)]
    pub executable_path: Option<String>,
    /// `sense_voice` for a single-model offline recognizer, or `zipformer`
    /// for encoder/decoder/joiner streaming models.
    #[serde(default = "default_stt_sherpa_model_family")]
    pub sherpa_model_family: String,
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub tokens_path: Option<String>,
    #[serde(default)]
    pub encoder_path: Option<String>,
    #[serde(default)]
    pub decoder_path: Option<String>,
    #[serde(default)]
    pub joiner_path: Option<String>,
    #[serde(default = "default_stt_num_threads")]
    pub num_threads: u32,
}

impl Default for SpeechToTextConfig {
    fn default() -> Self {
        Self {
            provider: default_stt_provider(),
            api_style: default_stt_api_style(),
            api_key: String::new(),
            base_url: default_stt_base_url_option(),
            model: default_stt_model(),
            language: None,
            executable_path: None,
            sherpa_model_family: default_stt_sherpa_model_family(),
            model_path: None,
            tokens_path: None,
            encoder_path: None,
            decoder_path: None,
            joiner_path: None,
            num_threads: default_stt_num_threads(),
        }
    }
}

impl SpeechToTextConfig {
    /// The microphone's Qwen preset is realtime. Upgrade the old official
    /// file-only preset without changing the account, region, or custom URLs.
    fn normalize_dictation(&mut self) -> bool {
        if self.provider != "alibaba_model_studio"
            || self.api_style != "dashscope_asr"
            || self.model.trim() != "qwen3-asr-flash"
        {
            return false;
        }
        let Some(base) = self.base_url.as_deref() else {
            return false;
        };
        let Ok(mut url) = url::Url::parse(base.trim()) else {
            return false;
        };
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some(
                    "dashscope.aliyuncs.com"
                        | "dashscope-intl.aliyuncs.com"
                        | "dashscope-us.aliyuncs.com"
                )
            )
            || url.path().trim_end_matches('/') != "/compatible-mode/v1"
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        url.set_path("/api-ws/v1");
        self.base_url = Some(url.to_string());
        self.api_style = "dashscope_realtime_asr".to_string();
        self.model = "qwen3-asr-flash-realtime".to_string();
        true
    }

    pub fn is_configured(&self) -> bool {
        match self.api_style.as_str() {
            "local_whisper" => true,
            "sherpa_onnx" => {
                let common = self
                    .executable_path
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                    && self
                        .tokens_path
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty());
                if self.sherpa_model_family == "zipformer" {
                    common
                        && [&self.encoder_path, &self.decoder_path, &self.joiner_path]
                            .into_iter()
                            .all(|path| {
                                path.as_deref()
                                    .is_some_and(|value| !value.trim().is_empty())
                            })
                } else {
                    common
                        && self
                            .model_path
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                }
            }
            "openai_realtime_transcription" => {
                self.model.trim() == "gpt-live-transcribe"
                    && !self.api_key.trim().is_empty()
                    && self
                        .base_url
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
            }
            "dashscope_realtime_asr" => {
                self.model.trim() == "qwen3-asr-flash-realtime"
                    && !self.api_key.trim().is_empty()
                    && self
                        .base_url
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
            }
            "openai_transcription" | "dashscope_asr" => {
                !self.api_key.trim().is_empty()
                    && !self.model.trim().is_empty()
                    && self
                        .base_url
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    #[serde(default)]
    pub execution_mode: crate::llm::native_search::SearchExecutionMode,
    #[serde(default)]
    pub provider_native_engine: crate::llm::native_search::ProviderNativeSearchEngine,
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
            execution_mode: crate::llm::native_search::SearchExecutionMode::Auto,
            provider_native_engine: crate::llm::native_search::ProviderNativeSearchEngine::Auto,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowCloseBehavior {
    #[default]
    Exit,
    MinimizeToTray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionDisplayMode {
    #[default]
    Always,
    DuringTasks,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionInteractionMode {
    #[default]
    Smart,
    Locked,
    ClickThrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionActiveRunPolicy {
    #[default]
    HighestPriority,
    PinnedRun,
    PinnedProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionAnchor {
    BottomLeft,
    #[default]
    BottomRight,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionLogicalPosition {
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_companion_scale_factor")]
    pub scale_factor: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub selected_pet_id: Option<String>,
    #[serde(default)]
    pub display_mode: CompanionDisplayMode,
    #[serde(default)]
    pub interaction_mode: CompanionInteractionMode,
    #[serde(default)]
    pub auto_show_on_start: bool,
    #[serde(default = "default_true")]
    pub continue_when_main_hidden: bool,
    #[serde(default = "default_companion_scale")]
    pub scale: f32,
    #[serde(default = "default_companion_fps_cap")]
    pub animation_fps_cap: u8,
    #[serde(default)]
    pub reduced_motion: bool,
    #[serde(default = "default_true")]
    pub idle_actions: bool,
    #[serde(default)]
    pub auto_walk: bool,
    #[serde(default = "default_true")]
    pub show_bubbles: bool,
    #[serde(default)]
    pub bubble_task_titles: bool,
    #[serde(default = "default_true")]
    pub privacy_mode: bool,
    #[serde(default = "default_success_hold_ms")]
    pub success_hold_ms: u32,
    #[serde(default = "default_failure_hold_ms")]
    pub failure_hold_ms: u32,
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    #[serde(default)]
    pub visible_on_all_workspaces: bool,
    #[serde(default)]
    pub lock_position: bool,
    #[serde(default)]
    pub active_run_policy: CompanionActiveRunPolicy,
    #[serde(default)]
    pub pinned_run_id: Option<String>,
    #[serde(default)]
    pub pinned_project_id: Option<String>,
    #[serde(default)]
    pub monitor_id: Option<String>,
    #[serde(default)]
    pub anchor: CompanionAnchor,
    #[serde(default)]
    pub position: Option<CompanionLogicalPosition>,
    #[serde(default = "default_true")]
    pub edge_snap: bool,
    #[serde(default = "default_true")]
    pub avoid_taskbar: bool,
    #[serde(default = "default_true")]
    pub allow_monitor_move: bool,
    #[serde(default)]
    pub codex_import_path: Option<String>,
}

impl CompanionSettings {
    fn normalize(&mut self) {
        self.scale = self.scale.clamp(0.5, 2.0);
        self.animation_fps_cap = match self.animation_fps_cap {
            0..=24 => 24,
            25..=30 => 30,
            _ => 60,
        };
        self.success_hold_ms = self.success_hold_ms.clamp(1_000, 30_000);
        self.failure_hold_ms = self.failure_hold_ms.clamp(1_000, 30_000);
        if let Some(position) = &mut self.position {
            if !position.x.is_finite() || !position.y.is_finite() {
                self.position = None;
            } else {
                position.scale_factor = if position.scale_factor.is_finite() {
                    position.scale_factor.clamp(0.5, 8.0)
                } else {
                    default_companion_scale_factor()
                };
            }
        }
        for value in [
            &mut self.selected_pet_id,
            &mut self.pinned_run_id,
            &mut self.pinned_project_id,
            &mut self.monitor_id,
            &mut self.codex_import_path,
        ] {
            if value
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                *value = None;
            }
        }
    }
}

impl Default for CompanionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            selected_pet_id: None,
            display_mode: CompanionDisplayMode::Always,
            interaction_mode: CompanionInteractionMode::Smart,
            auto_show_on_start: false,
            continue_when_main_hidden: true,
            scale: default_companion_scale(),
            animation_fps_cap: default_companion_fps_cap(),
            reduced_motion: false,
            idle_actions: true,
            auto_walk: false,
            show_bubbles: true,
            bubble_task_titles: false,
            privacy_mode: true,
            success_hold_ms: default_success_hold_ms(),
            failure_hold_ms: default_failure_hold_ms(),
            always_on_top: true,
            visible_on_all_workspaces: false,
            lock_position: false,
            active_run_policy: CompanionActiveRunPolicy::HighestPriority,
            pinned_run_id: None,
            pinned_project_id: None,
            monitor_id: None,
            anchor: CompanionAnchor::BottomRight,
            position: None,
            edge_snap: true,
            avoid_taskbar: true,
            allow_monitor_move: true,
            codex_import_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// UI locale shared with native desktop surfaces such as the system tray.
    #[serde(default = "default_ui_locale")]
    pub ui_locale: String,

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

    /// What the main window close button does. Default: exit the application.
    #[serde(default)]
    pub window_close_behavior: WindowCloseBehavior,

    /// Optional root directory for managed local model downloads. An empty
    /// value keeps the legacy per-model locations for existing installations.
    #[serde(default)]
    pub local_model_root: String,

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

    /// GitHub reverse-proxy base URL used for built-in GitHub downloads.
    /// Empty string disables the fallback. Default: `https://mirror.ghproxy.com`.
    #[serde(default = "default_ghproxy_base_url")]
    pub ghproxy_base_url: String,

    /// Dedicated image generation provider settings used by the generate_image tool.
    #[serde(default)]
    pub image_generation: ImageGenerationConfig,

    /// Dedicated cloud speech provider settings used by the synthesize_speech tool.
    #[serde(default)]
    pub text_to_speech: TextToSpeechConfig,

    /// Dedicated low-latency microphone transcription settings.
    #[serde(default)]
    pub speech_to_text: SpeechToTextConfig,

    /// Defaults for native no-key public web search tools.
    #[serde(default)]
    pub web_search: WebSearchConfig,

    /// Background knowledge consolidation and review queue settings.
    #[serde(default)]
    pub dreaming: DreamingConfig,

    /// Desktop Companion runtime, rendering, and pack-discovery settings.
    #[serde(default)]
    pub companion: CompanionSettings,
}

fn default_companion_scale() -> f32 {
    1.0
}
fn default_companion_scale_factor() -> f64 {
    1.0
}
fn default_companion_fps_cap() -> u8 {
    24
}
fn default_success_hold_ms() -> u32 {
    4_000
}
fn default_failure_hold_ms() -> u32 {
    6_000
}

fn default_ui_locale() -> String {
    "en".to_string()
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

fn default_tts_num_threads() -> u32 {
    2
}

fn default_stt_provider() -> String {
    "local_whisper".to_string()
}
fn default_stt_api_style() -> String {
    "local_whisper".to_string()
}
fn default_stt_base_url_option() -> Option<String> {
    None
}
fn default_stt_model() -> String {
    "whisper-1".to_string()
}
fn default_stt_sherpa_model_family() -> String {
    "sense_voice".to_string()
}
fn default_stt_num_threads() -> u32 {
    2
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui_locale: default_ui_locale(),
            default_search_limit: default_search_limit(),
            min_search_similarity: default_min_search_similarity(),
            max_text_file_size: default_max_text_file_size(),
            max_video_file_size: default_max_video_file_size(),
            max_audio_file_size: default_max_audio_file_size(),
            dynamic_tool_visibility: default_dynamic_tool_visibility(),
            tool_visibility_defaults_version: CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION,
            trace_enabled: default_trace_enabled(),
            window_close_behavior: WindowCloseBehavior::default(),
            local_model_root: String::new(),
            confirm_destructive: false,
            shell_access_mode: ShellAccessMode::Restricted,
            tool_approval_mode: crate::approval::ToolApprovalMode::default(),
            auto_memory_extraction: true,
            auto_skill_learning: true,
            hf_mirror_base_url: default_hf_mirror_base_url(),
            ghproxy_base_url: default_ghproxy_base_url(),
            image_generation: ImageGenerationConfig::default(),
            text_to_speech: TextToSpeechConfig::default(),
            speech_to_text: SpeechToTextConfig::default(),
            web_search: WebSearchConfig::default(),
            dreaming: DreamingConfig::default(),
            companion: CompanionSettings::default(),
        }
    }
}

fn encrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.companion.normalize();
    config.speech_to_text.normalize_dictation();
    config.image_generation.api_key =
        crate::crypto::encrypt_api_key(&config.image_generation.api_key)?;
    config.text_to_speech.api_key = crate::crypto::encrypt_api_key(&config.text_to_speech.api_key)?;
    config.speech_to_text.api_key = crate::crypto::encrypt_api_key(&config.speech_to_text.api_key)?;
    for provider in &mut config.web_search.custom_providers {
        provider.api_key = crate::crypto::encrypt_api_key(&provider.api_key)?;
    }
    Ok(config)
}

fn decrypt_app_config_secrets(mut config: AppConfig) -> Result<AppConfig, CoreError> {
    config.image_generation.api_key =
        crate::crypto::decrypt_api_key(&config.image_generation.api_key)?;
    config.text_to_speech.api_key = crate::crypto::decrypt_api_key(&config.text_to_speech.api_key)?;
    config.speech_to_text.api_key = crate::crypto::decrypt_api_key(&config.speech_to_text.api_key)?;
    for provider in &mut config.web_search.custom_providers {
        provider.api_key = crate::crypto::decrypt_api_key(&provider.api_key)?;
    }
    config.companion.normalize();
    Ok(config)
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
        let persisted_ui_locale = conn
            .query_row(
                "SELECT value FROM app_config WHERE key = ?1",
                params![UI_LOCALE_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let result = conn.query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![APP_CONFIG_KEY],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => {
                drop(conn);
                let config: AppConfig = serde_json::from_str(&json)?;
                let mut config = decrypt_app_config_secrets(config)?;
                if let Some(locale) = persisted_ui_locale {
                    config.ui_locale = locale;
                }
                let (mut config, visibility_migrated) = migrate_tool_visibility_defaults(config);
                let speech_migrated = config.speech_to_text.normalize_dictation();
                if visibility_migrated || speech_migrated {
                    self.save_app_config(&config)?;
                }
                Ok(config)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let mut config = AppConfig::default();
                if let Some(locale) = persisted_ui_locale {
                    config.ui_locale = locale;
                }
                Ok(config)
            }
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    pub fn save_app_config(&self, config: &AppConfig) -> Result<(), CoreError> {
        let mut conn = self.conn();
        let transaction =
            conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;
        let persisted_ui_locale = transaction
            .query_row(
                "SELECT value FROM app_config WHERE key = ?1",
                params![UI_LOCALE_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut config = config.clone();
        if let Some(locale) = persisted_ui_locale {
            config.ui_locale = locale;
        }
        let json = serde_json::to_string(&encrypt_app_config_secrets(config)?)?;
        transaction.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![APP_CONFIG_KEY, &json],
        )?;
        crate::settings_schema_v2::sync_legacy_app_config_in_transaction(&transaction)?;
        crate::capability_registry::sync_registry_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    /// Persist the UI locale independently from whole AppConfig snapshots.
    /// This prevents a stale renderer snapshot from reverting native surfaces.
    pub fn save_ui_locale(&self, locale: &str) -> Result<(), CoreError> {
        let locale = locale.trim();
        if locale.is_empty() {
            return Err(CoreError::InvalidInput(
                "UI locale must not be empty".to_string(),
            ));
        }
        let mut conn = self.conn();
        let transaction =
            conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;
        transaction.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![UI_LOCALE_KEY, locale],
        )?;
        transaction.commit()?;
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

        assert!(config.dynamic_tool_visibility);
        assert_eq!(
            config.tool_visibility_defaults_version,
            CURRENT_TOOL_VISIBILITY_DEFAULTS_VERSION
        );
        assert!(config.trace_enabled);
        assert_eq!(config.window_close_behavior, WindowCloseBehavior::Exit);
        assert!(config.local_model_root.is_empty());
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
        assert!(!config.companion.enabled);
        assert_eq!(
            config.companion.interaction_mode,
            CompanionInteractionMode::Smart
        );
        assert_eq!(config.companion.anchor, CompanionAnchor::BottomRight);
    }

    #[test]
    fn independently_persisted_ui_locale_survives_stale_app_config_saves() {
        let db = Database::open_memory().expect("open_memory");
        let mut config = AppConfig::default();
        config.default_search_limit = 12;
        db.save_app_config(&config).expect("save initial config");
        db.save_ui_locale("zh-CN").expect("save UI locale");

        let mut stale_snapshot = config;
        stale_snapshot.ui_locale = "en".to_string();
        stale_snapshot.default_search_limit = 48;
        db.save_app_config(&stale_snapshot)
            .expect("save stale whole-config snapshot");

        let loaded = db.load_app_config().expect("load merged config");
        assert_eq!(loaded.ui_locale, "zh-CN");
        assert_eq!(loaded.default_search_limit, 48);
    }

    #[test]
    fn independently_persisted_ui_locale_applies_before_app_config_exists() {
        let db = Database::open_memory().expect("open_memory");
        db.save_ui_locale("ja").expect("save UI locale");

        let loaded = db.load_app_config().expect("load default config");
        assert_eq!(loaded.ui_locale, "ja");
    }

    #[test]
    fn companion_settings_are_backward_compatible_and_normalized_on_save() {
        let legacy: AppConfig = serde_json::from_value(serde_json::json!({}))
            .expect("legacy app config should use companion defaults");
        assert_eq!(legacy.companion, CompanionSettings::default());

        let db = Database::open_memory().expect("open_memory");
        let mut config = AppConfig::default();
        config.companion.enabled = true;
        config.companion.scale = 9.0;
        config.companion.animation_fps_cap = 29;
        config.companion.position = Some(CompanionLogicalPosition {
            x: f64::NAN,
            y: 42.0,
            scale_factor: 1.0,
        });
        db.save_app_config(&config).expect("save normalized config");
        let loaded = db.load_app_config().expect("load normalized config");

        assert!(loaded.companion.enabled);
        assert_eq!(loaded.companion.scale, 2.0);
        assert_eq!(loaded.companion.animation_fps_cap, 30);
        assert_eq!(loaded.companion.position, None);
    }

    #[test]
    fn app_config_drops_obsolete_global_timeout_fields() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({
            "toolTimeoutSecs": 17,
            "agentTimeoutSecs": 23,
            "llmTimeoutSecs": 29,
            "mcpCallTimeoutSecs": 31,
            "timeoutDefaultsVersion": 1
        }))
        .expect("legacy app config should remain readable");

        let serialized = serde_json::to_value(config).expect("serialize app config");
        for field in [
            "toolTimeoutSecs",
            "agentTimeoutSecs",
            "llmTimeoutSecs",
            "mcpCallTimeoutSecs",
            "timeoutDefaultsVersion",
        ] {
            assert!(
                serialized.get(field).is_none(),
                "obsolete global timeout field {field} must not be persisted"
            );
        }
    }

    #[test]
    fn app_config_accepts_minimize_to_tray_close_behavior() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({
            "windowCloseBehavior": "minimize_to_tray"
        }))
        .expect("deserialize tray close behavior");

        assert_eq!(
            config.window_close_behavior,
            WindowCloseBehavior::MinimizeToTray
        );
        let json = serde_json::to_value(config).expect("serialize app config");
        assert_eq!(json["windowCloseBehavior"], "minimize_to_tray");
    }

    #[test]
    fn app_config_roundtrips_custom_local_model_root() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({
            "localModelRoot": "D:\\NexaModels"
        }))
        .expect("deserialize local model root");

        assert_eq!(config.local_model_root, "D:\\NexaModels");
        let json = serde_json::to_value(config).expect("serialize app config");
        assert_eq!(json["localModelRoot"], "D:\\NexaModels");
    }

    #[test]
    fn qwen_microphone_upgrade_preserves_region_and_refuses_custom_routes() {
        for host in [
            "dashscope.aliyuncs.com",
            "dashscope-intl.aliyuncs.com",
            "dashscope-us.aliyuncs.com",
        ] {
            let mut config = SpeechToTextConfig {
                provider: "alibaba_model_studio".into(),
                api_style: "dashscope_asr".into(),
                model: "qwen3-asr-flash".into(),
                api_key: "test-account".into(),
                base_url: Some(format!("https://{host}/compatible-mode/v1")),
                ..Default::default()
            };
            assert!(config.normalize_dictation());
            assert_eq!(config.base_url, Some(format!("https://{host}/api-ws/v1")));
            assert_eq!(config.api_key, "test-account");
            assert_eq!(config.model, "qwen3-asr-flash-realtime");
            assert!(!config.normalize_dictation());
        }
        let mut custom = SpeechToTextConfig {
            provider: "alibaba_model_studio".into(),
            api_style: "dashscope_asr".into(),
            model: "qwen3-asr-flash".into(),
            base_url: Some("https://relay.example/compatible-mode/v1".into()),
            ..Default::default()
        };
        assert!(!custom.normalize_dictation());
        assert_eq!(custom.model, "qwen3-asr-flash");
    }

    #[test]
    fn speech_to_text_configuration_covers_local_cloud_realtime_and_sherpa() {
        let local = SpeechToTextConfig::default();
        assert_eq!(local.api_style, "local_whisper");
        assert!(local.is_configured());

        let mut cloud = SpeechToTextConfig {
            api_style: "openai_transcription".to_string(),
            ..SpeechToTextConfig::default()
        };
        assert!(!cloud.is_configured());
        cloud.api_key = "secret".to_string();
        cloud.base_url = Some("https://api.openai.com/v1".to_string());
        assert!(cloud.is_configured());

        let realtime = SpeechToTextConfig {
            api_style: "openai_realtime_transcription".to_string(),
            api_key: "secret".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            model: "gpt-live-transcribe".to_string(),
            ..SpeechToTextConfig::default()
        };
        assert!(realtime.is_configured());

        let dashscope_realtime = SpeechToTextConfig {
            provider: "alibaba_model_studio".to_string(),
            api_style: "dashscope_realtime_asr".to_string(),
            api_key: "secret".to_string(),
            base_url: Some("https://dashscope.aliyuncs.com/api-ws/v1".to_string()),
            model: "qwen3-asr-flash-realtime".to_string(),
            ..SpeechToTextConfig::default()
        };
        assert!(dashscope_realtime.is_configured());
        assert!(!SpeechToTextConfig {
            model: "qwen3-asr-flash".to_string(),
            ..dashscope_realtime
        }
        .is_configured());

        let mut sherpa = SpeechToTextConfig {
            api_style: "sherpa_onnx".to_string(),
            executable_path: Some("sherpa-onnx-offline".to_string()),
            model_path: Some("model.onnx".to_string()),
            tokens_path: Some("tokens.txt".to_string()),
            ..SpeechToTextConfig::default()
        };
        assert!(sherpa.is_configured());

        sherpa.sherpa_model_family = "zipformer".to_string();
        assert!(!sherpa.is_configured());
        sherpa.encoder_path = Some("encoder.onnx".to_string());
        sherpa.decoder_path = Some("decoder.onnx".to_string());
        sherpa.joiner_path = Some("joiner.onnx".to_string());
        assert!(sherpa.is_configured());
    }

    #[test]
    fn web_search_provider_wire_names_match_frontend() {
        let config: WebSearchConfig = serde_json::from_value(serde_json::json!({
            "providerNativeEngine": "exa",
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
            config.provider_native_engine,
            crate::llm::native_search::ProviderNativeSearchEngine::Exa
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
    fn local_sherpa_tts_uses_files_instead_of_an_api_key() {
        let mut config = TextToSpeechConfig {
            api_style: "sherpa_onnx".to_string(),
            model: "vits".to_string(),
            executable_path: Some("sherpa-onnx-offline-tts".to_string()),
            model_path: Some("model.onnx".to_string()),
            tokens_path: Some("tokens.txt".to_string()),
            ..TextToSpeechConfig::default()
        };
        assert!(config.is_configured());

        config.model = "kokoro".to_string();
        assert!(!config.is_configured());
        config.voices_path = Some("voices.bin".to_string());
        assert!(config.is_configured());
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
