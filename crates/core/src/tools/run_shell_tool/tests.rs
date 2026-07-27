use super::*;
use crate::sources::CreateSourceInput;
use std::ffi::OsString;

fn db_with_source(root: &Path) -> Database {
    let db = Database::open_memory().expect("open memory db");
    db.add_source(CreateSourceInput {
        root_path: root.to_string_lossy().to_string(),
        include_globs: vec![],
        exclude_globs: vec![],
        watch_enabled: false,
    })
    .expect("register source");
    db
}

// --- validate_program ---------------------------------------------------

#[test]
fn test_reject_non_whitelisted_program() {
    for bad in &["rm", "curl", "sh", "powershell", "cmd", "bash", "zsh"] {
        assert!(
            validate_program(bad, ShellAccessMode::Restricted).is_err(),
            "expected '{bad}' to be rejected"
        );
    }
}

#[test]
fn test_accept_whitelisted_program() {
    for good in &[
        "python", "python3", "pip", "pip3", "node", "npm", "npx", "git", "pwd", "ls", "cat",
        "mkdir", "cp", "mv", "copy", "move",
    ] {
        assert!(
            validate_program(good, ShellAccessMode::Restricted).is_ok(),
            "expected '{good}' to be accepted"
        );
    }
}

#[test]
fn test_pip_program_normalizes_to_python_module_invocation() {
    let parsed = parse_run_shell_args(
        r#"{"program":"pip","args":["install","python-docx"],"cwd":"C:\\work"}"#,
    )
    .expect("pip args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("pip should normalize");

    assert_eq!(program, "python");
    assert_eq!(args, vec!["-m", "pip", "install", "python-docx"]);
}

#[test]
fn test_command_string_normalizes_to_argv() {
    let parsed = parse_run_shell_args(r#"{"command":"git status --short","cwd":"C:\\work"}"#)
        .expect("command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("simple command should normalize");

    assert_eq!(program, "git");
    assert_eq!(args, vec!["status", "--short"]);
}

#[test]
fn test_command_string_preserves_quoted_args() {
    let parsed = parse_run_shell_args(
        r#"{"command":"python -c \"print('hello world')\"","cwd":"C:\\work"}"#,
    )
    .expect("quoted command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("quoted command should normalize");

    assert_eq!(program, "python");
    assert_eq!(args, vec!["-c", "print('hello world')"]);
}

#[cfg(not(windows))]
#[test]
fn test_command_string_unix_backslash_escapes_spaces() {
    let parsed = parse_run_shell_args(
        r#"{"command":"python path\\ with\\ spaces/script.py","cwd":"/work"}"#,
    )
    .expect("Unix escaped path command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("Unix escaped path command should normalize");

    assert_eq!(program, "python");
    assert_eq!(args, vec!["path with spaces/script.py"]);
}

#[cfg(windows)]
#[test]
fn test_command_string_preserves_windows_backslash_paths() {
    let parsed =
        parse_run_shell_args(r#"{"command":"python C:\\Users\\WYF\\script.py","cwd":"C:\\work"}"#)
            .expect("Windows path command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("Windows path command should normalize");

    assert_eq!(program, "python");
    assert_eq!(args, vec![r#"C:\Users\WYF\script.py"#]);
}

#[cfg(windows)]
#[test]
fn test_command_string_preserves_quoted_windows_path_with_spaces() {
    let parsed = parse_run_shell_args(
        r#"{"command":"python \"C:\\Program Files\\Ask Myself\\script.py\"","cwd":"C:\\work"}"#,
    )
    .expect("quoted Windows path command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect("quoted Windows path command should normalize");

    assert_eq!(program, "python");
    assert_eq!(args, vec![r#"C:\Program Files\Ask Myself\script.py"#]);
}

#[test]
fn test_shell_command_rejected_in_restricted_mode() {
    let parsed = parse_run_shell_args(
        r#"{"command":"git status --short && git diff --stat","shell":"default","cwd":"C:\\work"}"#,
    )
    .expect("shell command args should parse");

    let err = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect_err("restricted mode should reject shell execution");

    assert!(err.contains("ConfirmAll or Open"));
}

#[test]
fn test_shell_command_maps_default_shell_in_open_mode() {
    let command = "git status --short && git diff --stat";
    let parsed = parse_run_shell_args(&format!(
        r#"{{"command":"{command}","shell":"default","cwd":"C:\\work"}}"#
    ))
    .expect("default shell command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Open)
        .expect("default shell command should normalize");

    #[cfg(windows)]
    {
        assert_eq!(program, "powershell.exe");
        assert!(args.contains(&"-Command".to_string()));
        assert_eq!(args.last().map(String::as_str), Some(command));
    }
    #[cfg(not(windows))]
    {
        assert_eq!(program, "sh");
        assert_eq!(args, vec!["-c", command]);
    }
}

#[test]
fn test_explicit_bash_shell_preserves_shell_operators() {
    let command = "printf hi && printf bye";
    let parsed = parse_run_shell_args(&format!(
        r#"{{"command":"{command}","shell":"bash","cwd":"C:\\work"}}"#
    ))
    .expect("bash shell command args should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::ConfirmAll)
        .expect("bash shell command should normalize");

    assert_eq!(program, "bash");
    assert_eq!(args, vec!["-lc", command]);
}

#[test]
fn test_shell_command_rejects_program_args_mix() {
    let parsed = parse_run_shell_args(
            r#"{"command":"git status","shell":"default","program":"git","args":["status"],"cwd":"C:\\work"}"#,
        )
        .expect("mixed shell args should parse");

    let err = normalize_run_shell_invocation(&parsed, ShellAccessMode::Open)
        .expect_err("shell and argv modes should not mix");

    assert!(err.contains("do not combine shell"));
}

#[test]
fn test_command_string_rejects_shell_operators() {
    let parsed =
        parse_run_shell_args(r#"{"command":"git status --short && git diff","cwd":"C:\\work"}"#)
            .expect("operator command args should parse");

    let err = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect_err("shell operator should be rejected");

    assert!(err.contains("shell operators"));
}

#[test]
fn test_command_string_rejects_ambiguous_args() {
    let parsed = parse_run_shell_args(
        r#"{"command":"git status","program":"git","args":["status"],"cwd":"C:\\work"}"#,
    )
    .expect("ambiguous args should parse");

    let err = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect_err("ambiguous invocation should be rejected");

    assert!(err.contains("either"));
}

#[test]
fn test_command_string_enforces_restricted_whitelist() {
    let parsed = parse_run_shell_args(r#"{"command":"rm -rf .","cwd":"C:\\work"}"#)
        .expect("non-whitelisted command should parse");

    let err = normalize_run_shell_invocation(&parsed, ShellAccessMode::Restricted)
        .expect_err("restricted whitelist should still apply");

    assert!(err.contains("whitelist"));
}

#[test]
fn test_lenient_parser_repairs_unescaped_windows_paths() {
    let parsed = parse_run_shell_args(
        r#"{"program":"python","args":["E:\Starting\convert_to_docx.py"],"cwd":"E:\Starting"}"#,
    )
    .expect("unescaped Windows paths should be repaired");

    assert_eq!(parsed.program.as_deref(), Some("python"));
    assert_eq!(parsed.args, vec![r#"E:\Starting\convert_to_docx.py"#]);
    assert_eq!(parsed.cwd.as_deref(), Some(r#"E:\Starting"#));
}

#[test]
fn test_parser_allows_omitted_cwd() {
    let parsed = parse_run_shell_args(r#"{"command":"git status --short"}"#)
        .expect("cwd should be optional");

    assert!(parsed.cwd.is_none());
}

#[test]
fn test_open_mode_auto_promotes_shell_syntax() {
    let parsed = parse_run_shell_args(
        r#"{"command":"git status --short && git diff --stat","cwd":"C:\\work"}"#,
    )
    .expect("shell command should parse");

    let (program, args) = normalize_run_shell_invocation(&parsed, ShellAccessMode::Open)
        .expect("open mode should use the platform shell automatically");

    assert!(!program.is_empty());
    assert!(args
        .iter()
        .any(|arg| arg.contains("git status --short && git diff --stat")));
}

#[test]
fn test_parser_accepts_managed_background_service_fields() {
    let parsed = parse_run_shell_args(
        r#"{"command":"python -m http.server 8080","cwd":"C:\\work","background":true,"ready_url":"http://127.0.0.1:8080","ready_timeout_secs":25}"#,
    )
    .expect("background service args should parse");

    assert!(parsed.background);
    assert_eq!(parsed.ready_url.as_deref(), Some("http://127.0.0.1:8080"));
    assert_eq!(parsed.ready_timeout_secs, Some(25));
}

#[test]
fn test_persistent_servers_are_recognized_for_automatic_backgrounding() {
    assert!(looks_like_persistent_service(
        "python",
        &["server.py".to_string()]
    ));
    assert!(looks_like_persistent_service(
        "python",
        &[
            "-m".to_string(),
            "http.server".to_string(),
            "8080".to_string()
        ]
    ));
    assert!(looks_like_persistent_service(
        "npm",
        &["run".to_string(), "dev".to_string()]
    ));
    assert!(!looks_like_persistent_service(
        "python",
        &["scripts/check.py".to_string()]
    ));
}

#[tokio::test]
#[ignore = "requires python3 on PATH"]
async fn test_managed_http_service_start_status_and_stop() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let tmp = tempfile::tempdir().unwrap();
    let db = db_with_source(tmp.path());
    let tool = RunShellTool;
    let ready_url = format!("http://127.0.0.1:{port}");
    let start_args = json!({
        "command": format!("python3 -m http.server {port}"),
        "cwd": tmp.path().to_string_lossy(),
        "background": true,
        "ready_url": ready_url,
        "ready_timeout_secs": 15,
    });

    let started = tool
        .execute("managed-http", &start_args.to_string(), &db, &[])
        .await
        .expect("managed service should return a tool result");
    assert!(
        !started.is_error,
        "unexpected start error: {}",
        started.content
    );
    assert_eq!(started.artifacts.as_ref().unwrap()["status"], "ready");

    let status_args = json!({
        "service_action": "status",
        "service_id": "managed-http",
        "cwd": tmp.path().to_string_lossy(),
    });
    let status = tool
        .execute("managed-http-status", &status_args.to_string(), &db, &[])
        .await
        .expect("managed status should return a tool result");
    assert!(
        !status.is_error,
        "unexpected status error: {}",
        status.content
    );
    assert_eq!(status.artifacts.as_ref().unwrap()["status"], "ready");

    let stop_args = json!({
        "service_action": "stop",
        "service_id": "managed-http",
        "cwd": tmp.path().to_string_lossy(),
    });
    let stopped = tool
        .execute("managed-http-stop", &stop_args.to_string(), &db, &[])
        .await
        .expect("managed stop should return a tool result");
    assert!(
        !stopped.is_error,
        "unexpected stop error: {}",
        stopped.content
    );
    assert_eq!(stopped.artifacts.as_ref().unwrap()["status"], "stopped");
}

#[tokio::test]
#[ignore = "requires python3 on PATH"]
async fn test_python_server_script_is_auto_promoted_and_discovers_url() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("server.py"),
        format!(
            "import http.server\nimport socketserver\nPORT = {port}\nprint(f'http://127.0.0.1:{{PORT}}', flush=True)\nwith socketserver.TCPServer(('127.0.0.1', PORT), http.server.SimpleHTTPRequestHandler) as server:\n    server.serve_forever()\n"
        ),
    )
    .unwrap();
    let db = db_with_source(tmp.path());
    let tool = RunShellTool;
    let start_args = json!({
        "command": "python3 server.py",
        "cwd": tmp.path().to_string_lossy(),
    });

    let started = tool
        .execute("auto-python-server", &start_args.to_string(), &db, &[])
        .await
        .expect("auto-promoted service result");
    assert!(!started.is_error, "unexpected error: {}", started.content);
    let artifacts = started.artifacts.as_ref().unwrap();
    assert_eq!(artifacts["autoPromoted"], true);
    assert_eq!(artifacts["status"], "ready");
    assert_eq!(artifacts["readyUrl"], format!("http://127.0.0.1:{port}/"));

    let stop_args = json!({
        "service_action": "stop",
        "service_id": "auto-python-server",
        "cwd": tmp.path().to_string_lossy(),
    });
    let stopped = tool
        .execute("auto-python-server-stop", &stop_args.to_string(), &db, &[])
        .await
        .expect("managed stop result");
    assert!(!stopped.is_error);
}

#[tokio::test]
async fn test_invalid_json_returns_run_shell_contract_error() {
    let tmp = tempfile::tempdir().unwrap();
    let db = db_with_source(tmp.path());
    let tool = RunShellTool;

    let result = tool
        .execute(
            "bad-run-shell-json",
            r#"{"program":"python","args":["-c","print("#,
            &db,
            &[],
        )
        .await
        .expect("malformed arguments should be returned as a tool result");

    assert!(result.is_error);
    assert!(result.content.contains("Invalid run_shell arguments"));
    assert!(result.content.contains("--spec -"));
    let artifacts = result.artifacts.expect("contract error artifact");
    assert_eq!(artifacts["kind"], "toolContractError");
    assert_eq!(artifacts["code"], "invalid_run_shell_arguments");
    assert!(artifacts["expectedFormat"]["examples"]["simpleCommand"]["command"].is_string());
    assert!(artifacts["expectedFormat"]["examples"]["exactArgv"]["args"].is_array());
    assert!(
        artifacts["expectedFormat"]["examples"]["exactArgv"]["stdin"]
            .as_str()
            .unwrap()
            .contains("HTML-first PPTX")
    );
}

#[test]
fn test_open_mode_accepts_non_whitelisted_program() {
    assert_eq!(
        validate_program("bash", ShellAccessMode::Open).unwrap(),
        "bash"
    );
    assert_eq!(
        validate_program("copy", ShellAccessMode::Open).unwrap(),
        "cp"
    );
}

#[test]
fn test_reject_program_with_path_separator() {
    assert!(validate_program("/usr/bin/python", ShellAccessMode::Restricted).is_err());
    assert!(validate_program("..\\python", ShellAccessMode::Open).is_err());
}

// --- validate_args ------------------------------------------------------

#[test]
fn test_reject_null_byte_in_args() {
    let args = vec!["hello\0world".to_string()];
    assert!(validate_args(ShellAccessMode::Restricted, "python", &args).is_err());
}

#[test]
fn test_reject_oversized_arg() {
    let big = "x".repeat(MAX_SINGLE_ARG_BYTES + 1);
    let args = vec![big];
    assert!(validate_args(ShellAccessMode::Restricted, "python", &args).is_err());
}

#[test]
fn test_reject_total_argv_too_large() {
    // Many args just under the single-arg limit, totalling > 32 KB.
    let chunk = "x".repeat(4 * 1024);
    let args: Vec<String> = (0..10).map(|_| chunk.clone()).collect();
    assert!(validate_args(ShellAccessMode::Restricted, "python", &args).is_err());
}

#[test]
fn test_stdin_cap_allows_larger_than_single_argv() {
    let input = "x".repeat(MAX_SINGLE_ARG_BYTES + 1);
    assert!(validate_stdin(Some(&input)).is_ok());
}

#[test]
fn test_reject_oversized_stdin() {
    let input = "x".repeat(MAX_STDIN_BYTES + 1);
    assert!(validate_stdin(Some(&input)).is_err());
}

#[test]
fn test_git_requires_readonly_subcommand() {
    assert!(validate_args(ShellAccessMode::Restricted, "git", &["status".to_string()]).is_ok());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &["diff".to_string(), "--stat".to_string()]
    )
    .is_ok());
    assert!(validate_args(ShellAccessMode::Restricted, "git", &["push".to_string()]).is_err());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &["commit".to_string(), "-m".to_string(), "x".to_string()]
    )
    .is_err());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &["reset".to_string(), "--hard".to_string()]
    )
    .is_err());
}

#[test]
fn test_git_empty_args_rejected() {
    let empty: Vec<String> = vec![];
    assert!(validate_args(ShellAccessMode::Restricted, "git", &empty).is_err());
}

#[test]
fn test_git_forbidden_token_in_later_args_rejected() {
    // Primary subcommand is OK but a forbidden token appears later.
    let args = vec![
        "config".to_string(),
        "--unset".to_string(),
        "user.name".to_string(),
    ];
    assert!(validate_args(ShellAccessMode::Restricted, "git", &args).is_err());
}

#[test]
fn test_git_config_requires_readonly_flag() {
    fn s(arr: &[&str]) -> Vec<String> {
        arr.iter().map(|x| x.to_string()).collect()
    }
    // Positional-write form: must reject.
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &s(&["config", "user.name", "evil"])
    )
    .is_err());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &s(&["config", "core.editor", "vim"])
    )
    .is_err());
    // Bare `git config` (no subaction): must reject.
    assert!(validate_args(ShellAccessMode::Restricted, "git", &s(&["config"])).is_err());
    // Read-only forms: must pass.
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &s(&["config", "--list"])
    )
    .is_ok());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &s(&["config", "--get", "user.name"])
    )
    .is_ok());
    assert!(validate_args(
        ShellAccessMode::Restricted,
        "git",
        &s(&["config", "--get-regexp", "^alias\\."])
    )
    .is_ok());
    assert!(validate_args(ShellAccessMode::Restricted, "git", &s(&["config", "-l"])).is_ok());
}

#[test]
fn test_python_accepts_arbitrary_args() {
    let args = vec!["-c".to_string(), "print('hello')".to_string()];
    assert!(validate_args(ShellAccessMode::Restricted, "python", &args).is_ok());
}

#[test]
fn test_pwd_rejects_args() {
    assert!(validate_args(ShellAccessMode::Restricted, "pwd", &["oops".to_string()]).is_err());
}

#[test]
fn test_open_mode_allows_write_style_git_args() {
    assert!(validate_args(ShellAccessMode::Open, "git", &["push".to_string()]).is_ok());
}

#[test]
fn test_collect_positional_args_respects_double_dash() {
    let args = vec![
        "-l".to_string(),
        "--".to_string(),
        "-literal".to_string(),
        "file.txt".to_string(),
    ];
    assert_eq!(collect_positional_args(&args), vec!["-literal", "file.txt"]);
}

// --- build_env ----------------------------------------------------------

#[test]
fn test_env_build_strips_secrets() {
    let parent: Vec<(OsString, OsString)> = vec![
        (OsString::from("PATH"), OsString::from("/usr/bin")),
        (
            OsString::from("AWS_ACCESS_KEY_ID"),
            OsString::from("AKIA..."),
        ),
        (OsString::from("MY_SECRET_TOKEN"), OsString::from("hunter2")),
        (OsString::from("GITHUB_TOKEN"), OsString::from("ghp_...")),
        (OsString::from("OPENAI_API_KEY"), OsString::from("sk-...")),
        (OsString::from("LANG"), OsString::from("en_US.UTF-8")),
        (OsString::from("HOME"), OsString::from("/home/user")),
        (OsString::from("FOO_PASSWORD"), OsString::from("swordfish")),
        (OsString::from("CREDENTIALS_DIR"), OsString::from("/tmp/c")),
        (OsString::from("MY_NORMAL_VAR"), OsString::from("ok")),
    ];
    let built = build_env_from(parent);
    let keys: Vec<String> = built
        .iter()
        .map(|(k, _)| k.to_string_lossy().to_string())
        .collect();

    // Preserved
    assert!(keys.iter().any(|k| k == "PATH"), "PATH should be preserved");
    assert!(keys.iter().any(|k| k == "LANG"), "LANG should be preserved");
    assert!(keys.iter().any(|k| k == "HOME"), "HOME should be preserved");
    assert!(
        keys.iter().any(|k| k == "MY_NORMAL_VAR"),
        "normal vars pass through"
    );

    // Stripped
    assert!(!keys.iter().any(|k| k == "AWS_ACCESS_KEY_ID"));
    assert!(!keys.iter().any(|k| k == "MY_SECRET_TOKEN"));
    assert!(!keys.iter().any(|k| k == "GITHUB_TOKEN"));
    assert!(!keys.iter().any(|k| k == "OPENAI_API_KEY"));
    assert!(!keys.iter().any(|k| k == "FOO_PASSWORD"));
    assert!(!keys.iter().any(|k| k == "CREDENTIALS_DIR"));
}

// --- clamp_timeout ------------------------------------------------------

#[test]
fn test_timeout_allows_unbounded_and_long_running_commands() {
    assert_eq!(clamp_timeout(Some(10_000)), 10_000);
    assert_eq!(clamp_timeout(Some(0)), 0);
    assert_eq!(clamp_timeout(Some(30)), 30);
    assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_SECS);
}

// --- Tool trait behaviour ----------------------------------------------

#[test]
fn test_confirmation_required() {
    let tool = RunShellTool;
    let args = serde_json::json!({
        "program": "python",
        "args": ["-c", "print(1)"],
        "cwd": "."
    });
    assert!(!tool.requires_confirmation(&args));
    assert!(!tool.requires_confirmation(&serde_json::json!({})));
}

#[test]
fn test_confirmation_message_excludes_env() {
    let tool = RunShellTool;
    let args = serde_json::json!({
        "program": "git",
        "args": ["status", "--short"],
        "cwd": "/workspace/project",
        "timeout_secs": 45
    });
    let msg = tool.confirmation_message(&args).expect("message present");
    assert!(msg.contains("git"));
    assert!(msg.contains("status"));
    assert!(msg.contains("--short"));
    assert!(msg.contains("/workspace/project"));
    assert!(msg.contains("45s"));
    // No env-var leakage.
    assert!(!msg.to_uppercase().contains("PATH="));
    assert!(!msg.to_uppercase().contains("TOKEN"));
    assert!(!msg.to_uppercase().contains("SECRET"));
}

#[test]
fn test_confirmation_message_shows_long_timeout() {
    let tool = RunShellTool;
    let args = serde_json::json!({
        "program": "python",
        "args": [],
        "cwd": ".",
        "timeout_secs": 99_999
    });
    let msg = tool.confirmation_message(&args).expect("message");
    assert!(msg.contains("99999s"));
}

#[test]
fn test_confirmation_message_shows_no_timeout() {
    let tool = RunShellTool;
    let args = serde_json::json!({
        "program": "python",
        "args": ["-m", "pip", "install", "large-package"],
        "cwd": ".",
        "timeout_secs": 0
    });
    let msg = tool.confirmation_message(&args).expect("message");
    assert!(msg.contains("no timeout"));
    assert!(!msg.contains("timeout 0s"));
}

#[test]
fn test_confirmation_message_shows_shell_mode() {
    let tool = RunShellTool;
    let args = serde_json::json!({
        "command": "git status --short && git diff --stat",
        "shell": "default",
        "cwd": "."
    });
    let msg = tool.confirmation_message(&args).expect("message");
    assert!(msg.contains("default shell"));
    assert!(msg.contains("git status --short && git diff --stat"));
}

// --- bytes_to_clamped_string -------------------------------------------

#[test]
fn test_output_truncation_respects_utf8() {
    // Build a buffer larger than max ending on a multi-byte char.
    let mut bytes = vec![b'a'; 10];
    // Append a 3-byte UTF-8 char that would straddle the cut point.
    bytes.extend_from_slice("€".as_bytes()); // 3 bytes
    let (s, trunc) = bytes_to_clamped_string(&bytes, 11);
    assert!(trunc);
    // Result must be valid UTF-8 and not contain the partial char.
    assert_eq!(s, "a".repeat(10));
}

#[test]
fn test_output_truncation_preserves_diagnostic_tail() {
    let bytes = format!("{}{}", "a".repeat(200), "FINAL COMPILER ERROR").into_bytes();
    let (output, truncated) = bytes_to_clamped_string(&bytes, 100);

    assert!(truncated);
    assert!(output.starts_with('a'));
    assert!(output.contains("output middle omitted"));
    assert!(output.ends_with("FINAL COMPILER ERROR"));
    assert!(output.len() <= 100);
}

// --- Integration tests (need real binaries; ignored by default) --------

// --- resolve_program ----------------------------------------------------

#[test]
fn test_resolve_program_identity() {
    // Non-python programs are returned unchanged on all platforms.
    assert_eq!(resolve_program("git"), "git");
    assert_eq!(resolve_program("node"), "node");
    assert_eq!(resolve_program("npm"), "npm");
    assert_eq!(resolve_program("npx"), "npx");
    assert_eq!(resolve_program("cp"), "cp");
    assert_eq!(resolve_program("mv"), "mv");
}

// --- build_env UTF-8 injection ------------------------------------------

#[test]
fn test_env_includes_pythonutf8() {
    let parent: Vec<(OsString, OsString)> =
        vec![(OsString::from("PATH"), OsString::from("/usr/bin"))];
    let built = build_env_from(parent);
    let keys: Vec<String> = built
        .iter()
        .map(|(k, _)| k.to_string_lossy().to_string())
        .collect();
    assert!(
        keys.contains(&"PYTHONUTF8".to_string()),
        "PYTHONUTF8 must be present"
    );
    assert!(
        keys.contains(&"PYTHONIOENCODING".to_string()),
        "PYTHONIOENCODING must be present"
    );

    let pythonutf8_val = built
        .iter()
        .find(|(k, _)| k == "PYTHONUTF8")
        .map(|(_, v)| v.to_string_lossy().to_string())
        .unwrap();
    assert_eq!(pythonutf8_val, "1");

    let pyioenc_val = built
        .iter()
        .find(|(k, _)| k == "PYTHONIOENCODING")
        .map(|(_, v)| v.to_string_lossy().to_string())
        .unwrap();
    assert_eq!(pyioenc_val, "utf-8");
}

#[test]
fn test_env_prepends_app_managed_office_python() {
    let dir = tempfile::tempdir().unwrap();
    let office_bin = dir.path().join("office-python-bin");
    std::fs::create_dir_all(&office_bin).unwrap();
    let original_path = std::env::join_paths([PathBuf::from("/usr/bin")]).unwrap();
    let parent: Vec<(OsString, OsString)> = vec![
        (OsString::from("PATH"), original_path),
        (
            OsString::from(crate::office_runtime::OFFICE_PYTHON_BIN_DIR_ENV),
            office_bin.as_os_str().to_os_string(),
        ),
    ];
    let built = build_env_from(parent);
    let path_value = built
        .iter()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone())
        .unwrap();
    let first = std::env::split_paths(&path_value).next().unwrap();
    assert_eq!(first, office_bin);
}

// --- format_output ------------------------------------------------------

#[test]
fn test_format_output_success() {
    let output = RunShellOutput {
        exit_code: Some(0),
        stdout: "hello world\n".to_string(),
        stderr: String::new(),
        duration_ms: 42,
        truncated_stdout: false,
        truncated_stderr: false,
        killed_by_timeout: false,
    };
    let text = format_output(&output);
    assert!(text.contains("Exit code: 0"), "should contain exit code");
    assert!(text.contains("Duration: 42ms"), "should contain duration");
    assert!(text.contains("stdout"), "should contain stdout header");
    assert!(
        text.contains("hello world"),
        "should contain stdout content"
    );
    assert!(
        !text.contains("stderr"),
        "should not contain stderr when empty"
    );
}

#[test]
fn test_format_output_timeout() {
    let output = RunShellOutput {
        exit_code: None,
        stdout: String::new(),
        stderr: "run_shell: killed after 30s timeout".to_string(),
        duration_ms: 30000,
        truncated_stdout: false,
        truncated_stderr: false,
        killed_by_timeout: true,
    };
    let text = format_output(&output);
    assert!(text.contains("timeout"), "should mention timeout");
    assert!(text.contains("stderr"), "should contain stderr header");
}

#[test]
fn test_format_output_stderr() {
    let output = RunShellOutput {
        exit_code: Some(1),
        stdout: "partial\n".to_string(),
        stderr: "error: something failed\n".to_string(),
        duration_ms: 100,
        truncated_stdout: false,
        truncated_stderr: false,
        killed_by_timeout: false,
    };
    let text = format_output(&output);
    assert!(text.contains("Exit code: 1"));
    assert!(text.contains("stdout"));
    assert!(text.contains("stderr"));
    assert!(text.contains("error: something failed"));
}

#[test]
fn test_format_output_truncation_markers() {
    let output = RunShellOutput {
        exit_code: Some(0),
        stdout: "data".to_string(),
        stderr: "warn".to_string(),
        duration_ms: 10,
        truncated_stdout: true,
        truncated_stderr: true,
        killed_by_timeout: false,
    };
    let text = format_output(&output);
    // Both truncation markers should appear
    assert_eq!(text.matches("truncated to 64KB").count(), 2);
}

#[tokio::test]
async fn test_run_shell_reports_created_text_file_diff() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("source.txt"), "hello\n").unwrap();
    let db = db_with_source(tmp.path());
    let tool = RunShellTool;
    let args = json!({
        "program": "cp",
        "args": ["source.txt", "copy.txt"],
        "cwd": tmp.path().to_string_lossy(),
    });

    let result = tool
        .execute("run-shell-copy", &args.to_string(), &db, &[])
        .await
        .unwrap();

    assert!(!result.is_error, "unexpected error: {}", result.content);
    let artifact = result.artifacts.as_ref().expect("file changes artifact");
    assert_eq!(artifact["kind"], "fileChangeSet");
    assert_eq!(artifact["source"], "run_shell");
    assert_eq!(artifact["diffStats"]["filesChanged"], 1);
    assert_eq!(artifact["diffStats"]["additions"], 1);
    assert_eq!(artifact["diffStats"]["deletions"], 0);
    assert_eq!(artifact["diffStats"]["paths"][0], "copy.txt");
    assert_eq!(artifact["diff"]["operation"], "create");
    assert_eq!(artifact["diff"]["path"], "copy.txt");
    assert_eq!(
        artifact["diff"]["absolutePath"],
        tmp.path().join("copy.txt").to_string_lossy().to_string()
    );
    assert_eq!(artifact["fileChanges"][0]["operation"], "create");
    assert_eq!(
        artifact["fileChanges"][0]["absolutePath"],
        tmp.path().join("copy.txt").to_string_lossy().to_string()
    );
    assert!(result.content.contains("file changes"));
    assert!(result.content.contains("copy.txt"));
}

