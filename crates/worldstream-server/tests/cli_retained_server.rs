//! An unavailable Controller must not hide safe retained Runtime diagnostics.
#![cfg(feature = "cli-operator-preview")]

use std::{
    fs,
    io::Write as _,
    process::{Command, Stdio},
};
use worldstream_studio_supervisor::process_ownership::{ProcessOwnership, ProcessRole};

#[test]
fn status_reports_retained_evidence_without_claiming_live_controller_health()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = worldstream_runtime::prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    ownership.reserve(ProcessRole::Controller)?;
    // Filesystem failure is confined to this test-owned Controller record.
    fs::write(state.join("managed-controller.v1"), b"damaged")?;
    let runtime = ownership.reserve(ProcessRole::Runtime)?;
    let lease = ownership.claim(ProcessRole::Runtime, runtime.generation())?;
    let mut checkpoint =
        worldstream_runtime::create_owner_only_file(&state.join("managed-lifecycle.v1.json"))?;
    checkpoint.write_all(br#"{"schema":"worldstream/managed-lifecycle-operation/v1","operation_id":7,"action":"restart","stage":"runtime_restart","captured_running":[],"restore_remaining":[]}"#)?;
    drop(checkpoint);
    let output = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"))
        .args(["server", "status", "--state-dir"])
        .arg(&state)
        .args(["--json"])
        .stdin(Stdio::null())
        .output()?;
    drop(lease);
    assert_eq!(output.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["code"], "controller_unavailable");
    assert!(report.get("server").is_none());
    assert_eq!(report["retained_server"]["evidence"], "retained_only");
    assert_eq!(
        report["retained_server"]["controller"]["availability"],
        "unavailable"
    );
    assert_eq!(report["retained_server"]["runtime"]["lease"], "held");
    assert_eq!(
        report["retained_server"]["operation"]["checkpoint"]["stage"],
        "runtime_restart"
    );
    assert_eq!(fs::read(state.join("managed-controller.v1"))?, b"damaged");
    Ok(())
}
