//! Provider-neutral, bounded logical transfer primitives.
//!
//! The crate provides a versioned byte bundle, bounded file export/import,
//! durable checkpoint/session seams, and a provider-facing destination
//! coordinator for finalization. It does not open `SQLite` or `PostgreSQL`
//! connections and makes no live-provider claim; adapters supply the
//! destination implementation and semantic verification.

#![allow(clippy::missing_errors_doc)]

mod native_sqlite;

pub use native_sqlite::{
    NativeSqliteBundleSummaryV1, NativeSqliteRoomPolicyV1, NativeSqliteTransferAdapterV1,
    NativeSqliteTransferError, NativeSqliteTransferSpecV1,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

const BUNDLE_MAGIC: &[u8; 8] = b"WSTRANS1";
const CHECKPOINT_MAGIC: &[u8; 8] = b"WSCHECK1";
const BUNDLE_VERSION: u16 = 3;
const LEGACY_BUNDLE_VERSION: u16 = 2;
const CHECKPOINT_VERSION: u16 = 1;
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMPORT_STATE_BYTES: usize = 1024;
const IMPORT_MAGIC: &[u8; 8] = b"WSIMPORT";
const IMPORT_VERSION: u16 = 1;

/// A domain-separated BLAKE3 digest used by the transfer wire contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DigestV1([u8; 32]);

impl DigestV1 {
    /// Hashes exact bytes without decoding or normalizing them.
    #[must_use]
    pub fn hash(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Constructs a digest from its exact 32-byte representation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransferError> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| TransferError::InvalidDigestLength {
                actual: bytes.len(),
            })?;
        Ok(Self(bytes))
    }

    /// Returns the exact digest bytes.
    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Display for DigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The two reviewed logical storage-profile identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleProfileV1 {
    /// The release-bundled `SQLite` source profile.
    SqliteBundled,
    /// The `PostgreSQL` 17 primary target profile.
    PostgresPrimary17,
}

/// The source-evidence scope carried by a transfer bundle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferScopeV1 {
    /// A complete deployment transfer with pack and resource admission data.
    WholeDeployment,
    /// Exact canonical Room export evidence only. Pack/resource admission is
    /// intentionally absent and must remain a separate acceptance decision.
    CanonicalExport,
}

/// One immutable entry in the ordered logical migration history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationIdentityV1 {
    version: u32,
    migration_id: String,
    checksum: DigestV1,
}

impl MigrationIdentityV1 {
    /// Creates one validated, one-based migration identity.
    pub fn new(
        version: u32,
        migration_id: impl Into<String>,
        checksum: DigestV1,
    ) -> Result<Self, TransferError> {
        if version == 0 {
            return Err(TransferError::InvalidValue {
                what: "migration version",
            });
        }
        let migration_id = migration_id.into();
        validate_text(&migration_id)?;
        Ok(Self {
            version,
            migration_id,
            checksum,
        })
    }

    /// Returns the one-based logical migration version.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Returns the immutable logical migration identifier.
    #[must_use]
    pub fn migration_id(&self) -> &str {
        &self.migration_id
    }

    /// Returns the exact backend migration-body checksum.
    #[must_use]
    pub const fn checksum(&self) -> DigestV1 {
        self.checksum
    }
}

/// The shared schema contract and ordered, forward-only migration prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaMigrationContractV1 {
    logical_history_id: String,
    schema_contract_fingerprint: DigestV1,
    migrations: Vec<MigrationIdentityV1>,
}

impl SchemaMigrationContractV1 {
    /// Creates a complete schema/migration contract.
    pub fn new(
        logical_history_id: impl Into<String>,
        schema_contract_fingerprint: DigestV1,
        migrations: Vec<MigrationIdentityV1>,
    ) -> Result<Self, TransferError> {
        let logical_history_id = logical_history_id.into();
        validate_text(&logical_history_id)?;
        if migrations.is_empty() {
            return Err(TransferError::InvalidValue {
                what: "empty migration history",
            });
        }
        let mut expected_version = 1_u32;
        let mut seen_ids = Vec::with_capacity(migrations.len());
        for migration in &migrations {
            if migration.version != expected_version {
                return Err(TransferError::NonDeterministicOrder {
                    what: "migration versions",
                });
            }
            if seen_ids.iter().any(|id| id == &migration.migration_id) {
                return Err(TransferError::DuplicateIdentity { what: "migration" });
            }
            seen_ids.push(migration.migration_id.as_str());
            expected_version =
                expected_version
                    .checked_add(1)
                    .ok_or(TransferError::BoundExceeded {
                        what: "migration history",
                    })?;
        }
        Ok(Self {
            logical_history_id,
            schema_contract_fingerprint,
            migrations,
        })
    }

    /// Returns the shared logical migration history identity.
    #[must_use]
    pub fn logical_history_id(&self) -> &str {
        &self.logical_history_id
    }

    /// Returns the canonical schema-contract fingerprint.
    #[must_use]
    pub const fn schema_contract_fingerprint(&self) -> DigestV1 {
        self.schema_contract_fingerprint
    }

    /// Returns the exact ordered migration prefix.
    #[must_use]
    pub fn migrations(&self) -> &[MigrationIdentityV1] {
        &self.migrations
    }

    /// Returns a deterministic digest of the complete contract.
    pub fn digest(&self) -> Result<DigestV1, TransferError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"worldstream/schema-migration-contract/v1");
        write_string(&mut bytes, &self.logical_history_id)?;
        bytes.extend_from_slice(&self.schema_contract_fingerprint.as_bytes());
        for migration in &self.migrations {
            write_u32(&mut bytes, migration.version);
            write_string(&mut bytes, &migration.migration_id)?;
            bytes.extend_from_slice(&migration.checksum.as_bytes());
        }
        Ok(DigestV1::hash(&bytes))
    }
}

/// Exact backend/build/schema identity supplied by a source or destination
/// adapter. This is evidence metadata, not a connection or a live-backend
/// probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendFingerprintV1 {
    profile: BundleProfileV1,
    engine_identity: String,
    schema: SchemaMigrationContractV1,
}

impl BackendFingerprintV1 {
    /// Creates a validated backend fingerprint.
    pub fn new(
        profile: BundleProfileV1,
        engine_identity: impl Into<String>,
        schema: SchemaMigrationContractV1,
    ) -> Result<Self, TransferError> {
        let engine_identity = engine_identity.into();
        validate_text(&engine_identity)?;
        Ok(Self {
            profile,
            engine_identity,
            schema,
        })
    }

    /// Returns the selected backend profile.
    #[must_use]
    pub const fn profile(&self) -> BundleProfileV1 {
        self.profile
    }

    /// Returns the redacted exact engine/build identity.
    #[must_use]
    pub fn engine_identity(&self) -> &str {
        &self.engine_identity
    }

    /// Returns the exact schema and migration contract.
    #[must_use]
    pub fn schema(&self) -> &SchemaMigrationContractV1 {
        &self.schema
    }

    /// Returns a deterministic digest over profile, engine, schema, and
    /// migration identities.
    pub fn digest(&self) -> Result<DigestV1, TransferError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"worldstream/backend-fingerprint/v1");
        bytes.push(self.profile.tag());
        write_string(&mut bytes, &self.engine_identity)?;
        bytes.extend_from_slice(&self.schema.digest()?.as_bytes());
        Ok(DigestV1::hash(&bytes))
    }
}

/// Compatibility alias matching the storage/backup terminology.
pub type MigrationContractV1 = SchemaMigrationContractV1;

impl BundleProfileV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::SqliteBundled => 1,
            Self::PostgresPrimary17 => 2,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        match tag {
            1 => Ok(Self::SqliteBundled),
            2 => Ok(Self::PostgresPrimary17),
            other => Err(TransferError::UnknownProfile(other)),
        }
    }
}

/// Canonical records that must retain their exact serialized bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CanonicalRecordKindV1 {
    DeploymentLineage,
    StorageEpoch,
    SchemaManifest,
    Principal,
    Capability,
    Revocation,
    RoomGenesis,
    RoomTransition,
    RoomHead,
    CoreMaterialization,
    ActivityMaterialization,
    Membership,
    Timer,
    ObservationFrame,
    ObservationCursor,
    OperationIdentity,
    SemanticReceipt,
    ActivationIntent,
    ActivationOperationReceipt,
    InvocationContext,
    InvocationContextTombstone,
    ArtifactMetadata,
    ArtifactBytes,
    IntegrityIncident,
    NativeOperationalRow,
}

impl CanonicalRecordKindV1 {
    const fn tag(self) -> u8 {
        self as u8 + 1
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        let kind = match tag {
            1 => Self::DeploymentLineage,
            2 => Self::StorageEpoch,
            3 => Self::SchemaManifest,
            4 => Self::Principal,
            5 => Self::Capability,
            6 => Self::Revocation,
            7 => Self::RoomGenesis,
            8 => Self::RoomTransition,
            9 => Self::RoomHead,
            10 => Self::CoreMaterialization,
            11 => Self::ActivityMaterialization,
            12 => Self::Membership,
            13 => Self::Timer,
            14 => Self::ObservationFrame,
            15 => Self::ObservationCursor,
            16 => Self::OperationIdentity,
            17 => Self::SemanticReceipt,
            18 => Self::ActivationIntent,
            19 => Self::ActivationOperationReceipt,
            20 => Self::InvocationContext,
            21 => Self::InvocationContextTombstone,
            22 => Self::ArtifactMetadata,
            23 => Self::ArtifactBytes,
            24 => Self::IntegrityIncident,
            25 => Self::NativeOperationalRow,
            other => return Err(TransferError::UnknownRecordKind(other)),
        };
        Ok(kind)
    }
}

/// Derived materializations that can be rebuilt after verified import.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DerivedRecordKindV1 {
    PairedSnapshot,
    SearchIndex,
    DeliveryAttempt,
    Telemetry,
}

impl DerivedRecordKindV1 {
    const fn tag(self) -> u8 {
        self as u8 + 1
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        match tag {
            1 => Ok(Self::PairedSnapshot),
            2 => Ok(Self::SearchIndex),
            3 => Ok(Self::DeliveryAttempt),
            4 => Ok(Self::Telemetry),
            other => Err(TransferError::UnknownRecordKind(other)),
        }
    }
}

/// The class tag prevents derived/session state from being mistaken for truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKindV1 {
    /// Canonical source-of-truth record.
    Canonical(CanonicalRecordKindV1),
    /// Rebuildable operational materialization.
    Derived(DerivedRecordKindV1),
}

impl RecordKindV1 {
    const fn class_tag(self) -> u8 {
        match self {
            Self::Canonical(_) => 1,
            Self::Derived(_) => 2,
        }
    }

    const fn kind_tag(self) -> u8 {
        match self {
            Self::Canonical(kind) => kind.tag(),
            Self::Derived(kind) => kind.tag(),
        }
    }
}

/// The explicit policy for session state, which is never imported as truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatePolicyV1 {
    /// Sessions, runner presence, mailboxes, and temporary delivery state are
    /// invalidated and rebuilt after target readiness.
    InvalidateAndRebuild,
}

impl SessionStatePolicyV1 {
    const fn tag() -> u8 {
        1
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        match tag {
            1 => Ok(Self::InvalidateAndRebuild),
            other => Err(TransferError::UnknownSessionPolicy(other)),
        }
    }
}

/// A pack revision identity bound to the transfer manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackIdentityV1 {
    pack_id: String,
    revision: String,
    digest: DigestV1,
}

impl PackIdentityV1 {
    /// Creates a validated exact pack identity.
    pub fn new(
        pack_id: impl Into<String>,
        revision: impl Into<String>,
        digest: DigestV1,
    ) -> Result<Self, TransferError> {
        let pack_id = pack_id.into();
        let revision = revision.into();
        validate_text(&pack_id)?;
        validate_text(&revision)?;
        Ok(Self {
            pack_id,
            revision,
            digest,
        })
    }

    /// Returns the pack identifier.
    #[must_use]
    pub fn pack_id(&self) -> &str {
        &self.pack_id
    }

    /// Returns the exact retained revision identity.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Returns the exact pack digest.
    #[must_use]
    pub const fn digest(&self) -> DigestV1 {
        self.digest
    }

    /// Verifies an exact retained pack byte stream against this identity.
    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<(), TransferError> {
        let actual = DigestV1::hash(bytes);
        if actual != self.digest {
            return Err(TransferError::HashMismatch {
                what: "pack",
                expected: self.digest,
                actual,
            });
        }
        Ok(())
    }
}

/// A resource identity recorded independently from its provider representation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResourceKindV1 {
    Artifact,
    Codec,
    Schema,
}

impl ResourceKindV1 {
    const fn tag(self) -> u8 {
        self as u8 + 1
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        match tag {
            1 => Ok(Self::Artifact),
            2 => Ok(Self::Codec),
            3 => Ok(Self::Schema),
            other => Err(TransferError::UnknownResourceKind(other)),
        }
    }
}

/// A resource's exact identity, length, and content digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceIdentityV1 {
    kind: ResourceKindV1,
    identity: String,
    size_bytes: u64,
    digest: DigestV1,
}

/// One source-authenticated deployment resource together with its exact
/// retained bytes.
///
/// Keeping the identity and payload in one validated value prevents transfer
/// adapters from treating metadata presence as evidence that the referenced
/// artifact, codec, or schema bytes were actually retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePayloadV1 {
    identity: ResourceIdentityV1,
    bytes: Vec<u8>,
}

/// The complete, source-authoritative deployment identity used by a whole
/// deployment transfer.
///
/// The presence of this value is the completeness witness: an empty resource
/// set is valid and distinct from absent metadata.  Identities are sorted and
/// hashed from their exact canonical encoding, so callers cannot replace the
/// source set with a partial, duplicated, or differently ordered assertion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentIdentityV1 {
    packs: Vec<PackIdentityV1>,
    resources: Vec<ResourceIdentityV1>,
    digest: DigestV1,
}

impl DeploymentIdentityV1 {
    /// Creates a complete deployment identity, including an explicit empty
    /// resource set when `resources` is empty.
    pub fn new(
        mut packs: Vec<PackIdentityV1>,
        mut resources: Vec<ResourceIdentityV1>,
    ) -> Result<Self, TransferError> {
        if packs.is_empty() {
            return Err(TransferError::InvalidValue {
                what: "empty pack identity set",
            });
        }
        packs.sort_by(|left, right| {
            (left.pack_id(), left.revision()).cmp(&(right.pack_id(), right.revision()))
        });
        if packs.windows(2).any(|pair| {
            (pair[0].pack_id(), pair[0].revision()) == (pair[1].pack_id(), pair[1].revision())
        }) {
            return Err(TransferError::DuplicateIdentity { what: "pack" });
        }
        resources.sort_by(|left, right| {
            (left.kind.tag(), left.identity()).cmp(&(right.kind.tag(), right.identity()))
        });
        let mut resource_identities = BTreeSet::new();
        if resources
            .iter()
            .any(|resource| !resource_identities.insert(resource.identity()))
        {
            return Err(TransferError::DuplicateIdentity { what: "resource" });
        }
        let mut identity = Self {
            packs,
            resources,
            digest: DigestV1::hash(&[]),
        };
        identity.digest = DigestV1::hash(&identity.canonical_bytes()?);
        Ok(identity)
    }

    /// Decodes the exact source-authenticated identity encoding.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, TransferError> {
        let mut reader = Reader::new(bytes);
        if reader.read_exact(8)? != b"WSDEPID1" {
            return Err(TransferError::InvalidMagic);
        }
        if reader.read_u16()? != 1 {
            return Err(TransferError::InvalidValue {
                what: "deployment identity version",
            });
        }
        let pack_count = reader.read_count()?;
        let mut packs = Vec::with_capacity(pack_count);
        for _ in 0..pack_count {
            packs.push(PackIdentityV1::new(
                reader.read_string()?,
                reader.read_string()?,
                reader.read_digest()?,
            )?);
        }
        let resource_count = reader.read_count()?;
        let mut resources = Vec::with_capacity(resource_count);
        for _ in 0..resource_count {
            resources.push(ResourceIdentityV1 {
                kind: ResourceKindV1::from_tag(reader.read_u8()?)?,
                identity: reader.read_string()?,
                size_bytes: reader.read_u64()?,
                digest: reader.read_digest()?,
            });
        }
        if !reader.is_done() {
            return Err(TransferError::TrailingBytes {
                count: reader.remaining(),
            });
        }
        let identity = Self::new(packs, resources)?;
        if identity.canonical_bytes()? != bytes {
            return Err(TransferError::InvalidValue {
                what: "non-canonical deployment identity",
            });
        }
        Ok(identity)
    }

    /// Returns all exact pack identities in canonical order.
    #[must_use]
    pub fn packs(&self) -> &[PackIdentityV1] {
        &self.packs
    }

    /// Returns all exact resource identities in canonical order. An empty
    /// slice is authoritative because this identity object is present.
    #[must_use]
    pub fn resources(&self) -> &[ResourceIdentityV1] {
        &self.resources
    }

    /// Returns the content-derived identity-set digest.
    #[must_use]
    pub const fn digest(&self) -> DigestV1 {
        self.digest
    }

    /// Returns the exact canonical identity bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransferError> {
        let mut output = Vec::new();
        output.extend_from_slice(b"WSDEPID1");
        write_u16(&mut output, 1);
        write_count(&mut output, self.packs.len())?;
        for pack in &self.packs {
            write_string(&mut output, pack.pack_id())?;
            write_string(&mut output, pack.revision())?;
            output.extend_from_slice(&pack.digest().as_bytes());
        }
        write_count(&mut output, self.resources.len())?;
        for resource in &self.resources {
            output.push(resource.kind.tag());
            write_string(&mut output, resource.identity())?;
            write_u64(&mut output, resource.size_bytes());
            output.extend_from_slice(&resource.digest().as_bytes());
        }
        Ok(output)
    }
}

