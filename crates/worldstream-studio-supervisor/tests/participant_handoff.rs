#![allow(clippy::panic)]

#[allow(dead_code)]
#[path = "../src/protected_publication.rs"]
mod protected_publication;

#[allow(dead_code)]
#[path = "../src/client_bindings.rs"]
mod client_bindings;
#[allow(dead_code)]
#[path = "../src/participant_handoff.rs"]
mod participant_handoff;

use std::{
    net::TcpListener,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt as _;
use participant_handoff::{
    CurrentMembershipSnapshotV1, FixedDaemonParticipantConsoleGatewayV1, HumanSeatAuthorityV1,
    ParticipantConsoleGatewayErrorV1, ParticipantConsoleGatewayV1, ParticipantConsoleObservationV1,
    ParticipantConsoleReadinessSourceV1, ParticipantConsoleSessionHealthV1,
    ParticipantHandoffAuthorityErrorV1, ParticipantHandoffAuthoritySourceV1,
    ParticipantHandoffBrokerV1, participant_handoff_router,
};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tungstenite::{Message, accept_hdr};
use worldstream_protocol::{AccessMode, PackReference, SealedCapabilityBearerV1};

use client_bindings::{
    ClientBindingStoreErrorV1, ClientCandidateClassV1, ClientCandidateV1, ClientSelectionRequestV1,
    ClientSelectionSourceV1, ClientSelectionV1, DeploymentTrustLevelV1,
};

const STUDIO_ORIGIN: &str = "http://127.0.0.1:5174";
const CONSOLE_ORIGIN: &str = "http://127.0.0.1:5173";
const DRAFT_ID: &str = "draft-participant-handoff";
const ROOM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const BEARER: &str = "wsb1:abababababababababababababababababababababababababababababababab";
const AGENT_HEIST_0_1_DIGEST: &str =
    "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407";
const AGENT_HEIST_0_2_DIGEST: &str =
    "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820";

#[derive(Clone)]
struct FakeAuthoritySource {
    state: Arc<Mutex<Result<(), ParticipantHandoffAuthorityErrorV1>>>,
    pack: Arc<Mutex<PackReference>>,
}

impl FakeAuthoritySource {
    fn usable() -> Self {
        Self::for_pack(agent_heist_pack("0.2.0", AGENT_HEIST_0_2_DIGEST))
    }

    fn for_pack(pack: PackReference) -> Self {
        Self {
            state: Arc::new(Mutex::new(Ok(()))),
            pack: Arc::new(Mutex::new(pack)),
        }
    }

    fn set(&self, state: Result<(), ParticipantHandoffAuthorityErrorV1>) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

impl ParticipantHandoffAuthoritySourceV1 for FakeAuthoritySource {
    fn resolve_provisioned_human_seat(
        &self,
        draft_id: &str,
        seat_id: &str,
    ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1> {
        (*self.state.lock().unwrap_or_else(PoisonError::into_inner))?;
        if draft_id != DRAFT_ID || seat_id != "navigator" {
            return Err(ParticipantHandoffAuthorityErrorV1::SeatNotFound);
        }
        HumanSeatAuthorityV1::new(
            ROOM_ID,
            MEMBER_ID,
            self.pack
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            AccessMode::Participant,
            Some("navigator".to_owned()),
            SealedCapabilityBearerV1::parse(BEARER.to_owned())
                .map_err(|_| ParticipantHandoffAuthorityErrorV1::Unavailable)?,
        )
        .map_err(|_| ParticipantHandoffAuthorityErrorV1::Unavailable)
    }
}

type RequiredMembership = Option<(AccessMode, Option<String>)>;

#[derive(Clone)]
struct FakeClientSelectionSource {
    selection: Arc<Mutex<Result<ClientSelectionV1, ClientBindingStoreErrorV1>>>,
    requests: Arc<Mutex<Vec<ClientSelectionRequestV1>>>,
    required_membership: Arc<Mutex<RequiredMembership>>,
}

impl FakeClientSelectionSource {
    fn selected(path: &str) -> Self {
        Self {
            selection: Arc::new(Mutex::new(Ok(ClientSelectionV1::Selected {
                candidate: ClientCandidateV1 {
                    candidate_id: "test-approved-client".to_owned(),
                    deployment_id: "test-client-host".to_owned(),
                    client_id: "test.activity-client".to_owned(),
                    release_digest: format!("sha256:{}", "d".repeat(64)),
                    surface_id: "test-browser".to_owned(),
                    trust_level: DeploymentTrustLevelV1::Verified,
                    launch_url: format!("{CONSOLE_ORIGIN}{path}"),
                },
            }))),
            requests: Arc::new(Mutex::new(Vec::new())),
            required_membership: Arc::new(Mutex::new(None)),
        }
    }

    fn inspector() -> Self {
        Self {
            selection: Arc::new(Mutex::new(Ok(ClientSelectionV1::InspectorFallback {
                candidate: ClientCandidateV1 {
                    candidate_id: "configured-inspector".to_owned(),
                    deployment_id: "inspector-host".to_owned(),
                    client_id: "worldstream.inspector.web".to_owned(),
                    release_digest: format!("sha256:{}", "c".repeat(64)),
                    surface_id: "inspector-web".to_owned(),
                    trust_level: DeploymentTrustLevelV1::Verified,
                    launch_url: format!("{CONSOLE_ORIGIN}/inspector/"),
                },
            }))),
            requests: Arc::new(Mutex::new(Vec::new())),
            required_membership: Arc::new(Mutex::new(None)),
        }
    }

    fn selection_required() -> Self {
        let primary = ClientCandidateV1 {
            candidate_id: "approved-primary".to_owned(),
            deployment_id: "primary-host".to_owned(),
            client_id: "test.primary-client".to_owned(),
            release_digest: format!("sha256:{}", "a".repeat(64)),
            surface_id: "participant-web".to_owned(),
            trust_level: DeploymentTrustLevelV1::Verified,
            launch_url: format!("{CONSOLE_ORIGIN}/primary/"),
        };
        let secondary = ClientCandidateV1 {
            candidate_id: "approved-secondary".to_owned(),
            deployment_id: "secondary-host".to_owned(),
            client_id: "test.secondary-client".to_owned(),
            release_digest: format!("sha256:{}", "b".repeat(64)),
            surface_id: "participant-web".to_owned(),
            trust_level: DeploymentTrustLevelV1::ExternallyTrusted,
            launch_url: format!("{CONSOLE_ORIGIN}/secondary/"),
        };
        Self {
            selection: Arc::new(Mutex::new(Ok(ClientSelectionV1::SelectionRequired {
                candidates: vec![primary, secondary],
            }))),
            requests: Arc::new(Mutex::new(Vec::new())),
            required_membership: Arc::new(Mutex::new(None)),
        }
    }

    fn require_membership(&self, access_mode: AccessMode, role: Option<&str>) {
        *self
            .required_membership
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((access_mode, role.map(str::to_owned)));
    }
}

impl ClientSelectionSourceV1 for FakeClientSelectionSource {
    fn select_client(
        &self,
        request: &ClientSelectionRequestV1,
        preferred_candidate_id: Option<&str>,
    ) -> Result<ClientSelectionV1, ClientBindingStoreErrorV1> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        if self
            .required_membership
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(access_mode, role)| {
                request.access_mode != *access_mode || request.role != *role
            })
        {
            return Err(ClientBindingStoreErrorV1::InvalidChoice);
        }
        let selection = self
            .selection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        if let Some(preferred) = preferred_candidate_id {
            return match selection {
                ClientSelectionV1::Selected { candidate }
                    if candidate.candidate_id == preferred =>
                {
                    Ok(ClientSelectionV1::Selected { candidate })
                }
                ClientSelectionV1::SelectionRequired { candidates } => candidates
                    .into_iter()
                    .find(|candidate| candidate.candidate_id == preferred)
                    .map(|candidate| ClientSelectionV1::Selected { candidate })
                    .ok_or(ClientBindingStoreErrorV1::InvalidChoice),
                _ => Err(ClientBindingStoreErrorV1::InvalidChoice),
            };
        }
        Ok(selection)
    }

    fn resolve_active_client(
        &self,
        request: &ClientSelectionRequestV1,
        class: ClientCandidateClassV1,
        candidate_id: &str,
    ) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
        let preferred = matches!(class, ClientCandidateClassV1::Binding).then_some(candidate_id);
        let selection = self.select_client(request, preferred)?;
        match (class, selection) {
            (ClientCandidateClassV1::Binding, ClientSelectionV1::Selected { candidate })
            | (
                ClientCandidateClassV1::InspectorFallback,
                ClientSelectionV1::InspectorFallback { candidate },
            ) if candidate.candidate_id == candidate_id => Ok(candidate),
            _ => Err(ClientBindingStoreErrorV1::InvalidChoice),
        }
    }
}

