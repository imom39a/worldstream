//! Generic assignment-bound Action listing and submission orchestration.
//!
//! This module accepts no Room, Membership, routing, or authority input from a
//! tool. Callers supply an already materialized assignment snapshot and sealed
//! adapters for exact Pack schemas, participant submission, and durability.

use std::{
    collections::BTreeSet,
    fmt,
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use crate::assignment_mcp::{AssignedMembershipAuthorityV1, MembershipStreamSnapshotV1};
use crate::assignment_mcp_operations::{
    AssignmentMcpOperationErrorV1, AssignmentMcpOperationIdentityV1,
    AssignmentMcpOperationIntentV1, AssignmentMcpOperationLedgerV1, AssignmentMcpOperationPhaseV1,
    AssignmentMcpOperationRecordV1, AssignmentMcpRemoteAcceptanceV1, AssignmentMcpRemoteOutcomeV1,
};
use axum::{body::Bytes, http::HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tungstenite::{Message, WebSocket, client, http};
use worldstream_core::{ACTION_OFFER_DOMAIN, Blake3DigestV1, CanonicalJsonV1};
use worldstream_protocol::{
    AccessMode, ActionAccepted, ActionOffer, ActionRejected, ActionSubmit,
    ActivityPackCatalogAction, ActivityPackCatalogSchema, ClientHello, ClientMode, ErrorCode,
    MAX_MESSAGE_BYTES, ObservationDeliver, PROTOCOL_VERSION, PackReference, PrincipalKind,
    ProjectionReset, ProtocolErrorBody, REQUIRED_CLIENT_CAPABILITIES, RoomAttached, ServerWelcome,
    SyncBranch, UlidString, VersionedEnvelope, WEBSOCKET_SUBPROTOCOL, decode_envelope,
};
use zeroize::Zeroizing;

const MAX_OFFERS: usize = 256;
const MAX_TEXT_BYTES: usize = 256;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SESSION_MESSAGES: usize = 64;
/// Multi-step assignment operations have a separate budget from health probes.
pub const ASSIGNMENT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const SUPPORTED_SCHEMA_KEYWORDS: [&str; 14] = [
    "type",
    "const",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "description",
];

/// Safe exact Head witness required by an Action submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpActionHeadV1 {
    pub room_seq: u64,
    pub head_hash: String,
}

/// One exact currently offered Action and its actual declared payload schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpListedActionOfferV1 {
    pub offer_id: String,
    pub action_type: String,
    pub payload_schema: ActivityPackCatalogSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligibility_window: Option<serde_json::Value>,
}

/// Generic list of exact Action Offers for the currently materialized Head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpActionOfferListV1 {
    pub schema: &'static str,
    pub precondition: AssignmentMcpActionHeadV1,
    pub offers: Vec<AssignmentMcpListedActionOfferV1>,
}

/// Strict tool input for one stable exact Action operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpSubmitActionV1 {
    pub operation_id: String,
    pub offer_id: String,
    pub precondition: AssignmentMcpActionHeadV1,
    pub payload: Value,
}

/// Exact safe request retained before the sealed gateway is called.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpExactActionRequestV1 {
    pub assignment_id: String,
    pub operation_id: String,
    pub request_id: String,
    pub action_id: String,
    pub based_on: AssignmentMcpActionHeadV1,
    pub offer_id: String,
    pub action_type: String,
    pub payload_schema_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligibility_window: Option<Value>,
    pub payload: Value,
}

/// Assignment-bound safe daemon response below the tool boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome", deny_unknown_fields)]
pub enum AssignmentMcpActionDaemonResultV1 {
    Accepted {
        action_id: String,
        transition_id: String,
        admitted_at: String,
        committed_head: AssignmentMcpActionHeadV1,
        duplicate: bool,
    },
    Rejected {
        action_id: String,
        admitted_at: String,
        code: String,
        message: String,
        current_room_seq: u64,
        current_action_offers: Vec<ActionOffer>,
        retryable_with_same_action_id: bool,
        may_submit_revised_action: bool,
        duplicate: bool,
        details: Value,
    },
}

/// Closed sealed-gateway failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AssignmentMcpActionGatewayErrorV1 {
    #[error("Action response is ambiguous")]
    Ambiguous,
    #[error("assignment daemon disconnected")]
    Disconnected,
    #[error("assignment authority was rejected")]
    Revoked,
    #[error("assignment daemon is unavailable")]
    Unavailable,
    #[error("assignment daemon returned invalid data")]
    InvalidData,
}

/// Sealed participant-only submission boundary.
pub trait AssignmentMcpActionGatewayV1: Send + Sync {
    /// Submits or reconciles the exact stable Action request.
    ///
    /// # Errors
    ///
    /// Returns closed authority, transport, ambiguity, or invalid-data state.
    fn submit_exact(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1>;
}

/// Fixed loopback daemon adapter using sealed participant authority.
#[derive(Clone)]
pub struct FixedDaemonAssignmentMcpActionGatewayV1 {
    address: SocketAddr,
    timeout: Duration,
}

impl fmt::Debug for FixedDaemonAssignmentMcpActionGatewayV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FixedDaemonAssignmentMcpActionGatewayV1(REDACTED)")
    }
}

impl FixedDaemonAssignmentMcpActionGatewayV1 {
    /// Constructs a bounded loopback-only Action gateway.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback addresses and zero or excessive timeouts.
    pub fn new(
        address: SocketAddr,
        timeout: Duration,
    ) -> Result<Self, AssignmentMcpActionGatewayErrorV1> {
        if !address.ip().is_loopback() || timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
        Ok(Self { address, timeout })
    }