impl ResourceIdentityV1 {
    /// Computes an identity from the exact resource bytes.
    pub fn from_bytes(
        kind: ResourceKindV1,
        identity: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, TransferError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "resource bytes",
            });
        }
        let identity = identity.into();
        validate_text(&identity)?;
        Ok(Self {
            kind,
            identity,
            size_bytes: u64::try_from(bytes.len()).map_err(|_| TransferError::BoundExceeded {
                what: "resource length",
            })?,
            digest: DigestV1::hash(bytes),
        })
    }

    /// Reconstructs an identity from trusted persisted size/digest metadata.
    /// The source adapter must validate the persisted row set and its
    /// enclosing canonical identity digest before exposing this value.
    pub fn from_persisted_parts(
        kind: ResourceKindV1,
        identity: impl Into<String>,
        size_bytes: u64,
        digest: DigestV1,
    ) -> Result<Self, TransferError> {
        if size_bytes > MAX_RECORD_BYTES as u64 {
            return Err(TransferError::BoundExceeded {
                what: "resource bytes",
            });
        }
        let identity = identity.into();
        validate_text(&identity)?;
        Ok(Self {
            kind,
            identity,
            size_bytes,
            digest,
        })
    }

    /// Returns the resource category.
    #[must_use]
    pub const fn kind(&self) -> ResourceKindV1 {
        self.kind
    }

    /// Returns the stable resource identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Returns the exact byte length.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Returns the exact resource digest.
    #[must_use]
    pub const fn digest(&self) -> DigestV1 {
        self.digest
    }

    /// Verifies exact resource length and bytes against this identity.
    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<(), TransferError> {
        let actual_size = u64::try_from(bytes.len()).map_err(|_| TransferError::BoundExceeded {
            what: "resource length",
        })?;
        if actual_size != self.size_bytes {
            return Err(TransferError::ResourceSizeMismatch {
                expected: self.size_bytes,
                actual: actual_size,
            });
        }
        let actual = DigestV1::hash(bytes);
        if actual != self.digest {
            return Err(TransferError::HashMismatch {
                what: "resource",
                expected: self.digest,
                actual,
            });
        }
        Ok(())
    }
}

impl ResourcePayloadV1 {
    /// Builds a payload from exact bytes and derives its content identity.
    pub fn from_bytes(
        kind: ResourceKindV1,
        identity: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, TransferError> {
        let identity = ResourceIdentityV1::from_bytes(kind, identity, bytes)?;
        Ok(Self {
            identity,
            bytes: bytes.to_vec(),
        })
    }

    /// Pairs a persisted identity with exact bytes, rejecting any size or
    /// digest mismatch before the value reaches a provider adapter.
    pub fn from_identity(
        identity: ResourceIdentityV1,
        bytes: &[u8],
    ) -> Result<Self, TransferError> {
        identity.verify_bytes(bytes)?;
        Ok(Self {
            identity,
            bytes: bytes.to_vec(),
        })
    }

    /// Returns the exact resource identity and content digest.
    #[must_use]
    pub const fn identity(&self) -> &ResourceIdentityV1 {
        &self.identity
    }

    /// Returns the exact retained bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// One byte-preserved logical record in a transfer bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalRecordV1 {
    ordinal: u64,
    kind: RecordKindV1,
    identity: String,
    bytes: Vec<u8>,
    digest: DigestV1,
}

impl LogicalRecordV1 {
    /// Creates a canonical record and hashes its exact bytes.
    pub fn canonical(
        ordinal: u64,
        kind: CanonicalRecordKindV1,
        identity: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, TransferError> {
        Self::from_parts(
            ordinal,
            RecordKindV1::Canonical(kind),
            identity,
            bytes,
            DigestV1::hash(bytes),
        )
    }

    /// Creates a derived record and hashes its exact bytes.
    pub fn derived(
        ordinal: u64,
        kind: DerivedRecordKindV1,
        identity: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, TransferError> {
        Self::from_parts(
            ordinal,
            RecordKindV1::Derived(kind),
            identity,
            bytes,
            DigestV1::hash(bytes),
        )
    }

    /// Validates provider-supplied fields without decoding or rewriting bytes.
    pub fn from_parts(
        ordinal: u64,
        kind: RecordKindV1,
        identity: impl Into<String>,
        bytes: &[u8],
        digest: DigestV1,
    ) -> Result<Self, TransferError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "record bytes",
            });
        }
        let identity = identity.into();
        validate_text(&identity)?;
        let actual = DigestV1::hash(bytes);
        if actual != digest {
            return Err(TransferError::HashMismatch {
                what: "record",
                expected: digest,
                actual,
            });
        }
        Ok(Self {
            ordinal,
            kind,
            identity,
            bytes: bytes.to_vec(),
            digest,
        })
    }

    /// Decodes the two wire tags and rejects unknown record kinds/classes.
    pub fn decode_kind(class_tag: u8, kind_tag: u8) -> Result<RecordKindV1, TransferError> {
        match class_tag {
            1 => Ok(RecordKindV1::Canonical(CanonicalRecordKindV1::from_tag(
                kind_tag,
            )?)),
            2 => Ok(RecordKindV1::Derived(DerivedRecordKindV1::from_tag(
                kind_tag,
            )?)),
            other => Err(TransferError::UnknownRecordClass(other)),
        }
    }

    /// Returns the source ordinal.
    #[must_use]
    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// Returns the canonical/derived class and kind.
    #[must_use]
    pub const fn kind(&self) -> RecordKindV1 {
        self.kind
    }

    /// Returns the stable logical identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Returns the exact original serialized bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the exact record digest.
    #[must_use]
    pub const fn digest(&self) -> DigestV1 {
        self.digest
    }
}

/// Deterministic parity for one Room's canonical serialized records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomParityV1 {
    room_id: String,
    record_count: usize,
    bytes_digest: DigestV1,
}

impl RoomParityV1 {
    /// Returns the stable Room identity.
    #[must_use]
    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    /// Returns the canonical record count for the Room.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }

    /// Returns the digest of the Room's exact canonical record wire entries.
    #[must_use]
    pub const fn bytes_digest(&self) -> DigestV1 {
        self.bytes_digest
    }
}

/// Deployment-wide and per-Room canonical-byte parity evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordParityV1 {
    record_count: usize,
    canonical_record_count: usize,
    derived_record_count: usize,
    total_digest: DigestV1,
    canonical_digest: DigestV1,
    deployment_record_count: usize,
    deployment_digest: DigestV1,
    rooms: Vec<RoomParityV1>,
}

impl RecordParityV1 {
    /// Computes deterministic parity from the ordered logical records.
    pub fn from_records(records: &[LogicalRecordV1]) -> Result<Self, TransferError> {
        let mut total_bytes = Vec::new();
        let mut canonical_bytes = Vec::new();
        let mut deployment_bytes = Vec::new();
        let mut rooms = BTreeMap::<String, Vec<&LogicalRecordV1>>::new();
        let mut canonical_record_count = 0usize;
        let mut deployment_record_count = 0usize;
        let mut derived_record_count = 0usize;

        for record in records {
            write_record(&mut total_bytes, record)?;
            match record.kind {
                RecordKindV1::Canonical(_) => {
                    canonical_record_count = canonical_record_count.checked_add(1).ok_or(
                        TransferError::BoundExceeded {
                            what: "canonical record count",
                        },
                    )?;
                    write_record(&mut canonical_bytes, record)?;
                    if let Some(room_id) = room_id_from_identity(&record.identity)? {
                        rooms.entry(room_id).or_default().push(record);
                    } else {
                        deployment_record_count = deployment_record_count.checked_add(1).ok_or(
                            TransferError::BoundExceeded {
                                what: "deployment record count",
                            },
                        )?;
                        write_record(&mut deployment_bytes, record)?;
                    }
                }
                RecordKindV1::Derived(_) => {
                    derived_record_count = derived_record_count.checked_add(1).ok_or(
                        TransferError::BoundExceeded {
                            what: "derived record count",
                        },
                    )?;
                }
            }
        }

        let rooms = rooms
            .into_iter()
            .map(|(room_id, room_records)| {
                let mut bytes = Vec::new();
                let record_count = room_records.len();
                for record in &room_records {
                    write_record(&mut bytes, record)?;
                }
                Ok(RoomParityV1 {
                    room_id,
                    record_count,
                    bytes_digest: DigestV1::hash(&bytes),
                })
            })
            .collect::<Result<Vec<_>, TransferError>>()?;

        Ok(Self {
            record_count: records.len(),
            canonical_record_count,
            derived_record_count,
            total_digest: DigestV1::hash(&total_bytes),
            canonical_digest: DigestV1::hash(&canonical_bytes),
            deployment_record_count,
            deployment_digest: DigestV1::hash(&deployment_bytes),
            rooms,
        })
    }

    /// Returns the total number of records.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }

    /// Returns the number of canonical records.
    #[must_use]
    pub const fn canonical_record_count(&self) -> usize {
        self.canonical_record_count
    }

    /// Returns the number of derived records.
    #[must_use]
    pub const fn derived_record_count(&self) -> usize {
        self.derived_record_count
    }

    /// Returns the digest over every exact ordered record entry.
    #[must_use]
    pub const fn total_digest(&self) -> DigestV1 {
        self.total_digest
    }

    /// Returns the digest over every exact ordered canonical record entry.
    #[must_use]
    pub const fn canonical_digest(&self) -> DigestV1 {
        self.canonical_digest
    }

    /// Returns the count of deployment-scoped canonical records.
    #[must_use]
    pub const fn deployment_record_count(&self) -> usize {
        self.deployment_record_count
    }

    /// Returns the digest of deployment-scoped canonical records.
    #[must_use]
    pub const fn deployment_digest(&self) -> DigestV1 {
        self.deployment_digest
    }

    /// Returns sorted per-Room canonical parity.
    #[must_use]
    pub fn rooms(&self) -> &[RoomParityV1] {
        &self.rooms
    }
}

/// A validated, deterministic, versioned offline transfer bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferBundleV1 {
    bundle_id: String,
    lineage_id: String,
    source_epoch: u64,
    source_profile: BundleProfileV1,
    target_profile: BundleProfileV1,
    source_backend: Option<BackendFingerprintV1>,
    target_backend: BackendFingerprintV1,
    pack: Option<PackIdentityV1>,
    resources: Vec<ResourceIdentityV1>,
    scope: TransferScopeV1,
    isolated_rooms: Vec<String>,
    session_state: SessionStatePolicyV1,
    records: Vec<LogicalRecordV1>,
}

