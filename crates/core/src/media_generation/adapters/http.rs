use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::RETRY_AFTER;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::{DownloadedAsset, NormalizedProviderError, ProviderJobResult};

pub(super) const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) fn client() -> Result<reqwest::Client, NormalizedProviderError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many redirects");
            }
            if valid_remote_download_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
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
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|error| transport_error(provider_id, "", error))?;
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
                file.write_all(&chunk).await.map_err(|error| {
                    local_error(provider_id, "download_io_error", &error.to_string())
                })?;
            }
            file.flush().await.map_err(|error| {
                local_error(provider_id, "download_io_error", &error.to_string())
            })?;
            staged.push((
                temporary_path,
                final_path,
                output.media_type.clone(),
                output_bytes,
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
    for (temporary_path, final_path, declared_media_type, byte_length) in staged {
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
            byte_length,
        });
    }
    let _ = tokio::fs::remove_dir(&staging_directory).await;
    Ok(downloaded)
}

fn valid_remote_download_url(url: &url::Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    host.parse::<std::net::IpAddr>()
        .map(|address| match address {
            std::net::IpAddr::V4(address) => {
                !address.is_private()
                    && !address.is_loopback()
                    && !address.is_link_local()
                    && !address.is_unspecified()
                    && !address.is_broadcast()
            }
            std::net::IpAddr::V6(address) => {
                !address.is_loopback() && !address.is_unspecified() && !address.is_unique_local()
            }
        })
        .unwrap_or(true)
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
    }

    #[test]
    fn output_urls_reject_private_network_targets_but_allow_signed_queries() {
        assert!(!valid_remote_download_url(
            &url::Url::parse("https://127.0.0.1/private.mp4").unwrap()
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
            let body = b"video-bytes";
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
