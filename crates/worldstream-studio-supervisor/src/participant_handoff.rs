//! Local, opaque Participant Console handoff and membership-bound session proxy.

use std::{
    collections::HashMap,
    fmt,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tungstenite::handshake::client::generate_key;
use tungstenite::{Message, WebSocket, client, http};
use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_core::CanonicalJsonV1;
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionRejected, BROWSER_WS_TICKET_VERSION,
    BrowserWebSocketTicketIssueResponse, ClientHello, ClientMode,
    HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION, HOSTED_BROWSER_WS_TICKET_VERSION,
    HostedBrowserWebSocketSessionRevokeRequest, HostedBrowserWebSocketTicketIssueRequest,
    ObservationDeliver, PROTOCOL_VERSION, PackReference, ProjectionReset,
    REQUIRED_CLIENT_CAPABILITIES, ReplayResponse, RoomAttached, RoomHead, SealedCapabilityBearerV1,
    VersionedEnvelope, WEBSOCKET_SUBPROTOCOL,
};
use worldstream_runtime::is_exact_loopback_origin;
use zeroize::Zeroizing;

use crate::client_bindings::{
    ClientBindingStoreErrorV1, ClientCandidateClassV1, ClientCandidateV1, ClientSelectionRequestV1,
    ClientSelectionSourceV1, ClientSelectionV1, DeploymentTrustLevelV1,
};

const HANDOFF_VERSION: &str = "activity_client_handoff.v1";
const SESSION_VERSION: &str = "participant_console_session.v1";
const CLIENT_CONTRACT_V1: &str = "worldstream/activity-client-protocol/v1";
const HANDOFF_HEADER: &str = "x-worldstream-participant-handoff";
const SESSION_COOKIE: &str = "ws_participant_session";
const SESSION_COOKIE_PATH: &str = "/api/v1/participant-console";
const SESSION_MAX_AGE_SECONDS: u64 = 12 * 60 * 60;
const TOKEN_BYTES: usize = 32;
const TOKEN_WIRE_LENGTH: usize = 69;
const MAX_SEAT_ID_BYTES: usize = 128;
const MAX_ACTION_TYPE_BYTES: usize = 128;
const MAX_OFFER_ID_BYTES: usize = 256;
const MAX_ACTION_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_REPLAY_RESPONSE_BYTES: u64 = 256 * 1024;
const MAX_TICKET_RESPONSE_BYTES: u64 = 16 * 1024;

/// Exact, non-serializable authority retained by one participant console.
pub struct HumanSeatAuthorityV1 {
    room_id: String,
    member_id: String,
    pack: PackReference,
    access_mode: AccessMode,
    role: Option<String>,
    principal_kind: worldstream_protocol::PrincipalKind,
    bearer: SealedCapabilityBearerV1,
}

impl HumanSeatAuthorityV1 {
    /// Constructs one exact Membership authority after validating both identities.
    ///
    /// # Errors
    ///
    /// Returns an error when either identity is not a canonical ULID.
    pub fn new(
        room_id: &str,
        member_id: &str,
        pack: PackReference,
        access_mode: AccessMode,
        role: Option<String>,
        bearer: SealedCapabilityBearerV1,
    ) -> Result<Self, ParticipantHandoffAuthorityErrorV1> {
        if !is_ulid(room_id)
            || !is_ulid(member_id)
            || role.as_deref().is_some_and(|role| {
                role.is_empty()
                    || role.len() > MAX_SEAT_ID_BYTES
                    || !role.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
            })
        {
            return Err(ParticipantHandoffAuthorityErrorV1::Unavailable);
        }
        Ok(Self {
            room_id: room_id.to_owned(),
            member_id: member_id.to_owned(),
            pack,
            access_mode,
            role,
            principal_kind: worldstream_protocol::PrincipalKind::Human,
            bearer,
        })
    }

    /// Constructs browser authority for an account-controlled human or agent
    /// Membership. Ordinary Participant Console callers remain human-only by
    /// using [`Self::new`].
    ///
    /// # Errors
    /// Rejects malformed Room and Membership identities.
    pub fn new_for_principal(
        room_id: &str,
        member_id: &str,
        pack: PackReference,
        access_mode: AccessMode,
        role: Option<String>,
        principal_kind: worldstream_protocol::PrincipalKind,
        bearer: SealedCapabilityBearerV1,
    ) -> Result<Self, ParticipantHandoffAuthorityErrorV1> {
        let mut authority = Self::new(room_id, member_id, pack, access_mode, role, bearer)?;
        authority.principal_kind = principal_kind;
        Ok(authority)
    }

    /// Returns the exact Room binding for the internal daemon adapter.
    #[must_use]
    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    /// Returns the exact Membership binding for the internal daemon adapter.
    #[must_use]
    pub fn member_id(&self) -> &str {
        &self.member_id
    }

    /// Returns the exact retained Activity Pack binding for client selection.
    #[must_use]
    pub const fn pack(&self) -> &PackReference {
        &self.pack
    }

    /// Returns the current Membership access mode used for client selection.
    #[must_use]
    pub const fn access_mode(&self) -> AccessMode {
        self.access_mode
    }

    /// Returns the current Membership role used for client selection.
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// Returns the exact Principal kind expected from Room attachment.
    #[must_use]
    pub const fn principal_kind(&self) -> worldstream_protocol::PrincipalKind {
        self.principal_kind
    }

    /// Returns the sealed participant bearer only at the internal daemon adapter.
    #[must_use]
    pub const fn bearer(&self) -> &SealedCapabilityBearerV1 {
        &self.bearer
    }

    fn install_current_membership(&mut self, current: &CurrentMembershipSnapshotV1) {
        self.pack.clone_from(&current.pack);
        self.access_mode = current.access_mode;
        self.role.clone_from(&current.role);
    }
}

impl fmt::Debug for HumanSeatAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HumanSeatAuthorityV1(REDACTED)")
    }
}

/// Closed resolution failures for an exact reviewed seat.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantHandoffAuthorityErrorV1 {
    SeatNotFound,
    NotHuman,
    NotProvisioned,
    AuthorityInvalid,
    Unavailable,
}

/// Resolves only participant authority for the exact provisioned human seat.
pub trait ParticipantHandoffAuthoritySourceV1: Send + Sync + 'static {
    /// Resolves authority without ever returning host or Runner authority.
    ///
    /// # Errors
    ///
    /// Returns a closed seat, provisioning, validity, or availability result.
    fn resolve_provisioned_human_seat(
        &self,
        draft_id: &str,
        seat_id: &str,
    ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1>;
}

/// Browser-safe health of one retained Participant Console session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantConsoleSessionHealthV1 {
    Usable,
    Missing,
    Stale,
    Invalid,
    Disconnected,
}

/// Read-only readiness seam for one exact Room and Membership.
pub trait ParticipantConsoleReadinessSourceV1: Send + Sync {
    /// Returns one closed health reason without disclosing retained authority.
    fn session_health(&self, room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1;
}

/// Closed upstream failures; no variant retains daemon details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantConsoleGatewayErrorV1 {
    Disconnected,
    Rejected,
    Unavailable,
}

/// Internal observation result whose durable Cursor never crosses the browser boundary.
pub struct ParticipantConsoleObservationV1 {
    /// The only observation data eligible for browser-safe response encoding.
    pub browser_value: Value,
    /// The daemon-reported Membership Cursor retained by the local broker only.
    pub durable_cursor: Option<u64>,
}

/// Current enabled Membership facts returned by the Room service.
///
/// This snapshot carries no capability or routing secret. The broker uses it
/// to select and continuously revalidate the exact Activity Client binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentMembershipSnapshotV1 {
    pub pack: PackReference,
    pub access_mode: AccessMode,
    pub role: Option<String>,
}

