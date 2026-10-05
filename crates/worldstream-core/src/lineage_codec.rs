//! Genesis-selected canonical record codecs. Codec validation verifies
//! commitments; exact Pack execution supplies semantic verification.

use crate::{
    Blake3DigestV1, CANONICAL_CODEC_ID, CORE_SCHEMA_VERSION, CanonicalJsonError, CanonicalJsonV1,
    CanonicalRequestHashV1, CompleteHeadV1, CoreRoomStateV1, CreationRecordedAt, GENESIS_VERSION,
    GenesisInputV1, GenesisV1, HASH_SUITE_ID, InitialMembershipProposalV1, PAYLOAD_BUDGET_V1_ID,
    PackDigestV1, RecordedStimulusV1, RoomCreationRequestV1, RoomId, RoomSeedV1, RoomSequenceV1,
    RoomStatusV1, ScheduledTimerV1, TRANSITION_VERSION, TimerChangeV1, TransitionV1,
    canonical::encode,
    lineage::{hash_activity_state, hash_authoritative_state, hash_core_state},
    primitives::compare_timestamp_text,
    reducer::validate_core_state,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Compact Genesis record identity.
pub const GENESIS_V2_VERSION: &str = "worldstream/genesis/v2";
/// Compact Genesis record codec identity. The checked JSON rules remain V1.
pub const GENESIS_V2_CODEC_ID: &str = "worldstream/genesis-record/v2";
/// Compact Transition record identity.
pub const TRANSITION_V2_VERSION: &str = "worldstream/transition/v2";
/// Compact Transition record codec identity.
pub const TRANSITION_V2_CODEC_ID: &str = "worldstream/transition-record/v2";
/// Record-specific V2 hash framing. State hash domains remain unchanged.
pub const LINEAGE_V2_HASH_SUITE_ID: &str = "blake3-canonical-json-v2";

/// Immutable Room lineage selection. Unknown strings fail typed decoding.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CanonicalHistoryFormat {
    /// Legacy complete-state Transition records.
    #[default]
    #[serde(rename = "worldstream/transition/v1")]
    V1,
    /// Compact Transition records with complete Genesis and checkpoints.
    #[serde(rename = "worldstream/transition/v2")]
    V2,
}

/// Closed canonical codec failure. This is not a Pack execution result.
#[derive(Debug, Error)]
pub enum LineageCodecError {
    /// Invalid canonical JSON or typed fields.
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    /// Unsupported record, codec, schema, or policy identity.
    #[error("unsupported canonical lineage identity")]
    UnsupportedIdentity,
    /// Record belongs to a different Genesis-selected format.
    #[error("record differs from the Genesis-selected history format")]
    MixedFormat,
    /// State, aggregate, or record commitment does not match.
    #[error("canonical record commitment mismatch")]
    CommitmentMismatch,
    /// Room, Pack, sequence, or predecessor does not match.
    #[error("canonical record is not the exact successor")]
    SuccessorMismatch,
    /// Invalid host-owned Core shape or initial Timer normalization.
    #[error("invalid canonical Genesis or Core shape")]
    InvalidCoreOrGenesis,
}

/// Immutable Genesis record with checked commitments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GenesisV2 {
    pub(crate) genesis_version: String,
    pub(crate) codec_id: String,
    pub(crate) hash_suite: String,
    pub(crate) room_id: RoomId,
    pub(crate) core_schema_version: String,
    pub(crate) pack_digest: PackDigestV1,
    pub(crate) configuration: CanonicalJsonV1,
    pub(crate) room_seed: RoomSeedV1,
    pub(crate) created_at: CreationRecordedAt,
    pub(crate) initial_timers: Vec<ScheduledTimerV1>,
    pub(crate) initial_core_state: CoreRoomStateV1,
    pub(crate) initial_activity_state: CanonicalJsonV1,
    pub(crate) initial_core_state_hash: Blake3DigestV1,
    pub(crate) initial_activity_state_hash: Blake3DigestV1,
    pub(crate) initial_authoritative_state_hash: Blake3DigestV1,
    pub(crate) transition_version: String,
    pub(crate) transition_codec_id: String,
    pub(crate) transition_hash_suite: String,
    pub(crate) payload_budget_id: String,
    pub(crate) genesis_hash: Blake3DigestV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenesisV2Wire {
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
    transition_version: String,
    transition_codec_id: String,
    transition_hash_suite: String,
    payload_budget_id: String,
    genesis_hash: Blake3DigestV1,
}

impl GenesisV2 {
    /// Decodes exact canonical bytes and verifies the closed tuple and commitments.
    /// This does not execute or semantically verify the retained Pack.
    ///
    /// # Errors
    /// Returns a codec error for malformed bytes, unsupported identity, or invalid commitments.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, LineageCodecError> {
        let raw: GenesisV2Wire = CanonicalJsonV1::decode_canonical(bytes)?;
        let record = Self {
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
            transition_version: raw.transition_version,
            transition_codec_id: raw.transition_codec_id,
            transition_hash_suite: raw.transition_hash_suite,
            payload_budget_id: raw.payload_budget_id,
            genesis_hash: raw.genesis_hash,
        };
        record.verify()?;
        if record.canonical_bytes()? != bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes.into());
        }
        Ok(record)
    }

    /// Returns the exact immutable canonical bytes.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }
}

