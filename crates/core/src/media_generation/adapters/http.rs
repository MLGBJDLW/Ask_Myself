use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{LOCATION, RETRY_AFTER};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::{DownloadedAsset, NormalizedProviderError, ProviderJobResult};

pub(super) const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) fn client() -> Result<reqwest::Client, NormalizedProviderError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(crate::USER_AGENT)
        .build()
        .map_err(|error| NormalizedProviderError {
            provider_id: "video_adapter".to_string(),
            code: "client_initialization_failed".to_string(),
            message: sanitize_message(&error.to_string(), None),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        })
}

pub(super) async fn execute_json<T: DeserializeOwned>(
    provider_id: &str,
    api_key: &str,
    request: reqwest::RequestBuilder,
) -> Result<T, NormalizedProviderError> {
    let response = request
        .send()
        .await
        .map_err(|error| transport_error(provider_id, api_key, error))?;
    let status = response.status();
    let retry_after_seconds = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_RESPONSE_BYTES as u64)
    {
        return Err(NormalizedProviderError {
            provider_id: provider_id.to_string(),
            code: "response_too_large".to_string(),
            message: "Provider JSON response exceeded 1 MiB".to_string(),
            retryable: false,
            retry_after_seconds: None,
            http_status: Some(status.as_u16()),
            request_id: None,
        });
    }
    let body = read_bounded(response, MAX_JSON_RESPONSE_BYTES, provider_id, api_key).await?;
    if !status.is_success() {
        return Err(provider_http_error(
            provider_id,
            api_key,
            status,
            retry_after_seconds,
            &body,
        ));
    }
    serde_json::from_slice(&body).map_err(|error| NormalizedProviderError {
        provider_id: provider_id.to_string(),
        code: "invalid_provider_response".to_string(),
        message: sanitize_message(&error.to_string(), Some(api_key)),
        retryable: false,
        retry_after_seconds: None,
        http_status: Some(status.as_u16()),
        request_id: None,
    })
}

pub(super) async fn execute_no_content(
    provider_id: &str,
    api_key: &str,
    request: reqwest::RequestBuilder,
) -> Result<(), NormalizedProviderError> {
    let response = request
        .send()
        .await
        .map_err(|error| transport_error(provider_id, api_key, error))?;
    let status = response.status();
    let retry_after_seconds = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let body = read_bounded(response, MAX_JSON_RESPONSE_BYTES, provider_id, api_key).await?;
    if status.is_success() {
        return Ok(());
    }
    Err(provider_http_error(
        provider_id,
        api_key,
        status,
        retry_after_seconds,
        &body,
    ))
}

async fn read_bounded(
    response: reqwest::Response,
    limit: usize,
    provider_id: &str,
    api_key: &str,
) -> Result<Vec<u8>, NormalizedProviderError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| transport_error(provider_id, api_key, error))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(NormalizedProviderError {
                provider_id: provider_id.to_string(),
                code: "response_too_large".to_string(),
                message: format!("Provider response exceeded {limit} bytes"),
                retryable: false,
                retry_after_seconds: None,
                http_status: None,
                request_id: None,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn provider_http_error(
    provider_id: &str,
    api_key: &str,
    status: reqwest::StatusCode,
    retry_after_seconds: Option<u64>,
    body: &[u8],
) -> NormalizedProviderError {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/code")
                .or_else(|| value.get("code"))
                .or_else(|| value.pointer("/base_resp/status_code"))
        })
        .map(json_scalar)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("http_{}", status.as_u16()));
    let message = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .or_else(|| value.pointer("/base_resp/status_msg"))
        })
        .map(json_scalar)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| String::from_utf8_lossy(body).to_string());
    let request_id = value
        .as_ref()
        .and_then(|value| {
            value
                .get("request_id")
                .or_else(|| value.get("requestId"))
                .or_else(|| value.pointer("/error/request_id"))
        })
        .map(json_scalar)
        .filter(|value| !value.is_empty())
        .map(|value| sanitize_message(&value, Some(api_key)));
    NormalizedProviderError {
        provider_id: provider_id.to_string(),
        code: sanitize_message(&code, Some(api_key)),
        message: sanitize_message(&message, Some(api_key)),
        retryable: status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error(),
        retry_after_seconds,
        http_status: Some(status.as_u16()),
        request_id,
    }
}

