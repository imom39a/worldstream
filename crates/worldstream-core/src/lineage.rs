use serde::{Deserialize, Serialize};

use crate::{
    CanonicalJsonError, CanonicalJsonV1, CompleteHeadV1, CoreRoomStateV1, PackDigestV1,
    RecordedStimulusV1, RoomId, RoomSeedV1, RoomSequenceV1, ScheduledTimerV1, TimerChangeV1,
    canonical::encode,
    primitives::{Blake3DigestV1, CreationRecordedAt},
};

/// Frozen canonical JSON codec identity.
pub const CANONICAL_CODEC_ID: &str = "worldstream/canonical-json/v1";
/// Frozen hash suite identity from the compatibility manifest.
pub const HASH_SUITE_ID: &str = "blake3-canonical-json-v1";
/// Frozen Core schema identity from the compatibility manifest.
pub const CORE_SCHEMA_VERSION: &str = "worldstream.core-room-state.v1";
/// Immutable Genesis record version.
pub const GENESIS_VERSION: &str = "worldstream/genesis/v1";
/// Immutable Transition record version.
pub const TRANSITION_VERSION: &str = "worldstream/transition/v1";

const CORE_STATE_DOMAIN: &str = "worldstream/core-state/v1";
const ACTIVITY_STATE_DOMAIN: &str = "worldstream/activity-state/v1";
const AUTHORITATIVE_STATE_DOMAIN: &str = "worldstream/authoritative-state/v1";

#[derive(Serialize)]
struct CoreStateHashObject<'a> {
    domain: &'static str,
    core_schema: &'static str,
    core: &'a CoreRoomStateV1,
}

#[derive(Serialize)]
struct ActivityStateHashObject<'a> {
    domain: &'static str,
    pack_digest: &'a PackDigestV1,
    activity: &'a CanonicalJsonV1,
}

#[derive(Serialize)]
struct AuthoritativeStateHashObject<'a> {
    domain: &'static str,
    core_schema: &'static str,
    pack_digest: &'a PackDigestV1,
    core_state_hash: &'a Blake3DigestV1,
    activity_state_hash: &'a Blake3DigestV1,
}

pub(crate) fn hash_core_state(
    core: &CoreRoomStateV1,
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    Ok(Blake3DigestV1::hash(&core_state_hash_bytes(core)?))
}

pub(crate) fn core_state_hash_bytes(core: &CoreRoomStateV1) -> Result<Vec<u8>, CanonicalJsonError> {
    encode(&CoreStateHashObject {
        domain: CORE_STATE_DOMAIN,
        core_schema: CORE_SCHEMA_VERSION,
        core,
    })
}

pub(crate) fn hash_activity_state(
    pack_digest: &PackDigestV1,
    activity: &CanonicalJsonV1,
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    Ok(Blake3DigestV1::hash(&activity_state_hash_bytes(
        pack_digest,
        activity,
    )?))
}

pub(crate) fn activity_state_hash_bytes(
    pack_digest: &PackDigestV1,
    activity: &CanonicalJsonV1,
) -> Result<Vec<u8>, CanonicalJsonError> {
    encode(&ActivityStateHashObject {
        domain: ACTIVITY_STATE_DOMAIN,
        pack_digest,
        activity,
    })
}

pub(crate) fn hash_authoritative_state(
    pack_digest: &PackDigestV1,
    core_state_hash: &Blake3DigestV1,
    activity_state_hash: &Blake3DigestV1,
) -> Result<Blake3DigestV1, CanonicalJsonError> {
    Ok(Blake3DigestV1::hash(&authoritative_state_hash_bytes(
        pack_digest,
        core_state_hash,
        activity_state_hash,
    )?))
}

pub(crate) fn authoritative_state_hash_bytes(
    pack_digest: &PackDigestV1,
    core_state_hash: &Blake3DigestV1,
    activity_state_hash: &Blake3DigestV1,
) -> Result<Vec<u8>, CanonicalJsonError> {
    encode(&AuthoritativeStateHashObject {
        domain: AUTHORITATIVE_STATE_DOMAIN,
        core_schema: CORE_SCHEMA_VERSION,
        pack_digest,
        core_state_hash,
        activity_state_hash,
    })
}

