use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use worldstream_core::{Blake3DigestV1, PackDigestV1};

use crate::PackBundleErrorV1;

pub const BUNDLE_FORMAT_ID: &str = "worldstream/activity-pack-bundle/v1";
pub const EXECUTION_PROFILE_ID: &str = "worldstream/component-deterministic/v1";
pub const HOST_CONTRACT_ID: &str = "worldstream/activity-pack/v1";
pub const CANONICAL_CODEC_ID: &str = "worldstream/canonical-json/v1";
pub const REVISION_LOCK_ID: &str = "worldstream/pack-revision-lock/v1";
pub const SCHEMA_BUNDLE_DOMAIN: &str = "worldstream/pack-schema-bundle/v1";
pub const CODEC_BUNDLE_DOMAIN: &str = "worldstream/pack-codec-bundle/v1";
pub const CONFORMANCE_ID: &str = "worldstream/pack-conformance/v1";

pub const MANIFEST_MEMBER: &str = "bundle-manifest.json";
pub const REVISION_LOCK_MEMBER: &str = "revision-lock.json";
pub const DESCRIPTOR_MEMBER: &str = "descriptor.json";
pub const SCHEMAS_MEMBER: &str = "schemas.json";
pub const CODEC_BUNDLE_MEMBER: &str = "codec-bundle.json";
pub const EXECUTOR_MEMBER: &str = "executor.component.wasm";
pub const GOLDEN_CORPUS_MEMBER: &str = "golden-corpus.json";
pub const CONFORMANCE_MEMBER: &str = "conformance.json";
pub const DEPENDENCY_LOCK_MEMBER: &str = "dependency-lock.json";

pub const REQUIRED_MEMBERS: [&str; 9] = [
    MANIFEST_MEMBER,
    REVISION_LOCK_MEMBER,
    DESCRIPTOR_MEMBER,
    SCHEMAS_MEMBER,
    CODEC_BUNDLE_MEMBER,
    EXECUTOR_MEMBER,
    GOLDEN_CORPUS_MEMBER,
    CONFORMANCE_MEMBER,
    DEPENDENCY_LOCK_MEMBER,
];

/// BLAKE3 identity of every exact byte in one `.wspack` archive.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PackBundleDigestV1(Blake3DigestV1);

impl PackBundleDigestV1 {
    #[must_use]
    pub fn hash(bytes: &[u8]) -> Self {
        Self(Blake3DigestV1::hash(bytes))
    }

    #[must_use]
    pub fn digest(&self) -> &Blake3DigestV1 {
        &self.0
    }

    #[must_use]
    pub fn path_component(&self) -> String {
        self.to_string()
            .strip_prefix("blake3:")
            .unwrap_or_else(|| unreachable!("validated BLAKE3 text has a prefix"))
            .to_owned()
    }
}

impl fmt::Display for PackBundleDigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PackBundleDigestV1 {
    type Err = PackBundleErrorV1;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse()
            .map(Self)
            .map_err(|_| PackBundleErrorV1::TypedJson("invalid bundle digest".to_owned()))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMemberManifestV1 {
    pub blake3: Blake3DigestV1,
    pub name: String,
    pub size: u64,
}

/// Closed, non-self-referential manifest for one physical bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifestV1 {
    pub bundle_format_id: String,
    pub canonical_codec: String,
    pub execution_profile_id: String,
    pub host_contract: String,
    pub members: Vec<BundleMemberManifestV1>,
    pub revision_digest: PackDigestV1,
    pub static_members: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SchemaDocumentV1 {
    pub(crate) canonical_schema: serde_json::Value,
    pub(crate) schema_digest: Blake3DigestV1,
    pub(crate) schema_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SchemaBundleDocumentV1 {
    pub(crate) schemas: Vec<SchemaDocumentV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConformanceDocumentV1 {
    pub(crate) conformance_id: String,
    pub(crate) execution_profile_id: String,
    pub(crate) executor_component_digest: Blake3DigestV1,
    pub(crate) golden_corpus_digest: Blake3DigestV1,
    pub(crate) passed: bool,
    pub(crate) revision_digest: PackDigestV1,
}
