use serde_json::Value;

use crate::{
    canonical::parse_exact_canonical,
    crypto::{
        a202_content_hash, a202_signature_message, decode_base64url, encode_base64url,
        exact_blake3_digest, resolver_attestation_message, verify_es256,
    },
    model::{
        A202_CALIBRATION_PROFILE, A202_COMMERCIAL_MEDIA_TYPE, A202_SPEC_VERSION, AdapterError,
        DetachedEs256SignatureV1, ExactA202ObjectV1, ExpectedA202ObjectV1, KeyStatusV1,
        OpaqueA202InputV1, OperatedSessionBindingV1, PreparedA202SubmissionV1, ProfilePinsV1,
        PublicKeyMap, ResolvedPublicKeyV1, ResolverEvidencePolicyV1, SignatureVerificationV1,
        SubmissionBasisV1, UnsignedResolverEvidenceV1, VerificationOutcomeV1,
        VerifiedResolverEvidenceV1,
    },
};

const MAX_A202_OBJECT_BYTES: usize = 256 * 1024;
const A202_MAX_STATUS_VALIDITY_MS: u64 = 60_000;
const RESOLVER_ATTESTATION_PURPOSE: &str = "resolver_evidence_authentication";

#[derive(Clone, Debug)]
pub struct A202AdapterV1 {
    pins: ProfilePinsV1,
    session: OperatedSessionBindingV1,
    keys: PublicKeyMap,
}

impl A202AdapterV1 {
    /// Construct the adapter for one operated A202 transaction/session binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile pins differ from the pinned operated
    /// profile or when two supplied keys have the same identifier.
    pub fn new(
        pins: ProfilePinsV1,
        session: OperatedSessionBindingV1,
        keys: impl IntoIterator<Item = ResolvedPublicKeyV1>,
    ) -> Result<Self, AdapterError> {
        pins.validate()?;
        let mut indexed = PublicKeyMap::new();
        for key in keys {
            let key_id = key.key_id.clone();
            if indexed.insert(key_id, key).is_some() {
                return Err(AdapterError::DuplicateKey);
            }
        }
        Ok(Self {
            pins,
            session,
            keys: indexed,
        })
    }

    #[must_use]
    pub const fn pins(&self) -> &ProfilePinsV1 {
        &self.pins
    }

    /// Verify and retain one exact, opaque A202 object.
    ///
    /// # Errors
    ///
    /// Returns an error when the media type, exact-byte digest, canonical form,
    /// operated-session binding, content hash, or required signatures fail
    /// verification.
    pub fn ingest_object(
        &self,
        input: OpaqueA202InputV1,
        expected: &ExpectedA202ObjectV1,
    ) -> Result<ExactA202ObjectV1, AdapterError> {
        if input.media_type != A202_COMMERCIAL_MEDIA_TYPE {
            return Err(AdapterError::InvalidMediaType);
        }
        if input.exact_bytes.len() > MAX_A202_OBJECT_BYTES {
            return Err(AdapterError::ObjectTooLarge);
        }
        let actual_wire_digest = exact_blake3_digest(&input.exact_bytes);
        if actual_wire_digest != input.wire_digest {
            return Err(AdapterError::WireDigestMismatch);
        }
        let value = parse_exact_canonical(&input.exact_bytes)
            .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?;
        let object = value
            .as_object()
            .ok_or(AdapterError::InvalidMember("object"))?;

        if string_member(object.get("spec_version"), "spec_version")? != A202_SPEC_VERSION {
            return Err(AdapterError::SpecVersionMismatch);
        }
        let object_id = string_member(object.get("id"), "id")?;
        let object_type = string_member(object.get("object_type"), "object_type")?;
        if object_type != expected.object_type {
            return Err(AdapterError::ObjectTypeMismatch);
        }
        let transaction_id = string_member(object.get("transaction_id"), "transaction_id")?;
        if transaction_id != expected.transaction_id
            || transaction_id != self.session.transaction_id
        {
            return Err(AdapterError::TransactionMismatch);
        }
        validate_single_session(&value, &self.session)?;
        validate_profile_when_applicable(&value, object_type)?;

        let declared_content_hash = string_member(object.get("content_hash"), "content_hash")?;
        let recomputed_content_hash = a202_content_hash(&value)
            .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?;
        if declared_content_hash != recomputed_content_hash {
            return Err(AdapterError::ContentHashMismatch);
        }

        let signatures = object
            .get("signatures")
            .and_then(Value::as_array)
            .ok_or(AdapterError::InvalidMember("signatures"))?;
        let mut signature_verifications = Vec::with_capacity(expected.signatures.len());
        for expectation in &expected.signatures {
            signature_verifications.push(self.verify_expected_signature(
                &value,
                signatures,
                expectation,
            )?);
        }
        signature_verifications.sort_by(|left, right| {
            (&left.key_id, &left.purpose).cmp(&(&right.key_id, &right.purpose))
        });

        Ok(ExactA202ObjectV1 {
            media_type: input.media_type,
            exact_bytes_base64url: encode_base64url(&input.exact_bytes),
            wire_digest: actual_wire_digest,
            object_id: object_id.to_owned(),
            object_type: object_type.to_owned(),
            transaction_id: transaction_id.to_owned(),
            a202_content_hash: recomputed_content_hash,
            signature_verifications,
        })
    }

