//! Retained participant origin selection reaches only the owned launch boundary.
#![cfg(unix)]

use std::{fs, net::TcpListener, os::unix::fs::PermissionsExt as _, time::Duration};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_studio_supervisor::{
    local_initialization::{InitializationRequest, initialize_local},
    operator_connection::{ControllerExecutables, OperatorConnection, OperatorConnectionError},
};

#[test]
fn participant_origin_is_retained_and_conflicts_do_not_launch()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path();
    let state = directory.join(".worldstream/studio");
    let config = directory.join(".worldstream/worldstream.toml");
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = listener.local_addr()?;
    initialize_local(&InitializationRequest {
        config: Some(config.clone()),
        overrides: CliOverrides::default(),
        state_dir: state.clone(),
        working_directory: directory.to_owned(),
        environment: Vec::new(),
        preview: false,
    })?;
    drop(listener);
    // Capture the real OS launch arguments and exit immediately. This fixture
    // never binds a listener, starts a Runtime, or claims controller readiness.
    let launcher = directory.join("capture-controller.sh");
    fs::write(
        &launcher,
        b"#!/bin/sh\nfor argument in \"$@\"; do printf '%s\\n' \"$argument\"; done >> \"${0%/*}/arguments\"\n",
    )?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;
    let executables = ControllerExecutables {
        controller: launcher.clone(),
        runtime: launcher.clone(),
        assignment_mcp: launcher,
    };
    let loader = ConfigLoader::with_environment(Some(config.clone()), CliOverrides::default(), []);
    let start = |origin| {
        OperatorConnection::open(&state, endpoint, Duration::from_secs(1))?
            .with_participant_console_origin(origin)?
            .ensure_started(&loader, &config, directory, &executables)
    };
    assert!(matches!(
        start(Some("http://127.0.0.1:15173")),
        Err(OperatorConnectionError::Incomplete)
    ));
    let arguments = directory.join("arguments");
    let first = fs::read_to_string(&arguments)?;
    assert!(first.contains("--participant-console-origin\nhttp://127.0.0.1:15173\n"));
    let diagnostic_log = state.join("hosted-session-diagnostics.ndjson");
    assert!(first.contains(&format!(
        "--hosted-session-diagnostic-log\n{}\n",
        diagnostic_log.display()
    )));
    let diagnostic_metadata = fs::metadata(&diagnostic_log)?;
    assert!(diagnostic_metadata.is_file());
    assert_eq!(diagnostic_metadata.permissions().mode() & 0o777, 0o600);
    assert!(matches!(
        start(None),
        Err(OperatorConnectionError::Incomplete)
    ));
    let repeated = fs::read_to_string(&arguments)?;
    assert_eq!(repeated.matches("http://127.0.0.1:15173\n").count(), 2);
    assert!(matches!(
        start(Some("http://127.0.0.1:15174")),
        Err(OperatorConnectionError::Invalid)
    ));
    assert_eq!(fs::read_to_string(&arguments)?, repeated);
    // Backward-compatible public configuration fixture: older retained records
    // have no participant origin and must preserve the original default.
    let selection_path = state.join("managed-controller-config.v1.json");
    let mut old_record: serde_json::Value = serde_json::from_slice(&fs::read(&selection_path)?)?;
    old_record
        .as_object_mut()
        .ok_or("configuration must be an object")?
        .remove("participant_console_origin");
    let old_bytes = serde_json::to_vec(&old_record)?;
    fs::write(&selection_path, &old_bytes)?;
    assert!(matches!(
        start(None),
        Err(OperatorConnectionError::Incomplete)
    ));
    let legacy_arguments = fs::read_to_string(&arguments)?;
    assert!(legacy_arguments.contains("--participant-console-origin\nhttp://127.0.0.1:5173\n"));
    assert_eq!(fs::read(&selection_path)?, old_bytes);
    Ok(())
}