fn agent_heist_pack(version: &str, digest: &str) -> PackReference {
    PackReference {
        id: "worldstream.agent-heist".to_owned(),
        version: version.to_owned(),
        digest: digest.to_owned(),
    }
}

type GatewayCall = (String, String, Option<u64>, &'static str);

#[derive(Clone, Default)]
struct FakeGateway {
    calls: Arc<Mutex<Vec<GatewayCall>>>,
    current: Arc<Mutex<Option<CurrentMembershipSnapshotV1>>>,
}

impl FakeGateway {
    fn set_current(&self, current: CurrentMembershipSnapshotV1) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(current);
    }
}

impl ParticipantConsoleGatewayV1 for FakeGateway {
    fn membership_status(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        self.current_membership(authority, None)
    }

    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        Ok(self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| CurrentMembershipSnapshotV1 {
                pack: authority.pack().clone(),
                access_mode: authority.access_mode(),
                role: authority.role().map(str::to_owned),
            }))
    }

    fn observe(
        &self,
        authority: &HumanSeatAuthorityV1,
        after_frame_seq: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                authority.room_id().to_owned(),
                authority.member_id().to_owned(),
                after_frame_seq,
                "observe",
            ));
        Ok(ParticipantConsoleObservationV1 {
            browser_value: json!({"projection": {"phase": "Lobby"}, "frame_head": 7}),
            durable_cursor: None,
        })
    }

    fn act(
        &self,
        authority: &HumanSeatAuthorityV1,
        after_frame_seq: Option<u64>,
        _request: &participant_handoff::ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                authority.room_id().to_owned(),
                authority.member_id().to_owned(),
                after_frame_seq,
                "act",
            ));
        Ok(json!({"state": "accepted", "room_seq": 8}))
    }

    fn replay(
        &self,
        authority: &HumanSeatAuthorityV1,
        at_room_seq: u64,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                authority.room_id().to_owned(),
                authority.member_id().to_owned(),
                None,
                "replay",
            ));
        Ok(json!({
            "requested_room_seq": at_room_seq,
            "room_head": {
                "room_seq": at_room_seq,
                "genesis_or_transition_hash": format!("blake3:{}", "1".repeat(64)),
                "core_schema_version": "core.v1",
                "pack_digest": format!("blake3:{}", "2".repeat(64)),
                "core_state_hash": format!("blake3:{}", "3".repeat(64)),
                "activity_state_hash": format!("blake3:{}", "4".repeat(64)),
                "authoritative_state_hash": format!("blake3:{}", "5".repeat(64)),
            },
            "projection": {"core": {}, "activity": {"value": 2}, "action_offers": []},
            "projection_hash": format!("blake3:{}", "6".repeat(64)),
            "verification": "verified",
            "room_health": "healthy",
            "integrity_generation": 1,
        }))
    }
}

#[derive(Clone)]
struct SingleAttachGateway {
    current: CurrentMembershipSnapshotV1,
    membership_status_calls: Arc<Mutex<usize>>,
    current_membership_calls: Arc<Mutex<usize>>,
}

impl SingleAttachGateway {
    fn new(current: CurrentMembershipSnapshotV1) -> Self {
        Self {
            current,
            membership_status_calls: Arc::new(Mutex::new(0)),
            current_membership_calls: Arc::new(Mutex::new(0)),
        }
    }