/// Immutable Transition record with checked commitments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TransitionV2 {
    pub(crate) transition_version: String,
    pub(crate) codec_id: String,
    pub(crate) hash_suite: String,
    pub(crate) room_id: RoomId,
    pub(crate) room_seq: RoomSequenceV1,
    pub(crate) core_schema_version: String,
    pub(crate) pack_digest: PackDigestV1,
    pub(crate) previous_transition_or_genesis_hash: Blake3DigestV1,
    pub(crate) recorded_stimulus: RecordedStimulusV1,
    pub(crate) ordered_domain_events: Vec<CanonicalJsonV1>,
    pub(crate) ordered_timer_changes: Vec<TimerChangeV1>,
    pub(crate) ordered_attention_signals: Vec<CanonicalJsonV1>,
    pub(crate) resulting_core_state_hash: Blake3DigestV1,
    pub(crate) resulting_activity_state_hash: Blake3DigestV1,
    pub(crate) resulting_authoritative_state_hash: Blake3DigestV1,
    pub(crate) transition_hash: Blake3DigestV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionV2Wire {
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
    resulting_core_state_hash: Blake3DigestV1,
    resulting_activity_state_hash: Blake3DigestV1,
    resulting_authoritative_state_hash: Blake3DigestV1,
    transition_hash: Blake3DigestV1,
}

impl TransitionV2 {
    /// Decodes exact canonical bytes and verifies the closed tuple and commitments.
    /// This does not execute or semantically verify the retained Pack.
    ///
    /// # Errors
    /// Returns a codec error for malformed bytes, unsupported identity, or invalid commitments.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, LineageCodecError> {
        let raw: TransitionV2Wire = CanonicalJsonV1::decode_canonical(bytes)?;
        let record = Self {
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
            resulting_core_state_hash: raw.resulting_core_state_hash,
            resulting_activity_state_hash: raw.resulting_activity_state_hash,
            resulting_authoritative_state_hash: raw.resulting_authoritative_state_hash,
            transition_hash: raw.transition_hash,
        };
        record.verify()?;
        if record.canonical_bytes()? != bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes.into());
        }
        Ok(record)
    }

    /// Returns the exact immutable canonical bytes.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(self)
    }
}

#[derive(Serialize)]
struct GenesisV2HashPreimage<'a> {
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
    transition_version: &'static str,
    transition_codec_id: &'static str,
    transition_hash_suite: &'static str,
    payload_budget_id: &'static str,
}

#[derive(Serialize)]
struct TransitionV2HashPreimage<'a> {
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

impl GenesisV2 {
    /// Encodes caller-supplied initial facts under the fixed V2 tuple and policy.
    /// Pack admission and execution remain separate preparation operations.
    ///
    /// # Errors
    /// Returns a codec error for invalid Core/Timer facts or canonical encoding.
    pub fn new(input: GenesisInputV1) -> Result<Self, LineageCodecError> {
        let core_hash = hash_core_state(&input.initial_core_state)?;
        let activity_hash = hash_activity_state(&input.pack_digest, &input.initial_activity_state)?;
        let authoritative_hash =
            hash_authoritative_state(&input.pack_digest, &core_hash, &activity_hash)?;
        let mut record = Self {
            genesis_version: GENESIS_V2_VERSION.to_owned(),
            codec_id: GENESIS_V2_CODEC_ID.to_owned(),
            hash_suite: LINEAGE_V2_HASH_SUITE_ID.to_owned(),
            room_id: input.room_id,
            core_schema_version: CORE_SCHEMA_VERSION.to_owned(),
            pack_digest: input.pack_digest,
            configuration: input.configuration,
            room_seed: input.room_seed,
            created_at: input.created_at,
            initial_timers: input.initial_timers,
            initial_core_state: input.initial_core_state,
            initial_activity_state: input.initial_activity_state,
            initial_core_state_hash: core_hash,
            initial_activity_state_hash: activity_hash,
            initial_authoritative_state_hash: authoritative_hash,
            transition_version: TRANSITION_V2_VERSION.to_owned(),
            transition_codec_id: TRANSITION_V2_CODEC_ID.to_owned(),
            transition_hash_suite: LINEAGE_V2_HASH_SUITE_ID.to_owned(),
            payload_budget_id: PAYLOAD_BUDGET_V1_ID.to_owned(),
            genesis_hash: Blake3DigestV1::hash(&[]),
        };
        record.genesis_hash = Blake3DigestV1::hash(&record.hash_preimage_bytes()?);
        record.verify()?;
        Ok(record)
    }

