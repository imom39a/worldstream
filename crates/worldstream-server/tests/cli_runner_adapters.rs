//! External Runners are inspectable but never managed process stop targets.
#![cfg(feature = "cli-operator-preview")]

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
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

fn external_runner() -> Value {
    json!({"version":"room_runner.v1","operation":"op-test","seat":"insider",
        "room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","runner_id":"01ARZ3NDEKTSV4RRFFQ69G5FAW",
        "execution":{"kind":"external"}})
}

fn invoke(
    root: &Path,
    state: &Path,
    endpoint: std::net::SocketAddr,
    action: &str,
) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .args([
            "runner",
            action,
            "--operation",
            "op-test",
            "--seat",
            "insider",
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
async fn runner_inspection_reports_external_execution_and_stop_is_rejected()
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
    let handlers = Router::new()
        .route(
            "/api/v1/room-setup-operations/op-test/seats/insider/runner",
            get(|| async { Json(external_runner()) }),
        )
        .route(
            "/api/v1/room-setup-operations/op-test/seats/insider/runner/stop",
            post(|| async { StatusCode::CONFLICT }),
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
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let output = invoke(&root, &state, endpoint, "inspect").map_err(|e| e.to_string())?;
        assert_eq!(output.status.code(), Some(0));
        let report: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        assert_eq!(report["runner"]["execution"]["kind"], "external");
        let output = invoke(&root, &state, endpoint, "stop").map_err(|e| e.to_string())?;
        assert_eq!(output.status.code(), Some(1));
        Ok(())
    })
    .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    result?.map_err(std::io::Error::other)?;
    Ok(())
}
