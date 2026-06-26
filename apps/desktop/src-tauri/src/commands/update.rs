use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, ResourceId, Runtime, Webview};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

const GITHUB_UPDATE_ENDPOINTS: &[&str] = &[
    "https://github.com/MLGBJDLW/Nexa/releases/latest/download/latest.json",
    "https://mirror.ghproxy.com/https://github.com/MLGBJDLW/Nexa/releases/latest/download/latest-ghproxy.json",
];

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    Github,
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

fn parse_endpoint(value: &str) -> Result<Url, String> {
    Url::parse(value).map_err(|error| format!("Invalid update endpoint {value}: {error}"))
}

async fn endpoints_for_source(
    source: UpdateSource,
    _timeout_ms: Option<u64>,
) -> Result<Vec<Url>, String> {
    match source {
        UpdateSource::Github => GITHUB_UPDATE_ENDPOINTS
            .iter()
            .map(|endpoint| parse_endpoint(endpoint))
            .collect(),
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
