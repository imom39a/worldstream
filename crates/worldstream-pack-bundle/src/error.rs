use std::io;

use thiserror::Error;
use worldstream_core::CanonicalJsonError;

/// Fail-closed Activity Pack Bundle verification and lifecycle errors.
#[derive(Debug, Error)]
pub enum PackBundleErrorV1 {
    #[error("Activity Pack Bundle exceeds a fixed byte or member bound")]
    LimitExceeded,
    #[error("Activity Pack Bundle archive framing is not canonical ustar")]
    ArchiveNotCanonical,
    #[error("Activity Pack Bundle archive metadata is invalid")]
    ArchiveMetadataInvalid,
    #[error("Activity Pack Bundle member path is unsafe: {0}")]
    UnsafeMemberPath(String),
    #[error("Activity Pack Bundle contains duplicate member: {0}")]
    DuplicateMember(String),
    #[error("Activity Pack Bundle contains unexpected member: {0}")]
    UnexpectedMember(String),
    #[error("Activity Pack Bundle is missing required member: {0}")]
    MissingMember(&'static str),
    #[error("Activity Pack Bundle member digest or size differs from its manifest: {0}")]
    MemberDigestMismatch(String),
    #[error("Activity Pack Bundle contains malformed typed JSON: {0}")]
    TypedJson(String),
    #[error("Activity Pack Bundle member {member} is not a valid typed artifact: {detail}")]
    InvalidTypedMember {
        member: &'static str,
        detail: String,
    },
    #[error("Activity Pack Bundle contains an invalid Core admission artifact: {0}")]
    CoreArtifact(String),
    #[error("Activity Pack Bundle semantic identities disagree: {0}")]
    SemanticIdentityMismatch(&'static str),
    #[error("Activity Pack Bundle uses an unsupported frozen contract")]
    UnsupportedContract,
    #[error("Activity Pack Bundle has not been approved by exact physical digest")]
    ApprovalMissing,
    #[error("Activity Pack Bundle approval does not match the exact verified bytes")]
    ApprovalDigestMismatch,
    #[error("a different physical bundle is already installed for this semantic revision")]
    SemanticRevisionAlreadyInstalled,
    #[error("installed Activity Pack Bundle object is absent or corrupt")]
    CorruptInstalledObject,
    #[error("installed Activity Pack Bundle inventory is not canonical")]
    InventoryNotCanonical,
    #[error("installed Activity Pack startup readiness seal is missing")]
    StartupReadinessMissing,
    #[error("installed Activity Pack startup readiness seal does not match the frozen target")]
    StartupReadinessMismatch,
    #[error("staged Activity Pack Bundle bytes differ from the verified source")]
    StagedObjectMismatch,
    #[error("Activity Pack Revision remains referenced by Room lineage")]
    RevisionReferenced,
    #[error("Activity Pack Bundle is not installed")]
    NotInstalled,
    #[error("retained Room reference lookup failed")]
    ReferenceQuery,
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error("Activity Pack Bundle filesystem operation failed: {0}")]
    Io(#[from] io::Error),
}
