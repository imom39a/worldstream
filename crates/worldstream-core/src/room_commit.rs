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
    ActivationDecisionV1, ActivityObservationOutcomeV1, AdministrationOperationIdentityV1,
    AuthorityCheckedAt, AuthorityErrorV1, AuthoritySnapshotQueryV1, AuthoritySnapshotV1,
    AuthorityStoreErrorV1, AuthorityUseV1, AuthorityV1, AuthorizedCoreAdministrationV1,
    AuthorizedExternalInputV1, AuthorizedParticipantActionV1, AuthorizedReceiptReadV1,
    AuthorizedRoomCreationV1, AuthorizedTimerFiredV1, Blake3DigestV1, CanonicalJsonError,
    CanonicalJsonV1, ClassifiedCoreAdministrationV1, CompleteHeadV1, CoreAdministrationClassV1,
    CoreChangeSetV1, CoreProposedKindV1, CoreProposedV1, CoreRecordedAt, CoreRoomStateV1,
    CoreTraceV1, CreationRecordedAt, ExternalInputRecordedAt, ExternalInputV1, GenesisV1, InputId,
    IntegrityGenerationV1, MemberAuthorityUseV1, MemberId, MembershipChangeKindV1,
    MembershipStandingV1, MembershipV1, PackDigestV1, PackRegistryV1, PackRevisionLockV1,
    PackViewerV1, ParticipantActionAuthorityV1, ParticipantActionV1, PreparedNewRoomGenesisV1,
    PresentedCapabilityV1, PrincipalId, PrincipalKindV1, RecordedStimulusV1, ReplayFailureClassV1,
    RoomId, RoomSequenceV1, RoomStatusV1, SourceId, TimerChangeV1, TimerFiredV1, TimerGenerationV1,
    TimerId, TimerScheduledFor, TraceErrorV1, TransitionId, TransitionV1,
    activity_pack::{ValidatedPackObservationV1, ValidatedPackViewV1},
    authority::{AuthorityFenceFactsV1, ReceiptReadAdapterInputV1, ReceiptReadTargetPolicyV1},
    canonical::encode,
    primitives::{DigestParseError, compare_timestamp_text},
    trace::{AdvanceDispositionV1, PreparedRoomTransitionV1},
};

const SEMANTIC_RECEIPT_DOMAIN: &str = "worldstream/semantic-result/v1";
const OPERATION_RECEIPT_CODEC_ID: &str = "worldstream/operation-receipt/v1";
const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

macro_rules! redacted_debug {
    ($name:ident) => {
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

/// Frozen operation kind for the sole no-basis Room creation operation.
pub const CREATE_ROOM_OPERATION_KIND: &str = "worldstream/create-room/v1";

/// Domain-specific hash of one versioned caller-semantic canonical request.
/// It cannot be substituted with a pack, state, lineage, or artifact digest.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalRequestHashV1(Blake3DigestV1);

impl fmt::Debug for CanonicalRequestHashV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalRequestHashV1([REDACTED])")
    }
}

impl CanonicalRequestHashV1 {
    fn calculate(request: &CanonicalJsonV1) -> Result<Self, CanonicalJsonError> {
        Ok(Self(Blake3DigestV1::hash(&request.to_bytes()?)))
    }

    pub(crate) fn calculate_canonical(request: &[u8]) -> Self {
        Self(Blake3DigestV1::hash(request))
    }

    /// Returns the digest bytes for storage comparison.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    #[must_use]
    pub(crate) const fn digest(&self) -> &Blake3DigestV1 {
        &self.0
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
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCreationRequestV1 {
    pack_digest: PackDigestV1,
    configuration: CanonicalJsonV1,
    ordered_initial_memberships: Vec<InitialMembershipProposalV1>,
}
redacted_debug!(RoomCreationRequestV1);

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

/// Closed caller-semantic existing-Room administration request. Operational
/// attribution and recorded time are deliberately added only after authority
/// grants this exact purpose.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreAdministrationRequestV1 {
    room_id: RoomId,
    operation_identity: AdministrationOperationIdentityV1,
    kind: CoreProposedKindV1,
    expected_room_seq: RoomSequenceV1,
    reason_code: String,
    canonical_changeset: CoreChangeSetV1,
}
redacted_debug!(CoreAdministrationRequestV1);

impl CoreAdministrationRequestV1 {
    /// Validates the operation identity, bounded reason, canonical changeset
    /// shape, and mandatory/vetoable class before authority is consulted.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed identity, reason, changeset shape, or
    /// a multi-Membership changeset that mixes mandatory and vetoable kinds.
    pub fn new(
        room_id: RoomId,
        operation_identity: AdministrationOperationIdentityV1,
        kind: CoreProposedKindV1,
        expected_room_seq: RoomSequenceV1,
        reason_code: impl Into<String>,
        canonical_changeset: CoreChangeSetV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let request = Self {
            room_id,
            operation_identity,
            kind,
            expected_room_seq,
            reason_code: reason_code.into(),
            canonical_changeset,
        };
        request.validate_shape()?;
        Ok(request)
    }

    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn operation_identity(&self) -> &AdministrationOperationIdentityV1 {
        &self.operation_identity
    }

    #[must_use]
    pub const fn kind(&self) -> CoreProposedKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn expected_room_seq(&self) -> RoomSequenceV1 {
        self.expected_room_seq
    }

    #[must_use]
    pub fn reason_code(&self) -> &str {
        &self.reason_code
    }

    #[must_use]
    pub const fn changeset(&self) -> &CoreChangeSetV1 {
        &self.canonical_changeset
    }

    /// Computes the frozen caller-semantic administration request hash before
    /// a host timestamp or authority attribution is added.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical encoding fails.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        self.validate_shape().map_err(|_| {
            CanonicalJsonError::TypedDecode(
                "Core administration request has an invalid closed shape".to_owned(),
            )
        })?;
        core_administration_request_hash(self)
    }

    #[doc(hidden)]
    pub fn classified(&self) -> Result<ClassifiedCoreAdministrationV1, PrepareRoomWriteErrorV1> {
        self.validate_shape()
    }

    fn validate_shape(&self) -> Result<ClassifiedCoreAdministrationV1, PrepareRoomWriteErrorV1> {
        if self.operation_identity.versioned_operation_kind != crate::CORE_OPERATION_KIND
            || self.operation_identity.idempotency_key.is_empty()
            || self.reason_code.is_empty()
            || self.reason_code.len() > 256
            || self
                .canonical_changeset
                .membership_changes()
                .windows(2)
                .any(|pair| pair[0].member_id() >= pair[1].member_id())
        {
            return Err(PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest);
        }

        let changes = self.canonical_changeset.membership_changes();
        let class = match self.kind {
            CoreProposedKindV1::Archive => {
                if self.canonical_changeset.room_status_change().is_none() || !changes.is_empty() {
                    return Err(PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest);
                }
                CoreAdministrationClassV1::Mandatory
            }
            CoreProposedKindV1::MembershipChangeSet => {
                if self.canonical_changeset.room_status_change().is_some() || changes.len() < 2 {
                    return Err(PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest);
                }
                let first = administration_component_class(changes[0].kind());
                if changes
                    .iter()
                    .any(|change| administration_component_class(change.kind()) != first)
                {
                    return Err(PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest);
                }
                first
            }
            kind => {
                if self.canonical_changeset.room_status_change().is_some()
                    || changes.len() != 1
                    || administration_kind_component(kind) != Some(changes[0].kind())
                {
                    return Err(PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest);
                }
                administration_component_class(changes[0].kind())
            }
        };
        let affected_memberships = u16::try_from(changes.len())
            .map_err(|_| PrepareRoomWriteErrorV1::InvalidCoreAdministrationRequest)?;
        Ok(ClassifiedCoreAdministrationV1::new(
            self.kind,
            class,
            affected_memberships,
        ))
    }

    fn normalized_proposal(
        &self,
        attribution: crate::CoreAuthorityAttributionV1,
        recorded_at: CoreRecordedAt,
    ) -> CoreProposedV1 {
        CoreProposedV1::new(
            self.kind,
            attribution,
            self.operation_identity.clone(),
            self.expected_room_seq,
            self.reason_code.clone(),
            recorded_at,
            self.canonical_changeset.clone(),
        )
    }

    pub(crate) fn from_proposal(room_id: RoomId, proposal: &CoreProposedV1) -> Self {
        Self {
            room_id,
            operation_identity: proposal.operation_identity().clone(),
            kind: proposal.kind(),
            expected_room_seq: proposal.expected_room_seq(),
            reason_code: proposal.reason_code().to_owned(),
            canonical_changeset: proposal.changeset().clone(),
        }
    }
}

const fn administration_kind_component(kind: CoreProposedKindV1) -> Option<MembershipChangeKindV1> {
    match kind {
        CoreProposedKindV1::Join => Some(MembershipChangeKindV1::Join),
        CoreProposedKindV1::Resume => Some(MembershipChangeKindV1::Resume),
        CoreProposedKindV1::AccessModeChange => Some(MembershipChangeKindV1::AccessModeChange),
        CoreProposedKindV1::RoleChange => Some(MembershipChangeKindV1::RoleChange),
        CoreProposedKindV1::Suspend => Some(MembershipChangeKindV1::Suspend),
        CoreProposedKindV1::Depart => Some(MembershipChangeKindV1::Depart),
        CoreProposedKindV1::MembershipChangeSet | CoreProposedKindV1::Archive => None,
    }
}

const fn administration_component_class(kind: MembershipChangeKindV1) -> CoreAdministrationClassV1 {
    match kind {
        MembershipChangeKindV1::Join
        | MembershipChangeKindV1::Resume
        | MembershipChangeKindV1::AccessModeChange
        | MembershipChangeKindV1::RoleChange => CoreAdministrationClassV1::Vetoable,
        MembershipChangeKindV1::Suspend | MembershipChangeKindV1::Depart => {
            CoreAdministrationClassV1::Mandatory
        }
    }
}

/// Closed caller request available before lane admission records Semantic
/// Time or resolves a full host Head. The Action ID is its identity component;
/// it is deliberately excluded from the versioned request hash bytes.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantActionRequestV1 {
    room_id: RoomId,
    member_id: MemberId,
    action_id: ActionId,
    based_on_room_seq: RoomSequenceV1,
    action_type: String,
    payload: CanonicalJsonV1,
}
redacted_debug!(ParticipantActionRequestV1);

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

    /// Returns the exact Room sequence supplied as this Action's basis.
    ///
    /// Storage adapters use this value only to choose the Core stale-head
    /// disposition path; it never authorizes rebasing the request.
    #[must_use]
    pub const fn based_on_room_seq(&self) -> RoomSequenceV1 {
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

fn participant_action_authority_use(
    request: &ParticipantActionRequestV1,
) -> Result<AuthorityUseV1, PrepareRoomWriteErrorV1> {
    let request_hash = request.canonical_request_hash()?;
    Ok(AuthorityUseV1::Member {
        room_id: request.room_id().clone(),
        member_id: request.member_id().clone(),
        operation: MemberAuthorityUseV1::SubmitAction {
            identity: ParticipantActionOperationIdentityV1 {
                room_id: request.room_id().clone(),
                member_id: request.member_id().clone(),
                action_id: request.action_id().clone(),
            },
            request_hash,
            action_type: request.action_type().to_owned(),
        },
    })
}

/// Exact Timer generation operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerOperationIdentityV1 {
    pub room_id: RoomId,
    pub timer_id: TimerId,
    pub generation: TimerGenerationV1,
    pub scheduled_for: TimerScheduledFor,
}

/// Immutable scheduled Timer candidate whose identity/hash can be resolved
/// before loading a current Room Head.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFiredRequestV1 {
    room_id: RoomId,
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload: CanonicalJsonV1,
}
redacted_debug!(TimerFiredRequestV1);

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
            scheduled_for: self.scheduled_for.clone(),
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
///
/// Production values are minted only by consuming one purpose-sealed
/// authority grant. The witness keeps exact capability, Principal,
/// Membership/Runner generation, scope, revocation, expiry, and request
/// purpose facts opaque while allowing a storage Adapter to ask Core to
/// revalidate them in its commit transaction.
#[derive(Clone)]
pub struct PreparedAuthorityWitnessV1 {
    kind: PreparedAuthorityWitnessKindV1,
}

#[derive(Clone)]
enum PreparedAuthorityWitnessKindV1 {
    Capability(AuthorityFenceFactsV1),
    #[cfg(any(test, feature = "conformance-tracer"))]
    Conformance(ConformanceAuthorityWitnessV1),
}

#[cfg(any(test, feature = "conformance-tracer"))]
#[derive(Clone, Eq, PartialEq)]
struct ConformanceAuthorityWitnessV1 {
    witness_id: String,
    authenticated_principal: PrincipalId,
    generation: u64,
    canonical_scope_revocation_bytes: Vec<u8>,
    scope_revocation_hash: Blake3DigestV1,
}

