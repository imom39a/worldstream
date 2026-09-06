//! Public v0.1 room protocol types. These types deliberately contain only
//! wire data; authority grants, Core state, and storage objects stay private.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{UlidString, envelope::MAX_ACTION_PAYLOAD_BYTES};

/// Version of the short-lived browser WebSocket admission response.
pub const BROWSER_WS_TICKET_VERSION: &str = "browser_ws_ticket.v1";

/// Version of the Host-internal target-bound browser ticket request.
pub const HOSTED_BROWSER_WS_TICKET_VERSION: &str = "hosted_browser_ws_ticket.v1";

/// Version of the Host-internal Browser Activity Session revocation request.
pub const HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION: &str = "hosted_browser_ws_session_revoke.v1";

/// One-time browser WebSocket admission response.
///
/// The ticket is an in-memory transport admission value, not a capability
/// bearer. It is deliberately redacted from diagnostics by its Debug
/// implementation in the gateway response wrapper.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWebSocketTicketIssueResponse {
    pub version: String,
    pub ticket: String,
    pub expires_in_ms: u64,
}

impl std::fmt::Debug for BrowserWebSocketTicketIssueResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserWebSocketTicketIssueResponse")
            .field("version", &self.version)
            .field("ticket", &"[REDACTED]")
            .field("expires_in_ms", &self.expires_in_ms)
            .finish()
    }
}

/// Host-internal request that binds a one-use browser ticket to one exact
/// Membership. This value is never accepted from browser JavaScript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedBrowserWebSocketTicketIssueRequest {
    pub version: String,
    pub room_id: String,
    pub member_id: String,
    pub mode: ClientMode,
    pub after_frame_seq: Option<u64>,
    pub browser_session_digest: String,
    pub client_release_digest: String,
    pub client_surface_id: String,
}

/// Host-internal request that retires pending tickets and an active stream for
/// one Browser Activity Session without carrying the session secret.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedBrowserWebSocketSessionRevokeRequest {
    pub version: String,
    pub browser_session_digest: String,
}

/// Capabilities required before a client can cross the attach/sync barrier.
/// Keeping this list in the protocol crate makes negotiation deterministic for
/// every gateway implementation and SDK.
pub const REQUIRED_CLIENT_CAPABILITIES: &[&str] = &["cursor_ack", "projection_reset"];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    pub client_name: String,
    pub client_version: String,
    pub mode: ClientMode,
    pub supported_protocols: Vec<String>,
    pub capabilities: Vec<String>,
}