    fn connect(
        &self,
        authority: &AssignedMembershipAuthorityV1,
    ) -> Result<WebSocket<TcpStream>, AssignmentMcpActionGatewayErrorV1> {
        let stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)?;
        let mut authorization = Zeroizing::new(Vec::with_capacity(
            "Bearer ".len() + authority.bearer().as_str().len(),
        ));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(authority.bearer().as_str().as_bytes());
        let authorization = HeaderValue::from_maybe_shared(Bytes::from_owner(authorization))
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("ws://{}/v1/stream", self.address))
            .header("Host", self.address.to_string())
            .header("Authorization", authorization)
            .header("Sec-WebSocket-Protocol", WEBSOCKET_SUBPROTOCOL)
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
        let (socket, response) = match client(request, stream) {
            Ok(connected) => connected,
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if matches!(
                    response.status(),
                    http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
                ) =>
            {
                return Err(AssignmentMcpActionGatewayErrorV1::Revoked);
            }
            Err(tungstenite::HandshakeError::Failure(
                tungstenite::Error::Io(_)
                | tungstenite::Error::ConnectionClosed
                | tungstenite::Error::AlreadyClosed
                | tungstenite::Error::Protocol(
                    tungstenite::error::ProtocolError::HandshakeIncomplete,
                ),
            )) => {
                return Err(AssignmentMcpActionGatewayErrorV1::Disconnected);
            }
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if response.status().is_server_error()
                    || response.status() == http::StatusCode::TOO_MANY_REQUESTS =>
            {
                return Err(AssignmentMcpActionGatewayErrorV1::Unavailable);
            }
            Err(_) => return Err(AssignmentMcpActionGatewayErrorV1::InvalidData),
        };
        if response.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
        Ok(socket)
    }

    fn synchronized_session(
        &self,
        authority: &AssignedMembershipAuthorityV1,
    ) -> Result<SynchronizedActionSessionV1, AssignmentMcpActionGatewayErrorV1> {
        let mut socket = self.connect(authority)?;
        let mut budget = ActionReadBudgetV1::new(self.timeout);
        send_protocol(
            &mut socket,
            "client.hello",
            None,
            &ClientHello {
                client_name: "worldstream-assignment-mcp".to_owned(),
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                mode: ClientMode::Participant,
                supported_protocols: vec![PROTOCOL_VERSION.to_owned()],
                capabilities: REQUIRED_CLIENT_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
        )?;
        let welcome: ServerWelcome =
            read_protocol(&mut socket, &mut budget, "server.welcome", None)?;
        if welcome.selected_protocol != PROTOCOL_VERSION
            || welcome.maximum_message_bytes != MAX_MESSAGE_BYTES
            || welcome.authenticated_principal.kind != PrincipalKind::Agent
            || welcome.authenticated_principal.principal_id != authority.principal_id()
        {
            return Err(AssignmentMcpActionGatewayErrorV1::Revoked);
        }
        send_protocol(
            &mut socket,
            "room.attach",
            None,
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "after_frame_seq": null,
            }),
        )?;
        let attached: RoomAttached =
            read_protocol(&mut socket, &mut budget, "room.attached", None)?;
        validate_action_attached(&attached, authority)?;
        match &attached.sync {
            SyncBranch::RetainedFrames {
                cursor_exclusive,
                through_frame_head,
            } => {
                let count = through_frame_head.saturating_sub(*cursor_exclusive);
                if count > u64::try_from(MAX_SESSION_MESSAGES).unwrap_or(u64::MAX) {
                    return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
                }
                for _ in 0..count {
                    let delivery: ObservationDeliver =
                        read_protocol(&mut socket, &mut budget, "observation.deliver", None)?;
                    if delivery.room_id != authority.room_id()
                        || delivery.member_id != authority.member_id()
                    {
                        return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
                    }
                }
            }
            SyncBranch::ProjectionReset { .. } => {
                let reset: ProjectionReset =
                    read_protocol(&mut socket, &mut budget, "projection.reset", None)?;
                if reset.room_id != authority.room_id()
                    || reset.member_id != authority.member_id()
                    || reset.room_head.room_id != authority.room_id()
                {
                    return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
                }
            }
        }
        send_protocol(
            &mut socket,
            "room.sync_ack",
            None,
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "through_frame_head": attached.frame_head,
                "sync_token": attached.sync_token,
            }),
        )?;
        let reply = read_scoped_action_reply(&mut socket, &mut budget, authority)?;
        if reply.message_type != "room.sync_acked" {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
        let synced: SyncAckedV1 = serde_json::from_value(reply.body)
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
        if synced.through_frame_head != attached.frame_head {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
        Ok(SynchronizedActionSessionV1 { socket, budget })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncAckedV1 {
    through_frame_head: u64,
}

impl AssignmentMcpActionGatewayV1 for FixedDaemonAssignmentMcpActionGatewayV1 {
    fn submit_exact(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        if request.assignment_id != authority.assignment_id() {
            return Err(AssignmentMcpActionGatewayErrorV1::Revoked);
        }
        let mut session = self.synchronized_session(authority)?;
        send_protocol(
            &mut session.socket,
            "action.submit",
            Some(&request.request_id),
            &ActionSubmit {
                room_id: authority.room_id().to_owned(),
                member_id: authority.member_id().to_owned(),
                action_id: request.action_id.clone(),
                based_on_room_seq: request.based_on.room_seq,
                action_type: request.action_type.clone(),
                payload: request.payload.clone(),
            },
        )?;
        read_action_result(&mut session.socket, &mut session.budget, authority, request)
    }
}

struct SynchronizedActionSessionV1 {
    socket: WebSocket<TcpStream>,
    budget: ActionReadBudgetV1,
}

struct ActionReadBudgetV1 {
    deadline: Instant,
    bytes: usize,
    messages: usize,
}

impl ActionReadBudgetV1 {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            bytes: 0,
            messages: 0,
        }
    }

    fn remaining(&self) -> Result<Duration, AssignmentMcpActionGatewayErrorV1> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(AssignmentMcpActionGatewayErrorV1::Disconnected)
    }

    fn consume(&mut self, bytes: usize) -> Result<(), AssignmentMcpActionGatewayErrorV1> {
        self.messages = self.messages.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.messages > MAX_SESSION_MESSAGES
            || self.bytes > MAX_MESSAGE_BYTES.saturating_mul(MAX_SESSION_MESSAGES)
        {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
        Ok(())
    }
}

