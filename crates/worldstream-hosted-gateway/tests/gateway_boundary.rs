#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread,
    time::Duration,
};

use axum::{body::Body, http::Request};
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::BodyExt as _;
use serde::Serialize;
use serde_json::{Value, json};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message as TungsteniteMessage,
        client::IntoClientRequest as _,
        handshake::server::{Request as WebSocketRequest, Response as WebSocketResponse},
    },
};
use tower::ServiceExt as _;
use worldstream_core::{CanonicalJsonV1, projection_hash_for_canonical_bytes};
use worldstream_hosted_contract::{
    HostedAuthorizedPublicProjectionV1, HostedBrowserHandoffRedeemRequestV1,
    HostedBrowserHandoffRedeemResponseV1, HostedBrowserHandoffRequestV1,
    HostedBrowserHandoffResponseV1, HostedBrowserSessionLogoutV1, HostedBrowserSessionRequestV1,
    HostedBrowserSessionStateV1, HostedBrowserSessionStatusV1, HostedBrowserStreamTicketRequestV1,
    HostedBrowserStreamTicketResponseV1, HostedCapacityAuthorizationV1, HostedGenesisAccessModeV1,
    HostedGenesisEvidenceV1, HostedGenesisHeadV1, HostedGenesisMembershipPurposeV1,
    HostedGenesisMembershipV1, HostedGenesisPrincipalKindV1, HostedHouseRunnerReservationOutcomeV1,
    HostedHouseRunnerReservationReceiptV1, HostedHouseRunnerReservationRequestV1,
    HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1, HostedLaunchStageV1,
    HostedLaunchStatusV1, HostedPublicRelayBindReceiptV1, HostedPublicRelayBindRequestV1,
    HostedPublicStreamTicketRequestV1, HostedResultIntegrityStatusV1, HostedResultReplayEvidenceV1,
    HostedResultSourceEvidenceV1, HostedResultSourceHeadV1, HostedResultSourceRequestV1,
    PackReference,
};
use worldstream_hosted_gateway::{
    FixedHostAdapterBackend, HostedGatewayBackend, HostedGatewayConfig, HostedGatewayError,
    hosted_gateway_router,
};

const LISTING: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "service-authority-that-never-leaves-fly";

#[test]
#[cfg(debug_assertions)]
fn native_handoff_trace_preserves_outcomes_and_never_echoes_payloads() {
    for (case, status, stage, code) in [
        ("no_response", None, "no_usable_http_response", "none"),
        ("429", Some(429), "non_201", "unclassified"),
        ("capacity", Some(503), "non_201", "hosted_browser_capacity"),
        ("unknown", Some(503), "non_201", "unclassified"),
        ("malformed", Some(201), "malformed_201", "none"),
        ("success", Some(201), "success_201", "none"),
    ] {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "native_handoff_trace_probe", "--nocapture"])
            .env("WORLDSTREAM_NATIVE_TRACE_CASE", case)
            .env("WORLDSTREAM_REENTRY_DIAGNOSTICS", "visible-local-only")
            .env("CI", "true")
            .output()
            .expect("bounded native probe");
        assert!(output.status.success(), "probe outcome changed for {case}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        let lines = stderr
            .lines()
            .filter_map(|line| line.strip_prefix("[DEBUG-reentry-native] "))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1, "one observation per original request");
        let diagnostic: Value = serde_json::from_str(lines[0]).expect("diagnostic JSON");
        assert_eq!(
            diagnostic,
            json!({"stage":stage,"status":status,"code":code})
        );
        assert!(!stderr.contains(TOKEN));
        assert!(!stderr.contains("private-sentinel"));
        assert!(!stderr.contains("wsh1:"));
    }
    for (ci, diagnostic) in [
        (None, Some("visible-local-only")),
        (Some("true"), None),
        (Some("true"), Some("wrong")),
    ] {
        let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
        command
            .args(["--exact", "native_handoff_trace_probe", "--nocapture"])
            .env("WORLDSTREAM_NATIVE_TRACE_CASE", "capacity")
            .env_remove("CI")
            .env_remove("WORLDSTREAM_REENTRY_DIAGNOSTICS");
        if let Some(ci) = ci {
            command.env("CI", ci);
        }
        if let Some(diagnostic) = diagnostic {
            command.env("WORLDSTREAM_REENTRY_DIAGNOSTICS", diagnostic);
        }
        let output = command.output().expect("disabled probe");
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("[DEBUG-reentry-native]"));
    }
}

#[test]
fn native_handoff_trace_probe() {
    let Ok(case) = std::env::var("WORLDSTREAM_NATIVE_TRACE_CASE") else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").expect("owned loopback fixture");
    let address = listener.local_addr().expect("fixture address");
    let expected_success = case == "success";
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("one upstream request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bounded fixture read");
        let (path, authorization, _, mut stream) = read_request(stream);
        assert!(path.starts_with("POST /api/v1/hosted-browser-handoffs:issue "));
        assert_eq!(authorization, format!("Bearer {TOKEN}"));
        if case == "no_response" {
            return;
        }
        let (status, body) = match case.as_str() {
            "429" => (429, json!({"error":{"code":"private-sentinel"}}).to_string()),
            "capacity" => (503, json!({"error":{"code":"hosted_browser_capacity","message":"private-sentinel"}}).to_string()),
            "unknown" => (503, json!({"error":{"code":"private-sentinel"}}).to_string()),
            "malformed" => (201, "private-sentinel".to_owned()),
            "success" => (201, json!({"schema":"worldstream/hosted-browser-handoff-response/v1","client_url":format!("https://arena.example/client/#handoff=wsh1:{}", "a".repeat(64))}).to_string()),
            _ => unreachable!("closed fixture case"),
        };
        write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("fixture response");
    });
    let backend = FixedHostAdapterBackend::new(address, TOKEN.to_owned(), Duration::from_secs(2))
        .expect("literal adapter");
    let result = backend.issue_browser_handoff(&browser_handoff_request(LISTING));
    server.join().expect("one original request");
    if expected_success {
        assert!(result.is_ok());
    } else {
        assert_eq!(result, Err(HostedGatewayError::Unavailable));
    }
}

#[derive(Clone, Default)]
struct Backend {
    launches: Arc<Mutex<Vec<HostedLaunchRequestV1>>>,
    evidence_reads: Arc<Mutex<Vec<HostedLaunchEvidenceRequestV1>>>,
    genesis_reads: Arc<Mutex<Vec<HostedLaunchEvidenceRequestV1>>>,
    result_source_reads: Arc<Mutex<Vec<HostedResultSourceRequestV1>>>,
    house_reservations: Arc<Mutex<Vec<HostedHouseRunnerReservationRequestV1>>>,
    house_reads: Arc<Mutex<Vec<HostedHouseRunnerReservationRequestV1>>>,
    browser_handoffs: Arc<Mutex<Vec<HostedBrowserHandoffRequestV1>>>,
    browser_redemptions: Arc<Mutex<Vec<HostedBrowserHandoffRedeemRequestV1>>>,
    browser_status_reads: Arc<Mutex<Vec<HostedBrowserSessionRequestV1>>>,
    browser_logouts: Arc<Mutex<Vec<HostedBrowserSessionRequestV1>>>,
    browser_stream_tickets: Arc<Mutex<Vec<HostedBrowserStreamTicketRequestV1>>>,
    public_relay_binds: Arc<Mutex<Vec<HostedPublicRelayBindRequestV1>>>,
    public_stream_lookups: Arc<Mutex<Vec<HostedPublicStreamTicketRequestV1>>>,
    ready: bool,
}

impl HostedGatewayBackend for Backend {
    fn ready(&self) -> bool {
        self.ready
    }