    #[must_use]
    pub fn prepare_submission(
        object: ExactA202ObjectV1,
        basis: SubmissionBasisV1,
    ) -> PreparedA202SubmissionV1 {
        PreparedA202SubmissionV1 { basis, object }
    }

    /// Prepare the operated submission boundary. A202 `expected_sequence`
    /// remains inside the signed `ActionEnvelope` while the Room Head remains
    /// outside as a `WorldStream` concurrency witness.
    ///
    /// # Errors
    ///
    /// Returns an error unless the object is a signed `ActionEnvelope` whose
    /// logical A202 head agrees with the external submission basis and whose
    /// signed bytes contain no Room concurrency witness.
    pub fn prepare_action_submission(
        object: ExactA202ObjectV1,
        basis: SubmissionBasisV1,
    ) -> Result<PreparedA202SubmissionV1, AdapterError> {
        if object.object_type != "action_envelope" {
            return Err(AdapterError::NotActionEnvelope);
        }
        let bytes = decode_base64url(&object.exact_bytes_base64url)?;
        let value = parse_exact_canonical(&bytes)
            .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?;
        if contains_room_witness(&value) {
            return Err(AdapterError::RoomWitnessInsideSignedObject);
        }
        let expected_sequence = value
            .pointer("/payload/expected_sequence")
            .and_then(Value::as_u64)
            .ok_or(AdapterError::InvalidMember("payload.expected_sequence"))?;
        let session = value.pointer("/payload/session_id");
        let targeted_sequence = match session {
            Some(Value::String(_)) => basis.session.sequence,
            Some(Value::Null) => basis.transaction.sequence,
            _ => return Err(AdapterError::InvalidMember("payload.session_id")),
        };
        if expected_sequence != targeted_sequence {
            return Err(AdapterError::A202ExpectedSequenceMismatch);
        }
        Ok(PreparedA202SubmissionV1 { basis, object })
    }

    /// Authenticate an exact resolver response under the configured source policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the response digest, source identity, attestation,
    /// or bounded validity window cannot be verified.
    pub fn ingest_resolver_evidence(
        &self,
        observation: UnsignedResolverEvidenceV1,
        source_attestation: DetachedEs256SignatureV1,
        policy: &ResolverEvidencePolicyV1,
    ) -> Result<VerifiedResolverEvidenceV1, AdapterError> {
        if observation.source_id != policy.source_id {
            return Err(AdapterError::ResolverSourceMismatch);
        }
        if source_attestation.key_id != policy.attestation_key_id {
            return Err(AdapterError::ResolverAttestationKeyMismatch);
        }
        if source_attestation.algorithm != "ES256"
            || source_attestation.purpose != RESOLVER_ATTESTATION_PURPOSE
        {
            return Err(AdapterError::ResolverAttestationMetadata);
        }
        let duration = observation
            .valid_until_unix_ms
            .checked_sub(observation.observed_at_unix_ms)
            .ok_or(AdapterError::ResolverValidityWindow)?;
        if duration == 0
            || policy.maximum_validity_ms == 0
            || policy.maximum_validity_ms > A202_MAX_STATUS_VALIDITY_MS
            || duration > policy.maximum_validity_ms
        {
            return Err(AdapterError::ResolverValidityWindow);
        }
        let response_bytes = decode_base64url(&observation.exact_response_base64url)?;
        if exact_blake3_digest(&response_bytes) != observation.response_digest {
            return Err(AdapterError::ResolverResponseDigestMismatch);
        }
        let public_key = decode_base64url(&policy.attestation_public_key_sec1_base64url)?;
        let signature = decode_base64url(&source_attestation.signature_base64url)?;
        let message = resolver_attestation_message(&observation, &source_attestation)?;
        verify_es256(&public_key, &message, &signature)
            .map_err(|_| AdapterError::ResolverAuthenticationFailed)?;

        Ok(VerifiedResolverEvidenceV1 {
            observation,
            source_attestation,
            authentication_result: VerificationOutcomeV1::Verified,
        })
    }