fn send_protocol(
    socket: &mut WebSocket<TcpStream>,
    message_type: &str,
    request_id: Option<&str>,
    body: &impl Serialize,
) -> Result<(), AssignmentMcpActionGatewayErrorV1> {
    let request_id = request_id
        .map(str::parse::<UlidString>)
        .transpose()
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
    let envelope = VersionedEnvelope {
        protocol: PROTOCOL_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: next_message_id()?,
        request_id,
        body,
    };
    let text = serde_json::to_string(&envelope)
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
    }
    socket
        .send(Message::Text(text.into()))
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)
}

fn read_protocol<T: DeserializeOwned>(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut ActionReadBudgetV1,
    expected_type: &str,
    expected_request_id: Option<&str>,
) -> Result<T, AssignmentMcpActionGatewayErrorV1> {
    let envelope = read_envelope(socket, budget)?;
    if envelope.message_type != expected_type
        || expected_request_id.is_some_and(|expected| {
            envelope.request_id.as_ref().map(UlidString::as_str) != Some(expected)
        })
    {
        return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
    }
    serde_json::from_value(envelope.body)
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)
}

fn read_envelope(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut ActionReadBudgetV1,
) -> Result<VersionedEnvelope<Value>, AssignmentMcpActionGatewayErrorV1> {
    loop {
        socket
            .get_mut()
            .set_read_timeout(Some(budget.remaining()?))
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)?;
        match socket
            .read()
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)?
        {
            Message::Text(text) => {
                if text.len() > MAX_MESSAGE_BYTES {
                    return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
                }
                budget.consume(text.len())?;
                let envelope = decode_envelope::<Value>(text.as_bytes())
                    .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
                if envelope.message_type == "server.ping" {
                    if !matches!(&envelope.body, Value::Object(body) if body.is_empty()) {
                        return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
                    }
                    send_protocol(socket, "client.pong", None, &serde_json::json!({}))?;
                    continue;
                }
                return Ok(envelope);
            }
            Message::Ping(payload) => {
                budget.consume(payload.len())?;
                socket
                    .send(Message::Pong(payload))
                    .map_err(|_| AssignmentMcpActionGatewayErrorV1::Disconnected)?;
            }
            Message::Pong(payload) => budget.consume(payload.len())?,
            Message::Frame(_) => budget.consume(0)?,
            Message::Binary(payload) => {
                budget.consume(payload.len())?;
                return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
            }
            Message::Close(_) => return Err(AssignmentMcpActionGatewayErrorV1::Disconnected),
        }
    }
}

fn read_action_result(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut ActionReadBudgetV1,
    authority: &AssignedMembershipAuthorityV1,
    request: &AssignmentMcpExactActionRequestV1,
) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
    let envelope = read_scoped_action_reply(socket, budget, authority)?;
    if envelope.request_id.as_ref().map(UlidString::as_str) != Some(&request.request_id) {
        return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
    }
    match envelope.message_type.as_str() {
        "action.accepted" => {
            let result: ActionAccepted = serde_json::from_value(envelope.body)
                .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
            if result.room_id != authority.room_id()
                || result.member_id != authority.member_id()
                || result.action_id != request.action_id
                || result.room_head.room_id != authority.room_id()
            {
                return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
            }
            Ok(AssignmentMcpActionDaemonResultV1::Accepted {
                action_id: result.action_id,
                transition_id: result.transition_id,
                admitted_at: result.admitted_at,
                committed_head: AssignmentMcpActionHeadV1 {
                    room_seq: result.room_head.room_seq,
                    head_hash: result.room_head.genesis_or_transition_hash,
                },
                duplicate: result.duplicate,
            })
        }
        "action.rejected" => {
            let result: ActionRejected = serde_json::from_value(envelope.body)
                .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
            if result.room_id != authority.room_id()
                || result.member_id != authority.member_id()
                || result.action_id != request.action_id
            {
                return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
            }
            Ok(AssignmentMcpActionDaemonResultV1::Rejected {
                action_id: result.action_id,
                admitted_at: result.admitted_at,
                code: result.code,
                message: result.message,
                current_room_seq: result.current_room_seq,
                current_action_offers: result.action_offers,
                retryable_with_same_action_id: result.retryable_with_same_action_id,
                may_submit_revised_action: result.may_submit_revised_action,
                duplicate: result.duplicate,
                details: result.details,
            })
        }
        "error" => {
            let error: ProtocolErrorBody = serde_json::from_value(envelope.body)
                .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
            Err(map_protocol_error(&error))
        }
        _ => Err(AssignmentMcpActionGatewayErrorV1::InvalidData),
    }
}

/// A synchronized participant socket also carries live deliveries. They can
/// precede both the sync acknowledgement and the correlated Action receipt.
/// This submission-only adapter does not install or acknowledge those frames:
/// the observation adapter still owns its Cursor. Keep the original Action
/// precondition, and let the Runtime accept or reject that exact Action.
fn read_scoped_action_reply(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut ActionReadBudgetV1,
    authority: &AssignedMembershipAuthorityV1,
) -> Result<VersionedEnvelope<Value>, AssignmentMcpActionGatewayErrorV1> {
    loop {
        // Every skipped frame consumes the existing byte/message/time budget.
        let envelope = read_envelope(socket, budget)?;
        if envelope.message_type != "observation.deliver" {
            return Ok(envelope);
        }
        let delivery: ObservationDeliver = serde_json::from_value(envelope.body)
            .map_err(|_| AssignmentMcpActionGatewayErrorV1::InvalidData)?;
        if delivery.room_id != authority.room_id() || delivery.member_id != authority.member_id() {
            return Err(AssignmentMcpActionGatewayErrorV1::InvalidData);
        }
    }
}

fn validate_action_attached(
    attached: &RoomAttached,
    authority: &AssignedMembershipAuthorityV1,
) -> Result<(), AssignmentMcpActionGatewayErrorV1> {
    if attached.room_id != authority.room_id()
        || attached.member_id != authority.member_id()
        || attached.room_head.room_id != authority.room_id()
        || attached.principal_kind != PrincipalKind::Agent
        || attached.access_mode != AccessMode::Participant
        || attached.role.as_deref() != Some(authority.role())
        || attached.membership_status != "enabled"
        || attached.room_status != "active"
        || attached.sync_token.is_empty()
    {
        return Err(AssignmentMcpActionGatewayErrorV1::Revoked);
    }
    Ok(())
}