    fn launch(
        &self,
        request: &HostedLaunchRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError> {
        self.launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(status(
            &request.listing_revision_digest,
            &request.launch_request_digest,
            &request.room_setup_operation_id,
        ))
    }

    fn evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError> {
        self.evidence_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(status(
            &request.listing_revision_digest,
            &request.launch_request_digest,
            &request.room_setup_operation_id,
        ))
    }

    fn genesis_evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedGenesisEvidenceV1, HostedGatewayError> {
        self.genesis_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(genesis(request))
    }

    fn result_source_evidence(
        &self,
        request: &HostedResultSourceRequestV1,
    ) -> Result<HostedResultSourceEvidenceV1, HostedGatewayError> {
        self.result_source_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(result_source_evidence(request))
    }

    fn reserve_house_runner(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        self.house_reservations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(house_receipt(request))
    }

    fn read_house_runner(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        self.house_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(house_receipt(request))
    }

    fn issue_browser_handoff(
        &self,
        request: &HostedBrowserHandoffRequestV1,
    ) -> Result<HostedBrowserHandoffResponseV1, HostedGatewayError> {
        self.browser_handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserHandoffResponseV1 {
            schema: "worldstream/hosted-browser-handoff-response/v1".to_owned(),
            client_url: format!(
                "https://arena.example/clients/heist/#handoff=wsh1:{}",
                "a".repeat(64)
            ),
        })
    }

    fn redeem_browser_handoff(
        &self,
        request: &HostedBrowserHandoffRedeemRequestV1,
    ) -> Result<HostedBrowserHandoffRedeemResponseV1, HostedGatewayError> {
        self.browser_redemptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserHandoffRedeemResponseV1 {
            schema: "worldstream/hosted-browser-handoff-redeem-response/v1".to_owned(),
            session: format!("wss1:{}", "b".repeat(64)),
        })
    }

    fn browser_session_status(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionStatusV1, HostedGatewayError> {
        self.browser_status_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserSessionStatusV1 {
            schema: "worldstream/hosted-browser-session-status/v1".to_owned(),
            state: HostedBrowserSessionStateV1::Usable,
        })
    }

    fn logout_browser_session(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionLogoutV1, HostedGatewayError> {
        self.browser_logouts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserSessionLogoutV1 {
            schema: "worldstream/hosted-browser-session-logout/v1".to_owned(),
            logged_out: true,
        })
    }

    fn issue_browser_stream_ticket(
        &self,
        request: &HostedBrowserStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedGatewayError> {
        self.browser_stream_tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserStreamTicketResponseV1 {
            schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
            ticket: format!("wst1:{}", "c".repeat(64)),
            expires_in_ms: 15_000,
        })
    }

    fn bind_public_relay(
        &self,
        request: &HostedPublicRelayBindRequestV1,
    ) -> Result<HostedPublicRelayBindReceiptV1, HostedGatewayError> {
        self.public_relay_binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedPublicRelayBindReceiptV1 {
            schema: "worldstream/hosted-public-relay-bind-receipt/v1".to_owned(),
            public_run_id: request.public_run_id.clone(),
            activity_run_id: request.activity_run_id.clone(),
            binding_request_digest: format!("sha256:{}", "d".repeat(64)),
            bound: true,
        })
    }

    fn issue_public_stream_ticket(
        &self,
        request: &HostedPublicStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedGatewayError> {
        self.public_stream_lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(HostedBrowserStreamTicketResponseV1 {
            schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
            ticket: format!("wst1:{}", "c".repeat(64)),
            expires_in_ms: 15_000,
        })
    }
}

fn status(listing: &str, launch: &str, operation: &str) -> HostedLaunchStatusV1 {
    HostedLaunchStatusV1 {
        schema: "worldstream/hosted-launch-status/v1".to_owned(),
        listing_revision_digest: listing.to_owned(),
        launch_request_digest: launch.to_owned(),
        room_setup_operation_id: operation.to_owned(),
        room_id: None,
        stage: HostedLaunchStageV1::Bound,
        room_setup_complete: false,
        lobby_launch_committed: false,
        retryable: true,
        terminal_before_genesis: false,
    }
}

fn genesis(request: &HostedLaunchEvidenceRequestV1) -> HostedGenesisEvidenceV1 {
    let digest = |value: char| format!("blake3:{}", value.to_string().repeat(64));
    let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned();
    HostedGenesisEvidenceV1 {
        schema: "worldstream/hosted-genesis-evidence/v1".to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        listing_revision_digest: request.listing_revision_digest.clone(),
        launch_request_digest: request.launch_request_digest.clone(),
        frozen_roster_digest: format!("sha256:{}", "b".repeat(64)),
        room_setup_specification_digest: digest('c'),
        room_setup_operation_id: request.room_setup_operation_id.clone(),
        room_id: room_id.clone(),
        pack: PackReference {
            id: "worldstream.test".to_owned(),
            version: "1.0.0".to_owned(),
            digest: digest('d'),
        },
        genesis_head: HostedGenesisHeadV1 {
            room_id,
            room_seq: 0,
            genesis_or_transition_hash: digest('e'),
            core_schema_version: "worldstream/core-room-state/v1".to_owned(),
            pack_digest: digest('d'),
            core_state_hash: digest('1'),
            activity_state_hash: digest('2'),
            authoritative_state_hash: digest('3'),
        },
        memberships: vec![
            HostedGenesisMembershipV1 {
                access_mode: HostedGenesisAccessModeV1::Participant,
                purpose: HostedGenesisMembershipPurposeV1::Participant,
                seat_id: Some("navigator".to_owned()),
                role: Some("navigator".to_owned()),
                principal_kind: HostedGenesisPrincipalKindV1::Human,
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
                membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
                scopes: vec![],
            },
            HostedGenesisMembershipV1 {
                access_mode: HostedGenesisAccessModeV1::Spectator,
                purpose: HostedGenesisMembershipPurposeV1::ResultIndexer,
                seat_id: None,
                role: None,
                principal_kind: HostedGenesisPrincipalKindV1::Agent,
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
                membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
                scopes: vec![
                    "room:attach".to_owned(),
                    "room:observe_public".to_owned(),
                    "room:replay".to_owned(),
                ],
            },
        ],
    }
}

fn config(max_requests: u32) -> HostedGatewayConfig {
    HostedGatewayConfig::new(
        "preview-abc123",
        TOKEN,
        BTreeSet::from([LISTING.to_owned()]),
        "arena.example",
        "127.0.0.1:9310"
            .parse::<SocketAddr>()
            .expect("loopback fixture"),
        max_requests,
        Duration::from_mins(1),
    )
    .expect("gateway fixture")
}

fn browser_stream_config() -> HostedGatewayConfig {
    browser_stream_config_with_runtime(
        "127.0.0.1:9"
            .parse::<SocketAddr>()
            .expect("loopback discard fixture"),
    )
}

fn browser_stream_config_with_runtime(runtime_upstream: SocketAddr) -> HostedGatewayConfig {
    config(64)
        .with_browser_stream(runtime_upstream, "https://arena.example")
        .expect("browser stream fixture")
}

#[test]
fn fixed_host_adapter_requires_loopback_strong_authority_and_bounded_timeout() {
    let loopback = "127.0.0.1:9420"
        .parse::<SocketAddr>()
        .expect("loopback fixture");
    assert!(
        FixedHostAdapterBackend::new(loopback, "weak".to_owned(), Duration::from_secs(5)).is_err()
    );
    assert!(
        FixedHostAdapterBackend::new(
            "192.0.2.1:9420".parse().expect("remote fixture"),
            TOKEN.to_owned(),
            Duration::from_secs(5),
        )
        .is_err()
    );
    assert!(
        FixedHostAdapterBackend::new(loopback, TOKEN.to_owned(), Duration::from_secs(31)).is_err()
    );
    assert!(
        FixedHostAdapterBackend::new(loopback, TOKEN.to_owned(), Duration::from_secs(5)).is_ok()
    );
}

