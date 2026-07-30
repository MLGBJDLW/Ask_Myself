use super::policy::{
    classify_agent_action, normalize_browser_url, BrowserActionRisk, NavigationActor,
};
use super::state::{BrowserControlOwner, ControlLease};

#[test]
fn browser_url_policy_accepts_top_level_http_navigation() {
    let normalized = normalize_browser_url("example.com/docs", NavigationActor::User).unwrap();
    assert_eq!(normalized.as_str(), "https://example.com/docs");
}

#[test]
fn browser_url_policy_rejects_script_and_file_schemes() {
    for url in [
        "javascript:alert(1)",
        "data:text/html,pwned",
        "file:///etc/passwd",
    ] {
        assert!(normalize_browser_url(url, NavigationActor::User).is_err());
    }
}

#[test]
fn agent_navigation_blocks_loopback_and_private_networks() {
    for url in [
        "http://localhost:3000",
        "http://127.0.0.1:8080",
        "http://10.0.0.8",
        "http://169.254.169.254/latest/meta-data",
        "http://192.168.1.1",
    ] {
        assert!(
            normalize_browser_url(url, NavigationActor::Agent).is_err(),
            "{url}"
        );
        assert!(
            normalize_browser_url(url, NavigationActor::User).is_ok(),
            "{url}"
        );
    }
}

#[test]
fn control_takeover_invalidates_the_previous_observation_generation() {
    let mut lease = ControlLease::default();
    let initial = lease.generation();
    lease.acquire(BrowserControlOwner::Agent {
        call_id: "call-1".into(),
    });
    let agent_generation = lease.generation();
    assert!(agent_generation > initial);
    lease.acquire(BrowserControlOwner::User);
    assert!(lease.generation() > agent_generation);
    assert!(matches!(lease.owner(), BrowserControlOwner::User));
}

#[test]
fn approval_policy_distinguishes_navigation_from_consequential_actions() {
    assert_eq!(
        classify_agent_action(
            "click",
            Some("link"),
            Some("Documentation"),
            Some("https://tauri.app"),
            None
        ),
        BrowserActionRisk::Low,
    );
    assert_eq!(
        classify_agent_action(
            "click",
            Some("button"),
            Some("Merge pull request"),
            None,
            None
        ),
        BrowserActionRisk::Consequential,
    );
    assert_eq!(
        classify_agent_action(
            "type",
            Some("textbox"),
            Some("Password"),
            None,
            Some("password")
        ),
        BrowserActionRisk::SensitiveInput,
    );
}
