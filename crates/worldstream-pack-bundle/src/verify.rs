use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use worldstream_core::{
    Blake3DigestV1, CanonicalJsonV1, PackCodecBundleV1, PackDigestV1, PackGoldenCorpusV1,
    PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1, PackSchemaV1,
};

use crate::{
    BundleManifestV1, CANONICAL_CODEC_ID, CODEC_BUNDLE_MEMBER, CONFORMANCE_MEMBER,
    DEPENDENCY_LOCK_MEMBER, DESCRIPTOR_MEMBER, EXECUTION_PROFILE_ID, EXECUTOR_MEMBER,
    GOLDEN_CORPUS_MEMBER, HOST_CONTRACT_ID, MANIFEST_MEMBER, PackBundleDigestV1, PackBundleErrorV1,
    REQUIRED_MEMBERS, REVISION_LOCK_MEMBER, SCHEMAS_MEMBER,
    manifest::{CONFORMANCE_ID, ConformanceDocumentV1, REVISION_LOCK_ID, SchemaBundleDocumentV1},
    ustar::{
        CanonicalUstarV1, MAX_COMPONENT_BYTES, MAX_JSON_MEMBER_BYTES, MAX_STATIC_MEMBER_BYTES,
        validate_member_name,
    },
};

const GOLDEN_CORPUS_ID: &str = "worldstream/pack-golden-corpus/v1";

/// Read-only summary of one fully verified physical bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackBundleInspectionV1 {
    pub bundle_digest: PackBundleDigestV1,
    pub revision_digest: PackDigestV1,
    pub pack_id: String,
    pub explanatory_version: String,
    pub member_count: usize,
    pub byte_count: usize,
}

/// Opaque proof that exact archive bytes passed all bundle-layer checks.
#[derive(Clone)]
pub struct VerifiedPackBundleV1 {
    archive: CanonicalUstarV1,
    bundle_digest: PackBundleDigestV1,
    manifest_digest: Blake3DigestV1,
    manifest: BundleManifestV1,
    pack_id: String,
    explanatory_version: String,
    revision_lock: PackRevisionLockV1,
    descriptor: PackRevisionDescriptorV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    golden_corpus: PackGoldenCorpusV1,
    golden_corpus_digest: Blake3DigestV1,
}

impl VerifiedPackBundleV1 {
    #[must_use]
    pub fn inspection(&self) -> PackBundleInspectionV1 {
        PackBundleInspectionV1 {
            bundle_digest: self.bundle_digest.clone(),
            revision_digest: self.manifest.revision_digest.clone(),
            pack_id: self.pack_id.clone(),
            explanatory_version: self.explanatory_version.clone(),
            member_count: self.archive.member_names().len(),
            byte_count: self.archive.bytes().len(),
        }
    }

    #[must_use]
    pub fn bundle_digest(&self) -> &PackBundleDigestV1 {
        &self.bundle_digest
    }

    #[must_use]
    pub fn revision_digest(&self) -> &PackDigestV1 {
        &self.manifest.revision_digest
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Blake3DigestV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn manifest(&self) -> &BundleManifestV1 {
        &self.manifest
    }

    /// Returns the typed semantic lock decoded from the verified exact member.
    #[must_use]
    pub fn revision_lock(&self) -> &PackRevisionLockV1 {
        &self.revision_lock
    }

    /// Returns the complete typed descriptor bound to the verified lock.
    #[must_use]
    pub fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        &self.descriptor
    }

    /// Returns the validated schema documents required by Core admission.
    #[must_use]
    pub fn schemas(&self) -> &PackSchemaBundleV1 {
        &self.schemas
    }

    /// Returns the frozen retained codec declaration required by Core admission.
    #[must_use]
    pub fn codecs(&self) -> &PackCodecBundleV1 {
        &self.codecs
    }

    /// Returns the typed retained behavioral corpus required by Core admission.
    #[must_use]
    pub fn golden_corpus(&self) -> &PackGoldenCorpusV1 {
        &self.golden_corpus
    }

    /// Returns the digest of the exact canonical golden-corpus member.
    #[must_use]
    pub fn golden_corpus_digest(&self) -> &Blake3DigestV1 {
        &self.golden_corpus_digest
    }

    /// Returns the exact verified Component digest bound by the revision lock.
    #[must_use]
    pub fn component_digest(&self) -> &Blake3DigestV1 {
        &self.revision_lock.rule_source_digest
    }

    #[must_use]
    pub fn archive_bytes(&self) -> &[u8] {
        self.archive.bytes()
    }