#[test]
fn configuration_requires_exact_listings_strong_authority_and_loopback_upstream() {
    assert!(
        HostedGatewayConfig::new(
            "preview-abc123",
            "weak",
            BTreeSet::from([LISTING.to_owned()]),
            "arena.example",
            "127.0.0.1:9310"
                .parse::<SocketAddr>()
                .expect("loopback fixture"),
            8,
            Duration::from_mins(1),
        )
        .is_err()
    );
    assert!(
        HostedGatewayConfig::new(
            "preview-abc123",
            TOKEN,
            BTreeSet::from(["latest".to_owned()]),
            "arena.example",
            "127.0.0.1:9310"
                .parse::<SocketAddr>()
                .expect("loopback fixture"),
            8,
            Duration::from_mins(1),
        )
        .is_err()
    );
    assert!(
        HostedGatewayConfig::new(
            "preview-abc123",
            TOKEN,
            BTreeSet::from([LISTING.to_owned()]),
            "arena.example",
            "192.0.2.1:9310"
                .parse::<SocketAddr>()
                .expect("remote fixture"),
            8,
            Duration::from_mins(1),
        )
        .is_err()
    );
}

fn launch_request(listing: &str) -> HostedLaunchRequestV1 {
    HostedLaunchRequestV1 {
        schema: "worldstream/hosted-launch-request/v1".to_owned(),
        listing_revision_digest: listing.to_owned(),
        launch_request_digest:
            "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        launch_input_digest:
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
        frozen_roster_digest:
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_owned(),
        room_setup_specification_digest:
            "blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        capacity_authorization: HostedCapacityAuthorizationV1 {
            schema: "worldstream/platform-capacity-authorization/v1".to_owned(),
            host_installation_id: "hosted-test".to_owned(),
            reservation_reference: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        },
        house_runner_assignments: vec![],
        frozen_launch_request: json!({}),
        frozen_roster: json!({}),
        frozen_room_setup_specification: json!({}),
    }
}

fn evidence_request(listing: &str) -> HostedLaunchEvidenceRequestV1 {
    let launch = launch_request(listing);
    HostedLaunchEvidenceRequestV1 {
        schema: "worldstream/hosted-launch-evidence-request/v1".to_owned(),
        listing_revision_digest: launch.listing_revision_digest,
        launch_request_digest: launch.launch_request_digest,
        room_setup_operation_id: launch.room_setup_operation_id,
    }
}

fn result_source_request(listing: &str) -> HostedResultSourceRequestV1 {
    let launch = launch_request(listing);
    HostedResultSourceRequestV1 {
        schema: "worldstream/hosted-result-source-request/v1".to_owned(),
        run_id: "30000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: launch.listing_revision_digest,
        launch_request_digest: launch.launch_request_digest,
        room_setup_operation_id: launch.room_setup_operation_id,
    }
}

fn result_source_evidence(request: &HostedResultSourceRequestV1) -> HostedResultSourceEvidenceV1 {
    let digest = |value: char| format!("blake3:{}", value.to_string().repeat(64));
    let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned();
    let projection = HostedAuthorizedPublicProjectionV1 {
        projection_schema: "worldstream.test/projection/v1".to_owned(),
        authorized_core: json!({"access_mode":"spectator"}),
        projection: json!({"status":"complete"}),
        action_offers: vec![],
    };
    let projection_bytes = serde_json::to_vec(&projection)
        .ok()
        .and_then(|bytes| CanonicalJsonV1::parse(&bytes).ok())
        .and_then(|value| value.to_bytes().ok())
        .expect("canonical projection fixture");
    let projection_hash = projection_hash_for_canonical_bytes(&projection_bytes)
        .expect("projection hash fixture")
        .to_string();
    let head = HostedResultSourceHeadV1 {
        room_id: room_id.clone(),
        room_seq: 14,
        genesis_or_transition_hash: digest('e'),
        core_schema_version: "worldstream.core-room-state.v1".to_owned(),
        pack_digest: digest('d'),
        core_state_hash: digest('1'),
        activity_state_hash: digest('2'),
        authoritative_state_hash: digest('3'),
    };
    HostedResultSourceEvidenceV1 {
        schema: "worldstream/hosted-result-source-evidence/v1".to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        run_id: request.run_id.clone(),
        listing_revision_digest: request.listing_revision_digest.clone(),
        launch_request_digest: request.launch_request_digest.clone(),
        room_setup_operation_id: request.room_setup_operation_id.clone(),
        room_id,
        pack: PackReference {
            id: "worldstream.test".to_owned(),
            version: "1.0.0".to_owned(),
            digest: digest('d'),
        },
        result_indexer_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
        access_mode: HostedGenesisAccessModeV1::Spectator,
        source_head: head.clone(),
        integrity_status: HostedResultIntegrityStatusV1::Healthy,
        integrity_generation: 2,
        projection_schema: projection.projection_schema.clone(),
        public_projection: projection,
        projection_hash: projection_hash.clone(),
        replay: Some(HostedResultReplayEvidenceV1 {
            verifier_revision: "worldstream.authorized-replay/v1".to_owned(),
            verified_head: head,
            projection_hash,
            verification_receipt_digest: format!("sha256:{}", "4".repeat(64)),
        }),
    }
}

fn house_request(listing: &str) -> HostedHouseRunnerReservationRequestV1 {
    HostedHouseRunnerReservationRequestV1 {
        schema: "worldstream/house-runner-reservation-request/v1".to_owned(),
        host_installation_id: "hosted-test".to_owned(),
        reservation_operation_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        launch_request_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: listing.to_owned(),
        seat_id: "navigator".to_owned(),
        house_agent_revision_digest:
            "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
    }
}

fn house_receipt(
    request: &HostedHouseRunnerReservationRequestV1,
) -> HostedHouseRunnerReservationReceiptV1 {
    HostedHouseRunnerReservationReceiptV1 {
        schema: "worldstream/house-runner-reservation-receipt/v1".to_owned(),
        host_installation_id: request.host_installation_id.clone(),
        reservation_operation_id: request.reservation_operation_id.clone(),
        launch_request_id: request.launch_request_id.clone(),
        listing_revision_digest: request.listing_revision_digest.clone(),
        seat_id: request.seat_id.clone(),
        house_agent_revision_digest: request.house_agent_revision_digest.clone(),
        outcome: HostedHouseRunnerReservationOutcomeV1::Succeeded,
        runner_unit_id: Some("house-runner-01".to_owned()),
        failure_code: None,
        binding_digest: "blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_owned(),
        authentication_tag: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
            .to_owned(),
    }
}

fn browser_handoff_request(listing: &str) -> HostedBrowserHandoffRequestV1 {
    HostedBrowserHandoffRequestV1 {
        schema: "worldstream/hosted-browser-handoff-request/v1".to_owned(),
        platform_account_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: listing.to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        pack: PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: format!("blake3:{}", "d".repeat(64)),
        },
        client_release_digest: format!("sha256:{}", "e".repeat(64)),
        client_surface_id: "participant".to_owned(),
        access_mode: HostedGenesisAccessModeV1::Participant,
        purpose: HostedGenesisMembershipPurposeV1::Participant,
        seat_id: Some("navigator".to_owned()),
        role: Some("navigator".to_owned()),
        principal_kind: HostedGenesisPrincipalKindV1::Human,
        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
        membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
    }
}

