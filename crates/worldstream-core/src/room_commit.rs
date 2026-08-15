//! Backend-neutral prepared Room Commit seam.
//!
//! Callers prepare complete immutable write bundles before crossing this seam.
//! Storage adapters own identity serialization, transaction ordering, witness
//! checks, atomic persistence, duplicate resolution, and unknown-COMMIT
//! handling. No SQL-shaped operation is exposed here.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ACTION_OFFER_DOMAIN, AccessModeV1, ActionAdmittedAt, ActionId, ActionOfferV1,
    ActivityObservationOutcomeV1, AdministrationOperationIdentityV1, Blake3DigestV1,
    CanonicalJsonError, CanonicalJsonV1, CompleteHeadV1, CoreProposedV1, CoreRecordedAt,
    CoreRoomStateV1, CoreTraceV1, CreationRecordedAt, ExternalInputRecordedAt, ExternalInputV1,
    GenesisV1, InputId, IntegrityGenerationV1, MemberId, MembershipStandingV1, MembershipV1,
    PackDigestV1, PackRegistryV1, PackRevisionLockV1, PackViewerV1, ParticipantActionV1,
    PreparedNewRoomGenesisV1, PrincipalId, PrincipalKindV1, RecordedStimulusV1,
    ReplayFailureClassV1, RoomId, RoomSequenceV1, RoomStatusV1, SourceId, TimerChangeV1,
    TimerFiredV1, TimerGenerationV1, TimerId, TimerScheduledFor, TraceErrorV1, TransitionId,
    TransitionV1,
    activity_pack::ValidatedPackObservationV1,
    canonical::encode,
    primitives::{DigestParseError, compare_timestamp_text},
    trace::{AdvanceDispositionV1, PreparedRoomTransitionV1},
};

const SEMANTIC_RECEIPT_DOMAIN: &str = "worldstream/semantic-result/v1";
const OPERATION_RECEIPT_CODEC_ID: &str = "worldstream/operation-receipt/v1";
const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

/// Frozen operation kind for the sole no-basis Room creation operation.
pub const CREATE_ROOM_OPERATION_KIND: &str = "worldstream/create-room/v1";

/// Domain-specific hash of one versioned caller-semantic canonical request.
/// It cannot be substituted with a pack, state, lineage, or artifact digest.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalRequestHashV1(Blake3DigestV1);

impl CanonicalRequestHashV1 {
    fn calculate(request: &CanonicalJsonV1) -> Result<Self, CanonicalJsonError> {
        Ok(Self(Blake3DigestV1::hash(&request.to_bytes()?)))
    }

    /// Returns the digest bytes for storage comparison.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

/// One caller-semantic initial Membership proposal. Generated Member IDs are
/// deliberately absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialMembershipProposalV1 {
    principal_id: PrincipalId,
    principal_kind: PrincipalKindV1,
    standing: MembershipStandingV1,
    access_mode: AccessModeV1,
    role: Option<String>,
}

impl InitialMembershipProposalV1 {
    /// Constructs one initial proposal with the same participant/Role shape as
    /// a canonical Membership.
    ///
    /// # Errors
    ///
    /// Returns an error if participant and Role presence disagree.
    pub fn new(
        principal_id: PrincipalId,
        principal_kind: PrincipalKindV1,
        standing: MembershipStandingV1,
        access_mode: AccessModeV1,
        role: Option<String>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if matches!(access_mode, AccessModeV1::Participant) != role.is_some() {
            return Err(PrepareRoomWriteErrorV1::InvalidInitialMembershipProposal);
        }
        Ok(Self {
            principal_id,
            principal_kind,
            standing,
            access_mode,
            role,
        })
    }

    /// Returns whether one generated Membership exactly realizes this proposal.
    #[must_use]
    pub fn matches_membership(&self, membership: &MembershipV1) -> bool {
        self.principal_id == *membership.principal_id()
            && self.principal_kind == membership.principal_kind()
            && self.standing == membership.standing()
            && self.access_mode == membership.access_mode()
            && self.role.as_deref() == membership.role()
    }

    fn has_valid_shape(&self) -> bool {
        matches!(self.access_mode, AccessModeV1::Participant) == self.role.is_some()
    }

    #[must_use]
    pub const fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
}

/// Closed caller-semantic creation request. Generated Room/Member IDs, seed,
/// creation time, and commit time cannot enter its hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCreationRequestV1 {
    pack_digest: PackDigestV1,
    configuration: CanonicalJsonV1,
    ordered_initial_memberships: Vec<InitialMembershipProposalV1>,
}

impl RoomCreationRequestV1 {
    /// Constructs an exact ordered creation request.
    #[must_use]
    pub fn new(
        pack_digest: PackDigestV1,
        configuration: CanonicalJsonV1,
        ordered_initial_memberships: Vec<InitialMembershipProposalV1>,
    ) -> Self {
        Self {
            pack_digest,
            configuration,
            ordered_initial_memberships,
        }
    }

    /// Computes the sole request hash accepted by creation preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed request cannot be canonically encoded.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        #[derive(Serialize)]
        struct HashInput<'a> {
            domain: &'static str,
            pack_digest: &'a PackDigestV1,
            configuration: &'a CanonicalJsonV1,
            ordered_initial_memberships: &'a [InitialMembershipProposalV1],
        }
        self.validate_shape()?;
        let canonical = CanonicalJsonV1::from_canonical_bytes(&encode(&HashInput {
            domain: "worldstream/create-room-request/v1",
            pack_digest: &self.pack_digest,
            configuration: &self.configuration,
            ordered_initial_memberships: &self.ordered_initial_memberships,
        })?)?;
        CanonicalRequestHashV1::calculate(&canonical)
    }

    fn validate_shape(&self) -> Result<(), CanonicalJsonError> {
        if self.ordered_initial_memberships.is_empty()
            || self
                .ordered_initial_memberships
                .iter()
                .any(|proposal| !proposal.has_valid_shape())
        {
            return Err(CanonicalJsonError::TypedDecode(
                "creation request has an invalid initial Membership shape".to_owned(),
            ));
        }
        let mut principals = std::collections::BTreeSet::new();
        if !self
            .ordered_initial_memberships
            .iter()
            .all(|proposal| principals.insert(proposal.principal_id.clone()))
        {
            return Err(CanonicalJsonError::TypedDecode(
                "creation request repeats an initial Principal".to_owned(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn pack_digest(&self) -> &PackDigestV1 {
        &self.pack_digest
    }

    #[must_use]
    pub const fn configuration(&self) -> &CanonicalJsonV1 {
        &self.configuration
    }

    #[must_use]
    pub fn ordered_initial_memberships(&self) -> &[InitialMembershipProposalV1] {
        &self.ordered_initial_memberships
    }
}

impl fmt::Display for CanonicalRequestHashV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CanonicalRequestHashV1 {
    type Err = DigestParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl Serialize for CanonicalRequestHashV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CanonicalRequestHashV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Participant Action operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantActionOperationIdentityV1 {
    pub room_id: RoomId,
    pub member_id: MemberId,
    pub action_id: ActionId,
}

/// Closed caller request available before lane admission records Semantic
/// Time or resolves a full host Head. The Action ID is its identity component;
/// it is deliberately excluded from the versioned request hash bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantActionRequestV1 {
    room_id: RoomId,
    member_id: MemberId,
    action_id: ActionId,
    based_on_room_seq: RoomSequenceV1,
    action_type: String,
    payload: CanonicalJsonV1,
}

impl ParticipantActionRequestV1 {
    #[must_use]
    pub fn new(
        room_id: RoomId,
        member_id: MemberId,
        action_id: ActionId,
        based_on_room_seq: RoomSequenceV1,
        action_type: impl Into<String>,
        payload: CanonicalJsonV1,
    ) -> Self {
        Self {
            room_id,
            member_id,
            action_id,
            based_on_room_seq,
            action_type: action_type.into(),
            payload,
        }
    }

    /// Derives the operation identity before any host timestamp is sampled.
    #[must_use]
    pub fn operation_identity(&self) -> OperationIdentityV1 {
        OperationIdentityV1::ParticipantAction(Box::new(ParticipantActionOperationIdentityV1 {
            room_id: self.room_id.clone(),
            member_id: self.member_id.clone(),
            action_id: self.action_id.clone(),
        }))
    }

    /// Computes the frozen protocol 0.1 Action idempotency hash.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid bounded shape or canonical encoding failure.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        if self.action_type.is_empty()
            || self.action_type.len() > 256
            || self.payload.to_bytes()?.len() > 1_048_576
        {
            return Err(CanonicalJsonError::TypedDecode(
                "Action request exceeds its closed shape bounds".to_owned(),
            ));
        }
        action_request_hash(self)
    }

    pub(crate) const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    pub(crate) const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    pub(crate) const fn action_id(&self) -> &ActionId {
        &self.action_id
    }

    pub(crate) const fn based_on_room_seq(&self) -> RoomSequenceV1 {
        self.based_on_room_seq
    }

    pub(crate) fn action_type(&self) -> &str {
        &self.action_type
    }

    pub(crate) const fn payload(&self) -> &CanonicalJsonV1 {
        &self.payload
    }

    fn matches_normalized(&self, action: &ParticipantActionV1) -> bool {
        self.room_id == *action.exact_basis_head.room_id()
            && self.member_id == action.member_id
            && self.action_id == action.action_id
            && self.based_on_room_seq == action.exact_basis_head.room_seq()
            && self.action_type == action.action_type
            && self.payload == action.canonical_payload
    }
}

/// Exact Timer generation operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerOperationIdentityV1 {
    pub room_id: RoomId,
    pub timer_id: TimerId,
    pub generation: TimerGenerationV1,
}

/// Immutable scheduled Timer candidate whose identity/hash can be resolved
/// before loading a current Room Head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFiredRequestV1 {
    room_id: RoomId,
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload: CanonicalJsonV1,
}

impl TimerFiredRequestV1 {
    #[must_use]
    pub fn new(
        room_id: RoomId,
        timer_id: TimerId,
        generation: TimerGenerationV1,
        scheduled_for: TimerScheduledFor,
        canonical_payload: CanonicalJsonV1,
    ) -> Self {
        Self {
            room_id,
            timer_id,
            generation,
            scheduled_for,
            canonical_payload,
        }
    }

    #[must_use]
    pub fn operation_identity(&self) -> OperationIdentityV1 {
        OperationIdentityV1::TimerFired(Box::new(TimerOperationIdentityV1 {
            room_id: self.room_id.clone(),
            timer_id: self.timer_id.clone(),
            generation: self.generation,
        }))
    }

    /// Computes the frozen Timer candidate idempotency hash.
    ///
    /// # Errors
    ///
    /// Returns an error if the immutable Timer request cannot be canonically encoded.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        timer_request_hash(self)
    }

    pub(crate) fn recorded_stimulus(&self) -> TimerFiredV1 {
        TimerFiredV1 {
            timer_id: self.timer_id.clone(),
            generation: self.generation,
            scheduled_for: self.scheduled_for.clone(),
            canonical_payload: self.canonical_payload.clone(),
        }
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn timer_id(&self) -> &TimerId {
        &self.timer_id
    }

    #[must_use]
    pub const fn generation(&self) -> TimerGenerationV1 {
        self.generation
    }

    #[must_use]
    pub const fn scheduled_for(&self) -> &TimerScheduledFor {
        &self.scheduled_for
    }

    #[must_use]
    pub const fn canonical_payload(&self) -> &CanonicalJsonV1 {
        &self.canonical_payload
    }
}

/// Immutable external-input operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalInputOperationIdentityV1 {
    pub room_id: RoomId,
    pub source_id: SourceId,
    pub input_id: InputId,
}

/// One durable semantic-operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "identity_type", content = "identity", rename_all = "snake_case")]
pub enum OperationIdentityV1 {
    ParticipantAction(Box<ParticipantActionOperationIdentityV1>),
    Administration(Box<AdministrationOperationIdentityV1>),
    TimerFired(Box<TimerOperationIdentityV1>),
    ExternalInput(Box<ExternalInputOperationIdentityV1>),
}

impl OperationIdentityV1 {
    /// Constructs the exact Action identity from a normalized Action.
    #[must_use]
    pub fn participant_action(room_id: RoomId, action: &ParticipantActionV1) -> Self {
        Self::ParticipantAction(Box::new(ParticipantActionOperationIdentityV1 {
            room_id,
            member_id: action.member_id.clone(),
            action_id: action.action_id.clone(),
        }))
    }

    /// Returns the target Room when the identity is Room-scoped.
    #[must_use]
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Self::ParticipantAction(identity) => Some(&identity.room_id),
            Self::TimerFired(identity) => Some(&identity.room_id),
            Self::ExternalInput(identity) => Some(&identity.room_id),
            Self::Administration(_) => None,
        }
    }

    /// Returns the canonical identity bytes used as the durable guard key.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed identity cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    /// Returns the stable receipt-table discriminator for this identity.
    #[must_use]
    pub const fn operation_kind(&self) -> &'static str {
        match self {
            Self::ParticipantAction(_) => "action",
            Self::Administration(_) => "administration",
            Self::TimerFired(_) => "timer_fired",
            Self::ExternalInput(_) => "external_input",
        }
    }
}

/// Operational authority state observed before preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedAuthorityWitnessV1 {
    witness_id: String,
    authenticated_principal: PrincipalId,
    generation: u64,
    canonical_scope_revocation_bytes: Vec<u8>,
    scope_revocation_hash: Blake3DigestV1,
}

impl PreparedAuthorityWitnessV1 {
    /// Seals an opaque authority/capability scope and revocation witness.
    /// Storage compares the ID, Principal, generation, and digest with its
    /// independently maintained authority fence; it never interprets policy.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or a value outside canonical safe integers.
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub(crate) fn new(
        witness_id: impl Into<String>,
        authenticated_principal: PrincipalId,
        generation: u64,
        canonical_scope_revocation: &CanonicalJsonV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let witness_id = witness_id.into();
        if witness_id.is_empty() || generation == 0 || generation > MAX_SAFE_INTEGER_U64 {
            return Err(PrepareRoomWriteErrorV1::InvalidAuthorityGeneration);
        }
        let canonical_scope_revocation_bytes = canonical_scope_revocation.to_bytes()?;
        let scope_revocation_hash = Blake3DigestV1::hash(&canonical_scope_revocation_bytes);
        Ok(Self {
            witness_id,
            authenticated_principal,
            generation,
            canonical_scope_revocation_bytes,
            scope_revocation_hash,
        })
    }

    /// Mints an opaque witness only for cross-crate storage conformance tests.
    /// Production authority owners must provide the internal issuer seam.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty witness ID or invalid generation.
    #[cfg(feature = "conformance-tracer")]
    pub fn mint_for_conformance(
        witness_id: impl Into<String>,
        authenticated_principal: PrincipalId,
        generation: u64,
        canonical_scope_revocation: &CanonicalJsonV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::new(
            witness_id,
            authenticated_principal,
            generation,
            canonical_scope_revocation,
        )
    }

    #[must_use]
    pub fn witness_id(&self) -> &str {
        &self.witness_id
    }

