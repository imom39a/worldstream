#![cfg(all(feature = "cli-operator-preview", unix))]

use std::{
    fs,
    io::{Read as _, Write as _},
    net::TcpListener,
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::{
    activity_packs::HttpDaemonActivityPackSource,
    agent_profiles::AgentProfileStoreV1,
    assignment_mcp::AssignmentMcpLaunchRegistryV1,
    managed_agent_host::ManagedAgentHostOperationsV1,
    managed_lifecycle::{LifecycleError, ManagedLifecycle, RuntimeControl, RuntimeObservation},
    room_creation::{HttpDaemonRoomCreatorV1, RoomCreationSupervisorV1},
    room_drafts::{ExactActivityPackDraftValidatorV1, RoomDraftStoreV1},
    runner_templates::{
        RunnerInstanceHealthV1, RunnerInstanceStateV1, RunnerSupervisorV1, RunnerTemplateRegistryV1,
    },
    secrets::{FileSecretVaultV1, SecretKindV1},
    task_setup::{
        FileAssignedMembershipSourceV1, HttpDaemonTaskSetupProvisionerV1, TaskSetupSupervisorV1,
    },
};

#[derive(Clone)]
struct RuntimeFixture(Arc<AtomicBool>);

impl RuntimeControl for RuntimeFixture {
    fn observe(&self) -> RuntimeObservation {
        if self.0.load(Ordering::SeqCst) {
            RuntimeObservation::Ready
        } else {
            RuntimeObservation::Stopped
        }
    }
    fn start(&self) -> Result<(), LifecycleError> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn stop(&self) -> Result<(), LifecycleError> {
        self.0.store(false, Ordering::SeqCst);
        Ok(())
    }
}

struct OwnedFixture {
    directory: Option<tempfile::TempDir>,
    runners: RunnerSupervisorV1,
    health_done: Arc<AtomicBool>,
    health: Option<thread::JoinHandle<()>>,
    marker: PathBuf,
    release: PathBuf,
}

impl Drop for OwnedFixture {
    fn drop(&mut self) {
        let _ = fs::write(&self.release, b"");
        let stopped = self.runners.stop("bound-local").is_some_and(|status| {
            status.instances.iter().any(|row| {
                row.instance_id == "bound-local"
                    && row.state == RunnerInstanceStateV1::Stopped
                    && !row.managed_by_supervisor
            })
        });
        self.health_done.store(true, Ordering::SeqCst);
        if let Some(health) = self.health.take() {
            let _ = health.join();
        }
        if !stopped || fs::read(&self.marker).is_ok_and(|bytes| bytes == b"running") {
            if let Some(directory) = self.directory.take() {
                eprintln!(
                    "Owned restart fixture retained for cleanup: {}",
                    directory.keep().display()
                );
            }
        }
    }
}

fn await_healthy(runners: &RunnerSupervisorV1) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if runners.statuses().instances.iter().any(|row| {
            row.instance_id == "bound-local" && row.health == RunnerInstanceHealthV1::Healthy
        }) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err("owned Runner did not become healthy".into())
}