    /// Returns the exact Genesis hash preimage. Complete state and the stored
    /// Genesis hash are excluded; state commitments bind the initial values.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn hash_preimage_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&GenesisV2HashPreimage {
            domain: GENESIS_V2_VERSION,
            codec_id: GENESIS_V2_CODEC_ID,
            hash_suite: LINEAGE_V2_HASH_SUITE_ID,
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
            transition_version: TRANSITION_V2_VERSION,
            transition_codec_id: TRANSITION_V2_CODEC_ID,
            transition_hash_suite: LINEAGE_V2_HASH_SUITE_ID,
            payload_budget_id: PAYLOAD_BUDGET_V1_ID,
        })
    }

    fn verify(&self) -> Result<(), LineageCodecError> {
        if self.genesis_version != GENESIS_V2_VERSION
            || self.codec_id != GENESIS_V2_CODEC_ID
            || self.hash_suite != LINEAGE_V2_HASH_SUITE_ID
            || self.core_schema_version != CORE_SCHEMA_VERSION
            || self.transition_version != TRANSITION_V2_VERSION
            || self.transition_codec_id != TRANSITION_V2_CODEC_ID
            || self.transition_hash_suite != LINEAGE_V2_HASH_SUITE_ID
            || self.payload_budget_id != PAYLOAD_BUDGET_V1_ID
        {
            return Err(LineageCodecError::UnsupportedIdentity);
        }
        verify_genesis_facts(
            &self.initial_core_state,
            &self.initial_timers,
            &self.created_at,
        )?;
        verify_state_commitments(
            &self.pack_digest,
            &self.initial_core_state,
            &self.initial_activity_state,
            &self.initial_core_state_hash,
            &self.initial_activity_state_hash,
            &self.initial_authoritative_state_hash,
        )?;
        if self.genesis_hash != Blake3DigestV1::hash(&self.hash_preimage_bytes()?) {
            return Err(LineageCodecError::CommitmentMismatch);
        }
        Ok(())
    }

    /// Returns the complete sequence-zero Head.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
        CompleteHeadV1 {
            room_id: self.room_id.clone(),
            room_seq: RoomSequenceV1::new(0).unwrap_or_else(|_| unreachable!("zero is valid")),
            genesis_or_transition_hash: self.genesis_hash.clone(),
            core_schema_version: self.core_schema_version.clone(),
            pack_digest: self.pack_digest.clone(),
            core_state_hash: self.initial_core_state_hash.clone(),
            activity_state_hash: self.initial_activity_state_hash.clone(),
            authoritative_state_hash: self.initial_authoritative_state_hash.clone(),
        }
    }
}

/// Complete prepared inputs for compact record construction. Resulting state
/// values are borrowed for hashing and never enter the stored record.
pub struct TransitionV2Input<'a> {
    /// Exact resulting Room identity.
    pub room_id: RoomId,
    /// Nonzero resulting Room sequence.
    pub room_seq: RoomSequenceV1,
    /// Exact pinned Pack identity.
    pub pack_digest: PackDigestV1,
    /// Previous Genesis or Transition hash.
    pub previous_lineage_hash: Blake3DigestV1,
    /// Complete normalized accepted input.
    pub recorded_stimulus: RecordedStimulusV1,
    /// Exact ordered Domain Events.
    pub ordered_domain_events: Vec<CanonicalJsonV1>,
    /// Exact normalized ordered Timer changes.
    pub ordered_timer_changes: Vec<TimerChangeV1>,
    /// Exact ordered Attention Signals.
    pub ordered_attention_signals: Vec<CanonicalJsonV1>,
    /// Complete prepared host-owned Core state.
    pub resulting_core_state: &'a CoreRoomStateV1,
    /// Complete prepared Pack-owned Activity state.
    pub resulting_activity_state: &'a CanonicalJsonV1,
}

impl TransitionV2 {
    /// Constructs a compact record from checked preparation inputs. This
    /// computes commitments but does not establish Stimulus or Pack legality.
    ///
    /// # Errors
    /// Returns a codec error for invalid Core shape, zero sequence, or encoding.
    pub fn new(input: TransitionV2Input<'_>) -> Result<Self, LineageCodecError> {
        verify_core_shape(input.resulting_core_state)?;
        let core_hash = hash_core_state(input.resulting_core_state)?;
        let activity_hash =
            hash_activity_state(&input.pack_digest, input.resulting_activity_state)?;
        let authoritative_hash =
            hash_authoritative_state(&input.pack_digest, &core_hash, &activity_hash)?;
        let mut record = Self {
            transition_version: TRANSITION_V2_VERSION.to_owned(),
            codec_id: TRANSITION_V2_CODEC_ID.to_owned(),
            hash_suite: LINEAGE_V2_HASH_SUITE_ID.to_owned(),
            room_id: input.room_id,
            room_seq: input.room_seq,
            core_schema_version: CORE_SCHEMA_VERSION.to_owned(),
            pack_digest: input.pack_digest,
            previous_transition_or_genesis_hash: input.previous_lineage_hash,
            recorded_stimulus: input.recorded_stimulus,
            ordered_domain_events: input.ordered_domain_events,
            ordered_timer_changes: input.ordered_timer_changes,
            ordered_attention_signals: input.ordered_attention_signals,
            resulting_core_state_hash: core_hash,
            resulting_activity_state_hash: activity_hash,
            resulting_authoritative_state_hash: authoritative_hash,
            transition_hash: Blake3DigestV1::hash(&[]),
        };
        record.transition_hash = Blake3DigestV1::hash(&record.hash_preimage_bytes()?);
        record.verify()?;
        Ok(record)
    }

    /// Returns the separate exact Transition hash preimage. The stored
    /// Transition hash and complete resulting state values are excluded.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn hash_preimage_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        encode(&TransitionV2HashPreimage {
            domain: TRANSITION_V2_VERSION,
            codec_id: TRANSITION_V2_CODEC_ID,
            hash_suite: LINEAGE_V2_HASH_SUITE_ID,
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

