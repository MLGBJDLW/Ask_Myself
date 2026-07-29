//! WebSearchTool — native no-key public web search.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{stream::FuturesUnordered, StreamExt};
use reqwest::header::{ACCEPT, RETRY_AFTER};
use tokio::sync::OnceCell;

use crate::app_settings::{
    WebSearchConfig, WebSearchCustomProviderConfig, WebSearchCustomProviderPreset,
    WebSearchProviderMode,
};
use crate::db::Database;
use crate::error::CoreError;
use crate::web_search::{
    build_search_request, default_engines_for_profile, provider_for_engine, SearchCacheInfo,
    SearchEngine, SearchLanguage, SearchProviderContext, SearchProviderFailure,
    SearchProviderHealthState, SearchProviderRunInfo, SearchRegion, SearchRequest, SearchResponse,
    SearchResultItem, SearchTimeRangeInfo, WebSearchArgs, WebSearchProviderProfile,
    WebSearchProviderStatus, WebSearchReranker,
};

use super::{tool_contract_error_result, Tool, ToolCategory, ToolDef, ToolResult, TrustBoundary};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/web_search.json");
const SEARCH_SUCCESS_CACHE_TTL: Duration = Duration::from_secs(300);
const SEARCH_PARTIAL_CACHE_TTL: Duration = Duration::from_secs(120);
const SEARCH_EMPTY_CACHE_TTL: Duration = Duration::from_secs(30);
const PROVIDER_MIN_INTERVAL: Duration = Duration::from_secs(2);
const PROVIDER_DEGRADED_COOLDOWN: Duration = Duration::from_secs(30);
const PROVIDER_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(300);
const PROVIDER_BLOCKED_COOLDOWN: Duration = Duration::from_secs(600);
const PROVIDER_FAILURE_THRESHOLD: u32 = 3;
const REDIRECT_RESOLVE_MAX_RESULTS: usize = 4;
const REDIRECT_RESOLVE_MAX_HOPS: usize = 3;
const REDIRECT_RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
static SEARCH_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
static SEARCH_IN_FLIGHT: OnceLock<Mutex<HashMap<String, Arc<OnceCell<SearchExecution>>>>> =
    OnceLock::new();
static PROVIDER_HEALTH: OnceLock<Mutex<HashMap<ProviderRuntimeKey, ProviderRuntimeState>>> =
    OnceLock::new();

pub struct WebSearchTool;

#[derive(Clone)]
struct CacheEntry {
    response: SearchResponse,
    stored_at: Instant,
    ttl: Duration,
}

#[derive(Clone)]
pub(crate) struct SearchExecution {
    pub(crate) response: SearchResponse,
    pub(crate) all_failed: bool,
}

#[derive(Debug, Clone)]
struct ProviderRuntimeState {
    health: SearchProviderHealthState,
    consecutive_failures: u32,
    last_error_code: Option<String>,
    last_success_at: Option<Instant>,
    last_failure_at: Option<Instant>,
    next_request_at: Instant,
    next_retry_at: Option<Instant>,
}

#[derive(Clone)]
struct SearchRuntimeConfig {
    provider_mode: WebSearchProviderMode,
    custom_providers: Vec<WebSearchCustomProviderConfig>,
}

#[derive(Clone)]
enum SearchPlanItem {
    Native(SearchEngine),
    Custom(WebSearchCustomProviderConfig),
}

#[derive(Clone)]
struct SearchPlanEntry {
    index: usize,
    item: SearchPlanItem,
    engine: SearchEngine,
    runtime_key: ProviderRuntimeKey,
}

struct ProviderAttemptResult {
    entry: SearchPlanEntry,
    time_range_applied: bool,
    time_range_ignored: bool,
    outcome: ProviderAttemptOutcome,
}