impl TransferBundleV1 {
    /// Builds the only supported direction: bundled `SQLite` to `PostgreSQL` 17.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        source_profile: BundleProfileV1,
        target_profile: BundleProfileV1,
        pack: PackIdentityV1,
        resources: Vec<ResourceIdentityV1>,
        session_state: SessionStatePolicyV1,
        records: Vec<LogicalRecordV1>,
    ) -> Result<Self, TransferError> {
        let source_backend = default_backend_fingerprint(source_profile)?;
        let target_backend = default_backend_fingerprint(target_profile)?;
        Self::new_with_backend_fingerprints(
            bundle_id,
            lineage_id,
            source_epoch,
            source_backend,
            target_backend,
            pack,
            resources,
            session_state,
            records,
        )
    }

    /// Builds a transfer bundle bound to exact source and target backend
    /// fingerprints supplied by their adapters.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn new_with_backend_fingerprints(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
        pack: PackIdentityV1,
        resources: Vec<ResourceIdentityV1>,
        session_state: SessionStatePolicyV1,
        records: Vec<LogicalRecordV1>,
    ) -> Result<Self, TransferError> {
        let bundle_id = bundle_id.into();
        let lineage_id = lineage_id.into();
        validate_text(&bundle_id)?;
        validate_text(&lineage_id)?;
        if source_epoch == 0 {
            return Err(TransferError::InvalidValue {
                what: "source epoch",
            });
        }
        let source_profile = source_backend.profile();
        let target_profile = target_backend.profile();
        if source_profile != BundleProfileV1::SqliteBundled
            || target_profile != BundleProfileV1::PostgresPrimary17
        {
            return Err(TransferError::UnsupportedDirection);
        }
        if source_backend.schema.logical_history_id != target_backend.schema.logical_history_id
            || source_backend.schema.schema_contract_fingerprint
                != target_backend.schema.schema_contract_fingerprint
            || source_backend.schema.migrations.len() != target_backend.schema.migrations.len()
            || source_backend
                .schema
                .migrations
                .iter()
                .zip(&target_backend.schema.migrations)
                .any(|(source, target)| {
                    source.version != target.version || source.migration_id != target.migration_id
                })
        {
            return Err(TransferError::InvalidValue {
                what: "source/target schema contract",
            });
        }
        if resources.windows(2).any(|pair| {
            (pair[0].kind.tag(), pair[0].identity.as_str())
                >= (pair[1].kind.tag(), pair[1].identity.as_str())
        }) {
            return Err(TransferError::NonDeterministicOrder { what: "resources" });
        }
        if records.len() > MAX_RECORDS {
            return Err(TransferError::BoundExceeded { what: "records" });
        }
        let mut total_bytes = 0usize;
        let mut seen = Vec::with_capacity(records.len());
        let mut previous_class = 0u8;
        let mut previous_order = None;
        for (index, record) in records.iter().enumerate() {
            if record.ordinal
                != u64::try_from(index).map_err(|_| TransferError::BoundExceeded {
                    what: "record ordinal",
                })?
            {
                return Err(TransferError::NonDeterministicOrder {
                    what: "record ordinals",
                });
            }
            if record.kind.class_tag() < previous_class {
                return Err(TransferError::NonDeterministicOrder {
                    what: "canonical/derived sections",
                });
            }
            previous_class = record.kind.class_tag();
            let order = (
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            );
            if previous_order.is_some_and(|previous| previous > order) {
                return Err(TransferError::NonDeterministicOrder { what: "records" });
            }
            previous_order = Some(order);
            if seen.contains(&(
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            )) {
                return Err(TransferError::DuplicateIdentity { what: "record" });
            }
            seen.push((
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            ));
            total_bytes = total_bytes.checked_add(record.bytes.len()).ok_or(
                TransferError::BoundExceeded {
                    what: "bundle bytes",
                },
            )?;
        }
        if total_bytes > MAX_BUNDLE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        Ok(Self {
            bundle_id,
            lineage_id,
            source_epoch,
            source_profile,
            target_profile,
            source_backend: Some(source_backend),
            target_backend,
            pack: Some(pack),
            resources,
            scope: TransferScopeV1::WholeDeployment,
            isolated_rooms: Vec::new(),
            session_state,
            records,
        })
    }

    /// Builds a transfer bundle from exact canonical `SQLite` export evidence.
    ///
    /// This constructor deliberately carries no source backend fingerprint,
    /// pack identity, or resource identities. It is a mechanics probe for
    /// canonical Room publication only; callers must report whole-deployment
    /// pack/resource acceptance separately. Deployment lineage and epoch must
    /// already be present as exact canonical records in `records`.
    pub fn from_canonical_export(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        target_backend: BackendFingerprintV1,
        isolated_rooms: Vec<String>,
        records: Vec<LogicalRecordV1>,
    ) -> Result<Self, TransferError> {
        let bundle_id = bundle_id.into();
        let lineage_id = lineage_id.into();
        validate_text(&bundle_id)?;
        validate_text(&lineage_id)?;
        if source_epoch == 0 {
            return Err(TransferError::InvalidValue {
                what: "source epoch",
            });
        }
        if target_backend.profile() != BundleProfileV1::PostgresPrimary17 {
            return Err(TransferError::UnsupportedDirection);
        }
        let mut records = records;
        records.sort_by(|left, right| {
            (
                left.kind.class_tag(),
                left.kind.kind_tag(),
                left.identity.as_str(),
            )
                .cmp(&(
                    right.kind.class_tag(),
                    right.kind.kind_tag(),
                    right.identity.as_str(),
                ))
        });
        for (ordinal, record) in records.iter_mut().enumerate() {
            record.ordinal = u64::try_from(ordinal).map_err(|_| TransferError::BoundExceeded {
                what: "record ordinal",
            })?;
        }
        let bundle = Self {
            bundle_id,
            lineage_id,
            source_epoch,
            source_profile: BundleProfileV1::SqliteBundled,
            target_profile: BundleProfileV1::PostgresPrimary17,
            source_backend: None,
            target_backend,
            pack: None,
            resources: Vec::new(),
            scope: TransferScopeV1::CanonicalExport,
            isolated_rooms,
            session_state: SessionStatePolicyV1::InvalidateAndRebuild,
            records,
        };
        bundle.validate_records()?;
        Ok(bundle)
    }

    #[allow(clippy::too_many_lines)]
    fn validate_records(&self) -> Result<(), TransferError> {
        if self.records.len() > MAX_RECORDS {
            return Err(TransferError::BoundExceeded { what: "records" });
        }
        let mut previous_class = 0_u8;
        let mut previous_order = None;
        let mut seen = Vec::with_capacity(self.records.len());
        let mut total_bytes = 0usize;
        let mut deployment_kinds = BTreeSet::new();
        let mut room_kinds = BTreeMap::<String, BTreeSet<CanonicalRecordKindV1>>::new();
        for (index, record) in self.records.iter().enumerate() {
            let RecordKindV1::Canonical(kind) = record.kind else {
                return Err(TransferError::InvalidValue {
                    what: "canonical export derived record",
                });
            };
            if let Some(room_id) = room_id_from_identity(&record.identity)? {
                room_kinds.entry(room_id).or_default().insert(kind);
            } else {
                deployment_kinds.insert(kind);
            }
            if record.ordinal
                != u64::try_from(index).map_err(|_| TransferError::BoundExceeded {
                    what: "record ordinal",
                })?
            {
                return Err(TransferError::NonDeterministicOrder {
                    what: "record ordinals",
                });
            }
            if record.kind.class_tag() < previous_class {
                return Err(TransferError::NonDeterministicOrder {
                    what: "canonical/derived sections",
                });
            }
            previous_class = record.kind.class_tag();
            let order = (
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            );
            if previous_order.is_some_and(|previous| previous > order) {
                return Err(TransferError::NonDeterministicOrder { what: "records" });
            }
            previous_order = Some(order);
            if seen.contains(&(
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            )) {
                return Err(TransferError::DuplicateIdentity { what: "record" });
            }
            seen.push((
                record.kind.class_tag(),
                record.kind.kind_tag(),
                record.identity.as_str(),
            ));
            total_bytes = total_bytes.checked_add(record.bytes.len()).ok_or(
                TransferError::BoundExceeded {
                    what: "bundle bytes",
                },
            )?;
        }
        if total_bytes > MAX_BUNDLE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        let lineage = self
            .records
            .iter()
            .find(|record| {
                record.kind == RecordKindV1::Canonical(CanonicalRecordKindV1::DeploymentLineage)
            })
            .ok_or(TransferError::InvalidValue {
                what: "canonical export lineage record",
            })?;
        if lineage.bytes != self.lineage_id.as_bytes() {
            return Err(TransferError::HashMismatch {
                what: "canonical export lineage",
                expected: DigestV1::hash(self.lineage_id.as_bytes()),
                actual: lineage.digest,
            });
        }
        let epoch = self
            .records
            .iter()
            .find(|record| {
                record.kind == RecordKindV1::Canonical(CanonicalRecordKindV1::StorageEpoch)
            })
            .ok_or(TransferError::InvalidValue {
                what: "canonical export epoch record",
            })?;
        let expected_epoch = self.source_epoch.to_string();
        if epoch.bytes != expected_epoch.as_bytes() {
            return Err(TransferError::HashMismatch {
                what: "canonical export epoch",
                expected: DigestV1::hash(expected_epoch.as_bytes()),
                actual: epoch.digest,
            });
        }
        if deployment_kinds
            != BTreeSet::from([
                CanonicalRecordKindV1::DeploymentLineage,
                CanonicalRecordKindV1::StorageEpoch,
            ])
        {
            return Err(TransferError::InvalidValue {
                what: "canonical export deployment records",
            });
        }
        for kinds in room_kinds.values() {
            for required in [
                CanonicalRecordKindV1::RoomGenesis,
                CanonicalRecordKindV1::RoomHead,
                CanonicalRecordKindV1::CoreMaterialization,
                CanonicalRecordKindV1::ActivityMaterialization,
            ] {
                if !kinds.contains(&required) {
                    return Err(TransferError::InvalidValue {
                        what: "canonical export Room records",
                    });
                }
            }
        }
        for isolated_room in &self.isolated_rooms {
            if room_kinds.contains_key(isolated_room) {
                return Err(TransferError::InvalidValue {
                    what: "canonical export isolated Room promotion",
                });
            }
        }
        for room_id in &self.isolated_rooms {
            validate_text(room_id)?;
        }
        Ok(())
    }

    /// Decodes and verifies a complete bundle; partial/trailing input fails.
    #[allow(clippy::too_many_lines)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransferError> {
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        let mut reader = Reader::new(bytes);
        if reader.read_exact(8)? != BUNDLE_MAGIC {
            return Err(TransferError::InvalidMagic);
        }
        let version = reader.read_u16()?;
        if version != BUNDLE_VERSION && version != LEGACY_BUNDLE_VERSION {
            return Err(TransferError::UnsupportedBundleVersion(version));
        }
        let bundle_id = reader.read_string()?;
        let lineage_id = reader.read_string()?;
        let source_epoch = reader.read_u64()?;
        let source_profile = BundleProfileV1::from_tag(reader.read_u8()?)?;
        let target_profile = BundleProfileV1::from_tag(reader.read_u8()?)?;
        let (scope, source_backend, isolated_rooms, pack, resources, target_backend) =
            if version == LEGACY_BUNDLE_VERSION {
                let source_backend = read_backend_fingerprint(&mut reader)?;
                let target_backend = read_backend_fingerprint(&mut reader)?;
                let pack = PackIdentityV1::new(
                    reader.read_string()?,
                    reader.read_string()?,
                    reader.read_digest()?,
                )?;
                let resource_count = reader.read_count()?;
                let mut resources = Vec::with_capacity(resource_count);
                for _ in 0..resource_count {
                    let kind = ResourceKindV1::from_tag(reader.read_u8()?)?;
                    let identity = reader.read_string()?;
                    let size_bytes = reader.read_u64()?;
                    let digest = reader.read_digest()?;
                    resources.push(ResourceIdentityV1 {
                        kind,
                        identity,
                        size_bytes,
                        digest,
                    });
                }
                (
                    TransferScopeV1::WholeDeployment,
                    Some(source_backend),
                    Vec::new(),
                    Some(pack),
                    resources,
                    target_backend,
                )
            } else {
                let scope = match reader.read_u8()? {
                    1 => TransferScopeV1::WholeDeployment,
                    2 => TransferScopeV1::CanonicalExport,
                    _ => {
                        return Err(TransferError::InvalidValue {
                            what: "transfer scope",
                        });
                    }
                };
                let source_backend = match reader.read_u8()? {
                    0 => None,
                    1 => Some(read_backend_fingerprint(&mut reader)?),
                    _ => {
                        return Err(TransferError::InvalidValue {
                            what: "source backend presence",
                        });
                    }
                };
                let isolated_count = reader.read_count()?;
                let mut isolated_rooms = Vec::with_capacity(isolated_count);
                for _ in 0..isolated_count {
                    isolated_rooms.push(reader.read_string()?);
                }
                let pack = match reader.read_u8()? {
                    0 => None,
                    1 => Some(PackIdentityV1::new(
                        reader.read_string()?,
                        reader.read_string()?,
                        reader.read_digest()?,
                    )?),
                    _ => {
                        return Err(TransferError::InvalidValue {
                            what: "pack presence",
                        });
                    }
                };
                let resource_count = reader.read_count()?;
                let mut resources = Vec::with_capacity(resource_count);
                for _ in 0..resource_count {
                    let kind = ResourceKindV1::from_tag(reader.read_u8()?)?;
                    let identity = reader.read_string()?;
                    let size_bytes = reader.read_u64()?;
                    let digest = reader.read_digest()?;
                    resources.push(ResourceIdentityV1 {
                        kind,
                        identity,
                        size_bytes,
                        digest,
                    });
                }
                let target_backend = read_backend_fingerprint(&mut reader)?;
                (
                    scope,
                    source_backend,
                    isolated_rooms,
                    pack,
                    resources,
                    target_backend,
                )
            };
        if source_backend
            .as_ref()
            .is_some_and(|backend| backend.profile() != source_profile)
            || target_backend.profile() != target_profile
        {
            return Err(TransferError::TargetMismatch {
                what: "wire/backend profile",
            });
        }
        let session_state = SessionStatePolicyV1::from_tag(reader.read_u8()?)?;
        let record_count = reader.read_count()?;
        let mut records = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            let ordinal = reader.read_u64()?;
            let kind = LogicalRecordV1::decode_kind(reader.read_u8()?, reader.read_u8()?)?;
            let identity = reader.read_string()?;
            let bytes = reader.read_blob(MAX_RECORD_BYTES)?;
            let digest = reader.read_digest()?;
            records.push(LogicalRecordV1::from_parts(
                ordinal, kind, identity, &bytes, digest,
            )?);
        }
        if !reader.is_done() {
            return Err(TransferError::TrailingBytes {
                count: reader.remaining(),
            });
        }
        match scope {
            TransferScopeV1::WholeDeployment => Self::new_with_backend_fingerprints(
                bundle_id,
                lineage_id,
                source_epoch,
                source_backend.ok_or(TransferError::InvalidValue {
                    what: "whole-deployment source backend",
                })?,
                target_backend,
                pack.ok_or(TransferError::InvalidValue {
                    what: "whole-deployment pack",
                })?,
                resources,
                session_state,
                records,
            ),
            TransferScopeV1::CanonicalExport => {
                if source_profile != BundleProfileV1::SqliteBundled
                    || source_backend.is_some()
                    || pack.is_some()
                    || !resources.is_empty()
                {
                    return Err(TransferError::InvalidValue {
                        what: "canonical-export global evidence",
                    });
                }
                let mut bundle = Self::from_canonical_export(
                    bundle_id,
                    lineage_id,
                    source_epoch,
                    target_backend,
                    isolated_rooms,
                    records,
                )?;
                bundle.session_state = session_state;
                Ok(bundle)
            }
        }
    }

    /// Encodes the bundle without touching any record payload bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, TransferError> {
        let mut output = Vec::new();
        output.extend_from_slice(BUNDLE_MAGIC);
        write_u16(&mut output, BUNDLE_VERSION);
        write_string(&mut output, &self.bundle_id)?;
        write_string(&mut output, &self.lineage_id)?;
        write_u64(&mut output, self.source_epoch);
        output.push(self.source_profile.tag());
        output.push(self.target_profile.tag());
        output.push(match self.scope {
            TransferScopeV1::WholeDeployment => 1,
            TransferScopeV1::CanonicalExport => 2,
        });
        match &self.source_backend {
            Some(source_backend) => {
                output.push(1);
                write_backend_fingerprint(&mut output, source_backend)?;
            }
            None => output.push(0),
        }
        write_count(&mut output, self.isolated_rooms.len())?;
        for room_id in &self.isolated_rooms {
            write_string(&mut output, room_id)?;
        }
        match &self.pack {
            Some(pack) => {
                output.push(1);
                write_string(&mut output, pack.pack_id())?;
                write_string(&mut output, pack.revision())?;
                output.extend_from_slice(&pack.digest().as_bytes());
            }
            None => output.push(0),
        }
        write_count(&mut output, self.resources.len())?;
        for resource in &self.resources {
            output.push(resource.kind.tag());
            write_string(&mut output, resource.identity())?;
            write_u64(&mut output, resource.size_bytes());
            output.extend_from_slice(&resource.digest().as_bytes());
        }
        write_backend_fingerprint(&mut output, &self.target_backend)?;
        output.push(SessionStatePolicyV1::tag());
        write_count(&mut output, self.records.len())?;
        for record in &self.records {
            write_record(&mut output, record)?;
        }
        if output.len() > MAX_BUNDLE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        Ok(output)
    }

    /// Atomically exports deterministic bundle bytes to a new regular file.
    ///
    /// Existing destinations and symlinks are rejected. The temporary sibling
    /// is flushed before the rename so a caller can use the resulting file as
    /// a bounded local export/import fixture without treating it as provider
    /// durability evidence.
    pub fn export_to_path(&self, path: &Path) -> Result<DigestV1, TransferError> {
        reject_existing_or_symlink(path)?;
        let bytes = self.to_bytes()?;
        let temporary = temporary_export_path(path);
        if temporary.exists() {
            return Err(TransferError::DestinationExists);
        }
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| io_error("create export", &error))?;
            file.write_all(&bytes)
                .map_err(|error| io_error("write export", &error))?;
            file.sync_all()
                .map_err(|error| io_error("flush export", &error))?;
            fs::rename(&temporary, path).map_err(|error| io_error("rename export", &error))?;
            Ok(DigestV1::hash(&bytes))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Imports a complete bounded bundle from a regular local file.
    pub fn import_from_path(path: &Path) -> Result<Self, TransferError> {
        let metadata =
            fs::symlink_metadata(path).map_err(|error| io_error("stat import", &error))?;
        if !metadata.file_type().is_file() {
            return Err(TransferError::InvalidPath);
        }
        if metadata.len() > MAX_BUNDLE_BYTES as u64 {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        let mut file = File::open(path).map_err(|error| io_error("open import", &error))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_BUNDLE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("read import", &error))?;
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "bundle bytes",
            });
        }
        Self::from_bytes(&bytes)
    }

    /// Hashes the complete deterministic bundle wire bytes.
    pub fn bundle_hash(&self) -> Result<DigestV1, TransferError> {
        Ok(DigestV1::hash(&self.to_bytes()?))
    }

    /// Returns the stable bundle identifier.
    #[must_use]
    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    /// Returns the source deployment epoch.
    #[must_use]
    pub const fn source_epoch(&self) -> u64 {
        self.source_epoch
    }

    /// Returns the source storage profile.
    #[must_use]
    pub const fn source_profile(&self) -> BundleProfileV1 {
        self.source_profile
    }

    /// Returns the target storage profile.
    #[must_use]
    pub const fn target_profile(&self) -> BundleProfileV1 {
        self.target_profile
    }

    /// Returns the exact source backend/build/schema identity.
    #[must_use]
    pub fn source_backend(&self) -> Option<&BackendFingerprintV1> {
        self.source_backend.as_ref()
    }

    /// Returns the exact target backend/build/schema identity required by the bundle.
    #[must_use]
    pub fn target_backend(&self) -> &BackendFingerprintV1 {
        &self.target_backend
    }

    /// Returns the deployment lineage identity.
    #[must_use]
    pub fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    /// Returns the exact pack identity.
    #[must_use]
    pub fn pack(&self) -> Option<&PackIdentityV1> {
        self.pack.as_ref()
    }

    /// Returns all exact resource identities.
    #[must_use]
    pub fn resources(&self) -> &[ResourceIdentityV1] {
        &self.resources
    }

    /// Returns the explicit source-evidence scope.
    #[must_use]
    pub const fn scope(&self) -> TransferScopeV1 {
        self.scope
    }

    /// Returns isolated source Rooms omitted from healthy canonical records.
    #[must_use]
    pub fn isolated_rooms(&self) -> &[String] {
        &self.isolated_rooms
    }

    /// Returns the session-state import policy.
    #[must_use]
    pub const fn session_state(&self) -> SessionStatePolicyV1 {
        self.session_state
    }

    /// Returns records in their deterministic source order.
    #[must_use]
    pub fn records(&self) -> &[LogicalRecordV1] {
        &self.records
    }

    /// Computes deployment and per-Room exact-record parity evidence.
    pub fn parity(&self) -> Result<RecordParityV1, TransferError> {
        RecordParityV1::from_records(&self.records)
    }

    /// Creates a bounded, contiguous chunk for checkpointed transfer.
    pub fn chunk(&self, start: usize, end: usize) -> Result<TransferChunkV1, TransferError> {
        if start >= end || end > self.records.len() {
            return Err(TransferError::InvalidChunkRange { start, end });
        }
        let records = self.records[start..end].to_vec();
        TransferChunkV1::new(self.bundle_hash()?, start, records)
    }

    /// Verifies the exact target profile, epoch, lineage, pack, resources, and bundle hash.
    pub fn verify_target(&self, target: &TargetFingerprintV1) -> Result<(), TransferError> {
        if target.profile != self.target_profile {
            return Err(TransferError::TargetMismatch { what: "profile" });
        }
        let expected_epoch =
            self.source_epoch
                .checked_add(1)
                .ok_or(TransferError::InvalidValue {
                    what: "target epoch",
                })?;
        if target.storage_epoch != expected_epoch {
            return Err(TransferError::TargetMismatch { what: "epoch" });
        }
        if target.backend != self.target_backend {
            return Err(TransferError::TargetMismatch {
                what: "backend fingerprint/schema/migrations",
            });
        }
        if target.lineage_id != self.lineage_id
            || target.pack != self.pack
            || target.resources != self.resources
            || target.scope != self.scope
            || target.isolated_rooms != self.isolated_rooms
            || target.bundle_hash != self.bundle_hash()?
            || target.parity != self.parity()?
        {
            return Err(TransferError::TargetMismatch {
                what: "lineage/pack/resources/bundle hash",
            });
        }
        Ok(())
    }
}

/// A target-side identity snapshot required before finalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetFingerprintV1 {
    profile: BundleProfileV1,
    storage_epoch: u64,
    lineage_id: String,
    backend: BackendFingerprintV1,
    pack: Option<PackIdentityV1>,
    resources: Vec<ResourceIdentityV1>,
    scope: TransferScopeV1,
    isolated_rooms: Vec<String>,
    bundle_hash: DigestV1,
    parity: RecordParityV1,
}

impl TargetFingerprintV1 {
    /// Builds the exact expected target fingerprint for a bundle.
    pub fn for_bundle(bundle: &TransferBundleV1) -> Result<Self, TransferError> {
        Ok(Self {
            profile: bundle.target_profile,
            storage_epoch: bundle.source_epoch.checked_add(1).ok_or(
                TransferError::InvalidValue {
                    what: "target epoch",
                },
            )?,
            lineage_id: bundle.lineage_id.clone(),
            backend: bundle.target_backend.clone(),
            pack: bundle.pack.clone(),
            resources: bundle.resources.clone(),
            scope: bundle.scope,
            isolated_rooms: bundle.isolated_rooms.clone(),
            bundle_hash: bundle.bundle_hash()?,
            parity: bundle.parity()?,
        })
    }

    /// Constructs an observed target fingerprint from adapter-supplied
    /// evidence. No target is considered verified until this exact value
    /// matches the bundle contract.
    #[allow(clippy::too_many_arguments)]
    pub fn observed(
        profile: BundleProfileV1,
        storage_epoch: u64,
        lineage_id: impl Into<String>,
        backend: BackendFingerprintV1,
        pack: PackIdentityV1,
        resources: Vec<ResourceIdentityV1>,
        bundle_hash: DigestV1,
        parity: RecordParityV1,
    ) -> Result<Self, TransferError> {
        if storage_epoch == 0 {
            return Err(TransferError::InvalidValue {
                what: "target epoch",
            });
        }
        if backend.profile() != profile {
            return Err(TransferError::TargetMismatch {
                what: "backend/profile",
            });
        }
        let lineage_id = lineage_id.into();
        validate_text(&lineage_id)?;
        Ok(Self {
            profile,
            storage_epoch,
            lineage_id,
            backend,
            pack: Some(pack),
            resources,
            scope: TransferScopeV1::WholeDeployment,
            isolated_rooms: Vec::new(),
            bundle_hash,
            parity,
        })
    }