    fn verify(&self) -> Result<(), LineageCodecError> {
        if self.transition_version != TRANSITION_V2_VERSION
            || self.codec_id != TRANSITION_V2_CODEC_ID
            || self.hash_suite != LINEAGE_V2_HASH_SUITE_ID
            || self.core_schema_version != CORE_SCHEMA_VERSION
        {
            return Err(LineageCodecError::UnsupportedIdentity);
        }
        if self.room_seq.get() == 0 {
            return Err(LineageCodecError::SuccessorMismatch);
        }
        if self.resulting_authoritative_state_hash
            != hash_authoritative_state(
                &self.pack_digest,
                &self.resulting_core_state_hash,
                &self.resulting_activity_state_hash,
            )?
            || self.transition_hash != Blake3DigestV1::hash(&self.hash_preimage_bytes()?)
        {
            return Err(LineageCodecError::CommitmentMismatch);
        }
        Ok(())
    }

    /// Returns the complete resulting Head from recorded commitments.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
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

fn verify_core_shape(core: &CoreRoomStateV1) -> Result<(), LineageCodecError> {
    validate_core_state(core, &|_| Ok(())).map_err(|_| LineageCodecError::InvalidCoreOrGenesis)
}

fn verify_genesis_facts(
    core: &CoreRoomStateV1,
    timers: &[ScheduledTimerV1],
    created_at: &CreationRecordedAt,
) -> Result<(), LineageCodecError> {
    verify_core_shape(core)?;
    if core.room_status() != RoomStatusV1::Active
        || timers
            .windows(2)
            .any(|pair| pair[0].timer_id >= pair[1].timer_id)
        || timers.iter().any(|timer| {
            timer.generation.get() != 1
                || compare_timestamp_text(timer.scheduled_for.as_str(), created_at.as_str())
                    != std::cmp::Ordering::Greater
        })
    {
        return Err(LineageCodecError::InvalidCoreOrGenesis);
    }
    Ok(())
}

fn verify_state_commitments(
    pack: &PackDigestV1,
    core: &CoreRoomStateV1,
    activity: &CanonicalJsonV1,
    core_hash: &Blake3DigestV1,
    activity_hash: &Blake3DigestV1,
    authoritative_hash: &Blake3DigestV1,
) -> Result<(), LineageCodecError> {
    if hash_core_state(core)? != *core_hash
        || hash_activity_state(pack, activity)? != *activity_hash
        || hash_authoritative_state(pack, core_hash, activity_hash)? != *authoritative_hash
    {
        return Err(LineageCodecError::CommitmentMismatch);
    }
    Ok(())
}
/// Immutable Genesis variants. No decoder inference or mutable format metadata
/// participates in lineage selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum GenesisRecord {
    /// Unchanged legacy record.
    V1(GenesisV1),
    /// Complete initial state and hash-bound compact format/policy selectors.
    V2(GenesisV2),
}

/// Immutable Transition variants. Full resulting values exist only in V1.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TransitionRecord {
    /// Unchanged legacy record.
    V1(TransitionV1),
    /// Compact commitments and exact ordered effects.
    V2(TransitionV2),
}

fn record_version(bytes: &[u8], key: &str) -> Result<String, LineageCodecError> {
    let canonical = CanonicalJsonV1::from_canonical_bytes(bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&canonical.to_bytes()?)
        .map_err(|error| CanonicalJsonError::TypedDecode(error.to_string()))?;
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(LineageCodecError::UnsupportedIdentity)
}

impl GenesisRecord {
    /// Dispatches once by the closed Genesis version, then verifies its exact
    /// tuple, canonical representation, initial facts, and commitments.
    ///
    /// # Errors
    /// Returns a codec error for invalid bytes, identities, or commitments.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, LineageCodecError> {
        let record = match record_version(bytes, "genesis_version")?.as_str() {
            GENESIS_VERSION => Self::V1(GenesisV1::from_canonical_bytes(bytes)?),
            GENESIS_V2_VERSION => Self::V2(GenesisV2::from_canonical_bytes(bytes)?),
            _ => return Err(LineageCodecError::UnsupportedIdentity),
        };
        record.verify()?;
        if record.canonical_bytes()? != bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes.into());
        }
        Ok(record)
    }

    /// Verifies this immutable Genesis without executing its Pack.
    ///
    /// # Errors
    /// Returns a codec error for invalid identity, initial facts, or commitments.
    pub fn verify(&self) -> Result<(), LineageCodecError> {
        match self {
            Self::V2(record) => record.verify(),
            Self::V1(record) => {
                if record.genesis_version != GENESIS_VERSION
                    || record.codec_id != CANONICAL_CODEC_ID
                    || record.hash_suite != HASH_SUITE_ID
                    || record.core_schema_version != CORE_SCHEMA_VERSION
                {
                    return Err(LineageCodecError::UnsupportedIdentity);
                }
                verify_genesis_facts(
                    &record.initial_core_state,
                    &record.initial_timers,
                    &record.created_at,
                )?;
                verify_state_commitments(
                    &record.pack_digest,
                    &record.initial_core_state,
                    &record.initial_activity_state,
                    &record.initial_core_state_hash,
                    &record.initial_activity_state_hash,
                    &record.initial_authoritative_state_hash,
                )?;
                if record.genesis_hash != record.calculate_hash()? {
                    return Err(LineageCodecError::CommitmentMismatch);
                }
                Ok(())
            }
        }
    }

    /// Returns the fixed history format. Verification must precede use of a
    /// record assembled from a legacy unverified decoder.
    #[must_use]
    pub const fn format(&self) -> CanonicalHistoryFormat {
        match self {
            Self::V1(_) => CanonicalHistoryFormat::V1,
            Self::V2(_) => CanonicalHistoryFormat::V2,
        }
    }

    /// Returns the immutable policy selector. Legacy Genesis selects no new policy.
    #[must_use]
    pub fn payload_budget_id(&self) -> Option<&str> {
        match self {
            Self::V1(_) => None,
            Self::V2(record) => Some(&record.payload_budget_id),
        }
    }

    /// Decodes a Transition only under this verified Genesis selection.
    ///
    /// # Errors
    /// Returns a codec error for invalid Genesis or an unsupported/mixed record.
    pub fn decode_transition(&self, bytes: &[u8]) -> Result<TransitionRecord, LineageCodecError> {
        self.verify()?;
        TransitionRecord::from_canonical_bytes(self.format(), bytes)
    }

    /// Returns exact original-format canonical bytes.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        match self {
            Self::V1(record) => record.canonical_bytes(),
            Self::V2(record) => record.canonical_bytes(),
        }
    }

    /// Returns the complete sequence-zero Head.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
        match self {
            Self::V1(record) => record.complete_head(),
            Self::V2(record) => record.complete_head(),
        }
    }
}

