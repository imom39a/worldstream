//! Assignment-bound Runner Activation tools for the local MCP helper.
//!
//! This module deliberately owns no participant observation or Action client.
//! Its sealed authority contains exactly one Runner and one permitted
//! Membership, and the public tool arguments contain no scope or credential.

use std::{
    fmt,
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::body::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use tungstenite::handshake::client::generate_key;
use tungstenite::{Message, WebSocket, client, http};
use worldstream_core::{
    ActivationContextInputV1, ActivationDeliveryV1 as CoreActivationDeliveryV1,
    ActivationFrameV1 as CoreActivationFrameV1, Blake3DigestV1, CanonicalJsonV1, CompleteHeadV1,
    RoomSequenceV1, TimerScheduledFor, prepare_activation_context,
};
use worldstream_protocol::{
    ActivationClaim, ActivationIntentState, ActivationInvocationContext, ActivationLeaseOperation,
    ActivationOffer, ActivationOfferRequest, ActivationOffers, ActivationOperationReply,
    ActivationResultCode, BearerWireV1, ClientHello, ClientMode, ErrorCode, MAX_MESSAGE_BYTES,
    PROTOCOL_VERSION, PackReference, PrincipalKind, ProtocolErrorBody,
    REQUIRED_CLIENT_CAPABILITIES, RunnerHello, RunnerReady, SealedCapabilityBearerV1,
    ServerWelcome, UlidString, VersionedEnvelope, WEBSOCKET_SUBPROTOCOL, decode_envelope,
};
use zeroize::{Zeroize, Zeroizing};

const MAX_LABEL_BYTES: usize = 256;
const MAX_DISPOSITION_BYTES: usize = 128;
const MAX_RUNNER_SESSION_BYTES: usize = 4 * 1024 * 1024;
const MAX_RUNNER_SESSION_MESSAGES: usize = 64;
const MAX_LEASE_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;

/// Sealed Runner-control authority for one exact assignment Membership.
///
/// This type intentionally does not implement `Serialize` or `Clone`; the
/// bearer cannot be copied into MCP results, browser DTOs, or logs.
pub struct AssignedRunnerActivationAuthorityV1 {
    assignment_id: String,
    principal_id: String,
    runner_id: String,
    room_id: String,
    member_id: String,
    pack: PackReference,
    bearer: SealedCapabilityBearerV1,
}

impl AssignedRunnerActivationAuthorityV1 {
    /// Constructs one Supervisor-resolved Runner-control context.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities or an unbounded exact Pack reference.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        assignment_id: &str,
        principal_id: &str,
        runner_id: &str,
        room_id: &str,
        member_id: &str,
        pack: PackReference,
        bearer: SealedCapabilityBearerV1,
    ) -> Result<Self, ActivationAuthorityErrorV1> {
        if [assignment_id, principal_id, runner_id, room_id, member_id]
            .into_iter()
            .any(|value| value.parse::<UlidString>().is_err())
            || !bounded_label(&pack.id)
            || !bounded_label(&pack.version)
            || !valid_digest(&pack.digest)
        {
            return Err(ActivationAuthorityErrorV1::Invalid);
        }
        Ok(Self {
            assignment_id: assignment_id.to_owned(),
            principal_id: principal_id.to_owned(),
            runner_id: runner_id.to_owned(),
            room_id: room_id.to_owned(),
            member_id: member_id.to_owned(),
            pack,
            bearer,
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    #[must_use]
    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    #[must_use]
    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    #[must_use]
    pub fn member_id(&self) -> &str {
        &self.member_id
    }

    #[must_use]
    pub const fn pack(&self) -> &PackReference {
        &self.pack
    }

    #[must_use]
    const fn bearer(&self) -> &SealedCapabilityBearerV1 {
        &self.bearer
    }
}

impl fmt::Debug for AssignedRunnerActivationAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignedRunnerActivationAuthorityV1")
            .field("assignment_id", &self.assignment_id)
            .field("runner_id", &self.runner_id)
            .field("room_id", &"[SEALED]")
            .field("member_id", &"[SEALED]")
            .field("bearer", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActivationAuthorityErrorV1 {
    #[error("the assigned Runner authority is invalid")]
    Invalid,
}

/// Private Activation lease delivered only through the assigned agent tool.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignedActivationLeaseV1 {
    assignment_id: String,
    pub activation_cursor: u64,
    activation_id: String,
    claim_id: String,
    context_hash: String,
    pub context: ActivationInvocationContext,
}

impl AssignedActivationLeaseV1 {
    /// Binds daemon context to the exact durable acquisition operation.
    ///
    /// # Errors
    ///
    /// Rejects mismatched context identities, invalid generations, or an
    /// invalid context digest.
    pub fn new(
        assignment_id: &str,
        activation_cursor: u64,
        activation_id: &str,
        claim_id: &str,
        context_hash: &str,
        context: ActivationInvocationContext,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        if assignment_id.parse::<UlidString>().is_err()
            || activation_cursor == 0
            || activation_id.parse::<UlidString>().is_err()
            || claim_id.parse::<UlidString>().is_err()
            || !valid_digest(context_hash)
            || context.activation_id != activation_id
            || context.claim_id != claim_id
            || context.lease_generation == 0
            || !bounded_label(&context.lease_until)
            || context.retained_floor > context.frame_head
            || context
                .cursor
                .is_some_and(|cursor| cursor > context.frame_head)
            || canonical_activation_context_hash_v1(&context).as_deref() != Ok(context_hash)
        {
            return Err(ActivationLedgerErrorV1::InvalidData);
        }
        Ok(Self {
            assignment_id: assignment_id.to_owned(),
            activation_cursor,
            activation_id: activation_id.to_owned(),
            claim_id: claim_id.to_owned(),
            context_hash: context_hash.to_owned(),
            context,
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub fn activation_id(&self) -> &str {
        &self.activation_id
    }

    #[must_use]
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    #[must_use]
    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }

    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.context.lease_generation
    }

    #[must_use]
    pub fn lease_until(&self) -> &str {
        &self.context.lease_until
    }
}

impl fmt::Debug for AssignedActivationLeaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignedActivationLeaseV1")
            .field("assignment_id", &self.assignment_id)
            .field("activation_cursor", &self.activation_cursor)
            .field("activation_id", &self.activation_id)
            .field("claim_id", &self.claim_id)
            .field("context_hash", &self.context_hash)
            .field("context", &"[PRIVATE ACTIVATION INPUT]")
            .finish()
    }
}

/// Stable durable preparation for offer discovery and claim retry.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedActivationAcquireV1 {
    assignment_id: String,
    activation_cursor: u64,
    offer_operation_id: String,
    claim_id: String,
    selected_offer: Option<ActivationOffer>,
}

impl PreparedActivationAcquireV1 {
    /// Constructs the stable identities persisted before daemon I/O.
    ///
    /// # Errors
    ///
    /// Rejects malformed operation identities or a zero cursor.
    pub fn new(
        assignment_id: &str,
        activation_cursor: u64,
        offer_operation_id: &str,
        claim_id: &str,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        if assignment_id.parse::<UlidString>().is_err()
            || activation_cursor == 0
            || offer_operation_id.parse::<UlidString>().is_err()
            || claim_id.parse::<UlidString>().is_err()
        {
            return Err(ActivationLedgerErrorV1::InvalidData);
        }
        Ok(Self {
            assignment_id: assignment_id.to_owned(),
            activation_cursor,
            offer_operation_id: offer_operation_id.to_owned(),
            claim_id: claim_id.to_owned(),
            selected_offer: None,
        })
    }

    /// Binds the chosen daemon offer before the first claim attempt.
    ///
    /// # Errors
    ///
    /// Rejects a malformed offer or an attempt to replace a retained choice.
    pub fn with_selected_offer(
        mut self,
        offer: ActivationOffer,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        if self
            .selected_offer
            .as_ref()
            .is_some_and(|retained| retained != &offer)
            || offer.activation_id.parse::<UlidString>().is_err()
            || offer.room_id.parse::<UlidString>().is_err()
            || offer.member_id.parse::<UlidString>().is_err()
            || offer.lease_duration_ms == 0
            || offer.lease_duration_ms > MAX_LEASE_DURATION_MS
            || !bounded_label(&offer.reason_code)
            || offer
                .deadline
                .as_ref()
                .is_some_and(|value| !bounded_label(value))
        {
            return Err(ActivationLedgerErrorV1::InvalidData);
        }
        self.selected_offer = Some(offer);
        Ok(self)
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub const fn activation_cursor(&self) -> u64 {
        self.activation_cursor
    }

    #[must_use]
    pub fn offer_operation_id(&self) -> &str {
        &self.offer_operation_id
    }

    #[must_use]
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    #[must_use]
    pub const fn selected_offer(&self) -> Option<&ActivationOffer> {
        self.selected_offer.as_ref()
    }
}

impl fmt::Debug for PreparedActivationAcquireV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedActivationAcquireV1")
            .field("assignment_id", &self.assignment_id)
            .field("activation_cursor", &self.activation_cursor)
            .field("offer_operation_id", &"[PRIVATE]")
            .field("claim_id", &"[PRIVATE]")
            .field(
                "selected_offer",
                &self.selected_offer.as_ref().map(|_| "[PRIVATE OFFER]"),
            )
            .finish()
    }
}