impl fmt::Debug for PreparedAuthorityWitnessV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut value = formatter.debug_struct("PreparedAuthorityWitnessV1");
        value.field("authenticated_principal", self.authenticated_principal());
        match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(fence) => {
                value
                    .field("capability_id", fence.capability_id())
                    .field("principal_generation", &fence.principal_generation())
                    .field("authority_generation", &fence.authority_generation())
                    .field("authorized_at", fence.authorized_at())
                    .field("expires_at", &fence.expires_at())
                    .field("scope_revocation", &"[REDACTED]")
                    .field("purpose", &"[REDACTED]");
            }
            #[cfg(any(test, feature = "conformance-tracer"))]
            PreparedAuthorityWitnessKindV1::Conformance(witness) => {
                value
                    .field("conformance_id", &witness.witness_id)
                    .field("generation", &witness.generation)
                    .field("scope_revocation", &"[REDACTED]");
            }
        }
        value.finish()
    }
}

impl PreparedAuthorityWitnessV1 {
    fn from_fence_for_use(
        fence: AuthorityFenceFactsV1,
        expected_use: &AuthorityUseV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !fence.binds_use(expected_use) {
            return Err(PrepareRoomWriteErrorV1::AuthorityPurposeMismatch);
        }
        Ok(Self {
            kind: PreparedAuthorityWitnessKindV1::Capability(fence),
        })
    }

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
            kind: PreparedAuthorityWitnessKindV1::Conformance(ConformanceAuthorityWitnessV1 {
                witness_id,
                authenticated_principal,
                generation,
                canonical_scope_revocation_bytes,
                scope_revocation_hash,
            }),
        })
    }

    /// Mints an opaque witness only for cross-crate storage conformance tests.
    /// Production authority owners must provide the internal issuer seam.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty witness ID or invalid generation.
    #[cfg(any(test, feature = "conformance-tracer"))]
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
    pub const fn authenticated_principal(&self) -> &PrincipalId {
        match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(fence) => fence.authenticated_principal(),
            #[cfg(any(test, feature = "conformance-tracer"))]
            PreparedAuthorityWitnessKindV1::Conformance(witness) => {
                &witness.authenticated_principal
            }
        }
    }

    /// Returns the transaction snapshot query for a production capability
    /// witness. Conformance-only witnesses deliberately have no production
    /// authority query.
    #[must_use]
    pub fn authority_snapshot_query(&self) -> Option<AuthoritySnapshotQueryV1> {
        match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(fence) => Some(fence.snapshot_query()),
            #[cfg(any(test, feature = "conformance-tracer"))]
            PreparedAuthorityWitnessKindV1::Conformance(_) => None,
        }
    }

    /// Revalidates every sealed production authority fact at commit time.
    ///
    /// # Errors
    ///
    /// Returns stale generation for revocation, expiry, Principal disablement,
    /// Membership/Runner generation drift, or any changed scope/profile fact.
    /// Malformed snapshots and backwards trusted time fail closed.
    #[allow(clippy::infallible_destructuring_match)]
    pub fn revalidate_current(
        &self,
        snapshot: &AuthoritySnapshotV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<(), AuthorityStoreErrorV1> {
        let fence = match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(fence) => fence,
            #[cfg(any(test, feature = "conformance-tracer"))]
            PreparedAuthorityWitnessKindV1::Conformance(_) => {
                return Err(AuthorityStoreErrorV1::InvalidChange);
            }
        };
        fence
            .revalidate_current(snapshot, checked_at)
            .map_err(|error| match error {
                AuthorityErrorV1::StaleAuthorityGeneration
                | AuthorityErrorV1::Unauthenticated
                | AuthorityErrorV1::Forbidden
                | AuthorityErrorV1::MembershipNotEnabled => AuthorityStoreErrorV1::StaleGeneration,
                AuthorityErrorV1::InvalidAuthorityRequest | AuthorityErrorV1::Conflict => {
                    AuthorityStoreErrorV1::InvalidChange
                }
                AuthorityErrorV1::Unavailable => AuthorityStoreErrorV1::Corrupt,
            })
    }

    #[cfg(any(test, feature = "conformance-tracer"))]
    #[must_use]
    pub fn conformance_key(&self) -> Option<(&str, &PrincipalId, u64, &Blake3DigestV1)> {
        match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(_) => None,
            PreparedAuthorityWitnessKindV1::Conformance(witness) => Some((
                &witness.witness_id,
                &witness.authenticated_principal,
                witness.generation,
                &witness.scope_revocation_hash,
            )),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        match &self.kind {
            PreparedAuthorityWitnessKindV1::Capability(fence) => fence.authority_generation().get(),
            PreparedAuthorityWitnessKindV1::Conformance(witness) => witness.generation,
        }
    }
}

/// One exact prepared Membership materialization.
#[derive(Clone)]
pub struct PreparedMembershipMaterializationV1 {
    pub membership: MembershipV1,
    pub canonical_membership_bytes: Vec<u8>,
}
redacted_debug!(PreparedMembershipMaterializationV1);

/// One addressed, coalesced Observation Frame.
#[derive(Clone)]
pub struct PreparedObservationFrameV1 {
    member_id: MemberId,
    previous_frame_head: u64,
    frame_seq: u64,
    cause_room_seq: RoomSequenceV1,
    canonical_payload_bytes: Vec<u8>,
    payload_hash: Blake3DigestV1,
}
redacted_debug!(PreparedObservationFrameV1);

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

/// One Membership-addressed delivery consequence of an accepted Transition.
///
/// A hidden Transition has no value in this collection. An observation frame,
/// projection reset, or visibility loss is represented exactly once for the
/// affected Membership. Reset views are already checked and authorized by the
/// Activity Pack host; Core does not reinterpret or broaden their bytes.
#[derive(Clone)]
pub enum PreparedObservationConsequenceV1 {
    /// One durable, zero-or-one coalesced Observation Frame.
    ObservationFrame(PreparedObservationFrameV1),
    /// Incremental delivery is invalid and the recipient needs a full reset.
    ResetRequired(Box<ValidatedPackViewV1>),
    /// The recipient no longer has an enabled Membership view.
    VisibilityLost(MemberId),
}
redacted_debug!(PreparedObservationConsequenceV1);

impl PreparedObservationConsequenceV1 {
    #[must_use]
    pub fn member_id(&self) -> &MemberId {
        match self {
            Self::ObservationFrame(frame) => frame.member_id(),
            Self::ResetRequired(view) => view.viewer().member_id(),
            Self::VisibilityLost(member_id) => member_id,
        }
    }
}

/// One already-decided operational Activation consequence. Counter produces
/// none, but the Advance bundle keeps the common seam complete.
#[derive(Clone)]
pub struct PreparedActivationDecisionV1 {
    decision_id: String,
    target_member_id: Option<MemberId>,
    canonical_decision_bytes: Vec<u8>,
}
redacted_debug!(PreparedActivationDecisionV1);

