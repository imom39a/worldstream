//! Public Hosted Gateway boundary. This crate intentionally does not depend on
//! `worldstream-server` or the Studio Supervisor, so their generic route graphs
//! cannot become reachable through this listener.

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

const MAX_SERVICE_BODY_BYTES: usize = 64 * 1024;
const MAX_LISTINGS: usize = 64;
const SERVICE_AUTHORITY_TAG_KEY: &[u8] = b"worldstream/hosted-service-authority/v1";

/// Closed gateway failure classes. No variant carries credentials, private
/// Projection bytes, upstream responses, or internal addresses.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HostedGatewayError {
    #[error("hosted gateway configuration is invalid")]
    InvalidConfiguration,
    #[error("hosted operation was rejected")]
    Rejected,
    #[error("hosted operation is unavailable")]
    Unavailable,
}

/// Validated immutable process configuration. The service authority is reduced
/// to a digest at construction and the fixed upstream must be loopback-only.
#[derive(Clone)]
pub struct HostedGatewayConfig {
    deployment_version: Arc<str>,
    service_authority_tag: [u8; 32],
    listing_allowlist: Arc<BTreeSet<String>>,
    public_authority: Arc<str>,
    _fixed_upstream: SocketAddr,
    max_requests_per_window: u32,
    rate_window: Duration,
}

impl HostedGatewayConfig {
    /// Builds the only public-listener configuration accepted by the gateway.
    ///
    /// # Errors
    /// Rejects weak service authority, unbounded or malformed allowlists,
    /// unsafe versions, non-loopback upstreams, and invalid rate bounds.
    pub fn new(
        deployment_version: impl Into<String>,
        service_authority: &str,
        listing_allowlist: BTreeSet<String>,
        public_authority: impl Into<String>,
        fixed_upstream: SocketAddr,
        max_requests_per_window: u32,
        rate_window: Duration,
    ) -> Result<Self, HostedGatewayError> {
        let deployment_version = deployment_version.into();
        let public_authority = public_authority.into();
        if deployment_version.is_empty()
            || deployment_version.len() > 128
            || !deployment_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || service_authority.len() < 32
            || service_authority.len() > 512
            || !service_authority
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || listing_allowlist.is_empty()
            || listing_allowlist.len() > MAX_LISTINGS
            || !listing_allowlist.iter().all(|value| is_digest(value))
            || !valid_public_authority(&public_authority)
            || !fixed_upstream.ip().is_loopback()
            || max_requests_per_window == 0
            || max_requests_per_window > 10_000
            || rate_window < Duration::from_secs(1)
            || rate_window > Duration::from_hours(1)
        {
            return Err(HostedGatewayError::InvalidConfiguration);
        }
        let authority_key = hmac::Key::new(hmac::HMAC_SHA256, SERVICE_AUTHORITY_TAG_KEY);
        let service_authority_tag = hmac::sign(&authority_key, service_authority.as_bytes());
        Ok(Self {
            deployment_version: Arc::from(deployment_version),
            service_authority_tag: service_authority_tag
                .as_ref()
                .try_into()
                .map_err(|_| HostedGatewayError::InvalidConfiguration)?,
            listing_allowlist: Arc::new(listing_allowlist),
            public_authority: Arc::from(public_authority),
            _fixed_upstream: fixed_upstream,
            max_requests_per_window,
            rate_window,
        })
    }
}

/// Typed Vercel-to-Fly stub request. HTTP headers and arbitrary upstreams are
/// deliberately absent, preventing browser forwarding data from crossing the
/// internal call seam.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedServiceRequestV1 {
    pub listing_revision_digest: String,
    pub operation_reference: String,
}

/// Narrow backend seam used by later retained launch and evidence tickets.
pub trait HostedGatewayBackend: Send + Sync + 'static {
    fn ready(&self) -> bool;

    /// Begins or resumes one typed, allowlisted launch operation.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn launch(&self, request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError>;

    /// Reads one typed, allowlisted evidence operation.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn evidence(&self, request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError>;
}

/// Production skeleton backend. It proves route isolation while all stateful
/// hosted operations remain unavailable until their dedicated tickets land.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableHostedGatewayBackend;

impl HostedGatewayBackend for UnavailableHostedGatewayBackend {
    fn ready(&self) -> bool {
        false
    }

    fn launch(&self, _request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        Err(HostedGatewayError::Unavailable)
    }

    fn evidence(&self, _request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        Err(HostedGatewayError::Unavailable)
    }
}

struct RateWindow {
    started: Instant,
    accepted: u32,
}

#[derive(Clone)]
struct GatewayState {
    config: HostedGatewayConfig,
    backend: Arc<dyn HostedGatewayBackend>,
    rate: Arc<Mutex<RateWindow>>,
}

/// Builds the complete public Fly surface. No generic `WorldStream` router is
/// merged into this value.
pub fn hosted_gateway_router(
    config: HostedGatewayConfig,
    backend: impl HostedGatewayBackend,
) -> Router {
    let state = GatewayState {
        config,
        backend: Arc::new(backend),
        rate: Arc::new(Mutex::new(RateWindow {
            started: Instant::now(),
            accepted: 0,
        })),
    };
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route("/version", get(version))
        .route("/v1/hosted/launch", post(launch))
        .route("/v1/hosted/evidence", post(evidence))
        .route(
            "/v1/hosted/browser-sessions/admit",
            post(browser_admission_seam),
        )
        .route(
            "/v1/hosted/public-runs/{public_run_id}/stream",
            get(public_stream_seam),
        )
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_SERVICE_BODY_BYTES))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","version":"hosted_gateway_health.v1"}))
}

