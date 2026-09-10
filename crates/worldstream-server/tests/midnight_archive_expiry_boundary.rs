//! Expiry qualification through the retained Component, SQLite, HTTP and WS boundaries.
use std::{
    fs,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tungstenite::{Message, WebSocket, client::IntoClientRequest as _};
use worldstream_component_host::ComponentPackHostV1;
use worldstream_core::{
    AuthorityBootstrapV1, AuthorityV1, CapabilityBearerV1, HostClockErrorV1, HostClockSampleV1,
    HostClockV1, PackRegistryStatusV1, PackRegistryV1, PrincipalKindV1, builtin_counter_registry,
};
use worldstream_pack_bundle::PackBundleVerifierV1;
use worldstream_protocol::BearerWireV1;
use worldstream_runtime::EffectiveConfig;
use worldstream_server::{
    GatewayBackend as _, OperatorState, SqliteGatewayBackend, operator_router,
};
use worldstream_sqlite::SqliteRoomStore;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const SESSION_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA5";
const HOST: &str = "01ARZ3NDEKTSV4RRFFQ69G5G11";
// Portable Component work can add local build latency between boundary steps.
// This bounds test transport/gate waits without changing any product deadline.
const BOUNDARY_WAIT: Duration = Duration::from_secs(180);
static QUALIFICATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SampleGate {
    sampled: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

struct Clock {
    now: Mutex<HostClockSampleV1>,
    gate: Mutex<Option<SampleGate>>,
    observed: Mutex<Option<mpsc::Sender<()>>>,
}

impl HostClockV1 for Clock {
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
        let now = self
            .now
            .lock()
            .map_err(|_| HostClockErrorV1::Unavailable)?
            .clone();
        let gate = self
            .gate
            .lock()
            .map_err(|_| HostClockErrorV1::Unavailable)?
            .take();
        if let Some(gate) = gate {
            gate.sampled
                .send(())
                .map_err(|_| HostClockErrorV1::Unavailable)?;
            gate.release
                .recv_timeout(BOUNDARY_WAIT)
                .map_err(|_| HostClockErrorV1::Unavailable)?;
        } else if let Some(observer) = self
            .observed
            .lock()
            .map_err(|_| HostClockErrorV1::Unavailable)?
            .take()
        {
            let _ = observer.send(());
        }
        Ok(now)
    }
}

impl Clock {
    fn set(&self, at: time::OffsetDateTime) -> TestResult {
        *self.now.lock().map_err(|_| "clock poisoned")? =
            HostClockSampleV1::new(at.format(&time::format_description::well_known::Rfc3339)?)?;
        Ok(())
    }
}

struct Runtime {
    routes: Router,
    backend: Arc<SqliteGatewayBackend>,
    server: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    address: SocketAddr,
}

impl Runtime {
    async fn open(
        path: &Path,
        registry: Arc<PackRegistryV1>,
        clock: Arc<Clock>,
    ) -> TestResult<Self> {
        let backend = Arc::new(
            SqliteGatewayBackend::with_host_clock(SqliteRoomStore::open(path)?, registry, clock)
                .with_timer_authority(CapabilityBearerV1::from_bytes([0xc1; 32]))?,
        );
        // Invoke the production scheduler explicitly so each contender is
        // controlled by recorded clock samples, without a background tick race.
        backend.scheduler_tick()?;
        let routes = operator_router(
            OperatorState::new(EffectiveConfig::default())?.with_backend(backend.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let serving = routes.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                serving.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });
        Ok(Self {
            routes,
            backend,
            server,
            address,
        })
    }

    async fn close(self) {
        self.server.abort();
        let _ = self.server.await;
        drop(self.routes);
        drop(self.backend);
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    registry: Arc<PackRegistryV1>,
    clock: Arc<Clock>,
    runtime: Runtime,
    room: String,
    member: String,
    bearer: String,
    host: String,
    agent: (String, String),
    plan_schema: Value,
}

async fn request(
    routes: &Router,
    bearer: &str,
    method: &str,
    path: &str,
    body: Value,
) -> TestResult<(u16, Value)> {
    let response = routes
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {bearer}"))
                .header("origin", "https://arena.example")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((status, serde_json::from_slice(&bytes)?))
}

async fn successful(
    routes: &Router,
    bearer: &str,
    method: &str,
    path: &str,
    body: Value,
) -> TestResult<Value> {
    let (status, value) = request(routes, bearer, method, path, body).await?;
    assert!((200..300).contains(&status), "{path}: {status} {value}");
    Ok(value)
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let proof: Value = serde_json::from_slice(&fs::read(
            workspace
                .join("packs/midnight-archive/evidence/production-proof-0.1.0-session-expiry.json"),
        )?)?;
        let bundle = std::env::var_os("WORLDSTREAM_ARCHIVE_EXPIRY_BUNDLE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                workspace.join(format!(
                    "packs/midnight-archive/releases/0.1.0/worldstream-midnight-archive-{}.wspack",
                    proof["bundleDigest"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("blake3:")
                ))
            });
        let verified = PackBundleVerifierV1.inspect(Arc::<[u8]>::from(fs::read(bundle)?))?;
        let revision = verified.revision_digest().to_string();
        let admission = ComponentPackHostV1::new()?.admit(
            verified,
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
                approved_for_activity_start: true,
            },
        )?;
        let registry = Arc::new(builtin_counter_registry()?.admit_portable([admission])?);
        let revision_digest = revision.parse()?;
        let retained = registry.load_retained(&revision_digest)?;
        let plan_reference = &retained
            .descriptor()
            .actions
            .iter()
            .find(|action| action.action_type == "submit_companion_plan")
            .ok_or("plan action absent")?
            .payload_schema;
        let plan_schema = serde_json::from_slice(
            &registry
                .resolve_schema(&revision_digest, plan_reference)?
                .to_bytes()?,
        )?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("expiry.sqlite");
        let store = SqliteRoomStore::open(&path)?;
        AuthorityV1::new(Arc::new(store.clone())).bootstrap(
            AuthorityBootstrapV1::new(
                HOST.parse()?,
                "01ARZ3NDEKTSV4RRFFQ69G5G12".parse()?,
                PrincipalKindV1::Human,
                "01ARZ3NDEKTSV4RRFFQ69G5G13".parse()?,
                CapabilityBearerV1::from_bytes([0xc1; 32]).token_hash(),
                None,
            )?,
            "2026-08-15T12:00:00Z".parse()?,
        )?;
        drop(store);
        let clock = Arc::new(Clock {
            now: Mutex::new(HostClockSampleV1::new(
                time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)?,
            )?),
            gate: Mutex::new(None),
            observed: Mutex::new(None),
        });
        let runtime = Runtime::open(&path, registry.clone(), clock.clone()).await?;
        let host = BearerWireV1::from_bytes([0xc1; 32]).to_wire();
        let created = successful(&runtime.routes, &host, "POST", "/v1/rooms", json!({
            "pack": {"id":"worldstream.midnight-archive","version":"0.1.0","digest":revision},
            "configuration":{"scenario_id":"standard-v1"}, "idempotency_key":"expiry-boundary",
                "members":[{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G14","principal_kind":"human","role":"lead","access_mode":"participant"},
                    {"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G17","principal_kind":"agent","role":"jonah","access_mode":"participant"}]
        })).await?;
        let room = created["room_id"].as_str().unwrap().to_owned();
        let member = created["member_ids"][0].as_str().unwrap().to_owned();
        let capability = successful(&runtime.routes, &host, "POST", "/v1/operator/member-capabilities", json!({
            "room_id":room,"member_id":member,"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G14",
            "scopes":["room:attach","room:observe_member","room:act","room:replay"],"idempotency_key":"01ARZ3NDEKTSV4RRFFQ69G5G15","expires_at":null
        })).await?;
        let bearer = capability["bearer"].as_str().unwrap().to_owned();
        let agent_member = created["member_ids"][1].as_str().unwrap().to_owned();
        let agent_capability = successful(
            &runtime.routes,
            &host,
            "POST",
            "/v1/operator/member-capabilities",
            json!({
                "room_id":room,"member_id":agent_member,"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5G17",
                "scopes":["room:attach","room:observe_member","room:act","room:replay"],
                "idempotency_key":"01ARZ3NDEKTSV4RRFFQ69G5G18","expires_at":null
            }),
        )
        .await?;
        let agent = (
            agent_member,
            agent_capability["bearer"].as_str().unwrap().to_owned(),
        );
        successful(
            &runtime.routes,
            &host,
            "POST",
            &format!("/v1/operator/rooms/{room}/lobby/launch"),
            json!({
                "input_id":"01ARZ3NDEKTSV4RRFFQ69G5G16","based_on_room_seq":0,"pack_digest":revision
            }),
        )
        .await?;
        clock.set(time::OffsetDateTime::now_utc() + time::Duration::seconds(1))?;
        Ok(Self {
            directory,
            registry,
            clock,
            runtime,
            room,
            member,
            bearer,
            host,
            agent,
            plan_schema,
        })
    }

    async fn view(&self) -> TestResult<Value> {
        successful(
            &self.runtime.routes,
            &self.bearer,
            "GET",
            &format!("/v1/rooms/{}/projection", self.room),
            Value::Null,
        )
        .await
    }

    async fn connect(&self) -> TestResult<Client> {
        let (address, bearer, room, member) = (
            self.runtime.address,
            self.bearer.clone(),
            self.room.clone(),
            self.member.clone(),
        );
        tokio::task::spawn_blocking(move || Client::connect(address, &bearer, room, member)).await?
    }
}

