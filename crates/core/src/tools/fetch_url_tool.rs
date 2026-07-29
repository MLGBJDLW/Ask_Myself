//! FetchUrlTool — fetches public web content and extracts readable text.

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use encoding_rs::Encoding;
use futures::StreamExt;
use readabilityrs::{Readability, ReadabilityOptions};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE,
    IF_NONE_MATCH, LAST_MODIFIED, LOCATION,
};
use reqwest::redirect::Policy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/fetch_url.json");
const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_IMAGE_ASSETS: usize = 25;
const JS_RENDER_SETTLE_MS: u64 = 1500;
const JS_RENDER_TIMEOUT_SECS: u64 = 10;
static FETCH_BODY_CACHE: OnceLock<Mutex<HashMap<String, CachedFetchBody>>> = OnceLock::new();

/// Tool that fetches a public URL and returns readable text plus provenance.
pub struct FetchUrlTool;

#[derive(Deserialize)]
struct FetchUrlArgs {
    url: String,
    #[serde(default = "default_max_length")]
    max_length: usize,
    #[serde(default)]
    mode: FetchMode,
    #[serde(default = "default_include_assets")]
    include_assets: bool,
    #[serde(default)]
    render_js: FetchRenderPolicy,
}

fn default_max_length() -> usize {
    5000
}

fn default_include_assets() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum FetchMode {
    #[default]
    Auto,
    Readability,
    Text,
    Metadata,
    Assets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum FetchRenderPolicy {
    #[default]
    Auto,
    Never,
    Always,
}

impl FetchRenderPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Never => "never",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    Html,
    Json,
    Text,
    UnsupportedBinary,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageMetadata {
    title: Option<String>,
    description: Option<String>,
    byline: Option<String>,
    site_name: Option<String>,
    lang: Option<String>,
    published_time: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageAsset {
    kind: String,
    url: String,
    alt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

struct ImageAssetCandidate<'a> {
    kind: &'a str,
    raw_url: &'a str,
    alt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

impl<'a> ImageAssetCandidate<'a> {
    fn new(kind: &'a str, raw_url: &'a str) -> Self {
        Self {
            kind,
            raw_url,
            alt: None,
            width: None,
            height: None,
        }
    }

    fn with_details(
        mut self,
        alt: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Self {
        self.alt = alt;
        self.width = width;
        self.height = height;
        self
    }
}

#[derive(Debug, Clone)]
struct HtmlExtraction {
    text: String,
    title: Option<String>,
    method: &'static str,
    metadata: PageMetadata,
}

pub(crate) struct BrowserRenderedHtml {
    pub(crate) final_url: reqwest::Url,
    pub(crate) html: String,
    pub(crate) title: Option<String>,
    pub(crate) rendered_text: String,
    pub(crate) interactive_elements: Vec<serde_json::Value>,
    pub(crate) diagnostics: BrowserDiagnostics,
    pub(crate) blocked_requests: usize,
    pub(crate) screenshot_png: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserDiagnostics {
    pub(crate) console_entries: Vec<String>,
    pub(crate) runtime_exceptions: Vec<String>,
    pub(crate) network_failures: Vec<String>,
    pub(crate) http_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConditionalRequestHeaders {
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedFetchBody {
    final_url: String,
    content_type: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    body_bytes: Vec<u8>,
    body_truncated: bool,
}

#[derive(Debug)]
struct FetchBodyPayload {
    final_url: reqwest::Url,
    content_type: Option<String>,
    body_bytes: Vec<u8>,
    body_truncated: bool,
    redirect_count: usize,
    cache_status: &'static str,
}

// ---------------------------------------------------------------------------
// URL validation helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the URL scheme and host are allowed (no private IPs).
fn validate_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Unsupported scheme '{other}': only http and https are allowed"
            ))
        }
    }

    let host = parsed.host_str().ok_or("URL has no host")?;

    // Block localhost variants.
    let lower = host.to_lowercase();
    if lower == "localhost"
        || lower == "127.0.0.1"
        || lower == "::1"
        || lower == "[::1]"
        || lower == "0.0.0.0"
    {
        return Err("Access to localhost is not allowed".to_string());
    }

    // Block private IP ranges.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!("Access to private IP {ip} is not allowed"));
        }
    }

    Ok(parsed)
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.x
                || v4.is_private()    // 10.x, 172.16-31.x, 192.168.x
                || v4.is_link_local() // 169.254.x
                || v4.octets()[0] == 0 // 0.0.0.0/8
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() // ::1
                || v6.is_unspecified() // ::
                || v6.is_multicast()
                || ((v6.segments()[0] & 0xfe00) == 0xfc00) // unique local
                || ((v6.segments()[0] & 0xffc0) == 0xfe80) // link local
        }
    }
}

async fn validate_resolved_host(url: &reqwest::Url) -> Result<(), String> {
    let host = url.host_str().ok_or("URL has no host")?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or("URL scheme has no known default port")?;
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("DNS lookup failed for {host}: {e}"))?;
    let mut resolved_any = false;
    for addr in addrs {
        resolved_any = true;
        let ip = addr.ip();
        if is_private_ip(&ip) {
            return Err(format!(
                "Access to host {host} is not allowed because it resolves to private IP {ip}"
            ));
        }
    }
    if !resolved_any {
        return Err(format!("DNS lookup for {host} returned no addresses"));
    }
    Ok(())
}

fn validate_resolved_host_blocking(url: &reqwest::Url) -> Result<(), String> {
    let host = url.host_str().ok_or("URL has no host")?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or("URL scheme has no known default port")?;
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed for {host}: {e}"))?;
    let mut resolved_any = false;
    for addr in addrs {
        resolved_any = true;
        let ip = addr.ip();
        if is_private_ip(&ip) {
            return Err(format!(
                "Access to host {host} is not allowed because it resolves to private IP {ip}"
            ));
        }
    }
    if !resolved_any {
        return Err(format!("DNS lookup for {host} returned no addresses"));
    }
    Ok(())
}

pub(crate) async fn validate_url_for_fetch(url: &str) -> Result<reqwest::Url, String> {
    let parsed = validate_url(url)?;
    validate_resolved_host(&parsed).await?;
    Ok(parsed)
}

fn validate_url_for_fetch_blocking(url: &str) -> Result<reqwest::Url, String> {
    let parsed = validate_url(url)?;
    validate_resolved_host_blocking(&parsed)?;
    Ok(parsed)
}

fn is_loopback_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn normalize_browser_capture_url(url: &str) -> Result<reqwest::Url, String> {
    let mut parsed = reqwest::Url::parse(url).map_err(|error| format!("Invalid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Browser capture only supports http and https URLs".to_string());
    }
    match parsed.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("0.0.0.0") | Some("::") | Some("[::]") => parsed
            .set_host(Some("127.0.0.1"))
            .map_err(|_| "failed to normalize local development URL".to_string())?,
        _ => {}
    }
    Ok(parsed)
}

async fn validate_url_for_browser_capture(url: &str) -> Result<reqwest::Url, String> {
    let parsed = normalize_browser_capture_url(url)?;
    if is_loopback_url(&parsed) {
        return Ok(parsed);
    }
    validate_url_for_fetch(parsed.as_str()).await
}

fn validate_url_for_browser_capture_blocking(
    url: &str,
    allow_loopback: bool,
) -> Result<reqwest::Url, String> {
    let parsed = normalize_browser_capture_url(url)?;
    if allow_loopback && is_loopback_url(&parsed) {
        return Ok(parsed);
    }
    validate_url_for_fetch_blocking(parsed.as_str())
}