    fn verify_expected_signature(
        &self,
        value: &Value,
        signatures: &[Value],
        expected: &crate::model::ExpectedSignatureV1,
    ) -> Result<SignatureVerificationV1, AdapterError> {
        let matching_key = signatures.iter().find(|signature| {
            signature.get("key_id").and_then(Value::as_str) == Some(expected.key_id.as_str())
        });
        let Some(signature) = matching_key else {
            if signatures.iter().any(|candidate| {
                candidate.get("purpose").and_then(Value::as_str) == Some(expected.purpose.as_str())
            }) {
                return Err(AdapterError::WrongSigner);
            }
            return Err(AdapterError::SignatureMissing);
        };
        let purpose = signature
            .get("purpose")
            .and_then(Value::as_str)
            .ok_or(AdapterError::InvalidMember("signature.purpose"))?;
        if purpose != expected.purpose {
            return Err(AdapterError::WrongSignaturePurpose);
        }
        let algorithm = signature
            .get("algorithm")
            .and_then(Value::as_str)
            .ok_or(AdapterError::InvalidMember("signature.algorithm"))?;
        if algorithm != "ES256" {
            return Err(AdapterError::InvalidSignature);
        }
        let signed_at = signature
            .get("signed_at")
            .and_then(Value::as_str)
            .ok_or(AdapterError::InvalidMember("signature.signed_at"))?;
        let Some(key) = self.keys.get(&expected.key_id) else {
            return Err(AdapterError::SignatureNotCheckable);
        };
        if key.subject_id != expected.subject_id {
            return Err(AdapterError::WrongSigner);
        }
        if key.status_at != signed_at {
            return Err(AdapterError::SignatureNotCheckable);
        }
        if key.status_at_signing_time != KeyStatusV1::Active {
            return Err(AdapterError::SigningKeyInactive);
        }
        if expected.require_current_active && key.current_status != KeyStatusV1::Active {
            return Err(AdapterError::CurrentKeyInactive);
        }
        let public_key = decode_base64url(&key.public_key_sec1_base64url)?;
        let signature_bytes = decode_base64url(
            signature
                .get("signature")
                .and_then(Value::as_str)
                .ok_or(AdapterError::InvalidMember("signature.signature"))?,
        )?;
        let message =
            a202_signature_message(value, &expected.key_id, algorithm, purpose, signed_at)
                .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?;
        verify_es256(&public_key, &message, &signature_bytes)?;

        Ok(SignatureVerificationV1 {
            key_id: expected.key_id.clone(),
            subject_id: expected.subject_id.clone(),
            purpose: purpose.to_owned(),
            signed_at: signed_at.to_owned(),
            cryptographic_result: VerificationOutcomeV1::Verified,
            status_at_signing_time: key.status_at_signing_time,
            current_status: key.current_status,
            status_evidence_ids: key.status_evidence_ids.clone(),
        })
    }
}

fn string_member<'a>(
    value: Option<&'a Value>,
    name: &'static str,
) -> Result<&'a str, AdapterError> {
    value
        .and_then(Value::as_str)
        .ok_or(AdapterError::InvalidMember(name))
}

fn validate_profile_when_applicable(value: &Value, object_type: &str) -> Result<(), AdapterError> {
    if object_type != "offer" {
        return Ok(());
    }
    let profile = value
        .pointer("/payload/terms/profile")
        .and_then(Value::as_str)
        .ok_or(AdapterError::InvalidMember("payload.terms.profile"))?;
    if profile != A202_CALIBRATION_PROFILE {
        return Err(AdapterError::TransactionProfileMismatch);
    }
    Ok(())
}

fn validate_single_session(
    value: &Value,
    binding: &OperatedSessionBindingV1,
) -> Result<(), AdapterError> {
    if let Some(session_id) = value.pointer("/payload/session_id")
        && let Some(session_id) = session_id.as_str()
        && session_id != binding.session_id
    {
        return Err(AdapterError::SessionMismatch);
    }
    let stream_kind = value
        .pointer("/payload/stream/kind")
        .and_then(Value::as_str);
    let stream_id = value.pointer("/payload/stream/id").and_then(Value::as_str);
    match stream_kind {
        Some("session") if stream_id != Some(binding.session_id.as_str()) => {
            return Err(AdapterError::SessionMismatch);
        }
        Some("transaction") if stream_id != Some(binding.transaction_id.as_str()) => {
            return Err(AdapterError::TransactionMismatch);
        }
        _ => {}
    }
    Ok(())
}

fn contains_room_witness(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_room_witness),
        Value::Object(object) => object.iter().any(|(name, value)| {
            matches!(name.as_str(), "room_head" | "room_digest" | "room_sequence")
                || contains_room_witness(value)
        }),
        _ => false,
    }
}