impl TransitionRecord {
    /// Decodes only the format selected by verified Genesis. This method
    /// validates record commitments, not omitted state or Stimulus legality.
    ///
    /// # Errors
    /// Returns a codec error for invalid bytes, mixed formats, or commitments.
    pub fn from_canonical_bytes(
        format: CanonicalHistoryFormat,
        bytes: &[u8],
    ) -> Result<Self, LineageCodecError> {
        let recorded_format = match record_version(bytes, "transition_version")?.as_str() {
            TRANSITION_VERSION => CanonicalHistoryFormat::V1,
            TRANSITION_V2_VERSION => CanonicalHistoryFormat::V2,
            _ => return Err(LineageCodecError::UnsupportedIdentity),
        };
        if format != recorded_format {
            return Err(LineageCodecError::MixedFormat);
        }
        let record = match format {
            CanonicalHistoryFormat::V1 => Self::V1(TransitionV1::from_canonical_bytes(bytes)?),
            CanonicalHistoryFormat::V2 => Self::V2(TransitionV2::from_canonical_bytes(bytes)?),
        };
        record.verify_commitments()?;
        if record.canonical_bytes()? != bytes {
            return Err(CanonicalJsonError::NonCanonicalBytes.into());
        }
        Ok(record)
    }

    /// Verifies tuple, aggregate, and record commitments. V1 additionally
    /// verifies embedded state. V2 requires derived state or exact execution
    /// before its component commitments can be semantically verified.
    ///
    /// # Errors
    /// Returns a codec error for invalid identity, Core shape, or commitments.
    pub fn verify_commitments(&self) -> Result<(), LineageCodecError> {
        match self {
            Self::V2(record) => record.verify(),
            Self::V1(record) => {
                if record.transition_version != TRANSITION_VERSION
                    || record.codec_id != CANONICAL_CODEC_ID
                    || record.hash_suite != HASH_SUITE_ID
                    || record.core_schema_version != CORE_SCHEMA_VERSION
                {
                    return Err(LineageCodecError::UnsupportedIdentity);
                }
                if record.room_seq.get() == 0 {
                    return Err(LineageCodecError::SuccessorMismatch);
                }
                verify_core_shape(&record.resulting_core_state)?;
                verify_state_commitments(
                    &record.pack_digest,
                    &record.resulting_core_state,
                    &record.resulting_activity_state,
                    &record.resulting_core_state_hash,
                    &record.resulting_activity_state_hash,
                    &record.resulting_authoritative_state_hash,
                )?;
                if record.transition_hash != record.calculate_hash()? {
                    return Err(LineageCodecError::CommitmentMismatch);
                }
                Ok(())
            }
        }
    }

    /// Checks exact Room, Pack, Core schema, successor sequence, predecessor,
    /// aggregate commitment, and record hash. This does not execute the Pack.
    /// The caller must decode using the verified Genesis format first.
    ///
    /// # Errors
    /// Returns a codec error for invalid commitments or successor linkage.
    pub fn verify_successor(&self, previous: &CompleteHeadV1) -> Result<(), LineageCodecError> {
        self.verify_commitments()?;
        let next = self.complete_head();
        if next.room_id() != previous.room_id()
            || next.pack_digest() != previous.pack_digest()
            || next.core_schema_version() != previous.core_schema_version()
            || previous.room_seq().checked_successor().ok() != Some(next.room_seq())
            || self.previous_lineage_hash() != previous.genesis_or_transition_hash()
        {
            return Err(LineageCodecError::SuccessorMismatch);
        }
        Ok(())
    }

    /// Returns exact original-format canonical bytes.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        match self {
            Self::V1(record) => record.canonical_bytes(),
            Self::V2(record) => record.canonical_bytes(),
        }
    }

    /// Returns the complete resulting Head from record commitments.
    #[must_use]
    pub fn complete_head(&self) -> CompleteHeadV1 {
        match self {
            Self::V1(record) => record.complete_head(),
            Self::V2(record) => record.complete_head(),
        }
    }

    /// Returns the immutable record format.
    #[must_use]
    pub const fn format(&self) -> CanonicalHistoryFormat {
        match self {
            Self::V1(_) => CanonicalHistoryFormat::V1,
            Self::V2(_) => CanonicalHistoryFormat::V2,
        }
    }
}