#[tokio::test]
async fn test_run_shell_reports_modified_text_file_diff() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("source.txt"), "new\n").unwrap();
    std::fs::write(tmp.path().join("dest.txt"), "old\n").unwrap();
    let db = db_with_source(tmp.path());
    let tool = RunShellTool;
    let args = json!({
        "program": "cp",
        "args": ["source.txt", "dest.txt"],
        "cwd": tmp.path().to_string_lossy(),
    });

    let result = tool
        .execute("run-shell-overwrite", &args.to_string(), &db, &[])
        .await
        .unwrap();

    assert!(!result.is_error, "unexpected error: {}", result.content);
    let artifact = result.artifacts.as_ref().expect("file changes artifact");
    assert_eq!(artifact["diffStats"]["filesChanged"], 1);
    assert_eq!(artifact["diffStats"]["additions"], 1);
    assert_eq!(artifact["diffStats"]["deletions"], 1);
    assert_eq!(artifact["diffStats"]["paths"][0], "dest.txt");
    assert_eq!(artifact["diff"]["operation"], "run_shell");
    assert_eq!(
        artifact["diff"]["absolutePath"],
        tmp.path().join("dest.txt").to_string_lossy().to_string()
    );
    assert!(artifact["diff"]["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line["type"] == "deletion" && line["content"] == "old"));
    assert!(artifact["diff"]["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line["type"] == "addition" && line["content"] == "new"));
}

#[tokio::test]
async fn test_native_filesystem_mkdir_and_ls() {
    let tmp = tempfile::tempdir().unwrap();
    let out = execute_native_filesystem(
        "mkdir",
        &["notes".to_string(), "drafts".to_string()],
        tmp.path(),
    )
    .await
    .expect("mkdir should run natively");
    assert_eq!(out.exit_code, Some(0));
    assert!(tmp.path().join("notes").is_dir());
    assert!(tmp.path().join("drafts").is_dir());

    let listing = execute_native_filesystem("ls", &[], tmp.path())
        .await
        .expect("ls should run natively");
    assert!(listing.stdout.contains("notes"));
    assert!(listing.stdout.contains("drafts"));
}

#[tokio::test]
async fn test_native_filesystem_cat_cp_and_mv() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("source.txt"), "hello").unwrap();

    let cat = execute_native_filesystem("cat", &["source.txt".to_string()], tmp.path())
        .await
        .expect("cat should run natively");
    assert_eq!(cat.stdout, "hello\n");

    execute_native_filesystem(
        "cp",
        &["source.txt".to_string(), "copy.txt".to_string()],
        tmp.path(),
    )
    .await
    .expect("cp should run natively");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("copy.txt")).unwrap(),
        "hello"
    );

    execute_native_filesystem(
        "mv",
        &["copy.txt".to_string(), "moved.txt".to_string()],
        tmp.path(),
    )
    .await
    .expect("mv should run natively");
    assert!(!tmp.path().join("copy.txt").exists());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("moved.txt")).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn local_run_shell_environment_executes_native_filesystem_request() {
    let tmp = tempfile::tempdir().unwrap();
    let mut request = ExecutionRequest::for_run_shell(
        "mkdir",
        vec!["notes".to_string()],
        ShellAccessMode::Restricted,
        vec![tmp.path().to_string_lossy().to_string()],
    );
    request.cwd = Some(tmp.path().to_string_lossy().to_string());

    let environment = LocalRunShellExecutionEnvironment;
    let artifact = environment
        .execute(request)
        .await
        .expect("environment should execute native filesystem command");

    assert_eq!(environment.id(), "local_run_shell");
    assert_eq!(artifact.decision.kind, ExecutionDecisionKind::Allowed);
    assert_eq!(artifact.exit_status, Some(0));
    assert!(!artifact.timed_out);
    assert!(tmp.path().join("notes").is_dir());
}

