use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("read source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files
}

#[test]
fn desktop_host_cannot_escape_through_raw_database_connections() {
    let desktop = repository_root().join("apps/desktop/src-tauri/src");
    let offenders = rust_sources(&desktop)
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read Rust source")
                .contains(".conn()")
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "desktop code must use storage APIs instead of Database::conn(): {offenders:?}"
    );
}

#[test]
fn desktop_host_cannot_construct_the_builtin_tool_registry_directly() {
    let desktop = repository_root().join("apps/desktop/src-tauri/src");
    let offenders = rust_sources(&desktop)
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read Rust source")
                .contains("default_tool_registry()")
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "desktop code must obtain tools from PackageRuntimeAssembler: {offenders:?}"
    );
}

#[test]
fn timeline_visibility_cannot_depend_on_backend_labels() {
    let timeline = repository_root().join("apps/desktop/src/lib/streaming/timelineViewModel.ts");
    let source = fs::read_to_string(timeline).expect("read timeline projection");
    for forbidden in [
        "INTERNAL_TRACE_STATUSES",
        "shouldHideTraceStatus",
        "Task queued",
        "任务已排队",
        "排隊",
        "User steering:",
        "steeringTextFromTraceStatus",
    ] {
        assert!(
            !source.contains(forbidden),
            "timeline projection must use semantic visibility, not label token {forbidden:?}"
        );
    }
}

#[test]
fn chat_ui_consumes_the_canonical_live_timeline_projection() {
    let chat = repository_root().join("apps/desktop/src/features/chat/ChatMessages.tsx");
    let source = fs::read_to_string(chat).expect("read chat UI");
    assert!(source.contains("projectLiveConversationTimeline"));
    for forbidden in [
        "visibleTraceEventsForTimeline,",
        "buildCurrentTimelineSections,",
        "buildLiveTraceTimeline,",
        "buildCollapsedLiveTrace,",
    ] {
        assert!(
            !source.contains(forbidden),
            "ChatMessages must not compose low-level timeline projection {forbidden:?}"
        );
    }
}

#[test]
fn context_hud_uses_theme_tokens_instead_of_tailwind_palette_colors() {
    let hud = repository_root().join("apps/desktop/src/components/chat/ChatRunOverview.tsx");
    let source = fs::read_to_string(hud).expect("read context HUD");
    for forbidden in [
        "bg-sky-",
        "bg-indigo-",
        "bg-amber-",
        "bg-orange-",
        "bg-purple-",
        "bg-pink-",
    ] {
        assert!(
            !source.contains(forbidden),
            "Context HUD colors must use semantic theme variables, not {forbidden}"
        );
    }
    for required in [
        "--context-prompts",
        "--context-conversation",
        "--context-tool-results",
        "--context-tools",
        "--context-mcp",
        "--context-overhead",
    ] {
        assert!(
            source.contains(required),
            "Context HUD must consume semantic variable {required}"
        );
    }
}

#[test]
fn readme_frontend_versions_match_the_manifest() {
    let root = repository_root();
    let package: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("apps/desktop/package.json")).expect("read package.json"),
    )
    .expect("parse package.json");
    let readme = fs::read_to_string(root.join("README.md")).expect("read README");

    for (dependency, label) in [("react", "React"), ("react-router", "React Router")] {
        let version = package["dependencies"][dependency]
            .as_str()
            .expect("frontend dependency version")
            .trim_start_matches(|character: char| !character.is_ascii_digit());
        let documented = version.split('.').take(2).collect::<Vec<_>>().join(".");
        assert!(
            readme.contains(&format!("{label} {documented}")),
            "README must document {label} {documented}"
        );
    }
}