impl PreparedActivationDecisionV1 {
    /// Builds the default versioned host policy evidence for one validated
    /// Attention.  Policy is operational and intentionally absent from the
    /// Transition hash; `SQLite` installs the resulting intent in the same
    /// transaction as the causing Transition.
    pub(crate) fn from_attention(
        signal: &CanonicalJsonV1,
        cause_room_seq: RoomSequenceV1,
    ) -> Result<Self, CanonicalJsonError> {
        let attention = crate::ActivationAttentionV1::from_canonical(signal)
            .map_err(|error| CanonicalJsonError::TypedDecode(error.to_string()))?;
        let decision = crate::ActivationDecisionV1 {
            decision_id: format!(
                "decision:{}:{}",
                cause_room_seq.get(),
                attention.deduplication_key
            ),
            activation_id: Some(crate::activation_id_for_attention_v1(
                cause_room_seq,
                &attention.target_member_id,
                &attention.deduplication_key,
            )?),
            cause_room_seq,
            attention,
            policy: crate::ActivationPolicyDecisionV1 {
                policy_revision: 1,
                disposition: crate::ActivationPolicyDispositionV1::Intent,
                maximum_lease_ms: 30_000,
            },
        };
        let canonical_decision_bytes = encode(&decision)?;
        Ok(Self {
            decision_id: decision.decision_id,
            target_member_id: Some(decision.attention.target_member_id.clone()),
            canonical_decision_bytes,
        })
    }
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

fn prepare_activation_decisions(
    transition: &TransitionV1,
) -> Result<Vec<PreparedActivationDecisionV1>, CanonicalJsonError> {
    transition
        .ordered_attention_signals()
        .iter()
        .map(|signal| PreparedActivationDecisionV1::from_attention(signal, transition.room_seq()))
        .collect()
}

/// Exact initial/current scheduled Timer row, including prepared payload bytes.
#[derive(Clone)]
pub struct PreparedTimerMaterializationV1 {
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload_bytes: Vec<u8>,
}
redacted_debug!(PreparedTimerMaterializationV1);

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
#[derive(Clone)]
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
redacted_debug!(PreparedTimerMutationV1);

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
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
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
redacted_debug!(ReceiptSemanticInputV1);

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
#[derive(Clone, Eq, PartialEq)]
pub struct StoredSemanticResultV1 {
    operation_identity: OperationIdentityV1,
    canonical_request_hash: CanonicalRequestHashV1,
    basis_complete_head: Option<CompleteHeadV1>,
    semantic_input: ReceiptSemanticInputV1,
    semantic_time: ReceiptSemanticTimeV1,
    result: SemanticResultV1,
    canonical_receipt_bytes: Vec<u8>,
}

impl fmt::Debug for StoredSemanticResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoredSemanticResultV1([REDACTED])")
    }
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

    /// Constructs the exact administration Transition receipt used by
    /// storage conformance fixtures.
    ///
    /// This seam is unavailable in production builds. It preserves the
    /// canonical receipt domain/hash implementation in Core while requiring
    /// the supplied proposal, basis, and Transition to be one exact accepted
    /// lineage step.
    ///
    /// # Errors
    ///
    /// Returns an error if the Transition does not exactly succeed `basis`,
    /// does not record `proposal`, or the canonical receipt cannot be built.
    #[cfg(any(test, feature = "conformance-tracer"))]
    pub fn from_core_transition_for_conformance(
        basis: &CompleteHeadV1,
        proposal: &CoreProposedV1,
        transition_id: TransitionId,
        transition: &TransitionV1,
    ) -> Result<Self, CanonicalJsonError> {
        let expected_sequence = basis.room_seq().checked_successor().map_err(|_| {
            CanonicalJsonError::TypedDecode(
                "conformance Transition sequence cannot advance".to_owned(),
            )
        })?;
        let complete_head = transition.complete_head();
        if transition.room_seq() != expected_sequence
            || transition.previous_lineage_hash() != basis.genesis_or_transition_hash()
            || transition.recorded_stimulus() != &RecordedStimulusV1::CoreProposed(proposal.clone())
            || complete_head.room_id() != basis.room_id()
            || complete_head.room_seq() != expected_sequence
        {
            return semantic_receipt_mismatch();
        }
        Self::prepare(
            OperationIdentityV1::Administration(Box::new(proposal.operation_identity().clone())),
            Some(basis.clone()),
            ReceiptSemanticInputV1::CoreAdministration {
                proposal: proposal.clone(),
            },
            SemanticResultV1::TransitionCommitted {
                room_id: complete_head.room_id().clone(),
                transition_id,
                room_seq: complete_head.room_seq(),
                previous_lineage_hash: transition.previous_lineage_hash().clone(),
                complete_head,
            },
        )
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
                && proposal.authority_attribution().principal_id == identity.authenticated_principal
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
                && proposal.authority_attribution().principal_id == identity.authenticated_principal
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
                && identity.scheduled_for == request.scheduled_for
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
#[derive(Clone)]
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
redacted_debug!(PreparedCreationPersistenceV1);

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
        authority: AuthorizedRoomCreationV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let request_hash = request.canonical_request_hash()?;
        let expected_use = AuthorityUseV1::CreateRoom {
            identity: identity.clone(),
            request_hash,
        };
        if authority.attribution().principal_id != identity.authenticated_principal {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        Self::from_registry_genesis_with_witness(
            identity,
            request,
            authority_witness,
            prepared_genesis,
        )
    }

    fn from_registry_genesis_with_witness(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestV1,
        authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let trace = CoreTraceV1::create_uncommitted(prepared_genesis)?;
        Self::from_trace(identity, request, authority_witness, trace)
    }

    /// Conformance-only raw witness entry point. Production creation accepts
    /// only a purpose-sealed [`AuthorizedRoomCreationV1`].
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn from_registry_genesis_for_conformance(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestV1,
        authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::from_registry_genesis_with_witness(
            identity,
            request,
            authority_witness,
            prepared_genesis,
        )
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
#[derive(Clone)]
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
redacted_debug!(PreparedActionInputWitnessV1);

/// Exact scheduled-generation and materialization facts used by Timer
/// preparation. `SQLite` rechecks and consumes this row atomically.
#[derive(Clone)]
pub struct PreparedTimerInputWitnessV1 {
    pub request: TimerFiredRequestV1,
    pub canonical_core_before_bytes: Vec<u8>,
    pub canonical_activity_before_bytes: Vec<u8>,
    pub canonical_timer_payload_bytes: Vec<u8>,
}
redacted_debug!(PreparedTimerInputWitnessV1);

/// Exact `ExternalInput` facts used during pure preparation.
#[derive(Clone)]
pub struct PreparedExternalInputWitnessV1 {
    pub room_id: RoomId,
    pub based_on_room_seq: RoomSequenceV1,
    pub input: ExternalInputV1,
    pub canonical_core_before_bytes: Vec<u8>,
    pub canonical_activity_before_bytes: Vec<u8>,
}
redacted_debug!(PreparedExternalInputWitnessV1);

/// Exact normalized host-administration facts used during pure preparation.
#[derive(Clone)]
pub struct PreparedCoreAdministrationInputWitnessV1 {
    pub request: CoreAdministrationRequestV1,
    pub proposal: CoreProposedV1,
    pub canonical_core_before_bytes: Vec<u8>,
    pub canonical_activity_before_bytes: Vec<u8>,
}
redacted_debug!(PreparedCoreAdministrationInputWitnessV1);

/// Closed operation-specific witness envelope. Later slices can add opaque
/// host-minted administration, Timer, and external-input witnesses without
/// changing the adapter's transaction interface.
#[derive(Clone)]
pub enum PreparedOperationInputWitnessV1 {
    ParticipantAction(Box<PreparedActionInputWitnessV1>),
    TimerFired(Box<PreparedTimerInputWitnessV1>),
    ExternalInput(Box<PreparedExternalInputWitnessV1>),
    CoreAdministration(Box<PreparedCoreAdministrationInputWitnessV1>),
}
redacted_debug!(PreparedOperationInputWitnessV1);

/// Complete immutable Advance persistence bundle.
#[derive(Clone)]
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
    pub delivery_consequences: Vec<PreparedObservationConsequenceV1>,
    pub activation_decisions: Vec<PreparedActivationDecisionV1>,
}
redacted_debug!(PreparedAdvancePersistenceV1);

/// The only two prepared existing-Room intents.
#[derive(Clone)]
pub enum PreparedExistingIntentV1 {
    Advance(Box<PreparedAdvancePersistenceV1>),
    DurableDisposition,
}
redacted_debug!(PreparedExistingIntentV1);

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
                let delivery_consequences = prepare_transition_consequences(
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
                    delivery_consequences,
                    activation_decisions: prepare_activation_decisions(transition)?,
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
        let delivery_consequences = prepare_transition_consequences(
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
            delivery_consequences,
            activation_decisions: prepare_activation_decisions(transition)?,
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

    /// Seals one exact Timer firing using the `HostOperator` Room-root grant.
    /// The grant binds the Room and request hash; this method then delegates
    /// to the existing exact Timer witness and commit-fence preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another Room/request, or if the
    /// prepared Timer transition or any persistence witness is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_timer_fired(
        trace: &CoreTraceV1,
        request: &TimerFiredRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedTimerFiredV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let request_hash = request.canonical_request_hash()?;
        if authority.room_id() != request.room_id() || authority.request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = AuthorityUseV1::TimerFired {
            room_id: request.room_id().clone(),
            request_hash,
        };
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
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

    /// Seals one exact host-authorized `ExternalInput` Advance.
    ///
    /// # Errors
    ///
    /// Returns an error when the Room/basis/input/grant or prepared transition differs.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn for_authorized_external_input(
        trace: &CoreTraceV1,
        room_id: &RoomId,
        based_on_room_seq: RoomSequenceV1,
        input: &ExternalInputV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedExternalInputV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !prepared.is_new()
            || prepared.basis_complete_head() != trace.head()
            || trace.head().room_id() != room_id
            || trace.head().room_seq() != based_on_room_seq
        {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::ExternalInput(recorded) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotExternalInput);
        };
        if recorded != input {
            return Err(PrepareRoomWriteErrorV1::ExternalInputRequestMismatch);
        }
        let request_hash = external_input_request_hash(room_id, based_on_room_seq, input)?;
        if authority.room_id() != room_id || authority.request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = AuthorityUseV1::ExternalInput {
            room_id: room_id.clone(),
            request_hash: request_hash.clone(),
        };
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
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
        let delivery_consequences = prepare_transition_consequences(
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
            delivery_consequences,
            activation_decisions: prepare_activation_decisions(transition)?,
        };
        let identity =
            OperationIdentityV1::ExternalInput(Box::new(ExternalInputOperationIdentityV1 {
                room_id: room_id.clone(),
                source_id: input.source_id.clone(),
                input_id: input.input_id.clone(),
            }));
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ExternalInput {
                room_id: room_id.clone(),
                input: input.clone(),
            },
            SemanticResultV1::TransitionCommitted {
                room_id: resulting_complete_head.room_id().clone(),
                transition_id,
                room_seq: resulting_complete_head.room_seq(),
                previous_lineage_hash: transition.previous_lineage_hash().clone(),
                complete_head: resulting_complete_head,
            },
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
            input_witness: PreparedOperationInputWitnessV1::ExternalInput(Box::new(
                PreparedExternalInputWitnessV1 {
                    room_id: room_id.clone(),
                    based_on_room_seq,
                    input: input.clone(),
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                },
            )),
            intent: PreparedExistingIntentV1::Advance(Box::new(persistence)),
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Authorizes, normalizes, reduces, and seals one exact existing-Room
    /// Core administration request. Caller-supplied attribution is impossible:
    /// the recorded proposal is derived from the consumed authority grant.
    ///
    /// # Errors
    ///
    /// Returns an error if the request/classification/grant disagree, Core or
    /// pack validation fails, or the prepared consequence cannot be sealed.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_core_administration(
        trace: &CoreTraceV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedCoreAdministrationV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let classified = request.classified()?;
        let request_hash = request.canonical_request_hash()?;
        let expected_use = AuthorityUseV1::CoreAdministration {
            room_id: request.room_id().clone(),
            classified: classified.clone(),
            identity: request.operation_identity().clone(),
            request_hash: request_hash.clone(),
        };
        if authority.classified() != &classified
            || authority.attribution().principal_id
                != request.operation_identity().authenticated_principal
        {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let attribution = authority.attribution().clone();
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        let proposal = request.normalized_proposal(attribution, recorded_at);
        let prepared = trace.prepare(RecordedStimulusV1::CoreProposed(proposal.clone()))?;
        Self::for_core_administration(
            trace,
            request,
            proposal,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn for_core_administration(
        trace: &CoreTraceV1,
        request: &CoreAdministrationRequestV1,
        proposal: CoreProposedV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::CoreProposed(stimulus) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        };
        if stimulus != &proposal
            || request.room_id() != trace.head().room_id()
            || request.expected_room_seq() != trace.head().room_seq()
            || request.operation_identity() != proposal.operation_identity()
            || request.kind() != proposal.kind()
            || request.reason_code() != proposal.reason_code()
            || request.changeset() != proposal.changeset()
        {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        }

        let basis_complete_head = trace.head().clone();
        let identity =
            OperationIdentityV1::Administration(Box::new(request.operation_identity().clone()));
        let request_hash = request.canonical_request_hash()?;
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
                let delivery_consequences = prepare_transition_consequences(
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
                    delivery_consequences,
                    activation_decisions: prepare_activation_decisions(transition)?,
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
            AdvanceDispositionV1::NoChangeRecorded { .. } => (
                PreparedExistingIntentV1::DurableDisposition,
                SemanticResultV1::NoChangeRecorded {
                    code: "administrative_no_change".to_owned(),
                    safe_details: CanonicalJsonV1::parse(br"{}")?,
                },
            ),
        };
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::CoreAdministration {
                proposal: proposal.clone(),
            },
            result,
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::CoreAdministration(Box::new(
                PreparedCoreAdministrationInputWitnessV1 {
                    request: request.clone(),
                    proposal,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                },
            )),
            intent,
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

    /// Seals one checked participant Action using the exact purpose-specific
    /// authority grant that admitted its caller request.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another request or Membership,
    /// or if the prepared transition/persistence bundle is inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_action(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        prepared: PreparedRoomTransitionV1,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedParticipantActionV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let current_membership = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?;
        if authority.membership() != current_membership {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = participant_action_authority_use(request)?;
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
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

    /// Seals a stable disabled-Membership Action disposition using the exact
    /// authority grant produced for that caller request.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another request or Membership,
    /// or the current trace does not yield a stable receiptable disposition.
    pub fn for_authorized_stable_action_disposition(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority: ParticipantActionAuthorityV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let (authorized_membership, fence) = match authority {
            ParticipantActionAuthorityV1::EnabledParticipant(authority) => {
                (authority.membership().clone(), authority.into_fence_facts())
            }
            ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority) => {
                (authority.membership().clone(), authority.into_fence_facts())
            }
        };
        let current_membership = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?;
        if &authorized_membership != current_membership {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = participant_action_authority_use(request)?;
        let authority_witness =
            PreparedAuthorityWitnessV1::from_fence_for_use(fence, &expected_use)?;
        Self::for_stable_action_disposition(
            trace,
            request,
            admitted_at,
            integrity_generation,
            authority_witness,
        )
    }

    /// Conformance-only entry point for sealing a prepared participant Action.
    /// Production callers receive sealed plans from the Room admission lane.
    #[cfg(any(test, feature = "conformance-tracer"))]
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

    /// Conformance-only entry point for sealing a prepared Core
    /// administration request without exercising an external authority
    /// provider. Production callers must consume an authority grant through
    /// [`Self::for_authorized_core_administration`].
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn for_core_administration_for_conformance(
        trace: &CoreTraceV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if authority_witness.authenticated_principal()
            != &request.operation_identity().authenticated_principal
        {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let proposal = request.normalized_proposal(
            crate::CoreAuthorityAttributionV1 {
                principal_id: request.operation_identity().authenticated_principal.clone(),
                authority_kind: crate::CoreAuthorityKindV1::RoomAdministrator,
            },
            recorded_at,
        );
        let prepared = trace.prepare(RecordedStimulusV1::CoreProposed(proposal.clone()))?;
        Self::for_core_administration(
            trace,
            request,
            proposal,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a prepared Timer firing.
    /// Production callers receive sealed plans from the Timer lane.
    #[cfg(any(test, feature = "conformance-tracer"))]
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

pub(crate) fn prepare_transition_consequences(
    trace: &CoreTraceV1,
    prepared: &PreparedRoomTransitionV1,
    resulting_core: &CoreRoomStateV1,
    cause_room_seq: RoomSequenceV1,
    current_frame_heads: &BTreeMap<MemberId, u64>,
) -> Result<Vec<PreparedObservationConsequenceV1>, PrepareRoomWriteErrorV1> {
    if current_frame_heads.len() != trace.core_state().memberships().len()
        || !trace
            .core_state()
            .memberships()
            .keys()
            .all(|member_id| current_frame_heads.contains_key(member_id))
    {
        return Err(PrepareRoomWriteErrorV1::FrameHeadWitnessMismatch);
    }
    let mut consequences = Vec::new();
    let mut member_ids = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    member_ids.extend(resulting_core.memberships().keys().cloned());
    for member_id in member_ids {
        let before = trace.core_state().membership(&member_id);
        let after = resulting_core.membership(&member_id);
        let viewer_membership = match (before, after) {
            (Some(before), _) if before.standing() == MembershipStandingV1::Enabled => before,
            (_, Some(after)) if after.standing() == MembershipStandingV1::Enabled => after,
            _ => continue,
        };
        let viewer = match viewer_membership.access_mode() {
            AccessModeV1::Participant => PackViewerV1::Participant(member_id.clone()),
            AccessModeV1::Spectator => PackViewerV1::Public(member_id.clone()),
            AccessModeV1::Operator => PackViewerV1::Operator(member_id.clone()),
        };
        let outcome = trace.observe_prepared(prepared, &viewer)?;
        match outcome {
            ActivityObservationOutcomeV1::Hidden => {}
            ActivityObservationOutcomeV1::Observation(observation) => {
                let frame = PreparedObservationFrameV1::from_validated_observation(
                    member_id.clone(),
                    current_frame_heads.get(&member_id).copied().unwrap_or(0),
                    cause_room_seq,
                    &observation,
                )?;
                consequences.push(PreparedObservationConsequenceV1::ObservationFrame(frame));
            }
            ActivityObservationOutcomeV1::ProjectionReset(view) => {
                consequences.push(PreparedObservationConsequenceV1::ResetRequired(view));
            }
            ActivityObservationOutcomeV1::VisibilityLost => {
                consequences.push(PreparedObservationConsequenceV1::VisibilityLost(member_id));
            }
        }
    }
    Ok(consequences)
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

fn core_administration_request_hash(
    request: &CoreAdministrationRequestV1,
) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
    #[derive(Serialize)]
    struct AdministrationRequest<'a> {
        domain: &'static str,
        codec_id: &'static str,
        hash_suite: &'static str,
        room_id: &'a RoomId,
        operation_kind: &'static str,
        proposal_kind: CoreProposedKindV1,
        expected_room_seq: RoomSequenceV1,
        reason_code: &'a str,
        canonical_changeset: &'a CoreChangeSetV1,
    }

    let bytes = encode(&AdministrationRequest {
        domain: "worldstream/core-administration-request/v1",
        codec_id: crate::CANONICAL_CODEC_ID,
        hash_suite: crate::HASH_SUITE_ID,
        room_id: request.room_id(),
        operation_kind: crate::CORE_OPERATION_KIND,
        proposal_kind: request.kind(),
        expected_room_seq: request.expected_room_seq(),
        reason_code: request.reason_code(),
        canonical_changeset: request.changeset(),
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

/// Derives the stable request hash for one existing-Room `ExternalInput`.
///
/// # Errors
///
/// Returns an error if the bounded request cannot be canonically encoded.
pub fn external_input_request_hash(
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
        current_authority: AuthorizedRoomCreationV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<PreparedRoomCreationV1, PrepareRoomWriteErrorV1> {
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
            self.identity,
            &self.request,
            current_authority,
            prepared_genesis,
        )?;
        if prepared.request_hash != self.request_hash
            || prepared.persistence.pack_revision_lock != self.selected_pack_revision_lock
        {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        Ok(prepared)
    }

    /// Conformance-only raw witness reseal.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn reseal_for_conformance(
        self,
        current_authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<PreparedRoomCreationV1, PrepareRoomWriteErrorV1> {
        let prepared = PreparedRoomCreationV1::from_registry_genesis_with_witness(
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
        current_authority: ParticipantActionAuthorityV1,
    ) -> Result<PreparedRoomCommitV1, PrepareRoomWriteErrorV1> {
        let prepared = PreparedRoomCommitV1::for_authorized_stable_action_disposition(
            trace,
            &self.request,
            self.admitted_at,
            integrity_generation,
            current_authority,
        )?;
        if prepared.request_hash != self.request_hash {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        Ok(prepared)
    }

    /// Conformance-only raw witness reseal.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn seal_stable_disposition_for_conformance(
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
            PreparedOperationInputWitnessV1::CoreAdministration(_)
            | PreparedOperationInputWitnessV1::ExternalInput(_) => None,
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
#[derive(Clone, Eq, PartialEq)]
pub enum ResolveOutcomeV1 {
    StoredResolution(Box<StoredSemanticResultV1>),
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    KnownAbsent,
    ResolutionUnavailable,
}

impl fmt::Debug for ResolveOutcomeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StoredResolution(_) => "ResolveOutcomeV1::StoredResolution([REDACTED])",
            Self::Conflict { .. } => "ResolveOutcomeV1::Conflict([REDACTED])",
            Self::KnownAbsent => "ResolveOutcomeV1::KnownAbsent",
            Self::ResolutionUnavailable => "ResolveOutcomeV1::ResolutionUnavailable",
        })
    }
}

/// One bounded durable history candidate returned by a recovery storage port.
/// Core treats every byte as untrusted until registry replay and projection
/// verification complete, then rechecks the exact Head and integrity fence
/// through the same storage port before yielding an executable trace.
#[derive(Clone)]
pub struct RoomRecoveryCandidateV1 {
    head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    canonical_head_bytes: Vec<u8>,
    canonical_pack_revision_lock_bytes: Vec<u8>,
    canonical_genesis_bytes: Vec<u8>,
    canonical_transition_bytes: Vec<Vec<u8>>,
    canonical_core_state_bytes: Option<Vec<u8>>,
    canonical_activity_state_bytes: Option<Vec<u8>>,
    checkpoint: Option<RoomRecoveryCheckpointV1>,
}

/// A verified-cache witness for a durable checkpoint.  The checkpoint is
/// disposable acceleration data: its Head and materialized state must match
/// the checkpoint record, while the immutable tail is still replayed through
/// the retained executor before installation.
#[derive(Clone)]
pub struct RoomRecoveryCheckpointV1 {
    checkpoint_head: CompleteHeadV1,
    checkpoint_record_bytes: Vec<u8>,
    core_state_bytes: Vec<u8>,
    activity_state_bytes: Vec<u8>,
    timers: Vec<RecoveredTimerMaterializationV1>,
    observation_frames: Vec<RecoveredObservationFrameV1>,
    observation_consequences: Vec<RecoveredObservationConsequenceV1>,
    membership_generations: BTreeMap<String, i64>,
    observation_frame_heads: BTreeMap<MemberId, u64>,
    activation_decisions: Vec<RecoveredActivationDecisionV1>,
    operational_history_roots: Option<BTreeMap<String, OperationalHistoryRootV2>>,
}

impl RoomRecoveryCheckpointV1 {
    #[must_use]
    pub fn new(
        checkpoint_head: CompleteHeadV1,
        checkpoint_record_bytes: Vec<u8>,
        core_state_bytes: Vec<u8>,
        activity_state_bytes: Vec<u8>,
        timers: Vec<RecoveredTimerMaterializationV1>,
    ) -> Self {
        Self {
            checkpoint_head,
            checkpoint_record_bytes,
            core_state_bytes,
            activity_state_bytes,
            timers,
            observation_frames: Vec::new(),
            observation_consequences: Vec::new(),
            membership_generations: BTreeMap::new(),
            observation_frame_heads: BTreeMap::new(),
            activation_decisions: Vec::new(),
            operational_history_roots: None,
        }
    }

    #[must_use]
    pub fn with_operational_witnesses(
        mut self,
        observation_frames: Vec<RecoveredObservationFrameV1>,
        observation_consequences: Vec<RecoveredObservationConsequenceV1>,
        membership_generations: BTreeMap<String, i64>,
    ) -> Self {
        self.observation_frame_heads = membership_generations
            .keys()
            .filter_map(|member_id| member_id.parse::<MemberId>().ok())
            .map(|member_id| {
                let frame_head = observation_frames
                    .iter()
                    .filter(|frame| frame.member_id() == &member_id)
                    .map(RecoveredObservationFrameV1::frame_seq)
                    .max()
                    .unwrap_or(0);
                (member_id, frame_head)
            })
            .collect();
        self.observation_frames = observation_frames;
        self.observation_consequences = observation_consequences;
        self.membership_generations = membership_generations;
        self
    }

    /// Adds the position and Activation decision witnesses needed to replay a
    /// tail without consulting the skipped canonical prefix.
    #[must_use]
    pub fn with_bounded_operational_witnesses(
        mut self,
        observation_frame_heads: BTreeMap<MemberId, u64>,
        activation_decisions: Vec<RecoveredActivationDecisionV1>,
    ) -> Self {
        self.observation_frame_heads = observation_frame_heads;
        self.activation_decisions = activation_decisions;
        self
    }

    /// Adds the compact, incrementally maintained receipts for guard-only
    /// histories. Their original rows remain retained for forensic replay.
    #[must_use]
    pub fn with_operational_history_roots(
        mut self,
        roots: BTreeMap<String, OperationalHistoryRootV2>,
    ) -> Self {
        self.operational_history_roots = Some(roots);
        self
    }

    pub(crate) fn head(&self) -> &CompleteHeadV1 {
        &self.checkpoint_head
    }
    pub(crate) fn record_bytes(&self) -> &[u8] {
        &self.checkpoint_record_bytes
    }
    pub(crate) fn core_state_bytes(&self) -> &[u8] {
        &self.core_state_bytes
    }
    pub(crate) fn activity_state_bytes(&self) -> &[u8] {
        &self.activity_state_bytes
    }
    pub(crate) fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        &self.timers
    }
    pub(crate) fn observation_frames(&self) -> &[RecoveredObservationFrameV1] {
        &self.observation_frames
    }
    pub(crate) fn observation_consequences(&self) -> &[RecoveredObservationConsequenceV1] {
        &self.observation_consequences
    }
    pub(crate) fn membership_generations(&self) -> &BTreeMap<String, i64> {
        &self.membership_generations
    }
    pub(crate) fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.observation_frame_heads
    }
    pub(crate) fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1] {
        &self.activation_decisions
    }
    pub(crate) fn operational_history_roots(
        &self,
    ) -> Option<&BTreeMap<String, OperationalHistoryRootV2>> {
        self.operational_history_roots.as_ref()
    }
}

impl fmt::Debug for RoomRecoveryCandidateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RoomRecoveryCandidateV1([REDACTED])")
    }
}

impl RoomRecoveryCandidateV1 {
    /// Creates one storage-owned recovery candidate. Construction does not
    /// validate the bytes; the host-internal recovery coordinator is the sole
    /// trust boundary that replays and checks them before guarded installation.
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
            checkpoint: None,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_checkpoint(
        head: CompleteHeadV1,
        integrity_generation: IntegrityGenerationV1,
        canonical_head_bytes: Vec<u8>,
        canonical_pack_revision_lock_bytes: Vec<u8>,
        canonical_genesis_bytes: Vec<u8>,
        canonical_transition_bytes: Vec<Vec<u8>>,
        canonical_core_state_bytes: Option<Vec<u8>>,
        canonical_activity_state_bytes: Option<Vec<u8>>,
        checkpoint: RoomRecoveryCheckpointV1,
    ) -> Self {
        let mut candidate = Self::new(
            head,
            integrity_generation,
            canonical_head_bytes,
            canonical_pack_revision_lock_bytes,
            canonical_genesis_bytes,
            canonical_transition_bytes,
            canonical_core_state_bytes,
            canonical_activity_state_bytes,
        );
        candidate.checkpoint = Some(checkpoint);
        candidate
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
    pub(crate) fn preflight_lineage_materializations(
        &self,
    ) -> Result<RecoveredRoomMaterializationsV1, RoomRecoveryErrorV1> {
        RecoveredRoomMaterializationsV1::preflight_persisted_history_for_storage(
            &self.head,
            &self.canonical_genesis_bytes,
            &self.canonical_transition_bytes,
            self.canonical_core_state_bytes.as_deref(),
            self.canonical_activity_state_bytes.as_deref(),
        )
    }

    pub(crate) fn checkpoint(&self) -> Option<&RoomRecoveryCheckpointV1> {
        self.checkpoint.as_ref()
    }

    /// Whether this candidate carries a verified-cache checkpoint witness.
    #[must_use]
    pub fn has_checkpoint(&self) -> bool {
        self.checkpoint.is_some()
    }

    /// Number of immutable Transition rows retained after the checkpoint.
    #[must_use]
    pub fn tail_transition_count(&self) -> usize {
        self.canonical_transition_bytes.len()
    }
}

/// Durable state of one exact Timer generation reconstructed from immutable
/// Genesis and Transition records.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveredTimerStateV1 {
    Scheduled,
    Fired,
    Cancelled,
}

/// Bounded verification of the current canonical Room record and serving
/// Core materialization. This is a storage-adapter primitive, not a history
/// Replay or caller authorization result.
#[derive(Clone)]
pub struct VerifiedCurrentRoomMaterializationV1 {
    room_status: RoomStatusV1,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    previous_lineage_hash: Option<Blake3DigestV1>,
}
redacted_debug!(VerifiedCurrentRoomMaterializationV1);

impl VerifiedCurrentRoomMaterializationV1 {
    /// Recomputes the state, aggregate, and record hashes for the one
    /// Genesis/Transition named by `expected_head`, then requires exact
    /// equality with the current Core and Activity materialization bytes.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` for a malformed record, invalid Core shape, hash or
    /// Head disagreement, or a current materialization mismatch.
    pub fn verify_for_storage(
        expected_head: &CompleteHeadV1,
        canonical_current_record_bytes: &[u8],
        canonical_core_state_bytes: &[u8],
        canonical_activity_state_bytes: &[u8],
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let (core_state, previous_lineage_hash) =
            CoreTraceV1::preflight_current_storage_materialization(
                expected_head,
                canonical_current_record_bytes,
                canonical_core_state_bytes,
                canonical_activity_state_bytes,
            )
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
        Ok(Self {
            room_status: core_state.room_status(),
            memberships,
            previous_lineage_hash,
        })
    }

    #[must_use]
    pub const fn room_status(&self) -> RoomStatusV1 {
        self.room_status
    }

    #[must_use]
    pub fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.memberships
    }

    /// Returns the immediate predecessor hash for a current Transition, or
    /// `None` when the current record is Genesis.
    #[must_use]
    pub const fn previous_lineage_hash(&self) -> Option<&Blake3DigestV1> {
        self.previous_lineage_hash.as_ref()
    }
}

/// One complete Timer generation row expected after lineage replay.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredTimerMaterializationV1 {
    timer_id: TimerId,
    generation: TimerGenerationV1,
    scheduled_for: TimerScheduledFor,
    canonical_payload_bytes: Vec<u8>,
    state: RecoveredTimerStateV1,
}
redacted_debug!(RecoveredTimerMaterializationV1);

impl RecoveredTimerMaterializationV1 {
    #[must_use]
    pub fn new(
        timer_id: TimerId,
        generation: TimerGenerationV1,
        scheduled_for: TimerScheduledFor,
        canonical_payload_bytes: Vec<u8>,
        state: RecoveredTimerStateV1,
    ) -> Self {
        Self {
            timer_id,
            generation,
            scheduled_for,
            canonical_payload_bytes,
            state,
        }
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

    #[must_use]
    pub const fn state(&self) -> RecoveredTimerStateV1 {
        self.state
    }
}

/// Replay-derived disposable Room projections supplied to storage only after
/// the immutable lineage has verified.
#[derive(Clone)]
pub struct RecoveredRoomMaterializationsV1 {
    room_status: RoomStatusV1,
    canonical_core_state_bytes: Vec<u8>,
    canonical_activity_state_bytes: Vec<u8>,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    timers: Vec<RecoveredTimerMaterializationV1>,
    observation_frames: Vec<RecoveredObservationFrameV1>,
    observation_consequences: Vec<RecoveredObservationConsequenceV1>,
    observation_frame_heads: BTreeMap<MemberId, u64>,
    membership_generations: Option<BTreeMap<String, i64>>,
    activation_decisions: Vec<RecoveredActivationDecisionV1>,
    operational_history_roots: Option<BTreeMap<String, OperationalHistoryRootV2>>,
}
redacted_debug!(RecoveredRoomMaterializationsV1);

impl RecoveredRoomMaterializationsV1 {
    /// Purely validates caller-supplied immutable history and derives the
    /// executor-independent storage projections for the exact expected Head.
    /// This is a storage-adapter verification primitive, not an authorization
    /// or data-release boundary: every returned byte is already present in the
    /// supplied Genesis or Transition records.
    ///
    /// Optional current materializations are checked when present; absence is
    /// left rebuildable for the final guarded recovery transaction.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` for malformed lineage, a Head mismatch, an invalid
    /// host-owned Core/Timer consequence, or a present materialization that
    /// disagrees with immutable history.
    pub fn preflight_persisted_history_for_storage(
        expected_head: &CompleteHeadV1,
        canonical_genesis_bytes: &[u8],
        canonical_transition_bytes: &[Vec<u8>],
        stored_core_state_bytes: Option<&[u8]>,
        stored_activity_state_bytes: Option<&[u8]>,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let (head, canonical_core_state_bytes, canonical_activity_state_bytes) =
            CoreTraceV1::preflight_recovery_materializations(
                canonical_genesis_bytes,
                canonical_transition_bytes,
            )
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if &head != expected_head
            || stored_core_state_bytes
                .is_some_and(|stored| stored != canonical_core_state_bytes.as_slice())
            || stored_activity_state_bytes
                .is_some_and(|stored| stored != canonical_activity_state_bytes.as_slice())
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
        Ok(Self {
            room_status: core_state.room_status(),
            canonical_core_state_bytes,
            canonical_activity_state_bytes,
            memberships,
            timers: recover_timer_ledger(canonical_genesis_bytes, canonical_transition_bytes)?,
            observation_frames: Vec::new(),
            observation_consequences: Vec::new(),
            observation_frame_heads: BTreeMap::new(),
            membership_generations: None,
            activation_decisions: recover_activation_decisions(canonical_transition_bytes)?,
            operational_history_roots: None,
        })
    }

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
            observation_consequences: Vec::new(),
            observation_frame_heads: trace
                .core_state()
                .memberships()
                .keys()
                .cloned()
                .map(|member_id| (member_id, 0))
                .collect(),
            membership_generations: None,
            activation_decisions: Vec::new(),
            operational_history_roots: None,
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

    #[must_use]
    pub fn observation_consequences(&self) -> &[RecoveredObservationConsequenceV1] {
        &self.observation_consequences
    }

    #[must_use]
    pub const fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.observation_frame_heads
    }

    #[must_use]
    pub fn membership_generations(&self) -> Option<&BTreeMap<String, i64>> {
        self.membership_generations.as_ref()
    }

    #[must_use]
    pub fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1] {
        &self.activation_decisions
    }

    /// Compact roots are present only for a V2 checkpoint recovery candidate.
    /// They replace a serving-time scan of retained guard-only histories.
    #[must_use]
    pub const fn operational_history_roots(
        &self,
    ) -> Option<&BTreeMap<String, OperationalHistoryRootV2>> {
        self.operational_history_roots.as_ref()
    }
}

/// Non-secret addressed frame integrity witness reproduced during replay.
/// The private observation payload stays inside Core; storage compares its
/// persisted bytes by the exact expected BLAKE3 digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredObservationFrameV1 {
    member_id: MemberId,
    frame_seq: u64,
    cause_room_seq: RoomSequenceV1,
    payload_hash: Blake3DigestV1,
}