impl ClientHello {
    /// Returns the first required capability that this client does not offer.
    #[must_use]
    pub fn missing_required_capability(&self) -> Option<&'static str> {
        REQUIRED_CLIENT_CAPABILITIES
            .iter()
            .find(|required| !self.capabilities.iter().any(|actual| actual == **required))
            .copied()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientMode {
    Participant,
    Spectator,
    Operator,
    Runner,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAttach {
    pub room_id: String,
    pub member_id: String,
    pub after_frame_seq: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSyncAck {
    pub room_id: String,
    pub member_id: String,
    pub through_frame_head: u64,
    pub sync_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationAck {
    pub room_id: String,
    pub member_id: String,
    pub through_frame_seq: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSubmit {
    pub room_id: String,
    pub member_id: String,
    pub action_id: String,
    pub based_on_room_seq: u64,
    pub action_type: String,
    pub payload: Value,
}

impl ActionSubmit {
    /// Validates the transport-side bounds before a backend is called.
    ///
    /// # Errors
    ///
    /// Returns an error when the action type is empty or exceeds 256 bytes,
    /// when the payload cannot be encoded as JSON, or when the encoded payload
    /// exceeds [`MAX_ACTION_PAYLOAD_BYTES`].
    pub fn validate_bounds(&self) -> Result<(), &'static str> {
        if self.room_id.is_empty() || self.member_id.is_empty() || self.action_id.is_empty() {
            return Err("action identity fields must not be empty");
        }
        if self.action_type.is_empty() || self.action_type.len() > 256 {
            return Err("action_type exceeds its bound");
        }
        let bytes = serde_json::to_vec(&self.payload).map_err(|_| "payload is not JSON")?;
        if bytes.len() > MAX_ACTION_PAYLOAD_BYTES {
            return Err("payload exceeds its bound");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomHead {
    pub room_id: String,
    pub room_seq: u64,
    pub genesis_or_transition_hash: String,
    pub core_schema_version: String,
    pub pack_digest: String,
    pub core_state_hash: String,
    pub activity_state_hash: String,
    pub authoritative_state_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionOffer {
    pub domain: String,
    pub action_type: String,
    pub payload_schema_digest: String,
    pub eligibility_window: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub core: Value,
    pub activity: Value,
    pub action_offers: Vec<ActionOffer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoomRequest {
    pub pack: PackReference,
    pub configuration: Value,
    pub members: Vec<CreateMember>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackReference {
    pub id: String,
    pub version: String,
    pub digest: String,
}

/// Version of the bounded host-authorized Activity Pack catalog response.
pub const ACTIVITY_PACK_CATALOG_VERSION: &str = "activity_pack_catalog.v1";

/// One exact installed revision in the catalog list.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogRevisionSummary {
    pub pack: PackReference,
    pub name: String,
    pub selectable_for_new_rooms: bool,
    pub runnable_for_retained_rooms: bool,
}

/// Complete bounded list of revisions compiled into the running daemon.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogResponse {
    pub version: String,
    pub revisions: Vec<ActivityPackCatalogRevisionSummary>,
}

/// One exact canonical JSON schema document from an installed revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogSchema {
    pub schema_id: String,
    pub schema_digest: String,
    pub schema: Value,
}

/// One descriptor-declared Role and its supported assignment cardinality.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogRole {
    pub role: String,
    pub minimum: u32,
    pub maximum: u32,
}

/// One descriptor-declared Action and exact payload schema document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogAction {
    pub action_type: String,
    pub payload_schema: ActivityPackCatalogSchema,
}

/// Optional Lobby contract declared by an exact Activity Pack revision.
/// Absence means the revision declares no Lobby compatibility; it must not be
/// inferred from another revision or from the pack name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackLobbyCompatibility {
    pub contract: String,
    pub configuration_schema: ActivityPackCatalogSchema,
}

/// Full detail for one exact installed Activity Pack revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogRevisionDetail {
    pub summary: ActivityPackCatalogRevisionSummary,
    pub roles: Vec<ActivityPackCatalogRole>,
    pub configuration_schema: ActivityPackCatalogSchema,
    pub actions: Vec<ActivityPackCatalogAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lobby_compatibility: Option<ActivityPackLobbyCompatibility>,
}

/// Versioned exact-revision detail response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackCatalogRevisionResponse {
    pub version: String,
    pub revision: ActivityPackCatalogRevisionDetail,
}

/// Maximum number of Rooms returned by one host-operator inventory page.
pub const MAX_OPERATOR_ROOM_PAGE_SIZE: usize = 100;
/// Default host-operator Room inventory page size.
pub const DEFAULT_OPERATOR_ROOM_PAGE_SIZE: usize = 50;

/// Request passed from the bounded HTTP query parser to a storage backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorRoomInventoryRequest {
    pub after_room_id: Option<String>,
    pub limit: usize,
}

/// Durable Room Integrity State exposed independently from Activity Phase and
/// data freshness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRoomIntegrityStatus {
    Healthy,
    Faulted,
    Quarantined,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorRoomIntegrity {
    pub status: OperatorRoomIntegrityStatus,
    pub generation: u64,
}

/// Activity Phase availability at the host diagnostic boundary. The host
/// capability is not an Operator Membership, so callers must not infer a
/// phase from canonical Activity State or participant-private projections.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperatorActivityPhase {
    Available { value: String },
    Unavailable { reason: String },
}

/// Freshness is represented separately from setup, Activity Phase, and Room
/// Integrity so a consumer never collapses operational axes into one status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperatorDataFreshness {
    Fresh { observed_at: String },
    Stale { observed_at: String, reason: String },
    Unavailable { reason: String },
}

