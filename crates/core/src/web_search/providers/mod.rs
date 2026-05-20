use std::collections::HashSet;
use std::net::IpAddr;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, RANGE, REFERER, RETRY_AFTER};
use reqwest::Url;
use scraper::{ElementRef, Html, Selector};

use super::model::{
    SearchEngine, SearchProviderFailure, SearchRequest, SearchResultItem, TimeRange,
};

pub mod baidu;
pub mod bing;
pub mod duckduckgo;
pub mod sogou;

pub struct SearchProviderContext<'a> {
    pub client: &'a reqwest::Client,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn engine(&self) -> SearchEngine;

    fn supports_time_range(&self, _time_range: TimeRange) -> bool {
        false
    }

    async fn search(
        &self,
        request: &SearchRequest,
        ctx: &SearchProviderContext<'_>,
    ) -> Result<Vec<SearchResultItem>, SearchProviderFailure>;
}

pub fn provider_for_engine(engine: SearchEngine) -> Box<dyn SearchProvider> {
    match engine {
        SearchEngine::Baidu => Box::new(baidu::BaiduProvider),
        SearchEngine::Sogou => Box::new(sogou::SogouProvider),
        SearchEngine::Bing => Box::new(bing::BingProvider),
        SearchEngine::DuckDuckGo => Box::new(duckduckgo::DuckDuckGoProvider),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ParserConfig<'a> {
    pub engine: SearchEngine,
    pub base_url: &'a str,
    pub result_selectors: &'a [&'a str],
    pub title_selectors: &'a [&'a str],
    pub snippet_selectors: &'a [&'a str],
}

pub(crate) async fn fetch_search_html(
    client: &reqwest::Client,
    engine: SearchEngine,
    url: Url,
    referer: Option<&str>,
    accept_language: &str,
) -> Result<String, SearchProviderFailure> {
    let mut request = client
        .get(url)
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(ACCEPT_LANGUAGE, accept_language);
    if let Some(referer) = referer {
        request = request.header(REFERER, referer);
    }

    let response = request.send().await.map_err(|e| {
        SearchProviderFailure::new(
            engine,
            "request_failed",
            format!("{engine:?} request failed: {e}"),
        )
    })?;
    let status = response.status();
    let retry_after = retry_after_secs(response.headers());
    if status.as_u16() == 429 {
        return Err(SearchProviderFailure::new(
            engine,
            "rate_limited",
            format!("{engine:?} returned HTTP {status}"),
        )
        .with_retry_after(retry_after));
    }
    if status.as_u16() == 403 {
        return Err(SearchProviderFailure::new(
            engine,
            "blocked",
            format!("{engine:?} returned HTTP {status}"),
        ));
    }
    if !status.is_success() {
        return Err(SearchProviderFailure::new(
            engine,
            "http_status",
            format!("{engine:?} returned HTTP {status}"),
        ));
    }

    response.text().await.map_err(|e| {
        SearchProviderFailure::new(
            engine,
            "body_read_failed",
            format!("{engine:?} response body could not be read: {e}"),
        )
    })
}

fn retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

pub(crate) fn parse_results(html: &str, config: ParserConfig<'_>) -> Vec<SearchResultItem> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for result_selector in config.result_selectors {
        let Ok(selector) = Selector::parse(result_selector) else {
            continue;
        };
        for card in document.select(&selector) {
            if results.len() >= 30 {
                break;
            }
            let Some((title, url, resolved)) =
                extract_link(card, config.title_selectors, config.base_url)
            else {
                continue;
            };
            let Some(url_info) = public_url_info(&url) else {
                continue;
            };
            let dedupe_key = dedupe_key(&url_info.url);
            if !seen.insert(dedupe_key) {
                continue;
            }
            let provider_rank = results.len() + 1;
            let snippet = extract_snippet(card, config.snippet_selectors, &title);
            results.push(SearchResultItem {
                rank: 0,
                title,
                url: url_info.url,
                display_url: url_info.display_url,
                snippet,
                source: url_info.source,
                engine: config.engine,
                provider_rank,
                resolved,
                confidence: if resolved { "medium" } else { "low" }.to_string(),
            });
        }
    }

    results
}

pub(crate) fn blocked_by_challenge(html: &str, needles: &[&str]) -> bool {
    let lower = html.to_lowercase();
    needles
        .iter()
        .any(|needle| lower.contains(&needle.to_lowercase()))
}

fn extract_link(
    card: ElementRef<'_>,
    title_selectors: &[&str],
    base_url: &str,
) -> Option<(String, String, bool)> {
    for selector in title_selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for link in card.select(&selector) {
            let Some(href) = link.value().attr("href") else {
                continue;
            };
            let title = normalize_space(&link.text().collect::<Vec<_>>().join(" "));
            if title.is_empty() {
                continue;
            }
            if let Some((url, resolved)) = normalize_href(base_url, href) {
                return Some((title, url, resolved));
            }
        }
    }

    let selector = Selector::parse("a[href]").ok()?;
    for link in card.select(&selector) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let title = normalize_space(&link.text().collect::<Vec<_>>().join(" "));
        if title.is_empty() {
            continue;
        }
        if let Some((url, resolved)) = normalize_href(base_url, href) {
            return Some((title, url, resolved));
        }
    }
    None
}

