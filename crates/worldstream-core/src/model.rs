use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

use crate::{
    CanonicalJsonV1,
    primitives::{
        ActionAdmittedAt, ActionId, Blake3DigestV1, CoreRecordedAt, ExternalInputRecordedAt,
        InputId, IntegrityGenerationV1, MemberId, PackDigestV1, PrincipalId, RoomId, RoomSeedV1,
        RoomSequenceV1, SourceId, TimerGenerationV1, TimerId, TimerScheduledFor,
    },
};

/// The complete v1 Core state. No operational field is part of this value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreRoomStateV1 {
    /// Memberships keyed by their embedded immutable Member ID.
    pub(crate) memberships: BTreeMap<MemberId, MembershipV1>,
    /// Canonical Room lifecycle status.
    pub(crate) room_status: RoomStatusV1,
}

impl CoreRoomStateV1 {
    /// Constructs a host-valid active Core candidate from initial Memberships.
    /// Pack Role vocabulary/cardinality is additionally checked by the tracer.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate IDs, inconsistent map bindings, or a
    /// duplicate live Principal.
    pub fn active(
        memberships: impl IntoIterator<Item = MembershipV1>,
    ) -> Result<Self, CoreShapeErrorV1> {
        let mut by_id = BTreeMap::new();
        for membership in memberships {
            let member_id = membership.member_id.clone();
            if by_id.insert(member_id.clone(), membership).is_some() {
                return Err(CoreShapeErrorV1::DuplicateMemberId(member_id));
            }
        }
        Self::from_parts(by_id, RoomStatusV1::Active)
    }

    fn from_parts(
        memberships: BTreeMap<MemberId, MembershipV1>,
        room_status: RoomStatusV1,
    ) -> Result<Self, CoreShapeErrorV1> {
        let mut live_principals = BTreeSet::new();
        for (key, membership) in &memberships {
            if key != &membership.member_id {
                return Err(CoreShapeErrorV1::MemberMapKeyMismatch);
            }
            if membership.standing != MembershipStandingV1::Departed
                && !live_principals.insert(membership.principal_id.clone())
            {
                return Err(CoreShapeErrorV1::DuplicateLivePrincipal(
                    membership.principal_id.clone(),
                ));
            }
        }
        Ok(Self {
            memberships,
            room_status,
        })
    }

    /// Returns the immutable canonical Membership map.
    #[must_use]
    pub fn memberships(&self) -> &BTreeMap<MemberId, MembershipV1> {
        &self.memberships
    }

    /// Returns one historical or current Membership by immutable ID.
    #[must_use]
    pub fn membership(&self, member_id: &MemberId) -> Option<&MembershipV1> {
        self.memberships.get(member_id)
    }

    /// Returns active or archived.
    #[must_use]
    pub const fn room_status(&self) -> RoomStatusV1 {
        self.room_status
    }
}

impl<'de> Deserialize<'de> for CoreRoomStateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            memberships: BTreeMap<MemberId, MembershipV1>,
            room_status: RoomStatusV1,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::from_parts(raw.memberships, raw.room_status).map_err(de::Error::custom)
    }
}

/// Canonical Room lifecycle status, independent of pack phase and outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatusV1 {
    /// The Room may accept operations allowed by its Activity Pack.
    Active,
    /// The Room was irreversibly archived.
    Archived,
}

/// One semantic Membership owned by Core.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipV1 {
    /// Immutable identity, duplicated in the map key and checked for equality.
    pub(crate) member_id: MemberId,
    /// Immutable authenticated Principal binding.
    pub(crate) principal_id: PrincipalId,
    /// Immutable room-local Principal kind.
    pub(crate) principal_kind: PrincipalKindV1,
    /// Whether the Membership may currently participate.
    pub(crate) standing: MembershipStandingV1,
    /// Participant, spectator, or operator access.
    pub(crate) access_mode: AccessModeV1,
    /// Exactly one Role for participants; `null` otherwise.
    pub(crate) role: Option<String>,
}

