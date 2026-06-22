use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, ResourceId, Runtime, Webview};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

const GITHUB_UPDATE_ENDPOINTS: &[&str] = &[
    "https://github.com/MLGBJDLW/Nexa/releases/latest/download/latest.json",
    "https://mirror.ghproxy.com/https://github.com/MLGBJDLW/Nexa/releases/latest/download/latest-ghproxy.json",
];
const GITEE_RELEASES_API: &str =
    "https://gitee.com/api/v5/repos/ButlerW/Nexa/releases?direction=desc&per_page=20";
const GITEE_MANIFEST_ASSET_NAMES: &[&str] = &["latest-gitee.json", "latest.json"];
const DEFAULT_UPDATE_TIMEOUT_MS: u64 = 90_000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    Github,
    Gitee,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GiteeRelease {
    prerelease: Option<bool>,
    assets: Option<Vec<GiteeAsset>>,
}

#[derive(Debug, Deserialize)]
struct GiteeAsset {
    name: Option<String>,
    browser_download_url: Option<String>,
}

fn parse_endpoint(value: &str) -> Result<Url, String> {
    Url::parse(value).map_err(|error| format!("Invalid update endpoint {value}: {error}"))
}

async fn resolve_gitee_manifest_endpoint(timeout_ms: Option<u64>) -> Result<Url, String> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_UPDATE_TIMEOUT_MS));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("Nexa updater")
        .build()
        .map_err(|error| format!("Failed to prepare Gitee update check: {error}"))?;

    let releases = client
        .get(GITEE_RELEASES_API)
        .send()
        .await
        .map_err(|error| format!("Failed to query Gitee releases: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Gitee releases request failed: {error}"))?
        .json::<Vec<GiteeRelease>>()
        .await
        .map_err(|error| format!("Failed to parse Gitee releases: {error}"))?;

    if releases.is_empty() {
        return Err("No Gitee releases were found for ButlerW/Nexa.".to_string());
    }

    for release in releases {
        if release.prerelease.unwrap_or(false) {
            continue;
        }

        let assets = release.assets.unwrap_or_default();
        for manifest_name in GITEE_MANIFEST_ASSET_NAMES {
            if let Some(asset) = assets.iter().find(|asset| {
                asset
                    .name
                    .as_deref()
                    .map(|name| name.eq_ignore_ascii_case(manifest_name))
                    .unwrap_or(false)
            }) {
                let download_url = asset.browser_download_url.as_deref().ok_or_else(|| {
                    format!("Gitee release asset {manifest_name} is missing a download URL.")
                })?;
                return parse_endpoint(download_url);
            }
        }
    }

    Err(format!(
        "No supported updater manifest ({}) was found in the latest Gitee releases.",
        GITEE_MANIFEST_ASSET_NAMES.join(", ")
    ))
}

async fn endpoints_for_source(
    source: UpdateSource,
    timeout_ms: Option<u64>,
) -> Result<Vec<Url>, String> {
    match source {
        UpdateSource::Github => GITHUB_UPDATE_ENDPOINTS
            .iter()
            .map(|endpoint| parse_endpoint(endpoint))
            .collect(),
        UpdateSource::Gitee => Ok(vec![resolve_gitee_manifest_endpoint(timeout_ms).await?]),
    }
}

#[tauri::command]
pub async fn check_update_from_source_cmd<R: Runtime>(
    webview: Webview<R>,
    source: UpdateSource,
    timeout: Option<u64>,
) -> Result<Option<UpdateMetadata>, String> {
    let endpoints = endpoints_for_source(source, timeout).await?;
    let mut builder = webview
        .updater_builder()
        .endpoints(endpoints)
        .map_err(|error| error.to_string())?;

    if let Some(timeout) = timeout {
        builder = builder.timeout(Duration::from_millis(timeout));
    }

    let updater = builder.build().map_err(|error| error.to_string())?;
    let update = updater.check().await.map_err(|error| error.to_string())?;

    Ok(update.map(|update| {
        let metadata = UpdateMetadata {
            current_version: update.current_version.clone(),
            version: update.version.clone(),
            date: update.date.as_ref().map(ToString::to_string),
            body: update.body.clone(),
            raw_json: update.raw_json.clone(),
            rid: webview.resources_table().add(update),
        };
        metadata
    }))
}
