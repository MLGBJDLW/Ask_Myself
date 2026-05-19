//! FetchUrlTool — fetches web page content and strips HTML.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use encoding_rs::Encoding;
use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, LOCATION};
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/fetch_url.json");
const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Tool that fetches a web page and returns its text content with HTML
/// tags stripped.
pub struct FetchUrlTool;

#[derive(Deserialize)]
struct FetchUrlArgs {
    url: String,
    #[serde(default = "default_max_length")]
    max_length: usize,
}

fn default_max_length() -> usize {
    5000
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

async fn validate_url_for_fetch(url: &str) -> Result<reqwest::Url, String> {
    let parsed = validate_url(url)?;
    validate_resolved_host(&parsed).await?;
    Ok(parsed)
}

async fn send_with_safe_redirects(
    client: &reqwest::Client,
    initial_url: reqwest::Url,
) -> Result<(reqwest::Response, reqwest::Url, usize), String> {
    let mut current = initial_url;
    for redirect_count in 0..=MAX_REDIRECTS {
        validate_resolved_host(&current).await?;
        let response = client
            .get(current.clone())
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

async fn read_limited_body(
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

// ---------------------------------------------------------------------------
// Basic HTML-to-text conversion
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

fn content_is_html(content_type: Option<&str>, body: &str) -> bool {
    content_type
        .map(|value| value.to_ascii_lowercase().contains("html"))
        .unwrap_or_else(|| body.trim_start().starts_with("<!doctype") || body.contains("<html"))
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

        // Build an async reqwest client.
        let client = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CoreError::InvalidInput(format!("Failed to build HTTP client: {e}")))?;

        let (response, final_url, redirect_count) =
            match send_with_safe_redirects(&client, parsed_url).await {
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
        if !status.is_success() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("HTTP {status} fetching {}", final_url),
                is_error: true,
                artifacts: None,
            });
        }

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

        let body = decode_body(&body_bytes, content_type.as_deref());
        let is_html = content_is_html(content_type.as_deref(), &body);
        let title = if is_html { extract_title(&body) } else { None };
        let extraction_method = if is_html { "html_basic" } else { "plain_text" };

        // Convert content to text and truncate.
        let mut text = if is_html {
            html_to_text(&body)
        } else {
            collapse_whitespace(&body)
        };
        let truncated = text.len() > max_length || body_truncated;
        if text.len() > max_length {
            text.truncate(max_length);
            // Don't break mid-word — find last space.
            if let Some(last_space) = text.rfind(' ') {
                text.truncate(last_space);
            }
            text.push_str("\n\n[… truncated]");
        } else if body_truncated {
            text.push_str("\n\n[… truncated at download limit]");
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "URL: {}\nFinal URL: {}\nTitle: {}\nSuggested citation: [url:{}|{}]\n---\n{}",
                args.url,
                final_url,
                title.as_deref().unwrap_or("(untitled page)"),
                final_url,
                title.as_deref().unwrap_or("web page"),
                text
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "url": args.url,
                "finalUrl": final_url.as_str(),
                "title": title,
                "truncated": truncated,
                "bodyTruncated": body_truncated,
                "contentType": content_type,
                "redirectCount": redirect_count,
                "extractionMethod": extraction_method,
            })),
        })
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
}
