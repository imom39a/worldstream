use std::{
    any::TypeId,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    str::FromStr,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    AccessModeV1, ActionAdmittedAt, ActionId, ActivityDispositionV1, ActivityReduceInputV1,
    Blake3DigestV1, CANONICAL_CODEC_ID, CanonicalJsonError, CanonicalJsonV1, CompleteHeadV1,
    CoreRoomStateV1, CoreTraceV1, CreationRecordedAt, ExternalInputV1, GenesisInputV1, MemberId,
    MembershipChangeKindV1, MembershipStandingV1, PackDigestV1, PackFaultV1, ParticipantActionV1,
    PrincipalKindV1, RecordedStimulusV1, RoomId, RoomSeedV1, RoomSequenceV1, ScheduledTimerV1,
    TimerGenerationV1, TimerRequestV1, TimestampParseError,
    canonical::encode,
    lineage::{hash_activity_state, hash_authoritative_state, hash_core_state},
    primitives::compare_timestamp_text,
};

/// Frozen trusted Activity Pack host-contract identity.
pub const ACTIVITY_PACK_HOST_CONTRACT_ID: &str = "worldstream/activity-pack/v1";
/// Canonical JSON interface carried by the five WASI-free Component exports.
pub const ACTIVITY_PACK_OPERATION_CODEC_ID: &str = "worldstream/activity-pack-operation-codec/v1";
/// Frozen semantic revision-lock identity.
pub const PACK_REVISION_LOCK_ID: &str = "worldstream/pack-revision-lock/v1";
/// Frozen Action Offer identity.
pub const ACTION_OFFER_DOMAIN: &str = "worldstream/action-offer/v1";
/// Frozen complete Projection schema identity.
pub const PROJECTION_SCHEMA_V1: &str = "worldstream.projection.v1";
/// Frozen domain used for Projection hashes.
pub const PROJECTION_HASH_DOMAIN_V1: &str = "worldstream/projection-hash/v1";

const SCHEMA_BUNDLE_DOMAIN: &str = "worldstream/pack-schema-bundle/v1";
const CODEC_BUNDLE_DOMAIN: &str = "worldstream/pack-codec-bundle/v1";
const PACK_GOLDEN_CORPUS_DOMAIN: &str = "worldstream/pack-golden-corpus/v1";
const PACK_GOLDEN_TRANSCRIPT_DOMAIN: &str = "worldstream/pack-golden-transcript/v1";
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

/// One of the frozen five Activity Pack operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityPackOperationV1 {
    Descriptor,
    Initialize,
    Reduce,
    View,
    Observe,
}

/// The complete trusted semantic seam. Implementations are compiled in and
/// selected only through [`PackRegistryV1`].
pub trait ActivityPackV1: Send + Sync + 'static {
    /// Returns immutable metadata for this exact semantic revision.
    fn descriptor(&self) -> &PackRevisionDescriptorV1;

    /// Produces initial Activity State and generation-free Timer requests.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed pack fault; no Genesis is prepared.
    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1>;

    /// Reduces one normalized Stimulus against immutable Core views.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed pack fault; no Transition is prepared.
    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1>;

    /// Constructs one authorized Activity Projection and its sole Action
    /// Offer representation.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed pack fault; no projection is exposed.
    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1>;

    /// Constructs zero or one authorized viewer observation.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed pack fault; no observation is exposed.
    fn observe(&self, input: &ObserveInputV1<'_>)
    -> Result<Option<PackObservationV1>, PackFaultV1>;
}

/// Exact recorded creation facts visible to `initialize`.
#[derive(Clone, Copy, Debug)]
pub struct ActivityGenesisInputV1<'a> {
    /// Generated Room identity.
    pub room_id: &'a RoomId,
    /// Recomputed exact pack revision digest.
    pub pack_digest: &'a PackDigestV1,
    /// Canonical pack configuration.
    pub configuration: &'a CanonicalJsonV1,
    /// Immutable initial Core value.
    pub initial_core_state: &'a CoreRoomStateV1,
    /// Recorded Room seed.
    pub room_seed: &'a RoomSeedV1,
    /// Logical creation time.
    pub created_at: &'a CreationRecordedAt,
}

/// Owned recorded creation input presented to the registry before Genesis
/// preparation. It deliberately omits Activity State and Timer generations;
/// those must come from the selected exact executor and host normalization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackGenesisRequestV1 {
    pub room_id: RoomId,
    pub pack_digest: PackDigestV1,
    pub configuration: CanonicalJsonV1,
    pub room_seed: RoomSeedV1,
    pub created_at: CreationRecordedAt,
    pub initial_core_state: CoreRoomStateV1,
}

/// Opaque checked creation token obtainable only through a registry row that
/// is selectable for new Rooms.
pub struct PreparedNewRoomGenesisV1 {
    genesis_input: GenesisInputV1,
    retained_pack: RetainedActivityPackV1,
}

impl PreparedNewRoomGenesisV1 {
    /// Returns the complete checked Core Genesis input.
    #[must_use]
    pub fn genesis_input(&self) -> &GenesisInputV1 {
        &self.genesis_input
    }

    /// Returns the exact retained pack binding that must own subsequent Room
    /// execution.
    #[must_use]
    pub fn retained_pack(&self) -> &RetainedActivityPackV1 {
        &self.retained_pack
    }
}

/// Opaque retained-Genesis verification result for recovery/replay. It cannot
/// be converted into the new-Room creation token.
pub(crate) struct VerifiedRetainedGenesisV1 {
    #[allow(dead_code)]
    genesis_input: GenesisInputV1,
    retained_pack: RetainedActivityPackV1,
}

impl VerifiedRetainedGenesisV1 {
    /// Returns the exact runnable retained revision used for verification.
    #[must_use]
    pub(crate) fn retained_pack(&self) -> &RetainedActivityPackV1 {
        &self.retained_pack
    }

    #[allow(dead_code)]
    pub(crate) fn genesis_input(&self) -> &GenesisInputV1 {
        &self.genesis_input
    }
}

/// The only output admitted from `initialize`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialOutputV1 {
    /// Complete initial canonical Activity State.
    pub initial_activity_state: CanonicalJsonV1,
    /// Ordered generation-free Timer requests.
    pub timer_requests: Vec<TimerRequestV1>,
}

/// Complete before/after facts used by the checked observation host.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ObserveTransitionInputV1<'a> {
    pub(crate) core_before: &'a CoreRoomStateV1,
    pub(crate) activity_before: &'a CanonicalJsonV1,
    pub(crate) head_before: &'a CompleteHeadV1,
    pub(crate) core_after: &'a CoreRoomStateV1,
    pub(crate) activity_after: &'a CanonicalJsonV1,
    pub(crate) head_after: &'a CompleteHeadV1,
    pub(crate) recorded_stimulus: &'a RecordedStimulusV1,
    pub(crate) ordered_domain_events: &'a [CanonicalJsonV1],
    pub(crate) viewer: &'a PackViewerV1,
}

/// Host-validated optional observation and its canonical bytes.
#[derive(Clone, Debug)]
pub struct ValidatedPackObservationV1 {
    observation_schema: String,
    observation: CanonicalJsonV1,
    action_offers: Option<CanonicalActionOffersV1>,
    canonical_bytes: Arc<[u8]>,
}

/// Host-side result of checked observation construction. Projection resets and
/// visibility loss are delivery classifications, not Activity Pack faults.
#[derive(Clone, Debug)]
pub enum ActivityObservationOutcomeV1 {
    /// Complete authorized before/after bytes were identical and the pack
    /// emitted no observation.
    Hidden,
    /// One checked, bounded Membership observation.
    Observation(ValidatedPackObservationV1),
    /// Access/viewer class changed; the host must construct a reset under the
    /// new typed viewer rather than invoking `observe` across classes.
    ProjectionReset(Box<ValidatedPackViewV1>),
    /// The Membership is absent, suspended, or departed after the transition;
    /// delivery closes without treating visibility removal as a pack fault.
    VisibilityLost,
}

impl ValidatedPackObservationV1 {
    #[must_use]
    pub fn observation_schema(&self) -> &str {
        &self.observation_schema
    }

    #[must_use]
    pub fn observation(&self) -> &CanonicalJsonV1 {
        &self.observation
    }

    #[must_use]
    pub fn action_offers(&self) -> Option<&CanonicalActionOffersV1> {
        self.action_offers.as_ref()
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
}

/// Deterministic helpers bound to one Room, revision, and next sequence.
#[derive(Clone, Copy, Debug)]
pub struct DeterministicContextV1<'a> {
    room_seed: &'a RoomSeedV1,
    pack_digest: &'a PackDigestV1,
    next_room_sequence: RoomSequenceV1,
}

impl<'a> DeterministicContextV1<'a> {
    /// Binds helpers to the complete deterministic derivation identity.
    #[must_use]
    pub const fn new(
        room_seed: &'a RoomSeedV1,
        pack_digest: &'a PackDigestV1,
        next_room_sequence: RoomSequenceV1,
    ) -> Self {
        Self {
            room_seed,
            pack_digest,
            next_room_sequence,
        }
    }

    /// Selects an index with domain-separated deterministic rejection
    /// sampling. No ambient entropy is consulted.
    ///
    /// # Errors
    ///
    /// Returns an error when `bound` is zero or outside the canonical safe
    /// integer range.
    pub fn uniform_index(
        &self,
        label: &str,
        index: u64,
        bound: u64,
    ) -> Result<u64, DeterministicContextErrorV1> {
        if bound == 0 || bound > crate::MAX_SAFE_INTEGER as u64 {
            return Err(DeterministicContextErrorV1::InvalidBound);
        }
        let threshold = bound.wrapping_neg() % bound;
        for rejection_attempt in 0..=u64::MAX {
            let input = encode(&RandomInputV1 {
                domain: "worldstream/activity-random/v1",
                room_seed: self.room_seed,
                pack_digest: self.pack_digest,
                next_room_sequence: self.next_room_sequence,
                label,
                index,
                rejection_attempt,
            })?;
            let mut prefix = [0_u8; 8];
            prefix.copy_from_slice(&blake3::hash(&input).as_bytes()[..8]);
            let sample = u64::from_be_bytes(prefix);
            if sample >= threshold {
                return Ok(sample % bound);
            }
        }
        Err(DeterministicContextErrorV1::RejectionSamplingExhausted)
    }
}

#[derive(Serialize)]
struct RandomInputV1<'a> {
    domain: &'static str,
    room_seed: &'a RoomSeedV1,
    pack_digest: &'a PackDigestV1,
    next_room_sequence: RoomSequenceV1,
    label: &'a str,
    index: u64,
    rejection_attempt: u64,
}

/// Invalid deterministic-helper input.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DeterministicContextErrorV1 {
    /// The requested choice set is empty or not canonically representable.
    #[error("uniform-index bound must be in 1..=MAX_SAFE_INTEGER")]
    InvalidBound,
    /// The deterministic derivation exhausted every retry index.
    #[error("uniform-index rejection sampling exhausted")]
    RejectionSamplingExhausted,
    /// Canonical derivation input could not be encoded.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
}

/// One typed viewer. Historical and final-reveal authorization remain
/// explicit and never become an operator bypass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackViewerV1 {
    /// Explicit public projection materialized for a spectator Membership.
    Public(MemberId),
    /// Current participant-private projection.
    Participant(MemberId),
    /// Explicit bounded operator projection.
    Operator(MemberId),
    /// Membership-scoped view at a reconstructed sequence.
    Historical(MemberId),
    /// Separately authorized post-completion reveal.
    FinalReveal(MemberId),
}

/// Descriptor key for every host-addressable viewer class. Role-specific
/// projection differences remain values within the participant schema rather
/// than untyped string dispatch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackViewerClassV1 {
    Public,
    Participant,
    Operator,
    HistoricalPublic,
    HistoricalParticipant,
    HistoricalOperator,
    FinalReveal,
}

impl PackViewerClassV1 {
    const ALL: [Self; 7] = [
        Self::Public,
        Self::Participant,
        Self::Operator,
        Self::HistoricalPublic,
        Self::HistoricalParticipant,
        Self::HistoricalOperator,
        Self::FinalReveal,
    ];
}

impl PackViewerV1 {
    /// Returns the immutable Membership identity addressed by this view.
    #[must_use]
    pub fn member_id(&self) -> &MemberId {
        match self {
            Self::Public(member_id)
            | Self::Participant(member_id)
            | Self::Operator(member_id)
            | Self::Historical(member_id)
            | Self::FinalReveal(member_id) => member_id,
        }
    }
}

/// Exact input to `view`.
#[derive(Clone, Copy, Debug)]
pub struct ViewInputV1<'a> {
    /// Exact Core at this Head.
    pub core: &'a CoreRoomStateV1,
    /// Exact opaque Activity State at this Head.
    pub activity_state: &'a CanonicalJsonV1,
    /// Complete indivisible Head.
    pub complete_head: &'a CompleteHeadV1,
    /// Typed authorized viewer.
    pub viewer: &'a PackViewerV1,
}

/// Canonical UTC time used by an Action eligibility window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EligibilityTimeV1(ActionAdmittedAt);

impl EligibilityTimeV1 {
    /// Returns normalized UTC RFC 3339 text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for EligibilityTimeV1 {
    type Err = TimestampParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl Serialize for EligibilityTimeV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EligibilityTimeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Half-open Action eligibility window with canonical typed times.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityWindowV1 {
    /// Inclusive opening Semantic Time.
    pub opens_at: EligibilityTimeV1,
    /// Exclusive deadline Semantic Time.
    pub deadline: EligibilityTimeV1,
}

/// The sole canonical representation of current Action legality.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionOfferV1 {
    /// Always [`ACTION_OFFER_DOMAIN`].
    pub domain: String,
    /// Descriptor-declared Action type.
    pub action_type: String,
    /// Exact payload schema-content digest.
    pub payload_schema_digest: Blake3DigestV1,
    /// Optional recorded half-open eligibility window.
    pub eligibility_window: Option<EligibilityWindowV1>,
}

/// Ordinary participant Action admission failures. These are request
/// dispositions, not Activity Pack faults, and never invoke `reduce`.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ActionAdmissionErrorV1 {
    #[error("Action is not present in the exact current Action Offers: {0}")]
    ActionNotAllowed(String),
    #[error("Action basis is not the exact current checked Head")]
    StaleBasis,
    #[error("Action eligibility window has not opened")]
    NotOpen,
    #[error("Action was admitted at or after its exact deadline")]
    DeadlinePassed,
    #[error("Action payload is invalid: {0}")]
    InvalidPayload(String),
}

/// Checked reduction failure separates ordinary Action admission from pack
/// contract failure so malformed client work cannot fault a Room.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ActivityPackReduceErrorV1 {
    #[error(transparent)]
    Admission(#[from] ActionAdmissionErrorV1),
    #[error(transparent)]
    Pack(#[from] PackFaultV1),
}

/// Raw pack result from `view`; the host validates and canonicalizes it before
/// any surface can reuse it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackViewV1 {
    /// Descriptor-declared projection schema identity.
    pub projection_schema: String,
    /// Authorized pack-owned Activity Projection.
    pub projection: CanonicalJsonV1,
    /// Ordered Action Offers.
    pub action_offers: Vec<ActionOfferV1>,
}

/// One host-canonicalized Action Offer list. Clones retain the same byte
/// allocation so resets, observations, Invocation Context, and admission can
/// reuse it without independent serialization.
#[derive(Clone, Debug)]
pub struct CanonicalActionOffersV1 {
    offers: Arc<[ActionOfferV1]>,
    canonical_bytes: Arc<[u8]>,
}

impl CanonicalActionOffersV1 {
    /// Returns the ordered typed values backed by these exact bytes.
    #[must_use]
    pub fn offers(&self) -> &[ActionOfferV1] {
        &self.offers
    }

    /// Returns the sole canonical byte representation reused by every host
    /// surface.
    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.offers, &other.offers)
            && Arc::ptr_eq(&self.canonical_bytes, &other.canonical_bytes)
    }
}

/// Host-validated authorized pack view.
#[derive(Clone, Debug)]
pub struct ValidatedPackViewV1 {
    viewer: PackViewerV1,
    complete_head: CompleteHeadV1,
    projection_schema: String,
    projection: CanonicalJsonV1,
    action_offers: CanonicalActionOffersV1,
    canonical_bytes: Arc<[u8]>,
}

impl ValidatedPackViewV1 {
    /// Returns the exact typed viewer used to construct this checked value.
    #[must_use]
    pub fn viewer(&self) -> &PackViewerV1 {
        &self.viewer
    }

    /// Returns the indivisible Head against which this view was checked.
    #[must_use]
    pub fn complete_head(&self) -> &CompleteHeadV1 {
        &self.complete_head
    }

    #[must_use]
    pub fn projection_schema(&self) -> &str {
        &self.projection_schema
    }

    #[must_use]
    pub fn projection(&self) -> &CanonicalJsonV1 {
        &self.projection
    }

    #[must_use]
    pub fn action_offers(&self) -> &CanonicalActionOffersV1 {
        &self.action_offers
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Computes the frozen domain-separated hash of this complete Projection.
    ///
    /// The view bytes are the canonical `projection` value. Operational
    /// envelope fields, including Room and frame positions, are deliberately
    /// absent from this hash input.
    ///
    /// # Errors
    ///
    /// Returns the canonical JSON error if the validated view cannot be
    /// encoded as the frozen hash input.
    pub fn projection_hash(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        projection_hash_for_canonical_bytes(&self.canonical_bytes)
    }
}

#[derive(Serialize)]
struct ProjectionHashInputV1<'a> {
    domain: &'static str,
    projection_schema: &'static str,
    projection: &'a CanonicalJsonV1,
}

/// Computes the frozen Projection hash for already-canonical complete view
/// bytes. Adapters use this when validating durable reset evidence.
///
/// # Errors
///
/// Returns the canonical JSON error if the bytes are not canonical JSON or
/// the frozen hash input cannot be encoded.
pub fn projection_hash_for_canonical_bytes(
    canonical_bytes: &[u8],
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    let projection = CanonicalJsonV1::from_canonical_bytes(canonical_bytes)?;
    let hash_input = CanonicalJsonV1::from_serialize(&ProjectionHashInputV1 {
        domain: PROJECTION_HASH_DOMAIN_V1,
        projection_schema: PROJECTION_SCHEMA_V1,
        projection: &projection,
    })?;
    Ok(Blake3DigestV1::hash(&hash_input.to_bytes()?))
}

/// Exact input to `observe`.
#[derive(Clone, Copy, Debug)]
pub struct ObserveInputV1<'a> {
    /// Exact Core before the accepted Transition.
    pub core_before: &'a CoreRoomStateV1,
    /// Exact Activity State before the accepted Transition.
    pub activity_before: &'a CanonicalJsonV1,
    /// Exact Core after the accepted Transition.
    pub core_after: &'a CoreRoomStateV1,
    /// Exact Activity State after the accepted Transition.
    pub activity_after: &'a CanonicalJsonV1,
    /// Exact normalized causing Stimulus.
    pub recorded_stimulus: &'a RecordedStimulusV1,
    /// Ordered canonical Domain Events.
    pub ordered_domain_events: &'a [CanonicalJsonV1],
    /// Exact Membership-scoped viewer.
    pub viewer: &'a PackViewerV1,
    /// Host-validated exact after-view, including the one shared Action Offer
    /// byte container.
    pub after_view: &'a ValidatedPackViewV1,
}

/// Raw zero-or-one result from `observe`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackObservationV1 {
    /// Descriptor-declared observation schema identity.
    pub observation_schema: String,
    /// Authorized pack-owned change value.
    pub observation: CanonicalJsonV1,
    /// Exact supplied after-view offers when those offers changed.
    pub action_offers: Option<CanonicalActionOffersV1>,
}

impl PackObservationV1 {
    /// Constructs an observation without an Action Offer update.
    #[must_use]
    pub fn new(observation_schema: impl Into<String>, observation: CanonicalJsonV1) -> Self {
        Self {
            observation_schema: observation_schema.into(),
            observation,
            action_offers: None,
        }
    }

    /// Reuses the host-supplied exact after-view Action Offer container.
    #[must_use]
    pub fn with_action_offers(mut self, action_offers: CanonicalActionOffersV1) -> Self {
        self.action_offers = Some(action_offers);
        self
    }
}

/// Owned deterministic helper inputs carried across a portable executor seam.
///
/// The values duplicate the existing trusted context exactly; they do not add
/// an entropy, time, or host-effect source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackDeterministicContextV1 {
    pub room_seed: RoomSeedV1,
    pub pack_digest: PackDigestV1,
    pub next_room_sequence: RoomSequenceV1,
}

impl PackDeterministicContextV1 {
    fn from_borrowed(value: &DeterministicContextV1<'_>) -> Self {
        Self {
            room_seed: value.room_seed.clone(),
            pack_digest: value.pack_digest.clone(),
            next_room_sequence: value.next_room_sequence,
        }
    }
}

/// Owned, explicitly tagged representation of one typed pack viewer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "viewer_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackWireViewerV1 {
    Public { member_id: MemberId },
    Participant { member_id: MemberId },
    Operator { member_id: MemberId },
    Historical { member_id: MemberId },
    FinalReveal { member_id: MemberId },
}

impl PackWireViewerV1 {
    fn from_borrowed(value: &PackViewerV1) -> Self {
        match value {
            PackViewerV1::Public(member_id) => Self::Public {
                member_id: member_id.clone(),
            },
            PackViewerV1::Participant(member_id) => Self::Participant {
                member_id: member_id.clone(),
            },
            PackViewerV1::Operator(member_id) => Self::Operator {
                member_id: member_id.clone(),
            },
            PackViewerV1::Historical(member_id) => Self::Historical {
                member_id: member_id.clone(),
            },
            PackViewerV1::FinalReveal(member_id) => Self::FinalReveal {
                member_id: member_id.clone(),
            },
        }
    }

    /// Restores the trusted typed viewer represented by this owned value.
    #[must_use]
    pub fn into_viewer(self) -> PackViewerV1 {
        match self {
            Self::Public { member_id } => PackViewerV1::Public(member_id),
            Self::Participant { member_id } => PackViewerV1::Participant(member_id),
            Self::Operator { member_id } => PackViewerV1::Operator(member_id),
            Self::Historical { member_id } => PackViewerV1::Historical(member_id),
            Self::FinalReveal { member_id } => PackViewerV1::FinalReveal(member_id),
        }
    }
}

/// Complete owned input for the portable `initialize` operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackInitializeRequestV1 {
    pub room_id: RoomId,
    pub pack_digest: PackDigestV1,
    pub configuration: CanonicalJsonV1,
    pub initial_core_state: CoreRoomStateV1,
    pub room_seed: RoomSeedV1,
    pub created_at: CreationRecordedAt,
    pub deterministic_context: PackDeterministicContextV1,
}

/// Complete owned input for the portable `reduce` operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackReduceRequestV1 {
    pub prior_activity_state: CanonicalJsonV1,
    pub core_before: CoreRoomStateV1,
    pub proposed_core_after: CoreRoomStateV1,
    pub scheduled_timers: BTreeMap<crate::TimerId, ScheduledTimerV1>,
    pub next_room_seq: RoomSequenceV1,
    pub recorded_stimulus: RecordedStimulusV1,
    pub deterministic_context: PackDeterministicContextV1,
}

/// Complete owned input for the portable `view` operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackViewRequestV1 {
    pub core: CoreRoomStateV1,
    pub activity_state: CanonicalJsonV1,
    pub complete_head: CompleteHeadV1,
    pub viewer: PackWireViewerV1,
}

/// Owned checked after-view supplied to portable observation code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackWireValidatedViewV1 {
    pub viewer: PackWireViewerV1,
    pub complete_head: CompleteHeadV1,
    pub projection_schema: String,
    pub projection: CanonicalJsonV1,
    pub action_offers: Vec<ActionOfferV1>,
}

/// Complete owned input for the portable `observe` operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackObserveRequestV1 {
    pub core_before: CoreRoomStateV1,
    pub activity_before: CanonicalJsonV1,
    pub core_after: CoreRoomStateV1,
    pub activity_after: CanonicalJsonV1,
    pub recorded_stimulus: RecordedStimulusV1,
    pub ordered_domain_events: Vec<CanonicalJsonV1>,
    pub viewer: PackWireViewerV1,
    pub after_view: PackWireValidatedViewV1,
}

/// The only portable observation choices for Action Offers.
///
/// A Component can request reuse of the checked after-view allocation but can
/// never supply an independently serialized replacement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackWireObservationOffersV1 {
    Unchanged,
    ReuseAfterView,
}

/// Owned portable observation output before exact offer-reference restoration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackWireObservationV1 {
    pub observation_schema: String,
    pub observation: CanonicalJsonV1,
    pub action_offers: PackWireObservationOffersV1,
}

/// Bounded semantic faults a portable callback may deliberately return.
///
/// Invalid schemas, bounds, declarations, timers, offers, and state remain
/// checked exclusively by [`ActivityPackHostV1`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "callback_fault_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PackCallbackFaultV1 {
    Callback { bounded_safe_detail: String },
    PrivacyContract { bounded_safe_detail: String },
}

impl PackCallbackFaultV1 {
    fn into_pack_fault(self) -> PackFaultV1 {
        match self {
            Self::Callback {
                bounded_safe_detail,
            } => PackFaultV1::Callback(bounded_safe_detail),
            Self::PrivacyContract {
                bounded_safe_detail,
            } => PackFaultV1::PrivacyContract(bounded_safe_detail),
        }
    }
}

/// Canonical success/fault envelope returned by a portable callback.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation_result_type",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PackOperationResultV1<T> {
    Success { output: T },
    Fault { fault: PackCallbackFaultV1 },
}

impl<T> PackOperationResultV1<T> {
    /// Converts the portable callback result into the existing trusted fault
    /// channel without bypassing host-side output validation.
    ///
    /// # Errors
    ///
    /// Returns the exact bounded callback fault carried by a `fault` result.
    pub fn into_result(self) -> Result<T, PackFaultV1> {
        match self {
            Self::Success { output } => Ok(output),
            Self::Fault { fault } => Err(fault.into_pack_fault()),
        }
    }

    fn map<U>(self, convert: impl FnOnce(T) -> U) -> PackOperationResultV1<U> {
        match self {
            Self::Success { output } => PackOperationResultV1::Success {
                output: convert(output),
            },
            Self::Fault { fault } => PackOperationResultV1::Fault { fault },
        }
    }
}

/// Sealed canonical codec for the five portable operation envelopes.
///
/// This is deliberately separate from [`CanonicalPackCodecV1`], whose bytes
/// and retained identity remain frozen for persisted Room values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalPackOperationCodecV1 {
    _sealed: (),
}

#[allow(clippy::missing_errors_doc, clippy::unused_self)]
impl CanonicalPackOperationCodecV1 {
    #[must_use]
    pub const fn canonical_v1() -> Self {
        Self { _sealed: () }
    }

    pub fn decode_descriptor(
        self,
        bytes: &[u8],
    ) -> Result<PackDescriptorContentV1, CanonicalJsonError> {
        CanonicalJsonV1::decode_canonical(bytes)
    }

    pub fn encode_initialize_request(
        self,
        input: &ActivityGenesisInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&PackInitializeRequestV1 {
            room_id: input.room_id.clone(),
            pack_digest: input.pack_digest.clone(),
            configuration: input.configuration.clone(),
            initial_core_state: input.initial_core_state.clone(),
            room_seed: input.room_seed.clone(),
            created_at: input.created_at.clone(),
            deterministic_context: PackDeterministicContextV1::from_borrowed(context),
        })
    }

    pub fn decode_initialize_result(
        self,
        bytes: &[u8],
    ) -> Result<PackOperationResultV1<InitialOutputV1>, CanonicalJsonError> {
        CanonicalJsonV1::decode_canonical(bytes)
    }

    pub fn encode_reduce_request(
        self,
        input: &ActivityReduceInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&PackReduceRequestV1 {
            prior_activity_state: input.prior_activity_state.clone(),
            core_before: input.core_before.clone(),
            proposed_core_after: input.proposed_core_after.clone(),
            scheduled_timers: input.scheduled_timers.clone(),
            next_room_seq: input.next_room_seq,
            recorded_stimulus: input.recorded_stimulus.clone(),
            deterministic_context: PackDeterministicContextV1::from_borrowed(context),
        })
    }

    pub fn decode_reduce_result(
        self,
        bytes: &[u8],
    ) -> Result<PackOperationResultV1<ActivityDispositionV1>, CanonicalJsonError> {
        CanonicalJsonV1::decode_canonical(bytes)
    }

    pub fn encode_view_request(
        self,
        input: &ViewInputV1<'_>,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&PackViewRequestV1 {
            core: input.core.clone(),
            activity_state: input.activity_state.clone(),
            complete_head: input.complete_head.clone(),
            viewer: PackWireViewerV1::from_borrowed(input.viewer),
        })
    }

    pub fn decode_view_result(
        self,
        bytes: &[u8],
    ) -> Result<PackOperationResultV1<PackViewV1>, CanonicalJsonError> {
        CanonicalJsonV1::decode_canonical(bytes)
    }

    pub fn encode_observe_request(
        self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&PackObserveRequestV1 {
            core_before: input.core_before.clone(),
            activity_before: input.activity_before.clone(),
            core_after: input.core_after.clone(),
            activity_after: input.activity_after.clone(),
            recorded_stimulus: input.recorded_stimulus.clone(),
            ordered_domain_events: input.ordered_domain_events.to_vec(),
            viewer: PackWireViewerV1::from_borrowed(input.viewer),
            after_view: PackWireValidatedViewV1 {
                viewer: PackWireViewerV1::from_borrowed(input.after_view.viewer()),
                complete_head: input.after_view.complete_head().clone(),
                projection_schema: input.after_view.projection_schema().to_owned(),
                projection: input.after_view.projection().clone(),
                action_offers: input.after_view.action_offers().offers().to_vec(),
            },
        })
    }

    pub fn decode_observe_result(
        self,
        bytes: &[u8],
        after_view: &ValidatedPackViewV1,
    ) -> Result<PackOperationResultV1<Option<PackObservationV1>>, CanonicalJsonError> {
        let result: PackOperationResultV1<Option<PackWireObservationV1>> =
            CanonicalJsonV1::decode_canonical(bytes)?;
        Ok(result.map(|observation| {
            observation.map(|observation| {
                let mut restored =
                    PackObservationV1::new(observation.observation_schema, observation.observation);
                if observation.action_offers == PackWireObservationOffersV1::ReuseAfterView {
                    restored = restored.with_action_offers(after_view.action_offers().clone());
                }
                restored
            })
        }))
    }
}

impl PartialEq for CanonicalActionOffersV1 {
    fn eq(&self, other: &Self) -> bool {
        self.offers == other.offers && self.canonical_bytes == other.canonical_bytes
    }
}

impl Eq for CanonicalActionOffersV1 {}

/// One schema-content identity used by descriptors and revision locks.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaReferenceV1 {
    /// Stable schema identity.
    pub schema_id: String,
    /// BLAKE3 of exact canonical schema bytes.
    pub schema_digest: Blake3DigestV1,
}

/// One descriptor-declared Role and final-state cardinality.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleDefinitionV1 {
    /// Stable Role name.
    pub role: String,
    /// Minimum non-departed assignments.
    pub minimum: u32,
    /// Maximum non-departed assignments.
    pub maximum: u32,
}

/// One descriptor-declared Action and exact payload schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDefinitionV1 {
    /// Stable Action type, in descriptor offer order.
    pub action_type: String,
    /// Exact payload schema.
    pub payload_schema: SchemaReferenceV1,
}