    #[must_use]
    pub const fn authenticated_principal(&self) -> &PrincipalId {
        &self.authenticated_principal
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn canonical_scope_revocation_bytes(&self) -> &[u8] {
        &self.canonical_scope_revocation_bytes
    }

    #[must_use]
    pub const fn scope_revocation_hash(&self) -> &Blake3DigestV1 {
        &self.scope_revocation_hash
    }
}

/// One exact prepared Membership materialization.
#[derive(Clone, Debug)]
pub struct PreparedMembershipMaterializationV1 {
    pub membership: MembershipV1,
    pub canonical_membership_bytes: Vec<u8>,
}

/// One addressed, coalesced Observation Frame.
#[derive(Clone, Debug)]
pub struct PreparedObservationFrameV1 {
    member_id: MemberId,
    previous_frame_head: u64,
    frame_seq: u64,
    cause_room_seq: RoomSequenceV1,
    canonical_payload_bytes: Vec<u8>,
    payload_hash: Blake3DigestV1,
}

impl PreparedObservationFrameV1 {
    fn from_validated_observation(
        member_id: MemberId,
        previous_frame_head: u64,
        cause_room_seq: RoomSequenceV1,
        observation: &ValidatedPackObservationV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let frame_seq = previous_frame_head
            .checked_add(1)
            .filter(|value| *value <= MAX_SAFE_INTEGER_U64)
            .ok_or(PrepareRoomWriteErrorV1::FrameSequenceExhausted)?;
        let canonical_payload_bytes = observation.canonical_bytes().to_vec();
        let payload_hash = Blake3DigestV1::hash(&canonical_payload_bytes);
        Ok(Self {
            member_id,
            previous_frame_head,
            frame_seq,
            cause_room_seq,
            canonical_payload_bytes,
            payload_hash,
        })
    }

    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub const fn previous_frame_head(&self) -> u64 {
        self.previous_frame_head
    }

    #[must_use]
    pub const fn frame_seq(&self) -> u64 {
        self.frame_seq
    }

    #[must_use]
    pub const fn cause_room_seq(&self) -> RoomSequenceV1 {
        self.cause_room_seq
    }

    #[must_use]
    pub fn canonical_payload_bytes(&self) -> &[u8] {
        &self.canonical_payload_bytes
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &Blake3DigestV1 {
        &self.payload_hash
    }
}

/// One already-decided operational Activation consequence. Counter produces
/// none, but the Advance bundle keeps the common seam complete.
#[derive(Clone, Debug)]
pub struct PreparedActivationDecisionV1 {
    decision_id: String,
    target_member_id: Option<MemberId>,
    canonical_decision_bytes: Vec<u8>,
}

impl PreparedActivationDecisionV1 {
    #[must_use]
    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }

    #[must_use]
    pub const fn target_member_id(&self) -> Option<&MemberId> {
        self.target_member_id.as_ref()
    }

    #[must_use]
    pub fn canonical_decision_bytes(&self) -> &[u8] {
        &self.canonical_decision_bytes
    }
}

/// Exact initial/current scheduled Timer row, including prepared payload bytes.
#[derive(Clone, Debug)]
pub struct PreparedTimerMaterializationV1 {
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload_bytes: Vec<u8>,
}

impl PreparedTimerMaterializationV1 {
    fn from_scheduled(timer: &crate::ScheduledTimerV1) -> Result<Self, CanonicalJsonError> {
        Ok(Self {
            timer_id: timer.timer_id.clone(),
            generation: timer.generation,
            scheduled_for: timer.scheduled_for.clone(),
            canonical_payload_bytes: timer.canonical_payload.to_bytes()?,
        })
    }

    #[must_use]
    pub const fn timer_id(&self) -> &TimerId {
        &self.timer_id
    }

    #[must_use]
    pub const fn generation(&self) -> TimerGenerationV1 {
        self.generation
    }

    #[must_use]
    pub const fn scheduled_for(&self) -> &TimerScheduledFor {
        &self.scheduled_for
    }

    #[must_use]
    pub fn canonical_payload_bytes(&self) -> &[u8] {
        &self.canonical_payload_bytes
    }
}

/// One fully encoded normalized Timer mutation.
#[derive(Clone, Debug)]
pub enum PreparedTimerMutationV1 {
    Schedule {
        timer_id: TimerId,
        generation: TimerGenerationV1,
        scheduled_for: TimerScheduledFor,
        canonical_payload_bytes: Vec<u8>,
    },
    Cancel {
        timer_id: TimerId,
        generation: TimerGenerationV1,
    },
    Reschedule {
        timer_id: TimerId,
        previous_generation: TimerGenerationV1,
        generation: TimerGenerationV1,
        scheduled_for: TimerScheduledFor,
        canonical_payload_bytes: Vec<u8>,
    },
}

/// Borrowed, read-only shape of a sealed Timer mutation.
pub enum PreparedTimerMutationKindV1<'a> {
    Schedule {
        timer_id: &'a TimerId,
        generation: TimerGenerationV1,
        scheduled_for: &'a TimerScheduledFor,
        canonical_payload_bytes: &'a [u8],
    },
    Cancel {
        timer_id: &'a TimerId,
        generation: TimerGenerationV1,
    },
    Reschedule {
        timer_id: &'a TimerId,
        previous_generation: TimerGenerationV1,
        generation: TimerGenerationV1,
        scheduled_for: &'a TimerScheduledFor,
        canonical_payload_bytes: &'a [u8],
    },
}

impl PreparedTimerMutationV1 {
    fn from_change(change: &crate::TimerChangeV1) -> Result<Self, CanonicalJsonError> {
        Ok(match change {
            crate::TimerChangeV1::Schedule {
                timer_id,
                generation,
                scheduled_for,
                canonical_payload,
            } => Self::Schedule {
                timer_id: timer_id.clone(),
                generation: *generation,
                scheduled_for: scheduled_for.clone(),
                canonical_payload_bytes: canonical_payload.to_bytes()?,
            },
            crate::TimerChangeV1::Cancel {
                timer_id,
                generation,
            } => Self::Cancel {
                timer_id: timer_id.clone(),
                generation: *generation,
            },
            crate::TimerChangeV1::Reschedule {
                timer_id,
                previous_generation,
                generation,
                scheduled_for,
                canonical_payload,
            } => Self::Reschedule {
                timer_id: timer_id.clone(),
                previous_generation: *previous_generation,
                generation: *generation,
                scheduled_for: scheduled_for.clone(),
                canonical_payload_bytes: canonical_payload.to_bytes()?,
            },
        })
    }

    #[must_use]
    pub fn kind(&self) -> PreparedTimerMutationKindV1<'_> {
        match self {
            Self::Schedule {
                timer_id,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => PreparedTimerMutationKindV1::Schedule {
                timer_id,
                generation: *generation,
                scheduled_for,
                canonical_payload_bytes,
            },
            Self::Cancel {
                timer_id,
                generation,
            } => PreparedTimerMutationKindV1::Cancel {
                timer_id,
                generation: *generation,
            },
            Self::Reschedule {
                timer_id,
                previous_generation,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => PreparedTimerMutationKindV1::Reschedule {
                timer_id,
                previous_generation: *previous_generation,
                generation: *generation,
                scheduled_for,
                canonical_payload_bytes,
            },
        }
    }
}

/// Typed semantic result retained for identity resolution after restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticResultV1 {
    GenesisCreated {
        room_id: RoomId,
        initial_member_ids: Vec<MemberId>,
        complete_head: CompleteHeadV1,
    },
    TransitionCommitted {
        room_id: RoomId,
        transition_id: TransitionId,
        room_seq: RoomSequenceV1,
        previous_lineage_hash: Blake3DigestV1,
        complete_head: CompleteHeadV1,
    },
    RejectionRecorded {
        code: String,
        safe_details: CanonicalJsonV1,
    },
    NoChangeRecorded {
        code: String,
        safe_details: CanonicalJsonV1,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivityDomainRejectionDetailsV1 {
    declared_code: String,
    safe_details: CanonicalJsonV1,
}

/// Variant-specific typed Semantic Time retained in a receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "semantic_time_type",
    content = "value",
    rename_all = "snake_case"
)]
pub enum ReceiptSemanticTimeV1 {
    Creation(CreationRecordedAt),
    ActionAdmitted(ActionAdmittedAt),
    TimerScheduled(TimerScheduledFor),
    CoreRecorded(CoreRecordedAt),
    ExternalInputRecorded(ExternalInputRecordedAt),
}

/// Exact Action Offer evidence available at host pre-admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "offer_witness_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ActionOfferWitnessV1 {
    Current {
        canonical_action_offers: CanonicalJsonV1,
    },
    Unavailable {
        reason: ActionOffersUnavailableReasonV1,
    },
}

/// Closed host reason why no current Action Offer set can exist for a durable
/// stable disposition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOffersUnavailableReasonV1 {
    MembershipNotEnabled,
    RoomArchived,
}

/// Canonical Room/Membership facts that make a host Action disposition stable
/// and independently decodable after restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionAdmissionContextV1 {
    membership_before: MembershipV1,
    room_status: crate::RoomStatusV1,
}

/// Exact committed semantic input retained beside the request hash. The hash
/// binds caller semantics, while this value also retains host-recorded
/// semantic time and the storage-only evidence used during admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "semantic_input_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReceiptSemanticInputV1 {
    RoomCreation {
        request: RoomCreationRequestV1,
        created_at: CreationRecordedAt,
    },
    ParticipantAction {
        request: ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        normalized_action: Option<Box<ParticipantActionV1>>,
        action_offer_witness: ActionOfferWitnessV1,
        admission_context: ActionAdmissionContextV1,
    },
    TimerFired {
        request: TimerFiredRequestV1,
    },
    CoreAdministration {
        proposal: CoreProposedV1,
    },
    ExternalInput {
        room_id: RoomId,
        input: ExternalInputV1,
    },
}

impl ReceiptSemanticInputV1 {
    fn semantic_time(&self) -> ReceiptSemanticTimeV1 {
        match self {
            Self::RoomCreation { created_at, .. } => {
                ReceiptSemanticTimeV1::Creation(created_at.clone())
            }
            Self::ParticipantAction { admitted_at, .. } => {
                ReceiptSemanticTimeV1::ActionAdmitted(admitted_at.clone())
            }
            Self::TimerFired { request } => {
                ReceiptSemanticTimeV1::TimerScheduled(request.scheduled_for.clone())
            }
            Self::CoreAdministration { proposal } => {
                ReceiptSemanticTimeV1::CoreRecorded(proposal.recorded_at().clone())
            }
            Self::ExternalInput { input, .. } => {
                ReceiptSemanticTimeV1::ExternalInputRecorded(input.recorded_at.clone())
            }
        }
    }

    fn request_hash(
        &self,
        basis: Option<&CompleteHeadV1>,
    ) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        match self {
            Self::RoomCreation { request, .. } => {
                if basis.is_some() {
                    return semantic_receipt_mismatch();
                }
                request.canonical_request_hash()
            }
            Self::ParticipantAction { request, .. } => {
                if basis.is_none() {
                    return semantic_receipt_mismatch();
                }
                request.canonical_request_hash()
            }
            Self::TimerFired { request } => {
                let basis = basis.ok_or_else(|| {
                    CanonicalJsonError::TypedDecode(
                        "Timer receipt is missing its basis Head".to_owned(),
                    )
                })?;
                if basis.room_id() != &request.room_id {
                    return semantic_receipt_mismatch();
                }
                request.canonical_request_hash()
            }
            Self::CoreAdministration { proposal } => {
                let basis = basis.ok_or_else(|| {
                    CanonicalJsonError::TypedDecode(
                        "administration receipt is missing its basis Head".to_owned(),
                    )
                })?;
                Ok(CanonicalRequestHashV1(
                    crate::trace::hash_administration_request(basis, proposal)?,
                ))
            }
            Self::ExternalInput { room_id, input } => {
                let basis = basis.ok_or_else(|| {
                    CanonicalJsonError::TypedDecode(
                        "external-input receipt is missing its basis Head".to_owned(),
                    )
                })?;
                if basis.room_id() != room_id {
                    return semantic_receipt_mismatch();
                }
                external_input_request_hash(room_id, basis.room_seq(), input)
            }
        }
    }

    /// Returns the exact canonical semantic-input bytes stored in the receipt
    /// row independently of the complete receipt envelope.
    ///
    /// # Errors
    ///
    /// Returns an error if this validated semantic input cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SemanticReceiptRecordV1 {
    codec_id: String,
    domain: String,
    operation_identity: OperationIdentityV1,
    canonical_request_hash: CanonicalRequestHashV1,
    basis_complete_head: Option<CompleteHeadV1>,
    semantic_input: ReceiptSemanticInputV1,
    semantic_time: ReceiptSemanticTimeV1,
    result: SemanticResultV1,
}

/// Durable, typed semantic result returned through commit or resolve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSemanticResultV1 {
    operation_identity: OperationIdentityV1,
    canonical_request_hash: CanonicalRequestHashV1,
    basis_complete_head: Option<CompleteHeadV1>,
    semantic_input: ReceiptSemanticInputV1,
    semantic_time: ReceiptSemanticTimeV1,
    result: SemanticResultV1,
    canonical_receipt_bytes: Vec<u8>,
}

impl StoredSemanticResultV1 {
    fn prepare(
        operation_identity: OperationIdentityV1,
        basis_complete_head: Option<CompleteHeadV1>,
        semantic_input: ReceiptSemanticInputV1,
        result: SemanticResultV1,
    ) -> Result<Self, CanonicalJsonError> {
        let canonical_request_hash = semantic_input.request_hash(basis_complete_head.as_ref())?;
        let semantic_time = semantic_input.semantic_time();
        validate_semantic_result(
            &operation_identity,
            basis_complete_head.as_ref(),
            &semantic_input,
            &semantic_time,
            &result,
        )?;
        let record = SemanticReceiptRecordV1 {
            codec_id: OPERATION_RECEIPT_CODEC_ID.to_owned(),
            domain: SEMANTIC_RECEIPT_DOMAIN.to_owned(),
            operation_identity: operation_identity.clone(),
            canonical_request_hash: canonical_request_hash.clone(),
            basis_complete_head: basis_complete_head.clone(),
            semantic_input: semantic_input.clone(),
            semantic_time: semantic_time.clone(),
            result: result.clone(),
        };
        Ok(Self {
            operation_identity,
            canonical_request_hash,
            basis_complete_head,
            semantic_input,
            semantic_time,
            result,
            canonical_receipt_bytes: encode(&record)?,
        })
    }

    /// Strictly decodes original stored receipt bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the receipt is noncanonical or has another domain.
    pub fn from_canonical_receipt_bytes(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        let record: SemanticReceiptRecordV1 = CanonicalJsonV1::decode_canonical(input)?;
        if record.codec_id != OPERATION_RECEIPT_CODEC_ID || record.domain != SEMANTIC_RECEIPT_DOMAIN
        {
            return Err(CanonicalJsonError::TypedDecode(
                "semantic receipt codec/domain mismatch".to_owned(),
            ));
        }
        let recomputed_hash = record
            .semantic_input
            .request_hash(record.basis_complete_head.as_ref())?;
        if recomputed_hash != record.canonical_request_hash {
            return Err(CanonicalJsonError::TypedDecode(
                "semantic receipt request hash mismatch".to_owned(),
            ));
        }
        validate_semantic_result(
            &record.operation_identity,
            record.basis_complete_head.as_ref(),
            &record.semantic_input,
            &record.semantic_time,
            &record.result,
        )?;
        Ok(Self {
            operation_identity: record.operation_identity,
            canonical_request_hash: record.canonical_request_hash,
            basis_complete_head: record.basis_complete_head,
            semantic_input: record.semantic_input,
            semantic_time: record.semantic_time,
            result: record.result,
            canonical_receipt_bytes: input.to_vec(),
        })
    }