/// Non-secret witness for one non-frame delivery consequence reproduced during
/// retained-Pack recovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveredObservationConsequenceV1 {
    ResetRequired {
        member_id: MemberId,
        cause_room_seq: RoomSequenceV1,
        projection_hash: Blake3DigestV1,
    },
    VisibilityLost {
        member_id: MemberId,
        cause_room_seq: RoomSequenceV1,
    },
}

/// Exact host policy decision materialized for one retained Attention signal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredActivationDecisionV1 {
    cause_room_seq: RoomSequenceV1,
    decision_id: String,
    target_member_id: Option<MemberId>,
    canonical_decision_bytes: Vec<u8>,
}

impl RecoveredActivationDecisionV1 {
    #[must_use]
    pub fn new(
        cause_room_seq: RoomSequenceV1,
        decision_id: String,
        target_member_id: Option<MemberId>,
        canonical_decision_bytes: Vec<u8>,
    ) -> Self {
        Self {
            cause_room_seq,
            decision_id,
            target_member_id,
            canonical_decision_bytes,
        }
    }

    #[must_use]
    pub const fn cause_room_seq(&self) -> RoomSequenceV1 {
        self.cause_room_seq
    }

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

/// Canonical, disposable operational state captured at one exact paired
/// snapshot. The encoded bytes are content hashed by storage and remain a
/// cache witness rather than canonical Room history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCheckpointOperationalWitnessV1 {
    witness_schema: String,
    checkpoint_head: CompleteHeadV1,
    timers: Vec<RecoveredTimerMaterializationV1>,
    observation_frame_heads: BTreeMap<MemberId, u64>,
    observation_frames: Vec<RecoveredObservationFrameV1>,
    observation_consequences: Vec<RecoveredObservationConsequenceV1>,
    membership_generations: BTreeMap<String, i64>,
    activation_decisions: Vec<RecoveredActivationDecisionV1>,
}