/// Hard canonical output limits for one exact revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackLimitsV1 {
    pub maximum_state_bytes: u32,
    pub maximum_events: u32,
    pub maximum_timer_requests: u32,
    pub maximum_attention_signals: u32,
    pub maximum_projection_bytes: u32,
    pub maximum_observation_bytes: u32,
    pub maximum_nesting: u32,
    pub maximum_collection_items: u32,
    pub maximum_text_bytes: u32,
}

/// Immutable descriptor for one semantic revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackRevisionDescriptorV1 {
    pub pack_id: String,
    pub name: String,
    pub explanatory_version: String,
    pub revision_digest: PackDigestV1,
    pub host_contract: String,
    pub canonical_codec: String,
    pub configuration_schema: SchemaReferenceV1,
    pub state_schema: SchemaReferenceV1,
    pub roles: Vec<RoleDefinitionV1>,
    pub actions: Vec<ActionDefinitionV1>,
    pub rejection_codes: Vec<String>,
    pub attention_reasons: Vec<String>,
    /// Schemas keyed by normalized non-Action Stimulus/input type.
    pub stimulus_schemas: BTreeMap<String, SchemaReferenceV1>,
    /// Schemas keyed by disposition, rejection-detail, timer, Attention, or
    /// other declared output type not covered by the specialized maps below.
    pub output_schemas: BTreeMap<String, SchemaReferenceV1>,
    pub event_schemas: BTreeMap<String, SchemaReferenceV1>,
    pub projection_schemas: BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    pub observation_schemas: BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    pub limits: PackLimitsV1,
}

/// Portable descriptor callback output with the cyclic semantic revision
/// digest deliberately omitted.
///
/// Build order is Component bytes, Component BLAKE3 in the revision lock,
/// semantic revision digest, then the complete retained descriptor. The
/// Component Host compares this value byte-for-byte with the retained
/// descriptor's content form before constructing its adapter-owned complete
/// descriptor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackDescriptorContentV1 {
    pub pack_id: String,
    pub name: String,
    pub explanatory_version: String,
    pub host_contract: String,
    pub canonical_codec: String,
    pub configuration_schema: SchemaReferenceV1,
    pub state_schema: SchemaReferenceV1,
    pub roles: Vec<RoleDefinitionV1>,
    pub actions: Vec<ActionDefinitionV1>,
    pub rejection_codes: Vec<String>,
    pub attention_reasons: Vec<String>,
    pub stimulus_schemas: BTreeMap<String, SchemaReferenceV1>,
    pub output_schemas: BTreeMap<String, SchemaReferenceV1>,
    pub event_schemas: BTreeMap<String, SchemaReferenceV1>,
    pub projection_schemas: BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    pub observation_schemas: BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    pub limits: PackLimitsV1,
}

impl PackRevisionDescriptorV1 {
    /// Returns the exact descriptor callback form, excluding only the cyclic
    /// semantic revision digest.
    #[must_use]
    pub fn content(&self) -> PackDescriptorContentV1 {
        PackDescriptorContentV1 {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            explanatory_version: self.explanatory_version.clone(),
            host_contract: self.host_contract.clone(),
            canonical_codec: self.canonical_codec.clone(),
            configuration_schema: self.configuration_schema.clone(),
            state_schema: self.state_schema.clone(),
            roles: self.roles.clone(),
            actions: self.actions.clone(),
            rejection_codes: self.rejection_codes.clone(),
            attention_reasons: self.attention_reasons.clone(),
            stimulus_schemas: self.stimulus_schemas.clone(),
            output_schemas: self.output_schemas.clone(),
            event_schemas: self.event_schemas.clone(),
            projection_schemas: self.projection_schemas.clone(),
            observation_schemas: self.observation_schemas.clone(),
            limits: self.limits,
        }
    }

    /// Recomputes the descriptor-content digest while deliberately excluding
    /// the self-referential revision digest.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed descriptor cannot be canonically encoded.
    pub fn content_digest(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&encode(&DescriptorContentV1 {
            pack_id: &self.pack_id,
            name: &self.name,
            explanatory_version: &self.explanatory_version,
            host_contract: &self.host_contract,
            canonical_codec: &self.canonical_codec,
            configuration_schema: &self.configuration_schema,
            state_schema: &self.state_schema,
            roles: &self.roles,
            actions: &self.actions,
            rejection_codes: &self.rejection_codes,
            attention_reasons: &self.attention_reasons,
            stimulus_schemas: &self.stimulus_schemas,
            output_schemas: &self.output_schemas,
            event_schemas: &self.event_schemas,
            projection_schemas: &self.projection_schemas,
            observation_schemas: &self.observation_schemas,
            limits: self.limits,
        })?))
    }

    fn schema_references(&self) -> impl Iterator<Item = &SchemaReferenceV1> {
        std::iter::once(&self.configuration_schema)
            .chain(std::iter::once(&self.state_schema))
            .chain(self.actions.iter().map(|action| &action.payload_schema))
            .chain(self.stimulus_schemas.values())
            .chain(self.output_schemas.values())
            .chain(self.event_schemas.values())
            .chain(self.projection_schemas.values())
            .chain(self.observation_schemas.values())
    }

    fn validate_shape(&self, digest: &PackDigestV1) -> Result<(), PackRegistryErrorV1> {
        if self.pack_id.is_empty()
            || self.name.is_empty()
            || self.explanatory_version.is_empty()
            || self.roles.is_empty()
            || self.actions.is_empty()
        {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "identity, Roles, and Actions must be nonempty",
            });
        }

        let mut roles = BTreeSet::new();
        if self.roles.iter().any(|role| {
            role.role.is_empty()
                || role.maximum == 0
                || role.minimum > role.maximum
                || !roles.insert(&role.role)
        }) {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "Roles must be unique, named, and have coherent cardinality",
            });
        }

        let mut actions = BTreeSet::new();
        if self.actions.iter().any(|action| {
            action.action_type.is_empty()
                || action.payload_schema.schema_id.is_empty()
                || !actions.insert(&action.action_type)
        }) {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "Actions must be unique, named, and schema-bound",
            });
        }

        if !unique_nonempty(&self.rejection_codes)
            || !unique_nonempty(&self.attention_reasons)
            || [
                &self.stimulus_schemas,
                &self.output_schemas,
                &self.event_schemas,
            ]
            .into_iter()
            .any(|schemas| schemas.keys().any(String::is_empty))
            || self
                .schema_references()
                .any(|reference| reference.schema_id.is_empty())
        {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "declared codes and schema identities must be unique and nonempty",
            });
        }

        let viewer_coverage_is_exact = self.projection_schemas.len()
            == PackViewerClassV1::ALL.len()
            && self.observation_schemas.len() == PackViewerClassV1::ALL.len()
            && PackViewerClassV1::ALL.iter().all(|viewer| {
                self.projection_schemas.contains_key(viewer)
                    && self.observation_schemas.contains_key(viewer)
            });
        let output_coverage_is_complete = self.rejection_codes.iter().all(|code| {
            self.output_schemas
                .contains_key(&format!("rejection:{code}"))
        }) && self.attention_reasons.iter().all(|reason| {
            self.output_schemas
                .contains_key(&format!("attention:{reason}"))
        });
        if !viewer_coverage_is_exact || !output_coverage_is_complete {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "viewer and declared rejection/Attention schema coverage must be complete",
            });
        }

        let limits = self.limits;
        if [
            limits.maximum_state_bytes,
            limits.maximum_events,
            limits.maximum_timer_requests,
            limits.maximum_attention_signals,
            limits.maximum_projection_bytes,
            limits.maximum_observation_bytes,
            limits.maximum_nesting,
            limits.maximum_collection_items,
            limits.maximum_text_bytes,
        ]
        .contains(&0)
        {
            return Err(PackRegistryErrorV1::InvalidDescriptorShape {
                revision_digest: digest.clone(),
                detail: "every hard output bound must be nonzero",
            });
        }
        Ok(())
    }
}

fn unique_nonempty(values: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    values
        .iter()
        .all(|value| !value.is_empty() && unique.insert(value))
}

#[derive(Serialize)]
struct DescriptorContentV1<'a> {
    pack_id: &'a str,
    name: &'a str,
    explanatory_version: &'a str,
    host_contract: &'a str,
    canonical_codec: &'a str,
    configuration_schema: &'a SchemaReferenceV1,
    state_schema: &'a SchemaReferenceV1,
    roles: &'a [RoleDefinitionV1],
    actions: &'a [ActionDefinitionV1],
    rejection_codes: &'a [String],
    attention_reasons: &'a [String],
    stimulus_schemas: &'a BTreeMap<String, SchemaReferenceV1>,
    output_schemas: &'a BTreeMap<String, SchemaReferenceV1>,
    event_schemas: &'a BTreeMap<String, SchemaReferenceV1>,
    projection_schemas: &'a BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    observation_schemas: &'a BTreeMap<PackViewerClassV1, SchemaReferenceV1>,
    limits: PackLimitsV1,
}

/// One exact canonical schema document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackSchemaV1 {
    schema_id: String,
    schema_digest: Blake3DigestV1,
    canonical_schema: CanonicalJsonV1,
}

impl PackSchemaV1 {
    /// Binds the stable ID to the exact canonical schema bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical bytes cannot be produced.
    pub fn new(
        schema_id: impl Into<String>,
        canonical_schema: CanonicalJsonV1,
    ) -> Result<Self, CanonicalJsonError> {
        let schema_digest = Blake3DigestV1::hash(&canonical_schema.to_bytes()?);
        Ok(Self {
            schema_id: schema_id.into(),
            schema_digest,
            canonical_schema,
        })
    }

    #[must_use]
    pub fn reference(&self) -> SchemaReferenceV1 {
        SchemaReferenceV1 {
            schema_id: self.schema_id.clone(),
            schema_digest: self.schema_digest.clone(),
        }
    }

    #[must_use]
    pub fn canonical_schema(&self) -> &CanonicalJsonV1 {
        &self.canonical_schema
    }
}

/// Exact retained schema documents for one revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackSchemaBundleV1 {
    schemas: BTreeMap<String, PackSchemaV1>,
}

impl PackSchemaBundleV1 {
    /// Constructs a bundle and rejects duplicate schema IDs.
    ///
    /// # Errors
    ///
    /// Returns an error when an ID occurs more than once.
    pub fn new(
        schemas: impl IntoIterator<Item = PackSchemaV1>,
    ) -> Result<Self, PackRegistryErrorV1> {
        let mut by_id = BTreeMap::new();
        for schema in schemas {
            let id = schema.schema_id.clone();
            if id.is_empty() {
                return Err(PackRegistryErrorV1::InvalidSchemaId);
            }
            validate_schema_document(&schema.canonical_schema).map_err(|detail| {
                PackRegistryErrorV1::MalformedSchema {
                    schema_id: id.clone(),
                    detail,
                }
            })?;
            if by_id.insert(id.clone(), schema).is_some() {
                return Err(PackRegistryErrorV1::SchemaCollision(id));
            }
        }
        if by_id.is_empty() {
            return Err(PackRegistryErrorV1::EmptySchemaBundle);
        }
        Ok(Self { schemas: by_id })
    }

    /// Recomputes the bundle digest from sorted schema IDs and exact
    /// schema-content digests.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed bundle cannot be canonically encoded.
    pub fn digest(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        let schemas: Vec<_> = self.schemas.values().map(PackSchemaV1::reference).collect();
        Ok(Blake3DigestV1::hash(&encode(&SchemaBundleDigestV1 {
            domain: SCHEMA_BUNDLE_DOMAIN,
            schemas: &schemas,
        })?))
    }

    fn get(&self, schema_id: &str) -> Option<&PackSchemaV1> {
        self.schemas.get(schema_id)
    }

    fn validate_value(
        &self,
        reference: &SchemaReferenceV1,
        value: &CanonicalJsonV1,
    ) -> Result<(), PackFaultV1> {
        let schema = self.get(&reference.schema_id).ok_or_else(|| {
            PackFaultV1::SchemaViolation(format!("missing schema {}", reference.schema_id))
        })?;
        if schema.schema_digest != reference.schema_digest {
            return Err(PackFaultV1::SchemaViolation(format!(
                "wrong schema digest for {}",
                reference.schema_id
            )));
        }
        let schema_value = serde_json::to_value(&schema.canonical_schema)
            .map_err(|error| PackFaultV1::SchemaViolation(error.to_string()))?;
        let instance = serde_json::to_value(value)
            .map_err(|error| PackFaultV1::SchemaViolation(error.to_string()))?;
        validate_schema_node(&schema_value, &instance, "$", 0).map_err(PackFaultV1::SchemaViolation)
    }
}

fn validate_schema_document(schema: &CanonicalJsonV1) -> Result<(), String> {
    let schema = serde_json::to_value(schema)
        .map_err(|error| format!("schema cannot be decoded: {error}"))?;
    validate_schema_shape(&schema, "$", 0)
}

#[allow(clippy::too_many_lines)]
fn validate_schema_shape(schema: &serde_json::Value, path: &str, depth: u32) -> Result<(), String> {
    if depth > 64 {
        return Err(format!("{path}: schema nesting exceeds 64"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;
    for keyword in object.keys() {
        if !SUPPORTED_SCHEMA_KEYWORDS.contains(&keyword.as_str()) {
            return Err(format!("{path}: unsupported schema keyword {keyword}"));
        }
    }
    if let Some(description) = object.get("description")
        && !description.is_string()
    {
        return Err(format!("{path}: description must be a string"));
    }
    let declared_type = object
        .get("type")
        .map(|value| {
            let declared_type = value
                .as_str()
                .ok_or_else(|| format!("{path}: type must be a string"))?;
            if !["null", "boolean", "integer", "string", "array", "object"].contains(&declared_type)
            {
                return Err(format!("{path}: unsupported schema type {declared_type}"));
            }
            Ok(declared_type)
        })
        .transpose()?;
    if let Some(values) = object.get("enum") {
        let values = values
            .as_array()
            .ok_or_else(|| format!("{path}: enum must be an array"))?;
        if values.is_empty() {
            return Err(format!("{path}: enum must not be empty"));
        }
        for (index, value) in values.iter().enumerate() {
            if values[..index].contains(value) {
                return Err(format!("{path}: enum values must be unique"));
            }
        }
    }
    let minimum = schema_i64_keyword(object, "minimum", path)?;
    let maximum = schema_i64_keyword(object, "maximum", path)?;
    if (minimum.is_some() || maximum.is_some()) && declared_type != Some("integer") {
        return Err(format!("{path}: minimum/maximum require type integer"));
    }
    if minimum.zip(maximum).is_some_and(|(min, max)| min > max) {
        return Err(format!("{path}: minimum exceeds maximum"));
    }
    let minimum_length = schema_u64_keyword(object, "minLength", path)?;
    let maximum_length = schema_u64_keyword(object, "maxLength", path)?;
    if (minimum_length.is_some() || maximum_length.is_some()) && declared_type != Some("string") {
        return Err(format!("{path}: minLength/maxLength require type string"));
    }
    if minimum_length
        .zip(maximum_length)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(format!("{path}: minLength exceeds maxLength"));
    }
    let minimum_items = schema_u64_keyword(object, "minItems", path)?;
    let maximum_items = schema_u64_keyword(object, "maxItems", path)?;
    if (minimum_items.is_some() || maximum_items.is_some() || object.contains_key("items"))
        && declared_type != Some("array")
    {
        return Err(format!("{path}: item constraints require type array"));
    }
    if minimum_items
        .zip(maximum_items)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(format!("{path}: minItems exceeds maxItems"));
    }
    if let Some(items) = object.get("items") {
        validate_schema_shape(items, &format!("{path}.items"), depth + 1)?;
    }
    let has_object_keywords = object.contains_key("properties")
        || object.contains_key("required")
        || object.contains_key("additionalProperties");
    if has_object_keywords && declared_type != Some("object") {
        return Err(format!("{path}: object constraints require type object"));
    }
    let properties = object
        .get("properties")
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| format!("{path}: properties must be an object"))
        })
        .transpose()?;
    if let Some(properties) = properties {
        for (name, child) in properties {
            if name.is_empty() {
                return Err(format!("{path}: property names must be nonempty"));
            }
            validate_schema_shape(child, &format!("{path}.properties.{name}"), depth + 1)?;
        }
    }
    if let Some(required) = object.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| format!("{path}: required must be an array"))?;
        let mut unique = BTreeSet::new();
        for key in required {
            let key = key
                .as_str()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| format!("{path}: required entries must be nonempty strings"))?;
            if !unique.insert(key) {
                return Err(format!("{path}: required entries must be unique"));
            }
            if !properties.is_some_and(|properties| properties.contains_key(key)) {
                return Err(format!("{path}: required property {key} has no schema"));
            }
        }
    }
    if let Some(additional) = object.get("additionalProperties")
        && !additional.is_boolean()
    {
        return Err(format!("{path}: additionalProperties must be a boolean"));
    }
    Ok(())
}

fn schema_i64_keyword(
    schema: &serde_json::Map<String, serde_json::Value>,
    keyword: &str,
    path: &str,
) -> Result<Option<i64>, String> {
    schema
        .get(keyword)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| format!("{path}: {keyword} must be an integer"))
        })
        .transpose()
}

fn schema_u64_keyword(
    schema: &serde_json::Map<String, serde_json::Value>,
    keyword: &str,
    path: &str,
) -> Result<Option<u64>, String> {
    schema
        .get(keyword)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{path}: {keyword} must be a nonnegative integer"))
        })
        .transpose()
}

#[allow(clippy::too_many_lines)]
fn validate_schema_node(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    depth: u32,
) -> Result<(), String> {
    if depth > 64 {
        return Err(format!("{path}: schema nesting exceeds host maximum"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;
    if let Some(expected) = object.get("const")
        && value != expected
    {
        return Err(format!("{path}: value differs from schema const"));
    }
    if let Some(allowed) = object.get("enum") {
        let allowed = allowed
            .as_array()
            .ok_or_else(|| format!("{path}: schema enum must be an array"))?;
        if !allowed.contains(value) {
            return Err(format!("{path}: value is outside schema enum"));
        }
    }
    if let Some(expected_type) = object.get("type") {
        let expected_type = expected_type
            .as_str()
            .ok_or_else(|| format!("{path}: schema type must be a string"))?;
        let matches = match expected_type {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "string" => value.is_string(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            unsupported => return Err(format!("{path}: unsupported schema type {unsupported}")),
        };
        if !matches {
            return Err(format!("{path}: expected {expected_type}"));
        }
    }

    if let Some(integer) = value.as_i64() {
        if let Some(minimum) = object.get("minimum").and_then(serde_json::Value::as_i64)
            && integer < minimum
        {
            return Err(format!("{path}: integer is below minimum"));
        }
        if let Some(maximum) = object.get("maximum").and_then(serde_json::Value::as_i64)
            && integer > maximum
        {
            return Err(format!("{path}: integer exceeds maximum"));
        }
    }
    if let Some(text) = value.as_str() {
        if let Some(minimum) = object.get("minLength").and_then(serde_json::Value::as_u64)
            && text.len() < usize::try_from(minimum).unwrap_or(usize::MAX)
        {
            return Err(format!("{path}: string is shorter than minLength"));
        }
        if let Some(maximum) = object.get("maxLength").and_then(serde_json::Value::as_u64)
            && text.len() > usize::try_from(maximum).unwrap_or(usize::MAX)
        {
            return Err(format!("{path}: string exceeds maxLength"));
        }
    }
    if let Some(array) = value.as_array() {
        if let Some(minimum) = object.get("minItems").and_then(serde_json::Value::as_u64)
            && array.len() < usize::try_from(minimum).unwrap_or(usize::MAX)
        {
            return Err(format!("{path}: array is shorter than minItems"));
        }
        if let Some(maximum) = object.get("maxItems").and_then(serde_json::Value::as_u64)
            && array.len() > usize::try_from(maximum).unwrap_or(usize::MAX)
        {
            return Err(format!("{path}: array exceeds maxItems"));
        }
        if let Some(items) = object.get("items") {
            for (index, item) in array.iter().enumerate() {
                validate_schema_node(items, item, &format!("{path}[{index}]"), depth + 1)?;
            }
        }
    }
    if let Some(instance) = value.as_object() {
        let properties = object
            .get("properties")
            .map(|value| {
                value
                    .as_object()
                    .ok_or_else(|| format!("{path}: schema properties must be an object"))
            })
            .transpose()?
            .cloned()
            .unwrap_or_default();
        if let Some(required) = object.get("required") {
            for key in required
                .as_array()
                .ok_or_else(|| format!("{path}: schema required must be an array"))?
            {
                let key = key
                    .as_str()
                    .ok_or_else(|| format!("{path}: required key must be a string"))?;
                if !instance.contains_key(key) {
                    return Err(format!("{path}: missing required property {key}"));
                }
            }
        }
        if object.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            for key in instance.keys() {
                if !properties.contains_key(key) {
                    return Err(format!("{path}: undeclared property {key}"));
                }
            }
        }
        for (key, child_schema) in &properties {
            if let Some(child) = instance.get(key) {
                validate_schema_node(child_schema, child, &format!("{path}.{key}"), depth + 1)?;
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SchemaBundleDigestV1<'a> {
    domain: &'static str,
    schemas: &'a [SchemaReferenceV1],
}

/// Every canonical value class retained for exact replay.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackCodecKindV1 {
    Configuration,
    State,
    Stimulus,
    Disposition,
    DomainEvent,
    TimerRequest,
    AttentionSignal,
    View,
    Observation,
}

impl PackCodecKindV1 {
    const REQUIRED: [Self; 9] = [
        Self::Configuration,
        Self::State,
        Self::Stimulus,
        Self::Disposition,
        Self::DomainEvent,
        Self::TimerRequest,
        Self::AttentionSignal,
        Self::View,
        Self::Observation,
    ];
}

/// Exact retained codec identities for one revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackCodecBundleV1 {
    pub codec_id: String,
    pub writer_version: u32,
    pub retained_reader_versions: Vec<u32>,
    pub kinds: BTreeSet<PackCodecKindV1>,
}

impl PackCodecBundleV1 {
    /// Frozen complete canonical JSON codec set.
    #[must_use]
    pub fn canonical_v1() -> Self {
        Self {
            codec_id: CANONICAL_CODEC_ID.to_owned(),
            writer_version: 1,
            retained_reader_versions: vec![1],
            kinds: PackCodecKindV1::REQUIRED.into_iter().collect(),
        }
    }

    /// Recomputes the exact codec-bundle digest.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed bundle cannot be canonically encoded.
    pub fn digest(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&encode(&CodecBundleDigestV1 {
            domain: CODEC_BUNDLE_DOMAIN,
            bundle: self,
        })?))
    }
}

/// Sealed executable implementation of every retained canonical-v1 pack
/// codec. Registry metadata cannot replace or extend this behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalPackCodecV1 {
    _sealed: (),
}

#[allow(clippy::missing_errors_doc, clippy::unused_self)]
impl CanonicalPackCodecV1 {
    #[must_use]
    pub const fn canonical_v1() -> Self {
        Self { _sealed: () }
    }

    fn encode_identity(value: &CanonicalJsonV1) -> Result<Vec<u8>, CanonicalJsonError> {
        value.to_bytes()
    }

    fn decode_identity(bytes: &[u8]) -> Result<CanonicalJsonV1, CanonicalJsonError> {
        CanonicalJsonV1::from_canonical_bytes(bytes)
    }

    fn encode_typed<T: Serialize>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(value)
    }

    fn decode_typed<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CanonicalJsonError> {
        CanonicalJsonV1::decode_canonical(bytes)
    }

    pub fn encode_configuration(
        self,
        value: &CanonicalJsonV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_identity(value)
    }

    pub fn decode_configuration(self, bytes: &[u8]) -> Result<CanonicalJsonV1, CanonicalJsonError> {
        Self::decode_identity(bytes)
    }

    pub fn encode_state(self, value: &CanonicalJsonV1) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_identity(value)
    }

    pub fn decode_state(self, bytes: &[u8]) -> Result<CanonicalJsonV1, CanonicalJsonError> {
        Self::decode_identity(bytes)
    }

    pub fn encode_stimulus(
        self,
        value: &RecordedStimulusV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_typed(value)
    }

    pub fn decode_stimulus(self, bytes: &[u8]) -> Result<RecordedStimulusV1, CanonicalJsonError> {
        Self::decode_typed(bytes)
    }

    pub fn encode_disposition(
        self,
        value: &ActivityDispositionV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_typed(value)
    }

    pub fn decode_disposition(
        self,
        bytes: &[u8],
    ) -> Result<ActivityDispositionV1, CanonicalJsonError> {
        Self::decode_typed(bytes)
    }

    pub fn encode_domain_event(
        self,
        value: &CanonicalJsonV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_identity(value)
    }

    pub fn decode_domain_event(self, bytes: &[u8]) -> Result<CanonicalJsonV1, CanonicalJsonError> {
        Self::decode_identity(bytes)
    }

    pub fn encode_timer_request(
        self,
        value: &TimerRequestV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_typed(value)
    }

    pub fn decode_timer_request(self, bytes: &[u8]) -> Result<TimerRequestV1, CanonicalJsonError> {
        Self::decode_typed(bytes)
    }

    pub fn encode_attention_signal(
        self,
        value: &CanonicalJsonV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_identity(value)
    }

    pub fn decode_attention_signal(
        self,
        bytes: &[u8],
    ) -> Result<CanonicalJsonV1, CanonicalJsonError> {
        Self::decode_identity(bytes)
    }

    pub fn encode_view(self, value: &PackViewV1) -> Result<Vec<u8>, CanonicalJsonError> {
        Self::encode_typed(value)
    }

    pub fn decode_view(self, bytes: &[u8]) -> Result<PackViewV1, CanonicalJsonError> {
        Self::decode_typed(bytes)
    }

    pub fn encode_observation(
        self,
        value: &PackObservationV1,
    ) -> Result<Vec<u8>, CanonicalJsonError> {
        #[derive(Serialize)]
        struct WireObservationV1<'a> {
            observation_schema: &'a str,
            observation: &'a CanonicalJsonV1,
            action_offers: Option<&'a [ActionOfferV1]>,
        }
        Self::encode_typed(&WireObservationV1 {
            observation_schema: &value.observation_schema,
            observation: &value.observation,
            action_offers: value
                .action_offers
                .as_ref()
                .map(CanonicalActionOffersV1::offers),
        })
    }

    pub fn decode_observation(self, bytes: &[u8]) -> Result<PackObservationV1, CanonicalJsonError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireObservationV1 {
            observation_schema: String,
            observation: CanonicalJsonV1,
            action_offers: Option<Vec<ActionOfferV1>>,
        }
        let wire: WireObservationV1 = Self::decode_typed(bytes)?;
        let action_offers = wire
            .action_offers
            .map(|offers| {
                let canonical_bytes = encode(&offers)?;
                Ok(CanonicalActionOffersV1 {
                    offers: Arc::from(offers),
                    canonical_bytes: Arc::from(canonical_bytes),
                })
            })
            .transpose()?;
        Ok(PackObservationV1 {
            observation_schema: wire.observation_schema,
            observation: wire.observation,
            action_offers,
        })
    }

    fn validate_frozen_vectors(self) -> Result<(), CanonicalJsonError> {
        const STIMULUS: &[u8] = br#"{"canonical_payload":{},"generation":1,"scheduled_for":"2026-08-15T12:00:30Z","stimulus_type":"timer_fired","timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0"}"#;
        const DISPOSITION: &[u8] = br#"{"activity_disposition_type":"reject","bounded_safe_details":{},"declared_code":"denied"}"#;
        const TIMER: &[u8] = br#"{"expected_generation":1,"timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0","timer_request_type":"cancel_current"}"#;
        const VIEW: &[u8] =
            br#"{"action_offers":[],"projection":{},"projection_schema":"fixture/view/v1"}"#;
        const OBSERVATION: &[u8] = br#"{"action_offers":null,"observation":{},"observation_schema":"fixture/observation/v1"}"#;

        let configuration_bytes = br#"{"configuration":1}"#;
        let configuration = self.decode_configuration(configuration_bytes)?;
        if self.encode_configuration(&configuration)? != configuration_bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        let state_bytes = br#"{"state":1}"#;
        let state = self.decode_state(state_bytes)?;
        if self.encode_state(&state)? != state_bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        let stimulus = self.decode_stimulus(STIMULUS)?;
        let disposition = self.decode_disposition(DISPOSITION)?;
        let timer = self.decode_timer_request(TIMER)?;
        let view = self.decode_view(VIEW)?;
        let observation = self.decode_observation(OBSERVATION)?;
        if self.encode_stimulus(&stimulus)? != STIMULUS
            || self.encode_disposition(&disposition)? != DISPOSITION
            || self.encode_timer_request(&timer)? != TIMER
            || self.encode_view(&view)? != VIEW
            || self.encode_observation(&observation)? != OBSERVATION
        {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        let event_bytes = br#"{"event_type":"incremented"}"#;
        let event = self.decode_domain_event(event_bytes)?;
        if self.encode_domain_event(&event)? != event_bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        let attention_bytes = br#"{"reason":"ready"}"#;
        let attention = self.decode_attention_signal(attention_bytes)?;
        if self.encode_attention_signal(&attention)? != attention_bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes);
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct CodecBundleDigestV1<'a> {
    domain: &'static str,
    bundle: &'a PackCodecBundleV1,
}

/// One named deterministic build input to a semantic revision.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamedDigestV1 {
    pub name: String,
    pub digest: Blake3DigestV1,
}

/// Canonical semantic identity input. Its digest is always recomputed by the
/// host; no executor may self-declare its key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackRevisionLockV1 {
    pub revision_lock_id: String,
    pub pack_id: String,
    pub explanatory_version: String,
    pub host_contract: String,
    pub canonical_codec: String,
    pub descriptor_digest: Blake3DigestV1,
    pub schema_bundle_digest: Blake3DigestV1,
    pub codec_bundle_digest: Blake3DigestV1,
    pub deterministic_static_data_digests: Vec<NamedDigestV1>,
    pub rule_source_digest: Blake3DigestV1,
    pub deterministic_dependency_lock_digest: Blake3DigestV1,
}

impl PackRevisionLockV1 {
    /// Strictly decodes and verifies one persisted revision lock against its
    /// exact semantic digest.
    ///
    /// # Errors
    ///
    /// Returns an error for noncanonical bytes, invalid lock shape, or a
    /// digest mismatch.
    pub fn from_canonical_bytes(
        input: &[u8],
        expected_digest: &PackDigestV1,
    ) -> Result<Self, PackRegistryErrorV1> {
        let lock: Self = CanonicalJsonV1::decode_canonical(input)?;
        if lock.revision_digest()? != *expected_digest {
            return Err(PackRegistryErrorV1::RevisionDigestMismatch(
                expected_digest.clone(),
            ));
        }
        lock.validate_shape(expected_digest)?;
        Ok(lock)
    }

    /// Returns the unique canonical persistence bytes for this closed revision
    /// identity object.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed revision lock cannot be canonically encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    /// Recomputes the semantic revision digest from the complete canonical
    /// lock. This is the only registry-key derivation.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed lock cannot be canonically encoded.
    pub fn revision_digest(&self) -> Result<PackDigestV1, CanonicalJsonError> {
        let digest = Blake3DigestV1::hash(&encode(self)?).to_string();
        Ok(digest
            .parse()
            .unwrap_or_else(|_| unreachable!("a formatted BLAKE3 digest is canonical")))
    }

