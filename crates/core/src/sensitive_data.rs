//! Redaction helpers for diagnostic data that can cross a persistence boundary.
//!
//! Provider and transport errors may contain request URLs, headers, or opaque
//! response fields. Keep functional request/response data untouched, but scrub
//! those diagnostic copies before they reach logs, traces, run events, or
//! analytics.

use regex::Regex;
use std::sync::OnceLock;

const MAX_DIAGNOSTIC_CHARS: usize = 2_048;

fn redaction_patterns() -> &'static [Regex; 4] {
    static PATTERNS: OnceLock<[Regex; 4]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            Regex::new(r#"(?i)\b(https?://[^\s?#\"'<>]+)\?[^\s#\"'<>]*"#)
                .expect("valid URL query redaction pattern"),
            Regex::new(r#"(?i)(authorization\s*[:=]\s*bearer\s+)[^\s,;\"']+"#)
                .expect("valid Authorization redaction pattern"),
            Regex::new(
                r#"(?i)((?:x-goog-api-key|api[_-]?key|apikey|access[_-]?token|refresh[_-]?token|token|secret)\s*[:=]\s*(?:bearer\s+)?)[^\s,;\"']+"#,
            )
            .expect("valid credential field redaction pattern"),
            Regex::new(r"\bAIza[0-9A-Za-z_-]{16,}\b")
                .expect("valid Google API key redaction pattern"),
        ]
    })
}

fn truncate_chars(value: String) -> String {
    value.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
}

/// Remove inline credentials and all URL query strings from a diagnostic
/// message. The optional explicit secret is replaced before pattern matching
/// so provider-specific key formats remain covered.
pub(crate) fn sanitize_diagnostic(value: &str, explicit_secret: Option<&str>) -> String {
    let mut sanitized = value.to_string();
    if let Some(secret) = explicit_secret.filter(|secret| !secret.is_empty()) {
        sanitized = sanitized.replace(secret, "[REDACTED]");
    }
    for (index, pattern) in redaction_patterns().iter().enumerate() {
        let replacement = if index == 0 {
            "${1}?[REDACTED]"
        } else {
            "${1}[REDACTED]"
        };
        sanitized = pattern.replace_all(&sanitized, replacement).into_owned();
    }
    truncate_chars(sanitized)
}

/// Recursively sanitize string leaves while preserving the JSON structure.
pub(crate) fn sanitize_json_strings(
    value: &serde_json::Value,
    explicit_secret: Option<&str>,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => {
            serde_json::Value::String(sanitize_diagnostic(value, explicit_secret))
        }
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|value| sanitize_json_strings(value, explicit_secret))
                .collect(),
        ),
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_json_strings(value, explicit_secret)))
                .collect(),
        ),
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_redaction_covers_headers_keys_and_signed_urls() {
        let secret = "AIza0123456789abcdefghijklmnopqrst";
        let message = format!(
            "request https://example.test/v1/run?key={secret}&x=1; \
             Authorization: Bearer bearer-secret; api_key=another-secret; {secret}"
        );
        let sanitized = sanitize_diagnostic(&message, None);

        for leaked in [secret, "bearer-secret", "another-secret", "?key="] {
            assert!(!sanitized.contains(leaked), "leaked {leaked}");
        }
        assert!(sanitized.contains("?[REDACTED]"));
    }

    #[test]
    fn json_redaction_preserves_shape_and_non_string_values() {
        let input = serde_json::json!({
            "error": {
                "url": "https://example.test/path?X-Amz-Signature=secret",
                "attempt": 2,
                "retryable": false
            }
        });
        let sanitized = sanitize_json_strings(&input, None);

        assert_eq!(sanitized["error"]["attempt"], 2);
        assert_eq!(sanitized["error"]["retryable"], false);
        assert_eq!(
            sanitized["error"]["url"],
            "https://example.test/path?[REDACTED]"
        );
    }
}