/// Privacy-bounded host-operator Room inventory row and detail representation.
/// It intentionally carries no Membership, participant-private Projection,
/// Invocation, Activation, cursor, or delivery data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorRoomSummary {
    pub room_id: String,
    pub room_head: RoomHead,
    pub pack: PackReference,
    pub integrity: OperatorRoomIntegrity,
    pub activity_phase: OperatorActivityPhase,
    pub freshness: OperatorDataFreshness,
}

/// One deterministic Room-ID-ordered inventory page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorRoomInventoryPage {
    pub rooms: Vec<OperatorRoomSummary>,
    pub next_after_room_id: Option<String>,
}

/// Connection state of one exact provisioned Runner identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRunnerConnectionV1 {
    Connected,
    Disconnected,
}

/// Freshness of the daemon's last authenticated Runner observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRunnerFreshnessV1 {
    Fresh,
    Stale,
}

/// Host-authorized, secret-free presence and capacity for one exact Runner.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorRunnerPresenceV1 {
    pub version: String,
    pub runner_id: String,
    pub connection: OperatorRunnerConnectionV1,
    pub freshness: OperatorRunnerFreshnessV1,
    pub maximum_concurrent_activations: u32,
    pub active_activations: u32,
    pub available_activations: u32,
    pub supported_pack_revisions: Vec<PackReference>,
    pub observed_at_unix_ms: u64,
}

/// Host-authorized, participant-private-free Activation state counts for one
/// exact Room Membership at the instant durable storage is queried.
pub const OPERATOR_ACTIVATION_STATUS_VERSION: &str = "worldstream/operator-activation-status/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorActivationStatusV1 {
    pub version: String,
    pub waiting: u32,
    pub leased: u32,
    pub observed_at_unix_ms: u64,
}

/// Durable storage profile relevant to live-backup capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorBackupStorageProfile {
    SqliteBundled,
    PostgresPrimary,
    Ephemeral,
}

/// Storage health is independent from backup verification and freshness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorBackupStorageHealth {
    Healthy,
    Unhealthy,
    Unavailable,
}

/// Closed verification result for an exact backup artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorBackupVerification {
    Pass,
    Failed,
    Unavailable,
}

/// Profile-level health and support, queried without starting an operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorBackupProfileStatus {
    pub storage_profile: OperatorBackupStorageProfile,
    pub storage_health: OperatorBackupStorageHealth,
    pub live_backup_supported: bool,
    pub verification: OperatorBackupVerification,
    pub freshness: OperatorDataFreshness,
}

/// Internal Host-authorized preparation request. The daemon derives the
/// destination from its configured backup root and this stable operation ID;
/// callers cannot supply a filesystem path.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorLiveBackupPrepareRequest {
    pub operation_id: String,
}

/// Pathless summary of one exact verified native artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorLiveBackupArtifactSummary {
    pub artifact_name: String,
    pub byte_length: u64,
    pub blake3_digest: String,
    pub semantic_digest: String,
}

