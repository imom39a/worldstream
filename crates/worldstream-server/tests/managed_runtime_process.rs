use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    local_initialization::{InitializationRequest, initialize_local},
    process_ownership::{ProcessOwnership, ProcessPhase, ProcessRole},
    verified_control::VerifiedConnection,
};

// Only test-owned children can be terminated. A retained PID is never used.
struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn managed_runtime_holds_ownership_while_serving_and_releases_it_after_shutdown()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let port = TcpListener::bind("127.0.0.1:0")?;
    let bind = port.local_addr()?;
    let installation = initialize_local(&InitializationRequest {
        config: None,
        overrides: CliOverrides {
            bind: Some(bind),
            ..CliOverrides::default()
        },
        state_dir: temporary.path().join(".worldstream/studio"),
        working_directory: temporary.path().to_path_buf(),
        environment: Vec::new(),
        preview: false,
    })?;
    let ownership = ProcessOwnership::open(&installation.state_dir)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    drop(port);
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamd"));
    // This test must never inherit a developer's Runtime path/config overrides.
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|key| key.starts_with("WORLDSTREAM__"))
        {
            command.env_remove(key);
        }
    }
    let mut process = OwnedChild(
        command
            .arg("--config")
            .arg(&installation.config_path)
            .arg("--managed-state-dir")
            .arg(&installation.state_dir)
            .arg("--managed-generation")
            .arg(launch.generation())
            .env_remove("WORLDSTREAM_CONFIG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    let connection = loop {
        if let Ok(connection) =
            VerifiedConnection::connect(&ownership, ProcessRole::Runtime, Duration::from_secs(1))
        {
            break connection;
        }
        assert!(
            process.0.try_wait()?.is_none(),
            "managed Runtime exited before proving readiness"
        );
        assert!(
            Instant::now() < deadline,
            "managed Runtime startup did not complete"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(ownership.is_leased(ProcessRole::Runtime)?);
    assert!(ownership.reserve(ProcessRole::Runtime).is_err());
    let ready = connection.request_runtime("GET", "/api/v1/control/status", b"")?;
    assert_eq!(ready.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&ready.body)?;
    assert_eq!(body["schema"], "worldstream/managed-runtime-status/v1");
    assert_eq!(body["ready"], true);

    let access = ControlAccess::open(&installation.state_dir)?;
    let wrong_scope =
        VerifiedConnection::connect(&ownership, ProcessRole::Runtime, Duration::from_secs(3))?
            .request("GET", "/api/v1/control/status", b"", || {
                access.authorization_header()
            })?;
    assert_eq!(wrong_scope.status, 401);

    let stopped =
        VerifiedConnection::connect(&ownership, ProcessRole::Runtime, Duration::from_secs(3))?
            .request_runtime("POST", "/api/v1/control/stop", b"{}")?;
    assert_eq!(stopped.status, 202);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(exit) = process.0.try_wait()? {
            assert!(exit.success(), "managed Runtime shutdown failed");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "managed Runtime shutdown did not complete"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    assert_eq!(
        ownership
            .snapshot(ProcessRole::Runtime)?
            .ok_or("missing terminal record")?
            .phase,
        ProcessPhase::Stopped
    );
    Ok(())
}