    fn call_counts(&self) -> (usize, usize) {
        (
            *self
                .membership_status_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            *self
                .current_membership_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

impl ParticipantConsoleGatewayV1 for SingleAttachGateway {
    fn membership_status(
        &self,
        _: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        *self
            .membership_status_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        Ok(self.current.clone())
    }

    fn current_membership(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        let mut calls = self
            .current_membership_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *calls += 1;
        if *calls == 1 {
            Ok(self.current.clone())
        } else {
            Err(ParticipantConsoleGatewayErrorV1::Unavailable)
        }
    }

    fn observe(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    fn act(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
        _: &participant_handoff::ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }
}

#[derive(Clone)]
struct LeakingGateway;

impl ParticipantConsoleGatewayV1 for LeakingGateway {
    fn membership_status(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        self.current_membership(authority, None)
    }

    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        Ok(CurrentMembershipSnapshotV1 {
            pack: authority.pack().clone(),
            access_mode: authority.access_mode(),
            role: authority.role().map(str::to_owned),
        })
    }

    fn observe(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        Ok(ParticipantConsoleObservationV1 {
            browser_value: json!({"nested": {"bearer": BEARER, "durable_cursor": 7}}),
            durable_cursor: None,
        })
    }

    fn act(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
        _: &participant_handoff::ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Ok(json!({"nested": {"member_id": MEMBER_ID}}))
    }

    fn replay(
        &self,
        _: &HumanSeatAuthorityV1,
        _: u64,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Ok(json!({"projection": {"activity": {"prompt": "hidden"}}}))
    }
}

fn app(source: FakeAuthoritySource, gateway: FakeGateway) -> axum::Router {
    app_with_clients(
        source,
        gateway,
        FakeClientSelectionSource::selected("/agent-heist/"),
    )
}

fn app_with_clients(
    source: FakeAuthoritySource,
    gateway: FakeGateway,
    clients: FakeClientSelectionSource,
) -> axum::Router {
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        source,
        gateway,
        clients,
    )
    .unwrap_or_else(|error| panic!("test broker must be valid: {error:?}"));
    participant_handoff_router(broker)
}

fn fixed_gateway_app(address: std::net::SocketAddr) -> axum::Router {
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        FakeAuthoritySource::usable(),
        FixedDaemonParticipantConsoleGatewayV1::new(address, Duration::from_secs(1)),
        FakeClientSelectionSource::selected("/agent-heist/"),
    )
    .unwrap_or_else(|error| panic!("test broker must be valid: {error:?}"));
    participant_handoff_router(broker)
}

async fn json_response(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("response body: {error}"))
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("response json: {error}"))
}

async fn issue_handoff(router: &axum::Router) -> (String, String) {
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs")
                .header(header::ORIGIN, STUDIO_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"draft_id":"{DRAFT_ID}","seat_id":"navigator"}}"#
                )))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_response(response).await;
    assert_eq!(
        body.as_object().map(serde_json::Map::len),
        Some(3),
        "handoff response must stay exact and browser-safe"
    );
    assert_eq!(body["version"], "activity_client_handoff.v1");
    assert_eq!(body["state"], "ready");
    let client_url = body["client_url"].as_str().unwrap_or_default().to_owned();
    let handoff = client_url
        .split("#handoff=")
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    (client_url, handoff)
}

async fn redeem_handoff(
    router: &axum::Router,
    handoff: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::post("/api/v1/participant-console/handoffs:redeem")
        .header(header::ORIGIN, CONSOLE_ORIGIN)
        .header("x-worldstream-participant-handoff", handoff);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    router
        .clone()
        .oneshot(
            request
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"))
}

async fn console_request(
    router: &axum::Router,
    method: Method,
    path: &str,
    cookie: &str,
    body: Body,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"))
}

#[tokio::test]
async fn issues_only_a_short_lived_fragment_handoff_for_an_exact_human_seat() {
    let source = FakeAuthoritySource::usable();
    let router = app(source.clone(), FakeGateway::default());
    let (console_url, handoff) = issue_handoff(&router).await;

    assert!(console_url.starts_with("http://127.0.0.1:5173/agent-heist/#handoff=wsh1:"));
    assert!(!console_url.contains(DRAFT_ID));
    assert!(!console_url.contains(ROOM_ID));
    assert!(!console_url.contains(MEMBER_ID));
    assert!(!console_url.contains("wsb1:"));
    assert_eq!(handoff.len(), 69);

    source.set(Err(ParticipantHandoffAuthorityErrorV1::NotHuman));
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs")
                .header(header::ORIGIN, STUDIO_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"draft_id":"{DRAFT_ID}","seat_id":"navigator"}}"#
                )))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = json_response(response).await;
    assert_eq!(body["code"], "participant_handoff_human_seat_required");
    assert_eq!(
        body["message"],
        "Open participant client is available only for a human seat."
    );
}

#[tokio::test]
async fn returns_only_generic_candidates_until_the_operator_selects_one() {
    let router = app_with_clients(
        FakeAuthoritySource::usable(),
        FakeGateway::default(),
        FakeClientSelectionSource::selection_required(),
    );
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs")
                .header(header::ORIGIN, STUDIO_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"draft_id":"{DRAFT_ID}","seat_id":"navigator"}}"#
                )))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["state"], "selection_required");
    assert_eq!(body["candidates"].as_array().map(Vec::len), Some(2));
    assert!(body.get("client_url").is_none());
    assert!(!body.to_string().contains("launch_url"));

    let selected = router
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs")
                .header(header::ORIGIN, STUDIO_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"draft_id":"{DRAFT_ID}","seat_id":"navigator","candidate_id":"approved-secondary"}}"#
                )))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(selected.status(), StatusCode::CREATED);
    assert!(
        json_response(selected).await["client_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:5173/secondary/#handoff="))
    );
}

#[tokio::test]
async fn selects_agent_heist_client_for_the_other_supported_exact_revision() {
    let router = app(
        FakeAuthoritySource::for_pack(agent_heist_pack("0.1.0", AGENT_HEIST_0_1_DIGEST)),
        FakeGateway::default(),
    );

    let (console_url, _) = issue_handoff(&router).await;

    assert!(console_url.starts_with("http://127.0.0.1:5173/agent-heist/#handoff=wsh1:"));
}