/// Membership-bound observe/act adapter used after the local session is admitted.
pub trait ParticipantConsoleGatewayV1: Send + Sync + 'static {
    /// Reads current Membership authority without attaching a stream or
    /// requiring its Cursor. Hosted sessions use this independent read.
    ///
    /// # Errors
    /// Fails closed unless the adapter implements a cursor-independent read.
    fn membership_status(
        &self,
        _authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    /// Resolves current enabled Membership facts from the Room service.
    ///
    /// # Errors
    ///
    /// Returns `Rejected` when the Membership is absent, disabled, or no
    /// longer usable by this human authority.
    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1>;

    /// Returns browser-safe authorized observation data for this Membership only.
    ///
    /// # Errors
    ///
    /// Returns a closed connectivity, rejection, or availability failure.
    fn observe(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1>;

    /// Submits an exact Action through this Membership's participant authority only.
    ///
    /// # Errors
    ///
    /// Returns a closed connectivity, rejection, or availability failure.
    fn act(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
        request: &ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1>;

    /// Replays one historical Head through the exact existing Membership
    /// authority. The browser-facing result must contain no routing data.
    ///
    /// # Errors
    ///
    /// Returns a closed connectivity, authorization, or availability failure.
    fn replay(
        &self,
        _authority: &HumanSeatAuthorityV1,
        _at_room_seq: u64,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    /// Reports whether the exact Membership session can currently reconnect.
    ///
    /// # Errors
    ///
    /// Returns a closed connectivity, rejection, or availability failure.
    fn health(
        &self,
        _authority: &HumanSeatAuthorityV1,
        _durable_cursor: Option<u64>,
    ) -> Result<ParticipantConsoleSessionHealthV1, ParticipantConsoleGatewayErrorV1> {
        Ok(ParticipantConsoleSessionHealthV1::Usable)
    }

    /// Issues one Runtime-owned target-bound ticket after the hosted broker
    /// has revalidated the Browser Activity Session and exact client release.
    ///
    /// # Errors
    /// Returns a closed transport, authorization, or availability failure.
    fn issue_hosted_browser_stream_ticket(
        &self,
        _authority: &HumanSeatAuthorityV1,
        _request: &HostedBrowserWebSocketTicketIssueRequest,
        _origin: &str,
    ) -> Result<BrowserWebSocketTicketIssueResponse, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    /// Retires all pending admission tickets and the active stream for one
    /// Browser Activity Session digest.
    ///
    /// # Errors
    /// Returns a closed transport, authorization, or availability failure.
    fn revoke_hosted_browser_stream_session(
        &self,
        _authority: &HumanSeatAuthorityV1,
        _request: &HostedBrowserWebSocketSessionRevokeRequest,
    ) -> Result<(), ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }
}

/// Fixed-address participant protocol gateway; exact routing and authority stay server-side.
#[derive(Clone, Copy, Debug)]
pub struct FixedDaemonParticipantConsoleGatewayV1 {
    address: SocketAddr,
    timeout: Duration,
}

impl FixedDaemonParticipantConsoleGatewayV1 {
    #[must_use]
    pub const fn new(address: SocketAddr, timeout: Duration) -> Self {
        Self { address, timeout }
    }

    fn connect(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<WebSocket<TcpStream>, ParticipantConsoleGatewayErrorV1> {
        let stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        let mut authorization = Zeroizing::new(Vec::with_capacity(
            "Bearer ".len() + authority.bearer().as_str().len(),
        ));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(authority.bearer().as_str().as_bytes());
        let authorization = HeaderValue::from_maybe_shared(Bytes::from_owner(authorization))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("ws://{}/v1/stream", self.address))
            .header("Host", self.address.to_string())
            .header("Authorization", authorization)
            .header("Sec-WebSocket-Protocol", WEBSOCKET_SUBPROTOCOL)
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        let (mut socket, response) = client(request, stream).map_err(handshake_error)?;
        if response.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        Self::send(
            &mut socket,
            "client.hello",
            &ClientHello {
                client_name: "worldstream-participant-console".to_owned(),
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                mode: match authority.access_mode() {
                    AccessMode::Participant => ClientMode::Participant,
                    AccessMode::Spectator => ClientMode::Spectator,
                    AccessMode::Operator => ClientMode::Operator,
                },
                supported_protocols: vec![PROTOCOL_VERSION.to_owned()],
                capabilities: REQUIRED_CLIENT_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
        )?;
        Self::read_type(&mut socket, "server.welcome")?;
        Ok(socket)
    }

    // Observation acknowledgements persist a Membership Cursor. An authority check
    // must not attempt a new attach with an invented (or absent) Cursor.
    fn read_membership_status(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        let request = Zeroizing::new(format!(
            "GET /v1/rooms/{}/members/{}/status HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            authority.room_id(),
            authority.member_id(),
            self.address,
            authority.bearer().as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .and_then(|()| stream.write_all(request.as_bytes()))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        let mut response = Vec::new();
        stream
            .take(MAX_TICKET_RESPONSE_BYTES + 1)
            .read_to_end(&mut response)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        if u64::try_from(response.len()).unwrap_or(u64::MAX) > MAX_TICKET_RESPONSE_BYTES {
            return Err(ParticipantConsoleGatewayErrorV1::Unavailable);
        }
        let (status, body) = parse_http_response(&response)?;
        match status {
            200 => {}
            400 | 401 | 403 | 404 | 422 => return Err(ParticipantConsoleGatewayErrorV1::Rejected),
            _ => return Err(ParticipantConsoleGatewayErrorV1::Unavailable),
        }
        let current: worldstream_protocol::MembershipStatusResponse = serde_json::from_slice(body)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        if current.version != "membership_status.v1"
            || current.room_id != authority.room_id()
            || current.member_id != authority.member_id()
            || current.principal_kind != authority.principal_kind()
            || current.membership_status != "enabled"
            || !matches!(
                current.access_mode,
                AccessMode::Participant | AccessMode::Spectator
            )
            || (current.access_mode == AccessMode::Participant) != current.role.is_some()
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        Ok(CurrentMembershipSnapshotV1 {
            pack: current.pack,
            access_mode: current.access_mode,
            role: current.role,
        })
    }

    fn authenticated_json_call<T: Serialize>(
        &self,
        authority: &HumanSeatAuthorityV1,
        path: &'static str,
        origin: Option<&str>,
        body: &T,
    ) -> Result<(u16, Vec<u8>), ParticipantConsoleGatewayErrorV1> {
        let encoded =
            serde_json::to_vec(body).map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        let canonical = CanonicalJsonV1::parse(&encoded)
            .and_then(|value| value.to_bytes())
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        let origin_header = origin.map_or_else(String::new, |value| format!("Origin: {value}\r\n"));
        let request = Zeroizing::new(format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\n{origin_header}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.address,
            authority.bearer().as_str(),
            canonical.len(),
            std::str::from_utf8(&canonical)
                .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?,
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        let mut response = Vec::new();
        stream
            .take(MAX_TICKET_RESPONSE_BYTES + 1)
            .read_to_end(&mut response)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        if u64::try_from(response.len()).unwrap_or(u64::MAX) > MAX_TICKET_RESPONSE_BYTES {
            return Err(ParticipantConsoleGatewayErrorV1::Unavailable);
        }
        let (status, body) = parse_http_response(&response)?;
        Ok((status, body.to_vec()))
    }

    fn attach(
        socket: &mut WebSocket<TcpStream>,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<RoomAttached, ParticipantConsoleGatewayErrorV1> {
        Self::send(
            socket,
            "room.attach",
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "after_frame_seq": durable_cursor,
            }),
        )?;
        let attached: RoomAttached =
            serde_json::from_value(Self::read_type(socket, "room.attached")?)
                .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        if attached.room_id != authority.room_id()
            || attached.member_id != authority.member_id()
            || attached.room_head.room_id != authority.room_id()
            || attached.principal_kind != authority.principal_kind()
            || (attached.access_mode == AccessMode::Participant) != attached.role.is_some()
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        Ok(attached)
    }

    fn read_delivery(
        socket: &mut WebSocket<TcpStream>,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<Vec<Value>, ParticipantConsoleGatewayErrorV1> {
        let mut delivery = Vec::new();
        while let Ok(value) = Self::read_any(socket) {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = value.get("body").cloned().unwrap_or(Value::Null);
            match kind {
                "projection.reset" => {
                    let reset: ProjectionReset = serde_json::from_value(body)
                        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
                    if reset.room_id != authority.room_id()
                        || reset.member_id != authority.member_id()
                        || reset.room_head.room_id != authority.room_id()
                    {
                        return Err(ParticipantConsoleGatewayErrorV1::Rejected);
                    }
                    delivery.push(browser_delivery("projection_reset", reset)?);
                }
                "observation.deliver" => {
                    let observation: ObservationDeliver = serde_json::from_value(body)
                        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
                    if observation.room_id != authority.room_id()
                        || observation.member_id != authority.member_id()
                    {
                        return Err(ParticipantConsoleGatewayErrorV1::Rejected);
                    }
                    delivery.push(browser_delivery("observation", observation)?);
                }
                _ => break,
            }
        }
        Ok(delivery)
    }

    fn send(
        socket: &mut WebSocket<TcpStream>,
        message_type: &str,
        body: &impl Serialize,
    ) -> Result<(), ParticipantConsoleGatewayErrorV1> {
        let envelope = VersionedEnvelope {
            protocol: PROTOCOL_VERSION.to_owned(),
            message_type: message_type.to_owned(),
            message_id: next_protocol_ulid()?,
            request_id: None,
            body,
        };
        let text = serde_json::to_string(&envelope)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        socket
            .send(Message::Text(text.into()))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)
    }

    fn read_any(
        socket: &mut WebSocket<TcpStream>,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        match socket.read() {
            Ok(Message::Text(text)) if text.len() <= MAX_ACTION_PAYLOAD_BYTES => {
                serde_json::from_str(text.as_str())
                    .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)
            }
            Ok(Message::Ping(value)) => {
                socket
                    .send(Message::Pong(value))
                    .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
                Self::read_any(socket)
            }
            _ => Err(ParticipantConsoleGatewayErrorV1::Disconnected),
        }
    }

    fn read_type(
        socket: &mut WebSocket<TcpStream>,
        expected: &str,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        for _ in 0..8 {
            let value = Self::read_any(socket)?;
            if value.get("type").and_then(Value::as_str) == Some(expected) {
                return Ok(value.get("body").cloned().unwrap_or(Value::Null));
            }
            if value.get("type").and_then(Value::as_str) == Some("error") {
                return Err(ParticipantConsoleGatewayErrorV1::Rejected);
            }
        }
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    fn read_action_receipt(
        socket: &mut WebSocket<TcpStream>,
        authority: &HumanSeatAuthorityV1,
        expected_action_id: &str,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        for _ in 0..8 {
            let value = Self::read_any(socket)?;
            let message_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = value.get("body").cloned().unwrap_or(Value::Null);
            match message_type {
                "action.accepted" => {
                    let receipt: ActionAccepted = serde_json::from_value(body)
                        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
                    if receipt.room_id != authority.room_id()
                        || receipt.member_id != authority.member_id()
                        || receipt.action_id != expected_action_id
                        || receipt.room_head.room_id != authority.room_id()
                    {
                        return Err(ParticipantConsoleGatewayErrorV1::Rejected);
                    }
                    return browser_action_receipt("accepted", receipt);
                }
                "action.rejected" => {
                    let receipt: ActionRejected = serde_json::from_value(body)
                        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
                    if receipt.room_id != authority.room_id()
                        || receipt.member_id != authority.member_id()
                        || receipt.action_id != expected_action_id
                    {
                        return Err(ParticipantConsoleGatewayErrorV1::Rejected);
                    }
                    return browser_action_receipt("rejected", receipt);
                }
                "error" => return Err(ParticipantConsoleGatewayErrorV1::Rejected),
                _ => {}
            }
        }
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }
}

impl ParticipantConsoleGatewayV1 for FixedDaemonParticipantConsoleGatewayV1 {
    fn membership_status(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        self.read_membership_status(authority)
    }

    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        let mut socket = self.connect(authority)?;
        let attached = Self::attach(&mut socket, authority, durable_cursor)?;
        if attached.membership_status != "enabled"
            || !matches!(
                attached.access_mode,
                AccessMode::Participant | AccessMode::Spectator
            )
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        Ok(CurrentMembershipSnapshotV1 {
            pack: attached.pack,
            access_mode: attached.access_mode,
            role: attached.role,
        })
    }

    fn observe(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        let mut socket = self.connect(authority)?;
        let attached = Self::attach(&mut socket, authority, durable_cursor)?;
        if attached.membership_status != "enabled"
            || attached.access_mode != authority.access_mode()
            || attached.role.as_deref() != authority.role()
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        let delivery = Self::read_delivery(&mut socket, authority)?;
        Ok(ParticipantConsoleObservationV1 {
            browser_value: serde_json::json!({
                "pack": {
                    "id": attached.pack.id,
                    "version": attached.pack.version,
                    "digest": attached.pack.digest,
                },
                "room_head": browser_room_head(&attached.room_head),
                "frame_head": attached.frame_head,
                "delivery": delivery,
            }),
            durable_cursor: attached.cursor,
        })
    }

    fn act(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
        request: &ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        let mut socket = self.connect(authority)?;
        let attached = Self::attach(&mut socket, authority, durable_cursor)?;
        if attached.membership_status != "enabled"
            || attached.access_mode != AccessMode::Participant
            || attached.access_mode != authority.access_mode()
            || attached.role.as_deref() != authority.role()
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        Self::send(
            &mut socket,
            "room.sync_ack",
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "through_frame_head": attached.frame_head,
                "sync_token": attached.sync_token,
            }),
        )?;
        Self::read_type(&mut socket, "room.sync_acked")?;
        Self::send(
            &mut socket,
            "action.submit",
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "action_id": request.action_id,
                "based_on_room_seq": request.based_on_room_seq,
                "action_type": request.action_type,
                "payload": request.payload,
            }),
        )?;
        Self::read_action_receipt(&mut socket, authority, &request.action_id)
    }

    fn replay(
        &self,
        authority: &HumanSeatAuthorityV1,
        at_room_seq: u64,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        let request = Zeroizing::new(format!(
            "GET /v1/rooms/{}/replay?at_room_seq={at_room_seq} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            authority.room_id(),
            self.address,
            authority.bearer().as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        let mut response = Vec::new();
        stream
            .take(MAX_REPLAY_RESPONSE_BYTES + 1)
            .read_to_end(&mut response)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Disconnected)?;
        if u64::try_from(response.len()).unwrap_or(u64::MAX) > MAX_REPLAY_RESPONSE_BYTES {
            return Err(ParticipantConsoleGatewayErrorV1::Unavailable);
        }
        let (status, body) = parse_http_response(&response)?;
        match status {
            200 => {
                let replay: ReplayResponse = serde_json::from_slice(body)
                    .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
                if replay.room_id != authority.room_id()
                    || replay.requested_room_seq != at_room_seq
                    || replay.room_head.room_id != authority.room_id()
                    || replay.room_head.room_seq != at_room_seq
                    || replay.verification != "verified"
                {
                    return Err(ParticipantConsoleGatewayErrorV1::Rejected);
                }
                browser_replay(replay)
            }
            400 | 401 | 403 | 404 | 409 | 422 => Err(ParticipantConsoleGatewayErrorV1::Rejected),
            _ => Err(ParticipantConsoleGatewayErrorV1::Unavailable),
        }
    }

    fn health(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<ParticipantConsoleSessionHealthV1, ParticipantConsoleGatewayErrorV1> {
        let mut socket = self.connect(authority)?;
        let attached = Self::attach(&mut socket, authority, durable_cursor)?;
        match attached.membership_status.as_str() {
            "enabled"
                if attached.access_mode == authority.access_mode()
                    && attached.role.as_deref() == authority.role() =>
            {
                Ok(ParticipantConsoleSessionHealthV1::Usable)
            }
            "enabled" | "suspended" | "departed" => Ok(ParticipantConsoleSessionHealthV1::Invalid),
            _ => Err(ParticipantConsoleGatewayErrorV1::Rejected),
        }
    }

    fn issue_hosted_browser_stream_ticket(
        &self,
        authority: &HumanSeatAuthorityV1,
        request: &HostedBrowserWebSocketTicketIssueRequest,
        origin: &str,
    ) -> Result<BrowserWebSocketTicketIssueResponse, ParticipantConsoleGatewayErrorV1> {
        if request.version != HOSTED_BROWSER_WS_TICKET_VERSION
            || request.room_id != authority.room_id()
            || request.member_id != authority.member_id()
        {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        let (status, body) = self.authenticated_json_call(
            authority,
            "/v1/hosted/browser-stream-ticket",
            Some(origin),
            request,
        )?;
        if status != 201 {
            return Err(if matches!(status, 400 | 401 | 403 | 404 | 409 | 422) {
                ParticipantConsoleGatewayErrorV1::Rejected
            } else {
                ParticipantConsoleGatewayErrorV1::Unavailable
            });
        }
        let response: BrowserWebSocketTicketIssueResponse = serde_json::from_slice(&body)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        if response.version != BROWSER_WS_TICKET_VERSION
            || response.expires_in_ms == 0
            || response.expires_in_ms > 15_000
            || !is_token(&response.ticket, "wst1:")
        {
            return Err(ParticipantConsoleGatewayErrorV1::Unavailable);
        }
        Ok(response)
    }

    fn revoke_hosted_browser_stream_session(
        &self,
        authority: &HumanSeatAuthorityV1,
        request: &HostedBrowserWebSocketSessionRevokeRequest,
    ) -> Result<(), ParticipantConsoleGatewayErrorV1> {
        if request.version != HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION {
            return Err(ParticipantConsoleGatewayErrorV1::Rejected);
        }
        let (status, body) = self.authenticated_json_call(
            authority,
            "/v1/hosted/browser-stream-session:revoke",
            None,
            request,
        )?;
        if status != 200 {
            return Err(if matches!(status, 400 | 401 | 403 | 404 | 409 | 422) {
                ParticipantConsoleGatewayErrorV1::Rejected
            } else {
                ParticipantConsoleGatewayErrorV1::Unavailable
            });
        }
        let value: Value = serde_json::from_slice(&body)
            .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
        if value
            != serde_json::json!({
                "revoked": true,
                "version": HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION,
            })
        {
            return Err(ParticipantConsoleGatewayErrorV1::Unavailable);
        }
        Ok(())
    }
}

fn handshake_error(
    error: tungstenite::HandshakeError<tungstenite::handshake::client::ClientHandshake<TcpStream>>,
) -> ParticipantConsoleGatewayErrorV1 {
    match error {
        // A short daemon timeout and an interrupted TCP handshake are
        // reconnectable transport conditions. They are not evidence that the
        // retained Membership authority was revoked.
        tungstenite::HandshakeError::Interrupted(_)
        | tungstenite::HandshakeError::Failure(tungstenite::Error::Io(_)) => {
            ParticipantConsoleGatewayErrorV1::Disconnected
        }
        // Only explicit authentication or authorization statuses can
        // invalidate a participant session. Route and protocol failures stay
        // bounded upstream failures rather than being shown as revoked access.
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))
            if handshake_http_status_error(response.status()) =>
        {
            ParticipantConsoleGatewayErrorV1::Rejected
        }
        tungstenite::HandshakeError::Failure(_) => ParticipantConsoleGatewayErrorV1::Unavailable,
    }
}

fn handshake_http_status_error(status: http::StatusCode) -> bool {
    matches!(
        status,
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    )
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct BrowserRoomHeadV1<'a> {
    room_seq: u64,
    genesis_or_transition_hash: &'a str,
    core_schema_version: &'a str,
    pack_digest: &'a str,
    core_state_hash: &'a str,
    activity_state_hash: &'a str,
    authoritative_state_hash: &'a str,
}

fn browser_room_head(head: &RoomHead) -> BrowserRoomHeadV1<'_> {
    BrowserRoomHeadV1 {
        room_seq: head.room_seq,
        genesis_or_transition_hash: &head.genesis_or_transition_hash,
        core_schema_version: &head.core_schema_version,
        pack_digest: &head.pack_digest,
        core_state_hash: &head.core_state_hash,
        activity_state_hash: &head.activity_state_hash,
        authoritative_state_hash: &head.authoritative_state_hash,
    }
}