fn json_scalar(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn transport_error(
    provider_id: &str,
    api_key: &str,
    error: reqwest::Error,
) -> NormalizedProviderError {
    NormalizedProviderError {
        provider_id: provider_id.to_string(),
        code: if error.is_timeout() {
            "transport_timeout"
        } else {
            "transport_error"
        }
        .to_string(),
        message: sanitize_message(&error.to_string(), Some(api_key)),
        retryable: error.is_timeout() || error.is_connect(),
        retry_after_seconds: None,
        http_status: error.status().map(|status| status.as_u16()),
        request_id: None,
    }
}

pub(super) async fn download_outputs(
    provider_id: &str,
    client: &reqwest::Client,
    result: &ProviderJobResult,
    destination_directory: &Path,
    max_total_bytes: u64,
    allow_insecure_http: bool,
) -> Result<Vec<DownloadedAsset>, NormalizedProviderError> {
    if result.outputs.is_empty() {
        return Err(local_error(
            provider_id,
            "missing_output",
            "Provider result has no outputs",
        ));
    }
    if max_total_bytes == 0 {
        return Err(local_error(
            provider_id,
            "invalid_download_limit",
            "Download byte limit must be greater than zero",
        ));
    }
    tokio::fs::create_dir_all(destination_directory)
        .await
        .map_err(|error| local_error(provider_id, "download_io_error", &error.to_string()))?;
    let staging_directory =
        destination_directory.join(format!(".nexa-download-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&staging_directory)
        .await
        .map_err(|error| local_error(provider_id, "download_io_error", &error.to_string()))?;
    let mut staged = Vec::with_capacity(result.outputs.len());
    let mut total_bytes = 0_u64;
    let download_result = async {
        for output in &result.outputs {
            let url = url::Url::parse(&output.uri).map_err(|_| {
                local_error(
                    provider_id,
                    "invalid_output_url",
                    "Provider output URL is invalid",
                )
            })?;
            if (!allow_insecure_http && !valid_remote_download_url(&url))
                || (allow_insecure_http && !matches!(url.scheme(), "http" | "https"))
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(local_error(
                    provider_id,
                    "invalid_output_url",
                    "Provider output URL must use public HTTPS and contain no credentials",
                ));
            }
            let response = fetch_output(provider_id, client, url, allow_insecure_http).await?;
            if !response.status().is_success() {
                let status = response.status();
                return Err(NormalizedProviderError {
                    provider_id: provider_id.to_string(),
                    code: format!("download_http_{}", status.as_u16()),
                    message: "Provider output download failed".to_string(),
                    retryable: status == reqwest::StatusCode::TOO_MANY_REQUESTS
                        || status.is_server_error(),
                    retry_after_seconds: None,
                    http_status: Some(status.as_u16()),
                    request_id: None,
                });
            }
            if response
                .content_length()
                .is_some_and(|length| total_bytes.saturating_add(length) > max_total_bytes)
            {
                return Err(local_error(
                    provider_id,
                    "download_too_large",
                    "Provider outputs exceed the configured byte limit",
                ));
            }
            let extension = extension_for_media_type(&output.media_type);
            let name = format!("{}.{extension}", Uuid::new_v4());
            let temporary_path = staging_directory.join(&name);
            let final_path = destination_directory.join(name);
            let mut file = tokio::fs::File::create(&temporary_path)
                .await
                .map_err(|error| {
                    local_error(provider_id, "download_io_error", &error.to_string())
                })?;
            let mut stream = response.bytes_stream();
            let mut output_bytes = 0_u64;
            let mut header = Vec::with_capacity(16);
            let mut digest = Sha256::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| transport_error(provider_id, "", error))?;
                output_bytes = output_bytes.saturating_add(chunk.len() as u64);
                total_bytes = total_bytes.saturating_add(chunk.len() as u64);
                if total_bytes > max_total_bytes {
                    return Err(local_error(
                        provider_id,
                        "download_too_large",
                        "Provider outputs exceed the configured byte limit",
                    ));
                }
                if header.len() < 16 {
                    let remaining = 16 - header.len();
                    header.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
                digest.update(&chunk);
                file.write_all(&chunk).await.map_err(|error| {
                    local_error(provider_id, "download_io_error", &error.to_string())
                })?;
            }
            file.flush().await.map_err(|error| {
                local_error(provider_id, "download_io_error", &error.to_string())
            })?;
            let detected_media_type = detect_media_type(&header).ok_or_else(|| {
                local_error(
                    provider_id,
                    "invalid_output_content",
                    "Provider output bytes are not a supported MP4, QuickTime, or ZIP asset",
                )
            })?;
            if !media_types_compatible(&output.media_type, detected_media_type) {
                return Err(local_error(
                    provider_id,
                    "output_media_type_mismatch",
                    "Provider output bytes do not match the declared media type",
                ));
            }
            staged.push((
                temporary_path,
                final_path,
                output.media_type.clone(),
                detected_media_type.to_string(),
                output_bytes,
                format!("{:x}", digest.finalize()),
            ));
        }
        Ok::<(), NormalizedProviderError>(())
    }
    .await;
    if let Err(error) = download_result {
        let _ = tokio::fs::remove_dir_all(&staging_directory).await;
        return Err(error);
    }
    let mut downloaded = Vec::with_capacity(staged.len());
    let mut moved_paths = Vec::with_capacity(staged.len());
    for (
        temporary_path,
        final_path,
        declared_media_type,
        detected_media_type,
        byte_length,
        sha256,
    ) in staged
    {
        if let Err(error) = tokio::fs::rename(&temporary_path, &final_path).await {
            for moved_path in moved_paths {
                let _ = tokio::fs::remove_file(moved_path).await;
            }
            let _ = tokio::fs::remove_dir_all(&staging_directory).await;
            return Err(local_error(
                provider_id,
                "download_io_error",
                &error.to_string(),
            ));
        }
        let asset_path = final_path.clone();
        moved_paths.push(final_path);
        downloaded.push(DownloadedAsset {
            path: asset_path,
            declared_media_type,
            detected_media_type,
            byte_length,
            sha256,
        });
    }
    let _ = tokio::fs::remove_dir(&staging_directory).await;
    Ok(downloaded)
}

fn detect_media_type(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") {
        return Some("application/zip");
    }
    if header.len() >= 12 && &header[4..8] == b"ftyp" {
        return if &header[8..12] == b"qt  " {
            Some("video/quicktime")
        } else {
            Some("video/mp4")
        };
    }
    None
}

