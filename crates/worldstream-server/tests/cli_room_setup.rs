#![cfg(feature = "cli-operator-preview")]

use std::{
    env, fs,
    process::{Command, Output, Stdio},
};

fn heist_catalog() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use worldstream_core::{agent_heist_lobby_digest, builtin_agent_heist_registry};
    let registry = builtin_agent_heist_registry()?;
    let digest = agent_heist_lobby_digest();
    let revision = registry.catalog_revision(&digest)?;
    let reference = &revision.descriptor.configuration_schema;
    let schema: serde_json::Value =
        serde_json::from_slice(&registry.resolve_schema(&digest, reference)?.to_bytes()?)?;
    Ok(serde_json::json!({
        "version": "activity_pack_catalog.v1", "revision": {
            "summary": {"pack": {"id": "worldstream.agent-heist", "version": "0.2.0", "digest": digest.to_string()},
                "name": "Agent Heist", "selectable_for_new_rooms": true, "runnable_for_retained_rooms": true},
            "roles": revision.descriptor.roles.iter().map(|role| serde_json::json!({"role": role.role,
                "minimum": role.minimum, "maximum": role.maximum})).collect::<Vec<_>>(),
            "configuration_schema": {"schema_id": reference.schema_id,
                "schema_digest": reference.schema_digest.to_string(), "schema": schema},
            "actions": []
        }
    }))
}

#[test]
fn invalid_room_setup_is_rejected_before_controller_access_without_echoing_input()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    fs::write(
        directory.path().join("setup.json"),
        br#"{"token":"NEVER_PRINT_SETUP_SECRET"}"#,
    )?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(directory.path())
        .stdin(Stdio::null())
        .args(["room", "validate", "--file", "setup.json", "--json"]);
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    let output = command.output()?;
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["status"], "rejected");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("NEVER_PRINT"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("NEVER_PRINT"));
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

struct ValidationObservation {
    output: Output,
    requests: usize,
    runtime_exists: bool,
    files: usize,
}

fn dependency_routes(requests: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> axum::Router {
    use axum::{Json, Router, http::StatusCode, routing::get};
    use std::sync::atomic::Ordering;
    let missing_count = requests.clone();
    let profile_count = requests.clone();
    Router::new()
        .route(
            "/api/v1/agent-profiles/missing-profile/revisions/v1",
            get(move || {
                missing_count.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::NOT_FOUND }
            }),
        )
        .route(
            "/api/v1/agent-profiles/installed-profile/revisions/v1",
            get(move || {
                profile_count.fetch_add(1, Ordering::SeqCst);
                async {
                    Json(serde_json::json!({
                        "profile_id": "installed-profile", "revision": "v1",
                        "display_name": "Installed external profile",
                        "non_secret_configuration": {}, "secret_settings": [],
                        "host_contract": {"kind": "generic_mcp"}
                    }))
                }
            }),
        )
        .route(
            "/api/v1/runner-templates",
            get(move || {
                requests.fetch_add(1, Ordering::SeqCst);
                async {
                    Json(serde_json::json!({
                        "schema": "worldstream/studio-runner-template-catalog/v1", "templates": [{
                            "template_id": "counter-runner", "revision": "v1",
                            "display_name": "Counter-only Runner",
                            "executable_blake3": "0000000000000000000000000000000000000000000000000000000000000000",
                            "compatibility": [{"activity_pack_id": "worldstream.counter", "exact_revisions": ["4.0.0"]}],
                            "capacity": {"maximum_concurrent_invocations": 1},
                            "health_stale_after_ms": 5000, "non_secret_settings": [],
                            "secret_settings": [], "instances": ["counter-local"]
                        }]
                    }))
                }
            }),
        )
}