enum ProviderAttemptOutcome {
    Success {
        results: Vec<SearchResultItem>,
        run_info: SearchProviderRunInfo,
    },
    Failure {
        failure: SearchProviderFailure,
        run_info: SearchProviderRunInfo,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderRuntimeKey {
    engine: SearchEngine,
    scope: ProviderRuntimeScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProviderRuntimeScope {
    Native,
    Custom {
        id: String,
        preset: WebSearchCustomProviderPreset,
        key_fingerprint: String,
        base_url: Option<String>,
    },
}

impl ProviderRuntimeKey {
    fn native(engine: SearchEngine) -> Self {
        Self {
            engine,
            scope: ProviderRuntimeScope::Native,
        }
    }

    fn custom(provider: &WebSearchCustomProviderConfig) -> Self {
        Self {
            engine: engine_for_custom_provider(provider),
            scope: ProviderRuntimeScope::Custom {
                id: provider.id.clone(),
                preset: provider.preset,
                key_fingerprint: provider_key_fingerprint(provider.api_key.trim()),
                base_url: provider.effective_base_url(),
            },
        }
    }
}

impl Default for ProviderRuntimeState {
    fn default() -> Self {
        Self {
            health: SearchProviderHealthState::Healthy,
            consecutive_failures: 0,
            last_error_code: None,
            last_success_at: None,
            last_failure_at: None,
            next_request_at: Instant::now(),
            next_retry_at: None,
        }
    }
}

fn provider_key_fingerprint(api_key: &str) -> String {
    if api_key.trim().is_empty() {
        return "anonymous".to_string();
    }
    let hash = blake3::hash(api_key.trim().as_bytes());
    hash.to_hex().as_str()[..16].to_string()
}

fn web_search_expected_format() -> serde_json::Value {
    serde_json::json!({
        "query": "focused natural-language search query",
        "limit": "integer from 1 to 20",
        "region": "auto | mainland_cn | global",
        "language": "auto | zh | en",
        "engines": ["optional built-in fallback subset: baidu, sogou, google, bing, duckduckgo"],
        "time_range": "any | day | week | month | year",
        "site": "optional domain such as example.com",
        "include_snippets": true,
        "provider_profile": "default | free | free_verified | max_evidence",
        "reranker": "auto | none | docs_first | research | news_balanced"
    })
}

fn merge_and_rank_results(
    results: Vec<SearchResultItem>,
    limit: usize,
    reranker: WebSearchReranker,
) -> Vec<SearchResultItem> {
    let mut seen = HashSet::new();
    let mut merged: Vec<(usize, SearchResultItem)> = Vec::new();
    for (index, result) in results.into_iter().enumerate() {
        let key = result.url.trim_end_matches('/').to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        merged.push((index, result));
    }

    if reranker != WebSearchReranker::None {
        merged.sort_by(|(left_index, left), (right_index, right)| {
            rerank_score(right, reranker)
                .cmp(&rerank_score(left, reranker))
                .then_with(|| left_index.cmp(right_index))
        });
    }

    merged
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(rank, (_, mut result))| {
            result.rank = rank + 1;
            result
        })
        .collect()
}

fn should_stop_after_success(
    request: &SearchRequest,
    result_count: usize,
    responded_count: usize,
) -> bool {
    if request.explicit_engines || should_force_full_profile(request.provider_profile) {
        return result_count >= request.limit;
    }
    result_count >= request.limit || (result_count > 0 && responded_count >= 2)
}

fn rerank_score(result: &SearchResultItem, reranker: WebSearchReranker) -> i32 {
    let host = result.source.to_ascii_lowercase();
    let url = result.url.to_ascii_lowercase();
    let text = format!(
        "{} {} {}",
        result.title.to_ascii_lowercase(),
        result.snippet.to_ascii_lowercase(),
        url
    );
    let mut score = 0i32;

    match reranker {
        WebSearchReranker::DocsFirst => {
            if host.starts_with("docs.") || url.contains("/docs") {
                score += 60;
            }
            if contains_any(
                &text,
                &[
                    "documentation",
                    "developer",
                    "developers",
                    "reference",
                    "api",
                    "sdk",
                    "guide",
                    "manual",
                    "release notes",
                ],
            ) {
                score += 28;
            }
            if officialish_host(&host) {
                score += 20;
            }
            if low_context_host(&host) {
                score -= 30;
            }
        }
        WebSearchReranker::Research => {
            if host.ends_with(".edu") || host.ends_with(".gov") {
                score += 50;
            }
            if contains_any(
                &host,
                &[
                    "arxiv.org",
                    "pubmed.ncbi.nlm.nih.gov",
                    "ncbi.nlm.nih.gov",
                    "nature.com",
                    "science.org",
                    "ieee.org",
                    "acm.org",
                    "springer.com",
                    "sciencedirect.com",
                    "semanticscholar.org",
                ],
            ) {
                score += 45;
            }
            if contains_any(
                &text,
                &[
                    "paper",
                    "study",
                    "research",
                    "journal",
                    "doi",
                    "clinical trial",
                    "preprint",
                    "proceedings",
                ],
            ) {
                score += 25;
            }
            if low_context_host(&host) {
                score -= 25;
            }
        }
        WebSearchReranker::NewsBalanced => {
            if contains_any(
                &text,
                &[
                    "news",
                    "report",
                    "reported",
                    "analysis",
                    "press release",
                    "updated",
                ],
            ) {
                score += 22;
            }
            if contains_any(
                &host,
                &[
                    "reuters.com",
                    "apnews.com",
                    "bbc.com",
                    "bloomberg.com",
                    "wsj.com",
                    "ft.com",
                    "theverge.com",
                    "techcrunch.com",
                    "36kr.com",
                    "caixin.com",
                ],
            ) {
                score += 20;
            }
            if low_context_host(&host) {
                score -= 20;
            }
        }
        WebSearchReranker::Auto | WebSearchReranker::None => {}
    }

    score + (20usize.saturating_sub(result.provider_rank.min(20))) as i32
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn officialish_host(host: &str) -> bool {
    host.ends_with(".gov")
        || host.ends_with(".edu")
        || host.contains("github.com")
        || host.contains("developer.")
        || host.contains("developers.")
}

fn low_context_host(host: &str) -> bool {
    contains_any(
        host,
        &[
            "pinterest.",
            "facebook.",
            "instagram.",
            "tiktok.",
            "quora.",
            "reddit.",
            "medium.com",
            "youtube.",
        ],
    )
}

fn should_force_full_profile(provider_profile: WebSearchProviderProfile) -> bool {
    matches!(
        provider_profile,
        WebSearchProviderProfile::FreeVerified | WebSearchProviderProfile::MaxEvidence
    )
}

fn apply_app_config_defaults(mut args: WebSearchArgs, config: &WebSearchConfig) -> WebSearchArgs {
    if args.provider_profile.is_some() && args.reranker.is_some() {
        return args;
    }

    if args.provider_profile.is_none() {
        args.provider_profile = Some(config.provider_profile);
    }
    if args.reranker.is_none() {
        args.reranker = Some(config.reranker);
    }
    args
}

fn runtime_config_from_app_config(config: &WebSearchConfig) -> SearchRuntimeConfig {
    let mut custom_providers = config
        .custom_providers
        .iter()
        .filter(|provider| provider.enabled && provider.is_configured())
        .cloned()
        .collect::<Vec<_>>();
    custom_providers.sort_by_key(|provider| provider.priority);
    SearchRuntimeConfig {
        provider_mode: config.provider_mode,
        custom_providers,
    }
}

fn engine_for_custom_provider(provider: &WebSearchCustomProviderConfig) -> SearchEngine {
    match provider.preset {
        WebSearchCustomProviderPreset::Brave => SearchEngine::Brave,
        WebSearchCustomProviderPreset::Tavily => SearchEngine::Tavily,
        WebSearchCustomProviderPreset::AnySearch => SearchEngine::AnySearch,
        WebSearchCustomProviderPreset::SerpApiGoogle => SearchEngine::SerpApiGoogle,
        WebSearchCustomProviderPreset::Searxng => SearchEngine::Searxng,
    }
}

fn cache_key_for_runtime(request: &SearchRequest, runtime: &SearchRuntimeConfig) -> String {
    let engines = request
        .engines
        .iter()
        .map(|engine| engine.as_str())
        .collect::<Vec<_>>();
    let custom = runtime
        .custom_providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "preset": provider.preset,
                "baseUrl": provider.effective_base_url(),
                "keyFingerprint": provider_key_fingerprint(provider.api_key.trim()),
                "priority": provider.priority,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "query": request.effective_query,
        "limit": request.limit,
        "region": request.region,
        "language": request.language,
        "engines": engines,
        "timeRange": request.time_range,
        "site": request.site,
        "includeSnippets": request.include_snippets,
        "providerProfile": request.provider_profile,
        "reranker": request.reranker,
        "providerMode": runtime.provider_mode,
        "customProviders": custom,
    })
    .to_string()
}

fn build_provider_plan(
    request: &SearchRequest,
    runtime: &SearchRuntimeConfig,
) -> Vec<SearchPlanItem> {
    let native = request
        .engines
        .iter()
        .copied()
        .map(SearchPlanItem::Native)
        .collect::<Vec<_>>();
    let custom = runtime
        .custom_providers
        .iter()
        .cloned()
        .map(SearchPlanItem::Custom)
        .collect::<Vec<_>>();

    match runtime.provider_mode {
        WebSearchProviderMode::BuiltInFirst => native.into_iter().chain(custom).collect(),
        WebSearchProviderMode::CustomFirst => custom.into_iter().chain(native).collect(),
        WebSearchProviderMode::CustomOnly => custom,
    }
}

fn engine_for_plan_item(plan_item: &SearchPlanItem) -> SearchEngine {
    match plan_item {
        SearchPlanItem::Native(engine) => *engine,
        SearchPlanItem::Custom(provider) => engine_for_custom_provider(provider),
    }
}

fn provider_parallelism(request: &SearchRequest) -> usize {
    if request.explicit_engines || should_force_full_profile(request.provider_profile) {
        3
    } else {
        2
    }
}

fn same_provider_tier(left: &SearchPlanItem, right: &SearchPlanItem) -> bool {
    matches!(
        (left, right),
        (SearchPlanItem::Native(_), SearchPlanItem::Native(_))
            | (SearchPlanItem::Custom(_), SearchPlanItem::Custom(_))
    )
}

fn next_provider_wave_end(
    provider_plan: &[SearchPlanItem],
    start: usize,
    max_parallel: usize,
) -> usize {
    if start >= provider_plan.len() {
        return start;
    }

    let mut end = start + 1;
    let max_end = provider_plan.len().min(start + max_parallel.max(1));
    while end < max_end && same_provider_tier(&provider_plan[start], &provider_plan[end]) {
        end += 1;
    }
    end
}

fn should_continue_to_fallback_provider(
    request: &SearchRequest,
    plan_item: &SearchPlanItem,
    runtime: &SearchRuntimeConfig,
    result_count: usize,
) -> bool {
    if result_count >= request.limit {
        return false;
    }
    matches!(
        (runtime.provider_mode, plan_item),
        (
            WebSearchProviderMode::BuiltInFirst,
            SearchPlanItem::Native(_)
        ) | (
            WebSearchProviderMode::CustomFirst,
            SearchPlanItem::Custom(_)
        )
    ) && !request.explicit_engines
        && !should_force_full_profile(request.provider_profile)
}

fn format_engine_list(engines: &[SearchEngine]) -> String {
    engines
        .iter()
        .map(|engine| engine.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn cached_response(key: &str) -> Option<SearchResponse> {
    let cache = SEARCH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    let expired = cache
        .get(key)
        .map(|entry| entry.stored_at.elapsed() > entry.ttl)?;
    if expired {
        cache.remove(key);
        return None;
    }
    let entry = cache.get(key)?;
    let mut response = entry.response.clone();
    response.cache = SearchCacheInfo {
        status: "hit".to_string(),
        ttl_seconds: entry
            .ttl
            .checked_sub(entry.stored_at.elapsed())
            .unwrap_or_default()
            .as_secs(),
    };
    Some(response)
}

fn cache_ttl_for_response(response: &SearchResponse) -> Option<Duration> {
    if !response.results.is_empty() {
        return Some(if response.engines_failed.is_empty() {
            SEARCH_SUCCESS_CACHE_TTL
        } else {
            SEARCH_PARTIAL_CACHE_TTL
        });
    }

    if response.engines_failed.is_empty()
        && !response.engines_responded.is_empty()
        && response.engines_responded.len() == response.engines_requested.len()
    {
        return Some(SEARCH_EMPTY_CACHE_TTL);
    }

    None
}

fn store_response(key: String, response: &SearchResponse) {
    let Some(ttl) = cache_ttl_for_response(response) else {
        return;
    };
    let cache = SEARCH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        let mut stored = response.clone();
        stored.cache = SearchCacheInfo {
            status: "hit".to_string(),
            ttl_seconds: ttl.as_secs(),
        };
        cache.insert(
            key,
            CacheEntry {
                response: stored,
                stored_at: Instant::now(),
                ttl,
            },
        );
    }
}

fn seconds_until(deadline: Instant) -> Option<u64> {
    let now = Instant::now();
    (deadline > now).then(|| deadline.duration_since(now).as_secs().max(1))
}

fn provider_failure_cooldown(
    failure: &SearchProviderFailure,
    consecutive_failures: u32,
) -> Option<Duration> {
    match failure.code.as_str() {
        "captcha" | "blocked" | "auth_failed" => Some(PROVIDER_BLOCKED_COOLDOWN),
        "rate_limited" => Some(
            failure
                .retry_after_secs
                .map(Duration::from_secs)
                .unwrap_or(PROVIDER_RATE_LIMIT_COOLDOWN),
        ),
        "request_failed" | "http_status" | "body_read_failed" | "redirect_resolve_failed"
            if consecutive_failures >= PROVIDER_FAILURE_THRESHOLD =>
        {
            Some(PROVIDER_DEGRADED_COOLDOWN)
        }
        _ => None,
    }
}

async fn prepare_provider_call(
    key: ProviderRuntimeKey,
) -> Result<SearchProviderHealthState, (SearchProviderFailure, SearchProviderRunInfo)> {
    let engine = key.engine;
    loop {
        let wait = {
            let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
            let mut health = health.lock().map_err(|_| {
                let failure = SearchProviderFailure::new(
                    engine,
                    "health_state_unavailable",
                    "Provider health state lock is unavailable",
                );
                let run = SearchProviderRunInfo {
                    engine,
                    health: SearchProviderHealthState::Degraded,
                    skipped: true,
                    latency_ms: None,
                    result_count: 0,
                    error_code: Some(failure.code.clone()),
                    next_retry_seconds: None,
                };
                (failure, run)
            })?;
            let state = health.entry(key.clone()).or_default();
            let now = Instant::now();
            if let Some(next_retry_at) = state.next_retry_at {
                if next_retry_at > now {
                    let retry_after_secs = seconds_until(next_retry_at);
                    let failure = SearchProviderFailure::new(
                        engine,
                        "circuit_open",
                        "Provider is cooling down after recent failures",
                    )
                    .with_retry_after(retry_after_secs);
                    let run = SearchProviderRunInfo {
                        engine,
                        health: state.health,
                        skipped: true,
                        latency_ms: None,
                        result_count: 0,
                        error_code: Some(failure.code.clone()),
                        next_retry_seconds: retry_after_secs,
                    };
                    return Err((failure, run));
                }
                state.next_retry_at = None;
                if state.health == SearchProviderHealthState::Blocked {
                    state.health = SearchProviderHealthState::Degraded;
                }
            }

            if state.next_request_at > now {
                Some(state.next_request_at.duration_since(now))
            } else {
                state.next_request_at = now + PROVIDER_MIN_INTERVAL;
                return Ok(state.health);
            }
        };

        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
}

fn record_provider_success(key: &ProviderRuntimeKey) -> SearchProviderHealthState {
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut health) = health.lock() else {
        return SearchProviderHealthState::Healthy;
    };
    let state = health.entry(key.clone()).or_default();
    state.health = SearchProviderHealthState::Healthy;
    state.consecutive_failures = 0;
    state.last_error_code = None;
    state.last_success_at = Some(Instant::now());
    state.next_retry_at = None;
    state.health
}

fn record_provider_failure(
    key: &ProviderRuntimeKey,
    failure: &SearchProviderFailure,
) -> (SearchProviderHealthState, Option<u64>) {
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut health) = health.lock() else {
        return (SearchProviderHealthState::Degraded, None);
    };
    let state = health.entry(key.clone()).or_default();
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_error_code = Some(failure.code.clone());
    state.last_failure_at = Some(Instant::now());
    if let Some(cooldown) = provider_failure_cooldown(failure, state.consecutive_failures) {
        state.next_retry_at = Some(Instant::now() + cooldown);
        state.health = if matches!(
            failure.code.as_str(),
            "captcha" | "blocked" | "rate_limited"
        ) {
            SearchProviderHealthState::Blocked
        } else {
            SearchProviderHealthState::Degraded
        };
    } else if state.consecutive_failures > 0 {
        state.health = SearchProviderHealthState::Degraded;
    }
    let next_retry_seconds = state.next_retry_at.and_then(seconds_until);
    (state.health, next_retry_seconds)
}

fn run_info_for_success(
    engine: SearchEngine,
    key: &ProviderRuntimeKey,
    latency_ms: u128,
    result_count: usize,
) -> SearchProviderRunInfo {
    SearchProviderRunInfo {
        engine,
        health: record_provider_success(key),
        skipped: false,
        latency_ms: Some(latency_ms),
        result_count,
        error_code: None,
        next_retry_seconds: None,
    }
}

fn run_info_for_failure(
    engine: SearchEngine,
    key: &ProviderRuntimeKey,
    latency_ms: u128,
    failure: &SearchProviderFailure,
) -> SearchProviderRunInfo {
    let (health, next_retry_seconds) = record_provider_failure(key, failure);
    SearchProviderRunInfo {
        engine,
        health,
        skipped: false,
        latency_ms: Some(latency_ms),
        result_count: 0,
        error_code: Some(failure.code.clone()),
        next_retry_seconds,
    }
}

fn format_response(call_id: &str, response: SearchResponse, all_failed: bool) -> ToolResult {
    let mut text = format!(
        "Web search for: {}\nRegion: {:?}; language: {:?}; profile: {}; reranker: {}; engines requested: {}.\n",
        response.query,
        response.region,
        response.language,
        response.provider_profile.as_str(),
        response.reranker.as_str(),
        format_engine_list(&response.engines_requested)
    );

    if !response.engines_failed.is_empty() {
        text.push_str("Provider notes:\n");
        for failure in &response.engines_failed {
            let retry_after = failure
                .retry_after_secs
                .map(|seconds| format!("; retry after {seconds}s"))
                .unwrap_or_default();
            text.push_str(&format!(
                "- {}: {} ({}{retry_after})\n",
                failure.engine.as_str(),
                failure.code,
                failure.message
            ));
        }
    }
    if !response.time_range_info.ignored_by.is_empty() {
        text.push_str(&format!(
            "Time range note: {:?} was not applied by {}.\n",
            response.time_range_info.requested,
            format_engine_list(&response.time_range_info.ignored_by)
        ));
    }

    if response.results.is_empty() {
        text.push_str("\nNo readable search results were returned. Try one more focused query or a different engine only if the task still needs web evidence.");
    } else {
        text.push_str("\nUse fetch_url on authoritative result URLs before making factual claims. Search snippets are candidates, not citations.\n\n");
        for result in &response.results {
            text.push_str(&format!(
                "{}. {}\nURL: {}\nSource: {}\nEngine: {} (provider rank {})\n",
                result.rank,
                result.title,
                result.url,
                result.source,
                result.engine.as_str(),
                result.provider_rank,
            ));
            if !result.snippet.is_empty() {
                text.push_str(&format!("Snippet: {}\n", result.snippet));
            }
            if !result.resolved {
                text.push_str("Note: this URL is a search-provider redirect; fetch_url will validate the redirect before reading content.\n");
            }
            text.push('\n');
        }
    }

    let artifacts = serde_json::json!({
        "kind": "webSearchResults",
        "search": response,
        "trustBoundary": TrustBoundary {
            origin: "public_web_search".to_string(),
            authority: "candidate_evidence".to_string(),
            visibility: "current_chat".to_string(),
            mutability: "read_only".to_string(),
            externality: "external_network".to_string(),
            can_instruct: false,
        },
        "contract": {
            "sourceRole": "candidate",
            "authority": "search_result",
            "canInstruct": false,
            "note": "Search result snippets are discovery aids. Fetch authoritative result pages before citing facts."
        }
    });

    ToolResult {
        call_id: call_id.to_string(),
        content: text,
        is_error: all_failed,
        artifacts: Some(artifacts),
    }
}

fn build_search_client(
    redirect_policy: reqwest::redirect::Policy,
) -> Result<reqwest::Client, CoreError> {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(12))
        .redirect(redirect_policy)
        .build()
        .map_err(|e| CoreError::Internal(format!("Failed to build web search client: {e}")))
}