#[tokio::test]
async fn local_run_shell_environment_reviews_shell_policy_without_executing() {
    let request = ExecutionRequest::for_run_shell(
        "bash",
        vec!["-lc".to_string(), "echo ok".to_string()],
        ShellAccessMode::ConfirmAll,
        Vec::new(),
    );

    let decision = LocalRunShellExecutionEnvironment
        .review(&request)
        .await
        .expect("review should be deterministic");

    assert_eq!(decision.kind, ExecutionDecisionKind::RequiresApproval);
    assert!(decision.permission_key.starts_with("exec:run_shell"));
}

#[tokio::test]
#[ignore = "requires python on PATH"]
async fn test_python_hello() {
    let tmp = tempfile::tempdir().unwrap();
    let out = execute_inner(
        "python",
        &["-c".to_string(), "print('hello')".to_string()],
        tmp.path(),
        10,
        None,
    )
    .await
    .expect("run ok");
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout.contains("hello"));
    assert!(!out.killed_by_timeout);
}

#[tokio::test]
#[ignore = "requires python on PATH; sleeps"]
async fn test_python_timeout_kills() {
    let tmp = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let out = execute_inner(
        "python",
        &["-c".to_string(), "import time; time.sleep(60)".to_string()],
        tmp.path(),
        2,
        None,
    )
    .await
    .expect("run ok");
    assert!(out.killed_by_timeout);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
#[ignore = "requires python on PATH"]
async fn test_stdout_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    let out = execute_inner(
        "python",
        &["-c".to_string(), "print('x' * 200000)".to_string()],
        tmp.path(),
        10,
        None,
    )
    .await
    .expect("run ok");
    assert!(out.truncated_stdout);
    assert!(out.stdout.len() <= MAX_OUTPUT_BYTES);
}