impl GenesisV2 {
    /// Returns the exact genesis hash.
    #[must_use]
    pub fn genesis_hash(&self) -> &Blake3DigestV1 {
        &self.genesis_hash
    }
    /// Returns the exact room id.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }
    /// Returns the exact pack digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        &self.pack_digest
    }
    /// Returns the exact configuration.
    #[must_use]
    pub fn configuration(&self) -> &CanonicalJsonV1 {
        &self.configuration
    }
    /// Returns the exact room seed.
    #[must_use]
    pub fn room_seed(&self) -> &RoomSeedV1 {
        &self.room_seed
    }
    /// Returns the exact created at.
    #[must_use]
    pub fn created_at(&self) -> &CreationRecordedAt {
        &self.created_at
    }
    /// Returns the exact initial timers.
    #[must_use]
    pub fn initial_timers(&self) -> &[ScheduledTimerV1] {
        &self.initial_timers
    }
    /// Returns the exact initial core state.
    #[must_use]
    pub fn initial_core_state(&self) -> &CoreRoomStateV1 {
        &self.initial_core_state
    }
    /// Returns the exact initial activity state.
    #[must_use]
    pub fn initial_activity_state(&self) -> &CanonicalJsonV1 {
        &self.initial_activity_state
    }
    /// Returns the exact initial core state hash.
    #[must_use]
    pub fn initial_core_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_core_state_hash
    }
    /// Returns the exact initial activity state hash.
    #[must_use]
    pub fn initial_activity_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_activity_state_hash
    }
    /// Returns the exact initial authoritative state hash.
    #[must_use]
    pub fn initial_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        &self.initial_authoritative_state_hash
    }
    /// Returns the exact core schema version.
    #[must_use]
    pub fn core_schema_version(&self) -> &str {
        &self.core_schema_version
    }
}

impl TransitionV2 {
    /// Returns the exact room id.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }
    /// Returns the exact room seq.
    #[must_use]
    pub fn room_seq(&self) -> RoomSequenceV1 {
        self.room_seq
    }
    /// Returns the exact pack digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        &self.pack_digest
    }
    /// Returns the exact core schema version.
    #[must_use]
    pub fn core_schema_version(&self) -> &str {
        &self.core_schema_version
    }
    /// Returns the exact previous lineage hash.
    #[must_use]
    pub fn previous_lineage_hash(&self) -> &Blake3DigestV1 {
        &self.previous_transition_or_genesis_hash
    }
    /// Returns the exact recorded stimulus.
    #[must_use]
    pub fn recorded_stimulus(&self) -> &RecordedStimulusV1 {
        &self.recorded_stimulus
    }
    /// Returns the exact ordered domain events.
    #[must_use]
    pub fn ordered_domain_events(&self) -> &[CanonicalJsonV1] {
        &self.ordered_domain_events
    }
    /// Returns the exact ordered timer changes.
    #[must_use]
    pub fn ordered_timer_changes(&self) -> &[TimerChangeV1] {
        &self.ordered_timer_changes
    }
    /// Returns the exact ordered attention signals.
    #[must_use]
    pub fn ordered_attention_signals(&self) -> &[CanonicalJsonV1] {
        &self.ordered_attention_signals
    }
    /// Returns the exact resulting core state hash.
    #[must_use]
    pub fn resulting_core_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_core_state_hash
    }
    /// Returns the exact resulting activity state hash.
    #[must_use]
    pub fn resulting_activity_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_activity_state_hash
    }
    /// Returns the exact resulting authoritative state hash.
    #[must_use]
    pub fn resulting_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        &self.resulting_authoritative_state_hash
    }
    /// Returns the exact transition hash.
    #[must_use]
    pub fn transition_hash(&self) -> &Blake3DigestV1 {
        &self.transition_hash
    }
}