async fn validate_with_catalog(
    input: &[u8],
) -> Result<ValidationObservation, Box<dyn std::error::Error>> {
    use axum::{Json, Router, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use worldstream_runtime::prepare_data_directory;
    use worldstream_studio_supervisor::{
        control_access::ControlAccess,
        managed_controller::managed_controller_router,
        managed_http::AcceptedLocalSocket,
        process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
    };
    let directory = tempfile::tempdir()?;
    let state = prepare_data_directory(&directory.path().join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let claim = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, claim.generation())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let proof = lease.publish_endpoint(endpoint)?;
    let catalog = heist_catalog()?;
    let path = format!(
        "/api/v1/activity-packs/{}",
        worldstream_core::agent_heist_lobby_digest()
    );
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let routes = Router::new()
        .route(
            &path,
            get(move || {
                count.fetch_add(1, Ordering::SeqCst);
                let catalog = catalog.clone();
                async move { Json(catalog) }
            }),
        )
        .merge(dependency_routes(requests.clone()));
    let (shutdown, _requested) = tokio::sync::watch::channel(false);
    let router = managed_controller_router(routes, control, proof, shutdown);
    fs::write(directory.path().join("setup.json"), input)?;
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let root = directory.path().to_path_buf();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
        command
            .current_dir(root)
            .stdin(Stdio::null())
            .args([
                "room",
                "validate",
                "--file",
                "setup.json",
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
    })
    .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    Ok(ValidationObservation {
        output: output??,
        requests: requests.load(Ordering::SeqCst),
        runtime_exists: ownership.snapshot(ProcessRole::Runtime)?.is_some(),
        files: fs::read_dir(directory.path())?.count(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_validation_reads_exact_catalog_over_proved_control_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let observed = validate_with_catalog(include_bytes!(
        "../../../examples/room-setup/agent-heist-0.2.0.json"
    ))
    .await?;
    assert_eq!(observed.output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&observed.output.stdout)?;
    assert_eq!(report["room_setup"]["pack"]["version"], "0.2.0");
    assert_eq!(report["room_setup"]["seat_count"], 3);
    assert_eq!(observed.requests, 1);
    assert!(!observed.runtime_exists);
    assert_eq!(observed.files, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_missing_agent_profile_is_rejected_without_replacement_or_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../examples/room-setup/agent-heist-0.2.0.json"
    ))?;
    input["seats"][1]["assignment"]["agent_profile"] = serde_json::json!({
        "profile_id": "missing-profile", "revision": "v1"
    });
    let observed = validate_with_catalog(&serde_json::to_vec(&input)?).await?;
    assert_eq!(observed.output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&observed.output.stdout)?;
    assert_eq!(
        report["setup_issue"],
        serde_json::json!({
            "path": "/seats/1/assignment/agent_profile", "code": "dependency_missing"
        })
    );
    assert_eq!(observed.requests, 2);
    assert!(!observed.runtime_exists);
    assert_eq!(observed.files, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_installed_profile_metadata_allows_validation_without_issuing_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../examples/room-setup/agent-heist-0.2.0.json"
    ))?;
    input["seats"][1]["assignment"]["agent_profile"] = serde_json::json!({
        "profile_id": "installed-profile", "revision": "v1"
    });
    let observed = validate_with_catalog(&serde_json::to_vec(&input)?).await?;
    assert_eq!(observed.output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&observed.output.stdout)?;
    assert_eq!(report["status"], "complete");
    assert_eq!(report["room_setup"]["seat_count"], 3);
    assert_eq!(observed.requests, 2);
    assert!(!observed.runtime_exists);
    assert_eq!(observed.files, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_setup_rejects_missing_exact_runner_template_without_installing_it()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../examples/room-setup/agent-heist-0.2.0.json"
    ))?;
    input["seats"][1]["assignment"] = serde_json::json!({
        "mode": "managed",
        "agent_profile": {"profile_id": "installed-profile", "revision": "v1"},
        "runner_template": {"template_id": "missing-runner", "revision": "v1"}
    });
    let observed = validate_with_catalog(&serde_json::to_vec(&input)?).await?;
    assert_eq!(observed.output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&observed.output.stdout)?;
    assert_eq!(
        report["setup_issue"],
        serde_json::json!({
            "path": "/seats/1/assignment/runner_template", "code": "dependency_missing"
        })
    );
    assert_eq!(observed.requests, 3);
    assert!(!observed.runtime_exists);
    assert_eq!(observed.files, 2);
    input["seats"][1]["assignment"]["runner_template"]["template_id"] =
        serde_json::json!("counter-runner");
    let incompatible = validate_with_catalog(&serde_json::to_vec(&input)?).await?;
    assert_eq!(incompatible.output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&incompatible.output.stdout)?;
    assert_eq!(
        report["setup_issue"],
        serde_json::json!({
            "path": "/seats/1/assignment/runner_template", "code": "dependency_incompatible"
        })
    );
    Ok(())
}