#[tokio::test]
#[ignore = "requires git on PATH and a repo"]
async fn test_git_status() {
    let out = execute_inner(
        "git",
        &["status".to_string(), "--short".to_string()],
        Path::new("."),
        10,
        None,
    )
    .await
    .expect("run ok");
    assert_eq!(out.exit_code, Some(0));
}

fn source(id: &str, root: &Path) -> Source {
    Source {
        id: id.to_string(),
        kind: "local_folder".to_string(),
        root_path: root.to_string_lossy().to_string(),
        include_globs: vec![],
        exclude_globs: vec![],
        watch_enabled: false,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[test]
fn test_cp_paths_must_stay_within_source_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let cwd = root.join("nested");
    let src = root.join("hello.txt");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(&src, "hello").unwrap();
    let sources = vec![source("src-1", &root)];

    assert!(validate_scoped_args(
        ShellAccessMode::Restricted,
        "cp",
        &["../hello.txt".to_string(), "copy.txt".to_string()],
        &cwd,
        &sources,
    )
    .is_ok());
}

#[test]
fn test_mv_rejects_destination_outside_source_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let cwd = root.join("nested");
    let src = root.join("hello.txt");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(&src, "hello").unwrap();
    let sources = vec![source("src-1", &root)];

    let err = validate_scoped_args(
        ShellAccessMode::Restricted,
        "mv",
        &["../hello.txt".to_string(), "../../escape.txt".to_string()],
        &cwd,
        &sources,
    )
    .unwrap_err();
    assert!(err.contains("Access denied"), "err was: {err}");
}

#[test]
fn test_open_mode_skips_scoped_path_validation() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let cwd = root.join("nested");
    std::fs::create_dir_all(&cwd).unwrap();
    let sources = vec![source("src-1", &root)];

    assert!(validate_scoped_args(
        ShellAccessMode::Open,
        "cp",
        &["/tmp/source.txt".to_string(), "/tmp/dest.txt".to_string()],
        &cwd,
        &sources,
    )
    .is_ok());
}