    fn validate_shape(&self, digest: &PackDigestV1) -> Result<(), PackRegistryErrorV1> {
        let static_inputs_are_canonical = self
            .deterministic_static_data_digests
            .iter()
            .all(|item| !item.name.is_empty())
            && self
                .deterministic_static_data_digests
                .windows(2)
                .all(|pair| pair[0].name < pair[1].name);
        if self.revision_lock_id.is_empty()
            || self.pack_id.is_empty()
            || self.explanatory_version.is_empty()
            || !static_inputs_are_canonical
        {
            return Err(PackRegistryErrorV1::InvalidRevisionLockShape {
                revision_digest: digest.clone(),
                detail: "identity fields must be nonempty and static inputs strictly name-sorted",
            });
        }
        Ok(())
    }
}

/// Registry-side retained artifacts accompanying one executable revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackGoldenActionV1 {
    /// Exact enabled participant Membership used by this vector.
    pub member_id: MemberId,
    /// Stable Action identity used only inside the deterministic vector.
    pub action_id: ActionId,
    /// Descriptor-declared Action type.
    pub action_type: String,
    /// Exact descriptor payload schema-content digest.
    pub payload_schema_digest: Blake3DigestV1,
    /// Strict canonical Action payload.
    pub canonical_payload: CanonicalJsonV1,
    /// Recorded semantic admission time.
    pub admitted_at: ActionAdmittedAt,
}

/// One exact recorded `ExternalInput` in a retained behavioral corpus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackGoldenExternalInputV1 {
    pub input: ExternalInputV1,
}

/// Exact typed viewer exercised by a retained behavioral corpus.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackGoldenViewerKindV1 {
    Public,
    Participant,
    Operator,
    Historical,
    FinalReveal,
}

/// One Membership-scoped corpus viewer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackGoldenViewerV1 {
    pub kind: PackGoldenViewerKindV1,
    pub member_id: MemberId,
    /// Zero-based checkpoint at which this viewer must first succeed.
    pub available_after_action: u32,
    /// Exact stable privacy-denial detail required at every earlier
    /// checkpoint. It must be absent when the viewer is immediately available.
    pub denied_before_detail: Option<String>,
}

impl PackGoldenViewerV1 {
    fn to_pack_viewer(&self) -> PackViewerV1 {
        match self.kind {
            PackGoldenViewerKindV1::Public => PackViewerV1::Public(self.member_id.clone()),
            PackGoldenViewerKindV1::Participant => {
                PackViewerV1::Participant(self.member_id.clone())
            }
            PackGoldenViewerKindV1::Operator => PackViewerV1::Operator(self.member_id.clone()),
            PackGoldenViewerKindV1::Historical => PackViewerV1::Historical(self.member_id.clone()),
            PackGoldenViewerKindV1::FinalReveal => {
                PackViewerV1::FinalReveal(self.member_id.clone())
            }
        }
    }
}

/// One frozen behavior-distinguishing retained executor corpus.
///
/// The expected transcript is authored from a reviewed executor at build
/// time. Registry construction reruns the complete vector through the
/// candidate checked host; an executor cannot satisfy the row merely by
/// returning the right descriptor or reporting an artifact digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackGoldenCorpusV1 {
    pub corpus_id: String,
    pub genesis: PackGenesisRequestV1,
    pub viewers: Vec<PackGoldenViewerV1>,
    pub actions: Vec<PackGoldenActionV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_inputs: Vec<PackGoldenExternalInputV1>,
    pub expected_transcript_digest: Blake3DigestV1,
}

impl PackGoldenCorpusV1 {
    /// Recomputes the digest of the complete frozen input/output corpus.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed corpus cannot be canonically encoded.
    pub fn digest(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&encode(self)?))
    }
}

/// Registry-side retained artifacts accompanying one executable revision.
#[derive(Clone)]
pub(crate) struct PackRegistryArtifactsV1 {
    pub(crate) expected_revision_digest: PackDigestV1,
    pub(crate) schemas: Option<PackSchemaBundleV1>,
    pub(crate) codecs: Option<PackCodecBundleV1>,
    pub(crate) codec_implementation: Option<CanonicalPackCodecV1>,
    pub(crate) executor_artifact_digest: Blake3DigestV1,
    pub(crate) golden_corpus_digest: Blake3DigestV1,
    pub(crate) golden_corpus: Option<PackGoldenCorpusV1>,
}

/// Independent selection and retention statuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackRegistryStatusV1 {
    pub selectable_for_new_rooms: bool,
    pub runnable_for_retained_rooms: bool,
}

/// Bounded read-only catalog metadata for one exact embedded revision.
///
/// This value deliberately excludes the executor and codecs. Catalog callers
/// can inspect only validated identity, declarations, and availability; they
/// cannot acquire an invocation or code-loading capability through this seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityPackCatalogRevisionV1 {
    pub revision_digest: PackDigestV1,
    pub descriptor: PackRevisionDescriptorV1,
    pub selectable_for_new_rooms: bool,
    pub runnable_for_retained_rooms: bool,
}

/// One already verified portable revision offered to the retained registry.
///
/// Construction grants no execution authority. [`PackRegistryV1::admit_portable`]
/// reruns the complete semantic lock, descriptor, schema, codec, executor,
/// golden-corpus, and collision checks before the revision becomes visible.
pub struct PortablePackAdmissionV1 {
    revision_lock: PackRevisionLockV1,
    descriptor: PackRevisionDescriptorV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    component_digest: Blake3DigestV1,
    golden_corpus_digest: Blake3DigestV1,
    golden_corpus: PackGoldenCorpusV1,
    executor: Arc<dyn ActivityPackV1>,
    status: PackRegistryStatusV1,
}

impl PortablePackAdmissionV1 {
    /// Binds an already verified portable executor to its exact retained
    /// artifacts. The exact Component digest is checked against
    /// [`PackRevisionLockV1::rule_source_digest`] during admission.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        revision_lock: PackRevisionLockV1,
        descriptor: PackRevisionDescriptorV1,
        schemas: PackSchemaBundleV1,
        codecs: PackCodecBundleV1,
        component_digest: Blake3DigestV1,
        golden_corpus_digest: Blake3DigestV1,
        golden_corpus: PackGoldenCorpusV1,
        executor: Arc<dyn ActivityPackV1>,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self {
            revision_lock,
            descriptor,
            schemas,
            codecs,
            component_digest,
            golden_corpus_digest,
            golden_corpus,
            executor,
            status,
        }
    }

    fn into_registry_entry(self) -> PackRegistryEntryV1 {
        PackRegistryEntryV1 {
            artifacts: PackRegistryArtifactsV1 {
                expected_revision_digest: self.descriptor.revision_digest.clone(),
                schemas: Some(self.schemas),
                codecs: Some(self.codecs),
                codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
                executor_artifact_digest: self.component_digest.clone(),
                golden_corpus_digest: self.golden_corpus_digest,
                golden_corpus: Some(self.golden_corpus),
            },
            revision_lock: self.revision_lock,
            descriptor: self.descriptor,
            executor_provenance: None,
            executor: Some(ExecutorBindingV1::Portable {
                executor: self.executor,
                component_digest: self.component_digest,
            }),
            status: self.status,
        }
    }
}

/// Candidate embedded registry row. Construction alone confers no trust;
/// [`PackRegistryV1::try_new`] recomputes and cross-checks every identity.
#[derive(Clone)]
pub(crate) struct PackRegistryEntryV1 {
    revision_lock: PackRevisionLockV1,
    descriptor: PackRevisionDescriptorV1,
    artifacts: PackRegistryArtifactsV1,
    executor_provenance: Option<ReviewedExecutorProvenanceV1>,
    executor: Option<ExecutorBindingV1>,
    status: PackRegistryStatusV1,
}

#[derive(Clone)]
enum ExecutorBindingV1 {
    Embedded {
        executor: Arc<dyn ActivityPackV1>,
        concrete_type_id: TypeId,
        concrete_constructor: &'static str,
    },
    Portable {
        executor: Arc<dyn ActivityPackV1>,
        component_digest: Blake3DigestV1,
    },
}

#[derive(Clone)]
enum ReviewedExecutorProvenanceV1 {
    CounterV1,
    CounterV2,
    CounterV3,
    CounterV4,
    AgentHeistLobbyV2,
    AgentHeistLobbyV3,
    AgentHeistLobbyV4,
    AgentHeistV1,
    AgentHeistV0,
    #[cfg(test)]
    Test {
        expected_type_id: TypeId,
        expected_constructor: &'static str,
        executor_artifact_digest: Blake3DigestV1,
    },
}

impl ReviewedExecutorProvenanceV1 {
    fn expected_type_id(&self) -> TypeId {
        match self {
            Self::CounterV1 => TypeId::of::<crate::counter::CounterV1>(),
            Self::CounterV2 => TypeId::of::<crate::counter::CounterV2>(),
            Self::CounterV3 => TypeId::of::<crate::counter_attention::CounterV3>(),
            Self::CounterV4 => TypeId::of::<crate::counter_attention_v4::CounterV4>(),
            Self::AgentHeistLobbyV2 => TypeId::of::<crate::agent_heist_lobby::AgentHeistLobbyV2>(),
            Self::AgentHeistLobbyV3 => {
                TypeId::of::<crate::agent_heist_lobby_v3::AgentHeistLobbyV3>()
            }
            Self::AgentHeistV1 => TypeId::of::<crate::agent_heist::AgentHeistV1>(),
            Self::AgentHeistLobbyV4 => TypeId::of::<crate::AgentHeistLobbyV4>(),
            Self::AgentHeistV0 => TypeId::of::<crate::agent_heist::AgentHeistV0>(),
            #[cfg(test)]
            Self::Test {
                expected_type_id, ..
            } => *expected_type_id,
        }
    }

    fn expected_constructor(&self) -> &'static str {
        match self {
            Self::CounterV1 => std::any::type_name::<crate::counter::CounterV1>(),
            Self::CounterV2 => std::any::type_name::<crate::counter::CounterV2>(),
            Self::CounterV3 => std::any::type_name::<crate::counter_attention::CounterV3>(),
            Self::CounterV4 => std::any::type_name::<crate::counter_attention_v4::CounterV4>(),
            Self::AgentHeistLobbyV2 => {
                std::any::type_name::<crate::agent_heist_lobby::AgentHeistLobbyV2>()
            }
            Self::AgentHeistLobbyV3 => {
                std::any::type_name::<crate::agent_heist_lobby_v3::AgentHeistLobbyV3>()
            }
            Self::AgentHeistV1 => std::any::type_name::<crate::agent_heist::AgentHeistV1>(),
            Self::AgentHeistLobbyV4 => std::any::type_name::<crate::AgentHeistLobbyV4>(),
            Self::AgentHeistV0 => std::any::type_name::<crate::agent_heist::AgentHeistV0>(),
            #[cfg(test)]
            Self::Test {
                expected_constructor,
                ..
            } => expected_constructor,
        }
    }

    fn executor_artifact_digest(&self) -> Blake3DigestV1 {
        match self {
            Self::CounterV1 => crate::counter::counter_artifact_digest_v1(),
            Self::CounterV2 => crate::counter::counter_artifact_digest_v2(),
            Self::CounterV3 => crate::counter_attention::counter_artifact_digest_v3(),
            Self::CounterV4 => crate::counter_attention_v4::counter_artifact_digest_v4(),
            Self::AgentHeistLobbyV2 => {
                crate::agent_heist_lobby::agent_heist_lobby_artifact_digest()
            }
            Self::AgentHeistLobbyV3 => {
                crate::agent_heist_lobby_v3::agent_heist_lobby_artifact_digest()
            }
            Self::AgentHeistV1 => crate::agent_heist::agent_heist_artifact_digest(),
            Self::AgentHeistLobbyV4 => crate::agent_heist_lobby_v4::artifact_digest(),
            Self::AgentHeistV0 => crate::agent_heist::agent_heist_legacy_artifact_digest(),
            #[cfg(test)]
            Self::Test {
                executor_artifact_digest,
                ..
            } => executor_artifact_digest.clone(),
        }
    }
}

#[cfg(test)]
fn reviewed_executor_provenance<E: ActivityPackV1>(
    executor_artifact_digest: Blake3DigestV1,
) -> ReviewedExecutorProvenanceV1 {
    ReviewedExecutorProvenanceV1::Test {
        expected_type_id: TypeId::of::<E>(),
        expected_constructor: std::any::type_name::<E>(),
        executor_artifact_digest,
    }
}

impl PackRegistryEntryV1 {
    fn embedded<E: ActivityPackV1>(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        executor_provenance: ReviewedExecutorProvenanceV1,
        executor: E,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self {
            revision_lock,
            descriptor: descriptor.clone(),
            artifacts,
            executor_provenance: Some(executor_provenance),
            executor: Some(ExecutorBindingV1::Embedded {
                executor: Arc::new(executor),
                concrete_type_id: TypeId::of::<E>(),
                concrete_constructor: std::any::type_name::<E>(),
            }),
            status,
        }
    }

    pub(crate) fn counter_v1(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::CounterV1,
            crate::counter::CounterV1,
            status,
        )
    }

    pub(crate) fn counter_v2(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::CounterV2,
            crate::counter::CounterV2,
            status,
        )
    }

    pub(crate) fn counter_v3(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::CounterV3,
            crate::counter_attention::CounterV3,
            status,
        )
    }

    pub(crate) fn counter_v4(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::CounterV4,
            crate::counter_attention_v4::CounterV4,
            status,
        )
    }

    pub(crate) fn agent_heist_v1(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::AgentHeistV1,
            crate::agent_heist::AgentHeistV1,
            status,
        )
    }

    pub(crate) fn agent_heist_lobby_v2(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::AgentHeistLobbyV2,
            crate::agent_heist_lobby::AgentHeistLobbyV2,
            status,
        )
    }

    pub(crate) fn agent_heist_v0(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::AgentHeistV0,
            crate::agent_heist::AgentHeistV0,
            status,
        )
    }

    pub(crate) fn agent_heist_lobby_v3(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::AgentHeistLobbyV3,
            crate::agent_heist_lobby_v3::AgentHeistLobbyV3,
            status,
        )
    }

    pub(crate) fn agent_heist_lobby_v4(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        status: PackRegistryStatusV1,
    ) -> Self {
        Self::embedded(
            revision_lock,
            descriptor,
            artifacts,
            ReviewedExecutorProvenanceV1::AgentHeistLobbyV4,
            crate::AgentHeistLobbyV4,
            status,
        )
    }

    #[must_use]
    #[cfg(test)]
    fn new<E: ActivityPackV1>(
        revision_lock: PackRevisionLockV1,
        descriptor: &'static PackRevisionDescriptorV1,
        artifacts: PackRegistryArtifactsV1,
        executor_provenance: Option<ReviewedExecutorProvenanceV1>,
        executor: Option<E>,
        status: PackRegistryStatusV1,
    ) -> Self {
        match (executor_provenance, executor) {
            (Some(provenance), Some(executor)) => Self::embedded(
                revision_lock,
                descriptor,
                artifacts,
                provenance,
                executor,
                status,
            ),
            (executor_provenance, executor) => Self {
                revision_lock,
                descriptor: (*descriptor).clone(),
                artifacts,
                executor_provenance,
                executor: executor.map(|executor| ExecutorBindingV1::Embedded {
                    executor: Arc::new(executor),
                    concrete_type_id: TypeId::of::<E>(),
                    concrete_constructor: std::any::type_name::<E>(),
                }),
                status,
            },
        }
    }
}

struct ValidatedPackEntryV1 {
    revision_lock: PackRevisionLockV1,
    descriptor: PackRevisionDescriptorV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    codec_implementation: CanonicalPackCodecV1,
    executor_artifact_digest: Blake3DigestV1,
    golden_corpus_digest: Blake3DigestV1,
    // Exercised by the checked host operations added in the next slice; it is
    // deliberately retained here and never exposed raw to downstream callers.
    #[allow(dead_code)]
    executor: Arc<dyn ActivityPackV1>,
    status: PackRegistryStatusV1,
}

/// Exact retained executable selected by semantic digest.
#[derive(Clone)]
pub struct RetainedActivityPackV1(Arc<ValidatedPackEntryV1>);

impl RetainedActivityPackV1 {
    #[must_use]
    pub fn revision_lock(&self) -> &PackRevisionLockV1 {
        &self.0.revision_lock
    }

    #[must_use]
    pub fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        &self.0.descriptor
    }

    #[must_use]
    pub fn schemas(&self) -> &PackSchemaBundleV1 {
        &self.0.schemas
    }

    #[must_use]
    pub fn codecs(&self) -> &PackCodecBundleV1 {
        &self.0.codecs
    }

    /// Returns the sealed executable retained codec implementation.
    #[must_use]
    pub fn codec(&self) -> CanonicalPackCodecV1 {
        self.0.codec_implementation
    }

    #[must_use]
    pub fn executor_artifact_digest(&self) -> &Blake3DigestV1 {
        &self.0.executor_artifact_digest
    }

    #[must_use]
    pub fn golden_corpus_digest(&self) -> &Blake3DigestV1 {
        &self.0.golden_corpus_digest
    }

    #[must_use]
    pub fn status(&self) -> PackRegistryStatusV1 {
        self.0.status
    }

    /// Creates the checked host facade. The retained executor itself is never
    /// exposed, so every callback necessarily crosses validation and panic
    /// containment.
    #[must_use]
    pub fn host(&self) -> ActivityPackHostV1 {
        ActivityPackHostV1 {
            retained: self.clone(),
        }
    }
}

/// Checked facade over one exact retained executor. This is the only public
/// path that can invoke pack behavior.
#[derive(Clone)]
pub struct ActivityPackHostV1 {
    retained: RetainedActivityPackV1,
}