fn media_types_compatible(declared: &str, detected: &str) -> bool {
    declared == detected
        || (matches!(declared, "video/mp4" | "video/quicktime")
            && matches!(detected, "video/mp4" | "video/quicktime"))
}

async fn fetch_output(
    provider_id: &str,
    fallback_client: &reqwest::Client,
    mut url: url::Url,
    allow_insecure_http: bool,
) -> Result<reqwest::Response, NormalizedProviderError> {
    for redirect_count in 0..=5 {
        let client = if allow_insecure_http {
            fallback_client.clone()
        } else {
            pinned_public_client(provider_id, &url).await?
        };
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| transport_error(provider_id, "", error))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if redirect_count == 5 {
            return Err(local_error(
                provider_id,
                "too_many_redirects",
                "Provider output exceeded five redirects",
            ));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                local_error(
                    provider_id,
                    "invalid_output_redirect",
                    "Provider output redirect omitted a valid Location header",
                )
            })?;
        url = url.join(location).map_err(|_| {
            local_error(
                provider_id,
                "invalid_output_redirect",
                "Provider output redirect URL is invalid",
            )
        })?;
        if !allow_insecure_http && !valid_remote_download_url(&url) {
            return Err(local_error(
                provider_id,
                "invalid_output_redirect",
                "Provider output redirect must remain on public HTTPS",
            ));
        }
    }
    unreachable!("redirect loop returns within the bounded iteration")
}

async fn pinned_public_client(
    provider_id: &str,
    url: &url::Url,
) -> Result<reqwest::Client, NormalizedProviderError> {
    if !valid_remote_download_url(url) {
        return Err(local_error(
            provider_id,
            "invalid_output_url",
            "Provider output URL must use public HTTPS and contain no credentials",
        ));
    }
    let host = url.host().ok_or_else(|| {
        local_error(
            provider_id,
            "invalid_output_url",
            "Provider output URL has no host",
        )
    })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let (domain, addresses) = match host {
        url::Host::Domain(domain) => {
            let addresses = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|error| local_error(provider_id, "output_dns_error", &error.to_string()))?
                .collect::<Vec<_>>();
            (Some(domain.to_string()), addresses)
        }
        url::Host::Ipv4(address) => (None, vec![(address, port).into()]),
        url::Host::Ipv6(address) => (None, vec![(address, port).into()]),
    };
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(local_error(
            provider_id,
            "private_output_target",
            "Provider output host resolved to a non-public address",
        ));
    }
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent(crate::USER_AGENT);
    if let Some(domain) = domain {
        builder = builder.resolve_to_addrs(&domain, &addresses);
    }
    builder.build().map_err(|error| {
        local_error(
            provider_id,
            "client_initialization_failed",
            &error.to_string(),
        )
    })
}

fn valid_remote_download_url(url: &url::Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host() else {
        return false;
    };
    match host {
        url::Host::Domain(host) => {
            !host.eq_ignore_ascii_case("localhost") && !host.ends_with(".local")
        }
        url::Host::Ipv4(address) => is_public_ip(address.into()),
        url::Host::Ipv6(address) => is_public_ip(address.into()),
    }
}