fn custom_provider_failure(
    provider: &WebSearchCustomProviderConfig,
    code: impl Into<String>,
    message: impl Into<String>,
) -> SearchProviderFailure {
    SearchProviderFailure::new(engine_for_custom_provider(provider), code, message)
}

fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

async fn fetch_custom_json(
    provider: &WebSearchCustomProviderConfig,
    request_builder: reqwest::RequestBuilder,
) -> Result<serde_json::Value, SearchProviderFailure> {
    let response = request_builder.send().await.map_err(|e| {
        custom_provider_failure(
            provider,
            "request_failed",
            format!("{} request failed: {e}", provider.name),
        )
    })?;
    let status = response.status();
    let retry_after = retry_after_secs(response.headers());
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(custom_provider_failure(
            provider,
            "auth_failed",
            format!(
                "{} returned HTTP {status}; check the configured API key or endpoint",
                provider.name
            ),
        ));
    }
    if status.as_u16() == 429 {
        return Err(custom_provider_failure(
            provider,
            "rate_limited",
            format!("{} returned HTTP {status}", provider.name),
        )
        .with_retry_after(retry_after));
    }
    if !status.is_success() {
        return Err(custom_provider_failure(
            provider,
            "http_status",
            format!("{} returned HTTP {status}", provider.name),
        ));
    }
    response.json::<serde_json::Value>().await.map_err(|e| {
        custom_provider_failure(
            provider,
            "body_read_failed",
            format!("{} response JSON could not be read: {e}", provider.name),
        )
    })
}

