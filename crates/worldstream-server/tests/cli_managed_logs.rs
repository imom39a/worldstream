//! CLI logs use the existing proved Controller without creating a process.
#![cfg(feature = "cli-operator-preview")]

use axum::{Json, Router, extract::Request, http::StatusCode, routing::get};
use std::process::{Command, Stdio};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    managed_controller::managed_controller_router,
    managed_http::AcceptedLocalSocket,
    process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
};

async fn logs(request: Request) -> Result<Json<serde_json::Value>, StatusCode> {
    if request.uri().query() != Some("tail=3") {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(Json(serde_json::json!({
        "schema":"worldstream/managed-server-logs/v1",
        "entries":[{"sequence":1,"operation_id":1,"action":"stop","stage":"complete"}]
    })))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_use_bounded_proved_control_and_never_start_the_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let claim = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, claim.generation())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let proof = lease.publish_endpoint(endpoint)?;
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let router = managed_controller_router(
        Router::new().route("/api/v1/control/server/logs", get(logs)),
        control,
        proof,
        shutdown,
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_worldstreamctl"))
            .arg("server")
            .arg("logs")
            .arg("--tail")
            .arg("3")
            .arg("--state-dir")
            .arg(state)
            .arg("--controller")
            .arg(endpoint.to_string())
            .arg("--json")
            .stdin(Stdio::null())
            .output()
    })
    .await?;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    let output = output?;
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["logs"][0]["stage"], "complete");
    assert_eq!(report["logs"].as_array().map(Vec::len), Some(1));
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    Ok(())
}