#[derive(Serialize)]
struct GenesisHashObject<'a> {
    domain: &'static str,
    codec_id: &'static str,
    hash_suite: &'static str,
    room_id: &'a RoomId,
    core_schema: &'static str,
    pack_digest: &'a PackDigestV1,
    configuration: &'a CanonicalJsonV1,
    room_seed: &'a RoomSeedV1,
    created_at: &'a CreationRecordedAt,
    initial_timers: &'a [ScheduledTimerV1],
    initial_core_state_hash: &'a Blake3DigestV1,
    initial_activity_state_hash: &'a Blake3DigestV1,
    initial_authoritative_state_hash: &'a Blake3DigestV1,
}

#[derive(Serialize)]
struct TransitionHashObject<'a> {
    domain: &'static str,
    codec_id: &'static str,
    hash_suite: &'static str,
    room_id: &'a RoomId,
    room_seq: RoomSequenceV1,
    core_schema: &'static str,
    pack_digest: &'a PackDigestV1,
    previous_transition_or_genesis_hash: &'a Blake3DigestV1,
    recorded_stimulus: &'a RecordedStimulusV1,
    ordered_domain_events: &'a [CanonicalJsonV1],
    ordered_timer_changes: &'a [TimerChangeV1],
    ordered_attention_signals: &'a [CanonicalJsonV1],
    resulting_core_state_hash: &'a Blake3DigestV1,
    resulting_activity_state_hash: &'a Blake3DigestV1,
    resulting_authoritative_state_hash: &'a Blake3DigestV1,
}

/// Immutable reconstructible Genesis plus its complete initial hashes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenesisV1 {
    /// Record version identity.
    pub(crate) genesis_version: String,
    /// Canonical codec identity.
    pub(crate) codec_id: String,
    /// Hash suite identity.
    pub(crate) hash_suite: String,
    /// Generated Room identity.
    pub(crate) room_id: RoomId,
    /// Exact Core schema identity.
    pub(crate) core_schema_version: String,
    /// Exact immutable pack digest.
    pub(crate) pack_digest: PackDigestV1,
    /// Canonical configuration.
    pub(crate) configuration: CanonicalJsonV1,
    /// Recorded Room seed.
    pub(crate) room_seed: RoomSeedV1,
    /// Logical creation time.
    pub(crate) created_at: CreationRecordedAt,
    /// Initial normalized scheduled Timers.
    pub(crate) initial_timers: Vec<ScheduledTimerV1>,
    /// Reconstructible initial Core state.
    pub(crate) initial_core_state: CoreRoomStateV1,
    /// Reconstructible initial opaque Activity state.
    pub(crate) initial_activity_state: CanonicalJsonV1,
    /// Initial Core hash.
    pub(crate) initial_core_state_hash: Blake3DigestV1,
    /// Initial Activity hash.
    pub(crate) initial_activity_state_hash: Blake3DigestV1,
    /// Initial aggregate hash.
    pub(crate) initial_authoritative_state_hash: Blake3DigestV1,
    /// Genesis lineage hash.
    pub(crate) genesis_hash: Blake3DigestV1,
}

impl GenesisV1 {
    /// Strictly decodes original persisted bytes and requires byte equality
    /// after canonical re-encoding. Replay never accepts a normalized record.
    pub(crate) fn from_canonical_bytes(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnverifiedGenesis {
            genesis_version: String,
            codec_id: String,
            hash_suite: String,
            room_id: RoomId,
            core_schema_version: String,
            pack_digest: PackDigestV1,
            configuration: CanonicalJsonV1,
            room_seed: RoomSeedV1,
            created_at: CreationRecordedAt,
            initial_timers: Vec<ScheduledTimerV1>,
            initial_core_state: CoreRoomStateV1,
            initial_activity_state: CanonicalJsonV1,
            initial_core_state_hash: Blake3DigestV1,
            initial_activity_state_hash: Blake3DigestV1,
            initial_authoritative_state_hash: Blake3DigestV1,
            genesis_hash: Blake3DigestV1,
        }

        let raw: UnverifiedGenesis = CanonicalJsonV1::decode_canonical(input)?;
        Ok(Self {
            genesis_version: raw.genesis_version,
            codec_id: raw.codec_id,
            hash_suite: raw.hash_suite,
            room_id: raw.room_id,
            core_schema_version: raw.core_schema_version,
            pack_digest: raw.pack_digest,
            configuration: raw.configuration,
            room_seed: raw.room_seed,
            created_at: raw.created_at,
            initial_timers: raw.initial_timers,
            initial_core_state: raw.initial_core_state,
            initial_activity_state: raw.initial_activity_state,
            initial_core_state_hash: raw.initial_core_state_hash,
            initial_activity_state_hash: raw.initial_activity_state_hash,
            initial_authoritative_state_hash: raw.initial_authoritative_state_hash,
            genesis_hash: raw.genesis_hash,
        })
    }