impl ActivityPackHostV1 {
    /// Validates a declared Action payload before any lifecycle, Membership,
    /// offer-window, or stale-Head disposition can consume its identity.
    pub(crate) fn preflight_action_payload(
        &self,
        action_type: &str,
        payload: &CanonicalJsonV1,
    ) -> Result<Option<Blake3DigestV1>, ActivityPackReduceErrorV1> {
        let Some(definition) = self
            .retained
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == action_type)
        else {
            return Ok(None);
        };
        self.validate_value(
            &definition.payload_schema,
            payload,
            self.retained.descriptor().limits.maximum_state_bytes,
            "Action payload",
        )
        .map_err(|error| ActionAdmissionErrorV1::InvalidPayload(error.to_string()))?;
        Ok(Some(definition.payload_schema.schema_digest.clone()))
    }

    /// Returns immutable metadata for the bound exact semantic revision.
    #[must_use]
    pub fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        self.retained.descriptor()
    }

    /// Invokes and validates deterministic initialization for this exact
    /// retained revision.
    ///
    /// # Errors
    ///
    /// Fails closed on identity, schema, Role, bound, Timer, callback, or panic
    /// disagreement. No partially prepared Genesis is returned.
    fn initialize(&self, request: &PackGenesisRequestV1) -> Result<GenesisInputV1, PackFaultV1> {
        let descriptor = self.retained.descriptor();
        if request.pack_digest != descriptor.revision_digest {
            return Err(PackFaultV1::InvalidOutput(
                "Genesis revision differs from retained executor".to_owned(),
            ));
        }
        self.validate_roles(&request.initial_core_state)?;
        self.validate_value(
            &descriptor.configuration_schema,
            &request.configuration,
            descriptor.limits.maximum_state_bytes,
            "configuration",
        )?;
        let next_room_sequence = RoomSequenceV1::new(0)
            .unwrap_or_else(|_| unreachable!("zero is a valid Room sequence"));
        let cx = DeterministicContextV1::new(
            &request.room_seed,
            &request.pack_digest,
            next_room_sequence,
        );
        let input = ActivityGenesisInputV1 {
            room_id: &request.room_id,
            pack_digest: &request.pack_digest,
            configuration: &request.configuration,
            initial_core_state: &request.initial_core_state,
            room_seed: &request.room_seed,
            created_at: &request.created_at,
        };
        let output = invoke_pack(ActivityPackOperationV1::Initialize, || {
            self.retained.0.executor.initialize(&input, &cx)
        })?;
        (|| {
            self.validate_value(
                &descriptor.state_schema,
                &output.initial_activity_state,
                descriptor.limits.maximum_state_bytes,
                "initial Activity State",
            )?;
            let timers =
                self.normalize_initial_timers(output.timer_requests, request.created_at.as_str())?;
            Ok(GenesisInputV1::new(
                request.room_id.clone(),
                request.pack_digest.clone(),
                request.configuration.clone(),
                request.room_seed.clone(),
                request.created_at.clone(),
                request.initial_core_state.clone(),
                output.initial_activity_state,
            )
            .with_initial_timers(timers))
        })()
        .map_err(|fault| operation_fault(ActivityPackOperationV1::Initialize, fault))
    }

    /// Constructs and validates one complete authorized view.
    ///
    /// # Errors
    ///
    /// Fails closed before exposure when authorization, schema, ordering,
    /// Action Offer, output-bound, callback, or panic validation fails.
    pub fn view(&self, input: &ViewInputV1<'_>) -> Result<ValidatedPackViewV1, PackFaultV1> {
        self.validate_bound_head(input.core, input.activity_state, input.complete_head)?;
        self.validate_runtime_roles(input.core)?;
        self.validate_value(
            &self.retained.descriptor().state_schema,
            input.activity_state,
            self.retained.descriptor().limits.maximum_state_bytes,
            "Activity State",
        )?;
        let projection_reference = self.viewer_projection_schema(input.core, input.viewer)?;
        let raw = match invoke_pack(ActivityPackOperationV1::View, || {
            self.retained.0.executor.view(input)
        }) {
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::View,
                fault,
            }) if matches!(input.viewer, PackViewerV1::FinalReveal(_))
                && matches!(fault.as_ref(), PackFaultV1::PrivacyContract(_)) =>
            {
                return Err(*fault);
            }
            result => result?,
        };
        (|| {
            if raw.projection_schema != projection_reference.schema_id {
                return Err(PackFaultV1::PrivacyContract(format!(
                    "viewer requires projection schema {}",
                    projection_reference.schema_id
                )));
            }
            self.validate_value(
                projection_reference,
                &raw.projection,
                self.retained.descriptor().limits.maximum_projection_bytes,
                "Activity Projection",
            )?;
            if !matches!(input.viewer, PackViewerV1::Participant(_))
                && !raw.action_offers.is_empty()
            {
                return Err(PackFaultV1::PrivacyContract(
                    "nonparticipant view emitted Action Offers".to_owned(),
                ));
            }
            let action_offers = self.canonicalize_action_offers(raw.action_offers)?;
            let canonical_bytes = canonical_view_bytes(
                input.core,
                input.viewer,
                &raw.projection_schema,
                &raw.projection,
                &action_offers,
            )?;
            let complete_view = CanonicalJsonV1::from_canonical_bytes(&canonical_bytes)
                .map_err(canonical_pack_fault)?;
            validate_canonical_bounds(
                &complete_view,
                self.retained.descriptor().limits,
                "complete authorized view",
            )?;
            enforce_byte_bound(
                canonical_bytes.len(),
                self.retained.descriptor().limits.maximum_projection_bytes,
                "complete Activity Projection",
            )?;
            Ok(ValidatedPackViewV1 {
                viewer: input.viewer.clone(),
                complete_head: input.complete_head.clone(),
                projection_schema: raw.projection_schema,
                projection: raw.projection,
                action_offers,
                canonical_bytes: Arc::from(canonical_bytes),
            })
        })()
        .map_err(|fault| operation_fault(ActivityPackOperationV1::View, fault))
    }

    /// Checks exact current Action Offer membership and payload schema without
    /// invoking the reducer.
    ///
    /// # Errors
    ///
    /// Returns a stable admission fault for a wrong viewer/Head, missing
    /// Action, ineligible semantic time, wrong payload digest, or invalid
    /// payload. On error the reduce callback has not run.
    pub fn pre_admit_action(
        &self,
        action: &ParticipantActionV1,
        current_view: &ValidatedPackViewV1,
        basis_complete_head: &CompleteHeadV1,
    ) -> Result<(), ActivityPackReduceErrorV1> {
        if current_view.viewer != PackViewerV1::Participant(action.member_id.clone()) {
            return Err(
                ActionAdmissionErrorV1::ActionNotAllowed(action.action_type.clone()).into(),
            );
        }
        if current_view.complete_head != action.exact_basis_head
            || current_view.complete_head != *basis_complete_head
        {
            return Err(ActionAdmissionErrorV1::StaleBasis.into());
        }
        let offer = current_view
            .action_offers
            .offers()
            .iter()
            .find(|offer| offer.action_type == action.action_type)
            .ok_or_else(|| ActionAdmissionErrorV1::ActionNotAllowed(action.action_type.clone()))?;
        let definition = self
            .retained
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == action.action_type)
            .ok_or_else(|| ActionAdmissionErrorV1::ActionNotAllowed(action.action_type.clone()))?;
        if action.payload_schema_digest != offer.payload_schema_digest
            || action.payload_schema_digest != definition.payload_schema.schema_digest
        {
            return Err(ActionAdmissionErrorV1::InvalidPayload(
                "payload schema digest disagrees with the exact offer".to_owned(),
            )
            .into());
        }
        if let Some(window) = &offer.eligibility_window {
            if compare_timestamp_text(action.admitted_at.as_str(), window.opens_at.as_str())
                == Ordering::Less
            {
                return Err(ActionAdmissionErrorV1::NotOpen.into());
            }
            if compare_timestamp_text(action.admitted_at.as_str(), window.deadline.as_str())
                != Ordering::Less
            {
                return Err(ActionAdmissionErrorV1::DeadlinePassed.into());
            }
        }
        self.validate_value(
            &definition.payload_schema,
            &action.canonical_payload,
            self.retained.descriptor().limits.maximum_state_bytes,
            "Action payload",
        )
        .map_err(|error| ActionAdmissionErrorV1::InvalidPayload(error.to_string()).into())
    }

    /// Invokes the exact reducer only after all host-controlled input checks,
    /// then validates the complete disposition before returning it to Core.
    ///
    /// # Errors
    ///
    /// Any input, callback, panic, declaration, schema, ordering, authorization,
    /// or bound failure returns without a committable disposition.
    pub(crate) fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        basis_complete_head: &CompleteHeadV1,
        room_seed: &RoomSeedV1,
        participant_view: Option<&ValidatedPackViewV1>,
    ) -> Result<ActivityDispositionV1, ActivityPackReduceErrorV1> {
        self.reduce_with_invocation_hook(
            input,
            basis_complete_head,
            room_seed,
            participant_view,
            || {},
        )
    }

    pub(crate) fn reduce_with_invocation_hook<F>(
        &self,
        input: &ActivityReduceInputV1<'_>,
        basis_complete_head: &CompleteHeadV1,
        room_seed: &RoomSeedV1,
        participant_view: Option<&ValidatedPackViewV1>,
        on_invoke: F,
    ) -> Result<ActivityDispositionV1, ActivityPackReduceErrorV1>
    where
        F: FnOnce(),
    {
        self.validate_bound_head(
            input.core_before,
            input.prior_activity_state,
            basis_complete_head,
        )?;
        if basis_complete_head
            .room_seq()
            .checked_successor()
            .map_err(|_| {
                PackFaultV1::InvalidOutput("basis Room sequence cannot advance".to_owned())
            })?
            != input.next_room_seq
        {
            return Err(PackFaultV1::InvalidOutput(
                "reduce next Room sequence is not the basis successor".to_owned(),
            )
            .into());
        }
        self.validate_runtime_roles(input.core_before)?;
        self.validate_runtime_roles(input.proposed_core_after)?;
        self.validate_value(
            &self.retained.descriptor().state_schema,
            input.prior_activity_state,
            self.retained.descriptor().limits.maximum_state_bytes,
            "prior Activity State",
        )?;
        self.validate_stimulus(
            input.recorded_stimulus,
            participant_view,
            basis_complete_head,
        )?;
        let cx = DeterministicContextV1::new(
            room_seed,
            &self.retained.descriptor().revision_digest,
            input.next_room_seq,
        );
        on_invoke();
        let disposition = invoke_pack(ActivityPackOperationV1::Reduce, || {
            self.retained.0.executor.reduce(input, &cx)
        })?;
        self.validate_disposition(input, &disposition)
            .map_err(|fault| operation_fault(ActivityPackOperationV1::Reduce, fault))?;
        Ok(disposition)
    }

    /// Builds before/after views, invokes `observe`, and proves that zero output
    /// is used only for a completely hidden transition.
    ///
    /// # Errors
    ///
    /// Fails closed on any view/observe callback failure, panic, schema/bound
    /// violation, visibility contradiction, or failure to reuse the exact
    /// after-view Action Offer allocation.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn observe(
        &self,
        input: &ObserveTransitionInputV1<'_>,
    ) -> Result<ActivityObservationOutcomeV1, PackFaultV1> {
        self.validate_bound_head(input.core_before, input.activity_before, input.head_before)?;
        self.validate_bound_head(input.core_after, input.activity_after, input.head_after)?;
        if input.head_before.room_id() != input.head_after.room_id()
            || input
                .head_before
                .room_seq()
                .checked_successor()
                .map_err(|_| {
                    PackFaultV1::PrivacyContract(
                        "observation basis Room sequence cannot advance".to_owned(),
                    )
                })?
                != input.head_after.room_seq()
        {
            return Err(PackFaultV1::PrivacyContract(
                "observation Heads are not one same-Room successor pair".to_owned(),
            ));
        }
        match viewer_transition(input.core_before, input.core_after, input.viewer) {
            ViewerTransitionV1::VisibilityLost => {
                return Ok(ActivityObservationOutcomeV1::VisibilityLost);
            }
            ViewerTransitionV1::Reset(after_viewer) => {
                let reset = self.view(&ViewInputV1 {
                    core: input.core_after,
                    activity_state: input.activity_after,
                    complete_head: input.head_after,
                    viewer: &after_viewer,
                })?;
                return Ok(ActivityObservationOutcomeV1::ProjectionReset(Box::new(
                    reset,
                )));
            }
            ViewerTransitionV1::Incremental => {}
        }
        let before = self.view(&ViewInputV1 {
            core: input.core_before,
            activity_state: input.activity_before,
            complete_head: input.head_before,
            viewer: input.viewer,
        })?;
        let after = self.view(&ViewInputV1 {
            core: input.core_after,
            activity_state: input.activity_after,
            complete_head: input.head_after,
            viewer: input.viewer,
        })?;
        let raw = invoke_pack(ActivityPackOperationV1::Observe, || {
            self.retained.0.executor.observe(&ObserveInputV1 {
                core_before: input.core_before,
                activity_before: input.activity_before,
                core_after: input.core_after,
                activity_after: input.activity_after,
                recorded_stimulus: input.recorded_stimulus,
                ordered_domain_events: input.ordered_domain_events,
                viewer: input.viewer,
                after_view: &after,
            })
        })?;
        (|| {
            let view_changed = before.canonical_bytes != after.canonical_bytes;
            let Some(raw) = raw else {
                if view_changed {
                    return Err(PackFaultV1::PrivacyContract(
                        "observe returned None for a changed authorized view".to_owned(),
                    ));
                }
                return Ok(ActivityObservationOutcomeV1::Hidden);
            };
            let observation_reference =
                self.viewer_observation_schema(input.core_after, input.viewer)?;
            if raw.observation_schema != observation_reference.schema_id {
                return Err(PackFaultV1::PrivacyContract(format!(
                    "viewer requires observation schema {}",
                    observation_reference.schema_id
                )));
            }
            self.validate_value(
                observation_reference,
                &raw.observation,
                self.retained.descriptor().limits.maximum_observation_bytes,
                "Activity observation",
            )?;
            let offers_changed =
                before.action_offers.canonical_bytes != after.action_offers.canonical_bytes;
            match (offers_changed, &raw.action_offers) {
                (true, Some(offers)) if offers.shares_storage_with(&after.action_offers) => {}
                (true, _) => {
                    return Err(PackFaultV1::PrivacyContract(
                        "changed offers did not reuse the exact supplied after-view bytes"
                            .to_owned(),
                    ));
                }
                (false, None) => {}
                (false, Some(_)) => {
                    return Err(PackFaultV1::PrivacyContract(
                        "unchanged offers were redundantly re-emitted".to_owned(),
                    ));
                }
            }
            let canonical_bytes = canonical_observation_bytes(
                &raw.observation_schema,
                &raw.observation,
                raw.action_offers.as_ref(),
            )?;
            let complete_observation = CanonicalJsonV1::from_canonical_bytes(&canonical_bytes)
                .map_err(canonical_pack_fault)?;
            validate_canonical_bounds(
                &complete_observation,
                self.retained.descriptor().limits,
                "complete authorized observation",
            )?;
            enforce_byte_bound(
                canonical_bytes.len(),
                self.retained.descriptor().limits.maximum_observation_bytes,
                "complete Activity observation",
            )?;
            Ok(ActivityObservationOutcomeV1::Observation(
                ValidatedPackObservationV1 {
                    observation_schema: raw.observation_schema,
                    observation: raw.observation,
                    action_offers: raw.action_offers,
                    canonical_bytes: Arc::from(canonical_bytes),
                },
            ))
        })()
        .map_err(|fault| operation_fault(ActivityPackOperationV1::Observe, fault))
    }

    fn validate_bound_head(
        &self,
        core: &CoreRoomStateV1,
        activity_state: &CanonicalJsonV1,
        head: &CompleteHeadV1,
    ) -> Result<(), PackFaultV1> {
        let pack_digest = &self.retained.descriptor().revision_digest;
        let core_hash = hash_core_state(core).map_err(canonical_pack_fault)?;
        let activity_hash =
            hash_activity_state(pack_digest, activity_state).map_err(canonical_pack_fault)?;
        let authoritative_hash = hash_authoritative_state(pack_digest, &core_hash, &activity_hash)
            .map_err(canonical_pack_fault)?;
        if head.pack_digest() != pack_digest
            || head.core_schema_version() != crate::CORE_SCHEMA_VERSION
            || head.core_state_hash() != &core_hash
            || head.activity_state_hash() != &activity_hash
            || head.authoritative_state_hash() != &authoritative_hash
        {
            return Err(PackFaultV1::PrivacyContract(
                "view Core/Activity values do not match the complete Head".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_roles(&self, core: &CoreRoomStateV1) -> Result<(), PackFaultV1> {
        let descriptor = self.retained.descriptor();
        let mut counts: BTreeMap<&str, u32> = descriptor
            .roles
            .iter()
            .map(|role| (role.role.as_str(), 0))
            .collect();
        for membership in core.memberships().values() {
            if membership.standing() == MembershipStandingV1::Departed
                || membership.access_mode() != AccessModeV1::Participant
            {
                continue;
            }
            let role = membership.role().ok_or_else(|| {
                PackFaultV1::InvalidOutput("participant Membership has no Role".to_owned())
            })?;
            let count = counts.get_mut(role).ok_or_else(|| {
                PackFaultV1::InvalidOutput(format!("undeclared participant Role {role}"))
            })?;
            *count = count.saturating_add(1);
        }
        for definition in &descriptor.roles {
            let count = counts[definition.role.as_str()];
            if count < definition.minimum || count > definition.maximum {
                return Err(PackFaultV1::InvalidOutput(format!(
                    "Role {} cardinality {count} is outside {}..={}",
                    definition.role, definition.minimum, definition.maximum
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_runtime_roles(&self, core: &CoreRoomStateV1) -> Result<(), PackFaultV1> {
        let descriptor = self.retained.descriptor();
        let lobby_retains_departed_role_minima =
            crate::agent_heist_lobby::LOBBY_RETAINS_DEPARTED_ROLE_MINIMA
                && descriptor.pack_id == "worldstream.agent-heist"
                && (descriptor.explanatory_version == crate::AGENT_HEIST_AGENT_READY_VERSION
                    || descriptor.explanatory_version == crate::AGENT_HEIST_LOBBY_VERSION
                    || (crate::agent_heist_lobby_v3::LOBBY_RETAINS_DEPARTED_ROLE_MINIMA
                        && descriptor.explanatory_version
                            == crate::AGENT_HEIST_CLOCK_SAFE_VERSION))
                && descriptor
                    .stimulus_schemas
                    .contains_key(crate::HOST_LAUNCH_INPUT_TYPE);
        if !lobby_retains_departed_role_minima {
            return self.validate_roles(core);
        }

        let mut active_counts: BTreeMap<&str, u32> = descriptor
            .roles
            .iter()
            .map(|role| (role.role.as_str(), 0))
            .collect();
        let mut retained_counts = active_counts.clone();
        for membership in core.memberships().values() {
            if membership.access_mode() != AccessModeV1::Participant {
                continue;
            }
            let role = membership.role().ok_or_else(|| {
                PackFaultV1::InvalidOutput("participant Membership has no Role".to_owned())
            })?;
            let retained = retained_counts.get_mut(role).ok_or_else(|| {
                PackFaultV1::InvalidOutput(format!("undeclared participant Role {role}"))
            })?;
            *retained = retained.saturating_add(1);
            if membership.standing() != MembershipStandingV1::Departed {
                let active = active_counts
                    .get_mut(role)
                    .unwrap_or_else(|| unreachable!("retained Role was already validated"));
                *active = active.saturating_add(1);
            }
        }
        for definition in &descriptor.roles {
            let active = active_counts[definition.role.as_str()];
            let retained = retained_counts[definition.role.as_str()];
            if retained < definition.minimum || active > definition.maximum {
                return Err(PackFaultV1::InvalidOutput(format!(
                    "Role {} retained/active cardinality {retained}/{active} is outside minimum {} and maximum {}",
                    definition.role, definition.minimum, definition.maximum
                )));
            }
        }
        Ok(())
    }

    fn viewer_projection_schema(
        &self,
        core: &CoreRoomStateV1,
        viewer: &PackViewerV1,
    ) -> Result<&SchemaReferenceV1, PackFaultV1> {
        let key = Self::authorize_viewer(core, viewer)?;
        self.retained
            .descriptor()
            .projection_schemas
            .get(&key)
            .ok_or_else(|| {
                PackFaultV1::PrivacyContract(format!(
                    "descriptor has no projection schema for viewer class {key:?}"
                ))
            })
    }

    fn viewer_observation_schema(
        &self,
        core: &CoreRoomStateV1,
        viewer: &PackViewerV1,
    ) -> Result<&SchemaReferenceV1, PackFaultV1> {
        let key = Self::authorize_viewer(core, viewer)?;
        self.retained
            .descriptor()
            .observation_schemas
            .get(&key)
            .ok_or_else(|| {
                PackFaultV1::PrivacyContract(format!(
                    "descriptor has no observation schema for viewer class {key:?}"
                ))
            })
    }

    fn authorize_viewer(
        core: &CoreRoomStateV1,
        viewer: &PackViewerV1,
    ) -> Result<PackViewerClassV1, PackFaultV1> {
        let membership = core.membership(viewer.member_id()).ok_or_else(|| {
            PackFaultV1::PrivacyContract("viewer Membership is absent".to_owned())
        })?;
        if membership.standing() != MembershipStandingV1::Enabled {
            return Err(PackFaultV1::PrivacyContract(
                "viewer Membership is not enabled".to_owned(),
            ));
        }
        match (viewer, membership.access_mode()) {
            (PackViewerV1::Public(_), AccessModeV1::Spectator) => Ok(PackViewerClassV1::Public),
            (PackViewerV1::Participant(_), AccessModeV1::Participant) => {
                Ok(PackViewerClassV1::Participant)
            }
            (PackViewerV1::Operator(_), AccessModeV1::Operator) => Ok(PackViewerClassV1::Operator),
            (PackViewerV1::Historical(_), AccessModeV1::Spectator) => {
                Ok(PackViewerClassV1::HistoricalPublic)
            }
            (PackViewerV1::Historical(_), AccessModeV1::Participant) => {
                Ok(PackViewerClassV1::HistoricalParticipant)
            }
            (PackViewerV1::Historical(_), AccessModeV1::Operator) => {
                Ok(PackViewerClassV1::HistoricalOperator)
            }
            (PackViewerV1::FinalReveal(_), _) => Ok(PackViewerClassV1::FinalReveal),
            _ => Err(PackFaultV1::PrivacyContract(
                "typed viewer disagrees with Membership Access Mode".to_owned(),
            )),
        }
    }

    fn canonicalize_action_offers(
        &self,
        offers: Vec<ActionOfferV1>,
    ) -> Result<CanonicalActionOffersV1, PackFaultV1> {
        let descriptor = self.retained.descriptor();
        enforce_count_bound(
            offers.len(),
            descriptor.limits.maximum_collection_items,
            "Action Offers",
        )?;
        let order: BTreeMap<&str, usize> = descriptor
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| (action.action_type.as_str(), index))
            .collect();
        let mut previous = None;
        for offer in &offers {
            if offer.domain != ACTION_OFFER_DOMAIN {
                return Err(PackFaultV1::InvalidOutput(
                    "Action Offer domain is not v1".to_owned(),
                ));
            }
            let index = order
                .get(offer.action_type.as_str())
                .copied()
                .ok_or_else(|| {
                    PackFaultV1::InvalidOutput(format!(
                        "Action Offer uses undeclared type {}",
                        offer.action_type
                    ))
                })?;
            let definition = &descriptor.actions[index];
            if offer.payload_schema_digest != definition.payload_schema.schema_digest {
                return Err(PackFaultV1::ActionPayloadSchemaMismatch);
            }
            if previous.is_some_and(|previous| previous >= index) {
                return Err(PackFaultV1::InvalidOutput(
                    "Action Offers are duplicated or outside descriptor order".to_owned(),
                ));
            }
            previous = Some(index);
            if let Some(window) = &offer.eligibility_window
                && compare_timestamp_text(window.opens_at.as_str(), window.deadline.as_str())
                    != Ordering::Less
            {
                return Err(PackFaultV1::InvalidOutput(
                    "Action Offer window is not a nonempty half-open interval".to_owned(),
                ));
            }
        }
        let canonical_bytes = encode(&offers).map_err(canonical_pack_fault)?;
        let canonical_value = CanonicalJsonV1::from_canonical_bytes(&canonical_bytes)
            .map_err(canonical_pack_fault)?;
        validate_canonical_bounds(&canonical_value, descriptor.limits, "Action Offers")?;
        Ok(CanonicalActionOffersV1 {
            offers: Arc::from(offers),
            canonical_bytes: Arc::from(canonical_bytes),
        })
    }

    fn validate_stimulus(
        &self,
        stimulus: &RecordedStimulusV1,
        participant_view: Option<&ValidatedPackViewV1>,
        basis_complete_head: &CompleteHeadV1,
    ) -> Result<(), ActivityPackReduceErrorV1> {
        match stimulus {
            RecordedStimulusV1::ParticipantAction(action) => self.pre_admit_action(
                action,
                participant_view.ok_or_else(|| {
                    ActionAdmissionErrorV1::ActionNotAllowed(action.action_type.clone())
                })?,
                basis_complete_head,
            ),
            RecordedStimulusV1::TimerFired(timer) => self
                .validate_declared_stimulus_payload("timer_fired", &timer.canonical_payload)
                .map_err(Into::into),
            RecordedStimulusV1::ExternalInput(external) => self
                .validate_declared_stimulus_payload(
                    &external.input_type,
                    &external.canonical_payload,
                )
                .map_err(Into::into),
            RecordedStimulusV1::CoreProposed(_) => Ok(()),
        }
    }

    fn validate_declared_stimulus_payload(
        &self,
        stimulus_type: &str,
        payload: &CanonicalJsonV1,
    ) -> Result<(), PackFaultV1> {
        let reference = self
            .retained
            .descriptor()
            .stimulus_schemas
            .get(stimulus_type)
            .ok_or_else(|| {
                PackFaultV1::SchemaViolation(format!("undeclared stimulus schema {stimulus_type}"))
            })?;
        self.validate_value(
            reference,
            payload,
            self.retained.descriptor().limits.maximum_state_bytes,
            "Stimulus payload",
        )
    }

    #[allow(clippy::too_many_lines)]
    fn validate_disposition(
        &self,
        input: &ActivityReduceInputV1<'_>,
        disposition: &ActivityDispositionV1,
    ) -> Result<(), PackFaultV1> {
        let descriptor = self.retained.descriptor();
        match disposition {
            ActivityDispositionV1::Reject(rejection) => {
                if !clean_rejection_is_allowed(input.recorded_stimulus) {
                    return Err(PackFaultV1::MandatoryStimulusRejected);
                }
                if !descriptor
                    .rejection_codes
                    .contains(&rejection.declared_code)
                {
                    return Err(PackFaultV1::UndeclaredRejectionCode(
                        rejection.declared_code.clone(),
                    ));
                }
                let key = format!("rejection:{}", rejection.declared_code);
                let reference = descriptor.output_schemas.get(&key).ok_or_else(|| {
                    PackFaultV1::SchemaViolation(format!("undeclared output schema {key}"))
                })?;
                self.validate_value(
                    reference,
                    &rejection.bounded_safe_details,
                    descriptor.limits.maximum_observation_bytes,
                    "rejection details",
                )
            }
            ActivityDispositionV1::Apply(apply) => {
                self.validate_value(
                    &descriptor.state_schema,
                    &apply.next_activity_state,
                    descriptor.limits.maximum_state_bytes,
                    "next Activity State",
                )?;
                enforce_count_bound(
                    apply.ordered_domain_events.len(),
                    descriptor.limits.maximum_events,
                    "Domain Events",
                )?;
                for event in &apply.ordered_domain_events {
                    let event_type = canonical_object_string(event, "event_type", "Domain Event")?;
                    let reference = descriptor.event_schemas.get(&event_type).ok_or_else(|| {
                        PackFaultV1::SchemaViolation(format!(
                            "undeclared Domain Event schema {event_type}"
                        ))
                    })?;
                    self.validate_value(
                        reference,
                        event,
                        descriptor.limits.maximum_state_bytes,
                        "Domain Event",
                    )?;
                }
                self.validate_timer_requests(
                    &apply.timer_requests,
                    input.recorded_stimulus.semantic_time(),
                )?;
                enforce_count_bound(
                    apply.ordered_attention_signals.len(),
                    descriptor.limits.maximum_attention_signals,
                    "Attention Signals",
                )?;
                let mut targets = BTreeSet::new();
                for attention in &apply.ordered_attention_signals {
                    let reason = canonical_object_string(attention, "reason", "Attention Signal")?;
                    if !descriptor
                        .attention_reasons
                        .iter()
                        .any(|item| item == &reason)
                    {
                        return Err(PackFaultV1::InvalidOutput(format!(
                            "undeclared Attention reason {reason}"
                        )));
                    }
                    let target =
                        canonical_object_string(attention, "target_member_id", "Attention Signal")?;
                    if !targets.insert(target.clone()) {
                        return Err(PackFaultV1::InvalidOutput(
                            "multiple Attention Signals target one Membership".to_owned(),
                        ));
                    }
                    let target: MemberId = target.parse().map_err(|_| {
                        PackFaultV1::InvalidOutput(
                            "Attention target is not a canonical Member ID".to_owned(),
                        )
                    })?;
                    let membership =
                        input
                            .proposed_core_after
                            .membership(&target)
                            .ok_or_else(|| {
                                PackFaultV1::InvalidOutput(
                                    "Attention target Membership is absent".to_owned(),
                                )
                            })?;
                    if membership.standing() != MembershipStandingV1::Enabled
                        || membership.access_mode() != AccessModeV1::Participant
                        || membership.principal_kind() != PrincipalKindV1::Agent
                    {
                        return Err(PackFaultV1::InvalidOutput(
                            "Attention target is not an enabled Agent participant".to_owned(),
                        ));
                    }
                    let key = format!("attention:{reason}");
                    let reference = descriptor.output_schemas.get(&key).ok_or_else(|| {
                        PackFaultV1::SchemaViolation(format!("undeclared output schema {key}"))
                    })?;
                    self.validate_value(
                        reference,
                        attention,
                        descriptor.limits.maximum_observation_bytes,
                        "Attention Signal",
                    )?;
                }
                Ok(())
            }
        }
    }

    fn validate_timer_requests(
        &self,
        requests: &[TimerRequestV1],
        semantic_time: &str,
    ) -> Result<(), PackFaultV1> {
        enforce_count_bound(
            requests.len(),
            self.retained.descriptor().limits.maximum_timer_requests,
            "Timer requests",
        )?;
        let mut timer_ids = BTreeSet::new();
        for request in requests {
            if !timer_ids.insert(request.timer_id()) {
                return Err(PackFaultV1::InvalidTimerOutput(
                    "duplicate logical Timer ID".to_owned(),
                ));
            }
            match request {
                TimerRequestV1::ScheduleNext {
                    due,
                    canonical_payload,
                    ..
                } => {
                    self.validate_timer_payload(canonical_payload)?;
                    validate_due_after(due.as_str(), semantic_time)?;
                }
                TimerRequestV1::CancelCurrent { .. } => {}
                TimerRequestV1::RescheduleCurrent {
                    new_due,
                    new_canonical_payload,
                    ..
                } => {
                    self.validate_timer_payload(new_canonical_payload)?;
                    validate_due_after(new_due.as_str(), semantic_time)?;
                }
            }
        }
        Ok(())
    }

    fn validate_timer_payload(&self, payload: &CanonicalJsonV1) -> Result<(), PackFaultV1> {
        let reference = self
            .retained
            .descriptor()
            .output_schemas
            .get("timer_request")
            .ok_or_else(|| {
                PackFaultV1::SchemaViolation(
                    "descriptor has no timer_request output schema".to_owned(),
                )
            })?;
        self.validate_value(
            reference,
            payload,
            self.retained.descriptor().limits.maximum_state_bytes,
            "Timer payload",
        )
    }

    fn normalize_initial_timers(
        &self,
        requests: Vec<TimerRequestV1>,
        created_at: &str,
    ) -> Result<Vec<ScheduledTimerV1>, PackFaultV1> {
        self.validate_timer_requests(&requests, created_at)?;
        let generation = TimerGenerationV1::new(1)
            .unwrap_or_else(|_| unreachable!("one is a valid Timer generation"));
        let mut timers = Vec::with_capacity(requests.len());
        for request in requests {
            let TimerRequestV1::ScheduleNext {
                timer_id,
                due,
                canonical_payload,
            } = request
            else {
                return Err(PackFaultV1::InvalidTimerOutput(
                    "Genesis may only schedule first Timer generations".to_owned(),
                ));
            };
            timers.push(ScheduledTimerV1 {
                timer_id,
                generation,
                scheduled_for: due,
                canonical_payload,
            });
        }
        timers.sort_by(|left, right| left.timer_id.cmp(&right.timer_id));
        Ok(timers)
    }

    fn validate_value(
        &self,
        reference: &SchemaReferenceV1,
        value: &CanonicalJsonV1,
        maximum_bytes: u32,
        label: &str,
    ) -> Result<(), PackFaultV1> {
        self.retained.schemas().validate_value(reference, value)?;
        validate_canonical_bounds(value, self.retained.descriptor().limits, label)?;
        let bytes = value.to_bytes().map_err(canonical_pack_fault)?;
        enforce_byte_bound(bytes.len(), maximum_bytes, label)
    }
}

fn invoke_pack<T>(
    operation: ActivityPackOperationV1,
    callback: impl FnOnce() -> Result<T, PackFaultV1>,
) -> Result<T, PackFaultV1> {
    catch_unwind(AssertUnwindSafe(callback))
        .map_err(|_| PackFaultV1::OperationPanicked(operation))?
        .map_err(|fault| operation_fault(operation, fault))
}

fn operation_fault(operation: ActivityPackOperationV1, fault: PackFaultV1) -> PackFaultV1 {
    PackFaultV1::OperationFault {
        operation,
        fault: Box::new(fault),
    }
}

fn clean_rejection_is_allowed(stimulus: &RecordedStimulusV1) -> bool {
    match stimulus {
        RecordedStimulusV1::ParticipantAction(_) => true,
        RecordedStimulusV1::CoreProposed(proposal) => match proposal.kind() {
            crate::CoreProposedKindV1::Join
            | crate::CoreProposedKindV1::Resume
            | crate::CoreProposedKindV1::AccessModeChange
            | crate::CoreProposedKindV1::RoleChange => true,
            crate::CoreProposedKindV1::MembershipChangeSet => {
                let changes = proposal.changeset().membership_changes();
                !changes.is_empty()
                    && changes.iter().all(|change| {
                        matches!(
                            change.kind(),
                            MembershipChangeKindV1::Join
                                | MembershipChangeKindV1::Resume
                                | MembershipChangeKindV1::AccessModeChange
                                | MembershipChangeKindV1::RoleChange
                        )
                    })
            }
            crate::CoreProposedKindV1::Archive
            | crate::CoreProposedKindV1::Suspend
            | crate::CoreProposedKindV1::Depart => false,
        },
        RecordedStimulusV1::TimerFired(_) | RecordedStimulusV1::ExternalInput(_) => false,
    }
}

fn validate_due_after(due: &str, semantic_time: &str) -> Result<(), PackFaultV1> {
    if compare_timestamp_text(due, semantic_time) != Ordering::Greater {
        return Err(PackFaultV1::InvalidTimerOutput(
            "new due time is not strictly after semantic time".to_owned(),
        ));
    }
    Ok(())
}

fn enforce_count_bound(actual: usize, maximum: u32, label: &str) -> Result<(), PackFaultV1> {
    if actual > maximum as usize {
        return Err(PackFaultV1::OutputBoundExceeded(format!(
            "{label} count {actual} exceeds {maximum}"
        )));
    }
    Ok(())
}

fn enforce_byte_bound(actual: usize, maximum: u32, label: &str) -> Result<(), PackFaultV1> {
    if actual > maximum as usize {
        return Err(PackFaultV1::OutputBoundExceeded(format!(
            "{label} bytes {actual} exceeds {maximum}"
        )));
    }
    Ok(())
}

fn validate_canonical_bounds(
    value: &CanonicalJsonV1,
    limits: PackLimitsV1,
    label: &str,
) -> Result<(), PackFaultV1> {
    let value = serde_json::to_value(value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    validate_json_bounds(&value, limits, 1, label)
}

fn validate_json_bounds(
    value: &serde_json::Value,
    limits: PackLimitsV1,
    depth: u32,
    label: &str,
) -> Result<(), PackFaultV1> {
    if depth > limits.maximum_nesting {
        return Err(PackFaultV1::OutputBoundExceeded(format!(
            "{label} nesting exceeds {}",
            limits.maximum_nesting
        )));
    }
    match value {
        serde_json::Value::String(text) => {
            enforce_byte_bound(text.len(), limits.maximum_text_bytes, label)
        }
        serde_json::Value::Array(items) => {
            enforce_count_bound(items.len(), limits.maximum_collection_items, label)?;
            for item in items {
                validate_json_bounds(item, limits, depth + 1, label)?;
            }
            Ok(())
        }
        serde_json::Value::Object(fields) => {
            enforce_count_bound(fields.len(), limits.maximum_collection_items, label)?;
            for (key, child) in fields {
                enforce_byte_bound(key.len(), limits.maximum_text_bytes, label)?;
                validate_json_bounds(child, limits, depth + 1, label)?;
            }
            Ok(())
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            Ok(())
        }
    }
}

fn canonical_object_string(
    value: &CanonicalJsonV1,
    field: &str,
    label: &str,
) -> Result<String, PackFaultV1> {
    let value = serde_json::to_value(value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| PackFaultV1::InvalidOutput(format!("{label} lacks string {field}")))
}

fn canonical_view_bytes(
    core: &CoreRoomStateV1,
    viewer: &PackViewerV1,
    projection_schema: &str,
    projection: &CanonicalJsonV1,
    action_offers: &CanonicalActionOffersV1,
) -> Result<Vec<u8>, PackFaultV1> {
    let membership = core
        .membership(viewer.member_id())
        .ok_or_else(|| PackFaultV1::PrivacyContract("viewer Membership is absent".to_owned()))?;
    let authorized_core = encode(&AuthorizedCoreViewV1 {
        access_mode: membership.access_mode(),
        role: membership.role(),
        room_status: core.room_status(),
        standing: membership.standing(),
        viewer_class: viewer_class(viewer),
    })
    .map_err(canonical_pack_fault)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{\"action_offers\":");
    bytes.extend_from_slice(action_offers.canonical_bytes());
    bytes.extend_from_slice(b",\"authorized_core\":");
    bytes.extend_from_slice(&authorized_core);
    bytes.extend_from_slice(b",\"projection\":");
    bytes.extend_from_slice(&projection.to_bytes().map_err(canonical_pack_fault)?);
    bytes.extend_from_slice(b",\"projection_schema\":");
    bytes.extend_from_slice(&encode(&projection_schema).map_err(canonical_pack_fault)?);
    bytes.push(b'}');
    Ok(bytes)
}

#[derive(Serialize)]
struct AuthorizedCoreViewV1<'a> {
    access_mode: AccessModeV1,
    role: Option<&'a str>,
    room_status: crate::RoomStatusV1,
    standing: MembershipStandingV1,
    viewer_class: &'static str,
}

enum ViewerTransitionV1 {
    Incremental,
    Reset(PackViewerV1),
    VisibilityLost,
}

fn viewer_transition(
    core_before: &CoreRoomStateV1,
    core_after: &CoreRoomStateV1,
    viewer: &PackViewerV1,
) -> ViewerTransitionV1 {
    let before = core_before.membership(viewer.member_id());
    let after = core_after.membership(viewer.member_id());
    let before_enabled =
        before.is_some_and(|membership| membership.standing() == MembershipStandingV1::Enabled);
    let after_enabled =
        after.is_some_and(|membership| membership.standing() == MembershipStandingV1::Enabled);
    if !after_enabled {
        return ViewerTransitionV1::VisibilityLost;
    }
    let after = after.unwrap_or_else(|| unreachable!("enabled Membership exists"));
    let access_changed =
        before_enabled && before.is_some_and(|before| before.access_mode() != after.access_mode());
    if !before_enabled || access_changed {
        let member_id = viewer.member_id().clone();
        let after_viewer = match viewer {
            PackViewerV1::Historical(_) => PackViewerV1::Historical(member_id),
            PackViewerV1::FinalReveal(_) => PackViewerV1::FinalReveal(member_id),
            PackViewerV1::Public(_) | PackViewerV1::Participant(_) | PackViewerV1::Operator(_) => {
                match after.access_mode() {
                    AccessModeV1::Participant => PackViewerV1::Participant(member_id),
                    AccessModeV1::Spectator => PackViewerV1::Public(member_id),
                    AccessModeV1::Operator => PackViewerV1::Operator(member_id),
                }
            }
        };
        return ViewerTransitionV1::Reset(after_viewer);
    }
    ViewerTransitionV1::Incremental
}

const fn viewer_class(viewer: &PackViewerV1) -> &'static str {
    match viewer {
        PackViewerV1::Public(_) => "public",
        PackViewerV1::Participant(_) => "participant",
        PackViewerV1::Operator(_) => "operator",
        PackViewerV1::Historical(_) => "historical",
        PackViewerV1::FinalReveal(_) => "final_reveal",
    }
}

fn canonical_observation_bytes(
    observation_schema: &str,
    observation: &CanonicalJsonV1,
    action_offers: Option<&CanonicalActionOffersV1>,
) -> Result<Vec<u8>, PackFaultV1> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{\"action_offers\":");
    match action_offers {
        Some(offers) => bytes.extend_from_slice(offers.canonical_bytes()),
        None => bytes.extend_from_slice(b"null"),
    }
    bytes.extend_from_slice(b",\"observation\":");
    bytes.extend_from_slice(&observation.to_bytes().map_err(canonical_pack_fault)?);
    bytes.extend_from_slice(b",\"observation_schema\":");
    bytes.extend_from_slice(&encode(&observation_schema).map_err(canonical_pack_fault)?);
    bytes.push(b'}');
    Ok(bytes)
}

fn canonical_golden_disposition(
    disposition: &ActivityDispositionV1,
) -> Result<CanonicalJsonV1, CanonicalJsonError> {
    CanonicalJsonV1::from_serialize(disposition)
}

#[derive(Serialize)]
struct GoldenObservationOutcomeV1<'a> {
    outcome: &'static str,
    value: Option<&'a CanonicalJsonV1>,
}

fn canonical_golden_observation(
    outcome: &ActivityObservationOutcomeV1,
) -> Result<CanonicalJsonV1, CanonicalJsonError> {
    let owned;
    let (kind, value) = match outcome {
        ActivityObservationOutcomeV1::Hidden => ("hidden", None),
        ActivityObservationOutcomeV1::Observation(observation) => {
            owned = CanonicalJsonV1::from_canonical_bytes(observation.canonical_bytes())?;
            ("observation", Some(&owned))
        }
        ActivityObservationOutcomeV1::ProjectionReset(view) => {
            owned = CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())?;
            ("projection_reset", Some(&owned))
        }
        ActivityObservationOutcomeV1::VisibilityLost => ("visibility_lost", None),
    };
    CanonicalJsonV1::from_serialize(&GoldenObservationOutcomeV1 {
        outcome: kind,
        value,
    })
}

#[derive(Serialize)]
struct GoldenViewTranscriptV1<'a> {
    viewer: &'a PackGoldenViewerV1,
    outcome: &'static str,
    view: Option<CanonicalJsonV1>,
}

#[derive(Serialize)]
struct GoldenObservationTranscriptV1<'a> {
    viewer: &'a PackGoldenViewerV1,
    observation: CanonicalJsonV1,
}

#[derive(Serialize)]
struct GoldenStepTranscriptV1<'a> {
    action_id: &'a str,
    before_views: Vec<GoldenViewTranscriptV1<'a>>,
    disposition: CanonicalJsonV1,
    transition: crate::TransitionV1,
    observations: Vec<GoldenObservationTranscriptV1<'a>>,
    after_views: Vec<GoldenViewTranscriptV1<'a>>,
}

#[derive(Serialize)]
struct PackGoldenTranscriptV1<'a> {
    domain: &'static str,
    genesis: &'a GenesisInputV1,
    initial_views: Vec<GoldenViewTranscriptV1<'a>>,
    steps: Vec<GoldenStepTranscriptV1<'a>>,
}

fn execute_golden_views<'a>(
    host: &ActivityPackHostV1,
    core: &CoreRoomStateV1,
    activity_state: &CanonicalJsonV1,
    head: &CompleteHeadV1,
    viewers: &'a [PackGoldenViewerV1],
    checkpoint: u32,
) -> Result<
    Vec<(
        PackViewerV1,
        Option<ValidatedPackViewV1>,
        GoldenViewTranscriptV1<'a>,
    )>,
    (),
