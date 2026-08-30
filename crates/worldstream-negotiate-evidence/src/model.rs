use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResultV1 {
    Verified,
    Failed,
    NotCheckable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSectionV1 {
    PartyProtocol,
    VenueRuntime,
    CrossIndex,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceCheckV1 {
    pub section: EvidenceSectionV1,
    pub check: String,
    pub subject: String,
    pub result: CheckResultV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal_code: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceScopeV1 {
    pub transaction_id: String,
    pub session_id: String,
    pub room_id: String,
    pub objects: Vec<String>,
    pub venue_records: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NegotiationVerificationReportV1 {
    pub report_format: String,
    pub profile_revision: String,
    pub scope: EvidenceScopeV1,
    pub checks: Vec<EvidenceCheckV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrustedKeyV1 {
    pub key_id: String,
    pub subject_id: String,
    pub public_key_sec1_base64url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrustedResolverSourceV1 {
    pub source_id: String,
    pub key_id: String,
    pub public_key_sec1_base64url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifierTrustV1 {
    pub a202_keys: Vec<TrustedKeyV1>,
    pub resolver_sources: Vec<TrustedResolverSourceV1>,
}
