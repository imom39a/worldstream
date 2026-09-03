//! Separate CLI invocations operate one detached installation.
#![cfg(feature = "cli-operator-preview")]

use std::{
    env,
    path::Path,
    process::{Command, Output, Stdio},
};
use worldstream_studio_supervisor::process_ownership::{ProcessOwnership, ProcessRole};

fn cli(directory: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

struct Installation {
    root: Option<tempfile::TempDir>,
    controller: String,
}

impl Installation {
    fn directory(&self) -> Result<&Path, &'static str> {
        self.root
            .as_ref()
            .map(tempfile::TempDir::path)
            .ok_or("missing owned installation")
    }

    fn invoke(&self, operation: &str) -> Result<Output, Box<dyn std::error::Error>> {
        Ok(cli(
            self.directory()?,
            &[
                "server",
                operation,
                "--controller",
                &self.controller,
                "--timeout-seconds",
                "180",
                "--json",
            ],
        )?)
    }
}

fn initialized_installation() -> Result<Installation, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let runtime_port = std::net::TcpListener::bind("127.0.0.1:0")?;
    let controller_port = std::net::TcpListener::bind("127.0.0.1:0")?;
    let controller = controller_port.local_addr()?.to_string();
    assert!(
        cli(
            root.path(),
            &[
                "init",
                "--bind",
                &runtime_port.local_addr()?.to_string(),
                "--json"
            ]
        )?
        .status
        .success()
    );
    Ok(Installation {
        root: Some(root),
        controller,
    })
}

impl Drop for Installation {
    fn drop(&mut self) {
        let Some(root) = self.root.as_ref() else {
            return;
        };
        for operation in ["stop", "controller-stop"] {
            let _ = cli(
                root.path(),
                &[
                    "server",
                    operation,
                    "--controller",
                    &self.controller,
                    "--timeout-seconds",
                    "5",
                    "--json",
                ],
            );
        }
        let released = ProcessOwnership::open(&root.path().join(".worldstream/studio")).is_ok_and(
            |ownership| {
                [ProcessRole::Controller, ProcessRole::Runtime]
                    .into_iter()
                    .all(|role| ownership.is_leased(role) == Ok(false))
            },
        );
        if !released && let Some(root) = self.root.take() {
            eprintln!(
                "Owned CLI installation retained for authenticated cleanup: {}",
                root.keep().display()
            );
        }
    }
}

#[test]
fn separate_cli_calls_start_recover_and_stop_one_managed_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let runtime_port = std::net::TcpListener::bind("127.0.0.1:0")?;
    let controller_port = std::net::TcpListener::bind("127.0.0.1:0")?;
    let controller = controller_port.local_addr()?.to_string();
    assert!(
        cli(
            root.path(),
            &[
                "init",
                "--bind",
                &runtime_port.local_addr()?.to_string(),
                "--json"
            ]
        )?
        .status
        .success()
    );
    let installation = Installation {
        root: Some(root),
        controller,
    };
    let directory = installation
        .root
        .as_ref()
        .ok_or("missing test installation")?
        .path();
    let invoke = |operation: &str| {
        cli(
            directory,
            &[
                "server",
                operation,
                "--controller",
                &installation.controller,
                "--timeout-seconds",
                "180",
                "--json",
            ],
        )
    };
    drop(runtime_port);
    drop(controller_port);
    assert_eq!(
        invoke("status")?.status.code(),
        Some(3),
        "a read must not start the Controller"
    );
    let started = invoke("start")?;
    assert_eq!(
        started.status.code(),
        Some(0),
        "managed start must complete"
    );
    let started: serde_json::Value = serde_json::from_slice(&started.stdout)?;
    assert_eq!(started["server"]["runtime"], "ready");
    let ownership = ProcessOwnership::open(&directory.join(".worldstream/studio"))?;
    let generation = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing Runtime generation")?
        .generation;
    assert_eq!(invoke("status")?.status.code(), Some(0));
    assert_eq!(invoke("controller-stop")?.status.code(), Some(0));
    assert!(ownership.is_leased(ProcessRole::Runtime)?);
    assert!(!ownership.is_leased(ProcessRole::Controller)?);
    assert_eq!(
        invoke("status")?.status.code(),
        Some(3),
        "status must not restart a stopped Controller"
    );
    assert_eq!(invoke("start")?.status.code(), Some(0));
    assert_eq!(
        ownership
            .snapshot(ProcessRole::Runtime)?
            .ok_or("missing recovered Runtime")?
            .generation,
        generation
    );
    assert_eq!(invoke("stop")?.status.code(), Some(0));
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    assert!(ownership.is_leased(ProcessRole::Controller)?);
    assert_eq!(invoke("controller-stop")?.status.code(), Some(0));
    Ok(())
}

#[test]
fn controller_recovery_uses_retained_launch_config_after_original_file_is_deleted()
-> Result<(), Box<dyn std::error::Error>> {
    let installation = initialized_installation()?;
    assert!(installation.invoke("start")?.status.success());
    let ownership = ProcessOwnership::open(&installation.directory()?.join(".worldstream/studio"))?;
    let runtime_generation = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing Runtime")?
        .generation;
    assert!(installation.invoke("controller-stop")?.status.success());
    let original = installation
        .directory()?
        .join(".worldstream/worldstream.toml");
    let retained_bytes = std::fs::read(&original)?;
    std::fs::remove_file(&original)?;
    let recovered = installation.invoke("start");
    // Restore only this test's source file before asserting, so even the expected
    // red path can clean up the owned Runtime through supported CLI control.
    std::fs::write(&original, retained_bytes)?;
    assert!(installation.invoke("start")?.status.success());
    let preserved = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing recovered Runtime")?
        .generation;
    assert!(installation.invoke("stop")?.status.success());
    assert!(installation.invoke("controller-stop")?.status.success());
    assert!(
        recovered?.status.success(),
        "retained normalized configuration must permit Controller recovery"
    );
    assert_eq!(preserved, runtime_generation);
    Ok(())
}

