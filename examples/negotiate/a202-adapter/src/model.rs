use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const A202_PINNED_REVISION: &str = "fa85aa8b49bfe7b3f7ded487c98500a600e92e41";
pub const A202_SPEC_VERSION: &str = "a202-commercial/0.1";
pub const A202_RULES_VERSION: &str = "1.3";
pub const A202_COMMERCIAL_MEDIA_TYPE: &str = "application/a202-commercial+json";
pub const A202_OPERATED_SCOPE: &str = "a202-scope/operated/0.1";
pub const A202_BILATERAL_SCOPE: &str = "a202-scope/bilateral/0.1";
pub const A202_CALIBRATION_PROFILE: &str = "a202-profile/calibration-service/0.1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfilePinsV1 {
    pub repository_revision: String,
    pub spec_version: String,
    pub rules_version: String,
    pub operated_scope: String,
    pub bilateral_scope: String,
    pub transaction_profile: String,
}

impl Default for ProfilePinsV1 {
    fn default() -> Self {
        Self {
            repository_revision: A202_PINNED_REVISION.to_owned(),
            spec_version: A202_SPEC_VERSION.to_owned(),
            rules_version: A202_RULES_VERSION.to_owned(),
            operated_scope: A202_OPERATED_SCOPE.to_owned(),
            bilateral_scope: A202_BILATERAL_SCOPE.to_owned(),
            transaction_profile: A202_CALIBRATION_PROFILE.to_owned(),
        }
    }
}

