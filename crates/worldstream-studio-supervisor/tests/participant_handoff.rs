#![allow(clippy::panic)]

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
    FixedDaemonParticipantConsoleGatewayV1, HumanSeatAuthorityV1, ParticipantConsoleGatewayErrorV1,
    ParticipantConsoleGatewayV1, ParticipantConsoleObservationV1,
    ParticipantConsoleReadinessSourceV1, ParticipantConsoleSessionHealthV1,
    ParticipantHandoffAuthorityErrorV1, ParticipantHandoffAuthoritySourceV1,
    ParticipantHandoffBrokerV1, participant_handoff_router,
};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tungstenite::{Message, accept_hdr};
use worldstream_protocol::SealedCapabilityBearerV1;

const STUDIO_ORIGIN: &str = "http://127.0.0.1:5174";
const CONSOLE_ORIGIN: &str = "http://127.0.0.1:5173";
const DRAFT_ID: &str = "draft-participant-handoff";
const ROOM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const BEARER: &str = "wsb1:abababababababababababababababababababababababababababababababab";

#[derive(Clone)]
struct FakeAuthoritySource {
    state: Arc<Mutex<Result<(), ParticipantHandoffAuthorityErrorV1>>>,
}

impl FakeAuthoritySource {
    fn usable() -> Self {
        Self {
            state: Arc::new(Mutex::new(Ok(()))),
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
            SealedCapabilityBearerV1::parse(BEARER.to_owned())
                .map_err(|_| ParticipantHandoffAuthorityErrorV1::Unavailable)?,
        )
        .map_err(|_| ParticipantHandoffAuthorityErrorV1::Unavailable)
    }
}

type GatewayCall = (String, String, Option<u64>, &'static str);

#[derive(Clone, Default)]
struct FakeGateway {
    calls: Arc<Mutex<Vec<GatewayCall>>>,
}

impl ParticipantConsoleGatewayV1 for FakeGateway {
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

fn app(source: FakeAuthoritySource, gateway: FakeGateway) -> axum::Router {
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        source,
        gateway,
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
        Some(2),
        "handoff response must stay exact and browser-safe"
    );
    let console_url = body["console_url"].as_str().unwrap_or_default().to_owned();
    let handoff = console_url
        .split("#handoff=")
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    (console_url, handoff)
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

    assert!(console_url.starts_with("http://127.0.0.1:5173/#handoff=wsh1:"));
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
    assert_eq!(
        json_response(response).await["code"],
        "participant_handoff_human_seat_required"
    );
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
    #[derive(Clone)]
    struct LeakingGateway;
    impl ParticipantConsoleGatewayV1 for LeakingGateway {
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
    let broker = ParticipantHandoffBrokerV1::new(
        STUDIO_ORIGIN,
        CONSOLE_ORIGIN,
        Duration::from_secs(30),
        16,
        FakeAuthoritySource::usable(),
        LeakingGateway,
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
        "ready",
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
        for accepts_action in [false, false, true, false] {
            serve_cursor_enforcing_connection(&listener, accepts_action);
        }
    })
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

fn serve_cursor_enforcing_connection(listener: &TcpListener, accepts_action: bool) {
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
    if accepts_action {
        let sync_ack = read_fixture_message(&mut socket, "cursor sync ack");
        assert_eq!(sync_ack["type"], "room.sync_ack");
        send_fixture_message(&mut socket, "room.sync_acked", &json!({}));
        let action = read_fixture_message(&mut socket, "cursor action");
        assert_eq!(action["type"], "action.submit");
        send_fixture_message(&mut socket, "action.accepted", &json!({"state":"accepted"}));
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
            "domain":"activity", "action_type":"ready",
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