    pub(crate) fn calculate_hash(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&self.hash_input_bytes()?))
    }

    pub(crate) fn hash_input_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&GenesisHashObject {
            domain: GENESIS_VERSION,
            codec_id: CANONICAL_CODEC_ID,
            hash_suite: HASH_SUITE_ID,
            room_id: &self.room_id,
            core_schema: CORE_SCHEMA_VERSION,
            pack_digest: &self.pack_digest,
            configuration: &self.configuration,
            room_seed: &self.room_seed,
            created_at: &self.created_at,
            initial_timers: &self.initial_timers,
            initial_core_state_hash: &self.initial_core_state_hash,
            initial_activity_state_hash: &self.initial_activity_state_hash,
            initial_authoritative_state_hash: &self.initial_authoritative_state_hash,
        })
    }

    /// Returns the exact canonical immutable record bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if this closed record cannot be canonically encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    /// Returns the Genesis lineage hash.
    #[must_use]
    pub fn genesis_hash(&self) -> &Blake3DigestV1 {
        &self.genesis_hash
    }

    /// Returns the initial Core State hash.
    #[must_use]
    pub fn initial_core_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_core_state_hash
    }

    /// Returns the initial Activity State hash.
    #[must_use]
    pub fn initial_activity_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_activity_state_hash
    }

    /// Returns the initial aggregate state hash.
    #[must_use]
    pub fn initial_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_authoritative_state_hash
    }

    /// Returns the exact initial Core value.
    #[must_use]
    pub fn initial_core_state(&self) -> &CoreRoomStateV1 {
        &self.initial_core_state
    }

    /// Returns the exact initial Activity value.
    #[must_use]
    pub fn initial_activity_state(&self) -> &CanonicalJsonV1 {
        &self.initial_activity_state
    }

    /// Returns the generated Room identity.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    /// Returns the exact immutable pack digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        &self.pack_digest
    }

    /// Returns canonical Room/pack configuration.
    #[must_use]
    pub fn configuration(&self) -> &CanonicalJsonV1 {
        &self.configuration
    }

    /// Returns the recorded nondeterministic Room seed.
    #[must_use]
    pub fn room_seed(&self) -> &RoomSeedV1 {
        &self.room_seed
    }

    /// Returns the logical creation time.
    #[must_use]
    pub fn created_at(&self) -> &CreationRecordedAt {
        &self.created_at
    }

    /// Returns normalized initial Timer generations.
    #[must_use]
    pub fn initial_timers(&self) -> &[ScheduledTimerV1] {
        &self.initial_timers
    }

    /// Returns the complete sequence-zero Head.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
        self.head()
    }

    pub(crate) fn head(&self) -> CompleteHeadV1 {
        CompleteHeadV1 {
            room_id: self.room_id.clone(),
            room_seq: RoomSequenceV1::new(0)
                .unwrap_or_else(|_| unreachable!("zero is a valid Room sequence")),
            genesis_or_transition_hash: self.genesis_hash.clone(),
            core_schema_version: self.core_schema_version.clone(),
            pack_digest: self.pack_digest.clone(),
            core_state_hash: self.initial_core_state_hash.clone(),
            activity_state_hash: self.initial_activity_state_hash.clone(),
            authoritative_state_hash: self.initial_authoritative_state_hash.clone(),
        }
    }
}