#[tokio::test]
async fn falls_back_to_inspector_without_pack_name_or_version_matching() {
    let mismatches = [
        agent_heist_pack("0.2.0", AGENT_HEIST_0_1_DIGEST),
        agent_heist_pack("9.9.9", AGENT_HEIST_0_2_DIGEST),
        PackReference {
            id: "third-party.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: AGENT_HEIST_0_2_DIGEST.to_owned(),
        },
        PackReference {
            id: "worldstream.counter".to_owned(),
            version: "4.0.0".to_owned(),
            digest: format!("blake3:{}", "c".repeat(64)),
        },
    ];

    for pack in mismatches {
        let router = app_with_clients(
            FakeAuthoritySource::for_pack(pack),
            FakeGateway::default(),
            FakeClientSelectionSource::inspector(),
        );
        let (console_url, _) = issue_handoff(&router).await;
        assert!(
            console_url.starts_with("http://127.0.0.1:5173/inspector/#handoff=wsh1:"),
            "unsupported exact Pack must use Inspector: {console_url}"
        );
    }
}

#[tokio::test]
async fn redemption_is_origin_bound_one_use_and_rotates_a_scoped_http_only_cookie() {
    let router = app(FakeAuthoritySource::usable(), FakeGateway::default());
    let (_, handoff) = issue_handoff(&router).await;

    let forbidden = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs:redeem")
                .header(header::ORIGIN, "http://evil.test")
                .header("x-worldstream-participant-handoff", &handoff)
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let response = redeem_handoff(&router, &handoff, Some("ws_participant_session=stale")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(set_cookie.starts_with("ws_participant_session=wss1:"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/api/v1/participant-console"));
    assert!(!set_cookie.contains(ROOM_ID));
    assert!(!set_cookie.contains(MEMBER_ID));
    assert!(!set_cookie.contains("wsb1:"));
    let body = json_response(response).await;
    assert_eq!(
        body,
        json!({"version":"participant_console_session.v1","state":"usable","next_action":"continue"})
    );

    let replay = redeem_handoff(&router, &handoff, None).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_response(replay).await["code"],
        "participant_handoff_invalid"
    );
}

#[tokio::test]
async fn issuance_does_not_consume_the_only_membership_attach_needed_by_redemption() {
    let gateway = SingleAttachGateway::new(CurrentMembershipSnapshotV1 {
        pack: agent_heist_pack("0.2.0", AGENT_HEIST_0_2_DIGEST),
        access_mode: AccessMode::Participant,
        role: Some("navigator".to_owned()),
    });
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        FakeAuthoritySource::usable(),
        gateway.clone(),
        FakeClientSelectionSource::selected("/agent-heist/"),
    )
    .unwrap_or_else(|error| panic!("test broker must be valid: {error:?}"));
    let router = participant_handoff_router(broker);

    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;

    assert_eq!(redeemed.status(), StatusCode::OK);
    assert_eq!(gateway.call_counts(), (1, 1));
}

#[tokio::test]
async fn selection_uses_live_membership_and_spectator_sessions_are_read_only() {
    let gateway = FakeGateway::default();
    gateway.set_current(CurrentMembershipSnapshotV1 {
        pack: agent_heist_pack("0.2.0", AGENT_HEIST_0_2_DIGEST),
        access_mode: AccessMode::Spectator,
        role: None,
    });
    let clients = FakeClientSelectionSource::selected("/agent-heist/");
    let router = app_with_clients(FakeAuthoritySource::usable(), gateway, clients.clone());
    let (_, handoff) = issue_handoff(&router).await;
    assert_eq!(
        clients
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .first()
            .map(|request| (request.access_mode, request.role.clone())),
        Some((AccessMode::Spectator, None)),
    );

    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    let action = console_request(
        &router,
        Method::POST,
        "/api/v1/participant-console/session:act",
        &cookie,
        Body::from(
            r#"{"action_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","based_on_room_seq":7,"offer_id":"7:host_launch:0","schema_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","action_type":"host_launch","payload":{}}"#,
        ),
    )
    .await;
    assert_eq!(action.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn membership_change_away_and_back_cannot_resurrect_an_active_browser_session() {
    let clients = FakeClientSelectionSource::selected("/agent-heist/");
    clients.require_membership(AccessMode::Participant, Some("navigator"));
    let gateway = FakeGateway::default();
    let router = app_with_clients(
        FakeAuthoritySource::usable(),
        gateway.clone(),
        clients.clone(),
    );
    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();

    gateway.set_current(CurrentMembershipSnapshotV1 {
        pack: agent_heist_pack("0.1.0", AGENT_HEIST_0_1_DIGEST),
        access_mode: AccessMode::Participant,
        role: Some("insider".to_owned()),
    });
    let status = console_request(
        &router,
        Method::GET,
        "/api/v1/participant-console/session",
        &cookie,
        Body::empty(),
    )
    .await;

    assert_eq!(status.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_response(status).await["code"],
        "participant_session_authority_invalid",
    );

    gateway.set_current(CurrentMembershipSnapshotV1 {
        pack: agent_heist_pack("0.1.0", AGENT_HEIST_0_1_DIGEST),
        access_mode: AccessMode::Participant,
        role: Some("navigator".to_owned()),
    });
    let after_restore = console_request(
        &router,
        Method::GET,
        "/api/v1/participant-console/session",
        &cookie,
        Body::empty(),
    )
    .await;

    assert_eq!(after_restore.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_response(after_restore).await["code"],
        "participant_session_missing",
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn refresh_observe_and_act_reuse_only_the_bound_membership_authority() {
    let source = FakeAuthoritySource::usable();
    let gateway = FakeGateway::default();
    let router = app(source.clone(), gateway.clone());
    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let set_cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let cookie = set_cookie.split(';').next().unwrap_or_default().to_owned();

    let status = router
        .clone()
        .oneshot(
            Request::get("/api/v1/participant-console/session")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(json_response(status).await["state"], "usable");

    let observe = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/session:observe")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"after_frame_seq":null}"#))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(observe.status(), StatusCode::OK);
    let observed = json_response(observe).await;
    assert_eq!(observed["projection"]["phase"], "Lobby");
    assert!(!observed.to_string().contains(ROOM_ID));
    assert!(!observed.to_string().contains(MEMBER_ID));

    let refreshed = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/session:observe")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"after_frame_seq":7}"#))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(refreshed.status(), StatusCode::OK);

    let act = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/session:act")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"action_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","based_on_room_seq":7,"offer_id":"7:host_launch:0","schema_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","action_type":"host_launch","payload":{}}"#,
                ))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(act.status(), StatusCode::OK);
    assert_eq!(json_response(act).await["state"], "accepted");

    let replay = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/session:replay")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"at_room_seq":2}"#))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = json_response(replay).await;
    assert_eq!(replay["requested_room_seq"], 2);
    assert_eq!(replay["room_head"]["room_seq"], 2);
    assert_eq!(replay["projection"]["activity"]["value"], 2);
    assert_eq!(
        replay["room_head"]["genesis_or_transition_hash"],
        format!("blake3:{}", "1".repeat(64))
    );
    assert_eq!(
        replay["room_head"]["authoritative_state_hash"],
        format!("blake3:{}", "5".repeat(64))
    );
    assert_eq!(replay["verification"], "verified");
    assert!(!replay.to_string().contains(ROOM_ID));
    assert!(!replay.to_string().contains(MEMBER_ID));
    assert!(!replay.to_string().contains("prompt"));
    assert_eq!(
        gateway
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[
            (ROOM_ID.to_owned(), MEMBER_ID.to_owned(), None, "observe"),
            (ROOM_ID.to_owned(), MEMBER_ID.to_owned(), None, "observe"),
            (ROOM_ID.to_owned(), MEMBER_ID.to_owned(), None, "act"),
            (ROOM_ID.to_owned(), MEMBER_ID.to_owned(), None, "replay"),
        ]
    );

    source.set(Err(ParticipantHandoffAuthorityErrorV1::AuthorityInvalid));
    let invalid = router
        .oneshot(
            Request::get("/api/v1/participant-console/session")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    assert!(
        invalid
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.starts_with("ws_participant_session=; Max-Age=0"))
    );
    let body = json_response(invalid).await;
    assert_eq!(body["code"], "participant_session_authority_invalid");
    assert_eq!(body["next_action"], "return_to_task_setup");
}

#[tokio::test]
async fn rejects_unknown_fields_and_browser_unsafe_gateway_payloads() {
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        FakeAuthoritySource::usable(),
        LeakingGateway,
        FakeClientSelectionSource::selected("/agent-heist/"),
    )
    .unwrap_or_else(|error| panic!("test broker: {error:?}"));
    let router = participant_handoff_router(broker);
    let unknown = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs")
                .header(header::ORIGIN, STUDIO_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"draft_id":"{DRAFT_ID}","seat_id":"navigator","room_id":"{ROOM_ID}"}}"#
                )))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    let leaking = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/session:observe")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"after_frame_seq":null}"#))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(leaking.status(), StatusCode::BAD_GATEWAY);
    assert!(!json_response(leaking).await.to_string().contains(BEARER));

    let replay = router
        .oneshot(
            Request::post("/api/v1/participant-console/session:replay")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"at_room_seq":2}"#))
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(replay.status(), StatusCode::BAD_GATEWAY);
    assert!(!json_response(replay).await.to_string().contains("hidden"));
}