fn value_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn first_value_string(value: &serde_json::Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|key| value_string(value, key))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn custom_result_item(
    provider: &WebSearchCustomProviderConfig,
    rank: usize,
    title: String,
    url: String,
    snippet: String,
) -> Option<SearchResultItem> {
    let url_info = crate::web_search::providers::public_url_info(&url)?;
    Some(SearchResultItem {
        rank: 0,
        title: if title.trim().is_empty() {
            url_info.source.clone()
        } else {
            crate::web_search::providers::normalize_space(&title)
        },
        url: url_info.url,
        display_url: url_info.display_url,
        snippet: crate::web_search::providers::normalize_space(&snippet),
        source: url_info.source,
        engine: engine_for_custom_provider(provider),
        provider_rank: rank,
        resolved: true,
        confidence: "high".to_string(),
    })
}

fn language_code(language: SearchLanguage) -> &'static str {
    match language {
        SearchLanguage::Zh => "zh",
        SearchLanguage::En | SearchLanguage::Auto => "en",
    }
}

fn country_code(region: SearchRegion) -> &'static str {
    match region {
        SearchRegion::MainlandCn => "cn",
        SearchRegion::Global | SearchRegion::Auto => "us",
    }
}

fn time_range_for_serpapi(time_range: crate::web_search::TimeRange) -> Option<&'static str> {
    match time_range {
        crate::web_search::TimeRange::Day => Some("qdr:d"),
        crate::web_search::TimeRange::Week => Some("qdr:w"),
        crate::web_search::TimeRange::Month => Some("qdr:m"),
        crate::web_search::TimeRange::Year => Some("qdr:y"),
        crate::web_search::TimeRange::Any => None,
    }
}

fn time_range_for_brave(time_range: crate::web_search::TimeRange) -> Option<&'static str> {
    match time_range {
        crate::web_search::TimeRange::Day => Some("pd"),
        crate::web_search::TimeRange::Week => Some("pw"),
        crate::web_search::TimeRange::Month => Some("pm"),
        crate::web_search::TimeRange::Year => Some("py"),
        crate::web_search::TimeRange::Any => None,
    }
}

fn time_range_for_tavily(time_range: crate::web_search::TimeRange) -> Option<&'static str> {
    match time_range {
        crate::web_search::TimeRange::Day => Some("day"),
        crate::web_search::TimeRange::Week => Some("week"),
        crate::web_search::TimeRange::Month => Some("month"),
        crate::web_search::TimeRange::Year => Some("year"),
        crate::web_search::TimeRange::Any => None,
    }
}

fn time_range_for_anysearch(time_range: crate::web_search::TimeRange) -> Option<&'static str> {
    match time_range {
        crate::web_search::TimeRange::Day => Some("day"),
        crate::web_search::TimeRange::Week => Some("week"),
        crate::web_search::TimeRange::Month => Some("month"),
        crate::web_search::TimeRange::Year => Some("year"),
        crate::web_search::TimeRange::Any => None,
    }
}

fn time_range_for_searxng(time_range: crate::web_search::TimeRange) -> Option<&'static str> {
    match time_range {
        crate::web_search::TimeRange::Day => Some("day"),
        crate::web_search::TimeRange::Month => Some("month"),
        crate::web_search::TimeRange::Year => Some("year"),
        crate::web_search::TimeRange::Any | crate::web_search::TimeRange::Week => None,
    }
}

async fn search_brave_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    let Some(base_url) = provider.effective_base_url() else {
        return Err(custom_provider_failure(
            provider,
            "not_configured",
            "Brave Search API endpoint is not configured",
        ));
    };
    let mut url = crate::tools::fetch_url_tool::validate_url_for_fetch(&base_url)
        .await
        .map_err(|e| custom_provider_failure(provider, "invalid_base_url", e))?;
    url.query_pairs_mut()
        .append_pair("q", &request.effective_query)
        .append_pair("count", &request.limit.min(20).to_string())
        .append_pair("country", country_code(request.region))
        .append_pair("search_lang", language_code(request.language));
    if let Some(freshness) = time_range_for_brave(request.time_range) {
        url.query_pairs_mut().append_pair("freshness", freshness);
    }

    let json = fetch_custom_json(
        provider,
        client
            .get(url)
            .header(ACCEPT, "application/json")
            .header("X-Subscription-Token", provider.api_key.trim()),
    )
    .await?;
    let results = json
        .get("web")
        .and_then(|value| value.get("results"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            custom_result_item(
                provider,
                index + 1,
                value_string(item, "title"),
                value_string(item, "url"),
                value_string(item, "description"),
            )
        })
        .collect();
    Ok(results)
}

async fn search_tavily_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    let Some(base_url) = provider.effective_base_url() else {
        return Err(custom_provider_failure(
            provider,
            "not_configured",
            "Tavily Search endpoint is not configured",
        ));
    };
    let url = crate::tools::fetch_url_tool::validate_url_for_fetch(&base_url)
        .await
        .map_err(|e| custom_provider_failure(provider, "invalid_base_url", e))?;
    let mut body = serde_json::json!({
        "query": request.effective_query,
        "search_depth": "basic",
        "max_results": request.limit.min(20),
        "include_answer": false,
        "include_images": false,
    });
    if let Some(time_range) = time_range_for_tavily(request.time_range) {
        if let Some(body) = body.as_object_mut() {
            body.insert("time_range".to_string(), serde_json::json!(time_range));
        }
    }
    let json = fetch_custom_json(
        provider,
        client
            .post(url)
            .header(ACCEPT, "application/json")
            .bearer_auth(provider.api_key.trim())
            .json(&body),
    )
    .await?;
    let results = json
        .get("results")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            custom_result_item(
                provider,
                index + 1,
                value_string(item, "title"),
                value_string(item, "url"),
                value_string(item, "content"),
            )
        })
        .collect();
    Ok(results)
}

