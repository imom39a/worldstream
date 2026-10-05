//! HTTP ingress and real WebSocket delivery, with a deterministic durable-outcome backend.
#![allow(clippy::unwrap_used, clippy::panic, clippy::too_many_lines)]
use super::*;
use crate::{AttachReply, BackendError, UnavailableBackend};
use serde_json::{Value, json};
use tungstenite::{Message as WireMessage, client::IntoClientRequest};
use worldstream_protocol::{
    ExternalInputIngressRequestV1, ExternalInputIngressResponseV1, RoomAttached, SyncBranch,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OTHER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const HIDDEN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";

#[derive(Default)]
struct DurableRoom {
    receipts: HashMap<String, ExternalInputIngressResponseV1>,
    frames: Vec<ObservationDeliver>,
}
#[derive(Default)]
struct IngressBackend {
    rooms: Mutex<HashMap<String, DurableRoom>>,
    calls: AtomicUsize,
    ack_calls: AtomicUsize,
    block_room: Mutex<Option<String>>,
    block_all: AtomicBool,
    released: AtomicBool,
    share: AtomicBool,
    sessions: Mutex<Vec<String>>,
    send_failures: Arc<Mutex<HashMap<String, bool>>>,
}
use std::collections::HashMap;
impl IngressBackend {
    fn head(room: &str, seq: u64) -> RoomHead {
        operator_room_fixture(room, seq).room_head
    }
}
impl GatewayBackend for IngressBackend {
    fn admission_principal(&self, _: &GatewaySession) -> Result<String, BackendError> {
        Ok("host".into())
    }
    fn hello(
        &self,
        session: &GatewaySession,
        hello: &ClientHello,
    ) -> Result<ServerWelcome, BackendError> {
        self.sessions
            .lock()
            .unwrap()
            .push(session.session_id().to_string());
        NestedRuntimeBackend.hello(session, hello)
    }
    fn create_room(
        &self,
        session: &GatewaySession,
        request: CreateRoomRequest,
    ) -> Result<CreateRoomResponse, BackendError> {
        UnavailableBackend.create_room(session, request)
    }
    fn projection(
        &self,
        session: &GatewaySession,
        room: &str,
    ) -> Result<ProjectionResponse, BackendError> {
        UnavailableBackend.projection(session, room)
    }
    fn observation_ack(
        &self,
        session: &GatewaySession,
        request: ObservationAck,
    ) -> Result<Option<u64>, BackendError> {
        self.ack_calls.fetch_add(1, Ordering::AcqRel);
        UnavailableBackend.observation_ack(session, request)
    }
    fn action(
        &self,
        session: &GatewaySession,
        request: ActionSubmit,
    ) -> Result<crate::ActionReply, BackendError> {
        UnavailableBackend.action(session, request)
    }
    fn attach(&self, _: &GatewaySession, request: RoomAttach) -> Result<AttachReply, BackendError> {
        let state = self.rooms.lock().unwrap();
        let room = state.get(&request.room_id);
        let seq = room.map_or(0, |r| r.receipts.len() as u64);
        let frame_head = if request.member_id == HIDDEN {
            0
        } else {
            room.map_or(0, |r| r.frames.len() as u64)
        };
        Ok(AttachReply {
            attached: RoomAttached {
                room_id: request.room_id.clone(),
                member_id: request.member_id,
                principal_kind: PrincipalKind::Human,
                access_mode: AccessMode::Participant,
                role: Some("reader".into()),
                membership_status: "enabled".into(),
                room_status: "active".into(),
                room_health: "healthy".into(),
                integrity_generation: 1,
                room_head: Self::head(&request.room_id, seq),
                cursor: None,
                frame_head,
                retained_floor: 1,
                sync_token: "barrier".into(),
                sync: SyncBranch::RetainedFrames {
                    cursor_exclusive: frame_head,
                    through_frame_head: frame_head,
                },
                pack: PackReference {
                    id: "test".into(),
                    version: "1".into(),
                    digest: "blake3:pack".into(),
                },
            },
            reset: None,
            frames: vec![],
        })
    }
    fn sync_ack(
        &self,
        _: &GatewaySession,
        request: RoomSyncAck,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        if request.sync_token != "barrier" {
            return Err(BackendError::Rejected);
        }
        Ok(vec![])
    }
    fn live_observation_suffix(
        &self,
        _: &GatewaySession,
        room: &str,
        member: &str,
        after: u64,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.block_all.load(Ordering::Acquire)
            || self.block_room.lock().unwrap().as_deref() == Some(room)
        {
            while !self.released.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if member == HIDDEN {
            return Ok(vec![]);
        }
        Ok(self.rooms.lock().unwrap().get(room).map_or(vec![], |r| {
            r.frames
                .iter()
                .filter(|f| f.frame_seq > after)
                .cloned()
                .map(|mut f| {
                    f.member_id = member.into();
                    f
                })
                .collect()
        }))
    }
    fn supports_shared_live_observation_cut(&self) -> bool {
        self.share.load(Ordering::Acquire)
    }
    fn prepare_live_observation_batch(
        &self,
        recipients: &[crate::LiveObservationRecipient],
        frames: usize,
        bytes: usize,
    ) -> Vec<Result<crate::LiveObservationPreparedPage, BackendError>> {
        if !self.share.load(Ordering::Acquire) {
            return recipients
                .iter()
                .map(|r| {
                    self.live_observation_page(
                        &r.session,
                        &r.room_id,
                        &r.member_id,
                        r.after_frame_seq,
                        frames,
                        bytes,
                    )
                    .map(|page| crate::LiveObservationPreparedPage {
                        frames: page.frames.into(),
                        has_more: page.has_more,
                        fence: None,
                    })
                })
                .collect();
        }
        let mut pages = HashMap::new();
        recipients
            .iter()
            .map(|r| {
                let key = (r.room_id.clone(), r.member_id.clone(), r.after_frame_seq);
                if !pages.contains_key(&key) {
                    let page = self.live_observation_page(
                        &r.session,
                        &r.room_id,
                        &r.member_id,
                        r.after_frame_seq,
                        frames,
                        bytes,
                    )?;
                    pages.insert(
                        key.clone(),
                        (
                            Arc::<[ObservationDeliver]>::from(page.frames),
                            page.has_more,
                        ),
                    );
                }
                let (frames, has_more) = &pages[&key];
                Ok(crate::LiveObservationPreparedPage {
                    frames: Arc::clone(frames),
                    has_more: *has_more,
                    fence: Some(Arc::new(IngressFence {
                        session: r.session.session_id().to_string(),
                        failures: Arc::clone(&self.send_failures),
                        checks: AtomicUsize::new(0),
                    })),
                })
            })
            .collect()
    }
    fn ingest_external_input(
        &self,
        _: &GatewaySession,
        room: &str,
        request: ExternalInputIngressRequestV1,
    ) -> Result<ExternalInputIngressResponseV1, BackendError> {
        if request.payload["reject"] == true {
            return Err(BackendError::Rejected);
        }
        let mut state = self.rooms.lock().unwrap();
        let room_state = state.entry(room.into()).or_default();
        if let Some(receipt) = room_state.receipts.get(&request.input_id) {
            let mut r = receipt.clone();
            r.duplicate = true;
            return Ok(r);
        }
        let seq = room_state.receipts.len() as u64 + 1;
        let receipt = ExternalInputIngressResponseV1 {
            version: request.version,
            room_id: room.into(),
            source_id: request.source_id,
            input_id: request.input_id.clone(),
            input_type: request.input_type,
            recorded_at: "2026-09-12T15:03:00Z".into(),
            transition_id: request.input_id.clone(),
            room_head: Self::head(room, seq),
            duplicate: false,
        };
        if request.payload["hidden"] != true {
            room_state.frames.push(ObservationDeliver {
                room_id: room.into(),
                member_id: MEMBER.into(),
                frame_seq: room_state.frames.len() as u64 + 1,
                cause_room_seq: seq,
                frame_kind: "transition".into(),
                observation_schema: "test/v1".into(),
                observation: request.payload,
                frame_payload_hash: "hash".into(),
            });
        }
        room_state
            .receipts
            .insert(request.input_id, receipt.clone());
        Ok(receipt)
    }
}

struct IngressFence {
    session: String,
    failures: Arc<Mutex<HashMap<String, bool>>>,
    checks: AtomicUsize,
}
impl crate::LiveObservationFence for IngressFence {
    fn revalidate(&self, session: &GatewaySession) -> Result<(), BackendError> {
        assert_eq!(session.session_id().as_str(), self.session);
        if self.checks.fetch_add(1, Ordering::AcqRel) >= 1 {
            if let Some(reset) = self.failures.lock().unwrap().get(&self.session) {
                return Err(if *reset {
                    BackendError::ResetRequired
                } else {
                    BackendError::Rejected
                });
            }
        }
        Ok(())
    }
}

type Socket = tungstenite::WebSocket<TcpStream>;
fn send(socket: &mut Socket, kind: &str, body: Value) {
    let envelope = worldstream_protocol::VersionedEnvelope {
        protocol: "0.1".into(),
        message_type: kind.into(),
        message_id: "01ARZ3NDEKTSV4RRFFQ69G5FD0".parse().unwrap(),
        request_id: None,
        body,
    };
    socket
        .send(WireMessage::Text(
            serde_json::to_string(&envelope).unwrap().into(),
        ))
        .unwrap();
}

fn receive(socket: &mut Socket) -> Value {
    loop {
        match socket.read().unwrap() {
            WireMessage::Text(text) => {
                let message: Value = serde_json::from_str(&text).unwrap();
                if message["type"] == "server.ping" {
                    send(socket, "client.pong", json!({}));
                    continue;
                }
                return message;
            }
            WireMessage::Ping(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
fn attach(socket: &mut Socket, room: &str, member: &str) {
    send(
        socket,
        "room.attach",
        json!({"room_id":room,"member_id":member,"after_frame_seq":null}),
    );
    let attached = receive(socket);
    assert_eq!(attached["type"], "room.attached");
    send(
        socket,
        "room.sync_ack",
        json!({"room_id":room,"member_id":member,"through_frame_head":attached["body"]["frame_head"],"sync_token":attached["body"]["sync_token"]}),
    );
    assert_eq!(receive(socket)["type"], "room.sync_acked");
}
fn connect(address: SocketAddr, room: &str, member: &str) -> Socket {
    let mut request = format!("ws://{address}/v1/stream")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert("Authorization", auth_header());
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(worldstream_protocol::WEBSOCKET_SUBPROTOCOL),
    );
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (mut socket, _) = tungstenite::client(request, stream).unwrap();
    send(
        &mut socket,
        "client.hello",
        json!({"client_name":"ingress-test","client_version":"test","mode":"participant","supported_protocols":["0.1"],"capabilities":worldstream_protocol::REQUIRED_CLIENT_CAPABILITIES}),
    );
    assert_eq!(receive(&mut socket)["type"], "server.welcome");
    attach(&mut socket, room, member);
    socket
}
fn input(id: &str, basis: u64, payload: Value) -> ExternalInputIngressRequestV1 {
    ExternalInputIngressRequestV1 {
        version: worldstream_protocol::EXTERNAL_INPUT_INGRESS_REQUEST_VERSION.into(),
        source_id: worldstream_core::EXTERNAL_INPUT_INGRESS_SOURCE_ID.into(),
        input_id: id.into(),
        input_type: worldstream_core::EXTERNAL_INPUT_INGRESS_TYPE.into(),
        based_on_room_seq: basis,
        pack_digest: "blake3:pack".into(),
        payload,
        recorded_at: None,
    }
}
fn ingress(address: SocketAddr, room: &str, input: &ExternalInputIngressRequestV1) -> (u16, Value) {
    let body = serde_json::to_string(input).unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(stream, "POST /v1/rooms/{room}/external-input HTTP/1.1\r\nHost: {address}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", auth_header().to_str().unwrap(), body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}
fn quiet(socket: &mut Socket) {
    socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    loop {
        match socket.read() {
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Ok(WireMessage::Text(text))
                if serde_json::from_str::<Value>(&text).unwrap()["type"] == "server.ping" =>
            {
                send(socket, "client.pong", json!({}))
            }
            other => panic!("unexpected frame on quiet stream: {other:?}"),
        }
    }
    socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
}

#[tokio::test]
async fn external_input_http_publishes_to_live_websockets_and_recovers_durable_duplicate() {
    let backend = Arc::new(IngressBackend::default());
    let app = operator_router(
        OperatorState::new(EffectiveConfig::default())
            .unwrap()
            .with_backend(backend.clone()),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    tokio::task::spawn_blocking(move || {
        let mut first = connect(address, ROOM, MEMBER);
        let mut second = connect(address, ROOM, MEMBER);
        let mut hidden = connect(address, ROOM, HIDDEN);
        let mut other = connect(address, OTHER, MEMBER);
        let accepted = input("01ARZ3NDEKTSV4RRFFQ69G5FD1", 0, json!({"private":"first"}));
        assert_eq!(ingress(address, ROOM, &accepted).0, 200);
        for socket in [&mut first, &mut second] {
            let frame = receive(socket);
            assert_eq!(frame["type"], "observation.deliver");
            assert_eq!(frame["body"]["frame_seq"], 1);
            assert_eq!(frame["body"]["observation"]["private"], "first");
        }
        quiet(&mut hidden);
        quiet(&mut other);
        assert_eq!(ingress(address, ROOM, &accepted).1["duplicate"], true);
        quiet(&mut first);
        quiet(&mut second);
        // Commit without the gateway notification. A matching HTTP retry must
        // recover this exact durable frame without another canonical mutation.
        let recovered = input(
            "01ARZ3NDEKTSV4RRFFQ69G5FD2",
            1,
            json!({"private":"recovered"}),
        );
        backend
            .ingest_external_input(
                &GatewaySession::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FF0".parse().unwrap(),
                    CapabilityBearerV1::from_bytes([0x5a; 32]),
                ),
                ROOM,
                recovered.clone(),
            )
            .unwrap();
        assert_eq!(ingress(address, ROOM, &recovered).1["duplicate"], true);
        for socket in [&mut first, &mut second] {
            let frame = receive(socket);
            assert_eq!(frame["body"]["frame_seq"], 2);
            assert_eq!(frame["body"]["cause_room_seq"], 2);
        }
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input("01ARZ3NDEKTSV4RRFFQ69G5FD3", 2, json!({"hidden":true}))
            )
            .0,
            200
        );
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input("01ARZ3NDEKTSV4RRFFQ69G5FD4", 3, json!({"reject":true}))
            )
            .0,
            400
        );
        quiet(&mut first);
        quiet(&mut second);
        quiet(&mut hidden);
        quiet(&mut other);
        attach(&mut first, OTHER, MEMBER);
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input(
                    "01ARZ3NDEKTSV4RRFFQ69G5FD5",
                    3,
                    json!({"private":"old-room"})
                )
            )
            .0,
            200
        );
        assert_eq!(receive(&mut second)["body"]["frame_seq"], 3);
        quiet(&mut first);
        assert_eq!(
            ingress(
                address,
                OTHER,
                &input(
                    "01ARZ3NDEKTSV4RRFFQ69G5FD6",
                    0,
                    json!({"private":"new-room"})
                )
            )
            .0,
            200
        );
        for socket in [&mut first, &mut other] {
            let frame = receive(socket);
            assert_eq!(frame["body"]["room_id"], OTHER);
            assert_eq!(frame["body"]["frame_seq"], 1);
        }
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
}

struct ReleaseReads(Arc<IngressBackend>);
impl Drop for ReleaseReads {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Release);
    }
}
async fn serve_ingress(
    backend: Arc<IngressBackend>,
) -> (SocketAddr, tokio::task::JoinHandle<std::io::Result<()>>) {
    let mut state = OperatorState::new(EffectiveConfig::default())
        .unwrap()
        .with_backend(backend);
    state.rate_limiter =
        crate::rate_limit::GatewayRateLimiter::fixed_dimension_for_integration_tests(
            crate::rate_limit::AdmissionDimension::Ip,
            5000,
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = operator_router(state);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    (address, server)
}
fn fixture_host() -> GatewaySession {
    GatewaySession::new(
        "01ARZ3NDEKTSV4RRFFQ69G5FF0".parse().unwrap(),
        CapabilityBearerV1::from_bytes([0x5a; 32]),
    )
}

#[tokio::test]
async fn publication_recovers_missed_notification_without_retry_or_next_mutation() {
    let backend = Arc::new(IngressBackend::default());
    let (address, server) = serve_ingress(backend.clone()).await;
    tokio::task::spawn_blocking(move || {
        let mut socket = connect(address, ROOM, MEMBER);
        let request = input(
            "01ARZ3NDEKTSV4RRFFQ69G5FD1",
            0,
            json!({"private":"unnotified"}),
        );
        backend
            .ingest_external_input(&fixture_host(), ROOM, request)
            .unwrap();
        // There is no HTTP retry and no subsequent canonical mutation.
        let frame = receive(&mut socket);
        assert_eq!(frame["type"], "observation.deliver");
        assert_eq!(frame["body"]["frame_seq"], 1);
        assert_eq!(frame["body"]["observation"]["private"], "unnotified");
        quiet(&mut socket);
        assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn publication_coalesces_burst_and_keeps_every_frame_in_order() {
    let backend = Arc::new(IngressBackend::default());
    *backend.block_room.lock().unwrap() = Some(ROOM.into());
    let release = ReleaseReads(backend.clone());
    let (address, server) = serve_ingress(backend.clone()).await;
    tokio::task::spawn_blocking(move || {
        let mut socket = connect(address, ROOM, MEMBER);
        for seq in 1..=100u64 {
            let request = input(
                crate::next_ulid().unwrap().as_str(),
                seq - 1,
                json!({"seq":seq}),
            );
            assert_eq!(ingress(address, ROOM, &request).0, 200);
        }
        // One retained in-flight read per Room coalesces all these triggers.
        assert_eq!(backend.calls.load(Ordering::Acquire), 1);
        backend.released.store(true, Ordering::Release);
        for seq in 1..=100u64 {
            let frame = receive(&mut socket);
            assert_eq!(frame["body"]["frame_seq"], seq);
            assert_eq!(frame["body"]["observation"]["seq"], seq);
        }
        quiet(&mut socket);
        assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
    })
    .await
    .unwrap();
    drop(release);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn publication_pending_overflow_rediscovers_every_live_room() {
    let backend = Arc::new(IngressBackend::default());
    let release = ReleaseReads(backend.clone());
    let (address, server) = serve_ingress(backend.clone()).await;
    tokio::task::spawn_blocking(move || {
        let mut sockets = Vec::new();
        // More Rooms than all pending and running slots combined.
        for _ in 0..80 {
            let room = crate::next_ulid().unwrap().to_string();
            let socket = connect(address, &room, MEMBER);
            sockets.push((room, socket));
        }
        backend.block_all.store(true, Ordering::Release);
        for (room, _) in &sockets {
            assert_eq!(
                ingress(
                    address,
                    room,
                    &input(
                        crate::next_ulid().unwrap().as_str(),
                        0,
                        json!({"room":room})
                    )
                )
                .0,
                200
            );
        }
        backend.released.store(true, Ordering::Release);
        for (room, socket) in &mut sockets {
            socket
                .get_mut()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let frame = receive(socket);
            assert_eq!(frame["body"]["room_id"], *room);
            assert_eq!(frame["body"]["frame_seq"], 1);
        }
        assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
    })
    .await
    .unwrap();
    drop(release);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn publication_timeout_retains_one_stalled_read_and_healthy_room_progresses() {
    let backend = Arc::new(IngressBackend::default());
    *backend.block_room.lock().unwrap() = Some(ROOM.into());
    let release = ReleaseReads(backend.clone());
    let (address, server) = serve_ingress(backend.clone()).await;
    tokio::task::spawn_blocking(move || {
        let mut stalled = connect(address, ROOM, MEMBER);
        let mut healthy = connect(address, OTHER, MEMBER);
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input(
                    crate::next_ulid().unwrap().as_str(),
                    0,
                    json!({"room":"stalled"})
                )
            )
            .0,
            200
        );
        std::thread::sleep(Duration::from_millis(1200));
        let before = Instant::now();
        assert_eq!(
            ingress(
                address,
                OTHER,
                &input(
                    crate::next_ulid().unwrap().as_str(),
                    0,
                    json!({"room":"healthy"})
                )
            )
            .0,
            200
        );
        assert_eq!(
            receive(&mut healthy)["body"]["observation"]["room"],
            "healthy"
        );
        assert!(before.elapsed() < Duration::from_secs(1));
        backend.released.store(true, Ordering::Release);
        assert_eq!(receive(&mut stalled)["body"]["frame_seq"], 1);
        assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
    })
    .await
    .unwrap();
    drop(release);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn publication_shared_page_reuses_bodies_with_private_individual_envelopes() {
    let backend = Arc::new(IngressBackend::default());
    backend.share.store(true, Ordering::Release);
    let (address, server) = serve_ingress(backend.clone()).await;
    tokio::task::spawn_blocking(move || {
        let mut first = connect(address, ROOM, MEMBER);
        let mut second = connect(address, ROOM, MEMBER);
        let mut hidden = connect(address, ROOM, HIDDEN);
        std::thread::sleep(Duration::from_millis(20));
        let reads = backend.calls.load(Ordering::Acquire);
        let bodies = crate::SHARED_BODY_PREPARATIONS.load(Ordering::Acquire);
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input("01ARZ3NDEKTSV4RRFFQ69G5FH1", 0, json!({"private":"shared"}))
            )
            .0,
            200
        );
        let a = receive(&mut first);
        let b = receive(&mut second);
        assert_eq!(a["body"], b["body"]);
        assert_ne!(a["message_id"], b["message_id"]);
        assert_eq!(
            crate::SHARED_BODY_PREPARATIONS.load(Ordering::Acquire) - bodies,
            1
        );
        assert!(backend.calls.load(Ordering::Acquire) - reads <= 2);
        quiet(&mut hidden);
        quiet(&mut first);
        quiet(&mut second);
        assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
        // A replacement registration resumes independently. The other Session
        // remains at its own delivery position and sees no duplicate.
        attach(&mut second, ROOM, MEMBER);
        assert_eq!(
            ingress(
                address,
                ROOM,
                &input("01ARZ3NDEKTSV4RRFFQ69G5FH2", 1, json!({"private":"next"}))
            )
            .0,
            200
        );
        assert_eq!(receive(&mut first)["body"]["frame_seq"], 2);
        assert_eq!(receive(&mut second)["body"]["frame_seq"], 2);
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn publication_shared_page_rechecks_each_session_before_send() {
    for reset in [false, true] {
        let backend = Arc::new(IngressBackend::default());
        backend.share.store(true, Ordering::Release);
        let (address, server) = serve_ingress(backend.clone()).await;
        tokio::task::spawn_blocking(move || {
            let mut denied = connect(address, ROOM, MEMBER);
            let mut allowed = connect(address, ROOM, MEMBER);
            let denied_id = backend.sessions.lock().unwrap()[0].clone();
            backend
                .send_failures
                .lock()
                .unwrap()
                .insert(denied_id, reset);
            assert_eq!(
                ingress(
                    address,
                    ROOM,
                    &input("01ARZ3NDEKTSV4RRFFQ69G5FJ1", 0, json!({"private":"fenced"}))
                )
                .0,
                200
            );
            assert_eq!(receive(&mut denied)["type"], "error");
            let delivered = receive(&mut allowed);
            assert_eq!(delivered["type"], "observation.deliver");
            assert_eq!(delivered["body"]["frame_seq"], 1);
            quiet(&mut allowed);
            assert_eq!(backend.ack_calls.load(Ordering::Acquire), 0);
        })
        .await
        .unwrap();
        server.abort();
        let _ = server.await;
    }
}