    #[must_use]
    pub fn component_bytes(&self) -> &[u8] {
        self.archive
            .member(EXECUTOR_MEMBER)
            .unwrap_or_else(|| unreachable!("verified bundle retains the Component member"))
    }
}

/// Deep verifier from untrusted `.wspack` bytes to one opaque verified value.
#[derive(Clone, Copy, Debug, Default)]
pub struct PackBundleVerifierV1;

impl PackBundleVerifierV1 {
    /// Verifies a bounded, canonical, closed `.wspack` and exposes only
    /// artifacts that are safe to pass into Core portable admission.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed format, identity, limit, or typed-artifact error.
    pub fn inspect(self, bytes: Arc<[u8]>) -> Result<VerifiedPackBundleV1, PackBundleErrorV1> {
        let archive = CanonicalUstarV1::parse(bytes)?;
        let manifest_bytes = required_member(&archive, MANIFEST_MEMBER)?;
        enforce_json_bound(manifest_bytes)?;
        let manifest: BundleManifestV1 = decode_canonical_member(manifest_bytes, MANIFEST_MEMBER)?;
        validate_manifest_contract(&manifest)?;
        validate_member_set(&archive, &manifest)?;
        validate_member_digests(&archive, &manifest)?;
        validate_json_members(&archive)?;

        let lock_bytes = required_member(&archive, REVISION_LOCK_MEMBER)?;
        let lock = PackRevisionLockV1::from_canonical_bytes(lock_bytes, &manifest.revision_digest)
            .map_err(|error| typed_member_error(REVISION_LOCK_MEMBER, error))?;
        if lock.canonical_bytes().map_err(PackBundleErrorV1::from)? != lock_bytes {
            return Err(PackBundleErrorV1::TypedJson(
                "revision-lock typed encoding is not canonical".to_owned(),
            ));
        }
        validate_lock(&manifest, &lock, lock_bytes)?;

        let descriptor_bytes = required_member(&archive, DESCRIPTOR_MEMBER)?;
        let descriptor: PackRevisionDescriptorV1 =
            decode_typed_canonical(descriptor_bytes, DESCRIPTOR_MEMBER)?;
        validate_descriptor(&manifest, &lock, &descriptor)?;

        let schemas_bytes = required_member(&archive, SCHEMAS_MEMBER)?;
        let schemas: SchemaBundleDocumentV1 =
            decode_canonical_member(schemas_bytes, SCHEMAS_MEMBER)?;
        let (schemas_by_id, core_schemas) = validate_schemas(&lock, &schemas)?;
        validate_descriptor_schema_references(&descriptor, &schemas_by_id)?;

        let codec_bytes = required_member(&archive, CODEC_BUNDLE_MEMBER)?;
        let codecs: PackCodecBundleV1 = decode_typed_canonical(codec_bytes, CODEC_BUNDLE_MEMBER)?;
        validate_codecs(&lock, &codecs)?;
        validate_component_and_static(&archive, &manifest, &lock)?;
        validate_conformance(&archive, &manifest, &lock)?;
        let golden_bytes = required_member(&archive, GOLDEN_CORPUS_MEMBER)?;
        let golden_corpus: PackGoldenCorpusV1 =
            decode_typed_canonical(golden_bytes, GOLDEN_CORPUS_MEMBER)?;
        validate_golden_corpus(&manifest, &golden_corpus, golden_bytes)?;
        let golden_corpus_digest = Blake3DigestV1::hash(golden_bytes);

        let bundle_digest = PackBundleDigestV1::hash(archive.bytes());
        Ok(VerifiedPackBundleV1 {
            bundle_digest,
            manifest_digest: Blake3DigestV1::hash(manifest_bytes),
            pack_id: lock.pack_id.clone(),
            explanatory_version: lock.explanatory_version.clone(),
            manifest,
            archive,
            revision_lock: lock,
            descriptor,
            schemas: core_schemas,
            codecs,
            golden_corpus,
            golden_corpus_digest,
        })
    }
}

fn validate_manifest_contract(manifest: &BundleManifestV1) -> Result<(), PackBundleErrorV1> {
    if manifest.bundle_format_id != crate::BUNDLE_FORMAT_ID
        || manifest.execution_profile_id != EXECUTION_PROFILE_ID
        || manifest.host_contract != HOST_CONTRACT_ID
        || manifest.canonical_codec != CANONICAL_CODEC_ID
    {
        return Err(PackBundleErrorV1::UnsupportedContract);
    }
    if !strictly_sorted_unique(manifest.members.iter().map(|member| member.name.as_str()))
        || !strictly_sorted_unique(manifest.static_members.iter().map(String::as_str))
        || manifest.members.iter().any(|member| {
            member.name == MANIFEST_MEMBER || validate_member_name(&member.name).is_err()
        })
        || manifest
            .static_members
            .iter()
            .any(|name| !name.starts_with("static/") || validate_member_name(name).is_err())
    {
        return Err(PackBundleErrorV1::ArchiveNotCanonical);
    }
    Ok(())
}