impl ProfilePinsV1 {
    pub(crate) fn validate(&self) -> Result<(), AdapterError> {
        if self != &Self::default() {
            return Err(AdapterError::ProfileMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcomeV1 {
    Verified,
    Failed,
    NotCheckable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatusV1 {
    Active,
    Suspended,
    Revoked,
    Expired,
    Unresolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedPublicKeyV1 {
    pub key_id: String,
    pub subject_id: String,
    pub public_key_sec1_base64url: String,
    pub status_at_signing_time: KeyStatusV1,
    pub status_at: String,
    pub current_status: KeyStatusV1,
    pub status_evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignatureVerificationV1 {
    pub key_id: String,
    pub subject_id: String,
    pub purpose: String,
    pub signed_at: String,
    pub cryptographic_result: VerificationOutcomeV1,
    pub status_at_signing_time: KeyStatusV1,
    pub current_status: KeyStatusV1,
    pub status_evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExactA202ObjectV1 {
    pub media_type: String,
    pub exact_bytes_base64url: String,
    pub wire_digest: String,
    pub object_id: String,
    pub object_type: String,
    pub transaction_id: String,
    pub a202_content_hash: String,
    pub signature_verifications: Vec<SignatureVerificationV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpaqueA202InputV1 {
    pub media_type: String,
    pub exact_bytes: Vec<u8>,
    pub wire_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedSignatureV1 {
    pub key_id: String,
    pub subject_id: String,
    pub purpose: String,
    pub require_current_active: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedA202ObjectV1 {
    pub object_type: String,
    pub transaction_id: String,
    pub signatures: Vec<ExpectedSignatureV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatedSessionBindingV1 {
    pub transaction_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoomHeadWitnessV1 {
    pub sequence: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogicalA202HeadV1 {
    pub sequence: u64,
    pub event_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubmissionBasisV1 {
    pub room: RoomHeadWitnessV1,
    pub transaction: LogicalA202HeadV1,
    pub session: LogicalA202HeadV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedA202SubmissionV1 {
    pub basis: SubmissionBasisV1,
    pub object: ExactA202ObjectV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RetryDecisionV1 {
    ReuseExactBytes {
        submission: Box<PreparedA202SubmissionV1>,
    },
    RebuildAndResign {
        transaction_head_changed: bool,
        session_head_changed: bool,
    },
}

impl PreparedA202SubmissionV1 {
    /// Rebase only the `WorldStream` concurrency witness. Any logical A202 Head
    /// change makes the already signed bytes stale and requires a fresh object.
    #[must_use]
    pub fn retry_against(&self, observed: SubmissionBasisV1) -> RetryDecisionV1 {
        let transaction_head_changed = self.basis.transaction != observed.transaction;
        let session_head_changed = self.basis.session != observed.session;
        if transaction_head_changed || session_head_changed {
            return RetryDecisionV1::RebuildAndResign {
                transaction_head_changed,
                session_head_changed,
            };
        }
        RetryDecisionV1::ReuseExactBytes {
            submission: Box::new(Self {
                basis: observed,
                object: self.object.clone(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostValidityResultV1 {
    Active,
    Inactive,
    Unresolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DetachedEs256SignatureV1 {
    pub key_id: String,
    pub algorithm: String,
    pub purpose: String,
    pub signed_at: String,
    pub signature_base64url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnsignedResolverEvidenceV1 {
    pub evidence_id: String,
    pub source_id: String,
    pub subject: String,
    pub observed_at_unix_ms: u64,
    pub valid_until_unix_ms: u64,
    pub media_type: String,
    pub exact_response_base64url: String,
    pub response_digest: String,
    pub host_validity_result: HostValidityResultV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifiedResolverEvidenceV1 {
    #[serde(flatten)]
    pub observation: UnsignedResolverEvidenceV1,
    pub source_attestation: DetachedEs256SignatureV1,
    pub authentication_result: VerificationOutcomeV1,
}

impl VerifiedResolverEvidenceV1 {
    /// Require an active resolver result at the requested evaluation time.
    ///
    /// # Errors
    ///
    /// Returns an error when the evidence is stale or the resolved subject was
    /// not active.
    pub fn require_active_at(&self, evaluation_time_unix_ms: u64) -> Result<(), AdapterError> {
        if evaluation_time_unix_ms < self.observation.observed_at_unix_ms
            || evaluation_time_unix_ms >= self.observation.valid_until_unix_ms
        {
            return Err(AdapterError::ResolverEvidenceStale);
        }
        if self.observation.host_validity_result != HostValidityResultV1::Active {
            return Err(AdapterError::ResolverSubjectInactive);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolverEvidencePolicyV1 {
    pub source_id: String,
    pub attestation_key_id: String,
    pub attestation_public_key_sec1_base64url: String,
    pub maximum_validity_ms: u64,
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("A202 compatibility-profile pins do not match ADR 0015")]
    ProfileMismatch,
    #[error("A202 object media type is not application/a202-commercial+json")]
    InvalidMediaType,
    #[error("A202 object exceeds the 256 KiB exact-byte limit")]
    ObjectTooLarge,
    #[error("A202 object is not valid canonical JSON: {0}")]
    InvalidCanonicalJson(String),
    #[error("A202 object has the wrong pinned spec version")]
    SpecVersionMismatch,
    #[error("A202 object type does not match the requested act")]
    ObjectTypeMismatch,
    #[error("A202 transaction binding does not match the Room")]
    TransactionMismatch,
    #[error("A202 session binding does not match the operated single session")]
    SessionMismatch,
    #[error("only a signed A202 ActionEnvelope can become an operated adapter submission")]
    NotActionEnvelope,
    #[error("signed A202 expected_sequence does not match its targeted logical Head")]
    A202ExpectedSequenceMismatch,
    #[error("WorldStream Room concurrency framing appeared inside signed A202 bytes")]
    RoomWitnessInsideSignedObject,
    #[error("A202 offer does not use the pinned calibration-service profile")]
    TransactionProfileMismatch,
    #[error("exact A202 byte digest does not match")]
    WireDigestMismatch,
    #[error("A202 content hash does not match canonical content")]
    ContentHashMismatch,
    #[error("required A202 signature is absent")]
    SignatureMissing,
    #[error("A202 signature signer does not match the expected identity")]
    WrongSigner,
    #[error("A202 signature purpose does not match the expected act")]
    WrongSignaturePurpose,
    #[error("A202 signature could not be checked from the supplied evidence")]
    SignatureNotCheckable,
    #[error("A202 ES256 signature verification failed")]
    InvalidSignature,
    #[error("A202 signing key was not active at signing time")]
    SigningKeyInactive,
    #[error("A202 signing key is not currently active")]
    CurrentKeyInactive,
    #[error("duplicate A202 verification key identifier")]
    DuplicateKey,
    #[error("base64url material is invalid")]
    InvalidBase64,
    #[error("resolver evidence source is not the configured Host Stimulus Source")]
    ResolverSourceMismatch,
    #[error("resolver evidence attestation key is not the configured key")]
    ResolverAttestationKeyMismatch,
    #[error("resolver evidence purpose or algorithm is invalid")]
    ResolverAttestationMetadata,
    #[error("resolver evidence validity interval is invalid or exceeds policy")]
    ResolverValidityWindow,
    #[error("resolver exact-response digest does not match")]
    ResolverResponseDigestMismatch,
    #[error("resolver source authentication failed")]
    ResolverAuthenticationFailed,
    #[error("resolver evidence is stale for the requested evaluation time")]
    ResolverEvidenceStale,
    #[error("resolver reported that the subject is inactive or unresolved")]
    ResolverSubjectInactive,
    #[error("required JSON member `{0}` is missing or has the wrong type")]
    InvalidMember(&'static str),
}

pub(crate) type PublicKeyMap = BTreeMap<String, ResolvedPublicKeyV1>;