fn browser_delivery(
    kind: &'static str,
    body: impl Serialize,
) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
    let body =
        serde_json::to_value(body).map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
    Ok(serde_json::json!({"kind": kind, "body": strip_routing(body)}))
}

fn browser_replay(replay: ReplayResponse) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
    let projection = serde_json::to_value(replay.projection)
        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
    Ok(serde_json::json!({
        "requested_room_seq": replay.requested_room_seq,
        "room_head": browser_room_head(&replay.room_head),
        "projection": strip_routing(projection),
        "projection_hash": replay.projection_hash,
        "verification": "verified",
        "room_health": replay.room_health,
        "integrity_generation": replay.integrity_generation,
    }))
}

fn browser_action_receipt(
    state: &'static str,
    receipt: impl Serialize,
) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
    let receipt =
        serde_json::to_value(receipt).map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
    Ok(serde_json::json!({
        "state": state,
        "receipt": strip_routing(receipt),
    }))
}

fn strip_routing(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            for key in ["room_id", "member_id", "membership_id"] {
                object.remove(key);
            }
            for child in object.values_mut() {
                *child = strip_routing(std::mem::take(child));
            }
        }
        Value::Array(values) => {
            for child in values {
                *child = strip_routing(std::mem::take(child));
            }
        }
        _ => {}
    }
    value
}