fn validate_member_set(
    archive: &CanonicalUstarV1,
    manifest: &BundleManifestV1,
) -> Result<(), PackBundleErrorV1> {
    let manifest_names: BTreeSet<&str> = manifest
        .members
        .iter()
        .map(|member| member.name.as_str())
        .collect();
    let static_names: BTreeSet<&str> = manifest.static_members.iter().map(String::as_str).collect();
    for required in REQUIRED_MEMBERS {
        if archive.member(required).is_none() {
            return Err(PackBundleErrorV1::MissingMember(required));
        }
        if required != MANIFEST_MEMBER && !manifest_names.contains(required) {
            return Err(PackBundleErrorV1::MissingMember(required));
        }
    }
    for name in archive.member_names() {
        if name != MANIFEST_MEMBER && !manifest_names.contains(name) {
            return Err(PackBundleErrorV1::UnexpectedMember(name.to_owned()));
        }
        if name.starts_with("static/") && !static_names.contains(name) {
            return Err(PackBundleErrorV1::UnexpectedMember(name.to_owned()));
        }
    }
    for name in &manifest.static_members {
        if !manifest_names.contains(name.as_str()) || archive.member(name).is_none() {
            return Err(PackBundleErrorV1::UnexpectedMember(name.clone()));
        }
    }
    if archive.member_names().len() != manifest.members.len() + 1 {
        return Err(PackBundleErrorV1::ArchiveNotCanonical);
    }
    Ok(())
}

fn validate_member_digests(
    archive: &CanonicalUstarV1,
    manifest: &BundleManifestV1,
) -> Result<(), PackBundleErrorV1> {
    for declared in &manifest.members {
        let actual = archive
            .member(&declared.name)
            .ok_or_else(|| PackBundleErrorV1::UnexpectedMember(declared.name.clone()))?;
        let size = u64::try_from(actual.len()).map_err(|_| PackBundleErrorV1::LimitExceeded)?;
        if size != declared.size || Blake3DigestV1::hash(actual) != declared.blake3 {
            return Err(PackBundleErrorV1::MemberDigestMismatch(
                declared.name.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_json_members(archive: &CanonicalUstarV1) -> Result<(), PackBundleErrorV1> {
    for name in REQUIRED_MEMBERS {
        if name == EXECUTOR_MEMBER {
            continue;
        }
        let bytes = required_member(archive, name)?;
        enforce_json_bound(bytes)?;
        CanonicalJsonV1::from_canonical_bytes(bytes)?;
    }
    for name in archive
        .member_names()
        .filter(|name| name.starts_with("static/"))
    {
        let bytes = archive
            .member(name)
            .ok_or(PackBundleErrorV1::ArchiveNotCanonical)?;
        if bytes.len() > MAX_STATIC_MEMBER_BYTES {
            return Err(PackBundleErrorV1::LimitExceeded);
        }
    }
    Ok(())
}

fn validate_lock(
    manifest: &BundleManifestV1,
    lock: &PackRevisionLockV1,
    lock_bytes: &[u8],
) -> Result<(), PackBundleErrorV1> {
    if lock.revision_lock_id != REVISION_LOCK_ID
        || lock.host_contract != HOST_CONTRACT_ID
        || lock.canonical_codec != CANONICAL_CODEC_ID
    {
        return Err(PackBundleErrorV1::UnsupportedContract);
    }
    if lock.pack_id.is_empty()
        || lock.explanatory_version.is_empty()
        || !strictly_sorted_unique(
            lock.deterministic_static_data_digests
                .iter()
                .map(|entry| entry.name.as_str()),
        )
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "invalid revision-lock shape",
        ));
    }
    if manifest.revision_digest.digest() != &Blake3DigestV1::hash(lock_bytes) {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "revision digest",
        ));
    }
    Ok(())
}