    #[must_use]
    pub const fn operation_identity(&self) -> &OperationIdentityV1 {
        &self.operation_identity
    }

    #[must_use]
    pub const fn canonical_request_hash(&self) -> &CanonicalRequestHashV1 {
        &self.canonical_request_hash
    }

    #[must_use]
    pub const fn basis_complete_head(&self) -> Option<&CompleteHeadV1> {
        self.basis_complete_head.as_ref()
    }

    #[must_use]
    pub const fn semantic_input(&self) -> &ReceiptSemanticInputV1 {
        &self.semantic_input
    }

    #[must_use]
    pub const fn semantic_time(&self) -> &ReceiptSemanticTimeV1 {
        &self.semantic_time
    }

    /// Returns the canonical basis-Head bytes, or `None` for Room creation.
    ///
    /// # Errors
    ///
    /// Returns an error if the validated basis Head cannot be encoded.
    pub fn canonical_basis_head_bytes(&self) -> Result<Option<Vec<u8>>, CanonicalJsonError> {
        self.basis_complete_head.as_ref().map(encode).transpose()
    }

    /// Returns the exact canonical semantic-time bytes retained in the row.
    ///
    /// # Errors
    ///
    /// Returns an error if the validated semantic time cannot be encoded.
    pub fn canonical_semantic_time_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&self.semantic_time)
    }

    /// Returns the stable database resolution discriminator.
    #[must_use]
    pub const fn resolution_kind(&self) -> &'static str {
        match &self.result {
            SemanticResultV1::GenesisCreated { .. } => "genesis_created",
            SemanticResultV1::TransitionCommitted { .. } => "transition_committed",
            SemanticResultV1::RejectionRecorded { .. } => "rejection_recorded",
            SemanticResultV1::NoChangeRecorded { .. } => "no_change_recorded",
        }
    }

    /// Returns the accepted Transition sequence, if this receipt committed one.
    #[must_use]
    pub const fn transition_seq(&self) -> Option<RoomSequenceV1> {
        match &self.result {
            SemanticResultV1::TransitionCommitted { room_seq, .. } => Some(*room_seq),
            SemanticResultV1::GenesisCreated { .. }
            | SemanticResultV1::RejectionRecorded { .. }
            | SemanticResultV1::NoChangeRecorded { .. } => None,
        }
    }

    /// Returns the result Room ID when the receipt installs canonical state.
    #[must_use]
    pub fn target_room_id(&self) -> &RoomId {
        if let Some(basis) = &self.basis_complete_head {
            return basis.room_id();
        }
        match &self.result {
            SemanticResultV1::GenesisCreated { room_id, .. }
            | SemanticResultV1::TransitionCommitted { room_id, .. } => room_id,
            SemanticResultV1::RejectionRecorded { .. }
            | SemanticResultV1::NoChangeRecorded { .. } => {
                unreachable!("validated no-basis receipt is GenesisCreated")
            }
        }
    }

    #[must_use]
    pub const fn result(&self) -> &SemanticResultV1 {
        &self.result
    }

    #[must_use]
    pub fn canonical_receipt_bytes(&self) -> &[u8] {
        &self.canonical_receipt_bytes
    }
}

