use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    crypto::{decode_base64url, exact_blake3_digest},
    model::{
        A202_COMMERCIAL_MEDIA_TYPE, ExactA202ObjectV1, LogicalA202HeadV1, ProfilePinsV1,
        ResolvedPublicKeyV1, RoomHeadWitnessV1, VerificationOutcomeV1, VerifiedResolverEvidenceV1,
    },
};

pub const PROOF_PACKAGE_FORMAT_V1: &str = "worldstream/negotiate-proof-package/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackBundleReferenceV1 {
    pub pack_id: String,
    pub revision_digest: String,
    pub physical_bundle_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartyProtocolProofV1 {
    pub transaction_id: String,
    pub session_id: String,
    pub transaction_head: LogicalA202HeadV1,
    pub session_head: LogicalA202HeadV1,
    pub exact_objects: Vec<ExactA202ObjectV1>,
    pub key_resolution_evidence: Vec<ResolvedPublicKeyV1>,
    pub resolver_evidence: Vec<VerifiedResolverEvidenceV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VenueRecordV1 {
    pub record_kind: String,
    pub record_id: String,
    pub room_sequence: u64,
    pub exact_bytes_base64url: String,
    pub byte_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayEvidenceV1 {
    pub declared_result: VerificationOutcomeV1,
    pub exact_report_base64url: String,
    pub report_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CrossIndexEntryV1 {
    pub a202_object_id: String,
    pub a202_wire_digest: String,
    pub worldstream_action_id: String,
    pub worldstream_transition_record_id: String,
    pub room_sequence: u64,
    pub room_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VenueRuntimeProofV1 {
    pub room_id: String,
    pub genesis_hash: String,
    pub pack: PackBundleReferenceV1,
    pub final_room_head: RoomHeadWitnessV1,
    pub records: Vec<VenueRecordV1>,
    pub replay: ReplayEvidenceV1,
    pub cross_index: Vec<CrossIndexEntryV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NegotiationProofPackageV1 {
    pub format: String,
    pub profile: ProfilePinsV1,
    pub party_protocol_proof: PartyProtocolProofV1,
    pub venue_runtime_proof: VenueRuntimeProofV1,
}

impl NegotiationProofPackageV1 {
    /// Construct and validate a two-section negotiation proof package.
    ///
    /// # Errors
    ///
    /// Returns an error when either proof section is incomplete, out of order,
    /// internally inconsistent, or does not use the pinned profile.
    pub fn new(
        party_protocol_proof: PartyProtocolProofV1,
        venue_runtime_proof: VenueRuntimeProofV1,
    ) -> Result<Self, ProofError> {
        let package = Self {
            format: PROOF_PACKAGE_FORMAT_V1.to_owned(),
            profile: ProfilePinsV1::default(),
            party_protocol_proof,
            venue_runtime_proof,
        };
        package.validate()?;
        Ok(package)
    }

    /// Validate deterministic ordering, exact digests, and cross-index coverage.
    ///
    /// # Errors
    ///
    /// Returns an error for a profile mismatch or any invalid party/protocol,
    /// venue/runtime, replay, or cross-index evidence.
    pub fn validate(&self) -> Result<(), ProofError> {
        if self.format != PROOF_PACKAGE_FORMAT_V1 {
            return Err(ProofError::Format);
        }
        self.profile.validate().map_err(|_| ProofError::Profile)?;
        validate_objects(&self.party_protocol_proof.exact_objects)?;
        validate_resolver_evidence(&self.party_protocol_proof.resolver_evidence)?;
        validate_venue_records(
            &self.venue_runtime_proof.records,
            &self.venue_runtime_proof.final_room_head,
        )?;
        validate_replay(&self.venue_runtime_proof.replay)?;
        validate_cross_index(
            &self.party_protocol_proof.exact_objects,
            &self.venue_runtime_proof.records,
            &self.venue_runtime_proof.cross_index,
        )?;
        Ok(())
    }

    /// Encode the validated package as JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or JSON encoding fails.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, ProofError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| ProofError::Encoding(error.to_string()))
    }
}

fn validate_objects(objects: &[ExactA202ObjectV1]) -> Result<(), ProofError> {
    if objects.is_empty() {
        return Err(ProofError::MissingProtocolObjects);
    }
    let mut previous = None;
    for object in objects {
        if object.media_type != A202_COMMERCIAL_MEDIA_TYPE {
            return Err(ProofError::ObjectMediaType);
        }
        if previous.is_some_and(|value: &str| value >= object.object_id.as_str()) {
            return Err(ProofError::ObjectOrder);
        }
        let bytes = decode_base64url(&object.exact_bytes_base64url)
            .map_err(|_| ProofError::ObjectDigest)?;
        if exact_blake3_digest(&bytes) != object.wire_digest {
            return Err(ProofError::ObjectDigest);
        }
        previous = Some(object.object_id.as_str());
    }
    Ok(())
}

fn validate_resolver_evidence(evidence: &[VerifiedResolverEvidenceV1]) -> Result<(), ProofError> {
    let mut previous = None;
    for item in evidence {
        if previous.is_some_and(|value: &str| value >= item.observation.evidence_id.as_str()) {
            return Err(ProofError::ResolverEvidenceOrder);
        }
        let bytes = decode_base64url(&item.observation.exact_response_base64url)
            .map_err(|_| ProofError::ResolverEvidenceDigest)?;
        if exact_blake3_digest(&bytes) != item.observation.response_digest {
            return Err(ProofError::ResolverEvidenceDigest);
        }
        previous = Some(item.observation.evidence_id.as_str());
    }
    Ok(())
}

fn validate_venue_records(
    records: &[VenueRecordV1],
    final_head: &RoomHeadWitnessV1,
) -> Result<(), ProofError> {
    let mut keys = BTreeSet::new();
    let mut previous = None;
    for record in records {
        let key = (record.room_sequence, record.record_id.as_str());
        if previous.is_some_and(|prior| prior >= key) {
            return Err(ProofError::VenueRecordOrder);
        }
        if !keys.insert(record.record_id.as_str()) {
            return Err(ProofError::VenueRecordOrder);
        }
        let bytes = decode_base64url(&record.exact_bytes_base64url)
            .map_err(|_| ProofError::VenueRecordDigest)?;
        if exact_blake3_digest(&bytes) != record.byte_digest {
            return Err(ProofError::VenueRecordDigest);
        }
        if record.room_sequence > final_head.sequence {
            return Err(ProofError::VenueRecordBeyondHead);
        }
        previous = Some(key);
    }
    Ok(())
}

fn validate_replay(replay: &ReplayEvidenceV1) -> Result<(), ProofError> {
    let bytes =
        decode_base64url(&replay.exact_report_base64url).map_err(|_| ProofError::ReplayDigest)?;
    if exact_blake3_digest(&bytes) != replay.report_digest {
        return Err(ProofError::ReplayDigest);
    }
    Ok(())
}

fn validate_cross_index(
    objects: &[ExactA202ObjectV1],
    records: &[VenueRecordV1],
    index: &[CrossIndexEntryV1],
) -> Result<(), ProofError> {
    let object_ids: BTreeSet<_> = objects
        .iter()
        .map(|object| object.object_id.as_str())
        .collect();
    let record_ids: BTreeSet<_> = records
        .iter()
        .map(|record| record.record_id.as_str())
        .collect();
    let indexed_ids: BTreeSet<_> = index
        .iter()
        .map(|entry| entry.a202_object_id.as_str())
        .collect();
    if object_ids != indexed_ids || object_ids.len() != index.len() {
        return Err(ProofError::CrossIndexCoverage);
    }
    let mut previous = None;
    for entry in index {
        if previous.is_some_and(|value: &str| value >= entry.a202_object_id.as_str()) {
            return Err(ProofError::CrossIndexOrder);
        }
        let Some(object) = objects
            .iter()
            .find(|object| object.object_id == entry.a202_object_id)
        else {
            return Err(ProofError::CrossIndexCoverage);
        };
        if object.wire_digest != entry.a202_wire_digest
            || !record_ids.contains(entry.worldstream_action_id.as_str())
            || !record_ids.contains(entry.worldstream_transition_record_id.as_str())
        {
            return Err(ProofError::CrossIndexBinding);
        }
        previous = Some(entry.a202_object_id.as_str());
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProofError {
    #[error("unsupported Negotiate proof-package format")]
    Format,
    #[error("proof-package A202 profile does not match ADR 0015")]
    Profile,
    #[error("party/protocol proof has no A202 object")]
    MissingProtocolObjects,
    #[error("A202 objects must be strictly ordered by immutable object id")]
    ObjectOrder,
    #[error("A202 object has the wrong pinned media type")]
    ObjectMediaType,
    #[error("A202 exact object bytes do not match their wire digest")]
    ObjectDigest,
    #[error("resolver evidence must be strictly ordered by evidence id")]
    ResolverEvidenceOrder,
    #[error("resolver response bytes do not match their digest")]
    ResolverEvidenceDigest,
    #[error("venue records must be strictly ordered by Room sequence and record id")]
    VenueRecordOrder,
    #[error("venue record bytes do not match their digest")]
    VenueRecordDigest,
    #[error("venue record sequence is beyond the exported final Room Head")]
    VenueRecordBeyondHead,
    #[error("Replay report bytes do not match their digest")]
    ReplayDigest,
    #[error("cross-index must cover every A202 object exactly once")]
    CrossIndexCoverage,
    #[error("cross-index entries must be strictly ordered by A202 object id")]
    CrossIndexOrder,
    #[error("cross-index does not bind the exact object and venue record")]
    CrossIndexBinding,
    #[error("proof package could not be encoded: {0}")]
    Encoding(String),
}
