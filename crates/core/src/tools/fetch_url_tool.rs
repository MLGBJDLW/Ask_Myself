//! FetchUrlTool — fetches web page content and strips HTML.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/fetch_url.json");

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
        }
    }
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
        let parsed_url = match validate_url(&args.url) {
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
            .build()
            .map_err(|e| CoreError::InvalidInput(format!("Failed to build HTTP client: {e}")))?;

        let response = match client.get(parsed_url.as_str()).send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("HTTP request failed: {e}"),
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("HTTP {status} fetching {}", args.url),
                is_error: true,
                artifacts: None,
            });
        }

        let body = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Failed to read response body: {e}"),
                    is_error: true,
                    artifacts: None,
                });
            }
        };

        // Convert HTML to text and truncate.
        let title = extract_title(&body);
        let mut text = html_to_text(&body);
        let truncated = text.len() > max_length;
        if text.len() > max_length {
            text.truncate(max_length);
            // Don't break mid-word — find last space.
            if let Some(last_space) = text.rfind(' ') {
                text.truncate(last_space);
            }
            text.push_str("\n\n[… truncated]");
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "URL: {}\nTitle: {}\nSuggested citation: [url:{}|{}]\n---\n{}",
                args.url,
                title.as_deref().unwrap_or("(untitled page)"),
                args.url,
                title.as_deref().unwrap_or("web page"),
                text
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "url": args.url,
                "title": title,
                "truncated": truncated,
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
}
