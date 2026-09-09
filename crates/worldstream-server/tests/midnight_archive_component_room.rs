//! Live qualification lane for the solo Midnight Archive route.
//!
//! The test uses the retained reviewed Bundle, the production portable
//! Component Host, and the same SQLite gateway/WebSocket boundary used by the
//! local Runtime.

use std::{
    env, fs,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tungstenite::{Message, WebSocket, client::IntoClientRequest as _};
use worldstream_component_host::ComponentPackHostV1;
use worldstream_core::{
    AuthorityBootstrapV1, AuthorityV1, CapabilityBearerV1, PackRegistryStatusV1, PrincipalKindV1,
    builtin_counter_registry,
};
use worldstream_pack_bundle::PackBundleVerifierV1;
use worldstream_protocol::{BearerWireV1, CreateRoomResponse};
use worldstream_runtime::EffectiveConfig;
use worldstream_server::{
    MemberCapabilityIssueResponse, OperatorState, SqliteGatewayBackend, operator_router,
};
use worldstream_sqlite::SqliteRoomStore;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PACK_ID: &str = "worldstream.midnight-archive";
const PACK_VERSION: &str = "0.1.0";
const CURRENT_BUNDLE_DIGEST: &str =
    "blake3:877702b321352288553cc0e5ea6510f1f8dea3e18687759658714ebc09a3c269";
const CURRENT_REVISION_DIGEST: &str =
    "blake3:6c3ad825a65307b9f5434d4a9140b7db4bd70d1f7830c66d6f6af1d2ba9dc0da";
const RETAINED_EVIDENCE_BUNDLE_DIGEST: &str =
    "blake3:e0626769fa745fafd0e41238473902988f446b453f283c7a7a7155a9122cf03f";
const RETAINED_EVIDENCE_REVISION_DIGEST: &str =
    "blake3:ee85f264b9c3dfb185ebedc9646bea655740f351c287793cce997336f0f419f2";
const RETAINED_FIRST_PLAYABLE_BUNDLE_DIGEST: &str =
    "blake3:d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b";
const RETAINED_FIRST_PLAYABLE_REVISION_DIGEST: &str =
    "blake3:679022bf13c15ea014e18a7129b679c9bfd27873c570fd9cd0c02818f0880a7a";
const FORBIDDEN_PRIVATE_KEYS: &[&str] = &["authentic_candidate_id", "is_authentic", "truth_marker"];

fn client_binding_identity() -> TestResult<(String, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/activity-clients/releases/midnight-archive-web-v3.json");
    let release: Value = serde_json::from_slice(&fs::read(path)?)?;
    let release_digest = release["release_digest"]
        .as_str()
        .ok_or("Archive client Release digest missing")?
        .to_owned();
    let surface_id = release["surfaces"]
        .as_array()
        .and_then(|surfaces| {
            surfaces.iter().find_map(|surface| {
                (surface["entrypoint"] == "/midnight-archive-v3/")
                    .then(|| surface["surface_id"].as_str())
                    .flatten()
            })
        })
        .ok_or("Archive standalone Client Surface missing")?
        .to_owned();
    Ok((release_digest, surface_id))
}

fn release_bundle_path(bundle_digest: &str) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let file_digest = bundle_digest
        .strip_prefix("blake3:")
        .unwrap_or_else(|| panic!("Archive Bundle digest has no blake3 prefix: {bundle_digest}"));
    workspace.join(format!(
        "packs/midnight-archive/releases/0.1.0/worldstream-midnight-archive-{file_digest}.wspack"
    ))
}

fn current_bundle_path() -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let proof_path = workspace
        .join("packs/midnight-archive/evidence/production-proof-0.1.0-agreement-route.json");
    let proof: Value = serde_json::from_slice(
        &fs::read(&proof_path)
            .unwrap_or_else(|error| panic!("read Archive proof {proof_path:?}: {error}")),
    )
    .unwrap_or_else(|error| panic!("parse Archive proof {proof_path:?}: {error}"));
    assert_eq!(proof["status"], "passed", "current Archive proof must pass");
    assert_eq!(proof["packId"], PACK_ID, "Archive proof Pack ID drifted");
    assert_eq!(
        proof["bundleDigest"], CURRENT_BUNDLE_DIGEST,
        "Archive proof physical Bundle digest drifted"
    );
    assert_eq!(
        proof["revisionDigest"], CURRENT_REVISION_DIGEST,
        "Archive proof semantic revision digest drifted"
    );
    env::var_os("WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| release_bundle_path(CURRENT_BUNDLE_DIGEST))
}

