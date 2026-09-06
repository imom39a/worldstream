//! Public Hosted Gateway boundary. This crate intentionally does not depend on
//! `worldstream-server` or the Studio Supervisor, so their generic route graphs
//! cannot become reachable through this listener.

use std::{
    collections::{BTreeSet, HashMap},
    convert::Infallible,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequestParts, Path, RawQuery, State, WebSocketUpgrade,
        rejection::BytesRejection,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt as _, StreamExt as _};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio_tungstenite::{
    client_async,
    tungstenite::{
        Message as UpstreamMessage, handshake::client::generate_key, http as upstream_http,
    },
};
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedBrowserHandoffRedeemRequestV1, HostedBrowserHandoffRedeemResponseV1,
    HostedBrowserHandoffRequestV1, HostedBrowserHandoffResponseV1, HostedBrowserSessionLogoutV1,
    HostedBrowserSessionRequestV1, HostedBrowserSessionStatusV1,
    HostedBrowserStreamTicketRequestV1, HostedBrowserStreamTicketResponseV1,
    HostedGenesisEvidenceV1, HostedHouseRunnerReservationReceiptV1,
    HostedHouseRunnerReservationRequestV1, HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1,
    HostedLaunchStatusV1, HostedResultSourceEvidenceV1, HostedResultSourceRequestV1,
    validate_hosted_browser_handoff_redeem_request,
    validate_hosted_browser_handoff_redeem_response, validate_hosted_browser_handoff_request,
    validate_hosted_browser_handoff_response, validate_hosted_browser_session_logout,
    validate_hosted_browser_session_request, validate_hosted_browser_session_status,
    validate_hosted_browser_stream_ticket_request, validate_hosted_browser_stream_ticket_response,
    validate_hosted_genesis_evidence, validate_hosted_house_runner_reservation_receipt,
    validate_hosted_launch_evidence_request, validate_hosted_result_source_evidence,
    validate_hosted_result_source_request,
};
use zeroize::Zeroizing;

const MAX_SERVICE_BODY_BYTES: usize = 256 * 1024;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 384 * 1024;
const MAX_UPSTREAM_HEADERS_BYTES: usize = 16 * 1024;
const MAX_LISTINGS: usize = 64;
const SERVICE_AUTHORITY_TAG_KEY: &[u8] = b"worldstream/hosted-service-authority/v1";
const WORLDSTREAM_WEBSOCKET_SUBPROTOCOL: &str = "worldstream.json.v0.1";
const MAX_BROWSER_MESSAGE_BYTES: usize = 512 * 1024;
const BROWSER_TICKET_FRAME_BYTES: usize = 69;
const BROWSER_TICKET_TIMEOUT: Duration = Duration::from_secs(15);
const BROWSER_PROXY_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BROWSER_CONNECTIONS: usize = 256;
const MAX_BROWSER_CONNECTIONS_PER_PEER: usize = 8;
const BROWSER_ADMISSION_CLOSE_REASON: &str = "browser authorization failed";

/// Closed gateway failure classes. No variant carries credentials, private
/// Projection bytes, upstream responses, or internal addresses.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HostedGatewayError {
    #[error("hosted gateway configuration is invalid")]
    InvalidConfiguration,
    #[error("hosted operation was rejected")]
    Rejected,
    #[error("hosted operation target is missing")]
    Missing,
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
    browser_stream: Option<BrowserStreamProxyConfig>,
    max_requests_per_window: u32,
    rate_window: Duration,
}

