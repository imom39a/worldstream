//! A timed-out owned Runner must not become a stopped record after reopen.
#![cfg(all(feature = "cli-operator-preview", unix))]

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::{
    runner_templates::{RunnerInstanceStateV1, RunnerSupervisorV1, RunnerTemplateRegistryV1},
    secrets::FileSecretVaultV1,
};

struct OwnedRunner {
    directory: Option<tempfile::TempDir>,
    state: PathBuf,
    supervisor: RunnerSupervisorV1,
}

impl Drop for OwnedRunner {
    fn drop(&mut self) {
        let _ = fs::write(self.state.join("runner-release"), b"");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self
                .supervisor
                .statuses()
                .instances
                .iter()
                .all(|instance| !instance.managed_by_supervisor)
            {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        if let Some(directory) = self.directory.take() {
            eprintln!(
                "Owned Runner fixture retained for exact-owner cleanup: {}",
                directory.keep().display()
            );
        }
    }
}

#[test]
fn failed_owned_runner_stop_retains_running_intent_across_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state = prepare_data_directory(&directory.path().join("state"))?;
    let executable = state.join("owned-runner.sh");
    let script = b"#!/bin/sh\ntrap '' INT\nrunner_fixture_dir=\"${0%/*}\"\n: > \"$runner_fixture_dir/runner-ready\"\nwhile [ ! -f \"$runner_fixture_dir/runner-release\" ]; do /bin/sleep 0.02; done\n";
    fs::write(&executable, script)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let declarations = prepare_data_directory(&state.join("declarations"))?;
    let health = std::net::TcpListener::bind("127.0.0.1:0")?;
    fs::write(
        declarations.join("runner.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema":"worldstream/runner-template/v1", "template_id":"owned-runner", "revision":"1", "display_name":"Owned runner",
            "executable":{"path":executable,"blake3":blake3::hash(script).to_hex().to_string()},
            "compatibility":[{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}],
            "capacity":{"maximum_concurrent_invocations":1},"health":{"path":"/health","timeout_ms":100,"stale_after_ms":1000},
            "non_secret_environment":{},"secret_environment":[],
            "instances":[{"instance_id":"owned-local","health_address":health.local_addr()?.to_string()}]
        }))?,
    )?;
    drop(health);
    let registry = RunnerTemplateRegistryV1::open(&state.join("templates"), &declarations)?;
    let vault = FileSecretVaultV1::open(&state.join("secrets"))?;
    let supervisor = RunnerSupervisorV1::open(
        registry,
        &state.join("runtime"),
        vault.clone(),
        Duration::from_millis(100),
    )?;
    let fixture = OwnedRunner {
        directory: Some(directory),
        state: state.clone(),
        supervisor,
    };
    let started = fixture
        .supervisor
        .start("owned-local")
        .ok_or("missing Runner")?;
    assert_eq!(started.instances[0].state, RunnerInstanceStateV1::Running);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.join("runner-ready").exists() {
        assert!(
            Instant::now() < deadline,
            "owned Runner did not install its stop behavior"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let stopped = fixture
        .supervisor
        .stop("owned-local")
        .ok_or("missing Runner")?;
    let reopened = RunnerSupervisorV1::open(
        RunnerTemplateRegistryV1::open_installed(&state.join("templates"))?,
        &state.join("runtime"),
        vault,
        Duration::from_millis(100),
    )?;
    let retained = reopened.statuses();
    drop(fixture); // Exact owner releases and reaps its child before any failing assertion.
    assert_eq!(stopped.instances[0].state, RunnerInstanceStateV1::Failed);
    assert!(stopped.instances[0].managed_by_supervisor);
    assert_eq!(
        retained.instances[0].state,
        RunnerInstanceStateV1::Failed,
        "timed-out stop was incorrectly retained as stopped"
    );
    assert!(!retained.instances[0].managed_by_supervisor);
    Ok(())
}
