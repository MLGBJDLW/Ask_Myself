use std::collections::HashSet;

use super::policy::{
    classify_agent_action, form_navigation_approval_key, navigation_preapproved,
    normalize_browser_url, BrowserActionRisk, NavigationActor,
};
use super::scripts::BROWSER_INIT_SCRIPT;
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
        "http://[::ffff:127.0.0.1]",
        "http://[::ffff:10.0.0.8]",
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
fn validated_form_navigation_allows_one_query_variant_only() {
    let form_action = url::Url::parse("https://public.example/search").unwrap();
    let submitted = url::Url::parse("https://public.example/search?q=nexa").unwrap();
    let other_path = url::Url::parse("https://public.example/admin?q=nexa").unwrap();
    let mut approved = HashSet::from([form_navigation_approval_key(&form_action)]);

    assert!(navigation_preapproved(&submitted, true, &mut approved));
    assert!(!navigation_preapproved(&submitted, true, &mut approved));
    assert!(!navigation_preapproved(&other_path, true, &mut approved));
}

#[test]
fn observation_script_never_serializes_form_values_or_hidden_inputs() {
    assert!(!BROWSER_INIT_SCRIPT.contains("el.value ||"));
    assert!(BROWSER_INIT_SCRIPT.contains("input:not([type=\"hidden\" i])"));
    assert!(BROWSER_INIT_SCRIPT.contains("isObservable(element)"));
}

#[test]
fn agent_navigation_fails_closed_for_unvalidated_redirect_targets() {
    let initial = url::Url::parse("https://public.example/start").unwrap();
    let redirect = url::Url::parse("https://redirect.example/next").unwrap();
    let mut approved = HashSet::from([initial.to_string()]);
    assert!(navigation_preapproved(&initial, true, &mut approved));
    assert!(!navigation_preapproved(&initial, true, &mut approved));
    assert!(!navigation_preapproved(&redirect, true, &mut approved));
    assert!(navigation_preapproved(&redirect, false, &mut approved));
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
            "click",
            Some("link"),
            Some("Delete account"),
            Some("https://example.com/account/delete"),
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