fn extract_snippet(card: ElementRef<'_>, snippet_selectors: &[&str], title: &str) -> String {
    for selector in snippet_selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in card.select(&selector) {
            let text = normalize_space(&element.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() && text != title {
                return text;
            }
        }
    }

    let full = normalize_space(&card.text().collect::<Vec<_>>().join(" "));
    full.strip_prefix(title)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(&full)
        .chars()
        .take(280)
        .collect()
}

fn normalize_href(base_url: &str, href: &str) -> Option<(String, bool)> {
    let href = href.trim();
    if href.is_empty()
        || href.starts_with('#')
        || href.to_ascii_lowercase().starts_with("javascript:")
    {
        return None;
    }

    let base = Url::parse(base_url).ok()?;
    let url = base.join(href).ok()?;
    if let Some(target) = extract_known_redirect_target(&url) {
        return Some((target, true));
    }
    Some((url.to_string(), !is_known_search_redirect(&url)))
}

pub(crate) async fn resolve_search_redirect_target(
    client: &reqwest::Client,
    engine: SearchEngine,
    raw_url: &str,
    max_hops: usize,
) -> Result<Option<UrlInfo>, SearchProviderFailure> {
    let mut current = Url::parse(raw_url)
        .map_err(|e| SearchProviderFailure::new(engine, "invalid_redirect_url", e.to_string()))?;
    if !is_known_search_redirect(&current) {
        return Ok(None);
    }

    for _ in 0..max_hops {
        crate::tools::fetch_url_tool::validate_url_for_fetch(current.as_str())
            .await
            .map_err(|e| SearchProviderFailure::new(engine, "redirect_blocked", e))?;
        let response = client
            .get(current.clone())
            .header(
                ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.5",
            )
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| {
                SearchProviderFailure::new(
                    engine,
                    "redirect_resolve_failed",
                    format!("{engine:?} redirect resolver failed: {e}"),
                )
            })?;
        if !response.status().is_redirection() {
            return Ok(None);
        }

        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                SearchProviderFailure::new(
                    engine,
                    "redirect_location_missing",
                    "Search redirect did not include a valid Location header",
                )
            })?;
        let next = current.join(location).map_err(|e| {
            SearchProviderFailure::new(engine, "invalid_redirect_location", e.to_string())
        })?;
        crate::tools::fetch_url_tool::validate_url_for_fetch(next.as_str())
            .await
            .map_err(|e| SearchProviderFailure::new(engine, "redirect_blocked", e))?;
        if !is_known_search_redirect(&next) {
            return Ok(public_url_info(next.as_str()));
        }
        current = next;
    }

    Err(SearchProviderFailure::new(
        engine,
        "redirect_hop_limit",
        format!("Search redirect exceeded {max_hops} hop(s)"),
    ))
}

fn extract_known_redirect_target(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let known_host = host.ends_with("duckduckgo.com")
        || host.ends_with("sogou.com")
        || host.ends_with("bing.com");
    if !known_host {
        return None;
    }

    for (key, value) in url.query_pairs() {
        let key = key.to_ascii_lowercase();
        if matches!(key.as_str(), "uddg" | "url" | "u" | "target" | "to" | "r") {
            if value.starts_with("http://") || value.starts_with("https://") {
                if let Some(info) = public_url_info(&value) {
                    return Some(info.url);
                }
            }
        }
    }
    None
}

fn is_known_search_redirect(url: &Url) -> bool {
    let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    (host.ends_with("baidu.com") && url.path().starts_with("/link"))
        || (host.ends_with("sogou.com") && url.path().contains("link"))
        || (host.ends_with("bing.com") && url.path().contains("/ck/"))
        || (host.ends_with("duckduckgo.com") && url.path().starts_with("/l/"))
}

#[derive(Debug)]
pub(crate) struct UrlInfo {
    pub(crate) url: String,
    pub(crate) display_url: String,
    pub(crate) source: String,
}

pub(crate) fn public_url_info(value: &str) -> Option<UrlInfo> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);

    let host = url.host_str()?.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_local_ip(&ip) {
            return None;
        }
    }

    strip_tracking_params(&mut url);
    let path = url.path().trim_end_matches('/');
    let display_path = if path.is_empty() || path == "/" {
        String::new()
    } else if path.chars().count() > 48 {
        format!("{}...", path.chars().take(48).collect::<String>())
    } else {
        path.to_string()
    };
    Some(UrlInfo {
        url: url.to_string(),
        display_url: format!("{host}{display_path}"),
        source: host,
    })
}

fn strip_tracking_params(url: &mut Url) {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !(key.starts_with("utm_")
                || matches!(
                    key.as_str(),
                    "fbclid" | "gclid" | "yclid" | "mc_cid" | "mc_eid" | "spm" | "from"
                ))
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !pairs.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }
}

pub(crate) fn is_private_or_local_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || ((v6.segments()[0] & 0xfe00) == 0xfc00)
                || ((v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

fn dedupe_key(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|mut parsed| {
            parsed.set_fragment(None);
            parsed.set_query(None);
            Some(
                parsed
                    .to_string()
                    .trim_end_matches('/')
                    .to_ascii_lowercase(),
            )
        })
        .unwrap_or_else(|| url.trim_end_matches('/').to_ascii_lowercase())
}

pub(crate) fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