fn reject_private(value: &Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                assert!(
                    !FORBIDDEN_PRIVATE_KEYS.contains(&key.as_str()),
                    "private authenticity key leaked through participant/public observation: {key}"
                );
                reject_private(child);
            }
        }
        Value::Array(values) => values.iter().for_each(reject_private),
        _ => {}
    }
}

fn send(socket: &mut WebSocket<TcpStream>, kind: &str, body: &Value, id: &str) -> TestResult {
    socket.send(Message::Text(
        json!({"protocol":"0.1","type":kind,"message_id":id,"body":body})
            .to_string()
            .into(),
    ))?;
    Ok(())
}

fn receive(socket: &mut WebSocket<TcpStream>, kind: &str) -> TestResult<Value> {
    for _ in 0..32 {
        if let Message::Text(text) = socket.read()? {
            let envelope: Value = serde_json::from_str(&text)?;
            reject_private(&envelope["body"]);
            if envelope["type"] == kind {
                return Ok(envelope["body"].clone());
            }
            if envelope["type"] == "error" {
                return Err(
                    format!("server rejected expected {kind}: {}", envelope["body"]).into(),
                );
            }
        }
    }
    Err(format!("expected {kind} response missing").into())
}

fn receive_envelope(socket: &mut WebSocket<TcpStream>) -> TestResult<Value> {
    for _ in 0..32 {
        if let Message::Text(text) = socket.read()? {
            let envelope: Value = serde_json::from_str(&text)?;
            reject_private(&envelope["body"]);
            return Ok(envelope);
        }
    }
    Err("expected protocol response missing".into())
}

fn projection(body: &Value) -> Value {
    let envelope = body
        .get("projection")
        .or_else(|| body.get("observation"))
        .unwrap_or(body);
    envelope
        .get("activity")
        .cloned()
        .unwrap_or_else(|| envelope.clone())
}

fn action_offers(body: &Value) -> Option<&Vec<Value>> {
    let envelope = body
        .get("projection")
        .or_else(|| body.get("observation"))
        .unwrap_or(body);
    envelope.get("action_offers").and_then(Value::as_array)
}

fn room_seq(body: &Value) -> u64 {
    body.get("room_head")
        .and_then(|head| head.get("room_seq"))
        .and_then(Value::as_u64)
        .or_else(|| body.get("cause_room_seq").and_then(Value::as_u64))
        .unwrap_or_else(|| panic!("missing Room sequence in {body}"))
}

fn turns_used(state: &Value) -> u64 {
    state
        .get("turns_used")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("missing turns_used in {state}"))
}