const fn map_protocol_error(error: &ProtocolErrorBody) -> AssignmentMcpActionGatewayErrorV1 {
    match error.code {
        ErrorCode::Unauthenticated
        | ErrorCode::Forbidden
        | ErrorCode::RoomNotFound
        | ErrorCode::MembershipNotFound
        | ErrorCode::MembershipNotEnabled => AssignmentMcpActionGatewayErrorV1::Revoked,
        ErrorCode::Internal
        | ErrorCode::StorageNotInitialized
        | ErrorCode::RoomBusy
        | ErrorCode::CommitIndeterminate
        | ErrorCode::RateLimited
        | ErrorCode::StorageUnavailable
            if error.retryable =>
        {
            AssignmentMcpActionGatewayErrorV1::Ambiguous
        }
        ErrorCode::ConfigInvalid
        | ErrorCode::StorageNotInitialized
        | ErrorCode::Internal
        | ErrorCode::UnsupportedProtocol
        | ErrorCode::InvalidEnvelope
        | ErrorCode::MessageTooLarge
        | ErrorCode::ActivityPackRevisionUnavailable
        | ErrorCode::RoomFaulted
        | ErrorCode::RoomQuarantined
        | ErrorCode::RoomBusy
        | ErrorCode::CursorAhead
        | ErrorCode::CursorOutOfRange
        | ErrorCode::SyncBarrierMismatch
        | ErrorCode::IdempotencyConflict
        | ErrorCode::WrongPhase
        | ErrorCode::CommitIndeterminate
        | ErrorCode::InvalidPayload
        | ErrorCode::ActivityFault
        | ErrorCode::RateLimited
        | ErrorCode::StorageUnavailable
        | ErrorCode::SlowConsumer => AssignmentMcpActionGatewayErrorV1::InvalidData,
    }
}

fn next_message_id() -> Result<UlidString, AssignmentMcpActionGatewayErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| AssignmentMcpActionGatewayErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    std::str::from_utf8(&encoded)
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::Unavailable)?
        .parse()
        .map_err(|_| AssignmentMcpActionGatewayErrorV1::Unavailable)
}

/// Why the caller must observe and list current offers again.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentMcpActionRefreshReasonV1 {
    StaleHead,
    OfferUnavailable,
    OfferExpired,
}

/// Safe stable outcome returned by Action submission and reconciliation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum AssignmentMcpActionSubmitResultV1 {
    Accepted {
        operation_id: String,
        action_id: String,
        transition_id: String,
        admitted_at: String,
        committed_head: AssignmentMcpActionHeadV1,
        duplicate: bool,
    },
    Rejected {
        operation_id: String,
        action_id: String,
        admitted_at: String,
        code: String,
        message: String,
        may_submit_revised_action: bool,
        duplicate: bool,
        details: Value,
    },
    RefreshRequired {
        operation_id: String,
        reason: AssignmentMcpActionRefreshReasonV1,
        current_room_seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_head_hash: Option<String>,
        requires_new_operation_id: bool,
        next_action: &'static str,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedActionResponseV1 {
    request_id: String,
    action_id: String,
    result: AssignmentMcpActionDaemonResultV1,
}

/// Closed exact-schema lookup failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AssignmentMcpActionSchemaErrorV1 {
    #[error("exact Action schema is missing")]
    Missing,
    #[error("exact Action schema source is unavailable")]
    Unavailable,
    #[error("exact Action schema is invalid")]
    Invalid,
}

/// Host-authorized source for one Action schema from one exact Pack revision.
pub trait AssignmentMcpActionSchemaSourceV1: Send + Sync {
    /// Returns only the exact descriptor-declared Action requested from the
    /// pinned Pack reference.
    ///
    /// # Errors
    ///
    /// Fails closed when the exact Pack/Action is absent, unavailable, or
    /// internally inconsistent.
    fn exact_action(
        &self,
        pack: &PackReference,
        action_type: &str,
    ) -> Result<ActivityPackCatalogAction, AssignmentMcpActionSchemaErrorV1>;
}

/// Closed safe tool failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AssignmentMcpActionErrorV1 {
    #[error("current Action Offers are invalid")]
    InvalidOfferData,
    #[error("exact Action schema is unavailable")]
    SchemaUnavailable,
    #[error("Action tool input is invalid")]
    InvalidInput,
    #[error("the exact Action is not currently offered")]
    Unoffered,
    #[error("the Action payload does not match its exact schema")]
    InvalidPayload,
    #[error("the stable operation identity conflicts with retained intent")]
    OperationConflict,
    #[error("Action operation storage is unavailable")]
    OperationUnavailable,
    #[error("Action response is ambiguous; retry the same operation")]
    AmbiguousRetrySameOperation,
    #[error("assignment authority was rejected")]
    AssignmentRevoked,
    #[error("assignment daemon returned invalid data")]
    InvalidDaemonData,
}