impl MembershipV1 {
    /// Constructs one host-shape-valid immutable Membership value.
    ///
    /// # Errors
    ///
    /// Returns an error unless participants have exactly one Role and
    /// nonparticipants have none.
    pub fn new(
        member_id: MemberId,
        principal_id: PrincipalId,
        principal_kind: PrincipalKindV1,
        standing: MembershipStandingV1,
        access_mode: AccessModeV1,
        role: Option<String>,
    ) -> Result<Self, CoreShapeErrorV1> {
        match (access_mode, role.is_some()) {
            (AccessModeV1::Participant, true)
            | (AccessModeV1::Spectator | AccessModeV1::Operator, false) => Ok(Self {
                member_id,
                principal_id,
                principal_kind,
                standing,
                access_mode,
                role,
            }),
            (AccessModeV1::Participant, false) => Err(CoreShapeErrorV1::ParticipantMissingRole),
            (AccessModeV1::Spectator | AccessModeV1::Operator, true) => {
                Err(CoreShapeErrorV1::NonParticipantHasRole)
            }
        }
    }

    /// Returns the immutable Member ID.
    #[must_use]
    pub fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    /// Returns the immutable Principal ID.
    #[must_use]
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    /// Returns the immutable room-local Principal kind.
    #[must_use]
    pub const fn principal_kind(&self) -> PrincipalKindV1 {
        self.principal_kind
    }

    /// Returns current standing.
    #[must_use]
    pub const fn standing(&self) -> MembershipStandingV1 {
        self.standing
    }

    /// Returns current access mode.
    #[must_use]
    pub const fn access_mode(&self) -> AccessModeV1 {
        self.access_mode
    }

    /// Returns the participant Role, or `None` for spectator/operator.
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    fn with_standing(&self, standing: MembershipStandingV1) -> Self {
        let mut result = self.clone();
        result.standing = standing;
        result
    }

    fn with_access(
        &self,
        access_mode: AccessModeV1,
        role: Option<String>,
    ) -> Result<Self, CoreShapeErrorV1> {
        Self::new(
            self.member_id.clone(),
            self.principal_id.clone(),
            self.principal_kind,
            self.standing,
            access_mode,
            role,
        )
    }

    fn with_role(&self, role: String) -> Result<Self, CoreShapeErrorV1> {
        self.with_access(self.access_mode, Some(role))
    }
}

impl<'de> Deserialize<'de> for MembershipV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            member_id: MemberId,
            principal_id: PrincipalId,
            principal_kind: PrincipalKindV1,
            standing: MembershipStandingV1,
            access_mode: AccessModeV1,
            role: Option<String>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(
            raw.member_id,
            raw.principal_id,
            raw.principal_kind,
            raw.standing,
            raw.access_mode,
            raw.role,
        )
        .map_err(de::Error::custom)
    }
}

/// Principal kind is independent from Role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKindV1 {
    /// A human Principal.
    Human,
    /// An agent Principal.
    Agent,
}

/// Canonical Membership standing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStandingV1 {
    /// Eligible to attach and, for participants, act.
    Enabled,
    /// Retained but unable to attach, act, receive, or activate.
    Suspended,
    /// Terminal Membership history.
    Departed,
}

/// Canonical Membership access mode.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessModeV1 {
    /// Acting pack participant; requires a Role.
    Participant,
    /// Nonacting spectator; forbids a Role.
    Spectator,
    /// Nonacting operator; forbids a Role.
    Operator,
}

/// An optional active-to-archived Room status change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomStatusChangeV1 {
    /// Observed status.
    pub(crate) before: RoomStatusV1,
    /// Desired status.
    pub(crate) after: RoomStatusV1,
}

/// A canonical typed before/after Membership component.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipChangeV1 {
    /// Component semantics.
    pub(crate) kind: MembershipChangeKindV1,
    /// Affected immutable identity.
    pub(crate) member_id: MemberId,
    /// Exact observed value; `null` only for Join.
    pub(crate) before: Option<MembershipV1>,
    /// Exact desired final value.
    pub(crate) after: MembershipV1,
}

