use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BrowserControlOwner {
    #[default]
    None,
    User,
    Agent {
        call_id: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BrowserBounds {
    pub fn sanitized(self) -> Self {
        Self {
            x: self.x.max(0.0),
            y: self.y.max(0.0),
            width: self.width.clamp(1.0, 16_384.0),
            height: self.height.clamp(1.0, 16_384.0),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLocatorFingerprint {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub test_id: Option<String>,
    pub name: Option<String>,
    pub href: Option<String>,
    pub css_path: Option<String>,
    pub text_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserElement {
    #[serde(rename = "ref")]
    pub element_ref: String,
    pub tag: String,
    pub role: String,
    pub name: String,
    pub href: Option<String>,
    pub input_type: Option<String>,
    pub enabled: bool,
    pub visible: bool,
    pub bounds: BrowserElementBounds,
    pub locator_fingerprint: BrowserLocatorFingerprint,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserScreenshot {
    pub mime_type: String,
    pub content_hash: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    #[serde(skip, default)]
    pub image_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserObservation {
    pub observation_id: String,
    pub session_id: String,
    pub tab_id: String,
    pub url: String,
    pub title: String,
    pub text: String,
    pub viewport: serde_json::Value,
    pub content_hash: String,
    pub elements: Vec<BrowserElement>,
    pub accessibility_tree: Vec<BrowserElement>,
    pub control_owner: BrowserControlOwner,
    pub screenshot: Option<BrowserScreenshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTab {
    pub id: String,
    pub session_id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub loading: bool,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSession {
    pub id: String,
    pub conversation_id: Option<String>,
    pub profile_id: String,
    pub active_tab_id: Option<String>,
    pub tabs: Vec<BrowserTab>,
    pub control_owner: BrowserControlOwner,
    pub workspace_visible: bool,
    #[serde(default)]
    pub cleanup_pending: bool,
    #[serde(default)]
    pub visibility_revision: u64,
    #[serde(default)]
    pub visibility_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_request_revision: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::BrowserSession;

    #[test]
    fn legacy_browser_session_json_defaults_cleanup_state_to_active() {
        let session: BrowserSession = serde_json::from_value(serde_json::json!({
            "id": "browser-1",
            "conversationId": "conversation-1",
            "profileId": "profile-1",
            "activeTabId": null,
            "tabs": [],
            "controlOwner": { "type": "none" },
            "workspaceVisible": false
        }))
        .unwrap();

        assert!(!session.cleanup_pending);
    }
}