/// Lists exact current offers with actual declared schemas and a Head fence.
///
/// # Errors
///
/// Fails closed for malformed/unbounded offers, a schema lookup failure, or
/// any mismatch between the exact offer and descriptor schema.
pub fn list_current_action_offers(
    snapshot: &MembershipStreamSnapshotV1,
    schemas: &impl AssignmentMcpActionSchemaSourceV1,
) -> Result<AssignmentMcpActionOfferListV1, AssignmentMcpActionErrorV1> {
    if snapshot.current_action_offers.len() > MAX_OFFERS
        || snapshot.room_head.pack_digest != snapshot.pack.digest
    {
        return Err(AssignmentMcpActionErrorV1::InvalidOfferData);
    }
    let mut offers = Vec::with_capacity(snapshot.current_action_offers.len());
    for (index, offer) in snapshot.action_offers().iter().enumerate() {
        if offer.domain != ACTION_OFFER_DOMAIN
            || !bounded(&offer.action_type)
            || offer
                .payload_schema_digest
                .parse::<Blake3DigestV1>()
                .is_err()
        {
            return Err(AssignmentMcpActionErrorV1::InvalidOfferData);
        }
        let declared = schemas
            .exact_action(&snapshot.pack, &offer.action_type)
            .map_err(map_schema_error)?;
        if declared.action_type != offer.action_type
            || declared.payload_schema.schema_digest != offer.payload_schema_digest
            || !valid_schema(&declared.payload_schema)
        {
            return Err(AssignmentMcpActionErrorV1::InvalidOfferData);
        }
        offers.push(AssignmentMcpListedActionOfferV1 {
            offer_id: format!(
                "{}:{index}:{}",
                snapshot.room_head.room_seq, offer.payload_schema_digest
            ),
            action_type: offer.action_type.clone(),
            payload_schema: declared.payload_schema,
            eligibility_window: offer.eligibility_window.clone(),
        });
    }
    Ok(AssignmentMcpActionOfferListV1 {
        schema: "worldstream/assignment-action-offer-list/v1",
        precondition: AssignmentMcpActionHeadV1 {
            room_seq: snapshot.room_head.room_seq,
            head_hash: snapshot.room_head.genesis_or_transition_hash.clone(),
        },
        offers,
    })
}

/// Submits or reconciles one exact listed Action using stable durable identity.
///
/// # Errors
///
/// Fails closed for invalid/unoffered payloads, altered operation reuse,
/// unavailable durability, ambiguous transport, revoked authority, or invalid
/// daemon data.
pub fn submit_current_action<L, G>(
    authority: &AssignedMembershipAuthorityV1,
    snapshot: &MembershipStreamSnapshotV1,
    schemas: &impl AssignmentMcpActionSchemaSourceV1,
    ledger: &L,
    gateway: &G,
    arguments: AssignmentMcpSubmitActionV1,
) -> Result<AssignmentMcpActionSubmitResultV1, AssignmentMcpActionErrorV1>
where
    L: AssignmentMcpOperationLedgerV1,
    G: AssignmentMcpActionGatewayV1,
{
    if snapshot.room_head.room_id != authority.room_id() {
        return Err(AssignmentMcpActionErrorV1::InvalidOfferData);
    }
    let identity = action_identity(authority.assignment_id(), &arguments.operation_id)?;
    if let Some(record) = ledger.load(&identity).map_err(map_operation_error)? {
        let request = retained_request(authority, &record, &arguments)?;
        return reconcile_record(authority, ledger, gateway, &identity, &record, &request);
    }
    let listed = list_current_action_offers(snapshot, schemas)?;
    if arguments.precondition != listed.precondition {
        return Ok(AssignmentMcpActionSubmitResultV1::RefreshRequired {
            operation_id: arguments.operation_id,
            reason: AssignmentMcpActionRefreshReasonV1::StaleHead,
            current_room_seq: listed.precondition.room_seq,
            current_head_hash: Some(listed.precondition.head_hash),
            requires_new_operation_id: true,
            next_action: "observe_list_then_submit_new_operation",
        });
    }
    let offer = listed
        .offers
        .iter()
        .find(|offer| offer.offer_id == arguments.offer_id)
        .ok_or(AssignmentMcpActionErrorV1::Unoffered)?;
    if !valid_schema_instance(&offer.payload_schema.schema, &arguments.payload, 0) {
        return Err(AssignmentMcpActionErrorV1::InvalidPayload);
    }
    let request = AssignmentMcpExactActionRequestV1 {
        assignment_id: authority.assignment_id().to_owned(),
        operation_id: arguments.operation_id.clone(),
        request_id: arguments.operation_id.clone(),
        action_id: arguments.operation_id.clone(),
        based_on: arguments.precondition,
        offer_id: arguments.offer_id,
        action_type: offer.action_type.clone(),
        payload_schema_digest: offer.payload_schema.schema_digest.clone(),
        eligibility_window: offer.eligibility_window.clone(),
        payload: arguments.payload,
    };
    let intent = AssignmentMcpOperationIntentV1::new_action(
        identity.clone(),
        request.action_id.clone(),
        request.request_id.clone(),
        canonical_bytes(&request)?,
    )
    .map_err(map_operation_error)?;
    let record = ledger.reserve(&intent).map_err(map_operation_error)?;
    reconcile_record(authority, ledger, gateway, &identity, &record, &request)
}

/// Reconciles an Action already durably reserved under its exact stable identity.
///
/// This managed-turn recovery seam never reconstructs an offer, payload, or
/// precondition from a new model call. It resumes only the immutable request
/// retained by the generic Action operation ledger.
///
/// # Errors
///
/// Fails closed when the exact retained request is absent, malformed, or no
/// longer belongs to the sealed assignment authority.
pub fn resume_reserved_action<L, G>(
    authority: &AssignedMembershipAuthorityV1,
    ledger: &L,
    gateway: &G,
    operation_id: &str,
) -> Result<AssignmentMcpActionSubmitResultV1, AssignmentMcpActionErrorV1>
where
    L: AssignmentMcpOperationLedgerV1,
    G: AssignmentMcpActionGatewayV1,
{
    let identity = action_identity(authority.assignment_id(), operation_id)?;
    let record = ledger
        .load(&identity)
        .map_err(map_operation_error)?
        .ok_or(AssignmentMcpActionErrorV1::OperationConflict)?;
    let request: AssignmentMcpExactActionRequestV1 =
        serde_json::from_slice(record.canonical_request())
            .map_err(|_| AssignmentMcpActionErrorV1::InvalidDaemonData)?;
    if request.assignment_id != authority.assignment_id()
        || request.operation_id != operation_id
        || request.request_id != operation_id
        || request.action_id != operation_id
    {
        return Err(AssignmentMcpActionErrorV1::OperationConflict);
    }
    reconcile_record(authority, ledger, gateway, &identity, &record, &request)
}

