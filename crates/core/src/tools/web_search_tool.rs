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
    build_search_request, provider_for_engine, SearchCacheInfo, SearchEngine,
    SearchProviderContext, SearchProviderFailure, SearchProviderHealthState, SearchProviderRunInfo,
    SearchRequest, SearchResponse, SearchResultItem, SearchTimeRangeInfo, WebSearchArgs,
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
struct SearchExecution {
    response: SearchResponse,
    all_failed: bool,
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
        "include_snippets": true
    })
}

fn merge_and_rank_results(results: Vec<SearchResultItem>, limit: usize) -> Vec<SearchResultItem> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for mut result in results {
        let key = result.url.trim_end_matches('/').to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        result.rank = merged.len() + 1;
        merged.push(result);
        if merged.len() >= limit {
            break;
        }
    }
    merged
}

fn should_stop_after_success(
    request_explicit_engines: bool,
    result_count: usize,
    responded_count: usize,
    limit: usize,
) -> bool {
    if request_explicit_engines {
        return result_count >= limit;
    }
    result_count >= limit || (result_count > 0 && responded_count >= 2)
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
        "Web search for: {}\nRegion: {:?}; language: {:?}; engines requested: {}.\n",
        response.query,
        response.region,
        response.language,
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

        if should_stop_after_success(
            request.explicit_engines,
            raw_results.len(),
            engines_responded.len(),
            request.limit,
        ) {
            break;
        }
    }

    let mut results = merge_and_rank_results(raw_results, request.limit);
    let redirect_failures = resolve_search_redirects(&redirect_client, &mut results).await;
    results = merge_and_rank_results(results, request.limit);
    engines_failed.extend(redirect_failures);

    let mut response = SearchResponse {
        query: request.query.clone(),
        region: request.region,
        language: request.language,
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
        _db: &Database,
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
        let request = match build_search_request(args) {
            Ok(request) => request,
            Err(message) => {
                return Ok(tool_contract_error_result(
                    call_id,
                    "invalid_search_request",
                    message,
                    web_search_expected_format(),
                ));
            }
        };
        let cache_key = cache_key(&request);
        if let Some(response) = cached_response(&cache_key) {
            return Ok(format_response(call_id, response, false));
        }

        let execution = execute_with_singleflight(request, cache_key.clone()).await?;
        store_response(cache_key, &execution.response);
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
        assert!(should_stop_after_success(false, 8, 1, 8));
        assert!(should_stop_after_success(false, 2, 2, 8));
        assert!(!should_stop_after_success(false, 0, 2, 8));
        assert!(!should_stop_after_success(true, 2, 2, 8));
    }

    #[test]
    fn cache_policy_uses_short_ttl_for_empty_successes() {
        let response = SearchResponse {
            query: "no matches".to_string(),
            region: SearchRegion::Global,
            language: SearchLanguage::En,
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
