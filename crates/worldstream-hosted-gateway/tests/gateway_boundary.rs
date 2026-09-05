#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use worldstream_hosted_gateway::{
    HostedGatewayBackend, HostedGatewayConfig, HostedGatewayError, HostedServiceRequestV1,
    hosted_gateway_router,
};

const LISTING: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "service-authority-that-never-leaves-fly";

#[derive(Clone, Default)]
struct Backend {
    requests: Arc<Mutex<Vec<HostedServiceRequestV1>>>,
    ready: bool,
}

impl HostedGatewayBackend for Backend {
    fn ready(&self) -> bool {
        self.ready
    }

    fn launch(&self, request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(())
    }

    fn evidence(&self, request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        self.launch(request)
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

fn service_request(path: &str, token: &str, listing: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "listing_revision_digest": listing,
                "operation_reference": "launch-01"
            }))
            .expect("request fixture"),
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
            LISTING,
        ),
        service_request(
            "/v1/hosted/launch",
            TOKEN,
            "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ),
        Request::builder()
            .method("POST")
            .uri("/v1/hosted/evidence")
            .header("content-type", "application/json")
            .body(Body::from(format!(
                "{{\"listing_revision_digest\":\"{LISTING}\",\"operation_reference\":\"run-01\"}}"
            )))
            .expect("request"),
    ] {
        let response = app.clone().oneshot(request).await.expect("response");
        assert!(matches!(response.status().as_u16(), 401 | 403));
    }
    assert!(
        backend
            .requests
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
        .oneshot(service_request("/v1/hosted/launch", TOKEN, LISTING))
        .await
        .expect("response");
    assert_eq!(accepted.status(), 202);
    let limited = app
        .oneshot(service_request("/v1/hosted/evidence", TOKEN, LISTING))
        .await
        .expect("response");
    assert_eq!(limited.status(), 429);
    let requests = backend
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].listing_revision_digest, LISTING);
}

#[tokio::test]
async fn browser_and_public_seams_reject_header_smuggling_without_internal_calls() {
    let backend = Backend::default();
    let app = hosted_gateway_router(config(8), backend.clone());
    for (name, value) in [
        ("authorization", "Bearer browser-token"),
        ("cookie", "secret=value"),
        ("forwarded", "host=internal"),
        ("x-forwarded-host", "127.0.0.1:9310"),
        ("upgrade", "websocket"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/hosted/browser-sessions/admit")
                    .header("host", "arena.example")
                    .header(name, value)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), 400, "{name}");
    }
    let hostile_host = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hosted/browser-sessions/admit")
                .header("host", "attacker.internal")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(hostile_host.status(), 400);
    let admitted_seam = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hosted/browser-sessions/admit")
                .header("host", "arena.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(admitted_seam.status(), 501);
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
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
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
        .body(Body::from(vec![b'x'; 65 * 1024]))
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
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}
