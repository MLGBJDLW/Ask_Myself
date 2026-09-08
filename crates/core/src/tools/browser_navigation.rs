//! Navigation readiness for the owned Chromium document, independent of
//! network-idle lifecycle events (which live applications may never emit).

use std::time::{Duration, Instant};

use headless_chrome::{protocol::cdp::Page, Tab};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentState {
    url: String,
    ready_state: String,
    has_document: bool,
}

struct NavigationIdentity {
    frame_id: String,
    loader_id: Option<String>,
    requested_url: String,
}

impl NavigationIdentity {
    fn owns_document(&self, frame_id: &str, loader_id: &str, document_url: &str) -> bool {
        self.frame_id == frame_id
            && self.loader_id.as_deref().map_or_else(
                || self.requested_url == document_url,
                |expected| expected == loader_id,
            )
    }
}

pub(crate) fn navigate_to_document(tab: &Tab, url: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let navigation = tab
        .call_method(Page::Navigate {
            url: url.to_string(),
            referrer: None,
            transition_Type: None,
            frame_id: None,
            referrer_policy: None,
        })
        .map_err(|error| format!("browser navigation dispatch failed: {error}"))?;
    if let Some(error) = navigation.error_text {
        return Err(format!("browser navigation failed: {error}"));
    }
    let identity = NavigationIdentity {
        frame_id: navigation.frame_id,
        loader_id: navigation.loader_id,
        requested_url: url.to_string(),
    };
    loop {
        // A ready previous page must never satisfy a new navigation. Match the
        // exact main-frame loader, then inspect the currently committed DOM.
        let observed = (|| {
            let frame = tab.call_method(Page::GetFrameTree(None))?.frame_tree.frame;
            let value = tab.evaluate(
                "JSON.stringify({url:location.href,readyState:document.readyState,hasDocument:Boolean(document.documentElement)})", false,
            )?.value;
            let state: DocumentState = serde_json::from_str(
                value
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("null"),
            )?;
            Ok::<_, Box<dyn std::error::Error>>((frame, state))
        })();
        let last_state = match observed {
            Ok((frame, state)) => {
                if identity.owns_document(&frame.id, &frame.loader_id, &state.url)
                    && same_document_url(&frame.url, &state.url)
                    && state.has_document
                    && matches!(state.ready_state.as_str(), "interactive" | "complete")
                {
                    return Ok(());
                }
                format!("main document is {}", state.ready_state)
            }
            Err(error) => format!("document observation failed: {error}"),
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "browser document did not become ready within {}ms: {last_state}",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn same_document_url(frame_url: &str, document_url: &str) -> bool {
    let (Ok(mut frame), Ok(mut document)) =
        (url::Url::parse(frame_url), url::Url::parse(document_url))
    else {
        return false;
    };
    frame.set_fragment(None);
    document.set_fragment(None);
    frame == document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_never_accepts_a_ready_previous_document_or_a_subframe() {
        let navigation = NavigationIdentity {
            frame_id: "main".into(),
            loader_id: Some("new".into()),
            requested_url: "https://example.com/next".into(),
        };
        assert!(!navigation.owns_document("main", "old", "https://example.com/next"));
        assert!(!navigation.owns_document("child", "new", "https://example.com/next"));
        assert!(navigation.owns_document("main", "new", "https://example.com/redirected"));
        assert!(!same_document_url(
            "https://example.com/new",
            "https://example.com/old"
        ));
    }

    #[test]
    fn same_document_navigation_requires_the_requested_address() {
        let navigation = NavigationIdentity {
            frame_id: "main".into(),
            loader_id: None,
            requested_url: "https://example.com/#next".into(),
        };
        assert!(!navigation.owns_document("main", "current", "https://example.com/#old"));
        assert!(navigation.owns_document("main", "current", "https://example.com/#next"));
        assert!(same_document_url(
            "https://example.com/",
            "https://example.com/#next"
        ));
    }
}