    /// Constructs an observed target fence for canonical-export mechanics.
    /// Global pack/resource evidence remains explicitly absent.
    pub fn observed_canonical_export(
        storage_epoch: u64,
        lineage_id: impl Into<String>,
        backend: BackendFingerprintV1,
        isolated_rooms: Vec<String>,
        bundle_hash: DigestV1,
        parity: RecordParityV1,
    ) -> Result<Self, TransferError> {
        if storage_epoch == 0 {
            return Err(TransferError::InvalidValue {
                what: "target epoch",
            });
        }
        if backend.profile() != BundleProfileV1::PostgresPrimary17 {
            return Err(TransferError::TargetMismatch {
                what: "backend/profile",
            });
        }
        let lineage_id = lineage_id.into();
        validate_text(&lineage_id)?;
        for room_id in &isolated_rooms {
            validate_text(room_id)?;
        }
        Ok(Self {
            profile: BundleProfileV1::PostgresPrimary17,
            storage_epoch,
            lineage_id,
            backend,
            pack: None,
            resources: Vec::new(),
            scope: TransferScopeV1::CanonicalExport,
            isolated_rooms,
            bundle_hash,
            parity,
        })
    }

    /// Makes a deliberately different pack fingerprint for mismatch tests or adapters.
    pub fn with_pack(target: &Self, pack: PackIdentityV1) -> Result<Self, TransferError> {
        if target.scope != TransferScopeV1::WholeDeployment {
            return Err(TransferError::TargetMismatch {
                what: "canonical-export pack override",
            });
        }
        Ok(Self {
            profile: target.profile,
            storage_epoch: target.storage_epoch,
            lineage_id: target.lineage_id.clone(),
            backend: target.backend.clone(),
            pack: Some(pack),
            resources: target.resources.clone(),
            scope: target.scope,
            isolated_rooms: target.isolated_rooms.clone(),
            bundle_hash: target.bundle_hash,
            parity: target.parity.clone(),
        })
    }

    /// Returns a deterministic digest over every target admission field.
    ///
    /// Adapters persist this digest beside their durable import markers. It is
    /// intentionally derived from the exact target contract rather than from
    /// a provider-specific representation, so a retry cannot silently attach
    /// to a different epoch, schema, pack, resource set, or parity result.
    pub fn fingerprint_digest(&self) -> Result<DigestV1, TransferError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"worldstream/target-fingerprint/v1");
        bytes.push(self.profile.tag());
        write_u64(&mut bytes, self.storage_epoch);
        write_string(&mut bytes, &self.lineage_id)?;
        write_backend_fingerprint(&mut bytes, &self.backend)?;
        match &self.pack {
            Some(pack) => {
                bytes.push(1);
                write_string(&mut bytes, pack.pack_id())?;
                write_string(&mut bytes, pack.revision())?;
                bytes.extend_from_slice(&pack.digest().as_bytes());
            }
            None => bytes.push(0),
        }
        write_count(&mut bytes, self.resources.len())?;
        for resource in &self.resources {
            bytes.push(resource.kind.tag());
            write_string(&mut bytes, resource.identity())?;
            write_u64(&mut bytes, resource.size_bytes());
            bytes.extend_from_slice(&resource.digest().as_bytes());
        }
        bytes.push(match self.scope {
            TransferScopeV1::WholeDeployment => 1,
            TransferScopeV1::CanonicalExport => 2,
        });
        write_count(&mut bytes, self.isolated_rooms.len())?;
        for room_id in &self.isolated_rooms {
            write_string(&mut bytes, room_id)?;
        }
        bytes.extend_from_slice(&self.bundle_hash.as_bytes());
        write_parity(&mut bytes, &self.parity)?;
        Ok(DigestV1::hash(&bytes))
    }

    /// Returns the target profile selected by the bundle contract.
    #[must_use]
    pub const fn profile(&self) -> BundleProfileV1 {
        self.profile
    }

    /// Returns the next fenced storage epoch.
    #[must_use]
    pub const fn storage_epoch(&self) -> u64 {
        self.storage_epoch
    }

    /// Returns the exact deployment lineage identity.
    #[must_use]
    pub fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    /// Returns the exact observed backend/build/schema identity.
    #[must_use]
    pub fn backend(&self) -> &BackendFingerprintV1 {
        &self.backend
    }

    /// Returns the complete bundle hash bound to this target.
    #[must_use]
    pub const fn bundle_hash(&self) -> DigestV1 {
        self.bundle_hash
    }

    /// Returns the exact pack identity expected on the target.
    #[must_use]
    pub fn pack(&self) -> Option<&PackIdentityV1> {
        self.pack.as_ref()
    }

    /// Returns the exact resource identities expected on the target.
    #[must_use]
    pub fn resources(&self) -> &[ResourceIdentityV1] {
        &self.resources
    }

    /// Returns the deployment and per-Room parity observed at the target.
    #[must_use]
    pub fn parity(&self) -> &RecordParityV1 {
        &self.parity
    }
}

/// A contiguous export/import chunk with an exact chunk digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferChunkV1 {
    bundle_hash: DigestV1,
    start: usize,
    end: usize,
    records: Vec<LogicalRecordV1>,
    digest: DigestV1,
}

impl TransferChunkV1 {
    fn new(
        bundle_hash: DigestV1,
        start: usize,
        records: Vec<LogicalRecordV1>,
    ) -> Result<Self, TransferError> {
        let end = start
            .checked_add(records.len())
            .ok_or(TransferError::BoundExceeded { what: "chunk" })?;
        let digest = chunk_digest(start, &records)?;
        Ok(Self {
            bundle_hash,
            start,
            end,
            records,
            digest,
        })
    }

    /// Builds a chunk against an existing bundle fence without changing any
    /// source record bytes. Adapters use this to verify conflict handling for
    /// a same-range, different-payload retry.
    pub fn from_records(
        bundle_hash: DigestV1,
        start: usize,
        records: Vec<LogicalRecordV1>,
    ) -> Result<Self, TransferError> {
        Self::new(bundle_hash, start, records)
    }

    /// Returns the bundle hash covered by this chunk.
    #[must_use]
    pub const fn bundle_hash(&self) -> DigestV1 {
        self.bundle_hash
    }

    /// Returns the inclusive record start ordinal.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// Returns the exclusive record end ordinal.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// Returns the exact chunk digest used for durable deduplication.
    #[must_use]
    pub const fn digest(&self) -> DigestV1 {
        self.digest
    }

    /// Returns the byte-preserved records in this chunk.
    #[must_use]
    pub fn records(&self) -> &[LogicalRecordV1] {
        &self.records
    }

    /// Returns the exact canonical wire bytes for the records in this chunk.
    ///
    /// The chunk envelope is deliberately excluded; adapters persist this
    /// byte stream as the source payload and retain the separately computed
    /// chunk digest/range as the durable idempotency key.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransferError> {
        let mut bytes = Vec::new();
        for record in &self.records {
            write_record(&mut bytes, record)?;
        }
        Ok(bytes)
    }
}

/// The result of a destination's durable chunk write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferChunkDispositionV1 {
    /// The destination durably applied this chunk during this call.
    Applied,
    /// The exact bundle/range/digest was already durably applied.
    AlreadyApplied,
}

/// The provider-facing destination seam for a bounded logical import.
///
/// Implementations backed by `SQLite` or `PostgreSQL` must persist the tuple
/// `(bundle_hash, start, end, digest)` atomically with the chunk rows and
/// return `AlreadyApplied` only after verifying that exact tuple. This makes
/// a retry safe when a process stops after the destination commit but before
/// the serialized transfer checkpoint is persisted. `verify_complete` must
/// perform destination-side semantic verification; the coordinator does not
/// treat row count or a successful SQL transaction as proof of correctness.
/// For a whole-deployment move, `verify_complete` must finish target hydration
/// and full semantic replay while the target is non-serving; `record_finalization`
/// may persist only that exact provider-derived verified target. Consequently,
/// `accept_target_write` publishes/unfences an already verified target and must
/// not defer target hydration or semantic verification until after source
/// retirement.
pub trait TransferDestinationV1 {
    /// The provider-specific error returned by destination operations.
    type Error: fmt::Debug + fmt::Display;

    /// Applies one chunk or confirms its exact durable prior application.
    fn apply_chunk(
        &mut self,
        chunk: &TransferChunkV1,
    ) -> Result<TransferChunkDispositionV1, Self::Error>;

    /// Verifies the complete imported canonical state and target identities.
    fn verify_complete(
        &mut self,
        bundle: &TransferBundleV1,
        target: &TargetFingerprintV1,
    ) -> Result<(), Self::Error>;