fn next_protocol_ulid() -> Result<worldstream_protocol::UlidString, ParticipantConsoleGatewayErrorV1>
{
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    std::str::from_utf8(&encoded)
        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?
        .parse()
        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)
}

#[derive(Clone)]
struct SeatBindingV1 {
    draft_id: String,
    seat_id: String,
}

struct HandoffRecordV1 {
    binding: SeatBindingV1,
    selection: RetainedClientSelectionV1,
    expires_at: Instant,
}

#[derive(Clone)]
enum RetainedClientSelectionV1 {
    Candidate(String),
    Inspector(String),
}

fn client_selection_request(current: &CurrentMembershipSnapshotV1) -> ClientSelectionRequestV1 {
    ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: current.pack.id.clone(),
            version: current.pack.version.clone(),
            digest: current.pack.digest.clone(),
        },
        client_contract: CLIENT_CONTRACT_V1.to_owned(),
        access_mode: current.access_mode,
        role: current.role.clone(),
    }
}

#[derive(Clone)]
struct SessionRecordV1 {
    binding: SeatBindingV1,
    selection: RetainedClientSelectionV1,
    target_fingerprint: [u8; 32],
    durable_cursor: Option<u64>,
    expires_at: Instant,
    pending_delivery: Option<(String, u64, Instant)>,
    acknowledged_at: Option<Instant>,
}

struct ResolvedParticipantSessionV1 {
    authority: HumanSeatAuthorityV1,
    current: CurrentMembershipSnapshotV1,
    durable_cursor: Option<u64>,
}

#[derive(Default)]
struct BrokerStateV1 {
    handoffs: HashMap<String, HandoffRecordV1>,
    sessions: HashMap<String, SessionRecordV1>,
}

struct BrokerInnerV1 {
    studio_origin: String,
    console_origin: String,
    handoff_ttl: Duration,
    maximum_retained: usize,
    readiness_key: [u8; 32],
    authority: Arc<dyn ParticipantHandoffAuthoritySourceV1>,
    gateway: Arc<dyn ParticipantConsoleGatewayV1>,
    clients: Arc<dyn ClientSelectionSourceV1>,
    state: Mutex<BrokerStateV1>,
}

/// Bounded local broker for one-use handoffs and retained opaque sessions.
#[derive(Clone)]
pub struct ParticipantHandoffBrokerV1 {
    inner: Arc<BrokerInnerV1>,
}

impl ParticipantHandoffBrokerV1 {
    /// Constructs a broker bound to fixed loopback Studio and Console origins.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback origins, invalid limits, or unbounded expiry.
    pub fn new(
        studio_origin: &str,
        console_origin: &str,
        handoff_ttl: Duration,
        maximum_retained: usize,
        authority: impl ParticipantHandoffAuthoritySourceV1,
        gateway: impl ParticipantConsoleGatewayV1,
        clients: impl ClientSelectionSourceV1,
    ) -> Result<Self, ParticipantHandoffConfigErrorV1> {
        if !is_exact_loopback_origin(studio_origin)
            || !is_exact_loopback_origin(console_origin)
            || studio_origin == console_origin
            || handoff_ttl.is_zero()
            || handoff_ttl > Duration::from_mins(5)
            || maximum_retained == 0
            || maximum_retained > 1024
        {
            return Err(ParticipantHandoffConfigErrorV1::Invalid);
        }
        let mut readiness_key = [0_u8; 32];
        getrandom::fill(&mut readiness_key)
            .map_err(|_| ParticipantHandoffConfigErrorV1::Unavailable)?;
        Ok(Self {
            inner: Arc::new(BrokerInnerV1 {
                studio_origin: studio_origin.to_owned(),
                console_origin: console_origin.to_owned(),
                handoff_ttl,
                maximum_retained,
                readiness_key,
                authority: Arc::new(authority),
                gateway: Arc::new(gateway),
                clients: Arc::new(clients),
                state: Mutex::new(BrokerStateV1::default()),
            }),
        })
    }

    fn issue(
        &self,
        request: IssueHandoffRequestV1,
    ) -> Result<ClientHandoffResponseV1, ParticipantHandoffErrorV1> {
        validate_binding(&request.draft, &request.seat)?;
        if request
            .candidate
            .as_deref()
            .is_some_and(|candidate| !is_client_candidate_id(candidate))
        {
            return Err(ParticipantHandoffErrorV1::InvalidRequest);
        }
        let authority = self
            .inner
            .authority
            .resolve_provisioned_human_seat(&request.draft, &request.seat)
            .map_err(ParticipantHandoffErrorV1::from_authority)?;
        let current = self.current_membership(&authority, None)?;
        let selection = self.select_client(&current, request.candidate.as_deref())?;
        let (launch_base, retained_selection) = match selection {
            ClientSelectionV1::SelectionRequired { candidates } => {
                return Ok(ClientHandoffResponseV1::SelectionRequired {
                    version: HANDOFF_VERSION.to_owned(),
                    candidates: candidates
                        .into_iter()
                        .map(ClientCandidateSummaryV1::from)
                        .collect(),
                });
            }
            ClientSelectionV1::InspectorFallback { candidate } => {
                let launch_base =
                    selected_launch_base(&self.inner.console_origin, &candidate.launch_url)?;
                (
                    launch_base,
                    RetainedClientSelectionV1::Inspector(candidate.candidate_id),
                )
            }
            ClientSelectionV1::Selected { candidate } => {
                let launch_base =
                    selected_launch_base(&self.inner.console_origin, &candidate.launch_url)?;
                (
                    launch_base,
                    RetainedClientSelectionV1::Candidate(candidate.candidate_id),
                )
            }
        };
        let mut state = self.lock();
        prune_expired(&mut state);
        if state.handoffs.len().saturating_add(state.sessions.len()) >= self.inner.maximum_retained
        {
            return Err(ParticipantHandoffErrorV1::Capacity);
        }
        let handoff = unique_token("wsh1:", &state.handoffs)?;
        state.handoffs.insert(
            handoff.clone(),
            HandoffRecordV1 {
                binding: SeatBindingV1 {
                    draft_id: request.draft,
                    seat_id: request.seat,
                },
                selection: retained_selection,
                expires_at: Instant::now() + self.inner.handoff_ttl,
            },
        );
        Ok(ClientHandoffResponseV1::Ready {
            version: HANDOFF_VERSION.to_owned(),
            client_url: format!("{launch_base}#handoff={handoff}"),
        })
    }

    fn select_client(
        &self,
        current: &CurrentMembershipSnapshotV1,
        preferred_candidate_id: Option<&str>,
    ) -> Result<ClientSelectionV1, ParticipantHandoffErrorV1> {
        self.inner
            .clients
            .select_client(&client_selection_request(current), preferred_candidate_id)
            .map_err(|error| match error {
                ClientBindingStoreErrorV1::InvalidChoice => {
                    ParticipantHandoffErrorV1::SelectionInvalid
                }
                ClientBindingStoreErrorV1::Invalid | ClientBindingStoreErrorV1::Unavailable => {
                    ParticipantHandoffErrorV1::ClientUnavailable
                }
            })
    }

    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        durable_cursor: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantHandoffErrorV1> {
        self.inner
            .gateway
            .current_membership(authority, durable_cursor)
            .map_err(ParticipantHandoffErrorV1::from_gateway)
    }