fn action_identity(
    assignment_id: &str,
    operation_id: &str,
) -> Result<AssignmentMcpOperationIdentityV1, AssignmentMcpActionErrorV1> {
    if assignment_id.parse::<UlidString>().is_err() || operation_id.parse::<UlidString>().is_err() {
        return Err(AssignmentMcpActionErrorV1::InvalidInput);
    }
    AssignmentMcpOperationIdentityV1::new(assignment_id, operation_id).map_err(map_operation_error)
}

fn retained_request(
    authority: &AssignedMembershipAuthorityV1,
    record: &AssignmentMcpOperationRecordV1,
    arguments: &AssignmentMcpSubmitActionV1,
) -> Result<AssignmentMcpExactActionRequestV1, AssignmentMcpActionErrorV1> {
    let request: AssignmentMcpExactActionRequestV1 =
        serde_json::from_slice(record.canonical_request())
            .map_err(|_| AssignmentMcpActionErrorV1::InvalidDaemonData)?;
    if request.operation_id != arguments.operation_id
        || request.offer_id != arguments.offer_id
        || request.based_on != arguments.precondition
        || request.payload != arguments.payload
        || request.assignment_id != record.identity().assignment_id()
        || request.request_id != record.remote_request_id()
        || request.action_id != arguments.operation_id
        || request.assignment_id != authority.assignment_id()
    {
        return Err(AssignmentMcpActionErrorV1::OperationConflict);
    }
    Ok(request)
}

fn reconcile_record<L, G>(
    authority: &AssignedMembershipAuthorityV1,
    ledger: &L,
    gateway: &G,
    identity: &AssignmentMcpOperationIdentityV1,
    record: &AssignmentMcpOperationRecordV1,
    request: &AssignmentMcpExactActionRequestV1,
) -> Result<AssignmentMcpActionSubmitResultV1, AssignmentMcpActionErrorV1>
where
    L: AssignmentMcpOperationLedgerV1,
    G: AssignmentMcpActionGatewayV1,
{
    if let Some(acceptance) = record.remote_acceptance() {
        let result = decode_retained_response(acceptance, request)?;
        if record.phase() != AssignmentMcpOperationPhaseV1::Complete {
            ledger
                .complete(identity, acceptance)
                .map_err(map_operation_error)?;
        }
        return Ok(result);
    }
    let remote = gateway
        .submit_exact(authority, request)
        .map_err(map_gateway_error)?;
    validate_daemon_result(request, &remote)?;
    let response = RetainedActionResponseV1 {
        request_id: request.request_id.clone(),
        action_id: request.action_id.clone(),
        result: remote,
    };
    let outcome = match response.result {
        AssignmentMcpActionDaemonResultV1::Accepted { .. } => {
            AssignmentMcpRemoteOutcomeV1::Accepted
        }
        AssignmentMcpActionDaemonResultV1::Rejected { .. } => {
            AssignmentMcpRemoteOutcomeV1::Rejected
        }
    };
    let acceptance = AssignmentMcpRemoteAcceptanceV1::new_action(
        &request.request_id,
        &request.action_id,
        outcome,
        canonical_bytes(&response)?,
    )
    .map_err(map_operation_error)?;
    ledger
        .record_remote(identity, &acceptance)
        .map_err(map_operation_error)?;
    ledger
        .complete(identity, &acceptance)
        .map_err(map_operation_error)?;
    Ok(response_to_result(response, &request.operation_id))
}

fn decode_retained_response(
    acceptance: &AssignmentMcpRemoteAcceptanceV1,
    request: &AssignmentMcpExactActionRequestV1,
) -> Result<AssignmentMcpActionSubmitResultV1, AssignmentMcpActionErrorV1> {
    let response: RetainedActionResponseV1 =
        serde_json::from_slice(acceptance.canonical_response())
            .map_err(|_| AssignmentMcpActionErrorV1::InvalidDaemonData)?;
    if response.request_id != request.request_id || response.action_id != request.action_id {
        return Err(AssignmentMcpActionErrorV1::InvalidDaemonData);
    }
    validate_daemon_result(request, &response.result)?;
    Ok(response_to_result(response, &request.operation_id))
}

fn response_to_result(
    response: RetainedActionResponseV1,
    operation_id: &str,
) -> AssignmentMcpActionSubmitResultV1 {
    match response.result {
        AssignmentMcpActionDaemonResultV1::Accepted {
            action_id,
            transition_id,
            admitted_at,
            committed_head,
            duplicate,
        } => AssignmentMcpActionSubmitResultV1::Accepted {
            operation_id: operation_id.to_owned(),
            action_id,
            transition_id,
            admitted_at,
            committed_head,
            duplicate,
        },
        AssignmentMcpActionDaemonResultV1::Rejected {
            code,
            current_room_seq,
            ..
        } if matches!(
            code.as_str(),
            "stale_room_state" | "deadline_passed" | "action_not_allowed"
        ) =>
        {
            let reason = match code.as_str() {
                "stale_room_state" => AssignmentMcpActionRefreshReasonV1::StaleHead,
                "deadline_passed" => AssignmentMcpActionRefreshReasonV1::OfferExpired,
                _ => AssignmentMcpActionRefreshReasonV1::OfferUnavailable,
            };
            AssignmentMcpActionSubmitResultV1::RefreshRequired {
                operation_id: operation_id.to_owned(),
                reason,
                current_room_seq,
                current_head_hash: None,
                requires_new_operation_id: true,
                next_action: "observe_list_then_submit_new_operation",
            }
        }
        AssignmentMcpActionDaemonResultV1::Rejected {
            action_id,
            admitted_at,
            code,
            message,
            may_submit_revised_action,
            duplicate,
            details,
            ..
        } => AssignmentMcpActionSubmitResultV1::Rejected {
            operation_id: operation_id.to_owned(),
            action_id,
            admitted_at,
            code,
            message,
            may_submit_revised_action,
            duplicate,
            details,
        },
    }
}

