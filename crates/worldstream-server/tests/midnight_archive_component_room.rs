//! Live qualification lanes for solo and companion Midnight Archive Rooms.
//!
//! The test uses the retained reviewed Bundle, the production portable
//! Component Host, and the same SQLite gateway/WebSocket boundary used by the
//! local Runtime.

use std::{
    collections::BTreeMap,
    env, fs,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    HostClockV1, PackRegistryStatusV1, PrincipalKindV1, builtin_counter_registry,
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
    "blake3:0ddcd385d0b250f9ec5a285df1783fbae45a685a90fcf69703026787959b540f";
const CURRENT_REVISION_DIGEST: &str =
    "blake3:d398d13df28f50edcc271aa8f6ffa75f1c26eca1851ac78215aa8e0f6017d8e5";
const CURRENT_COMPONENT_DIGEST: &str =
    "blake3:326eaf7db7474a9484f8e53a6398f111077e8bf4f41d0b4fc79590a2a0ac6d0d";
const RETAINED_CONSECUTIVE_BUNDLE_DIGEST: &str =
    "blake3:b82e0d5df065af1b31252a06e06a3bbd685a5ba63c3d2bfeb71a0d6dd06ebbab";
const RETAINED_CONSECUTIVE_REVISION_DIGEST: &str =
    "blake3:59bb814b920b0eb6f8b8886f2d368052189c0f9263d6c36c91894f639c8e19f5";
const RETAINED_SPECIALIST_BUNDLE_DIGEST: &str =
    "blake3:309721db5290e9f2daf8c092ed97528d8cf7dc2bdcf577edef737f26e14157ea";
const RETAINED_SPECIALIST_REVISION_DIGEST: &str =
    "blake3:894f7a58c01b0ca99ac29b1b84a9083bab7f0f858f0b33ce495413255cf91339";
const RETAINED_MIRA_BUNDLE_DIGEST: &str =
    "blake3:ea79ce7ff3e90ab5d073409486af1227286b82512daec98db3e921097dd8ac99";
const RETAINED_MIRA_REVISION_DIGEST: &str =
    "blake3:ec4689e090f05f1c1894f21c1dba95e1f56b3afc49fc88c5c8f3530a03a80b61";
const RETAINED_AGREEMENT_BUNDLE_DIGEST: &str =
    "blake3:877702b321352288553cc0e5ea6510f1f8dea3e18687759658714ebc09a3c269";
const RETAINED_AGREEMENT_REVISION_DIGEST: &str =
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
        .join("../../config/activity-clients/releases/midnight-archive-web-v6.json");
    let release: Value = serde_json::from_slice(&fs::read(path)?)?;
    let release_digest = release["release_digest"]
        .as_str()
        .ok_or("Archive client Release digest missing")?
        .to_owned();
    let surface_id = release["surfaces"]
        .as_array()
        .and_then(|surfaces| {
            surfaces.iter().find_map(|surface| {
                (surface["entrypoint"] == "/midnight-archive-v6/")
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
        .join("packs/midnight-archive/evidence/production-proof-0.1.0-unavailable-companions.json");
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
    for _ in 0..256 {
        if let Message::Text(text) = socket.read()? {
            let envelope: Value = serde_json::from_str(&text)?;
            reject_private(&envelope["body"]);
            if envelope["type"] == kind {
                return Ok(envelope["body"].clone());
            }
            if envelope["type"] == "error"
                || (kind == "action.accepted" && envelope["type"] == "action.rejected")
            {
                return Err(
                    format!("server rejected expected {kind}: {}", envelope["body"]).into(),
                );
            }
        }
    }
    Err(format!("expected {kind} response missing").into())
}

fn receive_envelope(socket: &mut WebSocket<TcpStream>) -> TestResult<Value> {
    for _ in 0..256 {
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
    run_archive_witness(&[], Scenario::Complete).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mira_assay_and_ordinary_hatch_complete_with_the_starting_crew() -> TestResult {
    run_archive_witness(&["mira"], Scenario::Complete).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jonah_specialist_hatch_completes_with_the_starting_crew() -> TestResult {
    run_archive_witness(&["jonah"], Scenario::Complete).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_crew_resolves_reservations_assay_and_extraction_before_replay() -> TestResult {
    run_archive_witness(&["mira", "jonah"], Scenario::Complete).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_extraction_retains_missing_starting_crew_and_executed_work() -> TestResult {
    run_archive_witness(&["mira", "jonah"], Scenario::Partial).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_companions_expire_reconnect_and_allow_explicit_partial_extraction()
-> TestResult {
    run_archive_witness(&["mira", "jonah"], Scenario::Unavailable).await
}

#[derive(Clone, Copy)]
enum Scenario {
    Complete,
    Partial,
    Unavailable,
}

struct QualificationClock(Mutex<HostClockSampleV1>);

impl HostClockV1 for QualificationClock {
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
        self.0
            .lock()
            .map(|sample| sample.clone())
            .map_err(|_| HostClockErrorV1::Unavailable)
    }
}

impl QualificationClock {
    fn advance_to(&self, timestamp: &str) -> TestResult {
        *self
            .0
            .lock()
            .map_err(|_| "qualification clock unavailable")? =
            HostClockSampleV1::new(timestamp.to_owned())?;
        Ok(())
    }
}

struct LiveWitness {
    lead: WebSocket<TcpStream>,
    agents: BTreeMap<String, WebSocket<TcpStream>>,
    room: String,
    agent_members: BTreeMap<String, String>,
    agent_objectives: BTreeMap<String, Value>,
    state: Value,
    offers: Vec<Value>,
    sequence: u64,
    next_id: u64,
    descriptor_actions: Vec<String>,
}

impl LiveWitness {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("{:026X}", self.next_id)
    }

    fn act(&mut self, role: &str, action: &str, payload: Value) -> TestResult {
        if role == "lead" {
            assert!(
                self.offers
                    .iter()
                    .any(|offer| offer["action_type"] == action),
                "{action} absent from current lead offers at Head {}",
                self.sequence
            );
        }
        let action_id = self.id();
        let message_id = self.id();
        let sequence = self.sequence;
        let mut body = json!({"action_id":action_id,"based_on_room_seq":sequence,
            "action_type":action,"payload":payload});
        if role != "lead" {
            body["room_id"] = json!(self.room);
            body["member_id"] = json!(
                self.agent_members
                    .get(role)
                    .ok_or("agent Membership missing")?
            );
        }
        let actor = if role == "lead" {
            &mut self.lead
        } else {
            self.agents.get_mut(role).ok_or("agent stream missing")?
        };
        send(actor, "action.submit", &body, &message_id)?;
        let receipt = receive(actor, "action.accepted")
            .map_err(|error| format!("{role}/{action} at Head {sequence}: {error}"))?;
        let observation = receive(&mut self.lead, "observation.deliver")?;
        assert_eq!(receipt["room_head"]["room_seq"], sequence + 1);
        self.install_observation(observation, sequence + 1)
    }

    fn install_observation(&mut self, observation: Value, sequence: u64) -> TestResult {
        self.state = projection(&observation);
        // Delta delivery omits unchanged offers; retain the last authorized set.
        if let Some(offers) = action_offers(&observation) {
            self.offers = offers.clone();
        }
        self.sequence = room_seq(&observation);
        assert_eq!(self.sequence, sequence);
        let indices = self
            .offers
            .iter()
            .map(|offer| {
                self.descriptor_actions
                    .iter()
                    .position(|declared| offer["action_type"] == declared.as_str())
                    .ok_or("Action offer outside descriptor")
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            indices.windows(2).all(|pair| pair[0] < pair[1]),
            "offers must be unique and in descriptor order"
        );
        let ack_id = self.id();
        send(
            &mut self.lead,
            "observation.ack",
            &json!({"through_frame_seq":observation["frame_seq"]}),
            &ack_id,
        )?;
        receive(&mut self.lead, "observation.acked")?;
        Ok(())
    }

    fn reject_agent(
        &mut self,
        role: &str,
        payload: Value,
        based_on: u64,
        code: &str,
    ) -> TestResult {
        let action_id = self.id();
        let message_id = self.id();
        let body = json!({"room_id":self.room,"member_id":self.agent_members[role],"action_id":action_id,
            "based_on_room_seq":based_on,"action_type":"submit_companion_plan","payload":payload});
        let socket = self.agents.get_mut(role).ok_or("agent missing")?;
        send(socket, "action.submit", &body, &message_id)?;
        let rejection = receive(socket, "action.rejected")?;
        assert_eq!(rejection["current_room_seq"], self.sequence);
        if code == "stale_plan" {
            assert_eq!(rejection["code"], "activity_domain_rejection");
            assert_eq!(rejection["details"]["declared_code"], code);
        } else {
            assert_eq!(rejection["code"], code);
        }
        Ok(())
    }

    fn reconnect_agent(&mut self, address: SocketAddr, role: &str, bearer: &str) -> TestResult {
        self.agents
            .remove(role)
            .ok_or("old agent stream missing")?
            .close(None)?;
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        let mut request = format!("ws://{address}/v1/stream").into_client_request()?;
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {bearer}").parse()?);
        request.headers_mut().insert(
            "sec-websocket-protocol",
            worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?,
        );
        let (mut socket, _) = tungstenite::client(request, stream)?;
        let hello_id = self.id();
        send(
            &mut socket,
            "client.hello",
            &json!({"client_name":"archive-reconnect-proof","client_version":"1","mode":"participant",
            "supported_protocols":[worldstream_protocol::PROTOCOL_VERSION],"capabilities":["cursor_ack","projection_reset"]}),
            &hello_id,
        )?;
        let welcome = receive(&mut socket, "server.welcome")?;
        assert_eq!(welcome["authenticated_principal"]["kind"], "agent");
        let attach_id = self.id();
        send(
            &mut socket,
            "room.attach",
            &json!({"room_id":self.room,"member_id":self.agent_members[role],"after_frame_seq":null}),
            &attach_id,
        )?;
        let attached = receive(&mut socket, "room.attached")?;
        assert_eq!(attached["member_id"], self.agent_members[role]);
        assert_eq!(attached["room_head"]["room_seq"], self.sequence);
        let reset = receive(&mut socket, "projection.reset")?;
        let mut actual = projection(&reset);
        let mut expected = self.state.clone();
        // The role note is personalized even before either agent investigates.
        expected["objective"] = self.agent_objectives[role].clone();
        actual
            .as_object_mut()
            .ok_or("reconnect projection missing")?
            .remove("action_offers");
        expected
            .as_object_mut()
            .ok_or("lead projection missing")?
            .remove("action_offers");
        for (field, value) in expected.as_object().ok_or("expected projection missing")? {
            assert_eq!(
                &actual[field], value,
                "{role} reconnect changed authorized {field}"
            );
        }
        assert_eq!(
            actual.as_object().map(serde_json::Map::len),
            expected.as_object().map(serde_json::Map::len)
        );
        assert!(
            action_offers(&reset).is_some_and(Vec::is_empty),
            "an expired or cancelled opportunity must not reappear on reconnect"
        );
        let sync_id = self.id();
        send(
            &mut socket,
            "room.sync_ack",
            &json!({"room_id":self.room,"member_id":self.agent_members[role],
            "through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}),
            &sync_id,
        )?;
        receive(&mut socket, "room.sync_acked")?;
        self.agents.insert(role.to_owned(), socket);
        Ok(())
    }

    fn reject(&mut self, action: &str, payload: Value, stale: bool) -> TestResult {
        let action_id = self.id();
        let message_id = self.id();
        let sequence = if stale {
            self.sequence.saturating_sub(1)
        } else {
            self.sequence
        };
        send(
            &mut self.lead,
            "action.submit",
            &json!({"action_id":action_id,"based_on_room_seq":sequence,
            "action_type":action,"payload":payload}),
            &message_id,
        )?;
        let rejected = receive_envelope(&mut self.lead)?;
        assert_eq!(
            rejected["type"], "action.rejected",
            "known-invalid action must be rejected"
        );
        assert_eq!(rejected["body"]["current_room_seq"], self.sequence);
        Ok(())
    }

    fn personal(&mut self, action: &str, payload: Value) -> TestResult {
        let before = turns_used(&self.state);
        self.act("lead", action, payload)?;
        assert_eq!(
            turns_used(&self.state),
            before,
            "staging must not advance a turn"
        );
        self.act("lead", "commit_turn", json!({}))?;
        assert_eq!(turns_used(&self.state), before + 1);
        Ok(())
    }

    fn task(&mut self, role: &str, task: &str, allowance: u64, steps: Value) -> TestResult {
        let before = turns_used(&self.state);
        self.act(
            "lead",
            &format!("assign_{role}_task"),
            json!({"task_kind":task,"power_allowance":allowance}),
        )?;
        self.act("lead", &format!("request_{role}_plan"), json!({}))?;
        let revision = self.state[role]["task"]["revision"].clone();
        let opportunity = self.state[role]["planning"]["opportunity_revision"].clone();
        self.act(
            role,
            "submit_companion_plan",
            json!({"task_revision":revision,"opportunity_revision":opportunity,"steps":steps}),
        )?;
        assert_eq!(
            turns_used(&self.state),
            before,
            "an authenticated plan cannot commit a turn"
        );
        assert_eq!(self.state[role]["planning"]["status"], "ready");
        Ok(())
    }

    fn prepare(&mut self, role: &str) -> TestResult {
        self.act("lead", &format!("prepare_{role}_contribution"), json!({}))
    }

    fn extract(&mut self) -> TestResult {
        self.act("lead", "stage_extract", json!({}))?;
        self.reject("commit_turn", json!({}), false)?;
        self.act("lead", "prepare_extraction", json!({}))?;
        let preview = self.state["extraction"].clone();
        self.act("lead", "acknowledge_extraction", json!({"preview_revision":preview["revision"],"left_behind_roles":preview["left_behind_roles"]}))?;
        self.act("lead", "commit_turn", json!({}))
    }
}

fn plan_step(kind: &str, destination: &str, source: &str, power: u64) -> Value {
    json!({"step_type":kind,"destination":destination,"source_id":source,"power_cost":power})
}

fn unavailable_route(live: &mut LiveWitness, clock: &QualificationClock) -> TestResult {
    let admitted_at = time::OffsetDateTime::parse(
        clock.sample()?.as_str(),
        &time::format_description::well_known::Rfc3339,
    )?;
    live.personal("stage_move", json!({"destination":"records"}))?;
    for role in ["mira", "jonah"] {
        live.act(
            "lead",
            &format!("assign_{role}_task"),
            json!({"task_kind":"investigate_records","power_allowance":0}),
        )?;
    }
    live.act("lead", "request_mira_plan", json!({}))?;
    let mira_head = live.sequence;
    let mira_payload = json!({"task_revision":live.state["mira"]["task"]["revision"],
        "opportunity_revision":live.state["mira"]["planning"]["opportunity_revision"],
        "steps":[plan_step("inspect_source","none","records",0)]});
    let deadline = live.state["mira"]["planning"]["deadline"]
        .as_str()
        .ok_or("Mira deadline missing")?
        .to_owned();
    assert_eq!(
        time::OffsetDateTime::parse(&deadline, &time::format_description::well_known::Rfc3339)?
            - admitted_at,
        time::Duration::seconds(15)
    );
    live.reject("request_jonah_plan", json!({}), false)?;
    // A deliberate edit cancels the old opportunity. Neither rebasing the old
    // payload nor resubmitting it at its original Head can resurrect it.
    live.act("lead", "stage_wait", json!({}))?;
    assert_eq!(live.state["mira"]["planning"]["status"], "not_requested");
    live.reject_agent("mira", mira_payload.clone(), mira_head, "stale_room_state")?;
    live.reject_agent("mira", mira_payload, live.sequence, "action_not_allowed")?;
    live.act("lead", "request_jonah_plan", json!({}))?;
    let jonah_head = live.sequence;
    let jonah_payload = json!({"task_revision":live.state["jonah"]["task"]["revision"],
        "opportunity_revision":live.state["jonah"]["planning"]["opportunity_revision"],
        "steps":[plan_step("inspect_source","none","records",0)]});
    let mut wrong_task = jonah_payload.clone();
    wrong_task["task_revision"] = json!(999);
    live.reject_agent("jonah", wrong_task, live.sequence, "stale_plan")?;
    assert_eq!(live.state["jonah"]["planning"]["deadline"], deadline);
    let before = live.state.clone();
    clock.advance_to(&deadline)?;
    // The real Runtime scheduler commits the one due timer at the injected
    // semantic time. No sleep is used to infer whether a reply has expired.
    let expired = receive(&mut live.lead, "observation.deliver")
        .map_err(|error| format!("scheduler expiry observation at Head {jonah_head}: {error}"))?;
    live.install_observation(expired, jonah_head + 1)?;
    assert_eq!(live.state["jonah"]["planning"]["status"], "expired");
    for field in [
        "turns_used",
        "turns_remaining",
        "power",
        "location",
        "staged_action",
        "crew_debrief",
    ] {
        assert_eq!(live.state[field], before[field], "expiry changed {field}");
    }
    for role in ["mira", "jonah"] {
        assert_eq!(live.state[role]["location"], "records");
        assert_eq!(live.state[role]["planning"]["steps_completed"], 0);
    }
    live.reject_agent(
        "jonah",
        jonah_payload.clone(),
        jonah_head,
        "stale_room_state",
    )?;
    live.reject_agent("jonah", jonah_payload, live.sequence, "action_not_allowed")?;
    Ok(())
}

fn finish_unavailable_route(live: &mut LiveWitness) -> TestResult {
    for role in ["mira", "jonah"] {
        live.act("lead", &format!("defer_{role}_contribution"), json!({}))?;
    }
    live.act("lead", "set_jonah_regroup", json!({}))?;
    live.personal("stage_use_verifier", json!({}))?;
    for (action, payload) in [
        ("stage_move", json!({"destination":"plant"})),
        ("stage_open_service_hatch", json!({})),
        ("stage_move", json!({"destination":"vault"})),
        (
            "stage_recover_candidate",
            json!({"candidate_id":"ledger-violet"}),
        ),
        ("stage_move", json!({"destination":"plant"})),
        ("stage_move", json!({"destination":"records"})),
        ("stage_move", json!({"destination":"atrium"})),
    ] {
        live.personal(action, payload)?;
    }
    live.extract()?;
    assert_eq!(live.state["outcome"]["kind"], "partial_extraction");
    assert_eq!(live.state["turns_used"], 10);
    assert_eq!(live.state["power"], 0);
    assert_eq!(
        live.state["crew_debrief"]["starting_roles"],
        json!(["lead", "mira", "jonah"])
    );
    assert_eq!(
        live.state["crew_debrief"]["extracted_roles"],
        json!(["lead", "jonah"])
    );
    assert_eq!(
        live.state["crew_debrief"]["left_behind_roles"],
        json!(["mira"])
    );
    assert!(
        live.state["crew_debrief"]["completed_work"]
            .as_array()
            .ok_or("work debrief missing")?
            .iter()
            .all(|work| work["kind"] == "follow_move" || work["kind"] == "regroup_move"),
        "missing replies must not be attributed fabricated specialist work"
    );
    Ok(())
}

fn specialist_route(live: &mut LiveWitness, roles: &[&str], partial: bool) -> TestResult {
    let mira = roles.contains(&"mira");
    let jonah = roles.contains(&"jonah");
    live.personal("stage_move", json!({"destination":"records"}))?;
    if mira && jonah {
        live.act("lead", "set_mira_hold", json!({}))?;
    } else if !mira {
        live.personal("stage_use_verifier", json!({}))?;
    }
    live.personal("stage_move", json!({"destination":"plant"}))?;
    let hatch_role = if jonah { "jonah" } else { "mira" };
    let hatch_cost = if jonah { 1 } else { 2 };
    if mira && jonah {
        live.task(
            "mira",
            "investigate_records",
            1,
            json!([plan_step("use_verifier", "none", "records", 1)]),
        )?;
    }
    live.task(
        hatch_role,
        "open_service_hatch",
        hatch_cost,
        json!([plan_step("open_service_hatch", "none", "none", hatch_cost)]),
    )?;
    live.act("lead", "stage_open_service_hatch", json!({}))?;
    live.prepare(hatch_role)?;
    if mira && jonah {
        live.prepare("mira")?;
        assert_eq!(live.state["turn_resolution"]["power_reserved"], 4);
        assert!(
            live.state["turn_resolution"]["conflicts"]
                .as_array()
                .ok_or("conflicts missing")?
                .iter()
                .any(|conflict| conflict["code"] == "shared_power")
        );
    }
    assert!(
        live.state["turn_resolution"]["conflicts"]
            .as_array()
            .ok_or("conflicts missing")?
            .iter()
            .any(|conflict| conflict["code"] == "service_hatch")
    );
    live.reject("commit_turn", json!({}), false)?;
    if mira && jonah {
        live.act("lead", "defer_mira_contribution", json!({}))?;
    }
    live.act("lead", "stage_wait", json!({}))?;
    let before_power = live.state["power"].as_u64().ok_or("power missing")?;
    // A prepared opening cannot make a beginning-of-turn closed edge legal.
    live.reject("stage_move", json!({"destination":"vault"}), false)?;
    live.act("lead", "commit_turn", json!({}))?;
    assert_eq!(live.state["gates"]["service_hatch"], "open");
    assert_eq!(
        live.state["power"].as_u64(),
        Some(before_power - hatch_cost)
    );
    assert_eq!(live.state[hatch_role]["planning"]["steps_completed"], 1);
    if mira {
        if jonah {
            assert_eq!(
                live.state["verifier_result"],
                Value::Null,
                "deferred verifier must not claim work"
            );
        }
        live.act("lead", "set_mira_follow", json!({}))?;
    }
    live.personal("stage_move", json!({"destination":"vault"}))?;
    if mira && jonah {
        live.personal("stage_wait", json!({}))?;
    }
    if mira {
        assert_eq!(live.state["mira"]["location"], "vault");
        live.task(
            "mira",
            "field_assay",
            0,
            json!([
                plan_step("collect_assay_sample", "none", "none", 0),
                plan_step("complete_field_assay", "none", "none", 0)
            ]),
        )?;
        let assay_power = live.state["power"].clone();
        for complete in [false, true] {
            live.act("lead", "stage_wait", json!({}))?;
            live.prepare("mira")?;
            live.act("lead", "commit_turn", json!({}))?;
            assert_eq!(live.state["power"], assay_power);
            assert_eq!(
                live.state["mira"]["field_assay"]["steps_completed"],
                if complete { 2 } else { 1 }
            );
            if complete {
                assert_eq!(
                    live.state["mira"]["field_assay"]["result"],
                    json!({"candidate_id":"ledger-violet","confidence":"verified"})
                );
            } else {
                assert_eq!(live.state["mira"]["field_assay"]["result"], Value::Null);
                assert!(
                    live.state["candidates"]
                        .as_array()
                        .ok_or("candidate board missing")?
                        .iter()
                        .all(|candidate| candidate["observed_evidence"]
                            .as_array()
                            .is_some_and(Vec::is_empty))
                );
            }
        }
    }
    if jonah {
        live.act("lead", "set_jonah_regroup", json!({}))?;
    }
    live.personal(
        "stage_recover_candidate",
        json!({"candidate_id":"ledger-violet"}),
    )?;
    for destination in ["plant", "records", "atrium"] {
        live.personal("stage_move", json!({"destination":destination}))?;
    }
    if mira {
        live.act("lead", "stage_extract", json!({}))?;
        live.act("lead", "prepare_extraction", json!({}))?;
        assert_eq!(
            live.state["extraction"]["left_behind_roles"],
            json!(["mira"])
        );
        let preview = live.state["extraction"]["revision"].clone();
        live.reject(
            "acknowledge_extraction",
            json!({"preview_revision":preview,"left_behind_roles":[]}),
            false,
        )?;
        if !partial {
            live.act("lead", "set_mira_regroup", json!({}))?;
            assert_eq!(live.state["extraction"]["status"], "none");
            // Two completed return steps leave Mira in Records. The third is
            // included in the extraction preview and resolves with extraction.
            for _ in 0..2 {
                live.personal("stage_wait", json!({}))?;
            }
            assert_eq!(live.state["mira"]["location"], "records");
        }
    }
    live.extract()?;
    let expected_outcome = if partial {
        "partial_extraction"
    } else {
        "success"
    };
    assert_eq!(live.state["outcome"]["kind"], expected_outcome);
    let mut expected_crew = vec!["lead"];
    expected_crew.extend_from_slice(roles);
    assert_eq!(
        live.state["crew_debrief"]["starting_roles"],
        json!(expected_crew)
    );
    assert_eq!(
        live.state["crew_debrief"]["left_behind_roles"],
        if partial { json!(["mira"]) } else { json!([]) }
    );
    if !partial {
        assert_eq!(
            live.state["crew_debrief"]["extracted_roles"],
            json!(expected_crew)
        );
    }
    let work = live.state["crew_debrief"]["completed_work"]
        .as_array()
        .ok_or("completed work missing")?;
    assert_eq!(
        work.iter()
            .filter(|entry| entry["kind"] == "open_service_hatch")
            .count(),
        1
    );
    assert!(
        work.iter()
            .any(|entry| entry["role"] == hatch_role && entry["kind"] == "open_service_hatch")
    );
    if mira {
        assert_eq!(
            work.iter()
                .filter(|entry| entry["role"] == "mira" && entry["kind"] == "complete_field_assay")
                .count(),
            1
        );
        assert!(
            !work
                .iter()
                .any(|entry| entry["role"] == "mira" && entry["kind"] == "use_verifier")
        );
    }
    assert!(turns_used(&live.state) <= 16);
    Ok(())
}

async fn run_archive_witness(roles: &[&str], scenario: Scenario) -> TestResult {
    let (client_release_digest, client_surface_id) = client_binding_identity()?;
    let verified =
        PackBundleVerifierV1.inspect(Arc::<[u8]>::from(fs::read(current_bundle_path())?))?;
    assert_eq!(verified.inspection().pack_id, PACK_ID);
    assert_eq!(verified.inspection().explanatory_version, PACK_VERSION);
    assert_eq!(verified.bundle_digest().to_string(), CURRENT_BUNDLE_DIGEST);
    assert_eq!(
        verified.revision_digest().to_string(),
        CURRENT_REVISION_DIGEST
    );
    assert_eq!(
        verified.component_digest().to_string(),
        CURRENT_COMPONENT_DIGEST
    );
    let revision = verified.revision_digest().clone();
    let configuration: Value =
        serde_json::from_slice(&verified.golden_corpus().genesis.configuration.to_bytes()?)?;
    let witness = verified.golden_corpus().actions.clone();
    assert_eq!(
        witness
            .iter()
            .filter(|action| action.action_type == "commit_turn")
            .count(),
        15
    );
    assert_eq!(witness.len(), 32);
    assert_eq!(witness[witness.len() - 3].action_type, "prepare_extraction");
    assert_eq!(
        witness[witness.len() - 2].action_type,
        "acknowledge_extraction"
    );
    assert_eq!(witness[witness.len() - 1].action_type, "commit_turn");
    let host = ComponentPackHostV1::new()?;
    let mut admissions = Vec::new();
    let mut retained_revisions = Vec::new();
    if roles.is_empty() {
        for (bundle, expected_revision) in [
            (
                RETAINED_FIRST_PLAYABLE_BUNDLE_DIGEST,
                RETAINED_FIRST_PLAYABLE_REVISION_DIGEST,
            ),
            (
                RETAINED_EVIDENCE_BUNDLE_DIGEST,
                RETAINED_EVIDENCE_REVISION_DIGEST,
            ),
            (
                RETAINED_AGREEMENT_BUNDLE_DIGEST,
                RETAINED_AGREEMENT_REVISION_DIGEST,
            ),
            (RETAINED_MIRA_BUNDLE_DIGEST, RETAINED_MIRA_REVISION_DIGEST),
            (
                RETAINED_SPECIALIST_BUNDLE_DIGEST,
                RETAINED_SPECIALIST_REVISION_DIGEST,
            ),
            (
                RETAINED_CONSECUTIVE_BUNDLE_DIGEST,
                RETAINED_CONSECUTIVE_REVISION_DIGEST,
            ),
        ] {
            let old = PackBundleVerifierV1
                .inspect(Arc::<[u8]>::from(fs::read(release_bundle_path(bundle))?))?;
            assert_eq!(old.bundle_digest().to_string(), bundle);
            assert_eq!(old.revision_digest().to_string(), expected_revision);
            assert_eq!(old.inspection().pack_id, PACK_ID);
            retained_revisions.push(old.revision_digest().clone());
            admissions.push(host.admit(
                old,
                PackRegistryStatusV1 {
                    selectable_for_new_rooms: false,
                    runnable_for_retained_rooms: true,
                    approved_for_activity_start: true,
                },
            )?);
        }
    }
    admissions.push(host.admit(
        verified,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: true,
        },
    )?);
    // Admission verifies the actual retained golden corpora under their original executors.
    let registry = builtin_counter_registry()?.admit_portable(admissions)?;
    for retained in retained_revisions {
        registry.load_retained(&retained)?;
        let status = registry.catalog_revision(&retained)?;
        assert!(!status.selectable_for_new_rooms);
        assert!(status.runnable_for_retained_rooms && status.approved_for_activity_start);
    }
    let status = registry.catalog_revision(&revision)?;
    assert!(
        status.selectable_for_new_rooms
            && status.runnable_for_retained_rooms
            && status.approved_for_activity_start
    );
    let descriptor_actions = registry
        .load_retained(&revision)?
        .descriptor()
        .actions
        .iter()
        .map(|action| action.action_type.clone())
        .collect();
    let directory = tempfile::tempdir()?;
    let store = SqliteRoomStore::open(directory.path().join("archive.sqlite"))?;
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
    // Activity Start currently uses authority wall time. Start the injected
    // Action/Timer clock beyond it, then advance only when the witness directs.
    let clock_anchor =
        (time::OffsetDateTime::now_utc() + time::Duration::days(1)).replace_nanosecond(0)?;
    let clock = Arc::new(QualificationClock(Mutex::new(HostClockSampleV1::new(
        clock_anchor.format(&time::format_description::well_known::Rfc3339)?,
    )?)));
    let backend = Arc::new(if matches!(scenario, Scenario::Unavailable) {
        SqliteGatewayBackend::with_host_clock(store, Arc::new(registry), clock.clone())
            .with_timer_authority(host_bearer)?
    } else {
        SqliteGatewayBackend::new(store, Arc::new(registry))
    });
    let state = OperatorState::new(EffectiveConfig::default())?.with_backend(backend);
    let routes = operator_router(if matches!(scenario, Scenario::Unavailable) {
        state.with_scheduler()?
    } else {
        state
    });
    let host_header = format!("Bearer {}", BearerWireV1::from_bytes([0xc1; 32]).to_wire());
    let roster = std::iter::once("lead")
        .chain(roles.iter().copied())
        .collect::<Vec<_>>();
    let principals = roster
        .iter()
        .enumerate()
        .map(|(index, _)| format!("{:026X}", index + 200))
        .collect::<Vec<_>>();
    let members = roster.iter().zip(&principals).map(|(role, principal)| json!({"principal_id":principal,
        "principal_kind":if *role == "lead" { "human" } else { "agent" },"role":role,"access_mode":"participant"})).collect::<Vec<_>>();
    let created: CreateRoomResponse = serde_json::from_value(post(&routes, &host_header, "/v1/rooms", json!({
        "pack":{"id":PACK_ID,"version":PACK_VERSION,"digest":CURRENT_REVISION_DIGEST},"configuration":configuration,
        "members":members,"idempotency_key":"midnight-archive-live-witness"
    })).await?)?;
    assert_eq!(created.member_ids.len(), roster.len());
    assert_eq!(
        created
            .member_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        roster.len()
    );
    let mut capabilities = BTreeMap::new();
    for (index, role) in roster.iter().enumerate() {
        let issued: MemberCapabilityIssueResponse = serde_json::from_value(post(&routes, &host_header,
            "/v1/operator/member-capabilities", json!({"room_id":created.room_id,"member_id":created.member_ids[index],
            "principal_id":principals[index],"scopes":["room:attach","room:observe_member","room:act","room:replay"],
            "idempotency_key":format!("{:026X}",index+300),"expires_at":null})).await?)?;
        capabilities.insert(
            (*role).to_owned(),
            (created.member_ids[index].clone(), issued.bearer),
        );
    }
    let (lead_member, lead_bearer) = capabilities
        .get("lead")
        .ok_or("lead authority missing")?
        .clone();
    let ticket = post(&routes, &format!("Bearer {lead_bearer}"), "/v1/hosted/browser-stream-ticket", json!({
        "version":"hosted_browser_ws_ticket.v1","room_id":created.room_id,"member_id":lead_member,"mode":"participant",
        "after_frame_seq":null,"browser_session_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "client_release_digest":client_release_digest,"client_surface_id":client_surface_id
    })).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server_routes = routes.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            server_routes.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let blocking_routes = routes.clone();
    let room = created.room_id.clone();
    let owned_roles = roles
        .iter()
        .map(|role| (*role).to_owned())
        .collect::<Vec<_>>();
    let result = tokio::task::spawn_blocking(move || -> TestResult<(u64, Value)> {
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
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
        send(&mut socket, "room.sync_ack", &json!({"through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}), "00000000000000000000000400")?;
        receive(&mut socket, "room.sync_acked")?;
        let launch = tokio::runtime::Handle::current().block_on(post(&blocking_routes, &host_header,
            &format!("/v1/operator/rooms/{room}/lobby/launch"), json!({"input_id":"00000000000000000000000401","based_on_room_seq":0,"pack_digest":CURRENT_REVISION_DIGEST})))?;
        assert_eq!(launch["room_head"]["room_seq"], 1);
        let active = receive(&mut socket, "observation.deliver")?;
        let state = projection(&active);
        assert_eq!(state["phase"], "active");
        assert_eq!(state["turns_used"], 0);
        assert_eq!(state["turns_remaining"], 16);
        assert_eq!(state["power"], 3);
        assert_eq!(state["location"], "atrium");
        let mut agents = BTreeMap::new();
        let mut agent_objectives = BTreeMap::new();
        for role in &owned_roles {
            assert_eq!(state[role]["presence"], "active");
            assert_eq!(state[role]["location"], "atrium");
            let (member, bearer) = capabilities.get(role).ok_or("companion authority missing")?;
            let stream = TcpStream::connect(address)?;
            stream.set_read_timeout(Some(Duration::from_secs(15)))?;
            let mut request = format!("ws://{address}/v1/stream").into_client_request()?;
            request.headers_mut().insert("authorization", format!("Bearer {bearer}").parse()?);
            request.headers_mut().insert("sec-websocket-protocol", worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse()?);
            let (mut agent, _) = tungstenite::client(request, stream)?;
            send(&mut agent, "client.hello", &json!({"client_name":"archive-specialist-proof","client_version":"1","mode":"participant",
                "supported_protocols":[worldstream_protocol::PROTOCOL_VERSION],"capabilities":["cursor_ack","projection_reset"]}), "00000000000000000000000402")?;
            let welcome = receive(&mut agent, "server.welcome")?;
            assert_eq!(welcome["authenticated_principal"]["kind"], "agent");
            send(&mut agent, "room.attach", &json!({"room_id":room,"member_id":member,"after_frame_seq":null}), "00000000000000000000000403")?;
            let attached = receive(&mut agent, "room.attached")?;
            let reset = receive(&mut agent, "projection.reset")?;
            agent_objectives.insert(role.clone(), projection(&reset)["objective"].clone());
            send(&mut agent, "room.sync_ack", &json!({"room_id":room,"member_id":member,"through_frame_head":attached["frame_head"],"sync_token":attached["sync_token"]}), "00000000000000000000000404")?;
            receive(&mut agent, "room.sync_acked")?;
            agents.insert(role.clone(), agent);
        }
        let agent_members = capabilities.iter().filter(|(role, _)| role.as_str() != "lead")
            .map(|(role, (member, _))| (role.clone(), member.to_string())).collect();
        let mut live = LiveWitness { lead: socket, agents, room: room.to_string(), agent_members, agent_objectives, state,
            offers: action_offers(&active).cloned().ok_or("initial offers missing")?, sequence: room_seq(&active), next_id: 1000, descriptor_actions };
        live.reject("stage_move", json!({"destination":"vault"}), false)?;
        live.reject("stage_wait", json!({}), true)?;
        if owned_roles.is_empty() {
            for expected in witness {
                let before_turns = turns_used(&live.state);
                let before_power = live.state["power"].clone();
                let before_gates = live.state["gates"].clone();
                let payload = serde_json::from_slice(&expected.canonical_payload.to_bytes()?)?;
                live.act("lead", &expected.action_type, payload)?;
                if expected.action_type == "commit_turn" { assert_eq!(turns_used(&live.state), before_turns + 1); }
                else {
                    assert_eq!(turns_used(&live.state), before_turns);
                    assert_eq!(live.state["power"], before_power);
                    assert_eq!(live.state["gates"], before_gates);
                }
            }
            assert_eq!(live.state["outcome"]["kind"], "success");
            assert_eq!(live.state["turns_used"], 15);
            assert_eq!(live.state["power"], 0);
            assert_eq!(live.state["crew_debrief"]["extracted_roles"], json!(["lead"]));
        } else if matches!(scenario, Scenario::Unavailable) {
            unavailable_route(&mut live, &clock)?;
            for role in ["mira", "jonah"] {
                live.reconnect_agent(address, role, &capabilities.get(role).ok_or("original agent capability missing")?.1)?;
            }
            finish_unavailable_route(&mut live)?;
        } else {
            specialist_route(&mut live, &owned_roles.iter().map(String::as_str).collect::<Vec<_>>(), matches!(scenario, Scenario::Partial))?;
        }
        Ok((live.sequence, live.state))
    }).await?;
    server.abort();
    let _ = server.await;
    let (last_sequence, mut final_state) = result?;
    let replay = get(
        &routes,
        &format!("Bearer {lead_bearer}"),
        &format!(
            "/v1/rooms/{}/replay?at_room_seq={last_sequence}",
            created.room_id
        ),
    )
    .await?;
    assert_eq!(replay["requested_room_seq"], last_sequence);
    // Hosted live delivery adds transport Action Offers to the Pack projection;
    // the historical replay endpoint returns the Pack-authored facts alone.
    assert_eq!(final_state["action_offers"], json!([]));
    final_state
        .as_object_mut()
        .ok_or("terminal projection missing")?
        .remove("action_offers");
    assert_eq!(
        projection(&replay),
        final_state,
        "Replay must reconstruct all authorized terminal facts and actual companion work"
    );
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