    fn redeem(
        &self,
        handoff: &str,
        retained_cookie: Option<&str>,
    ) -> Result<(ParticipantSessionStatusV1, String), ParticipantHandoffErrorV1> {
        if !is_token(handoff, "wsh1:") {
            return Err(ParticipantHandoffErrorV1::InvalidHandoff);
        }
        let record = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .handoffs
                .remove(handoff)
                .ok_or(ParticipantHandoffErrorV1::InvalidHandoff)?
        };
        let authority = self
            .inner
            .authority
            .resolve_provisioned_human_seat(&record.binding.draft_id, &record.binding.seat_id)
            .map_err(ParticipantHandoffErrorV1::from_authority)?;
        let current = self.current_membership(&authority, None)?;
        match &record.selection {
            RetainedClientSelectionV1::Candidate(candidate_id) => {
                let ClientSelectionV1::Selected { candidate } =
                    self.select_client(&current, Some(candidate_id))?
                else {
                    return Err(ParticipantHandoffErrorV1::InvalidHandoff);
                };
                if candidate.candidate_id != *candidate_id {
                    return Err(ParticipantHandoffErrorV1::InvalidHandoff);
                }
            }
            RetainedClientSelectionV1::Inspector(candidate_id) => {
                let ClientSelectionV1::InspectorFallback { candidate } =
                    self.select_client(&current, None)?
                else {
                    return Err(ParticipantHandoffErrorV1::InvalidHandoff);
                };
                if candidate.candidate_id != *candidate_id {
                    return Err(ParticipantHandoffErrorV1::InvalidHandoff);
                }
            }
        }
        let mut state = self.lock();
        prune_expired(&mut state);
        if let Some(retained) = retained_cookie.and_then(parse_session_cookie) {
            state.sessions.remove(retained);
        }
        if state.handoffs.len().saturating_add(state.sessions.len()) >= self.inner.maximum_retained
        {
            return Err(ParticipantHandoffErrorV1::Capacity);
        }
        let session = unique_token("wss1:", &state.sessions)?;
        state.sessions.insert(
            session.clone(),
            SessionRecordV1 {
                binding: record.binding,
                selection: record.selection,
                target_fingerprint: target_fingerprint(
                    &self.inner.readiness_key,
                    authority.room_id(),
                    authority.member_id(),
                ),
                durable_cursor: None,
                expires_at: Instant::now() + Duration::from_secs(SESSION_MAX_AGE_SECONDS),
                pending_delivery: None,
                acknowledged_at: None,
            },
        );
        Ok((ParticipantSessionStatusV1::usable(), session))
    }

    fn session_status(
        &self,
        cookie: Option<&str>,
    ) -> Result<ParticipantSessionStatusV1, ParticipantHandoffErrorV1> {
        let resolved = self.resolve_session(cookie)?;
        let result = match self
            .inner
            .gateway
            .health(&resolved.authority, resolved.durable_cursor)
        {
            Ok(ParticipantConsoleSessionHealthV1::Usable) => {
                Ok(ParticipantSessionStatusV1::usable())
            }
            Ok(
                ParticipantConsoleSessionHealthV1::Disconnected
                | ParticipantConsoleSessionHealthV1::Stale,
            )
            | Err(ParticipantConsoleGatewayErrorV1::Disconnected) => {
                Ok(ParticipantSessionStatusV1::disconnected())
            }
            Ok(
                ParticipantConsoleSessionHealthV1::Missing
                | ParticipantConsoleSessionHealthV1::Invalid,
            ) => Err(ParticipantHandoffErrorV1::AuthorityInvalid),
            Err(ParticipantConsoleGatewayErrorV1::Rejected) => {
                Err(ParticipantHandoffErrorV1::AuthorityInvalid)
            }
            Err(ParticipantConsoleGatewayErrorV1::Unavailable) => {
                Err(ParticipantHandoffErrorV1::Upstream)
            }
        };
        self.invalidate_cookie_on_terminal_error(cookie, result)
    }

    fn observe(
        &self,
        cookie: Option<&str>,
        request: ObserveRequestV1,
    ) -> Result<Value, ParticipantHandoffErrorV1> {
        let resolved = self.resolve_session(cookie)?;
        let observation = self.invalidate_cookie_on_terminal_error(
            cookie,
            self.inner
                .gateway
                .observe(&resolved.authority, resolved.durable_cursor)
                .map_err(ParticipantHandoffErrorV1::from_gateway),
        )?;
        self.update_session_cursor(cookie, observation.durable_cursor)?;
        // A browser frame head records what this page rendered. The protected Console
        // does not issue `observation.ack`, so it cannot become a Membership Cursor.
        let _ = request.after_frame_seq;
        browser_safe_gateway_value(observation.browser_value)
    }

    fn act(
        &self,
        cookie: Option<&str>,
        request: &ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantHandoffErrorV1> {
        validate_action(request)?;
        let resolved = self.resolve_session(cookie)?;
        if resolved.current.access_mode != AccessMode::Participant {
            return Err(ParticipantHandoffErrorV1::AuthorityInvalid);
        }
        let value = self.invalidate_cookie_on_terminal_error(
            cookie,
            self.inner
                .gateway
                .act(&resolved.authority, resolved.durable_cursor, request)
                .map_err(ParticipantHandoffErrorV1::from_gateway),
        )?;
        browser_safe_gateway_value(value)
    }

    fn replay(
        &self,
        cookie: Option<&str>,
        request: ReplayRequestV1,
    ) -> Result<Value, ParticipantHandoffErrorV1> {
        let resolved = self.resolve_session(cookie)?;
        let value = self
            .inner
            .gateway
            .replay(&resolved.authority, request.at_room_seq)
            .map_err(|error| match error {
                ParticipantConsoleGatewayErrorV1::Rejected => {
                    ParticipantHandoffErrorV1::ReplayUnavailable
                }
                other => ParticipantHandoffErrorV1::from_gateway(other),
            })?;
        browser_safe_gateway_value(value)
    }

    fn resolve_session(
        &self,
        cookie: Option<&str>,
    ) -> Result<ResolvedParticipantSessionV1, ParticipantHandoffErrorV1> {
        let token = cookie
            .and_then(parse_session_cookie)
            .filter(|value| is_token(value, "wss1:"))
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        let record = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .sessions
                .get(token)
                .cloned()
                .ok_or(ParticipantHandoffErrorV1::SessionMissing)?
        };
        let resolved: Result<ResolvedParticipantSessionV1, ParticipantHandoffErrorV1> = (|| {
            let mut authority = self
                .inner
                .authority
                .resolve_provisioned_human_seat(&record.binding.draft_id, &record.binding.seat_id)
                .map_err(ParticipantHandoffErrorV1::from_authority)?;
            let current = self.current_membership(&authority, record.durable_cursor)?;
            self.validate_retained_selection(&current, &record.selection)?;
            authority.install_current_membership(&current);
            Ok(ResolvedParticipantSessionV1 {
                authority,
                current,
                durable_cursor: record.durable_cursor,
            })
        })(
        );
        if resolved
            .as_ref()
            .is_err_and(|error| error.invalidates_retained_session())
        {
            self.invalidate_session(token);
        }
        resolved
    }

    fn validate_retained_selection(
        &self,
        current: &CurrentMembershipSnapshotV1,
        retained: &RetainedClientSelectionV1,
    ) -> Result<(), ParticipantHandoffErrorV1> {
        let (class, candidate_id) = match retained {
            RetainedClientSelectionV1::Candidate(candidate_id) => {
                (ClientCandidateClassV1::Binding, candidate_id)
            }
            RetainedClientSelectionV1::Inspector(candidate_id) => {
                (ClientCandidateClassV1::InspectorFallback, candidate_id)
            }
        };
        self.inner
            .clients
            .resolve_active_client(&client_selection_request(current), class, candidate_id)
            .map(|_| ())
            .map_err(|error| match error {
                ClientBindingStoreErrorV1::InvalidChoice => {
                    ParticipantHandoffErrorV1::AuthorityInvalid
                }
                ClientBindingStoreErrorV1::Invalid | ClientBindingStoreErrorV1::Unavailable => {
                    ParticipantHandoffErrorV1::ClientUnavailable
                }
            })
    }

    fn update_session_cursor(
        &self,
        cookie: Option<&str>,
        cursor: Option<u64>,
    ) -> Result<(), ParticipantHandoffErrorV1> {
        let token = cookie
            .and_then(parse_session_cookie)
            .filter(|value| is_token(value, "wss1:"))
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        let mut state = self.lock();
        prune_expired(&mut state);
        let record = state
            .sessions
            .get_mut(token)
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        record.durable_cursor = cursor;
        Ok(())
    }

    fn invalidate_cookie_on_terminal_error<T>(
        &self,
        cookie: Option<&str>,
        result: Result<T, ParticipantHandoffErrorV1>,
    ) -> Result<T, ParticipantHandoffErrorV1> {
        if result
            .as_ref()
            .is_err_and(|error| error.invalidates_retained_session())
            && let Some(token) = cookie.and_then(parse_session_cookie)
        {
            self.invalidate_session(token);
        }
        result
    }

    fn invalidate_session(&self, token: &str) {
        self.lock().sessions.remove(token);
    }

    fn lock(&self) -> MutexGuard<'_, BrokerStateV1> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Short-lived evidence of actual browser receipt, separate from legacy health
/// probes and from the canonical Membership Cursor.
#[derive(Clone)]
pub struct BrowserDeliveryReadinessV1(ParticipantHandoffBrokerV1);

const BROWSER_RECEIPT_LEASE: Duration = Duration::from_secs(5);

impl ParticipantHandoffBrokerV1 {
    #[must_use]
    pub fn browser_readiness(&self) -> BrowserDeliveryReadinessV1 {
        BrowserDeliveryReadinessV1(self.clone())
    }

    fn delivery_receipt(
        &self,
        cookie: Option<&str>,
        frame_head: u64,
    ) -> Result<String, ParticipantHandoffErrorV1> {
        let token = cookie
            .and_then(parse_session_cookie)
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        let acknowledgement = random_token("wsa1:")?;
        let mut state = self.lock();
        prune_expired(&mut state);
        let record = state
            .sessions
            .get_mut(token)
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        record.pending_delivery = Some((
            acknowledgement.clone(),
            frame_head,
            Instant::now() + BROWSER_RECEIPT_LEASE,
        ));
        Ok(acknowledgement)
    }