/// Frozen canonical schema marker for checkpoint operational witnesses.
pub const CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V1: &str =
    "worldstream/checkpoint-operational-witness/v1";

/// Frozen canonical schema marker for compact V2 checkpoint witnesses.
pub const CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2: &str =
    "worldstream/checkpoint-operational-witness/v2";

const OPERATIONAL_HISTORY_ROOT_DOMAINS_V2: [&str; 3] =
    ["frames", "consequences", "activation_decisions"];
const OPERATIONAL_HISTORY_ROOT_DOMAIN_TAG_V2: &[u8] =
    b"worldstream/operational-history-root/v2\0";

/// One incrementally maintained root for a retained guard-only operational
/// history. It is a cache-verification receipt, never a replacement for the
/// original forensic rows.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalHistoryRootV2 {
    domain: String,
    entry_count: u64,
    root_hash: Blake3DigestV1,
}

impl OperationalHistoryRootV2 {
    /// Constructs one domain-separated root receipt.
    pub fn new(
        domain: impl Into<String>,
        entry_count: u64,
        root_hash: Blake3DigestV1,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let root = Self {
            domain: domain.into(),
            entry_count,
            root_hash,
        };
        if !OPERATIONAL_HISTORY_ROOT_DOMAINS_V2.contains(&root.domain.as_str()) {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Ok(root)
    }

    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    #[must_use]
    pub const fn entry_count(&self) -> u64 {
        self.entry_count
    }

    #[must_use]
    pub const fn root_hash(&self) -> &Blake3DigestV1 {
        &self.root_hash
    }
}

/// Canonical V2 witness with bounded current state and compact receipts for
/// retained guard-only histories. The V1 encoding remains accepted for legacy
/// Rooms; this format is used only by post-admission V2 Rooms.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCheckpointOperationalWitnessV2 {
    witness_schema: String,
    checkpoint_head: CompleteHeadV1,
    timers: Vec<RecoveredTimerMaterializationV1>,
    observation_frame_heads: BTreeMap<MemberId, u64>,
    membership_generations: BTreeMap<String, i64>,
    operational_history_roots: BTreeMap<String, OperationalHistoryRootV2>,
}

