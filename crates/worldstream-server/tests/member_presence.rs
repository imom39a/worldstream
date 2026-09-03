//! Direct participant synchronization supplies metadata-only readiness evidence.

use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use std::{
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
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

struct ParticipantFixture {
    _file: tempfile::NamedTempFile,
    routes: Router,
    host_header: String,
    member_header: String,
    room: String,
    member: String,
}

async fn fixture() -> TestResult<ParticipantFixture> {
    let file = tempfile::NamedTempFile::new()?;
    let store = SqliteRoomStore::open(file.path())?;
    let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
    let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse()?;
    AuthorityV1::new(Arc::new(store.clone())).bootstrap(
        AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse()?,
            principal,
            PrincipalKindV1::Human,
            "01ARZ3NDEKTSV4RRFFQ69G5FC3".parse()?,
            bearer.token_hash(),
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
    let host_header = format!("Bearer {}", BearerWireV1::from_bytes([0xa9; 32]).to_wire());
    let created: CreateRoomResponse = serde_json::from_value(post(&routes, &host_header, "/v1/rooms", json!({
        "pack":{"id":"worldstream.counter","version":"2.0.0","digest":counter_v2_digest().to_string()},
        "configuration":{"initial_value":0,"maximum_value":16},
        "members":[{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FC2","principal_kind":"human","role":"counter","access_mode":"participant"}],
        "idempotency_key":"presence-create"
    })).await?)?;
    let issued: MemberCapabilityIssueResponse = serde_json::from_value(post(&routes, &host_header, "/v1/operator/member-capabilities", json!({
        "room_id":created.room_id,"member_id":created.member_ids[0],"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FC2",
        "scopes":["room:attach","room:observe_member","room:act"],"idempotency_key":"01ARZ3NDEKTSV4RRFFQ69G5FD1","expires_at":null
    })).await?)?;
    Ok(ParticipantFixture {
        _file: file,
        routes,
        host_header,
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
    assert!(
        response.status().is_success(),
        "public fixture operation failed at {path}"
    );
    Ok(serde_json::from_slice(
        &response.into_body().collect().await?.to_bytes(),
    )?)
}

async fn presence(fixture: &ParticipantFixture, authorized: bool) -> TestResult<(u16, Value)> {
    let mut request = Request::builder().uri(format!(
        "/v1/operator/rooms/{}/members/{}/presence",
        fixture.room, fixture.member
    ));
    if authorized {
        request = request.header("authorization", &fixture.host_header);
    }
    let response = fixture
        .routes
        .clone()
        .oneshot(request.body(Body::empty())?)
        .await?;
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn send(socket: &mut WebSocket<TcpStream>, kind: &str, body: &Value) -> TestResult {
    socket.send(Message::Text(
        json!({"protocol":"0.1","type":kind,"message_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","body":body})
            .to_string()
            .into(),
    ))?;
    Ok(())
}

fn receive(socket: &mut WebSocket<TcpStream>, kind: &str) -> TestResult<Value> {
    for _ in 0..16 {
        let frame = socket.read()?;
        if let Message::Text(text) = frame {
            let envelope: Value = serde_json::from_str(&text)?;
            if envelope["type"] == kind {
                return Ok(envelope["body"].clone());
            }
        }
    }
    Err("expected protocol response missing".into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_sync_ack_qualifies_membership_until_disconnect() -> TestResult {
    let fixture = fixture().await?;
    // Runtime Host routes preserve their existing forbidden-without-authority contract.
    assert_eq!(presence(&fixture, false).await?.0, 403);
    assert_eq!(
        presence(&fixture, true).await?,
        (
            200,
            json!({"version":"membership_presence.v1","synchronized":false})
        )
    );
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
    let result=tokio::task::spawn_blocking(move || -> TestResult {
        let stream=TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut request=format!("ws://{address}/v1/stream").into_client_request()?;
        request.headers_mut().insert("authorization",fixture.member_header.parse()?);
        request.headers_mut().insert("sec-websocket-protocol",worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?);
        let (mut socket,_)=tungstenite::client(request,stream)?;
        send(&mut socket,"client.hello",&json!({"client_name":"direct-sdk","client_version":"1","mode":"participant","supported_protocols":[worldstream_protocol::PROTOCOL_VERSION],"capabilities":["cursor_ack","projection_reset"]}))?;
        receive(&mut socket,"server.welcome")?;
        send(&mut socket,"room.attach",&json!({"room_id":fixture.room,"member_id":fixture.member,"after_frame_seq":null}))?;
        let attached=receive(&mut socket,"room.attached")?;
        assert_eq!(runtime.block_on(presence(&fixture,true))?.1["synchronized"],false);
        send(&mut socket,"room.sync_ack",&json!({"room_id":fixture.room,"member_id":fixture.member,"through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}))?;
        receive(&mut socket,"room.sync_acked")?;
        assert_eq!(runtime.block_on(presence(&fixture,true))?.1,json!({"version":"membership_presence.v1","synchronized":true}));
        socket.close(None)?;
        let _=socket.read();
        drop(socket);
        for _ in 0..50 {
            if runtime.block_on(presence(&fixture,true))?.1["synchronized"]==false {return Ok(());}
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("disconnected membership remained ready".into())
    }).await;
    server.abort();
    let _ = server.await;
    result??;
    Ok(())
}