    fn acknowledge_delivery(
        &self,
        cookie: Option<&str>,
        request: &DeliveryAcknowledgementV1,
    ) -> Result<ParticipantSessionStatusV1, ParticipantHandoffErrorV1> {
        if !is_token(&request.acknowledgement, "wsa1:") {
            return Err(ParticipantHandoffErrorV1::InvalidRequest);
        }
        // Revalidate current authority and exact active Client selection. An old
        // response can never revive an invalidated or replaced session.
        self.resolve_session(cookie)?;
        let token = cookie
            .and_then(parse_session_cookie)
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        let mut state = self.lock();
        prune_expired(&mut state);
        let record = state
            .sessions
            .get_mut(token)
            .ok_or(ParticipantHandoffErrorV1::SessionMissing)?;
        let (nonce, frame_head, expires_at) = record
            .pending_delivery
            .take()
            .ok_or(ParticipantHandoffErrorV1::InvalidRequest)?;
        if nonce != request.acknowledgement
            || frame_head != request.frame_head
            || Instant::now() >= expires_at
        {
            return Err(ParticipantHandoffErrorV1::InvalidRequest);
        }
        record.acknowledged_at = Some(Instant::now());
        Ok(ParticipantSessionStatusV1::usable())
    }
}

impl ParticipantConsoleReadinessSourceV1 for BrowserDeliveryReadinessV1 {
    fn session_health(&self, room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1 {
        if !is_ulid(room_id) || !is_ulid(member_id) {
            return ParticipantConsoleSessionHealthV1::Invalid;
        }
        let expected = target_fingerprint(&self.0.inner.readiness_key, room_id, member_id);
        let mut tokens = {
            let mut state = self.0.lock();
            prune_expired(&mut state);
            state
                .sessions
                .iter()
                .filter(|(_, record)| {
                    record.target_fingerprint == expected
                        && record
                            .acknowledged_at
                            .is_some_and(|ack| ack.elapsed() < BROWSER_RECEIPT_LEASE)
                })
                .map(|(token, _)| token.clone())
                .collect::<Vec<_>>()
        };
        tokens.sort();
        let mut result = ParticipantConsoleSessionHealthV1::Missing;
        for token in tokens {
            let cookie = Zeroizing::new(format!("{SESSION_COOKIE}={token}"));
            match self.0.resolve_session(Some(cookie.as_str())) {
                Ok(resolved)
                    if resolved.authority.room_id() == room_id
                        && resolved.authority.member_id() == member_id =>
                {
                    let state = self.0.lock();
                    if state.sessions.get(&token).is_some_and(|record| {
                        record.expires_at > Instant::now()
                            && record
                                .acknowledged_at
                                .is_some_and(|ack| ack.elapsed() < BROWSER_RECEIPT_LEASE)
                    }) {
                        return ParticipantConsoleSessionHealthV1::Usable;
                    }
                }
                Ok(_) => {
                    self.0.invalidate_session(&token);
                }
                Err(error) if !error.invalidates_retained_session() => {
                    result = ParticipantConsoleSessionHealthV1::Disconnected;
                }
                Err(_) => {}
            }
        }
        result
    }
}

impl ParticipantConsoleReadinessSourceV1 for ParticipantHandoffBrokerV1 {
    fn session_health(&self, room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1 {
        if !is_ulid(room_id) || !is_ulid(member_id) {
            return ParticipantConsoleSessionHealthV1::Invalid;
        }
        let expected = target_fingerprint(&self.inner.readiness_key, room_id, member_id);
        let retained = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .sessions
                .iter()
                .find(|(_, record)| record.target_fingerprint == expected)
                .map(|(token, record)| {
                    (
                        token.clone(),
                        record.binding.clone(),
                        record.selection.clone(),
                        record.durable_cursor,
                    )
                })
        };
        let Some((token, binding, selection, cursor)) = retained else {
            return ParticipantConsoleSessionHealthV1::Missing;
        };
        let mut authority = match self
            .inner
            .authority
            .resolve_provisioned_human_seat(&binding.draft_id, &binding.seat_id)
        {
            Ok(authority) => authority,
            Err(ParticipantHandoffAuthorityErrorV1::Unavailable) => {
                return ParticipantConsoleSessionHealthV1::Disconnected;
            }
            Err(
                ParticipantHandoffAuthorityErrorV1::SeatNotFound
                | ParticipantHandoffAuthorityErrorV1::NotHuman
                | ParticipantHandoffAuthorityErrorV1::NotProvisioned
                | ParticipantHandoffAuthorityErrorV1::AuthorityInvalid,
            ) => {
                self.invalidate_session(&token);
                return ParticipantConsoleSessionHealthV1::Invalid;
            }
        };
        let current = match self.inner.gateway.current_membership(&authority, cursor) {
            Ok(current) => current,
            Err(
                ParticipantConsoleGatewayErrorV1::Disconnected
                | ParticipantConsoleGatewayErrorV1::Unavailable,
            ) => return ParticipantConsoleSessionHealthV1::Disconnected,
            Err(ParticipantConsoleGatewayErrorV1::Rejected) => {
                self.invalidate_session(&token);
                return ParticipantConsoleSessionHealthV1::Invalid;
            }
        };
        if let Err(error) = self.validate_retained_selection(&current, &selection) {
            if error.invalidates_retained_session() {
                self.invalidate_session(&token);
                return ParticipantConsoleSessionHealthV1::Invalid;
            }
            return ParticipantConsoleSessionHealthV1::Disconnected;
        }
        authority.install_current_membership(&current);
        match self.inner.gateway.health(&authority, cursor) {
            Ok(
                ParticipantConsoleSessionHealthV1::Missing
                | ParticipantConsoleSessionHealthV1::Invalid,
            )
            | Err(ParticipantConsoleGatewayErrorV1::Rejected) => {
                self.invalidate_session(&token);
                ParticipantConsoleSessionHealthV1::Invalid
            }
            Ok(health) => health,
            Err(
                ParticipantConsoleGatewayErrorV1::Disconnected
                | ParticipantConsoleGatewayErrorV1::Unavailable,
            ) => ParticipantConsoleSessionHealthV1::Disconnected,
        }
    }
}

/// Invalid fixed broker configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantHandoffConfigErrorV1 {
    Invalid,
    Unavailable,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueHandoffRequestV1 {
    #[serde(rename = "draft_id")]
    draft: String,
    #[serde(rename = "seat_id")]
    seat: String,
    #[serde(rename = "candidate_id")]
    candidate: Option<String>,
}

/// Explicit one-use browser transfer. Ready contains a secret URL, so it must
/// never be included in ordinary operator reports or process arguments.
#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClientHandoffResponseV1 {
    Ready {
        version: String,
        client_url: String,
    },
    SelectionRequired {
        version: String,
        candidates: Vec<ClientCandidateSummaryV1>,
    },
}

/// Safe selection metadata contains no handoff token, URL or authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCandidateSummaryV1 {
    pub candidate_id: String,
    pub deployment_id: String,
    pub client_id: String,
    pub release_digest: String,
    pub surface_id: String,
    pub trust_level: DeploymentTrustLevelV1,
}

/// Optional exact candidate selection; absence uses the existing approved
/// selection rules and may return an actionable candidate list.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorClientHandoffRequestV1 {
    #[serde(default)]
    pub binding: Option<String>,
}