#[allow(clippy::too_many_lines)]
fn validate_semantic_result(
    identity: &OperationIdentityV1,
    basis: Option<&CompleteHeadV1>,
    semantic_input: &ReceiptSemanticInputV1,
    semantic_time: &ReceiptSemanticTimeV1,
    result: &SemanticResultV1,
) -> Result<(), CanonicalJsonError> {
    if semantic_input.semantic_time() != *semantic_time {
        return semantic_receipt_mismatch();
    }
    match result {
        SemanticResultV1::RejectionRecorded { code, safe_details }
        | SemanticResultV1::NoChangeRecorded { code, safe_details }
            if code.is_empty() || code.len() > 128 || safe_details.to_bytes()?.len() > 4096 =>
        {
            return semantic_receipt_mismatch();
        }
        _ => {}
    }
    if let SemanticResultV1::RejectionRecorded { code, safe_details } = result
        && code == "activity_domain_rejection"
    {
        let details: ActivityDomainRejectionDetailsV1 =
            CanonicalJsonV1::decode_canonical(&safe_details.to_bytes()?)?;
        if details.declared_code.is_empty() || details.declared_code.len() > 128 {
            return semantic_receipt_mismatch();
        }
    }
    let valid = match (identity, basis, semantic_input, result) {
        (
            OperationIdentityV1::Administration(identity),
            None,
            ReceiptSemanticInputV1::RoomCreation { request, .. },
            SemanticResultV1::GenesisCreated {
                room_id,
                initial_member_ids,
                complete_head,
            },
        ) => {
            let unique_member_ids = initial_member_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            identity.versioned_operation_kind == CREATE_ROOM_OPERATION_KIND
                && complete_head.room_seq().get() == 0
                && complete_head.room_id() == room_id
                && complete_head.pack_digest() == request.pack_digest()
                && initial_member_ids.len() == request.ordered_initial_memberships().len()
                && unique_member_ids.len() == initial_member_ids.len()
        }
        (
            OperationIdentityV1::ParticipantAction(identity),
            Some(basis),
            ReceiptSemanticInputV1::ParticipantAction {
                request,
                admitted_at,
                normalized_action: Some(action),
                action_offer_witness,
                admission_context,
            },
            SemanticResultV1::TransitionCommitted {
                room_id,
                room_seq,
                previous_lineage_hash,
                complete_head,
                ..
            },
        ) => {
            let request_identity = request.operation_identity();
            identity.room_id == *room_id
                && matches!(
                    request_identity,
                    OperationIdentityV1::ParticipantAction(request_identity)
                        if request_identity.as_ref() == identity.as_ref()
                )
                && basis == &action.exact_basis_head
                && request.matches_normalized(action)
                && &action.admitted_at == admitted_at
                && valid_action_admission_evidence(
                    "accepted",
                    request,
                    admitted_at,
                    Some(action),
                    action_offer_witness,
                    admission_context,
                    basis,
                )
                && valid_transition_result(
                    basis,
                    room_id,
                    *room_seq,
                    previous_lineage_hash,
                    complete_head,
                )
        }
        (
            OperationIdentityV1::ParticipantAction(identity),
            Some(basis),
            ReceiptSemanticInputV1::ParticipantAction {
                request,
                admitted_at,
                normalized_action,
                action_offer_witness,
                admission_context,
            },
            SemanticResultV1::RejectionRecorded { code, .. },
        ) => {
            let request_identity = request.operation_identity();
            basis.room_id() == &identity.room_id
                && matches!(
                    request_identity,
                    OperationIdentityV1::ParticipantAction(request_identity)
                        if request_identity.as_ref() == identity.as_ref()
                )
                && valid_action_admission_evidence(
                    code,
                    request,
                    admitted_at,
                    normalized_action.as_deref(),
                    action_offer_witness,
                    admission_context,
                    basis,
                )
        }
        (
            OperationIdentityV1::Administration(identity),
            Some(basis),
            ReceiptSemanticInputV1::CoreAdministration { proposal },
            SemanticResultV1::RejectionRecorded { .. } | SemanticResultV1::NoChangeRecorded { .. },
        ) => {
            proposal.operation_identity() == identity.as_ref()
                && proposal.expected_room_seq() == basis.room_seq()
        }
        (
            OperationIdentityV1::Administration(identity),
            Some(basis),
            ReceiptSemanticInputV1::CoreAdministration { proposal },
            SemanticResultV1::TransitionCommitted {
                room_id,
                room_seq,
                previous_lineage_hash,
                complete_head,
                ..
            },
        ) => {
            proposal.operation_identity() == identity.as_ref()
                && proposal.expected_room_seq() == basis.room_seq()
                && valid_transition_result(
                    basis,
                    room_id,
                    *room_seq,
                    previous_lineage_hash,
                    complete_head,
                )
        }
        (
            OperationIdentityV1::TimerFired(identity),
            Some(basis),
            ReceiptSemanticInputV1::TimerFired { request },
            SemanticResultV1::TransitionCommitted {
                room_id: result_room_id,
                room_seq,
                previous_lineage_hash,
                complete_head,
                ..
            },
        ) => {
            identity.room_id == request.room_id
                && identity.timer_id == request.timer_id
                && identity.generation == request.generation
                && &request.room_id == result_room_id
                && valid_transition_result(
                    basis,
                    &request.room_id,
                    *room_seq,
                    previous_lineage_hash,
                    complete_head,
                )
        }
        (
            OperationIdentityV1::ExternalInput(identity),
            Some(basis),
            ReceiptSemanticInputV1::ExternalInput { room_id, input },
            SemanticResultV1::TransitionCommitted {
                room_id: result_room_id,
                room_seq,
                previous_lineage_hash,
                complete_head,
                ..
            },
        ) => {
            identity.room_id == *room_id
                && identity.source_id == input.source_id
                && identity.input_id == input.input_id
                && room_id == result_room_id
                && valid_transition_result(
                    basis,
                    room_id,
                    *room_seq,
                    previous_lineage_hash,
                    complete_head,
                )
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        semantic_receipt_mismatch()
    }
}

#[allow(clippy::too_many_lines)]
fn valid_action_admission_evidence(
    code: &str,
    request: &ParticipantActionRequestV1,
    admitted_at: &ActionAdmittedAt,
    normalized_action: Option<&ParticipantActionV1>,
    witness: &ActionOfferWitnessV1,
    context: &ActionAdmissionContextV1,
    basis: &CompleteHeadV1,
) -> bool {
    if context.membership_before.member_id() != request.member_id() {
        return false;
    }
    let normalized_matches = normalized_action.is_none_or(|action| {
        request.matches_normalized(action)
            && &action.admitted_at == admitted_at
            && &action.exact_basis_head == basis
    });
    if !normalized_matches {
        return false;
    }
    let current_offers = match witness {
        ActionOfferWitnessV1::Current {
            canonical_action_offers,
        } => {
            let Ok(bytes) = canonical_action_offers.to_bytes() else {
                return false;
            };
            let Ok(offers) = CanonicalJsonV1::decode_canonical::<Vec<ActionOfferV1>>(&bytes) else {
                return false;
            };
            Some(offers)
        }
        ActionOfferWitnessV1::Unavailable { .. } => None,
    };
    if current_offers.as_ref().is_some_and(|offers| {
        let mut action_types = std::collections::BTreeSet::new();
        offers.iter().any(|offer| {
            offer.domain != ACTION_OFFER_DOMAIN
                || !action_types.insert(offer.action_type.as_str())
                || offer.eligibility_window.as_ref().is_some_and(|window| {
                    compare_timestamp_text(window.opens_at.as_str(), window.deadline.as_str())
                        != std::cmp::Ordering::Less
                })
        })
    }) {
        return false;
    }
    let offer = current_offers.as_ref().and_then(|offers| {
        offers
            .iter()
            .find(|offer| offer.action_type == request.action_type())
    });
    let enabled_participant = context.room_status == crate::RoomStatusV1::Active
        && context.membership_before.standing() == MembershipStandingV1::Enabled
        && context.membership_before.access_mode() == AccessModeV1::Participant;
    match code {
        "accepted" | "activity_domain_rejection" => {
            enabled_participant
                && normalized_action.is_some_and(|action| {
                    offer.is_some_and(|offer| {
                        offer.payload_schema_digest == action.payload_schema_digest
                            && action_within_offer(admitted_at, offer)
                    })
                })
        }
        "deadline_passed" => {
            enabled_participant
                && normalized_action.is_some_and(|action| {
                    offer.is_some_and(|offer| {
                        offer.payload_schema_digest == action.payload_schema_digest
                    })
                })
                && offer
                    .and_then(|offer| offer.eligibility_window.as_ref())
                    .is_some_and(|window| {
                        compare_timestamp_text(admitted_at.as_str(), window.deadline.as_str())
                            != std::cmp::Ordering::Less
                    })
        }
        "action_not_allowed" => {
            enabled_participant
                && ((offer.is_none() && normalized_action.is_none())
                    || (normalized_action.is_some_and(|action| {
                        offer.is_some_and(|offer| {
                            offer.payload_schema_digest == action.payload_schema_digest
                        })
                    }) && offer
                        .and_then(|offer| offer.eligibility_window.as_ref())
                        .is_some_and(|window| {
                            compare_timestamp_text(admitted_at.as_str(), window.opens_at.as_str())
                                == std::cmp::Ordering::Less
                        })))
        }
        "stale_room_state" => {
            enabled_participant
                && request.based_on_room_seq() != basis.room_seq()
                && normalized_action.is_none()
                && current_offers.is_some()
        }
        "membership_not_enabled" => {
            context.room_status == crate::RoomStatusV1::Active
                && context.membership_before.standing() != MembershipStandingV1::Enabled
                && normalized_action.is_none()
                && context.membership_before.access_mode() == AccessModeV1::Participant
                && matches!(
                    witness,
                    ActionOfferWitnessV1::Unavailable {
                        reason: ActionOffersUnavailableReasonV1::MembershipNotEnabled
                    }
                )
        }
        "room_archived" => {
            context.room_status == crate::RoomStatusV1::Archived
                && normalized_action.is_none()
                && context.membership_before.access_mode() == AccessModeV1::Participant
                && matches!(
                    witness,
                    ActionOfferWitnessV1::Unavailable {
                        reason: ActionOffersUnavailableReasonV1::RoomArchived
                    }
                )
        }
        _ => false,
    }
}

fn action_within_offer(admitted_at: &ActionAdmittedAt, offer: &ActionOfferV1) -> bool {
    offer.eligibility_window.as_ref().is_none_or(|window| {
        compare_timestamp_text(admitted_at.as_str(), window.opens_at.as_str())
            != std::cmp::Ordering::Less
            && compare_timestamp_text(admitted_at.as_str(), window.deadline.as_str())
                == std::cmp::Ordering::Less
    })
}

fn valid_transition_result(
    basis: &CompleteHeadV1,
    room_id: &RoomId,
    room_seq: RoomSequenceV1,
    previous_lineage_hash: &Blake3DigestV1,
    complete_head: &CompleteHeadV1,
) -> bool {
    basis.room_id() == room_id
        && complete_head.room_id() == room_id
        && complete_head.room_seq() == room_seq
        && basis.room_seq().checked_successor().ok() == Some(room_seq)
        && complete_head.core_schema_version() == basis.core_schema_version()
        && complete_head.pack_digest() == basis.pack_digest()
        && previous_lineage_hash == basis.genesis_or_transition_hash()
}

fn semantic_receipt_mismatch<T>() -> Result<T, CanonicalJsonError> {
    Err(CanonicalJsonError::TypedDecode(
        "semantic receipt identity/basis/input/time/result mismatch".to_owned(),
    ))
}

/// Immutable initial persistence bundle. It contains no snapshot or frame.
#[derive(Clone, Debug)]
pub struct PreparedCreationPersistenceV1 {
    pub pack_revision_lock: PackRevisionLockV1,
    pub canonical_pack_revision_lock_bytes: Vec<u8>,
    pub genesis: crate::GenesisV1,
    pub canonical_genesis_bytes: Vec<u8>,
    pub complete_head: CompleteHeadV1,
    pub canonical_head_bytes: Vec<u8>,
    pub core_state: CoreRoomStateV1,
    pub canonical_core_state_bytes: Vec<u8>,
    pub activity_state: CanonicalJsonV1,
    pub canonical_activity_state_bytes: Vec<u8>,
    pub memberships: Vec<PreparedMembershipMaterializationV1>,
    pub initial_timers: Vec<PreparedTimerMaterializationV1>,
    pub integrity_generation: IntegrityGenerationV1,
}

/// Opaque fully prepared creation branch.
pub struct PreparedRoomCreationV1 {
    identity: OperationIdentityV1,
    request_hash: CanonicalRequestHashV1,
    authority_witness: PreparedAuthorityWitnessV1,
    persistence: PreparedCreationPersistenceV1,
    semantic_result: StoredSemanticResultV1,
    pending_trace: Option<CoreTraceV1>,
}

impl PreparedRoomCreationV1 {
    /// Seals a checked sequence-zero trace for one atomic Create transaction.
    ///
    /// # Errors
    ///
    /// Returns an error unless the trace is Genesis-only and the administration
    /// identity belongs to the authority witness.
    pub(crate) fn from_trace(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestV1,
        authority_witness: PreparedAuthorityWitnessV1,
        trace: CoreTraceV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if identity.authenticated_principal != *authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        if identity.versioned_operation_kind != CREATE_ROOM_OPERATION_KIND {
            return Err(PrepareRoomWriteErrorV1::InvalidCreationOperationKind);
        }
        if trace.head().room_seq().get() != 0 || !trace.transitions().is_empty() {
            return Err(PrepareRoomWriteErrorV1::CreationTraceAdvanced);
        }
        let retained_pack = trace
            .retained_pack()
            .ok_or(PrepareRoomWriteErrorV1::CreationTraceNotRegistryBound)?;
        if request.pack_digest != *trace.head().pack_digest()
            || request.configuration != *trace.genesis().configuration()
            || request.ordered_initial_memberships.len() != trace.core_state().memberships().len()
        {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        let mut memberships_by_principal = BTreeMap::new();
        for membership in trace.core_state().memberships().values() {
            if memberships_by_principal
                .insert(membership.principal_id().clone(), membership)
                .is_some()
            {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            }
        }
        let mut initial_member_ids = Vec::with_capacity(request.ordered_initial_memberships.len());
        for proposal in &request.ordered_initial_memberships {
            let Some(membership) = memberships_by_principal.remove(&proposal.principal_id) else {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            };
            if !proposal.matches_membership(membership) {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            }
            initial_member_ids.push(membership.member_id().clone());
        }
        if !memberships_by_principal.is_empty() {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        let request_hash = request.canonical_request_hash()?;
        let operation_identity = OperationIdentityV1::Administration(Box::new(identity));
        let complete_head = trace.head().clone();
        let mut memberships = Vec::with_capacity(trace.core_state().memberships().len());
        for membership in trace.core_state().memberships().values() {
            memberships.push(PreparedMembershipMaterializationV1 {
                membership: membership.clone(),
                canonical_membership_bytes: encode(membership)?,
            });
        }
        let semantic_result = StoredSemanticResultV1::prepare(
            operation_identity.clone(),
            None,
            ReceiptSemanticInputV1::RoomCreation {
                request: request.clone(),
                created_at: trace.genesis().created_at().clone(),
            },
            SemanticResultV1::GenesisCreated {
                room_id: complete_head.room_id().clone(),
                initial_member_ids,
                complete_head: complete_head.clone(),
            },
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        Ok(Self {
            identity: operation_identity,
            request_hash,
            authority_witness,
            persistence: PreparedCreationPersistenceV1 {
                pack_revision_lock: retained_pack.revision_lock().clone(),
                canonical_pack_revision_lock_bytes: encode(retained_pack.revision_lock())?,
                genesis: trace.genesis().clone(),
                canonical_genesis_bytes: trace.genesis_bytes()?,
                complete_head: complete_head.clone(),
                canonical_head_bytes: encode(&complete_head)?,
                core_state: trace.core_state().clone(),
                canonical_core_state_bytes: encode(trace.core_state())?,
                activity_state: trace.activity_state().clone(),
                canonical_activity_state_bytes: trace.activity_state().to_bytes()?,
                memberships,
                initial_timers: trace
                    .genesis()
                    .initial_timers()
                    .iter()
                    .map(PreparedTimerMaterializationV1::from_scheduled)
                    .collect::<Result<Vec<_>, _>>()?,
                integrity_generation: IntegrityGenerationV1::new(1)
                    .map_err(|_| PrepareRoomWriteErrorV1::InvalidIntegrityGeneration)?,
            },
            semantic_result,
            pending_trace: Some(trace),
        })
    }

    /// Seals a registry-created Genesis without exposing a live speculative
    /// trace. The trace remains owned by this plan until the Create coordinator
    /// observes `GenesisCreated { New }` from durable storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry Genesis or prepared persistence bundle is invalid.
    pub fn from_registry_genesis(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestV1,
        authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let trace = CoreTraceV1::create_uncommitted(prepared_genesis)?;
        Self::from_trace(identity, request, authority_witness, trace)
    }

    #[must_use]
    pub const fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        &self.authority_witness
    }

    #[must_use]
    pub const fn persistence(&self) -> &PreparedCreationPersistenceV1 {
        &self.persistence
    }

    #[must_use]
    pub const fn semantic_result(&self) -> &StoredSemanticResultV1 {
        &self.semantic_result
    }
}

/// Exact Action and Membership facts that affected preparation.
#[derive(Clone, Debug)]
pub struct PreparedActionInputWitnessV1 {
    pub request: ParticipantActionRequestV1,
    pub admitted_at: ActionAdmittedAt,
    pub normalized_action: Option<ParticipantActionV1>,
    pub membership_before: MembershipV1,
    pub canonical_membership_before_bytes: Vec<u8>,
    pub canonical_core_before_bytes: Vec<u8>,
    pub canonical_activity_before_bytes: Vec<u8>,
    pub action_offer_witness: ActionOfferWitnessV1,
}

/// Exact scheduled-generation and materialization facts used by Timer
/// preparation. `SQLite` rechecks and consumes this row atomically.
#[derive(Clone, Debug)]
pub struct PreparedTimerInputWitnessV1 {
    pub request: TimerFiredRequestV1,
    pub canonical_core_before_bytes: Vec<u8>,
    pub canonical_activity_before_bytes: Vec<u8>,
    pub canonical_timer_payload_bytes: Vec<u8>,
}

/// Closed operation-specific witness envelope. Later slices can add opaque
/// host-minted administration, Timer, and external-input witnesses without
/// changing the adapter's transaction interface.
#[derive(Clone, Debug)]
pub enum PreparedOperationInputWitnessV1 {
    ParticipantAction(Box<PreparedActionInputWitnessV1>),
    TimerFired(Box<PreparedTimerInputWitnessV1>),
}

/// Complete immutable Advance persistence bundle.
#[derive(Clone, Debug)]
pub struct PreparedAdvancePersistenceV1 {
    pub transition_id: TransitionId,
    pub transition: TransitionV1,
    pub canonical_transition_bytes: Vec<u8>,
    pub resulting_complete_head: CompleteHeadV1,
    pub canonical_resulting_head_bytes: Vec<u8>,
    pub resulting_core_state: CoreRoomStateV1,
    pub canonical_resulting_core_state_bytes: Vec<u8>,
    pub resulting_activity_state: CanonicalJsonV1,
    pub canonical_resulting_activity_state_bytes: Vec<u8>,
    pub resulting_memberships: Vec<PreparedMembershipMaterializationV1>,
    pub timer_changes: Vec<PreparedTimerMutationV1>,
    pub observation_frames: Vec<PreparedObservationFrameV1>,
    pub activation_decisions: Vec<PreparedActivationDecisionV1>,
}

/// The only two prepared existing-Room intents.
#[derive(Clone, Debug)]
pub enum PreparedExistingIntentV1 {
    Advance(Box<PreparedAdvancePersistenceV1>),
    DurableDisposition,
}

/// Opaque fully prepared existing-Room branch.
pub struct PreparedRoomCommitV1 {
    identity: OperationIdentityV1,
    request_hash: CanonicalRequestHashV1,
    basis_complete_head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    authority_witness: PreparedAuthorityWitnessV1,
    input_witness: PreparedOperationInputWitnessV1,
    intent: PreparedExistingIntentV1,
    semantic_result: StoredSemanticResultV1,
    pending_transition: Option<PreparedRoomTransitionV1>,
}

impl PreparedRoomCommitV1 {
    /// Converts one checked Counter/pack Action preparation into the complete
    /// persistence bundle. Storage performs no reducer, projection, or hash work.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Action preparation, mismatched trace basis,
    /// authority, addressed frame, or sealed transition state.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[allow(clippy::too_many_lines)]
    pub(crate) fn for_action(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::ParticipantAction(action) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotParticipantAction);
        };
        if !request.matches_normalized(action) {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        let membership_before = trace
            .core_state()
            .membership(&action.member_id)
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?
            .clone();
        if membership_before.principal_id() != authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let canonical_action_offers_bytes = prepared
            .action_offer_witness()
            .ok_or(PrepareRoomWriteErrorV1::MissingActionOfferWitness)?
            .to_vec();
        let basis_complete_head = trace.head().clone();
        let (intent, result) = match prepared.disposition() {
            AdvanceDispositionV1::TransitionAccepted { transition, .. } => {
                let resulting_state = prepared
                    .resulting_state()
                    .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
                let resulting_complete_head = transition.complete_head();
                if resulting_state.head() != &resulting_complete_head
                    || transition.previous_lineage_hash()
                        != basis_complete_head.genesis_or_transition_hash()
                {
                    return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
                }
                let observation_frames = prepare_transition_frames(
                    trace,
                    &prepared,
                    resulting_state.core_state(),
                    transition.room_seq(),
                    current_frame_heads,
                )?;
                let mut resulting_memberships =
                    Vec::with_capacity(resulting_state.core_state().memberships().len());
                for membership in resulting_state.core_state().memberships().values() {
                    resulting_memberships.push(PreparedMembershipMaterializationV1 {
                        membership: membership.clone(),
                        canonical_membership_bytes: encode(membership)?,
                    });
                }
                let persistence = PreparedAdvancePersistenceV1 {
                    transition_id: transition_id.clone(),
                    transition: (**transition).clone(),
                    canonical_transition_bytes: transition.canonical_bytes()?,
                    resulting_complete_head: resulting_complete_head.clone(),
                    canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
                    resulting_core_state: resulting_state.core_state().clone(),
                    canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
                    resulting_activity_state: resulting_state.activity_state().clone(),
                    canonical_resulting_activity_state_bytes: resulting_state
                        .activity_state()
                        .to_bytes()?,
                    resulting_memberships,
                    timer_changes: transition
                        .ordered_timer_changes()
                        .iter()
                        .map(PreparedTimerMutationV1::from_change)
                        .collect::<Result<Vec<_>, _>>()?,
                    observation_frames,
                    activation_decisions: Vec::new(),
                };
                (
                    PreparedExistingIntentV1::Advance(Box::new(persistence)),
                    SemanticResultV1::TransitionCommitted {
                        room_id: resulting_complete_head.room_id().clone(),
                        transition_id,
                        room_seq: resulting_complete_head.room_seq(),
                        previous_lineage_hash: transition.previous_lineage_hash().clone(),
                        complete_head: resulting_complete_head,
                    },
                )
            }
            AdvanceDispositionV1::RejectionRecorded { rejection, .. } => (
                PreparedExistingIntentV1::DurableDisposition,
                SemanticResultV1::RejectionRecorded {
                    code: "activity_domain_rejection".to_owned(),
                    safe_details: CanonicalJsonV1::from_serialize(
                        &ActivityDomainRejectionDetailsV1 {
                            declared_code: rejection.declared_code.clone(),
                            safe_details: rejection.bounded_safe_details.clone(),
                        },
                    )?,
                },
            ),
            AdvanceDispositionV1::NoChangeRecorded { .. } => {
                return Err(PrepareRoomWriteErrorV1::ActionNoChange);
            }
        };
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ParticipantAction {
                request: request.clone(),
                admitted_at: action.admitted_at.clone(),
                normalized_action: Some(Box::new(action.clone())),
                action_offer_witness: ActionOfferWitnessV1::Current {
                    canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(
                        &canonical_action_offers_bytes,
                    )?,
                },
                admission_context: ActionAdmissionContextV1 {
                    membership_before: membership_before.clone(),
                    room_status: trace.core_state().room_status(),
                },
            },
            result,
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::ParticipantAction(Box::new(
                PreparedActionInputWitnessV1 {
                    request: request.clone(),
                    admitted_at: action.admitted_at.clone(),
                    normalized_action: Some(action.clone()),
                    canonical_membership_before_bytes: encode(&membership_before)?,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    action_offer_witness: ActionOfferWitnessV1::Current {
                        canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(
                            &canonical_action_offers_bytes,
                        )?,
                    },
                    membership_before,
                },
            )),
            intent,
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Seals one exact, still-scheduled Timer generation Advance. Obsolescence
    /// remains a conditional storage result (`NotApplicable`) and never gains
    /// a Semantic Receipt.
    ///
    /// # Errors
    ///
    /// Returns an error if the prepared Timer transition or any persistence witness is invalid.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn for_timer_fired(
        trace: &CoreTraceV1,
        request: &TimerFiredRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::TimerFired(timer) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotTimerFired);
        };
        if timer != &request.recorded_stimulus() {
            return Err(PrepareRoomWriteErrorV1::TimerRequestMismatch);
        }
        let AdvanceDispositionV1::TransitionAccepted { transition, .. } = prepared.disposition()
        else {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        };
        let resulting_state = prepared
            .resulting_state()
            .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
        let resulting_complete_head = transition.complete_head();
        let basis_complete_head = trace.head().clone();
        if resulting_state.head() != &resulting_complete_head
            || transition.previous_lineage_hash()
                != basis_complete_head.genesis_or_transition_hash()
        {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        let observation_frames = prepare_transition_frames(
            trace,
            &prepared,
            resulting_state.core_state(),
            transition.room_seq(),
            current_frame_heads,
        )?;
        let resulting_memberships = resulting_state
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)?,
                })
            })
            .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
        let persistence = PreparedAdvancePersistenceV1 {
            transition_id: transition_id.clone(),
            transition: (**transition).clone(),
            canonical_transition_bytes: transition.canonical_bytes()?,
            resulting_complete_head: resulting_complete_head.clone(),
            canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
            resulting_core_state: resulting_state.core_state().clone(),
            canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
            resulting_activity_state: resulting_state.activity_state().clone(),
            canonical_resulting_activity_state_bytes: resulting_state
                .activity_state()
                .to_bytes()?,
            resulting_memberships,
            timer_changes: transition
                .ordered_timer_changes()
                .iter()
                .map(PreparedTimerMutationV1::from_change)
                .collect::<Result<Vec<_>, _>>()?,
            observation_frames,
            activation_decisions: Vec::new(),
        };
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::TimerFired {
                request: request.clone(),
            },
            SemanticResultV1::TransitionCommitted {
                room_id: resulting_complete_head.room_id().clone(),
                transition_id,
                room_seq: resulting_complete_head.room_seq(),
                previous_lineage_hash: transition.previous_lineage_hash().clone(),
                complete_head: resulting_complete_head,
            },
        )?;
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::TimerFired(Box::new(
                PreparedTimerInputWitnessV1 {
                    request: request.clone(),
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    canonical_timer_payload_bytes: request.canonical_payload().to_bytes()?,
                },
            )),
            intent: PreparedExistingIntentV1::Advance(Box::new(persistence)),
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Seals a stable host-controlled Action rejection without invoking the
    /// Activity reducer. Malformed payload/schema input and an actually
    /// admissible Action are rejected before a storage plan exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the request is malformed, admissible, or cannot be sealed exactly.
    pub(crate) fn for_stable_action_disposition(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        request.canonical_request_hash()?;
        let membership_before = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?
            .clone();
        if membership_before.principal_id() != authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let assessment = trace
            .assess_stable_action_disposition(request, &admitted_at)?
            .ok_or(PrepareRoomWriteErrorV1::ActionCurrentlyAdmissible)?;
        let action_offer_witness = if let Some(bytes) = assessment.current_action_offers {
            ActionOfferWitnessV1::Current {
                canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(&bytes)?,
            }
        } else {
            let reason = match (assessment.code, assessment.unavailable_reason) {
                ("membership_not_enabled", Some("membership_not_enabled")) => {
                    ActionOffersUnavailableReasonV1::MembershipNotEnabled
                }
                ("room_archived", Some("room_archived")) => {
                    ActionOffersUnavailableReasonV1::RoomArchived
                }
                _ => return Err(PrepareRoomWriteErrorV1::MissingActionOfferWitness),
            };
            ActionOfferWitnessV1::Unavailable { reason }
        };
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let basis_complete_head = trace.head().clone();
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ParticipantAction {
                request: request.clone(),
                admitted_at: admitted_at.clone(),
                normalized_action: assessment.normalized_action.clone().map(Box::new),
                action_offer_witness: action_offer_witness.clone(),
                admission_context: ActionAdmissionContextV1 {
                    membership_before: membership_before.clone(),
                    room_status: trace.core_state().room_status(),
                },
            },
            SemanticResultV1::RejectionRecorded {
                code: assessment.code.to_owned(),
                safe_details: CanonicalJsonV1::parse(br"{}")?,
            },
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::ParticipantAction(Box::new(
                PreparedActionInputWitnessV1 {
                    request: request.clone(),
                    admitted_at,
                    normalized_action: assessment.normalized_action,
                    canonical_membership_before_bytes: encode(&membership_before)?,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    action_offer_witness,
                    membership_before,
                },
            )),
            intent: PreparedExistingIntentV1::DurableDisposition,
            semantic_result,
            pending_transition: None,
        })
    }

    /// Conformance-only entry point for sealing a prepared participant Action.
    /// Production callers receive sealed plans from the Room admission lane.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn for_action_for_conformance(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_action(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a prepared Timer firing.
    /// Production callers receive sealed plans from the Timer lane.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn for_timer_fired_for_conformance(
        trace: &CoreTraceV1,
        request: &TimerFiredRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_timer_fired(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a stable Action disposition.
    /// Production callers receive sealed plans from the Room admission lane.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn for_stable_action_disposition_for_conformance(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_stable_action_disposition(
            trace,
            request,
            admitted_at,
            integrity_generation,
            authority_witness,
        )
    }

    /// Returns the indivisible eight-field existing-Room witness.
    #[must_use]
    pub const fn basis_complete_head(&self) -> &CompleteHeadV1 {
        &self.basis_complete_head
    }

    #[must_use]
    pub const fn integrity_generation(&self) -> IntegrityGenerationV1 {
        self.integrity_generation
    }

    #[must_use]
    pub const fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        &self.authority_witness
    }

    #[must_use]
    pub const fn input_witness(&self) -> &PreparedOperationInputWitnessV1 {
        &self.input_witness
    }

    #[must_use]
    pub const fn intent(&self) -> &PreparedExistingIntentV1 {
        &self.intent
    }

    #[must_use]
    pub const fn semantic_result(&self) -> &StoredSemanticResultV1 {
        &self.semantic_result
    }
}

