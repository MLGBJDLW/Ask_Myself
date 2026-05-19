//! WebSearchTool — native no-key public web search.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::db::Database;
use crate::error::CoreError;
use crate::web_search::{
    build_search_request, provider_for_engine, SearchCacheInfo, SearchEngine,
    SearchProviderContext, SearchProviderFailure, SearchRequest, SearchResponse, SearchResultItem,
    WebSearchArgs,
};

use super::{tool_contract_error_result, Tool, ToolCategory, ToolDef, ToolResult, TrustBoundary};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/web_search.json");
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(300);
static SEARCH_CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

pub struct WebSearchTool;

#[derive(Clone)]
struct CacheEntry {
    response: SearchResponse,
    stored_at: Instant,
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
        .map(|entry| entry.stored_at.elapsed() > SEARCH_CACHE_TTL)?;
    if expired {
        cache.remove(key);
        return None;
    }
    let entry = cache.get(key)?;
    let mut response = entry.response.clone();
    response.cache = SearchCacheInfo {
        status: "hit".to_string(),
        ttl_seconds: SEARCH_CACHE_TTL
            .checked_sub(entry.stored_at.elapsed())
            .unwrap_or_default()
            .as_secs(),
    };
    Some(response)
}

fn store_response(key: String, response: &SearchResponse) {
    if response.results.is_empty() {
        return;
    }
    let cache = SEARCH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        let mut stored = response.clone();
        stored.cache = SearchCacheInfo {
            status: "hit".to_string(),
            ttl_seconds: SEARCH_CACHE_TTL.as_secs(),
        };
        cache.insert(
            key,
            CacheEntry {
                response: stored,
                stored_at: Instant::now(),
            },
        );
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
            text.push_str(&format!(
                "- {}: {} ({})\n",
                failure.engine.as_str(),
                failure.code,
                failure.message
            ));
        }
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

        let client = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|e| CoreError::Internal(format!("Failed to build web search client: {e}")))?;
        let ctx = SearchProviderContext { client: &client };

        let mut raw_results = Vec::new();
        let mut engines_responded = Vec::new();
        let mut engines_failed: Vec<SearchProviderFailure> = Vec::new();
        let mut attempted = 0usize;

        for engine in &request.engines {
            attempted += 1;
            let provider = provider_for_engine(*engine);
            match provider.search(&request, &ctx).await {
                Ok(mut results) => {
                    if !request.include_snippets {
                        for result in &mut results {
                            result.snippet.clear();
                        }
                    }
                    if !results.is_empty() {
                        engines_responded.push(*engine);
                        raw_results.append(&mut results);
                    } else {
                        engines_responded.push(*engine);
                    }
                }
                Err(failure) => engines_failed.push(failure),
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

        let mut response = SearchResponse {
            query: request.query.clone(),
            region: request.region,
            language: request.language,
            engines_requested: request.engines.iter().copied().take(attempted).collect(),
            engines_responded,
            engines_failed,
            total_results: 0,
            results: merge_and_rank_results(raw_results, request.limit),
            cache: SearchCacheInfo {
                status: "miss".to_string(),
                ttl_seconds: SEARCH_CACHE_TTL.as_secs(),
            },
        };
        response.total_results = response.results.len();

        let all_failed = response.results.is_empty() && response.engines_responded.is_empty();
        if response.results.is_empty() {
            response.cache = SearchCacheInfo {
                status: "bypass".to_string(),
                ttl_seconds: 0,
            };
        } else {
            store_response(cache_key, &response);
        }
        Ok(format_response(call_id, response, all_failed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_policy_uses_one_or_two_default_providers() {
        assert!(should_stop_after_success(false, 8, 1, 8));
        assert!(should_stop_after_success(false, 2, 2, 8));
        assert!(!should_stop_after_success(false, 0, 2, 8));
        assert!(!should_stop_after_success(true, 2, 2, 8));
    }
}