fn validate_daemon_result(
    request: &AssignmentMcpExactActionRequestV1,
    result: &AssignmentMcpActionDaemonResultV1,
) -> Result<(), AssignmentMcpActionErrorV1> {
    match result {
        AssignmentMcpActionDaemonResultV1::Accepted {
            action_id,
            transition_id,
            admitted_at,
            committed_head,
            ..
        } => {
            if action_id != &request.action_id
                || transition_id.parse::<UlidString>().is_err()
                || !bounded(admitted_at)
                || request
                    .based_on
                    .room_seq
                    .checked_add(1)
                    .is_none_or(|expected| committed_head.room_seq != expected)
                || committed_head.head_hash.parse::<Blake3DigestV1>().is_err()
            {
                return Err(AssignmentMcpActionErrorV1::InvalidDaemonData);
            }
        }
        AssignmentMcpActionDaemonResultV1::Rejected {
            action_id,
            admitted_at,
            code,
            message,
            current_room_seq,
            current_action_offers,
            retryable_with_same_action_id,
            may_submit_revised_action,
            ..
        } => {
            if action_id != &request.action_id
                || !bounded(admitted_at)
                || !bounded(code)
                || !bounded(message)
                || current_action_offers.len() > MAX_OFFERS
                || current_action_offers.iter().any(|offer| {
                    offer.domain != ACTION_OFFER_DOMAIN
                        || !bounded(&offer.action_type)
                        || offer
                            .payload_schema_digest
                            .parse::<Blake3DigestV1>()
                            .is_err()
                })
                || matches!(
                    code.as_str(),
                    "stale_room_state" | "deadline_passed" | "action_not_allowed"
                ) && (*retryable_with_same_action_id
                    || !*may_submit_revised_action
                    || match code.as_str() {
                        "stale_room_state" => *current_room_seq <= request.based_on.room_seq,
                        "deadline_passed" | "action_not_allowed" => {
                            *current_room_seq != request.based_on.room_seq
                        }
                        _ => false,
                    })
            {
                return Err(AssignmentMcpActionErrorV1::InvalidDaemonData);
            }
        }
    }
    Ok(())
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, AssignmentMcpActionErrorV1> {
    let encoded =
        serde_json::to_vec(value).map_err(|_| AssignmentMcpActionErrorV1::InvalidInput)?;
    CanonicalJsonV1::parse(&encoded)
        .and_then(|value| value.to_bytes())
        .map_err(|_| AssignmentMcpActionErrorV1::InvalidInput)
}

const fn map_operation_error(error: AssignmentMcpOperationErrorV1) -> AssignmentMcpActionErrorV1 {
    match error {
        AssignmentMcpOperationErrorV1::Conflict => AssignmentMcpActionErrorV1::OperationConflict,
        AssignmentMcpOperationErrorV1::Unavailable
        | AssignmentMcpOperationErrorV1::CapacityExceeded => {
            AssignmentMcpActionErrorV1::OperationUnavailable
        }
        AssignmentMcpOperationErrorV1::InvalidData
        | AssignmentMcpOperationErrorV1::CredentialData
        | AssignmentMcpOperationErrorV1::PrivateData
        | AssignmentMcpOperationErrorV1::NotFound => AssignmentMcpActionErrorV1::InvalidInput,
    }
}

const fn map_gateway_error(error: AssignmentMcpActionGatewayErrorV1) -> AssignmentMcpActionErrorV1 {
    match error {
        AssignmentMcpActionGatewayErrorV1::Ambiguous
        | AssignmentMcpActionGatewayErrorV1::Disconnected
        | AssignmentMcpActionGatewayErrorV1::Unavailable => {
            AssignmentMcpActionErrorV1::AmbiguousRetrySameOperation
        }
        AssignmentMcpActionGatewayErrorV1::Revoked => AssignmentMcpActionErrorV1::AssignmentRevoked,
        AssignmentMcpActionGatewayErrorV1::InvalidData => {
            AssignmentMcpActionErrorV1::InvalidDaemonData
        }
    }
}

const fn map_schema_error(error: AssignmentMcpActionSchemaErrorV1) -> AssignmentMcpActionErrorV1 {
    match error {
        AssignmentMcpActionSchemaErrorV1::Unavailable => {
            AssignmentMcpActionErrorV1::SchemaUnavailable
        }
        AssignmentMcpActionSchemaErrorV1::Missing | AssignmentMcpActionSchemaErrorV1::Invalid => {
            AssignmentMcpActionErrorV1::InvalidOfferData
        }
    }
}

fn valid_schema(schema: &ActivityPackCatalogSchema) -> bool {
    if !bounded(&schema.schema_id) || schema.schema_digest.parse::<Blake3DigestV1>().is_err() {
        return false;
    }
    let Ok(encoded) = serde_json::to_vec(&schema.schema) else {
        return false;
    };
    let Ok(canonical) = CanonicalJsonV1::parse(&encoded) else {
        return false;
    };
    let Ok(bytes) = canonical.to_bytes() else {
        return false;
    };
    Blake3DigestV1::hash(&bytes).to_string() == schema.schema_digest
        && valid_schema_shape(&schema.schema, 0)
}

/// Checks one model-proposed payload against an already verified listed-Action schema.
///
/// This validation-only seam is used by the authority-free managed model host.
/// The assignment helper repeats the complete offer, schema, Head, and
/// participant-authority checks before it submits the Action.
#[must_use]
pub fn action_payload_matches_schema_v1(schema: &Value, payload: &Value) -> bool {
    valid_schema_shape(schema, 0) && valid_schema_instance(schema, payload, 0)
}

/// Checks the bounded JSON-Schema subset accepted for listed Action payloads.
#[must_use]
pub fn action_payload_schema_is_valid_v1(schema: &Value) -> bool {
    valid_schema_shape(schema, 0)
}

fn valid_schema_instance(schema: &Value, instance: &Value, depth: usize) -> bool {
    if depth > MAX_SCHEMA_DEPTH {
        return false;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.get("const").is_some_and(|value| value != instance)
        || object
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| !values.contains(instance))
    {
        return false;
    }
    match object.get("type").and_then(Value::as_str) {
        Some("null") => instance.is_null(),
        Some("boolean") => instance.is_boolean(),
        Some("integer") => {
            let Some(value) = instance.as_i64() else {
                return false;
            };
            integer_keyword(object, "minimum")
                .ok()
                .flatten()
                .is_none_or(|minimum| value >= minimum)
                && integer_keyword(object, "maximum")
                    .ok()
                    .flatten()
                    .is_none_or(|maximum| value <= maximum)
        }
        Some("string") => {
            let Some(value) = instance.as_str() else {
                return false;
            };
            let length = u64::try_from(value.chars().count()).unwrap_or(u64::MAX);
            unsigned_keyword(object, "minLength")
                .ok()
                .flatten()
                .is_none_or(|minimum| length >= minimum)
                && unsigned_keyword(object, "maxLength")
                    .ok()
                    .flatten()
                    .is_none_or(|maximum| length <= maximum)
        }
        Some("array") => {
            let Some(values) = instance.as_array() else {
                return false;
            };
            let length = u64::try_from(values.len()).unwrap_or(u64::MAX);
            unsigned_keyword(object, "minItems")
                .ok()
                .flatten()
                .is_none_or(|minimum| length >= minimum)
                && unsigned_keyword(object, "maxItems")
                    .ok()
                    .flatten()
                    .is_none_or(|maximum| length <= maximum)
                && object.get("items").is_none_or(|items| {
                    values
                        .iter()
                        .all(|value| valid_schema_instance(items, value, depth + 1))
                })
        }
        Some("object") => validate_object_instance(object, instance, depth),
        None => true,
        Some(_) => false,
    }
}

fn validate_object_instance(
    schema: &serde_json::Map<String, Value>,
    instance: &Value,
    depth: usize,
) -> bool {
    let Some(instance) = instance.as_object() else {
        return false;
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    if schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .any(|field| !instance.contains_key(field))
        })
    {
        return false;
    }
    let allow_additional = schema
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    instance.iter().all(|(key, value)| {
        properties
            .and_then(|values| values.get(key))
            .map_or(allow_additional, |field_schema| {
                valid_schema_instance(field_schema, value, depth + 1)
            })
    })
}