/// One accepted immutable Transition and its reconstructible resulting states.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionV1 {
    /// Record version identity.
    pub(crate) transition_version: String,
    /// Canonical codec identity.
    pub(crate) codec_id: String,
    /// Hash suite identity.
    pub(crate) hash_suite: String,
    /// Room identity.
    pub(crate) room_id: RoomId,
    /// Monotonic accepted-Stimulus sequence.
    pub(crate) room_seq: RoomSequenceV1,
    /// Exact Core schema identity.
    pub(crate) core_schema_version: String,
    /// Exact immutable pack digest.
    pub(crate) pack_digest: PackDigestV1,
    /// Previous Genesis or Transition hash.
    pub(crate) previous_transition_or_genesis_hash: Blake3DigestV1,
    /// Complete normalized Stimulus.
    pub(crate) recorded_stimulus: RecordedStimulusV1,
    /// Exact ordered Domain Events.
    pub(crate) ordered_domain_events: Vec<CanonicalJsonV1>,
    /// Exact normalized ordered Timer changes.
    pub(crate) ordered_timer_changes: Vec<TimerChangeV1>,
    /// Exact deterministic Attention Signals.
    pub(crate) ordered_attention_signals: Vec<CanonicalJsonV1>,
    /// Reconstructible resulting Core state.
    pub(crate) resulting_core_state: CoreRoomStateV1,
    /// Reconstructible resulting opaque Activity state.
    pub(crate) resulting_activity_state: CanonicalJsonV1,
    /// Resulting Core hash.
    pub(crate) resulting_core_state_hash: Blake3DigestV1,
    /// Resulting Activity hash.
    pub(crate) resulting_activity_state_hash: Blake3DigestV1,
    /// Resulting aggregate hash.
    pub(crate) resulting_authoritative_state_hash: Blake3DigestV1,
    /// Complete Transition lineage hash.
    pub(crate) transition_hash: Blake3DigestV1,
}

impl TransitionV1 {
    /// Strictly decodes original persisted bytes and requires byte equality
    /// after canonical re-encoding. Replay never accepts a normalized record.
    pub(crate) fn from_canonical_bytes(input: &[u8]) -> Result<Self, CanonicalJsonError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnverifiedTransition {
            transition_version: String,
            codec_id: String,
            hash_suite: String,
            room_id: RoomId,
            room_seq: RoomSequenceV1,
            core_schema_version: String,
            pack_digest: PackDigestV1,
            previous_transition_or_genesis_hash: Blake3DigestV1,
            recorded_stimulus: RecordedStimulusV1,
            ordered_domain_events: Vec<CanonicalJsonV1>,
            ordered_timer_changes: Vec<TimerChangeV1>,
            ordered_attention_signals: Vec<CanonicalJsonV1>,
            resulting_core_state: CoreRoomStateV1,
            resulting_activity_state: CanonicalJsonV1,
            resulting_core_state_hash: Blake3DigestV1,
            resulting_activity_state_hash: Blake3DigestV1,
            resulting_authoritative_state_hash: Blake3DigestV1,
            transition_hash: Blake3DigestV1,
        }