    /// Writes or idempotently confirms the durable finalization record that
    /// retires source authority.
    fn record_finalization(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error>;

    /// Performs or idempotently confirms the first authoritative target write
    /// in the next epoch.
    fn accept_target_write(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error>;

    /// Removes an incomplete destination import before source rollback. A
    /// successful return must durably tombstone the exact bundle/target
    /// binding, including when no chunk or import marker was ever written;
    /// this return is the provider evidence that authorizes source restoration.
    fn abort_import(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProviderDerivedTargetBindingV1 {
    bundle_hash: DigestV1,
    target_fingerprint: DigestV1,
    source_epoch: u64,
    target_epoch: u64,
    lineage_id: String,
}

impl ProviderDerivedTargetBindingV1 {
    fn verified(
        bundle: &TransferBundleV1,
        target: &TargetFingerprintV1,
    ) -> Result<Self, TransferError> {
        if bundle.scope() != TransferScopeV1::WholeDeployment {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        bundle.verify_target(target)?;
        Ok(Self {
            bundle_hash: bundle.bundle_hash()?,
            target_fingerprint: target.fingerprint_digest()?,
            source_epoch: bundle.source_epoch(),
            target_epoch: target.storage_epoch(),
            lineage_id: bundle.lineage_id().to_owned(),
        })
    }
}

/// Opaque proof that the destination provider semantically verified and
/// durably finalized one exact whole-deployment import.
///
/// Its fields are intentionally private. Only the coordinator can construct
/// this proof, after the destination implementation has confirmed its actual
/// durable state; callers cannot substitute bundle or target digest labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedTargetFinalizationV1(ProviderDerivedTargetBindingV1);

impl VerifiedTargetFinalizationV1 {
    /// Returns the exact verified bundle hash.
    #[must_use]
    pub const fn bundle_hash(&self) -> DigestV1 {
        self.0.bundle_hash
    }

    /// Returns the digest of every verified target fingerprint field.
    #[must_use]
    pub const fn target_fingerprint(&self) -> DigestV1 {
        self.0.target_fingerprint
    }

    /// Returns the source epoch captured by the verified bundle.
    #[must_use]
    pub const fn source_epoch(&self) -> u64 {
        self.0.source_epoch
    }

    /// Returns the next target epoch admitted by the provider.
    #[must_use]
    pub const fn target_epoch(&self) -> u64 {
        self.0.target_epoch
    }

    /// Returns the exact deployment lineage admitted by the provider.
    #[must_use]
    pub fn lineage_id(&self) -> &str {
        &self.0.lineage_id
    }
}

/// Opaque proof that the destination provider discarded or durably tombstoned
/// one exact incomplete whole-deployment import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedTargetAbortV1(ProviderDerivedTargetBindingV1);

impl VerifiedTargetAbortV1 {
    /// Returns the exact aborted bundle hash.
    #[must_use]
    pub const fn bundle_hash(&self) -> DigestV1 {
        self.0.bundle_hash
    }

    /// Returns the fingerprint of the exact discarded target import.
    #[must_use]
    pub const fn target_fingerprint(&self) -> DigestV1 {
        self.0.target_fingerprint
    }

    /// Returns the source epoch captured by the discarded bundle.
    #[must_use]
    pub const fn source_epoch(&self) -> u64 {
        self.0.source_epoch
    }

    /// Returns the target epoch that was discarded before source restoration.
    #[must_use]
    pub const fn target_epoch(&self) -> u64 {
        self.0.target_epoch
    }

    /// Returns the exact deployment lineage of the discarded import.
    #[must_use]
    pub fn lineage_id(&self) -> &str {
        &self.0.lineage_id
    }
}

/// Source-authority seam consumed only by provider-derived coordinator proofs.
///
/// Implementations must make both methods idempotent for the same proof so a
/// process stop between provider and source commits can be retried safely.
pub trait TransferSourceAuthorityV1 {
    /// Provider-specific source error.
    type Error: fmt::Debug + fmt::Display;

    /// Binds the exact provider finalization and permanently retires the source.
    fn retire_after_verified_target(
        &self,
        proof: &VerifiedTargetFinalizationV1,
    ) -> Result<(), Self::Error>;

    /// Restores source authority only after the provider discarded the target.
    fn restore_after_verified_abort(
        &self,
        proof: &VerifiedTargetAbortV1,
    ) -> Result<(), Self::Error>;
}

/// Failure from the cross-provider authority coordinator.
#[derive(Debug, Error)]
pub enum WholeDeploymentTransferErrorV1<DestinationError, SourceError>
where
    DestinationError: fmt::Debug + fmt::Display,
    SourceError: fmt::Debug + fmt::Display,
{
    /// Provider-neutral lifecycle or exact-binding failure.
    #[error(transparent)]
    Contract(#[from] TransferError),
    /// Destination operation failed before its authority transition completed.
    #[error("destination {operation} failed: {error}")]
    Destination {
        /// Exact provider operation that failed.
        operation: &'static str,
        /// Provider error.
        error: DestinationError,
    },
    /// Source transition failed after the destination disposition was durable.
    #[error("source {operation} failed: {error}")]
    Source {
        /// Exact source operation that failed.
        operation: &'static str,
        /// Source error.
        error: SourceError,
    },
}

/// Provider-neutral errors from the coordinated import flow.
#[derive(Debug, Error)]
pub enum TransferRunError<E: fmt::Debug + fmt::Display> {
    /// A transfer contract or lifecycle invariant failed.
    #[error(transparent)]
    Contract(#[from] TransferError),
    /// A destination operation failed and authority did not advance.
    #[error("destination {operation} failed: {error}")]
    Destination {
        /// The operation that failed.
        operation: &'static str,
        /// The provider-specific failure.
        error: E,
    },
}

/// A resumable checkpoint shared by deterministic export and import fixtures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferCheckpointV1 {
    bundle_hash: DigestV1,
    next_ordinal: usize,
    committed_digest: DigestV1,
    last_chunk: Option<(usize, usize, DigestV1)>,
}

impl TransferCheckpointV1 {
    /// Starts a checkpoint before the first record.
    pub fn initial(bundle: &TransferBundleV1) -> Result<Self, TransferError> {
        Ok(Self {
            bundle_hash: bundle.bundle_hash()?,
            next_ordinal: 0,
            committed_digest: DigestV1::hash(b"worldstream/transfer-checkpoint/v1"),
            last_chunk: None,
        })
    }

    /// Commits a chunk, returning `true` when it is the exact already-committed chunk.
    pub fn apply(&mut self, chunk: &TransferChunkV1) -> Result<bool, TransferError> {
        if chunk.bundle_hash != self.bundle_hash {
            return Err(TransferError::CheckpointMismatch {
                what: "bundle hash",
            });
        }
        if chunk.start < self.next_ordinal {
            if self.last_chunk == Some((chunk.start, chunk.end, chunk.digest)) {
                return Ok(true);
            }
            return Err(TransferError::CheckpointMismatch {
                what: "replayed chunk",
            });
        }
        if chunk.start != self.next_ordinal {
            return Err(TransferError::CheckpointMismatch {
                what: "chunk start",
            });
        }
        let mut chain = Vec::with_capacity(64);
        chain.extend_from_slice(&self.committed_digest.as_bytes());
        chain.extend_from_slice(&chunk.digest.as_bytes());
        self.committed_digest = DigestV1::hash(&chain);
        self.next_ordinal = chunk.end;
        self.last_chunk = Some((chunk.start, chunk.end, chunk.digest));
        Ok(false)
    }

    /// Returns whether every bundle record has been committed contiguously.
    #[must_use]
    pub fn is_complete(&self, bundle: &TransferBundleV1) -> bool {
        let Ok(bundle_hash) = bundle.bundle_hash() else {
            return false;
        };
        self.bundle_hash == bundle_hash && self.next_ordinal == bundle.records.len()
    }

    /// Encodes the resumable checkpoint for durable retry state.
    pub fn to_bytes(&self) -> Result<Vec<u8>, TransferError> {
        let mut output = Vec::new();
        output.extend_from_slice(CHECKPOINT_MAGIC);
        write_u16(&mut output, CHECKPOINT_VERSION);
        output.extend_from_slice(&self.bundle_hash.as_bytes());
        write_u64(
            &mut output,
            u64::try_from(self.next_ordinal).map_err(|_| TransferError::BoundExceeded {
                what: "checkpoint ordinal",
            })?,
        );
        output.extend_from_slice(&self.committed_digest.as_bytes());
        match self.last_chunk {
            Some((start, end, digest)) => {
                output.push(1);
                write_u64(
                    &mut output,
                    u64::try_from(start).map_err(|_| TransferError::BoundExceeded {
                        what: "checkpoint ordinal",
                    })?,
                );
                write_u64(
                    &mut output,
                    u64::try_from(end).map_err(|_| TransferError::BoundExceeded {
                        what: "checkpoint ordinal",
                    })?,
                );
                output.extend_from_slice(&digest.as_bytes());
            }
            None => output.push(0),
        }
        Ok(output)
    }

    /// Decodes a complete checkpoint and rejects unknown versions or trailing bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransferError> {
        let mut reader = Reader::new(bytes);
        if reader.read_exact(8)? != CHECKPOINT_MAGIC {
            return Err(TransferError::InvalidMagic);
        }
        let version = reader.read_u16()?;
        if version != CHECKPOINT_VERSION {
            return Err(TransferError::UnsupportedCheckpointVersion(version));
        }
        let bundle_hash = reader.read_digest()?;
        let next_ordinal = reader.read_u64()?;
        let next_ordinal =
            usize::try_from(next_ordinal).map_err(|_| TransferError::BoundExceeded {
                what: "checkpoint ordinal",
            })?;
        let committed_digest = reader.read_digest()?;
        let last_chunk = match reader.read_u8()? {
            0 => None,
            1 => Some((
                usize::try_from(reader.read_u64()?).map_err(|_| TransferError::BoundExceeded {
                    what: "checkpoint ordinal",
                })?,
                usize::try_from(reader.read_u64()?).map_err(|_| TransferError::BoundExceeded {
                    what: "checkpoint ordinal",
                })?,
                reader.read_digest()?,
            )),
            other => return Err(TransferError::UnknownCheckpointMarker(other)),
        };
        if !reader.is_done() {
            return Err(TransferError::TrailingBytes {
                count: reader.remaining(),
            });
        }
        if let Some((start, end, _)) = last_chunk {
            if start >= end || end != next_ordinal {
                return Err(TransferError::InvalidValue {
                    what: "checkpoint range",
                });
            }
        } else if next_ordinal != 0 {
            return Err(TransferError::InvalidValue {
                what: "checkpoint without last chunk",
            });
        }
        Ok(Self {
            bundle_hash,
            next_ordinal,
            committed_digest,
            last_chunk,
        })
    }
}

/// Coarse offline authority state. Target-authoritative means `SQLite` is retired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferStateV1 {
    SourceAuthoritative,
    TransferPending,
    TargetVerified,
    Finalized,
    TargetAuthoritative,
}

impl TransferStateV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::SourceAuthoritative => 1,
            Self::TransferPending => 2,
            Self::TargetVerified => 3,
            Self::Finalized => 4,
            Self::TargetAuthoritative => 5,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, TransferError> {
        match tag {
            1 => Ok(Self::SourceAuthoritative),
            2 => Ok(Self::TransferPending),
            3 => Ok(Self::TargetVerified),
            4 => Ok(Self::Finalized),
            5 => Ok(Self::TargetAuthoritative),
            other => Err(TransferError::UnknownImportState(other)),
        }
    }
}

/// Source-retirement and deployment-epoch fence for one transfer attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferLifecycleV1 {
    bundle_hash: DigestV1,
    target: TargetFingerprintV1,
    source_epoch: u64,
    target_epoch: u64,
    state: TransferStateV1,
}

impl TransferLifecycleV1 {
    /// Quiesces the source and starts a resumable transfer fence.
    pub fn begin(bundle: &TransferBundleV1) -> Result<Self, TransferError> {
        let target = TargetFingerprintV1::for_bundle(bundle)?;
        Ok(Self {
            bundle_hash: target.bundle_hash,
            target,
            source_epoch: bundle.source_epoch,
            target_epoch: bundle.source_epoch.checked_add(1).ok_or(
                TransferError::InvalidValue {
                    what: "target epoch",
                },
            )?,
            state: TransferStateV1::TransferPending,
        })
    }

    fn restore(bundle: &TransferBundleV1, state: TransferStateV1) -> Result<Self, TransferError> {
        if bundle.scope() == TransferScopeV1::CanonicalExport
            && matches!(
                state,
                TransferStateV1::Finalized | TransferStateV1::TargetAuthoritative
            )
        {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        let mut lifecycle = Self::begin(bundle)?;
        lifecycle.state = state;
        Ok(lifecycle)
    }

    /// Returns the current authority state.
    #[must_use]
    pub const fn state(&self) -> TransferStateV1 {
        self.state
    }

    /// Verifies the empty target before finalization.
    pub fn verify_target(&mut self, target: &TargetFingerprintV1) -> Result<(), TransferError> {
        if self.state != TransferStateV1::TransferPending {
            return Err(self.invalid_state("verify target"));
        }
        if target != &self.target {
            return Err(TransferError::TargetMismatch {
                what: "epoch or bundle hash",
            });
        }
        self.state = TransferStateV1::TargetVerified;
        Ok(())
    }

    /// Records the first target write only when its complete lineage fence is
    /// exactly the one verified before finalization.
    pub fn accept_target_write_checked(
        &mut self,
        target: &TargetFingerprintV1,
    ) -> Result<(), TransferError> {
        if self.target.scope == TransferScopeV1::CanonicalExport {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        if self.state != TransferStateV1::Finalized {
            return Err(self.invalid_state("accept checked target write"));
        }
        if target != &self.target {
            return Err(TransferError::EpochMismatch);
        }
        self.state = TransferStateV1::TargetAuthoritative;
        Ok(())
    }

    /// Writes the explicit finalization record and retires source writes.
    pub fn finalize(&mut self) -> Result<(), TransferError> {
        if self.target.scope == TransferScopeV1::CanonicalExport {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        if self.state != TransferStateV1::TargetVerified {
            return Err(self.invalid_state("finalize"));
        }
        self.state = TransferStateV1::Finalized;
        Ok(())
    }

    /// Records the first target write in the next epoch; rollback is now fenced.
    pub fn accept_target_write(
        &mut self,
        epoch: u64,
        bundle_hash: DigestV1,
    ) -> Result<(), TransferError> {
        if self.target.scope == TransferScopeV1::CanonicalExport {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        if self.state != TransferStateV1::Finalized {
            return Err(self.invalid_state("accept target write"));
        }
        if epoch != self.target_epoch || bundle_hash != self.bundle_hash {
            return Err(TransferError::EpochMismatch);
        }
        self.state = TransferStateV1::TargetAuthoritative;
        Ok(())
    }

    /// Aborts only while no finalization record exists.
    pub fn abort(&mut self) -> Result<(), TransferError> {
        match self.state {
            TransferStateV1::TransferPending | TransferStateV1::TargetVerified => {
                self.state = TransferStateV1::SourceAuthoritative;
                Ok(())
            }
            _ => Err(self.invalid_state("abort")),
        }
    }

    /// Checks whether one profile/epoch may accept an authoritative write.
    pub fn check_write(&self, profile: BundleProfileV1, epoch: u64) -> Result<(), TransferError> {
        match (self.state, profile, epoch) {
            (TransferStateV1::SourceAuthoritative, BundleProfileV1::SqliteBundled, epoch)
                if epoch == self.source_epoch =>
            {
                Ok(())
            }
            (TransferStateV1::TargetAuthoritative, BundleProfileV1::PostgresPrimary17, epoch)
                if epoch == self.target_epoch =>
            {
                Ok(())
            }
            (_, _, _) => Err(TransferError::EpochMismatch),
        }
    }

    fn invalid_state(&self, operation: &'static str) -> TransferError {
        TransferError::InvalidLifecycleState {
            operation,
            state: self.state,
        }
    }
}

/// A bounded, resumable import session joining chunks to the authority fence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferImportSessionV1 {
    lifecycle: TransferLifecycleV1,
    checkpoint: TransferCheckpointV1,
    record_count: usize,
}

impl TransferImportSessionV1 {
    /// Starts an import in the pending state without contacting a provider.
    pub fn begin(bundle: &TransferBundleV1) -> Result<Self, TransferError> {
        Ok(Self {
            lifecycle: TransferLifecycleV1::begin(bundle)?,
            checkpoint: TransferCheckpointV1::initial(bundle)?,
            record_count: bundle.records.len(),
        })
    }

    /// Returns the current authority state.
    #[must_use]
    pub const fn state(&self) -> TransferStateV1 {
        self.lifecycle.state()
    }

    /// Returns the exact expected target identity.
    #[must_use]
    pub fn target(&self) -> &TargetFingerprintV1 {
        &self.lifecycle.target
    }

    /// Verifies the target before allowing any destination writes.
    pub fn verify_target(&mut self, target: &TargetFingerprintV1) -> Result<(), TransferError> {
        self.lifecycle.verify_target(target)
    }

    /// Returns whether all source records have been checkpointed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.checkpoint.next_ordinal == self.record_count
    }

    /// Returns the first record ordinal that has not been durably checkpointed.
    ///
    /// Coordinators use this value to construct only the next bounded chunk
    /// after restoring an opaque serialized session. Destination adapters
    /// still reconfirm exact replay markers; this ordinal is never treated as
    /// provider durability evidence by itself.
    #[must_use]
    pub const fn next_ordinal(&self) -> usize {
        self.checkpoint.next_ordinal
    }

    /// Applies one contiguous chunk through the durable destination seam.
    pub fn apply_chunk<D: TransferDestinationV1>(
        &mut self,
        destination: &mut D,
        chunk: &TransferChunkV1,
    ) -> Result<TransferChunkDispositionV1, TransferRunError<D::Error>> {
        if self.state() != TransferStateV1::TargetVerified {
            return Err(self.lifecycle.invalid_state("apply import chunk").into());
        }
        if chunk.bundle_hash != self.checkpoint.bundle_hash {
            return Err(TransferError::CheckpointMismatch {
                what: "bundle hash",
            }
            .into());
        }
        if chunk.start > chunk.end
            || chunk.end > self.record_count
            || chunk.end.saturating_sub(chunk.start) != chunk.records.len()
        {
            return Err(TransferError::InvalidValue {
                what: "chunk bounds",
            }
            .into());
        }
        if chunk.start < self.checkpoint.next_ordinal {
            if self.checkpoint.last_chunk == Some((chunk.start, chunk.end, chunk.digest)) {
                // A serialized checkpoint is only a retry hint. Reconfirm the
                // destination's durable idempotency marker so a lost target
                // cannot be mistaken for a completed import.
                return destination.apply_chunk(chunk).map_err(|error| {
                    TransferRunError::Destination {
                        operation: "confirm replayed chunk",
                        error,
                    }
                });
            }
            return Err(TransferError::CheckpointMismatch {
                what: "replayed chunk",
            }
            .into());
        }
        if chunk.start != self.checkpoint.next_ordinal {
            return Err(TransferError::CheckpointMismatch {
                what: "chunk start",
            }
            .into());
        }

        let disposition =
            destination
                .apply_chunk(chunk)
                .map_err(|error| TransferRunError::Destination {
                    operation: "apply chunk",
                    error,
                })?;
        self.checkpoint.apply(chunk)?;
        Ok(disposition)
    }

    /// Publishes an ordered chunk slice, allowing the caller to serialize the
    /// session checkpoint after each successful return and resume by passing
    /// the same slice again. Exact replays are confirmed by the destination's
    /// durable idempotency marker.
    pub fn publish_chunks<D: TransferDestinationV1>(
        &mut self,
        destination: &mut D,
        chunks: &[TransferChunkV1],
    ) -> Result<(), TransferRunError<D::Error>> {
        for chunk in chunks {
            self.apply_chunk(destination, chunk)?;
        }
        Ok(())
    }

    /// Verifies the complete destination and writes its finalization record.
    fn finalize<D: TransferDestinationV1>(
        &mut self,
        destination: &mut D,
        bundle: &TransferBundleV1,
    ) -> Result<(), TransferRunError<D::Error>> {
        if bundle.scope() == TransferScopeV1::CanonicalExport {
            return Err(TransferError::CanonicalExportNotAuthoritative.into());
        }
        if self.state() != TransferStateV1::TargetVerified {
            return Err(self.lifecycle.invalid_state("finalize import").into());
        }
        if !self.checkpoint.is_complete(bundle) || bundle.records.len() != self.record_count {
            return Err(TransferError::InvalidValue {
                what: "incomplete import",
            }
            .into());
        }
        bundle.verify_target(self.target())?;
        destination
            .verify_complete(bundle, self.target())
            .map_err(|error| TransferRunError::Destination {
                operation: "verify complete import",
                error,
            })?;
        destination
            .record_finalization(self.target())
            .map_err(|error| TransferRunError::Destination {
                operation: "record finalization",
                error,
            })?;
        self.lifecycle.finalize()?;
        Ok(())
    }

    /// Verifies a complete canonical-export import without recording
    /// finalization or changing source/target authority.
    pub fn verify_canonical_export<D: TransferDestinationV1>(
        &mut self,
        destination: &mut D,
        bundle: &TransferBundleV1,
    ) -> Result<(), TransferRunError<D::Error>> {
        if bundle.scope() != TransferScopeV1::CanonicalExport {
            return Err(TransferError::InvalidValue {
                what: "canonical-export verification scope",
            }
            .into());
        }
        if self.state() != TransferStateV1::TargetVerified {
            return Err(self
                .lifecycle
                .invalid_state("verify canonical export")
                .into());
        }
        if !self.checkpoint.is_complete(bundle) || bundle.records.len() != self.record_count {
            return Err(TransferError::InvalidValue {
                what: "incomplete canonical export",
            }
            .into());
        }
        bundle.verify_target(self.target())?;
        destination
            .verify_complete(bundle, self.target())
            .map_err(|error| TransferRunError::Destination {
                operation: "verify canonical export",
                error,
            })
    }

    /// Commits the first matching target write after finalization.
    fn accept_target_write<D: TransferDestinationV1>(
        &mut self,
        destination: &mut D,
    ) -> Result<(), TransferRunError<D::Error>> {
        if self.target().scope == TransferScopeV1::CanonicalExport {
            return Err(TransferError::CanonicalExportNotAuthoritative.into());
        }
        if self.state() != TransferStateV1::Finalized {
            return Err(self
                .lifecycle
                .invalid_state("accept target authority")
                .into());
        }
        let target = self.target().clone();
        destination.accept_target_write(&target).map_err(|error| {
            TransferRunError::Destination {
                operation: "accept target authority",
                error,
            }
        })?;
        self.lifecycle.accept_target_write_checked(&target)?;
        Ok(())
    }

    /// Serializes the bounded session state for durable process restart.
    pub fn to_bytes(&self) -> Result<Vec<u8>, TransferError> {
        let checkpoint = self.checkpoint.to_bytes()?;
        let mut output = Vec::new();
        output.extend_from_slice(IMPORT_MAGIC);
        write_u16(&mut output, IMPORT_VERSION);
        output.extend_from_slice(&self.checkpoint.bundle_hash.as_bytes());
        write_u64(
            &mut output,
            u64::try_from(self.record_count).map_err(|_| TransferError::BoundExceeded {
                what: "import record count",
            })?,
        );
        output.push(self.lifecycle.state.tag());
        write_u64(
            &mut output,
            u64::try_from(checkpoint.len()).map_err(|_| TransferError::BoundExceeded {
                what: "import checkpoint",
            })?,
        );
        output.extend_from_slice(&checkpoint);
        if output.len() > MAX_IMPORT_STATE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "import state bytes",
            });
        }
        Ok(output)
    }

    /// Restores a session only when its bundle identity and record count match.
    pub fn from_bytes(bundle: &TransferBundleV1, bytes: &[u8]) -> Result<Self, TransferError> {
        if bytes.len() > MAX_IMPORT_STATE_BYTES {
            return Err(TransferError::BoundExceeded {
                what: "import state bytes",
            });
        }
        let mut reader = Reader::new(bytes);
        if reader.read_exact(8)? != IMPORT_MAGIC {
            return Err(TransferError::InvalidMagic);
        }
        let version = reader.read_u16()?;
        if version != IMPORT_VERSION {
            return Err(TransferError::UnsupportedImportVersion(version));
        }
        let bundle_hash = reader.read_digest()?;
        let expected_hash = bundle.bundle_hash()?;
        if bundle_hash != expected_hash {
            return Err(TransferError::CheckpointMismatch {
                what: "import session bundle hash",
            });
        }
        let record_count =
            usize::try_from(reader.read_u64()?).map_err(|_| TransferError::BoundExceeded {
                what: "import record count",
            })?;
        if record_count != bundle.records.len() {
            return Err(TransferError::CheckpointMismatch {
                what: "import record count",
            });
        }
        let state = TransferStateV1::from_tag(reader.read_u8()?)?;
        if state == TransferStateV1::SourceAuthoritative {
            return Err(TransferError::InvalidValue {
                what: "source-authoritative import state",
            });
        }
        if bundle.scope() == TransferScopeV1::CanonicalExport
            && matches!(
                state,
                TransferStateV1::Finalized | TransferStateV1::TargetAuthoritative
            )
        {
            return Err(TransferError::CanonicalExportNotAuthoritative);
        }
        let checkpoint_bytes = reader.read_blob(MAX_IMPORT_STATE_BYTES)?;
        if !reader.is_done() {
            return Err(TransferError::TrailingBytes {
                count: reader.remaining(),
            });
        }
        let checkpoint = TransferCheckpointV1::from_bytes(&checkpoint_bytes)?;
        if checkpoint.bundle_hash != bundle_hash || checkpoint.next_ordinal > record_count {
            return Err(TransferError::CheckpointMismatch {
                what: "import checkpoint",
            });
        }
        if matches!(
            state,
            TransferStateV1::Finalized | TransferStateV1::TargetAuthoritative
        ) && checkpoint.next_ordinal != record_count
        {
            return Err(TransferError::InvalidValue {
                what: "finalized incomplete import",
            });
        }
        Ok(Self {
            lifecycle: TransferLifecycleV1::restore(bundle, state)?,
            checkpoint,
            record_count,
        })
    }
}

/// Semantically verifies and finalizes the provider target, retires the exact
/// `SQLite` source through an opaque provider-derived proof, then performs the
/// first authoritative target write.
///
/// Every provider operation is deliberately idempotently reconfirmed on a
/// retry. A stop after target finalization leaves the source pending; a stop
/// after source retirement leaves the target finalization retryable but does
/// not restore source authority.
pub fn finalize_whole_deployment<S, D>(
    session: &mut TransferImportSessionV1,
    destination: &mut D,
    source: &S,
    bundle: &TransferBundleV1,
) -> Result<(), WholeDeploymentTransferErrorV1<D::Error, S::Error>>
where
    S: TransferSourceAuthorityV1,
    D: TransferDestinationV1,
{
    if bundle.scope() != TransferScopeV1::WholeDeployment {
        return Err(TransferError::CanonicalExportNotAuthoritative.into());
    }
    if !matches!(
        session.state(),
        TransferStateV1::TargetVerified
            | TransferStateV1::Finalized
            | TransferStateV1::TargetAuthoritative
    ) {
        return Err(session
            .lifecycle
            .invalid_state("finalize whole deployment")
            .into());
    }
    if !session.checkpoint.is_complete(bundle) || bundle.records.len() != session.record_count {
        return Err(TransferError::InvalidValue {
            what: "incomplete import",
        }
        .into());
    }
    bundle.verify_target(session.target())?;
    let target = session.target().clone();
    if session.state() == TransferStateV1::TargetVerified {
        session
            .finalize(destination, bundle)
            .map_err(|error| match error {
                TransferRunError::Contract(error) => {
                    WholeDeploymentTransferErrorV1::Contract(error)
                }
                TransferRunError::Destination { operation, error } => {
                    WholeDeploymentTransferErrorV1::Destination { operation, error }
                }
            })?;
    } else {
        // Serialized state is only a retry hint. Re-run both provider checks
        // before minting source-retirement proof.
        destination
            .verify_complete(bundle, &target)
            .map_err(|error| WholeDeploymentTransferErrorV1::Destination {
                operation: "confirm complete import",
                error,
            })?;
        destination.record_finalization(&target).map_err(|error| {
            WholeDeploymentTransferErrorV1::Destination {
                operation: "confirm finalization",
                error,
            }
        })?;
    }
    let proof =
        VerifiedTargetFinalizationV1(ProviderDerivedTargetBindingV1::verified(bundle, &target)?);
    source
        .retire_after_verified_target(&proof)
        .map_err(|error| WholeDeploymentTransferErrorV1::Source {
            operation: "retire verified source",
            error,
        })?;
    if session.state() == TransferStateV1::Finalized {
        session
            .accept_target_write(destination)
            .map_err(|error| match error {
                TransferRunError::Contract(error) => {
                    WholeDeploymentTransferErrorV1::Contract(error)
                }
                TransferRunError::Destination { operation, error } => {
                    WholeDeploymentTransferErrorV1::Destination { operation, error }
                }
            })?;
    } else {
        // A serialized authoritative label is not evidence. Reconfirm the
        // provider's durable first-write marker on every retry.
        destination.accept_target_write(&target).map_err(|error| {
            WholeDeploymentTransferErrorV1::Destination {
                operation: "confirm target authority",
                error,
            }
        })?;
    }
    Ok(())
}

/// Discards or tombstones the exact provider target before restoring `SQLite`
/// authority through an opaque provider-derived proof.
///
/// The session does not become source-authoritative until both provider abort
/// and source restoration succeed. Retrying an interrupted abort reconfirms
/// the provider tombstone before replaying the exact source proof.
pub fn abort_whole_deployment<S, D>(
    session: &mut TransferImportSessionV1,
    destination: &mut D,
    source: &S,
    bundle: &TransferBundleV1,
) -> Result<(), WholeDeploymentTransferErrorV1<D::Error, S::Error>>
where
    S: TransferSourceAuthorityV1,
    D: TransferDestinationV1,
{
    if bundle.scope() != TransferScopeV1::WholeDeployment {
        return Err(TransferError::CanonicalExportNotAuthoritative.into());
    }
    if !matches!(
        session.state(),
        TransferStateV1::TransferPending
            | TransferStateV1::TargetVerified
            | TransferStateV1::SourceAuthoritative
    ) {
        return Err(session
            .lifecycle
            .invalid_state("abort whole deployment")
            .into());
    }
    bundle.verify_target(session.target())?;
    let target = session.target().clone();
    destination.abort_import(&target).map_err(|error| {
        WholeDeploymentTransferErrorV1::Destination {
            operation: "abort import",
            error,
        }
    })?;
    let proof = VerifiedTargetAbortV1(ProviderDerivedTargetBindingV1::verified(bundle, &target)?);
    source
        .restore_after_verified_abort(&proof)
        .map_err(|error| WholeDeploymentTransferErrorV1::Source {
            operation: "restore verified source",
            error,
        })?;
    if session.state() != TransferStateV1::SourceAuthoritative {
        session.lifecycle.abort()?;
    }
    Ok(())
}

/// Errors are explicit so unknown records and mismatches fail closed.
#[derive(Debug, Eq, Error, PartialEq)]
pub enum TransferError {
    #[error("invalid transfer bundle magic")]
    InvalidMagic,
    #[error("transfer path is not a regular file")]
    InvalidPath,
    #[error("transfer destination already exists")]
    DestinationExists,
    #[error("transfer file I/O failed during {operation}: {message}")]
    Io {
        operation: &'static str,
        message: String,
    },
    #[error("unsupported bundle version {0}")]
    UnsupportedBundleVersion(u16),
    #[error("unsupported checkpoint version {0}")]
    UnsupportedCheckpointVersion(u16),
    #[error("unsupported import session version {0}")]
    UnsupportedImportVersion(u16),
    #[error("unknown bundle profile tag {0}")]
    UnknownProfile(u8),
    #[error("unknown record class tag {0}")]
    UnknownRecordClass(u8),
    #[error("unknown record kind tag {0}")]
    UnknownRecordKind(u8),
    #[error("unknown resource kind tag {0}")]
    UnknownResourceKind(u8),
    #[error("unknown session policy tag {0}")]
    UnknownSessionPolicy(u8),
    #[error("unknown checkpoint marker {0}")]
    UnknownCheckpointMarker(u8),
    #[error("unknown import session state {0}")]
    UnknownImportState(u8),
    #[error("invalid UTF-8 text")]
    InvalidUtf8,
    #[error("invalid empty or NUL-containing {what}")]
    InvalidText { what: &'static str },
    #[error("invalid {what}")]
    InvalidValue { what: &'static str },
    #[error("unsupported transfer direction")]
    UnsupportedDirection,
    #[error("{what} exceeds the bounded transfer limit")]
    BoundExceeded { what: &'static str },
    #[error("non-deterministic {what} order")]
    NonDeterministicOrder { what: &'static str },
    #[error("duplicate {what} identity")]
    DuplicateIdentity { what: &'static str },
    #[error("digest length is {actual}, expected 32")]
    InvalidDigestLength { actual: usize },
    #[error("resource size mismatch: expected {expected}, actual {actual}")]
    ResourceSizeMismatch { expected: u64, actual: u64 },
    #[error("{what} hash mismatch: expected {expected}, actual {actual}")]
    HashMismatch {
        what: &'static str,
        expected: DigestV1,
        actual: DigestV1,
    },
    #[error("bundle has {count} trailing bytes")]
    TrailingBytes { count: usize },
    #[error("invalid chunk range {start}..{end}")]
    InvalidChunkRange { start: usize, end: usize },
    #[error("checkpoint mismatch: {what}")]
    CheckpointMismatch { what: &'static str },
    #[error("target mismatch: {what}")]
    TargetMismatch { what: &'static str },
    #[error("canonical-export evidence cannot retire source or accept target authority")]
    CanonicalExportNotAuthoritative,
    #[error("deployment epoch mismatch")]
    EpochMismatch,
    #[error("lifecycle operation {operation:?} is invalid in state {state:?}")]
    InvalidLifecycleState {
        operation: &'static str,
        state: TransferStateV1,
    },
    #[error("unexpected end of transfer bytes")]
    UnexpectedEnd,
    #[error("length {length} exceeds the allowed {what} limit")]
    LengthExceeded { what: &'static str, length: u64 },
}

fn validate_text(value: &str) -> Result<(), TransferError> {
    if value.is_empty() || value.contains('\0') {
        return Err(TransferError::InvalidText { what: "identity" });
    }
    if value.len() > u32::MAX as usize {
        return Err(TransferError::BoundExceeded { what: "text" });
    }
    Ok(())
}

fn room_id_from_identity(identity: &str) -> Result<Option<String>, TransferError> {
    let Some(room_identity) = identity.strip_prefix("room/") else {
        return Ok(None);
    };
    let room_id = room_identity.split('/').next().unwrap_or_default();
    if room_id.is_empty() {
        return Err(TransferError::InvalidValue {
            what: "room record identity",
        });
    }
    Ok(Some(room_id.to_owned()))
}

fn default_backend_fingerprint(
    profile: BundleProfileV1,
) -> Result<BackendFingerprintV1, TransferError> {
    let engine_identity = match profile {
        BundleProfileV1::SqliteBundled => "sqlite-bundled",
        BundleProfileV1::PostgresPrimary17 => "postgresql-17",
    };
    let schema = SchemaMigrationContractV1::new(
        "worldstream-storage-v1",
        DigestV1::hash(b"worldstream/storage-schema/v1"),
        vec![MigrationIdentityV1::new(
            1,
            "transfer-contract-default",
            DigestV1::hash(b"worldstream/transfer-contract-default-migration/v1"),
        )?],
    )?;
    BackendFingerprintV1::new(profile, engine_identity, schema)
}

fn reject_existing_or_symlink(path: &Path) -> Result<(), TransferError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(TransferError::InvalidPath),
        Ok(_) => Err(TransferError::DestinationExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("stat export", &error)),
    }
}

fn temporary_export_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".part.{}", std::process::id()));
    PathBuf::from(temporary)
}

fn io_error(operation: &'static str, error: &std::io::Error) -> TransferError {
    TransferError::Io {
        operation,
        message: error.to_string(),
    }
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_count(output: &mut Vec<u8>, value: usize) -> Result<(), TransferError> {
    write_u32(
        output,
        u32::try_from(value).map_err(|_| TransferError::BoundExceeded { what: "count" })?,
    );
    Ok(())
}

fn write_string(output: &mut Vec<u8>, value: &str) -> Result<(), TransferError> {
    validate_text(value)?;
    write_u32(
        output,
        u32::try_from(value.len()).map_err(|_| TransferError::BoundExceeded { what: "text" })?,
    );
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_backend_fingerprint(
    output: &mut Vec<u8>,
    backend: &BackendFingerprintV1,
) -> Result<(), TransferError> {
    output.push(backend.profile.tag());
    write_string(output, &backend.engine_identity)?;
    write_string(output, &backend.schema.logical_history_id)?;
    output.extend_from_slice(&backend.schema.schema_contract_fingerprint.as_bytes());
    write_count(output, backend.schema.migrations.len())?;
    for migration in &backend.schema.migrations {
        write_u32(output, migration.version);
        write_string(output, &migration.migration_id)?;
        output.extend_from_slice(&migration.checksum.as_bytes());
    }
    Ok(())
}

fn read_backend_fingerprint(
    reader: &mut Reader<'_>,
) -> Result<BackendFingerprintV1, TransferError> {
    let profile = BundleProfileV1::from_tag(reader.read_u8()?)?;
    let engine_identity = reader.read_string()?;
    let logical_history_id = reader.read_string()?;
    let schema_contract_fingerprint = reader.read_digest()?;
    let migration_count = reader.read_count()?;
    let mut migrations = Vec::with_capacity(migration_count);
    for _ in 0..migration_count {
        migrations.push(MigrationIdentityV1::new(
            reader.read_u32()?,
            reader.read_string()?,
            reader.read_digest()?,
        )?);
    }
    let schema = SchemaMigrationContractV1::new(
        logical_history_id,
        schema_contract_fingerprint,
        migrations,
    )?;
    BackendFingerprintV1::new(profile, engine_identity, schema)
}

fn write_record(output: &mut Vec<u8>, record: &LogicalRecordV1) -> Result<(), TransferError> {
    write_u64(output, record.ordinal);
    output.push(record.kind.class_tag());
    output.push(record.kind.kind_tag());
    write_string(output, &record.identity)?;
    write_u64(
        output,
        u64::try_from(record.bytes.len()).map_err(|_| TransferError::BoundExceeded {
            what: "record length",
        })?,
    );
    output.extend_from_slice(&record.bytes);
    output.extend_from_slice(&record.digest.as_bytes());
    Ok(())
}

fn write_parity(output: &mut Vec<u8>, parity: &RecordParityV1) -> Result<(), TransferError> {
    write_count(output, parity.record_count)?;
    write_count(output, parity.canonical_record_count)?;
    write_count(output, parity.derived_record_count)?;
    output.extend_from_slice(&parity.total_digest.as_bytes());
    output.extend_from_slice(&parity.canonical_digest.as_bytes());
    write_count(output, parity.deployment_record_count)?;
    output.extend_from_slice(&parity.deployment_digest.as_bytes());
    write_count(output, parity.rooms.len())?;
    for room in &parity.rooms {
        write_string(output, &room.room_id)?;
        write_count(output, room.record_count)?;
        output.extend_from_slice(&room.bytes_digest.as_bytes());
    }
    Ok(())
}

fn chunk_digest(start: usize, records: &[LogicalRecordV1]) -> Result<DigestV1, TransferError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"worldstream/transfer-chunk/v1");
    write_u64(
        &mut bytes,
        u64::try_from(start).map_err(|_| TransferError::BoundExceeded { what: "chunk" })?,
    );
    for record in records {
        write_record(&mut bytes, record)?;
    }
    Ok(DigestV1::hash(&bytes))
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], TransferError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(TransferError::UnexpectedEnd)?;
        if end > self.bytes.len() {
            return Err(TransferError::UnexpectedEnd);
        }
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn read_u8(&mut self) -> Result<u8, TransferError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, TransferError> {
        Ok(u16::from_le_bytes(
            self.read_exact(2)?
                .try_into()
                .map_err(|_| TransferError::UnexpectedEnd)?,
        ))
    }

    fn read_u32(&mut self) -> Result<u32, TransferError> {
        Ok(u32::from_le_bytes(
            self.read_exact(4)?
                .try_into()
                .map_err(|_| TransferError::UnexpectedEnd)?,
        ))
    }

    fn read_u64(&mut self) -> Result<u64, TransferError> {
        Ok(u64::from_le_bytes(
            self.read_exact(8)?
                .try_into()
                .map_err(|_| TransferError::UnexpectedEnd)?,
        ))
    }

    fn read_digest(&mut self) -> Result<DigestV1, TransferError> {
        DigestV1::from_bytes(self.read_exact(32)?)
    }

    fn read_string(&mut self) -> Result<String, TransferError> {
        let length = self.read_u32()?;
        let length =
            usize::try_from(length).map_err(|_| TransferError::BoundExceeded { what: "text" })?;
        let bytes = self.read_exact(length)?;
        let value = std::str::from_utf8(bytes).map_err(|_| TransferError::InvalidUtf8)?;
        validate_text(value)?;
        Ok(value.to_owned())
    }

    fn read_blob(&mut self, maximum: usize) -> Result<Vec<u8>, TransferError> {
        let length = self.read_u64()?;
        let length_usize = usize::try_from(length).map_err(|_| TransferError::LengthExceeded {
            what: "blob",
            length,
        })?;
        if length_usize > maximum {
            return Err(TransferError::LengthExceeded {
                what: "blob",
                length,
            });
        }
        Ok(self.read_exact(length_usize)?.to_vec())
    }

    fn read_count(&mut self) -> Result<usize, TransferError> {
        let count = usize::try_from(self.read_u32()?)
            .map_err(|_| TransferError::BoundExceeded { what: "count" })?;
        if count > MAX_RECORDS {
            return Err(TransferError::BoundExceeded { what: "count" });
        }
        Ok(count)
    }

    fn is_done(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::{
        BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DeploymentIdentityV1,
        DerivedRecordKindV1, DigestV1, LogicalRecordV1, MigrationIdentityV1, PackIdentityV1,
        ResourceIdentityV1, ResourceKindV1, SchemaMigrationContractV1, SessionStatePolicyV1,
        TargetFingerprintV1, TransferBundleV1, TransferCheckpointV1, TransferChunkDispositionV1,
        TransferDestinationV1, TransferError, TransferImportSessionV1, TransferLifecycleV1,
        TransferRunError, TransferScopeV1, TransferSourceAuthorityV1, TransferStateV1,
        VerifiedTargetAbortV1, VerifiedTargetFinalizationV1, WholeDeploymentTransferErrorV1,
        abort_whole_deployment, default_backend_fingerprint, finalize_whole_deployment,
    };

    #[allow(clippy::struct_excessive_bools)]
    #[derive(Default)]
    struct FixtureDestination {
        chunks: Vec<(usize, usize, DigestV1)>,
        records: Vec<LogicalRecordV1>,
        hydrated: bool,
        semantically_verified: bool,
        provider_verified_marker: bool,
        finalized: bool,
        authoritative: bool,
        aborted: bool,
        verification_calls: usize,
        finalization_calls: usize,
        abort_calls: usize,
        fail_hydration_once: bool,
        fail_replay_verification_once: bool,
        fail_verified_marker_once: bool,
        fail_reconfirmation_once: bool,
        fail_finalization_once: bool,
        fail_accept_once: bool,
        fail_abort_once: bool,
        events: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    #[derive(Default)]
    struct FixtureSource {
        state: Mutex<FixtureSourceState>,
        events: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    #[derive(Default)]
    struct FixtureSourceState {
        retired: Option<(DigestV1, DigestV1, u64, u64, String)>,
        restored: Option<(DigestV1, DigestV1, u64, u64, String)>,
        retire_calls: usize,
        restore_calls: usize,
        fail_retire_after_commit_once: bool,
        fail_restore_after_commit_once: bool,
    }

    impl TransferSourceAuthorityV1 for FixtureSource {
        type Error = String;

        fn retire_after_verified_target(
            &self,
            proof: &VerifiedTargetFinalizationV1,
        ) -> Result<(), Self::Error> {
            if let Some(events) = &self.events {
                events
                    .lock()
                    .map_err(|_| "source event lock".to_owned())?
                    .push("retire source");
            }
            let binding = (
                proof.bundle_hash(),
                proof.target_fingerprint(),
                proof.source_epoch(),
                proof.target_epoch(),
                proof.lineage_id().to_owned(),
            );
            let mut state = self.state.lock().map_err(|_| "source lock".to_owned())?;
            state.retire_calls += 1;
            if state.restored.is_some() || state.retired.as_ref().is_some_and(|old| old != &binding)
            {
                return Err("source retirement binding mismatch".to_owned());
            }
            state.retired = Some(binding);
            if state.fail_retire_after_commit_once {
                state.fail_retire_after_commit_once = false;
                return Err("source retirement result lost after commit".to_owned());
            }
            Ok(())
        }

        fn restore_after_verified_abort(
            &self,
            proof: &VerifiedTargetAbortV1,
        ) -> Result<(), Self::Error> {
            let binding = (
                proof.bundle_hash(),
                proof.target_fingerprint(),
                proof.source_epoch(),
                proof.target_epoch(),
                proof.lineage_id().to_owned(),
            );
            let mut state = self.state.lock().map_err(|_| "source lock".to_owned())?;
            state.restore_calls += 1;
            if state.retired.is_some() || state.restored.as_ref().is_some_and(|old| old != &binding)
            {
                return Err("source abort binding mismatch".to_owned());
            }
            state.restored = Some(binding);
            if state.fail_restore_after_commit_once {
                state.fail_restore_after_commit_once = false;
                return Err("source restoration result lost after commit".to_owned());
            }
            Ok(())
        }
    }

    #[test]
    fn deployment_identity_encoding_is_canonical_and_explicitly_empty_safe() {
        let identity = DeploymentIdentityV1::new(
            vec![
                PackIdentityV1::new("pack-b", "r2", DigestV1::hash(b"b")).expect("pack"),
                PackIdentityV1::new("pack-a", "r1", DigestV1::hash(b"a")).expect("pack"),
            ],
            Vec::new(),
        )
        .expect("identity");
        assert!(identity.resources().is_empty());
        let bytes = identity.canonical_bytes().expect("bytes");
        assert_eq!(
            DeploymentIdentityV1::from_canonical_bytes(&bytes).expect("round trip"),
            identity
        );
        assert_eq!(identity.digest(), DigestV1::hash(&bytes));

        let mut tampered = bytes.clone();
        *tampered.last_mut().expect("non-empty") ^= 1;
        assert!(DeploymentIdentityV1::from_canonical_bytes(&tampered).is_err());
        tampered.push(0);
        assert!(DeploymentIdentityV1::from_canonical_bytes(&tampered).is_err());
    }

    #[test]
    fn deployment_identity_rejects_duplicate_pack_and_resource_rows() {
        let pack = PackIdentityV1::new("pack", "r1", DigestV1::hash(b"pack")).expect("pack");
        assert!(matches!(
            DeploymentIdentityV1::new(vec![pack.clone(), pack], Vec::new()),
            Err(TransferError::DuplicateIdentity { what: "pack" })
        ));
        let resource =
            ResourceIdentityV1::from_bytes(ResourceKindV1::Artifact, "artifact", b"bytes")
                .expect("resource");
        assert!(matches!(
            DeploymentIdentityV1::new(
                vec![PackIdentityV1::new("pack", "r1", DigestV1::hash(b"pack")).expect("pack")],
                vec![resource.clone(), resource],
            ),
            Err(TransferError::DuplicateIdentity { what: "resource" })
        ));
        let cross_kind = [
            ResourceIdentityV1::from_bytes(ResourceKindV1::Artifact, "shared-id", b"artifact")
                .expect("artifact"),
            ResourceIdentityV1::from_bytes(ResourceKindV1::Schema, "shared-id", b"schema")
                .expect("schema"),
        ];
        assert!(matches!(
            DeploymentIdentityV1::new(
                vec![PackIdentityV1::new("pack", "r1", DigestV1::hash(b"pack")).expect("pack")],
                cross_kind.into_iter().collect(),
            ),
            Err(TransferError::DuplicateIdentity { what: "resource" })
        ));
    }

    impl TransferDestinationV1 for FixtureDestination {
        type Error = String;

        fn apply_chunk(
            &mut self,
            chunk: &super::TransferChunkV1,
        ) -> Result<TransferChunkDispositionV1, Self::Error> {
            if self.aborted {
                return Err("target import is durably aborted".to_owned());
            }
            let identity = (chunk.start(), chunk.end(), chunk.digest());
            if self.chunks.contains(&identity) {
                return Ok(TransferChunkDispositionV1::AlreadyApplied);
            }
            self.chunks.push(identity);
            self.records.extend_from_slice(chunk.records());
            Ok(TransferChunkDispositionV1::Applied)
        }

        fn verify_complete(
            &mut self,
            bundle: &TransferBundleV1,
            target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if let Some(events) = &self.events {
                events
                    .lock()
                    .map_err(|_| "destination event lock".to_owned())?
                    .push("hydrate and verify target");
            }
            self.verification_calls += 1;
            bundle
                .verify_target(target)
                .map_err(|error| error.to_string())?;
            if self.records != bundle.records {
                return Err("destination records differ from bundle".to_owned());
            }
            if self.provider_verified_marker {
                if !self.hydrated || !self.semantically_verified {
                    return Err("stale provider verified marker".to_owned());
                }
                return Ok(());
            }
            if self.fail_hydration_once {
                self.fail_hydration_once = false;
                return Err("target hydration failed before commit".to_owned());
            }
            self.hydrated = true;
            if self.fail_replay_verification_once {
                self.fail_replay_verification_once = false;
                // Model provider transaction rollback: hydration is not
                // durable when the semantic replay gate fails.
                self.hydrated = false;
                self.semantically_verified = false;
                return Err("target replay verification failed before commit".to_owned());
            }
            self.semantically_verified = true;
            if self.fail_verified_marker_once {
                self.fail_verified_marker_once = false;
                // Model one provider transaction containing hydration,
                // executable replay, and the verified marker. Losing the
                // marker write rolls every target mutation back together.
                self.hydrated = false;
                self.semantically_verified = false;
                return Err("target verified marker failed before commit".to_owned());
            }
            self.provider_verified_marker = true;
            Ok(())
        }

        fn record_finalization(
            &mut self,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if let Some(events) = &self.events {
                events
                    .lock()
                    .map_err(|_| "destination event lock".to_owned())?
                    .push("persist verified target");
            }
            self.finalization_calls += 1;
            if !self.hydrated || !self.semantically_verified || !self.provider_verified_marker {
                return Err("target finalization before semantic verification".to_owned());
            }
            if self.fail_reconfirmation_once {
                self.fail_reconfirmation_once = false;
                return Err("target verification reconfirmation failed before commit".to_owned());
            }
            if self.fail_finalization_once {
                self.fail_finalization_once = false;
                return Err("target finalization failed before commit".to_owned());
            }
            self.finalized = true;
            Ok(())
        }

        fn accept_target_write(
            &mut self,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if let Some(events) = &self.events {
                events
                    .lock()
                    .map_err(|_| "destination event lock".to_owned())?
                    .push("publish target authority");
            }
            if !self.finalized
                || !self.hydrated
                || !self.semantically_verified
                || !self.provider_verified_marker
            {
                return Err("target write before verified finalization".to_owned());
            }
            self.authoritative = true;
            if self.fail_accept_once {
                self.fail_accept_once = false;
                return Err("target authority result lost after commit".to_owned());
            }
            Ok(())
        }

        fn abort_import(&mut self, _target: &TargetFingerprintV1) -> Result<(), Self::Error> {
            if self.finalized {
                return Err("rollback refused after finalization".to_owned());
            }
            self.abort_calls += 1;
            if self.fail_abort_once {
                self.fail_abort_once = false;
                return Err("target abort failed before commit".to_owned());
            }
            self.chunks.clear();
            self.records.clear();
            self.hydrated = false;
            self.semantically_verified = false;
            self.provider_verified_marker = false;
            self.aborted = true;
            Ok(())
        }
    }

    fn fixture_bundle() -> Result<TransferBundleV1, TransferError> {
        let pack = PackIdentityV1::new("fixture-pack", "revision-1", DigestV1::hash(b"pack"))?;
        let resources = vec![ResourceIdentityV1::from_bytes(
            ResourceKindV1::Artifact,
            "artifact/fixture",
            b"artifact bytes",
        )?];
        resources[0].verify_bytes(b"artifact bytes")?;
        assert!(resources[0].verify_bytes(b"changed bytes").is_err());
        let records = vec![
            LogicalRecordV1::canonical(
                0,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/fixture",
                b"lineage bytes\0preserved",
            )?,
            LogicalRecordV1::canonical(
                1,
                CanonicalRecordKindV1::RoomGenesis,
                "room/fixture/genesis",
                b"genesis bytes",
            )?,
            LogicalRecordV1::derived(
                2,
                DerivedRecordKindV1::PairedSnapshot,
                "room/fixture/snapshot",
                b"derived snapshot",
            )?,
        ];
        TransferBundleV1::new(
            "bundle/fixture",
            "lineage/fixture",
            7,
            BundleProfileV1::SqliteBundled,
            BundleProfileV1::PostgresPrimary17,
            pack,
            resources,
            SessionStatePolicyV1::InvalidateAndRebuild,
            records,
        )
    }

    fn fixture_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "worldstream-transfer-{name}-{}-{}.bundle",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    fn complete_fixture_import(
        bundle: &TransferBundleV1,
        destination: &mut FixtureDestination,
    ) -> Result<TransferImportSessionV1, TransferError> {
        let target = TargetFingerprintV1::for_bundle(bundle)?;
        let mut session = TransferImportSessionV1::begin(bundle)?;
        session.verify_target(&target)?;
        session
            .publish_chunks(destination, &[bundle.chunk(0, bundle.records().len())?])
            .map_err(|_| TransferError::InvalidValue {
                what: "complete fixture import",
            })?;
        Ok(session)
    }

    #[test]
    fn bundle_round_trip_is_deterministic_and_byte_preserving() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let encoded = bundle.to_bytes()?;
        assert_eq!(encoded, bundle.to_bytes()?);

        let decoded = TransferBundleV1::from_bytes(&encoded)?;
        assert_eq!(decoded, bundle);
        assert_eq!(decoded.records()[0].bytes(), b"lineage bytes\0preserved");
        assert_eq!(decoded.bundle_hash()?, bundle.bundle_hash()?);
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn canonical_export_bundle_exercises_lifecycle_without_global_scope()
    -> Result<(), TransferError> {
        let records = vec![
            LogicalRecordV1::canonical(
                0,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/lineage",
                b"lineage/fixture",
            )?,
            LogicalRecordV1::canonical(
                1,
                CanonicalRecordKindV1::StorageEpoch,
                "deployment/epoch",
                b"7",
            )?,
            LogicalRecordV1::canonical(
                2,
                CanonicalRecordKindV1::RoomGenesis,
                "room/fixture/genesis",
                b"genesis",
            )?,
            LogicalRecordV1::canonical(
                3,
                CanonicalRecordKindV1::RoomHead,
                "room/fixture/head",
                b"head",
            )?,
            LogicalRecordV1::canonical(
                4,
                CanonicalRecordKindV1::CoreMaterialization,
                "room/fixture/core",
                b"core",
            )?,
            LogicalRecordV1::canonical(
                5,
                CanonicalRecordKindV1::ActivityMaterialization,
                "room/fixture/activity",
                b"activity",
            )?,
            LogicalRecordV1::canonical(
                6,
                CanonicalRecordKindV1::ArtifactMetadata,
                "room/fixture/pack-revision-lock",
                b"exact pack lock",
            )?,
        ];
        let target_backend = default_backend_fingerprint(BundleProfileV1::PostgresPrimary17)?;
        let bundle = TransferBundleV1::from_canonical_export(
            "canonical-export/fixture",
            "lineage/fixture",
            7,
            target_backend,
            vec!["isolated-room".to_owned()],
            records,
        )?;
        assert_eq!(bundle.scope(), TransferScopeV1::CanonicalExport);
        assert!(bundle.source_backend().is_none());
        assert!(bundle.pack().is_none());
        assert!(bundle.resources().is_empty());
        assert_eq!(bundle.isolated_rooms(), &["isolated-room"]);

        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let encoded = bundle.to_bytes()?;
        assert_eq!(TransferBundleV1::from_bytes(&encoded)?, bundle);
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination {
            chunks: Vec::new(),
            records: Vec::new(),
            finalized: false,
            authoritative: false,
            aborted: false,
            ..FixtureDestination::default()
        };
        let mut chunks = Vec::new();
        for start in (0..bundle.records().len()).step_by(2) {
            chunks.push(bundle.chunk(start, (start + 2).min(bundle.records().len()))?);
        }
        session
            .publish_chunks(&mut destination, &chunks)
            .map_err(|error| {
                let _ = error;
                TransferError::InvalidValue {
                    what: "canonical export lifecycle fixture",
                }
            })?;
        session
            .verify_canonical_export(&mut destination, &bundle)
            .map_err(|error| {
                let _ = error;
                TransferError::InvalidValue {
                    what: "canonical export verification fixture",
                }
            })?;
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        assert!(!destination.finalized);
        assert!(!destination.authoritative);
        let persisted = session.to_bytes()?;
        let mut restarted = TransferImportSessionV1::from_bytes(&bundle, &persisted)?;
        let mut forged = persisted.clone();
        forged[8 + 2 + 32 + 8] = TransferStateV1::Finalized.tag();
        assert!(matches!(
            TransferImportSessionV1::from_bytes(&bundle, &forged),
            Err(TransferError::CanonicalExportNotAuthoritative)
        ));
        assert!(matches!(
            restarted.finalize(&mut destination, &bundle),
            Err(TransferRunError::Contract(
                TransferError::CanonicalExportNotAuthoritative
            ))
        ));
        assert!(matches!(
            restarted.accept_target_write(&mut destination),
            Err(TransferRunError::Contract(
                TransferError::CanonicalExportNotAuthoritative
            ))
        ));
        assert!(!destination.finalized);
        assert!(!destination.authoritative);
        Ok(())
    }

    #[test]
    fn parity_binds_deployment_rooms_schema_and_backend_identity() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let parity = bundle.parity()?;
        assert_eq!(parity.record_count(), 3);
        assert_eq!(parity.canonical_record_count(), 2);
        assert_eq!(parity.derived_record_count(), 1);
        assert_eq!(parity.deployment_record_count(), 1);
        assert_eq!(parity.rooms().len(), 1);
        assert_eq!(parity.rooms()[0].room_id(), "fixture");

        let decoded = TransferBundleV1::from_bytes(&bundle.to_bytes()?)?;
        assert_eq!(decoded.parity()?, parity);
        assert_eq!(decoded.source_backend(), bundle.source_backend());
        assert_eq!(decoded.target_backend(), bundle.target_backend());
        assert_eq!(
            decoded.target_backend().schema().digest()?,
            bundle.target_backend().schema().digest()?
        );

        let wrong_schema = SchemaMigrationContractV1::new(
            "worldstream-storage-v1",
            DigestV1::hash(b"different-schema"),
            vec![MigrationIdentityV1::new(
                1,
                "transfer-contract-default",
                DigestV1::hash(b"different-migration"),
            )?],
        )?;
        let wrong_backend = BackendFingerprintV1::new(
            BundleProfileV1::PostgresPrimary17,
            "postgresql-17",
            wrong_schema,
        )?;
        let observed = TargetFingerprintV1::observed(
            BundleProfileV1::PostgresPrimary17,
            8,
            bundle.lineage_id(),
            wrong_backend,
            bundle.pack().cloned().ok_or(TransferError::InvalidValue {
                what: "fixture pack",
            })?,
            bundle.resources().to_vec(),
            bundle.bundle_hash()?,
            parity,
        )?;
        assert!(matches!(
            bundle.verify_target(&observed),
            Err(TransferError::TargetMismatch {
                what: "backend fingerprint/schema/migrations"
            })
        ));
        Ok(())
    }

    #[test]
    fn record_order_is_canonical_and_mismatch_fails_closed() -> Result<(), TransferError> {
        let pack = PackIdentityV1::new("pack", "revision", DigestV1::hash(b"pack"))?;
        let records = vec![
            LogicalRecordV1::canonical(
                0,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/z",
                b"z",
            )?,
            LogicalRecordV1::canonical(
                1,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/a",
                b"a",
            )?,
        ];
        assert!(matches!(
            TransferBundleV1::new(
                "bundle",
                "lineage",
                1,
                BundleProfileV1::SqliteBundled,
                BundleProfileV1::PostgresPrimary17,
                pack,
                Vec::new(),
                SessionStatePolicyV1::InvalidateAndRebuild,
                records,
            ),
            Err(TransferError::NonDeterministicOrder { what: "records" })
        ));
        Ok(())
    }

    #[test]
    fn file_export_import_is_bounded_atomic_and_deterministic() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let path = fixture_path("round-trip");
        let _ = std::fs::remove_file(&path);
        let expected_hash = bundle.bundle_hash()?;
        assert_eq!(bundle.export_to_path(&path)?, expected_hash);
        let imported = TransferBundleV1::import_from_path(&path)?;
        assert_eq!(imported, bundle);
        assert_eq!(imported.bundle_hash()?, expected_hash);
        assert!(matches!(
            bundle.export_to_path(&path),
            Err(TransferError::DestinationExists)
        ));
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn unknown_wire_values_fail_closed() {
        assert!(matches!(
            TransferBundleV1::from_bytes(b"WSTRANS1\x63\x00"),
            Err(TransferError::UnsupportedBundleVersion(99))
        ));
        assert!(matches!(
            LogicalRecordV1::decode_kind(1, 255),
            Err(TransferError::UnknownRecordKind(255))
        ));
        assert!(matches!(
            LogicalRecordV1::decode_kind(255, 1),
            Err(TransferError::UnknownRecordClass(255))
        ));
    }

    #[test]
    fn checkpoint_resume_is_idempotent_and_rejects_mismatch() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let mut checkpoint = TransferCheckpointV1::initial(&bundle)?;
        let first = bundle.chunk(0, 2)?;
        assert!(!checkpoint.apply(&first)?);
        assert!(checkpoint.apply(&first)?);

        let wrong = bundle.chunk(0, 1)?;
        assert!(matches!(
            checkpoint.apply(&wrong),
            Err(TransferError::CheckpointMismatch { .. })
        ));

        let persisted = checkpoint.to_bytes()?;
        let mut resumed = TransferCheckpointV1::from_bytes(&persisted)?;
        resumed.apply(&bundle.chunk(2, 3)?)?;
        assert!(resumed.is_complete(&bundle));
        Ok(())
    }

    #[test]
    fn target_fingerprint_requires_exact_bundle_identities() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        bundle.verify_target(&target)?;

        let wrong_pack =
            PackIdentityV1::new("fixture-pack", "revision-2", DigestV1::hash(b"pack"))?;
        let wrong_target = TargetFingerprintV1::with_pack(&target, wrong_pack)?;
        assert!(matches!(
            bundle.verify_target(&wrong_target),
            Err(TransferError::TargetMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn target_fingerprint_digest_is_deterministic_and_pack_bound() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        assert_eq!(target.fingerprint_digest()?, target.fingerprint_digest()?);
        let wrong_pack =
            PackIdentityV1::new("fixture-pack", "revision-2", DigestV1::hash(b"pack"))?;
        let wrong_target = TargetFingerprintV1::with_pack(&target, wrong_pack)?;
        assert_ne!(
            target.fingerprint_digest()?,
            wrong_target.fingerprint_digest()?
        );
        Ok(())
    }

    #[test]
    fn import_session_restarts_after_destination_commit_and_finalizes() -> Result<(), TransferError>
    {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination::default();
        let first = bundle.chunk(0, 2)?;

        // Model a provider commit followed by a process stop before the
        // transfer checkpoint was persisted. The retry must be durable-safe.
        assert_eq!(
            destination
                .apply_chunk(&first)
                .map_err(|_| TransferError::InvalidValue {
                    what: "fixture destination"
                })?,
            TransferChunkDispositionV1::Applied
        );
        assert_eq!(
            session.apply_chunk(&mut destination, &first).map_err(|_| {
                TransferError::InvalidValue {
                    what: "import retry",
                }
            })?,
            TransferChunkDispositionV1::AlreadyApplied
        );

        let persisted = session.to_bytes()?;
        let mut resumed = TransferImportSessionV1::from_bytes(&bundle, &persisted)?;
        let second = bundle.chunk(2, 3)?;
        assert_eq!(
            resumed
                .apply_chunk(&mut destination, &second)
                .map_err(|_| TransferError::InvalidValue {
                    what: "import continuation"
                })?,
            TransferChunkDispositionV1::Applied
        );
        assert!(resumed.is_complete());
        resumed
            .finalize(&mut destination, &bundle)
            .map_err(|_| TransferError::InvalidValue {
                what: "import finalization",
            })?;
        assert_eq!(resumed.state(), TransferStateV1::Finalized);
        assert!(destination.finalized);

        let persisted = resumed.to_bytes()?;
        let mut finalized = TransferImportSessionV1::from_bytes(&bundle, &persisted)?;
        finalized
            .accept_target_write(&mut destination)
            .map_err(|_| TransferError::InvalidValue {
                what: "target authority",
            })?;
        assert_eq!(finalized.state(), TransferStateV1::TargetAuthoritative);
        assert!(destination.authoritative);
        assert_eq!(destination.records, bundle.records);
        Ok(())
    }

    #[test]
    fn persisted_checkpoint_reconfirms_or_repairs_missing_destination_chunk()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination::default();
        let first = bundle.chunk(0, 2)?;
        session
            .apply_chunk(&mut destination, &first)
            .map_err(|_| TransferError::InvalidValue {
                what: "initial chunk",
            })?;
        let persisted = session.to_bytes()?;

        // The checkpoint survived, but the destination rows did not. A retry
        // must call the destination seam and repair the missing chunk rather
        // than trusting the local checkpoint as proof of durable publication.
        destination.chunks.clear();
        destination.records.clear();
        let mut resumed = TransferImportSessionV1::from_bytes(&bundle, &persisted)?;
        assert_eq!(
            resumed.apply_chunk(&mut destination, &first).map_err(|_| {
                TransferError::InvalidValue {
                    what: "checkpoint replay",
                }
            })?,
            TransferChunkDispositionV1::Applied
        );
        assert_eq!(destination.records, first.records());
        Ok(())
    }

    #[test]
    fn import_session_abort_cleans_destination_before_source_rollback() -> Result<(), TransferError>
    {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination::default();
        session
            .apply_chunk(&mut destination, &bundle.chunk(0, 2)?)
            .map_err(|_| TransferError::InvalidValue {
                what: "partial import",
            })?;

        let source = FixtureSource::default();
        abort_whole_deployment(&mut session, &mut destination, &source, &bundle).map_err(|_| {
            TransferError::InvalidValue {
                what: "import abort",
            }
        })?;
        assert_eq!(session.state(), TransferStateV1::SourceAuthoritative);
        assert!(destination.aborted);
        assert!(destination.chunks.is_empty());
        assert!(destination.records.is_empty());
        assert!(
            source
                .state
                .lock()
                .expect("source state")
                .restored
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn empty_import_abort_tombstones_before_source_proof_and_rejects_stale_publisher()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let pending_checkpoint = session.to_bytes()?;
        let mut destination = FixtureDestination::default();
        let source = FixtureSource::default();

        // No destination chunk or import marker exists. The provider abort
        // must nevertheless complete before the coordinator can mint and
        // deliver the source-restoration proof.
        abort_whole_deployment(&mut session, &mut destination, &source, &bundle).map_err(|_| {
            TransferError::InvalidValue {
                what: "empty import abort",
            }
        })?;
        assert_eq!(session.state(), TransferStateV1::SourceAuthoritative);
        assert!(destination.aborted);
        assert_eq!(destination.abort_calls, 1);
        assert_eq!(source.state.lock().expect("source state").restore_calls, 1);

        // A publisher resumed from the pre-abort checkpoint must consult the
        // destination and be rejected by the durable tombstone.
        let mut stale_publisher =
            TransferImportSessionV1::from_bytes(&bundle, &pending_checkpoint)?;
        assert!(matches!(
            stale_publisher.apply_chunk(&mut destination, &bundle.chunk(0, 1)?),
            Err(TransferRunError::Destination {
                operation: "apply chunk",
                error,
            }) if error == "target import is durably aborted"
        ));

        // Replaying the exact abort remains idempotent and reconfirms the same
        // provider tombstone before replaying the same source binding.
        abort_whole_deployment(&mut stale_publisher, &mut destination, &source, &bundle).map_err(
            |_| TransferError::InvalidValue {
                what: "empty import abort retry",
            },
        )?;
        assert_eq!(
            stale_publisher.state(),
            TransferStateV1::SourceAuthoritative
        );
        assert_eq!(destination.abort_calls, 2);
        let source_state = source.state.lock().expect("source state");
        assert_eq!(source_state.restore_calls, 2);
        assert!(source_state.restored.is_some());
        Ok(())
    }

    #[test]
    fn whole_deployment_finalize_retries_lost_source_commit_after_restart()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let mut destination = FixtureDestination::default();
        let mut session = complete_fixture_import(&bundle, &mut destination)?;
        let pending_bytes = session.to_bytes()?;
        let source = FixtureSource::default();
        source
            .state
            .lock()
            .expect("source state")
            .fail_retire_after_commit_once = true;

        assert!(matches!(
            finalize_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Source {
                operation: "retire verified source",
                ..
            })
        ));
        assert_eq!(session.state(), TransferStateV1::Finalized);
        assert!(destination.finalized);
        assert!(!destination.authoritative);
        assert!(source.state.lock().expect("source state").retired.is_some());

        // Model a process stop before the mutated in-memory session could be
        // persisted. The old target-verified checkpoint must safely replay
        // both durable provider and source transitions.
        let mut resumed = TransferImportSessionV1::from_bytes(&bundle, &pending_bytes)?;
        finalize_whole_deployment(&mut resumed, &mut destination, &source, &bundle).map_err(
            |_| TransferError::InvalidValue {
                what: "resumed whole-deployment finalization",
            },
        )?;
        assert_eq!(resumed.state(), TransferStateV1::TargetAuthoritative);
        assert!(destination.authoritative);
        let source_state = source.state.lock().expect("source state");
        assert_eq!(source_state.retire_calls, 2);
        assert_eq!(destination.finalization_calls, 2);
        Ok(())
    }

    #[test]
    fn whole_deployment_finalize_orders_target_before_source_and_reconfirms_lost_first_write()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let mut destination = FixtureDestination {
            fail_finalization_once: true,
            ..FixtureDestination::default()
        };
        let mut session = complete_fixture_import(&bundle, &mut destination)?;
        let source = FixtureSource::default();

        assert!(matches!(
            finalize_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Destination {
                operation: "record finalization",
                ..
            })
        ));
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        assert_eq!(source.state.lock().expect("source state").retire_calls, 0);

        destination.fail_accept_once = true;
        assert!(matches!(
            finalize_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Destination {
                operation: "accept target authority",
                ..
            })
        ));
        assert_eq!(session.state(), TransferStateV1::Finalized);
        assert!(destination.authoritative);
        let finalized_bytes = session.to_bytes()?;

        let mut resumed = TransferImportSessionV1::from_bytes(&bundle, &finalized_bytes)?;
        finalize_whole_deployment(&mut resumed, &mut destination, &source, &bundle).map_err(
            |_| TransferError::InvalidValue {
                what: "resumed target authority confirmation",
            },
        )?;
        assert_eq!(resumed.state(), TransferStateV1::TargetAuthoritative);
        assert_eq!(source.state.lock().expect("source state").retire_calls, 2);
        assert_eq!(destination.finalization_calls, 3);
        Ok(())
    }