impl MembershipChangeV1 {
    /// Constructs a Join with a never-before-used Member ID.
    #[must_use]
    pub fn join(after: MembershipV1) -> Self {
        Self {
            kind: MembershipChangeKindV1::Join,
            member_id: after.member_id.clone(),
            before: None,
            after,
        }
    }

    /// Constructs a desired Resume; an already-enabled value normalizes to
    /// `NoChange` at the tracer seam.
    #[must_use]
    pub fn resume(before: MembershipV1) -> Self {
        let after = before.with_standing(MembershipStandingV1::Enabled);
        Self::from_before_after(MembershipChangeKindV1::Resume, before, after)
    }

    /// Constructs a desired Access Mode/Role pair atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested Access Mode/Role shape is invalid.
    pub fn access_mode_change(
        before: MembershipV1,
        access_mode: AccessModeV1,
        role: Option<String>,
    ) -> Result<Self, CoreShapeErrorV1> {
        let after = before.with_access(access_mode, role)?;
        Ok(Self::from_before_after(
            MembershipChangeKindV1::AccessModeChange,
            before,
            after,
        ))
    }

    /// Constructs a desired participant Role; the existing Role normalizes to
    /// `NoChange` at the tracer seam.
    ///
    /// # Errors
    ///
    /// Returns an error unless the Membership is a participant with a Role.
    pub fn role_change(
        before: MembershipV1,
        role: impl Into<String>,
    ) -> Result<Self, CoreShapeErrorV1> {
        let after = before.with_role(role.into())?;
        Ok(Self::from_before_after(
            MembershipChangeKindV1::RoleChange,
            before,
            after,
        ))
    }

    /// Constructs a desired suspension.
    #[must_use]
    pub fn suspend(before: MembershipV1) -> Self {
        let after = before.with_standing(MembershipStandingV1::Suspended);
        Self::from_before_after(MembershipChangeKindV1::Suspend, before, after)
    }

    /// Constructs a desired terminal departure.
    #[must_use]
    pub fn depart(before: MembershipV1) -> Self {
        let after = before.with_standing(MembershipStandingV1::Departed);
        Self::from_before_after(MembershipChangeKindV1::Depart, before, after)
    }

    fn from_before_after(
        kind: MembershipChangeKindV1,
        before: MembershipV1,
        after: MembershipV1,
    ) -> Self {
        Self {
            kind,
            member_id: before.member_id.clone(),
            before: Some(before),
            after,
        }
    }

    /// Returns the typed component kind.
    #[must_use]
    pub const fn kind(&self) -> MembershipChangeKindV1 {
        self.kind
    }

    /// Returns the affected immutable Member ID.
    #[must_use]
    pub fn member_id(&self) -> &MemberId {
        &self.member_id
    }

    /// Returns the exact before value, absent only for Join.
    #[must_use]
    pub fn before(&self) -> Option<&MembershipV1> {
        self.before.as_ref()
    }

    /// Returns the desired final value.
    #[must_use]
    pub fn after(&self) -> &MembershipV1 {
        &self.after
    }
}

/// The six legal Membership component kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipChangeKindV1 {
    /// Add a new immutable Membership ID.
    Join,
    /// Move suspended to enabled.
    Resume,
    /// Atomically replace Access Mode and its Role shape.
    AccessModeChange,
    /// Replace one participant Role.
    RoleChange,
    /// Move enabled to suspended.
    Suspend,
    /// Move enabled or suspended to terminal departed.
    Depart,
}

impl MembershipChangeKindV1 {
    pub(crate) const fn veto_class(self) -> CoreProposalClassV1 {
        match self {
            Self::Join | Self::Resume | Self::AccessModeChange | Self::RoleChange => {
                CoreProposalClassV1::Vetoable
            }
            Self::Suspend | Self::Depart => CoreProposalClassV1::Mandatory,
        }
    }
}

/// The exact canonical Core before/after changeset shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreChangeSetV1 {
    /// Present only for top-level Archive.
    pub(crate) room_status_change: Option<RoomStatusChangeV1>,
    /// Strictly Member-ID-sorted components with no repeated identity.
    pub(crate) membership_changes: Vec<MembershipChangeV1>,
}