async fn search_anysearch_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    let Some(base_url) = provider.effective_base_url() else {
        return Err(custom_provider_failure(
            provider,
            "not_configured",
            "AnySearch endpoint is not configured",
        ));
    };
    let url = crate::tools::fetch_url_tool::validate_url_for_fetch(&base_url)
        .await
        .map_err(|e| custom_provider_failure(provider, "invalid_base_url", e))?;
    let mut body = serde_json::json!({
        "query": request.effective_query,
        "max_results": request.limit.min(100),
        "language": match request.language {
            SearchLanguage::Zh => "zh-CN",
            SearchLanguage::En | SearchLanguage::Auto => "en",
        },
        "zone": match request.region {
            SearchRegion::MainlandCn => "cn",
            SearchRegion::Global | SearchRegion::Auto => "intl",
        },
    });
    if let Some(freshness) = time_range_for_anysearch(request.time_range) {
        if let Some(body) = body.as_object_mut() {
            body.insert(
                "constraint".to_string(),
                serde_json::json!({ "freshness": freshness }),
            );
        }
    }

    let mut request_builder = client
        .post(url)
        .header(ACCEPT, "application/json")
        .json(&body);
    let api_key = provider.api_key.trim();
    if !api_key.is_empty() {
        request_builder = request_builder.bearer_auth(api_key);
    }

    let json = fetch_custom_json(provider, request_builder).await?;
    let results = json
        .get("results")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            custom_result_item(
                provider,
                index + 1,
                value_string(item, "title"),
                value_string(item, "url"),
                first_value_string(item, &["description", "content", "raw_content"]),
            )
        })
        .collect();
    Ok(results)
}

async fn search_serpapi_google_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    let Some(base_url) = provider.effective_base_url() else {
        return Err(custom_provider_failure(
            provider,
            "not_configured",
            "SerpAPI endpoint is not configured",
        ));
    };
    let mut url = crate::tools::fetch_url_tool::validate_url_for_fetch(&base_url)
        .await
        .map_err(|e| custom_provider_failure(provider, "invalid_base_url", e))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("engine", "google")
            .append_pair("q", &request.effective_query)
            .append_pair("api_key", provider.api_key.trim())
            .append_pair("num", &request.limit.min(20).to_string())
            .append_pair("hl", language_code(request.language))
            .append_pair("gl", country_code(request.region));
        if let Some(tbs) = time_range_for_serpapi(request.time_range) {
            query.append_pair("tbs", tbs);
        }
    }

    let json =
        fetch_custom_json(provider, client.get(url).header(ACCEPT, "application/json")).await?;
    let results = json
        .get("organic_results")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            custom_result_item(
                provider,
                index + 1,
                value_string(item, "title"),
                value_string(item, "link"),
                value_string(item, "snippet"),
            )
        })
        .collect();
    Ok(results)
}

async fn search_searxng_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    let Some(base_url) = provider.effective_base_url() else {
        return Err(custom_provider_failure(
            provider,
            "not_configured",
            "SearXNG instance URL is not configured",
        ));
    };
    let mut url = crate::tools::fetch_url_tool::validate_url_for_fetch(&base_url)
        .await
        .map_err(|e| custom_provider_failure(provider, "invalid_base_url", e))?;
    if !url.path().trim_end_matches('/').ends_with("/search") {
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}/search"));
    }
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("q", &request.effective_query)
            .append_pair("format", "json")
            .append_pair("pageno", "1")
            .append_pair("language", language_code(request.language));
        if let Some(time_range) = time_range_for_searxng(request.time_range) {
            query.append_pair("time_range", time_range);
        }
    }

    let json =
        fetch_custom_json(provider, client.get(url).header(ACCEPT, "application/json")).await?;
    let results = json
        .get("results")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .take(request.limit.min(20))
        .enumerate()
        .filter_map(|(index, item)| {
            custom_result_item(
                provider,
                index + 1,
                value_string(item, "title"),
                value_string(item, "url"),
                value_string(item, "content"),
            )
        })
        .collect();
    Ok(results)
}

async fn search_custom_provider(
    provider: &WebSearchCustomProviderConfig,
    request: &SearchRequest,
    client: &reqwest::Client,
) -> Result<Vec<SearchResultItem>, SearchProviderFailure> {
    match provider.preset {
        WebSearchCustomProviderPreset::Brave => {
            search_brave_provider(provider, request, client).await
        }
        WebSearchCustomProviderPreset::Tavily => {
            search_tavily_provider(provider, request, client).await
        }
        WebSearchCustomProviderPreset::AnySearch => {
            search_anysearch_provider(provider, request, client).await
        }
        WebSearchCustomProviderPreset::SerpApiGoogle => {
            search_serpapi_google_provider(provider, request, client).await
        }
        WebSearchCustomProviderPreset::Searxng => {
            search_searxng_provider(provider, request, client).await
        }
    }
}

async fn resolve_search_redirects(
    redirect_client: &reqwest::Client,
    results: &mut [SearchResultItem],
) -> Vec<SearchProviderFailure> {
    let mut failures = Vec::new();
    for result in results
        .iter_mut()
        .filter(|result| !result.resolved)
        .take(REDIRECT_RESOLVE_MAX_RESULTS)
    {
        let engine = result.engine;
        let url = result.url.clone();
        match tokio::time::timeout(
            REDIRECT_RESOLVE_TIMEOUT,
            crate::web_search::providers::resolve_search_redirect_target(
                redirect_client,
                engine,
                &url,
                REDIRECT_RESOLVE_MAX_HOPS,
            ),
        )
        .await
        {
            Ok(Ok(Some(resolved))) => {
                result.url = resolved.url;
                result.display_url = resolved.display_url;
                result.source = resolved.source;
                result.resolved = true;
                result.confidence = "medium".to_string();
            }
            Ok(Ok(None)) => {}
            Ok(Err(failure)) => failures.push(failure),
            Err(_) => failures.push(SearchProviderFailure::new(
                engine,
                "redirect_resolve_timeout",
                format!(
                    "Search redirect resolver exceeded {}s",
                    REDIRECT_RESOLVE_TIMEOUT.as_secs()
                ),
            )),
        }
    }
    failures
}

fn runtime_key_for_plan_item(plan_item: &SearchPlanItem) -> ProviderRuntimeKey {
    match plan_item {
        SearchPlanItem::Native(engine) => ProviderRuntimeKey::native(*engine),
        SearchPlanItem::Custom(provider) => ProviderRuntimeKey::custom(provider),
    }
}

fn custom_provider_supports_time_range(
    preset: WebSearchCustomProviderPreset,
    time_range: crate::web_search::TimeRange,
) -> bool {
    match preset {
        WebSearchCustomProviderPreset::Brave => time_range_for_brave(time_range).is_some(),
        WebSearchCustomProviderPreset::Tavily => time_range_for_tavily(time_range).is_some(),
        WebSearchCustomProviderPreset::AnySearch => time_range_for_anysearch(time_range).is_some(),
        WebSearchCustomProviderPreset::SerpApiGoogle => {
            time_range_for_serpapi(time_range).is_some()
        }
        WebSearchCustomProviderPreset::Searxng => time_range_for_searxng(time_range).is_some(),
    }
}

fn plan_item_time_range_support(
    plan_item: &SearchPlanItem,
    request: &SearchRequest,
) -> (bool, bool) {
    if request.time_range == crate::web_search::TimeRange::Any {
        return (false, false);
    }

    match plan_item {
        SearchPlanItem::Native(engine) => {
            if provider_for_engine(*engine).supports_time_range(request.time_range) {
                (true, false)
            } else {
                (false, true)
            }
        }
        SearchPlanItem::Custom(provider) => {
            if custom_provider_supports_time_range(provider.preset, request.time_range) {
                (true, false)
            } else {
                (false, true)
            }
        }
    }
}