#[tokio::test]
async fn reports_exact_closed_readiness_without_retaining_room_or_member_ids() {
    let source = FakeAuthoritySource::usable();
    let gateway = FakeGateway::default();
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        source.clone(),
        gateway,
        FakeClientSelectionSource::selected("/agent-heist/"),
    )
    .unwrap_or_else(|error| panic!("test broker: {error:?}"));
    let router = participant_handoff_router(broker.clone());
    assert_eq!(
        broker.session_health(ROOM_ID, MEMBER_ID),
        ParticipantConsoleSessionHealthV1::Missing,
    );
    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    assert_eq!(redeemed.status(), StatusCode::OK);
    assert_eq!(
        broker.session_health(ROOM_ID, MEMBER_ID),
        ParticipantConsoleSessionHealthV1::Usable,
    );
    assert_eq!(
        broker.session_health("not-a-room", MEMBER_ID),
        ParticipantConsoleSessionHealthV1::Invalid,
    );
    source.set(Err(ParticipantHandoffAuthorityErrorV1::AuthorityInvalid));
    assert_eq!(
        broker.session_health(ROOM_ID, MEMBER_ID),
        ParticipantConsoleSessionHealthV1::Invalid,
    );
}

#[tokio::test]
async fn allows_only_the_fixed_local_origins_through_browser_cors() {
    let router = app(FakeAuthoritySource::usable(), FakeGateway::default());
    let preflight = router
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/participant-console/handoffs:redeem")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "cache-control, pragma, x-worldstream-participant-handoff",
                )
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some(CONSOLE_ORIGIN),
    );
    assert_eq!(
        preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .and_then(|value| value.to_str().ok()),
        Some("true"),
    );
    assert_eq!(
        preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok()),
        Some("Cache-Control, Pragma, Content-Type, X-WorldStream-Participant-Handoff"),
    );
    assert_eq!(
        preflight
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store, max-age=0"),
    );

    let unknown_header = router
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/participant-console/handoffs:redeem")
                .header(header::ORIGIN, CONSOLE_ORIGIN)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "cache-control, x-worldstream-private-routing",
                )
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(unknown_header.status(), StatusCode::FORBIDDEN);
    assert!(
        unknown_header
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
    assert_eq!(
        unknown_header
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store, max-age=0"),
    );

    let rejected = router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/participant-console/handoffs:redeem")
                .header(header::ORIGIN, "http://evil.test")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("response: {error}"));
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert!(
        rejected
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[test]
fn fixed_daemon_gateway_sanitizes_projection_and_attaches_health_to_exact_membership() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("bind daemon fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("daemon fixture address: {error}"));
    let server = spawn_daemon_fixture(listener);

    let authority = HumanSeatAuthorityV1::new(
        ROOM_ID,
        MEMBER_ID,
        PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: AGENT_HEIST_0_2_DIGEST.to_owned(),
        },
        AccessMode::Participant,
        Some("navigator".to_owned()),
        SealedCapabilityBearerV1::parse(BEARER.to_owned())
            .unwrap_or_else(|error| panic!("fixture bearer: {error}")),
    )
    .unwrap_or_else(|error| panic!("fixture authority: {error:?}"));
    let gateway = FixedDaemonParticipantConsoleGatewayV1::new(address, Duration::from_secs(1));
    let observed = gateway
        .observe(&authority, None)
        .unwrap_or_else(|error| panic!("production observe: {error:?}"));
    assert_eq!(observed.browser_value["room_head"]["room_seq"], 7);
    assert_eq!(
        observed.browser_value["delivery"][0]["kind"],
        "projection_reset"
    );
    assert_eq!(
        observed.browser_value["delivery"][0]["body"]["projection"]["action_offers"][0]["action_type"],
        "inspect_clue",
    );
    assert_eq!(observed.durable_cursor, None);
    let serialized = observed.browser_value.to_string();
    assert!(!serialized.contains("room_id"));
    assert!(!serialized.contains(ROOM_ID));
    assert!(!serialized.contains("member_id"));
    assert_eq!(
        gateway.health(&authority, None),
        Ok(ParticipantConsoleSessionHealthV1::Usable)
    );
    assert_eq!(
        gateway.health(&authority, None),
        Ok(ParticipantConsoleSessionHealthV1::Invalid)
    );
    assert_eq!(
        gateway.health(&authority, None),
        Ok(ParticipantConsoleSessionHealthV1::Invalid)
    );
    assert_eq!(
        gateway.health(&authority, None),
        Err(ParticipantConsoleGatewayErrorV1::Rejected)
    );
    server
        .join()
        .unwrap_or_else(|error| panic!("daemon fixture thread: {error:?}"));
}