impl CoreChangeSetV1 {
    /// Constructs one individual Membership component changeset.
    #[must_use]
    pub fn one(change: MembershipChangeV1) -> Self {
        Self {
            room_status_change: None,
            membership_changes: vec![change],
        }
    }

    /// Constructs a sorted atomic multi-Membership final-state changeset.
    ///
    /// # Errors
    ///
    /// Returns an error unless at least two distinct Member IDs are present.
    pub fn atomic(mut changes: Vec<MembershipChangeV1>) -> Result<Self, CoreShapeErrorV1> {
        changes.sort_by(|left, right| left.member_id.cmp(&right.member_id));
        if changes.len() < 2 {
            return Err(CoreShapeErrorV1::AtomicChangesetTooSmall);
        }
        if let Some(duplicate) = changes
            .windows(2)
            .find(|pair| pair[0].member_id == pair[1].member_id)
        {
            return Err(CoreShapeErrorV1::DuplicateMemberId(
                duplicate[0].member_id.clone(),
            ));
        }
        Ok(Self {
            room_status_change: None,
            membership_changes: changes,
        })
    }

    /// Constructs active-to-archived, or archived-to-archived for an
    /// idempotent administrative `NoChange`.
    #[must_use]
    pub fn archive(current: RoomStatusV1) -> Self {
        Self {
            room_status_change: Some(RoomStatusChangeV1 {
                before: current,
                after: RoomStatusV1::Archived,
            }),
            membership_changes: Vec::new(),
        }
    }

    /// Returns the optional Room Status pair.
    #[must_use]
    pub fn room_status_change(&self) -> Option<(RoomStatusV1, RoomStatusV1)> {
        self.room_status_change
            .as_ref()
            .map(|change| (change.before, change.after))
    }

    /// Returns the strictly sorted Membership components.
    #[must_use]
    pub fn membership_changes(&self) -> &[MembershipChangeV1] {
        &self.membership_changes
    }
}

/// The eight normalized Core proposal kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreProposedKindV1 {
    /// One Join component.
    Join,
    /// One Resume component.
    Resume,
    /// One Access Mode component.
    AccessModeChange,
    /// One Role component.
    RoleChange,
    /// Multiple atomic homogeneous-class Membership components.
    MembershipChangeSet,
    /// The active-to-archived status transition.
    Archive,
    /// One Suspend component.
    Suspend,
    /// One Depart component.
    Depart,
}

impl CoreProposedKindV1 {
    pub(crate) const fn component_kind(self) -> Option<MembershipChangeKindV1> {
        match self {
            Self::Join => Some(MembershipChangeKindV1::Join),
            Self::Resume => Some(MembershipChangeKindV1::Resume),
            Self::AccessModeChange => Some(MembershipChangeKindV1::AccessModeChange),
            Self::RoleChange => Some(MembershipChangeKindV1::RoleChange),
            Self::Suspend => Some(MembershipChangeKindV1::Suspend),
            Self::Depart => Some(MembershipChangeKindV1::Depart),
            Self::MembershipChangeSet | Self::Archive => None,
        }
    }
}

/// Stable attributable authority without bearer material.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreAuthorityAttributionV1 {
    /// Principal responsible for the proposal.
    pub principal_id: PrincipalId,
    /// Whether authority was Room-local or host-operator administration.
    pub authority_kind: CoreAuthorityKindV1,
}

/// Canonical authority attribution kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreAuthorityKindV1 {
    /// Room-local administration.
    RoomAdministrator,
    /// Host-operator administration.
    HostOperator,
}

/// Existing-Room administration idempotency identity.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdministrationOperationIdentityV1 {
    /// Currently authenticated Principal.
    pub authenticated_principal: PrincipalId,
    /// Versioned operation kind, e.g. `worldstream/core-proposed/v1`.
    pub versioned_operation_kind: String,
    /// Caller-controlled idempotency key.
    pub idempotency_key: String,
}