fn browser_redeem_request() -> HostedBrowserHandoffRedeemRequestV1 {
    HostedBrowserHandoffRedeemRequestV1 {
        schema: "worldstream/hosted-browser-handoff-redeem-request/v1".to_owned(),
        platform_account_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        handoff: format!("wsh1:{}", "a".repeat(64)),
        prior_session: None,
    }
}

fn browser_session_request() -> HostedBrowserSessionRequestV1 {
    HostedBrowserSessionRequestV1 {
        schema: "worldstream/hosted-browser-session-request/v1".to_owned(),
        session: format!("wss1:{}", "b".repeat(64)),
    }
}

fn browser_stream_ticket_request() -> HostedBrowserStreamTicketRequestV1 {
    HostedBrowserStreamTicketRequestV1 {
        schema: "worldstream/hosted-browser-stream-ticket-request/v1".to_owned(),
        session: format!("wss1:{}", "b".repeat(64)),
        after_frame_seq: Some(17),
    }
}

fn public_relay_bind_request() -> HostedPublicRelayBindRequestV1 {
    HostedPublicRelayBindRequestV1 {
        schema: "worldstream/hosted-public-relay-bind-request/v1".to_owned(),
        public_run_id: "0123456789abcdef0123456789abcdef".to_owned(),
        activity_run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        launch_request_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: LISTING.to_owned(),
        launch_request_digest: format!("blake3:{}", "b".repeat(64)),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        pack: PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: format!("blake3:{}", "d".repeat(64)),
        },
        relay_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
        relay_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
    }
}

#[test]
fn public_relay_adapter_request_is_canonicalizable() {
    let request = public_relay_bind_request();
    let encoded = serde_json::to_vec(&request).expect("public relay request JSON");
    let canonical = CanonicalJsonV1::parse(&encoded).and_then(|value| value.to_bytes());
    assert!(
        canonical.is_ok(),
        "public relay request must cross the fixed adapter: {canonical:?}"
    );
}

fn service_request(path: &str, token: &str, body: &impl Serialize) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(body).expect("request fixture"),
        ))
        .expect("request")
}

async fn browser_stream_handshake(
    path: &str,
    origin: &str,
    protocol: Option<&str>,
    extra_header: Option<&str>,
) -> String {
    let app = hosted_gateway_router(browser_stream_config(), Backend::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let address = listener.local_addr().expect("gateway address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let path = path.to_owned();
    let origin = origin.to_owned();
    let protocol = protocol.map(ToOwned::to_owned);
    let extra_header = extra_header.map(ToOwned::to_owned);
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).expect("gateway connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("gateway read timeout");
        let protocol_header = protocol.as_deref()
            .map(|value| format!("Sec-WebSocket-Protocol: {value}\r\n"))
            .unwrap_or_default();
        let extra_header = extra_header.as_deref()
            .map(|value| format!("{value}\r\n"))
            .unwrap_or_default();
        let nonce = ["dGhlIHNhbXBsZSBu", "b25jZQ=="].concat();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: arena.example\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {nonce}\r\n{protocol_header}{extra_header}\r\n"
        );
        stream.write_all(request.as_bytes()).expect("gateway request");
        read_handshake_response(&mut stream)
    })
    .await
    .expect("gateway handshake task");
    server.abort();
    let _ = server.await;
    response
}

fn read_handshake_response(reader: &mut impl std::io::Read) -> String {
    let mut response = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 1024];
    loop {
        if let Some(end) = response.windows(4).position(|value| value == b"\r\n\r\n") {
            // A TCP read can also contain binary WebSocket frames after 101.
            // This helper tests HTTP admission, not the upgraded frame stream.
            if response.starts_with(b"HTTP/1.1 101 ") {
                response.truncate(end + 4);
            }
            return String::from_utf8(response).expect("gateway HTTP response encoding");
        }
        assert!(
            response.len() < 16 * 1024,
            "bounded gateway response headers"
        );
        let remaining = chunk.len().min(16 * 1024 - response.len());
        let count = reader
            .read(&mut chunk[..remaining])
            .expect("gateway response");
        assert!(count > 0, "gateway response ended before headers");
        response.extend_from_slice(&chunk[..count]);
    }
}

#[test]
fn handshake_headers_exclude_coalesced_binary_websocket_frames() {
    let headers = b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\n\r\n";
    let response = [headers.as_slice(), &[0x88, 0x02, 0x03, 0xf0]].concat();
    assert_eq!(
        read_handshake_response(&mut response.as_slice()),
        std::str::from_utf8(headers).expect("fixture headers")
    );
}

#[test]
fn handshake_headers_allow_a_fragmented_terminator() {
    struct Fragments<'a>(&'a [u8]);
    impl std::io::Read for Fragments<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(self.0.len()).min(3);
            output[..count].copy_from_slice(&self.0[..count]);
            self.0 = &self.0[count..];
            Ok(count)
        }
    }
    let headers = b"HTTP/1.1 101 Switching Protocols\r\n\r\n";
    let response = [headers.as_slice(), &[0x88, 0x02, 0x03, 0xf0]].concat();
    assert_eq!(
        read_handshake_response(&mut Fragments(&response)),
        std::str::from_utf8(headers).expect("fixture headers")
    );
}

#[test]
fn handshake_response_keeps_a_coalesced_rejection_body() {
    let response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 6\r\n\r\ndenied";
    assert_eq!(
        read_handshake_response(&mut response.as_slice()),
        std::str::from_utf8(response).expect("fixture response")
    );
}

#[tokio::test]
async fn health_readiness_and_version_are_bounded_and_secret_free() {
    let backend = Backend {
        ready: true,
        ..Backend::default()
    };
    let app = hosted_gateway_router(config(8), backend);
    for path in ["/healthz", "/readyz", "/version"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 200);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let text = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(!text.contains(TOKEN));
        assert!(!text.contains("9310"));
    }
}

#[tokio::test]
async fn generic_worldstream_and_arbitrary_proxy_routes_are_absent() {
    let app = hosted_gateway_router(config(8), Backend::default());
    for path in [
        "/v1/rooms",
        "/v1/operator/packs",
        "/api/v1/room-setup-operations",
        "/api/v1/client-bindings",
        "/proxy/http://127.0.0.1:9310/v1/rooms",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 404, "{path}");
    }
}

#[tokio::test]
async fn browser_upgrades_ignore_fly_transport_metadata_without_trusting_it() {
    let proxy_headers = "X-Forwarded-For: 192.0.2.10, 198.51.100.20\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Port: 443\r\nX-Forwarded-Ssl: on";
    for (path, protocol) in [
        ("/v1/hosted/browser-stream", "worldstream.json.v0.1"),
        (
            "/v1/hosted/public-runs/0123456789abcdef0123456789abcdef/stream",
            "worldstream.public-projection.v1",
        ),
    ] {
        let accepted = browser_stream_handshake(
            path,
            "https://arena.example",
            Some(protocol),
            Some(proxy_headers),
        )
        .await;
        assert!(accepted.starts_with("HTTP/1.1 101 "), "{accepted}");
        let rejected = browser_stream_handshake(
            path,
            "https://other.example",
            Some(protocol),
            Some(proxy_headers),
        )
        .await;
        assert!(rejected.starts_with("HTTP/1.1 403 "), "{rejected}");
    }
}

