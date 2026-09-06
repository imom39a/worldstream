//! Hosted browser admission binds one opaque first-frame ticket to the exact
//! Membership and streams directly from the Runtime without browser routing IDs.

use std::{
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tungstenite::{Message, WebSocket, client::IntoClientRequest as _};
use worldstream_core::{
    AuthorityBootstrapV1, AuthorityV1, CapabilityBearerV1, PrincipalKindV1,
    builtin_worldstream_registry, counter_v2_digest,
};
use worldstream_protocol::{BearerWireV1, CreateRoomResponse};
use worldstream_runtime::EffectiveConfig;
use worldstream_server::{
    MemberCapabilityIssueResponse, OperatorState, SqliteGatewayBackend, operator_router,
};
use worldstream_sqlite::SqliteRoomStore;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct HostedFixture {
    _file: tempfile::NamedTempFile,
    routes: Router,
    member_header: String,
    room: String,
    member: String,
}

async fn fixture() -> TestResult<HostedFixture> {
    let file = tempfile::NamedTempFile::new()?;
    let store = SqliteRoomStore::open(file.path())?;
    let host_bearer = CapabilityBearerV1::from_bytes([0xc1; 32]);
    let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse()?;
    AuthorityV1::new(Arc::new(store.clone())).bootstrap(
        AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse()?,
            principal,
            PrincipalKindV1::Human,
            "01ARZ3NDEKTSV4RRFFQ69G5FC3".parse()?,
            host_bearer.token_hash(),
            None,
        )?,
        "2026-08-15T12:00:00Z".parse()?,
    )?;
    let backend = Arc::new(SqliteGatewayBackend::new(
        store,
        Arc::new(builtin_worldstream_registry()?),
    ));
    let routes =
        operator_router(OperatorState::new(EffectiveConfig::default())?.with_backend(backend));
    let host_header = format!("Bearer {}", BearerWireV1::from_bytes([0xc1; 32]).to_wire());
    let created: CreateRoomResponse = serde_json::from_value(
        post(
            &routes,
            &host_header,
            "/v1/rooms",
            json!({
                "pack": {
                    "id": "worldstream.counter",
                    "version": "2.0.0",
                    "digest": counter_v2_digest().to_string()
                },
                "configuration": {"initial_value": 0, "maximum_value": 16},
                "members": [{
                    "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                    "principal_kind": "human",
                    "role": "counter",
                    "access_mode": "participant"
                }],
                "idempotency_key": "hosted-browser-create"
            }),
        )
        .await?,
    )?;
    let issued: MemberCapabilityIssueResponse = serde_json::from_value(
        post(
            &routes,
            &host_header,
            "/v1/operator/member-capabilities",
            json!({
                "room_id": created.room_id,
                "member_id": created.member_ids[0],
                "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                "scopes": ["room:attach", "room:observe_member", "room:act"],
                "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD1",
                "expires_at": null
            }),
        )
        .await?,
    )?;
    Ok(HostedFixture {
        _file: file,
        routes,
        member_header: format!("Bearer {}", issued.bearer),
        room: created.room_id,
        member: created.member_ids[0].clone(),
    })
}

async fn post(routes: &Router, authority: &str, path: &str, body: Value) -> TestResult<Value> {
    let response = routes
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", authority)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    assert!(response.status().is_success(), "fixture failed at {path}");
    Ok(serde_json::from_slice(
        &response.into_body().collect().await?.to_bytes(),
    )?)
}

fn send(socket: &mut WebSocket<TcpStream>, kind: &str, body: &Value) -> TestResult {
    socket.send(Message::Text(
        json!({
            "protocol": "0.1",
            "type": kind,
            "message_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "body": body
        })
        .to_string()
        .into(),
    ))?;
    Ok(())
}

fn receive(socket: &mut WebSocket<TcpStream>, kind: &str) -> TestResult<Value> {
    for _ in 0..16 {
        if let Message::Text(text) = socket.read()? {
            let envelope: Value = serde_json::from_str(&text)?;
            if envelope["type"] == kind {
                return Ok(envelope["body"].clone());
            }
        }
    }
    Err(format!("expected {kind} response missing").into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn ticket_bootstraps_exact_live_stream_and_session_revocation_closes_it() -> TestResult {
    let fixture = fixture().await?;
    let browser_session_digest = format!("blake3:{}", "a".repeat(64));
    let ticket_response = fixture
        .routes
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hosted/browser-stream-ticket")
                .header("authorization", &fixture.member_header)
                .header("origin", "https://arena.example")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "version": "hosted_browser_ws_ticket.v1",
                        "room_id": fixture.room,
                        "member_id": fixture.member,
                        "mode": "participant",
                        "after_frame_seq": null,
                        "browser_session_digest": browser_session_digest,
                        "client_release_digest": format!("blake3:{}", "b".repeat(64)),
                        "client_surface_id": "participant"
                    })
                    .to_string(),
                ))?,
        )
        .await?;
    assert_eq!(ticket_response.status(), 201);
    assert_eq!(ticket_response.headers()["cache-control"], "no-store");
    let ticket_body: Value =
        serde_json::from_slice(&ticket_response.into_body().collect().await?.to_bytes())?;
    let ticket = ticket_body["ticket"]
        .as_str()
        .ok_or("ticket response did not contain a ticket")?
        .to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let routes = fixture.routes.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            routes.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let runtime = tokio::runtime::Handle::current();
    let result = tokio::task::spawn_blocking(move || -> TestResult {
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut request =
            format!("ws://{address}/v1/hosted/browser-stream").into_client_request()?;
        request
            .headers_mut()
            .insert("origin", "https://arena.example".parse()?);
        request.headers_mut().insert(
            "sec-websocket-protocol",
            worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?,
        );
        let (mut socket, response) = tungstenite::client(request, stream)?;
        assert_eq!(
            response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|value| value.to_str().ok()),
            Some(worldstream_protocol::WEBSOCKET_SUBPROTOCOL)
        );
        socket.send(Message::Text(ticket.into()))?;

        let welcome = receive(&mut socket, "server.welcome")?;
        assert_eq!(
            welcome["selected_protocol"],
            worldstream_protocol::PROTOCOL_VERSION
        );
        let attached = receive(&mut socket, "room.attached")?;
        assert_eq!(attached["room_id"], fixture.room);
        assert_eq!(attached["member_id"], fixture.member);
        assert_eq!(attached["access_mode"], "participant");
        let reset = receive(&mut socket, "projection.reset")?;
        assert_eq!(reset["room_id"], fixture.room);
        assert_eq!(reset["member_id"], fixture.member);

        send(
            &mut socket,
            "room.sync_ack",
            &json!({
                "through_frame_head": attached["frame_head"],
                "sync_token": attached["sync_token"]
            }),
        )?;
        let synced = receive(&mut socket, "room.sync_acked")?;
        assert_eq!(synced["through_frame_head"], attached["frame_head"]);

        let revoke = runtime.block_on(post(
            &fixture.routes,
            &fixture.member_header,
            "/v1/hosted/browser-stream-session:revoke",
            json!({
                "version": "hosted_browser_ws_session_revoke.v1",
                "browser_session_digest": browser_session_digest
            }),
        ))?;
        assert_eq!(revoke["revoked"], true);
        let error = receive(&mut socket, "error")?;
        assert_eq!(error["code"], "forbidden");
        Ok(())
    })
    .await;
    server.abort();
    let _ = server.await;
    result??;
    Ok(())
}
