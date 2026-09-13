//! Storage-neutral Activation contracts.
//!
//! Attention is canonical pack evidence.  Everything after that boundary is
//! operational state: policy evidence, intents, leases, receipts, and the
//! exact context handed to a Runner.  This module intentionally contains no
//! scheduler, Runner, or database behavior.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Blake3DigestV1, CanonicalJsonError, CanonicalJsonV1, CanonicalRequestHashV1, CompleteHeadV1,
    MemberId, RoomSequenceV1, TimerScheduledFor, canonical::encode,
};

/// Maximum canonical byte length of one retained Invocation Context.
///
/// This is a transport bound, not a Runner tokenizer allowance. Runner prompt
/// construction must select and enforce its own token budget.
pub const MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES: usize = 512 * 1024;

/// The operational policy revision for bounded Activation attention.
pub const ACTIVATION_ATTENTION_POLICY_REVISION_V1: u64 = 1;
/// Maximum number of pending refresh considerations for one Membership.
pub const MAX_PENDING_REFRESH_ACTIVATIONS_V1: u64 = 64;
/// Maximum encoded attention bytes retained for pending refresh work.
pub const MAX_PENDING_REFRESH_BYTES_V1: u64 = 1_048_576;
/// Maximum age of a pending refresh consideration before it is terminally retired.
pub const MAX_PENDING_REFRESH_AGE_MS_V1: u64 = 86_400_000;
/// Maximum completed Activation executions per Membership in one minute.
pub const MAX_ACTIVATION_EXECUTIONS_PER_MINUTE_V1: u64 = 60;
/// Fixed estimate for the durable row envelope around reason and deduplication text.
pub const ACTIVATION_ATTENTION_ROW_OVERHEAD_BYTES_V1: u64 = 128;

/// Returns whether one more pending refresh fits the v1 count and byte budget.
#[must_use]
pub const fn activation_refresh_budget_allows_v1(
    pending_count: u64,
    pending_bytes: u64,
    incoming_bytes: u64,
) -> bool {
    pending_count < MAX_PENDING_REFRESH_ACTIVATIONS_V1
        && pending_bytes.saturating_add(incoming_bytes) <= MAX_PENDING_REFRESH_BYTES_V1
}

/// Operational classification used by the bounded attention policy.
///
/// Refreshable work is a request to reconsider current state and can be
/// superseded by a newer refresh. An obligation carries a semantic deadline
/// and remains pending until it is completed, expired, or explicitly
/// cancelled by its governing Room/Membership lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationAttentionClassV1 {
    Refreshable,
    Obligation,
}

impl ActivationAttentionClassV1 {
    #[must_use]
    pub const fn from_semantic_deadline(semantic_deadline: Option<&TimerScheduledFor>) -> Self {
        if semantic_deadline.is_some() {
            Self::Obligation
        } else {
            Self::Refreshable
        }
    }
}

const ACTIVATION_ID_DOMAIN_V1: &str = "worldstream/activation-id/v1";
const CROCKFORD_BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Serialize)]
struct ActivationIdInputV1<'a> {
    domain: &'static str,
    cause_room_seq: u64,
    target_member_id: &'a str,
    deduplication_key: &'a str,
}

/// Derives the stable opaque ULID-shaped identity of one canonical Attention.
///
/// The identifier is deterministic for recovery, but its 128 hash-derived
/// bits do not embed the Membership or pack-owned deduplication text.
///
/// # Errors
///
/// Returns an error if the canonical domain-separated input cannot be encoded.
pub fn activation_id_for_attention_v1(
    cause_room_seq: RoomSequenceV1,
    target_member_id: &MemberId,
    deduplication_key: &str,
) -> Result<String, CanonicalJsonError> {
    let input = CanonicalJsonV1::from_serialize(&ActivationIdInputV1 {
        domain: ACTIVATION_ID_DOMAIN_V1,
        cause_room_seq: cause_room_seq.get(),
        target_member_id: target_member_id.as_str(),
        deduplication_key,
    })?
    .to_bytes()?;
    let digest = blake3::hash(&input);
    let mut value_bytes = [0_u8; 16];
    value_bytes.copy_from_slice(&digest.as_bytes()[..16]);
    let mut value = u128::from_be_bytes(value_bytes);
    let mut encoded = [b'0'; 26];
    for position in (0..encoded.len()).rev() {
        let index =
            usize::try_from(value & 31).unwrap_or_else(|_| unreachable!("five bits fit usize"));
        encoded[position] = CROCKFORD_BASE32[index];
        value >>= 5;
    }
    Ok(String::from_utf8(encoded.to_vec())
        .unwrap_or_else(|_| unreachable!("Crockford alphabet is valid UTF-8")))
}

