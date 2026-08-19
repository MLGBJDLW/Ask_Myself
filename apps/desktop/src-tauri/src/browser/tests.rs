use std::collections::HashSet;

use super::agent_tool::browser_action_names;
use super::policy::{
    classify_agent_action, form_navigation_approval_key, navigation_preapproved,
    normalize_browser_url, BrowserActionRisk, NavigationActor,
};
use super::scripts::{browser_init_script, browser_takeover_script, BROWSER_INIT_SCRIPT};
use super::state::{browser_target_screen_point, BrowserControlOwner, ControlLease};
use nexa_core::browser_runtime::{BrowserBounds, BrowserElementBounds};

#[test]
fn browser_url_policy_accepts_top_level_http_navigation() {
    let normalized = normalize_browser_url("example.com/docs", NavigationActor::User).unwrap();
    assert_eq!(normalized.as_str(), "https://example.com/docs");
}

#[test]
fn browser_url_policy_searches_words_and_preserves_plausible_hosts() {
    let search = normalize_browser_url("weather", NavigationActor::User).unwrap();
    assert_eq!(search.as_str(), "https://www.google.com/search?q=weather");

    let phrase = normalize_browser_url("weather tomorrow", NavigationActor::User).unwrap();
    assert_eq!(
        phrase.as_str(),
        "https://www.google.com/search?q=weather+tomorrow"
    );

    for (input, expected) in [
        ("example.com/docs", "https://example.com/docs"),
        ("localhost:3000", "https://localhost:3000/"),
        ("127.0.0.1:8080", "https://127.0.0.1:8080/"),
        ("[::1]:8443", "https://[::1]:8443/"),
    ] {
        assert_eq!(
            normalize_browser_url(input, NavigationActor::User)
                .unwrap()
                .as_str(),
            expected
        );
    }
}

#[test]
fn browser_url_policy_rejects_script_and_file_schemes() {
    for url in [
        "javascript:alert(1)",
        "JAVASCRIPT:alert(1)",
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
        "http://100.64.0.1",
        "http://100.127.255.254",
        "http://198.18.0.1",
        "http://[fc00::1]",
        "http://[fe80::1]",
        "http://[2001:db8::1]",
        "http://[2002:7f00:1::1]",
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

    for url in [
        "https://1.1.1.1",
        "https://[2606:4700:4700::1111]",
        "http://[::ffff:8.8.8.8]",
    ] {
        assert!(
            normalize_browser_url(url, NavigationActor::Agent).is_ok(),
            "{url}"
        );
    }
}

#[test]
fn takeover_script_uses_an_unforgeable_all_frame_navigation_signal() {
    let script = browser_takeover_script("takeover-secret");

    assert!(script.contains("event.isTrusted"));
    assert!(script.contains("window.top"));
    assert!(script.contains("stopImmediatePropagation"));
    assert!(script.contains("nexa-user-input://takeover-secret"));
    assert!(!script.contains("document.title"));
    assert!(!BROWSER_INIT_SCRIPT.contains("__NEXA_USER_TAKEOVER__"));
}

#[test]
fn browser_tool_advertises_only_platform_supported_pointer_actions() {
    let actions = browser_action_names();
    #[cfg(target_os = "windows")]
    {
        assert!(actions.contains(&"move"));
        assert!(actions.contains(&"hover"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert!(!actions.contains(&"move"));
        assert!(!actions.contains(&"hover"));
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
    assert!(BROWSER_INIT_SCRIPT.contains("dragDestinationElements"));
    assert!(BROWSER_INIT_SCRIPT.contains("[class*=\"drop\" i]"));
    assert!(BROWSER_INIT_SCRIPT.contains("invalidateForUserTakeover"));
    assert!(BROWSER_INIT_SCRIPT.contains("requestSubmit"));
    assert!(BROWSER_INIT_SCRIPT.contains("Unsupported browser key"));
    assert!(BROWSER_INIT_SCRIPT.contains("el.ownerDocument || document"));
    assert!(BROWSER_INIT_SCRIPT.contains("__NEXA_BROWSER_PICK_BRIDGE__"));
    assert!(BROWSER_INIT_SCRIPT.contains("event.source === window.parent"));
    assert!(BROWSER_INIT_SCRIPT.contains("target === event.source"));
    assert!(BROWSER_INIT_SCRIPT.contains("event.isTrusted"));
}

#[test]
fn agent_interactions_have_a_visible_two_phase_cursor_and_complete_pointer_sequences() {
    assert!(BROWSER_INIT_SCRIPT.contains("previewAction"));
    assert!(BROWSER_INIT_SCRIPT.contains("validateAction"));
    assert!(BROWSER_INIT_SCRIPT.contains("prepareNativePointer"));
    assert!(BROWSER_INIT_SCRIPT.contains("elementFromPoint"));
    assert!(BROWSER_INIT_SCRIPT.contains("data-nexa-agent-cursor"));
    assert!(BROWSER_INIT_SCRIPT.contains("prefers-reduced-motion: reduce"));
    assert!(BROWSER_INIT_SCRIPT.contains("cubic-bezier(.22,.8,.24,1)"));
    assert!(BROWSER_INIT_SCRIPT.contains("pointerdown"));
    assert!(BROWSER_INIT_SCRIPT.contains("dblclick"));
    assert!(BROWSER_INIT_SCRIPT.contains("dragBetween"));
    assert!(BROWSER_INIT_SCRIPT.contains("expectedEnd"));
    assert!(BROWSER_INIT_SCRIPT.contains("domFingerprintOf"));
}

#[test]
fn browser_target_coordinates_respect_window_origin_webview_offset_and_scale() {
    let point = browser_target_screen_point(
        (-1200, 80),
        1.5,
        BrowserBounds {
            x: 300.0,
            y: 100.0,
            width: 600.0,
            height: 500.0,
        },
        &BrowserElementBounds {
            x: 40.0,
            y: 60.0,
            width: 80.0,
            height: 40.0,
        },
    )
    .unwrap();

    assert_eq!(point, (-630, 350));
}

#[test]
fn picker_bridge_uses_a_per_webview_token_and_native_message_primitives() {
    let script = browser_init_script("picker-secret");

    assert!(script.contains("const pickMessageToken = \"picker-secret\""));
    assert!(!script.contains("__NEXA_PICK_TOKEN__"));
    assert!(script.contains("Window.prototype.postMessage"));
    assert!(script.contains("stopImmediatePropagation"));
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
        classify_agent_action("hover", Some("button"), Some("Preview"), None, None),
        BrowserActionRisk::Low,
    );
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
            "double_click",
            Some("link"),
            Some("Documentation"),
            Some("https://tauri.app"),
            None
        ),
        BrowserActionRisk::Low,
    );
    assert_eq!(
        classify_agent_action("drag", Some("button"), Some("Move item"), None, None),
        BrowserActionRisk::Consequential,
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