async fn execute_provider_attempt(
    request: SearchRequest,
    client: reqwest::Client,
    entry: SearchPlanEntry,
) -> ProviderAttemptResult {
    let (time_range_applied, time_range_ignored) =
        plan_item_time_range_support(&entry.item, &request);

    match prepare_provider_call(entry.runtime_key.clone()).await {
        Ok(_) => {}
        Err((failure, run_info)) => {
            return ProviderAttemptResult {
                entry,
                time_range_applied,
                time_range_ignored,
                outcome: ProviderAttemptOutcome::Failure { failure, run_info },
            };
        }
    }

    let started_at = Instant::now();
    let search_result = match &entry.item {
        SearchPlanItem::Native(engine) => {
            let ctx = SearchProviderContext { client: &client };
            provider_for_engine(*engine).search(&request, &ctx).await
        }
        SearchPlanItem::Custom(provider) => {
            search_custom_provider(provider, &request, &client).await
        }
    };

    match search_result {
        Ok(mut results) => {
            let latency_ms = started_at.elapsed().as_millis();
            if !request.include_snippets {
                for result in &mut results {
                    result.snippet.clear();
                }
            }
            let result_count = results.len();
            let run_info =
                run_info_for_success(entry.engine, &entry.runtime_key, latency_ms, result_count);
            ProviderAttemptResult {
                entry,
                time_range_applied,
                time_range_ignored,
                outcome: ProviderAttemptOutcome::Success { results, run_info },
            }
        }
        Err(failure) => {
            let latency_ms = started_at.elapsed().as_millis();
            let run_info =
                run_info_for_failure(entry.engine, &entry.runtime_key, latency_ms, &failure);
            ProviderAttemptResult {
                entry,
                time_range_applied,
                time_range_ignored,
                outcome: ProviderAttemptOutcome::Failure { failure, run_info },
            }
        }
    }
}

async fn execute_search_request(
    request: SearchRequest,
    runtime: SearchRuntimeConfig,
    client: reqwest::Client,
    redirect_client: reqwest::Client,
) -> SearchExecution {
    let provider_plan = build_provider_plan(&request, &runtime);

    let mut raw_results = Vec::new();
    let mut engines_responded = Vec::new();
    let mut engines_failed: Vec<SearchProviderFailure> = Vec::new();
    let mut provider_health = Vec::new();
    let mut time_range_applied_by = Vec::new();
    let mut time_range_ignored_by = Vec::new();
    let mut attempted = 0usize;
    let mut engines_requested = Vec::new();
    let max_parallel = provider_parallelism(&request);
    let mut next_index = 0usize;

    while next_index < provider_plan.len() {
        let wave_end = next_provider_wave_end(&provider_plan, next_index, max_parallel);
        if wave_end <= next_index {
            break;
        }

        let mut wave = FuturesUnordered::new();
        for (index, item) in provider_plan
            .iter()
            .enumerate()
            .take(wave_end)
            .skip(next_index)
        {
            let item = item.clone();
            let entry = SearchPlanEntry {
                index,
                engine: engine_for_plan_item(&item),
                runtime_key: runtime_key_for_plan_item(&item),
                item,
            };
            wave.push(execute_provider_attempt(
                request.clone(),
                client.clone(),
                entry,
            ));
        }

        let mut attempts = Vec::new();
        while let Some(attempt) = wave.next().await {
            attempts.push(attempt);
        }
        attempts.sort_by_key(|attempt| attempt.entry.index);

        let mut last_plan_item = None;
        for attempt in attempts {
            attempted += 1;
            engines_requested.push(attempt.entry.engine);
            if attempt.time_range_applied {
                time_range_applied_by.push(attempt.entry.engine);
            }
            if attempt.time_range_ignored {
                time_range_ignored_by.push(attempt.entry.engine);
            }
            last_plan_item = Some(attempt.entry.item.clone());

            match attempt.outcome {
                ProviderAttemptOutcome::Success {
                    mut results,
                    run_info,
                } => {
                    engines_responded.push(attempt.entry.engine);
                    raw_results.append(&mut results);
                    provider_health.push(run_info);
                }
                ProviderAttemptOutcome::Failure { failure, run_info } => {
                    engines_failed.push(failure);
                    provider_health.push(run_info);
                }
            }
        }
        next_index = wave_end;

        if should_stop_after_success(&request, raw_results.len(), engines_responded.len()) {
            if last_plan_item.is_some_and(|plan_item| {
                should_continue_to_fallback_provider(
                    &request,
                    &plan_item,
                    &runtime,
                    raw_results.len(),
                )
            }) {
                continue;
            }
            break;
        }
    }

    let mut results = merge_and_rank_results(raw_results, request.limit, request.reranker);
    let redirect_failures = resolve_search_redirects(&redirect_client, &mut results).await;
    results = merge_and_rank_results(results, request.limit, request.reranker);
    engines_failed.extend(redirect_failures);

    let mut response = SearchResponse {
        query: request.query.clone(),
        region: request.region,
        language: request.language,
        provider_profile: request.provider_profile,
        reranker: request.reranker,
        time_range: request.time_range,
        time_range_info: SearchTimeRangeInfo {
            requested: request.time_range,
            applied_by: time_range_applied_by,
            ignored_by: time_range_ignored_by,
        },
        engines_requested: engines_requested.into_iter().take(attempted).collect(),
        engines_responded,
        engines_failed,
        provider_health,
        total_results: 0,
        results,
        cache: SearchCacheInfo {
            status: "miss".to_string(),
            ttl_seconds: SEARCH_SUCCESS_CACHE_TTL.as_secs(),
        },
    };
    response.total_results = response.results.len();
    if let Some(ttl) = cache_ttl_for_response(&response) {
        response.cache = SearchCacheInfo {
            status: "miss".to_string(),
            ttl_seconds: ttl.as_secs(),
        };
    } else {
        response.cache = SearchCacheInfo {
            status: "bypass".to_string(),
            ttl_seconds: 0,
        };
    }

    let all_failed = response.results.is_empty() && response.engines_responded.is_empty();

    SearchExecution {
        response,
        all_failed,
    }
}