/// One deterministic Attention Signal normalized from pack output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationAttentionV1 {
    pub target_member_id: MemberId,
    #[serde(rename = "reason")]
    pub reason_code: String,
    pub deduplication_key: String,
    pub priority: u32,
    #[serde(rename = "deadline")]
    pub semantic_deadline: Option<TimerScheduledFor>,
    /// Pack-declared Action types that can address this Attention. This is a
    /// bounded hint for the Runner and remains opaque to Core legality.
    #[serde(default)]
    pub action_types: Vec<String>,
}

impl ActivationAttentionV1 {
    /// Decodes the pack's canonical Attention representation without changing
    /// its semantic values.
    ///
    /// # Errors
    ///
    /// Returns an error when the canonical bytes cannot be decoded or when a
    /// bounded Attention text field is invalid.
    pub fn from_canonical(value: &CanonicalJsonV1) -> Result<Self, ActivationShapeErrorV1> {
        let bytes = value
            .to_bytes()
            .map_err(ActivationShapeErrorV1::Canonical)?;
        let attention: Self =
            CanonicalJsonV1::decode_canonical(&bytes).map_err(ActivationShapeErrorV1::Canonical)?;
        if attention.reason_code.is_empty()
            || attention.deduplication_key.is_empty()
            || attention.reason_code.len() > 128
            || attention.deduplication_key.len() > 256
            || attention.action_types.len() > 32
            || attention
                .action_types
                .iter()
                .any(|action_type| action_type.is_empty() || action_type.len() > 256)
        {
            return Err(ActivationShapeErrorV1::InvalidText);
        }
        Ok(attention)
    }

    ///
    /// # Errors
    ///
    /// Returns an error if this Attention cannot be represented as canonical
    /// JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }
}

/// The only policy dispositions that can be recorded beside an Attention.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationPolicyDispositionV1 {
    Deny,
    Intent,
}

/// Versioned operational policy evidence.  It is never part of a lineage
/// hash and Replay only verifies the recorded value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationPolicyDecisionV1 {
    pub policy_revision: u64,
    pub disposition: ActivationPolicyDispositionV1,
    pub maximum_lease_ms: u64,
}

/// The exact non-canonical decision persisted beside a causing Transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationDecisionV1 {
    pub decision_id: String,
    pub activation_id: Option<String>,
    pub cause_room_seq: RoomSequenceV1,
    pub attention: ActivationAttentionV1,
    pub policy: ActivationPolicyDecisionV1,
}

/// A lifecycle state of one durable Activation Intent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationIntentStateV1 {
    Pending,
    Leased,
    Completed,
    Expired,
    Cancelled,
}

/// Why an operational activation operation resolved.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationResultCodeV1 {
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

/// One exact retained delivery branch in an Invocation Context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationDeliveryV1 {
    RetainedFrames {
        cursor_exclusive: u64,
        through_frame_head: u64,
        frames: Vec<ActivationFrameV1>,
    },
    ProjectionReset {
        baseline_frame_head: u64,
        reason: String,
    },
}

/// One exact frame payload retained in an Invocation Context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationFrameV1 {
    pub frame_seq: u64,
    pub cause_room_seq: RoomSequenceV1,
    pub payload_hash: Blake3DigestV1,
    pub payload_bytes: Vec<u8>,
}

