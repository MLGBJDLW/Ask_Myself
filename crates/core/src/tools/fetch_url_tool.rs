//! FetchUrlTool — fetches public web content and extracts readable text.

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
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

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/fetch_url.json");
const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_IMAGE_ASSETS: usize = 25;
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

pub(crate) async fn validate_url_for_fetch(url: &str) -> Result<reqwest::Url, String> {
    let parsed = validate_url(url)?;
    validate_resolved_host(&parsed).await?;
    Ok(parsed)
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

fn render_fetch_payload(
    call_id: &str,
    args: &FetchUrlArgs,
    max_length: usize,
    payload: FetchBodyPayload,
) -> Result<ToolResult, CoreError> {
    let FetchBodyPayload {
        final_url,
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
    let (mut text, title, extraction_method) = match body_kind {
        BodyKind::Html => {
            metadata = extract_page_metadata(&body, &final_url);
            let extraction = extract_html_text(&body, &final_url, args.mode, &metadata);
            metadata = extraction.metadata.clone();
            if args.include_assets || args.mode == FetchMode::Assets {
                assets = extract_image_assets(&body, &final_url, metadata.image.as_deref());
            }
            (
                extraction.text,
                extraction.title.or_else(|| metadata.title.clone()),
                extraction.method,
            )
        }
        BodyKind::Json => {
            let pretty = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| collapse_whitespace(&body));
            (pretty, None, "json")
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
            (collapse_whitespace(&body), None, method)
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
        call_id: &str,
        arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
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
            return render_fetch_payload(call_id, &args, max_length, payload);
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

    #[test]
    fn truncate_text_preserves_utf8_boundaries() {
        let mut text = "中文内容 mixed words".to_string();
        truncate_text_for_output(&mut text, 5);

        assert!(text.starts_with("中文"));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.ends_with("[… truncated]"));
    }
}