fn is_public_ip(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            let [first, second, _, _] = address.octets();
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_unspecified()
                && !address.is_broadcast()
                && !address.is_multicast()
                && !address.is_documentation()
                && first != 0
                && !(first == 100 && (64..=127).contains(&second))
                && !(first == 192 && second == 0)
                && !(first == 198 && (18..=19).contains(&second))
                && first < 224
        }
        std::net::IpAddr::V6(address) => {
            let segments = address.segments();
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !address.is_multicast()
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && (segments[0] & 0xffc0) != 0xfec0
                && address
                    .to_ipv4()
                    .is_none_or(|address| is_public_ip(std::net::IpAddr::V4(address)))
        }
    }
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type {
        "video/quicktime" => "mov",
        "application/zip" => "zip",
        _ => "mp4",
    }
}

fn local_error(provider_id: &str, code: &str, message: &str) -> NormalizedProviderError {
    NormalizedProviderError {
        provider_id: provider_id.to_string(),
        code: code.to_string(),
        message: sanitize_message(message, None),
        retryable: false,
        retry_after_seconds: None,
        http_status: None,
        request_id: None,
    }
}

pub(super) fn sanitize_message(value: &str, api_key: Option<&str>) -> String {
    let mut sanitized = value.chars().take(4096).collect::<String>();
    if let Some(api_key) = api_key.filter(|value| !value.is_empty()) {
        sanitized = sanitized.replace(api_key, "[REDACTED]");
    }
    let patterns = [
        r#"(?i)(authorization\s*[:=]\s*bearer\s+)[^\s,;"']+"#,
        r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{8,}",
        r#"(?i)((?:api[_-]?key|token|secret)\s*[:=]\s*)[^\s,;"']+"#,
    ];
    for pattern in patterns {
        if let Ok(regex) = regex::Regex::new(pattern) {
            sanitized = regex.replace_all(&sanitized, "${1}[REDACTED]").into_owned();
        }
    }
    if let Ok(url_query) = regex::Regex::new(r#"(?i)\b(https?://[^\s?#\"'<>]+)\?[^\s#\"'<>]*"#) {
        sanitized = url_query
            .replace_all(&sanitized, "${1}?[REDACTED]")
            .into_owned();
    }
    if let Ok(provider_locator) = regex::Regex::new(r"(?i)\b(?:runway|mm_file)://[^\s,;]+") {
        sanitized = provider_locator
            .replace_all(&sanitized, "[REDACTED_PROVIDER_LOCATOR]")
            .into_owned();
    }
    sanitized.chars().take(2048).collect()
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::media_generation::adapters::ProviderOutputLocator;

    #[test]
    fn error_messages_remove_inline_credentials() {
        let sanitized = sanitize_message(
            "Authorization: Bearer sk-secret and api_key=another-secret",
            Some("sk-secret"),
        );
        assert!(!sanitized.contains("sk-secret"));
        assert!(!sanitized.contains("another-secret"));
        assert!(sanitized.contains("[REDACTED]"));

        let signed = sanitize_message(
            "request failed for https://cdn.example.com/output.mp4?X-Amz-Signature=usable-secret",
            None,
        );
        assert!(!signed.contains("usable-secret"));
        assert!(signed.contains("?[REDACTED]"));
    }

    #[test]
    fn output_urls_reject_private_network_targets_but_allow_signed_queries() {
        assert!(!valid_remote_download_url(
            &url::Url::parse("https://127.0.0.1/private.mp4").unwrap()
        ));
        assert!(!valid_remote_download_url(
            &url::Url::parse("https://[::1]/private.mp4").unwrap()
        ));
        assert!(!valid_remote_download_url(
            &url::Url::parse("https://100.64.0.1/private.mp4").unwrap()
        ));
        assert!(!valid_remote_download_url(
            &url::Url::parse("https://[::ffff:127.0.0.1]/private.mp4").unwrap()
        ));
        assert!(valid_remote_download_url(
            &url::Url::parse("https://cdn.example.com/output.mp4?signature=temporary").unwrap()
        ));
    }

    #[tokio::test]
    async fn multi_output_download_rolls_back_when_a_later_locator_is_invalid() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let body = b"\0\0\0\x0cftypisom";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        });
        let destination = tempfile::tempdir().unwrap();
        let result = ProviderJobResult {
            provider_task_id: "task-1".to_string(),
            outputs: vec![
                ProviderOutputLocator {
                    uri: format!("http://{address}/first.mp4"),
                    media_type: "video/mp4".to_string(),
                    expires_hint: None,
                },
                ProviderOutputLocator {
                    uri: "file:///private/second.mp4".to_string(),
                    media_type: "video/mp4".to_string(),
                    expires_hint: None,
                },
            ],
            width: None,
            height: None,
            duration_ms: None,
        };
        let error = download_outputs(
            "test",
            &client().unwrap(),
            &result,
            destination.path(),
            1024,
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "invalid_output_url");
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
    }
}