/// Participant Action stimulus fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantActionV1 {
    /// Acting Membership.
    pub member_id: MemberId,
    /// Caller-generated idempotency identity.
    pub action_id: ActionId,
    /// Exact pack-defined Action type.
    pub action_type: String,
    /// Exact payload schema content digest.
    pub payload_schema_digest: Blake3DigestV1,
    /// Strict canonical pack payload.
    pub canonical_payload: CanonicalJsonV1,
    /// Indivisible observed Room Head.
    pub exact_basis_head: CompleteHeadV1,
    /// Host-recorded lane admission time.
    pub admitted_at: ActionAdmittedAt,
}

/// Exact scheduled generation stimulus. It deliberately has no `fired_at`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFiredV1 {
    /// Logical Timer ID.
    pub timer_id: TimerId,
    /// Exact immutable generation.
    pub generation: TimerGenerationV1,
    /// Generation's immutable scheduled time and effective semantic time.
    pub scheduled_for: TimerScheduledFor,
    /// Generation's immutable payload.
    pub canonical_payload: CanonicalJsonV1,
}

/// Normalized Core administration stimulus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreProposedV1 {
    /// Proposal kind and changeset-shape discriminator.
    pub(crate) kind: CoreProposedKindV1,
    /// Attribution without operational authorization witness.
    pub(crate) canonical_authority_attribution: CoreAuthorityAttributionV1,
    /// Existing-Room administrative operation identity.
    pub(crate) operation_identity: AdministrationOperationIdentityV1,
    /// Exact expected Room sequence.
    pub(crate) expected_room_seq: RoomSequenceV1,
    /// Stable machine reason.
    pub(crate) reason_code: String,
    /// Stimulus-specific administration time.
    pub(crate) recorded_at: CoreRecordedAt,
    /// Unambiguous Core before/after changes.
    pub(crate) canonical_changeset: CoreChangeSetV1,
}

impl CoreProposedV1 {
    /// Constructs a Core administrative command candidate. The tracer checks
    /// its exact kind/changeset semantics and authority identity.
    #[must_use]
    pub fn new(
        kind: CoreProposedKindV1,
        canonical_authority_attribution: CoreAuthorityAttributionV1,
        operation_identity: AdministrationOperationIdentityV1,
        expected_room_seq: RoomSequenceV1,
        reason_code: impl Into<String>,
        recorded_at: CoreRecordedAt,
        canonical_changeset: CoreChangeSetV1,
    ) -> Self {
        Self {
            kind,
            canonical_authority_attribution,
            operation_identity,
            expected_room_seq,
            reason_code: reason_code.into(),
            recorded_at,
            canonical_changeset,
        }
    }

    /// Returns the proposal kind.
    #[must_use]
    pub const fn kind(&self) -> CoreProposedKindV1 {
        self.kind
    }

    /// Returns the exact expected Room sequence.
    #[must_use]
    pub const fn expected_room_seq(&self) -> RoomSequenceV1 {
        self.expected_room_seq
    }

    /// Returns the canonical changeset.
    #[must_use]
    pub fn changeset(&self) -> &CoreChangeSetV1 {
        &self.canonical_changeset
    }

    /// Returns stable canonical authority attribution recorded in the
    /// Stimulus (but excluded from the caller-semantic request hash).
    #[must_use]
    pub fn authority_attribution(&self) -> &CoreAuthorityAttributionV1 {
        &self.canonical_authority_attribution
    }

    /// Returns the administration Operation Identity.
    #[must_use]
    pub fn operation_identity(&self) -> &AdministrationOperationIdentityV1 {
        &self.operation_identity
    }

    /// Returns the stable semantic reason code.
    #[must_use]
    pub fn reason_code(&self) -> &str {
        &self.reason_code
    }

    /// Returns the stimulus-specific recorded administration time.
    #[must_use]
    pub fn recorded_at(&self) -> &CoreRecordedAt {
        &self.recorded_at
    }
}

/// Predefined immutable external input stimulus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalInputV1 {
    /// Predefined source identity.
    pub source_id: SourceId,
    /// Source-scoped immutable input identity.
    pub input_id: InputId,
    /// Declared versioned input kind.
    pub input_type: String,
    /// Stimulus-specific recorded time.
    pub recorded_at: ExternalInputRecordedAt,
    /// Strict canonical payload.
    pub canonical_payload: CanonicalJsonV1,
    /// Already-authorized immutable references only.
    pub immutable_resource_references: Vec<CanonicalJsonV1>,
}

