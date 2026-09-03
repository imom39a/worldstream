//! Ambiguous approved client selection is explicit and never opens a browser.
#![cfg(feature = "cli-operator-preview")]

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    env,
    path::Path,
    process::{Command, Output, Stdio},
};
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    managed_controller::managed_controller_router,
    managed_http::AcceptedLocalSocket,
    process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
};

fn selection_response() -> Value {
    json!({"state":"selection_required","version":"activity_client_handoff.v1","candidates":[
        {"candidate_id":"heist-browser-a","deployment_id":"browser-a","client_id":"heist-client",
         "release_digest":format!("blake3:{}", "1".repeat(64)),"surface_id":"participant","trust_level":"externally_trusted"},
        {"candidate_id":"heist-browser-b","deployment_id":"browser-b","client_id":"heist-client",
         "release_digest":format!("blake3:{}", "2".repeat(64)),"surface_id":"participant","trust_level":"externally_trusted"}
    ]})
}

fn invoke(root: &Path, state: &Path, endpoint: std::net::SocketAddr) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .args([
            "client",
            "open",
            "--operation",
            "op-test",
            "--seat",
            "navigator",
            "--json",
            "--state-dir",
        ])
        .arg(state)
        .arg("--controller")
        .arg(endpoint.to_string());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_open_reports_exact_approved_choices_without_selecting_or_opening_one()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state = worldstream_runtime::prepare_data_directory(&directory.path().join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let claim = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, claim.generation())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let proof = lease.publish_endpoint(endpoint)?;
    let handlers = Router::new().route(
        "/api/v1/room-setup-operations/op-test/seats/navigator/client-handoff",
        post(|Json(body): Json<Value>| async move {
            assert!(body["binding"].is_null());
            Json(selection_response())
        }),
    );
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let service = managed_controller_router(handlers, control, proof, shutdown);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            service.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let root = directory.path().to_path_buf();
    let output = tokio::task::spawn_blocking(move || invoke(&root, &state, endpoint)).await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    let output = output??;
    assert_eq!(
        output.status.code(),
        Some(1),
        "an exact client selection is required"
    );
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["code"], "client_selection_required");
    assert_eq!(
        report["client_candidates"][0]["candidate_id"],
        "heist-browser-a"
    );
    assert_eq!(
        report["client_candidates"][1]["candidate_id"],
        "heist-browser-b"
    );
    assert!(
        report["next_action"]
            .as_str()
            .is_some_and(|text| text.contains("--binding"))
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("client_url"));
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}