#[tokio::test]
async fn public_browser_stream_requires_exact_origin_subprotocol_and_credential_free_upgrade() {
    let accepted = browser_stream_handshake(
        "/v1/hosted/browser-stream",
        "https://arena.example",
        Some("worldstream.json.v0.1"),
        None,
    )
    .await;
    assert!(accepted.starts_with("HTTP/1.1 101 "), "{accepted}");
    assert!(
        accepted.contains("\r\nsec-websocket-protocol: worldstream.json.v0.1\r\n"),
        "{accepted}"
    );

    for (path, origin, protocol, extra) in [
        (
            "/v1/hosted/browser-stream?ticket=secret",
            "https://arena.example",
            Some("worldstream.json.v0.1"),
            None,
        ),
        (
            "/v1/hosted/browser-stream",
            "https://other.example",
            Some("worldstream.json.v0.1"),
            None,
        ),
        (
            "/v1/hosted/browser-stream",
            "https://arena.example",
            None,
            None,
        ),
        (
            "/v1/hosted/browser-stream",
            "https://arena.example",
            Some("worldstream.json.v9.9"),
            None,
        ),
        (
            "/v1/hosted/browser-stream",
            "https://arena.example",
            Some("worldstream.json.v0.1"),
            Some("Authorization: Bearer must-not-cross"),
        ),
        (
            "/v1/hosted/browser-stream",
            "https://arena.example",
            Some("worldstream.json.v0.1"),
            Some("Cookie: ticket=must-not-cross"),
        ),
        (
            "/v1/hosted/browser-stream",
            "https://arena.example",
            Some("worldstream.json.v0.1"),
            Some("X-Forwarded-Host: internal.example"),
        ),
    ] {
        let rejected = browser_stream_handshake(path, origin, protocol, extra).await;
        assert!(
            rejected.starts_with("HTTP/1.1 400 ") || rejected.starts_with("HTTP/1.1 403 "),
            "{rejected}"
        );
        assert!(!rejected.contains("must-not-cross"));
        assert!(!rejected.contains("ticket=secret"));
    }
}