/// Stable durable preparation for completing one exact retained lease.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedActivationCompletionV1 {
    assignment_id: String,
    activation_cursor: u64,
    activation_id: String,
    claim_id: String,
    lease_generation: u64,
    operation_id: String,
    completion_id: String,
    remote_request_id: String,
    canonical_request_hash: String,
    disposition: String,
}

impl PreparedActivationCompletionV1 {
    /// Constructs a completion retry witness bound to one exact lease.
    ///
    /// # Errors
    ///
    /// Rejects malformed operation identities, hashes, or dispositions.
    pub fn new(
        lease: &AssignedActivationLeaseV1,
        operation_id: &str,
        completion_id: &str,
        remote_request_id: &str,
        canonical_request_hash: &str,
        disposition: &str,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        if operation_id.parse::<UlidString>().is_err()
            || completion_id.parse::<UlidString>().is_err()
            || remote_request_id.parse::<UlidString>().is_err()
            || !valid_digest(canonical_request_hash)
            || !bounded_disposition(disposition)
        {
            return Err(ActivationLedgerErrorV1::InvalidData);
        }
        Ok(Self {
            assignment_id: lease.assignment_id.clone(),
            activation_cursor: lease.activation_cursor,
            activation_id: lease.activation_id.clone(),
            claim_id: lease.claim_id.clone(),
            lease_generation: lease.lease_generation(),
            operation_id: operation_id.to_owned(),
            completion_id: completion_id.to_owned(),
            remote_request_id: remote_request_id.to_owned(),
            canonical_request_hash: canonical_request_hash.to_owned(),
            disposition: disposition.to_owned(),
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub const fn activation_cursor(&self) -> u64 {
        self.activation_cursor
    }

    #[must_use]
    pub fn activation_id(&self) -> &str {
        &self.activation_id
    }

    #[must_use]
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.lease_generation
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub fn completion_id(&self) -> &str {
        &self.completion_id
    }

    #[must_use]
    pub fn remote_request_id(&self) -> &str {
        &self.remote_request_id
    }

    #[must_use]
    pub fn canonical_request_hash(&self) -> &str {
        &self.canonical_request_hash
    }

    #[must_use]
    pub fn disposition(&self) -> &str {
        &self.disposition
    }

    pub(crate) fn matches_lease(&self, lease: &AssignedActivationLeaseV1) -> bool {
        self.assignment_id == lease.assignment_id
            && self.activation_cursor == lease.activation_cursor
            && self.activation_id == lease.activation_id
            && self.claim_id == lease.claim_id
            && self.lease_generation == lease.lease_generation()
    }
}

impl fmt::Debug for PreparedActivationCompletionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedActivationCompletionV1")
            .field("assignment_id", &self.assignment_id)
            .field("activation_cursor", &self.activation_cursor)
            .field("activation_id", &"[PRIVATE]")
            .field("claim_id", &"[PRIVATE]")
            .field("lease_generation", &self.lease_generation)
            .field("operation_id", &"[PRIVATE]")
            .field("completion_id", &"[PRIVATE]")
            .field("remote_request_id", &"[PRIVATE]")
            .field("canonical_request_hash", &"[PRIVATE]")
            .field("disposition", &"[PRIVATE]")
            .finish()
    }
}

/// Secret-free durable terminal receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationCompletionReceiptV1 {
    pub activation_cursor: u64,
    pub activation_id: String,
    pub lease_generation: u64,
    pub operation_id: String,
    pub completion_id: String,
    pub duplicate: bool,
}

impl ActivationCompletionReceiptV1 {
    /// Constructs an exact terminal receipt.
    ///
    /// # Errors
    ///
    /// Rejects a malformed operation identity.
    pub fn new(
        lease: &AssignedActivationLeaseV1,
        operation_id: &str,
        completion_id: &str,
        duplicate: bool,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        if operation_id.parse::<UlidString>().is_err()
            || completion_id.parse::<UlidString>().is_err()
        {
            return Err(ActivationLedgerErrorV1::InvalidData);
        }
        Ok(Self {
            activation_cursor: lease.activation_cursor,
            activation_id: lease.activation_id.clone(),
            lease_generation: lease.lease_generation(),
            operation_id: operation_id.to_owned(),
            completion_id: completion_id.to_owned(),
            duplicate,
        })
    }
}

/// Durable assignment-local lifecycle retained across helper restarts.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationLedgerSnapshotV1 {
    Idle {
        last_cursor: u64,
    },
    Acquiring(PreparedActivationAcquireV1),
    Leased(Box<AssignedActivationLeaseV1>),
    Completing {
        lease: Box<AssignedActivationLeaseV1>,
        prepared: PreparedActivationCompletionV1,
    },
    Completed(ActivationCompletionReceiptV1),
    Terminal {
        activation_cursor: u64,
        activation_id: String,
        claim_id: String,
        lease_generation: u64,
        outcome: ActivationTerminalOutcomeV1,
    },
    Abandoned(PreparedActivationAbandonV1),
}

impl fmt::Debug for ActivationLedgerSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle { last_cursor } => formatter
                .debug_struct("Idle")
                .field("last_cursor", last_cursor)
                .finish(),
            Self::Acquiring(prepared) => {
                formatter.debug_tuple("Acquiring").field(prepared).finish()
            }
            Self::Leased(lease) => formatter.debug_tuple("Leased").field(lease).finish(),
            Self::Completing { lease, prepared } => formatter
                .debug_struct("Completing")
                .field("lease", lease)
                .field("prepared", prepared)
                .finish(),
            Self::Completed(receipt) => formatter
                .debug_struct("Completed")
                .field("activation_cursor", &receipt.activation_cursor)
                .field("operation_id", &"[PRIVATE]")
                .finish(),
            Self::Terminal {
                activation_cursor,
                outcome,
                ..
            } => formatter
                .debug_struct("Terminal")
                .field("activation_cursor", activation_cursor)
                .field("activation_id", &"[PRIVATE]")
                .field("claim_id", &"[PRIVATE]")
                .field("outcome", outcome)
                .finish(),
            Self::Abandoned(abandoned) => {
                formatter.debug_tuple("Abandoned").field(abandoned).finish()
            }
        }
    }
}

/// Durable terminal outcome that permits the next Cursor after the caller has
/// observed the exact failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationTerminalOutcomeV1 {
    Expired,
    Stale,
    AlreadyCompleted,
}

/// Exact private claim witness retained when no lease was granted.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedActivationAbandonV1 {
    assignment_id: String,
    activation_cursor: u64,
    activation_id: String,
    claim_id: String,
    outcome: ActivationTerminalOutcomeV1,
}