fn prepare_transition_frames(
    trace: &CoreTraceV1,
    prepared: &PreparedRoomTransitionV1,
    resulting_core: &CoreRoomStateV1,
    cause_room_seq: RoomSequenceV1,
    current_frame_heads: &BTreeMap<MemberId, u64>,
) -> Result<Vec<PreparedObservationFrameV1>, PrepareRoomWriteErrorV1> {
    if current_frame_heads.len() != trace.core_state().memberships().len()
        || !trace
            .core_state()
            .memberships()
            .keys()
            .all(|member_id| current_frame_heads.contains_key(member_id))
    {
        return Err(PrepareRoomWriteErrorV1::FrameHeadWitnessMismatch);
    }
    let mut frames = Vec::new();
    for membership in resulting_core.memberships().values() {
        if membership.standing() != MembershipStandingV1::Enabled {
            continue;
        }
        let viewer = match membership.access_mode() {
            AccessModeV1::Participant => PackViewerV1::Participant(membership.member_id().clone()),
            AccessModeV1::Spectator => PackViewerV1::Public(membership.member_id().clone()),
            AccessModeV1::Operator => PackViewerV1::Operator(membership.member_id().clone()),
        };
        let outcome = trace.observe_prepared(prepared, &viewer)?;
        match outcome {
            ActivityObservationOutcomeV1::Hidden => {}
            ActivityObservationOutcomeV1::Observation(observation) => {
                frames.push(PreparedObservationFrameV1::from_validated_observation(
                    membership.member_id().clone(),
                    current_frame_heads[membership.member_id()],
                    cause_room_seq,
                    &observation,
                )?);
            }
            ActivityObservationOutcomeV1::ProjectionReset(_)
            | ActivityObservationOutcomeV1::VisibilityLost => {
                return Err(PrepareRoomWriteErrorV1::InvalidAddressedFrame);
            }
        }
    }
    Ok(frames)
}

fn action_request_hash(
    request: &ParticipantActionRequestV1,
) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
    #[derive(Serialize)]
    struct ActionRequest<'a> {
        domain: &'static str,
        protocol: &'static str,
        room_id: &'a RoomId,
        member_id: &'a MemberId,
        based_on_room_seq: RoomSequenceV1,
        action_type: &'a str,
        payload: &'a CanonicalJsonV1,
    }

    let bytes = encode(&ActionRequest {
        domain: "worldstream/action-idempotency/v1",
        protocol: "0.1",
        room_id: &request.room_id,
        member_id: &request.member_id,
        based_on_room_seq: request.based_on_room_seq,
        action_type: &request.action_type,
        payload: &request.payload,
    })?;
    Ok(CanonicalRequestHashV1(Blake3DigestV1::hash(&bytes)))
}

fn timer_request_hash(
    request: &TimerFiredRequestV1,
) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
    #[derive(Serialize)]
    struct TimerRequest<'a> {
        domain: &'static str,
        room_id: &'a RoomId,
        timer_id: &'a TimerId,
        generation: TimerGenerationV1,
        scheduled_for: &'a TimerScheduledFor,
        canonical_payload: &'a CanonicalJsonV1,
    }

    let bytes = encode(&TimerRequest {
        domain: "worldstream/timer-fired-request/v1",
        room_id: &request.room_id,
        timer_id: &request.timer_id,
        generation: request.generation,
        scheduled_for: &request.scheduled_for,
        canonical_payload: &request.canonical_payload,
    })?;
    Ok(CanonicalRequestHashV1(Blake3DigestV1::hash(&bytes)))
}

fn external_input_request_hash(
    room_id: &RoomId,
    based_on_room_seq: RoomSequenceV1,
    input: &ExternalInputV1,
) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
    #[derive(Serialize)]
    struct ExternalRequest<'a> {
        domain: &'static str,
        room_id: &'a RoomId,
        based_on_room_seq: RoomSequenceV1,
        source_id: &'a SourceId,
        input_id: &'a InputId,
        input_type: &'a str,
        canonical_payload: &'a CanonicalJsonV1,
        immutable_resource_references: &'a [CanonicalJsonV1],
    }

    let bytes = encode(&ExternalRequest {
        domain: "worldstream/external-input-request/v1",
        room_id,
        based_on_room_seq,
        source_id: &input.source_id,
        input_id: &input.input_id,
        input_type: &input.input_type,
        canonical_payload: &input.canonical_payload,
        immutable_resource_references: &input.immutable_resource_references,
    })?;
    Ok(CanonicalRequestHashV1(Blake3DigestV1::hash(&bytes)))
}

/// Exactly one fully prepared Room write branch.
pub enum PreparedRoomWriteV1 {
    Create(Box<PreparedRoomCreationV1>),
    Existing(Box<PreparedRoomCommitV1>),
}

impl PreparedRoomWriteV1 {
    #[must_use]
    pub const fn identity(&self) -> &OperationIdentityV1 {
        match self {
            Self::Create(prepared) => &prepared.identity,
            Self::Existing(prepared) => &prepared.identity,
        }
    }

    #[must_use]
    pub const fn request_hash(&self) -> &CanonicalRequestHashV1 {
        match self {
            Self::Create(prepared) => &prepared.request_hash,
            Self::Existing(prepared) => &prepared.request_hash,
        }
    }
}

impl From<PreparedRoomCreationV1> for PreparedRoomWriteV1 {
    fn from(value: PreparedRoomCreationV1) -> Self {
        Self::Create(Box::new(value))
    }
}

impl From<PreparedRoomCommitV1> for PreparedRoomWriteV1 {
    fn from(value: PreparedRoomCommitV1) -> Self {
        Self::Existing(Box::new(value))
    }
}

/// Result of the Create coordinator. A speculative live trace is released
/// only for the exact newly committed Genesis result.
pub struct RoomCreationCommitOutcomeV1 {
    resolution: RoomCommitResolutionV1,
    committed_trace: Option<CoreTraceV1>,
    pending_attempt: Option<RoomCreationPendingAttemptV1>,
}

impl RoomCreationCommitOutcomeV1 {
    #[must_use]
    pub const fn resolution(&self) -> &RoomCommitResolutionV1 {
        &self.resolution
    }

    /// Consumes the outcome and returns the live trace only after a new Create
    /// commit. Duplicate creation must reload the durable Room instead.
    #[must_use]
    pub fn into_committed_trace(self) -> Option<CoreTraceV1> {
        self.committed_trace
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RoomCommitResolutionV1,
        Option<CoreTraceV1>,
        Option<RoomCreationPendingAttemptV1>,
    ) {
        (self.resolution, self.committed_trace, self.pending_attempt)
    }
}

/// Opaque recovery capability returned when a Create attempt cannot yet be
/// classified. Its variants deliberately distinguish an exact retry from an
/// identity-guarded resolution: an indeterminate write cannot be resubmitted
/// until guarded resolution has proved the original identity absent.
pub enum RoomCreationPendingAttemptV1 {
    Retryable(RoomCreationRetryV1),
    ResolveOnly(RoomCreationResolveV1),
    Reprepare(RoomCreationReprepareV1),
}

/// Immutable caller semantics and authority path retained after a generated
/// Room-ID collision. A new registry Genesis may change only generated IDs,
/// seed, creation time, and the derived persistence bundle.
pub struct RoomCreationReprepareV1 {
    identity: AdministrationOperationIdentityV1,
    request: RoomCreationRequestV1,
    request_hash: CanonicalRequestHashV1,
    selected_pack_revision_lock: PackRevisionLockV1,
}

impl RoomCreationReprepareV1 {
    #[must_use]
    pub const fn selected_pack_revision_lock(&self) -> &PackRevisionLockV1 {
        &self.selected_pack_revision_lock
    }

    /// Seals a newly generated, registry-bound Genesis while preserving the
    /// exact caller identity/hash and selected Pack semantics.
    ///
    /// # Errors
    ///
    /// Returns an error if authority, caller semantics, or selected revision changed.
    pub fn reseal(
        self,
        current_authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<PreparedRoomCreationV1, PrepareRoomWriteErrorV1> {
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
            self.identity,
            &self.request,
            current_authority_witness,
            prepared_genesis,
        )?;
        if prepared.request_hash != self.request_hash
            || prepared.persistence.pack_revision_lock != self.selected_pack_revision_lock
        {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        Ok(prepared)
    }
}

/// The exact sealed Create plan after guarded absence is known.
pub struct RoomCreationRetryV1 {
    prepared: PreparedRoomCreationV1,
}

impl RoomCreationRetryV1 {
    /// Retries the identical sealed plan without regenerating Room/Member IDs,
    /// seed, semantic time, or canonical receipt bytes.
    #[must_use]
    pub fn retry(self, storage: &dyn RoomCommitStorageV1) -> RoomCreationCommitOutcomeV1 {
        attempt_room_creation(storage, self.prepared)
    }
}

/// The exact original Create identity/hash and sealed plan after an unknown
/// commit outcome. Only guarded resolution is exposed.
pub struct RoomCreationResolveV1 {
    prepared: PreparedRoomCreationV1,
}

impl RoomCreationResolveV1 {
    /// Resolves the original identity/hash. A guarded absence yields the exact
    /// retry capability; an unavailable resolution retains this token.
    #[must_use]
    pub fn resolve(self, storage: &dyn RoomCommitStorageV1) -> RoomCreationCommitOutcomeV1 {
        let prepared = self.prepared;
        let outcome = storage.resolve(&prepared.identity, &prepared.request_hash);
        match outcome {
            ResolveOutcomeV1::StoredResolution(result)
                if duplicate_result_matches_prepared(&result, &prepared.semantic_result) =>
            {
                RoomCreationCommitOutcomeV1 {
                    resolution: RoomCommitResolutionV1::resolved(
                        ResolutionStatusV1::Existing,
                        *result,
                    ),
                    committed_trace: None,
                    pending_attempt: None,
                }
            }
            ResolveOutcomeV1::Conflict {
                existing_request_hash,
            } if existing_request_hash != prepared.request_hash => RoomCreationCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::Conflict {
                    existing_request_hash,
                },
                committed_trace: None,
                pending_attempt: None,
            },
            ResolveOutcomeV1::StoredResolution(_) | ResolveOutcomeV1::Conflict { .. } => {
                RoomCreationCommitOutcomeV1 {
                    resolution: RoomCommitResolutionV1::Fault,
                    committed_trace: None,
                    pending_attempt: None,
                }
            }
            ResolveOutcomeV1::KnownAbsent => RoomCreationCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::RetryableKnownAbsent,
                committed_trace: None,
                pending_attempt: Some(RoomCreationPendingAttemptV1::Retryable(
                    RoomCreationRetryV1 { prepared },
                )),
            },
            ResolveOutcomeV1::ResolutionUnavailable => RoomCreationCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::Indeterminate,
                committed_trace: None,
                pending_attempt: Some(RoomCreationPendingAttemptV1::ResolveOnly(
                    RoomCreationResolveV1 { prepared },
                )),
            },
        }
    }
}