#[tokio::test]
async fn anonymous_public_stream_uses_a_separate_read_only_upgrade_contract() {
    let public_id = "0123456789abcdef0123456789abcdef";
    let public_path = format!("/v1/hosted/public-runs/{public_id}/stream");
    let accepted = browser_stream_handshake(
        &public_path,
        "https://arena.example",
        Some("worldstream.public-projection.v1"),
        None,
    )
    .await;
    assert!(accepted.starts_with("HTTP/1.1 101 "), "{accepted}");
    assert!(
        accepted.contains("\r\nsec-websocket-protocol: worldstream.public-projection.v1\r\n"),
        "{accepted}"
    );

    for (origin, protocol, extra) in [
        (
            "https://other.example",
            Some("worldstream.public-projection.v1"),
            None,
        ),
        ("https://arena.example", Some("worldstream.json.v0.1"), None),
        (
            "https://arena.example",
            Some("worldstream.public-projection.v1"),
            Some("Authorization: Bearer must-not-cross"),
        ),
    ] {
        let rejected = browser_stream_handshake(&public_path, origin, protocol, extra).await;
        assert!(
            rejected.starts_with("HTTP/1.1 400 ") || rejected.starts_with("HTTP/1.1 403 "),
            "{rejected}"
        );
        assert!(!rejected.contains("must-not-cross"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::result_large_err, clippy::too_many_lines)]
async fn public_browser_stream_proxies_first_frame_ticket_and_live_frames_only_to_runtime() {
    let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("runtime listener");
    let runtime_address = runtime_listener.local_addr().expect("runtime address");
    let runtime = tokio::spawn(async move {
        let (stream, _) = runtime_listener.accept().await.expect("runtime connection");
        let mut socket = accept_hdr_async(
            stream,
            |request: &WebSocketRequest, mut response: WebSocketResponse| {
                assert_eq!(request.uri().path(), "/v1/hosted/browser-stream");
                assert_eq!(
                    request
                        .headers()
                        .get("origin")
                        .and_then(|value| value.to_str().ok()),
                    Some("https://arena.example")
                );
                assert_eq!(
                    request
                        .headers()
                        .get("sec-websocket-protocol")
                        .and_then(|value| value.to_str().ok()),
                    Some("worldstream.json.v0.1")
                );
                assert!(request.headers().get("authorization").is_none());
                assert!(request.headers().get("cookie").is_none());
                for name in [
                    "x-forwarded-for",
                    "x-forwarded-proto",
                    "x-forwarded-port",
                    "x-forwarded-ssl",
                ] {
                    assert!(request.headers().get(name).is_none());
                }
                response.headers_mut().insert(
                    "sec-websocket-protocol",
                    "worldstream.json.v0.1".parse().expect("protocol header"),
                );
                Ok(response)
            },
        )
        .await
        .expect("runtime handshake");
        let ticket = socket
            .next()
            .await
            .expect("ticket frame")
            .expect("valid ticket frame");
        assert_eq!(
            ticket,
            TungsteniteMessage::Text(format!("wst1:{}", "a".repeat(64)).into())
        );
        socket
            .send(TungsteniteMessage::Text("runtime-frame".into()))
            .await
            .expect("runtime frame");
        let browser_frame = socket
            .next()
            .await
            .expect("browser frame")
            .expect("valid browser frame");
        assert_eq!(
            browser_frame,
            TungsteniteMessage::Text("browser-frame".into())
        );
    });

    let app = hosted_gateway_router(
        browser_stream_config_with_runtime(runtime_address),
        Backend::default(),
    );
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    let gateway = tokio::spawn(async move {
        axum::serve(
            gateway_listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });

    tokio::task::spawn_blocking(move || {
        let stream = TcpStream::connect(gateway_address).expect("browser connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("browser read timeout");
        let mut request = "ws://arena.example/v1/hosted/browser-stream"
            .into_client_request()
            .expect("browser request");
        request
            .headers_mut()
            .insert("origin", "https://arena.example".parse().expect("origin"));
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "worldstream.json.v0.1".parse().expect("protocol"),
        );
        for (name, value) in [
            ("x-forwarded-for", "192.0.2.10"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-port", "443"),
            ("x-forwarded-ssl", "on"),
        ] {
            request
                .headers_mut()
                .insert(name, value.parse().expect("transport metadata"));
        }
        let (mut socket, response) =
            tokio_tungstenite::tungstenite::client(request, stream).expect("gateway handshake");
        assert_eq!(
            response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|value| value.to_str().ok()),
            Some("worldstream.json.v0.1")
        );
        socket
            .send(TungsteniteMessage::Text(
                format!("wst1:{}", "a".repeat(64)).into(),
            ))
            .expect("browser ticket");
        assert_eq!(
            socket.read().expect("runtime frame"),
            TungsteniteMessage::Text("runtime-frame".into())
        );
        socket
            .send(TungsteniteMessage::Text("browser-frame".into()))
            .expect("browser frame");
    })
    .await
    .expect("browser task");
    runtime.await.expect("runtime task");
    gateway.abort();
    let _ = gateway.await;
}

#[allow(clippy::result_large_err, clippy::too_many_lines)]
async fn serve_public_projection_fixture(stream: tokio::net::TcpStream) {
    let mut socket = accept_hdr_async(
        stream,
        |request: &WebSocketRequest, mut response: WebSocketResponse| {
            assert_eq!(request.uri().path(), "/v1/hosted/browser-stream");
            response.headers_mut().insert(
                "sec-websocket-protocol",
                "worldstream.json.v0.1".parse().expect("protocol header"),
            );
            Ok(response)
        },
    )
    .await
    .expect("runtime handshake");
    assert_eq!(
        socket.next().await.expect("ticket").expect("ticket frame"),
        TungsteniteMessage::Text(format!("wst1:{}", "c".repeat(64)).into())
    );
    let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let member_id = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
    let digest = |value: char| format!("blake3:{}", value.to_string().repeat(64));
    let head = json!({
        "room_id": room_id,
        "room_seq": 0,
        "genesis_or_transition_hash": digest('1'),
        "core_schema_version": "worldstream/core-room-state/v1",
        "pack_digest": digest('2'),
        "core_state_hash": digest('3'),
        "activity_state_hash": digest('4'),
        "authoritative_state_hash": digest('5')
    });
    let envelope = |message_type: &str, body: Value| {
        TungsteniteMessage::Text(
            json!({
                "protocol": "0.1",
                "type": message_type,
                "message_id": "01ARZ3NDEKTSV4RRFFQ69G5FAY",
                "body": body
            })
            .to_string()
            .into(),
        )
    };
    socket
        .send(envelope(
            "server.welcome",
            json!({
                "session_id": "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
                "selected_protocol": "0.1",
                "server_version": "fixture",
                "heartbeat_interval_ms": 30000,
                "maximum_message_bytes": 524_288,
                "authenticated_principal": {
                    "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAW",
                    "kind": "agent"
                }
            }),
        ))
        .await
        .expect("welcome");
    socket
        .send(envelope(
            "room.attached",
            json!({
                "room_id": room_id,
                "member_id": member_id,
                "principal_kind": "agent",
                "access_mode": "spectator",
                "role": null,
                "membership_status": "enabled",
                "room_status": "active",
                "room_health": "healthy",
                "integrity_generation": 0,
                "room_head": head.clone(),
                "cursor": null,
                "frame_head": 0,
                "retained_floor": 0,
                "sync_token": "sync-public-1",
                "sync": {
                    "kind": "projection_reset",
                    "baseline_frame_head": 0,
                    "reason": "first_attach"
                },
                "pack": {
                    "id": "worldstream.agent-heist",
                    "version": "0.2.0",
                    "digest": digest('2')
                }
            }),
        ))
        .await
        .expect("attached");
    socket
        .send(envelope(
            "projection.reset",
            json!({
                "room_id": room_id,
                "member_id": member_id,
                "room_head": head,
                "room_health": "healthy",
                "integrity_generation": 0,
                "baseline_frame_head": 0,
                "reset_reason": "first_attach",
                "projection_schema": "agent-heist/projection/v1",
                "projection": {
                    "core": {
                        "access_mode": "spectator",
                        "standing": "enabled",
                        "role": null,
                        "room_status": "active",
                        "viewer_class": "public"
                    },
                    "activity": {"phase": "lobby", "principal_id": "must-strip"},
                    "action_offers": []
                },
                "projection_hash": digest('6')
            }),
        ))
        .await
        .expect("reset");
    let sync_ack: Value = serde_json::from_str(
        socket
            .next()
            .await
            .expect("sync ack")
            .expect("valid sync ack")
            .into_text()
            .expect("text ack")
            .as_str(),
    )
    .expect("sync ack json");
    assert_eq!(sync_ack["type"], "room.sync_ack");
    assert!(sync_ack["body"].get("room_id").is_none());
    socket
        .send(envelope(
            "room.sync_acked",
            json!({"through_frame_head": 0}),
        ))
        .await
        .expect("sync receipt");
    socket
        .send(envelope(
            "observation.deliver",
            json!({
                "room_id": room_id,
                "member_id": member_id,
                "frame_seq": 1,
                "cause_room_seq": 1,
                "frame_kind": "activity",
                "observation_schema": "agent-heist/observation/v1",
                "observation": {"phase": "planning", "action_offers": []},
                "frame_payload_hash": digest('7')
            }),
        ))
        .await
        .expect("observation");
    // Anonymous browsers share one relay Membership, not a consumption cursor.
    // A per-browser acknowledgement would make a later viewer's reset fail.
    if let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) =
        tokio::time::timeout(Duration::from_secs(2), socket.next()).await
    {
        let request: Value = serde_json::from_str(text.as_str()).expect("runtime request");
        assert_ne!(
            request["type"], "observation.ack",
            "public viewers must not consume the shared relay Cursor"
        );
    }
}

fn read_public_projection_fixture(gateway_address: SocketAddr) {
    let stream = TcpStream::connect(gateway_address).expect("browser connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("browser read timeout");
    let mut request =
        "ws://arena.example/v1/hosted/public-runs/0123456789abcdef0123456789abcdef/stream"
            .into_client_request()
            .expect("browser request");
    request
        .headers_mut()
        .insert("origin", "https://arena.example".parse().expect("origin"));
    request.headers_mut().insert(
        "sec-websocket-protocol",
        "worldstream.public-projection.v1"
            .parse()
            .expect("protocol"),
    );
    let (mut socket, response) =
        tokio_tungstenite::tungstenite::client(request, stream).expect("gateway handshake");
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("worldstream.public-projection.v1")
    );
    for expected_kind in ["projection_reset", "observation"] {
        let frame = socket.read().expect("public frame");
        let text = frame.into_text().expect("public text frame");
        let value: Value = serde_json::from_str(&text).expect("public frame json");
        assert_eq!(value["version"], "worldstream/public-projection-stream/v1");
        assert_eq!(value["batch"]["delivery"][0]["kind"], expected_kind);
        for forbidden in [
            "room_id",
            "member_id",
            "membership_id",
            "principal_id",
            "ticket",
            "replay",
            "final_reveal",
        ] {
            assert!(!text.contains(forbidden), "leaked {forbidden}: {text}");
        }
    }
    socket.close(None).expect("close public stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::result_large_err)]
async fn two_anonymous_browsers_receive_only_pushed_authorized_projection_frames() {
    let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("runtime listener");
    let runtime_address = runtime_listener.local_addr().expect("runtime address");
    let runtime = tokio::spawn(async move {
        let (first, _) = runtime_listener
            .accept()
            .await
            .expect("first runtime connection");
        let (second, _) = runtime_listener
            .accept()
            .await
            .expect("second runtime connection");
        tokio::join!(
            serve_public_projection_fixture(first),
            serve_public_projection_fixture(second)
        );
    });

    let backend = Backend::default();
    let app = hosted_gateway_router(
        browser_stream_config_with_runtime(runtime_address),
        backend.clone(),
    );
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    let gateway = tokio::spawn(async move {
        axum::serve(
            gateway_listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });

    let first =
        tokio::task::spawn_blocking(move || read_public_projection_fixture(gateway_address));
    let second =
        tokio::task::spawn_blocking(move || read_public_projection_fixture(gateway_address));
    let (first_result, second_result) = tokio::join!(first, second);
    first_result.expect("first browser task");
    second_result.expect("second browser task");
    runtime.await.expect("runtime task");
    assert_eq!(
        backend
            .public_stream_lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        2
    );
    gateway.abort();
    let _ = gateway.await;
}

#[tokio::test]
async fn service_authority_and_listing_allowlist_fail_before_backend_mutation() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    for request in [
        service_request(
            "/v1/hosted/launch",
            "wrong-authority-value-long-enough",
            &launch_request(LISTING),
        ),
        service_request(
            "/v1/hosted/launch",
            TOKEN,
            &launch_request(
                "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            ),
        ),
        Request::builder()
            .method("POST")
            .uri("/v1/hosted/evidence")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&evidence_request(LISTING)).expect("request fixture"),
            ))
            .expect("request"),
    ] {
        let response = app.clone().oneshot(request).await.expect("response");
        assert!(matches!(response.status().as_u16(), 401 | 403));
    }
    assert!(
        backend
            .launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}

#[tokio::test]
async fn accepted_service_calls_are_typed_and_globally_rate_bounded() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(1), backend.clone());
    let accepted = app
        .clone()
        .oneshot(service_request(
            "/v1/hosted/launch",
            TOKEN,
            &launch_request(LISTING),
        ))
        .await
        .expect("response");
    assert_eq!(accepted.status(), 202);
    let limited = app
        .oneshot(service_request(
            "/v1/hosted/evidence",
            TOKEN,
            &evidence_request(LISTING),
        ))
        .await
        .expect("response");
    assert_eq!(limited.status(), 429);
    let requests = backend
        .launches
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].listing_revision_digest, LISTING);
}

