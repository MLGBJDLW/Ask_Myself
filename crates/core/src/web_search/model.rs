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
}

impl SearchEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baidu => "baidu",
            Self::Sogou => "sogou",
            Self::Bing => "bing",
            Self::DuckDuckGo => "duckduckgo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "baidu" | "百度" => Some(Self::Baidu),
            "sogou" | "搜狗" => Some(Self::Sogou),
            "bing" | "必应" => Some(Self::Bing),
            "duckduckgo" | "duck_duck_go" | "ddg" => Some(Self::DuckDuckGo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchRegion {
    Auto,
    MainlandCn,
    Global,
}

impl Default for SearchRegion {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchLanguage {
    Auto,
    Zh,
    En,
}

impl Default for SearchLanguage {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange {
    Any,
    Day,
    Week,
    Month,
    Year,
}

impl Default for TimeRange {
    fn default() -> Self {
        Self::Any
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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchProviderFailure {
    pub engine: SearchEngine,
    pub code: String,
    pub message: String,
}

impl SearchProviderFailure {
    pub fn new(engine: SearchEngine, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            engine,
            code: code.into(),
            message: message.into(),
        }
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

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub region: SearchRegion,
    pub language: SearchLanguage,
    pub engines_requested: Vec<SearchEngine>,
    pub engines_responded: Vec<SearchEngine>,
    pub engines_failed: Vec<SearchProviderFailure>,
    pub total_results: usize,
    pub results: Vec<SearchResultItem>,
    pub cache: SearchCacheInfo,
}