impl GenesisRecord {
    /// Returns the exact genesis hash.
    #[must_use]
    pub fn genesis_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.genesis_hash,
            Self::V2(record) => &record.genesis_hash,
        }
    }
    /// Returns the exact room id.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        match self {
            Self::V1(record) => &record.room_id,
            Self::V2(record) => &record.room_id,
        }
    }
    /// Returns the exact pack digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        match self {
            Self::V1(record) => &record.pack_digest,
            Self::V2(record) => &record.pack_digest,
        }
    }
    /// Returns the exact configuration.
    #[must_use]
    pub fn configuration(&self) -> &CanonicalJsonV1 {
        match self {
            Self::V1(record) => &record.configuration,
            Self::V2(record) => &record.configuration,
        }
    }
    /// Returns the exact room seed.
    #[must_use]
    pub fn room_seed(&self) -> &RoomSeedV1 {
        match self {
            Self::V1(record) => &record.room_seed,
            Self::V2(record) => &record.room_seed,
        }
    }
    /// Returns the exact created at.
    #[must_use]
    pub fn created_at(&self) -> &CreationRecordedAt {
        match self {
            Self::V1(record) => &record.created_at,
            Self::V2(record) => &record.created_at,
        }
    }
    /// Returns the exact initial timers.
    #[must_use]
    pub fn initial_timers(&self) -> &[ScheduledTimerV1] {
        match self {
            Self::V1(record) => &record.initial_timers,
            Self::V2(record) => &record.initial_timers,
        }
    }
    /// Returns the exact initial core state.
    #[must_use]
    pub fn initial_core_state(&self) -> &CoreRoomStateV1 {
        match self {
            Self::V1(record) => &record.initial_core_state,
            Self::V2(record) => &record.initial_core_state,
        }
    }
    /// Returns the exact initial activity state.
    #[must_use]
    pub fn initial_activity_state(&self) -> &CanonicalJsonV1 {
        match self {
            Self::V1(record) => &record.initial_activity_state,
            Self::V2(record) => &record.initial_activity_state,
        }
    }
    /// Returns the exact initial core state hash.
    #[must_use]
    pub fn initial_core_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.initial_core_state_hash,
            Self::V2(record) => &record.initial_core_state_hash,
        }
    }
    /// Returns the exact initial activity state hash.
    #[must_use]
    pub fn initial_activity_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.initial_activity_state_hash,
            Self::V2(record) => &record.initial_activity_state_hash,
        }
    }
    /// Returns the exact initial authoritative state hash.
    #[must_use]
    pub fn initial_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.initial_authoritative_state_hash,
            Self::V2(record) => &record.initial_authoritative_state_hash,
        }
    }
    /// Returns the exact core schema version.
    #[must_use]
    pub fn core_schema_version(&self) -> &str {
        match self {
            Self::V1(record) => &record.core_schema_version,
            Self::V2(record) => &record.core_schema_version,
        }
    }
}

impl TransitionRecord {
    /// Returns the exact room id.
    #[must_use]
    pub fn room_id(&self) -> &RoomId {
        match self {
            Self::V1(record) => &record.room_id,
            Self::V2(record) => &record.room_id,
        }
    }
    /// Returns the exact room seq.
    #[must_use]
    pub fn room_seq(&self) -> RoomSequenceV1 {
        match self {
            Self::V1(record) => record.room_seq,
            Self::V2(record) => record.room_seq,
        }
    }
    /// Returns the exact pack digest.
    #[must_use]
    pub fn pack_digest(&self) -> &PackDigestV1 {
        match self {
            Self::V1(record) => &record.pack_digest,
            Self::V2(record) => &record.pack_digest,
        }
    }
    /// Returns the exact core schema version.
    #[must_use]
    pub fn core_schema_version(&self) -> &str {
        match self {
            Self::V1(record) => &record.core_schema_version,
            Self::V2(record) => &record.core_schema_version,
        }
    }
    /// Returns the exact previous lineage hash.
    #[must_use]
    pub fn previous_lineage_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.previous_transition_or_genesis_hash,
            Self::V2(record) => &record.previous_transition_or_genesis_hash,
        }
    }
    /// Returns the exact recorded stimulus.
    #[must_use]
    pub fn recorded_stimulus(&self) -> &RecordedStimulusV1 {
        match self {
            Self::V1(record) => &record.recorded_stimulus,
            Self::V2(record) => &record.recorded_stimulus,
        }
    }
    /// Returns the exact ordered domain events.
    #[must_use]
    pub fn ordered_domain_events(&self) -> &[CanonicalJsonV1] {
        match self {
            Self::V1(record) => &record.ordered_domain_events,
            Self::V2(record) => &record.ordered_domain_events,
        }
    }
    /// Returns the exact ordered timer changes.
    #[must_use]
    pub fn ordered_timer_changes(&self) -> &[TimerChangeV1] {
        match self {
            Self::V1(record) => &record.ordered_timer_changes,
            Self::V2(record) => &record.ordered_timer_changes,
        }
    }
    /// Returns the exact ordered attention signals.
    #[must_use]
    pub fn ordered_attention_signals(&self) -> &[CanonicalJsonV1] {
        match self {
            Self::V1(record) => &record.ordered_attention_signals,
            Self::V2(record) => &record.ordered_attention_signals,
        }
    }
    /// Returns the exact resulting core state hash.
    #[must_use]
    pub fn resulting_core_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.resulting_core_state_hash,
            Self::V2(record) => &record.resulting_core_state_hash,
        }
    }
    /// Returns the exact resulting activity state hash.
    #[must_use]
    pub fn resulting_activity_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.resulting_activity_state_hash,
            Self::V2(record) => &record.resulting_activity_state_hash,
        }
    }
    /// Returns the exact resulting authoritative state hash.
    #[must_use]
    pub fn resulting_authoritative_state_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.resulting_authoritative_state_hash,
            Self::V2(record) => &record.resulting_authoritative_state_hash,
        }
    }
    /// Returns the exact transition hash.
    #[must_use]
    pub fn transition_hash(&self) -> &Blake3DigestV1 {
        match self {
            Self::V1(record) => &record.transition_hash,
            Self::V2(record) => &record.transition_hash,
        }
    }
}

/// Additive caller-semantic creation contract. Existing ingress and receipts
/// continue to use `RoomCreationRequestV1` until lifecycle integration is ready.
#[derive(Clone, Eq, PartialEq)]
pub struct RoomCreationRequestWithFormat {
    legacy: RoomCreationRequestV1,
    format: CanonicalHistoryFormat,
}