/// Terminal result from a bounded daemon-side backup preparation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorLiveBackupStatus {
    pub operation_id: String,
    pub storage_profile: OperatorBackupStorageProfile,
    pub storage_health: OperatorBackupStorageHealth,
    pub native_verification: OperatorBackupVerification,
    pub semantic_verification: OperatorBackupVerification,
    pub freshness: OperatorDataFreshness,
    pub artifact: Option<OperatorLiveBackupArtifactSummary>,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMember {
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub role: Option<String>,
    pub access_mode: AccessMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Agent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    Participant,
    Spectator,
    Operator,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerWelcome {
    pub session_id: UlidString,
    pub selected_protocol: String,
    pub server_version: String,
    pub heartbeat_interval_ms: u64,
    pub maximum_message_bytes: usize,
    pub authenticated_principal: Principal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub principal_id: String,
    pub kind: PrincipalKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAttached {
    pub room_id: String,
    pub member_id: String,
    pub principal_kind: PrincipalKind,
    pub access_mode: AccessMode,
    pub role: Option<String>,
    pub membership_status: String,
    pub room_status: String,
    pub room_health: String,
    pub integrity_generation: u64,
    pub room_head: RoomHead,
    pub cursor: Option<u64>,
    pub frame_head: u64,
    pub retained_floor: u64,
    pub sync_token: String,
    pub sync: SyncBranch,
    pub pack: PackReference,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum SyncBranch {
    RetainedFrames {
        cursor_exclusive: u64,
        through_frame_head: u64,
    },
    ProjectionReset {
        baseline_frame_head: u64,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReset {
    pub room_id: String,
    pub member_id: String,
    pub room_head: RoomHead,
    pub room_health: String,
    pub integrity_generation: u64,
    pub baseline_frame_head: u64,
    pub reset_reason: String,
    pub projection_schema: String,
    pub projection: Projection,
    pub projection_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationDeliver {
    pub room_id: String,
    pub member_id: String,
    pub frame_seq: u64,
    pub cause_room_seq: u64,
    pub frame_kind: String,
    pub observation_schema: String,
    pub observation: Value,
    pub frame_payload_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionAccepted {
    pub room_id: String,
    pub member_id: String,
    pub action_id: String,
    pub transition_id: String,
    pub admitted_at: String,
    pub room_head: RoomHead,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRejected {
    pub room_id: String,
    pub member_id: String,
    pub action_id: String,
    pub admitted_at: String,
    pub code: String,
    pub message: String,
    pub current_room_seq: u64,
    pub action_offers: Vec<ActionOffer>,
    pub retryable_with_same_action_id: bool,
    pub may_submit_revised_action: bool,
    pub duplicate: bool,
    pub details: Value,
}

/// Host/operator request to consume one exact Timer generation. The durable
/// Timer row supplies the schedule and payload; neither is accepted from or
/// returned to the wire.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFireRequest {
    pub timer_id: String,
    pub generation: u64,
}

impl TimerFireRequest {
    /// Validates bounded transport fields before the backend loads the exact
    /// durable Timer witness.
    ///
    /// # Errors
    ///
    /// Returns an error when the Timer identity is empty/oversized or the
    /// generation is zero.
    pub fn validate_bounds(&self) -> Result<(), &'static str> {
        if self.timer_id.is_empty() || self.timer_id.len() > 256 {
            return Err("timer_id exceeds its bound");
        }
        if self.generation == 0 {
            return Err("generation must be positive");
        }
        Ok(())
    }
}

/// Safe public result of one operator Timer transition. The response carries
/// only the committed identity/head and never the Timer payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFireResponse {
    pub room_id: String,
    pub timer_id: String,
    pub generation: u64,
    pub transition_id: String,
    pub room_head: RoomHead,
    pub duplicate: bool,
}

/// Bounded host request to launch one Activity-defined Lobby.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LobbyLaunchRequest {
    pub input_id: String,
    pub based_on_room_seq: u64,
}

/// Safe result of one recorded Lobby launch `ExternalInput`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LobbyLaunchResponse {
    pub room_id: String,
    pub input_id: String,
    pub transition_id: String,
    pub room_head: RoomHead,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoomResponse {
    pub room_id: String,
    pub member_ids: Vec<String>,
    pub room_head: RoomHead,
}

pub const HOSTED_ROOM_CREATION_SCHEMA_V2: &str = "worldstream/hosted-room-creation/v2";
pub const HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2: &str =
    "worldstream/hosted-room-creation-response/v2";

/// Closed non-playing purpose accepted by the hosted pre-Genesis operation.
/// The Runtime derives authority from this value; callers cannot submit scopes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedSpectatorPurposeV2 {
    ResultIndexer,
    Creator,
    PublicRelay,
}

impl HostedSpectatorPurposeV2 {
    #[must_use]
    pub const fn principal_kind(self) -> PrincipalKind {
        match self {
            Self::Creator => PrincipalKind::Human,
            Self::ResultIndexer | Self::PublicRelay => PrincipalKind::Agent,
        }
    }

    #[must_use]
    pub const fn capability_scopes(self) -> &'static [&'static str] {
        match self {
            Self::ResultIndexer => &["room:attach", "room:observe_public", "room:replay"],
            Self::Creator | Self::PublicRelay => &["room:attach", "room:observe_public"],
        }
    }
}

/// One caller-sealed spectator credential installed in the Room-Genesis
/// transaction. Scope, Access Mode, and Role are derived by the Runtime.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedSpectatorCredentialInputV2 {
    pub purpose: HostedSpectatorPurposeV2,
    pub member_index: u16,
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub capability: SealedCapabilityInputV1,
}

/// Narrow all-or-nothing hosted Room setup request.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedRoomCreationRequestV2 {
    pub schema: String,
    pub room: CreateRoomRequest,
    pub spectators: Vec<HostedSpectatorCredentialInputV2>,
}

