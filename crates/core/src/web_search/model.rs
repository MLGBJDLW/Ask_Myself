use serde::{Deserialize, Serialize};

pub const DEFAULT_SEARCH_LIMIT: usize = 8;
pub const MAX_SEARCH_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEngine {
    Baidu,
    Sogou,
    Bing,
    DuckDuckGo,
    Brave,
    Tavily,
    SerpApiGoogle,
    Searxng,
}

impl SearchEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baidu => "baidu",
            Self::Sogou => "sogou",
            Self::Bing => "bing",
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::SerpApiGoogle => "serpapi_google",
            Self::Searxng => "searxng",
        }
    }

    pub fn is_native(self) -> bool {
        matches!(
            self,
            Self::Baidu | Self::Sogou | Self::Bing | Self::DuckDuckGo
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "baidu" | "百度" => Some(Self::Baidu),
            "sogou" | "搜狗" => Some(Self::Sogou),
            "bing" | "必应" => Some(Self::Bing),
            "duckduckgo" | "duck_duck_go" | "ddg" => Some(Self::DuckDuckGo),
            "brave" | "brave_search" => Some(Self::Brave),
            "tavily" => Some(Self::Tavily),
            "serpapi_google" | "serpapi" => Some(Self::SerpApiGoogle),
            "searxng" | "searx" => Some(Self::Searxng),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchRegion {
    #[default]
    Auto,
    MainlandCn,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchLanguage {
    #[default]
    Auto,
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange {
    #[default]
    Any,
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderProfile {
    #[serde(alias = "native", alias = "native_balanced")]
    #[default]
    Default,
    #[serde(alias = "free")]
    Free,
    #[serde(alias = "free-verified")]
    FreeVerified,
    #[serde(alias = "max-evidence")]
    MaxEvidence,
}

impl WebSearchProviderProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Free => "free",
            Self::FreeVerified => "free_verified",
            Self::MaxEvidence => "max_evidence",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchReranker {
    #[default]
    Auto,
    None,
    #[serde(alias = "docs-first")]
    DocsFirst,
    Research,
    #[serde(alias = "news-balanced")]
    NewsBalanced,
}

impl WebSearchReranker {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::DocsFirst => "docs_first",
            Self::Research => "research",
            Self::NewsBalanced => "news_balanced",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub region: SearchRegion,
    #[serde(default)]
    pub language: SearchLanguage,
    #[serde(default)]
    pub engines: Vec<String>,
    #[serde(default)]
    pub time_range: TimeRange,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default = "default_include_snippets")]
    pub include_snippets: bool,
    #[serde(default)]
    pub provider_profile: Option<WebSearchProviderProfile>,
    #[serde(default)]
    pub reranker: Option<WebSearchReranker>,
}

fn default_limit() -> usize {
    DEFAULT_SEARCH_LIMIT
}

fn default_include_snippets() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub effective_query: String,
    pub limit: usize,
    pub region: SearchRegion,
    pub language: SearchLanguage,
    pub engines: Vec<SearchEngine>,
    pub explicit_engines: bool,
    pub time_range: TimeRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    pub include_snippets: bool,
    pub provider_profile: WebSearchProviderProfile,
    pub reranker: WebSearchReranker,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchProviderFailure {
    pub engine: SearchEngine,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl SearchProviderFailure {
    pub fn new(engine: SearchEngine, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            engine,
            code: code.into(),
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after_secs: Option<u64>) -> Self {
        self.retry_after_secs = retry_after_secs;
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResultItem {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub display_url: String,
    pub snippet: String,
    pub source: String,
    pub engine: SearchEngine,
    pub provider_rank: usize,
    pub resolved: bool,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchCacheInfo {
    pub status: String,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProviderHealthState {
    Healthy,
    Degraded,
    Blocked,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchTimeRangeInfo {
    pub requested: TimeRange,
    pub applied_by: Vec<SearchEngine>,
    pub ignored_by: Vec<SearchEngine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchProviderRunInfo {
    pub engine: SearchEngine,
    pub health: SearchProviderHealthState,
    pub skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u128>,
    pub result_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchProviderStatus {
    pub engine: SearchEngine,
    pub id: String,
    pub label: String,
    pub health: SearchProviderHealthState,
    pub built_in: bool,
    pub enabled_by_profile: bool,
    pub enabled: bool,
    pub configured: bool,
    pub requires_api_key: bool,
    pub requires_base_url: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub region: SearchRegion,
    pub language: SearchLanguage,
    pub provider_profile: WebSearchProviderProfile,
    pub reranker: WebSearchReranker,
    pub time_range: TimeRange,
    pub time_range_info: SearchTimeRangeInfo,
    pub engines_requested: Vec<SearchEngine>,
    pub engines_responded: Vec<SearchEngine>,
    pub engines_failed: Vec<SearchProviderFailure>,
    pub provider_health: Vec<SearchProviderRunInfo>,
    pub total_results: usize,
    pub results: Vec<SearchResultItem>,
    pub cache: SearchCacheInfo,
}
