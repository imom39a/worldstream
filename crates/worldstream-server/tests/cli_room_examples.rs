//! Reviewed examples use one exact-catalog CLI path for both reference Packs.
#![cfg(feature = "cli-operator-preview")]

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use std::{
    env, fs,
    net::SocketAddr,
    path::Path,
    process::{Command, Output, Stdio},
};
use worldstream_core::{Blake3DigestV1, agent_heist_lobby_digest, builtin_agent_heist_registry};
use worldstream_pack_bundle::PackBundleVerifierV1;
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    managed_controller::managed_controller_router,
    managed_http::AcceptedLocalSocket,
    process_ownership::{ProcessOwnership, ProcessRole, ProcessTermination},
};

fn catalogs() -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let registry = builtin_agent_heist_registry()?;
    let digest = agent_heist_lobby_digest();
    let heist = registry.catalog_revision(&digest)?;
    let schema: Value = serde_json::from_slice(
        &registry
            .resolve_schema(&digest, &heist.descriptor.configuration_schema)?
            .to_bytes()?,
    )?;
    let bundle = PackBundleVerifierV1.inspect(std::sync::Arc::from(include_bytes!(
        "../../../packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack"
    ).as_slice()))?;
    assert_eq!(
        bundle.descriptor().configuration_schema.schema_digest,
        Blake3DigestV1::hash(br#"{"type":"object"}"#)
    );
    Ok([
        (heist.descriptor, digest.to_string(), schema),
        (bundle.descriptor().clone(), bundle.revision_digest().to_string(), json!({"type": "object"})),
    ].into_iter().map(|(descriptor, digest, schema)| json!({
        "version": "activity_pack_catalog.v1", "revision": {
            "summary": {"pack": {"id": descriptor.pack_id, "version": descriptor.explanatory_version, "digest": digest},
                "name": descriptor.name, "selectable_for_new_rooms": true, "runnable_for_retained_rooms": true},
            "roles": descriptor.roles.iter().map(|role| json!({"role": role.role, "minimum": role.minimum, "maximum": role.maximum})).collect::<Vec<_>>(),
            "configuration_schema": {"schema_id": descriptor.configuration_schema.schema_id,
                "schema_digest": descriptor.configuration_schema.schema_digest.to_string(), "schema": schema},
            "actions": []
        }
    })).collect())
}

fn catalog_routes(catalogs: &[Value]) -> Router {
    let list = json!({"version": "activity_pack_catalog.v1", "revisions": catalogs.iter().map(|catalog| catalog["revision"]["summary"].clone()).collect::<Vec<_>>()});
    let mut routes = Router::new().route(
        "/api/v1/activity-packs",
        get(move || {
            let list = list.clone();
            async move { Json(list) }
        }),
    );
    for catalog in catalogs {
        let path = format!(
            "/api/v1/activity-packs/{}",
            catalog["revision"]["summary"]["pack"]["digest"]
                .as_str()
                .unwrap_or_default()
        );
        let catalog = catalog.clone();
        routes = routes.route(
            &path,
            get(move || {
                let catalog = catalog.clone();
                async move { Json(catalog) }
            }),
        );
    }
    routes
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_heist_and_negotiate_examples_can_be_generated_then_validated()
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
    let router = managed_controller_router(catalog_routes(&catalogs()?), control, proof, shutdown);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .await
    });
    let root = directory.path().to_path_buf();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        for (selector, filename, seat_count) in [
            ("worldstream.agent-heist@0.2.0", "heist.json", 3),
            ("worldstream.negotiate@0.1.0", "negotiate.json", 4),
        ] {
            let example = cli(
                &root,
                &state,
                endpoint,
                &["room", "example", "--pack", selector, "--output", filename],
            )
            .map_err(|error| error.to_string())?;
            assert_eq!(
                example.status.code(),
                Some(0),
                "reviewed example must be generated"
            );
            let input: Value = serde_json::from_slice(
                &fs::read(root.join(filename)).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            assert_eq!(input["schema"], "worldstream/room-setup/v1");
            assert_eq!(input["seats"].as_array().map(Vec::len), Some(seat_count));
            assert_eq!(input["operator_view"], false);
            let validation = cli(
                &root,
                &state,
                endpoint,
                &["room", "validate", "--file", filename],
            )
            .map_err(|error| error.to_string())?;
            assert_eq!(
                validation.status.code(),
                Some(0),
                "generated example must validate"
            );
            let report: Value =
                serde_json::from_slice(&validation.stdout).map_err(|error| error.to_string())?;
            assert_eq!(report["room_setup"]["seat_count"], seat_count);
        }
        Ok(())
    })
    .await;
    server.abort();
    let _ = server.await;
    lease.finish(ProcessTermination::Stopped)?;
    result?.map_err(std::io::Error::other)?;
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    assert_eq!(fs::read_dir(directory.path())?.count(), 3);
    Ok(())
}
