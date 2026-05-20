//! WebSearchTool — native no-key public web search.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::OnceCell;

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
static PROVIDER_HEALTH: OnceLock<Mutex<HashMap<SearchEngine, ProviderRuntimeState>>> =
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

fn web_search_expected_format() -> serde_json::Value {
    serde_json::json!({
        "query": "focused natural-language search query",
        "limit": "integer from 1 to 20",
        "region": "auto | mainland_cn | global",
        "language": "auto | zh | en",
        "engines": ["optional subset: baidu, sogou, bing, duckduckgo"],
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

fn apply_app_config_defaults(mut args: WebSearchArgs, db: &Database) -> WebSearchArgs {
    if args.provider_profile.is_some() && args.reranker.is_some() {
        return args;
    }

    if let Ok(config) = db.load_app_config() {
        if args.provider_profile.is_none() {
            args.provider_profile = Some(config.web_search.provider_profile);
        }
        if args.reranker.is_none() {
            args.reranker = Some(config.web_search.reranker);
        }
    }
    args
}

fn format_engine_list(engines: &[SearchEngine]) -> String {
    engines
        .iter()
        .map(|engine| engine.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn cache_key(request: &SearchRequest) -> String {
    let engines = request
        .engines
        .iter()
        .map(|engine| engine.as_str())
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
    })
    .to_string()
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
        "captcha" | "blocked" => Some(PROVIDER_BLOCKED_COOLDOWN),
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
    engine: SearchEngine,
) -> Result<SearchProviderHealthState, (SearchProviderFailure, SearchProviderRunInfo)> {
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
            let state = health.entry(engine).or_default();
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

fn record_provider_success(engine: SearchEngine) -> SearchProviderHealthState {
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut health) = health.lock() else {
        return SearchProviderHealthState::Healthy;
    };
    let state = health.entry(engine).or_default();
    state.health = SearchProviderHealthState::Healthy;
    state.consecutive_failures = 0;
    state.last_error_code = None;
    state.last_success_at = Some(Instant::now());
    state.next_retry_at = None;
    state.health
}

fn record_provider_failure(
    engine: SearchEngine,
    failure: &SearchProviderFailure,
) -> (SearchProviderHealthState, Option<u64>) {
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut health) = health.lock() else {
        return (SearchProviderHealthState::Degraded, None);
    };
    let state = health.entry(engine).or_default();
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
    latency_ms: u128,
    result_count: usize,
) -> SearchProviderRunInfo {
    SearchProviderRunInfo {
        engine,
        health: record_provider_success(engine),
        skipped: false,
        latency_ms: Some(latency_ms),
        result_count,
        error_code: None,
        next_retry_seconds: None,
    }
}

fn run_info_for_failure(
    engine: SearchEngine,
    latency_ms: u128,
    failure: &SearchProviderFailure,
) -> SearchProviderRunInfo {
    let (health, next_retry_seconds) = record_provider_failure(engine, failure);
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

async fn resolve_search_redirects(
    redirect_client: &reqwest::Client,
    results: &mut [SearchResultItem],
) -> Vec<SearchProviderFailure> {
    let mut failures = Vec::new();
    let mut attempted = 0usize;
    for result in results.iter_mut().filter(|result| !result.resolved) {
        if attempted >= REDIRECT_RESOLVE_MAX_RESULTS {
            break;
        }
        attempted += 1;
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

async fn execute_search_request(
    request: SearchRequest,
    client: reqwest::Client,
    redirect_client: reqwest::Client,
) -> SearchExecution {
    let ctx = SearchProviderContext { client: &client };

    let mut raw_results = Vec::new();
    let mut engines_responded = Vec::new();
    let mut engines_failed: Vec<SearchProviderFailure> = Vec::new();
    let mut provider_health = Vec::new();
    let mut time_range_applied_by = Vec::new();
    let mut time_range_ignored_by = Vec::new();
    let mut attempted = 0usize;

    for engine in &request.engines {
        attempted += 1;
        let provider = provider_for_engine(*engine);
        if request.time_range != crate::web_search::TimeRange::Any {
            if provider.supports_time_range(request.time_range) {
                time_range_applied_by.push(*engine);
            } else {
                time_range_ignored_by.push(*engine);
            }
        }

        match prepare_provider_call(*engine).await {
            Ok(_) => {}
            Err((failure, run_info)) => {
                engines_failed.push(failure);
                provider_health.push(run_info);
                continue;
            }
        }

        let started_at = Instant::now();
        match provider.search(&request, &ctx).await {
            Ok(mut results) => {
                let latency_ms = started_at.elapsed().as_millis();
                if !request.include_snippets {
                    for result in &mut results {
                        result.snippet.clear();
                    }
                }
                let result_count = results.len();
                engines_responded.push(*engine);
                raw_results.append(&mut results);
                provider_health.push(run_info_for_success(*engine, latency_ms, result_count));
            }
            Err(failure) => {
                let latency_ms = started_at.elapsed().as_millis();
                provider_health.push(run_info_for_failure(*engine, latency_ms, &failure));
                engines_failed.push(failure);
            }
        }

        if should_stop_after_success(&request, raw_results.len(), engines_responded.len()) {
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
        engines_requested: request.engines.iter().copied().take(attempted).collect(),
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
    let execution = cell
        .get_or_init(|| async move {
            execute_search_request(request_for_cell, client, redirect_client).await
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
    let args = apply_app_config_defaults(args, db);
    let request = build_search_request(args).map_err(CoreError::InvalidInput)?;
    let cache_key = cache_key(&request);
    if let Some(response) = cached_response(&cache_key) {
        return Ok(SearchExecution {
            response,
            all_failed: false,
        });
    }

    let execution = execute_with_singleflight(request, cache_key.clone()).await?;
    store_response(cache_key, &execution.response);
    Ok(execution)
}

pub fn provider_status_snapshot(profile: WebSearchProviderProfile) -> Vec<WebSearchProviderStatus> {
    let enabled = default_engines_for_profile(SearchLanguage::Auto, SearchRegion::Auto, profile)
        .into_iter()
        .collect::<HashSet<_>>();
    let health = PROVIDER_HEALTH.get_or_init(|| Mutex::new(HashMap::new()));
    let locked = health.lock().ok();
    [
        SearchEngine::Baidu,
        SearchEngine::Sogou,
        SearchEngine::Bing,
        SearchEngine::DuckDuckGo,
    ]
    .into_iter()
    .map(|engine| {
        let runtime = locked.as_ref().and_then(|state| state.get(&engine));
        WebSearchProviderStatus {
            engine,
            label: engine.as_str().to_string(),
            health: runtime
                .map(|state| state.health)
                .unwrap_or(SearchProviderHealthState::Healthy),
            built_in: true,
            enabled_by_profile: enabled.contains(&engine),
            last_error_code: runtime.and_then(|state| state.last_error_code.clone()),
            next_retry_seconds: runtime
                .and_then(|state| state.next_retry_at)
                .and_then(seconds_until),
        }
    })
    .collect()
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
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
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
            health.remove(&engine);
        }
    }

    #[test]
    fn stop_policy_uses_one_or_two_default_providers() {
        let mut request = SearchRequest {
            query: "nexa".to_string(),
            effective_query: "nexa".to_string(),
            limit: 8,
            region: SearchRegion::Global,
            language: SearchLanguage::En,
            engines: vec![SearchEngine::Bing, SearchEngine::DuckDuckGo],
            explicit_engines: false,
            time_range: TimeRange::Any,
            site: None,
            include_snippets: true,
            provider_profile: WebSearchProviderProfile::Default,
            reranker: WebSearchReranker::None,
        };

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

    #[tokio::test]
    async fn rate_limit_failure_opens_provider_circuit() {
        let engine = SearchEngine::DuckDuckGo;
        reset_provider_state(engine);
        let failure = SearchProviderFailure::new(engine, "rate_limited", "provider returned 429")
            .with_retry_after(Some(7));

        let (health, retry_after) = record_provider_failure(engine, &failure);

        assert_eq!(health, SearchProviderHealthState::Blocked);
        assert!(retry_after.is_some_and(|seconds| seconds <= 7));

        let Err((failure, run_info)) = prepare_provider_call(engine).await else {
            panic!("open circuit should skip provider call");
        };
        assert_eq!(failure.code, "circuit_open");
        assert!(run_info.skipped);
        assert_eq!(run_info.health, SearchProviderHealthState::Blocked);
        reset_provider_state(engine);
    }
}