/// The exact four normalized recorded Stimulus variants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "stimulus_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordedStimulusV1 {
    /// A host-admitted participant Action.
    ParticipantAction(ParticipantActionV1),
    /// An exact scheduled Timer generation.
    TimerFired(TimerFiredV1),
    /// Canonical Core administration.
    CoreProposed(CoreProposedV1),
    /// A predefined required external input.
    ExternalInput(ExternalInputV1),
}

impl RecordedStimulusV1 {
    /// Returns the variant-specific semantic time: `admitted_at`,
    /// `scheduled_for`, or the declared `recorded_at`.
    #[must_use]
    pub fn semantic_time(&self) -> &str {
        match self {
            Self::ParticipantAction(action) => action.admitted_at.as_str(),
            Self::TimerFired(timer) => timer.scheduled_for.as_str(),
            Self::CoreProposed(core) => core.recorded_at.as_str(),
            Self::ExternalInput(input) => input.recorded_at.as_str(),
        }
    }
}

/// A generation installed at Genesis or currently scheduled in memory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledTimerV1 {
    /// Logical Timer ID.
    pub timer_id: TimerId,
    /// Host-owned immutable generation.
    pub generation: TimerGenerationV1,
    /// Immutable semantic due time.
    pub scheduled_for: TimerScheduledFor,
    /// Immutable canonical payload.
    pub canonical_payload: CanonicalJsonV1,
}

/// A normalized host-assigned Timer mutation bound by a Transition hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "change_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimerChangeV1 {
    /// Install a generation when none is currently scheduled.
    Schedule {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Newly allocated generation.
        generation: TimerGenerationV1,
        /// Immutable due time.
        scheduled_for: TimerScheduledFor,
        /// Immutable payload.
        canonical_payload: CanonicalJsonV1,
    },
    /// Cancel an exact current generation.
    Cancel {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Exact generation consumed.
        generation: TimerGenerationV1,
    },
    /// Atomically consume the current generation and install its successor.
    Reschedule {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Exact generation consumed.
        previous_generation: TimerGenerationV1,
        /// Newly allocated generation.
        generation: TimerGenerationV1,
        /// Immutable successor due time.
        scheduled_for: TimerScheduledFor,
        /// Immutable successor payload.
        canonical_payload: CanonicalJsonV1,
    },
}

/// Generation-free Timer intent emitted by the pure Activity reducer. Core
/// owns generation allocation and converts requests to hashed
/// [`TimerChangeV1`] values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimerRequestV1 {
    /// Schedule the next generation when no generation is currently pending.
    ScheduleNext {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Requested immutable due time.
        due: TimerScheduledFor,
        /// Requested immutable canonical payload.
        canonical_payload: CanonicalJsonV1,
    },
    /// Cancel the currently pending generation.
    CancelCurrent {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Exact current host-owned generation witness.
        expected_generation: TimerGenerationV1,
    },
    /// Consume the current generation and schedule its successor atomically.
    RescheduleCurrent {
        /// Logical Timer ID.
        timer_id: TimerId,
        /// Exact current host-owned generation witness.
        expected_generation: TimerGenerationV1,
        /// Requested immutable successor due time.
        new_due: TimerScheduledFor,
        /// Requested immutable successor canonical payload.
        new_canonical_payload: CanonicalJsonV1,
    },
}

impl TimerRequestV1 {
    pub(crate) fn timer_id(&self) -> &TimerId {
        match self {
            Self::ScheduleNext { timer_id, .. }
            | Self::CancelCurrent { timer_id, .. }
            | Self::RescheduleCurrent { timer_id, .. } => timer_id,
        }
    }
}