pub(crate) fn build_http_client() -> Result<reqwest::Client, reqwest::Error> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,text/markdown;q=0.8,application/json;q=0.7,*/*;q=0.5",
        ),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7"),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .user_agent("Nexa/0.6 public-page-fetcher")
        .redirect(Policy::none())
        .cookie_store(true)
        .timeout(Duration::from_secs(20))
        .build()
}

pub(crate) async fn send_with_safe_redirects(
    client: &reqwest::Client,
    initial_url: reqwest::Url,
) -> Result<(reqwest::Response, reqwest::Url, usize), String> {
    send_with_safe_redirects_conditional(client, initial_url, None).await
}

pub(crate) async fn send_with_safe_redirects_conditional(
    client: &reqwest::Client,
    initial_url: reqwest::Url,
    conditional: Option<&ConditionalRequestHeaders>,
) -> Result<(reqwest::Response, reqwest::Url, usize), String> {
    let mut current = initial_url;
    for redirect_count in 0..=MAX_REDIRECTS {
        validate_resolved_host(&current).await?;
        let mut request = client.get(current.clone());
        if let Some(conditional) = conditional {
            if let Some(etag) = &conditional.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &conditional.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_redirection() {
            return Ok((response, current, redirect_count));
        }
        if redirect_count == MAX_REDIRECTS {
            return Err(format!("Too many redirects (>{MAX_REDIRECTS})"));
        }

        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                "Redirect response did not include a valid Location header".to_string()
            })?;
        let next = current
            .join(location)
            .map_err(|e| format!("Invalid redirect Location: {e}"))?;
        validate_url(next.as_str())?;
        current = next;
    }

    Err(format!("Too many redirects (>{MAX_REDIRECTS})"))
}

pub(crate) async fn read_limited_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut body = Vec::new();
    let mut truncated = false;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed to read response body: {e}"))?;
        if body.len() + chunk.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
}

fn cached_fetch_body(key: &str) -> Option<CachedFetchBody> {
    let cache = FETCH_BODY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    cache.lock().ok()?.get(key).cloned()
}

fn conditional_headers_from_cache(
    cached: Option<&CachedFetchBody>,
) -> Option<ConditionalRequestHeaders> {
    let cached = cached?;
    if cached.etag.is_none() && cached.last_modified.is_none() {
        return None;
    }
    Some(ConditionalRequestHeaders {
        etag: cached.etag.clone(),
        last_modified: cached.last_modified.clone(),
    })
}

fn header_to_string(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn store_fetch_body(
    input_url: &str,
    final_url: &reqwest::Url,
    headers: &HeaderMap,
    content_type: Option<String>,
    body_bytes: &[u8],
    body_truncated: bool,
) {
    if body_truncated {
        return;
    }
    let etag = header_to_string(headers, ETAG);
    let last_modified = header_to_string(headers, LAST_MODIFIED);
    if etag.is_none() && last_modified.is_none() {
        return;
    }
    let entry = CachedFetchBody {
        final_url: final_url.to_string(),
        content_type,
        etag,
        last_modified,
        body_bytes: body_bytes.to_vec(),
        body_truncated,
    };
    let cache = FETCH_BODY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(input_url.to_string(), entry.clone());
        cache.insert(final_url.to_string(), entry);
    }
}

fn cached_payload(cached: CachedFetchBody, redirect_count: usize) -> Option<FetchBodyPayload> {
    Some(FetchBodyPayload {
        final_url: reqwest::Url::parse(&cached.final_url).ok()?,
        content_type: cached.content_type,
        body_bytes: cached.body_bytes,
        body_truncated: cached.body_truncated,
        redirect_count,
        cache_status: "validated",
    })
}

fn charset_from_content_type(content_type: Option<&str>) -> Option<&'static Encoding> {
    let content_type = content_type?;
    for part in content_type.split(';') {
        let part = part.trim();
        let Some(value) = part.strip_prefix("charset=") else {
            continue;
        };
        return Encoding::for_label(value.trim_matches('"').as_bytes());
    }
    None
}

fn decode_body(bytes: &[u8], content_type: Option<&str>) -> String {
    let encoding = charset_from_content_type(content_type).unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn classify_body_kind(content_type: Option<&str>, bytes: &[u8]) -> BodyKind {
    if let Some(content_type) = content_type.map(|value| value.to_ascii_lowercase()) {
        let media_type = content_type
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or(content_type.as_str());
        if media_type == "text/html" || media_type == "application/xhtml+xml" {
            return BodyKind::Html;
        }
        if media_type.ends_with("+json") || media_type == "application/json" {
            return BodyKind::Json;
        }
        if media_type.starts_with("text/")
            || media_type.ends_with("+xml")
            || media_type == "application/xml"
            || media_type == "application/rss+xml"
            || media_type == "application/atom+xml"
        {
            return BodyKind::Text;
        }
        return BodyKind::UnsupportedBinary;
    }

    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    let trimmed = prefix.trim_start();
    if trimmed.starts_with("<!doctype") || trimmed.starts_with("<html") || trimmed.contains("<html")
    {
        BodyKind::Html
    } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
        BodyKind::Json
    } else if bytes.iter().take(512).any(|byte| *byte == 0) {
        BodyKind::UnsupportedBinary
    } else {
        BodyKind::Text
    }
}

fn blocked_reason(status: reqwest::StatusCode, body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();
    if lower.contains("captcha") || lower.contains("verify you are human") {
        return Some("bot_challenge");
    }
    match status.as_u16() {
        401 => Some("authentication_required"),
        403 => Some("forbidden"),
        429 => Some("rate_limited"),
        503 if lower.contains("cloudflare") || lower.contains("checking your browser") => {
            Some("bot_challenge")
        }
        503 => Some("temporarily_unavailable"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// HTML extraction helpers
// ---------------------------------------------------------------------------

/// Strip HTML tags and convert to readable text. This is a simple, fast
/// implementation — not a full parser, but good enough for most web pages.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let input = html;

    // Step 1: Remove <script> and <style> blocks entirely.
    let input = strip_blocks(input, "script");
    let input = strip_blocks(&input, "style");

    // Step 2: Replace block-level tags with newlines for readability.
    let block_tags = [
        "</p>", "</div>", "</li>", "</h1>", "</h2>", "</h3>", "</h4>", "</h5>", "</h6>", "</tr>",
        "<br>", "<br/>", "<br />",
    ];

    let mut processed = input.to_string();
    for tag in &block_tags {
        processed = processed.replace(tag, "\n");
    }

    // Step 3: Strip all remaining HTML tags.
    let mut in_tag = false;
    for ch in processed.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(ch);
        }
    }

    // Step 4: Decode common HTML entities.
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");

    // Step 5: Collapse whitespace — multiple spaces/tabs become one space,
    // multiple blank lines become a single blank line.
    collapse_whitespace(&out)
}

/// Remove all `<tag ...>...</tag>` blocks (case-insensitive, non-greedy).
fn strip_blocks(input: &str, tag: &str) -> String {
    let lower = input.to_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let mut result = String::with_capacity(input.len());
    let mut pos = 0;

    loop {
        match lower[pos..].find(&open) {
            Some(start) => {
                result.push_str(&input[pos..pos + start]);
                match lower[pos + start..].find(&close) {
                    Some(end) => {
                        pos = pos + start + end + close.len();
                    }
                    None => {
                        // Unclosed tag — skip to end.
                        break;
                    }
                }
            }
            None => {
                result.push_str(&input[pos..]);
                break;
            }
        }
    }

    result
}

/// Collapse runs of whitespace: spaces/tabs → single space per line,
/// 2+ consecutive blank lines → single blank line.
fn collapse_whitespace(input: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in input.lines() {
        // Collapse horizontal whitespace within the line.
        let trimmed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(trimmed);
    }

    // Collapse consecutive blank lines.
    let mut out = String::with_capacity(input.len());
    let mut prev_blank = false;
    for line in &lines {
        if line.is_empty() {
            if !prev_blank {
                out.push('\n');
                prev_blank = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
            prev_blank = false;
        }
    }

    out.trim().to_string()
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let raw = html[open_end..end].trim();
    if raw.is_empty() {
        None
    } else {
        Some(
            raw.replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&apos;", "'")
                .replace("&nbsp;", " "),
        )
    }
}

fn clean_text(input: &str) -> Option<String> {
    let decoded = input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    let collapsed = collapse_whitespace(&decoded);
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

fn selector(css: &str) -> Option<Selector> {
    Selector::parse(css).ok()
}

fn first_selected_text(document: &Html, css: &str) -> Option<String> {
    let selector = selector(css)?;
    document
        .select(&selector)
        .find_map(|node| clean_text(&node.text().collect::<Vec<_>>().join(" ")))
}

fn first_selected_attr(document: &Html, css: &str, attr: &str) -> Option<String> {
    let selector = selector(css)?;
    document
        .select(&selector)
        .find_map(|node| node.value().attr(attr).and_then(clean_text))
}

fn meta_content(document: &Html, keys: &[&str]) -> Option<String> {
    let selector = selector("meta")?;
    document.select(&selector).find_map(|node| {
        let value = node
            .value()
            .attr("property")
            .or_else(|| node.value().attr("name"))
            .or_else(|| node.value().attr("itemprop"))?
            .to_ascii_lowercase();
        if keys.iter().any(|key| value == key.to_ascii_lowercase()) {
            node.value().attr("content").and_then(clean_text)
        } else {
            None
        }
    })
}

fn public_absolute_url(base_url: &reqwest::Url, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("javascript:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
    {
        return None;
    }
    let url = base_url.join(raw).ok()?;
    validate_url(url.as_str()).ok()?;
    Some(url.to_string())
}

fn extract_page_metadata(html: &str, base_url: &reqwest::Url) -> PageMetadata {
    let document = Html::parse_document(html);
    let title = meta_content(&document, &["og:title", "twitter:title", "title"])
        .or_else(|| first_selected_text(&document, "title"))
        .or_else(|| first_selected_text(&document, "h1"))
        .or_else(|| extract_title(html).and_then(|title| clean_text(&title)));
    let description = meta_content(
        &document,
        &["og:description", "twitter:description", "description"],
    );
    let byline = meta_content(&document, &["author", "article:author"]);
    let site_name = meta_content(&document, &["og:site_name", "application-name"]);
    let published_time = meta_content(
        &document,
        &[
            "article:published_time",
            "datepublished",
            "date",
            "pubdate",
            "publishdate",
        ],
    );
    let image = meta_content(
        &document,
        &[
            "og:image:secure_url",
            "og:image:url",
            "og:image",
            "twitter:image",
            "thumbnail",
            "image",
        ],
    )
    .and_then(|url| public_absolute_url(base_url, &url));
    let lang = first_selected_attr(&document, "html", "lang");

    PageMetadata {
        title,
        description,
        byline,
        site_name,
        lang,
        published_time,
        image,
    }
}

fn push_image_asset(
    assets: &mut Vec<ImageAsset>,
    seen: &mut HashSet<String>,
    base_url: &reqwest::Url,
    candidate: ImageAssetCandidate<'_>,
) {
    if assets.len() >= MAX_IMAGE_ASSETS {
        return;
    }
    let Some(url) = public_absolute_url(base_url, candidate.raw_url) else {
        return;
    };
    if !seen.insert(url.clone()) {
        return;
    }
    assets.push(ImageAsset {
        kind: candidate.kind.to_string(),
        url,
        alt: candidate.alt,
        width: candidate.width,
        height: candidate.height,
    });
}

fn parse_u32_attr(value: Option<&str>) -> Option<u32> {
    value?.trim().parse::<u32>().ok()
}

fn best_srcset_url(srcset: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (index, candidate) in srcset.split(',').enumerate() {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else {
            continue;
        };
        let descriptor = parts.next().unwrap_or_default();
        let score = descriptor
            .strip_suffix('w')
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                descriptor
                    .strip_suffix('x')
                    .and_then(|value| value.parse::<f32>().ok())
                    .map(|value| (value * 1000.0) as usize)
            })
            .unwrap_or(index + 1);
        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best = Some((score, url.to_string())),
        }
    }
    best.map(|(_, url)| url)
}

fn extract_image_assets(
    html: &str,
    base_url: &reqwest::Url,
    primary_image: Option<&str>,
) -> Vec<ImageAsset> {
    let document = Html::parse_document(html);
    let mut assets = Vec::new();
    let mut seen = HashSet::new();

    if let Some(image) = primary_image {
        push_image_asset(
            &mut assets,
            &mut seen,
            base_url,
            ImageAssetCandidate::new("primary_image", image),
        );
    }

    for key in [
        "og:image:secure_url",
        "og:image:url",
        "og:image",
        "twitter:image",
        "thumbnail",
        "image",
    ] {
        if let Some(url) = meta_content(&document, &[key]) {
            push_image_asset(
                &mut assets,
                &mut seen,
                base_url,
                ImageAssetCandidate::new("metadata_image", &url),
            );
        }
    }

    if let Some(selector) = selector(r#"link[rel~="image_src"]"#) {
        for node in document.select(&selector) {
            if let Some(href) = node.value().attr("href") {
                push_image_asset(
                    &mut assets,
                    &mut seen,
                    base_url,
                    ImageAssetCandidate::new("linked_image", href),
                );
            }
        }
    }

    if let Some(selector) = selector("img") {
        for node in document.select(&selector) {
            let alt = node.value().attr("alt").and_then(clean_text);
            let width = parse_u32_attr(node.value().attr("width"));
            let height = parse_u32_attr(node.value().attr("height"));
            if let Some(srcset) = node.value().attr("srcset") {
                if let Some(url) = best_srcset_url(srcset) {
                    push_image_asset(
                        &mut assets,
                        &mut seen,
                        base_url,
                        ImageAssetCandidate::new("image_srcset", &url).with_details(
                            alt.clone(),
                            width,
                            height,
                        ),
                    );
                }
            }
            for attr in ["src", "data-src", "data-original", "data-lazy-src"] {
                if let Some(src) = node.value().attr(attr) {
                    push_image_asset(
                        &mut assets,
                        &mut seen,
                        base_url,
                        ImageAssetCandidate::new("image", src).with_details(
                            alt.clone(),
                            width,
                            height,
                        ),
                    );
                }
            }
        }
    }

    if let Some(selector) = selector("source") {
        for node in document.select(&selector) {
            if let Some(srcset) = node.value().attr("srcset") {
                if let Some(url) = best_srcset_url(srcset) {
                    push_image_asset(
                        &mut assets,
                        &mut seen,
                        base_url,
                        ImageAssetCandidate::new("picture_source", &url),
                    );
                }
            }
        }
    }

    assets
}

fn metadata_text(metadata: &PageMetadata) -> String {
    let mut lines = Vec::new();
    if let Some(title) = &metadata.title {
        lines.push(format!("Title: {title}"));
    }
    if let Some(description) = &metadata.description {
        lines.push(format!("Description: {description}"));
    }
    if let Some(site_name) = &metadata.site_name {
        lines.push(format!("Site: {site_name}"));
    }
    if let Some(byline) = &metadata.byline {
        lines.push(format!("Author: {byline}"));
    }
    if let Some(published_time) = &metadata.published_time {
        lines.push(format!("Published: {published_time}"));
    }
    if lines.is_empty() {
        "No readable body text was found. The page may require browser rendering.".to_string()
    } else {
        lines.join("\n")
    }
}

fn merge_readability_metadata(
    metadata: &PageMetadata,
    article: &readabilityrs::Article,
) -> PageMetadata {
    PageMetadata {
        title: article.title.clone().or_else(|| metadata.title.clone()),
        description: article
            .excerpt
            .clone()
            .or_else(|| metadata.description.clone()),
        byline: article.byline.clone().or_else(|| metadata.byline.clone()),
        site_name: article
            .site_name
            .clone()
            .or_else(|| metadata.site_name.clone()),
        lang: article.lang.clone().or_else(|| metadata.lang.clone()),
        published_time: article
            .published_time
            .clone()
            .or_else(|| metadata.published_time.clone()),
        image: article.image.clone().or_else(|| metadata.image.clone()),
    }
}

fn extract_with_readability(
    html: &str,
    base_url: &reqwest::Url,
    metadata: &PageMetadata,
) -> Option<(HtmlExtraction, PageMetadata)> {
    let options = ReadabilityOptions::builder()
        .char_threshold(80)
        .nb_top_candidates(8)
        .remove_title_from_content(false)
        .clean_whitespace(true)
        .build();
    let article = Readability::new(html, Some(base_url.as_str()), Some(options))
        .ok()?
        .parse()?;
    let merged = merge_readability_metadata(metadata, &article);
    let text = article
        .content
        .as_deref()
        .map(html_to_text)
        .or(article.text_content.as_deref().and_then(clean_text))?;
    if text.chars().count() < 80 && metadata.description.is_some() {
        return None;
    }
    Some((
        HtmlExtraction {
            text,
            title: merged.title.clone(),
            method: "readability",
            metadata: merged.clone(),
        },
        merged,
    ))
}

fn extract_main_or_body_text(html: &str) -> Option<String> {
    let cleaned = strip_blocks(&strip_blocks(html, "script"), "style");
    let document = Html::parse_document(&cleaned);
    let mut best: Option<String> = None;
    for css in ["article", "main", r#"[role="main"]"#, "body"] {
        let Some(selector) = selector(css) else {
            continue;
        };
        for node in document.select(&selector) {
            let Some(text) = clean_text(&node.text().collect::<Vec<_>>().join(" ")) else {
                continue;
            };
            if text.chars().count() < 40 {
                continue;
            }
            if best
                .as_ref()
                .map(|current| text.chars().count() > current.chars().count())
                .unwrap_or(true)
            {
                best = Some(text);
            }
        }
        if best.is_some() && css != "body" {
            break;
        }
    }
    best
}

fn extract_html_text(
    html: &str,
    base_url: &reqwest::Url,
    mode: FetchMode,
    metadata: &PageMetadata,
) -> HtmlExtraction {
    if matches!(mode, FetchMode::Metadata | FetchMode::Assets) {
        return HtmlExtraction {
            text: metadata_text(metadata),
            title: metadata.title.clone(),
            method: "metadata",
            metadata: metadata.clone(),
        };
    }

    if !matches!(mode, FetchMode::Text) {
        if let Some((extraction, _merged)) = extract_with_readability(html, base_url, metadata) {
            return extraction;
        }
    }

    if let Some(text) = extract_main_or_body_text(html) {
        return HtmlExtraction {
            text,
            title: metadata.title.clone(),
            method: "main_fallback",
            metadata: metadata.clone(),
        };
    }

    let text = html_to_text(html);
    if text.chars().count() >= 40 {
        HtmlExtraction {
            text,
            title: metadata.title.clone(),
            method: "html_basic",
            metadata: metadata.clone(),
        }
    } else {
        HtmlExtraction {
            text: metadata_text(metadata),
            title: metadata.title.clone(),
            method: "metadata",
            metadata: metadata.clone(),
        }
    }
}

fn html_render_reason(
    html: &str,
    extraction: &HtmlExtraction,
    mode: FetchMode,
    policy: FetchRenderPolicy,
) -> Option<&'static str> {
    if matches!(mode, FetchMode::Metadata | FetchMode::Assets) {
        return None;
    }
    match policy {
        FetchRenderPolicy::Never => None,
        FetchRenderPolicy::Always => Some("requested"),
        FetchRenderPolicy::Auto => html_needs_browser_render(html, extraction),
    }
}

fn html_needs_browser_render(html: &str, extraction: &HtmlExtraction) -> Option<&'static str> {
    let lower = html.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "enable javascript",
            "enable js",
            "javascript is disabled",
            "requires javascript",
            "require javascript",
            "please enable javascript",
            "please turn on javascript",
            "请启用 javascript",
            "请开启 javascript",
            "需要启用 javascript",
            "请启用js",
            "请开启js",
        ],
    ) {
        return Some("javascript_required_message");
    }

    let extracted_len = extraction.text.chars().count();
    if extraction.method != "metadata" && extracted_len >= 120 {
        return None;
    }

    let document = Html::parse_document(html);
    let script_count = selector("script")
        .map(|selector| document.select(&selector).count())
        .unwrap_or(0);
    let has_mount = [
        "#root",
        "#app",
        "#__next",
        "#___gatsby",
        r#"[data-reactroot]"#,
        r#"[ng-version]"#,
    ]
    .iter()
    .any(|css| {
        selector(css)
            .map(|selector| document.select(&selector).next().is_some())
            .unwrap_or(false)
    });

    if has_mount && script_count > 0 && extracted_len < 160 {
        return Some("app_shell");
    }

    if script_count >= 4
        && extracted_len < 120
        && contains_any(
            &lower,
            &[
                "__next_data__",
                "__nuxt__",
                "__initial_state__",
                "webpack",
                "vite",
                "react",
                "vue",
                "angular",
            ],
        )
    {
        return Some("script_heavy_shell");
    }

    None
}

fn rendered_html_is_better(
    policy: FetchRenderPolicy,
    static_extraction: &HtmlExtraction,
    rendered_extraction: &HtmlExtraction,
) -> bool {
    let rendered_len = rendered_extraction.text.chars().count();
    if rendered_len < 80 {
        return false;
    }
    let static_len = static_extraction.text.chars().count();
    policy == FetchRenderPolicy::Always
        || static_extraction.method == "metadata"
        || rendered_len > static_len.saturating_add(80)
}

fn truncate_note(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let cutoff = value
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    let mut out = value[..cutoff].trim_end().to_string();
    out.push_str("...");
    out
}

fn browser_internal_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower == "about:blank"
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("chrome-error://")
}

fn browser_request_allowed(
    url: &str,
    cache: &Mutex<HashMap<String, bool>>,
    allow_loopback: bool,
) -> bool {
    if browser_internal_url(url) {
        return true;
    }
    if let Ok(cache) = cache.lock() {
        if let Some(allowed) = cache.get(url) {
            return *allowed;
        }
    }

    let allowed = validate_url_for_browser_capture_blocking(url, allow_loopback).is_ok();
    if let Ok(mut cache) = cache.lock() {
        cache.insert(url.to_string(), allowed);
    }
    allowed
}

fn push_browser_diagnostic(target: &mut Vec<String>, value: String) {
    const MAX_DIAGNOSTICS_PER_KIND: usize = 50;
    const MAX_DIAGNOSTIC_CHARS: usize = 2_000;
    let value = truncate_note(&value, MAX_DIAGNOSTIC_CHARS);
    if target.len() >= MAX_DIAGNOSTICS_PER_KIND || target.iter().any(|existing| existing == &value)
    {
        return;
    }
    target.push(value);
}

fn extract_interactive_elements(html: &str) -> Vec<serde_json::Value> {
    let document = Html::parse_document(html);
    let Ok(selector) =
        Selector::parse("a[href], button, input, textarea, select, [role=button], [role=link]")
    else {
        return Vec::new();
    };
    document
        .select(&selector)
        .take(100)
        .map(|element| {
            let value = element.value();
            let text = element
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            serde_json::json!({
                "tag": value.name(),
                "text": truncate_note(&text, 240),
                "id": value.attr("id"),
                "role": value.attr("role"),
                "ariaLabel": value.attr("aria-label"),
                "name": value.attr("name"),
                "href": value.attr("href"),
                "type": value.attr("type"),
            })
        })
        .collect()
}

async fn render_html_with_browser(url: reqwest::Url) -> Result<BrowserRenderedHtml, String> {
    tokio::task::spawn_blocking(move || render_html_with_browser_blocking(url))
        .await
        .map_err(|e| format!("browser render task failed: {e}"))?
}

pub(crate) async fn capture_browser_page(url: &str) -> Result<BrowserRenderedHtml, String> {
    let url = validate_url_for_browser_capture(url).await?;
    render_html_with_browser(url).await
}

fn browser_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("NEXA_BROWSER_EXECUTABLE") {
        candidates.push(PathBuf::from(path));
    }

    #[cfg(windows)]
    {
        for base in [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let base = PathBuf::from(base);
            candidates.push(base.join("Google/Chrome/Application/chrome.exe"));
            candidates.push(base.join("Microsoft/Edge/Application/msedge.exe"));
        }
    }

    #[cfg(target_os = "linux")]
    for candidate in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ] {
        candidates.push(PathBuf::from(candidate));
    }

    candidates.retain(|candidate| candidate.is_file());
    candidates.dedup();
    candidates
}

fn launch_browser_for_capture() -> Result<headless_chrome::Browser, String> {
    use headless_chrome::{Browser, LaunchOptionsBuilder};

    let mut launch_errors = Vec::new();
    for executable in browser_executable_candidates() {
        let options = LaunchOptionsBuilder::default()
            .headless(true)
            .path(Some(executable.clone()))
            .build()
            .map_err(|error| format!("invalid browser launch options: {error}"))?;
        match Browser::new(options) {
            Ok(browser) => return Ok(browser),
            Err(error) => launch_errors.push(format!("{}: {error}", executable.display())),
        }
    }

    Browser::default().map_err(|error| {
        let attempted = if launch_errors.is_empty() {
            "no configured or installed Edge/Chrome/Chromium candidate was found".to_string()
        } else {
            launch_errors.join("; ")
        };
        format!(
            "failed to launch browser: {error}. Attempted fallbacks: {attempted}. Set NEXA_BROWSER_EXECUTABLE to a Chrome, Edge, or Chromium executable."
        )
    })
}

fn render_html_with_browser_blocking(url: reqwest::Url) -> Result<BrowserRenderedHtml, String> {
    use headless_chrome::browser::tab::RequestPausedDecision;
    use headless_chrome::protocol::cdp::types::Event;
    use headless_chrome::protocol::cdp::Fetch::{
        events::RequestPausedEvent, FailRequest, RequestPattern, RequestStage,
    };
    use headless_chrome::protocol::cdp::Network;
    use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
    let browser = launch_browser_for_capture()?;
    let tab = browser
        .new_tab()
        .map_err(|e| format!("failed to open browser tab: {e}"))?;
    tab.set_default_timeout(Duration::from_secs(JS_RENDER_TIMEOUT_SECS));
    let _ = tab.set_user_agent(
        "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko) Nexa/0.8 browser-renderer",
        Some("zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7"),
        None,
    );

    let allow_loopback = is_loopback_url(&url);
    let diagnostics = Arc::new(Mutex::new(BrowserDiagnostics::default()));
    tab.enable_log()
        .and_then(|tab| tab.enable_runtime())
        .map_err(|error| format!("failed to enable browser diagnostics: {error}"))?;
    tab.call_method(Network::Enable {
        max_total_buffer_size: None,
        max_resource_buffer_size: None,
        max_post_data_size: None,
        report_direct_socket_traffic: None,
        enable_durable_messages: None,
    })
    .map_err(|error| format!("failed to enable browser network diagnostics: {error}"))?;
    let diagnostics_for_listener = Arc::clone(&diagnostics);
    tab.add_event_listener(Arc::new(move |event: &Event| {
        let Ok(mut diagnostics) = diagnostics_for_listener.lock() else {
            return;
        };
        match event {
            Event::LogEntryAdded(event) => push_browser_diagnostic(
                &mut diagnostics.console_entries,
                format!(
                    "{:?}: {}",
                    event.params.entry.level, event.params.entry.text
                ),
            ),
            Event::RuntimeConsoleAPICalled(event) => push_browser_diagnostic(
                &mut diagnostics.console_entries,
                format!("{:?}", event.params),
            ),
            Event::RuntimeExceptionThrown(event) => {
                let details = &event.params.exception_details;
                push_browser_diagnostic(
                    &mut diagnostics.runtime_exceptions,
                    format!(
                        "{} at {}:{}:{}",
                        details.text,
                        details.url.as_deref().unwrap_or("<inline>"),
                        details.line_number,
                        details.column_number,
                    ),
                );
            }
            Event::NetworkLoadingFailed(event) => push_browser_diagnostic(
                &mut diagnostics.network_failures,
                format!(
                    "{:?} request {:?}: {} (blocked: {:?})",
                    event.params.Type,
                    event.params.request_id,
                    event.params.error_text,
                    event.params.blocked_reason,
                ),
            ),
            Event::NetworkResponseReceived(event) if event.params.response.status >= 400 => {
                push_browser_diagnostic(
                    &mut diagnostics.http_errors,
                    format!(
                        "{} {} {}",
                        event.params.response.status,
                        event.params.response.status_text,
                        event.params.response.url,
                    ),
                );
            }
            _ => {}
        }
    }))
    .map_err(|error| format!("failed to install browser diagnostics listener: {error}"))?;

    let blocked_requests = Arc::new(AtomicUsize::new(0));
    let request_cache = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
    let blocked_for_interceptor = Arc::clone(&blocked_requests);
    let cache_for_interceptor = Arc::clone(&request_cache);
    tab.enable_fetch(
        Some(&[RequestPattern {
            url_pattern: None,
            resource_Type: None,
            request_stage: Some(RequestStage::Request),
        }]),
        None,
    )
    .map_err(|e| format!("failed to enable browser request validation: {e}"))?;
    tab.enable_request_interception(Arc::new(
        move |_transport, _session_id, intercepted: RequestPausedEvent| {
            if browser_request_allowed(
                &intercepted.params.request.url,
                cache_for_interceptor.as_ref(),
                allow_loopback,
            ) {
                RequestPausedDecision::Continue(None)
            } else {
                blocked_for_interceptor.fetch_add(1, Ordering::Relaxed);
                RequestPausedDecision::Fail(FailRequest {
                    request_id: intercepted.params.request_id,
                    error_reason: Network::ErrorReason::BlockedByClient,
                })
            }
        },
    ))
    .map_err(|e| format!("failed to install browser request validator: {e}"))?;

    tab.navigate_to(url.as_str())
        .map_err(|e| format!("browser navigation failed: {e}"))?;
    std::thread::sleep(Duration::from_millis(JS_RENDER_SETTLE_MS));

    let final_url = validate_url_for_browser_capture_blocking(&tab.get_url(), allow_loopback)?;
    let mut html = tab
        .get_content()
        .map_err(|e| format!("failed to read rendered HTML: {e}"))?;
    if html.len() > MAX_BODY_BYTES {
        let mut cutoff = MAX_BODY_BYTES;
        while cutoff > 0 && !html.is_char_boundary(cutoff) {
            cutoff -= 1;
        }
        html.truncate(cutoff);
    }
    let screenshot_png = tab
        .capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
        .map_err(|e| format!("failed to capture rendered page screenshot: {e}"))?;
    let metadata = extract_page_metadata(&html, &final_url);
    let rendered = extract_html_text(&html, &final_url, FetchMode::Auto, &metadata);
    let interactive_elements = extract_interactive_elements(&html);
    let diagnostics = diagnostics
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();

    Ok(BrowserRenderedHtml {
        final_url,
        html,
        title: rendered.title,
        rendered_text: truncate_note(&rendered.text, 20_000),
        interactive_elements,
        diagnostics,
        blocked_requests: blocked_requests.load(Ordering::Relaxed),
        screenshot_png,
    })
}

fn truncate_text_for_output(text: &mut String, max_chars: usize) -> bool {
    if text.chars().count() <= max_chars {
        return false;
    }

    let cutoff = text
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.truncate(cutoff);
    if let Some(last_space) = text.rfind(' ') {
        text.truncate(last_space);
    }
    text.push_str("\n\n[… truncated]");
    true
}

async fn render_fetch_payload(
    call_id: &str,
    args: &FetchUrlArgs,
    max_length: usize,
    payload: FetchBodyPayload,
) -> Result<ToolResult, CoreError> {
    let FetchBodyPayload {
        mut final_url,
        content_type,
        body_bytes,
        body_truncated,
        redirect_count,
        cache_status,
    } = payload;
    let body_kind = classify_body_kind(content_type.as_deref(), &body_bytes);
    let body = decode_body(&body_bytes, content_type.as_deref());
    let mut metadata = PageMetadata::default();
    let mut assets: Vec<ImageAsset> = Vec::new();
    let mut js_render_attempted = false;
    let mut js_render_used = false;
    let mut js_render_reason: Option<&'static str> = None;
    let mut js_render_error: Option<String> = None;
    let mut js_render_blocked_requests = 0usize;
    let (mut text, title, extraction_method) = match body_kind {
        BodyKind::Html => {
            metadata = extract_page_metadata(&body, &final_url);
            let mut extraction = extract_html_text(&body, &final_url, args.mode, &metadata);
            if let Some(reason) = html_render_reason(&body, &extraction, args.mode, args.render_js)
            {
                js_render_attempted = true;
                js_render_reason = Some(reason);
                match render_html_with_browser(final_url.clone()).await {
                    Ok(rendered) => {
                        js_render_blocked_requests = rendered.blocked_requests;
                        let rendered_metadata =
                            extract_page_metadata(&rendered.html, &rendered.final_url);
                        let rendered_extraction = extract_html_text(
                            &rendered.html,
                            &rendered.final_url,
                            args.mode,
                            &rendered_metadata,
                        );
                        if rendered_html_is_better(
                            args.render_js,
                            &extraction,
                            &rendered_extraction,
                        ) {
                            final_url = rendered.final_url;
                            metadata = rendered_extraction.metadata.clone();
                            extraction = HtmlExtraction {
                                method: rendered_extraction.method,
                                ..rendered_extraction
                            };
                            js_render_used = true;
                            if args.include_assets || args.mode == FetchMode::Assets {
                                assets = extract_image_assets(
                                    &rendered.html,
                                    &final_url,
                                    metadata.image.as_deref(),
                                );
                            }
                        } else {
                            js_render_error = Some(
                                "browser-rendered page did not expose more readable text"
                                    .to_string(),
                            );
                        }
                    }
                    Err(error) => {
                        js_render_error = Some(truncate_note(&error, 240));
                    }
                }
            }
            if !js_render_used {
                metadata = extraction.metadata.clone();
            }
            if (args.include_assets || args.mode == FetchMode::Assets) && assets.is_empty() {
                assets = extract_image_assets(&body, &final_url, metadata.image.as_deref());
            }
            (
                extraction.text,
                extraction.title.or_else(|| metadata.title.clone()),
                if js_render_used {
                    format!("browser_{}", extraction.method)
                } else {
                    extraction.method.to_string()
                },
            )
        }
        BodyKind::Json => {
            let pretty = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| collapse_whitespace(&body));
            (pretty, None, "json".to_string())
        }
        BodyKind::Text => {
            let method = if content_type
                .as_deref()
                .is_some_and(|value| value.to_ascii_lowercase().contains("markdown"))
                || final_url.path().to_ascii_lowercase().ends_with(".md")
            {
                "markdown"
            } else {
                "plain_text"
            };
            (collapse_whitespace(&body), None, method.to_string())
        }
        BodyKind::UnsupportedBinary => {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "URL: {}\nFinal URL: {}\nContent type: {}\n---\nfetch_url only returns readable text. Use download_asset for supported remote images.",
                    args.url,
                    final_url,
                    content_type.as_deref().unwrap_or("unknown")
                ),
                is_error: true,
                artifacts: Some(serde_json::json!({
                    "kind": "fetchUrl",
                    "url": args.url.as_str(),
                    "finalUrl": final_url.as_str(),
                    "truncated": false,
                    "bodyTruncated": body_truncated,
                    "contentType": content_type,
                    "redirectCount": redirect_count,
                    "extractionMethod": "unsupported_binary",
                    "assets": [{
                        "kind": "direct_asset",
                        "url": final_url.to_string(),
                        "alt": null,
                        "width": null,
                        "height": null,
                    }],
                    "cacheStatus": cache_status,
                })),
            });
        }
    };

    let output_truncated = truncate_text_for_output(&mut text, max_length);
    if body_truncated && !output_truncated {
        text.push_str("\n\n[… truncated at download limit]");
    }
    let truncated = output_truncated || body_truncated;

    let mut content = format!(
        "URL: {}\nFinal URL: {}\nTitle: {}\nExtraction: {}\nSuggested citation: [url:{}|{}]\n---\n{}",
        args.url,
        final_url,
        title.as_deref().unwrap_or("(untitled page)"),
        extraction_method,
        final_url,
        title.as_deref().unwrap_or("web page"),
        text
    );
    if js_render_attempted && !js_render_used {
        content.push_str("\n\nFetch note: This page appears to require JavaScript rendering");
        if let Some(reason) = js_render_reason {
            content.push_str(&format!(" ({reason})"));
        }
        if let Some(error) = &js_render_error {
            content.push_str(&format!(
                ", but browser rendering did not provide usable text: {error}"
            ));
        } else {
            content.push_str(", but browser rendering did not provide usable text.");
        }
    }
    if !assets.is_empty() {
        content.push_str("\n\nImage candidates:\n");
        for asset in assets.iter().take(8) {
            content.push_str(&format!("- {} ({})\n", asset.url, asset.kind));
        }
        if assets.len() > 8 {
            content.push_str(&format!("- … {} more\n", assets.len() - 8));
        }
        content.push_str("Use download_asset to save a supported image candidate.");
    }

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: false,
        artifacts: Some(serde_json::json!({
            "kind": "fetchUrl",
            "url": args.url.as_str(),
            "finalUrl": final_url.as_str(),
            "title": title,
            "metadata": metadata,
            "assets": assets,
            "truncated": truncated,
            "bodyTruncated": body_truncated,
            "contentType": content_type,
            "redirectCount": redirect_count,
            "extractionMethod": extraction_method,
            "blockedReason": null,
            "jsRender": {
                "policy": args.render_js.as_str(),
                "attempted": js_render_attempted,
                "used": js_render_used,
                "reason": js_render_reason,
                "error": js_render_error,
                "blockedRequests": js_render_blocked_requests,
            },
            "cacheStatus": cache_status,
        })),
    })
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for FetchUrlTool {
    fn name(&self) -> &str {
        "fetch_url"
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
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: FetchUrlArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid fetch_url arguments: {e}")))?;

        // Validate the URL.
        let parsed_url = match validate_url_for_fetch(&args.url).await {
            Ok(u) => u,
            Err(msg) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: msg,
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        let max_length = if args.max_length == 0 {
            default_max_length()
        } else {
            args.max_length
        };

        let client = build_http_client()
            .map_err(|e| CoreError::InvalidInput(format!("Failed to build HTTP client: {e}")))?;

        let fetch_cache_key = parsed_url.to_string();
        let cached_body = cached_fetch_body(&fetch_cache_key);
        let conditional = conditional_headers_from_cache(cached_body.as_ref());
        let (response, final_url, redirect_count) =
            match send_with_safe_redirects_conditional(&client, parsed_url, conditional.as_ref())
                .await
            {
                Ok(result) => result,
                Err(e) => {
                    return Ok(ToolResult {
                        call_id: call_id.to_string(),
                        content: e,
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if status == reqwest::StatusCode::NOT_MODIFIED {
            let Some(payload) =
                cached_body.and_then(|cached| cached_payload(cached, redirect_count))
            else {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "HTTP {status} fetching {final_url}, but no cached body was available"
                    ),
                    is_error: true,
                    artifacts: Some(serde_json::json!({
                        "kind": "fetchUrl",
                        "url": args.url,
                        "finalUrl": final_url.as_str(),
                        "status": status.as_u16(),
                        "blockedReason": null,
                        "contentType": content_type,
                        "redirectCount": redirect_count,
                        "bodyTruncated": false,
                        "cacheStatus": "validator_miss",
                    })),
                });
            };
            return render_fetch_payload(call_id, &args, max_length, payload).await;
        }
        if !status.is_success() {
            let (error_bytes, error_truncated) = read_limited_body(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            let error_body = decode_body(&error_bytes, content_type.as_deref());
            let reason = blocked_reason(status, &error_body);
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "HTTP {status} fetching {}{}",
                    final_url,
                    reason
                        .map(|value| format!("\nBlocked reason: {value}"))
                        .unwrap_or_default()
                ),
                is_error: true,
                artifacts: Some(serde_json::json!({
                    "kind": "fetchUrl",
                    "url": args.url,
                    "finalUrl": final_url.as_str(),
                    "status": status.as_u16(),
                    "blockedReason": reason,
                    "contentType": content_type,
                    "redirectCount": redirect_count,
                    "bodyTruncated": error_truncated,
                    "cacheStatus": "bypass",
                })),
            });
        }

        let response_headers = response.headers().clone();
        let (body_bytes, body_truncated) = match read_limited_body(response, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: e,
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        let body_kind = classify_body_kind(content_type.as_deref(), &body_bytes);
        if body_kind == BodyKind::UnsupportedBinary {
            let direct_asset = ImageAsset {
                kind: "direct_asset".to_string(),
                url: final_url.to_string(),
                alt: None,
                width: None,
                height: None,
            };
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "URL: {}\nFinal URL: {}\nContent type: {}\n---\nfetch_url only returns readable text. Use download_asset for supported remote images.",
                    args.url,
                    final_url,
                    content_type.as_deref().unwrap_or("unknown")
                ),
                is_error: true,
                artifacts: Some(serde_json::json!({
                    "kind": "fetchUrl",
                    "url": args.url,
                    "finalUrl": final_url.as_str(),
                    "truncated": false,
                    "bodyTruncated": body_truncated,
                    "contentType": content_type,
                    "redirectCount": redirect_count,
                    "extractionMethod": "unsupported_binary",
                    "assets": [direct_asset],
                    "cacheStatus": "bypass",
                })),
            });
        }

        store_fetch_body(
            &fetch_cache_key,
            &final_url,
            &response_headers,
            content_type.clone(),
            &body_bytes,
            body_truncated,
        );

        render_fetch_payload(
            call_id,
            &args,
            max_length,
            FetchBodyPayload {
                final_url,
                content_type,
                body_bytes,
                body_truncated,
                redirect_count,
                cache_status: "miss",
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_reads_basic_title_tag() {
        let html = "<html><head><title>Example &amp; Test</title></head><body>ok</body></html>";
        assert_eq!(extract_title(html).as_deref(), Some("Example & Test"));
    }

    #[test]
    fn validate_url_rejects_private_ip_literal() {
        let err = validate_url("http://127.0.0.1/admin").unwrap_err();
        assert!(err.contains("localhost") || err.contains("private IP"));
    }

    #[test]
    fn browser_request_validation_allows_internal_urls_but_blocks_private_networks() {
        let cache = Mutex::new(HashMap::new());

        assert!(browser_request_allowed("about:blank", &cache, false));
        assert!(browser_request_allowed(
            "data:text/plain,hello",
            &cache,
            false
        ));
        assert!(!browser_request_allowed(
            "http://localhost/app.js",
            &cache,
            false
        ));
        assert!(!browser_request_allowed(
            "http://127.0.0.1/app.js",
            &cache,
            false
        ));

        let local_debug_cache = Mutex::new(HashMap::new());
        assert!(browser_request_allowed(
            "http://localhost/app.js",
            &local_debug_cache,
            true
        ));
        assert!(browser_request_allowed(
            "http://127.0.0.1/app.js",
            &local_debug_cache,
            true
        ));
        assert!(!browser_request_allowed(
            "http://192.168.1.10/private.js",
            &local_debug_cache,
            true
        ));
    }

    #[test]
    fn browser_capture_normalizes_unspecified_dev_server_hosts() {
        let url = normalize_browser_capture_url("http://0.0.0.0:5173/app").unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:5173/app");
        assert!(is_loopback_url(&url));
    }

    #[test]
    fn conditional_headers_require_cached_validators() {
        assert!(conditional_headers_from_cache(None).is_none());

        let cached = CachedFetchBody {
            final_url: "https://example.com/page".to_string(),
            content_type: Some("text/html".to_string()),
            etag: Some("\"abc\"".to_string()),
            last_modified: None,
            body_bytes: b"<html>cached</html>".to_vec(),
            body_truncated: false,
        };

        let conditional = conditional_headers_from_cache(Some(&cached)).unwrap();
        assert_eq!(conditional.etag.as_deref(), Some("\"abc\""));
        assert_eq!(conditional.last_modified, None);
    }

    #[test]
    fn readable_extraction_prefers_article_body_over_chrome() {
        let url = reqwest::Url::parse("https://example.com/news/story").unwrap();
        let html = r#"
            <html>
              <head><title>Fallback Title</title></head>
              <body>
                <nav>Subscribe Login Trending Markets</nav>
                <article>
                  <h1>Native Fetch Gets Cleaner</h1>
                  <p>This is the first substantial article paragraph, with enough detail to be treated as the central body rather than navigation.</p>
                  <p>This second paragraph continues the same report, adding useful evidence and context for citation workflows.</p>
                </article>
                <footer>Privacy Terms Advertise</footer>
              </body>
            </html>
        "#;

        let metadata = extract_page_metadata(html, &url);
        let extracted = extract_html_text(html, &url, FetchMode::Auto, &metadata);

        assert_eq!(extracted.method, "readability");
        assert!(extracted.text.contains("Native Fetch Gets Cleaner"));
        assert!(extracted.text.contains("central body"));
        assert!(!extracted.text.contains("Subscribe Login Trending"));
        assert!(!extracted.text.contains("Privacy Terms Advertise"));
    }

    #[test]
    fn spa_metadata_fallback_returns_page_summary() {
        let url = reqwest::Url::parse("https://example.com/app").unwrap();
        let html = r#"
            <html lang="zh-CN">
              <head>
                <title>研究面板</title>
                <meta name="description" content="一个用于整理网页证据的研究面板">
                <meta property="og:site_name" content="Example Research">
              </head>
              <body><div id="root"></div><script>window.__APP__ = true;</script></body>
            </html>
        "#;

        let metadata = extract_page_metadata(html, &url);
        let extracted = extract_html_text(html, &url, FetchMode::Auto, &metadata);

        assert_eq!(extracted.method, "metadata");
        assert!(extracted.text.contains("研究面板"));
        assert!(extracted.text.contains("整理网页证据"));
        assert_eq!(metadata.lang.as_deref(), Some("zh-CN"));
    }

    #[test]
    fn spa_shell_requests_browser_render_in_auto_mode() {
        let url = reqwest::Url::parse("https://example.com/app").unwrap();
        let html = r#"
            <html>
              <head><title>Dashboard</title></head>
              <body>
                <div id="root"></div>
                <script src="/assets/app.js"></script>
              </body>
            </html>
        "#;
        let metadata = extract_page_metadata(html, &url);
        let extracted = extract_html_text(html, &url, FetchMode::Auto, &metadata);

        assert_eq!(
            html_render_reason(html, &extracted, FetchMode::Auto, FetchRenderPolicy::Auto),
            Some("app_shell")
        );
        assert_eq!(
            html_render_reason(html, &extracted, FetchMode::Auto, FetchRenderPolicy::Never),
            None
        );
        assert_eq!(
            html_render_reason(
                html,
                &extracted,
                FetchMode::Metadata,
                FetchRenderPolicy::Always
            ),
            None
        );
    }

    #[test]
    fn explicit_javascript_required_message_requests_browser_render() {
        let url = reqwest::Url::parse("https://example.com/report").unwrap();
        let html = r#"
            <html>
              <head><title>Report</title></head>
              <body>
                <noscript>Please enable JavaScript to view this page.</noscript>
                <div id="app"></div>
              </body>
            </html>
        "#;
        let metadata = extract_page_metadata(html, &url);
        let extracted = extract_html_text(html, &url, FetchMode::Auto, &metadata);

        assert_eq!(
            html_needs_browser_render(html, &extracted),
            Some("javascript_required_message")
        );
    }

    #[test]
    fn article_html_does_not_request_browser_render() {
        let url = reqwest::Url::parse("https://example.com/story").unwrap();
        let html = r#"
            <html>
              <head><title>Story</title></head>
              <body>
                <article>
                  <h1>Static pages still work</h1>
                  <p>This static article has enough readable text to be extracted without browser rendering or additional fallback work.</p>
                  <p>The second paragraph makes the body long enough that the JavaScript-render heuristic should stay quiet.</p>
                </article>
              </body>
            </html>
        "#;
        let metadata = extract_page_metadata(html, &url);
        let extracted = extract_html_text(html, &url, FetchMode::Auto, &metadata);

        assert_eq!(html_needs_browser_render(html, &extracted), None);
    }

    #[test]
    fn image_asset_extraction_resolves_metadata_and_srcset_urls() {
        let url = reqwest::Url::parse("https://example.com/articles/story/index.html").unwrap();
        let html = r#"
            <html>
              <head>
                <meta property="og:image" content="/images/cover.jpg">
              </head>
              <body>
                <figure>
                  <img alt="Chart" src="thumb.jpg" srcset="small.jpg 480w, /images/chart-large.webp 1200w">
                </figure>
              </body>
            </html>
        "#;

        let assets = extract_image_assets(html, &url, None);

        assert_eq!(assets[0].url, "https://example.com/images/cover.jpg");
        assert!(assets.iter().any(|asset| asset.url
            == "https://example.com/images/chart-large.webp"
            && asset.alt.as_deref() == Some("Chart")));
    }

    #[test]
    fn binary_content_type_is_reported_as_unsupported() {
        assert_eq!(
            classify_body_kind(Some("image/png"), &[]),
            BodyKind::UnsupportedBinary
        );
        assert_eq!(
            classify_body_kind(Some("application/pdf"), b"%PDF-1.7"),
            BodyKind::UnsupportedBinary
        );
    }

    #[tokio::test]
    #[ignore = "requires a locally installed Chrome or Chromium browser"]
    async fn browser_capture_opens_loopback_and_collects_runtime_diagnostics() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut request = [0_u8; 2_048];
                let read = stream.read(&mut request).unwrap_or_default();
                let request = String::from_utf8_lossy(&request[..read]);
                let missing = request.starts_with("GET /missing.js ");
                let (status, content_type, body) = if missing {
                    ("404 Not Found", "text/javascript", "missing")
                } else {
                    (
                        "200 OK",
                        "text/html; charset=utf-8",
                        r#"<!doctype html><html><head><title>Local debug page</title></head><body><main>Rendered local content</main><button aria-label="Save changes">Save</button><script>console.error('console boom'); throw new Error('runtime boom');</script><script src="/missing.js"></script></body></html>"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        let captured = capture_browser_page(&format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        assert_eq!(captured.title.as_deref(), Some("Local debug page"));
        assert!(captured.rendered_text.contains("Rendered local content"));
        assert!(captured.interactive_elements.iter().any(|element| {
            element.get("ariaLabel").and_then(|value| value.as_str()) == Some("Save changes")
        }));
        assert!(!captured.diagnostics.console_entries.is_empty());
        assert!(!captured.diagnostics.runtime_exceptions.is_empty());
        assert!(captured
            .diagnostics
            .http_errors
            .iter()
            .any(|entry| entry.contains("404")));
    }

    #[test]
    fn truncate_text_preserves_utf8_boundaries() {
        let mut text = "中文内容 mixed words".to_string();
        truncate_text_for_output(&mut text, 5);

        assert!(text.starts_with("中文"));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.ends_with("[… truncated]"));
    }
}