/// Runs the sole Create storage call and withholds speculative Core state
/// until SQLite/PostgreSQL reports the exact durable new Genesis receipt.
pub fn commit_room_creation(
    storage: &dyn RoomCommitStorageV1,
    prepared: PreparedRoomCreationV1,
) -> RoomCreationCommitOutcomeV1 {
    attempt_room_creation(storage, prepared)
}

#[allow(clippy::too_many_lines)]
fn attempt_room_creation(
    storage: &dyn RoomCommitStorageV1,
    prepared: PreparedRoomCreationV1,
) -> RoomCreationCommitOutcomeV1 {
    let expected_identity = prepared.identity.clone();
    let expected_hash = prepared.request_hash.clone();
    let expected_head = prepared.persistence.complete_head.clone();
    let expected_result = prepared.semantic_result.clone();
    let write = PreparedRoomWriteV1::Create(Box::new(prepared));
    let mut resolution = storage.commit(&write);
    if !matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated { .. }
            | RoomCommitResolutionV1::Conflict { .. }
            | RoomCommitResolutionV1::Fenced
            | RoomCommitResolutionV1::Reprepare
            | RoomCommitResolutionV1::RetryableKnownAbsent
            | RoomCommitResolutionV1::Indeterminate
            | RoomCommitResolutionV1::Fault
    ) {
        resolution = RoomCommitResolutionV1::Fault;
    }
    if matches!(
        &resolution,
        RoomCommitResolutionV1::Conflict {
            existing_request_hash
        } if existing_request_hash == &expected_hash
    ) {
        resolution = RoomCommitResolutionV1::Fault;
    }
    let reported_new = matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    );
    let reported_existing = matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::Existing,
            ..
        }
    );
    let release_trace = matches!(
        &resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            result,
        } if result.operation_identity() == &expected_identity
            && result.canonical_request_hash() == &expected_hash
            && result.as_ref() == &expected_result
            && matches!(
                result.result(),
                SemanticResultV1::GenesisCreated { complete_head, .. }
                    if complete_head == &expected_head
            )
    );
    let PreparedRoomWriteV1::Create(mut prepared) = write else {
        unreachable!("Create coordinator owns a Create plan")
    };
    let committed_trace = if release_trace {
        prepared.pending_trace.take()
    } else {
        None
    };
    let exact_existing_result = resolution
        .stored_result()
        .is_some_and(|result| duplicate_result_matches_prepared(result, &expected_result));
    if (reported_new && !release_trace)
        || (reported_existing && !exact_existing_result)
        || (release_trace && committed_trace.is_none())
    {
        resolution = RoomCommitResolutionV1::Fault;
    }
    let pending_attempt = match &resolution {
        RoomCommitResolutionV1::RetryableKnownAbsent => Some(
            RoomCreationPendingAttemptV1::Retryable(RoomCreationRetryV1 {
                prepared: *prepared,
            }),
        ),
        RoomCommitResolutionV1::Indeterminate => Some(RoomCreationPendingAttemptV1::ResolveOnly(
            RoomCreationResolveV1 {
                prepared: *prepared,
            },
        )),
        RoomCommitResolutionV1::Reprepare => {
            let OperationIdentityV1::Administration(identity) = &prepared.identity else {
                unreachable!("validated Create identity is administrative")
            };
            let ReceiptSemanticInputV1::RoomCreation { request, .. } =
                prepared.semantic_result.semantic_input()
            else {
                unreachable!("validated Create receipt carries creation input")
            };
            Some(RoomCreationPendingAttemptV1::Reprepare(
                RoomCreationReprepareV1 {
                    identity: (**identity).clone(),
                    request: request.clone(),
                    request_hash: prepared.request_hash.clone(),
                    selected_pack_revision_lock: prepared.persistence.pack_revision_lock.clone(),
                },
            ))
        }
        _ => None,
    };
    RoomCreationCommitOutcomeV1 {
        resolution,
        committed_trace,
        pending_attempt,
    }
}

/// Actor-memory consequence of an Existing commit. Only `Installed` may be
/// published as a newly advanced live Room state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorInstallationV1 {
    Installed,
    Unchanged,
    ReloadRequired,
    Withheld,
    QuarantineRequired,
}

/// Result of the Existing coordinator, keeping durable resolution distinct
/// from postcommit actor-memory installation.
pub struct ExistingRoomCommitOutcomeV1 {
    resolution: RoomCommitResolutionV1,
    actor_installation: ActorInstallationV1,
    pending_attempt: Option<ExistingRoomPendingAttemptV1>,
    reprepare: Option<ExistingRoomReprepareV1>,
}

impl ExistingRoomCommitOutcomeV1 {
    #[must_use]
    pub const fn resolution(&self) -> &RoomCommitResolutionV1 {
        &self.resolution
    }

    #[must_use]
    pub const fn actor_installation(&self) -> ActorInstallationV1 {
        self.actor_installation
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RoomCommitResolutionV1,
        ActorInstallationV1,
        Option<ExistingRoomPendingAttemptV1>,
        Option<ExistingRoomReprepareV1>,
    ) {
        (
            self.resolution,
            self.actor_installation,
            self.pending_attempt,
            self.reprepare,
        )
    }
}

/// Opaque recovery capability for an Existing-Room write.
pub enum ExistingRoomPendingAttemptV1 {
    Retryable(ExistingRoomRetryV1),
    ResolveOnly(ExistingRoomResolveV1),
}

/// The identical prepared Existing-Room plan after guarded absence is known.
pub struct ExistingRoomRetryV1 {
    prepared: PreparedRoomCommitV1,
}

impl ExistingRoomRetryV1 {
    /// Retries without re-running the Activity pack or resampling admitted-at.
    #[must_use]
    pub fn retry(
        self,
        storage: &dyn RoomCommitStorageV1,
        trace: &mut CoreTraceV1,
    ) -> ExistingRoomCommitOutcomeV1 {
        attempt_existing_room_commit(storage, trace, self.prepared)
    }
}

/// The original identity/hash and sealed Existing plan after an unknown commit
/// result. It intentionally offers resolution, not commit.
pub struct ExistingRoomResolveV1 {
    prepared: PreparedRoomCommitV1,
}

impl ExistingRoomResolveV1 {
    /// Resolves the original identity/hash. Only a guarded absence makes the
    /// exact sealed plan retryable.
    #[must_use]
    pub fn resolve(
        self,
        storage: &dyn RoomCommitStorageV1,
        trace: &mut CoreTraceV1,
    ) -> ExistingRoomCommitOutcomeV1 {
        let prepared = self.prepared;
        let outcome = storage.resolve(&prepared.identity, &prepared.request_hash);
        match outcome {
            ResolveOutcomeV1::StoredResolution(result)
                if duplicate_result_matches_prepared(&result, &prepared.semantic_result) =>
            {
                let actor_installation = match result.result() {
                    SemanticResultV1::TransitionCommitted { .. } => {
                        ActorInstallationV1::ReloadRequired
                    }
                    SemanticResultV1::RejectionRecorded { .. }
                    | SemanticResultV1::NoChangeRecorded { .. } => ActorInstallationV1::Unchanged,
                    SemanticResultV1::GenesisCreated { .. } => ActorInstallationV1::ReloadRequired,
                };
                let _ = trace;
                ExistingRoomCommitOutcomeV1 {
                    resolution: RoomCommitResolutionV1::resolved(
                        ResolutionStatusV1::Existing,
                        *result,
                    ),
                    actor_installation,
                    pending_attempt: None,
                    reprepare: None,
                }
            }
            ResolveOutcomeV1::Conflict {
                existing_request_hash,
            } if existing_request_hash != prepared.request_hash => ExistingRoomCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::Conflict {
                    existing_request_hash,
                },
                actor_installation: ActorInstallationV1::Unchanged,
                pending_attempt: None,
                reprepare: None,
            },
            ResolveOutcomeV1::StoredResolution(_) | ResolveOutcomeV1::Conflict { .. } => {
                ExistingRoomCommitOutcomeV1 {
                    resolution: RoomCommitResolutionV1::Fault,
                    actor_installation: ActorInstallationV1::QuarantineRequired,
                    pending_attempt: None,
                    reprepare: None,
                }
            }
            ResolveOutcomeV1::KnownAbsent => ExistingRoomCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::RetryableKnownAbsent,
                actor_installation: ActorInstallationV1::Unchanged,
                pending_attempt: Some(ExistingRoomPendingAttemptV1::Retryable(
                    ExistingRoomRetryV1 { prepared },
                )),
                reprepare: None,
            },
            ResolveOutcomeV1::ResolutionUnavailable => ExistingRoomCommitOutcomeV1 {
                resolution: RoomCommitResolutionV1::Indeterminate,
                actor_installation: ActorInstallationV1::Withheld,
                pending_attempt: Some(ExistingRoomPendingAttemptV1::ResolveOnly(
                    ExistingRoomResolveV1 { prepared },
                )),
                reprepare: None,
            },
        }
    }
}

/// Immutable Timer candidate retained across a changed Room Head. The exact
/// identity/hash remain fixed while Core prepares against the new Head.
pub struct TimerFiredReprepareV1 {
    request: TimerFiredRequestV1,
    request_hash: CanonicalRequestHashV1,
}

/// Result of re-evaluating one immutable Timer candidate after the durable
/// Room Head changed.
pub enum TimerReprepareOutcomeV1 {
    Prepared(Box<PreparedRoomCommitV1>),
    NotApplicable,
}

/// Exact Action caller request and host admission time retained when the Room
/// Head changes before COMMIT. Core can record the stable stale disposition
/// without re-running the pack or resampling time.
pub struct ParticipantActionReprepareV1 {
    request: ParticipantActionRequestV1,
    admitted_at: ActionAdmittedAt,
    request_hash: CanonicalRequestHashV1,
}

impl ParticipantActionReprepareV1 {
    /// Seals the retained caller request and admission time against the
    /// current durable trace. The token cannot be repurposed for a changed
    /// Action body or a resampled host time.
    ///
    /// # Errors
    ///
    /// Returns an error if the retained request cannot be sealed against the current trace.
    pub fn seal_stable_disposition(
        self,
        trace: &CoreTraceV1,
        integrity_generation: IntegrityGenerationV1,
        current_authority_witness: PreparedAuthorityWitnessV1,
    ) -> Result<PreparedRoomCommitV1, PrepareRoomWriteErrorV1> {
        let prepared = PreparedRoomCommitV1::for_stable_action_disposition(
            trace,
            &self.request,
            self.admitted_at,
            integrity_generation,
            current_authority_witness,
        )?;
        if prepared.request_hash != self.request_hash {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        Ok(prepared)
    }
}

/// Operation-specific immutable input retained for preparation against a new
/// durable Room Head.
pub enum ExistingRoomReprepareV1 {
    ParticipantAction(ParticipantActionReprepareV1),
    TimerFired(TimerFiredReprepareV1),
}

impl TimerFiredReprepareV1 {
    /// Re-evaluates the exact immutable Timer candidate against the current
    /// durable trace and seals the resulting Advance under the original hash.
    ///
    /// # Errors
    ///
    /// Returns an error if the current trace faults or the retained request hash changes.
    pub fn seal_against(
        self,
        trace: &CoreTraceV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        current_authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<TimerReprepareOutcomeV1, PrepareRoomWriteErrorV1> {
        let transition = match trace.prepare(RecordedStimulusV1::TimerFired(
            self.request.recorded_stimulus(),
        )) {
            Ok(transition) => transition,
            Err(TraceErrorV1::TimerWitnessMismatch | TraceErrorV1::ArchivedStimulusForbidden) => {
                return Ok(TimerReprepareOutcomeV1::NotApplicable);
            }
            Err(error) => return Err(error.into()),
        };
        let prepared = PreparedRoomCommitV1::for_timer_fired(
            trace,
            &self.request,
            transition,
            transition_id,
            integrity_generation,
            current_authority_witness,
            current_frame_heads,
        )?;
        if prepared.request_hash != self.request_hash {
            return Err(PrepareRoomWriteErrorV1::TimerRequestMismatch);
        }
        Ok(TimerReprepareOutcomeV1::Prepared(Box::new(prepared)))
    }
}

/// Runs one Existing Room Commit and installs the owned sealed transition only
/// after the exact new durable receipt is verified. Duplicate, disposition,
/// and indeterminate branches never install speculative state.
pub fn commit_existing_room(
    storage: &dyn RoomCommitStorageV1,
    trace: &mut CoreTraceV1,
    prepared: PreparedRoomCommitV1,
) -> ExistingRoomCommitOutcomeV1 {
    attempt_existing_room_commit(storage, trace, prepared)
}

#[allow(clippy::too_many_lines)]
fn attempt_existing_room_commit(
    storage: &dyn RoomCommitStorageV1,
    trace: &mut CoreTraceV1,
    prepared: PreparedRoomCommitV1,
) -> ExistingRoomCommitOutcomeV1 {
    let expected_identity = prepared.identity.clone();
    let expected_hash = prepared.request_hash.clone();
    let expected_basis = prepared.basis_complete_head.clone();
    let expected_result = prepared.semantic_result.clone();
    let expected_transition = match &prepared.intent {
        PreparedExistingIntentV1::Advance(advance) => Some((
            advance.transition_id.clone(),
            advance.resulting_complete_head.clone(),
        )),
        PreparedExistingIntentV1::DurableDisposition => None,
    };
    let write = PreparedRoomWriteV1::Existing(Box::new(prepared));
    let mut resolution = storage.commit(&write);
    let timer_operation = matches!(
        write,
        PreparedRoomWriteV1::Existing(ref prepared)
            if matches!(prepared.input_witness, PreparedOperationInputWitnessV1::TimerFired(_))
    );
    let allowed = matches!(
        resolution,
        RoomCommitResolutionV1::TransitionCommitted { .. }
            | RoomCommitResolutionV1::RejectionRecorded { .. }
            | RoomCommitResolutionV1::NoChangeRecorded { .. }
            | RoomCommitResolutionV1::Conflict { .. }
            | RoomCommitResolutionV1::Fenced
            | RoomCommitResolutionV1::Reprepare
            | RoomCommitResolutionV1::RetryableKnownAbsent
            | RoomCommitResolutionV1::Indeterminate
            | RoomCommitResolutionV1::Fault
    ) || (timer_operation
        && matches!(resolution, RoomCommitResolutionV1::NotApplicable));
    if !allowed {
        resolution = RoomCommitResolutionV1::Fault;
    }
    if matches!(
        &resolution,
        RoomCommitResolutionV1::Conflict {
            existing_request_hash
        } if existing_request_hash == &expected_hash
    ) {
        resolution = RoomCommitResolutionV1::Fault;
    }
    let reported_new = matches!(
        resolution,
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::New,
            ..
        } | RoomCommitResolutionV1::RejectionRecorded {
            status: ResolutionStatusV1::New,
            ..
        } | RoomCommitResolutionV1::NoChangeRecorded {
            status: ResolutionStatusV1::New,
            ..
        }
    );
    let reported_existing = matches!(
        resolution,
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        } | RoomCommitResolutionV1::RejectionRecorded {
            status: ResolutionStatusV1::Existing,
            ..
        } | RoomCommitResolutionV1::NoChangeRecorded {
            status: ResolutionStatusV1::Existing,
            ..
        }
    );
    let exact_new_result = resolution
        .stored_result()
        .is_some_and(|result| result == &expected_result);
    let exact_new_transition = matches!(
        (&resolution, &expected_transition),
        (
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                result,
            },
            Some((expected_transition_id, expected_head)),
        ) if result.operation_identity() == &expected_identity
            && result.canonical_request_hash() == &expected_hash
            && result.basis_complete_head() == Some(&expected_basis)
            && matches!(
                result.result(),
                SemanticResultV1::TransitionCommitted {
                    transition_id,
                    complete_head,
                    ..
                } if transition_id == expected_transition_id && complete_head == expected_head
            )
    );
    let PreparedRoomWriteV1::Existing(mut prepared) = write else {
        unreachable!("Existing coordinator owns an Existing plan")
    };
    let exact_existing_result = resolution
        .stored_result()
        .is_some_and(|result| duplicate_result_matches_prepared(result, &expected_result));
    let actor_installation = if (reported_new && !exact_new_result)
        || (reported_existing && !exact_existing_result)
    {
        resolution = RoomCommitResolutionV1::Fault;
        ActorInstallationV1::QuarantineRequired
    } else if exact_new_transition {
        match prepared
            .pending_transition
            .take()
            .ok_or(TraceErrorV1::InvalidPreparedAdvance)
            .and_then(|pending| trace.install_prepared(pending))
        {
            Ok(AdvanceDispositionV1::TransitionAccepted { .. }) => ActorInstallationV1::Installed,
            Ok(
                AdvanceDispositionV1::RejectionRecorded { .. }
                | AdvanceDispositionV1::NoChangeRecorded { .. },
            )
            | Err(_) => ActorInstallationV1::ReloadRequired,
        }
    } else {
        match &resolution {
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            } => ActorInstallationV1::ReloadRequired,
            RoomCommitResolutionV1::Indeterminate => ActorInstallationV1::Withheld,
            RoomCommitResolutionV1::Fault => ActorInstallationV1::QuarantineRequired,
            _ => ActorInstallationV1::Unchanged,
        }
    };
    let reprepare = if matches!(resolution, RoomCommitResolutionV1::Reprepare) {
        match &prepared.input_witness {
            PreparedOperationInputWitnessV1::TimerFired(witness) => {
                Some(ExistingRoomReprepareV1::TimerFired(TimerFiredReprepareV1 {
                    request: witness.request.clone(),
                    request_hash: prepared.request_hash.clone(),
                }))
            }
            PreparedOperationInputWitnessV1::ParticipantAction(witness) => Some(
                ExistingRoomReprepareV1::ParticipantAction(ParticipantActionReprepareV1 {
                    request: witness.request.clone(),
                    admitted_at: witness.admitted_at.clone(),
                    request_hash: prepared.request_hash.clone(),
                }),
            ),
        }
    } else {
        None
    };
    let pending_attempt = match &resolution {
        RoomCommitResolutionV1::RetryableKnownAbsent => Some(
            ExistingRoomPendingAttemptV1::Retryable(ExistingRoomRetryV1 {
                prepared: *prepared,
            }),
        ),
        RoomCommitResolutionV1::Indeterminate => Some(ExistingRoomPendingAttemptV1::ResolveOnly(
            ExistingRoomResolveV1 {
                prepared: *prepared,
            },
        )),
        _ => None,
    };
    ExistingRoomCommitOutcomeV1 {
        resolution,
        actor_installation,
        pending_attempt,
        reprepare,
    }
}