impl From<ClientCandidateV1> for ClientCandidateSummaryV1 {
    fn from(candidate: ClientCandidateV1) -> Self {
        Self {
            candidate_id: candidate.candidate_id,
            deployment_id: candidate.deployment_id,
            client_id: candidate.client_id,
            release_digest: candidate.release_digest,
            surface_id: candidate.surface_id,
            trust_level: candidate.trust_level,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveRequestV1 {
    after_frame_seq: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRequestV1 {
    at_room_seq: u64,
}

/// Exact participant Action request with routing and authority supplied server-side.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantActionRequestV1 {
    pub action_id: String,
    pub based_on_room_seq: u64,
    pub offer_id: String,
    pub schema_digest: String,
    pub action_type: String,
    pub payload: Value,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ParticipantSessionStatusV1 {
    version: &'static str,
    state: &'static str,
    next_action: &'static str,
}

impl ParticipantSessionStatusV1 {
    const fn usable() -> Self {
        Self {
            version: SESSION_VERSION,
            state: "usable",
            next_action: "continue",
        }
    }

    const fn disconnected() -> Self {
        Self {
            version: SESSION_VERSION,
            state: "disconnected",
            next_action: "reconnect",
        }
    }
}

/// Builds the isolated local handoff and Participant Console session routes.
pub fn participant_handoff_router(broker: ParticipantHandoffBrokerV1) -> Router {
    let router = Router::new()
        .route(
            "/api/v1/participant-console/handoffs",
            post(issue_handoff).options(cors_preflight),
        )
        .route(
            "/api/v1/participant-console/handoffs:redeem",
            post(redeem_handoff).options(cors_preflight),
        )
        .route(
            "/api/v1/participant-console/session",
            get(session_status).options(cors_preflight),
        )
        .route(
            "/api/v1/participant-console/session:observe",
            post(observe).options(cors_preflight),
        )
        .route(
            "/api/v1/participant-console/session:act",
            post(act).options(cors_preflight),
        )
        .route(
            "/api/v1/participant-console/session:replay",
            post(replay).options(cors_preflight),
        );
    let router = router.route(
        "/api/v1/participant-console/session:acknowledge",
        post(acknowledge_delivery).options(cors_preflight),
    );
    router
        .layer(from_fn_with_state(broker.clone(), local_cors))
        .with_state(broker)
}

/// Merged only inside installation-control admission. This is deliberately not
/// a browser CORS route and never substitutes a forged Studio Origin.
pub fn operator_client_handoff_router(broker: ParticipantHandoffBrokerV1) -> Router {
    Router::new()
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/client-handoff",
            post(issue_operator_handoff),
        )
        .layer(axum::extract::DefaultBodyLimit::max(8 * 1024))
        .with_state(broker)
}

async fn issue_operator_handoff(
    State(broker): State<ParticipantHandoffBrokerV1>,
    axum::extract::Path((operation, seat)): axum::extract::Path<(String, String)>,
    body: Bytes,
) -> Response {
    let request = if body.is_empty() {
        Ok(OperatorClientHandoffRequestV1 { binding: None })
    } else {
        serde_json::from_slice(&body).map_err(|_| ParticipantHandoffErrorV1::InvalidRequest)
    };
    let result = match request {
        Ok(request) => {
            run_blocking(move || {
                broker.issue(IssueHandoffRequestV1 {
                    draft: operation,
                    seat,
                    candidate: request.binding,
                })
            })
            .await
        }
        Err(error) => Err(error),
    };
    let mut response = match result {
        Ok(response) => {
            let status = match &response {
                ClientHandoffResponseV1::Ready { .. } => StatusCode::CREATED,
                ClientHandoffResponseV1::SelectionRequired { .. } => StatusCode::OK,
            };
            (status, Json(response)).into_response()
        }
        Err(error) => error.into_response(),
    };
    apply_no_store(response.headers_mut());
    response
}

async fn cors_preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn local_cors(
    State(broker): State<ParticipantHandoffBrokerV1>,
    request: Request,
    next: Next,
) -> Response {
    let expected = if request.uri().path() == "/api/v1/participant-console/handoffs" {
        &broker.inner.studio_origin
    } else {
        &broker.inner.console_origin
    };
    let origin_allowed = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(expected.as_str());
    if request.method() == Method::OPTIONS {
        if !origin_allowed || !valid_preflight(&request) {
            let mut response = ParticipantHandoffErrorV1::OriginForbidden.into_response();
            apply_no_store(response.headers_mut());
            return response;
        }
        let mut response = StatusCode::NO_CONTENT.into_response();
        apply_cors_headers(response.headers_mut(), expected);
        return response;
    }
    let mut response = next.run(request).await;
    apply_no_store(response.headers_mut());
    if response.status() == StatusCode::UNAUTHORIZED
        && let Ok(cookie) = cleared_session_cookie()
    {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    if origin_allowed {
        apply_cors_headers(response.headers_mut(), expected);
    }
    response
}

fn valid_preflight(request: &Request) -> bool {
    let requested_method = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| value.to_str().ok());
    let expected_method = if request.uri().path() == "/api/v1/participant-console/session" {
        "GET"
    } else {
        "POST"
    };
    if requested_method != Some(expected_method) {
        return false;
    }
    request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|headers| {
            headers.split(',').all(|header| {
                matches!(
                    header.trim().to_ascii_lowercase().as_str(),
                    "cache-control"
                        | "pragma"
                        | "content-type"
                        | "x-worldstream-participant-handoff"
                )
            })
        })
}

fn apply_cors_headers(headers: &mut HeaderMap, origin: &str) {
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("X-WorldStream-Delivery-Acknowledgement"),
    );
    if let Ok(origin) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "Cache-Control, Pragma, Content-Type, X-WorldStream-Participant-Handoff",
        ),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    apply_no_store(headers);
}

fn apply_no_store(headers: &mut HeaderMap) {
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
}

async fn issue_handoff(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
    Json(request): Json<IssueHandoffRequestV1>,
) -> Result<(StatusCode, Json<ClientHandoffResponseV1>), ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.studio_origin)?;
    let response = run_blocking(move || broker.issue(request)).await?;
    let status = match &response {
        ClientHandoffResponseV1::Ready { .. } => StatusCode::CREATED,
        ClientHandoffResponseV1::SelectionRequired { .. } => StatusCode::OK,
    };
    Ok((status, Json(response)))
}

async fn redeem_handoff(
    State(broker): State<ParticipantHandoffBrokerV1>,
    request: Request,
) -> Result<Response, ParticipantHandoffErrorV1> {
    require_origin(request.headers(), &broker.inner.console_origin)?;
    let handoff = Zeroizing::new(
        request
            .headers()
            .get(HANDOFF_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or(ParticipantHandoffErrorV1::InvalidHandoff)?
            .to_owned(),
    );
    let retained = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    let (status, session) = run_blocking(move || {
        broker.redeem(
            handoff.as_str(),
            retained.as_ref().map(|value| value.as_str()),
        )
    })
    .await?;
    let cookie = session_cookie(&session)?;
    let mut response = Json(status).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    Ok(response)
}

async fn session_status(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
) -> Result<Json<ParticipantSessionStatusV1>, ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.console_origin)?;
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    run_blocking(move || broker.session_status(cookie.as_ref().map(|value| value.as_str())))
        .await
        .map(Json)
}

async fn observe(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
    Json(request): Json<ObserveRequestV1>,
) -> Result<Response, ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.console_origin)?;
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    run_blocking(move || {
        let cookie = cookie.as_ref().map(|value| value.as_str());
        let value = broker.observe(cookie, request)?;
        let acknowledgement = broker.delivery_receipt(
            cookie,
            value
                .get("frame_head")
                .and_then(Value::as_u64)
                .ok_or(ParticipantHandoffErrorV1::Upstream)?,
        )?;
        let response = Json(value).into_response();
        let response = {
            let mut response = response;
            response.headers_mut().insert(
                "x-worldstream-delivery-acknowledgement",
                HeaderValue::from_str(&acknowledgement)
                    .map_err(|_| ParticipantHandoffErrorV1::Upstream)?,
            );
            response
        };
        Ok(response)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryAcknowledgementV1 {
    acknowledgement: String,
    frame_head: u64,
}

async fn acknowledge_delivery(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
    Json(request): Json<DeliveryAcknowledgementV1>,
) -> Result<Json<ParticipantSessionStatusV1>, ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.console_origin)?;
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    run_blocking(move || {
        broker.acknowledge_delivery(cookie.as_ref().map(|value| value.as_str()), &request)
    })
    .await
    .map(Json)
}

async fn act(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
    Json(request): Json<ParticipantActionRequestV1>,
) -> Result<Json<Value>, ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.console_origin)?;
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    run_blocking(move || broker.act(cookie.as_ref().map(|value| value.as_str()), &request))
        .await
        .map(Json)
}

async fn replay(
    State(broker): State<ParticipantHandoffBrokerV1>,
    headers: HeaderMap,
    Json(request): Json<ReplayRequestV1>,
) -> Result<Json<Value>, ParticipantHandoffErrorV1> {
    require_origin(&headers, &broker.inner.console_origin)?;
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| Zeroizing::new(value.to_owned()));
    run_blocking(move || broker.replay(cookie.as_ref().map(|value| value.as_str()), request))
        .await
        .map(Json)
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, ParticipantHandoffErrorV1> + Send + 'static,
) -> Result<T, ParticipantHandoffErrorV1> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| ParticipantHandoffErrorV1::Upstream)?
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParticipantHandoffErrorV1 {
    InvalidRequest,
    OriginForbidden,
    SeatNotFound,
    HumanSeatRequired,
    NotProvisioned,
    SelectionInvalid,
    ClientUnavailable,
    AuthorityInvalid,
    ReplayUnavailable,
    InvalidHandoff,
    SessionMissing,
    Capacity,
    Upstream,
}

impl ParticipantHandoffErrorV1 {
    const fn from_authority(error: ParticipantHandoffAuthorityErrorV1) -> Self {
        match error {
            ParticipantHandoffAuthorityErrorV1::SeatNotFound => Self::SeatNotFound,
            ParticipantHandoffAuthorityErrorV1::NotHuman => Self::HumanSeatRequired,
            ParticipantHandoffAuthorityErrorV1::NotProvisioned => Self::NotProvisioned,
            ParticipantHandoffAuthorityErrorV1::AuthorityInvalid => Self::AuthorityInvalid,
            ParticipantHandoffAuthorityErrorV1::Unavailable => Self::Upstream,
        }
    }

    const fn from_gateway(error: ParticipantConsoleGatewayErrorV1) -> Self {
        match error {
            ParticipantConsoleGatewayErrorV1::Disconnected
            | ParticipantConsoleGatewayErrorV1::Unavailable => Self::Upstream,
            ParticipantConsoleGatewayErrorV1::Rejected => Self::AuthorityInvalid,
        }
    }

    const fn invalidates_retained_session(self) -> bool {
        matches!(
            self,
            Self::SeatNotFound
                | Self::HumanSeatRequired
                | Self::NotProvisioned
                | Self::SelectionInvalid
                | Self::AuthorityInvalid
        )
    }
}