#[tokio::test]
async fn genesis_correspondence_is_service_only_and_never_uses_a_browser_route() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    let response = app
        .oneshot(service_request(
            "/v1/hosted/genesis-evidence",
            TOKEN,
            &evidence_request(LISTING),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store, max-age=0")
    );
    let evidence: HostedGenesisEvidenceV1 = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("typed Genesis evidence");
    assert_eq!(evidence.genesis_head.room_seq, 0);
    assert_eq!(evidence.memberships.len(), 2);
    assert_eq!(
        backend
            .genesis_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
}

#[tokio::test]
async fn result_source_evidence_is_service_only_typed_and_secret_free() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    let response = app
        .clone()
        .oneshot(service_request(
            "/v1/hosted/result-source-evidence",
            TOKEN,
            &result_source_request(LISTING),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store, max-age=0")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let evidence: HostedResultSourceEvidenceV1 =
        serde_json::from_slice(&body).expect("typed result-source evidence");
    assert_eq!(evidence.run_id, "30000000-0000-4000-8000-000000000001");
    assert!(evidence.replay.is_some());
    let text = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(!text.contains("principal_id"));
    assert!(!text.contains("secret_reference"));
    assert!(!text.contains(TOKEN));

    let unauthorized = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hosted/result-source-evidence")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&result_source_request(LISTING)).expect("request fixture"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        backend
            .result_source_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
}

#[tokio::test]
async fn house_reservation_routes_are_service_only_typed_and_allowlisted() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    for path in [
        "/v1/hosted/house-runners/reserve",
        "/v1/hosted/house-runners/read",
    ] {
        let response = app
            .clone()
            .oneshot(service_request(path, TOKEN, &house_request(LISTING)))
            .await
            .expect("response");
        assert_eq!(response.status(), 200, "{path}");
        let receipt: HostedHouseRunnerReservationReceiptV1 = serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("typed receipt");
        assert_eq!(receipt.seat_id, "navigator");
    }
    assert_eq!(
        backend
            .house_reservations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        backend
            .house_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );

    let unauthorized = app
        .oneshot(service_request(
            "/v1/hosted/house-runners/reserve",
            "wrong-authority-value-long-enough",
            &house_request(LISTING),
        ))
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), 401);
}

#[tokio::test]
async fn browser_session_and_public_binding_routes_are_service_only() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    for path in [
        "/v1/hosted/browser-handoffs/issue",
        "/v1/hosted/browser-sessions/admit",
        "/v1/hosted/browser-sessions/status",
        "/v1/hosted/browser-sessions/logout",
        "/v1/hosted/browser-sessions/stream-ticket",
        "/v1/hosted/public-relays/bind",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(
                        "authorization",
                        "Bearer browser-token-that-is-not-the-service",
                    )
                    .header("content-type", "application/json")
                    .header("cookie", "browser=value")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 401, "{path}");
    }
    let public = app
        .oneshot(
            Request::builder()
                .uri("/v1/hosted/public-runs/not-an-opaque-id/stream")
                .header("host", "arena.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(public.status(), 400);
    assert!(
        backend
            .launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
    assert!(
        backend
            .browser_handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
    assert!(
        backend
            .browser_redemptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}

#[tokio::test]
async fn hosted_browser_session_service_calls_are_typed_no_store_and_rate_bounded() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    let calls = [
        (
            service_request(
                "/v1/hosted/browser-handoffs/issue",
                TOKEN,
                &browser_handoff_request(LISTING),
            ),
            201,
        ),
        (
            service_request(
                "/v1/hosted/browser-sessions/admit",
                TOKEN,
                &browser_redeem_request(),
            ),
            201,
        ),
        (
            service_request(
                "/v1/hosted/browser-sessions/status",
                TOKEN,
                &browser_session_request(),
            ),
            200,
        ),
        (
            service_request(
                "/v1/hosted/browser-sessions/logout",
                TOKEN,
                &browser_session_request(),
            ),
            200,
        ),
        (
            service_request(
                "/v1/hosted/browser-sessions/stream-ticket",
                TOKEN,
                &browser_stream_ticket_request(),
            ),
            201,
        ),
    ];
    for (index, (request, expected_status)) in calls.into_iter().enumerate() {
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), expected_status, "call {index}");
        assert_eq!(
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("private, no-store, max-age=0")
        );
    }
    assert_eq!(
        backend
            .browser_handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        backend
            .browser_redemptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        backend
            .browser_status_reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        backend
            .browser_logouts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        backend
            .browser_stream_tickets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[browser_stream_ticket_request()]
    );
}

#[tokio::test]
async fn public_relay_binding_is_typed_allowlisted_and_service_only() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    let request = public_relay_bind_request();
    let response = app
        .clone()
        .oneshot(service_request(
            "/v1/hosted/public-relays/bind",
            TOKEN,
            &request,
        ))
        .await
        .expect("binding response");
    assert_eq!(response.status(), 201);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("binding body")
        .to_bytes();
    let receipt: HostedPublicRelayBindReceiptV1 =
        serde_json::from_slice(&bytes).expect("typed receipt");
    assert_eq!(receipt.public_run_id, request.public_run_id);
    assert!(!String::from_utf8_lossy(&bytes).contains("membership_id"));
    assert_eq!(
        backend
            .public_relay_binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[request]
    );

    let unauthorized = app
        .oneshot(service_request(
            "/v1/hosted/public-relays/bind",
            "wrong-authority-value-long-enough",
            &public_relay_bind_request(),
        ))
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), 401);
}