async fn execute_with_singleflight(
    request: SearchRequest,
    runtime: SearchRuntimeConfig,
    cache_key: String,
) -> Result<SearchExecution, CoreError> {
    let client = build_search_client(reqwest::redirect::Policy::limited(3))?;
    let redirect_client = build_search_client(reqwest::redirect::Policy::none())?;
    let cell = {
        let in_flight = SEARCH_IN_FLIGHT.get_or_init(|| Mutex::new(HashMap::new()));
        let mut in_flight = in_flight.lock().map_err(|_| {
            CoreError::Internal("web_search singleflight lock is unavailable".to_string())
        })?;
        in_flight
            .entry(cache_key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };

    let request_for_cell = request.clone();
    let runtime_for_cell = runtime.clone();
    let execution = cell
        .get_or_init(|| async move {
            execute_search_request(request_for_cell, runtime_for_cell, client, redirect_client)
                .await
        })
        .await
        .clone();

    if let Some(in_flight) = SEARCH_IN_FLIGHT.get() {
        if let Ok(mut in_flight) = in_flight.lock() {
            if let Some(current) = in_flight.get(&cache_key) {
                if Arc::ptr_eq(current, &cell) {
                    in_flight.remove(&cache_key);
                }
            }
        }
    }

    Ok(execution)
}

pub(crate) async fn run_web_search(
    args: WebSearchArgs,
    db: &Database,
) -> Result<SearchExecution, CoreError> {
    let web_config = db
        .load_app_config()
        .map(|config| config.web_search)
        .unwrap_or_default();
    let args = apply_app_config_defaults(args, &web_config);
    let request = build_search_request(args).map_err(CoreError::InvalidInput)?;
    let runtime = runtime_config_from_app_config(&web_config);
    let cache_key = cache_key_for_runtime(&request, &runtime);
    if let Some(response) = cached_response(&cache_key) {
        return Ok(SearchExecution {
            response,
            all_failed: false,
        });
    }

    let execution = execute_with_singleflight(request, runtime, cache_key.clone()).await?;
    store_response(cache_key, &execution.response);
    Ok(execution)
}

pub fn provider_status_snapshot(config: &WebSearchConfig) -> Vec<WebSearchProviderStatus> {
    let enabled = default_engines_for_profile(
        SearchLanguage::Auto,
        SearchRegion::Auto,
        config.provider_profile,
    )
    .into_iter()
    .collect::<HashSet<_>>();
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let locked = health.lock().ok();
    let mut statuses = [
        SearchEngine::Baidu,
        SearchEngine::Sogou,
        SearchEngine::Google,
        SearchEngine::Bing,
        SearchEngine::DuckDuckGo,
    ]
    .into_iter()
    .map(|engine| {
        let runtime_key = ProviderRuntimeKey::native(engine);
        let runtime = locked.as_ref().and_then(|state| state.get(&runtime_key));
        WebSearchProviderStatus {
            engine,
            id: engine.as_str().to_string(),
            label: engine.as_str().to_string(),
            health: runtime
                .map(|state| state.health)
                .unwrap_or(SearchProviderHealthState::Healthy),
            built_in: true,
            enabled_by_profile: enabled.contains(&engine)
                && config.provider_mode == WebSearchProviderMode::BuiltInFirst,
            enabled: config.provider_mode != WebSearchProviderMode::CustomOnly,
            configured: true,
            requires_api_key: false,
            requires_base_url: false,
            last_error_code: runtime.and_then(|state| state.last_error_code.clone()),
            next_retry_seconds: runtime
                .and_then(|state| state.next_retry_at)
                .and_then(seconds_until),
        }
    })
    .collect::<Vec<_>>();

    for provider in &config.custom_providers {
        let engine = engine_for_custom_provider(provider);
        let runtime_key = ProviderRuntimeKey::custom(provider);
        let runtime = locked.as_ref().and_then(|state| state.get(&runtime_key));
        statuses.push(WebSearchProviderStatus {
            engine,
            id: provider.id.clone(),
            label: provider.name.clone(),
            health: if provider.enabled {
                runtime
                    .map(|state| state.health)
                    .unwrap_or(SearchProviderHealthState::Healthy)
            } else {
                SearchProviderHealthState::Disabled
            },
            built_in: false,
            enabled_by_profile: provider.enabled
                && provider.is_configured()
                && config.provider_mode != WebSearchProviderMode::BuiltInFirst,
            enabled: provider.enabled,
            configured: provider.is_configured(),
            requires_api_key: provider.preset.requires_api_key(),
            requires_base_url: provider.preset.requires_base_url(),
            last_error_code: runtime.and_then(|state| state.last_error_code.clone()),
            next_retry_seconds: runtime
                .and_then(|state| state.next_retry_at)
                .and_then(seconds_until),
        });
    }

    statuses
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Web]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: WebSearchArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(e) => {
                return Ok(tool_contract_error_result(
                    call_id,
                    "invalid_arguments_json",
                    format!("Invalid web_search arguments: {e}"),
                    web_search_expected_format(),
                ));
            }
        };
        let execution = match run_web_search(args, db).await {
            Ok(execution) => execution,
            Err(CoreError::InvalidInput(message)) => {
                return Ok(tool_contract_error_result(
                    call_id,
                    "invalid_search_request",
                    message,
                    web_search_expected_format(),
                ));
            }
            Err(error) => return Err(error),
        };
        Ok(format_response(
            call_id,
            execution.response,
            execution.all_failed,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_search::{SearchLanguage, SearchRegion, TimeRange};

    fn reset_provider_state(engine: SearchEngine) {
        let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut health) = health.lock() {
            health.remove(&ProviderRuntimeKey::native(engine));
        }
    }

    fn test_search_request() -> SearchRequest {
        SearchRequest {
            query: "nexa".to_string(),
            effective_query: "nexa".to_string(),
            limit: 8,
            region: SearchRegion::Global,
            language: SearchLanguage::En,
            engines: vec![
                SearchEngine::Google,
                SearchEngine::DuckDuckGo,
                SearchEngine::Bing,
            ],
            explicit_engines: false,
            time_range: TimeRange::Any,
            site: None,
            include_snippets: true,
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::None,
        }
    }

    #[test]
    fn stop_policy_uses_one_or_two_default_providers() {
        let mut request = test_search_request();

        assert!(should_stop_after_success(&request, 8, 1));
        assert!(should_stop_after_success(&request, 2, 2));
        assert!(!should_stop_after_success(&request, 0, 2));
        request.explicit_engines = true;
        assert!(!should_stop_after_success(&request, 2, 2));
        request.explicit_engines = false;
        request.provider_profile = WebSearchProviderProfile::MaxEvidence;
        assert!(!should_stop_after_success(&request, 2, 2));
    }

    #[test]
    fn default_provider_parallelism_uses_two_provider_waves() {
        let request = test_search_request();
        let provider_plan = build_provider_plan(
            &request,
            &SearchRuntimeConfig {
                provider_mode: WebSearchProviderMode::BuiltInFirst,
                custom_providers: Vec::new(),
            },
        );

        assert_eq!(provider_parallelism(&request), 2);
        assert_eq!(
            next_provider_wave_end(&provider_plan, 0, provider_parallelism(&request)),
            2
        );
        assert_eq!(
            next_provider_wave_end(&provider_plan, 2, provider_parallelism(&request)),
            3
        );
    }

    #[test]
    fn provider_waves_do_not_mix_custom_and_native_tiers() {
        let mut request = test_search_request();
        request.explicit_engines = true;
        let custom = WebSearchCustomProviderConfig {
            id: "anysearch".to_string(),
            preset: WebSearchCustomProviderPreset::AnySearch,
            name: "AnySearch".to_string(),
            enabled: true,
            api_key: String::new(),
            base_url: WebSearchCustomProviderPreset::AnySearch.default_base_url(),
            priority: 1,
        };
        let runtime = SearchRuntimeConfig {
            provider_mode: WebSearchProviderMode::CustomFirst,
            custom_providers: vec![custom],
        };
        let provider_plan = build_provider_plan(&request, &runtime);

        assert_eq!(provider_parallelism(&request), 3);
        assert_eq!(
            next_provider_wave_end(&provider_plan, 0, provider_parallelism(&request)),
            1
        );
        assert_eq!(
            next_provider_wave_end(&provider_plan, 1, provider_parallelism(&request)),
            4
        );
    }

    #[test]
    fn cache_policy_uses_short_ttl_for_empty_successes() {
        let response = SearchResponse {
            query: "no matches".to_string(),
            region: SearchRegion::Global,
            language: SearchLanguage::En,
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::None,
            time_range: TimeRange::Any,
            time_range_info: SearchTimeRangeInfo {
                requested: TimeRange::Any,
                applied_by: Vec::new(),
                ignored_by: Vec::new(),
            },
            engines_requested: vec![SearchEngine::Bing],
            engines_responded: vec![SearchEngine::Bing],
            engines_failed: Vec::new(),
            provider_health: Vec::new(),
            total_results: 0,
            results: Vec::new(),
            cache: SearchCacheInfo {
                status: "miss".to_string(),
                ttl_seconds: 0,
            },
        };

        assert_eq!(
            cache_ttl_for_response(&response),
            Some(SEARCH_EMPTY_CACHE_TTL)
        );
    }

    #[test]
    fn docs_first_reranker_promotes_documentation_hosts() {
        let results = vec![
            SearchResultItem {
                rank: 0,
                title: "Blog post".to_string(),
                url: "https://example.com/blog/api-wrapper".to_string(),
                display_url: "example.com/blog/api-wrapper".to_string(),
                snippet: "A tutorial".to_string(),
                source: "example.com".to_string(),
                engine: SearchEngine::Bing,
                provider_rank: 1,
                resolved: true,
                confidence: "medium".to_string(),
            },
            SearchResultItem {
                rank: 0,
                title: "API reference".to_string(),
                url: "https://docs.example.dev/reference".to_string(),
                display_url: "docs.example.dev/reference".to_string(),
                snippet: "Official documentation".to_string(),
                source: "docs.example.dev".to_string(),
                engine: SearchEngine::Bing,
                provider_rank: 2,
                resolved: true,
                confidence: "medium".to_string(),
            },
        ];

        let ranked = merge_and_rank_results(results, 2, WebSearchReranker::DocsFirst);

        assert_eq!(ranked[0].source, "docs.example.dev");
        assert_eq!(ranked[0].rank, 1);
    }

    #[test]
    fn runtime_config_keeps_only_configured_custom_providers_by_priority() {
        let mut config = WebSearchConfig::default();
        config.provider_mode = WebSearchProviderMode::CustomFirst;
        config.custom_providers = vec![
            WebSearchCustomProviderConfig {
                id: "missing".to_string(),
                preset: WebSearchCustomProviderPreset::Brave,
                name: "Missing".to_string(),
                enabled: true,
                api_key: String::new(),
                base_url: WebSearchCustomProviderPreset::Brave.default_base_url(),
                priority: 1,
            },
            WebSearchCustomProviderConfig {
                id: "tavily".to_string(),
                preset: WebSearchCustomProviderPreset::Tavily,
                name: "Tavily".to_string(),
                enabled: true,
                api_key: "secret".to_string(),
                base_url: WebSearchCustomProviderPreset::Tavily.default_base_url(),
                priority: 20,
            },
            WebSearchCustomProviderConfig {
                id: "brave".to_string(),
                preset: WebSearchCustomProviderPreset::Brave,
                name: "Brave".to_string(),
                enabled: true,
                api_key: "secret".to_string(),
                base_url: WebSearchCustomProviderPreset::Brave.default_base_url(),
                priority: 10,
            },
        ];

        let runtime = runtime_config_from_app_config(&config);

        assert_eq!(
            runtime
                .custom_providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            vec!["brave", "tavily"]
        );
    }

    #[test]
    fn custom_first_is_not_bypassed_by_explicit_native_engines() {
        let request = SearchRequest {
            query: "nexa".to_string(),
            effective_query: "nexa".to_string(),
            limit: 8,
            region: SearchRegion::Global,
            language: SearchLanguage::En,
            engines: vec![SearchEngine::DuckDuckGo],
            explicit_engines: true,
            time_range: TimeRange::Any,
            site: None,
            include_snippets: true,
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::None,
        };
        let runtime = SearchRuntimeConfig {
            provider_mode: WebSearchProviderMode::CustomFirst,
            custom_providers: vec![WebSearchCustomProviderConfig {
                id: "tavily".to_string(),
                preset: WebSearchCustomProviderPreset::Tavily,
                name: "Tavily".to_string(),
                enabled: true,
                api_key: "secret".to_string(),
                base_url: WebSearchCustomProviderPreset::Tavily.default_base_url(),
                priority: 20,
            }],
        };

        let plan = build_provider_plan(&request, &runtime);

        assert!(matches!(
            plan.first(),
            Some(SearchPlanItem::Custom(provider)) if provider.id == "tavily"
        ));
        assert!(matches!(
            plan.get(1),
            Some(SearchPlanItem::Native(SearchEngine::DuckDuckGo))
        ));
    }

    #[test]
    fn custom_only_ignores_explicit_native_engines() {
        let request = SearchRequest {
            query: "nexa".to_string(),
            effective_query: "nexa".to_string(),
            limit: 8,
            region: SearchRegion::Global,
            language: SearchLanguage::En,
            engines: vec![SearchEngine::DuckDuckGo],
            explicit_engines: true,
            time_range: TimeRange::Any,
            site: None,
            include_snippets: true,
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::None,
        };
        let runtime = SearchRuntimeConfig {
            provider_mode: WebSearchProviderMode::CustomOnly,
            custom_providers: vec![WebSearchCustomProviderConfig {
                id: "tavily".to_string(),
                preset: WebSearchCustomProviderPreset::Tavily,
                name: "Tavily".to_string(),
                enabled: true,
                api_key: "secret".to_string(),
                base_url: WebSearchCustomProviderPreset::Tavily.default_base_url(),
                priority: 20,
            }],
        };

        let plan = build_provider_plan(&request, &runtime);

        assert_eq!(plan.len(), 1);
        assert!(matches!(
            plan.first(),
            Some(SearchPlanItem::Custom(provider)) if provider.id == "tavily"
        ));
    }

    #[test]
    fn provider_status_reports_custom_provider_setup_state() {
        let mut config = WebSearchConfig::default();
        config.custom_providers = vec![WebSearchCustomProviderConfig {
            id: "brave".to_string(),
            preset: WebSearchCustomProviderPreset::Brave,
            name: "Brave".to_string(),
            enabled: true,
            api_key: String::new(),
            base_url: WebSearchCustomProviderPreset::Brave.default_base_url(),
            priority: 10,
        }];

        let status = provider_status_snapshot(&config);
        let brave = status
            .iter()
            .find(|provider| provider.id == "brave")
            .expect("brave status");

        assert!(!brave.configured);
        assert!(brave.requires_api_key);
        assert!(!brave.built_in);
    }

    #[test]
    fn anysearch_can_be_enabled_without_api_key() {
        let mut config = WebSearchConfig::default();
        config.custom_providers = vec![WebSearchCustomProviderConfig {
            id: "anysearch".to_string(),
            preset: WebSearchCustomProviderPreset::AnySearch,
            name: "AnySearch".to_string(),
            enabled: true,
            api_key: String::new(),
            base_url: WebSearchCustomProviderPreset::AnySearch.default_base_url(),
            priority: 25,
        }];

        let runtime = runtime_config_from_app_config(&config);
        let status = provider_status_snapshot(&config);
        let anysearch = status
            .iter()
            .find(|provider| provider.id == "anysearch")
            .expect("anysearch status");

        assert_eq!(runtime.custom_providers.len(), 1);
        assert!(anysearch.configured);
        assert!(!anysearch.requires_api_key);
    }

    #[test]
    fn custom_provider_health_is_scoped_to_key_and_endpoint() {
        let mut provider = WebSearchCustomProviderConfig {
            id: "tavily".to_string(),
            preset: WebSearchCustomProviderPreset::Tavily,
            name: "Tavily".to_string(),
            enabled: true,
            api_key: "tvly-dev-old".to_string(),
            base_url: WebSearchCustomProviderPreset::Tavily.default_base_url(),
            priority: 20,
        };
        let old_key = ProviderRuntimeKey::custom(&provider);
        let failure = SearchProviderFailure::new(
            SearchEngine::Tavily,
            "auth_failed",
            "Tavily returned HTTP 401",
        );

        let (health, _) = record_provider_failure(&old_key, &failure);
        assert_eq!(health, SearchProviderHealthState::Degraded);

        provider.api_key = "tvly-dev-new".to_string();
        let new_key = ProviderRuntimeKey::custom(&provider);

        assert_ne!(old_key, new_key);
        let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
        let locked = health.lock().expect("health lock");
        assert!(locked.get(&old_key).is_some());
        assert!(locked.get(&new_key).is_none());
    }

    #[tokio::test]
    async fn rate_limit_failure_opens_provider_circuit() {
        let engine = SearchEngine::DuckDuckGo;
        reset_provider_state(engine);
        let failure = SearchProviderFailure::new(engine, "rate_limited", "provider returned 429")
            .with_retry_after(Some(7));
        let key = ProviderRuntimeKey::native(engine);

        let (health, retry_after) = record_provider_failure(&key, &failure);

        assert_eq!(health, SearchProviderHealthState::Blocked);
        assert!(retry_after.is_some_and(|seconds| seconds <= 7));

        let Err((failure, run_info)) = prepare_provider_call(key).await else {
            panic!("open circuit should skip provider call");
        };
        assert_eq!(failure.code, "circuit_open");
        assert!(run_info.skipped);
        assert_eq!(run_info.health, SearchProviderHealthState::Blocked);
        reset_provider_state(engine);
    }
}