fn validate_descriptor(
    manifest: &BundleManifestV1,
    lock: &PackRevisionLockV1,
    descriptor: &PackRevisionDescriptorV1,
) -> Result<(), PackBundleErrorV1> {
    if descriptor.pack_id != lock.pack_id
        || descriptor.explanatory_version != lock.explanatory_version
        || descriptor.host_contract != HOST_CONTRACT_ID
        || descriptor.canonical_codec != CANONICAL_CODEC_ID
        || descriptor.revision_digest != manifest.revision_digest
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "descriptor identity",
        ));
    }
    if descriptor
        .content_digest()
        .map_err(PackBundleErrorV1::from)?
        != lock.descriptor_digest
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "descriptor digest",
        ));
    }
    Ok(())
}

fn validate_schemas(
    lock: &PackRevisionLockV1,
    schemas: &SchemaBundleDocumentV1,
) -> Result<(BTreeMap<String, Blake3DigestV1>, PackSchemaBundleV1), PackBundleErrorV1> {
    if schemas.schemas.is_empty()
        || !strictly_sorted_unique(schemas.schemas.iter().map(|entry| entry.schema_id.as_str()))
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "schema bundle ordering",
        ));
    }
    let mut by_id = BTreeMap::new();
    let mut core_schemas = Vec::with_capacity(schemas.schemas.len());
    for schema in &schemas.schemas {
        if schema.schema_id.is_empty() {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "empty schema identity",
            ));
        }
        let schema_bytes = canonical_value_bytes(&schema.canonical_schema)?;
        if Blake3DigestV1::hash(&schema_bytes) != schema.schema_digest {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "schema content digest",
            ));
        }
        let canonical_schema = CanonicalJsonV1::parse(&schema_bytes)?;
        let core_schema = PackSchemaV1::new(schema.schema_id.clone(), canonical_schema)
            .map_err(PackBundleErrorV1::from)?;
        if core_schema.reference().schema_digest != schema.schema_digest {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "schema content digest",
            ));
        }
        by_id.insert(schema.schema_id.clone(), schema.schema_digest.clone());
        core_schemas.push(core_schema);
    }
    let core_bundle = PackSchemaBundleV1::new(core_schemas).map_err(core_artifact_error)?;
    if core_bundle.digest().map_err(PackBundleErrorV1::from)? != lock.schema_bundle_digest {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "schema bundle digest",
        ));
    }
    Ok((by_id, core_bundle))
}

fn validate_descriptor_schema_references(
    descriptor: &PackRevisionDescriptorV1,
    schemas: &BTreeMap<String, Blake3DigestV1>,
) -> Result<(), PackBundleErrorV1> {
    let references = std::iter::once(&descriptor.configuration_schema)
        .chain(std::iter::once(&descriptor.state_schema))
        .chain(
            descriptor
                .actions
                .iter()
                .map(|action| &action.payload_schema),
        )
        .chain(descriptor.stimulus_schemas.values())
        .chain(descriptor.output_schemas.values())
        .chain(descriptor.event_schemas.values())
        .chain(descriptor.projection_schemas.values())
        .chain(descriptor.observation_schemas.values());
    for reference in references {
        if schemas.get(&reference.schema_id) != Some(&reference.schema_digest) {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "descriptor schema reference",
            ));
        }
    }
    Ok(())
}

fn validate_codecs(
    lock: &PackRevisionLockV1,
    codecs: &PackCodecBundleV1,
) -> Result<(), PackBundleErrorV1> {
    let canonical_codecs = PackCodecBundleV1::canonical_v1();
    if codecs.codec_id != CANONICAL_CODEC_ID
        || codecs.writer_version != 1
        || codecs.retained_reader_versions != [1]
        || *codecs != canonical_codecs
    {
        return Err(PackBundleErrorV1::UnsupportedContract);
    }
    if codecs.digest().map_err(PackBundleErrorV1::from)? != lock.codec_bundle_digest {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "codec bundle digest",
        ));
    }
    Ok(())
}

fn validate_component_and_static(
    archive: &CanonicalUstarV1,
    manifest: &BundleManifestV1,
    lock: &PackRevisionLockV1,
) -> Result<(), PackBundleErrorV1> {
    let component = required_member(archive, EXECUTOR_MEMBER)?;
    if component.is_empty() || component.len() > MAX_COMPONENT_BYTES {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    if Blake3DigestV1::hash(component) != lock.rule_source_digest {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "Component digest",
        ));
    }
    let dependency_lock = required_member(archive, DEPENDENCY_LOCK_MEMBER)?;
    if Blake3DigestV1::hash(dependency_lock) != lock.deterministic_dependency_lock_digest {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "dependency-lock digest",
        ));
    }
    let declared_static: Vec<&str> = lock
        .deterministic_static_data_digests
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    let manifest_static: Vec<&str> = manifest.static_members.iter().map(String::as_str).collect();
    if declared_static != manifest_static {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "static member inventory",
        ));
    }
    for declared in &lock.deterministic_static_data_digests {
        let bytes = archive
            .member(&declared.name)
            .ok_or_else(|| PackBundleErrorV1::UnexpectedMember(declared.name.clone()))?;
        if Blake3DigestV1::hash(bytes) != declared.digest {
            return Err(PackBundleErrorV1::SemanticIdentityMismatch(
                "static member digest",
            ));
        }
    }
    Ok(())
}

