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
use http_body_util::BodyExt as _;
use serde::Serialize;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use worldstream_core::{CanonicalJsonV1, projection_hash_for_canonical_bytes};
use worldstream_hosted_contract::{
    HostedAuthorizedPublicProjectionV1, HostedBrowserHandoffRedeemRequestV1,
    HostedBrowserHandoffRedeemResponseV1, HostedBrowserHandoffRequestV1,
    HostedBrowserHandoffResponseV1, HostedBrowserSessionLogoutV1, HostedBrowserSessionRequestV1,
    HostedBrowserSessionStateV1, HostedBrowserSessionStatusV1, HostedCapacityAuthorizationV1,
    HostedGenesisAccessModeV1, HostedGenesisEvidenceV1, HostedGenesisHeadV1,
    HostedGenesisMembershipPurposeV1, HostedGenesisMembershipV1, HostedGenesisPrincipalKindV1,
    HostedHouseRunnerReservationOutcomeV1, HostedHouseRunnerReservationReceiptV1,
    HostedHouseRunnerReservationRequestV1, HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1,
    HostedLaunchStageV1, HostedLaunchStatusV1, HostedResultIntegrityStatusV1,
    HostedResultReplayEvidenceV1, HostedResultSourceEvidenceV1, HostedResultSourceHeadV1,
    HostedResultSourceRequestV1, PackReference,
};
use worldstream_hosted_gateway::{
    FixedHostAdapterBackend, HostedGatewayBackend, HostedGatewayConfig, HostedGatewayError,
    hosted_gateway_router,
};

const LISTING: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "service-authority-that-never-leaves-fly";

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
        client_release_digest: format!("blake3:{}", "e".repeat(64)),
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
async fn browser_session_routes_are_service_only_and_public_stream_stays_browser_safe() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    for path in [
        "/v1/hosted/browser-handoffs/issue",
        "/v1/hosted/browser-sessions/admit",
        "/v1/hosted/browser-sessions/status",
        "/v1/hosted/browser-sessions/logout",
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
                .uri("/v1/hosted/public-runs/public-01/stream")
                .header("host", "arena.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(public.status(), 501);
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
        service_request(
            "/v1/hosted/browser-handoffs/issue",
            TOKEN,
            &browser_handoff_request(LISTING),
        ),
        service_request(
            "/v1/hosted/browser-sessions/admit",
            TOKEN,
            &browser_redeem_request(),
        ),
        service_request(
            "/v1/hosted/browser-sessions/status",
            TOKEN,
            &browser_session_request(),
        ),
        service_request(
            "/v1/hosted/browser-sessions/logout",
            TOKEN,
            &browser_session_request(),
        ),
    ];
    for (index, request) in calls.into_iter().enumerate() {
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(
            response.status(),
            if index < 2 { 201 } else { 200 },
            "call {index}"
        );
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
        for index in 0..11 {
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
                _ => serde_json::to_vec(&status(
                    LISTING,
                    "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "hosted-launch-01",
                ))
                .expect("status response"),
            };
            let status_code = if index == 1 {
                202
            } else if matches!(index, 7 | 8) {
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
    server.join().expect("fixture server");

    let observations = receiver.try_iter().collect::<Vec<_>>();
    assert_fixed_adapter_observations(&observations);
}

fn assert_fixed_adapter_observations(observations: &[(String, String, Vec<u8>)]) {
    assert_eq!(observations.len(), 11);
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
