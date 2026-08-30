use std::collections::HashSet;

use super::agent_tool::browser_action_names;
use super::policy::{
    classify_agent_action, form_navigation_approval_key, managed_permit_matches_url,
    navigation_preapproved, normalize_browser_url, validate_agent_network_url_with_permit,
    BrowserActionRisk, NavigationActor,
};
use super::scripts::{browser_init_script, browser_takeover_script, BROWSER_INIT_SCRIPT};
use super::state::{
    accept_visibility_revision, action_snapshot_changed, agent_tab_surface_is_valid,
    browser_history_target_expression, browser_host_window_allows_agent_action,
    browser_tab_open_allowed, browser_target_screen_point, dispatch_browser_navigation,
    dispatch_terminal_browser_mutation, next_active_tab_for_terminal_close,
    next_visibility_request_revision, trusted_action_budget, validated_temporary_profile_dir,
    visibility_request_is_satisfied, with_agent_navigation_approval, BrowserActCommitTracker,
    BrowserActFailurePhase, BrowserControlOwner, BrowserHistoryDirection, BrowserSessionPhase,
    ControlLease,
};
use super::webview_host::TrustedInputEventBudget;
use nexa_core::browser_runtime::{
    BrowserBounds, BrowserElement, BrowserElementBounds, BrowserLocatorFingerprint,
};
use nexa_core::tools::run_shell_tool::ManagedLoopbackPermitIssuer;

#[test]
fn every_browser_mutation_requires_a_visible_restored_focused_host() {
    for action in ["click", "type", "press", "drag", "select", "scroll"] {
        assert!(
            browser_host_window_allows_agent_action(true, false, true),
            "{action} should be allowed only in the fully visible host state"
        );
        assert!(
            !browser_host_window_allows_agent_action(false, false, true),
            "{action} must reject a hidden host"
        );
        assert!(
            !browser_host_window_allows_agent_action(true, true, true),
            "{action} must reject a minimized host"
        );
        assert!(
            !browser_host_window_allows_agent_action(true, false, false),
            "{action} must reject an unfocused host"
        );
    }
}

#[test]
fn browser_action_tracker_distinguishes_claim_from_commit() {
    let tracker = BrowserActCommitTracker::default();
    let untouched = tracker.failure("stale visibility".to_string());
    assert_eq!(untouched.phase, BrowserActFailurePhase::PreCommit);
    assert!(!untouched.observation_consumed);

    tracker.mark_observation_consumed();
    let claimed = tracker.failure("stale target".to_string());
    assert_eq!(claimed.phase, BrowserActFailurePhase::PreCommit);
    assert!(claimed.observation_consumed);

    tracker.mark_committed();
    let committed = tracker.failure("dispatch response dropped".to_string());
    assert_eq!(
        committed.phase,
        BrowserActFailurePhase::EffectMayHaveOccurred
    );
    assert!(committed.observation_consumed);
}

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

#[tokio::test]
async fn managed_loopback_permit_allows_only_its_exact_origin() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let permit = ManagedLoopbackPermitIssuer::new("service-1", Some(42)).issue(
        format!("http://127.0.0.1:{port}"),
        "127.0.0.1",
        port,
    );
    let exact = url::Url::parse(&format!("http://127.0.0.1:{port}/app")).unwrap();
    let alias = url::Url::parse(&format!("http://localhost:{port}/app")).unwrap();
    let unpermitted_port = if port == u16::MAX { port - 1 } else { port + 1 };
    let other_port = url::Url::parse(&format!("http://127.0.0.1:{unpermitted_port}/app")).unwrap();

    assert!(managed_permit_matches_url(&permit, &exact));
    assert!(!managed_permit_matches_url(&permit, &alias));
    assert!(!managed_permit_matches_url(&permit, &other_port));
    validate_agent_network_url_with_permit(&exact, Some(&permit))
        .await
        .expect("exact managed loopback origin should be observable by its conversation");
    assert!(
        validate_agent_network_url_with_permit(&alias, Some(&permit))
            .await
            .is_err()
    );
}

#[test]
fn validated_managed_loopback_navigation_is_single_use_without_global_private_access() {
    let target = url::Url::parse("http://127.0.0.1:4173/app").unwrap();
    let other = url::Url::parse("http://127.0.0.1:4174/app").unwrap();
    let mut approved = HashSet::from([target.to_string()]);

    assert!(navigation_preapproved(&target, true, &mut approved));
    assert!(!navigation_preapproved(&target, true, &mut approved));
    assert!(!navigation_preapproved(&other, true, &mut approved));
}