fn validate_conformance(
    archive: &CanonicalUstarV1,
    manifest: &BundleManifestV1,
    lock: &PackRevisionLockV1,
) -> Result<(), PackBundleErrorV1> {
    let bytes = required_member(archive, CONFORMANCE_MEMBER)?;
    let conformance: ConformanceDocumentV1 = decode_canonical_member(bytes, CONFORMANCE_MEMBER)?;
    let golden_digest = Blake3DigestV1::hash(required_member(archive, GOLDEN_CORPUS_MEMBER)?);
    if conformance.conformance_id != CONFORMANCE_ID
        || conformance.execution_profile_id != EXECUTION_PROFILE_ID
        || conformance.revision_digest != manifest.revision_digest
        || conformance.executor_component_digest != lock.rule_source_digest
        || conformance.golden_corpus_digest != golden_digest
        || !conformance.passed
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "conformance evidence",
        ));
    }
    Ok(())
}

fn validate_golden_corpus(
    manifest: &BundleManifestV1,
    golden_corpus: &PackGoldenCorpusV1,
    bytes: &[u8],
) -> Result<(), PackBundleErrorV1> {
    if golden_corpus.corpus_id != GOLDEN_CORPUS_ID
        || golden_corpus.genesis.pack_digest != manifest.revision_digest
    {
        return Err(PackBundleErrorV1::SemanticIdentityMismatch(
            "golden corpus identity",
        ));
    }
    if golden_corpus.digest().map_err(PackBundleErrorV1::from)? != Blake3DigestV1::hash(bytes) {
        return Err(PackBundleErrorV1::TypedJson(
            "golden-corpus typed encoding is not canonical".to_owned(),
        ));
    }
    Ok(())
}

fn required_member<'a>(
    archive: &'a CanonicalUstarV1,
    name: &'static str,
) -> Result<&'a [u8], PackBundleErrorV1> {
    archive
        .member(name)
        .ok_or(PackBundleErrorV1::MissingMember(name))
}

fn enforce_json_bound(bytes: &[u8]) -> Result<(), PackBundleErrorV1> {
    if bytes.len() > MAX_JSON_MEMBER_BYTES {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    Ok(())
}

fn decode_canonical_member<T: DeserializeOwned>(
    bytes: &[u8],
    member: &'static str,
) -> Result<T, PackBundleErrorV1> {
    CanonicalJsonV1::decode_canonical(bytes).map_err(|error| typed_member_error(member, error))
}

fn decode_typed_canonical<T>(bytes: &[u8], member: &'static str) -> Result<T, PackBundleErrorV1>
where
    T: DeserializeOwned + Serialize,
{
    let value: T = decode_canonical_member(bytes, member)?;
    if canonical_serialize(&value)? != bytes {
        return Err(typed_member_error(
            member,
            "typed artifact has more than one accepted wire representation",
        ));
    }
    Ok(value)
}

fn canonical_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, PackBundleErrorV1> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| PackBundleErrorV1::TypedJson(error.to_string()))?;
    Ok(CanonicalJsonV1::parse(&bytes)?.to_bytes()?)
}

fn canonical_value_bytes(value: &Value) -> Result<Vec<u8>, PackBundleErrorV1> {
    canonical_serialize(value)
}

fn core_artifact_error(error: impl std::fmt::Display) -> PackBundleErrorV1 {
    PackBundleErrorV1::CoreArtifact(error.to_string())
}

fn typed_member_error(member: &'static str, error: impl std::fmt::Display) -> PackBundleErrorV1 {
    PackBundleErrorV1::InvalidTypedMember {
        member,
        detail: error.to_string(),
    }
}

fn strictly_sorted_unique<'a>(values: impl IntoIterator<Item = &'a str>) -> bool {
    let mut previous: Option<&str> = None;
    for value in values {
        if value.is_empty() || previous.is_some_and(|prior| prior >= value) {
            return false;
        }
        previous = Some(value);
    }
    true
}