> {
    viewers
        .iter()
        .map(|golden_viewer| {
            let viewer = golden_viewer.to_pack_viewer();
            if checkpoint < golden_viewer.available_after_action {
                let expected_detail = golden_viewer.denied_before_detail.as_deref().ok_or(())?;
                match host.view(&ViewInputV1 {
                    core,
                    activity_state,
                    complete_head: head,
                    viewer: &viewer,
                }) {
                    Err(PackFaultV1::PrivacyContract(detail)) if detail == expected_detail => {}
                    _ => return Err(()),
                }
                return Ok((
                    viewer,
                    None,
                    GoldenViewTranscriptV1 {
                        viewer: golden_viewer,
                        outcome: "unavailable",
                        view: None,
                    },
                ));
            }
            if golden_viewer.denied_before_detail.is_some()
                && golden_viewer.available_after_action == 0
            {
                return Err(());
            }
            let view = host
                .view(&ViewInputV1 {
                    core,
                    activity_state,
                    complete_head: head,
                    viewer: &viewer,
                })
                .map_err(|_| ())?;
            let transcript = GoldenViewTranscriptV1 {
                viewer: golden_viewer,
                outcome: "available",
                view: Some(
                    CanonicalJsonV1::from_canonical_bytes(view.canonical_bytes())
                        .map_err(|_| ())?,
                ),
            };
            Ok((viewer, Some(view), transcript))
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn execute_golden_corpus(
    host: &ActivityPackHostV1,
    corpus: &PackGoldenCorpusV1,
) -> Result<CanonicalJsonV1, ()> {
    if corpus.corpus_id != PACK_GOLDEN_CORPUS_DOMAIN
        || corpus.genesis.pack_digest != host.descriptor().revision_digest
        || corpus.viewers.is_empty()
        || (corpus.actions.is_empty() && corpus.external_inputs.is_empty())
    {
        return Err(());
    }
    let action_count =
        u32::try_from(corpus.actions.len() + corpus.external_inputs.len()).map_err(|_| ())?;
    if corpus.viewers.iter().any(|viewer| {
        // A retained corpus may defer a post-Complete FinalReveal check one
        // checkpoint beyond its Action-only transcript. The runtime still
        // enforces the viewer's denial; timer completion is covered by the
        // pack's focused transition tests.
        viewer.available_after_action > action_count.saturating_add(1)
            || (viewer.available_after_action == 0 && viewer.denied_before_detail.is_some())
            || (viewer.available_after_action > 0
                && viewer
                    .denied_before_detail
                    .as_ref()
                    .is_none_or(String::is_empty))
    }) {
        return Err(());
    }
    let covered_viewer_classes = corpus
        .viewers
        .iter()
        .map(|viewer| {
            ActivityPackHostV1::authorize_viewer(
                &corpus.genesis.initial_core_state,
                &viewer.to_pack_viewer(),
            )
            .map_err(|_| ())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if covered_viewer_classes != PackViewerClassV1::ALL.into_iter().collect() {
        return Err(());
    }
    let mut action_ids = BTreeSet::new();
    if !corpus
        .actions
        .iter()
        .all(|action| action_ids.insert(action.action_id.to_string()))
    {
        return Err(());
    }
    for (step_index, action) in corpus.actions.iter().enumerate() {
        let checkpoint = u32::try_from(step_index).map_err(|_| ())?;
        let participant_is_available = corpus.viewers.iter().any(|viewer| {
            viewer.kind == PackGoldenViewerKindV1::Participant
                && viewer.member_id == action.member_id
                && viewer.available_after_action <= checkpoint
        });
        if !participant_is_available {
            return Err(());
        }
    }
    let genesis = host.initialize(&corpus.genesis).map_err(|_| ())?;
    let dispositions = Arc::new(Mutex::new(BTreeMap::<String, ActivityDispositionV1>::new()));
    let reducer_dispositions = Arc::clone(&dispositions);
    let mut trace = CoreTraceV1::create_for_conformance(
        genesis.clone(),
        |_| Ok(()),
        move |input| {
            let operation_id = match input.recorded_stimulus {
                RecordedStimulusV1::ParticipantAction(action) => action.action_id.to_string(),
                RecordedStimulusV1::ExternalInput(external) => external.input_id.to_string(),
                _ => {
                    return Err(PackFaultV1::Callback(
                        "golden corpus contains an unsupported transition".to_owned(),
                    ));
                }
            };
            reducer_dispositions
                .lock()
                .map_err(|_| PackFaultV1::Callback("golden reducer lock poisoned".to_owned()))?
                .get(&operation_id)
                .cloned()
                .ok_or_else(|| {
                    PackFaultV1::Callback("golden disposition was not checked".to_owned())
                })
        },
    )
    .map_err(|_| ())?;
    let initial_views = execute_golden_views(
        host,
        trace.core_state(),
        trace.activity_state(),
        trace.head(),
        &corpus.viewers,
        0,
    )?
    .into_iter()
    .map(|(_, _, transcript)| transcript)
    .collect();
    let mut steps = Vec::with_capacity(corpus.actions.len() + corpus.external_inputs.len());
    for (step_index, golden_external) in corpus.external_inputs.iter().enumerate() {
        let checkpoint = u32::try_from(step_index).map_err(|_| ())?;
        let core_before = trace.core_state().clone();
        let activity_before = trace.activity_state().clone();
        let head_before = trace.head().clone();
        let before_views = execute_golden_views(
            host,
            &core_before,
            &activity_before,
            &head_before,
            &corpus.viewers,
            checkpoint,
        )?;
        let stimulus = RecordedStimulusV1::ExternalInput(golden_external.input.clone());
        let reduce_input = ActivityReduceInputV1 {
            prior_activity_state: &activity_before,
            core_before: &core_before,
            proposed_core_after: &core_before,
            scheduled_timers: trace.scheduled_timers(),
            next_room_seq: head_before.room_seq().checked_successor().map_err(|_| ())?,
            recorded_stimulus: &stimulus,
        };
        let disposition = host
            .reduce(&reduce_input, &head_before, &corpus.genesis.room_seed, None)
            .map_err(|_| ())?;
        if !matches!(disposition, ActivityDispositionV1::Apply(_)) {
            return Err(());
        }
        dispositions.lock().map_err(|_| ())?.insert(
            golden_external.input.input_id.to_string(),
            disposition.clone(),
        );
        trace.advance(stimulus.clone()).map_err(|_| ())?;
        let transition = trace.transitions().last().cloned().ok_or(())?;
        let mut observations = Vec::with_capacity(corpus.viewers.len());
        for golden_viewer in &corpus.viewers {
            let viewer = golden_viewer.to_pack_viewer();
            let outcome = if checkpoint < golden_viewer.available_after_action {
                CanonicalJsonV1::from_serialize(&GoldenObservationOutcomeV1 {
                    outcome: if checkpoint.saturating_add(1) >= golden_viewer.available_after_action
                    {
                        "became_available"
                    } else {
                        "unavailable"
                    },
                    value: None,
                })
                .map_err(|_| ())?
            } else {
                canonical_golden_observation(
                    &host
                        .observe(&ObserveTransitionInputV1 {
                            core_before: &core_before,
                            activity_before: &activity_before,
                            head_before: &head_before,
                            core_after: trace.core_state(),
                            activity_after: trace.activity_state(),
                            head_after: trace.head(),
                            recorded_stimulus: &stimulus,
                            ordered_domain_events: transition.ordered_domain_events(),
                            viewer: &viewer,
                        })
                        .map_err(|_| ())?,
                )
                .map_err(|_| ())?
            };
            observations.push(GoldenObservationTranscriptV1 {
                viewer: golden_viewer,
                observation: outcome,
            });
        }
        let after_views = execute_golden_views(
            host,
            trace.core_state(),
            trace.activity_state(),
            trace.head(),
            &corpus.viewers,
            checkpoint.checked_add(1).ok_or(())?,
        )?
        .into_iter()
        .map(|(_, _, transcript)| transcript)
        .collect();
        steps.push(GoldenStepTranscriptV1 {
            action_id: golden_external.input.input_id.as_str(),
            before_views: before_views
                .into_iter()
                .map(|(_, _, transcript)| transcript)
                .collect(),
            disposition: canonical_golden_disposition(&disposition).map_err(|_| ())?,
            transition,
            observations,
            after_views,
        });
    }
    for (step_index, golden_action) in corpus.actions.iter().enumerate() {
        let checkpoint =
            u32::try_from(step_index + corpus.external_inputs.len()).map_err(|_| ())?;
        let core_before = trace.core_state().clone();
        let activity_before = trace.activity_state().clone();
        let head_before = trace.head().clone();
        let before_views = execute_golden_views(
            host,
            &core_before,
            &activity_before,
            &head_before,
            &corpus.viewers,
            checkpoint,
        )?;
        let participant_view = before_views
            .iter()
            .find(|(viewer, _, _)| {
                matches!(viewer, PackViewerV1::Participant(member_id) if member_id == &golden_action.member_id)
            })
            .and_then(|(_, view, _)| view.as_ref())
            .ok_or(())?;
        let action = ParticipantActionV1 {
            member_id: golden_action.member_id.clone(),
            action_id: golden_action.action_id.clone(),
            action_type: golden_action.action_type.clone(),
            payload_schema_digest: golden_action.payload_schema_digest.clone(),
            canonical_payload: golden_action.canonical_payload.clone(),
            exact_basis_head: head_before.clone(),
            admitted_at: golden_action.admitted_at.clone(),
        };
        let stimulus = RecordedStimulusV1::ParticipantAction(action);
        let reduce_input = ActivityReduceInputV1 {
            prior_activity_state: &activity_before,
            core_before: &core_before,
            proposed_core_after: &core_before,
            scheduled_timers: trace.scheduled_timers(),
            next_room_seq: head_before.room_seq().checked_successor().map_err(|_| ())?,
            recorded_stimulus: &stimulus,
        };
        let disposition = host
            .reduce(
                &reduce_input,
                &head_before,
                &corpus.genesis.room_seed,
                Some(participant_view),
            )
            .map_err(|_| ())?;
        if !matches!(disposition, ActivityDispositionV1::Apply(_)) {
            return Err(());
        }
        dispositions
            .lock()
            .map_err(|_| ())?
            .insert(golden_action.action_id.to_string(), disposition.clone());
        trace.advance(stimulus.clone()).map_err(|_| ())?;
        let transition = trace.transitions().last().cloned().ok_or(())?;
        let mut observations = Vec::with_capacity(corpus.viewers.len());
        for golden_viewer in &corpus.viewers {
            let viewer = golden_viewer.to_pack_viewer();
            let outcome = if checkpoint < golden_viewer.available_after_action {
                CanonicalJsonV1::from_serialize(&GoldenObservationOutcomeV1 {
                    outcome: if checkpoint.saturating_add(1) >= golden_viewer.available_after_action
                    {
                        "became_available"
                    } else {
                        "unavailable"
                    },
                    value: None,
                })
                .map_err(|_| ())?
            } else {
                canonical_golden_observation(
                    &host
                        .observe(&ObserveTransitionInputV1 {
                            core_before: &core_before,
                            activity_before: &activity_before,
                            head_before: &head_before,
                            core_after: trace.core_state(),
                            activity_after: trace.activity_state(),
                            head_after: trace.head(),
                            recorded_stimulus: &stimulus,
                            ordered_domain_events: transition.ordered_domain_events(),
                            viewer: &viewer,
                        })
                        .map_err(|_| ())?,
                )
                .map_err(|_| ())?
            };
            observations.push(GoldenObservationTranscriptV1 {
                viewer: golden_viewer,
                observation: outcome,
            });
        }
        let after_views = execute_golden_views(
            host,
            trace.core_state(),
            trace.activity_state(),
            trace.head(),
            &corpus.viewers,
            checkpoint.checked_add(1).ok_or(())?,
        )?
        .into_iter()
        .map(|(_, _, transcript)| transcript)
        .collect();
        steps.push(GoldenStepTranscriptV1 {
            action_id: golden_action.action_id.as_str(),
            before_views: before_views
                .into_iter()
                .map(|(_, _, transcript)| transcript)
                .collect(),
            disposition: canonical_golden_disposition(&disposition).map_err(|_| ())?,
            transition,
            observations,
            after_views,
        });
    }
    CanonicalJsonV1::from_serialize(&PackGoldenTranscriptV1 {
        domain: PACK_GOLDEN_TRANSCRIPT_DOMAIN,
        genesis: &genesis,
        initial_views,
        steps,
    })
    .map_err(|_| ())
}

#[cfg(test)]
pub(crate) fn author_golden_transcript_for_test<E: ActivityPackV1>(
    revision_lock: PackRevisionLockV1,
    descriptor: &'static PackRevisionDescriptorV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    executor_artifact_digest: Blake3DigestV1,
    corpus: &PackGoldenCorpusV1,
    executor: E,
) -> Result<CanonicalJsonV1, ()> {
    let entry = Arc::new(ValidatedPackEntryV1 {
        revision_lock,
        descriptor: descriptor.clone(),
        schemas,
        codecs,
        codec_implementation: CanonicalPackCodecV1::canonical_v1(),
        executor_artifact_digest,
        golden_corpus_digest: corpus.digest().map_err(|_| ())?,
        executor: Arc::new(executor),
        status: PackRegistryStatusV1 {
            selectable_for_new_rooms: false,
            runnable_for_retained_rooms: true,
        },
    });
    execute_golden_corpus(
        &ActivityPackHostV1 {
            retained: RetainedActivityPackV1(entry),
        },
        corpus,
    )
}

#[cfg(test)]
pub(crate) fn author_golden_transcript_digest_for_test<E: ActivityPackV1>(
    revision_lock: PackRevisionLockV1,
    descriptor: &'static PackRevisionDescriptorV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    executor_artifact_digest: Blake3DigestV1,
    corpus: &PackGoldenCorpusV1,
    executor: E,
) -> Result<Blake3DigestV1, ()> {
    let transcript = author_golden_transcript_for_test(
        revision_lock,
        descriptor,
        schemas,
        codecs,
        executor_artifact_digest,
        corpus,
        executor,
    )?;
    Ok(Blake3DigestV1::hash(
        &transcript.to_bytes().map_err(|_| ())?,
    ))
}

#[allow(clippy::needless_pass_by_value)]
fn canonical_pack_fault(error: CanonicalJsonError) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}

/// Retained executable registry. It has no pack-name dispatch path.
pub struct PackRegistryV1 {
    revisions: BTreeMap<PackDigestV1, Arc<ValidatedPackEntryV1>>,
}

impl PackRegistryV1 {
    /// Validates and freezes an embedded registry.
    ///
    /// # Errors
    ///
    /// Returns a precise compatibility error for every incomplete,
    /// inconsistent, or colliding row.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn try_new(
        entries: impl IntoIterator<Item = PackRegistryEntryV1>,
    ) -> Result<Self, PackRegistryErrorV1> {
        let mut revisions = BTreeMap::new();
        for candidate in entries {
            let digest = candidate.revision_lock.revision_digest()?;
            candidate.revision_lock.validate_shape(&digest)?;
            candidate.descriptor.validate_shape(&digest)?;
            if digest != candidate.artifacts.expected_revision_digest
                || digest != candidate.descriptor.revision_digest
            {
                return Err(PackRegistryErrorV1::RevisionDigestMismatch(digest));
            }
            if candidate.revision_lock.revision_lock_id != PACK_REVISION_LOCK_ID
                || candidate.revision_lock.host_contract != ACTIVITY_PACK_HOST_CONTRACT_ID
                || candidate.revision_lock.canonical_codec != CANONICAL_CODEC_ID
                || candidate.descriptor.host_contract != ACTIVITY_PACK_HOST_CONTRACT_ID
                || candidate.descriptor.canonical_codec != CANONICAL_CODEC_ID
            {
                return Err(PackRegistryErrorV1::UnsupportedContract(digest));
            }
            if candidate.revision_lock.pack_id != candidate.descriptor.pack_id
                || candidate.revision_lock.explanatory_version
                    != candidate.descriptor.explanatory_version
            {
                return Err(PackRegistryErrorV1::DescriptorIdentityMismatch(digest));
            }
            let descriptor_digest = candidate.descriptor.content_digest()?;
            if descriptor_digest != candidate.revision_lock.descriptor_digest {
                return Err(PackRegistryErrorV1::DescriptorDigestMismatch(digest));
            }
            let schemas = candidate
                .artifacts
                .schemas
                .ok_or_else(|| PackRegistryErrorV1::MissingSchemaBundle(digest.clone()))?;
            if schemas.digest()? != candidate.revision_lock.schema_bundle_digest {
                return Err(PackRegistryErrorV1::SchemaBundleDigestMismatch(digest));
            }
            for reference in candidate.descriptor.schema_references() {
                let Some(schema) = schemas.get(&reference.schema_id) else {
                    return Err(PackRegistryErrorV1::MissingSchema {
                        revision_digest: digest,
                        schema_id: reference.schema_id.clone(),
                    });
                };
                if schema.schema_digest != reference.schema_digest {
                    return Err(PackRegistryErrorV1::WrongSchema {
                        revision_digest: digest,
                        schema_id: reference.schema_id.clone(),
                    });
                }
            }
            let codecs = candidate
                .artifacts
                .codecs
                .ok_or_else(|| PackRegistryErrorV1::MissingCodecBundle(digest.clone()))?;
            let canonical_codecs = PackCodecBundleV1::canonical_v1();
            if codecs.codec_id != canonical_codecs.codec_id
                || codecs.writer_version != canonical_codecs.writer_version
                || codecs.retained_reader_versions != canonical_codecs.retained_reader_versions
            {
                return Err(PackRegistryErrorV1::WrongCodec(digest));
            }
            for kind in PackCodecKindV1::REQUIRED {
                if !codecs.kinds.contains(&kind) {
                    return Err(PackRegistryErrorV1::MissingCodec {
                        revision_digest: digest,
                        kind,
                    });
                }
            }
            if codecs.kinds != canonical_codecs.kinds {
                return Err(PackRegistryErrorV1::WrongCodec(digest));
            }
            if codecs.digest()? != candidate.revision_lock.codec_bundle_digest {
                return Err(PackRegistryErrorV1::CodecBundleDigestMismatch(digest));
            }
            let codec_implementation = candidate
                .artifacts
                .codec_implementation
                .ok_or_else(|| PackRegistryErrorV1::MissingCodecImplementation(digest.clone()))?;
            codec_implementation
                .validate_frozen_vectors()
                .map_err(|_| PackRegistryErrorV1::WrongCodecImplementation(digest.clone()))?;
            if candidate.status.selectable_for_new_rooms
                && !candidate.status.runnable_for_retained_rooms
            {
                return Err(PackRegistryErrorV1::InvalidStatus(digest));
            }
            let executor_binding = candidate
                .executor
                .ok_or_else(|| PackRegistryErrorV1::MissingExecutor(digest.clone()))?;
            let executor = match (executor_binding, candidate.executor_provenance) {
                (
                    ExecutorBindingV1::Embedded {
                        executor,
                        concrete_type_id,
                        concrete_constructor,
                    },
                    Some(provenance),
                ) => {
                    if provenance.expected_type_id() != concrete_type_id
                        || provenance.expected_constructor() != concrete_constructor
                        || provenance.executor_artifact_digest()
                            != candidate.artifacts.executor_artifact_digest
                        || candidate.artifacts.executor_artifact_digest
                            != candidate.revision_lock.rule_source_digest
                    {
                        return Err(PackRegistryErrorV1::WrongExecutorProvenance(digest));
                    }
                    executor
                }
                (ExecutorBindingV1::Embedded { .. }, None) => {
                    return Err(PackRegistryErrorV1::MissingExecutorProvenance(digest));
                }
                (
                    ExecutorBindingV1::Portable {
                        executor,
                        component_digest,
                    },
                    None,
                ) => {
                    if component_digest != candidate.artifacts.executor_artifact_digest
                        || component_digest != candidate.revision_lock.rule_source_digest
                    {
                        return Err(PackRegistryErrorV1::WrongPortableExecutorIdentity(digest));
                    }
                    executor
                }
                (ExecutorBindingV1::Portable { .. }, Some(_)) => {
                    return Err(PackRegistryErrorV1::WrongPortableExecutorIdentity(digest));
                }
            };
            let executor_descriptor = catch_unwind(AssertUnwindSafe(|| executor.descriptor()))
                .map_err(|_| PackRegistryErrorV1::DescriptorPanicked(digest.clone()))?;
            if executor_descriptor != &candidate.descriptor {
                return Err(PackRegistryErrorV1::WrongExecutor(digest));
            }
            let golden_corpus = candidate
                .artifacts
                .golden_corpus
                .clone()
                .ok_or_else(|| PackRegistryErrorV1::MissingGoldenCorpus(digest.clone()))?;
            if golden_corpus.digest()? != candidate.artifacts.golden_corpus_digest {
                return Err(PackRegistryErrorV1::GoldenCorpusDigestMismatch(digest));
            }
            let entry = Arc::new(ValidatedPackEntryV1 {
                revision_lock: candidate.revision_lock,
                descriptor: candidate.descriptor,
                schemas,
                codecs,
                codec_implementation,
                executor_artifact_digest: candidate.artifacts.executor_artifact_digest,
                golden_corpus_digest: candidate.artifacts.golden_corpus_digest,
                executor,
                status: candidate.status,
            });
            let actual_transcript = execute_golden_corpus(
                &ActivityPackHostV1 {
                    retained: RetainedActivityPackV1(Arc::clone(&entry)),
                },
                &golden_corpus,
            )
            .map_err(|()| PackRegistryErrorV1::GoldenExecutionFailed(digest.clone()))?;
            let actual_transcript_digest = Blake3DigestV1::hash(
                &actual_transcript
                    .to_bytes()
                    .map_err(PackRegistryErrorV1::Canonical)?,
            );
            if actual_transcript_digest != golden_corpus.expected_transcript_digest {
                return Err(PackRegistryErrorV1::GoldenMismatch {
                    revision_digest: digest,
                    actual_transcript_digest,
                });
            }
            if revisions.insert(digest.clone(), entry).is_some() {
                return Err(PackRegistryErrorV1::DigestCollision(digest));
            }
        }
        if revisions.is_empty() {
            return Err(PackRegistryErrorV1::EmptyRegistry);
        }
        Ok(Self { revisions })
    }

    /// Combines independently validated embedded registries without changing
    /// the validation contract of [`Self::try_new`]. Each input has already
    /// passed the complete revision-lock, executor, codec, schema, and golden
    /// transcript checks; this seam only rechecks nonempty composition and
    /// rejects a semantic-digest collision.
    pub(crate) fn combine(
        registries: impl IntoIterator<Item = Self>,
    ) -> Result<Self, PackRegistryErrorV1> {
        let mut revisions = BTreeMap::new();
        for registry in registries {
            for (digest, entry) in registry.revisions {
                if revisions.insert(digest.clone(), entry).is_some() {
                    return Err(PackRegistryErrorV1::DigestCollision(digest));
                }
            }
        }
        if revisions.is_empty() {
            return Err(PackRegistryErrorV1::EmptyRegistry);
        }
        Ok(Self { revisions })
    }

    /// Admits already verified portable revisions through the same semantic
    /// validation and golden-execution path as embedded revisions.
    ///
    /// The receiver is consumed so registry publication remains atomic. No
    /// mutable registration, name/version lookup, or executor getter is
    /// introduced by this seam.
    ///
    /// # Errors
    ///
    /// Returns the existing precise registry error for any incomplete or
    /// inconsistent candidate, or for a semantic digest collision.
    pub fn admit_portable(
        self,
        candidates: impl IntoIterator<Item = PortablePackAdmissionV1>,
    ) -> Result<Self, PackRegistryErrorV1> {
        let portable = Self::try_new(
            candidates
                .into_iter()
                .map(PortablePackAdmissionV1::into_registry_entry),
        )?;
        Self::combine([self, portable])
    }

    #[cfg(any(test, feature = "conformance-tracer"))]
    pub(crate) fn replace_executor_for_conformance(
        &mut self,
        digest: &PackDigestV1,
        executor: Arc<dyn ActivityPackV1>,
    ) -> Result<(), PackRegistryErrorV1> {
        let current = self
            .revisions
            .get(digest)
            .ok_or_else(|| PackRegistryErrorV1::MissingRevision(digest.clone()))?;
        let descriptor = catch_unwind(AssertUnwindSafe(|| executor.descriptor()))
            .map_err(|_| PackRegistryErrorV1::DescriptorPanicked(digest.clone()))?;
        if descriptor != &current.descriptor {
            return Err(PackRegistryErrorV1::WrongExecutor(digest.clone()));
        }
        let replacement = ValidatedPackEntryV1 {
            revision_lock: current.revision_lock.clone(),
            descriptor: current.descriptor.clone(),
            schemas: current.schemas.clone(),
            codecs: current.codecs.clone(),
            codec_implementation: current.codec_implementation,
            executor_artifact_digest: current.executor_artifact_digest.clone(),
            golden_corpus_digest: current.golden_corpus_digest.clone(),
            executor,
            status: current.status,
        };
        self.revisions.insert(digest.clone(), Arc::new(replacement));
        Ok(())
    }

    /// Resolves the exact runnable executor for retained lineage.
    ///
    /// # Errors
    ///
    /// Returns an explicit missing-revision or non-runnable compatibility
    /// error. It never substitutes another revision.
    pub fn load_retained(
        &self,
        digest: &PackDigestV1,
    ) -> Result<RetainedActivityPackV1, PackRegistryErrorV1> {
        let entry = self
            .revisions
            .get(digest)
            .ok_or_else(|| PackRegistryErrorV1::MissingRevision(digest.clone()))?;
        if !entry.status.runnable_for_retained_rooms {
            return Err(PackRegistryErrorV1::NotRunnable(digest.clone()));
        }
        Ok(RetainedActivityPackV1(Arc::clone(entry)))
    }

    /// Resolves an exact revision only when it may create new Rooms.
    ///
    /// # Errors
    ///
    /// Returns an explicit lookup, retention, or selection error.
    pub fn select_for_new_room(
        &self,
        digest: &PackDigestV1,
    ) -> Result<RetainedActivityPackV1, PackRegistryErrorV1> {
        let retained = self.load_retained(digest)?;
        if !retained.0.status.selectable_for_new_rooms {
            return Err(PackRegistryErrorV1::NotSelectable(digest.clone()));
        }
        Ok(retained)
    }

    /// Selects an exact new-Room revision and runs checked initialization.
    ///
    /// # Errors
    ///
    /// Returns registry-selection or checked pack-initialization failure. A
    /// retained-only revision cannot enter this path.
    pub fn prepare_genesis_for_new_room(
        &self,
        request: &PackGenesisRequestV1,
    ) -> Result<PreparedNewRoomGenesisV1, PackGenesisErrorV1> {
        let retained_pack = self.select_for_new_room(&request.pack_digest)?;
        let genesis_input = retained_pack.host().initialize(request)?;
        Ok(PreparedNewRoomGenesisV1 {
            genesis_input,
            retained_pack,
        })
    }

    /// Loads an exact retained revision and reruns checked initialization for
    /// Genesis recovery/replay verification. Selection status is irrelevant;
    /// runnable-retention status is mandatory.
    ///
    /// # Errors
    ///
    /// Returns registry-retention or checked pack-initialization failure.
    pub(crate) fn prepare_genesis_for_retained_room(
        &self,
        request: &PackGenesisRequestV1,
    ) -> Result<VerifiedRetainedGenesisV1, PackGenesisErrorV1> {
        let retained_pack = self.load_retained(&request.pack_digest)?;
        let genesis_input = retained_pack.host().initialize(request)?;
        Ok(VerifiedRetainedGenesisV1 {
            genesis_input,
            retained_pack,
        })
    }

    /// Number of exact semantic revisions embedded in this registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.revisions.len()
    }

    /// Whether no revisions are embedded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }

    /// Returns the exact validated revision locks retained by this registry.
    ///
    /// The iterator follows semantic-digest order. Consumers may use these
    /// already-validated identities to bind deployment and backup metadata;
    /// no executor or registry mutation capability is exposed.
    #[must_use]
    pub fn retained_revision_locks(&self) -> impl ExactSizeIterator<Item = &PackRevisionLockV1> {
        self.revisions.values().map(|entry| &entry.revision_lock)
    }

    /// Lists every exact embedded revision in semantic-digest order.
    ///
    /// The result is a metadata snapshot. It does not expose an executor and
    /// cannot be used as a name/version selection path.
    #[must_use]
    pub fn catalog_revisions(
        &self,
    ) -> impl ExactSizeIterator<Item = ActivityPackCatalogRevisionV1> + '_ {
        self.revisions
            .iter()
            .map(|(revision_digest, entry)| ActivityPackCatalogRevisionV1 {
                revision_digest: revision_digest.clone(),
                descriptor: entry.descriptor.clone(),
                selectable_for_new_rooms: entry.status.selectable_for_new_rooms,
                runnable_for_retained_rooms: entry.status.runnable_for_retained_rooms,
            })
    }

    /// Reads catalog metadata for one exact semantic digest.
    ///
    /// # Errors
    ///
    /// Returns [`PackRegistryErrorV1::MissingRevision`] for an unknown digest.
    /// No name, explanatory version, or neighboring digest is consulted.
    pub fn catalog_revision(
        &self,
        revision_digest: &PackDigestV1,
    ) -> Result<ActivityPackCatalogRevisionV1, PackRegistryErrorV1> {
        let entry = self
            .revisions
            .get(revision_digest)
            .ok_or_else(|| PackRegistryErrorV1::MissingRevision(revision_digest.clone()))?;
        Ok(ActivityPackCatalogRevisionV1 {
            revision_digest: revision_digest.clone(),
            descriptor: entry.descriptor.clone(),
            selectable_for_new_rooms: entry.status.selectable_for_new_rooms,
            runnable_for_retained_rooms: entry.status.runnable_for_retained_rooms,
        })
    }

    /// Resolves one exact schema reference within one exact embedded revision.
    ///
    /// # Errors
    ///
    /// Returns an explicit missing-revision, missing-schema, or wrong-schema
    /// error. Resolution never crosses to another revision with the same
    /// schema ID.
    pub fn resolve_schema(
        &self,
        revision_digest: &PackDigestV1,
        reference: &SchemaReferenceV1,
    ) -> Result<&CanonicalJsonV1, PackRegistryErrorV1> {
        let entry = self
            .revisions
            .get(revision_digest)
            .ok_or_else(|| PackRegistryErrorV1::MissingRevision(revision_digest.clone()))?;
        let schema = entry.schemas.get(&reference.schema_id).ok_or_else(|| {
            PackRegistryErrorV1::MissingSchema {
                revision_digest: revision_digest.clone(),
                schema_id: reference.schema_id.clone(),
            }
        })?;
        if schema.schema_digest != reference.schema_digest {
            return Err(PackRegistryErrorV1::WrongSchema {
                revision_digest: revision_digest.clone(),
                schema_id: reference.schema_id.clone(),
            });
        }
        Ok(schema.canonical_schema())
    }
}

/// Registry-backed Genesis preparation failure. The two layers remain
/// distinguishable for startup/recovery diagnostics.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PackGenesisErrorV1 {
    #[error(transparent)]
    Registry(#[from] PackRegistryErrorV1),
    #[error(transparent)]
    Pack(#[from] PackFaultV1),
}

/// Fail-closed retained-registry construction and lookup errors.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PackRegistryErrorV1 {
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error("pack revision digest does not equal recomputed revision lock: {0}")]
    RevisionDigestMismatch(PackDigestV1),
    #[error("pack revision uses an unsupported host, lock, or codec contract: {0}")]
    UnsupportedContract(PackDigestV1),
    #[error("pack descriptor identity differs from its revision lock: {0}")]
    DescriptorIdentityMismatch(PackDigestV1),
    #[error("pack descriptor digest differs from its revision lock: {0}")]
    DescriptorDigestMismatch(PackDigestV1),
    #[error("pack descriptor for {revision_digest} has invalid canonical shape: {detail}")]
    InvalidDescriptorShape {
        revision_digest: PackDigestV1,
        detail: &'static str,
    },
    #[error("pack revision lock for {revision_digest} has invalid canonical shape: {detail}")]
    InvalidRevisionLockShape {
        revision_digest: PackDigestV1,
        detail: &'static str,
    },
    #[error("pack revision has no retained schema bundle: {0}")]
    MissingSchemaBundle(PackDigestV1),
    #[error("pack schema bundle digest differs from its revision lock: {0}")]
    SchemaBundleDigestMismatch(PackDigestV1),
    #[error("pack revision {revision_digest} is missing schema {schema_id}")]
    MissingSchema {
        revision_digest: PackDigestV1,
        schema_id: String,
    },
    #[error("pack revision {revision_digest} has wrong bytes for schema {schema_id}")]
    WrongSchema {
        revision_digest: PackDigestV1,
        schema_id: String,
    },
    #[error("schema ID collision: {0}")]
    SchemaCollision(String),
    #[error("pack schema {schema_id} is malformed or unsupported: {detail}")]
    MalformedSchema { schema_id: String, detail: String },
    #[error("pack schema ID must be nonempty")]
    InvalidSchemaId,
    #[error("pack schema bundle must not be empty")]
    EmptySchemaBundle,
    #[error("pack revision has no retained codec bundle: {0}")]
    MissingCodecBundle(PackDigestV1),
    #[error("pack revision has a wrong canonical codec contract: {0}")]
    WrongCodec(PackDigestV1),
    #[error("pack revision {revision_digest} is missing {kind:?} codec support")]
    MissingCodec {
        revision_digest: PackDigestV1,
        kind: PackCodecKindV1,
    },
    #[error("pack codec bundle digest differs from its revision lock: {0}")]
    CodecBundleDigestMismatch(PackDigestV1),
    #[error("pack revision has no executable retained codec implementation: {0}")]
    MissingCodecImplementation(PackDigestV1),
    #[error("pack executable retained codec fails canonical-v1 frozen vectors: {0}")]
    WrongCodecImplementation(PackDigestV1),
    #[error("selectable pack revision is not retained-runnable: {0}")]
    InvalidStatus(PackDigestV1),
    #[error("pack revision has no compiled executor: {0}")]
    MissingExecutor(PackDigestV1),
    #[error("pack executor descriptor operation panicked: {0}")]
    DescriptorPanicked(PackDigestV1),
    #[error("pack revision is bound to the wrong compiled executor: {0}")]
    WrongExecutor(PackDigestV1),
    #[error("pack revision has no independently reviewed executor provenance: {0}")]
    MissingExecutorProvenance(PackDigestV1),
    #[error("pack executor lacks its sealed build constructor/artifact provenance: {0}")]
    WrongExecutorProvenance(PackDigestV1),
    #[error("portable pack executor differs from the exact Component digest in its lock: {0}")]
    WrongPortableExecutorIdentity(PackDigestV1),
    #[error("pack revision has no retained behavioral golden corpus: {0}")]
    MissingGoldenCorpus(PackDigestV1),
    #[error("pack retained golden corpus digest disagrees with its exact bytes: {0}")]
    GoldenCorpusDigestMismatch(PackDigestV1),
    #[error("pack executor cannot complete its retained behavioral golden corpus: {0}")]
    GoldenExecutionFailed(PackDigestV1),
    #[error("pack executor behavior disagrees with its retained golden corpus: {revision_digest}")]
    GoldenMismatch {
        revision_digest: PackDigestV1,
        actual_transcript_digest: Blake3DigestV1,
    },
    #[error("pack semantic revision digest collision: {0}")]
    DigestCollision(PackDigestV1),
    #[error("embedded pack registry must retain at least one executable revision")]
    EmptyRegistry,
    #[error("pack semantic revision is absent: {0}")]
    MissingRevision(PackDigestV1),
    #[error("pack semantic revision is not runnable for retained Rooms: {0}")]
    NotRunnable(PackDigestV1),
    #[error("pack semantic revision is not selectable for new Rooms: {0}")]
    NotSelectable(PackDigestV1),
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fmt::Display,
        str::FromStr,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
    };

    use super::*;
    use crate::{
        AccessModeV1, ActionId, ActivityApplyV1, ActivityRejectionV1,
        AdministrationOperationIdentityV1, CoreAuthorityAttributionV1, CoreAuthorityKindV1,
        CoreChangeSetV1, CoreProposedKindV1, CoreProposedV1, CoreRecordedAt, CoreTraceV1,
        MembershipChangeV1, MembershipV1, PrincipalId, PrincipalKindV1, RoomStatusV1,
    };

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const MEMBER_TWO: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const SPECTATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
    const OPERATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
    const PRINCIPAL_TWO: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
    const ADMIN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
    const SPECTATOR_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
    const OPERATOR_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB4";
    const ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const ACTION_TWO: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
    const ROOM_SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const CREATED_AT: &str = "2026-08-15T12:00:00Z";

    fn parsed<T>(value: &str) -> T
    where
        T: FromStr,
        T::Err: Display,
    {
        value
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture parse failed: {error}"))
    }

    fn json(value: &str) -> CanonicalJsonV1 {
        CanonicalJsonV1::parse(value.as_bytes())
            .unwrap_or_else(|error| unreachable!("fixture JSON failed: {error}"))
    }

    #[derive(Default)]
    struct CallbackCounts {
        descriptor: AtomicUsize,
        initialize: AtomicUsize,
        reduce: AtomicUsize,
        view: AtomicUsize,
        observe: AtomicUsize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ReduceBehavior {
        Apply,
        RejectDeclared,
        RejectUndeclared,
        InvalidState,
        ExcessiveState,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ProjectionBehavior {
        Constant,
        ActivityState,
        Oversize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum OfferBehavior {
        Always,
        AllDeclared,
        Never,
        StateFlag,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ObservationBehavior {
        None,
        SomeWithAfterOffers,
    }

    struct ControlledPack {
        descriptor: &'static PackRevisionDescriptorV1,
        panic_on: Option<ActivityPackOperationV1>,
        reduce_behavior: ReduceBehavior,
        projection_behavior: ProjectionBehavior,
        offer_behavior: OfferBehavior,
        observation_behavior: ObservationBehavior,
        counts: Arc<CallbackCounts>,
        supplied_after_offers: Arc<Mutex<Option<CanonicalActionOffersV1>>>,
    }

    impl ControlledPack {
        fn good(counts: Arc<CallbackCounts>) -> Self {
            Self {
                descriptor: fixture().descriptor,
                panic_on: None,
                reduce_behavior: ReduceBehavior::Apply,
                projection_behavior: ProjectionBehavior::Constant,
                offer_behavior: OfferBehavior::Always,
                observation_behavior: ObservationBehavior::None,
                counts,
                supplied_after_offers: Arc::new(Mutex::new(None)),
            }
        }

        fn maybe_panic(&self, operation: ActivityPackOperationV1) {
            assert_ne!(
                self.panic_on,
                Some(operation),
                "intentional {operation:?} callback panic"
            );
        }

        fn offer(definition: &ActionDefinitionV1) -> ActionOfferV1 {
            ActionOfferV1 {
                domain: ACTION_OFFER_DOMAIN.to_owned(),
                action_type: definition.action_type.clone(),
                payload_schema_digest: definition.payload_schema.schema_digest.clone(),
                eligibility_window: None,
            }
        }
    }

    impl ActivityPackV1 for ControlledPack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            self.counts.descriptor.fetch_add(1, AtomicOrdering::Relaxed);
            self.maybe_panic(ActivityPackOperationV1::Descriptor);
            self.descriptor
        }

        fn initialize(
            &self,
            _input: &ActivityGenesisInputV1<'_>,
            _cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            self.counts.initialize.fetch_add(1, AtomicOrdering::Relaxed);
            self.maybe_panic(ActivityPackOperationV1::Initialize);
            Ok(InitialOutputV1 {
                initial_activity_state: json("{}"),
                timer_requests: Vec::new(),
            })
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            _cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            self.counts.reduce.fetch_add(1, AtomicOrdering::Relaxed);
            self.maybe_panic(ActivityPackOperationV1::Reduce);
            match self.reduce_behavior {
                ReduceBehavior::Apply => Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                    next_activity_state: input.prior_activity_state.clone(),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                })),
                ReduceBehavior::RejectUndeclared => {
                    Ok(ActivityDispositionV1::Reject(ActivityRejectionV1 {
                        declared_code: "not_declared".to_owned(),
                        bounded_safe_details: json("{}"),
                    }))
                }
                ReduceBehavior::RejectDeclared => {
                    Ok(ActivityDispositionV1::Reject(ActivityRejectionV1 {
                        declared_code: "denied".to_owned(),
                        bounded_safe_details: json("{}"),
                    }))
                }
                ReduceBehavior::InvalidState => Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                    next_activity_state: json("null"),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                })),
                ReduceBehavior::ExcessiveState => {
                    let long = "x".repeat(300);
                    Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                        next_activity_state: json(&format!("{{\"long\":\"{long}\"}}")),
                        ordered_domain_events: Vec::new(),
                        timer_requests: Vec::new(),
                        ordered_attention_signals: Vec::new(),
                    }))
                }
            }
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            self.counts.view.fetch_add(1, AtomicOrdering::Relaxed);
            self.maybe_panic(ActivityPackOperationV1::View);
            let projection = match self.projection_behavior {
                ProjectionBehavior::Constant => json("{}"),
                ProjectionBehavior::ActivityState => input.activity_state.clone(),
                ProjectionBehavior::Oversize => {
                    json(&format!("{{\"oversize\":\"{}\"}}", "x".repeat(2048)))
                }
            };
            let offered = match self.offer_behavior {
                OfferBehavior::Always | OfferBehavior::AllDeclared => true,
                OfferBehavior::Never => false,
                OfferBehavior::StateFlag => serde_json::to_value(input.activity_state)
                    .ok()
                    .and_then(|value| value.get("offered").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false),
            } && matches!(input.viewer, PackViewerV1::Participant(_));
            Ok(PackViewV1 {
                projection_schema: self.descriptor.projection_schemas
                    [&PackViewerClassV1::Participant]
                    .schema_id
                    .clone(),
                projection,
                action_offers: if !offered {
                    Vec::new()
                } else if self.offer_behavior == OfferBehavior::AllDeclared {
                    self.descriptor.actions.iter().map(Self::offer).collect()
                } else {
                    vec![Self::offer(&self.descriptor.actions[0])]
                },
            })
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            self.counts.observe.fetch_add(1, AtomicOrdering::Relaxed);
            self.maybe_panic(ActivityPackOperationV1::Observe);
            let schema = self.descriptor.observation_schemas[&PackViewerClassV1::Participant]
                .schema_id
                .clone();
            match self.observation_behavior {
                ObservationBehavior::None => Ok(None),
                ObservationBehavior::SomeWithAfterOffers => {
                    let offers = input.after_view.action_offers().clone();
                    *self
                        .supplied_after_offers
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(offers.clone());
                    Ok(Some(
                        PackObservationV1::new(schema, json("{}")).with_action_offers(offers),
                    ))
                }
            }
        }
    }

    struct FixturePack;

    impl FixturePack {
        fn count(state: &CanonicalJsonV1) -> i64 {
            serde_json::to_value(state)
                .ok()
                .and_then(|value| value.get("count").and_then(serde_json::Value::as_i64))
                .unwrap_or(0)
        }

        fn projection(
            core: &CoreRoomStateV1,
            state: &CanonicalJsonV1,
            viewer: &PackViewerV1,
        ) -> CanonicalJsonV1 {
            let participant = core
                .membership(viewer.member_id())
                .is_some_and(|membership| membership.access_mode() == AccessModeV1::Participant);
            if participant {
                json(&format!("{{\"count\":{}}}", Self::count(state)))
            } else {
                json("{}")
            }
        }
    }

    impl ActivityPackV1 for FixturePack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            fixture().descriptor
        }

        fn initialize(
            &self,
            _input: &ActivityGenesisInputV1<'_>,
            _cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            Ok(InitialOutputV1 {
                initial_activity_state: json(r#"{"count":0}"#),
                timer_requests: Vec::new(),
            })
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            _cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                next_activity_state: json(&format!(
                    "{{\"count\":{}}}",
                    Self::count(input.prior_activity_state) + 1
                )),
                ordered_domain_events: vec![json(r#"{"event_type":"incremented"}"#)],
                timer_requests: Vec::new(),
                ordered_attention_signals: Vec::new(),
            }))
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            if matches!(input.viewer, PackViewerV1::FinalReveal(_))
                && Self::count(input.activity_state) < 2
            {
                return Err(PackFaultV1::PrivacyContract(
                    "final reveal is unavailable before completion".to_owned(),
                ));
            }
            let class = ActivityPackHostV1::authorize_viewer(input.core, input.viewer)?;
            let participant = matches!(input.viewer, PackViewerV1::Participant(_));
            let offers = participant && Self::count(input.activity_state) < 2;
            Ok(PackViewV1 {
                projection_schema: fixture().descriptor.projection_schemas[&class]
                    .schema_id
                    .clone(),
                projection: Self::projection(input.core, input.activity_state, input.viewer),
                action_offers: offers
                    .then(|| ControlledPack::offer(&fixture().descriptor.actions[0]))
                    .into_iter()
                    .collect(),
            })
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            let before = Self::projection(input.core_before, input.activity_before, input.viewer);
            let after = Self::projection(input.core_after, input.activity_after, input.viewer);
            if before == after {
                return Ok(None);
            }
            let class = ActivityPackHostV1::authorize_viewer(input.core_after, input.viewer)?;
            let mut observation = PackObservationV1::new(
                fixture().descriptor.observation_schemas[&class]
                    .schema_id
                    .clone(),
                after,
            );
            let offers_changed = Self::count(input.activity_before) < 2
                && Self::count(input.activity_after) >= 2
                && matches!(input.viewer, PackViewerV1::Participant(_));
            if offers_changed {
                observation =
                    observation.with_action_offers(input.after_view.action_offers().clone());
            }
            Ok(Some(observation))
        }
    }

    struct CountingFixturePack {
        counts: Arc<CallbackCounts>,
    }

    impl ActivityPackV1 for CountingFixturePack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            self.counts.descriptor.fetch_add(1, AtomicOrdering::Relaxed);
            FixturePack.descriptor()
        }

        fn initialize(
            &self,
            input: &ActivityGenesisInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            self.counts.initialize.fetch_add(1, AtomicOrdering::Relaxed);
            FixturePack.initialize(input, cx)
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            self.counts.reduce.fetch_add(1, AtomicOrdering::Relaxed);
            FixturePack.reduce(input, cx)
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            self.counts.view.fetch_add(1, AtomicOrdering::Relaxed);
            FixturePack.view(input)
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            self.counts.observe.fetch_add(1, AtomicOrdering::Relaxed);
            FixturePack.observe(input)
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum CorpusImpostorBehavior {
        PublicView,
        OfferRemovalObservation,
        EarlyFinalReveal,
        WrongFinalRevealDenial,
        PanicBeforeFinalReveal,
    }

    struct CorpusImpostorPack {
        behavior: CorpusImpostorBehavior,
    }

    impl ActivityPackV1 for CorpusImpostorPack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            FixturePack.descriptor()
        }

        fn initialize(
            &self,
            input: &ActivityGenesisInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            FixturePack.initialize(input, cx)
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            FixturePack.reduce(input, cx)
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            if matches!(input.viewer, PackViewerV1::FinalReveal(_))
                && FixturePack::count(input.activity_state) < 2
            {
                match self.behavior {
                    CorpusImpostorBehavior::EarlyFinalReveal => {
                        return Ok(PackViewV1 {
                            projection_schema: fixture().descriptor.projection_schemas
                                [&PackViewerClassV1::FinalReveal]
                                .schema_id
                                .clone(),
                            projection: json(r#"{"premature_reveal":true}"#),
                            action_offers: Vec::new(),
                        });
                    }
                    CorpusImpostorBehavior::WrongFinalRevealDenial => {
                        return Err(PackFaultV1::PrivacyContract(
                            "wrong pre-completion denial".to_owned(),
                        ));
                    }
                    CorpusImpostorBehavior::PanicBeforeFinalReveal => {
                        assert_ne!(
                            self.behavior,
                            CorpusImpostorBehavior::PanicBeforeFinalReveal,
                            "intentional pre-completion FinalReveal panic"
                        );
                        return Err(PackFaultV1::Callback(
                            "unreachable FinalReveal fixture branch".to_owned(),
                        ));
                    }
                    CorpusImpostorBehavior::PublicView
                    | CorpusImpostorBehavior::OfferRemovalObservation => {}
                }
            }
            let mut view = FixturePack.view(input)?;
            if matches!(self.behavior, CorpusImpostorBehavior::PublicView)
                && matches!(input.viewer, PackViewerV1::Public(_))
            {
                view.projection = json(r#"{"impostor":true}"#);
            }
            Ok(view)
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            if matches!(
                self.behavior,
                CorpusImpostorBehavior::OfferRemovalObservation
            ) && matches!(input.viewer, PackViewerV1::Participant(_))
                && FixturePack::count(input.activity_before) < 2
                && FixturePack::count(input.activity_after) >= 2
            {
                let class = ActivityPackHostV1::authorize_viewer(input.core_after, input.viewer)?;
                return Ok(Some(
                    PackObservationV1::new(
                        fixture().descriptor.observation_schemas[&class]
                            .schema_id
                            .clone(),
                        json(r#"{"count":999}"#),
                    )
                    .with_action_offers(input.after_view.action_offers().clone()),
                ));
            }
            FixturePack.observe(input)
        }
    }

    struct EquivalentFixturePack;

    impl ActivityPackV1 for EquivalentFixturePack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            FixturePack.descriptor()
        }

        fn initialize(
            &self,
            input: &ActivityGenesisInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            FixturePack.initialize(input, cx)
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            FixturePack.reduce(input, cx)
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            FixturePack.view(input)
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            FixturePack.observe(input)
        }
    }

    struct WireFixturePack {
        descriptor: PackRevisionDescriptorV1,
    }

    impl WireFixturePack {
        fn new() -> Self {
            let expected_content = fixture().descriptor.content();
            let descriptor_bytes = encode(&expected_content)
                .unwrap_or_else(|error| unreachable!("fixture descriptor bytes: {error}"));
            let actual_content = CanonicalPackOperationCodecV1::canonical_v1()
                .decode_descriptor(&descriptor_bytes)
                .unwrap_or_else(|error| unreachable!("fixture descriptor decode: {error}"));
            assert_eq!(actual_content, expected_content);
            Self {
                descriptor: fixture().descriptor.clone(),
            }
        }

        fn callback_fault(error: impl Display) -> PackFaultV1 {
            PackFaultV1::Callback(error.to_string())
        }

        fn success_bytes<T: Serialize>(output: T) -> Result<Vec<u8>, PackFaultV1> {
            encode(&PackOperationResultV1::Success { output }).map_err(Self::callback_fault)
        }
    }

    impl ActivityPackV1 for WireFixturePack {
        fn descriptor(&self) -> &PackRevisionDescriptorV1 {
            &self.descriptor
        }

        fn initialize(
            &self,
            input: &ActivityGenesisInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<InitialOutputV1, PackFaultV1> {
            let codec = CanonicalPackOperationCodecV1::canonical_v1();
            let request_bytes = codec
                .encode_initialize_request(input, cx)
                .map_err(Self::callback_fault)?;
            let request: PackInitializeRequestV1 =
                CanonicalJsonV1::decode_canonical(&request_bytes).map_err(Self::callback_fault)?;
            if request.room_id != *input.room_id
                || request.pack_digest != *input.pack_digest
                || request.configuration != *input.configuration
                || request.initial_core_state != *input.initial_core_state
                || request.room_seed != *input.room_seed
                || request.created_at != *input.created_at
                || request.deterministic_context != PackDeterministicContextV1::from_borrowed(cx)
            {
                return Err(PackFaultV1::Callback(
                    "initialize wire request lost an input".to_owned(),
                ));
            }
            let response = Self::success_bytes(FixturePack.initialize(input, cx)?)?;
            codec
                .decode_initialize_result(&response)
                .map_err(Self::callback_fault)?
                .into_result()
        }

        fn reduce(
            &self,
            input: &ActivityReduceInputV1<'_>,
            cx: &DeterministicContextV1<'_>,
        ) -> Result<ActivityDispositionV1, PackFaultV1> {
            let codec = CanonicalPackOperationCodecV1::canonical_v1();
            let request_bytes = codec
                .encode_reduce_request(input, cx)
                .map_err(Self::callback_fault)?;
            let request: PackReduceRequestV1 =
                CanonicalJsonV1::decode_canonical(&request_bytes).map_err(Self::callback_fault)?;
            if request.prior_activity_state != *input.prior_activity_state
                || request.core_before != *input.core_before
                || request.proposed_core_after != *input.proposed_core_after
                || request.scheduled_timers != *input.scheduled_timers
                || request.next_room_seq != input.next_room_seq
                || request.recorded_stimulus != *input.recorded_stimulus
                || request.deterministic_context != PackDeterministicContextV1::from_borrowed(cx)
            {
                return Err(PackFaultV1::Callback(
                    "reduce wire request lost an input".to_owned(),
                ));
            }
            let response = Self::success_bytes(FixturePack.reduce(input, cx)?)?;
            codec
                .decode_reduce_result(&response)
                .map_err(Self::callback_fault)?
                .into_result()
        }

        fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
            let codec = CanonicalPackOperationCodecV1::canonical_v1();
            let request_bytes = codec
                .encode_view_request(input)
                .map_err(Self::callback_fault)?;
            let request: PackViewRequestV1 =
                CanonicalJsonV1::decode_canonical(&request_bytes).map_err(Self::callback_fault)?;
            if request.core != *input.core
                || request.activity_state != *input.activity_state
                || request.complete_head != *input.complete_head
                || request.viewer != PackWireViewerV1::from_borrowed(input.viewer)
            {
                return Err(PackFaultV1::Callback(
                    "view wire request lost an input".to_owned(),
                ));
            }
            let response = Self::success_bytes(FixturePack.view(input)?)?;
            codec
                .decode_view_result(&response)
                .map_err(Self::callback_fault)?
                .into_result()
        }

        fn observe(
            &self,
            input: &ObserveInputV1<'_>,
        ) -> Result<Option<PackObservationV1>, PackFaultV1> {
            let codec = CanonicalPackOperationCodecV1::canonical_v1();
            let request_bytes = codec
                .encode_observe_request(input)
                .map_err(Self::callback_fault)?;
            let request: PackObserveRequestV1 =
                CanonicalJsonV1::decode_canonical(&request_bytes).map_err(Self::callback_fault)?;
            if request.core_before != *input.core_before
                || request.activity_before != *input.activity_before
                || request.core_after != *input.core_after
                || request.activity_after != *input.activity_after
                || request.recorded_stimulus != *input.recorded_stimulus
                || request.ordered_domain_events != input.ordered_domain_events
                || request.viewer != PackWireViewerV1::from_borrowed(input.viewer)
                || request.after_view.action_offers != input.after_view.action_offers().offers()
            {
                return Err(PackFaultV1::Callback(
                    "observe wire request lost an input".to_owned(),
                ));
            }
            let output = FixturePack
                .observe(input)?
                .map(|observation| PackWireObservationV1 {
                    observation_schema: observation.observation_schema,
                    observation: observation.observation,
                    action_offers: if observation.action_offers.is_some() {
                        PackWireObservationOffersV1::ReuseAfterView
                    } else {
                        PackWireObservationOffersV1::Unchanged
                    },
                });
            let response = Self::success_bytes(output)?;
            codec
                .decode_observe_result(&response, input.after_view)
                .map_err(Self::callback_fault)?
                .into_result()
        }
    }

    struct Fixture {
        descriptor: &'static PackRevisionDescriptorV1,
        lock: PackRevisionLockV1,
        schemas: PackSchemaBundleV1,
        codecs: PackCodecBundleV1,
        digest: PackDigestV1,
        artifact: Blake3DigestV1,
    }

    #[allow(clippy::too_many_lines)]
    fn fixture() -> &'static Fixture {
        static FIXTURE: OnceLock<Fixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let schema = PackSchemaV1::new(
                "fixture/value/v1",
                CanonicalJsonV1::parse(br#"{"type":"object"}"#)
                    .unwrap_or_else(|error| unreachable!("fixture schema: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("fixture schema: {error}"));
            let reference = schema.reference();
            let schemas = PackSchemaBundleV1::new([schema])
                .unwrap_or_else(|error| unreachable!("fixture bundle: {error}"));
            let codecs = PackCodecBundleV1::canonical_v1();
            let placeholder: PackDigestV1 =
                "blake3:0000000000000000000000000000000000000000000000000000000000000000"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("fixture digest: {error}"));
            let mut descriptor = PackRevisionDescriptorV1 {
                pack_id: "worldstream.fixture".to_owned(),
                name: "Fixture".to_owned(),
                explanatory_version: "1.0.0".to_owned(),
                revision_digest: placeholder,
                host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
                canonical_codec: CANONICAL_CODEC_ID.to_owned(),
                configuration_schema: reference.clone(),
                state_schema: reference.clone(),
                roles: vec![
                    RoleDefinitionV1 {
                        role: "counter".to_owned(),
                        minimum: 0,
                        maximum: 8,
                    },
                    RoleDefinitionV1 {
                        role: "counter2".to_owned(),
                        minimum: 0,
                        maximum: 8,
                    },
                ],
                actions: vec![ActionDefinitionV1 {
                    action_type: "increment".to_owned(),
                    payload_schema: reference.clone(),
                }],
                rejection_codes: vec!["denied".to_owned()],
                attention_reasons: Vec::new(),
                stimulus_schemas: BTreeMap::new(),
                output_schemas: BTreeMap::from([(
                    "rejection:denied".to_owned(),
                    reference.clone(),
                )]),
                event_schemas: BTreeMap::from([("incremented".to_owned(), reference.clone())]),
                projection_schemas: PackViewerClassV1::ALL
                    .into_iter()
                    .map(|viewer| (viewer, reference.clone()))
                    .collect(),
                observation_schemas: PackViewerClassV1::ALL
                    .into_iter()
                    .map(|viewer| (viewer, reference.clone()))
                    .collect(),
                limits: PackLimitsV1 {
                    maximum_state_bytes: 1024,
                    maximum_events: 4,
                    maximum_timer_requests: 4,
                    maximum_attention_signals: 4,
                    maximum_projection_bytes: 1024,
                    maximum_observation_bytes: 1024,
                    maximum_nesting: 8,
                    maximum_collection_items: 32,
                    maximum_text_bytes: 256,
                },
            };
            let artifact = Blake3DigestV1::hash(b"fixture-executor");
            let lock = PackRevisionLockV1 {
                revision_lock_id: PACK_REVISION_LOCK_ID.to_owned(),
                pack_id: descriptor.pack_id.clone(),
                explanatory_version: descriptor.explanatory_version.clone(),
                host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
                canonical_codec: CANONICAL_CODEC_ID.to_owned(),
                descriptor_digest: descriptor
                    .content_digest()
                    .unwrap_or_else(|error| unreachable!("descriptor: {error}")),
                schema_bundle_digest: schemas
                    .digest()
                    .unwrap_or_else(|error| unreachable!("schemas: {error}")),
                codec_bundle_digest: codecs
                    .digest()
                    .unwrap_or_else(|error| unreachable!("codecs: {error}")),
                deterministic_static_data_digests: Vec::new(),
                rule_source_digest: artifact.clone(),
                deterministic_dependency_lock_digest: Blake3DigestV1::hash(b"none"),
            };
            let digest = lock
                .revision_digest()
                .unwrap_or_else(|error| unreachable!("lock: {error}"));
            descriptor.revision_digest = digest.clone();
            let descriptor = Box::leak(Box::new(descriptor));
            Fixture {
                descriptor,
                lock,
                schemas,
                codecs,
                digest,
                artifact,
            }
        })
    }

    fn fixture_golden() -> &'static PackGoldenCorpusV1 {
        static GOLDEN: OnceLock<PackGoldenCorpusV1> = OnceLock::new();
        GOLDEN.get_or_init(|| {
            let mut corpus = PackGoldenCorpusV1 {
                corpus_id: PACK_GOLDEN_CORPUS_DOMAIN.to_owned(),
                genesis: genesis_request(),
                viewers: vec![
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Public,
                        member_id: parsed(SPECTATOR),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Participant,
                        member_id: parsed(MEMBER),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Operator,
                        member_id: parsed(OPERATOR),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Historical,
                        member_id: parsed(SPECTATOR),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Historical,
                        member_id: parsed(MEMBER),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::Historical,
                        member_id: parsed(OPERATOR),
                        available_after_action: 0,
                        denied_before_detail: None,
                    },
                    PackGoldenViewerV1 {
                        kind: PackGoldenViewerKindV1::FinalReveal,
                        member_id: parsed(MEMBER),
                        available_after_action: 2,
                        denied_before_detail: Some(
                            "final reveal is unavailable before completion".to_owned(),
                        ),
                    },
                ],
                actions: [
                    (ACTION, "2026-08-15T12:00:01Z"),
                    (ACTION_TWO, "2026-08-15T12:00:02Z"),
                ]
                .into_iter()
                .map(|(action_id, admitted_at)| PackGoldenActionV1 {
                    member_id: parsed(MEMBER),
                    action_id: parsed(action_id),
                    action_type: "increment".to_owned(),
                    payload_schema_digest: fixture().descriptor.actions[0]
                        .payload_schema
                        .schema_digest
                        .clone(),
                    canonical_payload: json("{}"),
                    admitted_at: parsed(admitted_at),
                })
                .collect(),
                external_inputs: Vec::new(),
                expected_transcript_digest: Blake3DigestV1::hash(b"uninitialized"),
            };
            let transcript =
                execute_golden_corpus(&unchecked_fixture_host(Arc::new(FixturePack)), &corpus)
                    .unwrap_or_else(|()| unreachable!("fixture golden execution"));
            corpus.expected_transcript_digest = Blake3DigestV1::hash(
                &transcript
                    .to_bytes()
                    .unwrap_or_else(|error| unreachable!("fixture transcript: {error}")),
            );
            corpus
        })
    }

    fn entry(status: PackRegistryStatusV1) -> PackRegistryEntryV1 {
        entry_with_executor(status, FixturePack)
    }

    fn entry_with_executor<E: ActivityPackV1>(
        status: PackRegistryStatusV1,
        executor: E,
    ) -> PackRegistryEntryV1 {
        let fixture = fixture();
        let golden = fixture_golden().clone();
        let provenance = reviewed_executor_provenance::<E>(fixture.artifact.clone());
        PackRegistryEntryV1::new(
            fixture.lock.clone(),
            fixture.descriptor,
            PackRegistryArtifactsV1 {
                expected_revision_digest: fixture.digest.clone(),
                schemas: Some(fixture.schemas.clone()),
                codecs: Some(fixture.codecs.clone()),
                codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
                executor_artifact_digest: fixture.artifact.clone(),
                golden_corpus_digest: golden
                    .digest()
                    .unwrap_or_else(|error| unreachable!("fixture golden digest: {error}")),
                golden_corpus: Some(golden),
            },
            Some(provenance),
            Some(executor),
            status,
        )
    }

    fn portable_admission(
        component_digest: Blake3DigestV1,
        golden: PackGoldenCorpusV1,
    ) -> PortablePackAdmissionV1 {
        PortablePackAdmissionV1::new(
            fixture().lock.clone(),
            fixture().descriptor.clone(),
            fixture().schemas.clone(),
            fixture().codecs.clone(),
            component_digest,
            golden
                .digest()
                .unwrap_or_else(|error| unreachable!("fixture golden digest: {error}")),
            golden,
            Arc::new(WireFixturePack::new()),
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
        )
    }

    fn rebind_schema_bundle(candidate: &mut PackRegistryEntryV1) {
        let schemas = candidate
            .artifacts
            .schemas
            .as_ref()
            .unwrap_or_else(|| unreachable!("fixture schema bundle"));
        candidate.revision_lock.schema_bundle_digest = schemas
            .digest()
            .unwrap_or_else(|error| unreachable!("changed schema bundle digest: {error}"));
        let revision_digest = candidate
            .revision_lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("changed revision digest: {error}"));
        candidate.artifacts.expected_revision_digest = revision_digest.clone();
        let mut descriptor = candidate.descriptor.clone();
        descriptor.revision_digest = revision_digest;
        candidate.descriptor = descriptor;
    }

    fn checked_host(pack: ControlledPack) -> ActivityPackHostV1 {
        unchecked_fixture_host(Arc::new(pack))
    }

    fn unchecked_fixture_host(executor: Arc<dyn ActivityPackV1>) -> ActivityPackHostV1 {
        let fixture = fixture();
        ActivityPackHostV1 {
            retained: RetainedActivityPackV1(Arc::new(ValidatedPackEntryV1 {
                revision_lock: fixture.lock.clone(),
                descriptor: fixture.descriptor.clone(),
                schemas: fixture.schemas.clone(),
                codecs: fixture.codecs.clone(),
                codec_implementation: CanonicalPackCodecV1::canonical_v1(),
                executor_artifact_digest: fixture.artifact.clone(),
                golden_corpus_digest: Blake3DigestV1::hash(b"test-only-unchecked"),
                executor,
                status: PackRegistryStatusV1 {
                    selectable_for_new_rooms: true,
                    runnable_for_retained_rooms: true,
                },
            })),
        }
    }

    fn checked_host_with_descriptor(
        mut descriptor: PackRevisionDescriptorV1,
        mut pack: ControlledPack,
    ) -> ActivityPackHostV1 {
        let mut revision_lock = fixture().lock.clone();
        revision_lock.descriptor_digest = descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("custom descriptor digest: {error}"));
        let digest = revision_lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("custom revision digest: {error}"));
        descriptor.revision_digest = digest.clone();
        let descriptor = Box::leak(Box::new(descriptor));
        pack.descriptor = descriptor;
        ActivityPackHostV1 {
            retained: RetainedActivityPackV1(Arc::new(ValidatedPackEntryV1 {
                revision_lock,
                descriptor: (*descriptor).clone(),
                schemas: fixture().schemas.clone(),
                codecs: fixture().codecs.clone(),
                codec_implementation: CanonicalPackCodecV1::canonical_v1(),
                executor_artifact_digest: fixture().artifact.clone(),
                golden_corpus_digest: Blake3DigestV1::hash(b"test-only-unchecked"),
                executor: Arc::new(pack),
                status: PackRegistryStatusV1 {
                    selectable_for_new_rooms: true,
                    runnable_for_retained_rooms: true,
                },
            })),
        }
    }

    fn initial_core() -> CoreRoomStateV1 {
        let member = MembershipV1::new(
            parsed(MEMBER),
            parsed(PRINCIPAL),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("valid fixture Membership: {error}"));
        let spectator = MembershipV1::new(
            parsed(SPECTATOR),
            parsed(SPECTATOR_PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Spectator,
            None,
        )
        .unwrap_or_else(|error| unreachable!("valid fixture spectator: {error}"));
        let operator = MembershipV1::new(
            parsed(OPERATOR),
            parsed(OPERATOR_PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Operator,
            None,
        )
        .unwrap_or_else(|error| unreachable!("valid fixture operator: {error}"));
        CoreRoomStateV1::active([member, spectator, operator])
            .unwrap_or_else(|error| unreachable!("valid fixture Core: {error}"))
    }

    fn genesis_request() -> PackGenesisRequestV1 {
        PackGenesisRequestV1 {
            room_id: parsed(ROOM),
            pack_digest: fixture().digest.clone(),
            configuration: json("{}"),
            room_seed: parsed(ROOM_SEED),
            created_at: parsed(CREATED_AT),
            initial_core_state: initial_core(),
        }
    }

    fn trace_with_activity(activity_state: CanonicalJsonV1) -> CoreTraceV1 {
        trace_with_core(initial_core(), activity_state)
    }

    fn trace_with_core(
        initial_core_state: CoreRoomStateV1,
        activity_state: CanonicalJsonV1,
    ) -> CoreTraceV1 {
        trace_with_core_and_digest(initial_core_state, activity_state, fixture().digest.clone())
    }

    fn trace_with_core_and_digest(
        initial_core_state: CoreRoomStateV1,
        activity_state: CanonicalJsonV1,
        pack_digest: PackDigestV1,
    ) -> CoreTraceV1 {
        let request = genesis_request();
        CoreTraceV1::create_for_conformance(
            GenesisInputV1::new(
                request.room_id,
                pack_digest,
                request.configuration,
                request.room_seed,
                request.created_at,
                initial_core_state,
                activity_state,
            ),
            |_| Ok(()),
            |input| {
                Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                    next_activity_state: input.prior_activity_state.clone(),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                }))
            },
        )
        .unwrap_or_else(|error| unreachable!("valid fixture trace: {error}"))
    }

    fn participant_action(head: &CompleteHeadV1) -> ParticipantActionV1 {
        ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed::<ActionId>(ACTION),
            action_type: "increment".to_owned(),
            payload_schema_digest: fixture().descriptor.actions[0]
                .payload_schema
                .schema_digest
                .clone(),
            canonical_payload: json("{}"),
            exact_basis_head: head.clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }
    }

    struct ActivityTransitionFixture {
        core_before: CoreRoomStateV1,
        activity_before: CanonicalJsonV1,
        head_before: CompleteHeadV1,
        after: CoreTraceV1,
        stimulus: RecordedStimulusV1,
    }

    #[allow(clippy::needless_pass_by_value)]
    fn activity_transition(
        activity_before: CanonicalJsonV1,
        activity_after: CanonicalJsonV1,
        pack_digest: PackDigestV1,
    ) -> ActivityTransitionFixture {
        let request = genesis_request();
        let next = activity_after.clone();
        let mut trace = CoreTraceV1::create_for_conformance(
            GenesisInputV1::new(
                request.room_id,
                pack_digest,
                request.configuration,
                request.room_seed,
                request.created_at,
                request.initial_core_state,
                activity_before,
            ),
            |_| Ok(()),
            move |_| {
                Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                    next_activity_state: next.clone(),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                }))
            },
        )
        .unwrap_or_else(|error| unreachable!("valid transition fixture trace: {error}"));
        let core_before = trace.core_state().clone();
        let activity_before = trace.activity_state().clone();
        let head_before = trace.head().clone();
        let stimulus = RecordedStimulusV1::ParticipantAction(participant_action(trace.head()));
        trace
            .advance(stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid transition fixture advance: {error}"));
        ActivityTransitionFixture {
            core_before,
            activity_before,
            head_before,
            after: trace,
            stimulus,
        }
    }

    fn current_view(
        host: &ActivityPackHostV1,
        trace: &CoreTraceV1,
    ) -> Result<ValidatedPackViewV1, PackFaultV1> {
        host.view(&ViewInputV1 {
            core: trace.core_state(),
            activity_state: trace.activity_state(),
            complete_head: trace.head(),
            viewer: &PackViewerV1::Participant(parsed(MEMBER)),
        })
    }

    fn reduce_action(
        host: &ActivityPackHostV1,
        trace: &CoreTraceV1,
        action: ParticipantActionV1,
        view: Option<&ValidatedPackViewV1>,
    ) -> Result<ActivityDispositionV1, ActivityPackReduceErrorV1> {
        reduce_action_with_hook(host, trace, action, view, || {})
    }

    fn reduce_action_with_hook<F>(
        host: &ActivityPackHostV1,
        trace: &CoreTraceV1,
        action: ParticipantActionV1,
        view: Option<&ValidatedPackViewV1>,
        on_invoke: F,
    ) -> Result<ActivityDispositionV1, ActivityPackReduceErrorV1>
    where
        F: FnOnce(),
    {
        let timers = BTreeMap::new();
        let stimulus = RecordedStimulusV1::ParticipantAction(action);
        host.reduce_with_invocation_hook(
            &ActivityReduceInputV1 {
                prior_activity_state: trace.activity_state(),
                core_before: trace.core_state(),
                proposed_core_after: trace.core_state(),
                scheduled_timers: &timers,
                next_room_seq: trace
                    .head()
                    .room_seq()
                    .checked_successor()
                    .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
                recorded_stimulus: &stimulus,
            },
            trace.head(),
            &parsed(ROOM_SEED),
            view,
            on_invoke,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_with_stimulus(
        host: &ActivityPackHostV1,
        core_before: &CoreRoomStateV1,
        activity_before: &CanonicalJsonV1,
        head_before: &CompleteHeadV1,
        core_after: &CoreRoomStateV1,
        activity_after: &CanonicalJsonV1,
        head_after: &CompleteHeadV1,
        stimulus: &RecordedStimulusV1,
    ) -> Result<ActivityObservationOutcomeV1, PackFaultV1> {
        host.observe(&ObserveTransitionInputV1 {
            core_before,
            activity_before,
            head_before,
            core_after,
            activity_after,
            head_after,
            recorded_stimulus: stimulus,
            ordered_domain_events: &[],
            viewer: &PackViewerV1::Participant(parsed(MEMBER)),
        })
    }

    fn administration_stimulus(
        trace: &CoreTraceV1,
        kind: CoreProposedKindV1,
        changeset: CoreChangeSetV1,
        key: &str,
    ) -> RecordedStimulusV1 {
        RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
            kind,
            CoreAuthorityAttributionV1 {
                principal_id: parsed::<PrincipalId>(ADMIN),
                authority_kind: CoreAuthorityKindV1::HostOperator,
            },
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(ADMIN),
                versioned_operation_kind: "worldstream/core-proposed/v1".to_owned(),
                idempotency_key: key.to_owned(),
            },
            trace.head().room_seq(),
            key,
            parsed::<CoreRecordedAt>("2026-08-15T12:00:02Z"),
            changeset,
        ))
    }

    #[test]
    fn portable_operation_codec_preserves_exact_after_view_offer_storage() {
        let descriptor_content = fixture().descriptor.content();
        let mut different_revision = fixture().descriptor.clone();
        different_revision.revision_digest =
            "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .parse()
                .unwrap_or_else(|error| unreachable!("different fixture digest: {error}"));
        assert_eq!(descriptor_content, different_revision.content());
        let descriptor_bytes = encode(&descriptor_content)
            .unwrap_or_else(|error| unreachable!("descriptor content bytes: {error}"));
        assert!(!String::from_utf8_lossy(&descriptor_bytes).contains("revision_digest"));

        let host = unchecked_fixture_host(Arc::new(FixturePack));
        let trace = trace_with_activity(json(r#"{"count":0}"#));
        let after_view = current_view(&host, &trace)
            .unwrap_or_else(|error| unreachable!("checked fixture view: {error}"));
        assert_eq!(after_view.action_offers().offers().len(), 1);

        let response = encode(&PackOperationResultV1::Success {
            output: Some(PackWireObservationV1 {
                observation_schema: fixture().descriptor.observation_schemas
                    [&PackViewerClassV1::Participant]
                    .schema_id
                    .clone(),
                observation: json(r#"{"count":0}"#),
                action_offers: PackWireObservationOffersV1::ReuseAfterView,
            }),
        })
        .unwrap_or_else(|error| unreachable!("portable observation response: {error}"));
        let decoded = CanonicalPackOperationCodecV1::canonical_v1()
            .decode_observe_result(&response, &after_view)
            .unwrap_or_else(|error| unreachable!("portable observation decode: {error}"))
            .into_result()
            .unwrap_or_else(|error| unreachable!("portable observation result: {error}"))
            .unwrap_or_else(|| unreachable!("portable observation is present"));
        let emitted = decoded
            .action_offers
            .as_ref()
            .unwrap_or_else(|| unreachable!("after-view offers were requested"));
        assert!(emitted.shares_storage_with(after_view.action_offers()));

        let malformed = CanonicalJsonV1::from_serialize(&serde_json::json!({
            "operation_result_type": "success",
            "output": {
                "action_offers": [],
                "observation": {},
                "observation_schema": "fixture/value/v1"
            }
        }))
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("malformed fixture bytes: {error}"));
        assert!(
            CanonicalPackOperationCodecV1::canonical_v1()
                .decode_observe_result(&malformed, &after_view)
                .is_err()
        );

        let fault = encode(&PackOperationResultV1::<PackViewV1>::Fault {
            fault: PackCallbackFaultV1::PrivacyContract {
                bounded_safe_detail: "not yet visible".to_owned(),
            },
        })
        .unwrap_or_else(|error| unreachable!("portable fault response: {error}"));
        assert!(matches!(
            CanonicalPackOperationCodecV1::canonical_v1()
                .decode_view_result(&fault)
                .unwrap_or_else(|error| unreachable!("portable fault decode: {error}"))
                .into_result(),
            Err(PackFaultV1::PrivacyContract(detail)) if detail == "not yet visible"
        ));
        assert!(
            CanonicalPackOperationCodecV1::canonical_v1()
                .decode_descriptor(b"{ }")
                .is_err()
        );
    }

    #[test]
    fn portable_admission_uses_the_existing_checked_host_and_golden_path() {
        let registry = crate::builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"))
            .admit_portable([portable_admission(
                fixture().artifact.clone(),
                fixture_golden().clone(),
            )])
            .unwrap_or_else(|error| unreachable!("valid portable admission: {error}"));
        let retained = registry
            .load_retained(&fixture().digest)
            .unwrap_or_else(|error| unreachable!("portable retained revision: {error}"));
        assert_eq!(retained.descriptor(), fixture().descriptor);
        assert_eq!(retained.revision_lock(), &fixture().lock);
        assert_eq!(retained.executor_artifact_digest(), &fixture().artifact);
        let genesis = registry
            .prepare_genesis_for_new_room(&genesis_request())
            .unwrap_or_else(|error| unreachable!("portable checked Genesis: {error}"));
        assert_eq!(
            genesis.genesis_input().initial_activity_state,
            json(r#"{"count":0}"#)
        );
    }

    #[test]
    fn portable_admission_rejects_wrong_component_golden_and_collision() {
        let wrong_component = crate::builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"))
            .admit_portable([portable_admission(
                Blake3DigestV1::hash(b"different-component"),
                fixture_golden().clone(),
            )]);
        assert!(matches!(
            wrong_component,
            Err(PackRegistryErrorV1::WrongPortableExecutorIdentity(digest))
                if digest == fixture().digest
        ));

        let mut wrong_golden = fixture_golden().clone();
        wrong_golden.expected_transcript_digest = Blake3DigestV1::hash(b"wrong-transcript");
        let wrong_golden = crate::builtin_counter_registry()
            .unwrap_or_else(|error| unreachable!("reviewed Counter registry: {error}"))
            .admit_portable([portable_admission(fixture().artifact.clone(), wrong_golden)]);
        assert!(matches!(
            wrong_golden,
            Err(PackRegistryErrorV1::GoldenMismatch {
                revision_digest,
                actual_transcript_digest,
            }) if revision_digest == fixture().digest
                && actual_transcript_digest == fixture_golden().expected_transcript_digest
        ));

        let collision = PackRegistryV1::try_new([entry(PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        })])
        .unwrap_or_else(|error| unreachable!("fixture registry: {error}"))
        .admit_portable([portable_admission(
            fixture().artifact.clone(),
            fixture_golden().clone(),
        )]);
        assert!(matches!(
            collision,
            Err(PackRegistryErrorV1::DigestCollision(digest)) if digest == fixture().digest
        ));
    }

    #[test]
    fn schema_bundle_rejects_empty_ids_empty_semantic_keys_and_unsupported_constraints() {
        let empty_id = PackSchemaV1::new("", json(r#"{"type":"object"}"#))
            .unwrap_or_else(|error| unreachable!("canonical schema: {error}"));
        assert!(matches!(
            PackSchemaBundleV1::new([empty_id]),
            Err(PackRegistryErrorV1::InvalidSchemaId)
        ));

        for invalid in [
            r#"{"pattern":"^[a-z]+$","type":"string"}"#,
            r#"{"minimum":"zero","type":"integer"}"#,
            r#"{"additionalProperties":{},"type":"object"}"#,
        ] {
            let schema = PackSchemaV1::new("fixture/unsupported/v1", json(invalid))
                .unwrap_or_else(|error| unreachable!("canonical schema: {error}"));
            assert!(matches!(
                PackSchemaBundleV1::new([schema]),
                Err(PackRegistryErrorV1::MalformedSchema { .. })
            ));
        }

        let mut descriptor = fixture().descriptor.clone();
        descriptor
            .projection_schemas
            .remove(&PackViewerClassV1::Operator);
        let descriptor = Box::leak(Box::new(descriptor));
        let candidate = PackRegistryEntryV1::new(
            fixture().lock.clone(),
            descriptor,
            PackRegistryArtifactsV1 {
                expected_revision_digest: fixture().digest.clone(),
                schemas: Some(fixture().schemas.clone()),
                codecs: Some(fixture().codecs.clone()),
                codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
                executor_artifact_digest: fixture().artifact.clone(),
                golden_corpus_digest: fixture_golden()
                    .digest()
                    .unwrap_or_else(|error| unreachable!("fixture golden digest: {error}")),
                golden_corpus: Some(fixture_golden().clone()),
            },
            Some(reviewed_executor_provenance::<FixturePack>(
                fixture().artifact.clone(),
            )),
            Some(FixturePack),
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
        );
        assert!(matches!(
            PackRegistryV1::try_new([candidate]),
            Err(PackRegistryErrorV1::InvalidDescriptorShape { .. })
        ));
    }

    #[test]
    fn registry_genesis_obeys_selection_and_really_invokes_initialize() {
        let counts = Arc::new(CallbackCounts::default());
        let registry = PackRegistryV1::try_new([entry_with_executor(
            PackRegistryStatusV1 {
                selectable_for_new_rooms: false,
                runnable_for_retained_rooms: true,
            },
            CountingFixturePack {
                counts: Arc::clone(&counts),
            },
        )])
        .unwrap_or_else(|error| unreachable!("valid retained registry: {error}"));
        let startup_initialize_count = counts.initialize.load(AtomicOrdering::Relaxed);
        assert_eq!(startup_initialize_count, 1);
        assert!(matches!(
            registry.prepare_genesis_for_new_room(&genesis_request()),
            Err(PackGenesisErrorV1::Registry(
                PackRegistryErrorV1::NotSelectable(_)
            ))
        ));
        assert_eq!(
            counts.initialize.load(AtomicOrdering::Relaxed),
            startup_initialize_count
        );
        let prepared = registry
            .prepare_genesis_for_retained_room(&genesis_request())
            .unwrap_or_else(|error| unreachable!("retained Genesis: {error}"));
        assert_eq!(
            counts.initialize.load(AtomicOrdering::Relaxed),
            startup_initialize_count + 1
        );
        assert_eq!(
            prepared.genesis_input().initial_activity_state,
            json(r#"{"count":0}"#)
        );
    }

    #[test]
    fn retained_behavioral_corpus_rejects_privacy_and_offer_observation_impostors() {
        let status = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        };
        for behavior in [
            CorpusImpostorBehavior::PublicView,
            CorpusImpostorBehavior::OfferRemovalObservation,
        ] {
            let candidate = entry_with_executor(status, CorpusImpostorPack { behavior });
            assert!(matches!(
                PackRegistryV1::try_new([candidate]),
                Err(PackRegistryErrorV1::GoldenMismatch { revision_digest, .. })
                    if revision_digest == fixture().digest
            ));
        }
    }

    #[test]
    fn retained_behavioral_corpus_proves_precompletion_reveal_denial() {
        let status = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        };
        for behavior in [
            CorpusImpostorBehavior::EarlyFinalReveal,
            CorpusImpostorBehavior::WrongFinalRevealDenial,
            CorpusImpostorBehavior::PanicBeforeFinalReveal,
        ] {
            let candidate = entry_with_executor(status, CorpusImpostorPack { behavior });
            assert!(matches!(
                PackRegistryV1::try_new([candidate]),
                Err(PackRegistryErrorV1::GoldenExecutionFailed(digest))
                    if digest == fixture().digest
            ));
        }
    }

    #[test]
    fn registry_reports_missing_and_wrong_executor_schema_and_codec_artifacts() {
        let status = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        };

        let mut missing_executor = entry(status);
        missing_executor.executor = None;
        assert!(matches!(
            PackRegistryV1::try_new([missing_executor]),
            Err(PackRegistryErrorV1::MissingExecutor(digest))
                if digest == fixture().digest
        ));

        let counts = Arc::new(CallbackCounts::default());
        let mut wrong_executor = ControlledPack::good(counts);
        let mut wrong_descriptor = fixture().descriptor.clone();
        wrong_descriptor.name = "Wrong executor descriptor".to_owned();
        wrong_executor.descriptor = Box::leak(Box::new(wrong_descriptor));
        assert!(matches!(
            PackRegistryV1::try_new([entry_with_executor(status, wrong_executor)]),
            Err(PackRegistryErrorV1::WrongExecutor(digest))
                if digest == fixture().digest
        ));

        let mut missing_schema = entry(status);
        let schemas = missing_schema
            .artifacts
            .schemas
            .as_mut()
            .unwrap_or_else(|| unreachable!("fixture schema bundle"));
        schemas.schemas.remove("fixture/value/v1");
        rebind_schema_bundle(&mut missing_schema);
        assert!(matches!(
            PackRegistryV1::try_new([missing_schema]),
            Err(PackRegistryErrorV1::MissingSchema { schema_id, .. })
                if schema_id == "fixture/value/v1"
        ));

        let mut wrong_schema = entry(status);
        let schemas = wrong_schema
            .artifacts
            .schemas
            .as_mut()
            .unwrap_or_else(|| unreachable!("fixture schema bundle"));
        let replacement = PackSchemaV1::new(
            "fixture/value/v1",
            json(r#"{"additionalProperties":false,"type":"object"}"#),
        )
        .unwrap_or_else(|error| unreachable!("replacement schema: {error}"));
        schemas
            .schemas
            .insert("fixture/value/v1".to_owned(), replacement);
        rebind_schema_bundle(&mut wrong_schema);
        assert!(matches!(
            PackRegistryV1::try_new([wrong_schema]),
            Err(PackRegistryErrorV1::WrongSchema { schema_id, .. })
                if schema_id == "fixture/value/v1"
        ));

        let mut missing_codecs = entry(status);
        missing_codecs.artifacts.codecs = None;
        assert!(matches!(
            PackRegistryV1::try_new([missing_codecs]),
            Err(PackRegistryErrorV1::MissingCodecBundle(digest))
                if digest == fixture().digest
        ));
    }

    #[test]
    fn sealed_executor_provenance_rejects_an_equivalent_distinct_constructor() {
        let mut candidate = entry_with_executor(
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
            EquivalentFixturePack,
        );
        candidate.executor_provenance = Some(reviewed_executor_provenance::<FixturePack>(
            fixture().artifact.clone(),
        ));
        assert!(matches!(
            PackRegistryV1::try_new([candidate]),
            Err(PackRegistryErrorV1::WrongExecutorProvenance(digest))
                if digest == fixture().digest
        ));
    }

    #[test]
    fn retained_codec_is_executable_complete_and_fail_closed() {
        let codec = CanonicalPackCodecV1::canonical_v1();
        codec
            .validate_frozen_vectors()
            .unwrap_or_else(|error| unreachable!("canonical codec vectors: {error}"));
        assert!(codec
            .decode_view(
                br#"{"action_offers":[],"extra":true,"projection":{},"projection_schema":"fixture/view/v1"}"#
            )
            .is_err());

        let status = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        };
        let mut missing_implementation = entry(status);
        missing_implementation.artifacts.codec_implementation = None;
        assert!(matches!(
            PackRegistryV1::try_new([missing_implementation]),
            Err(PackRegistryErrorV1::MissingCodecImplementation(digest))
                if digest == fixture().digest
        ));

        let mut missing_kind = entry(status);
        let codecs = missing_kind
            .artifacts
            .codecs
            .as_mut()
            .unwrap_or_else(|| unreachable!("fixture codec bundle"));
        codecs.kinds.remove(&PackCodecKindV1::TimerRequest);
        missing_kind.revision_lock.codec_bundle_digest = codecs
            .digest()
            .unwrap_or_else(|error| unreachable!("changed codec digest: {error}"));
        let digest = missing_kind
            .revision_lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("changed revision digest: {error}"));
        missing_kind.artifacts.expected_revision_digest = digest.clone();
        let mut descriptor = missing_kind.descriptor.clone();
        descriptor.revision_digest = digest;
        missing_kind.descriptor = descriptor;
        assert!(matches!(
            PackRegistryV1::try_new([missing_kind]),
            Err(PackRegistryErrorV1::MissingCodec {
                kind: PackCodecKindV1::TimerRequest,
                ..
            })
        ));
    }

    #[test]
    fn every_pack_callback_panic_is_contained() {
        let descriptor_counts = Arc::new(CallbackCounts::default());
        let mut descriptor_pack = ControlledPack::good(Arc::clone(&descriptor_counts));
        descriptor_pack.panic_on = Some(ActivityPackOperationV1::Descriptor);
        assert!(matches!(
            PackRegistryV1::try_new([entry_with_executor(
                PackRegistryStatusV1 {
                    selectable_for_new_rooms: true,
                    runnable_for_retained_rooms: true,
                },
                descriptor_pack,
            )]),
            Err(PackRegistryErrorV1::DescriptorPanicked(_))
        ));

        let initialize_counts = Arc::new(CallbackCounts::default());
        let mut initialize_pack = ControlledPack::good(Arc::clone(&initialize_counts));
        initialize_pack.panic_on = Some(ActivityPackOperationV1::Initialize);
        let initialize_host = checked_host(initialize_pack);
        assert!(matches!(
            initialize_host.initialize(&genesis_request()),
            Err(PackFaultV1::OperationPanicked(
                ActivityPackOperationV1::Initialize
            ))
        ));

        let trace = trace_with_activity(json("{}"));
        let view_counts = Arc::new(CallbackCounts::default());
        let mut view_pack = ControlledPack::good(Arc::clone(&view_counts));
        view_pack.panic_on = Some(ActivityPackOperationV1::View);
        assert!(matches!(
            current_view(&checked_host(view_pack), &trace),
            Err(PackFaultV1::OperationPanicked(
                ActivityPackOperationV1::View
            ))
        ));

        let reduce_counts = Arc::new(CallbackCounts::default());
        let mut reduce_pack = ControlledPack::good(Arc::clone(&reduce_counts));
        reduce_pack.panic_on = Some(ActivityPackOperationV1::Reduce);
        let reduce_host = checked_host(reduce_pack);
        let view = current_view(&reduce_host, &trace)
            .unwrap_or_else(|error| unreachable!("valid view: {error}"));
        assert!(matches!(
            reduce_action(
                &reduce_host,
                &trace,
                participant_action(trace.head()),
                Some(&view)
            ),
            Err(ActivityPackReduceErrorV1::Pack(
                PackFaultV1::OperationPanicked(ActivityPackOperationV1::Reduce)
            ))
        ));

        let observe_counts = Arc::new(CallbackCounts::default());
        let mut observe_pack = ControlledPack::good(Arc::clone(&observe_counts));
        observe_pack.panic_on = Some(ActivityPackOperationV1::Observe);
        let observe_host = checked_host(observe_pack);
        let transition = activity_transition(json("{}"), json("{}"), fixture().digest.clone());
        assert!(matches!(
            observe_with_stimulus(
                &observe_host,
                &transition.core_before,
                &transition.activity_before,
                &transition.head_before,
                transition.after.core_state(),
                transition.after.activity_state(),
                transition.after.head(),
                &transition.stimulus,
            ),
            Err(PackFaultV1::OperationPanicked(
                ActivityPackOperationV1::Observe
            ))
        ));
        assert_eq!(observe_counts.observe.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn action_admission_fails_before_reduce_without_faulting_the_pack() {
        let trace = trace_with_activity(json("{}"));
        let counts = Arc::new(CallbackCounts::default());
        let mut pack = ControlledPack::good(Arc::clone(&counts));
        pack.offer_behavior = OfferBehavior::Never;
        let host = checked_host(pack);
        let view = current_view(&host, &trace)
            .unwrap_or_else(|error| unreachable!("valid empty-offer view: {error}"));
        let core_before = trace.core_state().clone();
        let activity_before = trace.activity_state().clone();
        assert!(matches!(
            reduce_action(
                &host,
                &trace,
                participant_action(trace.head()),
                Some(&view)
            ),
            Err(ActivityPackReduceErrorV1::Admission(
                ActionAdmissionErrorV1::ActionNotAllowed(action)
            )) if action == "increment"
        ));
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(trace.core_state(), &core_before);
        assert_eq!(trace.activity_state(), &activity_before);

        let counts = Arc::new(CallbackCounts::default());
        let host = checked_host(ControlledPack::good(Arc::clone(&counts)));
        let view = current_view(&host, &trace)
            .unwrap_or_else(|error| unreachable!("valid offered view: {error}"));
        let mut wrong_schema = participant_action(trace.head());
        wrong_schema.payload_schema_digest = Blake3DigestV1::hash(b"wrong payload schema");
        assert!(matches!(
            reduce_action(&host, &trace, wrong_schema, Some(&view)),
            Err(ActivityPackReduceErrorV1::Admission(
                ActionAdmissionErrorV1::InvalidPayload(_)
            ))
        ));
        let mut stale = participant_action(trace.head());
        stale.exact_basis_head = trace_with_activity(json(r#"{"other":1}"#)).head().clone();
        assert!(matches!(
            reduce_action(&host, &trace, stale, Some(&view)),
            Err(ActivityPackReduceErrorV1::Admission(
                ActionAdmissionErrorV1::StaleBasis
            ))
        ));
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn invocation_hook_distinguishes_admission_from_an_invoked_pack_fault() {
        let trace = trace_with_activity(json("{}"));

        let admission_counts = Arc::new(CallbackCounts::default());
        let mut no_offer_pack = ControlledPack::good(Arc::clone(&admission_counts));
        no_offer_pack.offer_behavior = OfferBehavior::Never;
        let no_offer_host = checked_host(no_offer_pack);
        let no_offer_view = current_view(&no_offer_host, &trace)
            .unwrap_or_else(|error| unreachable!("valid no-offer view: {error}"));
        let admission_invocations = AtomicUsize::new(0);
        assert!(matches!(
            reduce_action_with_hook(
                &no_offer_host,
                &trace,
                participant_action(trace.head()),
                Some(&no_offer_view),
                || {
                    admission_invocations.fetch_add(1, AtomicOrdering::Relaxed);
                },
            ),
            Err(ActivityPackReduceErrorV1::Admission(
                ActionAdmissionErrorV1::ActionNotAllowed(_)
            ))
        ));
        assert_eq!(admission_invocations.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(admission_counts.reduce.load(AtomicOrdering::Relaxed), 0);

        let fault_counts = Arc::new(CallbackCounts::default());
        let mut panic_pack = ControlledPack::good(Arc::clone(&fault_counts));
        panic_pack.panic_on = Some(ActivityPackOperationV1::Reduce);
        let panic_host = checked_host(panic_pack);
        let offered_view = current_view(&panic_host, &trace)
            .unwrap_or_else(|error| unreachable!("valid offered view: {error}"));
        let fault_invocations = AtomicUsize::new(0);
        assert!(matches!(
            reduce_action_with_hook(
                &panic_host,
                &trace,
                participant_action(trace.head()),
                Some(&offered_view),
                || {
                    fault_invocations.fetch_add(1, AtomicOrdering::Relaxed);
                },
            ),
            Err(ActivityPackReduceErrorV1::Pack(
                PackFaultV1::OperationPanicked(ActivityPackOperationV1::Reduce)
            ))
        ));
        assert_eq!(fault_invocations.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(fault_counts.reduce.load(AtomicOrdering::Relaxed), 1);
    }

    #[test]
    fn substituted_state_cannot_be_labeled_with_an_unrelated_head() {
        let trace = trace_with_activity(json("{}"));
        let substituted = json(r#"{"offered":true}"#);
        let counts = Arc::new(CallbackCounts::default());
        let host = checked_host(ControlledPack::good(Arc::clone(&counts)));
        assert!(matches!(
            host.view(&ViewInputV1 {
                core: trace.core_state(),
                activity_state: &substituted,
                complete_head: trace.head(),
                viewer: &PackViewerV1::Participant(parsed(MEMBER)),
            }),
            Err(PackFaultV1::PrivacyContract(_))
        ));
        assert_eq!(counts.view.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn old_action_and_view_cannot_reduce_a_newer_verified_basis() {
        let transition = activity_transition(
            json("{}"),
            json(r#"{"advanced":true}"#),
            fixture().digest.clone(),
        );
        let counts = Arc::new(CallbackCounts::default());
        let host = checked_host(ControlledPack::good(Arc::clone(&counts)));
        let old_view = host
            .view(&ViewInputV1 {
                core: &transition.core_before,
                activity_state: &transition.activity_before,
                complete_head: &transition.head_before,
                viewer: &PackViewerV1::Participant(parsed(MEMBER)),
            })
            .unwrap_or_else(|error| unreachable!("valid old view: {error}"));
        let old_action = participant_action(&transition.head_before);
        let timers = BTreeMap::new();
        let stimulus = RecordedStimulusV1::ParticipantAction(old_action);
        let result = host.reduce(
            &ActivityReduceInputV1 {
                prior_activity_state: transition.after.activity_state(),
                core_before: transition.after.core_state(),
                proposed_core_after: transition.after.core_state(),
                scheduled_timers: &timers,
                next_room_seq: transition
                    .after
                    .head()
                    .room_seq()
                    .checked_successor()
                    .unwrap_or_else(|error| unreachable!("fixture successor: {error}")),
                recorded_stimulus: &stimulus,
            },
            transition.after.head(),
            &parsed(ROOM_SEED),
            Some(&old_view),
        );
        assert!(matches!(
            result,
            Err(ActivityPackReduceErrorV1::Admission(
                ActionAdmissionErrorV1::StaleBasis
            ))
        ));
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn action_offers_obey_collection_and_text_bounds_independently() {
        let counts = Arc::new(CallbackCounts::default());
        let mut two_offer_descriptor = fixture().descriptor.clone();
        two_offer_descriptor.actions.push(ActionDefinitionV1 {
            action_type: "second_increment".to_owned(),
            payload_schema: two_offer_descriptor.actions[0].payload_schema.clone(),
        });
        two_offer_descriptor.limits.maximum_collection_items = 1;
        let mut two_offer_pack = ControlledPack::good(Arc::clone(&counts));
        two_offer_pack.offer_behavior = OfferBehavior::AllDeclared;
        let host = checked_host_with_descriptor(two_offer_descriptor, two_offer_pack);
        let trace = trace_with_core_and_digest(
            initial_core(),
            json("{}"),
            host.descriptor().revision_digest.clone(),
        );
        assert!(matches!(
            current_view(&host, &trace),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::View,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(detail)
                if detail.contains("Action Offers count 2"))
        ));

        let counts = Arc::new(CallbackCounts::default());
        let mut long_action_descriptor = fixture().descriptor.clone();
        long_action_descriptor.actions[0].action_type = "x".repeat(100);
        long_action_descriptor.limits.maximum_text_bytes = 80;
        let host = checked_host_with_descriptor(
            long_action_descriptor,
            ControlledPack::good(Arc::clone(&counts)),
        );
        let trace = trace_with_core_and_digest(
            initial_core(),
            json("{}"),
            host.descriptor().revision_digest.clone(),
        );
        assert!(matches!(
            current_view(&host, &trace),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::View,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(_))
        ));
        assert_eq!(counts.view.load(AtomicOrdering::Relaxed), 1);

        let long_role = "r".repeat(100);
        let mut long_role_descriptor = fixture().descriptor.clone();
        long_role_descriptor.roles = vec![RoleDefinitionV1 {
            role: long_role.clone(),
            minimum: 1,
            maximum: 1,
        }];
        long_role_descriptor.limits.maximum_text_bytes = 80;
        let mut long_role_pack = ControlledPack::good(Arc::new(CallbackCounts::default()));
        long_role_pack.offer_behavior = OfferBehavior::Never;
        let host = checked_host_with_descriptor(long_role_descriptor, long_role_pack);
        let membership = MembershipV1::new(
            parsed(MEMBER),
            parsed(PRINCIPAL),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some(long_role),
        )
        .unwrap_or_else(|error| unreachable!("valid long-role Membership: {error}"));
        let core = CoreRoomStateV1::active([membership])
            .unwrap_or_else(|error| unreachable!("valid long-role Core: {error}"));
        let trace =
            trace_with_core_and_digest(core, json("{}"), host.descriptor().revision_digest.clone());
        assert!(matches!(
            current_view(&host, &trace),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::View,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(_))
        ));
    }

    #[test]
    fn malformed_undeclared_and_excessive_reduce_outputs_fail_before_commit() {
        let trace = trace_with_activity(json("{}"));
        for (behavior, expected) in [
            (ReduceBehavior::RejectUndeclared, "undeclared"),
            (ReduceBehavior::InvalidState, "schema"),
            (ReduceBehavior::ExcessiveState, "bound"),
        ] {
            let counts = Arc::new(CallbackCounts::default());
            let mut pack = ControlledPack::good(Arc::clone(&counts));
            pack.reduce_behavior = behavior;
            let host = checked_host(pack);
            let view = current_view(&host, &trace)
                .unwrap_or_else(|error| unreachable!("valid view: {error}"));
            let result =
                reduce_action(&host, &trace, participant_action(trace.head()), Some(&view));
            let error = result
                .err()
                .unwrap_or_else(|| unreachable!("malicious output must fail"));
            match (expected, error) {
                (
                    "undeclared",
                    ActivityPackReduceErrorV1::Pack(PackFaultV1::OperationFault {
                        operation: ActivityPackOperationV1::Reduce,
                        fault,
                    }),
                ) if matches!(fault.as_ref(), PackFaultV1::UndeclaredRejectionCode(_)) => {}
                (
                    "schema",
                    ActivityPackReduceErrorV1::Pack(PackFaultV1::OperationFault {
                        operation: ActivityPackOperationV1::Reduce,
                        fault,
                    }),
                ) if matches!(fault.as_ref(), PackFaultV1::SchemaViolation(_)) => {}
                (
                    "bound",
                    ActivityPackReduceErrorV1::Pack(PackFaultV1::OperationFault {
                        operation: ActivityPackOperationV1::Reduce,
                        fault,
                    }),
                ) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(_)) => {}
                (_, other) => unreachable!("unexpected malicious-output error: {other}"),
            }
            assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 1);
        }
    }

    #[test]
    fn observe_none_is_only_valid_for_a_complete_hidden_view() {
        let transition = activity_transition(
            json(r#"{"count":0}"#),
            json(r#"{"count":1}"#),
            fixture().digest.clone(),
        );

        let visible_counts = Arc::new(CallbackCounts::default());
        let mut visible_pack = ControlledPack::good(Arc::clone(&visible_counts));
        visible_pack.projection_behavior = ProjectionBehavior::ActivityState;
        let visible_host = checked_host(visible_pack);
        assert!(matches!(
            observe_with_stimulus(
                &visible_host,
                &transition.core_before,
                &transition.activity_before,
                &transition.head_before,
                transition.after.core_state(),
                transition.after.activity_state(),
                transition.after.head(),
                &transition.stimulus,
            ),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Observe,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::PrivacyContract(_))
        ));

        let hidden_counts = Arc::new(CallbackCounts::default());
        let hidden_host = checked_host(ControlledPack::good(Arc::clone(&hidden_counts)));
        assert!(matches!(
            observe_with_stimulus(
                &hidden_host,
                &transition.core_before,
                &transition.activity_before,
                &transition.head_before,
                transition.after.core_state(),
                transition.after.activity_state(),
                transition.after.head(),
                &transition.stimulus,
            ),
            Ok(ActivityObservationOutcomeV1::Hidden)
        ));
    }

    #[test]
    fn observe_rejects_non_successor_heads_before_any_pack_callback() {
        let mut trace = trace_with_activity(json("{}"));
        let core_h0 = trace.core_state().clone();
        let activity_h0 = trace.activity_state().clone();
        let head_h0 = trace.head().clone();
        trace
            .advance(RecordedStimulusV1::ParticipantAction(participant_action(
                trace.head(),
            )))
            .unwrap_or_else(|error| unreachable!("valid first transition: {error}"));
        let mut second_action = participant_action(trace.head());
        second_action.action_id = parsed(ACTION_TWO);
        second_action.admitted_at = parsed("2026-08-15T12:00:02Z");
        let second_stimulus = RecordedStimulusV1::ParticipantAction(second_action);
        trace
            .advance(second_stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid second transition: {error}"));

        let counts = Arc::new(CallbackCounts::default());
        let host = checked_host(ControlledPack::good(Arc::clone(&counts)));
        assert!(matches!(
            host.observe(&ObserveTransitionInputV1 {
                core_before: &core_h0,
                activity_before: &activity_h0,
                head_before: &head_h0,
                core_after: trace.core_state(),
                activity_after: trace.activity_state(),
                head_after: trace.head(),
                recorded_stimulus: &second_stimulus,
                ordered_domain_events: &[],
                viewer: &PackViewerV1::Participant(parsed(MEMBER)),
            }),
            Err(PackFaultV1::PrivacyContract(_))
        ));
        assert_eq!(counts.view.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(counts.observe.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn changed_offers_reuse_the_exact_after_view_container() {
        let transition = activity_transition(
            json(r#"{"offered":false}"#),
            json(r#"{"offered":true}"#),
            fixture().digest.clone(),
        );
        let counts = Arc::new(CallbackCounts::default());
        let supplied = Arc::new(Mutex::new(None));
        let mut pack = ControlledPack::good(Arc::clone(&counts));
        pack.offer_behavior = OfferBehavior::StateFlag;
        pack.observation_behavior = ObservationBehavior::SomeWithAfterOffers;
        pack.supplied_after_offers = Arc::clone(&supplied);
        let host = checked_host(pack);
        let result = observe_with_stimulus(
            &host,
            &transition.core_before,
            &transition.activity_before,
            &transition.head_before,
            transition.after.core_state(),
            transition.after.activity_state(),
            transition.after.head(),
            &transition.stimulus,
        )
        .unwrap_or_else(|error| unreachable!("valid changed-offer observation: {error}"));
        let ActivityObservationOutcomeV1::Observation(observation) = result else {
            unreachable!("changed offers require one observation");
        };
        let emitted = observation
            .action_offers()
            .unwrap_or_else(|| unreachable!("changed offers are present"));
        let supplied = supplied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            emitted.shares_storage_with(
                supplied
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("pack received exact offers"))
            )
        );
    }

    #[test]
    fn authorized_core_changes_are_visible_even_when_pack_bytes_are_identical() {
        let counts = Arc::new(CallbackCounts::default());
        let host = checked_host(ControlledPack::good(Arc::clone(&counts)));

        let mut role_trace = trace_with_activity(json("{}"));
        let role_before_core = role_trace.core_state().clone();
        let role_before_activity = role_trace.activity_state().clone();
        let role_before_head = role_trace.head().clone();
        let role_change = MembershipChangeV1::role_change(
            role_trace
                .core_state()
                .membership(&parsed(MEMBER))
                .unwrap_or_else(|| unreachable!("fixture Membership"))
                .clone(),
            "counter2",
        )
        .unwrap_or_else(|error| unreachable!("valid Role change: {error}"));
        let role_stimulus = administration_stimulus(
            &role_trace,
            CoreProposedKindV1::RoleChange,
            CoreChangeSetV1::one(role_change),
            "role-change",
        );
        role_trace
            .advance(role_stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid Role transition: {error}"));
        assert!(matches!(
            observe_with_stimulus(
                &host,
                &role_before_core,
                &role_before_activity,
                &role_before_head,
                role_trace.core_state(),
                role_trace.activity_state(),
                role_trace.head(),
                &role_stimulus,
            ),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Observe,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::PrivacyContract(_))
        ));

        let mut archive_trace = trace_with_activity(json("{}"));
        let archive_before_core = archive_trace.core_state().clone();
        let archive_before_activity = archive_trace.activity_state().clone();
        let archive_before_head = archive_trace.head().clone();
        let archive_stimulus = administration_stimulus(
            &archive_trace,
            CoreProposedKindV1::Archive,
            CoreChangeSetV1::archive(RoomStatusV1::Active),
            "archive",
        );
        archive_trace
            .advance(archive_stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid archive transition: {error}"));
        assert!(matches!(
            observe_with_stimulus(
                &host,
                &archive_before_core,
                &archive_before_activity,
                &archive_before_head,
                archive_trace.core_state(),
                archive_trace.activity_state(),
                archive_trace.head(),
                &archive_stimulus,
            ),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Observe,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::PrivacyContract(_))
        ));
        assert_eq!(counts.observe.load(AtomicOrdering::Relaxed), 2);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn access_class_change_and_visibility_loss_are_host_delivery_outcomes() {
        let reset_counts = Arc::new(CallbackCounts::default());
        let reset_host = checked_host(ControlledPack::good(Arc::clone(&reset_counts)));
        let mut reset_trace = trace_with_activity(json("{}"));
        let before_core = reset_trace.core_state().clone();
        let before_activity = reset_trace.activity_state().clone();
        let before_head = reset_trace.head().clone();
        let access_change = MembershipChangeV1::access_mode_change(
            reset_trace
                .core_state()
                .membership(&parsed(MEMBER))
                .unwrap_or_else(|| unreachable!("fixture Membership"))
                .clone(),
            AccessModeV1::Spectator,
            None,
        )
        .unwrap_or_else(|error| unreachable!("valid Access change: {error}"));
        let stimulus = administration_stimulus(
            &reset_trace,
            CoreProposedKindV1::AccessModeChange,
            CoreChangeSetV1::one(access_change),
            "access-change",
        );
        reset_trace
            .advance(stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid Access transition: {error}"));
        let reset = observe_with_stimulus(
            &reset_host,
            &before_core,
            &before_activity,
            &before_head,
            reset_trace.core_state(),
            reset_trace.activity_state(),
            reset_trace.head(),
            &stimulus,
        )
        .unwrap_or_else(|error| unreachable!("valid projection reset: {error}"));
        let ActivityObservationOutcomeV1::ProjectionReset(reset_view) = reset else {
            unreachable!("Access change requires a checked reset")
        };
        assert!(
            matches!(reset_view.viewer(), PackViewerV1::Public(member) if member == &parsed(MEMBER))
        );
        assert!(reset_view.action_offers().offers().is_empty());
        assert_eq!(reset_counts.view.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(reset_counts.observe.load(AtomicOrdering::Relaxed), 0);

        let reverse_counts = Arc::new(CallbackCounts::default());
        let reverse_host = checked_host(ControlledPack::good(Arc::clone(&reverse_counts)));
        let mut reverse_trace = trace_with_activity(json("{}"));
        let reverse_before_core = reverse_trace.core_state().clone();
        let reverse_before_activity = reverse_trace.activity_state().clone();
        let reverse_before_head = reverse_trace.head().clone();
        let reverse_change = MembershipChangeV1::access_mode_change(
            reverse_trace
                .core_state()
                .membership(&parsed(SPECTATOR))
                .unwrap_or_else(|| unreachable!("fixture spectator"))
                .clone(),
            AccessModeV1::Participant,
            Some("counter2".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("valid reverse Access change: {error}"));
        let reverse_stimulus = administration_stimulus(
            &reverse_trace,
            CoreProposedKindV1::AccessModeChange,
            CoreChangeSetV1::one(reverse_change),
            "reverse-access-change",
        );
        reverse_trace
            .advance(reverse_stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid reverse Access transition: {error}"));
        let reverse = reverse_host
            .observe(&ObserveTransitionInputV1 {
                core_before: &reverse_before_core,
                activity_before: &reverse_before_activity,
                head_before: &reverse_before_head,
                core_after: reverse_trace.core_state(),
                activity_after: reverse_trace.activity_state(),
                head_after: reverse_trace.head(),
                recorded_stimulus: &reverse_stimulus,
                ordered_domain_events: &[],
                viewer: &PackViewerV1::Participant(parsed(SPECTATOR)),
            })
            .unwrap_or_else(|error| unreachable!("valid reverse reset: {error}"));
        let ActivityObservationOutcomeV1::ProjectionReset(reverse_view) = reverse else {
            unreachable!("reverse Access change requires a checked reset")
        };
        assert_eq!(reverse_view.action_offers().offers().len(), 1);
        assert_eq!(reverse_counts.view.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(reverse_counts.observe.load(AtomicOrdering::Relaxed), 0);

        let mut join_trace = trace_with_activity(json("{}"));
        let join_before_core = join_trace.core_state().clone();
        let join_before_activity = join_trace.activity_state().clone();
        let join_before_head = join_trace.head().clone();
        let joined = MembershipV1::new(
            parsed(MEMBER_TWO),
            parsed(PRINCIPAL_TWO),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter2".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("valid joined Membership: {error}"));
        let join_stimulus = administration_stimulus(
            &join_trace,
            CoreProposedKindV1::Join,
            CoreChangeSetV1::one(MembershipChangeV1::join(joined)),
            "join",
        );
        join_trace
            .advance(join_stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid Join transition: {error}"));
        let join_input = ObserveTransitionInputV1 {
            core_before: &join_before_core,
            activity_before: &join_before_activity,
            head_before: &join_before_head,
            core_after: join_trace.core_state(),
            activity_after: join_trace.activity_state(),
            head_after: join_trace.head(),
            recorded_stimulus: &join_stimulus,
            ordered_domain_events: &[],
            viewer: &PackViewerV1::Participant(parsed(MEMBER_TWO)),
        };
        let join_counts = Arc::new(CallbackCounts::default());
        let join_host = checked_host(ControlledPack::good(Arc::clone(&join_counts)));
        assert!(matches!(
            join_host.observe(&join_input),
            Ok(ActivityObservationOutcomeV1::ProjectionReset(_))
        ));
        assert_eq!(join_counts.view.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(join_counts.observe.load(AtomicOrdering::Relaxed), 0);

        let panic_counts = Arc::new(CallbackCounts::default());
        let mut panic_pack = ControlledPack::good(Arc::clone(&panic_counts));
        panic_pack.panic_on = Some(ActivityPackOperationV1::View);
        assert!(matches!(
            checked_host(panic_pack).observe(&join_input),
            Err(PackFaultV1::OperationPanicked(
                ActivityPackOperationV1::View
            ))
        ));
        assert_eq!(panic_counts.view.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(panic_counts.observe.load(AtomicOrdering::Relaxed), 0);

        let oversize_counts = Arc::new(CallbackCounts::default());
        let mut oversize_pack = ControlledPack::good(Arc::clone(&oversize_counts));
        oversize_pack.projection_behavior = ProjectionBehavior::Oversize;
        assert!(matches!(
            checked_host(oversize_pack).observe(&join_input),
            Err(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::View,
                fault,
            }) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(_))
        ));
        assert_eq!(oversize_counts.view.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(oversize_counts.observe.load(AtomicOrdering::Relaxed), 0);

        let loss_counts = Arc::new(CallbackCounts::default());
        let loss_host = checked_host(ControlledPack::good(Arc::clone(&loss_counts)));
        let mut loss_trace = trace_with_activity(json("{}"));
        let before_core = loss_trace.core_state().clone();
        let before_activity = loss_trace.activity_state().clone();
        let before_head = loss_trace.head().clone();
        let suspend = MembershipChangeV1::suspend(
            loss_trace
                .core_state()
                .membership(&parsed(MEMBER))
                .unwrap_or_else(|| unreachable!("fixture Membership"))
                .clone(),
        );
        let stimulus = administration_stimulus(
            &loss_trace,
            CoreProposedKindV1::Suspend,
            CoreChangeSetV1::one(suspend),
            "suspend",
        );
        loss_trace
            .advance(stimulus.clone())
            .unwrap_or_else(|error| unreachable!("valid suspend transition: {error}"));
        assert!(matches!(
            observe_with_stimulus(
                &loss_host,
                &before_core,
                &before_activity,
                &before_head,
                loss_trace.core_state(),
                loss_trace.activity_state(),
                loss_trace.head(),
                &stimulus,
            ),
            Ok(ActivityObservationOutcomeV1::VisibilityLost)
        ));
        assert_eq!(loss_counts.observe.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(loss_counts.view.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn mandatory_membership_change_set_cannot_be_cleanly_rejected() {
        let member_one = MembershipV1::new(
            parsed(MEMBER),
            parsed(PRINCIPAL),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("valid first Membership: {error}"));
        let member_two = MembershipV1::new(
            parsed(MEMBER_TWO),
            parsed(PRINCIPAL_TWO),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter2".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("valid second Membership: {error}"));
        let core_before = CoreRoomStateV1::active([member_one.clone(), member_two.clone()])
            .unwrap_or_else(|error| unreachable!("valid two-member Core: {error}"));
        let changeset = CoreChangeSetV1::atomic(vec![
            MembershipChangeV1::suspend(member_one),
            MembershipChangeV1::depart(member_two),
        ])
        .unwrap_or_else(|error| unreachable!("valid mandatory changeset: {error}"));
        let trace = trace_with_core(core_before.clone(), json("{}"));
        let stimulus = administration_stimulus(
            &trace,
            CoreProposedKindV1::MembershipChangeSet,
            changeset,
            "mandatory-set",
        );
        let RecordedStimulusV1::CoreProposed(proposal) = &stimulus else {
            unreachable!("administration fixture");
        };
        let core_reducer = crate::CoreReducerV1::new(|_| Ok(()));
        let verified = core_reducer
            .validate_state(core_before.clone())
            .unwrap_or_else(|error| unreachable!("valid before Core: {error}"));
        let proposed = core_reducer
            .reduce(&verified, proposal)
            .unwrap_or_else(|error| unreachable!("valid proposed Core: {error}"));
        let counts = Arc::new(CallbackCounts::default());
        let mut pack = ControlledPack::good(Arc::clone(&counts));
        pack.reduce_behavior = ReduceBehavior::RejectDeclared;
        let host = checked_host(pack);
        let timers = BTreeMap::new();
        let result = host.reduce(
            &ActivityReduceInputV1 {
                prior_activity_state: trace.activity_state(),
                core_before: &core_before,
                proposed_core_after: proposed.state(),
                scheduled_timers: &timers,
                next_room_seq: RoomSequenceV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
                recorded_stimulus: &stimulus,
            },
            trace.head(),
            &parsed(ROOM_SEED),
            None,
        );
        assert!(matches!(
            result,
            Err(ActivityPackReduceErrorV1::Pack(
                PackFaultV1::OperationFault {
                    operation: ActivityPackOperationV1::Reduce,
                    fault,
                }
            )) if matches!(fault.as_ref(), PackFaultV1::MandatoryStimulusRejected)
        ));
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(&core_before, verified.state());
        assert_eq!(trace.activity_state(), &json("{}"));
    }

    #[test]
    fn revision_digest_is_recomputed_and_every_lock_field_is_semantic() {
        let original = fixture().lock.clone();
        let original_digest = original
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("fixture lock: {error}"));
        let changed_digest = Blake3DigestV1::hash(b"changed");
        let mut mutations = Vec::new();

        let mut changed = original.clone();
        changed.revision_lock_id = "worldstream/pack-revision-lock/v2".to_owned();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.pack_id = "worldstream.other".to_owned();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.explanatory_version = "1.0.1".to_owned();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.host_contract = "worldstream/activity-pack/v2".to_owned();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.canonical_codec = "worldstream/canonical-json/v2".to_owned();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.descriptor_digest = changed_digest.clone();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.schema_bundle_digest = changed_digest.clone();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.codec_bundle_digest = changed_digest.clone();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.deterministic_static_data_digests = vec![NamedDigestV1 {
            name: "fixture".to_owned(),
            digest: changed_digest.clone(),
        }];
        mutations.push(changed);
        let mut changed = original.clone();
        changed.rule_source_digest = changed_digest.clone();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.deterministic_dependency_lock_digest = changed_digest;
        mutations.push(changed);

        for changed in &mutations {
            assert_ne!(
                original_digest,
                changed
                    .revision_digest()
                    .unwrap_or_else(|error| unreachable!("changed lock: {error}"))
            );
        }

        let mut mismatched = entry(PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        });
        mismatched.revision_lock = mutations
            .pop()
            .unwrap_or_else(|| unreachable!("mutation corpus is nonempty"));
        assert!(matches!(
            PackRegistryV1::try_new([mismatched]),
            Err(PackRegistryErrorV1::RevisionDigestMismatch(_))
        ));
    }

    #[test]
    fn registry_rejects_empty_and_ambiguous_descriptor_vocabularies() {
        assert!(matches!(
            PackRegistryV1::try_new(Vec::<PackRegistryEntryV1>::new()),
            Err(PackRegistryErrorV1::EmptyRegistry)
        ));

        let mut duplicate = fixture().descriptor.clone();
        duplicate.actions.push(duplicate.actions[0].clone());
        let duplicate = Box::leak(Box::new(duplicate));
        let fixture = fixture();
        let candidate = PackRegistryEntryV1::new(
            fixture.lock.clone(),
            duplicate,
            PackRegistryArtifactsV1 {
                expected_revision_digest: fixture.digest.clone(),
                schemas: Some(fixture.schemas.clone()),
                codecs: Some(fixture.codecs.clone()),
                codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
                executor_artifact_digest: fixture.artifact.clone(),
                golden_corpus_digest: fixture_golden()
                    .digest()
                    .unwrap_or_else(|error| unreachable!("fixture golden digest: {error}")),
                golden_corpus: Some(fixture_golden().clone()),
            },
            Some(reviewed_executor_provenance::<FixturePack>(
                fixture.artifact.clone(),
            )),
            Some(FixturePack),
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
        );
        assert!(matches!(
            PackRegistryV1::try_new([candidate]),
            Err(PackRegistryErrorV1::InvalidDescriptorShape { .. })
        ));
    }

    #[test]
    fn revision_lock_static_inputs_must_be_strictly_name_sorted() {
        let mut candidate = entry(PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        });
        candidate.revision_lock.deterministic_static_data_digests = vec![
            NamedDigestV1 {
                name: "z".to_owned(),
                digest: Blake3DigestV1::hash(b"z"),
            },
            NamedDigestV1 {
                name: "a".to_owned(),
                digest: Blake3DigestV1::hash(b"a"),
            },
        ];
        assert!(matches!(
            PackRegistryV1::try_new([candidate]),
            Err(PackRegistryErrorV1::InvalidRevisionLockShape { .. })
        ));
    }

    #[test]
    fn duplicate_semantic_digest_is_an_explicit_collision() {
        let status = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        };
        assert!(matches!(
            PackRegistryV1::try_new([entry(status), entry(status)]),
            Err(PackRegistryErrorV1::DigestCollision(digest)) if digest == fixture().digest
        ));
    }

    #[test]
    fn retained_and_selectable_statuses_are_independent_and_fail_closed() {
        let retained_only = PackRegistryStatusV1 {
            selectable_for_new_rooms: false,
            runnable_for_retained_rooms: true,
        };
        let registry = PackRegistryV1::try_new([entry(retained_only)])
            .unwrap_or_else(|error| unreachable!("valid registry: {error}"));
        assert!(registry.load_retained(&fixture().digest).is_ok());
        assert!(matches!(
            registry.select_for_new_room(&fixture().digest),
            Err(PackRegistryErrorV1::NotSelectable(digest)) if digest == fixture().digest
        ));

        let invalid = PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: false,
        };
        assert!(matches!(
            PackRegistryV1::try_new([entry(invalid)]),
            Err(PackRegistryErrorV1::InvalidStatus(digest)) if digest == fixture().digest
        ));
    }

    #[test]
    fn registry_reports_missing_revision_without_fallback() {
        let registry = PackRegistryV1::try_new([entry(PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
        })])
        .unwrap_or_else(|error| unreachable!("valid registry: {error}"));
        let missing: PackDigestV1 =
            "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .parse()
                .unwrap_or_else(|error| unreachable!("missing digest: {error}"));
        assert!(matches!(
            registry.load_retained(&missing),
            Err(PackRegistryErrorV1::MissingRevision(digest)) if digest == missing
        ));
    }

    #[test]
    fn registry_rejects_self_consistent_noncanonical_codec_bundle() {
        let mut wrong_codecs = fixture().codecs.clone();
        wrong_codecs.retained_reader_versions = vec![0, 1];
        let mut revision_lock = fixture().lock.clone();
        revision_lock.codec_bundle_digest = wrong_codecs
            .digest()
            .unwrap_or_else(|error| unreachable!("wrong codec digest: {error}"));
        let digest = revision_lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("wrong revision digest: {error}"));
        let mut descriptor = fixture().descriptor.clone();
        descriptor.revision_digest = digest.clone();
        let descriptor = Box::leak(Box::new(descriptor));
        let candidate = PackRegistryEntryV1::new(
            revision_lock,
            descriptor,
            PackRegistryArtifactsV1 {
                expected_revision_digest: digest.clone(),
                schemas: Some(fixture().schemas.clone()),
                codecs: Some(wrong_codecs),
                codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
                executor_artifact_digest: fixture().artifact.clone(),
                golden_corpus_digest: fixture_golden()
                    .digest()
                    .unwrap_or_else(|error| unreachable!("fixture golden digest: {error}")),
                golden_corpus: Some(fixture_golden().clone()),
            },
            Some(reviewed_executor_provenance::<FixturePack>(
                fixture().artifact.clone(),
            )),
            Some(FixturePack),
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
        );
        assert!(matches!(
            PackRegistryV1::try_new([candidate]),
            Err(PackRegistryErrorV1::WrongCodec(actual)) if actual == digest
        ));
    }

    // Keep imported legacy output types exercised here so this trait fixture
    // cannot accidentally drift to a parallel disposition family.
    #[test]
    fn seam_reuses_core_disposition_types() {
        let _ = ActivityDispositionV1::Apply(ActivityApplyV1 {
            next_activity_state: CanonicalJsonV1::parse(b"null")
                .unwrap_or_else(|error| unreachable!("canonical null: {error}")),
            ordered_domain_events: Vec::new(),
            timer_requests: Vec::new(),
            ordered_attention_signals: Vec::new(),
        });
        let _ = ActivityDispositionV1::Reject(ActivityRejectionV1 {
            declared_code: "fixture".to_owned(),
            bounded_safe_details: CanonicalJsonV1::parse(b"{}")
                .unwrap_or_else(|error| unreachable!("canonical object: {error}")),
        });
    }
}