/// Secret-free proof of one spectator capability committed with Genesis.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedSpectatorCredentialReceiptV2 {
    pub purpose: HostedSpectatorPurposeV2,
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub capability_id: String,
    pub scopes: Vec<String>,
}

/// Atomic Room-Genesis and spectator-credential receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedRoomCreationResponseV2 {
    pub schema: String,
    pub room: CreateRoomResponse,
    pub spectators: Vec<HostedSpectatorCredentialReceiptV2>,
}

/// Caller-sealed input for an idempotently registered Capability.
///
/// The bearer is delivered to the daemon only over the authenticated local
/// operator channel. The daemon persists only its hash; exact retries reuse
/// the same capability and authority-change identities.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedCapabilityInputV1 {
    pub capability_id: String,
    pub capability_idempotency_key: String,
    pub bearer: crate::SealedCapabilityBearerV1,
}

impl std::fmt::Debug for SealedCapabilityInputV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedCapabilityInputV1")
            .field("capability_id", &self.capability_id)
            .field(
                "capability_idempotency_key",
                &self.capability_idempotency_key,
            )
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

/// Host-authorized, exactly retryable member-Capability provisioning input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberCapabilityProvisionRequestV1 {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    /// Participants carry a pack Role; spectator and operator Memberships do
    /// not. `Some` preserves the v1 participant wire representation.
    pub role: Option<String>,
    pub access_mode: AccessMode,
    pub scopes: Vec<String>,
    pub capability: SealedCapabilityInputV1,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Secret-free receipt for one provisioned member Capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberCapabilityProvisionResponseV1 {
    pub capability_id: String,
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub scopes: Vec<String>,
}

/// One public Room/Membership target for a sealed Runner Capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerMembershipProvisionTargetV1 {
    pub room_id: String,
    pub member_id: String,
}

