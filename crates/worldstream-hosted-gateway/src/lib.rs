//! Public Hosted Gateway boundary. This crate intentionally does not depend on
//! `worldstream-server` or the Studio Supervisor, so their generic route graphs
//! cannot become reachable through this listener.

use std::{
    collections::BTreeSet,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1, HostedLaunchStatusV1,
    validate_hosted_launch_evidence_request,
};
use zeroize::Zeroizing;

const MAX_SERVICE_BODY_BYTES: usize = 256 * 1024;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_UPSTREAM_HEADERS_BYTES: usize = 16 * 1024;
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

/// Narrow backend seam between the public gateway and the retained Host adapter.
pub trait HostedGatewayBackend: Send + Sync + 'static {
    fn ready(&self) -> bool;

    /// Begins or resumes one typed, allowlisted launch operation.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn launch(
        &self,
        request: &HostedLaunchRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError>;

    /// Reads one typed, allowlisted evidence operation.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError>;
}

/// Fixed loopback-only client for the Controller's three hosted-launch routes.
/// It cannot forward caller paths, headers, credentials, or arbitrary operations.
#[derive(Clone)]
pub struct FixedHostAdapterBackend {
    upstream: SocketAddr,
    controller_authority: Arc<Zeroizing<String>>,
    timeout: Duration,
}

impl FixedHostAdapterBackend {
    /// Builds a least-privilege adapter using one literal loopback endpoint.
    ///
    /// # Errors
    /// Rejects remote endpoints, weak authority material, and unsafe timeouts.
    pub fn new(
        upstream: SocketAddr,
        controller_authority: String,
        timeout: Duration,
    ) -> Result<Self, HostedGatewayError> {
        if !upstream.ip().is_loopback()
            || controller_authority.len() < 32
            || controller_authority.len() > 512
            || !controller_authority
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || timeout < Duration::from_millis(100)
            || timeout > Duration::from_secs(30)
        {
            return Err(HostedGatewayError::InvalidConfiguration);
        }
        Ok(Self {
            upstream,
            controller_authority: Arc::new(Zeroizing::new(controller_authority)),
            timeout,
        })
    }

    fn call<T: Serialize>(
        &self,
        path: &'static str,
        body: &T,
    ) -> Result<(u16, Vec<u8>), HostedGatewayError> {
        let encoded = serde_json::to_vec(body).map_err(|_| HostedGatewayError::Rejected)?;
        let body = CanonicalJsonV1::parse(&encoded)
            .and_then(|value| value.to_bytes())
            .map_err(|_| HostedGatewayError::Rejected)?;
        fixed_http_request(
            self.upstream,
            self.timeout,
            "POST",
            path,
            &self.controller_authority,
            &body,
        )
    }
}

impl HostedGatewayBackend for FixedHostAdapterBackend {
    fn ready(&self) -> bool {
        fixed_http_request(
            self.upstream,
            self.timeout,
            "GET",
            "/api/v1/hosted-launches/ready",
            &self.controller_authority,
            &[],
        )
        .ok()
        .filter(|(status, _)| *status == 200)
        .and_then(|(_, body)| serde_json::from_slice::<HostedAdapterReadinessV1>(&body).ok())
        .is_some_and(|ready| {
            ready.schema == "worldstream/hosted-launch-readiness/v1" && ready.ready
        })
    }