#[tokio::test]
async fn protected_console_keeps_its_browser_frame_head_out_of_the_membership_cursor() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("bind cursor fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("cursor fixture address: {error}"));
    let server = spawn_cursor_enforcing_daemon_fixture(listener);
    let router = fixed_gateway_app(address);
    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();

    let initial = console_request(
        &router,
        Method::POST,
        "/api/v1/participant-console/session:observe",
        &cookie,
        Body::from(r#"{"after_frame_seq":null}"#),
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(json_response(initial).await["frame_head"], 7);

    let refresh = console_request(
        &router,
        Method::POST,
        "/api/v1/participant-console/session:observe",
        &cookie,
        Body::from(r#"{"after_frame_seq":7}"#),
    )
    .await;
    assert_eq!(refresh.status(), StatusCode::OK);

    let accepted = console_request(
        &router,
        Method::POST,
        "/api/v1/participant-console/session:act",
        &cookie,
        Body::from(
            r#"{"action_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","based_on_room_seq":7,"offer_id":"7:host_launch:0","schema_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","action_type":"host_launch","payload":{}}"#,
        ),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted = json_response(accepted).await;
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(
        accepted["receipt"]["action_id"],
        "01ARZ3NDEKTSV4RRFFQ69G5FAY"
    );
    assert!(accepted["receipt"].get("room_id").is_none());
    assert!(accepted["receipt"]["room_head"].get("room_id").is_none());

    let reconnect = console_request(
        &router,
        Method::GET,
        "/api/v1/participant-console/session",
        &cookie,
        Body::empty(),
    )
    .await;
    assert_eq!(reconnect.status(), StatusCode::OK);

    server
        .join()
        .unwrap_or_else(|error| panic!("cursor fixture thread: {error:?}"));
}

#[tokio::test]
async fn protected_console_returns_a_sanitized_rejected_action_receipt() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("bind rejected receipt fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("rejected receipt fixture address: {error}"));
    let server = thread::spawn(move || {
        serve_membership_status_fixture_connection(&listener);
        for action_receipt in [None, None, Some("action.rejected")] {
            serve_cursor_enforcing_connection(&listener, action_receipt);
        }
    });
    let router = fixed_gateway_app(address);
    let (_, handoff) = issue_handoff(&router).await;
    let redeemed = redeem_handoff(&router, &handoff, None).await;
    let cookie = redeemed
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();

    let rejected = console_request(
        &router,
        Method::POST,
        "/api/v1/participant-console/session:act",
        &cookie,
        Body::from(
            r#"{"action_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","based_on_room_seq":7,"offer_id":"7:inspect_clue:0","schema_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","action_type":"inspect_clue","payload":{"clue_id":"route"}}"#,
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected = json_response(rejected).await;
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["receipt"]["code"], "stale_room_head");
    assert!(rejected["receipt"].get("room_id").is_none());
    assert!(rejected["receipt"].get("member_id").is_none());

    server
        .join()
        .unwrap_or_else(|error| panic!("rejected receipt fixture thread: {error:?}"));
}

fn spawn_daemon_fixture(listener: TcpListener) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for (index, standing) in ["enabled", "enabled", "suspended", "departed", "enabled"]
            .into_iter()
            .enumerate()
        {
            serve_daemon_fixture_connection(&listener, index, standing);
        }
    })
}

fn spawn_cursor_enforcing_daemon_fixture(listener: TcpListener) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        serve_membership_status_fixture_connection(&listener);
        for action_receipt in [
            None,
            None,
            None,
            None,
            None,
            None,
            Some("action.accepted"),
            None,
            None,
        ] {
            serve_cursor_enforcing_connection(&listener, action_receipt);
        }
    })
}