#[tokio::test]
async fn malformed_and_oversized_service_payloads_are_safe() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    let oversized = Request::builder()
        .method("POST")
        .uri("/v1/hosted/launch")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(vec![b'x'; 257 * 1024]))
        .expect("request");
    let response = app.oneshot(oversized).await.expect("response");
    assert_eq!(response.status(), 413);
    let value: Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("structured safe error");
    assert_eq!(value["error"]["code"], "payload_too_large");
    assert!(value.get("authority").is_none());
    assert!(
        backend
            .launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn fixed_adapter_uses_only_reviewed_routes_and_its_separate_authority() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        for index in 0..14 {
            let (stream, _) = listener.accept().expect("fixture connection");
            let (request_line, authorization, body, mut stream) = read_request(stream);
            sender
                .send((request_line, authorization, body.clone()))
                .expect("fixture observation");
            let response_body = match index {
                0 => serde_json::to_vec(&json!({
                    "schema": "worldstream/hosted-launch-readiness/v1",
                    "ready": true
                }))
                .expect("readiness response"),
                3 => serde_json::to_vec(&genesis(&evidence_request(LISTING)))
                    .expect("Genesis evidence response"),
                4 => serde_json::to_vec(&result_source_evidence(&result_source_request(LISTING)))
                    .expect("result-source evidence response"),
                5 | 6 => serde_json::to_vec(&house_receipt(&house_request(LISTING)))
                    .expect("House receipt response"),
                7 => serde_json::to_vec(&HostedBrowserHandoffResponseV1 {
                    schema: "worldstream/hosted-browser-handoff-response/v1".to_owned(),
                    client_url: format!(
                        "https://arena.example/clients/heist/#handoff=wsh1:{}",
                        "a".repeat(64)
                    ),
                })
                .expect("browser handoff response"),
                8 => serde_json::to_vec(&HostedBrowserHandoffRedeemResponseV1 {
                    schema: "worldstream/hosted-browser-handoff-redeem-response/v1".to_owned(),
                    session: format!("wss1:{}", "b".repeat(64)),
                })
                .expect("browser redemption response"),
                9 => serde_json::to_vec(&HostedBrowserSessionStatusV1 {
                    schema: "worldstream/hosted-browser-session-status/v1".to_owned(),
                    state: HostedBrowserSessionStateV1::Usable,
                })
                .expect("browser status response"),
                10 => serde_json::to_vec(&HostedBrowserSessionLogoutV1 {
                    schema: "worldstream/hosted-browser-session-logout/v1".to_owned(),
                    logged_out: true,
                })
                .expect("browser logout response"),
                11 => serde_json::to_vec(&HostedBrowserStreamTicketResponseV1 {
                    schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
                    ticket: format!("wst1:{}", "c".repeat(64)),
                    expires_in_ms: 15_000,
                })
                .expect("browser stream ticket response"),
                12 => {
                    let request = public_relay_bind_request();
                    serde_json::to_vec(&HostedPublicRelayBindReceiptV1 {
                        schema: "worldstream/hosted-public-relay-bind-receipt/v1".to_owned(),
                        public_run_id: request.public_run_id,
                        activity_run_id: request.activity_run_id,
                        binding_request_digest: format!("sha256:{}", "d".repeat(64)),
                        bound: true,
                    })
                    .expect("public relay receipt")
                }
                13 => serde_json::to_vec(&HostedBrowserStreamTicketResponseV1 {
                    schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
                    ticket: format!("wst1:{}", "c".repeat(64)),
                    expires_in_ms: 15_000,
                })
                .expect("public stream ticket response"),
                _ => serde_json::to_vec(&status(
                    LISTING,
                    "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "hosted-launch-01",
                ))
                .expect("status response"),
            };
            let status_code = if index == 1 {
                202
            } else if matches!(index, 7 | 8 | 11 | 12 | 13) {
                201
            } else {
                200
            };
            write!(
                stream,
                "HTTP/1.1 {status_code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .expect("fixture response headers");
            stream
                .write_all(&response_body)
                .expect("fixture response body");
        }
    });

    let backend = FixedHostAdapterBackend::new(address, TOKEN.to_owned(), Duration::from_secs(2))
        .expect("fixed adapter");
    assert!(backend.ready());
    assert!(backend.launch(&launch_request(LISTING)).is_ok());
    assert!(backend.evidence(&evidence_request(LISTING)).is_ok());
    assert!(backend.genesis_evidence(&evidence_request(LISTING)).is_ok());
    assert!(
        backend
            .result_source_evidence(&result_source_request(LISTING))
            .is_ok()
    );
    assert!(
        backend
            .reserve_house_runner(&house_request(LISTING))
            .is_ok()
    );
    assert!(backend.read_house_runner(&house_request(LISTING)).is_ok());
    assert!(
        backend
            .issue_browser_handoff(&browser_handoff_request(LISTING))
            .is_ok()
    );
    assert!(
        backend
            .redeem_browser_handoff(&browser_redeem_request())
            .is_ok()
    );
    assert!(
        backend
            .browser_session_status(&browser_session_request())
            .is_ok()
    );
    assert!(
        backend
            .logout_browser_session(&browser_session_request())
            .is_ok()
    );
    let stream_ticket_request = browser_stream_ticket_request();
    let stream_ticket_json = serde_json::to_vec(&stream_ticket_request).expect("ticket JSON");
    assert!(
        CanonicalJsonV1::parse(&stream_ticket_json)
            .and_then(|value| value.to_bytes())
            .is_ok(),
        "{}",
        String::from_utf8_lossy(&stream_ticket_json)
    );
    let stream_ticket = backend.issue_browser_stream_ticket(&stream_ticket_request);
    assert!(stream_ticket.is_ok(), "{stream_ticket:?}");
    let public_relay = backend.bind_public_relay(&public_relay_bind_request());
    assert!(public_relay.is_ok(), "{public_relay:?}");
    let public_stream = backend.issue_public_stream_ticket(&HostedPublicStreamTicketRequestV1 {
        schema: "worldstream/hosted-public-stream-ticket-request/v1".to_owned(),
        public_run_id: "0123456789abcdef0123456789abcdef".to_owned(),
    });
    assert!(public_stream.is_ok(), "{public_stream:?}");
    server.join().expect("fixture server");

    let observations = receiver.try_iter().collect::<Vec<_>>();
    assert_fixed_adapter_observations(&observations);
}

fn assert_fixed_adapter_observations(observations: &[(String, String, Vec<u8>)]) {
    assert_eq!(observations.len(), 14);
    assert_eq!(
        observations[0].0,
        "GET /api/v1/hosted-launches/ready HTTP/1.1"
    );
    assert_eq!(
        observations[1].0,
        "POST /api/v1/hosted-launches:submit HTTP/1.1"
    );
    assert_eq!(
        observations[2].0,
        "POST /api/v1/hosted-launches:read HTTP/1.1"
    );
    assert_eq!(
        observations[3].0,
        "POST /api/v1/hosted-launches:read-genesis HTTP/1.1"
    );
    assert_eq!(
        observations[4].0,
        "POST /api/v1/hosted-launches:read-result-source HTTP/1.1"
    );
    assert_eq!(
        observations[5].0,
        "POST /api/v1/hosted-house-runners:reserve HTTP/1.1"
    );
    assert_eq!(
        observations[6].0,
        "POST /api/v1/hosted-house-runners:read HTTP/1.1"
    );
    assert_eq!(
        observations[7].0,
        "POST /api/v1/hosted-browser-handoffs:issue HTTP/1.1"
    );
    assert_eq!(
        observations[8].0,
        "POST /api/v1/hosted-browser-handoffs:redeem HTTP/1.1"
    );
    assert_eq!(
        observations[9].0,
        "POST /api/v1/hosted-browser-sessions:status HTTP/1.1"
    );
    assert_eq!(
        observations[10].0,
        "POST /api/v1/hosted-browser-sessions:logout HTTP/1.1"
    );
    assert_eq!(
        observations[11].0,
        "POST /api/v1/hosted-browser-sessions:stream-ticket HTTP/1.1"
    );
    assert_eq!(
        observations[12].0,
        "POST /api/v1/hosted-public-relays:bind HTTP/1.1"
    );
    assert_eq!(
        observations[13].0,
        "POST /api/v1/hosted-public-streams:ticket HTTP/1.1"
    );
    assert!(
        observations
            .iter()
            .all(|(_, authority, _)| authority == &format!("Bearer {TOKEN}"))
    );
    assert!(observations[0].2.is_empty());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[1].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[2].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[3].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[4].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[5].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[6].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[7].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[8].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[9].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[10].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[11].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[12].2).is_ok());
    assert!(CanonicalJsonV1::from_canonical_bytes(&observations[13].2).is_ok());
}

fn read_request(stream: TcpStream) -> (String, String, Vec<u8>, TcpStream) {
    let writer = stream.try_clone().expect("fixture writer");
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("fixture request line");
    let mut authorization = None;
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("fixture header");
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Authorization: ") {
            authorization = Some(value.trim().to_owned());
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            content_length = Some(value.trim().parse::<usize>().expect("fixture length"));
        }
    }
    let mut body = vec![0_u8; content_length.expect("content length")];
    reader.read_exact(&mut body).expect("fixture body");
    (
        request_line.trim().to_owned(),
        authorization.expect("controller authority"),
        body,
        writer,
    )
}