    #[test]
    fn whole_deployment_hydrates_and_persists_proof_before_retirement_then_only_publishes()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut destination = FixtureDestination {
            events: Some(Arc::clone(&events)),
            ..FixtureDestination::default()
        };
        let mut session = complete_fixture_import(&bundle, &mut destination)?;
        events.lock().expect("event log").clear();
        let source = FixtureSource {
            events: Some(Arc::clone(&events)),
            ..FixtureSource::default()
        };

        finalize_whole_deployment(&mut session, &mut destination, &source, &bundle).map_err(
            |_| TransferError::InvalidValue {
                what: "ordered whole-deployment finalization",
            },
        )?;

        assert_eq!(session.state(), TransferStateV1::TargetAuthoritative);
        assert_eq!(
            events.lock().expect("event log").as_slice(),
            [
                "hydrate and verify target",
                "persist verified target",
                "retire source",
                "publish target authority",
            ]
        );
        Ok(())
    }

    fn assert_pre_retirement_destination_failure_is_abortable(
        mut destination: FixtureDestination,
        expected_operation: &'static str,
    ) -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let mut session = complete_fixture_import(&bundle, &mut destination)?;
        let source = FixtureSource::default();

        assert!(matches!(
            finalize_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Destination {
                operation,
                ..
            }) if operation == expected_operation
        ));
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        if expected_operation == "record finalization" {
            // The exact hydrated/replayed provider marker is durable, but the
            // retirement-authorizing confirmation is not. It remains a
            // non-serving, abortable import.
            assert!(destination.hydrated);
            assert!(destination.semantically_verified);
            assert!(destination.provider_verified_marker);
        } else {
            assert!(!destination.hydrated);
            assert!(!destination.semantically_verified);
        }
        assert!(!destination.finalized);
        assert!(!destination.authoritative);
        assert_eq!(
            destination.finalization_calls,
            usize::from(expected_operation == "record finalization")
        );
        {
            let source_state = source.state.lock().expect("source state");
            assert_eq!(source_state.retire_calls, 0);
            assert!(source_state.retired.is_none());
        }

        abort_whole_deployment(&mut session, &mut destination, &source, &bundle).map_err(|_| {
            TransferError::InvalidValue {
                what: "abort after pre-retirement destination failure",
            }
        })?;
        assert_eq!(session.state(), TransferStateV1::SourceAuthoritative);
        assert!(destination.aborted);
        assert!(
            source
                .state
                .lock()
                .expect("source state")
                .restored
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn whole_deployment_hydration_failure_keeps_source_pending_and_abort_legal()
    -> Result<(), TransferError> {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                fail_hydration_once: true,
                ..FixtureDestination::default()
            },
            "verify complete import",
        )
    }

    #[test]
    fn whole_deployment_replay_failure_keeps_source_pending_and_abort_legal()
    -> Result<(), TransferError> {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                fail_replay_verification_once: true,
                ..FixtureDestination::default()
            },
            "verify complete import",
        )
    }

    #[test]
    fn whole_deployment_verified_marker_failure_keeps_abort_legal() -> Result<(), TransferError> {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                fail_verified_marker_once: true,
                ..FixtureDestination::default()
            },
            "verify complete import",
        )
    }

    #[test]
    fn whole_deployment_reconfirmation_failure_keeps_abort_legal() -> Result<(), TransferError> {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                fail_reconfirmation_once: true,
                ..FixtureDestination::default()
            },
            "record finalization",
        )
    }

    #[test]
    fn whole_deployment_finalization_marker_failure_keeps_abort_legal() -> Result<(), TransferError>
    {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                fail_finalization_once: true,
                ..FixtureDestination::default()
            },
            "record finalization",
        )
    }

    #[test]
    fn whole_deployment_stale_verified_marker_cannot_retire_source() -> Result<(), TransferError> {
        assert_pre_retirement_destination_failure_is_abortable(
            FixtureDestination {
                // Models a marker written by the former staged-chunks-only gate.
                provider_verified_marker: true,
                hydrated: false,
                semantically_verified: false,
                ..FixtureDestination::default()
            },
            "verify complete import",
        )
    }

    #[test]
    fn whole_deployment_abort_orders_destination_before_source_and_retries_lost_source_commit()
    -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let pending_bytes = session.to_bytes()?;
        let mut destination = FixtureDestination {
            fail_abort_once: true,
            ..FixtureDestination::default()
        };
        let source = FixtureSource::default();

        assert!(matches!(
            abort_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Destination {
                operation: "abort import",
                ..
            })
        ));
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        assert_eq!(source.state.lock().expect("source state").restore_calls, 0);

        source
            .state
            .lock()
            .expect("source state")
            .fail_restore_after_commit_once = true;
        assert!(matches!(
            abort_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Source {
                operation: "restore verified source",
                ..
            })
        ));
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        assert!(destination.aborted);
        assert!(
            source
                .state
                .lock()
                .expect("source state")
                .restored
                .is_some()
        );

        let mut resumed = TransferImportSessionV1::from_bytes(&bundle, &pending_bytes)?;
        abort_whole_deployment(&mut resumed, &mut destination, &source, &bundle).map_err(|_| {
            TransferError::InvalidValue {
                what: "resumed whole-deployment abort",
            }
        })?;
        assert_eq!(resumed.state(), TransferStateV1::SourceAuthoritative);
        assert_eq!(source.state.lock().expect("source state").restore_calls, 2);
        assert_eq!(destination.abort_calls, 3);
        Ok(())
    }

    #[test]
    fn import_session_does_not_finalize_unverified_destination() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination::default();
        session
            .apply_chunk(&mut destination, &bundle.chunk(0, 2)?)
            .map_err(|_| TransferError::InvalidValue {
                what: "first verification chunk",
            })?;
        session
            .apply_chunk(&mut destination, &bundle.chunk(2, 3)?)
            .map_err(|_| TransferError::InvalidValue {
                what: "second verification chunk",
            })?;

        let complete_records = destination.records.clone();
        destination.records.clear();
        assert!(session.finalize(&mut destination, &bundle).is_err());
        assert_eq!(session.state(), TransferStateV1::TargetVerified);
        assert!(!destination.finalized);

        destination.records = complete_records;
        session
            .finalize(&mut destination, &bundle)
            .map_err(|_| TransferError::InvalidValue {
                what: "verified finalization",
            })?;
        assert_eq!(session.state(), TransferStateV1::Finalized);
        Ok(())
    }

    #[test]
    fn destination_refuses_post_write_rollback() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let mut session = TransferImportSessionV1::begin(&bundle)?;
        session.verify_target(&target)?;
        let mut destination = FixtureDestination::default();
        session
            .publish_chunks(
                &mut destination,
                &[bundle.chunk(0, 2)?, bundle.chunk(2, 3)?],
            )
            .map_err(|_| TransferError::InvalidValue {
                what: "fixture publication",
            })?;
        session
            .finalize(&mut destination, &bundle)
            .map_err(|_| TransferError::InvalidValue {
                what: "fixture finalization",
            })?;
        assert!(destination.abort_import(&target).is_err());
        assert_eq!(destination.records, bundle.records);
        let source = FixtureSource::default();
        assert!(matches!(
            abort_whole_deployment(&mut session, &mut destination, &source, &bundle),
            Err(WholeDeploymentTransferErrorV1::Contract(
                TransferError::InvalidLifecycleState {
                    operation: "abort whole deployment",
                    state: TransferStateV1::Finalized,
                }
            ))
        ));
        Ok(())
    }

    #[test]
    fn lifecycle_aborts_before_retirement_and_fences_epochs() -> Result<(), TransferError> {
        let bundle = fixture_bundle()?;
        let mut lifecycle = TransferLifecycleV1::begin(&bundle)?;
        assert_eq!(lifecycle.state(), TransferStateV1::TransferPending);
        assert!(
            lifecycle
                .check_write(BundleProfileV1::SqliteBundled, 7)
                .is_err()
        );

        lifecycle.abort()?;
        assert_eq!(lifecycle.state(), TransferStateV1::SourceAuthoritative);
        lifecycle.check_write(BundleProfileV1::SqliteBundled, 7)?;

        let mut lifecycle = TransferLifecycleV1::begin(&bundle)?;
        lifecycle.verify_target(&TargetFingerprintV1::for_bundle(&bundle)?)?;
        lifecycle.finalize()?;
        assert!(lifecycle.abort().is_err());
        assert!(
            lifecycle
                .check_write(BundleProfileV1::SqliteBundled, 7)
                .is_err()
        );
        let wrong_pack =
            PackIdentityV1::new("fixture-pack", "revision-2", DigestV1::hash(b"pack"))?;
        let wrong_target =
            TargetFingerprintV1::with_pack(&TargetFingerprintV1::for_bundle(&bundle)?, wrong_pack)?;
        assert!(
            lifecycle
                .accept_target_write_checked(&wrong_target)
                .is_err()
        );
        lifecycle.accept_target_write_checked(&TargetFingerprintV1::for_bundle(&bundle)?)?;
        lifecycle.check_write(BundleProfileV1::PostgresPrimary17, 8)?;
        assert!(
            lifecycle
                .check_write(BundleProfileV1::PostgresPrimary17, 7)
                .is_err()
        );
        Ok(())
    }
}
