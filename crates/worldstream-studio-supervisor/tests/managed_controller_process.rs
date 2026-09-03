//! Managed Controller proof and clean shutdown through the actual process.

use std::{
    env,
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    local_initialization::{InitializationRequest, initialize_local},
    process_ownership::{ProcessOwnership, ProcessPhase, ProcessRole},
    verified_control::VerifiedConnection,
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn managed_controller_proves_ownership_and_stops_without_starting_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state = directory.path().join(".worldstream/studio");
    let config = directory.path().join(".worldstream/worldstream.toml");
    let runtime_address = TcpListener::bind("127.0.0.1:0")?;
    let controller_address = TcpListener::bind("127.0.0.1:0")?;
    let address = controller_address.local_addr()?;
    initialize_local(&InitializationRequest {
        config: Some(config.clone()),
        overrides: CliOverrides {
            bind: Some(runtime_address.local_addr()?),
            ..CliOverrides::default()
        },
        state_dir: state.clone(),
        working_directory: directory.path().to_owned(),
        environment: Vec::new(),
        preview: false,
    })?;
    let ownership = ProcessOwnership::open(&state)?;
    let claim = ownership.reserve(ProcessRole::Controller)?;
    let control = ControlAccess::open(&state)?;
    drop(controller_address);
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstream-studio-supervisor"));
    command
        .current_dir(directory.path())
        .arg("--state-dir")
        .arg(&state)
        .arg("--daemon-config")
        .arg(&config)
        .arg("--daemon")
        .arg(runtime_address.local_addr()?.to_string())
        .arg("--bind")
        .arg(address.to_string())
        .arg("--assignment-mcp-executable")
        .arg(env!("CARGO_BIN_EXE_worldstream-assignment-mcp"))
        .arg("--managed-generation")
        .arg(claim.generation())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    let mut child = OwnedChild(command.spawn()?);
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        assert!(
            child.0.try_wait()?.is_none(),
            "controller exited before ownership proof"
        );
        if VerifiedConnection::connect(
            &ownership,
            ProcessRole::Controller,
            Duration::from_millis(250),
        )
        .is_ok()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "controller did not publish ownership proof"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(ownership.is_leased(ProcessRole::Controller)?);
    assert!(ownership.reserve(ProcessRole::Controller).is_err());
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    let connection =
        VerifiedConnection::connect(&ownership, ProcessRole::Controller, Duration::from_secs(5))?;
    let response = connection.request("POST", "/api/v1/control/controller-stop", b"", || {
        control.authorization_header().map_err(|_| ())
    })?;
    assert_eq!(response.status, 202);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.0.try_wait()? {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "controller did not finish shutdown"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(!ownership.is_leased(ProcessRole::Controller)?);
    assert_eq!(
        ownership
            .snapshot(ProcessRole::Controller)?
            .ok_or("missing terminal record")?
            .phase,
        ProcessPhase::Stopped
    );
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    assert!(
        !directory
            .path()
            .join(".worldstream/data/worldstream.sqlite3")
            .exists()
    );
    Ok(())
}