async fn readiness(State(state): State<GatewayState>) -> Response {
    let (status, value) = if state.backend.ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };
    (
        status,
        Json(json!({"status":value,"version":"hosted_gateway_readiness.v1"})),
    )
        .into_response()
}

async fn version(State(state): State<GatewayState>) -> Json<serde_json::Value> {
    Json(json!({
        "version":"hosted_gateway_deployment.v1",
        "deployment":state.config.deployment_version.as_ref()
    }))
}

async fn launch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    service_operation(&state, &headers, &body, ServiceOperation::Launch)
}

async fn evidence(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    service_operation(&state, &headers, &body, ServiceOperation::Evidence)
}

#[derive(Clone, Copy)]
enum ServiceOperation {
    Launch,
    Evidence,
}

impl ServiceOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Evidence => "evidence",
        }
    }
}

fn service_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
    operation: ServiceOperation,
) -> Response {
    if !is_json(headers) {
        return safe_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
    }
    if !service_authorized(headers, &state.config.service_authority_tag) {
        return safe_error(StatusCode::UNAUTHORIZED, "service_authority_required");
    }
    let Ok(request) = serde_json::from_slice::<HostedServiceRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if !is_digest(&request.listing_revision_digest) || !safe_reference(&request.operation_reference)
    {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !state
        .config
        .listing_allowlist
        .contains(&request.listing_revision_digest)
    {
        return safe_error(StatusCode::FORBIDDEN, "listing_not_allowed");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let result = match operation {
        ServiceOperation::Launch => state.backend.launch(&request),
        ServiceOperation::Evidence => state.backend.evidence(&request),
    };
    match result {
        Ok(()) => {
            tracing::info!(
                target: "worldstream.hosted_gateway",
                operation = operation.name(),
                listing_revision_digest = request.listing_revision_digest,
                outcome = "accepted",
                "hosted gateway operation"
            );
            (
                StatusCode::ACCEPTED,
                Json(json!({"version":"hosted_gateway_stub.v1","accepted":true})),
            )
                .into_response()
        }
        Err(HostedGatewayError::Rejected | HostedGatewayError::InvalidConfiguration) => {
            tracing::warn!(
                target: "worldstream.hosted_gateway",
                operation = operation.name(),
                listing_revision_digest = request.listing_revision_digest,
                outcome = "rejected",
                "hosted gateway operation"
            );
            safe_error(StatusCode::CONFLICT, "operation_rejected")
        }
        Err(HostedGatewayError::Unavailable) => {
            tracing::warn!(
                target: "worldstream.hosted_gateway",
                operation = operation.name(),
                listing_revision_digest = request.listing_revision_digest,
                outcome = "unavailable",
                "hosted gateway operation"
            );
            safe_error(StatusCode::SERVICE_UNAVAILABLE, "operation_unavailable")
        }
    }
}

async fn browser_admission_seam(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if !browser_headers_are_safe(&headers, &state.config.public_authority) {
        return safe_error(StatusCode::BAD_REQUEST, "unsafe_browser_headers");
    }
    safe_error(
        StatusCode::NOT_IMPLEMENTED,
        "browser_admission_not_implemented",
    )
}

async fn public_stream_seam(
    State(state): State<GatewayState>,
    Path(public_run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !safe_reference(&public_run_id)
        || !browser_headers_are_safe(&headers, &state.config.public_authority)
    {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_public_stream_request");
    }
    safe_error(StatusCode::NOT_IMPLEMENTED, "public_stream_not_implemented")
}

async fn not_found() -> Response {
    safe_error(StatusCode::NOT_FOUND, "route_not_found")
}

fn admit_rate(state: &GatewayState) -> bool {
    let mut rate = state.rate.lock().unwrap_or_else(PoisonError::into_inner);
    if rate.started.elapsed() >= state.config.rate_window {
        rate.started = Instant::now();
        rate.accepted = 0;
    }
    if rate.accepted >= state.config.max_requests_per_window {
        return false;
    }
    rate.accepted = rate.accepted.saturating_add(1);
    true
}

fn service_authorized(headers: &HeaderMap, expected: &[u8; 32]) -> bool {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(authority) = value.strip_prefix("Bearer ") else {
        return false;
    };
    if authority.is_empty() || authority.contains(char::is_whitespace) {
        return false;
    }
    let authority_key = hmac::Key::new(hmac::HMAC_SHA256, SERVICE_AUTHORITY_TAG_KEY);
    hmac::verify(&authority_key, authority.as_bytes(), expected).is_ok()
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn contains_smuggled_browser_header(headers: &HeaderMap) -> bool {
    headers.keys().any(|name| {
        let name = name.as_str();
        matches!(
            name,
            "authorization" | "cookie" | "forwarded" | "upgrade" | "proxy-authorization"
        ) || name.starts_with("x-forwarded-")
            || name.starts_with("x-original-")
    })
}

fn browser_headers_are_safe(headers: &HeaderMap, public_authority: &str) -> bool {
    let mut hosts = headers.get_all(header::HOST).iter();
    let host_matches = hosts
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == public_authority)
        && hosts.next().is_none();
    host_matches && !contains_smuggled_browser_header(headers)
}

fn valid_public_authority(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && value == value.to_ascii_lowercase()
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && !value.contains(['@', '/', '\\', '?', '#'])
        && value
            .parse::<axum::http::uri::Authority>()
            .is_ok_and(|authority| !authority.host().is_empty() && authority.as_str() == value)
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn safe_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(json!({"error":{"code":code,"retryable":status.is_server_error()}})),
    )
        .into_response()
}
