//! Operator CLI receipts retain the public setup reference across invocations.
#![cfg(feature = "cli-operator-preview")]

use axum::{
    Json, Router,
    extract::Path as RoutePath,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    env, fs,
    net::SocketAddr,
    path::Path,
    process::{Command, Output, Stdio},
};
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    managed_controller::managed_controller_router,
    managed_http::AcceptedLocalSocket,
    process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
};

fn status(operation: &str) -> Value {
    json!({"version":"room_setup_operation.v1", "operation":operation,
        "room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV", "complete":true,
        "stage":"complete", "active_stage":null, "next_action":"inspect_room"})
}

fn cli(
    root: &Path,
    state: &Path,
    endpoint: SocketAddr,
    arguments: &[&str],
) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .args(arguments)
        .arg("--state-dir")
        .arg(state)
        .arg("--controller")
        .arg(endpoint.to_string())
        .arg("--json");
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.output()
}

fn operation_routes() -> Router {
    Router::new()
        .route(
            "/api/v1/room-setup-operations/{operation}",
            post(
                |RoutePath(operation): RoutePath<String>, Json(body): Json<Value>| async move {
                    assert_eq!(body["specification"]["schema"], "worldstream/room-setup/v1");
                    if body["specification"]["pack"]["id"] == "worldstream.negotiate" {
                        assert_eq!(body["acknowledge_start"], false);
                        return (StatusCode::CONFLICT, Json(json!({"error": {
                            "code":"room_setup_acknowledgement_required",
                            "message":"Activity starts at Genesis; acknowledge this before creating the Room"
                        }})));
                    }
                    assert_eq!(
                        body["specification"]["pack"]["id"],
                        "worldstream.agent-heist"
                    );
                    assert_eq!(body["acknowledge_start"], false);
                    (StatusCode::OK, Json(status(&operation)))
                },
            )
            .get(|RoutePath(operation): RoutePath<String>| async move { Json(status(&operation)) }),
        )
        .route(
            "/api/v1/room-setup-operations/{operation}/resume",
            post(
                |RoutePath(operation): RoutePath<String>, body: String| async move {
                    assert!(
                        body.is_empty(),
                        "resume carries no replacement specification"
                    );
                    Json(status(&operation))
                },
            ),
        )
        .route(
            "/api/v1/room-setup-operations",
            get(|| async { Json(json!({"version":"room_setup_operations.v1", "operations":[]})) }),
        )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_launch_distinguishes_a_committed_lobby_from_active_at_genesis()
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
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let routes = Router::new().route("/api/v1/rooms/{room}/launch", post(
        |RoutePath(room): RoutePath<String>, body: String| async move {
            assert!(body.is_empty());
            let launched = room.ends_with('V');
            let assessment = json!({"version":"room_launch_assessment.v1", "operation":"op-test",
                "room_id":room, "provisioning_complete":true,
                "applicability":if launched {"lobby_launch"} else {"active_at_genesis"},
                "readiness":{"ready_to_launch":false,"seats":[]},
                "launch":if launched {json!({"state":"launched","attempts":1,"attention":null,
                    "transition_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY"})} else {Value::Null}});
            (if launched {StatusCode::OK} else {StatusCode::CONFLICT}, Json(assessment))
        }
    ));
    let service = managed_controller_router(routes, control, proof, shutdown);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            service.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let root = directory.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        for (room, exit, code) in [
            ("01ARZ3NDEKTSV4RRFFQ69G5FAV", 0, "complete"),
            ("01ARZ3NDEKTSV4RRFFQ69G5FAW", 1, "room_launch_inapplicable"),
        ] {
            let output = cli(&root, &state, endpoint, &["room", "launch", room])
                .map_err(|e| e.to_string())?;
            assert_eq!(output.status.code(), Some(exit));
            let receipt: Value =
                serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
            assert_eq!(receipt["code"], code);
            assert_eq!(receipt["launch_assessment"]["room_id"], room);
        }
        Ok(())
    })
    .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    result?.map_err(std::io::Error::other)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_receipt_can_be_inspected_and_resumed_without_reading_replacement_input()
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
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let router = managed_controller_router(operation_routes(), control, proof, shutdown);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    fs::write(
        directory.path().join("setup.json"),
        include_bytes!("../../../examples/room-setup/agent-heist-0.2.0.json"),
    )?;
    let root = directory.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let output = cli(
            &root,
            &state,
            endpoint,
            &["room", "create", "--file", "setup.json"],
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            output.status.code(),
            Some(0),
            "create should return its retained operation"
        );
        let report: Value =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        let operation = report["operation_id"]
            .as_str()
            .ok_or("missing operation reference")?;
        assert!(!operation.is_empty());
        assert_eq!(report["room_id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        fs::write(root.join("setup.json"), b"not replacement intent")
            .map_err(|error| error.to_string())?;
        for verb in ["status", "resume"] {
            let output = cli(&root, &state, endpoint, &["room", "setup", verb, operation])
                .map_err(|error| error.to_string())?;
            assert_eq!(output.status.code(), Some(0));
            let resumed: Value =
                serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
            assert_eq!(resumed["operation_id"], operation);
            assert_eq!(resumed["room_id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        }
        let output = cli(&root, &state, endpoint, &["room", "setup", "status"])
            .map_err(|error| error.to_string())?;
        assert_eq!(output.status.code(), Some(0));
        fs::write(
            root.join("negotiate.json"),
            include_bytes!("../../../examples/room-setup/negotiate-0.1.0.json"),
        )
        .map_err(|error| error.to_string())?;
        let output = cli(
            &root,
            &state,
            endpoint,
            &["room", "create", "--file", "negotiate.json"],
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(output.status.code(), Some(1));
        let warning: Value =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        assert_eq!(warning["code"], "start_acknowledgement_required");
        assert!(
            warning.get("room_id").is_none(),
            "rejection must not claim a Room"
        );
        let guidance = format!("{} {}", warning["message"], warning["next_action"]);
        assert!(guidance.contains("deadline"));
        assert!(guidance.contains("--acknowledge-start"));
        Ok(())
    })
    .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    result?.map_err(std::io::Error::other)?;
    Ok(())
}