impl PreparedActivationAbandonV1 {
    /// Constructs an exact abandonment witness from a durably selected offer.
    ///
    /// # Errors
    ///
    /// Rejects an acquisition without an exact selected offer.
    pub fn new(
        prepared: &PreparedActivationAcquireV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<Self, ActivationLedgerErrorV1> {
        let offer = prepared
            .selected_offer()
            .ok_or(ActivationLedgerErrorV1::InvalidData)?;
        Ok(Self {
            assignment_id: prepared.assignment_id().to_owned(),
            activation_cursor: prepared.activation_cursor(),
            activation_id: offer.activation_id.clone(),
            claim_id: prepared.claim_id().to_owned(),
            outcome,
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub const fn activation_cursor(&self) -> u64 {
        self.activation_cursor
    }

    #[must_use]
    pub fn activation_id(&self) -> &str {
        &self.activation_id
    }

    #[must_use]
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    #[must_use]
    pub const fn outcome(&self) -> ActivationTerminalOutcomeV1 {
        self.outcome
    }
}

impl fmt::Debug for PreparedActivationAbandonV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedActivationAbandonV1")
            .field("assignment_id", &self.assignment_id)
            .field("activation_cursor", &self.activation_cursor)
            .field("activation_id", &"[PRIVATE]")
            .field("claim_id", &"[PRIVATE]")
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// Narrow adapter over the shared assignment MCP operation ledger.
///
/// Every remote identity is durably reserved before I/O. Implementations map
/// these methods to the generic operation ledger without persisting private
/// Activation context in operator-facing records.
pub trait ActivationOperationLedgerV1: Send + Sync {
    /// Loads the exact retained assignment lifecycle.
    ///
    /// # Errors
    ///
    /// Returns a closed error for unavailable, conflicting, or corrupt storage.
    fn load(
        &self,
        assignment_id: &str,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1>;

    /// Durably reserves stable acquisition identities before daemon I/O.
    ///
    /// # Errors
    ///
    /// Returns a closed error when the Cursor conflicts or storage is unavailable.
    fn begin_acquire(
        &self,
        assignment_id: &str,
        activation_cursor: u64,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1>;

    /// Durably binds one exact offer before its first claim attempt.
    ///
    /// # Errors
    ///
    /// Returns a closed error for altered selection or invalid retained state.
    fn select_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
        offer: &ActivationOffer,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1>;

    /// Retains the exact private lease returned by a successful claim.
    ///
    /// # Errors
    ///
    /// Returns a closed error for a mismatched lease or persistence failure.
    fn retain_lease(
        &self,
        prepared: &PreparedActivationAcquireV1,
        lease: &AssignedActivationLeaseV1,
    ) -> Result<(), ActivationLedgerErrorV1>;

    /// Advances a completed empty poll without retaining a false offer.
    ///
    /// # Errors
    ///
    /// Returns a closed error for a mismatched preparation or persistence failure.
    fn retain_no_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<(), ActivationLedgerErrorV1>;

    /// Reserves one stable exact completion operation before daemon I/O.
    ///
    /// # Errors
    ///
    /// Returns a closed error for altered request reuse or persistence failure.
    fn begin_completion(
        &self,
        lease: &AssignedActivationLeaseV1,
        canonical_request_hash: &str,
        disposition: &str,
    ) -> Result<PreparedActivationCompletionV1, ActivationLedgerErrorV1>;

    /// Retains one exact successful completion receipt.
    ///
    /// # Errors
    ///
    /// Returns a closed error for an incoherent receipt or persistence failure.
    fn retain_completion(
        &self,
        prepared: &PreparedActivationCompletionV1,
        receipt: &ActivationCompletionReceiptV1,
    ) -> Result<(), ActivationLedgerErrorV1>;

    /// Retains an exact terminal lease witness after a definitive outcome.
    ///
    /// # Errors
    ///
    /// Returns a closed error for a mismatched lease or persistence failure.
    fn retain_terminal(
        &self,
        lease: &AssignedActivationLeaseV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1>;

    /// Retains an exact terminal claim witness when no lease was granted.
    ///
    /// # Errors
    ///
    /// Returns a closed error for a missing/mismatched offer or persistence failure.
    fn retain_abandoned(
        &self,
        prepared: &PreparedActivationAcquireV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActivationLedgerErrorV1 {
    #[error("the durable operation conflicts with an existing identity")]
    Conflict,
    #[error("the durable operation ledger is unavailable")]
    Unavailable,
    #[error("the durable operation ledger contains invalid data")]
    InvalidData,
}

/// Runner-only daemon boundary. Participant authority cannot satisfy this
/// interface because every call requires the sealed Runner assignment type.
pub trait RunnerActivationGatewayV1: Send + Sync {
    /// Lists pending offers only for the sealed assignment Membership.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, transport, availability, or data error.
    fn offers(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        operation_id: &str,
    ) -> Result<Vec<ActivationOffer>, RunnerActivationGatewayErrorV1>;

    /// Claims the exact durably selected offer with its retained identity.
    ///
    /// # Errors
    ///
    /// Returns a closed lease, authority, transport, or data error.
    fn claim(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<AssignedActivationLeaseV1, RunnerActivationGatewayErrorV1>;

    /// Completes the exact retained lease with its stable operation identity.
    ///
    /// # Errors
    ///
    /// Returns a closed terminal, authority, transport, or data error.
    fn complete(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        lease: &AssignedActivationLeaseV1,
        prepared: &PreparedActivationCompletionV1,
    ) -> Result<ActivationCompletionReceiptV1, RunnerActivationGatewayErrorV1>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunnerActivationGatewayErrorV1 {
    #[error("Runner authority was revoked")]
    Revoked,
    #[error("the Activation lease expired")]
    Expired,
    #[error("the Activation lease Cursor is stale")]
    StaleLease,
    #[error("the Activation was already completed")]
    AlreadyCompleted,
    #[error("the Activation is no longer available")]
    NotAvailable,
    #[error("the daemon connection was interrupted")]
    Disconnected,
    #[error("the daemon is temporarily unavailable")]
    Unavailable,
    #[error("the operation identity conflicts with a retained request")]
    IdempotencyConflict,
    #[error("the daemon returned invalid Activation data")]
    InvalidData,
}

/// Operational clock boundary used only to reject locally known-expired
/// leases before a completion attempt.
pub trait ActivationLeaseClockV1: Send + Sync {
    /// Compares a canonical lease deadline with trusted operational time.
    ///
    /// # Errors
    ///
    /// Returns an error when the retained deadline or host clock is invalid.
    fn is_expired(&self, lease_until: &str) -> Result<bool, ActivationLeaseClockErrorV1>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("the Activation lease clock value is invalid")]
pub struct ActivationLeaseClockErrorV1;

impl From<()> for ActivationLeaseClockErrorV1 {
    fn from((): ()) -> Self {
        Self
    }
}

/// Host operational clock used by the production helper.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemActivationLeaseClockV1;

impl ActivationLeaseClockV1 for SystemActivationLeaseClockV1 {
    fn is_expired(&self, lease_until: &str) -> Result<bool, ActivationLeaseClockErrorV1> {
        let lease_nanos = parse_utc_timestamp_nanos(lease_until)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ActivationLeaseClockErrorV1)?;
        let now_nanos = i128::from(now.as_secs())
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(i128::from(now.subsec_nanos())))
            .ok_or(ActivationLeaseClockErrorV1)?;
        Ok(now_nanos >= lease_nanos)
    }
}

/// Exact empty-input result for the assigned agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NextAssignedActivationV1 {
    pub activation_cursor: u64,
    pub reconnected: bool,
    pub activation: AssignedActivationLeaseV1,
}

/// Required optimistic preconditions for completing the current exact lease.
/// No Runner, Room, Membership, bearer, or Activation identifier is accepted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationCompletionArgumentsV1 {
    pub activation_cursor: u64,
    pub lease_generation: u64,
    pub context_hash: String,
    pub disposition: String,
}

/// Assignment-scoped generic Activation tools.
pub struct AssignmentActivationToolsV1<G, L, C> {
    authority: Arc<AssignedRunnerActivationAuthorityV1>,
    gateway: G,
    ledger: L,
    clock: C,
}

impl<G, L, C> AssignmentActivationToolsV1<G, L, C>
where
    G: RunnerActivationGatewayV1,
    L: ActivationOperationLedgerV1,
    C: ActivationLeaseClockV1,
{
    #[must_use]
    pub fn new(
        authority: AssignedRunnerActivationAuthorityV1,
        gateway: G,
        ledger: L,
        clock: C,
    ) -> Self {
        Self {
            authority: Arc::new(authority),
            gateway,
            ledger,
            clock,
        }
    }

    /// Acquires or resumes the next exact Activation for the sealed assignment.
    ///
    /// # Errors
    ///
    /// Returns a closed safe error for invalid arguments, durable conflicts,
    /// daemon authority failures, stale state, or unavailable work.
    pub fn next_activation(
        &self,
        arguments: &Value,
    ) -> Result<NextAssignedActivationV1, ActivationToolErrorV1> {
        decode_empty(arguments)?;
        let snapshot = self.load()?;
        match snapshot {
            ActivationLedgerSnapshotV1::Leased(lease) => self.resume(*lease),
            ActivationLedgerSnapshotV1::Completing { .. } => {
                Err(ActivationToolErrorV1::CompletionPending)
            }
            ActivationLedgerSnapshotV1::Acquiring(prepared) => self.acquire(prepared),
            ActivationLedgerSnapshotV1::Idle { last_cursor } => {
                let cursor = last_cursor
                    .checked_add(1)
                    .ok_or(ActivationToolErrorV1::InvalidRetainedState)?;
                let prepared = self
                    .ledger
                    .begin_acquire(self.authority.assignment_id(), cursor)
                    .map_err(map_ledger_error)?;
                self.acquire(prepared)
            }
            ActivationLedgerSnapshotV1::Completed(receipt) => {
                let cursor = receipt
                    .activation_cursor
                    .checked_add(1)
                    .ok_or(ActivationToolErrorV1::InvalidRetainedState)?;
                let prepared = self
                    .ledger
                    .begin_acquire(self.authority.assignment_id(), cursor)
                    .map_err(map_ledger_error)?;
                self.acquire(prepared)
            }
            ActivationLedgerSnapshotV1::Terminal {
                activation_cursor, ..
            } => {
                let cursor = activation_cursor
                    .checked_add(1)
                    .ok_or(ActivationToolErrorV1::InvalidRetainedState)?;
                let prepared = self
                    .ledger
                    .begin_acquire(self.authority.assignment_id(), cursor)
                    .map_err(map_ledger_error)?;
                self.acquire(prepared)
            }
            ActivationLedgerSnapshotV1::Abandoned(abandoned) => {
                let cursor = abandoned
                    .activation_cursor()
                    .checked_add(1)
                    .ok_or(ActivationToolErrorV1::InvalidRetainedState)?;
                let prepared = self
                    .ledger
                    .begin_acquire(self.authority.assignment_id(), cursor)
                    .map_err(map_ledger_error)?;
                self.acquire(prepared)
            }
        }
    }

    /// Completes only the exact current retained lease.
    ///
    /// # Errors
    ///
    /// Returns distinct safe errors for expiry, revocation, stale Cursor,
    /// already-completed state, conflicts, disconnects, or invalid arguments.
    pub fn complete(
        &self,
        arguments: Value,
    ) -> Result<ActivationCompletionReceiptV1, ActivationToolErrorV1> {
        let arguments: ActivationCompletionArgumentsV1 = serde_json::from_value(arguments)
            .map_err(|_| ActivationToolErrorV1::InvalidArguments)?;
        validate_completion_arguments(&arguments)?;
        let snapshot = self.load()?;
        let (lease, prepared) = match snapshot {
            ActivationLedgerSnapshotV1::Leased(lease) => {
                let lease = *lease;
                self.validate_completion_preconditions(&lease, &arguments)?;
                self.reject_expired(&lease)?;
                let request_hash = completion_request_hash(&arguments)?;
                let prepared = self
                    .ledger
                    .begin_completion(&lease, &request_hash, &arguments.disposition)
                    .map_err(map_ledger_error)?;
                (lease, prepared)
            }
            ActivationLedgerSnapshotV1::Completing { lease, prepared } => {
                let lease = *lease;
                self.validate_completion_preconditions(&lease, &arguments)?;
                let request_hash = completion_request_hash(&arguments)?;
                if !prepared.matches_lease(&lease)
                    || prepared.canonical_request_hash() != request_hash
                    || prepared.disposition() != arguments.disposition
                {
                    return Err(ActivationToolErrorV1::IdempotencyConflict);
                }
                (lease, prepared)
            }
            ActivationLedgerSnapshotV1::Completed(_) => {
                return Err(ActivationToolErrorV1::AlreadyCompleted);
            }
            ActivationLedgerSnapshotV1::Idle { .. } | ActivationLedgerSnapshotV1::Acquiring(_) => {
                return Err(ActivationToolErrorV1::StaleCursor);
            }
            ActivationLedgerSnapshotV1::Terminal { outcome, .. } => {
                return Err(map_terminal_outcome(outcome));
            }
            ActivationLedgerSnapshotV1::Abandoned(abandoned) => {
                return Err(map_terminal_outcome(abandoned.outcome()));
            }
        };
        let receipt = match self.gateway.complete(&self.authority, &lease, &prepared) {
            Ok(receipt) => receipt,
            Err(RunnerActivationGatewayErrorV1::Expired) => {
                self.ledger
                    .retain_terminal(&lease, ActivationTerminalOutcomeV1::Expired)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::LeaseExpired);
            }
            Err(RunnerActivationGatewayErrorV1::StaleLease) => {
                self.ledger
                    .retain_terminal(&lease, ActivationTerminalOutcomeV1::Stale)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::StaleCursor);
            }
            Err(RunnerActivationGatewayErrorV1::AlreadyCompleted) => {
                self.ledger
                    .retain_terminal(&lease, ActivationTerminalOutcomeV1::AlreadyCompleted)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::AlreadyCompleted);
            }
            Err(error) => return Err(map_gateway_error(error)),
        };
        if receipt.activation_cursor != lease.activation_cursor
            || receipt.activation_id != lease.activation_id
            || receipt.lease_generation != lease.lease_generation()
            || receipt.operation_id != prepared.operation_id()
            || receipt.completion_id != prepared.completion_id()
        {
            return Err(ActivationToolErrorV1::InvalidDaemonData);
        }
        self.ledger
            .retain_completion(&prepared, &receipt)
            .map_err(map_ledger_error)?;
        Ok(receipt)
    }

    fn load(&self) -> Result<ActivationLedgerSnapshotV1, ActivationToolErrorV1> {
        self.ledger
            .load(self.authority.assignment_id())
            .map_err(map_ledger_error)
    }

    fn resume(
        &self,
        lease: AssignedActivationLeaseV1,
    ) -> Result<NextAssignedActivationV1, ActivationToolErrorV1> {
        if lease.assignment_id() != self.authority.assignment_id() {
            return Err(ActivationToolErrorV1::InvalidRetainedState);
        }
        self.reject_expired(&lease)?;
        Ok(NextAssignedActivationV1 {
            activation_cursor: lease.activation_cursor,
            reconnected: true,
            activation: lease,
        })
    }

    fn acquire(
        &self,
        mut prepared: PreparedActivationAcquireV1,
    ) -> Result<NextAssignedActivationV1, ActivationToolErrorV1> {
        if prepared.assignment_id() != self.authority.assignment_id() {
            return Err(ActivationToolErrorV1::InvalidRetainedState);
        }
        if prepared.selected_offer().is_none() {
            let offers = self
                .gateway
                .offers(&self.authority, prepared.offer_operation_id())
                .map_err(map_gateway_error)?;
            let Some(offer) = offers.into_iter().next() else {
                self.ledger
                    .retain_no_offer(&prepared)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::NoActivation);
            };
            validate_offer_scope(&offer, &self.authority)?;
            prepared = self
                .ledger
                .select_offer(&prepared, &offer)
                .map_err(map_ledger_error)?;
        }
        let lease = match self.gateway.claim(&self.authority, &prepared) {
            Ok(lease) => lease,
            Err(RunnerActivationGatewayErrorV1::NotAvailable) => {
                self.ledger
                    .retain_no_offer(&prepared)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::NoActivation);
            }
            Err(RunnerActivationGatewayErrorV1::Expired) => {
                self.ledger
                    .retain_abandoned(&prepared, ActivationTerminalOutcomeV1::Expired)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::NoActivation);
            }
            Err(RunnerActivationGatewayErrorV1::StaleLease) => {
                self.ledger
                    .retain_abandoned(&prepared, ActivationTerminalOutcomeV1::Stale)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::StaleCursor);
            }
            Err(RunnerActivationGatewayErrorV1::AlreadyCompleted) => {
                self.ledger
                    .retain_abandoned(&prepared, ActivationTerminalOutcomeV1::AlreadyCompleted)
                    .map_err(map_ledger_error)?;
                return Err(ActivationToolErrorV1::AlreadyCompleted);
            }
            Err(error) => return Err(map_gateway_error(error)),
        };
        let selected = prepared
            .selected_offer()
            .ok_or(ActivationToolErrorV1::InvalidRetainedState)?;
        if lease.assignment_id() != self.authority.assignment_id()
            || lease.activation_cursor != prepared.activation_cursor()
            || lease.activation_id() != selected.activation_id
            || lease.claim_id() != prepared.claim_id()
            || lease.context.room_head.room_id != self.authority.room_id()
            || lease.context.room_head.pack_digest != self.authority.pack().digest
            || lease.context.cause_room_seq != selected.cause_room_seq
            || lease.context.reason_code != selected.reason_code
            || lease.context.deadline != selected.deadline
        {
            return Err(ActivationToolErrorV1::InvalidDaemonData);
        }
        self.ledger
            .retain_lease(&prepared, &lease)
            .map_err(map_ledger_error)?;
        Ok(NextAssignedActivationV1 {
            activation_cursor: lease.activation_cursor,
            reconnected: false,
            activation: lease,
        })
    }