#[derive(Clone)]
struct BrowserStreamProxyConfig {
    runtime_upstream: SocketAddr,
    client_origin: Arc<str>,
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
            browser_stream: None,
            max_requests_per_window,
            rate_window,
        })
    }

    /// Installs the one loopback Runtime target used by the dedicated public
    /// browser stream path.
    ///
    /// # Errors
    /// Rejects remote Runtime targets and non-canonical Activity Client origins.
    pub fn with_browser_stream(
        mut self,
        runtime_upstream: SocketAddr,
        client_origin: impl Into<String>,
    ) -> Result<Self, HostedGatewayError> {
        let client_origin = client_origin.into();
        if !runtime_upstream.ip().is_loopback() || !valid_client_origin(&client_origin) {
            return Err(HostedGatewayError::InvalidConfiguration);
        }
        self.browser_stream = Some(BrowserStreamProxyConfig {
            runtime_upstream,
            client_origin: Arc::from(client_origin),
        });
        Ok(self)
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

    /// Reads private sequence-zero correspondence for platform reconciliation.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn genesis_evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedGenesisEvidenceV1, HostedGatewayError>;

    /// Reads one Run-bound, authorized Public Projection and optional Replay proof.
    ///
    /// # Errors
    /// Returns only a closed rejection or availability class.
    fn result_source_evidence(
        &self,
        request: &HostedResultSourceRequestV1,
    ) -> Result<HostedResultSourceEvidenceV1, HostedGatewayError>;

    /// Reserves one exact Host-local House Runner before Room creation.
    ///
    /// # Errors
    /// Returns a closed rejection or availability failure.
    fn reserve_house_runner(
        &self,
        _request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Reads one exact retained House Runner reservation.
    ///
    /// # Errors
    /// Returns a closed rejection or availability failure.
    fn read_house_runner(
        &self,
        _request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Issues one Membership- and client-bound browser handoff.
    ///
    /// # Errors
    /// Returns a closed rejection, missing-target, or availability failure.
    fn issue_browser_handoff(
        &self,
        _request: &HostedBrowserHandoffRequestV1,
    ) -> Result<HostedBrowserHandoffResponseV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Atomically consumes one hosted browser handoff and rotates a prior session.
    ///
    /// # Errors
    /// Returns a closed rejection, missing-target, or availability failure.
    fn redeem_browser_handoff(
        &self,
        _request: &HostedBrowserHandoffRedeemRequestV1,
    ) -> Result<HostedBrowserHandoffRedeemResponseV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Revalidates one opaque Browser Activity Session.
    ///
    /// # Errors
    /// Returns a closed rejection, missing-session, or availability failure.
    fn browser_session_status(
        &self,
        _request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionStatusV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Idempotently retires one opaque Browser Activity Session.
    ///
    /// # Errors
    /// Returns a closed rejection, missing-session, or availability failure.
    fn logout_browser_session(
        &self,
        _request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionLogoutV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }

    /// Issues one target-bound Runtime ticket from a current Browser Activity
    /// Session. The backend must not return Room or Membership identifiers.
    ///
    /// # Errors
    /// Returns a closed rejection, missing-session, or availability failure.
    fn issue_browser_stream_ticket(
        &self,
        _request: &HostedBrowserStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedGatewayError> {
        Err(HostedGatewayError::Rejected)
    }
}

/// Fixed loopback-only client for the Controller's reviewed hosted routes.
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

    fn genesis_evidence(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedGenesisEvidenceV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-launches:read-genesis", request)?;
        if status != 200 {
            return Err(classify_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedGenesisEvidenceV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_genesis_response(&response, request)?;
        Ok(response)
    }

    fn result_source_evidence(
        &self,
        request: &HostedResultSourceRequestV1,
    ) -> Result<HostedResultSourceEvidenceV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-launches:read-result-source", request)?;
        if status != 200 {
            return Err(classify_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedResultSourceEvidenceV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_result_source_response(&response, request)?;
        Ok(response)
    }

    fn reserve_house_runner(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        self.house_runner_call("/api/v1/hosted-house-runners:reserve", request)
    }

    fn read_house_runner(
        &self,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        self.house_runner_call("/api/v1/hosted-house-runners:read", request)
    }

    fn issue_browser_handoff(
        &self,
        request: &HostedBrowserHandoffRequestV1,
    ) -> Result<HostedBrowserHandoffResponseV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-browser-handoffs:issue", request)?;
        if status != 201 {
            return Err(classify_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedBrowserHandoffResponseV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_hosted_browser_handoff_response(&response)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        Ok(response)
    }

    fn redeem_browser_handoff(
        &self,
        request: &HostedBrowserHandoffRedeemRequestV1,
    ) -> Result<HostedBrowserHandoffRedeemResponseV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-browser-handoffs:redeem", request)?;
        if status != 201 {
            return Err(classify_browser_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedBrowserHandoffRedeemResponseV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_hosted_browser_handoff_redeem_response(&response)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        Ok(response)
    }

    fn browser_session_status(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionStatusV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-browser-sessions:status", request)?;
        if status != 200 {
            return Err(classify_browser_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedBrowserSessionStatusV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_hosted_browser_session_status(&response)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        Ok(response)
    }

    fn logout_browser_session(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionLogoutV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-browser-sessions:logout", request)?;
        if status != 200 {
            return Err(classify_browser_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedBrowserSessionLogoutV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_hosted_browser_session_logout(&response)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        Ok(response)
    }

    fn issue_browser_stream_ticket(
        &self,
        request: &HostedBrowserStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedGatewayError> {
        let (status, body) = self.call("/api/v1/hosted-browser-sessions:stream-ticket", request)?;
        if status != 201 {
            return Err(classify_browser_upstream_status(status));
        }
        let response = serde_json::from_slice::<HostedBrowserStreamTicketResponseV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_hosted_browser_stream_ticket_response(&response)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        Ok(response)
    }
}

impl FixedHostAdapterBackend {
    fn house_runner_call(
        &self,
        path: &'static str,
        request: &HostedHouseRunnerReservationRequestV1,
    ) -> Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError> {
        let (status, body) = self.call(path, request)?;
        if status != 200 {
            return Err(classify_upstream_status(status));
        }
        let receipt = serde_json::from_slice::<HostedHouseRunnerReservationReceiptV1>(&body)
            .map_err(|_| HostedGatewayError::Unavailable)?;
        validate_house_runner_response(&receipt, request)?;
        Ok(receipt)
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

#[derive(Default)]
struct BrowserConnectionState {
    total: usize,
    peers: HashMap<String, usize>,
}

#[derive(Clone, Default)]
struct BrowserConnectionLimiter {
    state: Arc<Mutex<BrowserConnectionState>>,
}

impl BrowserConnectionLimiter {
    fn reserve(&self, peer: String) -> Option<BrowserConnectionPermit> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let peer_count = state.peers.get(&peer).copied().unwrap_or(0);
        if state.total >= MAX_BROWSER_CONNECTIONS || peer_count >= MAX_BROWSER_CONNECTIONS_PER_PEER
        {
            return None;
        }
        state.total += 1;
        state.peers.insert(peer.clone(), peer_count + 1);
        Some(BrowserConnectionPermit {
            limiter: self.clone(),
            peer,
        })
    }

    fn release(&self, peer: &str) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.total = state.total.saturating_sub(1);
        if let Some(count) = state.peers.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.peers.remove(peer);
            }
        }
    }
}

struct BrowserConnectionPermit {
    limiter: BrowserConnectionLimiter,
    peer: String,
}

struct GatewayPeer(String);

impl<S> FromRequestParts<S> for GatewayPeer
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map_or_else(
                    || "unknown".to_owned(),
                    |ConnectInfo(address)| address.ip().to_string(),
                ),
        ))
    }
}

impl Drop for BrowserConnectionPermit {
    fn drop(&mut self) {
        self.limiter.release(&self.peer);
    }
}

#[derive(Clone)]
struct GatewayState {
    config: HostedGatewayConfig,
    backend: Arc<dyn HostedGatewayBackend>,
    rate: Arc<Mutex<RateWindow>>,
    browser_connections: BrowserConnectionLimiter,
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
        browser_connections: BrowserConnectionLimiter::default(),
    };
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route("/version", get(version))
        .route("/v1/hosted/launch", post(launch))
        .route("/v1/hosted/evidence", post(evidence))
        .route("/v1/hosted/genesis-evidence", post(genesis_evidence))
        .route(
            "/v1/hosted/result-source-evidence",
            post(result_source_evidence),
        )
        .route(
            "/v1/hosted/house-runners/reserve",
            post(reserve_house_runner),
        )
        .route("/v1/hosted/house-runners/read", post(read_house_runner))
        .route(
            "/v1/hosted/browser-handoffs/issue",
            post(issue_browser_handoff),
        )
        .route(
            "/v1/hosted/browser-sessions/admit",
            post(redeem_browser_handoff),
        )
        .route(
            "/v1/hosted/browser-sessions/status",
            post(browser_session_status),
        )
        .route(
            "/v1/hosted/browser-sessions/logout",
            post(logout_browser_session),
        )
        .route(
            "/v1/hosted/browser-sessions/stream-ticket",
            post(issue_browser_stream_ticket),
        )
        .route("/v1/hosted/browser-stream", get(browser_stream))
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

async fn genesis_evidence(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    genesis_evidence_operation(&state, &headers, &body).await
}

async fn result_source_evidence(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    result_source_evidence_operation(&state, &headers, &body).await
}

async fn reserve_house_runner(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    house_runner_operation(&state, &headers, &body, true).await
}

async fn read_house_runner(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    house_runner_operation(&state, &headers, &body, false).await
}

async fn issue_browser_handoff(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    browser_handoff_issue_operation(&state, &headers, &body).await
}

async fn redeem_browser_handoff(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    browser_handoff_redeem_operation(&state, &headers, &body).await
}

async fn browser_session_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    browser_session_operation(&state, &headers, &body, false).await
}

async fn logout_browser_session(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    browser_session_operation(&state, &headers, &body, true).await
}

async fn issue_browser_stream_ticket(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(body) = body else {
        return safe_error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large");
    };
    browser_stream_ticket_operation(&state, &headers, &body).await
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

async fn genesis_evidence_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
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
    let result = tokio::task::spawn_blocking(move || backend.genesis_evidence(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    genesis_result(&listing_revision_digest, result)
}

async fn result_source_evidence_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedResultSourceRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_result_source_request(&request).is_err() {
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
    let result = tokio::task::spawn_blocking(move || backend.result_source_evidence(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    result_source_result(&listing_revision_digest, result)
}

async fn house_runner_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
    reserve: bool,
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedHouseRunnerReservationRequestV1>(body) else {
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
    let operation = if reserve {
        "house_reserve"
    } else {
        "house_read"
    };
    let result = tokio::task::spawn_blocking(move || {
        if reserve {
            backend.reserve_house_runner(&request)
        } else {
            backend.read_house_runner(&request)
        }
    })
    .await
    .map_err(|_| HostedGatewayError::Unavailable)
    .and_then(|result| result);
    house_runner_result(operation, &listing_revision_digest, result)
}

async fn browser_handoff_issue_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedBrowserHandoffRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_browser_handoff_request(&request).is_err() {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !listing_allowed(state, &request.listing_revision_digest) {
        return safe_error(StatusCode::FORBIDDEN, "listing_not_allowed");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || backend.issue_browser_handoff(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    browser_service_result(result, StatusCode::CREATED)
}

async fn browser_handoff_redeem_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedBrowserHandoffRedeemRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_browser_handoff_redeem_request(&request).is_err() {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || backend.redeem_browser_handoff(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    browser_service_result(result, StatusCode::CREATED)
}

async fn browser_session_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
    logout: bool,
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedBrowserSessionRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_browser_session_request(&request).is_err() {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    if logout {
        let result = tokio::task::spawn_blocking(move || backend.logout_browser_session(&request))
            .await
            .map_err(|_| HostedGatewayError::Unavailable)
            .and_then(|result| result);
        browser_service_result(result, StatusCode::OK)
    } else {
        let result = tokio::task::spawn_blocking(move || backend.browser_session_status(&request))
            .await
            .map_err(|_| HostedGatewayError::Unavailable)
            .and_then(|result| result);
        browser_service_result(result, StatusCode::OK)
    }
}

async fn browser_stream_ticket_operation(
    state: &GatewayState,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    if let Some(response) = reject_service_envelope(state, headers) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<HostedBrowserStreamTicketRequestV1>(body) else {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if validate_hosted_browser_stream_ticket_request(&request).is_err() {
        return safe_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if !admit_rate(state) {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || backend.issue_browser_stream_ticket(&request))
        .await
        .map_err(|_| HostedGatewayError::Unavailable)
        .and_then(|result| result);
    browser_service_result(result, StatusCode::CREATED)
}

fn browser_service_result<T: Serialize>(
    result: Result<T, HostedGatewayError>,
    status: StatusCode,
) -> Response {
    match result {
        Ok(value) => no_store((status, Json(value)).into_response()),
        Err(HostedGatewayError::Missing) => {
            safe_error(StatusCode::UNAUTHORIZED, "browser_session_missing")
        }
        Err(HostedGatewayError::Rejected | HostedGatewayError::InvalidConfiguration) => {
            safe_error(StatusCode::FORBIDDEN, "browser_session_rejected")
        }
        Err(HostedGatewayError::Unavailable) => safe_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser_session_unavailable",
        ),
    }
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
        Err(
            HostedGatewayError::Rejected
            | HostedGatewayError::Missing
            | HostedGatewayError::InvalidConfiguration,
        ) => {
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

fn house_runner_result(
    operation: &'static str,
    listing_revision_digest: &str,
    result: Result<HostedHouseRunnerReservationReceiptV1, HostedGatewayError>,
) -> Response {
    match result {
        Ok(receipt) => {
            tracing::info!(
                target: "worldstream.hosted_gateway",
                operation,
                listing_revision_digest,
                outcome = "accepted",
                "hosted gateway operation"
            );
            no_store((StatusCode::OK, Json(receipt)).into_response())
        }
        Err(
            HostedGatewayError::Rejected
            | HostedGatewayError::Missing
            | HostedGatewayError::InvalidConfiguration,
        ) => safe_error(StatusCode::CONFLICT, "operation_rejected"),
        Err(HostedGatewayError::Unavailable) => {
            safe_error(StatusCode::SERVICE_UNAVAILABLE, "operation_unavailable")
        }
    }
}

fn genesis_result(
    listing_revision_digest: &str,
    result: Result<HostedGenesisEvidenceV1, HostedGatewayError>,
) -> Response {
    match result {
        Ok(evidence) => {
            tracing::info!(
                target: "worldstream.hosted_gateway",
                operation = "genesis_evidence",
                listing_revision_digest,
                outcome = "accepted",
                "hosted gateway operation"
            );
            no_store((StatusCode::OK, Json(evidence)).into_response())
        }
        Err(
            HostedGatewayError::Rejected
            | HostedGatewayError::Missing
            | HostedGatewayError::InvalidConfiguration,
        ) => safe_error(StatusCode::CONFLICT, "operation_rejected"),
        Err(HostedGatewayError::Unavailable) => {
            safe_error(StatusCode::SERVICE_UNAVAILABLE, "operation_unavailable")
        }
    }
}

fn result_source_result(
    listing_revision_digest: &str,
    result: Result<HostedResultSourceEvidenceV1, HostedGatewayError>,
) -> Response {
    match result {
        Ok(evidence) => {
            tracing::info!(
                target: "worldstream.hosted_gateway",
                operation = "result_source_evidence",
                listing_revision_digest,
                outcome = "accepted",
                "hosted gateway operation"
            );
            no_store((StatusCode::OK, Json(evidence)).into_response())
        }
        Err(
            HostedGatewayError::Rejected
            | HostedGatewayError::Missing
            | HostedGatewayError::InvalidConfiguration,
        ) => safe_error(StatusCode::CONFLICT, "operation_rejected"),
        Err(HostedGatewayError::Unavailable) => {
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
                    "/api/v1/hosted-launches:submit"
                        | "/api/v1/hosted-launches:read"
                        | "/api/v1/hosted-launches:read-genesis"
                        | "/api/v1/hosted-launches:read-result-source"
                        | "/api/v1/hosted-house-runners:reserve"
                        | "/api/v1/hosted-house-runners:read"
                        | "/api/v1/hosted-browser-handoffs:issue"
                        | "/api/v1/hosted-browser-handoffs:redeem"
                        | "/api/v1/hosted-browser-sessions:status"
                        | "/api/v1/hosted-browser-sessions:logout"
                        | "/api/v1/hosted-browser-sessions:stream-ticket"
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

fn classify_browser_upstream_status(status: u16) -> HostedGatewayError {
    match status {
        401 | 404 => HostedGatewayError::Missing,
        400 | 403 | 409 | 422 => HostedGatewayError::Rejected,
        _ => HostedGatewayError::Unavailable,
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

fn validate_genesis_response(
    response: &HostedGenesisEvidenceV1,
    request: &HostedLaunchEvidenceRequestV1,
) -> Result<(), HostedGatewayError> {
    validate_hosted_genesis_evidence(response).map_err(|_| HostedGatewayError::Unavailable)?;
    if response.listing_revision_digest != request.listing_revision_digest
        || response.launch_request_digest != request.launch_request_digest
        || response.room_setup_operation_id != request.room_setup_operation_id
    {
        return Err(HostedGatewayError::Unavailable);
    }
    Ok(())
}

fn validate_result_source_response(
    response: &HostedResultSourceEvidenceV1,
    request: &HostedResultSourceRequestV1,
) -> Result<(), HostedGatewayError> {
    validate_hosted_result_source_evidence(response)
        .map_err(|_| HostedGatewayError::Unavailable)?;
    if response.run_id != request.run_id
        || response.listing_revision_digest != request.listing_revision_digest
        || response.launch_request_digest != request.launch_request_digest
        || response.room_setup_operation_id != request.room_setup_operation_id
    {
        return Err(HostedGatewayError::Unavailable);
    }
    Ok(())
}

fn validate_house_runner_response(
    receipt: &HostedHouseRunnerReservationReceiptV1,
    request: &HostedHouseRunnerReservationRequestV1,
) -> Result<(), HostedGatewayError> {
    validate_hosted_house_runner_reservation_receipt(receipt)
        .map_err(|_| HostedGatewayError::Unavailable)?;
    if receipt.host_installation_id != request.host_installation_id
        || receipt.reservation_operation_id != request.reservation_operation_id
        || receipt.launch_request_id != request.launch_request_id
        || receipt.listing_revision_digest != request.listing_revision_digest
        || receipt.seat_id != request.seat_id
        || receipt.house_agent_revision_digest != request.house_agent_revision_digest
    {
        return Err(HostedGatewayError::Unavailable);
    }
    Ok(())
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

async fn browser_stream(
    State(state): State<GatewayState>,
    RawQuery(query): RawQuery,
    GatewayPeer(peer): GatewayPeer,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(proxy) = state.config.browser_stream.clone() else {
        return safe_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser_stream_unavailable",
        );
    };
    if query.is_some()
        || !browser_stream_headers_are_safe(
            &headers,
            &state.config.public_authority,
            &proxy.client_origin,
        )
        || !admit_rate(&state)
    {
        return safe_error(StatusCode::FORBIDDEN, "browser_stream_rejected");
    }
    let protocols = upgrade.requested_protocols().collect::<Vec<_>>();
    if protocols.len() != 1
        || protocols[0].as_bytes() != WORLDSTREAM_WEBSOCKET_SUBPROTOCOL.as_bytes()
    {
        return safe_error(StatusCode::BAD_REQUEST, "browser_stream_rejected");
    }
    let Some(permit) = state.browser_connections.reserve(peer) else {
        return safe_error(StatusCode::TOO_MANY_REQUESTS, "browser_stream_capacity");
    };
    let origin = proxy.client_origin.to_string();
    upgrade
        .max_message_size(MAX_BROWSER_MESSAGE_BYTES)
        .max_frame_size(MAX_BROWSER_MESSAGE_BYTES)
        .protocols([WORLDSTREAM_WEBSOCKET_SUBPROTOCOL])
        .on_upgrade(move |socket| {
            proxy_browser_stream(socket, proxy.runtime_upstream, origin, permit)
        })
        .into_response()
}

async fn proxy_browser_stream(
    mut browser: WebSocket,
    runtime_upstream: SocketAddr,
    origin: String,
    _permit: BrowserConnectionPermit,
) {
    let Ok(stream) = tokio::net::TcpStream::connect(runtime_upstream).await else {
        close_browser_proxy(&mut browser).await;
        return;
    };
    let Ok(request) = upstream_http::Request::builder()
        .method("GET")
        .uri(format!("ws://{runtime_upstream}/v1/hosted/browser-stream"))
        .header("Host", runtime_upstream.to_string())
        .header("Origin", origin)
        .header("Sec-WebSocket-Protocol", WORLDSTREAM_WEBSOCKET_SUBPROTOCOL)
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .body(())
    else {
        close_browser_proxy(&mut browser).await;
        return;
    };
    let Ok((mut runtime, response)) = client_async(request, stream).await else {
        close_browser_proxy(&mut browser).await;
        return;
    };
    let selected = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok());
    if selected != Some(WORLDSTREAM_WEBSOCKET_SUBPROTOCOL) {
        close_browser_proxy(&mut browser).await;
        return;
    }
    let first = tokio::time::timeout(BROWSER_TICKET_TIMEOUT, browser.recv()).await;
    let ticket = match first {
        Ok(Some(Ok(Message::Text(ticket)))) if ticket.len() == BROWSER_TICKET_FRAME_BYTES => ticket,
        _ => {
            close_browser_proxy(&mut browser).await;
            return;
        }
    };
    if !matches!(
        tokio::time::timeout(
            BROWSER_PROXY_SEND_TIMEOUT,
            runtime.send(UpstreamMessage::Text(ticket.to_string().into())),
        )
        .await,
        Ok(Ok(()))
    ) {
        close_browser_proxy(&mut browser).await;
        return;
    }
    loop {
        tokio::select! {
            message = browser.recv() => {
                let Some(Ok(message)) = message else { break; };
                let Some(message) = browser_to_upstream(message) else { break; };
                if !matches!(
                    tokio::time::timeout(BROWSER_PROXY_SEND_TIMEOUT, runtime.send(message)).await,
                    Ok(Ok(()))
                )
                {
                    break;
                }
            }
            message = runtime.next() => {
                let Some(Ok(message)) = message else { break; };
                let Some(message) = upstream_to_browser(message) else { break; };
                if !matches!(
                    tokio::time::timeout(BROWSER_PROXY_SEND_TIMEOUT, browser.send(message)).await,
                    Ok(Ok(()))
                )
                {
                    break;
                }
            }
        }
    }
}

fn browser_to_upstream(message: Message) -> Option<UpstreamMessage> {
    match message {
        Message::Text(text) if text.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(UpstreamMessage::Text(text.to_string().into()))
        }
        Message::Binary(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(UpstreamMessage::Binary(bytes))
        }
        Message::Ping(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(UpstreamMessage::Ping(bytes))
        }
        Message::Pong(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(UpstreamMessage::Pong(bytes))
        }
        Message::Close(_) => Some(UpstreamMessage::Close(None)),
        _ => None,
    }
}

fn upstream_to_browser(message: UpstreamMessage) -> Option<Message> {
    match message {
        UpstreamMessage::Text(text) if text.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(Message::Text(text.to_string().into()))
        }
        UpstreamMessage::Binary(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(Message::Binary(bytes))
        }
        UpstreamMessage::Ping(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(Message::Ping(bytes))
        }
        UpstreamMessage::Pong(bytes) if bytes.len() <= MAX_BROWSER_MESSAGE_BYTES => {
            Some(Message::Pong(bytes))
        }
        UpstreamMessage::Close(_) => Some(Message::Close(None)),
        _ => None,
    }
}

async fn close_browser_proxy(browser: &mut WebSocket) {
    let _ = tokio::time::timeout(
        BROWSER_PROXY_SEND_TIMEOUT,
        browser.send(Message::Close(Some(CloseFrame {
            code: 1008,
            reason: BROWSER_ADMISSION_CLOSE_REASON.into(),
        }))),
    )
    .await;
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

fn browser_stream_headers_are_safe(
    headers: &HeaderMap,
    public_authority: &str,
    client_origin: &str,
) -> bool {
    let mut hosts = headers.get_all(header::HOST).iter();
    let host_matches = hosts
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == public_authority)
        && hosts.next().is_none();
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin_matches = origins
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == client_origin)
        && origins.next().is_none();
    host_matches
        && origin_matches
        && !headers.contains_key(header::AUTHORIZATION)
        && !headers.contains_key(header::COOKIE)
        && !headers.contains_key("forwarded")
        && !headers.contains_key("proxy-authorization")
        && !headers.keys().any(|name| {
            let name = name.as_str();
            name.starts_with("x-forwarded-") || name.starts_with("x-original-")
        })
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

fn valid_client_origin(value: &str) -> bool {
    let Ok(uri) = value.parse::<axum::http::Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = authority.host();
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
    let scheme_valid = if loopback {
        matches!(uri.scheme_str(), Some("http" | "https"))
    } else {
        uri.scheme_str() == Some("https")
    };
    if !scheme_valid
        || (uri.path() != "" && uri.path() != "/")
        || uri.query().is_some()
        || value.contains('@')
        || (!loopback
            && (host != host.to_ascii_lowercase()
                || !host.contains('.')
                || host.split('.').any(|label| {
                    label.is_empty()
                        || label.starts_with('-')
                        || label.ends_with('-')
                        || !label
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })))
    {
        return false;
    }
    let canonical = match (uri.scheme_str(), authority.port_u16()) {
        (Some("https"), Some(443) | None) => format!("https://{host}"),
        (Some(scheme), Some(port)) => format!("{scheme}://{host}:{port}"),
        (Some(scheme), None) => format!("{scheme}://{host}"),
        _ => return false,
    };
    canonical == value
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