#[test]
fn concurrent_cli_starts_converge_on_one_ready_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let installation = initialized_installation()?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let results = std::thread::scope(|scope| {
        let mut calls = Vec::new();
        for _ in 0..4 {
            let barrier = barrier.clone();
            let directory = installation.directory()?.to_path_buf();
            let endpoint = installation.controller.clone();
            calls.push(scope.spawn(move || {
                barrier.wait();
                cli(
                    &directory,
                    &[
                        "server",
                        "start",
                        "--controller",
                        &endpoint,
                        "--timeout-seconds",
                        "180",
                        "--json",
                    ],
                )
            }));
        }
        calls
            .into_iter()
            .map(|call| {
                call.join()
                    .map_err(|_| "start worker panicked")?
                    .map_err(|_| "start worker failed")
            })
            .collect::<Result<Vec<_>, &'static str>>()
    })?;
    let status = installation.invoke("status")?;
    let ownership = ProcessOwnership::open(&installation.directory()?.join(".worldstream/studio"))?;
    let controller_held = ownership.is_leased(ProcessRole::Controller)?;
    let runtime_held = ownership.is_leased(ProcessRole::Runtime)?;
    // Cleanup is supported control only; all worker results are collected first.
    let _ = installation.invoke("stop");
    let _ = installation.invoke("controller-stop");
    assert!(
        results.iter().all(|output| output.status.success()),
        "concurrent starts must join the same launch rather than cancel one another"
    );
    assert!(status.status.success());
    assert!(controller_held && runtime_held);
    Ok(())
}

struct OwnedController(std::process::Child);

impl Drop for OwnedController {
    fn drop(&mut self) {
        // Only the exact child spawned by this test, never a retained PID.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn explicit_start_recovers_a_crashed_controller_without_restarting_the_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};
    use worldstream_studio_supervisor::operator_connection::OperatorConnection;

    let installation = initialized_installation()?;
    assert!(installation.invoke("start")?.status.success());
    let state = installation.directory()?.join(".worldstream/studio");
    let ownership = ProcessOwnership::open(&state)?;
    let runtime_generation = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing initial Runtime")?
        .generation;
    assert!(installation.invoke("controller-stop")?.status.success());

    // Retain an actual OS child handle so this test can exercise a crash without
    // interpreting a diagnostic PID as authority. The public start above already
    // retained the reviewed configuration and permanent startup lock.
    let binaries = Path::new(env!("CARGO_BIN_EXE_worldstreamctl"))
        .parent()
        .ok_or("missing binary directory")?;
    let generation = ownership.reserve(ProcessRole::Controller)?;
    let runtime_endpoint = ownership
        .snapshot(ProcessRole::Runtime)?
        .and_then(|snapshot| snapshot.endpoint)
        .ok_or("missing Runtime endpoint")?;
    let mut command = Command::new(binaries.join("worldstream-studio-supervisor"));
    command
        .arg("--bind")
        .arg(&installation.controller)
        .arg("--daemon")
        .arg(runtime_endpoint.to_string())
        .arg("--daemon-executable")
        .arg(binaries.join("worldstreamd"))
        .arg("--daemon-config")
        .arg(state.join("managed-runtime.toml"))
        .arg("--state-dir")
        .arg(&state)
        .arg("--assignment-mcp-executable")
        .arg(binaries.join("worldstream-assignment-mcp"))
        .arg("--managed-generation")
        .arg(generation.generation())
        .current_dir(&state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    let mut child = OwnedController(command.spawn()?);
    let connection = OperatorConnection::open(
        &state,
        installation.controller.parse()?,
        Duration::from_millis(250),
    )?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        assert!(
            child.0.try_wait()?.is_none(),
            "owned Controller exited early"
        );
        if connection
            .request("GET", "/api/v1/control/server/status", b"")
            .is_ok_and(|response| response.status == 200)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "owned Controller never became ready"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    child.0.kill()?;
    child.0.wait()?;
    assert!(!ownership.is_leased(ProcessRole::Controller)?);
    assert!(ownership.is_leased(ProcessRole::Runtime)?);
    let recovered = installation.invoke("start")?;
    if !recovered.status.success() {
        // Preserve test cleanup on the expected red path through exact owned
        // generation fencing. No production credential or process is involved.
        ownership.cancel_abandoned(ProcessRole::Controller, generation.generation())?;
        assert!(installation.invoke("start")?.status.success());
    }
    let retained_runtime = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing surviving Runtime")?;
    assert!(installation.invoke("stop")?.status.success());
    assert!(installation.invoke("controller-stop")?.status.success());
    assert!(
        recovered.status.success(),
        "explicit start must recover after Controller crash"
    );
    assert_eq!(retained_runtime.generation, runtime_generation);
    Ok(())
}