/// Pure Activity output before host normalization of Timer requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityApplyV1 {
    /// Complete next canonical Activity State.
    pub next_activity_state: CanonicalJsonV1,
    /// Domain Events in exact pack order.
    pub ordered_domain_events: Vec<CanonicalJsonV1>,
    /// Generation-free Timer intents. Core canonicalizes by Timer ID and
    /// allocates every generation.
    pub timer_requests: Vec<TimerRequestV1>,
    /// Attention Signals in exact deterministic pack order.
    pub ordered_attention_signals: Vec<CanonicalJsonV1>,
}

/// A clean declared pack veto.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityRejectionV1 {
    /// Stable declared code from the pinned pack descriptor.
    pub declared_code: String,
    /// Bounded safe canonical detail.
    pub bounded_safe_details: CanonicalJsonV1,
}

/// The only two semantic reduction dispositions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityDispositionV1 {
    /// Apply complete output and commit one Transition.
    Apply(ActivityApplyV1),
    /// Declare a clean veto where the stimulus class allows one.
    Reject(ActivityRejectionV1),
}

/// Input passed immutably to the caller-supplied pure Activity reducer closure.
#[derive(Clone, Debug)]
pub struct ActivityReduceInputV1<'a> {
    /// Prior canonical Activity State.
    pub prior_activity_state: &'a CanonicalJsonV1,
    /// Exact immutable Core before.
    pub core_before: &'a CoreRoomStateV1,
    /// Exact immutable proposed Core after.
    pub proposed_core_after: &'a CoreRoomStateV1,
    /// Current scheduled Timer view sorted by Timer ID.
    pub scheduled_timers: &'a BTreeMap<TimerId, ScheduledTimerV1>,
    /// Sequence this accepted Stimulus would consume.
    pub next_room_seq: RoomSequenceV1,
    /// Exact normalized Stimulus.
    pub recorded_stimulus: &'a RecordedStimulusV1,
}

/// The exact indivisible eight-field Room Head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteHeadV1 {
    /// Room identity.
    pub(crate) room_id: RoomId,
    /// Current committed sequence.
    pub(crate) room_seq: RoomSequenceV1,
    /// Genesis hash at zero, otherwise current Transition hash.
    pub(crate) genesis_or_transition_hash: Blake3DigestV1,
    /// Fixed Core schema identity.
    pub(crate) core_schema_version: String,
    /// Exact immutable pack digest.
    pub(crate) pack_digest: PackDigestV1,
    /// Current canonical Core State hash.
    pub(crate) core_state_hash: Blake3DigestV1,
    /// Current canonical Activity State hash.
    pub(crate) activity_state_hash: Blake3DigestV1,
    /// Current aggregate Authoritative State hash.
    pub(crate) authoritative_state_hash: Blake3DigestV1,
}

impl CompleteHeadV1 {
    /// Returns the Room identity.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    /// Returns the current Room sequence.
    #[must_use]
    pub const fn room_seq(&self) -> RoomSequenceV1 {
        self.room_seq
    }

    /// Returns Genesis hash at zero or current Transition hash otherwise.
    #[must_use]
    pub fn genesis_or_transition_hash(&self) -> &Blake3DigestV1 {
        &self.genesis_or_transition_hash
    }

    /// Returns the exact Core schema identity.
    #[must_use]
    pub fn core_schema_version(&self) -> &str {
        &self.core_schema_version
    }

    /// Returns the exact pack revision digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        &self.pack_digest
    }

    /// Returns the Core State hash.
    #[must_use]
    pub fn core_state_hash(&self) -> &Blake3DigestV1 {
        &self.core_state_hash
    }

    /// Returns the Activity State hash.
    #[must_use]
    pub fn activity_state_hash(&self) -> &Blake3DigestV1 {
        &self.activity_state_hash
    }

    /// Returns the aggregate Authoritative State hash.
    #[must_use]
    pub fn authoritative_state_hash(&self) -> &Blake3DigestV1 {
        &self.authoritative_state_hash
    }
}

/// Operational integrity state. This type is never accepted by any canonical
/// hash function and is not part of Core or replay state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomIntegrityStateV1 {
    /// Current operational classification.
    status: RoomIntegrityStatusV1,
    /// Monotonic operational generation.
    generation: IntegrityGenerationV1,
}