    fn validate_completion_preconditions(
        &self,
        lease: &AssignedActivationLeaseV1,
        arguments: &ActivationCompletionArgumentsV1,
    ) -> Result<(), ActivationToolErrorV1> {
        if lease.assignment_id() != self.authority.assignment_id() {
            return Err(ActivationToolErrorV1::InvalidRetainedState);
        }
        if arguments.activation_cursor != lease.activation_cursor
            || arguments.lease_generation != lease.lease_generation()
        {
            return Err(ActivationToolErrorV1::StaleCursor);
        }
        if arguments.context_hash != lease.context_hash() {
            return Err(ActivationToolErrorV1::IdempotencyConflict);
        }
        Ok(())
    }

    fn reject_expired(
        &self,
        lease: &AssignedActivationLeaseV1,
    ) -> Result<(), ActivationToolErrorV1> {
        match self.clock.is_expired(lease.lease_until()) {
            Ok(true) => {
                self.ledger
                    .retain_terminal(lease, ActivationTerminalOutcomeV1::Expired)
                    .map_err(map_ledger_error)?;
                Err(ActivationToolErrorV1::LeaseExpired)
            }
            Ok(false) => Ok(()),
            Err(_) => Err(ActivationToolErrorV1::InvalidRetainedState),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationToolErrorCodeV1 {
    InvalidArguments,
    NoActivation,
    LeaseExpired,
    AuthorityRevoked,
    StaleCursor,
    AlreadyCompleted,
    CompletionPending,
    IdempotencyConflict,
    Disconnected,
    Unavailable,
    InvalidRetainedState,
    InvalidDaemonData,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ActivationToolErrorV1 {
    #[error("tool arguments do not match the bounded Activation schema")]
    InvalidArguments,
    #[error("no pending Activation is available for this assignment")]
    NoActivation,
    #[error("the current Activation lease has expired; request the next Activation")]
    LeaseExpired,
    #[error("Runner authority was revoked; ask an operator to reprovision this assignment")]
    AuthorityRevoked,
    #[error("the Activation Cursor or lease generation is stale; reload the current Activation")]
    StaleCursor,
    #[error("this exact Activation has already been completed")]
    AlreadyCompleted,
    #[error("completion is pending; retry the same completion request")]
    CompletionPending,
    #[error("the operation identity is bound to different completion input")]
    IdempotencyConflict,
    #[error("the daemon connection was interrupted; retry the same operation")]
    Disconnected,
    #[error("Activation control is temporarily unavailable")]
    Unavailable,
    #[error("retained Activation state is invalid")]
    InvalidRetainedState,
    #[error("the daemon returned invalid Activation data")]
    InvalidDaemonData,
}

impl ActivationToolErrorV1 {
    #[must_use]
    pub const fn code(&self) -> ActivationToolErrorCodeV1 {
        match self {
            Self::InvalidArguments => ActivationToolErrorCodeV1::InvalidArguments,
            Self::NoActivation => ActivationToolErrorCodeV1::NoActivation,
            Self::LeaseExpired => ActivationToolErrorCodeV1::LeaseExpired,
            Self::AuthorityRevoked => ActivationToolErrorCodeV1::AuthorityRevoked,
            Self::StaleCursor => ActivationToolErrorCodeV1::StaleCursor,
            Self::AlreadyCompleted => ActivationToolErrorCodeV1::AlreadyCompleted,
            Self::CompletionPending => ActivationToolErrorCodeV1::CompletionPending,
            Self::IdempotencyConflict => ActivationToolErrorCodeV1::IdempotencyConflict,
            Self::Disconnected => ActivationToolErrorCodeV1::Disconnected,
            Self::Unavailable => ActivationToolErrorCodeV1::Unavailable,
            Self::InvalidRetainedState => ActivationToolErrorCodeV1::InvalidRetainedState,
            Self::InvalidDaemonData => ActivationToolErrorCodeV1::InvalidDaemonData,
        }
    }
}

/// Fixed-loopback production Runner transport. Each method reconnects and
/// replays the stable durable operation identity supplied by the ledger.
#[derive(Clone)]
pub struct FixedDaemonRunnerActivationGatewayV1 {
    address: SocketAddr,
    timeout: Duration,
}

impl FixedDaemonRunnerActivationGatewayV1 {
    /// Constructs a bounded fixed-daemon transport.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback endpoints and unreasonable timeouts.
    pub fn new(
        address: SocketAddr,
        timeout: Duration,
    ) -> Result<Self, RunnerActivationGatewayErrorV1> {
        if !address.ip().is_loopback() || timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        Ok(Self { address, timeout })
    }

    fn connect(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
    ) -> Result<(WebSocket<TcpStream>, RunnerReadBudgetV1), RunnerActivationGatewayErrorV1> {
        let stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| RunnerActivationGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| RunnerActivationGatewayErrorV1::Disconnected)?;
        let mut authorization = Zeroizing::new(Vec::with_capacity(
            "Bearer ".len() + authority.bearer().as_str().len(),
        ));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(authority.bearer().as_str().as_bytes());
        let authorization =
            tungstenite::http::HeaderValue::from_maybe_shared(Bytes::from_owner(authorization))
                .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
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
            .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
        let (mut socket, response) = match client(request, stream) {
            Ok(connected) => connected,
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if matches!(
                    response.status(),
                    http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
                ) =>
            {
                return Err(RunnerActivationGatewayErrorV1::Revoked);
            }
            Err(tungstenite::HandshakeError::Failure(
                tungstenite::Error::Io(_)
                | tungstenite::Error::ConnectionClosed
                | tungstenite::Error::AlreadyClosed,
            )) => {
                return Err(RunnerActivationGatewayErrorV1::Disconnected);
            }
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if response.status().is_server_error()
                    || response.status() == http::StatusCode::TOO_MANY_REQUESTS =>
            {
                return Err(RunnerActivationGatewayErrorV1::Unavailable);
            }
            Err(_) => return Err(RunnerActivationGatewayErrorV1::InvalidData),
        };
        if response.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(RunnerActivationGatewayErrorV1::Revoked);
        }
        let mut budget = RunnerReadBudgetV1::new(self.timeout);
        send_runner_message(
            &mut socket,
            "client.hello",
            &ClientHello {
                client_name: "worldstream-assignment-mcp-activations".to_owned(),
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                mode: ClientMode::Runner,
                supported_protocols: vec![PROTOCOL_VERSION.to_owned()],
                capabilities: REQUIRED_CLIENT_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
        )?;
        let welcome: ServerWelcome = read_runner_type(&mut socket, &mut budget, "server.welcome")?;
        if welcome.selected_protocol != PROTOCOL_VERSION
            || welcome.maximum_message_bytes != MAX_MESSAGE_BYTES
            || welcome.authenticated_principal.kind != PrincipalKind::Agent
            || welcome.authenticated_principal.principal_id != authority.principal_id()
        {
            return Err(RunnerActivationGatewayErrorV1::Revoked);
        }
        send_runner_message(
            &mut socket,
            "runner.hello",
            &RunnerHello {
                runner_id: authority.runner_id().to_owned(),
                maximum_concurrent_activations: 1,
                supported_pack_ids: vec![authority.pack().id.clone()],
                supported_pack_revisions: vec![authority.pack().clone()],
            },
        )?;
        let ready: RunnerReady = read_runner_type(&mut socket, &mut budget, "runner.ready")?;
        if ready.runner_id != authority.runner_id() {
            return Err(RunnerActivationGatewayErrorV1::Revoked);
        }
        Ok((socket, budget))
    }
}

impl fmt::Debug for FixedDaemonRunnerActivationGatewayV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FixedDaemonRunnerActivationGatewayV1(REDACTED)")
    }
}

impl RunnerActivationGatewayV1 for FixedDaemonRunnerActivationGatewayV1 {
    fn offers(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        operation_id: &str,
    ) -> Result<Vec<ActivationOffer>, RunnerActivationGatewayErrorV1> {
        if operation_id.parse::<UlidString>().is_err() {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        let (mut socket, mut budget) = self.connect(authority)?;
        send_runner_message(
            &mut socket,
            "activation.offer",
            &ActivationOfferRequest {
                operation_id: operation_id.to_owned(),
                runner_id: authority.runner_id().to_owned(),
                room_id: authority.room_id().to_owned(),
                member_id: authority.member_id().to_owned(),
            },
        )?;
        let reply: ActivationOffers =
            read_runner_type(&mut socket, &mut budget, "activation.offers")?;
        if reply.operation_id != operation_id || reply.runner_id != authority.runner_id() {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        for offer in &reply.offers {
            validate_offer_scope(offer, authority)
                .map_err(|_| RunnerActivationGatewayErrorV1::Revoked)?;
        }
        Ok(reply.offers)
    }

    fn claim(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<AssignedActivationLeaseV1, RunnerActivationGatewayErrorV1> {
        let offer = prepared
            .selected_offer()
            .ok_or(RunnerActivationGatewayErrorV1::InvalidData)?;
        validate_offer_scope(offer, authority)
            .map_err(|_| RunnerActivationGatewayErrorV1::Revoked)?;
        let (mut socket, mut budget) = self.connect(authority)?;
        send_runner_message(
            &mut socket,
            "activation.claim",
            &ActivationClaim {
                activation_id: offer.activation_id.clone(),
                runner_id: authority.runner_id().to_owned(),
                claim_id: prepared.claim_id().to_owned(),
                requested_lease_ms: offer.lease_duration_ms,
            },
        )?;
        let reply: ActivationOperationReply =
            read_runner_type(&mut socket, &mut budget, "activation.claimed")?;
        validate_operation_reply(
            &reply,
            authority,
            Some(&offer.activation_id),
            Some(prepared.claim_id()),
        )?;
        if reply.operation_id != prepared.claim_id() {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        match reply.code {
            ActivationResultCode::Granted => {
                let context = reply
                    .context
                    .ok_or(RunnerActivationGatewayErrorV1::InvalidData)?;
                AssignedActivationLeaseV1::new(
                    authority.assignment_id(),
                    prepared.activation_cursor(),
                    &offer.activation_id,
                    prepared.claim_id(),
                    reply
                        .context_hash
                        .as_deref()
                        .ok_or(RunnerActivationGatewayErrorV1::InvalidData)?,
                    context,
                )
                .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)
            }
            ActivationResultCode::NotAvailable => Err(RunnerActivationGatewayErrorV1::NotAvailable),
            ActivationResultCode::Expired | ActivationResultCode::Cancelled => {
                Err(RunnerActivationGatewayErrorV1::Expired)
            }
            ActivationResultCode::Fenced | ActivationResultCode::StaleLease => {
                Err(RunnerActivationGatewayErrorV1::StaleLease)
            }
            ActivationResultCode::IdempotencyConflict => {
                Err(RunnerActivationGatewayErrorV1::IdempotencyConflict)
            }
            ActivationResultCode::Completed => {
                Err(RunnerActivationGatewayErrorV1::AlreadyCompleted)
            }
            ActivationResultCode::ResultRetired
            | ActivationResultCode::Renewed
            | ActivationResultCode::Released => Err(RunnerActivationGatewayErrorV1::InvalidData),
        }
    }

    fn complete(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        lease: &AssignedActivationLeaseV1,
        prepared: &PreparedActivationCompletionV1,
    ) -> Result<ActivationCompletionReceiptV1, RunnerActivationGatewayErrorV1> {
        if lease.assignment_id() != authority.assignment_id() || !prepared.matches_lease(lease) {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        let (mut socket, mut budget) = self.connect(authority)?;
        send_runner_message_with_request_id(
            &mut socket,
            "activation.complete",
            prepared.remote_request_id(),
            &ActivationLeaseOperation {
                activation_id: lease.activation_id().to_owned(),
                runner_id: authority.runner_id().to_owned(),
                claim_id: lease.claim_id().to_owned(),
                operation_id: prepared.operation_id().to_owned(),
                lease_generation: lease.lease_generation(),
                requested_lease_ms: None,
                disposition: Some(prepared.disposition().to_owned()),
            },
        )?;
        let reply: ActivationOperationReply =
            read_runner_type(&mut socket, &mut budget, "activation.completed")?;
        validate_operation_reply(
            &reply,
            authority,
            Some(lease.activation_id()),
            Some(lease.claim_id()),
        )?;
        if reply.operation_id != prepared.operation_id() {
            return Err(RunnerActivationGatewayErrorV1::InvalidData);
        }
        match reply.code {
            ActivationResultCode::Completed => ActivationCompletionReceiptV1::new(
                lease,
                prepared.operation_id(),
                prepared.completion_id(),
                false,
            )
            .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData),
            ActivationResultCode::Expired | ActivationResultCode::Cancelled => {
                Err(RunnerActivationGatewayErrorV1::Expired)
            }
            ActivationResultCode::Fenced | ActivationResultCode::StaleLease => {
                Err(RunnerActivationGatewayErrorV1::StaleLease)
            }
            ActivationResultCode::IdempotencyConflict => {
                Err(RunnerActivationGatewayErrorV1::IdempotencyConflict)
            }
            ActivationResultCode::ResultRetired => {
                Err(RunnerActivationGatewayErrorV1::AlreadyCompleted)
            }
            ActivationResultCode::Granted
            | ActivationResultCode::Renewed
            | ActivationResultCode::Released
            | ActivationResultCode::NotAvailable => {
                Err(RunnerActivationGatewayErrorV1::InvalidData)
            }
        }
    }
}

struct RunnerReadBudgetV1 {
    deadline: Instant,
    bytes: usize,
    messages: usize,
}

impl RunnerReadBudgetV1 {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            bytes: 0,
            messages: 0,
        }
    }
}

fn send_runner_message(
    socket: &mut WebSocket<TcpStream>,
    message_type: &str,
    body: &impl Serialize,
) -> Result<(), RunnerActivationGatewayErrorV1> {
    send_runner_message_inner(socket, message_type, None, body)
}

fn send_runner_message_with_request_id(
    socket: &mut WebSocket<TcpStream>,
    message_type: &str,
    request_id: &str,
    body: &impl Serialize,
) -> Result<(), RunnerActivationGatewayErrorV1> {
    let request_id = request_id
        .parse::<UlidString>()
        .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
    send_runner_message_inner(socket, message_type, Some(request_id), body)
}

fn send_runner_message_inner(
    socket: &mut WebSocket<TcpStream>,
    message_type: &str,
    request_id: Option<UlidString>,
    body: &impl Serialize,
) -> Result<(), RunnerActivationGatewayErrorV1> {
    let envelope = VersionedEnvelope {
        protocol: PROTOCOL_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: next_protocol_ulid()?,
        request_id,
        body,
    };
    let mut text = serde_json::to_string(&envelope)
        .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
    let result = socket
        .send(Message::Text(text.clone().into()))
        .map_err(|_| RunnerActivationGatewayErrorV1::Disconnected);
    text.zeroize();
    result
}

fn read_runner_type<T: DeserializeOwned>(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut RunnerReadBudgetV1,
    expected: &str,
) -> Result<T, RunnerActivationGatewayErrorV1> {
    loop {
        if Instant::now() >= budget.deadline
            || budget.messages >= MAX_RUNNER_SESSION_MESSAGES
            || budget.bytes > MAX_RUNNER_SESSION_BYTES
        {
            return Err(RunnerActivationGatewayErrorV1::Unavailable);
        }
        match socket.read() {
            Ok(Message::Text(text)) if text.len() <= MAX_MESSAGE_BYTES => {
                budget.bytes = budget
                    .bytes
                    .checked_add(text.len())
                    .filter(|bytes| *bytes <= MAX_RUNNER_SESSION_BYTES)
                    .ok_or(RunnerActivationGatewayErrorV1::InvalidData)?;
                budget.messages += 1;
                let envelope = decode_envelope::<Value>(text.as_bytes())
                    .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
                if envelope.message_type == "error" {
                    return Err(classify_daemon_error(envelope.body));
                }
                if envelope.message_type != expected {
                    return Err(RunnerActivationGatewayErrorV1::InvalidData);
                }
                return serde_json::from_value(envelope.body)
                    .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData);
            }
            Ok(Message::Ping(value)) => {
                budget.messages += 1;
                socket
                    .send(Message::Pong(value))
                    .map_err(|_| RunnerActivationGatewayErrorV1::Disconnected)?;
            }
            Ok(Message::Text(_) | Message::Binary(_)) => {
                return Err(RunnerActivationGatewayErrorV1::InvalidData);
            }
            Ok(Message::Close(_)) | Err(_) => {
                return Err(RunnerActivationGatewayErrorV1::Disconnected);
            }
            Ok(Message::Pong(_) | Message::Frame(_)) => budget.messages += 1,
        }
    }
}

fn validate_operation_reply(
    reply: &ActivationOperationReply,
    authority: &AssignedRunnerActivationAuthorityV1,
    activation_id: Option<&str>,
    claim_id: Option<&str>,
) -> Result<(), RunnerActivationGatewayErrorV1> {
    if reply.runner_id != authority.runner_id()
        || reply.activation_id.as_deref() != activation_id
        || reply.claim_id.as_deref() != claim_id
        || reply.context.as_ref().is_some_and(|context| {
            Some(context.activation_id.as_str()) != activation_id
                || Some(context.claim_id.as_str()) != claim_id
                || reply.lease_generation != Some(context.lease_generation)
                || reply.context_hash.is_none()
        })
        || matches!(reply.code, ActivationResultCode::Granted)
            && (reply.state != Some(ActivationIntentState::Leased)
                || reply.lease_generation.is_none()
                || reply.context.is_none())
    {
        return Err(RunnerActivationGatewayErrorV1::InvalidData);
    }
    Ok(())
}

fn classify_daemon_error(body: Value) -> RunnerActivationGatewayErrorV1 {
    let Ok(error) = serde_json::from_value::<ProtocolErrorBody>(body) else {
        return RunnerActivationGatewayErrorV1::InvalidData;
    };
    match error.code {
        ErrorCode::Unauthenticated
        | ErrorCode::Forbidden
        | ErrorCode::MembershipNotFound
        | ErrorCode::MembershipNotEnabled
        | ErrorCode::RoomNotFound => RunnerActivationGatewayErrorV1::Revoked,
        ErrorCode::CursorAhead | ErrorCode::CursorOutOfRange => {
            RunnerActivationGatewayErrorV1::StaleLease
        }
        ErrorCode::IdempotencyConflict => RunnerActivationGatewayErrorV1::IdempotencyConflict,
        ErrorCode::StorageUnavailable
        | ErrorCode::StorageNotInitialized
        | ErrorCode::RoomBusy
        | ErrorCode::RateLimited => RunnerActivationGatewayErrorV1::Unavailable,
        ErrorCode::SlowConsumer => RunnerActivationGatewayErrorV1::Disconnected,
        ErrorCode::ConfigInvalid
        | ErrorCode::Internal
        | ErrorCode::UnsupportedProtocol
        | ErrorCode::InvalidEnvelope
        | ErrorCode::MessageTooLarge
        | ErrorCode::ActivityPackRevisionUnavailable
        | ErrorCode::RoomFaulted
        | ErrorCode::RoomQuarantined
        | ErrorCode::SyncBarrierMismatch
        | ErrorCode::WrongPhase
        | ErrorCode::CommitIndeterminate
        | ErrorCode::InvalidPayload
        | ErrorCode::ActivityFault => RunnerActivationGatewayErrorV1::InvalidData,
    }
}

/// Computes the Core-defined hash of one exact protocol Invocation Context.
///
/// # Errors
///
/// Rejects malformed Heads or semantic time, non-canonical payloads,
/// incoherent Action offers, and invalid frame hashes or delivery bounds.
pub fn canonical_activation_context_hash_v1(
    context: &ActivationInvocationContext,
) -> Result<String, ActivationContextValidationErrorV1> {
    let projection: worldstream_protocol::Projection =
        serde_json::from_value(context.projection.clone()).map_err(|_| ())?;
    if projection.action_offers != context.action_offers {
        return Err(ActivationContextValidationErrorV1);
    }
    let room_head: CompleteHeadV1 =
        serde_json::from_value(serde_json::to_value(&context.room_head).map_err(|_| ())?)
            .map_err(|_| ())?;
    let projection_bytes = canonical_bytes(&context.projection)?;
    let action_offers_bytes = canonical_bytes(&context.action_offers)?;
    let runner_budget_bytes = canonical_bytes(&context.runner_budget)?;
    let runner_limits_bytes = canonical_bytes(&context.runner_limits)?;
    let artifact_references = context
        .artifact_references
        .iter()
        .map(canonical_value)
        .collect::<Result<Vec<_>, _>>()?;
    let delivery = match &context.delivery {
        worldstream_protocol::ActivationDelivery::RetainedFrames {
            cursor_exclusive,
            through_frame_head,
            frames,
        } => CoreActivationDeliveryV1::RetainedFrames {
            cursor_exclusive: *cursor_exclusive,
            through_frame_head: *through_frame_head,
            frames: frames
                .iter()
                .map(|frame| {
                    let payload_bytes = canonical_bytes(&frame.payload)?;
                    let payload_hash: Blake3DigestV1 =
                        frame.payload_hash.parse().map_err(|_| ())?;
                    if Blake3DigestV1::hash(&payload_bytes) != payload_hash {
                        return Err(ActivationContextValidationErrorV1);
                    }
                    Ok(CoreActivationFrameV1 {
                        frame_seq: frame.frame_seq,
                        cause_room_seq: RoomSequenceV1::new(frame.cause_room_seq)
                            .map_err(|_| ())?,
                        payload_hash,
                        payload_bytes,
                    })
                })
                .collect::<Result<Vec<_>, ActivationContextValidationErrorV1>>()?,
        },
        worldstream_protocol::ActivationDelivery::ProjectionReset {
            baseline_frame_head,
            reason,
        } => CoreActivationDeliveryV1::ProjectionReset {
            baseline_frame_head: *baseline_frame_head,
            reason: reason.clone(),
        },
    };
    let prepared = prepare_activation_context(ActivationContextInputV1 {
        activation_id: context.activation_id.clone(),
        claim_id: context.claim_id.clone(),
        cause_room_seq: RoomSequenceV1::new(context.cause_room_seq).map_err(|_| ())?,
        reason_code: context.reason_code.clone(),
        lease_generation: context.lease_generation,
        lease_until: context.lease_until.clone(),
        semantic_deadline: context
            .deadline
            .as_deref()
            .map(str::parse::<TimerScheduledFor>)
            .transpose()
            .map_err(|_| ())?,
        room_head,
        integrity_generation: context.integrity_generation,
        policy_revision: context.policy_revision,
        authority_generation: context.authority_generation,
        membership_generation: context.membership_generation,
        frame_head: context.frame_head,
        retained_floor: context.retained_floor,
        cursor: context.cursor,
        projection_schema: context.projection_schema.clone(),
        projection_bytes,
        action_offers_bytes,
        runner_budget_bytes,
        runner_limits_bytes,
        artifact_references,
        delivery,
    })
    .map_err(|_| ())?;
    prepared
        .context_hash()
        .map(|hash| hash.to_string())
        .map_err(|_| ActivationContextValidationErrorV1)
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("the daemon Activation context is invalid")]
pub struct ActivationContextValidationErrorV1;

impl From<()> for ActivationContextValidationErrorV1 {
    fn from((): ()) -> Self {
        Self
    }
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, ()> {
    CanonicalJsonV1::parse(&serde_json::to_vec(value).map_err(|_| ())?)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| ())
}

fn canonical_value(value: &impl Serialize) -> Result<CanonicalJsonV1, ()> {
    let bytes = canonical_bytes(value)?;
    CanonicalJsonV1::from_canonical_bytes(&bytes).map_err(|_| ())
}

fn validate_offer_scope(
    offer: &ActivationOffer,
    authority: &AssignedRunnerActivationAuthorityV1,
) -> Result<(), ActivationToolErrorV1> {
    if offer.room_id != authority.room_id()
        || offer.member_id != authority.member_id()
        || offer.activation_id.parse::<UlidString>().is_err()
        || offer.lease_duration_ms == 0
        || offer.lease_duration_ms > MAX_LEASE_DURATION_MS
        || !bounded_label(&offer.reason_code)
        || offer
            .deadline
            .as_ref()
            .is_some_and(|value| !bounded_label(value))
    {
        return Err(ActivationToolErrorV1::InvalidDaemonData);
    }
    Ok(())
}

fn validate_completion_arguments(
    arguments: &ActivationCompletionArgumentsV1,
) -> Result<(), ActivationToolErrorV1> {
    if arguments.activation_cursor == 0
        || arguments.lease_generation == 0
        || !valid_digest(&arguments.context_hash)
        || !bounded_disposition(&arguments.disposition)
    {
        return Err(ActivationToolErrorV1::InvalidArguments);
    }
    Ok(())
}

fn completion_request_hash(
    arguments: &ActivationCompletionArgumentsV1,
) -> Result<String, ActivationToolErrorV1> {
    let bytes =
        serde_json::to_vec(arguments).map_err(|_| ActivationToolErrorV1::InvalidArguments)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn decode_empty(value: &Value) -> Result<(), ActivationToolErrorV1> {
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(ActivationToolErrorV1::InvalidArguments)
    }
}

fn map_gateway_error(error: RunnerActivationGatewayErrorV1) -> ActivationToolErrorV1 {
    match error {
        RunnerActivationGatewayErrorV1::Revoked => ActivationToolErrorV1::AuthorityRevoked,
        RunnerActivationGatewayErrorV1::Expired => ActivationToolErrorV1::LeaseExpired,
        RunnerActivationGatewayErrorV1::StaleLease => ActivationToolErrorV1::StaleCursor,
        RunnerActivationGatewayErrorV1::AlreadyCompleted => ActivationToolErrorV1::AlreadyCompleted,
        RunnerActivationGatewayErrorV1::NotAvailable => ActivationToolErrorV1::NoActivation,
        RunnerActivationGatewayErrorV1::Disconnected => ActivationToolErrorV1::Disconnected,
        RunnerActivationGatewayErrorV1::Unavailable => ActivationToolErrorV1::Unavailable,
        RunnerActivationGatewayErrorV1::IdempotencyConflict => {
            ActivationToolErrorV1::IdempotencyConflict
        }
        RunnerActivationGatewayErrorV1::InvalidData => ActivationToolErrorV1::InvalidDaemonData,
    }
}

const fn map_terminal_outcome(outcome: ActivationTerminalOutcomeV1) -> ActivationToolErrorV1 {
    match outcome {
        ActivationTerminalOutcomeV1::Expired => ActivationToolErrorV1::LeaseExpired,
        ActivationTerminalOutcomeV1::Stale => ActivationToolErrorV1::StaleCursor,
        ActivationTerminalOutcomeV1::AlreadyCompleted => ActivationToolErrorV1::AlreadyCompleted,
    }
}

fn map_ledger_error(error: ActivationLedgerErrorV1) -> ActivationToolErrorV1 {
    match error {
        ActivationLedgerErrorV1::Conflict => ActivationToolErrorV1::IdempotencyConflict,
        ActivationLedgerErrorV1::Unavailable => ActivationToolErrorV1::Unavailable,
        ActivationLedgerErrorV1::InvalidData => ActivationToolErrorV1::InvalidRetainedState,
    }
}

fn next_protocol_ulid() -> Result<UlidString, RunnerActivationGatewayErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RunnerActivationGatewayErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    std::str::from_utf8(&encoded)
        .map_err(|_| RunnerActivationGatewayErrorV1::Unavailable)?
        .parse()
        .map_err(|_| RunnerActivationGatewayErrorV1::Unavailable)
}