    fn launch(
        &self,
        request: &HostedLaunchRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-launches:submit", request)?;
        if !matches!(status, 200 | 202) {
            return Err(classify_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedLaunchStatusV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_launch_response(
            &response,
            &request.listing_revision_digest,
            &request.launch_request_digest,
            &request.room_setup_operation_id,
        )?;
        Ok(response)
    }

    fn evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-launches:read", request)?;
        if status != 200 {
            return Err(classify_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedLaunchStatusV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_launch_response(
            &response,
            &request.listing_revision_digest,
            &request.launch_request_digest,
            &request.room_setup_operation_id,
        )?;
        Ok(response)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedAdapterReadinessV1 {
    schema: String,
    ready: bool,
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
    let backend = Arc::clone(&state.backend);
    let ready = tokio::task::spawn_blocking(move || backend.ready())
        .await
        .unwrap_or(false);
    let (status, value) = if ready {
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
    launch_operation(&state, &headers, &body).await
}

async fn evidence(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    evidence_operation(&state, &headers, &body).await
}

async fn launch_operation(state: &GatewayState, headers: &HeaderMap, body: &[u8]) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedLaunchRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if !is_digest(&request.listing_revision_digest) {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !listing_allowed(state, &request.listing_revision_digest) {
        return safe_error(StatusCode::FORBIDDEN, "listing_not_allowed");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    let listing_revision_digest = request.listing_revision_digest.clone();
    let result = tokio::task::spawn_blocking(move || backend.launch(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    service_result(
        "launch",
        &listing_revision_digest,
        result,
        StatusCode::ACCEPTED,
    )
}

async fn evidence_operation(state: &GatewayState, headers: &HeaderMap, body: &[u8]) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedLaunchEvidenceRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_launch_evidence_request(&request).is_err() {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !listing_allowed(state, &request.listing_revision_digest) {
        return safe_error(StatusCode::FORBIDDEN, "listing_not_allowed");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    let listing_revision_digest = request.listing_revision_digest.clone();
    let result = tokio::task::spawn_blocking(move || backend.evidence(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    service_result("evidence", &listing_revision_digest, result, StatusCode::OK)
}

fn reject_service_envelope(state: &GatewayState, headers: &HeaderMap) -> Option<Response> {
    if !is_json(headers) {
        return Some(safe_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
        ));
    }
    if !service_authorized(headers, &state.config.service_authority_tag) {
        return Some(safe_error(
            StatusCode::UNAUTHORIZED,
            "service_authority_required",
        ));
    }
    None
}

fn listing_allowed(state: &GatewayState, listing_revision_digest: &str) -> bool {
    state
        .config
        .listing_allowlist
        .contains(listing_revision_digest)
}

fn service_result(
    operation: &'static str,
    listing_revision_digest: &str,
    result: Result<HostedLaunchStatusV1, HostedGatewayError>,
    pending_status: StatusCode,
) -> Response {
    match result {
        Ok(status) => {
            tracing::info!(
                target: "worldstream.hosted_gateway",
                operation,
                listing_revision_digest,
                outcome = "accepted",
                "hosted gateway operation"
            );
            let response_status = if status.lobby_launch_committed {
                StatusCode::OK
            } else {
                pending_status
            };
            no_store((response_status, Json(status)).into_response())
        }
        Err(HostedGatewayError::Rejected | HostedGatewayError::InvalidConfiguration) => {
            tracing::warn!(
                target: "worldstream.hosted_gateway",
                operation,
                listing_revision_digest,
                outcome = "rejected",
                "hosted gateway operation"
            );
            safe_error(StatusCode::CONFLICT, "operation_rejected")
        }
        Err(HostedGatewayError::Unavailable) => {
            tracing::warn!(
                target: "worldstream.hosted_gateway",
                operation,
                listing_revision_digest,
                outcome = "unavailable",
                "hosted gateway operation"
            );
            safe_error(StatusCode::SERVICE_UNAVAILABLE, "operation_unavailable")
        }
    }
}

fn fixed_http_request(
    upstream: SocketAddr,
    timeout: Duration,
    method: &'static str,
    path: &'static str,
    controller_authority: &str,
    body: &[u8],
) -> Result<(u16, Vec<u8>), HostedGatewayError> {
    if !upstream.ip().is_loopback()
        || !matches!(
            (method, path),
            ("GET", "/api/v1/hosted-launches/ready")
                | (
                    "POST",
                    "/api/v1/hosted-launches:submit" | "/api/v1/hosted-launches:read"
                )
        )
        || body.len() > MAX_SERVICE_BODY_BYTES
    {
        return Err(HostedGatewayError::Rejected);
    }
    let mut request = Zeroizing::new(Vec::with_capacity(body.len().saturating_add(512)));
    write!(
        &mut *request,
        "{method} {path} HTTP/1.1\r\nHost: {upstream}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {controller_authority}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .map_err(|_| HostedGatewayError::Unavailable)?;
    request.extend_from_slice(body);

    let stream = TcpStream::connect_timeout(&upstream, timeout)
        .map_err(|_| HostedGatewayError::Unavailable)?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|_| HostedGatewayError::Unavailable)?;
    let mut reader = BufReader::new(stream);
    reader
        .get_mut()
        .write_all(&request)
        .and_then(|()| reader.get_mut().flush())
        .map_err(|_| HostedGatewayError::Unavailable)?;

    let mut used = 0_usize;
    let status_line = read_upstream_line(&mut reader, &mut used)?;
    let mut fields = status_line.split_whitespace();
    if fields.next() != Some("HTTP/1.1") {
        return Err(HostedGatewayError::Unavailable);
    }
    let status = fields
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| (200..=599).contains(value))
        .ok_or(HostedGatewayError::Unavailable)?;
    let mut content_length = None;
    loop {
        let line = read_upstream_line(&mut reader, &mut used)?;
        if line == "\r\n" {
            break;
        }
        let (name, value) = line
            .trim_end_matches(['\r', '\n'])
            .split_once(':')
            .ok_or(HostedGatewayError::Unavailable)?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(HostedGatewayError::Unavailable);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(HostedGatewayError::Unavailable);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|length| *length <= MAX_UPSTREAM_RESPONSE_BYTES)
                    .ok_or(HostedGatewayError::Unavailable)?,
            );
        }
    }
    let content_length = content_length.ok_or(HostedGatewayError::Unavailable)?;
    let mut response = vec![0_u8; content_length];
    reader
        .read_exact(&mut response)
        .map_err(|_| HostedGatewayError::Unavailable)?;
    Ok((status, response))
}

fn read_upstream_line(
    reader: &mut BufReader<TcpStream>,
    used: &mut usize,
) -> Result<String, HostedGatewayError> {
    let remaining = MAX_UPSTREAM_HEADERS_BYTES
        .checked_sub(*used)
        .filter(|remaining| *remaining > 0)
        .ok_or(HostedGatewayError::Unavailable)?;
    let mut line = String::new();
    let count = reader
        .take(u64::try_from(remaining).unwrap_or(u64::MAX))
        .read_line(&mut line)
        .map_err(|_| HostedGatewayError::Unavailable)?;
    *used = used.saturating_add(count);
    if count == 0 || !line.ends_with("\r\n") {
        return Err(HostedGatewayError::Unavailable);
    }
    Ok(line)
}

fn classify_upstream_status(status: u16) -> HostedGatewayError {
    if matches!(status, 400 | 401 | 403 | 404 | 409 | 422) {
        HostedGatewayError::Rejected
    } else {
        HostedGatewayError::Unavailable
    }
}

fn validate_launch_response(
    response: &HostedLaunchStatusV1,
    listing_revision_digest: &str,
    launch_request_digest: &str,
    room_setup_operation_id: &str,
) -> Result<(), HostedGatewayError> {
    if response.schema != "worldstream/hosted-launch-status/v1"
        || response.listing_revision_digest != listing_revision_digest
        || response.launch_request_digest != launch_request_digest
        || response.room_setup_operation_id != room_setup_operation_id
        || response
            .room_id
            .as_deref()
            .is_some_and(|room_id| !safe_reference(room_id))
        || (response.lobby_launch_committed
            && (!response.room_setup_complete || response.room_id.is_none()))
        || (response.terminal_before_genesis
            && (response.lobby_launch_committed
                || response.retryable
                || response.stage
                    != worldstream_hosted_contract::HostedLaunchStageV1::NeedsAttention))
    {
        return Err(HostedGatewayError::Unavailable);
    }
    Ok(())
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
    no_store(
        (
            status,
            Json(json!({"error":{"code":code,"retryable":status.is_server_error()}})),
        )
            .into_response(),
    )
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    response
}