impl RoomIntegrityStateV1 {
    /// Constructs operational integrity metadata. It never enters lineage.
    #[must_use]
    pub const fn new(status: RoomIntegrityStatusV1, generation: IntegrityGenerationV1) -> Self {
        Self { status, generation }
    }

    /// Returns the operational status.
    #[must_use]
    pub const fn status(&self) -> RoomIntegrityStatusV1 {
        self.status
    }

    /// Returns the operational generation.
    #[must_use]
    pub const fn generation(&self) -> IntegrityGenerationV1 {
        self.generation
    }
}

/// A separate append-only operational incident/repair audit record. Neither
/// this record nor `RoomIntegrityStateV1` is accepted by lineage hashing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomIntegrityAuditRecordV1 {
    /// Generation at which the operational record was appended.
    pub integrity_generation: IntegrityGenerationV1,
    /// Versioned bounded operational detail.
    pub record: CanonicalJsonV1,
}

/// Operational integrity classification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomIntegrityStatusV1 {
    /// Lineage verifies and may advance.
    Healthy,
    /// Last Head verifies but the Room cannot safely advance.
    Faulted,
    /// Canonical integrity cannot be established.
    Quarantined,
}

/// Checked Genesis creation input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisInputV1 {
    /// Generated Room identity.
    pub(crate) room_id: RoomId,
    /// Exact immutable pack digest.
    pub(crate) pack_digest: PackDigestV1,
    /// Canonical pack/Room configuration.
    pub(crate) configuration: CanonicalJsonV1,
    /// Recorded nondeterministic Room seed.
    pub(crate) room_seed: RoomSeedV1,
    /// Logical creation time.
    pub(crate) created_at: crate::CreationRecordedAt,
    /// Exact initial Core value.
    pub(crate) initial_core_state: CoreRoomStateV1,
    /// Exact initial opaque Activity value.
    pub(crate) initial_activity_state: CanonicalJsonV1,
    /// Normalized initial schedules, strictly sorted by Timer ID.
    pub(crate) initial_timers: Vec<ScheduledTimerV1>,
}

impl GenesisInputV1 {
    /// Constructs a Genesis input candidate. The tracer validates the injected
    /// pack Role/cardinality rule, state hashes, and Timer normalization.
    #[must_use]
    pub fn new(
        room_id: RoomId,
        pack_digest: PackDigestV1,
        configuration: CanonicalJsonV1,
        room_seed: RoomSeedV1,
        created_at: crate::CreationRecordedAt,
        initial_core_state: CoreRoomStateV1,
        initial_activity_state: CanonicalJsonV1,
    ) -> Self {
        Self {
            room_id,
            pack_digest,
            configuration,
            room_seed,
            created_at,
            initial_core_state,
            initial_activity_state,
            initial_timers: Vec::new(),
        }
    }

    /// Adds the normalized initial Timer candidates to this creation input.
    #[must_use]
    pub fn with_initial_timers(mut self, initial_timers: Vec<ScheduledTimerV1>) -> Self {
        self.initial_timers = initial_timers;
        self
    }
}

/// Why an externally constructed Core candidate cannot have the exact host
/// shape even before pack Role/cardinality validation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CoreShapeErrorV1 {
    /// Participant requires one Role.
    #[error("participant Membership requires a Role")]
    ParticipantMissingRole,
    /// Spectator/operator forbid Role.
    #[error("spectator/operator Membership forbids a Role")]
    NonParticipantHasRole,
    /// Initial map keys must be unique.
    #[error("duplicate Member ID {0}")]
    DuplicateMemberId(MemberId),
    /// Decoded map key disagreed with embedded identity.
    #[error("Membership map key differs from embedded Member ID")]
    MemberMapKeyMismatch,
    /// Initial Core cannot have two live Memberships for one Principal.
    #[error("Principal {0} has more than one non-departed Membership")]
    DuplicateLivePrincipal(PrincipalId),
    /// The atomic changeset form represents multiple Memberships.
    #[error("atomic Membership changeset requires at least two components")]
    AtomicChangesetTooSmall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreProposalClassV1 {
    Vetoable,
    Mandatory,
}