fn duplicate_result_matches_prepared(
    stored: &StoredSemanticResultV1,
    prepared: &StoredSemanticResultV1,
) -> bool {
    if stored.operation_identity() != prepared.operation_identity()
        || stored.canonical_request_hash() != prepared.canonical_request_hash()
    {
        return false;
    }
    match (stored.semantic_input(), prepared.semantic_input()) {
        (
            ReceiptSemanticInputV1::RoomCreation {
                request: stored, ..
            },
            ReceiptSemanticInputV1::RoomCreation {
                request: prepared, ..
            },
        ) => stored == prepared,
        (
            ReceiptSemanticInputV1::ParticipantAction {
                request: stored, ..
            },
            ReceiptSemanticInputV1::ParticipantAction {
                request: prepared, ..
            },
        ) => stored == prepared,
        (
            ReceiptSemanticInputV1::TimerFired { request: stored },
            ReceiptSemanticInputV1::TimerFired { request: prepared },
        ) => stored == prepared,
        (
            ReceiptSemanticInputV1::CoreAdministration { proposal: stored },
            ReceiptSemanticInputV1::CoreAdministration { proposal: prepared },
        ) => {
            stored.kind() == prepared.kind()
                && stored.operation_identity() == prepared.operation_identity()
                && stored.expected_room_seq() == prepared.expected_room_seq()
                && stored.reason_code() == prepared.reason_code()
                && stored.changeset() == prepared.changeset()
        }
        (
            ReceiptSemanticInputV1::ExternalInput {
                room_id: stored_room,
                input: _,
            },
            ReceiptSemanticInputV1::ExternalInput {
                room_id: prepared_room,
                input: _,
            },
        ) => stored_room == prepared_room,
        _ => false,
    }
}

/// Whether a stored result was created by this call or resolved from its
/// original durable receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionStatusV1 {
    New,
    Existing,
}

/// Backend-neutral Room Commit resolution algebra.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomCommitResolutionV1 {
    GenesisCreated {
        status: ResolutionStatusV1,
        result: Box<StoredSemanticResultV1>,
    },
    TransitionCommitted {
        status: ResolutionStatusV1,
        result: Box<StoredSemanticResultV1>,
    },
    RejectionRecorded {
        status: ResolutionStatusV1,
        result: Box<StoredSemanticResultV1>,
    },
    NoChangeRecorded {
        status: ResolutionStatusV1,
        result: Box<StoredSemanticResultV1>,
    },
    NotApplicable,
    Reprepare,
    Fenced,
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    RetryableKnownAbsent,
    Indeterminate,
    Fault,
}

impl RoomCommitResolutionV1 {
    /// Converts a typed stored result into its branch-specific resolution.
    #[must_use]
    pub fn resolved(status: ResolutionStatusV1, result: StoredSemanticResultV1) -> Self {
        match result.result {
            SemanticResultV1::GenesisCreated { .. } => Self::GenesisCreated {
                status,
                result: Box::new(result),
            },
            SemanticResultV1::TransitionCommitted { .. } => Self::TransitionCommitted {
                status,
                result: Box::new(result),
            },
            SemanticResultV1::RejectionRecorded { .. } => Self::RejectionRecorded {
                status,
                result: Box::new(result),
            },
            SemanticResultV1::NoChangeRecorded { .. } => Self::NoChangeRecorded {
                status,
                result: Box::new(result),
            },
        }
    }

    /// Returns the durable result for a resolved branch.
    #[must_use]
    pub fn stored_result(&self) -> Option<&StoredSemanticResultV1> {
        match self {
            Self::GenesisCreated { result, .. }
            | Self::TransitionCommitted { result, .. }
            | Self::RejectionRecorded { result, .. }
            | Self::NoChangeRecorded { result, .. } => Some(result),
            Self::NotApplicable
            | Self::Reprepare
            | Self::Fenced
            | Self::Conflict { .. }
            | Self::RetryableKnownAbsent
            | Self::Indeterminate
            | Self::Fault => None,
        }
    }

    /// Reports whether the returned transport rendering is a duplicate.
    #[must_use]
    pub const fn duplicate(&self) -> bool {
        matches!(
            self,
            Self::GenesisCreated {
                status: ResolutionStatusV1::Existing,
                ..
            } | Self::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            } | Self::RejectionRecorded {
                status: ResolutionStatusV1::Existing,
                ..
            } | Self::NoChangeRecorded {
                status: ResolutionStatusV1::Existing,
                ..
            }
        )
    }
}

/// Guarded resolution of one original identity/hash pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveOutcomeV1 {
    StoredResolution(Box<StoredSemanticResultV1>),
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    KnownAbsent,
    ResolutionUnavailable,
}

/// One bounded durable history candidate returned by a recovery storage port.
/// Core treats every byte as untrusted until registry replay and projection
/// verification complete, then rechecks the exact Head and integrity fence
/// through the same storage port before yielding an executable trace.
#[derive(Clone, Debug)]
pub struct RoomRecoveryCandidateV1 {
    head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    canonical_head_bytes: Vec<u8>,
    canonical_pack_revision_lock_bytes: Vec<u8>,
    canonical_genesis_bytes: Vec<u8>,
    canonical_transition_bytes: Vec<Vec<u8>>,
    canonical_core_state_bytes: Option<Vec<u8>>,
    canonical_activity_state_bytes: Option<Vec<u8>>,
}

impl RoomRecoveryCandidateV1 {
    /// Creates one storage-owned recovery candidate. Construction does not
    /// validate the bytes; [`recover_room_from_storage`] is the sole trust
    /// boundary that replays and checks them before guarded installation.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        head: CompleteHeadV1,
        integrity_generation: IntegrityGenerationV1,
        canonical_head_bytes: Vec<u8>,
        canonical_pack_revision_lock_bytes: Vec<u8>,
        canonical_genesis_bytes: Vec<u8>,
        canonical_transition_bytes: Vec<Vec<u8>>,
        canonical_core_state_bytes: Option<Vec<u8>>,
        canonical_activity_state_bytes: Option<Vec<u8>>,
    ) -> Self {
        Self {
            head,
            integrity_generation,
            canonical_head_bytes,
            canonical_pack_revision_lock_bytes,
            canonical_genesis_bytes,
            canonical_transition_bytes,
            canonical_core_state_bytes,
            canonical_activity_state_bytes,
        }
    }

    /// Validates immutable lineage without loading the retained executor and
    /// derives every projection that is independent of Activity execution.
    /// Observation Frames remain empty because their semantic reproduction
    /// requires the exact retained executor.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` if lineage, the captured final Head, or any present
    /// current Core/Activity materialization disagrees.
    pub fn preflight_lineage_materializations(
        &self,
    ) -> Result<RecoveredRoomMaterializationsV1, RoomRecoveryErrorV1> {
        let (head, canonical_core_state_bytes, canonical_activity_state_bytes) =
            CoreTraceV1::preflight_recovery_materializations(
                &self.canonical_genesis_bytes,
                &self.canonical_transition_bytes,
            )
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if head != self.head
            || self
                .canonical_core_state_bytes
                .as_ref()
                .is_some_and(|stored| stored != &canonical_core_state_bytes)
            || self
                .canonical_activity_state_bytes
                .as_ref()
                .is_some_and(|stored| stored != &canonical_activity_state_bytes)
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let core_state =
            CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(&canonical_core_state_bytes)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let memberships = core_state
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                })
            })
            .collect::<Result<Vec<_>, RoomRecoveryErrorV1>>()?;
        Ok(RecoveredRoomMaterializationsV1 {
            room_status: core_state.room_status(),
            canonical_core_state_bytes,
            canonical_activity_state_bytes,
            memberships,
            timers: recover_timer_ledger(self)?,
            observation_frames: Vec::new(),
        })
    }
}

/// Durable state of one exact Timer generation reconstructed from immutable
/// Genesis and Transition records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveredTimerStateV1 {
    Scheduled,
    Fired,
    Cancelled,
}

/// One complete Timer generation row expected after lineage replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredTimerMaterializationV1 {
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload_bytes: Vec<u8>,
    state: RecoveredTimerStateV1,
}

impl RecoveredTimerMaterializationV1 {
    #[must_use]
    pub const fn timer_id(&self) -> &TimerId {
        &self.timer_id
    }

    #[must_use]
    pub const fn generation(&self) -> TimerGenerationV1 {
        self.generation
    }

    #[must_use]
    pub const fn scheduled_for(&self) -> &TimerScheduledFor {
        &self.scheduled_for
    }

    #[must_use]
    pub fn canonical_payload_bytes(&self) -> &[u8] {
        &self.canonical_payload_bytes
    }

    #[must_use]
    pub const fn state(&self) -> RecoveredTimerStateV1 {
        self.state
    }
}

/// Replay-derived disposable Room projections supplied to storage only after
/// the immutable lineage has verified.
#[derive(Clone, Debug)]
pub struct RecoveredRoomMaterializationsV1 {
    room_status: RoomStatusV1,
    canonical_core_state_bytes: Vec<u8>,
    canonical_activity_state_bytes: Vec<u8>,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    timers: Vec<RecoveredTimerMaterializationV1>,
    observation_frames: Vec<RecoveredObservationFrameV1>,
}

impl RecoveredRoomMaterializationsV1 {
    /// Builds the current scheduled-only projection used by storage adapter
    /// conformance tests. Production recovery derives the complete ledger
    /// from immutable Genesis and Transitions.
    ///
    /// # Errors
    ///
    /// Returns an error if any canonical materialization cannot be encoded.
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub fn from_trace_for_conformance(trace: &CoreTraceV1) -> Result<Self, CanonicalJsonError> {
        let memberships = trace
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)?,
                })
            })
            .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
        let timers = trace
            .scheduled_timers()
            .values()
            .map(|timer| {
                Ok(RecoveredTimerMaterializationV1 {
                    timer_id: timer.timer_id.clone(),
                    generation: timer.generation,
                    scheduled_for: timer.scheduled_for.clone(),
                    canonical_payload_bytes: timer.canonical_payload.to_bytes()?,
                    state: RecoveredTimerStateV1::Scheduled,
                })
            })
            .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
        Ok(Self {
            room_status: trace.core_state().room_status(),
            canonical_core_state_bytes: encode(trace.core_state())?,
            canonical_activity_state_bytes: trace.activity_state().to_bytes()?,
            memberships,
            timers,
            observation_frames: Vec::new(),
        })
    }

    #[must_use]
    pub const fn room_status(&self) -> RoomStatusV1 {
        self.room_status
    }

    #[must_use]
    pub fn canonical_core_state_bytes(&self) -> &[u8] {
        &self.canonical_core_state_bytes
    }

    #[must_use]
    pub fn canonical_activity_state_bytes(&self) -> &[u8] {
        &self.canonical_activity_state_bytes
    }

    #[must_use]
    pub fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.memberships
    }

    #[must_use]
    pub fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        &self.timers
    }

    #[must_use]
    pub fn observation_frames(&self) -> &[RecoveredObservationFrameV1] {
        &self.observation_frames
    }
}

/// Non-secret addressed frame integrity witness reproduced during replay.
/// The private observation payload stays inside Core; storage compares its
/// persisted bytes by the exact expected BLAKE3 digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredObservationFrameV1 {
    member_id: MemberId,
    frame_seq: u64,
    cause_room_seq: RoomSequenceV1,
    payload_hash: Blake3DigestV1,
}

impl RecoveredObservationFrameV1 {
    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    #[must_use]
    pub const fn frame_seq(&self) -> u64 {
        self.frame_seq
    }

    #[must_use]
    pub const fn cause_room_seq(&self) -> RoomSequenceV1 {
        self.cause_room_seq
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &Blake3DigestV1 {
        &self.payload_hash
    }
}

/// Closed recovery failure classes. No failure yields an executable trace.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RoomRecoveryErrorV1 {
    #[error("durable Room recovery storage is unavailable")]
    StorageUnavailable,
    #[error("the exact retained Room runtime is unavailable")]
    RuntimeUnavailable,
    #[error("the exact retained Room runtime faulted during pure replay")]
    RuntimeFault,
    #[error("durable Room integrity is not healthy")]
    IntegrityUnavailable,
    #[error("durable Room history or materialization is corrupt")]
    Corrupt,
    #[error("durable Room Head or integrity generation changed during recovery")]
    ConcurrentChange,
}

