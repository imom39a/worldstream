//! Explicit exports keep Membership and Runner authority in separate protected files.

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    env, fs,
    net::SocketAddr,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    managed_controller::managed_controller_router,
    managed_http::AcceptedLocalSocket,
    process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
};

const MEMBER_BEARER: &str = "wsb1:1111111111111111111111111111111111111111111111111111111111111111";
const RUNNER_BEARER: &str = "wsb1:2222222222222222222222222222222222222222222222222222222222222222";

fn transfer_documents() -> [Value; 2] {
    let pack = json!({"id":"worldstream.agent-heist","version":"0.2.0",
        "digest":worldstream_core::agent_heist_lobby_digest().to_string()});
    [
        json!({"schema":"worldstream/membership-credentials/v1","operation":"op-test","seat":"insider",
        "runtime_url":"ws://127.0.0.1:9400/v1/stream","room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "member_id":"01ARZ3NDEKTSV4RRFFQ69G5FAW","principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FAX",
        "pack":pack,"role":"insider","scopes":["room:attach","room:act","room:observe_member"],"bearer":MEMBER_BEARER}),
        json!({"schema":"worldstream/runner-credentials/v1","operation":"op-test","seat":"insider",
        "runtime_url":"ws://127.0.0.1:9400/v1/runner/stream","runner_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY",
        "owner_principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FAX","pack":pack,
        "permitted_memberships":[{"room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","member_id":"01ARZ3NDEKTSV4RRFFQ69G5FAW"}],
        "scopes":["activation:offer_receive","activation:claim","activation:complete"],"bearer":RUNNER_BEARER}),
    ]
}

fn export_routes(requests: &Arc<AtomicUsize>) -> Router {
    let mut service = Router::new();
    for (suffix, document) in ["membership-credentials", "runner-credentials"]
        .into_iter()
        .zip(transfer_documents())
    {
        let requests = requests.clone();
        service = service.route(
            &format!("/api/v1/room-setup-operations/op-test/seats/insider/{suffix}"),
            post(move || {
                let document = document.clone();
                requests.fetch_add(1, Ordering::SeqCst);
                async move { Json(document) }
            }),
        );
    }
    service
}

fn export(
    root: &Path,
    state: &Path,
    endpoint: SocketAddr,
    family: &str,
    filename: &str,
) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .args([
            family,
            "export-credentials",
            "--operation",
            "op-test",
            "--seat",
            "insider",
            "--output",
            filename,
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

fn check_exports(
    root: &Path,
    state: &Path,
    endpoint: SocketAddr,
    requests: &AtomicUsize,
) -> Result<(), String> {
    for (family, filename, expected) in [
        ("client", "member.json", MEMBER_BEARER),
        ("runner", "runner.json", RUNNER_BEARER),
    ] {
        let output = export(root, state, endpoint, family, filename).map_err(|e| e.to_string())?;
        assert_eq!(
            output.status.code(),
            Some(0),
            "explicit scoped export should succeed"
        );
        let bytes = fs::read(root.join(filename)).map_err(|e| e.to_string())?;
        let credentials: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        assert_eq!(credentials["bearer"], expected);
        assert_eq!(credentials["operation"], "op-test");
        worldstream_runtime::validate_owner_only_file(&root.join(filename))
            .map_err(|e| e.to_string())?;
        for forbidden in [MEMBER_BEARER, RUNNER_BEARER] {
            assert!(!String::from_utf8_lossy(&output.stdout).contains(forbidden));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(forbidden));
        }
        assert_eq!(credentials.get("runner_id").is_some(), family == "runner");
        let before = requests.load(Ordering::SeqCst);
        let repeated =
            export(root, state, endpoint, family, filename).map_err(|e| e.to_string())?;
        assert_eq!(repeated.status.code(), Some(1));
        assert_eq!(
            fs::read(root.join(filename)).map_err(|e| e.to_string())?,
            bytes
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            before,
            "refuse existing output before credential retrieval"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_exports_use_distinct_owner_only_files_and_never_overwrite()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = worldstream_runtime::prepare_data_directory(&directory.path().join("protected"))?;
    let state = worldstream_runtime::prepare_data_directory(&root.join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let claim = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, claim.generation())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let proof = lease.publish_endpoint(endpoint)?;
    let requests = Arc::new(AtomicUsize::new(0));
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let service = managed_controller_router(export_routes(&requests), control, proof, shutdown);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            service.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let result =
        tokio::task::spawn_blocking(move || check_exports(&root, &state, endpoint, &requests))
            .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    result?.map_err(std::io::Error::other)?;
    Ok(())
}