#[allow(clippy::too_many_lines)]
fn valid_schema_shape(schema: &serde_json::Value, depth: usize) -> bool {
    if depth > MAX_SCHEMA_DEPTH {
        return false;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object
        .keys()
        .any(|key| !SUPPORTED_SCHEMA_KEYWORDS.contains(&key.as_str()))
        || object
            .get("description")
            .is_some_and(|value| !value.is_string())
    {
        return false;
    }
    let declared_type = object.get("type").and_then(serde_json::Value::as_str);
    if object.contains_key("type")
        && !["null", "boolean", "integer", "string", "array", "object"]
            .contains(&declared_type.unwrap_or_default())
    {
        return false;
    }
    if let Some(values) = object.get("enum") {
        let Some(values) = values.as_array() else {
            return false;
        };
        if values.is_empty()
            || values
                .iter()
                .enumerate()
                .any(|(index, value)| values[..index].contains(value))
        {
            return false;
        }
    }
    let minimum = integer_keyword(object, "minimum");
    let maximum = integer_keyword(object, "maximum");
    if minimum.is_err()
        || maximum.is_err()
        || (minimum.as_ref().ok().and_then(|value| *value).is_some()
            || maximum.as_ref().ok().and_then(|value| *value).is_some())
            && declared_type != Some("integer")
        || minimum
            .ok()
            .flatten()
            .zip(maximum.ok().flatten())
            .is_some_and(|(min, max)| min > max)
    {
        return false;
    }
    let min_length = unsigned_keyword(object, "minLength");
    let max_length = unsigned_keyword(object, "maxLength");
    if invalid_range(&min_length, &max_length)
        || (min_length.as_ref().ok().and_then(|value| *value).is_some()
            || max_length.as_ref().ok().and_then(|value| *value).is_some())
            && declared_type != Some("string")
    {
        return false;
    }
    let min_items = unsigned_keyword(object, "minItems");
    let max_items = unsigned_keyword(object, "maxItems");
    if invalid_range(&min_items, &max_items)
        || (min_items.as_ref().ok().and_then(|value| *value).is_some()
            || max_items.as_ref().ok().and_then(|value| *value).is_some()
            || object.contains_key("items"))
            && declared_type != Some("array")
        || object
            .get("items")
            .is_some_and(|items| !valid_schema_shape(items, depth + 1))
    {
        return false;
    }
    let object_keywords = object.contains_key("properties")
        || object.contains_key("required")
        || object.contains_key("additionalProperties");
    if object_keywords && declared_type != Some("object") {
        return false;
    }
    let properties = match object.get("properties") {
        Some(value) => match value.as_object() {
            Some(properties) => Some(properties),
            None => return false,
        },
        None => None,
    };
    if properties.is_some_and(|properties| {
        properties.iter().any(|(name, child)| {
            name.is_empty() || !valid_schema_shape(child, depth.saturating_add(1))
        })
    }) {
        return false;
    }
    if let Some(required) = object.get("required") {
        let Some(required) = required.as_array() else {
            return false;
        };
        let mut unique = BTreeSet::new();
        if required.iter().any(|key| {
            let Some(key) = key.as_str().filter(|key| !key.is_empty()) else {
                return true;
            };
            !unique.insert(key) || !properties.is_some_and(|values| values.contains_key(key))
        }) {
            return false;
        }
    }
    object
        .get("additionalProperties")
        .is_none_or(serde_json::Value::is_boolean)
}

fn integer_keyword(
    schema: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<i64>, ()> {
    schema
        .get(key)
        .map(|value| value.as_i64().ok_or(()))
        .transpose()
}

fn unsigned_keyword(
    schema: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, ()> {
    schema
        .get(key)
        .map(|value| value.as_u64().ok_or(()))
        .transpose()
}

fn invalid_range(minimum: &Result<Option<u64>, ()>, maximum: &Result<Option<u64>, ()>) -> bool {
    minimum.is_err()
        || maximum.is_err()
        || minimum
            .as_ref()
            .ok()
            .and_then(|value| *value)
            .zip(maximum.as_ref().ok().and_then(|value| *value))
            .is_some_and(|(min, max)| min > max)
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.chars().any(char::is_control)
}