        let raw: UnverifiedTransition = CanonicalJsonV1::decode_canonical(input)?;
        Ok(Self {
            transition_version: raw.transition_version,
            codec_id: raw.codec_id,
            hash_suite: raw.hash_suite,
            room_id: raw.room_id,
            room_seq: raw.room_seq,
            core_schema_version: raw.core_schema_version,
            pack_digest: raw.pack_digest,
            previous_transition_or_genesis_hash: raw.previous_transition_or_genesis_hash,
            recorded_stimulus: raw.recorded_stimulus,
            ordered_domain_events: raw.ordered_domain_events,
            ordered_timer_changes: raw.ordered_timer_changes,
            ordered_attention_signals: raw.ordered_attention_signals,
            resulting_core_state: raw.resulting_core_state,
            resulting_activity_state: raw.resulting_activity_state,
            resulting_core_state_hash: raw.resulting_core_state_hash,
            resulting_activity_state_hash: raw.resulting_activity_state_hash,
            resulting_authoritative_state_hash: raw.resulting_authoritative_state_hash,
            transition_hash: raw.transition_hash,
        })
    }

    pub(crate) fn calculate_hash(&self) -> Result<Blake3DigestV1, CanonicalJsonError> {
        Ok(Blake3DigestV1::hash(&self.hash_input_bytes()?))
    }

    pub(crate) fn hash_input_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&TransitionHashObject {
            domain: TRANSITION_VERSION,
            codec_id: CANONICAL_CODEC_ID,
            hash_suite: HASH_SUITE_ID,
            room_id: &self.room_id,
            room_seq: self.room_seq,
            core_schema: CORE_SCHEMA_VERSION,
            pack_digest: &self.pack_digest,
            previous_transition_or_genesis_hash: &self.previous_transition_or_genesis_hash,
            recorded_stimulus: &self.recorded_stimulus,
            ordered_domain_events: &self.ordered_domain_events,
            ordered_timer_changes: &self.ordered_timer_changes,
            ordered_attention_signals: &self.ordered_attention_signals,
            resulting_core_state_hash: &self.resulting_core_state_hash,
            resulting_activity_state_hash: &self.resulting_activity_state_hash,
            resulting_authoritative_state_hash: &self.resulting_authoritative_state_hash,
        })
    }

    /// Returns the exact canonical immutable record bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if this closed record cannot be canonically encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }

    /// Returns this Transition's Room sequence.
    #[must_use]
    pub const fn room_seq(&self) -> RoomSequenceV1 {
        self.room_seq
    }

    /// Returns the complete recorded Stimulus.
    #[must_use]
    pub fn recorded_stimulus(&self) -> &RecordedStimulusV1 {
        &self.recorded_stimulus
    }

    /// Returns the exact resulting Core state.
    #[must_use]
    pub fn resulting_core_state(&self) -> &CoreRoomStateV1 {
        &self.resulting_core_state
    }

    /// Returns the exact resulting Activity state.
    #[must_use]
    pub fn resulting_activity_state(&self) -> &CanonicalJsonV1 {
        &self.resulting_activity_state
    }

    /// Returns the Transition lineage hash.
    #[must_use]
    pub fn transition_hash(&self) -> &Blake3DigestV1 {
        &self.transition_hash
    }

    /// Returns the previous Genesis/Transition hash.
    #[must_use]
    pub fn previous_lineage_hash(&self) -> &Blake3DigestV1 {
        &self.previous_transition_or_genesis_hash
    }

    /// Returns ordered Domain Events.
    #[must_use]
    pub fn ordered_domain_events(&self) -> &[CanonicalJsonV1] {
        &self.ordered_domain_events
    }

    /// Returns normalized ordered Timer changes.
    #[must_use]
    pub fn ordered_timer_changes(&self) -> &[TimerChangeV1] {
        &self.ordered_timer_changes
    }

    /// Returns deterministic ordered Attention Signals.
    #[must_use]
    pub fn ordered_attention_signals(&self) -> &[CanonicalJsonV1] {
        &self.ordered_attention_signals
    }

    /// Returns the resulting Core State hash.
    #[must_use]
    pub fn resulting_core_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_core_state_hash
    }

    /// Returns the resulting Activity State hash.
    #[must_use]
    pub fn resulting_activity_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_activity_state_hash
    }

    /// Returns the resulting aggregate state hash.
    #[must_use]
    pub fn resulting_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_authoritative_state_hash
    }

    /// Returns the complete post-Transition Head.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
        self.head()
    }

    pub(crate) fn head(&self) -> CompleteHeadV1 {
        CompleteHeadV1 {
            room_id: self.room_id.clone(),
            room_seq: self.room_seq,
            genesis_or_transition_hash: self.transition_hash.clone(),
            core_schema_version: self.core_schema_version.clone(),
            pack_digest: self.pack_digest.clone(),
            core_state_hash: self.resulting_core_state_hash.clone(),
            activity_state_hash: self.resulting_activity_state_hash.clone(),
            authoritative_state_hash: self.resulting_authoritative_state_hash.clone(),
        }
    }
}