/// Host-authorized, exactly retryable Runner-control provisioning input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerCapabilityProvisionRequestV1 {
    pub runner_id: String,
    pub owner_principal_id: String,
    pub permitted_memberships: Vec<RunnerMembershipProvisionTargetV1>,
    pub scopes: Vec<String>,
    pub principal_idempotency_key: String,
    pub runner_idempotency_key: String,
    pub capability: SealedCapabilityInputV1,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Secret-free receipt for one provisioned Runner-control Capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerCapabilityProvisionResponseV1 {
    pub capability_id: String,
    pub runner_id: String,
    pub owner_principal_id: String,
    pub permitted_memberships: Vec<RunnerMembershipProvisionTargetV1>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionResponse {
    pub room_id: String,
    pub room_head: RoomHead,
    pub room_health: String,
    pub integrity_generation: u64,
    pub projection_schema: String,
    pub projection: Projection,
    pub projection_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayResponse {
    pub room_id: String,
    pub pack: PackReference,
    pub requested_room_seq: u64,
    pub room_head: RoomHead,
    pub projection: Projection,
    pub projection_hash: String,
    pub verification: String,
    pub room_health: String,
    pub integrity_generation: u64,
}

/// Operational capacity declared by a Runner control connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerHello {
    pub runner_id: String,
    pub maximum_concurrent_activations: u32,
    pub supported_pack_ids: Vec<String>,
    /// Exact immutable revisions supported by this connection. Older clients
    /// deserialize with an empty set and therefore cannot satisfy an exact
    /// launch-readiness check.
    #[serde(default)]
    pub supported_pack_revisions: Vec<PackReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerReady {
    pub runner_id: String,
}

/// A Runner asks for pending offers for one authorized Agent Membership.
/// The server never returns private projection bytes in this response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOfferRequest {
    pub operation_id: String,
    pub runner_id: String,
    pub room_id: String,
    pub member_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOffer {
    pub activation_id: String,
    pub room_id: String,
    pub member_id: String,
    pub cause_room_seq: u64,
    pub reason_code: String,
    pub priority: u64,
    pub deadline: Option<String>,
    pub lease_duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOffers {
    pub operation_id: String,
    pub runner_id: String,
    pub offers: Vec<ActivationOffer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationClaim {
    pub activation_id: String,
    pub runner_id: String,
    pub claim_id: String,
    pub requested_lease_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationLeaseOperation {
    pub activation_id: String,
    pub runner_id: String,
    pub claim_id: String,
    pub operation_id: String,
    pub lease_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_lease_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationResultCode {
    Granted,
    Renewed,
    Released,
    Completed,
    NotAvailable,
    Expired,
    Cancelled,
    Fenced,
    StaleLease,
    IdempotencyConflict,
    ResultRetired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationIntentState {
    Pending,
    Leased,
    Completed,
    Expired,
    Cancelled,
}

/// Wire representation of the result of a durable Activation operation.
/// `context` is populated only for a successful claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOperationReply {
    pub operation_id: String,
    pub activation_id: Option<String>,
    pub claim_id: Option<String>,
    pub runner_id: String,
    pub code: ActivationResultCode,
    pub state: Option<ActivationIntentState>,
    pub lease_generation: Option<u64>,
    pub context_hash: Option<String>,
    pub context: Option<ActivationInvocationContext>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationInvocationContext {
    pub activation_id: String,
    pub claim_id: String,
    pub cause_room_seq: u64,
    pub reason_code: String,
    pub lease_generation: u64,
    pub lease_until: String,
    pub deadline: Option<String>,
    pub room_head: RoomHead,
    pub integrity_generation: u64,
    pub policy_revision: u64,
    pub authority_generation: u64,
    pub membership_generation: u64,
    pub frame_head: u64,
    pub retained_floor: u64,
    pub cursor: Option<u64>,
    pub projection_schema: String,
    pub projection: Value,
    pub action_offers: Vec<ActionOffer>,
    pub runner_budget: Value,
    pub runner_limits: Value,
    pub artifact_references: Vec<Value>,
    pub delivery: ActivationDelivery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationDelivery {
    RetainedFrames {
        cursor_exclusive: u64,
        through_frame_head: u64,
        frames: Vec<ActivationFrame>,
    },
    ProjectionReset {
        baseline_frame_head: u64,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationFrame {
    pub frame_seq: u64,
    pub cause_room_seq: u64,
    pub payload_hash: String,
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use super::{ActionOffer, ActionSubmit, ClientHello, ClientMode};
    use serde_json::json;

    #[test]
    fn action_offer_preserves_absent_eligibility_as_canonical_null() {
        let offer = ActionOffer {
            domain: "worldstream/action-offer/v1".to_owned(),
            action_type: "inspect".to_owned(),
            payload_schema_digest: "blake3:fixture".to_owned(),
            eligibility_window: None,
        };
        let value = serde_json::to_value(offer)
            .unwrap_or_else(|error| unreachable!("action offer JSON: {error}"));
        assert_eq!(value["eligibility_window"], serde_json::Value::Null);
        assert!(value.as_object().is_some_and(|object| object.len() == 4));
    }

    #[test]
    fn action_payload_bound_is_checked_before_backend_admission() {
        let action = ActionSubmit {
            room_id: "room".to_owned(),
            member_id: "member".to_owned(),
            action_id: "action".to_owned(),
            based_on_room_seq: 0,
            action_type: "increment".to_owned(),
            payload: json!("x".repeat(super::MAX_ACTION_PAYLOAD_BYTES)),
        };
        assert!(action.validate_bounds().is_err());
    }

    #[test]
    fn hello_requires_cursor_and_reset_capabilities() {
        let hello = ClientHello {
            client_name: "test".to_owned(),
            client_version: "0.1".to_owned(),
            mode: ClientMode::Participant,
            supported_protocols: vec!["0.1".to_owned()],
            capabilities: vec!["cursor_ack".to_owned()],
        };
        assert_eq!(
            hello.missing_required_capability(),
            Some("projection_reset")
        );
    }

    #[test]
    fn action_identity_fields_are_required_before_admission() {
        let action = ActionSubmit {
            room_id: String::new(),
            member_id: "member".to_owned(),
            action_id: "action".to_owned(),
            based_on_room_seq: 0,
            action_type: "increment".to_owned(),
            payload: serde_json::json!({}),
        };
        assert_eq!(
            action.validate_bounds(),
            Err("action identity fields must not be empty")
        );
    }

    #[test]
    fn server_welcome_requires_a_canonical_session_id() {
        let valid = serde_json::json!({
            "session_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "selected_protocol": "0.1",
            "server_version": "0.1.0",
            "heartbeat_interval_ms": 1000,
            "maximum_message_bytes": 512,
            "authenticated_principal": {
                "principal_id": "principal",
                "kind": "human"
            }
        });
        let welcome: super::ServerWelcome = serde_json::from_value(valid)
            .unwrap_or_else(|error| unreachable!("canonical session identity: {error}"));
        assert_eq!(
            welcome.session_id,
            "01ARZ3NDEKTSV4RRFFQ69G5FAV"
                .parse()
                .unwrap_or_else(|error| unreachable!("ULID: {error}"))
        );

        let invalid = serde_json::json!({
            "session_id": "not-a-session",
            "selected_protocol": "0.1",
            "server_version": "0.1.0",
            "heartbeat_interval_ms": 1000,
            "maximum_message_bytes": 512,
            "authenticated_principal": {
                "principal_id": "principal",
                "kind": "human"
            }
        });
        assert!(serde_json::from_value::<super::ServerWelcome>(invalid).is_err());
    }

    #[test]
    fn operator_room_summary_keeps_phase_integrity_and_freshness_independent() {
        let summary: super::OperatorRoomSummary = serde_json::from_value(serde_json::json!({
            "room_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "room_head": {
                "room_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "room_seq": 7,
                "genesis_or_transition_hash": "blake3:lineage",
                "core_schema_version": "worldstream.core-room-state.v1",
                "pack_digest": "blake3:pack",
                "core_state_hash": "blake3:core",
                "activity_state_hash": "blake3:activity",
                "authoritative_state_hash": "blake3:authoritative"
            },
            "pack": {
                "id": "worldstream.counter",
                "version": "1.0.0",
                "digest": "blake3:pack"
            },
            "integrity": { "status": "faulted", "generation": 4 },
            "activity_phase": {
                "status": "unavailable",
                "reason": "operator_membership_required"
            },
            "freshness": {
                "status": "stale",
                "observed_at": "2026-08-23T20:00:00Z",
                "reason": "daemon_reconciliation_pending"
            }
        }))
        .unwrap_or_else(|error| unreachable!("operator summary: {error}"));

        assert!(matches!(
            summary.activity_phase,
            super::OperatorActivityPhase::Unavailable { .. }
        ));
        assert_eq!(
            summary.integrity.status,
            super::OperatorRoomIntegrityStatus::Faulted
        );
        assert!(matches!(
            summary.freshness,
            super::OperatorDataFreshness::Stale { .. }
        ));
    }

    #[test]
    fn operator_activation_status_rejects_private_or_unknown_fields() {
        let exact = serde_json::json!({
            "version": "worldstream/operator-activation-status/v1",
            "waiting": 2,
            "leased": 1,
            "observed_at_unix_ms": 1_777_000_000_000_u64
        });
        let status: super::OperatorActivationStatusV1 = serde_json::from_value(exact.clone())
            .unwrap_or_else(|error| unreachable!("bounded activation status: {error}"));
        assert_eq!(status.waiting, 2);
        assert_eq!(status.leased, 1);

        for private_field in ["activation_id", "claim_id", "context", "member_id"] {
            let mut with_private = exact.clone();
            with_private
                .as_object_mut()
                .unwrap_or_else(|| unreachable!("object fixture"))
                .insert(private_field.to_owned(), serde_json::json!("private"));
            assert!(
                serde_json::from_value::<super::OperatorActivationStatusV1>(with_private).is_err(),
                "accepted private field {private_field}"
            );
        }
        let mut overflow = exact;
        overflow
            .as_object_mut()
            .unwrap_or_else(|| unreachable!("object fixture"))
            .insert(
                "waiting".to_owned(),
                serde_json::json!(u64::from(u32::MAX) + 1),
            );
        assert!(serde_json::from_value::<super::OperatorActivationStatusV1>(overflow).is_err());
    }
}