#[test]
fn takeover_script_uses_an_unforgeable_all_frame_navigation_signal() {
    let script = browser_takeover_script("takeover-secret");

    assert!(script.contains("event.isTrusted"));
    assert!(script.contains("window.top"));
    assert!(script.contains("stopImmediatePropagation"));
    assert!(script.contains("nexa-user-input://takeover-secret"));
    assert!(script.contains("'wheel'"));
    assert!(script.contains("'touchstart'"));
    assert!(!script.contains("document.title"));
    assert!(!BROWSER_INIT_SCRIPT.contains("__NEXA_USER_TAKEOVER__"));
}

#[test]
fn browser_tool_advertises_only_platform_supported_pointer_actions() {
    let actions = browser_action_names();
    assert!(actions.contains(&"go_back"));
    assert!(actions.contains(&"go_forward"));
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
fn agent_history_traversal_targets_adjacent_entries_and_cleans_failed_approval() {
    let back = browser_history_target_expression(BrowserHistoryDirection::Back);
    let forward = browser_history_target_expression(BrowserHistoryDirection::Forward);
    assert!(back.contains("currentEntry.index + (-1)"));
    assert!(forward.contains("currentEntry.index + (1)"));
    assert!(back.contains("target.key"));
    assert!(back.contains("target.url"));

    let approved = std::sync::Mutex::new(HashSet::new());
    let result = with_agent_navigation_approval(
        &approved,
        "https://public.example/previous".to_string(),
        || Err("history traversal failed".to_string()),
    );
    assert!(result.is_err());
    assert!(approved.lock().unwrap().is_empty());
}

#[test]
fn agent_navigation_commit_tracker_changes_only_after_successful_webview_dispatch() {
    let rejected_tracker = BrowserActCommitTracker::default();
    let rejected: Result<(), String> = dispatch_browser_navigation(Some(&rejected_tracker), || {
        Err("WebView rejected navigation".to_string())
    });
    assert!(rejected.is_err());
    assert!(!rejected_tracker.effect_may_have_occurred());

    let committed_tracker = BrowserActCommitTracker::default();
    dispatch_browser_navigation(Some(&committed_tracker), || Ok(())).unwrap();
    assert!(committed_tracker.effect_may_have_occurred());
}

#[test]
fn terminal_browser_dispatch_marks_uncertainty_before_native_close_returns() {
    let tracker = BrowserActCommitTracker::default();
    let rejected: Result<(), String> = dispatch_terminal_browser_mutation(Some(&tracker), || {
        Err("native close returned an error after dispatch".to_string())
    });

    assert!(rejected.is_err());
    assert_eq!(
        tracker.failure("close failed".to_string()).phase,
        BrowserActFailurePhase::EffectMayHaveOccurred
    );
}

#[test]
fn terminal_session_close_follows_the_active_tab_across_three_tabs() {
    let mut remaining = vec![
        "tab-a".to_string(),
        "tab-b".to_string(),
        "tab-c".to_string(),
    ];
    let mut active = Some("tab-b".to_string());
    let mut close_order = Vec::new();

    loop {
        let next = next_active_tab_for_terminal_close(active.as_deref(), &remaining)
            .expect("active tab state must remain closable");
        let Some(next) = next else {
            break;
        };
        close_order.push(next.clone());
        remaining.retain(|tab_id| tab_id != &next);
        active = remaining.first().cloned();
    }

    assert_eq!(close_order, vec!["tab-b", "tab-a", "tab-c"]);
    assert!(remaining.is_empty());
    assert!(next_active_tab_for_terminal_close(Some("stale"), &remaining).is_err());
}

#[test]
fn temporary_profile_cleanup_path_cannot_escape_its_absolute_root() {
    let root = std::env::temp_dir().join("nexa-browser-profile-root");
    assert_eq!(
        validated_temporary_profile_dir(&root, "profile-123").unwrap(),
        root.join("profile-123")
    );
    for profile_id in ["../escape", "nested/profile", "/absolute", ""] {
        assert!(
            validated_temporary_profile_dir(&root, profile_id).is_err(),
            "unsafe profile id must be rejected: {profile_id}"
        );
    }
}

#[test]
fn terminal_close_phase_excludes_in_flight_and_new_tab_opening() {
    assert!(BrowserSessionPhase::Active.accepts_new_tabs());
    assert!(BrowserSessionPhase::Active.begin_close(1).is_err());

    let closing = BrowserSessionPhase::Active.begin_close(0).unwrap();
    assert_eq!(closing, BrowserSessionPhase::Closing);
    assert!(!closing.accepts_new_tabs());
    assert!(!BrowserSessionPhase::CleanupPending.accepts_new_tabs());
    assert_eq!(
        BrowserSessionPhase::CleanupPending.begin_close(0).unwrap(),
        BrowserSessionPhase::Closing
    );
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
    assert!(BROWSER_INIT_SCRIPT.contains("`v2|${location.href}|${scrollX}|${scrollY}|"));
    assert!(BROWSER_INIT_SCRIPT.contains("hashText(interactiveState)"));
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
fn visibility_revisions_reject_stale_or_replayed_bounds_updates() {
    let mut current = 0;
    accept_visibility_revision(&mut current, 1).expect("first revision");
    assert!(accept_visibility_revision(&mut current, 1).is_err());
    assert!(accept_visibility_revision(&mut current, 0).is_err());
    accept_visibility_revision(&mut current, 2).expect("newer revision");
    assert_eq!(current, 2);
}

#[test]
fn hidden_inactive_or_tiny_browser_surfaces_reject_agent_commits() {
    let valid = BrowserBounds {
        x: 0.0,
        y: 0.0,
        width: 640.0,
        height: 480.0,
    };
    assert!(agent_tab_surface_is_valid(true, true, valid));
    assert!(!agent_tab_surface_is_valid(false, true, valid));
    assert!(!agent_tab_surface_is_valid(true, false, valid));
    assert!(!agent_tab_surface_is_valid(
        true,
        true,
        BrowserBounds {
            width: 1.0,
            height: 1.0,
            ..valid
        }
    ));
}

#[test]
fn hidden_or_full_sessions_reject_tabs_and_popups() {
    assert!(browser_tab_open_allowed(0, 0, true, false));
    assert!(!browser_tab_open_allowed(1, 0, false, false));
    assert!(browser_tab_open_allowed(15, 0, false, true));
    assert!(!browser_tab_open_allowed(15, 1, false, true));
    assert!(!browser_tab_open_allowed(16, 0, false, true));
}

#[test]
fn workspace_visibility_request_remains_until_matching_visible_revision() {
    let requested = next_visibility_request_revision(7, None);
    assert_eq!(requested, 8);
    assert_eq!(next_visibility_request_revision(7, Some(requested)), 8);
    assert!(!visibility_request_is_satisfied(Some(requested), false, 8));
    assert!(!visibility_request_is_satisfied(Some(requested), true, 7));
    assert!(visibility_request_is_satisfied(Some(requested), true, 8));
}

#[test]
fn post_action_settle_distinguishes_observed_change_from_noop() {
    assert!(!action_snapshot_changed(
        "https://example.com/",
        "fingerprint-a",
        3,
        "https://example.com/",
        "fingerprint-a",
        3,
    ));
    assert!(action_snapshot_changed(
        "https://example.com/",
        "fingerprint-a",
        3,
        "https://example.com/next",
        "fingerprint-b",
        3,
    ));
}

fn trusted_budget_target(tag: &str, role: &str, input_type: Option<&str>) -> BrowserElement {
    BrowserElement {
        element_ref: "element-1".to_string(),
        tag: tag.to_string(),
        role: role.to_string(),
        name: "Test target".to_string(),
        href: None,
        input_type: input_type.map(str::to_string),
        enabled: true,
        visible: true,
        bounds: BrowserElementBounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 30.0,
        },
        locator_fingerprint: BrowserLocatorFingerprint {
            tag: Some(tag.to_string()),
            id: None,
            test_id: None,
            name: None,
            href: None,
            css_path: None,
            text_hash: None,
        },
    }
}

#[test]
fn trusted_action_budgets_cover_exact_native_input_side_effects() {
    let checkbox = trusted_budget_target("input", "checkbox", Some("checkbox"));
    assert_eq!(
        trusted_action_budget("click", Some(&checkbox), None).unwrap(),
        TrustedInputEventBudget::pointer_click(1, 1).unwrap()
    );
    assert_eq!(
        trusted_action_budget("double_click", Some(&checkbox), None).unwrap(),
        TrustedInputEventBudget::pointer_click(2, 2).unwrap()
    );

    let button = trusted_budget_target("button", "button", Some("submit"));
    assert_eq!(
        trusted_action_budget("click", Some(&button), None).unwrap(),
        TrustedInputEventBudget::pointer_click(1, 0).unwrap()
    );
    assert_eq!(
        trusted_action_budget("press", Some(&button), Some("Tab")).unwrap(),
        TrustedInputEventBudget::key_press(0).unwrap()
    );

    let textbox = trusted_budget_target("textarea", "textbox", None);
    assert_eq!(
        trusted_action_budget("type", Some(&textbox), None).unwrap(),
        TrustedInputEventBudget::text_insert()
    );
    for key in [" ", "Enter", "Backspace", "Delete"] {
        assert_eq!(
            trusted_action_budget("press", Some(&textbox), Some(key)).unwrap(),
            TrustedInputEventBudget::key_press(1).unwrap(),
            "{key} should reserve the target's one native input event"
        );
    }

    let select = trusted_budget_target("select", "combobox", None);
    assert_eq!(
        trusted_action_budget("press", Some(&select), Some("ArrowDown")).unwrap(),
        TrustedInputEventBudget::key_press(1).unwrap()
    );
    assert!(trusted_action_budget("press", Some(&button), Some("F5")).is_err());
    assert!(trusted_action_budget("click", None, None).is_err());
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