#[test]
fn mvp_restart_rejects_bound_runner_before_stopping_it() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state = prepare_data_directory(&directory.path().join("state"))?;
    let marker = state.join("running");
    let release = state.join("release");
    let launches = state.join("launches");
    let executable = state.join("runner.sh");
    let script = b"#!/bin/sh\ntrap 'printf stopped > \"$FIXTURE_MARKER\"; exit 0' INT TERM\nprintf running > \"$FIXTURE_MARKER\"\nprintf 'launch\\n' >> \"$FIXTURE_LAUNCHES\"\nwhile [ ! -f \"$FIXTURE_RELEASE\" ]; do /bin/sleep 0.02; done\nprintf stopped > \"$FIXTURE_MARKER\"\n";
    fs::write(&executable, script)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let health_done = Arc::new(AtomicBool::new(false));
    let done = health_done.clone();
    let health_marker = marker.clone();
    let health = thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                    let mut buffer = [0_u8; 4096];
                    let _ = stream.read(&mut buffer);
                    let running = fs::read(&health_marker).is_ok_and(|bytes| bytes == b"running");
                    let body = if running {
                        r#"{"status":"ok","instance_id":"bound-local","capacity_in_use":0}"#
                    } else {
                        "{}"
                    };
                    let status = if running { 200 } else { 503 };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break,
            }
        }
    });
    let declarations = prepare_data_directory(&state.join("declarations"))?;
    fs::write(
        declarations.join("runner.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema":"worldstream/runner-template/v1", "template_id":"bound-runner", "revision":"1", "display_name":"Bound fixture",
            "executable":{"path":executable,"blake3":blake3::hash(script).to_hex().to_string()},
            "compatibility":[{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}],
            "capacity":{"maximum_concurrent_invocations":1},
            "health":{"path":"/health","timeout_ms":200,"stale_after_ms":1000},
            "non_secret_environment":{"FIXTURE_MARKER":marker,"FIXTURE_LAUNCHES":launches,"FIXTURE_RELEASE":release},"secret_environment":[],
            "instances":[{"instance_id":"bound-local","health_address":endpoint}]
        }))?,
    )?;
    let registry = RunnerTemplateRegistryV1::open(&state.join("templates"), &declarations)?;
    let vault = FileSecretVaultV1::open(&state.join("secrets"))?;
    let runners = RunnerSupervisorV1::open(
        registry,
        &state.join("runtime"),
        vault.clone(),
        Duration::from_secs(3),
    )?;
    let fixture = OwnedFixture {
        directory: Some(directory),
        runners: runners.clone(),
        health_done,
        health: Some(health),
        marker,
        release,
    };
    runners.start("bound-local").ok_or("missing Runner")?;
    await_healthy(&runners)?;
    let authority = vault.store(SecretKindV1::RunnerAuthority, &[7_u8; 32])?;
    runners.bind_task_runner_authority("bound-local", "01ARZ3NDEKTSV4RRFFQ69G5FAW", &authority)?;
    await_healthy(&runners)?;
    let profiles = AgentProfileStoreV1::open(&state.join("profiles"), vault.clone())?;
    let packs = HttpDaemonActivityPackSource::new(
        endpoint,
        Duration::from_millis(200),
        vault.clone(),
        None,
    );
    let drafts = RoomDraftStoreV1::open(
        &state.join("drafts"),
        ExactActivityPackDraftValidatorV1::new(packs),
    )?;
    let creation = RoomCreationSupervisorV1::open(
        &state.join("creation"),
        drafts,
        HttpDaemonRoomCreatorV1::new(endpoint, Duration::from_millis(200), vault.clone(), None),
    )?;
    let setup = TaskSetupSupervisorV1::open(
        &state.join("task-setups"),
        creation,
        vault.clone(),
        HttpDaemonTaskSetupProvisionerV1::new(
            endpoint,
            Duration::from_millis(200),
            vault.clone(),
            None,
        ),
    )?
    .with_agent_profiles(profiles.clone());
    assert!(setup.statuses()?.is_empty());
    let source = FileAssignedMembershipSourceV1::open(
        &state.join("task-setups"),
        profiles.clone(),
        vault.clone(),
    )?;
    let mcp = AssignmentMcpLaunchRegistryV1::open(
        &state.join("mcp"),
        &state.join("progress"),
        source,
        endpoint,
        Duration::from_millis(200),
    )?;
    let hosts = ManagedAgentHostOperationsV1::open_production(
        &state.join("hosts"),
        &state,
        &executable,
        profiles,
        runners.clone(),
        mcp,
        vault,
        setup,
    )?;
    let lifecycle = ManagedLifecycle::open(
        &state,
        RuntimeFixture(Arc::new(AtomicBool::new(true))),
        runners.clone(),
        hosts,
    )?;
    let before = fs::read(&launches)?;
    assert_eq!(
        lifecycle.restart().unwrap_err(),
        LifecycleError::BoundRunnerRestartUnsupported
    );
    let status = lifecycle.status()?;
    assert!(
        status.operation.is_none(),
        "rejection must not begin a restart"
    );
    assert_eq!(status.runtime, RuntimeObservation::Ready);
    assert!(fs::read(&launches)? == before);
    assert!(
        runners
            .statuses()
            .instances
            .iter()
            .any(|row| row.state == RunnerInstanceStateV1::Running)
    );
    drop(fixture);
    Ok(())
}