fn bounded_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_LABEL_BYTES && !value.chars().any(char::is_control)
}

fn bounded_disposition(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DISPOSITION_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == "blake3:".len() + 64
        && value.starts_with("blake3:")
        && value["blake3:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_utc_timestamp_nanos(value: &str) -> Result<i128, ()> {
    let bytes = value.as_bytes();
    if !(bytes.len() == 20 || (22..=30).contains(&bytes.len()))
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || bytes.last() != Some(&b'Z')
    {
        return Err(());
    }
    let year = parse_digits(bytes.get(0..4).ok_or(())?)?;
    let month = parse_digits(bytes.get(5..7).ok_or(())?)?;
    let day = parse_digits(bytes.get(8..10).ok_or(())?)?;
    let hour = parse_digits(bytes.get(11..13).ok_or(())?)?;
    let minute = parse_digits(bytes.get(14..16).ok_or(())?)?;
    let second = parse_digits(bytes.get(17..19).ok_or(())?)?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(());
    }
    let fractional_nanos = if bytes.len() == 20 {
        0
    } else {
        if bytes.get(19) != Some(&b'.') {
            return Err(());
        }
        let fraction = &bytes[20..bytes.len() - 1];
        if fraction.is_empty()
            || fraction.len() > 9
            || !fraction.iter().all(u8::is_ascii_digit)
            || fraction.last() == Some(&b'0')
        {
            return Err(());
        }
        parse_digits(fraction)? * 10_u32.pow(9_u32 - u32::try_from(fraction.len()).map_err(|_| ())?)
    };
    let days = days_from_civil(i64::from(year), i64::from(month), i64::from(day));
    let seconds = i128::from(days)
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(i128::from(hour) * 3_600))
        .and_then(|value| value.checked_add(i128::from(minute) * 60))
        .and_then(|value| value.checked_add(i128::from(second)))
        .ok_or(())?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(fractional_nanos)))
        .ok_or(())
}

fn parse_digits(bytes: &[u8]) -> Result<u32, ()> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(());
    }
    bytes.iter().try_fold(0_u32, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(*byte - b'0')))
            .ok_or(())
    })
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[allow(dead_code)]
fn _bearer_type_separation(_: &BearerWireV1) {}