fn serve_membership_status_fixture_connection(listener: &TcpListener) {
    use std::io::{Read as _, Write as _};

    let (mut stream, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept membership-status fixture: {error}"));
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        assert!(request.len() < 4096, "membership-status request is bounded");
        let mut byte = [0_u8];
        stream
            .read_exact(&mut byte)
            .unwrap_or_else(|error| panic!("read membership-status request: {error}"));
        request.push(byte[0]);
    }
    assert!(request.starts_with(
        format!("GET /v1/rooms/{ROOM_ID}/members/{MEMBER_ID}/status HTTP/1.1\r\n").as_bytes()
    ));
    assert!(
        request
            .windows(BEARER.len())
            .any(|bytes| bytes == BEARER.as_bytes()),
        "membership-status request authenticates the exact Membership bearer"
    );
    let body = serde_json::to_vec(&json!({
        "version": "membership_status.v1",
        "room_id": ROOM_ID,
        "member_id": MEMBER_ID,
        "principal_kind": "human",
        "access_mode": "participant",
        "role": "navigator",
        "membership_status": "enabled",
        "pack": agent_heist_pack("0.2.0", AGENT_HEIST_0_2_DIGEST),
    }))
    .unwrap_or_else(|error| panic!("encode membership-status response: {error}"));
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap_or_else(|error| panic!("write membership-status response head: {error}"));
    stream
        .write_all(&body)
        .unwrap_or_else(|error| panic!("write membership-status response body: {error}"));
}

#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "the callback result is fixed by tungstenite's handshake API"
)]
fn authorize_fixture_handshake(
    request: &tungstenite::handshake::server::Request,
    mut response: tungstenite::handshake::server::Response,
) -> Result<tungstenite::handshake::server::Response, tungstenite::handshake::server::ErrorResponse>
{
    let expected_authorization = format!("Bearer {BEARER}");
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some(expected_authorization.as_str()),
    );
    response.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        worldstream_protocol::WEBSOCKET_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| panic!("fixture protocol header: {error}")),
    );
    Ok(response)
}

fn serve_daemon_fixture_connection(listener: &TcpListener, index: usize, standing: &str) {
    let (stream, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept daemon fixture: {error}"));
    let mut socket = accept_hdr(stream, authorize_fixture_handshake)
        .unwrap_or_else(|error| panic!("websocket fixture handshake: {error}"));
    let _hello = socket
        .read()
        .unwrap_or_else(|error| panic!("read hello: {error}"));
    send_fixture_message(&mut socket, "server.welcome", &json!({}));
    let attach = socket
        .read()
        .unwrap_or_else(|error| panic!("read attach: {error}"));
    let attach: Value = match attach {
        Message::Text(text) => serde_json::from_str(text.as_str())
            .unwrap_or_else(|error| panic!("attach json: {error}")),
        other => panic!("unexpected attach message: {other:?}"),
    };
    assert_eq!(attach["body"]["room_id"], ROOM_ID);
    assert_eq!(attach["body"]["member_id"], MEMBER_ID);
    let attached_member = if index == 4 { DRAFT_ID } else { MEMBER_ID };
    send_fixture_message(
        &mut socket,
        "room.attached",
        &fixture_attached(attached_member, standing),
    );
    if index == 0 {
        send_fixture_message(&mut socket, "projection.reset", &fixture_projection_reset());
        socket
            .get_mut()
            .shutdown(std::net::Shutdown::Both)
            .unwrap_or_else(|error| panic!("close projection fixture: {error}"));
    }
}

fn serve_cursor_enforcing_connection(listener: &TcpListener, action_receipt: Option<&str>) {
    let (stream, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept cursor fixture: {error}"));
    let mut socket = accept_hdr(stream, authorize_fixture_handshake)
        .unwrap_or_else(|error| panic!("cursor fixture handshake: {error}"));
    let _hello = socket
        .read()
        .unwrap_or_else(|error| panic!("read cursor hello: {error}"));
    send_fixture_message(&mut socket, "server.welcome", &json!({}));
    let attach = read_fixture_message(&mut socket, "cursor attach");
    assert_eq!(attach["type"], "room.attach");
    assert_eq!(attach["body"]["room_id"], ROOM_ID);
    assert_eq!(attach["body"]["member_id"], MEMBER_ID);
    // The protected Console is read-only with respect to Membership Cursor. A
    // rendered frame head is never an ACK and therefore must remain null here.
    assert_eq!(attach["body"]["after_frame_seq"], Value::Null);
    send_fixture_message(
        &mut socket,
        "room.attached",
        &fixture_attached(MEMBER_ID, "enabled"),
    );
    if let Some(action_receipt) = action_receipt {
        let sync_ack = read_fixture_message(&mut socket, "cursor sync ack");
        assert_eq!(sync_ack["type"], "room.sync_ack");
        send_fixture_message(&mut socket, "room.sync_acked", &json!({}));
        let action = read_fixture_message(&mut socket, "cursor action");
        assert_eq!(action["type"], "action.submit");
        let receipt = if action_receipt == "action.accepted" {
            json!({
                "room_id": ROOM_ID,
                "member_id": MEMBER_ID,
                "action_id": "01ARZ3NDEKTSV4RRFFQ69G5FAY",
                "transition_id": "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
                "admitted_at": "2026-09-01T12:00:00Z",
                "room_head": fixture_room_head(),
                "duplicate": false
            })
        } else {
            json!({
                "room_id": ROOM_ID,
                "member_id": MEMBER_ID,
                "action_id": "01ARZ3NDEKTSV4RRFFQ69G5FAY",
                "admitted_at": "2026-09-01T12:00:00Z",
                "code": "stale_room_head",
                "message": "Synchronize and submit a new Action.",
                "current_room_seq": 8,
                "action_offers": [],
                "retryable_with_same_action_id": false,
                "may_submit_revised_action": true,
                "duplicate": false,
                "details": {}
            })
        };
        send_fixture_message(&mut socket, action_receipt, &receipt);
    }
}

fn read_fixture_message(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    context: &str,
) -> Value {
    match socket
        .read()
        .unwrap_or_else(|error| panic!("read {context}: {error}"))
    {
        Message::Text(text) => serde_json::from_str(text.as_str())
            .unwrap_or_else(|error| panic!("parse {context}: {error}")),
        other => panic!("unexpected {context} message: {other:?}"),
    }
}

fn fixture_attached(member_id: &str, standing: &str) -> Value {
    json!({
        "room_id": ROOM_ID, "member_id": member_id, "principal_kind": "human",
        "access_mode": "participant", "role": "navigator", "membership_status": standing,
        "room_status": "active", "room_health": "healthy", "integrity_generation": 1,
        "room_head": fixture_room_head(), "cursor": null, "frame_head": 7, "retained_floor": 0,
        "sync_token": "fixture-sync-token",
        "sync": {"kind":"projection_reset","baseline_frame_head":7,"reason":"initial_attach"},
        "pack": {"id":"agent-heist","version":"0.2.0","digest":format!("blake3:{}", "2".repeat(64))}
    })
}

fn fixture_projection_reset() -> Value {
    json!({
        "room_id": ROOM_ID, "member_id": MEMBER_ID, "room_head": fixture_room_head(),
        "room_health": "healthy", "integrity_generation": 1, "baseline_frame_head": 7,
        "reset_reason": "initial_attach", "projection_schema": "agent-heist.participant.v1",
        "projection": {"core": {}, "activity": {"phase":"Lobby"}, "action_offers": [{
            "domain":"activity", "action_type":"inspect_clue",
            "payload_schema_digest":format!("blake3:{}", "a".repeat(64))
        }]},
        "projection_hash": format!("blake3:{}", "b".repeat(64))
    })
}

fn fixture_room_head() -> Value {
    json!({
        "room_id": ROOM_ID,
        "room_seq": 7,
        "genesis_or_transition_hash": format!("blake3:{}", "1".repeat(64)),
        "core_schema_version": "core.v1",
        "pack_digest": format!("blake3:{}", "2".repeat(64)),
        "core_state_hash": format!("blake3:{}", "3".repeat(64)),
        "activity_state_hash": format!("blake3:{}", "4".repeat(64)),
        "authoritative_state_hash": format!("blake3:{}", "5".repeat(64))
    })
}

fn send_fixture_message(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    kind: &str,
    body: &Value,
) {
    socket
        .send(Message::Text(
            json!({
                "protocol":"worldstream.v1",
                "type":kind,
                "message_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY",
                "request_id":null,
                "body":body
            })
            .to_string()
            .into(),
        ))
        .unwrap_or_else(|error| panic!("send fixture {kind}: {error}"));
}

#[cfg(all(unix, debug_assertions))]
#[test]
#[allow(
    clippy::expect_used,
    reason = "Isolated fixture setup failures must fail the child test"
)]
fn native_membership_diagnostic_child() {
    let Ok(address) = std::env::var("WORLDSTREAM_TEST_MEMBERSHIP_PEER") else {
        return;
    };
    let authority = FakeAuthoritySource::usable()
        .resolve_provisioned_human_seat(DRAFT_ID, "navigator")
        .expect("synthetic authority");
    let gateway = FixedDaemonParticipantConsoleGatewayV1::new(
        address.parse().expect("owned loopback peer"),
        Duration::from_millis(75),
    );
    let result = gateway.membership_status(&authority);
    let expected = if std::env::var("WORLDSTREAM_TEST_MEMBERSHIP_CASE").as_deref() == Ok("timeout")
    {
        ParticipantConsoleGatewayErrorV1::Disconnected
    } else {
        ParticipantConsoleGatewayErrorV1::Unavailable
    };
    assert_eq!(result.err(), Some(expected));
}