/// Pure input for exact context preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationContextInputV1 {
    pub activation_id: String,
    pub claim_id: String,
    pub cause_room_seq: RoomSequenceV1,
    pub reason_code: String,
    pub lease_generation: u64,
    pub lease_until: String,
    pub semantic_deadline: Option<TimerScheduledFor>,
    pub room_head: CompleteHeadV1,
    pub integrity_generation: u64,
    pub policy_revision: u64,
    pub authority_generation: u64,
    pub membership_generation: u64,
    pub frame_head: u64,
    pub retained_floor: u64,
    pub cursor: Option<u64>,
    pub projection_schema: String,
    pub projection_bytes: Vec<u8>,
    pub action_offers_bytes: Vec<u8>,
    pub runner_budget_bytes: Vec<u8>,
    pub runner_limits_bytes: Vec<u8>,
    pub artifact_references: Vec<CanonicalJsonV1>,
    pub delivery: ActivationDeliveryV1,
}

/// Exact byte-preserved private Invocation Context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationInvocationContextV1 {
    pub activation_id: String,
    pub claim_id: String,
    pub cause_room_seq: RoomSequenceV1,
    pub reason_code: String,
    pub lease_generation: u64,
    pub lease_until: String,
    pub semantic_deadline: Option<TimerScheduledFor>,
    pub room_head: CompleteHeadV1,
    pub integrity_generation: u64,
    pub policy_revision: u64,
    pub authority_generation: u64,
    pub membership_generation: u64,
    pub frame_head: u64,
    pub retained_floor: u64,
    pub cursor: Option<u64>,
    pub projection_schema: String,
    pub projection_bytes: Vec<u8>,
    pub action_offers_bytes: Vec<u8>,
    pub runner_budget_bytes: Vec<u8>,
    pub runner_limits_bytes: Vec<u8>,
    pub artifact_references: Vec<CanonicalJsonV1>,
    pub delivery: ActivationDeliveryV1,
}

impl ActivationInvocationContextV1 {
    /// Returns the hash of the exact serialized context bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the context cannot be represented as canonical
    /// JSON.
    pub fn context_hash(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&self.canonical_bytes()?))
    }

    /// Returns the exact canonical bytes of this invocation context.
    ///
    /// # Errors
    ///
    /// Returns an error if the context cannot be represented as canonical
    /// JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }
}

/// Pure context preparation failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ActivationContextErrorV1 {
    #[error("activation context field is empty or exceeds its bound")]
    InvalidField,
    #[error("activation context frame positions are inconsistent")]
    InvalidDelivery,
    #[error("activation context bytes are not canonical JSON")]
    InvalidCanonicalBytes,
    #[error("activation context exceeds the aggregate byte budget")]
    TooLarge,
    #[error("activation context could not be canonically encoded: {0}")]
    Canonical(#[from] CanonicalJsonError),
}

/// Prepares an exact context without consulting storage or a clock.
///
/// # Errors
///
/// Returns an error when a required field, canonical payload, or delivery
/// branch is invalid.
pub fn prepare_activation_context(
    input: ActivationContextInputV1,
) -> Result<ActivationInvocationContextV1, ActivationContextErrorV1> {
    prepare_activation_context_with_byte_limit(input, MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES)
}

