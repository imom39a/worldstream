use std::process::{Command, Output, Stdio};

fn control(arguments: &[&str]) -> Output {
    let directory =
        tempfile::tempdir().unwrap_or_else(|error| unreachable!("isolated CLI directory: {error}"));
    let protected = worldstream_runtime::prepare_data_directory(&directory.path().join("working"))
        .unwrap_or_else(|error| unreachable!("protected CLI directory: {error}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .args(arguments)
        .current_dir(&protected)
        .stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    let output = command
        .output()
        .unwrap_or_else(|error| unreachable!("operator CLI process: {error}"));
    assert_eq!(
        std::fs::read_dir(&protected)
            .unwrap_or_else(|error| unreachable!("isolated directory: {error}"))
            .count(),
        0,
        "contract-only dispatch must not create files"
    );
    output
}

#[test]
fn server_status_fails_closed_with_machine_readable_unavailable_output() {
    assert_eq!(
        control(&["server", "status", "--help"]).status.code(),
        Some(0)
    );
    let output = control(&["server", "status", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| unreachable!("JSON stdout: {error}"));
    assert_eq!(report["schema"], "worldstream/operator-command/v1");
    assert_eq!(report["command"], "server status");
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["code"], "controller_unavailable");
    assert!(String::from_utf8_lossy(&output.stderr).contains("controller"));
    assert_eq!(
        control(&["--config", "missing.toml", "server", "status", "--json"])
            .status
            .code(),
        Some(3)
    );
}

#[test]
fn managed_server_commands_accept_bounded_explicit_control_options() {
    for leaf in [
        "start",
        "stop",
        "restart",
        "controller-stop",
        "rotate-control-credential",
        "logs",
    ] {
        assert_eq!(control(&["server", leaf, "--help"]).status.code(), Some(0));
        // Rotation is local-only and does not accept Controller transport options.
        if leaf == "rotate-control-credential" {
            continue;
        }
        let output = control(&[
            "server",
            leaf,
            "--controller",
            "127.0.0.1:9420",
            "--state-dir",
            "local-state",
            "--timeout-seconds",
            "5",
            "--json",
        ]);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{leaf}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn server_start_accepts_an_explicit_local_participant_origin() {
    let output = control(&[
        "server",
        "start",
        "--participant-console-origin",
        "http://127.0.0.1:15173",
        "--state-dir",
        "missing-installation",
        "--controller",
        "127.0.0.1:19420",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(3));
    for origin in [
        "http://example.com:15173",
        "http://127.0.0.1:15173/path",
        "http://127.0.0.1:15173/",
        "http://127.0.0.1:5174",
        "http://127.0.0.1:80",
        "http://127.0.0.1:015173",
    ] {
        let output = control(&[
            "server",
            "start",
            "--participant-console-origin",
            origin,
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(origin));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(origin));
    }
}

#[test]
fn init_requires_exact_reviewed_import_approval_without_implicit_trust() {
    assert_eq!(control(&["init", "--help"]).status.code(), Some(0));
    let digest = format!("blake3:{}", "a".repeat(64));
    for arguments in [
        vec!["init", "--json"],
        vec![
            "init",
            "--preview",
            "--runner-template",
            "runner.json",
            "--provider-declaration",
            "provider.json",
            "--client-declaration",
            "clients.json",
            "--agent-profile",
            "profile.json",
            "--json",
        ],
        vec![
            "init",
            "--runner-template",
            "runner.json",
            "--approve-imports",
            &digest,
            "--json",
        ],
    ] {
        // The base initializer is covered by cli_initialization.rs.
        if arguments == ["init", "--json"] {
            continue;
        }
        let output = control(&arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| unreachable!("JSON stdout: {error}"));
        assert_eq!(report["code"], "initialization_required");
    }
    for arguments in [
        vec!["init", "--runner-template", "runner.json"],
        vec!["init", "--approve-imports", &digest],
        vec![
            "init",
            "--preview",
            "--runner-template",
            "runner.json",
            "--approve-imports",
            &digest,
        ],
        vec![
            "init",
            "--runner-template",
            "runner.json",
            "--approve-imports",
            "true",
        ],
    ] {
        assert_eq!(control(&arguments).status.code(), Some(2), "{arguments:?}");
    }
    let mut too_many = vec!["init", "--preview"];
    for _ in 0..17 {
        too_many.extend(["--agent-profile", "profile.json"]);
    }
    assert_eq!(control(&too_many).status.code(), Some(2));
}

#[test]
fn room_runner_client_and_pack_leaves_freeze_public_selectors() {
    for arguments in [
        vec!["pack", "list"],
        vec![
            "room",
            "example",
            "--pack",
            "heist@0.2.0",
            "--output",
            "setup.json",
        ],
        vec!["room", "validate", "--file", "setup.json"],
        vec![
            "room",
            "create",
            "--file",
            "setup.json",
            "--acknowledge-start",
        ],
        vec!["room", "list"],
        vec!["room", "inspect", "room-1"],
        vec!["room", "launch", "room-1"],
        vec!["room", "setup", "status"],
        vec!["room", "setup", "status", "operation-1"],
        vec!["room", "setup", "resume", "operation-1"],
        vec!["runner", "list"],
        vec!["runner", "list", "--operation", "operation-1"],
        vec![
            "runner",
            "inspect",
            "--operation",
            "operation-1",
            "--seat",
            "agent-1",
        ],
        vec![
            "runner",
            "start",
            "--operation",
            "operation-1",
            "--seat",
            "agent-1",
        ],
        vec![
            "runner",
            "stop",
            "--operation",
            "operation-1",
            "--seat",
            "agent-1",
        ],
        vec![
            "runner",
            "export-credentials",
            "--operation",
            "operation-1",
            "--seat",
            "agent-1",
            "--output",
            "credentials.json",
        ],
        vec![
            "client",
            "open",
            "--operation",
            "operation-1",
            "--seat",
            "human-1",
            "--binding",
            "local-client",
        ],
        vec![
            "client",
            "export-credentials",
            "--operation",
            "operation-1",
            "--seat",
            "human-1",
            "--output",
            "credentials.json",
        ],
    ] {
        let mut json_arguments = arguments.clone();
        json_arguments.push("--json");
        let output = control(&json_arguments);
        let (exit, code) = empty_installation_outcome(&arguments);
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| unreachable!("JSON stdout: {error}"));
        assert_eq!(report["code"], code);
        let mut help_arguments = arguments;
        help_arguments.push("--help");
        assert_eq!(control(&help_arguments).status.code(), Some(0));
    }
}

fn empty_installation_outcome(arguments: &[&str]) -> (i32, &'static str) {
    match arguments.get(..2) {
        Some(["room", "validate" | "create"]) => (1, "operation_rejected"),
        Some(
            ["room", "example" | "list" | "inspect" | "setup" | "launch"]
            | ["runner" | "client", _],
        ) => (3, "controller_unavailable"),
        Some(["pack", "list"]) => (3, "pack_inventory_unavailable"),
        _ => (3, "not_implemented"),
    }
}

#[test]
fn operator_argument_errors_do_not_echo_input() {
    let canary = "DO_NOT_ECHO_PROVIDER_SECRET";
    for arguments in [
        vec!["server", "status", "--unknown", canary, "--json"],
        vec!["--bind", canary, "server", "status", "--json"],
        vec!["server", "status", "--controller", canary, "--json"],
        vec![
            "init",
            "--state-dir",
            "DO_NOT_ECHO_PROVIDER_SECRET\n",
            "--json",
        ],
        vec!["room", "inspect", "DO_NOT_ECHO_PROVIDER_SECRET!", "--json"],
        vec![
            "room",
            "setup",
            "resume",
            "operation-1",
            "--file",
            canary,
            "--json",
        ],
    ] {
        let output = control(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(canary));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(canary));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| unreachable!("JSON stdout: {error}"));
        assert_eq!(report["code"], "invalid_arguments");
    }
}

#[test]
fn server_arguments_require_bounded_loopback_control() {
    for arguments in [
        vec!["server", "status", "--controller", "192.0.2.1:9420"],
        vec!["server", "status", "--controller", "localhost:9420"],
        vec!["server", "status", "--controller", "127.0.0.1:0"],
        vec!["server", "status", "--timeout-seconds", "0"],
        vec!["server", "status", "--timeout-seconds", "301"],
        vec!["server", "logs", "--tail", "0"],
        vec!["server", "logs", "--tail", "1001"],
    ] {
        assert_eq!(control(&arguments).status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn setup_arguments_require_exact_pack_and_explicit_participant_inputs() {
    for arguments in [
        vec![
            "room",
            "example",
            "--pack",
            "heist@0.2.0",
            "--output",
            "setup.json",
            "--interactive",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@stable",
            "--output",
            "setup.json",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@canary",
            "--output",
            "setup.json",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@01.2.0",
            "--output",
            "setup.json",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@latest",
            "--output",
            "setup.json",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@^0.2",
            "--output",
            "setup.json",
        ],
        vec![
            "room",
            "example",
            "--pack",
            "heist@0.2.0",
            "--output",
            "setup.json",
            "--interactive",
            "--json",
        ],
        vec!["room", "setup", "resume"],
        vec!["runner", "start", "--operation", "operation-1"],
        vec![
            "client",
            "export-credentials",
            "--operation",
            "operation-1",
            "--seat",
            "human-1",
        ],
    ] {
        assert_eq!(control(&arguments).status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn legacy_option_values_are_not_mistaken_for_operator_commands() {
    // A legacy value which resembles a new family is not an operator command.
    for arguments in [
        vec!["--config", "room", "health", "--bad"],
        vec!["--config=room", "health", "--bad"],
        vec!["pack", "--config=room", "export", "--bad"],
        vec!["pack", "export", "--output", "server", "--bad"],
    ] {
        let output = control(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
}

#[test]
fn operator_errors_skip_global_option_values_at_both_command_levels() {
    for arguments in [
        vec!["--config", "room", "server", "status", "--bad", "--json"],
        vec!["--config=room", "server", "status", "--bad", "--json"],
        vec!["pack", "--config", "server", "list", "--bad", "--json"],
        vec!["pack", "--config=server", "list", "--bad", "--json"],
    ] {
        let output = control(&arguments);
        assert_eq!(output.status.code(), Some(2));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| unreachable!("JSON stdout: {error}"));
        assert_eq!(report["code"], "invalid_arguments");
    }
}