impl ParticipantHandoffErrorV1 {
    const fn response_metadata(
        self,
    ) -> (StatusCode, &'static str, &'static str, &'static str, bool) {
        match self {
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "participant_handoff_invalid_request",
                "The participant client request is invalid.",
                "return_to_task_setup",
                false,
            ),
            Self::OriginForbidden => (
                StatusCode::FORBIDDEN,
                "participant_handoff_origin_forbidden",
                "Participant client access is available only from the configured local application.",
                "return_to_task_setup",
                false,
            ),
            Self::SeatNotFound => (
                StatusCode::NOT_FOUND,
                "participant_handoff_seat_not_found",
                "The selected seat is no longer available.",
                "return_to_task_setup",
                false,
            ),
            Self::HumanSeatRequired => (
                StatusCode::CONFLICT,
                "participant_handoff_human_seat_required",
                "Open participant client is available only for a human seat.",
                "return_to_task_setup",
                false,
            ),
            Self::NotProvisioned => (
                StatusCode::CONFLICT,
                "participant_handoff_authority_not_provisioned",
                "Participant authority must be provisioned before opening this seat.",
                "return_to_task_setup",
                false,
            ),
            Self::SelectionInvalid => (
                StatusCode::CONFLICT,
                "activity_client_selection_invalid",
                "The selected Activity Client is no longer approved for this Membership.",
                "return_to_task_setup",
                false,
            ),
            Self::ClientUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "activity_client_selection_unavailable",
                "The Host could not resolve an approved Activity Client safely.",
                "retry",
                true,
            ),
            Self::AuthorityInvalid => (
                StatusCode::UNAUTHORIZED,
                "participant_session_authority_invalid",
                "The retained Participant authority is no longer valid.",
                "return_to_task_setup",
                false,
            ),
            Self::ReplayUnavailable => (
                StatusCode::FORBIDDEN,
                "participant_replay_unavailable",
                "Historical Replay is not available for this provisioned participant authority.",
                "return_to_task_setup",
                false,
            ),
            Self::InvalidHandoff => (
                StatusCode::UNAUTHORIZED,
                "participant_handoff_invalid",
                "This participant client handoff is missing, expired, or already used.",
                "return_to_task_setup",
                false,
            ),
            Self::SessionMissing => (
                StatusCode::UNAUTHORIZED,
                "participant_session_missing",
                "This participant client session is missing or expired.",
                "return_to_task_setup",
                false,
            ),
            Self::Capacity => (
                StatusCode::SERVICE_UNAVAILABLE,
                "participant_handoff_capacity",
                "Participant client handoff capacity is temporarily unavailable.",
                "retry",
                true,
            ),
            Self::Upstream => (
                StatusCode::BAD_GATEWAY,
                "participant_session_unavailable",
                "The participant client cannot reach the Room service safely.",
                "reconnect",
                true,
            ),
        }
    }
}

impl IntoResponse for ParticipantHandoffErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, next_action, retryable) = self.response_metadata();
        (
            status,
            Json(ErrorBodyV1 {
                code,
                message,
                next_action,
                retryable,
            }),
        )
            .into_response()
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ErrorBodyV1 {
    code: &'static str,
    message: &'static str,
    next_action: &'static str,
    retryable: bool,
}

fn validate_binding(draft_id: &str, seat_id: &str) -> Result<(), ParticipantHandoffErrorV1> {
    if draft_id.is_empty()
        || draft_id.len() > 64
        || !draft_id
            .bytes()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == b'-')
        || seat_id.is_empty()
        || seat_id.len() > MAX_SEAT_ID_BYTES
        || !seat_id.bytes().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, b'-' | b'_')
        })
    {
        return Err(ParticipantHandoffErrorV1::InvalidRequest);
    }
    Ok(())
}

fn validate_action(request: &ParticipantActionRequestV1) -> Result<(), ParticipantHandoffErrorV1> {
    let encoded = serde_json::to_vec(&request.payload)
        .map_err(|_| ParticipantHandoffErrorV1::InvalidRequest)?;
    if !is_ulid(&request.action_id)
        || request.offer_id.is_empty()
        || request.offer_id.len() > MAX_OFFER_ID_BYTES
        || request.action_type.is_empty()
        || request.action_type.len() > MAX_ACTION_TYPE_BYTES
        || !is_blake3_digest(&request.schema_digest)
        || encoded.len() > MAX_ACTION_PAYLOAD_BYTES
        || contains_prohibited_browser_material(&request.payload)
    {
        return Err(ParticipantHandoffErrorV1::InvalidRequest);
    }
    Ok(())
}

fn require_origin(headers: &HeaderMap, expected: &str) -> Result<(), ParticipantHandoffErrorV1> {
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(expected)
    {
        return Err(ParticipantHandoffErrorV1::OriginForbidden);
    }
    Ok(())
}

fn browser_safe_gateway_value(value: Value) -> Result<Value, ParticipantHandoffErrorV1> {
    if contains_prohibited_browser_material(&value) {
        return Err(ParticipantHandoffErrorV1::Upstream);
    }
    Ok(value)
}

fn contains_prohibited_browser_material(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "bearer"
                    | "token_hash"
                    | "secret_reference"
                    | "secret_ref"
                    | "host_authority"
                    | "runner_authority"
                    | "room_id"
                    | "member_id"
                    | "membership_id"
                    | "durable_cursor"
                    | "path"
                    | "file_path"
                    | "agent_private_memory"
                    | "invocation_context"
                    | "prompt"
                    | "provider_response"
                    | "model_response"
            ) || contains_prohibited_browser_material(child)
        }),
        Value::Array(values) => values.iter().any(contains_prohibited_browser_material),
        Value::String(value) => value.contains("wsb1:") || value.contains("wst1:"),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn session_cookie(session: &str) -> Result<HeaderValue, ParticipantHandoffErrorV1> {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={session}; Max-Age={SESSION_MAX_AGE_SECONDS}; Path={SESSION_COOKIE_PATH}; HttpOnly; SameSite=Strict"
    ))
    .map_err(|_| ParticipantHandoffErrorV1::Upstream)
}

fn cleared_session_cookie() -> Result<HeaderValue, ParticipantHandoffErrorV1> {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; Max-Age=0; Path={SESSION_COOKIE_PATH}; HttpOnly; SameSite=Strict"
    ))
    .map_err(|_| ParticipantHandoffErrorV1::Upstream)
}

fn parse_session_cookie(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == SESSION_COOKIE).then_some(value)
    })
}

fn random_token(prefix: &str) -> Result<String, ParticipantHandoffErrorV1> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| ParticipantHandoffErrorV1::Upstream)?;
    let token = format!("{prefix}{}", encode_hex(&bytes));
    bytes.fill(0);
    Ok(token)
}

fn unique_token<T>(
    prefix: &str,
    retained: &HashMap<String, T>,
) -> Result<String, ParticipantHandoffErrorV1> {
    for _ in 0..4 {
        let token = random_token(prefix)?;
        if !retained.contains_key(&token) {
            return Ok(token);
        }
    }
    Err(ParticipantHandoffErrorV1::Upstream)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn prune_expired(state: &mut BrokerStateV1) {
    let now = Instant::now();
    state.handoffs.retain(|_, record| record.expires_at > now);
    state.sessions.retain(|_, record| record.expires_at > now);
}

fn is_token(value: &str, prefix: &str) -> bool {
    value.len() == TOKEN_WIRE_LENGTH
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'))
}

fn is_blake3_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn selected_launch_base(
    client_host_origin: &str,
    launch_url: &str,
) -> Result<String, ParticipantHandoffErrorV1> {
    let Some(path) = launch_url.strip_prefix(client_host_origin) else {
        return Err(ParticipantHandoffErrorV1::ClientUnavailable);
    };
    if !path.starts_with('/')
        || !path.ends_with('/')
        || path.contains(['?', '#', '\\', '\r', '\n'])
        || path.split('/').any(|segment| segment == "..")
    {
        return Err(ParticipantHandoffErrorV1::ClientUnavailable);
    }
    Ok(launch_url.to_owned())
}

fn is_client_candidate_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn target_fingerprint(key: &[u8; 32], room_id: &str, member_id: &str) -> [u8; 32] {
    let mut input = Vec::with_capacity(room_id.len() + member_id.len() + 1);
    input.extend_from_slice(room_id.as_bytes());
    input.push(0);
    input.extend_from_slice(member_id.as_bytes());
    *blake3::keyed_hash(key, &input).as_bytes()
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), ParticipantConsoleGatewayErrorV1> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(ParticipantConsoleGatewayErrorV1::Unavailable)?;
    let headers = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| ParticipantConsoleGatewayErrorV1::Unavailable)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ParticipantConsoleGatewayErrorV1::Unavailable)?;
    Ok((status, &bytes[(separator + 4)..]))
}

#[cfg(test)]
mod transport_error_tests {
    use super::*;

    type ClientHandshake = tungstenite::handshake::client::ClientHandshake<TcpStream>;

    #[test]
    fn handshake_transport_failure_is_reconnectable_not_authority_invalid() {
        let error: tungstenite::HandshakeError<ClientHandshake> =
            tungstenite::HandshakeError::Failure(tungstenite::Error::Io(std::io::Error::from(
                std::io::ErrorKind::TimedOut,
            )));
        assert_eq!(
            handshake_error(error),
            ParticipantConsoleGatewayErrorV1::Disconnected
        );
    }

    #[test]
    fn only_authorization_style_handshake_statuses_reject_authority() {
        assert!(handshake_http_status_error(http::StatusCode::UNAUTHORIZED));
        assert!(handshake_http_status_error(http::StatusCode::FORBIDDEN));
        assert!(!handshake_http_status_error(http::StatusCode::BAD_REQUEST));
        assert!(!handshake_http_status_error(http::StatusCode::NOT_FOUND));
        assert!(!handshake_http_status_error(
            http::StatusCode::SERVICE_UNAVAILABLE
        ));
    }
}