impl RoomCheckpointOperationalWitnessV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        checkpoint_head: CompleteHeadV1,
        timers: Vec<RecoveredTimerMaterializationV1>,
        observation_frame_heads: BTreeMap<MemberId, u64>,
        membership_generations: BTreeMap<String, i64>,
        operational_history_roots: BTreeMap<String, OperationalHistoryRootV2>,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let witness = Self {
            witness_schema: CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2.to_owned(),
            checkpoint_head,
            timers,
            observation_frame_heads,
            membership_generations,
            operational_history_roots,
        };
        witness.validate()?;
        Ok(witness)
    }

    pub fn from_canonical_bytes(
        bytes: &[u8],
        expected_head: &CompleteHeadV1,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let witness = CanonicalJsonV1::decode_canonical::<Self>(bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if &witness.checkpoint_head != expected_head {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        witness.validate()?;
        Ok(witness)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RoomRecoveryErrorV1> {
        encode(self).map_err(|_| RoomRecoveryErrorV1::Corrupt)
    }

    #[must_use]
    pub const fn checkpoint_head(&self) -> &CompleteHeadV1 {
        &self.checkpoint_head
    }

    #[must_use]
    pub fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        &self.timers
    }

    #[must_use]
    pub const fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.observation_frame_heads
    }

    #[must_use]
    pub const fn membership_generations(&self) -> &BTreeMap<String, i64> {
        &self.membership_generations
    }

    #[must_use]
    pub const fn operational_history_roots(&self) -> &BTreeMap<String, OperationalHistoryRootV2> {
        &self.operational_history_roots
    }

    fn validate(&self) -> Result<(), RoomRecoveryErrorV1> {
        if self.witness_schema != CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2 {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let members = self
            .observation_frame_heads
            .keys()
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        if members.len() != self.observation_frame_heads.len()
            || members.len() != self.membership_generations.len()
            || self.membership_generations.iter().any(|(member_id, generation)| {
                *generation < 1
                    || !members.contains(member_id)
                    || member_id.parse::<MemberId>().is_err()
            })
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let mut timer_ids = std::collections::BTreeSet::new();
        let mut scheduled = std::collections::BTreeSet::new();
        for timer in &self.timers {
            if !timer_ids.insert(timer.timer_id().clone())
                || CanonicalJsonV1::from_canonical_bytes(timer.canonical_payload_bytes()).is_err()
                || (timer.state() == RecoveredTimerStateV1::Scheduled
                    && !scheduled.insert(timer.timer_id().clone()))
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        if self.operational_history_roots.len() != OPERATIONAL_HISTORY_ROOT_DOMAINS_V2.len()
            || OPERATIONAL_HISTORY_ROOT_DOMAINS_V2.iter().any(|domain| {
                self.operational_history_roots.get(*domain).is_none_or(|root| {
                    root.domain() != *domain
                })
            })
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Ok(())
    }
}

impl RoomCheckpointOperationalWitnessV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        checkpoint_head: CompleteHeadV1,
        timers: Vec<RecoveredTimerMaterializationV1>,
        observation_frame_heads: BTreeMap<MemberId, u64>,
        observation_frames: Vec<RecoveredObservationFrameV1>,
        observation_consequences: Vec<RecoveredObservationConsequenceV1>,
        membership_generations: BTreeMap<String, i64>,
        activation_decisions: Vec<RecoveredActivationDecisionV1>,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let witness = Self {
            witness_schema: CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V1.to_owned(),
            checkpoint_head,
            timers,
            observation_frame_heads,
            observation_frames,
            observation_consequences,
            membership_generations,
            activation_decisions,
        };
        witness.validate()?;
        Ok(witness)
    }

    pub fn from_canonical_bytes(
        bytes: &[u8],
        expected_head: &CompleteHeadV1,
    ) -> Result<Self, RoomRecoveryErrorV1> {
        let witness = CanonicalJsonV1::decode_canonical::<Self>(bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if &witness.checkpoint_head != expected_head {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        witness.validate()?;
        Ok(witness)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RoomRecoveryErrorV1> {
        encode(self).map_err(|_| RoomRecoveryErrorV1::Corrupt)
    }

    #[must_use]
    pub const fn checkpoint_head(&self) -> &CompleteHeadV1 {
        &self.checkpoint_head
    }

    #[must_use]
    pub fn timers(&self) -> &[RecoveredTimerMaterializationV1] {
        &self.timers
    }

    #[must_use]
    pub const fn observation_frame_heads(&self) -> &BTreeMap<MemberId, u64> {
        &self.observation_frame_heads
    }

    #[must_use]
    pub fn observation_frames(&self) -> &[RecoveredObservationFrameV1] {
        &self.observation_frames
    }

    #[must_use]
    pub fn observation_consequences(&self) -> &[RecoveredObservationConsequenceV1] {
        &self.observation_consequences
    }

    #[must_use]
    pub const fn membership_generations(&self) -> &BTreeMap<String, i64> {
        &self.membership_generations
    }

    #[must_use]
    pub fn activation_decisions(&self) -> &[RecoveredActivationDecisionV1] {
        &self.activation_decisions
    }

    fn validate(&self) -> Result<(), RoomRecoveryErrorV1> {
        if self.witness_schema != CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V1 {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let maximum_cause = self.checkpoint_head.room_seq().get();
        let members = self
            .observation_frame_heads
            .keys()
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        if members.len() != self.observation_frame_heads.len()
            || members.len() != self.membership_generations.len()
            || self
                .membership_generations
                .iter()
                .any(|(member_id, generation)| {
                    *generation < 1
                        || !members.contains(member_id)
                        || member_id.parse::<MemberId>().is_err()
                })
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let mut timer_keys = std::collections::BTreeSet::new();
        let mut scheduled = std::collections::BTreeSet::new();
        for timer in &self.timers {
            if !timer_keys.insert((timer.timer_id().clone(), timer.generation()))
                || CanonicalJsonV1::from_canonical_bytes(timer.canonical_payload_bytes()).is_err()
                || (timer.state() == RecoveredTimerStateV1::Scheduled
                    && !scheduled.insert(timer.timer_id().clone()))
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        let mut frame_keys = std::collections::BTreeSet::new();
        for frame in &self.observation_frames {
            if !frame_keys.insert((frame.member_id().clone(), frame.frame_seq()))
                || !self.observation_frame_heads.contains_key(frame.member_id())
                || frame.frame_seq()
                    > self
                        .observation_frame_heads
                        .get(frame.member_id())
                        .copied()
                        .unwrap_or(0)
                || frame.cause_room_seq().get() == 0
                || frame.cause_room_seq().get() > maximum_cause
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        let mut consequence_keys = std::collections::BTreeSet::new();
        for consequence in &self.observation_consequences {
            if !consequence_keys.insert((
                consequence.member_id().clone(),
                consequence.cause_room_seq(),
            )) || !self
                .observation_frame_heads
                .contains_key(consequence.member_id())
                || consequence.cause_room_seq().get() == 0
                || consequence.cause_room_seq().get() > maximum_cause
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        let mut decision_keys = std::collections::BTreeSet::new();
        for decision in &self.activation_decisions {
            let record = CanonicalJsonV1::decode_canonical::<ActivationDecisionV1>(
                decision.canonical_decision_bytes(),
            )
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
            if decision.cause_room_seq().get() == 0
                || decision.cause_room_seq().get() > maximum_cause
                || record.cause_room_seq != decision.cause_room_seq()
                || record.decision_id != decision.decision_id()
                || Some(&record.attention.target_member_id) != decision.target_member_id()
                || !decision_keys
                    .insert((decision.cause_room_seq(), decision.decision_id().to_owned()))
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
        }
        Ok(())
    }
}

impl RecoveredObservationConsequenceV1 {
    #[must_use]
    pub fn reset_required(
        member_id: MemberId,
        cause_room_seq: RoomSequenceV1,
        projection_hash: Blake3DigestV1,
    ) -> Self {
        Self::ResetRequired {
            member_id,
            cause_room_seq,
            projection_hash,
        }
    }

    #[must_use]
    pub fn visibility_lost(member_id: MemberId, cause_room_seq: RoomSequenceV1) -> Self {
        Self::VisibilityLost {
            member_id,
            cause_room_seq,
        }
    }

    pub(crate) fn from_replay_reset(
        member_id: MemberId,
        cause_room_seq: RoomSequenceV1,
        projection_hash: Blake3DigestV1,
    ) -> Self {
        Self::ResetRequired {
            member_id,
            cause_room_seq,
            projection_hash,
        }
    }

    pub(crate) fn from_replay_visibility_lost(
        member_id: MemberId,
        cause_room_seq: RoomSequenceV1,
    ) -> Self {
        Self::VisibilityLost {
            member_id,
            cause_room_seq,
        }
    }

    #[must_use]
    pub const fn member_id(&self) -> &MemberId {
        match self {
            Self::ResetRequired { member_id, .. } | Self::VisibilityLost { member_id, .. } => {
                member_id
            }
        }
    }

    #[must_use]
    pub const fn cause_room_seq(&self) -> RoomSequenceV1 {
        match self {
            Self::ResetRequired { cause_room_seq, .. }
            | Self::VisibilityLost { cause_room_seq, .. } => *cause_room_seq,
        }
    }
}

impl RecoveredObservationFrameV1 {
    #[must_use]
    pub fn from_replay(
        member_id: MemberId,
        frame_seq: u64,
        cause_room_seq: RoomSequenceV1,
        payload_hash: Blake3DigestV1,
    ) -> Self {
        Self {
            member_id,
            frame_seq,
            cause_room_seq,
            payload_hash,
        }
    }

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

    /// Captures the canonical Genesis-to-Head fallback after a disposable
    /// checkpoint candidate cannot be trusted.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery failure if the full candidate cannot be read safely.
    fn inspect_full_recovery_candidate(
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

/// The successful replay path that delivered immutable Transition records to
/// Core. A checkpoint receipt is emitted only after the bounded checkpoint
/// replay and guarded install both complete; a checkpoint fallback emits a
/// full receipt instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomRecoveryExecutionPathV1 {
    Checkpoint,
    Full,
}

/// Closed accounting for one completed trusted recovery execution.
///
/// The counts are records actually handed to Core's replay entrypoint, rather
/// than adapter query estimates. `prefix_transitions_skipped` is nonzero only
/// for a completed checkpoint path. Full recovery delivers its complete
/// Genesis-to-Head Transition sequence as the prefix and never reports a
/// checkpoint tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomRecoveryExecutionReceiptV1 {
    path: RoomRecoveryExecutionPathV1,
    checkpoint_room_seq: Option<RoomSequenceV1>,
    prefix_transition_records_delivered: u64,
    prefix_transitions_skipped: u64,
    tail_transition_records_delivered: u64,
}

impl RoomRecoveryExecutionReceiptV1 {
    fn for_candidate(candidate: &RoomRecoveryCandidateV1) -> Result<Self, RoomRecoveryErrorV1> {
        let transition_count = u64::try_from(candidate.canonical_transition_bytes.len())
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if let Some(checkpoint) = candidate.checkpoint() {
            let checkpoint_room_seq = checkpoint.head().room_seq();
            return Ok(Self {
                path: RoomRecoveryExecutionPathV1::Checkpoint,
                checkpoint_room_seq: Some(checkpoint_room_seq),
                prefix_transition_records_delivered: 0,
                prefix_transitions_skipped: checkpoint_room_seq.get(),
                tail_transition_records_delivered: transition_count,
            });
        }
        Ok(Self {
            path: RoomRecoveryExecutionPathV1::Full,
            checkpoint_room_seq: None,
            prefix_transition_records_delivered: transition_count,
            prefix_transitions_skipped: 0,
            tail_transition_records_delivered: 0,
        })
    }

    #[must_use]
    pub const fn path(self) -> RoomRecoveryExecutionPathV1 {
        self.path
    }

    #[must_use]
    pub const fn used_checkpoint(self) -> bool {
        matches!(self.path, RoomRecoveryExecutionPathV1::Checkpoint)
    }

    #[must_use]
    pub const fn checkpoint_room_seq(self) -> Option<RoomSequenceV1> {
        self.checkpoint_room_seq
    }

    #[must_use]
    pub const fn prefix_transition_records_delivered(self) -> u64 {
        self.prefix_transition_records_delivered
    }

    #[must_use]
    pub const fn prefix_transitions_skipped(self) -> u64 {
        self.prefix_transitions_skipped
    }

    #[must_use]
    pub const fn tail_transition_records_delivered(self) -> u64 {
        self.tail_transition_records_delivered
    }
}

/// The executable trace and closed execution receipt yielded after a guarded
/// recovery install.
pub struct RecoveredRoomExecutionV1 {
    trace: CoreTraceV1,
    receipt: RoomRecoveryExecutionReceiptV1,
}

impl RecoveredRoomExecutionV1 {
    #[must_use]
    pub const fn trace(&self) -> &CoreTraceV1 {
        &self.trace
    }

    #[must_use]
    pub const fn receipt(&self) -> RoomRecoveryExecutionReceiptV1 {
        self.receipt
    }

    #[must_use]
    pub fn into_trace(self) -> CoreTraceV1 {
        self.trace
    }
}

/// Host-internal adapter SPI that loads, registry-replays, projection-verifies,
/// and finally fences one Room recovery before yielding its executable trace.
/// Application Replay and diagnostic callers must use their present-authorized
/// facades instead; this coordinator exists only for trusted actor recovery.
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
    recover_room_from_storage_with_receipt(storage, registry, room_id)
        .map(|execution| execution.map(RecoveredRoomExecutionV1::into_trace))
}

/// Recovers one Room through the ordinary trusted storage SPI and returns a
/// receipt for the path that actually completed. This is a qualification seam;
/// serving callers should use [`recover_room_from_storage`].
///
/// # Errors
///
/// Returns the same closed recovery errors as [`recover_room_from_storage`].
pub fn recover_room_from_storage_with_receipt(
    storage: &dyn RoomRecoveryStorageV1,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<RecoveredRoomExecutionV1>, RoomRecoveryErrorV1> {
    let Some(candidate) = storage.inspect_recovery_candidate(room_id)? else {
        return Ok(None);
    };
    let used_checkpoint = candidate.has_checkpoint();
    match recover_room_candidate(storage, registry, room_id, &candidate, !used_checkpoint) {
        Ok(execution) => Ok(Some(execution)),
        Err(
            error @ (RoomRecoveryErrorV1::Corrupt
            | RoomRecoveryErrorV1::RuntimeUnavailable
            | RoomRecoveryErrorV1::RuntimeFault),
        ) if used_checkpoint => {
            let Some(fallback) = storage.inspect_full_recovery_candidate(room_id)? else {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            };
            if fallback.has_checkpoint() {
                return Err(error);
            }
            recover_room_candidate(storage, registry, room_id, &fallback, true).map(Some)
        }
        Err(error) => Err(error),
    }
}

/// Replays the canonical Genesis-to-Head candidate even when a disposable
/// checkpoint is available. Trusted storage maintenance uses this to create a
/// new checkpoint only after the entire retained lineage and the current
/// materializations have passed the ordinary guarded recovery fence.
///
/// # Errors
///
/// Returns a closed recovery error if the canonical candidate cannot be read,
/// replayed, or installed at its exact durable fence.
pub fn recover_room_from_full_storage(
    storage: &dyn RoomRecoveryStorageV1,
    registry: &PackRegistryV1,
    room_id: &RoomId,
) -> Result<Option<CoreTraceV1>, RoomRecoveryErrorV1> {
    let Some(candidate) = storage.inspect_full_recovery_candidate(room_id)? else {
        return Ok(None);
    };
    if candidate.has_checkpoint() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    recover_room_candidate(storage, registry, room_id, &candidate, true)
        .map(|execution| Some(execution.into_trace()))
}

#[allow(clippy::too_many_lines)]
fn recover_room_candidate(
    storage: &dyn RoomRecoveryStorageV1,
    registry: &PackRegistryV1,
    room_id: &RoomId,
    candidate: &RoomRecoveryCandidateV1,
    record_failure: bool,
) -> Result<RecoveredRoomExecutionV1, RoomRecoveryErrorV1> {
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
        let report = if let Some(checkpoint) = candidate.checkpoint() {
            CoreTraceV1::replay_checkpoint_for_recovery(
                registry,
                &candidate.canonical_genesis_bytes,
                checkpoint,
                &candidate.canonical_transition_bytes,
            )
        } else {
            candidate.preflight_lineage_materializations()?;
            CoreTraceV1::replay_for_recovery(
                registry,
                &candidate.canonical_genesis_bytes,
                &candidate.canonical_transition_bytes,
            )
        }
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
        let recovered_materializations = recover_materializations(candidate, &report)?;
        Ok((report, recovered_materializations))
    })();
    let (report, recovered_materializations) = match verified {
        Ok(value) => value,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(RoomRecoveryErrorV1::RuntimeUnavailable) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
            }
            storage.record_recovery_failure(
                room_id,
                &candidate.head,
                candidate.integrity_generation,
                RecoveryIntegrityDispositionV1::Faulted,
            )?;
            return Err(RoomRecoveryErrorV1::RuntimeUnavailable);
        }
        Err(RoomRecoveryErrorV1::RuntimeFault) => {
            if !record_failure {
                return Err(RoomRecoveryErrorV1::RuntimeFault);
            }
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
    Ok(RecoveredRoomExecutionV1 {
        trace: report.into_trace_after_recovery_fence(),
        receipt: RoomRecoveryExecutionReceiptV1::for_candidate(candidate)?,
    })
}

#[allow(clippy::too_many_lines)]
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
    let operational_history_roots = candidate
        .checkpoint()
        .and_then(RoomRecoveryCheckpointV1::operational_history_roots)
        .map(|roots| {
            advance_operational_history_roots(roots, report, &candidate.canonical_transition_bytes)
        })
        .transpose()?;
    Ok(RecoveredRoomMaterializationsV1 {
        room_status: report.final_state().core_state().room_status(),
        canonical_core_state_bytes,
        canonical_activity_state_bytes,
        memberships,
        timers: if let Some(checkpoint) = candidate.checkpoint() {
            recover_timer_ledger_from_checkpoint(checkpoint, &candidate.canonical_transition_bytes)?
        } else {
            recover_timer_ledger(
                &candidate.canonical_genesis_bytes,
                &candidate.canonical_transition_bytes,
            )?
        },
        observation_frames: {
            let mut frames = candidate.checkpoint().map_or_else(Vec::new, |checkpoint| {
                checkpoint.observation_frames().to_vec()
            });
            frames.extend(
                report.observation_consequences().iter().filter_map(
                    |consequence| match consequence {
                        crate::trace::ReplayObservationConsequenceV1::ObservationFrame(frame) => {
                            Some(RecoveredObservationFrameV1 {
                                member_id: frame.member_id().clone(),
                                frame_seq: frame.frame_seq(),
                                cause_room_seq: frame.cause_room_seq(),
                                payload_hash: frame.payload_hash().clone(),
                            })
                        }
                        crate::trace::ReplayObservationConsequenceV1::ResetRequired { .. }
                        | crate::trace::ReplayObservationConsequenceV1::VisibilityLost { .. } => {
                            None
                        }
                    },
                ),
            );
            frames
        },
        observation_consequences: {
            let mut consequences = candidate.checkpoint().map_or_else(Vec::new, |checkpoint| {
                checkpoint.observation_consequences().to_vec()
            });
            consequences.extend(report.observation_consequences().iter().filter_map(
                |consequence| match consequence {
                    crate::trace::ReplayObservationConsequenceV1::ObservationFrame(_) => None,
                    crate::trace::ReplayObservationConsequenceV1::ResetRequired {
                        member_id,
                        cause_room_seq,
                        projection_hash,
                    } => Some(RecoveredObservationConsequenceV1::ResetRequired {
                        member_id: member_id.clone(),
                        cause_room_seq: *cause_room_seq,
                        projection_hash: projection_hash.clone(),
                    }),
                    crate::trace::ReplayObservationConsequenceV1::VisibilityLost {
                        member_id,
                        cause_room_seq,
                    } => Some(RecoveredObservationConsequenceV1::VisibilityLost {
                        member_id: member_id.clone(),
                        cause_room_seq: *cause_room_seq,
                    }),
                },
            ));
            consequences
        },
        observation_frame_heads: {
            let mut heads = candidate.checkpoint().map_or_else(
                || {
                    report
                        .final_state()
                        .core_state()
                        .memberships()
                        .keys()
                        .cloned()
                        .map(|member_id| (member_id, 0))
                        .collect::<BTreeMap<_, _>>()
                },
                |checkpoint| checkpoint.observation_frame_heads().clone(),
            );
            for consequence in report.observation_consequences() {
                if let crate::trace::ReplayObservationConsequenceV1::ObservationFrame(frame) =
                    consequence
                {
                    heads.insert(frame.member_id().clone(), frame.frame_seq());
                }
            }
            heads.retain(|member_id, _| {
                report
                    .final_state()
                    .core_state()
                    .memberships()
                    .contains_key(member_id)
            });
            for member_id in report.final_state().core_state().memberships().keys() {
                heads.entry(member_id.clone()).or_insert(0);
            }
            heads
        },
        membership_generations: candidate
            .checkpoint()
            .map(|checkpoint| {
                recover_membership_generations_from_checkpoint(
                    checkpoint,
                    &candidate.canonical_transition_bytes,
                )
            })
            .transpose()?,
        activation_decisions: {
            let mut decisions = candidate.checkpoint().map_or_else(Vec::new, |checkpoint| {
                checkpoint.activation_decisions().to_vec()
            });
            decisions.extend(recover_activation_decisions(
                &candidate.canonical_transition_bytes,
            )?);
            decisions
        },
        operational_history_roots,
    })
}

fn advance_operational_history_roots(
    roots: &BTreeMap<String, OperationalHistoryRootV2>,
    report: &crate::ReplayReportV1,
    tail_transition_bytes: &[Vec<u8>],
) -> Result<BTreeMap<String, OperationalHistoryRootV2>, RoomRecoveryErrorV1> {
    let mut advanced = roots.clone();
    for consequence in report.observation_consequences() {
        match consequence {
            crate::trace::ReplayObservationConsequenceV1::ObservationFrame(frame) => {
                append_operational_history_root_v2(
                    &mut advanced,
                    "frames",
                    &operational_history_entry_v2(&[
                        frame.member_id().to_string().as_bytes(),
                        &frame.frame_seq().to_be_bytes(),
                        &frame.cause_room_seq().get().to_be_bytes(),
                        frame.payload_hash().as_bytes(),
                    ])?,
                )?;
            }
            crate::trace::ReplayObservationConsequenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                projection_hash,
            } => {
                append_operational_history_root_v2(
                    &mut advanced,
                    "consequences",
                    &operational_history_entry_v2(&[
                        member_id.to_string().as_bytes(),
                        &cause_room_seq.get().to_be_bytes(),
                        b"reset_required",
                        projection_hash.as_bytes(),
                    ])?,
                )?;
            }
            crate::trace::ReplayObservationConsequenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => {
                append_operational_history_root_v2(
                    &mut advanced,
                    "consequences",
                    &operational_history_entry_v2(&[
                        member_id.to_string().as_bytes(),
                        &cause_room_seq.get().to_be_bytes(),
                        b"visibility_lost",
                    ])?,
                )?;
            }
        }
    }
    for decision in recover_activation_decisions(tail_transition_bytes)? {
        let target_member_id = decision
            .target_member_id()
            .map(ToString::to_string)
            .unwrap_or_default();
        append_operational_history_root_v2(
            &mut advanced,
            "activation_decisions",
            &operational_history_entry_v2(&[
                &decision.cause_room_seq().get().to_be_bytes(),
                decision.decision_id().as_bytes(),
                target_member_id.as_bytes(),
                decision.canonical_decision_bytes(),
            ])?,
        )?;
    }
    Ok(advanced)
}

fn operational_history_entry_v2(parts: &[&[u8]]) -> Result<Vec<u8>, RoomRecoveryErrorV1> {
    let mut entry = Vec::new();
    for part in parts {
        entry.extend_from_slice(
            &u64::try_from(part.len())
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
                .to_be_bytes(),
        );
        entry.extend_from_slice(part);
    }
    Ok(entry)
}

fn append_operational_history_root_v2(
    roots: &mut BTreeMap<String, OperationalHistoryRootV2>,
    domain: &str,
    entry: &[u8],
) -> Result<(), RoomRecoveryErrorV1> {
    let root = roots.get(domain).cloned().ok_or(RoomRecoveryErrorV1::Corrupt)?;
    let next_count = root
        .entry_count()
        .checked_add(1)
        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    let mut input = Vec::with_capacity(
        OPERATIONAL_HISTORY_ROOT_DOMAIN_TAG_V2.len()
            + root.root_hash().as_bytes().len()
            + domain.len()
            + entry.len()
            + 24,
    );
    input.extend_from_slice(OPERATIONAL_HISTORY_ROOT_DOMAIN_TAG_V2);
    input.extend_from_slice(
        &u64::try_from(domain.len())
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .to_be_bytes(),
    );
    input.extend_from_slice(domain.as_bytes());
    input.extend_from_slice(&root.entry_count().to_be_bytes());
    input.extend_from_slice(root.root_hash().as_bytes());
    input.extend_from_slice(
        &u64::try_from(entry.len())
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .to_be_bytes(),
    );
    input.extend_from_slice(entry);
    roots.insert(
        domain.to_owned(),
        OperationalHistoryRootV2::new(domain, next_count, Blake3DigestV1::hash(&input))?,
    );
    Ok(())
}

fn recover_activation_decisions(
    canonical_transition_bytes: &[Vec<u8>],
) -> Result<Vec<RecoveredActivationDecisionV1>, RoomRecoveryErrorV1> {
    let mut decisions = Vec::new();
    for bytes in canonical_transition_bytes {
        let transition =
            TransitionV1::from_canonical_bytes(bytes).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        for prepared in
            prepare_activation_decisions(&transition).map_err(|_| RoomRecoveryErrorV1::Corrupt)?
        {
            decisions.push(RecoveredActivationDecisionV1::new(
                transition.room_seq(),
                prepared.decision_id().to_owned(),
                prepared.target_member_id().cloned(),
                prepared.canonical_decision_bytes().to_vec(),
            ));
        }
    }
    Ok(decisions)
}

fn recover_membership_generations_from_checkpoint(
    checkpoint: &RoomRecoveryCheckpointV1,
    canonical_transition_bytes: &[Vec<u8>],
) -> Result<BTreeMap<String, i64>, RoomRecoveryErrorV1> {
    let checkpoint_core =
        CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(checkpoint.core_state_bytes())
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let mut previous = checkpoint_core.memberships().clone();
    let mut generations = checkpoint.membership_generations().clone();
    if generations.len() != previous.len()
        || previous.keys().any(|member_id| {
            generations
                .get(member_id.as_str())
                .is_none_or(|generation| *generation < 1)
        })
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    for bytes in canonical_transition_bytes {
        let transition =
            TransitionV1::from_canonical_bytes(bytes).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        for (member_id, membership) in transition.resulting_core_state().memberships() {
            let key = member_id.to_string();
            match previous.get(member_id) {
                Some(prior) if prior != membership => {
                    let generation = generations
                        .get_mut(&key)
                        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
                    *generation = generation
                        .checked_add(1)
                        .filter(|value| {
                            *value <= i64::try_from(MAX_SAFE_INTEGER_U64).unwrap_or(i64::MAX)
                        })
                        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
                }
                Some(_) => {}
                None => {
                    if generations.insert(key, 1).is_some() {
                        return Err(RoomRecoveryErrorV1::Corrupt);
                    }
                }
            }
        }
        if previous.keys().any(|member_id| {
            !transition
                .resulting_core_state()
                .memberships()
                .contains_key(member_id)
        }) {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        previous = transition.resulting_core_state().memberships().clone();
    }
    Ok(generations)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn recover_timer_ledger(
    canonical_genesis_bytes: &[u8],
    canonical_transition_bytes: &[Vec<u8>],
) -> Result<Vec<RecoveredTimerMaterializationV1>, RoomRecoveryErrorV1> {
    let genesis = GenesisV1::from_canonical_bytes(canonical_genesis_bytes)
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
    for bytes in canonical_transition_bytes {
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

fn recover_timer_ledger_from_checkpoint(
    checkpoint: &RoomRecoveryCheckpointV1,
    canonical_transition_bytes: &[Vec<u8>],
) -> Result<Vec<RecoveredTimerMaterializationV1>, RoomRecoveryErrorV1> {
    let mut timers = BTreeMap::new();
    let mut scheduled = BTreeMap::new();
    let mut last_generation = BTreeMap::new();
    for row in checkpoint.timers() {
        if timers
            .insert((row.timer_id().clone(), row.generation()), row.clone())
            .is_some()
            || last_generation
                .insert(row.timer_id().clone(), row.generation())
                .is_some_and(|previous| previous >= row.generation())
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        if row.state() == RecoveredTimerStateV1::Scheduled
            && scheduled
                .insert(row.timer_id().clone(), row.generation())
                .is_some()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    for bytes in canonical_transition_bytes {
        let transition =
            TransitionV1::from_canonical_bytes(bytes).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if let RecordedStimulusV1::TimerFired(fired) = transition.recorded_stimulus() {
            if scheduled.get(&fired.timer_id) != Some(&fired.generation) {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            let row = timers
                .get_mut(&(fired.timer_id.clone(), fired.generation))
                .ok_or(RoomRecoveryErrorV1::Corrupt)?;
            if row.state() != RecoveredTimerStateV1::Scheduled
                || row.scheduled_for() != &fired.scheduled_for
                || row.canonical_payload_bytes()
                    != fired
                        .canonical_payload
                        .to_bytes()
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            {
                return Err(RoomRecoveryErrorV1::Corrupt);
            }
            *row = RecoveredTimerMaterializationV1::new(
                row.timer_id().clone(),
                row.generation(),
                row.scheduled_for().clone(),
                row.canonical_payload_bytes().to_vec(),
                RecoveredTimerStateV1::Fired,
            );
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

/// Present-authorized receipt resolver implemented by a durable Adapter.
///
/// Implementations consume the grant, sample trusted time, reread its exact
/// current authority snapshot, and perform resolution under one serialized
/// transaction. This remains separate from the frozen two-method mutation SPI.
pub trait AuthorizedReceiptResolverV1: Send + Sync {
    /// Resolves one currently authorized receipt without releasing cross-Room
    /// identity, conflict, or result information.
    ///
    /// # Errors
    ///
    /// Returns a safe authority failure when current authority cannot be
    /// revalidated at the lookup boundary.
    fn resolve_authorized(
        &self,
        authority: AuthorizedReceiptReadV1,
    ) -> Result<ResolveOutcomeV1, AuthorityErrorV1>;
}

/// Result of receipt-first Room creation ingress.
pub enum RoomCreationIngressV1 {
    /// The original same-identity/hash Genesis receipt wins. No speculative
    /// Genesis values are generated or released.
    Existing(Box<StoredSemanticResultV1>),
    /// The identity already belongs to a different caller-semantic request.
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    /// Guarded absence was established and exact creation authority is ready
    /// to be consumed by Genesis preparation.
    Authorized(Box<AuthorizedRoomCreationV1>),
}

impl fmt::Debug for RoomCreationIngressV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Existing(_) => "RoomCreationIngressV1::Existing([REDACTED])",
            Self::Conflict { .. } => "RoomCreationIngressV1::Conflict([REDACTED])",
            Self::Authorized(_) => "RoomCreationIngressV1::Authorized([OPAQUE])",
        })
    }
}

/// Result of receipt-first existing-Room Core administration ingress.
pub enum CoreAdministrationIngressV1 {
    /// The original same-identity/hash administration receipt wins before
    /// current Room lifecycle or retained-pack execution is consulted.
    Existing(Box<StoredSemanticResultV1>),
    /// The identity already belongs to a different caller-semantic request.
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    /// Guarded absence was established and exact administration authority is
    /// ready to be consumed by preparation.
    Authorized(Box<AuthorizedCoreAdministrationV1>),
}

impl fmt::Debug for CoreAdministrationIngressV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Existing(_) => "CoreAdministrationIngressV1::Existing([REDACTED])",
            Self::Conflict { .. } => "CoreAdministrationIngressV1::Conflict([REDACTED])",
            Self::Authorized(_) => "CoreAdministrationIngressV1::Authorized([OPAQUE])",
        })
    }
}

/// Closed failure before creation or Core administration preparation.
#[derive(Debug, Error)]
pub enum RoomOperationIngressErrorV1 {
    #[error(transparent)]
    Authority(#[from] AuthorityErrorV1),
    #[error("guarded receipt resolution is unavailable")]
    ResolutionUnavailable,
    #[error("stored Room operation result disagrees with its requested identity")]
    InvalidStoredResult,
}

/// Resolves an existing Create receipt before selecting/running a pack or
/// generating Room IDs, Member IDs, seed, or semantic time.
///
/// # Errors
///
/// Returns a safe authority, guarded-resolution, or stored-result failure. No
/// speculative Genesis is constructed on failure or duplicate success.
pub fn authorize_room_creation_operation(
    authority: &AuthorityV1,
    resolver: &dyn AuthorizedReceiptResolverV1,
    presented: &PresentedCapabilityV1,
    identity: &AdministrationOperationIdentityV1,
    request: &RoomCreationRequestV1,
    checked_at: AuthorityCheckedAt,
) -> Result<RoomCreationIngressV1, RoomOperationIngressErrorV1> {
    let request_hash = request
        .canonical_request_hash()
        .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
    let operation_identity = OperationIdentityV1::Administration(Box::new(identity.clone()));
    let receipt_grant = authority.authorize_receipt_read(
        presented,
        operation_identity.clone(),
        request_hash.clone(),
        None,
        checked_at.clone(),
    )?;
    match resolver.resolve_authorized(receipt_grant)? {
        ResolveOutcomeV1::StoredResolution(result)
            if result.operation_identity() == &operation_identity
                && result.canonical_request_hash() == &request_hash
                && matches!(
                    result.semantic_input(),
                    ReceiptSemanticInputV1::RoomCreation { request: stored, .. }
                        if stored == request
                )
                && matches!(result.result(), SemanticResultV1::GenesisCreated { .. }) =>
        {
            Ok(RoomCreationIngressV1::Existing(result))
        }
        ResolveOutcomeV1::StoredResolution(_) => {
            Err(RoomOperationIngressErrorV1::InvalidStoredResult)
        }
        ResolveOutcomeV1::Conflict {
            existing_request_hash,
        } => Ok(RoomCreationIngressV1::Conflict {
            existing_request_hash,
        }),
        ResolveOutcomeV1::KnownAbsent => authority
            .authorize_room_creation(presented, identity.clone(), request, checked_at)
            .map(|grant| RoomCreationIngressV1::Authorized(Box::new(grant)))
            .map_err(Into::into),
        ResolveOutcomeV1::ResolutionUnavailable => {
            Err(RoomOperationIngressErrorV1::ResolutionUnavailable)
        }
    }
}

/// Resolves an existing Core administration receipt before current Room
/// lifecycle, retained-pack policy, reduction, or semantic time is consulted.
///
/// # Errors
///
/// Returns a safe authority, guarded-resolution, or stored-result failure. No
/// administration preparation or durable mutation occurs on failure.
pub fn authorize_core_administration_operation(
    authority: &AuthorityV1,
    resolver: &dyn AuthorizedReceiptResolverV1,
    presented: &PresentedCapabilityV1,
    request: &CoreAdministrationRequestV1,
    checked_at: AuthorityCheckedAt,
) -> Result<CoreAdministrationIngressV1, RoomOperationIngressErrorV1> {
    let request_hash = request
        .canonical_request_hash()
        .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
    let operation_identity =
        OperationIdentityV1::Administration(Box::new(request.operation_identity().clone()));
    let receipt_grant = authority.authorize_receipt_read(
        presented,
        operation_identity.clone(),
        request_hash.clone(),
        Some(request.room_id().clone()),
        checked_at.clone(),
    )?;
    match resolver.resolve_authorized(receipt_grant)? {
        ResolveOutcomeV1::StoredResolution(result)
            if result.operation_identity() == &operation_identity
                && result.canonical_request_hash() == &request_hash
                && result.target_room_id() == request.room_id()
                && matches!(
                    result.semantic_input(),
                    ReceiptSemanticInputV1::CoreAdministration { proposal }
                        if proposal.kind() == request.kind()
                            && proposal.operation_identity() == request.operation_identity()
                            && proposal.authority_attribution().principal_id
                                == request.operation_identity().authenticated_principal
                            && proposal.expected_room_seq() == request.expected_room_seq()
                            && proposal.reason_code() == request.reason_code()
                            && proposal.changeset() == request.changeset()
                )
                && !matches!(result.result(), SemanticResultV1::GenesisCreated { .. }) =>
        {
            Ok(CoreAdministrationIngressV1::Existing(result))
        }
        ResolveOutcomeV1::StoredResolution(_) => {
            Err(RoomOperationIngressErrorV1::InvalidStoredResult)
        }
        ResolveOutcomeV1::Conflict {
            existing_request_hash,
        } => Ok(CoreAdministrationIngressV1::Conflict {
            existing_request_hash,
        }),
        ResolveOutcomeV1::KnownAbsent => authority
            .authorize_core_administration(presented, request, checked_at)
            .map(|grant| CoreAdministrationIngressV1::Authorized(Box::new(grant)))
            .map_err(Into::into),
        ResolveOutcomeV1::ResolutionUnavailable => {
            Err(RoomOperationIngressErrorV1::ResolutionUnavailable)
        }
    }
}

/// Result of the receipt-first Participant Action ingress boundary.
pub enum ParticipantActionIngressV1 {
    /// The immutable original same-identity/hash receipt wins before current
    /// Action eligibility is consulted.
    Existing(Box<StoredSemanticResultV1>),
    /// The identity already belongs to a different caller-semantic request.
    Conflict {
        existing_request_hash: CanonicalRequestHashV1,
    },
    /// No receipt exists and present new-work authority was granted.
    Authorized(Box<ParticipantActionAuthorityV1>),
}

impl fmt::Debug for ParticipantActionIngressV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Existing(_) => "ParticipantActionIngressV1::Existing([REDACTED])",
            Self::Conflict { .. } => "ParticipantActionIngressV1::Conflict([REDACTED])",
            Self::Authorized(_) => "ParticipantActionIngressV1::Authorized([OPAQUE])",
        })
    }
}

/// Closed failure before an Action reaches preparation or durable mutation.
#[derive(Debug, Error)]
pub enum ParticipantActionIngressErrorV1 {
    #[error(transparent)]
    Authority(#[from] AuthorityErrorV1),
    #[error("guarded receipt resolution is unavailable")]
    ResolutionUnavailable,
    #[error("stored Action result disagrees with its requested identity")]
    InvalidStoredResult,
}

/// Resolves an existing Action receipt before consulting Standing, Access
/// Mode, Role, Room lifecycle, offers, or pack admission for a new identity.
///
/// # Errors
///
/// Returns a safe authority, guarded-resolution, or stored-result failure. No
/// Action preparation or durable mutation occurs on failure.
pub fn authorize_participant_action_operation(
    authority: &AuthorityV1,
    resolver: &dyn AuthorizedReceiptResolverV1,
    presented: &PresentedCapabilityV1,
    request: &ParticipantActionRequestV1,
    checked_at: AuthorityCheckedAt,
) -> Result<ParticipantActionIngressV1, ParticipantActionIngressErrorV1> {
    let receipt_grant =
        authority.authorize_action_receipt_read(presented, request, checked_at.clone())?;
    match resolver.resolve_authorized(receipt_grant)? {
        ResolveOutcomeV1::StoredResolution(result) => {
            let expected_identity = OperationIdentityV1::ParticipantAction(Box::new(
                ParticipantActionOperationIdentityV1 {
                    room_id: request.room_id().clone(),
                    member_id: request.member_id().clone(),
                    action_id: request.action_id().clone(),
                },
            ));
            let expected_hash = request
                .canonical_request_hash()
                .map_err(|_| AuthorityErrorV1::InvalidAuthorityRequest)?;
            if result.operation_identity() != &expected_identity
                || result.canonical_request_hash() != &expected_hash
                || result.target_room_id() != request.room_id()
            {
                return Err(ParticipantActionIngressErrorV1::InvalidStoredResult);
            }
            Ok(ParticipantActionIngressV1::Existing(result))
        }
        ResolveOutcomeV1::Conflict {
            existing_request_hash,
        } => Ok(ParticipantActionIngressV1::Conflict {
            existing_request_hash,
        }),
        ResolveOutcomeV1::KnownAbsent => authority
            .authorize_action(presented, request, checked_at)
            .map(|grant| ParticipantActionIngressV1::Authorized(Box::new(grant)))
            .map_err(Into::into),
        ResolveOutcomeV1::ResolutionUnavailable => {
            Err(ParticipantActionIngressErrorV1::ResolutionUnavailable)
        }
    }
}

/// Applies exact identity/hash/Room filtering for a trusted receipt Adapter.
///
/// Before calling this helper, the Adapter must call
/// [`ReceiptReadAdapterInputV1::revalidate_current`] with an Adapter-owned
/// trusted clock sample and current authority snapshot inside the same
/// serialized transaction. This pure filter does not itself establish current
/// authorization. The resolver closure is invoked once for the requested hash
/// and, for an administration conflict, once more for the immutable existing
/// hash.
#[doc(hidden)]
#[must_use]
pub fn resolve_authorized_room_operation_for_adapter<F>(
    authority: ReceiptReadAdapterInputV1,
    mut resolve: F,
) -> ResolveOutcomeV1
where
    F: FnMut(&OperationIdentityV1, &CanonicalRequestHashV1) -> ResolveOutcomeV1,
{
    let ReceiptReadAdapterInputV1 {
        fence: _,
        identity,
        request_hash,
        target_policy,
    } = authority;
    match resolve(&identity, &request_hash) {
        ResolveOutcomeV1::StoredResolution(result)
            if authorized_receipt_matches(
                &identity,
                &request_hash,
                &target_policy,
                result.as_ref(),
            ) =>
        {
            ResolveOutcomeV1::StoredResolution(result)
        }
        ResolveOutcomeV1::Conflict {
            existing_request_hash,
        } if existing_request_hash != request_hash => {
            match resolve(&identity, &existing_request_hash) {
                ResolveOutcomeV1::StoredResolution(existing)
                    if authorized_receipt_matches(
                        &identity,
                        &existing_request_hash,
                        &target_policy,
                        existing.as_ref(),
                    ) =>
                {
                    ResolveOutcomeV1::Conflict {
                        existing_request_hash,
                    }
                }
                ResolveOutcomeV1::StoredResolution(_)
                | ResolveOutcomeV1::Conflict { .. }
                | ResolveOutcomeV1::KnownAbsent
                | ResolveOutcomeV1::ResolutionUnavailable => {
                    ResolveOutcomeV1::ResolutionUnavailable
                }
            }
        }
        ResolveOutcomeV1::KnownAbsent => ResolveOutcomeV1::KnownAbsent,
        ResolveOutcomeV1::StoredResolution(_)
        | ResolveOutcomeV1::Conflict { .. }
        | ResolveOutcomeV1::ResolutionUnavailable => ResolveOutcomeV1::ResolutionUnavailable,
    }
}

fn authorized_receipt_matches(
    identity: &OperationIdentityV1,
    request_hash: &CanonicalRequestHashV1,
    target_policy: &ReceiptReadTargetPolicyV1,
    result: &StoredSemanticResultV1,
) -> bool {
    if result.operation_identity() != identity || result.canonical_request_hash() != request_hash {
        return false;
    }
    match target_policy {
        ReceiptReadTargetPolicyV1::ExactRoom(room_id) => result.target_room_id() == room_id,
        ReceiptReadTargetPolicyV1::GlobalCreate => {
            matches!(
                identity,
                OperationIdentityV1::Administration(identity)
                    if identity.versioned_operation_kind == CREATE_ROOM_OPERATION_KIND
            ) && matches!(result.result(), SemanticResultV1::GenesisCreated { .. })
        }
    }
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
    #[error("purpose-sealed authority does not match the prepared operation")]
    AuthorityPurposeMismatch,
    #[error("creation trace has already advanced")]
    CreationTraceAdvanced,
    #[error("creation trace is not bound to an exact retained Pack revision")]
    CreationTraceNotRegistryBound,
    #[error("creation request does not exactly match the initialized trace")]
    CreationRequestMismatch,
    #[error("administration identity does not name the frozen create-Room operation")]
    InvalidCreationOperationKind,
    #[error("Core administration request has an invalid identity, reason, or changeset class")]
    InvalidCoreAdministrationRequest,
    #[error("prepared operation is not the exact authorized Core administration request")]
    CoreAdministrationMismatch,
    #[error("prepared Transition basis does not match the supplied trace")]
    PreparedBasisMismatch,
    #[error("prepared operation is not a participant Action")]
    NotParticipantAction,
    #[error("prepared operation is not an exact Timer firing")]
    NotTimerFired,
    #[error("prepared Timer stimulus does not match its immutable request")]
    TimerRequestMismatch,
    #[error("prepared operation is not an exact ExternalInput")]
    NotExternalInput,
    #[error("prepared ExternalInput does not match its immutable request")]
    ExternalInputRequestMismatch,
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