async fn post(routes: &Router, authority: &str, path: &str, body: Value) -> TestResult<Value> {
    let response = routes
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", authority)
                .header("origin", "https://arena.example")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    assert!(status.is_success(), "{path} returned {status}: {bytes:?}");
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn solo_archive_witness_uses_real_component_host_and_replays_exactly() -> TestResult {
    let (client_release_digest, client_surface_id) = client_binding_identity()?;
    let current_path = current_bundle_path();
    let current_bytes = fs::read(&current_path)?;
    let verified = PackBundleVerifierV1.inspect(Arc::<[u8]>::from(current_bytes))?;
    let current_inspection = verified.inspection();
    assert_eq!(current_inspection.pack_id, PACK_ID);
    assert_eq!(current_inspection.explanatory_version, PACK_VERSION);
    assert_eq!(
        current_inspection.bundle_digest.to_string(),
        CURRENT_BUNDLE_DIGEST,
        "current release path must contain the exact proof-bound Bundle bytes"
    );
    assert_eq!(
        current_inspection.revision_digest.to_string(),
        CURRENT_REVISION_DIGEST,
        "current Bundle must inspect as the exact agreement-route revision"
    );
    assert_eq!(verified.bundle_digest().to_string(), CURRENT_BUNDLE_DIGEST);
    assert_eq!(
        verified.revision_digest().to_string(),
        CURRENT_REVISION_DIGEST
    );
    let current_revision_digest = verified.revision_digest().clone();
    let digest = current_revision_digest.to_string();
    let configuration: Value =
        serde_json::from_slice(&verified.golden_corpus().genesis.configuration.to_bytes()?)?;
    let witness = verified.golden_corpus().actions.clone();
    assert!(
        !witness.is_empty(),
        "the Bundle must freeze a non-empty witness"
    );
    assert_eq!(
        witness.len() % 2,
        0,
        "the Bundle witness must contain complete stage/commit turns"
    );
    assert!(witness.chunks_exact(2).all(|turn| {
        turn[0].action_type.starts_with("stage_") && turn[1].action_type == "commit_turn"
    }));
    let evidence_path = release_bundle_path(RETAINED_EVIDENCE_BUNDLE_DIGEST);
    let evidence_bytes = fs::read(&evidence_path)?;
    let evidence_verified = PackBundleVerifierV1.inspect(Arc::<[u8]>::from(evidence_bytes))?;
    let evidence_inspection = evidence_verified.inspection();
    assert_eq!(evidence_inspection.pack_id, PACK_ID);
    assert_eq!(evidence_inspection.explanatory_version, PACK_VERSION);
    assert_eq!(
        evidence_inspection.bundle_digest.to_string(),
        RETAINED_EVIDENCE_BUNDLE_DIGEST,
        "retained release path must contain the exact IMO-200 Bundle bytes"
    );
    assert_eq!(
        evidence_inspection.revision_digest.to_string(),
        RETAINED_EVIDENCE_REVISION_DIGEST,
        "retained Bundle must inspect as the exact IMO-200 revision"
    );
    let evidence_revision_digest = evidence_verified.revision_digest().clone();
    let first_playable_path = release_bundle_path(RETAINED_FIRST_PLAYABLE_BUNDLE_DIGEST);
    let first_playable_bytes = fs::read(&first_playable_path)?;
    let first_playable_verified =
        PackBundleVerifierV1.inspect(Arc::<[u8]>::from(first_playable_bytes))?;
    let first_playable_inspection = first_playable_verified.inspection();
    assert_eq!(first_playable_inspection.pack_id, PACK_ID);
    assert_eq!(first_playable_inspection.explanatory_version, PACK_VERSION);
    assert_eq!(
        first_playable_inspection.bundle_digest.to_string(),
        RETAINED_FIRST_PLAYABLE_BUNDLE_DIGEST,
        "retained release path must contain the exact IMO-199 Bundle bytes"
    );
    assert_eq!(
        first_playable_inspection.revision_digest.to_string(),
        RETAINED_FIRST_PLAYABLE_REVISION_DIGEST,
        "retained Bundle must inspect as the exact IMO-199 revision"
    );
    let first_playable_revision_digest = first_playable_verified.revision_digest().clone();

    let component_host = ComponentPackHostV1::new()?;
    let first_playable_admission = component_host.admit(
        first_playable_verified,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: false,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: true,
        },
    )?;
    let evidence_admission = component_host.admit(
        evidence_verified,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: false,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: true,
        },
    )?;
    let current_admission = component_host.admit(
        verified,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: true,
        },
    )?;
    // Registry construction executes and verifies both immutable golden corpora.
    let registry = builtin_counter_registry()?.admit_portable([
        first_playable_admission,
        evidence_admission,
        current_admission,
    ])?;
    registry.load_retained(&first_playable_revision_digest)?;
    registry.load_retained(&evidence_revision_digest)?;
    registry.load_retained(&current_revision_digest)?;
    for retained_revision in [&first_playable_revision_digest, &evidence_revision_digest] {
        let retained_catalog = registry.catalog_revision(retained_revision)?;
        assert!(!retained_catalog.selectable_for_new_rooms);
        assert!(retained_catalog.runnable_for_retained_rooms);
        assert!(retained_catalog.approved_for_activity_start);
    }
    let current_catalog = registry.catalog_revision(&current_revision_digest)?;
    assert!(current_catalog.selectable_for_new_rooms);
    assert!(current_catalog.runnable_for_retained_rooms);
    assert!(current_catalog.approved_for_activity_start);

    let directory = tempfile::tempdir()?;
    let file = tempfile::NamedTempFile::new_in(directory.path())?;
    let store = SqliteRoomStore::open(file.path())?;
    let host_bearer = CapabilityBearerV1::from_bytes([0xc1; 32]);
    AuthorityV1::new(Arc::new(store.clone())).bootstrap(
        AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5G11".parse()?,
            "01ARZ3NDEKTSV4RRFFQ69G5G12".parse()?,
            PrincipalKindV1::Human,
            "01ARZ3NDEKTSV4RRFFQ69G5G13".parse()?,
            host_bearer.token_hash(),
            None,
        )?,
        "2026-08-15T12:00:00Z".parse()?,
    )?;
    let backend = Arc::new(SqliteGatewayBackend::new(store, Arc::new(registry)));
    let routes =
        operator_router(OperatorState::new(EffectiveConfig::default())?.with_backend(backend));
    let host_header = format!("Bearer {}", BearerWireV1::from_bytes([0xc1; 32]).to_wire());
    let created: CreateRoomResponse = serde_json::from_value(post(
        &routes,
        &host_header,
        "/v1/rooms",
        json!({
            "pack":{"id":PACK_ID,"version":PACK_VERSION,"digest":digest},
            "configuration":configuration,
            "members":[{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G14","principal_kind":"human","role":"lead","access_mode":"participant"}],
            "idempotency_key":"midnight-archive-live-witness"
        }),
    ).await?)?;
    let member = created
        .member_ids
        .first()
        .ok_or("lead Membership missing")?
        .clone();
    let issued: MemberCapabilityIssueResponse = serde_json::from_value(post(
        &routes,
        &host_header,
        "/v1/operator/member-capabilities",
        json!({"room_id":created.room_id,"member_id":member,"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G14","scopes":["room:attach","room:observe_member","room:act","room:replay"],"idempotency_key":"01ARZ3NDEKTSV4RRFFQ69G5G15","expires_at":null}),
    ).await?)?;

    let ticket = post(
        &routes,
        &format!("Bearer {}", issued.bearer),
        "/v1/hosted/browser-stream-ticket",
        json!({"version":"hosted_browser_ws_ticket.v1","room_id":created.room_id,"member_id":member,"mode":"participant","after_frame_seq":null,"browser_session_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","client_release_digest":client_release_digest,"client_surface_id":client_surface_id}),
    ).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let routes_for_server = routes.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            routes_for_server.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let route_clone = routes.clone();
    let host_header_clone = host_header.clone();
    let digest_clone = digest.clone();
    let room_clone = created.room_id.clone();
    let result = tokio::task::spawn_blocking(move || -> TestResult<(u64, Value)> {
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut request = format!("ws://{address}/v1/hosted/browser-stream").into_client_request()?;
        request.headers_mut().insert("origin", "https://arena.example".parse()?);
        request.headers_mut().insert("sec-websocket-protocol", worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?);
        let (mut socket, _) = tungstenite::client(request, stream)?;
        socket.send(Message::Text(ticket["ticket"].as_str().ok_or("missing ticket")?.to_owned().into()))?;
        receive(&mut socket, "server.welcome")?;
        let attached = receive(&mut socket, "room.attached")?;
        let reset = receive(&mut socket, "projection.reset")?;
        assert_eq!(projection(&reset)["phase"], "briefing");
        assert!(action_offers(&reset).is_none_or(Vec::is_empty));
        send(&mut socket, "room.sync_ack", &json!({"through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}), "01ARZ3NDEKTSV4RRFFQ69G5G17")?;
        receive(&mut socket, "room.sync_acked")?;
        let launch = tokio::runtime::Handle::current().block_on(post(
            &route_clone,
            &host_header_clone,
            &format!("/v1/operator/rooms/{room_clone}/lobby/launch"),
            json!({"input_id":"01ARZ3NDEKSTV4RRFFQ69G5G20","based_on_room_seq":0,"pack_digest":digest_clone}),
        ))?;
        assert_eq!(launch["room_head"]["room_seq"], 1);
        let active = receive(&mut socket, "observation.deliver")?;
        let mut state = projection(&active);
        assert_eq!(state["phase"], "active");
        assert_eq!(turns_used(&state), 0);
        let initial_turns_remaining = state["turns_remaining"]
            .as_u64()
            .ok_or("initial turns_remaining missing")?;
        let mut last_seq = room_seq(&active);
        let mut offers = action_offers(&active)
            .cloned()
            .ok_or("active action offers missing")?;
        send(&mut socket, "action.submit", &json!({
            "action_id":"01ARZ3NDEKSTV4RRFFQ69G5G23",
            "based_on_room_seq":last_seq,
            "action_type":"stage_move",
            "payload":{"destination":"vault"}
        }), "01ARZ3NDEKSTV4RRFFQ69G5G24")?;
        let illegal = receive_envelope(&mut socket)?;
        assert_eq!(illegal["type"], "action.rejected", "illegal move must be rejected");
        assert_eq!(illegal["body"]["current_room_seq"], last_seq, "illegal Action changed the Room head");
        send(&mut socket, "action.submit", &json!({
            "action_id":"01ARZ3NDEKSTV4RRFFQ69G5G25",
            "based_on_room_seq":last_seq.saturating_sub(1),
            "action_type":"stage_wait",
            "payload":{}
        }), "01ARZ3NDEKSTV4RRFFQ69G5G26")?;
        let rejected = receive_envelope(&mut socket)?;
        assert_eq!(rejected["type"], "action.rejected", "stale Action must not commit");
        assert_eq!(rejected["body"]["current_room_seq"], last_seq, "stale Action changed the Room head");
        let mut committed_turns = 0_u64;
        for (index, expected) in witness.iter().enumerate() {
            let action_type = expected.action_type.as_str();
            assert!(
                offers.iter().any(|offer| offer["action_type"] == action_type),
                "expected witness Action {action_type} is absent at step {index}"
            );
            let payload: Value = serde_json::from_slice(&expected.canonical_payload.to_bytes()?)?;
            let before_turns = turns_used(&state);
            let before_remaining = state["turns_remaining"]
                .as_u64()
                .ok_or("turns_remaining missing before witness action")?;
            let before_power = state["power"]
                .as_u64()
                .ok_or("power missing before witness action")?;
            let before_gates = state["gates"].clone();
            send(&mut socket, "action.submit", &json!({"action_id":format!("01ARZ3NDEKTSV4RRFFQ69G5H{:02X}", index + 18),"based_on_room_seq":last_seq,"action_type":action_type,"payload":payload}), &format!("01ARZ3NDEKTSV4RRFFQ69G5J{:02X}", index + 40))?;
            let receipt = receive(&mut socket, "action.accepted")?;
            let observation = receive(&mut socket, "observation.deliver")?;
            state = projection(&observation);
            offers = action_offers(&observation)
                .cloned()
                .ok_or("next action offers missing")?;
            let next_seq = room_seq(&observation);
            assert_eq!(next_seq, last_seq + 1);
            if action_type.starts_with("stage_") {
                assert_eq!(turns_used(&state), before_turns);
                assert_eq!(state["turns_remaining"].as_u64(), Some(before_remaining));
                assert_eq!(state["power"].as_u64(), Some(before_power));
                assert_eq!(state["gates"], before_gates, "staging cannot change a gate before turn commit");
            }
            if action_type == "commit_turn" {
                committed_turns += 1;
                assert_eq!(turns_used(&state), before_turns + 1);
                assert_eq!(state["turns_remaining"].as_u64(), Some(before_remaining - 1));
                let after_power = state["power"].as_u64().ok_or("power missing after commit")?;
                assert!(
                    after_power <= before_power,
                    "a committed turn cannot create power ({before_power} -> {after_power})"
                );
            }
            assert_eq!(receipt["room_head"]["room_seq"], next_seq);
            last_seq = next_seq;
            let frame = observation["frame_seq"].as_u64().ok_or("observation frame missing")?;
            send(&mut socket, "observation.ack", &json!({"through_frame_seq":frame}), &format!("01ARZ3NDEKTSV4RRFFQ69G5K{:02X}", index + 70))?;
            receive(&mut socket, "observation.acked")?;
        }
        assert_eq!(committed_turns, witness.len() as u64 / 2);
        assert_eq!(turns_used(&state), committed_turns);
        assert_eq!(
            state["turns_remaining"].as_u64(),
            Some(initial_turns_remaining - committed_turns)
        );
        assert_eq!(state["outcome"]["kind"], "success");
        Ok((last_seq, state))
    }).await?;
    server.abort();
    let _ = server.await;
    let (last_seq, final_state) = result?;
    let replay = get(
        &routes,
        &format!("Bearer {}", issued.bearer),
        &format!(
            "/v1/rooms/{}/replay?at_room_seq={last_seq}",
            created.room_id
        ),
    )
    .await?;
    assert_eq!(replay["requested_room_seq"], last_seq);
    assert_eq!(projection(&replay)["phase"], final_state["phase"]);
    assert_eq!(projection(&replay)["outcome"], final_state["outcome"]);
    reject_private(&replay);
    Ok(())
}

async fn get(routes: &Router, authority: &str, path: &str) -> TestResult<Value> {
    let response = routes
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", authority)
                .header("origin", "https://arena.example")
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    assert!(status.is_success(), "{path} returned {status}: {bytes:?}");
    Ok(serde_json::from_slice(&bytes)?)
}
