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
const FORBIDDEN_PRIVATE_KEYS: &[&str] = &["authentic_candidate_id", "is_authentic", "truth_marker"];

fn client_binding_identity() -> TestResult<(String, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/activity-clients/releases/midnight-archive-web-v1.json");
    let release: Value = serde_json::from_slice(&fs::read(path)?)?;
    let release_digest = release["release_digest"]
        .as_str()
        .ok_or("Archive client Release digest missing")?
        .to_owned();
    let surface_id = release["surfaces"]
        .as_array()
        .and_then(|surfaces| {
            surfaces.iter().find_map(|surface| {
                (surface["entrypoint"] == "/midnight-archive-v1/")
                    .then(|| surface["surface_id"].as_str())
                    .flatten()
            })
        })
        .ok_or("Archive standalone Client Surface missing")?
        .to_owned();
    Ok((release_digest, surface_id))
}

fn bundle_path() -> PathBuf {
    if let Some(path) = env::var_os("WORLDSTREAM_MIDNIGHT_ARCHIVE_BUNDLE") {
        return PathBuf::from(path);
    }
    let release =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/midnight-archive/releases/0.1.0");
    let mut candidates = fs::read_dir(&release)
        .unwrap_or_else(|error| panic!("read Archive release directory {release:?}: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "wspack")
        })
        .filter(|path| !path.to_string_lossy().contains("candidate"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no immutable Archive Bundle in {release:?}"))
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
    let bytes = fs::read(bundle_path())?;
    let verified = PackBundleVerifierV1.inspect(Arc::<[u8]>::from(bytes))?;
    assert_eq!(verified.descriptor().pack_id, PACK_ID);
    assert_eq!(verified.descriptor().explanatory_version, PACK_VERSION);
    let digest = verified.revision_digest().to_string();
    let configuration: Value =
        serde_json::from_slice(&verified.golden_corpus().genesis.configuration.to_bytes()?)?;
    let witness = verified.golden_corpus().actions.clone();
    assert_eq!(
        witness.len(),
        20,
        "the Bundle must freeze ten stage/commit turns"
    );
    assert!(witness.iter().enumerate().all(|(index, action)| {
        (index % 2 == 0 && action.action_type.starts_with("stage_"))
            || (index % 2 == 1 && action.action_type == "commit_turn")
    }));
    let admission = ComponentPackHostV1::new()?.admit(
        verified,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: true,
        },
    )?;
    let registry = builtin_counter_registry()?.admit_portable([admission])?;

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
        for (index, expected) in witness.iter().enumerate() {
            let action_type = expected.action_type.as_str();
            if index == 6 {
                assert_eq!(state["gates"]["service_hatch"], "closed", "Plant gate must be closed before its opening turn");
            }
            if index == 8 {
                assert_eq!(state["gates"]["service_hatch"], "open", "Plant gate must open before the next turn can traverse it");
            }
            assert!(
                offers.iter().any(|offer| offer["action_type"] == action_type),
                "expected witness Action {action_type} is absent at step {index}"
            );
            let payload: Value = serde_json::from_slice(&expected.canonical_payload.to_bytes()?)?;
            let before_turns = turns_used(&state);
            send(&mut socket, "action.submit", &json!({"action_id":format!("01ARZ3NDEKTSV4RRFFQ69G5H{:02X}", index + 18),"based_on_room_seq":last_seq,"action_type":action_type,"payload":payload}), &format!("01ARZ3NDEKTSV4RRFFQ69G5J{:02X}", index + 40))?;
            let receipt = receive(&mut socket, "action.accepted")?;
            let observation = receive(&mut socket, "observation.deliver")?;
            state = projection(&observation);
            offers = action_offers(&observation)
                .cloned()
                .ok_or("next action offers missing")?;
            let next_seq = room_seq(&observation);
            assert_eq!(next_seq, last_seq + 1);
            if action_type.starts_with("stage_") { assert_eq!(turns_used(&state), before_turns); }
            if action_type == "commit_turn" { assert_eq!(turns_used(&state), before_turns + 1); }
            if action_type == "commit_turn" {
                let expected_power = match index {
                    1 => 3,
                    3 => 2,
                    5 => 2,
                    7 => 0,
                    _ => 0,
                };
                assert_eq!(state["power"].as_u64(), Some(expected_power));
            }
            assert_eq!(receipt["room_head"]["room_seq"], next_seq);
            last_seq = next_seq;
            let frame = observation["frame_seq"].as_u64().ok_or("observation frame missing")?;
            send(&mut socket, "observation.ack", &json!({"through_frame_seq":frame}), &format!("01ARZ3NDEKTSV4RRFFQ69G5K{:02X}", index + 70))?;
            receive(&mut socket, "observation.acked")?;
        }
        assert_eq!(turns_used(&state), 10);
        assert_eq!(state["turns_remaining"], 6);
        assert_eq!(state["power"], 0);
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