impl std::fmt::Debug for RoomCreationRequestWithFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RoomCreationRequestWithFormat([REDACTED])")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationRequestWire {
    pack_digest: PackDigestV1,
    configuration: CanonicalJsonV1,
    ordered_initial_memberships: Vec<InitialMembershipProposalV1>,
    #[serde(default)]
    canonical_history_format: CanonicalHistoryFormat,
}

#[derive(Serialize)]
struct CreationRequestOutput<'a> {
    pack_digest: &'a PackDigestV1,
    configuration: &'a CanonicalJsonV1,
    ordered_initial_memberships: &'a [InitialMembershipProposalV1],
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_history_format: Option<CanonicalHistoryFormat>,
}

#[derive(Serialize)]
struct CreationRequestHashPreimage<'a> {
    domain: &'static str,
    pack_digest: &'a PackDigestV1,
    configuration: &'a CanonicalJsonV1,
    ordered_initial_memberships: &'a [InitialMembershipProposalV1],
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_history_format: Option<CanonicalHistoryFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_budget_id: Option<&'static str>,
}

impl Serialize for RoomCreationRequestWithFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        CreationRequestOutput {
            pack_digest: self.legacy.pack_digest(),
            configuration: self.legacy.configuration(),
            ordered_initial_memberships: self.legacy.ordered_initial_memberships(),
            canonical_history_format: (self.format == CanonicalHistoryFormat::V2)
                .then_some(self.format),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RoomCreationRequestWithFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = CreationRequestWire::deserialize(deserializer)?;
        let request = Self::new(
            RoomCreationRequestV1::new(
                raw.pack_digest,
                raw.configuration,
                raw.ordered_initial_memberships,
            ),
            raw.canonical_history_format,
        );
        request
            .canonical_request_hash()
            .map_err(serde::de::Error::custom)?;
        Ok(request)
    }
}

impl RoomCreationRequestWithFormat {
    /// Selects a format without enabling production creation. V1 selection
    /// normalizes to the legacy request shape and request hash.
    #[must_use]
    pub const fn new(legacy: RoomCreationRequestV1, format: CanonicalHistoryFormat) -> Self {
        Self { legacy, format }
    }

    /// Strictly decodes the closed canonical creation wire object. Explicit
    /// V1 normalizes to omission. Unknown fields and selectors are rejected.
    ///
    /// # Errors
    /// Returns a codec error for malformed bytes, selectors, or Memberships.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, LineageCodecError> {
        let raw: CreationRequestWire = CanonicalJsonV1::decode_canonical(bytes)?;
        let request = Self::new(
            RoomCreationRequestV1::new(
                raw.pack_digest,
                raw.configuration,
                raw.ordered_initial_memberships,
            ),
            raw.canonical_history_format,
        );
        request.canonical_request_hash()?;
        Ok(request)
    }

    /// Returns the normalized wire request. V1 has the exact legacy shape.
    ///
    /// # Errors
    /// Returns a canonical encoding error.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        self.legacy.canonical_request_hash()?;
        encode(self)
    }

    /// Returns the exact versioned caller-semantic hash preimage. V2 binds
    /// history format and the fixed immutable payload policy. Generated Room
    /// and Member identities, seed, creation time, and commit time are absent.
    ///
    /// # Errors
    /// Returns a canonical encoding or creation-shape error.
    pub fn hash_preimage_bytes(&self) -> Result<Vec<u8>, CanonicalJsonError> {
        self.legacy.canonical_request_hash()?;
        let compact = self.format == CanonicalHistoryFormat::V2;
        encode(&CreationRequestHashPreimage {
            domain: if compact {
                "worldstream/create-room-request/v2"
            } else {
                "worldstream/create-room-request/v1"
            },
            pack_digest: self.legacy.pack_digest(),
            configuration: self.legacy.configuration(),
            ordered_initial_memberships: self.legacy.ordered_initial_memberships(),
            canonical_history_format: compact.then_some(self.format),
            payload_budget_id: compact.then_some(PAYLOAD_BUDGET_V1_ID),
        })
    }

    /// Computes the exact request identity. V1 delegates to the frozen legacy
    /// implementation. V2 uses the separate selector-bound hash preimage.
    ///
    /// # Errors
    /// Returns a canonical encoding or creation-shape error.
    pub fn canonical_request_hash(&self) -> Result<CanonicalRequestHashV1, CanonicalJsonError> {
        if self.format == CanonicalHistoryFormat::V1 {
            return self.legacy.canonical_request_hash();
        }
        Ok(CanonicalRequestHashV1::calculate_canonical(
            &self.hash_preimage_bytes()?,
        ))
    }

    /// Returns unchanged caller-semantic creation facts.
    #[must_use]
    pub const fn legacy_request(&self) -> &RoomCreationRequestV1 {
        &self.legacy
    }

    /// Returns the immutable selected history format.
    #[must_use]
    pub const fn format(&self) -> CanonicalHistoryFormat {
        self.format
    }

    /// Returns the selected immutable policy, absent for the legacy contract.
    #[must_use]
    pub const fn payload_budget_id(&self) -> Option<&'static str> {
        match self.format {
            CanonicalHistoryFormat::V1 => None,
            CanonicalHistoryFormat::V2 => Some(PAYLOAD_BUDGET_V1_ID),
        }
    }
}