struct Client {
    socket: WebSocket<TcpStream>,
    room: String,
    member: String,
    sequence: u64,
    next: u64,
}
impl Client {
    fn send(&mut self, kind: &str, body: Value) -> TestResult {
        self.next += 1;
        self.socket.send(Message::Text(json!({"protocol":"0.1","type":kind,"message_id":format!("{:026X}", self.next),"body":body}).to_string().into()))?;
        Ok(())
    }
    fn receive(&mut self, kind: &str) -> TestResult<Value> {
        loop {
            if let Message::Text(text) = self.socket.read()? {
                let value: Value = serde_json::from_str(&text)?;
                if value["type"] == kind {
                    return Ok(value["body"].clone());
                }
                assert_ne!(value["type"], "error", "{value}");
                if kind == "action.accepted" {
                    assert_ne!(value["type"], "action.rejected", "{value}");
                }
            }
        }
    }
    fn connect(
        address: SocketAddr,
        bearer: &str,
        room: String,
        member: String,
    ) -> TestResult<Self> {
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(BOUNDARY_WAIT))?;
        let mut request = format!("ws://{address}/v1/stream").into_client_request()?;
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {bearer}").parse()?);
        request.headers_mut().insert(
            "sec-websocket-protocol",
            worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?,
        );
        let (socket, _) = tungstenite::client(request, stream)?;
        let mut client = Self {
            socket,
            room,
            member,
            sequence: 0,
            next: 500,
        };
        client.send("client.hello", json!({"client_name":"expiry-boundary","client_version":"1","mode":"participant","supported_protocols":[worldstream_protocol::PROTOCOL_VERSION],"capabilities":["cursor_ack","projection_reset"]}))?;
        client.receive("server.welcome")?;
        client.send(
            "room.attach",
            json!({"room_id":client.room,"member_id":client.member,"after_frame_seq":null}),
        )?;
        let attached = client.receive("room.attached")?;
        client.sequence = attached["room_head"]["room_seq"].as_u64().unwrap();
        client.receive("projection.reset")?;
        client.send("room.sync_ack", json!({"room_id":client.room,"member_id":client.member,"through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}))?;
        client.receive("room.sync_acked")?;
        Ok(client)
    }
    fn submit(&mut self, action: &str, payload: Value, accepted: bool) -> TestResult<Value> {
        self.next += 1;
        self.send("action.submit", json!({"room_id":self.room,"member_id":self.member,"action_id":format!("{:026X}",self.next),"based_on_room_seq":self.sequence,"action_type":action,"payload":payload}))?;
        let expected = if accepted {
            "action.accepted"
        } else {
            "action.rejected"
        };
        let response = self.receive(expected).map_err(|error| {
            format!(
                "{action} at Room Head {} waiting for {expected}: {error}",
                self.sequence
            )
        })?;
        if accepted {
            self.sequence = response["room_head"]["room_seq"].as_u64().unwrap();
        }
        Ok(response)
    }
}

fn activity(view: &Value) -> &Value {
    &view["projection"]["activity"]
}
fn deadline(view: &Value) -> TestResult<time::OffsetDateTime> {
    Ok(time::OffsetDateTime::parse(
        activity(view)["session_deadline"]
            .as_str()
            .ok_or("session deadline missing")?,
        &time::format_description::well_known::Rfc3339,
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overdue_session_survives_runtime_restart_and_duplicate_timer_requests() -> TestResult {
    let _serial = QUALIFICATION.lock().await;
    let mut fixture = Fixture::new().await?;
    let mut client = fixture.connect().await?;
    tokio::task::spawn_blocking(move || client.submit("stage_wait", json!({}), true)).await??;
    let before = fixture.view().await?;
    fixture.runtime.close().await;
    fixture
        .clock
        .set(deadline(&before)? + time::Duration::minutes(10))?;
    fixture.runtime = Runtime::open(
        &fixture.directory.path().join("expiry.sqlite"),
        fixture.registry.clone(),
        fixture.clock.clone(),
    )
    .await?;
    let after = fixture.view().await?;
    assert_eq!(activity(&after)["phase"], "expired");
    assert_eq!(activity(&after)["outcome"], Value::Null);
    assert_eq!(
        after["room_head"]["room_seq"].as_u64(),
        before["room_head"]["room_seq"].as_u64().map(|seq| seq + 1)
    );
    assert_eq!(
        activity(&after)["turns_used"],
        activity(&before)["turns_used"]
    );
    let path = format!("/v1/operator/rooms/{}/timers/fire", fixture.room);
    let first = successful(
        &fixture.runtime.routes,
        &fixture.host,
        "POST",
        &path,
        json!({"timer_id":SESSION_TIMER,"generation":1}),
    )
    .await?;
    let duplicate = successful(
        &fixture.runtime.routes,
        &fixture.host,
        "POST",
        &path,
        json!({"timer_id":SESSION_TIMER,"generation":1}),
    )
    .await?;
    assert_eq!(first, duplicate);
    let (status, _) = request(
        &fixture.runtime.routes,
        &fixture.host,
        "POST",
        &path,
        json!({"timer_id":SESSION_TIMER,"generation":2}),
    )
    .await?;
    assert_eq!(status, 404);
    fixture.runtime.backend.scheduler_tick()?;
    assert_eq!(fixture.view().await?, after);
    let replay = successful(
        &fixture.runtime.routes,
        &fixture.bearer,
        "GET",
        &format!(
            "/v1/rooms/{}/replay?at_room_seq={}",
            fixture.room, after["room_head"]["room_seq"]
        ),
        Value::Null,
    )
    .await?;
    assert_eq!(activity(&replay), activity(&after));
    let _reentered = fixture.connect().await?;
    fixture.runtime.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extraction_admission_contends_with_expiry_without_overwriting_either_terminal()
-> TestResult {
    let _serial = QUALIFICATION.lock().await;
    for action_first in [true, false] {
        let fixture = Fixture::new().await?;
        let mut client = fixture.connect().await?;
        client = tokio::task::spawn_blocking(move || -> TestResult<Client> {
            client.submit("stage_extract", json!({}), true)?;
            client.submit("prepare_extraction", json!({}), true)?;
            Ok(client)
        })
        .await??;
        let prepared = fixture.view().await?;
        let preview = activity(&prepared)["extraction"]["revision"].clone();
        client = tokio::task::spawn_blocking(move || -> TestResult<Client> {
            client.submit(
                "acknowledge_extraction",
                json!({"preview_revision":preview,"left_behind_roles":[]}),
                true,
            )?;
            Ok(client)
        })
        .await??;
        let before = fixture.view().await?;
        let due = deadline(&before)?;
        if action_first {
            fixture.clock.set(due - time::Duration::nanoseconds(1))?;
            let (sampled_tx, sampled_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            *fixture.clock.gate.lock().unwrap() = Some(SampleGate {
                sampled: sampled_tx,
                release: release_rx,
            });
            let action =
                tokio::task::spawn_blocking(move || client.submit("commit_turn", json!({}), true));
            sampled_rx.recv_timeout(BOUNDARY_WAIT)?;
            // The Action holds a real admission reservation with a pre-deadline
            // sample while the due scheduler competes for the same Room lane.
            fixture.clock.set(due)?;
            let (timer_sampled_tx, timer_sampled_rx) = mpsc::channel();
            *fixture.clock.observed.lock().unwrap() = Some(timer_sampled_tx);
            let backend = fixture.runtime.backend.clone();
            let timer = tokio::task::spawn_blocking(move || backend.scheduler_tick());
            timer_sampled_rx.recv_timeout(BOUNDARY_WAIT)?;
            release_tx.send(())?;
            action.await??;
            timer.await??;
        } else {
            // At the deadline, the competing Action is ineligible regardless
            // of which thread reaches SQLite first; only expiry can commit.
            fixture.clock.set(due)?;
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let action_barrier = barrier.clone();
            let action = tokio::task::spawn_blocking(move || {
                action_barrier.wait();
                client.submit("commit_turn", json!({}), false)
            });
            let backend = fixture.runtime.backend.clone();
            let timer = tokio::task::spawn_blocking(move || {
                barrier.wait();
                backend.scheduler_tick()
            });
            action.await??;
            timer.await??;
        }
        let terminal = fixture.view().await?;
        assert_eq!(
            activity(&terminal)["phase"],
            if action_first { "complete" } else { "expired" }
        );
        assert_eq!(
            activity(&terminal)["outcome"],
            if action_first {
                json!({"kind":"no_ledger"})
            } else {
                Value::Null
            }
        );
        assert_eq!(
            activity(&terminal)["turns_used"],
            if action_first { 1 } else { 0 }
        );
        fixture.runtime.backend.scheduler_tick()?;
        assert_eq!(fixture.view().await?, terminal);
        fixture.runtime.close().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_flight_http_provider_completion_cannot_contribute_after_session_expiry() -> TestResult {
    use std::io::{Read as _, Write as _};
    use worldstream_studio_supervisor::house_model::{
        DevelopmentLoopbackOpenRouterProviderPortV1, FileHouseAllowanceLedgerV1,
        HouseAllowancePeriodV1, HouseInvocationIdentityV1, HouseModelExecutorV1,
        HouseProviderCredentialV1, HouseSpendLimitsV1,
    };
    let _serial = QUALIFICATION.lock().await;
    let fixture = Fixture::new().await?;
    let mut lead = fixture.connect().await?;
    lead = tokio::task::spawn_blocking(move || -> TestResult<Client> {
        lead.submit(
            "assign_jonah_task",
            json!({"task_kind":"investigate_records","power_allowance":0}),
            true,
        )?;
        Ok(lead)
    })
    .await??;
    let due = deadline(&fixture.view().await?)?;
    fixture.clock.set(due - time::Duration::seconds(5))?;
    tokio::task::spawn_blocking(move || lead.submit("request_jonah_plan", json!({}), true))
        .await??;
    let agent_view = successful(
        &fixture.runtime.routes,
        &fixture.agent.1,
        "GET",
        &format!("/v1/rooms/{}/projection", fixture.room),
        Value::Null,
    )
    .await?;
    let (index, offer) = agent_view["projection"]["action_offers"]
        .as_array()
        .ok_or("agent offers missing")?
        .iter()
        .enumerate()
        .find(|(_, offer)| offer["action_type"] == "submit_companion_plan")
        .ok_or("plan offer missing")?;
    let offer_id = format!(
        "{}:{index}:{}",
        agent_view["room_head"]["room_seq"],
        offer["payload_schema_digest"]
            .as_str()
            .ok_or("offer schema digest missing")?
    );
    let offers = json!({"schema":"worldstream/assignment-action-offer-list/v1",
        "precondition":{"room_seq":agent_view["room_head"]["room_seq"],
            "head_hash":agent_view["room_head"]["genesis_or_transition_hash"]},
        "offers":[{"action_type":"submit_companion_plan","offer_id":offer_id,
            "payload_schema":{"schema":fixture.plan_schema,"schema_digest":offer["payload_schema_digest"]}}]});
    let projected = activity(&agent_view);
    let proposal = json!({"task_revision":projected["jonah"]["task"]["revision"],
        "opportunity_revision":projected["jonah"]["planning"]["opportunity_revision"],
        "dialogue":"Recommend Records.","steps":[{"step_type":"move","destination":"records","source_id":"none","power_cost":0}]});
    let projection = json!({"phase":projected["phase"],"jonah":{
        "task":projected["jonah"]["task"],"planning":projected["jonah"]["planning"]}});
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let provider = DevelopmentLoopbackOpenRouterProviderPortV1::new(listener.local_addr()?)?;
    let (began_tx, began_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let http = std::thread::spawn(move || -> TestResult {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(BOUNDARY_WAIT))?;
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes)?;
        let separator = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .ok_or("HTTP body missing")?;
        let input: Value = serde_json::from_slice(&bytes[separator + 4..])?;
        let model = input["model"].clone();
        let route = input["provider"]["only"][0].clone();
        began_tx.send(())?;
        release_rx.recv_timeout(BOUNDARY_WAIT)?;
        let body = serde_json::to_vec(&json!({
            "choices":[{"finish_reason":"stop","index":0,"message":{"role":"assistant",
                "content":json!({"offer_id":offer_id,"payload":proposal}).to_string()}}],
            "id":"gen-expiry-boundary","model":model,"object":"chat.completion",
            "openrouter_metadata":{"attempt":1,"attempts":[{"provider":route,"model":model,"status":200}],
                "endpoints":{"available":[{"model":model,"provider":route,"selected":true}],"total":1},
                "requested":model,"strategy":"direct"},
            "usage":{"completion_tokens":1,"cost":0,"prompt_tokens":1,"total_tokens":2}
        }))?;
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        socket.write_all(&body)?;
        Ok(())
    });
    let (_, revisions) =
        worldstream_studio_supervisor::hosted_artifacts::reviewed_hosted_artifacts()?;
    let revision = revisions
        .into_iter()
        .next()
        .ok_or("reviewed House revision missing")?;
    let period = HouseAllowancePeriodV1::from_unix_seconds(
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )?;
    let ledger = FileHouseAllowanceLedgerV1::open_at(
        fixture.directory.path().join("allowance"),
        HouseSpendLimitsV1::hobby_preview(),
        period,
    )?;
    let identity = HouseInvocationIdentityV1::new(
        &fixture.agent.0,
        "expiry-response",
        1,
        &format!(
            "blake3:{}",
            blake3::hash(&serde_json::to_vec(&agent_view)?).to_hex()
        ),
    )?;
    let executor = HouseModelExecutorV1::new(provider, ledger.clone());
    let response = tokio::task::spawn_blocking(move || {
        let credential = HouseProviderCredentialV1::new(zeroize::Zeroizing::new(
            b"sk-test-expiry-boundary-no-real-provider".to_vec(),
        ))?;
        executor.execute(
            &credential,
            &revision,
            &identity,
            &projection,
            &offers,
            period,
        )
    });
    // This notification comes only after the real HTTP request was read. The
    // provider deliberately withholds its response until expiry has committed.
    began_rx.recv_timeout(BOUNDARY_WAIT)?;
    assert_eq!(ledger.usage(&fixture.agent.0)?.active_calls, 1);
    fixture.clock.set(due)?;
    fixture.runtime.backend.scheduler_tick()?;
    let terminal = fixture.view().await?;
    assert_eq!(activity(&terminal)["phase"], "expired");
    assert_eq!(activity(&terminal)["outcome"], Value::Null);
    release_tx.send(())?;
    let completion = response.await??;
    http.join().map_err(|_| "HTTP fixture panicked")??;
    let usage = ledger.usage(&fixture.agent.0)?;
    assert_eq!(usage.attempts, 1);
    assert_eq!(usage.active_calls, 0);
    assert!(usage.consumed_input_units > 0);
    let address = fixture.runtime.address;
    let room = fixture.room.clone();
    let (member, bearer) = fixture.agent.clone();
    let rejected = tokio::task::spawn_blocking(move || {
        let mut agent = Client::connect(address, &bearer, room, member)?;
        agent.submit("submit_companion_plan", completion.action.payload, false)
    })
    .await??;
    assert_eq!(rejected["code"], "action_not_allowed");
    assert_eq!(fixture.view().await?, terminal);
    fixture.runtime.close().await;
    Ok(())
}