/// Durable integrity disposition for a recovery failure whose exact Room
/// Head and prior healthy generation are still current.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryIntegrityDispositionV1 {
    /// Runtime dependencies are unavailable while durable bytes remain intact.
    Faulted,
    /// Canonical lineage or materialization disagrees with replay.
    Quarantined,
}

/// Separate bounded read/recovery port. The two-operation mutation seam stays
/// unchanged; implementations must serialize the final guard with Room writes.
pub trait RoomRecoveryStorageV1: Send + Sync {
    /// Captures one bounded untrusted recovery candidate.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery failure if the candidate cannot be read safely.
    fn inspect_recovery_candidate(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1>;

    /// Atomically rereads the install fence and verifies or rebuilds disposable
    /// projections from the replay-derived materialization bundle.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery failure unless the exact candidate is still current.
    fn guard_recovery_install(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        recovered_materializations: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1>;

    /// Records a closed recovery failure only if its exact Head and healthy
    /// integrity generation are still current.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery failure if the fence changed or quarantine cannot commit.
    fn record_recovery_failure(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1>;
}

/// Loads, registry-replays, projection-verifies, and finally fences one Room
/// recovery before yielding its executable trace.
///
/// # Errors
///
/// Returns a closed recovery error if storage is unavailable, integrity is not
/// healthy, replay/projection verification fails, or the durable fence changes.
pub fn recover_room_from_storage(
    storage: &dyn RoomRecoveryStorageV1,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<CoreTraceV1>, RoomRecoveryErrorV1> {
    let Some(candidate) = storage.inspect_recovery_candidate(room_id)? else {
        return Ok(None);
    };
    if candidate.head.room_id() != room_id {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let verified = (|| {
        PackRevisionLockV1::from_canonical_bytes(
            &candidate.canonical_pack_revision_lock_bytes,
            candidate.head.pack_digest(),
        )
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if candidate
            .head
            .canonical_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            != candidate.canonical_head_bytes
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        candidate.preflight_lineage_materializations()?;
        let report = CoreTraceV1::replay_for_recovery(
            registry,
            &candidate.canonical_genesis_bytes,
            &candidate.canonical_transition_bytes,
        )
        .map_err(|failure| {
            if failure.class == ReplayFailureClassV1::RuntimeUnavailable
                && failure.last_verified_head.as_deref() == Some(&candidate.head)
            {
                RoomRecoveryErrorV1::RuntimeUnavailable
            } else if failure.class == ReplayFailureClassV1::RuntimeFault {
                RoomRecoveryErrorV1::RuntimeFault
            } else {
                RoomRecoveryErrorV1::Corrupt
            }
        })?;
        let replayed_revision_lock_bytes = report
            .retained_pack_revision_lock()
            .ok_or(RoomRecoveryErrorV1::Corrupt)?
            .canonical_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if report.final_head != candidate.head
            || report
                .final_head
                .canonical_bytes()
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                != candidate.canonical_head_bytes
            || replayed_revision_lock_bytes != candidate.canonical_pack_revision_lock_bytes
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let recovered_materializations = recover_materializations(&candidate, &report)?;
        Ok((report, recovered_materializations))
    })();
    let (report, recovered_materializations) = match verified {
        Ok(value) => value,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(RoomRecoveryErrorV1::RuntimeUnavailable) => {
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Faulted,
            )?;
            return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
        }
        Err(RoomRecoveryErrorV1::RuntimeFault) => {
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Faulted,
            )?;
            return Err(RoomRecoveryErrorV1::RuntimeFault);
        }
        Err(error) => return Err(error),
    };
    storage.guard_recovery_install(
        room_id,
        &candidate.head,
        candidate.integrity_generation,
        &recovered_materializations,
    )?;
    Ok(Some(report.into_trace_after_recovery_fence()))
}

fn recover_materializations(
    candidate: &RoomRecoveryCandidateV1,
    report: &crate::ReplayReportV1,
) -> Result<RecoveredRoomMaterializationsV1, RoomRecoveryErrorV1> {
    let canonical_core_state_bytes = report
        .final_state()
        .core_state()
        .canonical_bytes()
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let canonical_activity_state_bytes = report
        .final_state()
        .activity_state()
        .to_bytes()
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    if candidate
        .canonical_core_state_bytes
        .as_ref()
        .is_some_and(|stored| stored != &canonical_core_state_bytes)
        || candidate
            .canonical_activity_state_bytes
            .as_ref()
            .is_some_and(|stored| stored != &canonical_activity_state_bytes)
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let memberships = report
        .final_state()
        .core_state()
        .memberships()
        .values()
        .map(|membership| {
            Ok(PreparedMembershipMaterializationV1 {
                membership: membership.clone(),
                canonical_membership_bytes: encode(membership)
                    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
            })
        })
        .collect::<Result<Vec<_>, RoomRecoveryErrorV1>>()?;
    Ok(RecoveredRoomMaterializationsV1 {
        room_status: report.final_state().core_state().room_status(),
        canonical_core_state_bytes,
        canonical_activity_state_bytes,
        memberships,
        timers: recover_timer_ledger(candidate)?,
        observation_frames: report
            .observation_frames()
            .iter()
            .map(|frame| RecoveredObservationFrameV1 {
                member_id: frame.member_id().clone(),
                frame_seq: frame.frame_seq(),
                cause_room_seq: frame.cause_room_seq(),
                payload_hash: frame.payload_hash().clone(),
            })
            .collect(),
    })
}

#[allow(clippy::too_many_lines)]
fn recover_timer_ledger(
    candidate: &RoomRecoveryCandidateV1,
) -> Result<Vec<RecoveredTimerMaterializationV1>, RoomRecoveryErrorV1> {
    let genesis = GenesisV1::from_canonical_bytes(&candidate.canonical_genesis_bytes)
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let mut timers =
        BTreeMap::<(TimerId, TimerGenerationV1), RecoveredTimerMaterializationV1>::new();
    let mut scheduled = BTreeMap::<TimerId, TimerGenerationV1>::new();
    let mut last_generation = BTreeMap::<TimerId, TimerGenerationV1>::new();
    if genesis
        .initial_timers()
        .windows(2)
        .any(|pair| pair[0].timer_id >= pair[1].timer_id)
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let generation_one = TimerGenerationV1::new(1).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    for timer in genesis.initial_timers() {
        if timer.generation != generation_one
            || compare_timestamp_text(timer.scheduled_for.as_str(), genesis.created_at().as_str())
                != std::cmp::Ordering::Greater
            || scheduled
                .insert(timer.timer_id.clone(), timer.generation)
                .is_some()
            || last_generation
                .insert(timer.timer_id.clone(), timer.generation)
                .is_some()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let row = RecoveredTimerMaterializationV1 {
            timer_id: timer.timer_id.clone(),
            generation: timer.generation,
            scheduled_for: timer.scheduled_for.clone(),
            canonical_payload_bytes: timer
                .canonical_payload
                .to_bytes()
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
            state: RecoveredTimerStateV1::Scheduled,
        };
        if timers
            .insert((row.timer_id.clone(), row.generation), row)
            .is_some()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    for bytes in &candidate.canonical_transition_bytes {
        let transition =
            TransitionV1::from_canonical_bytes(bytes).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if transition
            .ordered_timer_changes()
            .windows(2)
            .any(|pair| recovered_timer_change_id(&pair[0]) >= recovered_timer_change_id(&pair[1]))
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }

        if transition.resulting_core_state().room_status() == RoomStatusV1::Archived {
            let expected = scheduled
                .iter()
                .map(|(timer_id, generation)| TimerChangeV1::Cancel {
                    timer_id: timer_id.clone(),
                    generation: *generation,
                })
                .collect::<Vec<_>>();
            if transition.ordered_timer_changes() != expected {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            for (timer_id, generation) in &scheduled {
                mark_recovered_timer_cancelled(&mut timers, timer_id, *generation)?;
            }
            scheduled.clear();
            continue;
        }

        if let RecordedStimulusV1::TimerFired(fired) = transition.recorded_stimulus() {
            if scheduled.get(&fired.timer_id) != Some(&fired.generation) {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            let row = timers
                .get_mut(&(fired.timer_id.clone(), fired.generation))
                .ok_or(RoomRecoveryErrorV1::Corrupt)?;
            if row.state != RecoveredTimerStateV1::Scheduled
                || row.scheduled_for != fired.scheduled_for
                || row.canonical_payload_bytes
                    != fired
                        .canonical_payload
                        .to_bytes()
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            row.state = RecoveredTimerStateV1::Fired;
            scheduled.remove(&fired.timer_id);
        }
        for change in transition.ordered_timer_changes() {
            apply_recovered_timer_change(
                &mut timers,
                &mut scheduled,
                &mut last_generation,
                transition.recorded_stimulus(),
                change,
            )?;
        }
    }
    Ok(timers.into_values().collect())
}

fn apply_recovered_timer_change(
    timers: &mut BTreeMap<(TimerId, TimerGenerationV1), RecoveredTimerMaterializationV1>,
    scheduled: &mut BTreeMap<TimerId, TimerGenerationV1>,
    last_generation: &mut BTreeMap<TimerId, TimerGenerationV1>,
    stimulus: &RecordedStimulusV1,
    change: &TimerChangeV1,
) -> Result<(), RoomRecoveryErrorV1> {
    match change {
        TimerChangeV1::Schedule {
            timer_id,
            generation,
            scheduled_for,
            canonical_payload,
        } => {
            let expected = recovered_timer_successor(last_generation, timer_id)?;
            if *generation != expected
                || scheduled.contains_key(timer_id)
                || compare_timestamp_text(scheduled_for.as_str(), stimulus.semantic_time())
                    != std::cmp::Ordering::Greater
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            insert_recovered_timer(
                timers,
                timer_id,
                *generation,
                scheduled_for,
                canonical_payload,
            )?;
            scheduled.insert(timer_id.clone(), *generation);
            last_generation.insert(timer_id.clone(), *generation);
            Ok(())
        }
        TimerChangeV1::Cancel {
            timer_id,
            generation,
        } => {
            if scheduled.get(timer_id) != Some(generation) {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            mark_recovered_timer_cancelled(timers, timer_id, *generation)?;
            scheduled.remove(timer_id);
            Ok(())
        }
        TimerChangeV1::Reschedule {
            timer_id,
            previous_generation,
            generation,
            scheduled_for,
            canonical_payload,
        } => {
            let expected = recovered_timer_successor(last_generation, timer_id)?;
            if scheduled.get(timer_id) != Some(previous_generation)
                || *generation != expected
                || compare_timestamp_text(scheduled_for.as_str(), stimulus.semantic_time())
                    != std::cmp::Ordering::Greater
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            mark_recovered_timer_cancelled(timers, timer_id, *previous_generation)?;
            insert_recovered_timer(
                timers,
                timer_id,
                *generation,
                scheduled_for,
                canonical_payload,
            )?;
            scheduled.insert(timer_id.clone(), *generation);
            last_generation.insert(timer_id.clone(), *generation);
            Ok(())
        }
    }
}

fn recovered_timer_change_id(change: &TimerChangeV1) -> &TimerId {
    match change {
        TimerChangeV1::Schedule { timer_id, .. }
        | TimerChangeV1::Cancel { timer_id, .. }
        | TimerChangeV1::Reschedule { timer_id, .. } => timer_id,
    }
}

fn recovered_timer_successor(
    last_generation: &BTreeMap<TimerId, TimerGenerationV1>,
    timer_id: &TimerId,
) -> Result<TimerGenerationV1, RoomRecoveryErrorV1> {
    match last_generation.get(timer_id) {
        Some(generation) => generation
            .checked_successor()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt),
        None => TimerGenerationV1::new(1).map_err(|_| RoomRecoveryErrorV1::Corrupt),
    }
}

fn insert_recovered_timer(
    timers: &mut BTreeMap<(TimerId, TimerGenerationV1), RecoveredTimerMaterializationV1>,
    timer_id: &TimerId,
    generation: TimerGenerationV1,
    scheduled_for: &TimerScheduledFor,
    canonical_payload: &CanonicalJsonV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let row = RecoveredTimerMaterializationV1 {
        timer_id: timer_id.clone(),
        generation,
        scheduled_for: scheduled_for.clone(),
        canonical_payload_bytes: canonical_payload
            .to_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
        state: RecoveredTimerStateV1::Scheduled,
    };
    if timers.insert((timer_id.clone(), generation), row).is_some() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn mark_recovered_timer_cancelled(
    timers: &mut BTreeMap<(TimerId, TimerGenerationV1), RecoveredTimerMaterializationV1>,
    timer_id: &TimerId,
    generation: TimerGenerationV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let row = timers
        .get_mut(&(timer_id.clone(), generation))
        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    if row.state != RecoveredTimerStateV1::Scheduled {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    row.state = RecoveredTimerStateV1::Cancelled;
    Ok(())
}

/// The complete storage seam: one prepared write and one guarded resolution.
pub trait RoomCommitStorageV1: Send + Sync {
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1;

    fn resolve(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1;
}

/// Failure while sealing a prepared write. No storage transaction has opened.
#[derive(Debug, Error)]
pub enum PrepareRoomWriteErrorV1 {
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Trace(#[from] TraceErrorV1),
    #[error("authority generation must be a nonzero safe integer")]
    InvalidAuthorityGeneration,
    #[error("initial Membership proposal has an invalid participant/Role shape")]
    InvalidInitialMembershipProposal,
    #[error("integrity generation is invalid")]
    InvalidIntegrityGeneration,
    #[error("authority Principal does not match the operation or Membership")]
    AuthorityIdentityMismatch,
    #[error("creation trace has already advanced")]
    CreationTraceAdvanced,
    #[error("creation trace is not bound to an exact retained Pack revision")]
    CreationTraceNotRegistryBound,
    #[error("creation request does not exactly match the initialized trace")]
    CreationRequestMismatch,
    #[error("administration identity does not name the frozen create-Room operation")]
    InvalidCreationOperationKind,
    #[error("prepared Transition basis does not match the supplied trace")]
    PreparedBasisMismatch,
    #[error("prepared operation is not a participant Action")]
    NotParticipantAction,
    #[error("prepared operation is not an exact Timer firing")]
    NotTimerFired,
    #[error("prepared Timer stimulus does not match its immutable request")]
    TimerRequestMismatch,
    #[error("normalized Action does not match its closed caller request")]
    ActionRequestMismatch,
    #[error("Action is currently admissible and cannot become a host disposition")]
    ActionCurrentlyAdmissible,
    #[error("Action Membership is missing")]
    MembershipMissing,
    #[error("prepared Action does not retain its exact admitted Action Offer bytes")]
    MissingActionOfferWitness,
    #[error("prepared Transition is internally inconsistent")]
    InvalidPreparedTransition,
    #[error("Action cannot produce administrative NoChange")]
    ActionNoChange,
    #[error("addressed frame does not match the resulting Room state")]
    InvalidAddressedFrame,
    #[error("frame-head witness does not cover the exact current Membership set")]
    FrameHeadWitnessMismatch,
    #[error("frame sequence is exhausted")]
    FrameSequenceExhausted,
    #[error("Activation decision is malformed")]
    InvalidActivationDecision,
}
