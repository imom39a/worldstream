use std::sync::Arc;

use serde::{Deserialize, Serialize};
use worldstream_core::PackDigestV1;

use crate::{PackBundleDigestV1, PackBundleErrorV1, PackBundleVerifierV1, VerifiedPackBundleV1};

/// Canonical schema for an exact portable Pack archive carried by backup or
/// transfer evidence.
pub const RETAINED_PACK_BUNDLE_ARTIFACT_ID: &str = "worldstream/retained-pack-bundle-artifact/v1";

/// Original `.wspack` bytes plus the two identities needed to reject physical
/// substitution and semantic revision substitution before restore.
///
/// This value deliberately carries no approval or selectability state. A
/// target Host Operator must make a new local approval decision after restore.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedPackBundleArtifactV1 {
    pub artifact_id: String,
    pub bundle_digest: PackBundleDigestV1,
    pub revision_digest: PackDigestV1,
    pub archive_bytes: Vec<u8>,
}

impl RetainedPackBundleArtifactV1 {
    /// Builds a transport artifact only from bytes accepted by the complete
    /// production bundle verifier.
    ///
    /// # Errors
    ///
    /// Returns the closed bundle verifier error for malformed, oversized, or
    /// inconsistent archive bytes.
    pub fn from_archive_bytes(bytes: Vec<u8>) -> Result<Self, PackBundleErrorV1> {
        let verified = PackBundleVerifierV1.inspect(Arc::from(bytes.clone()))?;
        Ok(Self {
            artifact_id: RETAINED_PACK_BUNDLE_ARTIFACT_ID.to_owned(),
            bundle_digest: verified.bundle_digest().clone(),
            revision_digest: verified.revision_digest().clone(),
            archive_bytes: bytes,
        })
    }

    /// Re-runs complete verification and requires both carried identities to
    /// equal the identities derived from the exact original bytes.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed bundle error for a malformed artifact, digest
    /// mismatch, or semantic substitution.
    pub fn verify(&self) -> Result<VerifiedPackBundleV1, PackBundleErrorV1> {
        if self.artifact_id != RETAINED_PACK_BUNDLE_ARTIFACT_ID {
            return Err(PackBundleErrorV1::TypedJson(
                "retained Pack Bundle artifact identity is unsupported".to_owned(),
            ));
        }
        let verified = PackBundleVerifierV1.inspect(Arc::from(self.archive_bytes.clone()))?;
        if verified.bundle_digest() != &self.bundle_digest {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "retained bundle physical digest",
            ));
        }
        if verified.revision_digest() != &self.revision_digest {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "retained bundle revision digest",
            ));
        }
        Ok(verified)
    }
}
