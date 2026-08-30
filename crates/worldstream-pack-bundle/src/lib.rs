//! Canonical, offline Activity Pack Bundle verification and local lifecycle.

mod artifact;
mod error;
mod manifest;
mod store;
mod ustar;
mod verify;

#[cfg(test)]
mod tests;

pub use artifact::{RETAINED_PACK_BUNDLE_ARTIFACT_ID, RetainedPackBundleArtifactV1};
pub use error::PackBundleErrorV1;
pub use manifest::{
    BUNDLE_FORMAT_ID, BundleManifestV1, BundleMemberManifestV1, CANONICAL_CODEC_ID,
    CODEC_BUNDLE_DOMAIN, CODEC_BUNDLE_MEMBER, CONFORMANCE_ID, CONFORMANCE_MEMBER,
    DEPENDENCY_LOCK_MEMBER, DESCRIPTOR_MEMBER, EXECUTION_PROFILE_ID, EXECUTOR_MEMBER,
    GOLDEN_CORPUS_MEMBER, HOST_CONTRACT_ID, MANIFEST_MEMBER, PackBundleDigestV1, REQUIRED_MEMBERS,
    REVISION_LOCK_ID, REVISION_LOCK_MEMBER, SCHEMA_BUNDLE_DOMAIN, SCHEMAS_MEMBER,
};
pub use store::{
    ApprovalDecisionV1, InstalledPackBundleV1, MAX_INSTALLED_BUNDLE_COUNT, OperatorApprovalV1,
    PackBundleInventoryCountsV1, PackBundleStartupEntryV1, PackBundleStartupInventoryV1,
    PackBundleStoreV1, PackInstallStateV1, PackRemovalV1, PackStartupReadinessSealV1,
    RetainedRevisionSourceV1,
};
pub use ustar::{
    MAX_BUNDLE_BYTES, MAX_COMPONENT_BYTES, MAX_JSON_MEMBER_BYTES, MAX_MEMBER_COUNT,
    MAX_STATIC_MEMBER_BYTES, PackBundleWriterV1,
};
pub use verify::{PackBundleInspectionV1, PackBundleVerifierV1, VerifiedPackBundleV1};