/// Prepares an exact context under an explicit aggregate canonical-byte limit.
///
/// The production claim path uses [`MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES`];
/// the explicit form keeps boundary verification independent of that default.
/// This is still a transport-byte bound, not a model tokenizer budget.
pub fn prepare_activation_context_with_byte_limit(
    input: ActivationContextInputV1,
    maximum_bytes: usize,
) -> Result<ActivationInvocationContextV1, ActivationContextErrorV1> {
    if input.activation_id.is_empty()
        || input.claim_id.is_empty()
        || input.reason_code.is_empty()
        || input.lease_until.is_empty()
        || input.projection_schema.is_empty()
        || input.projection_bytes.is_empty()
        || input.action_offers_bytes.is_empty()
        || input.runner_budget_bytes.is_empty()
        || input.runner_limits_bytes.is_empty()
        || input.frame_head < input.retained_floor.saturating_sub(1)
        || input.cursor.is_some_and(|cursor| cursor > input.frame_head)
        || CanonicalJsonV1::from_canonical_bytes(&input.projection_bytes).is_err()
        || CanonicalJsonV1::from_canonical_bytes(&input.action_offers_bytes).is_err()
        || CanonicalJsonV1::from_canonical_bytes(&input.runner_budget_bytes).is_err()
        || CanonicalJsonV1::from_canonical_bytes(&input.runner_limits_bytes).is_err()
    {
        return Err(ActivationContextErrorV1::InvalidField);
    }
    match &input.delivery {
        ActivationDeliveryV1::RetainedFrames {
            cursor_exclusive,
            through_frame_head,
            frames,
        } => {
            if *through_frame_head != input.frame_head
                || *cursor_exclusive != input.cursor.unwrap_or(0)
                || frames
                    .windows(2)
                    .any(|pair| pair[0].frame_seq >= pair[1].frame_seq)
                || frames.iter().any(|frame| {
                    frame.frame_seq <= *cursor_exclusive
                        || frame.frame_seq > *through_frame_head
                        || frame.payload_bytes.is_empty()
                })
            {
                return Err(ActivationContextErrorV1::InvalidDelivery);
            }
        }
        ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head,
            reason,
        } if *baseline_frame_head != input.frame_head || reason.is_empty() => {
            return Err(ActivationContextErrorV1::InvalidDelivery);
        }
        ActivationDeliveryV1::ProjectionReset { .. } => {}
    }
    let context = ActivationInvocationContextV1 {
        activation_id: input.activation_id,
        claim_id: input.claim_id,
        cause_room_seq: input.cause_room_seq,
        reason_code: input.reason_code,
        lease_generation: input.lease_generation,
        lease_until: input.lease_until,
        semantic_deadline: input.semantic_deadline,
        room_head: input.room_head,
        integrity_generation: input.integrity_generation,
        policy_revision: input.policy_revision,
        authority_generation: input.authority_generation,
        membership_generation: input.membership_generation,
        frame_head: input.frame_head,
        retained_floor: input.retained_floor,
        cursor: input.cursor,
        projection_schema: input.projection_schema,
        projection_bytes: input.projection_bytes,
        action_offers_bytes: input.action_offers_bytes,
        runner_budget_bytes: input.runner_budget_bytes,
        runner_limits_bytes: input.runner_limits_bytes,
        artifact_references: input.artifact_references,
        delivery: input.delivery,
    };
    if context.canonical_bytes()?.len() > maximum_bytes {
        return Err(ActivationContextErrorV1::TooLarge);
    }
    Ok(context)
}

/// A canonical request envelope used by all Activation operation receipts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOperationRequestV1 {
    pub operation_kind: String,
    pub operation_id: String,
    pub activation_id: Option<String>,
    pub claim_id: Option<String>,
    pub runner_id: String,
    pub lease_generation: Option<u64>,
    pub requested_lease_ms: Option<u64>,
    pub disposition: Option<String>,
}

impl ActivationOperationRequestV1 {
    /// Derives the versioned operation hash from the complete authenticated
    /// request.  The Runner identity is caller-authenticated input.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be represented as canonical
    /// JSON.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        Ok(CanonicalRequestHashV1::calculate_canonical(&encode(self)?))
    }
}

/// A safe operation result returned by `SQLite` and retained in its receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationOperationResultV1 {
    pub operation_id: String,
    pub activation_id: Option<String>,
    pub claim_id: Option<String>,
    pub runner_id: String,
    pub code: ActivationResultCodeV1,
    pub state: Option<ActivationIntentStateV1>,
    pub lease_generation: Option<u64>,
    pub context_hash: Option<Blake3DigestV1>,
    pub context: Option<ActivationInvocationContextV1>,
}

/// Shape failures while normalizing canonical Attention evidence.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ActivationShapeErrorV1 {
    #[error("Attention canonical bytes are invalid: {0}")]
    Canonical(#[from] CanonicalJsonError),
    #[error("Attention text field is invalid")]
    InvalidText,
}