#[cfg(all(unix, debug_assertions))]
#[test]
#[allow(
    clippy::expect_used,
    reason = "Isolated fixture setup failures must fail the regression"
)]
fn native_membership_diagnostic_preserves_rate_limit_and_timeout_results() {
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Instant;

    for (case, expected) in [
        (
            "rate_limit",
            json!({"stage":"membership_status_http","status":429,"category":"rate_limited"}),
        ),
        (
            "timeout",
            json!({"stage":"membership_status_read","status":null,"category":"timeout"}),
        ),
    ] {
        let directory = tempfile::tempdir().expect("private diagnostic directory");
        let trace = std::fs::canonicalize(directory.path())
            .expect("real private directory")
            .join("native.jsonl");
        std::fs::write(&trace, []).expect("create diagnostic file");
        std::fs::set_permissions(&trace, std::fs::Permissions::from_mode(0o600))
            .expect("owner-only diagnostic file");
        let listener = TcpListener::bind("127.0.0.1:0").expect("owned HTTP peer");
        let address = listener.local_addr().expect("peer address");
        listener.set_nonblocking(true).expect("bounded accept");
        let peer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut connection = loop {
                match listener.accept() {
                    Ok((connection, _)) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "diagnostic child did not connect"
                        );
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => panic!("fixture accept failed"),
                }
            };
            connection
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("bounded read");
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 4096);
                let mut byte = [0_u8];
                connection
                    .read_exact(&mut byte)
                    .expect("complete fixture request");
                request.push(byte[0]);
            }
            assert!(
                request.starts_with(
                    format!("GET /v1/rooms/{ROOM_ID}/members/{MEMBER_ID}/status HTTP/1.1\r\n")
                        .as_bytes()
                )
            );
            assert!(
                request
                    .windows(BEARER.len())
                    .any(|bytes| bytes == BEARER.as_bytes())
            );
            if case == "timeout" {
                thread::sleep(Duration::from_millis(200));
            } else {
                let body = b"private-upstream-body-sentinel";
                write!(connection, "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("HTTP response");
                connection.write_all(body).expect("response body");
            }
            listener.set_nonblocking(true).expect("final accept check");
            assert!(listener.accept().is_err(), "diagnostics must not retry");
        });
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "native_membership_diagnostic_child",
                "--nocapture",
            ])
            .env("CI", "true")
            .env("WORLDSTREAM_REENTRY_DIAGNOSTICS", "visible-local-only")
            .env("WORLDSTREAM_REENTRY_NATIVE_TRACE_FILE", &trace)
            .env("WORLDSTREAM_TEST_MEMBERSHIP_PEER", address.to_string())
            .env("WORLDSTREAM_TEST_MEMBERSHIP_CASE", case)
            .output()
            .expect("bounded diagnostic child");
        peer.join().expect("fixture peer completed");
        assert!(output.status.success(), "public adapter result changed");
        let stored = std::fs::read_to_string(&trace).expect("read diagnostic receipt");
        let records = stored
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("closed JSON record"))
            .collect::<Vec<_>>();
        assert_eq!(records, vec![expected]);
        assert!(!stored.contains(BEARER));
        assert!(!stored.contains("private-upstream-body-sentinel"));
    }
}
