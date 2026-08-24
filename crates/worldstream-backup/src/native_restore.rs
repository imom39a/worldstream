//! Provider-neutral evidence contract for native restore verification.
//!
//! A provider adapter owns the mechanics of reading a native restore target.
//! It can implement [`NativeRestoreEvidenceAdapter`] and expose an immutable
//! [`NativeRestoreEvidenceV1`] value to this module.  The verifier only reads
//! that value; it never opens a provider connection, publishes a restore,
//! repairs a Room, changes readiness outside the returned report, or performs
//! any other side effect.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_core::CanonicalJsonV1;

use crate::{
    BackendNativePointV1, BackendProfileV1, BackupImageV1, CanonicalRecordKindV1,
    ConsistencyClassV1, DigestV1, IntegrityWitnessV1, MAX_SAFE_INTEGER, MigrationContractV1,
    PackIdentityV1, ResourceIdentityV1, RoomDispositionV1, VerificationReportV1, VerifierLimits,
    verify_restore,
};

/// Fixed provider-neutral durable domains required for a `PostgreSQL` native
/// restore to qualify as full semantic evidence.
pub const POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1: [NativeRestoreDurableDomainV1; 34] = [
    NativeRestoreDurableDomainV1::SchemaMigrations,
    NativeRestoreDurableDomainV1::OperationGuards,
    NativeRestoreDurableDomainV1::ExternalInputPreparations,
    NativeRestoreDurableDomainV1::RoomRoots,
    NativeRestoreDurableDomainV1::Genesis,
    NativeRestoreDurableDomainV1::Materializations,
    NativeRestoreDurableDomainV1::MemberDeliveryState,
    NativeRestoreDurableDomainV1::Timers,
    NativeRestoreDurableDomainV1::Transitions,
    NativeRestoreDurableDomainV1::Frames,
    NativeRestoreDurableDomainV1::ObservationConsequences,
    NativeRestoreDurableDomainV1::ActivationDecisions,
    NativeRestoreDurableDomainV1::ActivationIntents,
    NativeRestoreDurableDomainV1::ActivationOperationReceipts,
    NativeRestoreDurableDomainV1::SemanticReceipts,
    NativeRestoreDurableDomainV1::IntegrityIncidents,
    NativeRestoreDurableDomainV1::AuthorityFences,
    NativeRestoreDurableDomainV1::RetiredAuthorityFences,
    NativeRestoreDurableDomainV1::AuthorityState,
    NativeRestoreDurableDomainV1::AuthorityPrincipals,
    NativeRestoreDurableDomainV1::AuthorityRunners,
    NativeRestoreDurableDomainV1::AuthorityCapabilities,
    NativeRestoreDurableDomainV1::AuthorityCapabilityScopes,
    NativeRestoreDurableDomainV1::AuthorityRunnerCapabilityMemberships,
    NativeRestoreDurableDomainV1::AuthorityChangeReceipts,
    NativeRestoreDurableDomainV1::AuthorityAudit,
    NativeRestoreDurableDomainV1::TransferImports,
    NativeRestoreDurableDomainV1::TransferChunks,
    NativeRestoreDurableDomainV1::TransferTargetFence,
    NativeRestoreDurableDomainV1::DeploymentMetadata,
    NativeRestoreDurableDomainV1::DeploymentIdentityMetadata,
    NativeRestoreDurableDomainV1::DeploymentPackIdentities,
    NativeRestoreDurableDomainV1::DeploymentResourceIdentities,
    NativeRestoreDurableDomainV1::DeploymentResourceBlobs,
];

/// Returns the separate transport bound for one canonical durable-domain row.
///
/// `PostgreSQL`'s canonical `jsonb` projection renders each raw `bytea` byte as
/// two hexadecimal characters and one row can contain several independently
/// bounded payloads. The semantic object bound therefore cannot also be used
/// as the encoded-row bound: doing so rejects a valid object above roughly
/// half the advertised size. Eight payload expansions plus fixed JSON framing
/// remains finite while admitting every currently modeled multi-payload row.
#[must_use]
pub const fn max_native_restore_canonical_row_bytes(max_object_bytes: usize) -> usize {
    max_object_bytes.saturating_mul(8).saturating_add(64 * 1024)
}

/// One exact durable ledger domain captured on both sides of a native restore.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRestoreDurableDomainV1 {
    /// Full forward-only migration rows including lineage/fingerprint columns.
    SchemaMigrations,
    /// Operation identities, request hashes, Rooms, and indeterminate receipts.
    OperationGuards,
    /// `ExternalInput` identities, request hashes, and first sampled Recorded Times.
    ExternalInputPreparations,
    /// Current Room Heads and integrity state/generation.
    RoomRoots,
    /// Exact retained Pack lock and Genesis bytes.
    Genesis,
    /// Current Core and Activity materialization bytes.
    Materializations,
    /// Complete Membership delivery head, floor, Cursor, and reset state.
    MemberDeliveryState,
    /// Timer identities, generations, schedules, payloads, and states.
    Timers,
    /// Exact ordered Transition bytes.
    Transitions,
    /// Exact addressed Frame payloads and hashes.
    Frames,
    /// Durable reset/visibility consequences and their payload/hash witnesses.
    ObservationConsequences,
    /// Canonical Activation decisions that created zero or more intents.
    ActivationDecisions,
    /// Activation intent, lease, runner, context, and generation state.
    ActivationIntents,
    /// Activation operation identities, request hashes, results, and contexts.
    ActivationOperationReceipts,
    /// Room operation identities, request hashes, semantic inputs/times, and results.
    SemanticReceipts,
    /// Ordered operational Room-integrity incidents and generations.
    IntegrityIncidents,
    /// Core authority witnesses and their active generation fences.
    AuthorityFences,
    /// Exact archived source authority fences retained across migration.
    RetiredAuthorityFences,
    /// Singleton authority initialization state.
    AuthorityState,
    /// Durable principals, statuses, and generations.
    AuthorityPrincipals,
    /// Durable Runner owners, statuses, and generations.
    AuthorityRunners,
    /// Capability targets, revocations, expiry, and authority generations.
    AuthorityCapabilities,
    /// Exact capability scopes.
    AuthorityCapabilityScopes,
    /// Runner capability Room/Membership targets.
    AuthorityRunnerCapabilityMemberships,
    /// Idempotent authority mutation receipts.
    AuthorityChangeReceipts,
    /// Ordered immutable authority audit facts.
    AuthorityAudit,
    /// Offline transfer state and target fingerprint.
    TransferImports,
    /// Exact resumable transfer chunk evidence.
    TransferChunks,
    /// Target-wide transfer identity fence.
    TransferTargetFence,
    /// Target-wide deployment lineage and storage epoch bytes/value.
    DeploymentMetadata,
    /// Canonical deployment identity bytes and component-set hashes.
    DeploymentIdentityMetadata,
    /// Exact installed Pack identity rows.
    DeploymentPackIdentities,
    /// Exact installed resource identity rows.
    DeploymentResourceIdentities,
    /// Exact content-addressed resource payload bytes and their digests.
    DeploymentResourceBlobs,
}

impl NativeRestoreDurableDomainV1 {
    /// Stable diagnostic/report label for this domain.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaMigrations => "schema_migrations",
            Self::OperationGuards => "operation_guards",
            Self::ExternalInputPreparations => "external_input_preparations",
            Self::RoomRoots => "room_roots",
            Self::Genesis => "genesis",
            Self::Materializations => "materializations",
            Self::MemberDeliveryState => "member_delivery_state",
            Self::Timers => "timers",
            Self::Transitions => "transitions",
            Self::Frames => "frames",
            Self::ObservationConsequences => "observation_consequences",
            Self::ActivationDecisions => "activation_decisions",
            Self::ActivationIntents => "activation_intents",
            Self::ActivationOperationReceipts => "activation_operation_receipts",
            Self::SemanticReceipts => "semantic_receipts",
            Self::IntegrityIncidents => "integrity_incidents",
            Self::AuthorityFences => "authority_fences",
            Self::RetiredAuthorityFences => "retired_authority_fences",
            Self::AuthorityState => "authority_state",
            Self::AuthorityPrincipals => "authority_principals",
            Self::AuthorityRunners => "authority_runners",
            Self::AuthorityCapabilities => "authority_capabilities",
            Self::AuthorityCapabilityScopes => "authority_capability_scopes",
            Self::AuthorityRunnerCapabilityMemberships => "authority_runner_capability_memberships",
            Self::AuthorityChangeReceipts => "authority_change_receipts",
            Self::AuthorityAudit => "authority_audit",
            Self::TransferImports => "transfer_imports",
            Self::TransferChunks => "transfer_chunks",
            Self::TransferTargetFence => "transfer_target_fence",
            Self::DeploymentMetadata => "deployment_metadata",
            Self::DeploymentIdentityMetadata => "deployment_identity_metadata",
            Self::DeploymentPackIdentities => "deployment_pack_identities",
            Self::DeploymentResourceIdentities => "deployment_resource_identities",
            Self::DeploymentResourceBlobs => "deployment_resource_blobs",
        }
    }
}

/// One bounded canonical database row and its exact BLAKE3 digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRestoreCanonicalRowV1 {
    /// Canonical JSON array containing every modeled column in fixed order.
    pub canonical_bytes: Vec<u8>,
    /// Hash of `canonical_bytes`.
    pub digest: crate::DigestV1,
}

impl NativeRestoreCanonicalRowV1 {
    /// Constructs a row witness from adapter-canonicalized bytes.
    #[must_use]
    pub fn new(canonical_bytes: Vec<u8>) -> Self {
        let digest = crate::DigestV1::hash(&canonical_bytes);
        Self {
            canonical_bytes,
            digest,
        }
    }
}

/// Exact source/restored inventory for one durable domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRestoreDurableDomainEvidenceV1 {
    /// Typed domain identity.
    pub domain: NativeRestoreDurableDomainV1,
    /// Declared source row count, checked against `source_rows`.
    pub source_row_count: u64,
    /// Declared restored row count, checked against `restored_rows`.
    pub restored_row_count: u64,
    /// Digest over the typed domain and ordered source row witnesses.
    pub source_digest: crate::DigestV1,
    /// Digest over the typed domain and ordered restored row witnesses.
    pub restored_digest: crate::DigestV1,
    /// Ordered exact source rows.
    pub source_rows: Vec<NativeRestoreCanonicalRowV1>,
    /// Ordered exact restored rows.
    pub restored_rows: Vec<NativeRestoreCanonicalRowV1>,
}

impl NativeRestoreDurableDomainEvidenceV1 {
    /// Builds the complete count/digest binding for one domain.
    #[must_use]
    pub fn new(
        domain: NativeRestoreDurableDomainV1,
        source_rows: Vec<NativeRestoreCanonicalRowV1>,
        restored_rows: Vec<NativeRestoreCanonicalRowV1>,
    ) -> Self {
        Self {
            domain,
            source_row_count: source_rows.len() as u64,
            restored_row_count: restored_rows.len() as u64,
            source_digest: durable_domain_digest(domain, &source_rows),
            restored_digest: durable_domain_digest(domain, &restored_rows),
            source_rows,
            restored_rows,
        }
    }
}

fn durable_domain_digest(
    domain: NativeRestoreDurableDomainV1,
    rows: &[NativeRestoreCanonicalRowV1],
) -> crate::DigestV1 {
    let mut bytes = b"worldstream/native-restore-durable-domain/v1\0".to_vec();
    bytes.extend_from_slice(domain.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&(rows.len() as u64).to_be_bytes());
    for row in rows {
        bytes.extend_from_slice(&(row.canonical_bytes.len() as u64).to_be_bytes());
        bytes.extend_from_slice(row.digest.as_str().as_bytes());
    }
    crate::DigestV1::hash(&bytes)
}

/// The target-side metadata and membership witness supplied by a native
/// restore adapter.
///
/// `Option` is intentional.  A native adapter can represent an unavailable
/// query or an older target without inventing a value.  Every missing field is
/// a blocking verification failure.  The verifier compares every present
/// value to the immutable manifest and the typed image rather than trusting an
/// adapter's assertion.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRestoreTargetEvidenceV1 {
    /// Exact backend profile observed at the restored target.
    pub backend: Option<BackendProfileV1>,
    /// Explicit deployment lineage observed at the restored target.
    pub deployment_lineage: Option<String>,
    /// Storage epoch observed at the restored target.
    pub storage_epoch: Option<u64>,
    /// Exact native engine/build and restore-point identity observed at the target.
    pub native_point: Option<BackendNativePointV1>,
    /// Exact logical migration and schema contract observed at the target.
    pub migration_contract: Option<MigrationContractV1>,
    /// Exact Activity Pack identities installed at the target.
    pub pack_identities: Option<Vec<PackIdentityV1>>,
    /// Exact content-addressed resource identities observed at the target.
    pub resource_identities: Option<Vec<ResourceIdentityV1>>,
    /// Complete source/target integrity membership for every restored Room.
    pub room_membership: Option<Vec<NativeRestoreRoomMembershipV1>>,
    /// Exact source/restored durable operational-domain inventory.
    ///
    /// `PostgreSQL` full-semantic readiness requires the fixed inventory in
    /// [`POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1`]. `None` is retained for
    /// adapters whose native contract does not yet claim that coverage.
    pub durable_domains: Option<Vec<NativeRestoreDurableDomainEvidenceV1>>,
}

/// Source and target integrity evidence for one Room in a native restore.
///
/// The membership list is an explicit set, not a count.  This lets the
/// provider adapter preserve an old isolated Room while proving that every
/// unrelated healthy Room is present and independently verifiable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRestoreRoomMembershipV1 {
    /// Stable Room identity.
    pub room_id: String,
    /// Exact source/restored status, generations, and isolation witness.
    pub integrity: IntegrityWitnessV1,
}

/// Complete immutable evidence handed from a native adapter to the verifier.
///
/// `image` contains exact canonical record bytes, their stored digests, full
/// materializations, resource bytes, and operational ledgers.  `target`
/// contains the adapter-observed target metadata that cannot be inferred from
/// those bytes alone, such as the storage epoch and complete Room membership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRestoreEvidenceV1 {
    /// Immutable manifest and all typed target evidence.
    pub image: BackupImageV1,
    /// Independent target-side metadata and membership witness.
    pub target: NativeRestoreTargetEvidenceV1,
}

/// Deployment lifecycle projected from an immutable restore verification.
///
/// This is deliberately a pure state machine.  It does not install a
/// database, run catch-up, edit canonical history, or change a provider's
/// readiness.  A caller supplies the catch-up barrier and generation fence
/// after performing those operations in its own adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RestoreLifecycleStateV1 {
    /// Evidence has been verified and the caller has not started catch-up.
    Loading,
    /// The caller is replaying or otherwise catching up from the restore point.
    CatchingUp,
    /// Global evidence is verified and the caller has satisfied catch-up.
    Active,
    /// Global evidence is missing or invalid; serving must remain disabled.
    Faulted,
    /// No healthy Room is available, while preserved isolated Rooms remain explicit.
    Quarantined,
}

/// Per-Room lifecycle projection.  A quarantined Room never poisons unrelated
/// verified Rooms in the same deployment projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RestoreRoomLifecycleStateV1 {
    /// Room evidence is verified but catch-up has not completed.
    Loading,
    /// Room evidence is verified and the deployment is catching up.
    CatchingUp,
    /// Room is eligible to serve after the deployment becomes active.
    Active,
    /// Room is unavailable because its global or newly introduced evidence failed.
    Faulted,
    /// A pre-existing isolated Room was preserved without repair.
    Quarantined,
}

/// Pure caller-provided witness that a restore commit is durable before actor
/// installation.  The commit identity is opaque to this crate and is never
/// copied into diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestoreCommitWitnessV1 {
    /// Generation at which the commit became durable.
    pub generation: u64,
    /// Opaque caller-owned commit identity.
    pub commit_id: String,
}

/// Result of an idempotent post-commit installation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RestoreInstallOutcomeV1 {
    /// The installation witness was recorded for the first time.
    Installed,
    /// The same installation was already recorded; no second installation is required.
    AlreadyInstalled,
}

/// Deterministic errors for the provider-neutral restore lifecycle projection.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RestoreLifecycleErrorV1 {
    /// A zero generation is never a valid fence.
    #[error("restore lifecycle generation must be nonzero")]
    ZeroGeneration,
    /// The caller used a stale or future generation fence.
    #[error("restore lifecycle generation fence rejected")]
    GenerationFence,
    /// The requested operation is not valid in the current state.
    #[error("restore lifecycle transition is invalid")]
    InvalidTransition,
    /// Catch-up has not supplied a satisfied barrier.
    #[error("restore catch-up barrier is not satisfied")]
    CatchUpBarrierUnsatisfied,
    /// Installation requires a durable commit witness first.
    #[error("restore installation requires a durable commit witness")]
    CommitNotDurable,
    /// A second commit identity cannot replace the first one.
    #[error("restore commit identity mismatch")]
    CommitIdentityMismatch,
    /// Commit identities must be bounded and non-empty.
    #[error("restore commit identity is invalid")]
    InvalidCommitIdentity,
}

/// Provider-neutral lifecycle projection over one immutable verification report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RestoreLifecycleProjectionV1 {
    report: VerificationReportV1,
    state: RestoreLifecycleStateV1,
    generation: u64,
    rooms: BTreeMap<String, RestoreRoomLifecycleStateV1>,
    durable_commit: Option<RestoreCommitWitnessV1>,
    installed_commit: Option<RestoreCommitWitnessV1>,
}

impl RestoreLifecycleProjectionV1 {
    /// Starts a projection in `Loading` for a valid generation.
    ///
    /// A blocking verification report starts in `Faulted`.  A report with
    /// permitted pre-existing isolated Rooms remains usable for healthy Rooms.
    ///
    /// # Errors
    ///
    /// Returns [`RestoreLifecycleErrorV1::ZeroGeneration`] for generation zero.
    pub fn from_report(
        report: VerificationReportV1,
        generation: u64,
    ) -> Result<Self, RestoreLifecycleErrorV1> {
        if generation == 0 {
            return Err(RestoreLifecycleErrorV1::ZeroGeneration);
        }
        let state = if report.is_ready() {
            RestoreLifecycleStateV1::Loading
        } else {
            RestoreLifecycleStateV1::Faulted
        };
        let rooms = report
            .rooms
            .iter()
            .map(|(room_id, disposition)| {
                let room_state = match disposition {
                    RoomDispositionV1::Verified => {
                        if report.is_ready() {
                            RestoreRoomLifecycleStateV1::Loading
                        } else {
                            RestoreRoomLifecycleStateV1::Faulted
                        }
                    }
                    RoomDispositionV1::IsolatedPreExisting => {
                        RestoreRoomLifecycleStateV1::Quarantined
                    }
                    RoomDispositionV1::Blocked => RestoreRoomLifecycleStateV1::Faulted,
                };
                (room_id.clone(), room_state)
            })
            .collect();
        Ok(Self {
            report,
            state,
            generation,
            rooms,
            durable_commit: None,
            installed_commit: None,
        })
    }

    /// Returns the immutable report used to construct this projection.
    #[must_use]
    pub const fn report(&self) -> &VerificationReportV1 {
        &self.report
    }

    /// Returns the current global lifecycle state.
    #[must_use]
    pub const fn state(&self) -> RestoreLifecycleStateV1 {
        self.state
    }

    /// Returns the current generation fence.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the projected state for a Room, if it was present in the report.
    #[must_use]
    pub fn room_state(&self, room_id: &str) -> Option<RestoreRoomLifecycleStateV1> {
        self.rooms.get(room_id).copied()
    }

    /// Begins caller-owned catch-up from `Loading`.
    ///
    /// # Errors
    ///
    /// Returns a generation-fence or invalid-transition error when the caller
    /// is stale or the projection is not loading.
    pub fn begin_catch_up(&mut self, generation: u64) -> Result<(), RestoreLifecycleErrorV1> {
        self.check_generation(generation)?;
        if self.state != RestoreLifecycleStateV1::Loading {
            return Err(RestoreLifecycleErrorV1::InvalidTransition);
        }
        self.state = RestoreLifecycleStateV1::CatchingUp;
        for room_state in self.rooms.values_mut() {
            if *room_state == RestoreRoomLifecycleStateV1::Loading {
                *room_state = RestoreRoomLifecycleStateV1::CatchingUp;
            }
        }
        Ok(())
    }

    /// Completes catch-up only when the caller's barrier is satisfied.
    ///
    /// # Errors
    ///
    /// Returns a generation-fence, invalid-transition, or unsatisfied-barrier
    /// error when the supplied witness cannot activate the projection.
    pub fn complete_catch_up(
        &mut self,
        generation: u64,
        barrier_satisfied: bool,
    ) -> Result<(), RestoreLifecycleErrorV1> {
        self.check_generation(generation)?;
        if self.state != RestoreLifecycleStateV1::CatchingUp {
            return Err(RestoreLifecycleErrorV1::InvalidTransition);
        }
        if !barrier_satisfied {
            return Err(RestoreLifecycleErrorV1::CatchUpBarrierUnsatisfied);
        }
        self.state = if self
            .rooms
            .values()
            .all(|state| *state == RestoreRoomLifecycleStateV1::Quarantined)
        {
            RestoreLifecycleStateV1::Quarantined
        } else {
            RestoreLifecycleStateV1::Active
        };
        for room_state in self.rooms.values_mut() {
            if *room_state == RestoreRoomLifecycleStateV1::CatchingUp {
                *room_state = RestoreRoomLifecycleStateV1::Active;
            }
        }
        Ok(())
    }

    /// Passivates the projection and advances its generation fence.
    ///
    /// # Errors
    ///
    /// Returns a generation-fence error for stale/non-advancing generations or
    /// an invalid-transition error when the projection is not active.
    pub fn passivate(
        &mut self,
        generation: u64,
        next_generation: u64,
    ) -> Result<(), RestoreLifecycleErrorV1> {
        self.check_generation(generation)?;
        if next_generation <= generation {
            return Err(RestoreLifecycleErrorV1::GenerationFence);
        }
        if !matches!(
            self.state,
            RestoreLifecycleStateV1::Active | RestoreLifecycleStateV1::Quarantined
        ) {
            return Err(RestoreLifecycleErrorV1::InvalidTransition);
        }
        self.generation = next_generation;
        self.state = RestoreLifecycleStateV1::Loading;
        for room_state in self.rooms.values_mut() {
            if *room_state == RestoreRoomLifecycleStateV1::Active {
                *room_state = RestoreRoomLifecycleStateV1::Loading;
            }
        }
        self.durable_commit = None;
        self.installed_commit = None;
        Ok(())
    }

    /// Records a durable commit before actor installation.
    ///
    /// # Errors
    ///
    /// Returns a generation-fence, invalid-identity, or commit-identity error
    /// when the witness is stale, malformed, or conflicts with an existing one.
    pub fn record_durable_commit(
        &mut self,
        generation: u64,
        commit_id: impl Into<String>,
    ) -> Result<(), RestoreLifecycleErrorV1> {
        self.check_generation(generation)?;
        let commit_id = commit_id.into();
        if commit_id.is_empty() || commit_id.len() > 512 {
            return Err(RestoreLifecycleErrorV1::InvalidCommitIdentity);
        }
        let witness = RestoreCommitWitnessV1 {
            generation,
            commit_id,
        };
        if let Some(existing) = &self.durable_commit {
            if existing != &witness {
                return Err(RestoreLifecycleErrorV1::CommitIdentityMismatch);
            }
            return Ok(());
        }
        self.durable_commit = Some(witness);
        Ok(())
    }

    /// Records actor installation after the matching durable commit exactly once.
    ///
    /// # Errors
    ///
    /// Returns a generation-fence or commit-not-durable error unless the exact
    /// durable commit witness was recorded first.
    pub fn install_after_commit(
        &mut self,
        generation: u64,
        commit_id: impl Into<String>,
    ) -> Result<RestoreInstallOutcomeV1, RestoreLifecycleErrorV1> {
        self.check_generation(generation)?;
        let commit_id = commit_id.into();
        let witness = RestoreCommitWitnessV1 {
            generation,
            commit_id,
        };
        if self.installed_commit.as_ref() == Some(&witness) {
            return Ok(RestoreInstallOutcomeV1::AlreadyInstalled);
        }
        if self.durable_commit.as_ref() != Some(&witness) {
            return Err(RestoreLifecycleErrorV1::CommitNotDurable);
        }
        self.installed_commit = Some(witness);
        Ok(RestoreInstallOutcomeV1::Installed)
    }

    fn check_generation(&self, generation: u64) -> Result<(), RestoreLifecycleErrorV1> {
        if generation == 0 || generation != self.generation {
            return Err(RestoreLifecycleErrorV1::GenerationFence);
        }
        Ok(())
    }
}

impl NativeRestoreEvidenceV1 {
    /// Creates an evidence value without validating it.
    ///
    /// Validation is deliberately deferred to [`verify_native_restore`] so an
    /// adapter can report missing evidence explicitly and the verifier can
    /// return one bounded, actionable report.
    #[must_use]
    pub const fn new(image: BackupImageV1, target: NativeRestoreTargetEvidenceV1) -> Self {
        Self { image, target }
    }
}

/// Read-only adapter seam for provider-specific native restore evidence.
///
/// Implementations belong outside this crate.  The implementation should
/// capture evidence before calling the verifier and return the same immutable
/// value for the duration of verification; this trait contains no write or
/// provider API requirement.
pub trait NativeRestoreEvidenceAdapter {
    /// Returns the immutable evidence captured from the restored target.
    fn native_restore_evidence(&self) -> &NativeRestoreEvidenceV1;
}

impl NativeRestoreEvidenceAdapter for NativeRestoreEvidenceV1 {
    fn native_restore_evidence(&self) -> &NativeRestoreEvidenceV1 {
        self
    }
}

/// Verifies provider-neutral native restore evidence without side effects.
///
/// The existing immutable image verifier performs the full canonical and
/// operational checks.  This boundary adds the target-side evidence that a
/// provider adapter must supply: exact backend/point and storage epoch,
/// migration/schema contract, pack/resource identity sets, and complete Room
/// membership.  Any missing, duplicate, conflicting, or changed evidence
/// leaves the returned report not ready.
#[must_use]
pub fn verify_native_restore(
    evidence: &NativeRestoreEvidenceV1,
    limits: VerifierLimits,
) -> VerificationReportV1 {
    let mut report = verify_restore(&evidence.image, limits);
    check_target_metadata(evidence, &mut report, limits);
    check_pack_identities(evidence, &mut report, limits);
    check_resource_identities(evidence, &mut report, limits);
    check_room_membership(evidence, &mut report, limits);
    check_durable_domains(evidence, &mut report, limits);
    check_exact_canonical_evidence(evidence, &mut report, limits);
    report
}

fn check_durable_domains(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    if evidence.image.manifest.backend != BackendProfileV1::PostgresPrimary {
        return;
    }
    let Some(domains) = evidence.target.durable_domains.as_ref() else {
        block(
            report,
            limits,
            ConsistencyClassV1::OperationalRelation,
            "native_restore_durable_domains_missing",
            "durable operational domains",
            "recapture every required Activation, authority, and transfer-fence domain",
        );
        return;
    };
    if domains.len() != POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1.len() {
        block(
            report,
            limits,
            ConsistencyClassV1::OperationalRelation,
            "native_restore_durable_domain_inventory_mismatch",
            "durable operational domains",
            "restore the fixed complete durable-domain inventory exactly once",
        );
    }
    let mut observed = BTreeSet::new();
    let mut total_rows = 0usize;
    for domain in domains {
        let mut invalid = !observed.insert(domain.domain);
        let source_count = usize::try_from(domain.source_row_count).ok();
        let restored_count = usize::try_from(domain.restored_row_count).ok();
        invalid |= source_count != Some(domain.source_rows.len())
            || restored_count != Some(domain.restored_rows.len())
            || domain.source_rows != domain.restored_rows
            || domain.source_digest != domain.restored_digest
            || durable_domain_digest(domain.domain, &domain.source_rows) != domain.source_digest
            || durable_domain_digest(domain.domain, &domain.restored_rows)
                != domain.restored_digest;
        total_rows = total_rows.saturating_add(domain.source_rows.len());
        let mut source_identities = BTreeSet::new();
        let mut restored_identities = BTreeSet::new();
        let max_canonical_row_bytes =
            max_native_restore_canonical_row_bytes(limits.max_object_bytes);
        invalid |= domain.source_rows.iter().any(|row| {
            row.canonical_bytes.is_empty()
                || row.canonical_bytes.len() > max_canonical_row_bytes
                || CanonicalJsonV1::from_canonical_bytes(&row.canonical_bytes).is_err()
                || crate::DigestV1::hash(&row.canonical_bytes) != row.digest
                || !source_identities.insert(row.digest.clone())
        });
        invalid |= domain.restored_rows.iter().any(|row| {
            row.canonical_bytes.is_empty()
                || row.canonical_bytes.len() > max_canonical_row_bytes
                || CanonicalJsonV1::from_canonical_bytes(&row.canonical_bytes).is_err()
                || crate::DigestV1::hash(&row.canonical_bytes) != row.digest
                || !restored_identities.insert(row.digest.clone())
        });
        if invalid {
            block(
                report,
                limits,
                ConsistencyClassV1::OperationalRelation,
                "native_restore_durable_domain_mismatch",
                domain.domain.as_str(),
                "discard the target and restore the exact ordered source rows, counts, and hashes",
            );
        }
    }
    if total_rows > limits.max_ledger_rows {
        block(
            report,
            limits,
            ConsistencyClassV1::OperationalRelation,
            "native_restore_durable_domain_bound_exceeded",
            "durable operational domains",
            "restore within the reviewed aggregate durable-row bound",
        );
    }
    let required = POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
        .into_iter()
        .collect::<BTreeSet<_>>();
    if observed != required {
        block(
            report,
            limits,
            ConsistencyClassV1::OperationalRelation,
            "native_restore_durable_domain_inventory_mismatch",
            "durable operational domains",
            "recapture every required typed domain without omissions or additions",
        );
    }
}

/// Verifies evidence supplied through a provider-specific read-only adapter.
///
/// This helper is intentionally generic and has no dependency on a concrete
/// database crate.  The adapter remains responsible for collecting evidence;
/// this function only invokes the pure verifier over the returned value.
#[must_use]
pub fn verify_native_restore_from_adapter<A: NativeRestoreEvidenceAdapter>(
    adapter: &A,
    limits: VerifierLimits,
) -> VerificationReportV1 {
    verify_native_restore(adapter.native_restore_evidence(), limits)
}

#[allow(clippy::too_many_lines)]
fn check_target_metadata(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    let image = &evidence.image;
    let target = &evidence.target;

    if let Some(backend) = target.backend {
        if backend != image.manifest.backend {
            block(
                report,
                limits,
                ConsistencyClassV1::Manifest,
                "native_restore_backend_mismatch",
                "backend identity",
                "restore into the backend profile named by the immutable manifest",
            );
        }
    } else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_metadata_missing",
            "backend identity",
            "recapture the exact target backend identity before readiness",
        );
    }

    if let Some(storage_epoch) = target.storage_epoch {
        if !(1..=MAX_SAFE_INTEGER).contains(&storage_epoch)
            || storage_epoch != image.manifest.storage_epoch
        {
            block(
                report,
                limits,
                ConsistencyClassV1::Manifest,
                "native_restore_storage_epoch_mismatch",
                "storage epoch",
                "discard the target and restore under the manifest's fenced storage epoch",
            );
        }
    } else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_metadata_missing",
            "storage epoch",
            "recapture the target storage epoch and repeat verification",
        );
    }

    if let Some(deployment_lineage) = target.deployment_lineage.as_ref() {
        if deployment_lineage != &image.manifest.deployment_lineage {
            block(
                report,
                limits,
                ConsistencyClassV1::Manifest,
                "native_restore_deployment_lineage_mismatch",
                "deployment lineage",
                "restore the exact deployment lineage recorded by the source",
            );
        }
    } else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_metadata_missing",
            "deployment lineage",
            "recapture the target deployment lineage before readiness",
        );
    }

    if let Some(native_point) = target.native_point.as_ref() {
        if native_point != &image.manifest.native_point
            || native_point != &image.restored_native_point
        {
            block(
                report,
                limits,
                ConsistencyClassV1::Manifest,
                "native_restore_point_mismatch",
                "native restore point",
                "restore the exact engine identity and native point recorded by the manifest",
            );
        }
    } else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_metadata_missing",
            "native restore point",
            "recapture the exact backend-native restore point before readiness",
        );
    }

    if let Some(migration_contract) = target.migration_contract.as_ref() {
        if migration_contract != &image.manifest.migration_contract
            || migration_contract != &image.migrations
        {
            block(
                report,
                limits,
                ConsistencyClassV1::Migration,
                "native_restore_migration_mismatch",
                "migration/schema contract",
                "restore the exact forward-only migration prefix and schema fingerprint",
            );
        }
    } else {
        block(
            report,
            limits,
            ConsistencyClassV1::Migration,
            "native_restore_metadata_missing",
            "migration/schema contract",
            "recapture the complete logical migration and schema contract",
        );
    }
}

fn check_pack_identities(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    let Some(pack_identities) = evidence.target.pack_identities.as_ref() else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_metadata_missing",
            "Activity Pack identities",
            "recapture every exact retained Activity Pack identity",
        );
        return;
    };
    let Some(actual) = unique_pack_map(pack_identities) else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_duplicate_pack_identity",
            "Activity Pack identities",
            "remove duplicate or conflicting Pack evidence and repeat restore",
        );
        return;
    };
    let Some(expected) = unique_pack_map(&evidence.image.manifest.expected_packs) else {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_duplicate_pack_identity",
            "Activity Pack identities",
            "remove duplicate or conflicting Pack evidence and repeat restore",
        );
        return;
    };
    if actual != expected {
        block(
            report,
            limits,
            ConsistencyClassV1::Manifest,
            "native_restore_pack_identity_mismatch",
            "Activity Pack identities",
            "restore exactly the Pack revisions and executor/schema/codec identities named by the manifest",
        );
    }
}

fn check_resource_identities(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    let Some(resource_identities) = evidence.target.resource_identities.as_ref() else {
        block(
            report,
            limits,
            ConsistencyClassV1::Resource,
            "native_restore_metadata_missing",
            "resource identities",
            "recapture every expected immutable resource identity",
        );
        return;
    };
    let Some(actual) = unique_resource_map(resource_identities) else {
        block(
            report,
            limits,
            ConsistencyClassV1::Resource,
            "native_restore_duplicate_resource_identity",
            "resource identities",
            "remove duplicate or conflicting resource evidence and repeat restore",
        );
        return;
    };
    let expected = identity_map(&evidence.image.manifest.expected_resources, |resource| {
        resource.resource_id.as_str()
    });
    if actual != expected {
        block(
            report,
            limits,
            ConsistencyClassV1::Resource,
            "native_restore_resource_identity_mismatch",
            "resource identities",
            "restore exactly the immutable resource identities named by the manifest",
        );
    }
}

fn check_room_membership(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    let Some(membership) = evidence.target.room_membership.as_ref() else {
        block(
            report,
            limits,
            ConsistencyClassV1::Integrity,
            "native_restore_metadata_missing",
            "Room membership",
            "recapture complete healthy and isolated Room membership",
        );
        return;
    };
    if membership.len() > limits.max_rooms {
        block(
            report,
            limits,
            ConsistencyClassV1::Integrity,
            "native_restore_room_bound_exceeded",
            "Room membership",
            "restore within the reviewed Room bound",
        );
        return;
    }

    let Some(actual) = unique_membership_map(membership) else {
        block(
            report,
            limits,
            ConsistencyClassV1::Integrity,
            "native_restore_duplicate_room_membership",
            "Room membership",
            "remove duplicate or conflicting Room membership evidence and repeat restore",
        );
        return;
    };
    let expected = evidence
        .image
        .rooms
        .iter()
        .map(|room| (room.room_id.clone(), room.integrity.clone()))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != evidence.image.rooms.len() || actual != expected {
        block(
            report,
            limits,
            ConsistencyClassV1::Integrity,
            "native_restore_room_membership_incomplete",
            "Room membership",
            "restore every Room exactly once, including pre-existing isolated Rooms",
        );
    }
}

fn check_exact_canonical_evidence(
    evidence: &NativeRestoreEvidenceV1,
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
) {
    for room in &evidence.image.rooms {
        if report.rooms.get(&room.room_id) == Some(&RoomDispositionV1::IsolatedPreExisting) {
            // The base verifier already bound an isolated Room's source and
            // restored raw-byte digests plus its exact integrity generation.
            // Decoding synthetic canonical records here would contradict the
            // isolation contract and could turn preserved corruption into a
            // deployment-wide readiness failure.
            continue;
        }
        if !exact_canonical_evidence(room, limits) {
            block(
                report,
                limits,
                ConsistencyClassV1::CanonicalRecord,
                "native_restore_canonical_evidence_mismatch",
                &room.room_id,
                "restore exact canonical Room bytes, digests, Head, and materializations",
            );
        }
    }
}

fn exact_canonical_evidence(room: &crate::RoomImageV1, limits: VerifierLimits) -> bool {
    if room.records.is_empty() || room.records.len() > limits.max_records_per_room {
        return false;
    }
    let mut previous = None;
    for (index, record) in room.records.iter().enumerate() {
        let expected_kind = if index == 0 {
            CanonicalRecordKindV1::Genesis
        } else {
            CanonicalRecordKindV1::Transition
        };
        if record.kind != expected_kind
            || record.room_seq != index as u64
            || record.bytes.len() > limits.max_object_bytes
            || !crate::canonical_record_hash_matches(record)
            || record.previous_digest != previous
        {
            return false;
        }
        previous = Some(record.digest.clone());
    }
    let Some(last_digest) = previous else {
        return false;
    };
    room.head.room_id == room.room_id
        && room.head.room_seq == room.records.len().saturating_sub(1) as u64
        && room.head.lineage_digest == last_digest
        && room.materialization.core_state_bytes.len() <= limits.max_object_bytes
        && room.materialization.activity_state_bytes.len() <= limits.max_object_bytes
        && room.materialization.authoritative_state_bytes.len() <= limits.max_object_bytes
        && crate::materialization_hash_matches(room)
}

fn unique_pack_map(
    values: &[PackIdentityV1],
) -> Option<BTreeMap<(String, DigestV1), PackIdentityV1>> {
    let mut result = BTreeMap::new();
    for value in values {
        let key = (value.pack_id.clone(), value.revision_digest.clone());
        if result.insert(key, value.clone()).is_some() {
            return None;
        }
    }
    Some(result)
}

fn unique_resource_map(
    values: &[ResourceIdentityV1],
) -> Option<BTreeMap<String, ResourceIdentityV1>> {
    unique_identity_map(values, |resource| resource.resource_id.clone())
}

fn unique_identity_map<T, F>(values: &[T], key: F) -> Option<BTreeMap<String, T>>
where
    T: Clone,
    F: Fn(&T) -> String,
{
    let mut result = BTreeMap::new();
    for value in values {
        if result.insert(key(value), value.clone()).is_some() {
            return None;
        }
    }
    Some(result)
}

fn identity_map<T, F>(values: &[T], key: F) -> BTreeMap<String, T>
where
    T: Clone,
    F: Fn(&T) -> &str,
{
    values
        .iter()
        .map(|value| (key(value).to_owned(), value.clone()))
        .collect()
}

fn unique_membership_map(
    values: &[NativeRestoreRoomMembershipV1],
) -> Option<BTreeMap<String, IntegrityWitnessV1>> {
    unique_identity_map(values, |membership| membership.room_id.clone()).map(|map| {
        map.into_iter()
            .map(|(id, item)| (id, item.integrity))
            .collect()
    })
}

fn block(
    report: &mut VerificationReportV1,
    limits: VerifierLimits,
    class: ConsistencyClassV1,
    code: &str,
    subject: &str,
    action: &str,
) {
    report.add_blocking_diagnostic(limits.max_diagnostics, class, code, subject, action);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BACKUP_MANIFEST_SCHEMA_V1, BackupManifestV1, CanonicalRecordV1, CompleteHeadV1, DigestV1,
        IntegrityStatusV1, MaterializationV1, MigrationIdentityV1, ResourceBlobV1, RoomImageV1,
    };

    const PACK_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const GLOBAL_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn digest(value: &str) -> DigestV1 {
        DigestV1::parse(value.to_owned()).unwrap_or_else(|_| DigestV1::hash(value.as_bytes()))
    }

    fn migration_contract() -> MigrationContractV1 {
        MigrationContractV1 {
            logical_history_id: "worldstream-storage-v1".to_owned(),
            schema_contract_fingerprint: digest(PACK_DIGEST),
            records: vec![MigrationIdentityV1 {
                version: 1,
                migration_id: "0001-initial-storage-schema".to_owned(),
                checksum: digest(GLOBAL_DIGEST),
            }],
        }
    }

    fn manifest() -> BackupManifestV1 {
        BackupManifestV1 {
            schema: BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
            backup_id: "backup-imo-52-native".to_owned(),
            deployment_lineage: "deployment/test".to_owned(),
            storage_epoch: 7,
            backend: BackendProfileV1::SqliteBundled,
            native_point: BackendNativePointV1::SqliteOnlineBackup {
                engine_identity: "sqlite-3.53.4-bundled".to_owned(),
                point_id: "point-1".to_owned(),
            },
            migration_contract: migration_contract(),
            expected_packs: vec![PackIdentityV1 {
                pack_id: "worldstream.counter".to_owned(),
                revision_digest: digest(PACK_DIGEST),
                executor_digest: digest(GLOBAL_DIGEST),
                schema_bundle_digest: digest(PACK_DIGEST),
                codec_bundle_digest: digest(GLOBAL_DIGEST),
                resource_ids: vec!["counter-executor".to_owned()],
            }],
            expected_resources: vec![ResourceIdentityV1 {
                resource_id: "counter-executor".to_owned(),
                kind: "executor".to_owned(),
                byte_len: 8,
                digest: DigestV1::hash(b"executor"),
            }],
            expected_global_digest: digest(GLOBAL_DIGEST),
        }
    }

    fn room(room_id: &str, status: IntegrityStatusV1) -> RoomImageV1 {
        let genesis_bytes = format!("{room_id}-genesis").into_bytes();
        let transition_bytes = format!("{room_id}-transition").into_bytes();
        let genesis_digest = DigestV1::hash(&genesis_bytes);
        let transition_digest = DigestV1::hash(&transition_bytes);
        let core = format!("{room_id}-core").into_bytes();
        let activity = format!("{room_id}-activity").into_bytes();
        let authoritative = format!("{room_id}-authoritative").into_bytes();
        RoomImageV1 {
            room_id: room_id.to_owned(),
            integrity: IntegrityWitnessV1 {
                source_status: status,
                restored_status: status,
                source_generation: 2,
                restored_generation: 2,
                source_isolated: !matches!(status, IntegrityStatusV1::Healthy),
                restored_isolated: !matches!(status, IntegrityStatusV1::Healthy),
            },
            head: CompleteHeadV1 {
                room_id: room_id.to_owned(),
                room_seq: 1,
                lineage_digest: transition_digest.clone(),
                core_schema_version: "worldstream/core/v1".to_owned(),
                pack_revision_digest: digest(PACK_DIGEST),
                core_state_digest: DigestV1::hash(&core),
                activity_state_digest: DigestV1::hash(&activity),
                authoritative_state_digest: DigestV1::hash(&authoritative),
            },
            records: vec![
                CanonicalRecordV1 {
                    kind: CanonicalRecordKindV1::Genesis,
                    room_seq: 0,
                    bytes: genesis_bytes,
                    digest: genesis_digest.clone(),
                    previous_digest: None,
                },
                CanonicalRecordV1 {
                    kind: CanonicalRecordKindV1::Transition,
                    room_seq: 1,
                    bytes: transition_bytes,
                    digest: transition_digest,
                    previous_digest: Some(genesis_digest),
                },
            ],
            materialization: MaterializationV1 {
                core_state_bytes: core,
                activity_state_bytes: activity,
                authoritative_state_bytes: authoritative,
            },
            source_bytes_digest: digest(GLOBAL_DIGEST),
            restored_bytes_digest: digest(GLOBAL_DIGEST),
        }
    }

    fn evidence(rooms: Vec<RoomImageV1>) -> NativeRestoreEvidenceV1 {
        let manifest = manifest();
        let image = BackupImageV1 {
            restored_native_point: manifest.native_point.clone(),
            migrations: manifest.migration_contract.clone(),
            resources: vec![ResourceBlobV1 {
                resource_id: "counter-executor".to_owned(),
                bytes: b"executor".to_vec(),
            }],
            rooms,
            receipts: Vec::new(),
            timers: Vec::new(),
            frames: Vec::new(),
            activations: Vec::new(),
            activation_receipts: Vec::new(),
            source_global_digest: digest(GLOBAL_DIGEST),
            restored_global_digest: digest(GLOBAL_DIGEST),
            manifest,
        };
        let target = NativeRestoreTargetEvidenceV1 {
            backend: Some(image.manifest.backend),
            deployment_lineage: Some(image.manifest.deployment_lineage.clone()),
            storage_epoch: Some(image.manifest.storage_epoch),
            native_point: Some(image.restored_native_point.clone()),
            migration_contract: Some(image.migrations.clone()),
            pack_identities: Some(image.manifest.expected_packs.clone()),
            resource_identities: Some(image.manifest.expected_resources.clone()),
            room_membership: Some(
                image
                    .rooms
                    .iter()
                    .map(|room| NativeRestoreRoomMembershipV1 {
                        room_id: room.room_id.clone(),
                        integrity: room.integrity.clone(),
                    })
                    .collect(),
            ),
            durable_domains: None,
        };
        NativeRestoreEvidenceV1::new(image, target)
    }

    fn postgres_evidence() -> NativeRestoreEvidenceV1 {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        let native_point = BackendNativePointV1::PostgresNative {
            major: 17,
            engine_identity: "postgresql-17.11".to_owned(),
            point_id: "dump-point-1".to_owned(),
            mechanism: crate::PostgresNativeMechanismV1::Dump,
        };
        evidence.image.manifest.backend = BackendProfileV1::PostgresPrimary;
        evidence.image.manifest.native_point = native_point.clone();
        evidence.image.restored_native_point = native_point.clone();
        evidence.target.backend = Some(BackendProfileV1::PostgresPrimary);
        evidence.target.native_point = Some(native_point);
        evidence.target.durable_domains = Some(
            POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
                .into_iter()
                .map(|domain| {
                    let bytes = match domain {
                        NativeRestoreDurableDomainV1::OperationGuards => {
                            br#"["identity","request-hash","room",null]"#.to_vec()
                        }
                        NativeRestoreDurableDomainV1::MemberDeliveryState => {
                            // A member with a durable head/cursor but no Frame rows.
                            br#"["room","member","membership",0,1,1,null,null]"#.to_vec()
                        }
                        NativeRestoreDurableDomainV1::ObservationConsequences => {
                            br#"["room",1,"member",1,0,"payload-hash"]"#.to_vec()
                        }
                        NativeRestoreDurableDomainV1::IntegrityIncidents => {
                            br#"["room",1,"quarantined","reason",1]"#.to_vec()
                        }
                        _ => format!("[\"{}\"]", domain.as_str()).into_bytes(),
                    };
                    let rows = vec![NativeRestoreCanonicalRowV1::new(bytes)];
                    NativeRestoreDurableDomainEvidenceV1::new(domain, rows.clone(), rows)
                })
                .collect(),
        );
        evidence
    }

    struct FixtureAdapter(NativeRestoreEvidenceV1);

    impl NativeRestoreEvidenceAdapter for FixtureAdapter {
        fn native_restore_evidence(&self) -> &NativeRestoreEvidenceV1 {
            &self.0
        }
    }

    #[test]
    fn complete_evidence_is_accepted_through_the_adapter_seam() {
        let adapter = FixtureAdapter(evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]));
        let report = verify_native_restore_from_adapter(&adapter, VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(
            report.rooms.get("healthy"),
            Some(&crate::RoomDispositionV1::Verified)
        );
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn complete_evidence_accepts_two_retained_revisions_of_one_pack() {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        let mut retained = evidence.image.manifest.expected_packs[0].clone();
        retained.revision_digest = digest(GLOBAL_DIGEST);
        evidence
            .image
            .manifest
            .expected_packs
            .push(retained.clone());
        evidence
            .target
            .pack_identities
            .as_mut()
            .unwrap_or_else(|| unreachable!())
            .push(retained);

        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(report.is_ready(), "{:?}", report.diagnostics);
    }

    #[test]
    fn complete_postgres_durable_domain_inventory_is_required_and_ready() {
        let report = verify_native_restore(&postgres_evidence(), VerifierLimits::default());
        assert!(report.is_ready(), "{:?}", report.diagnostics);
    }

    #[test]
    fn exact_maximum_raw_resource_survives_postgres_hex_row_framing() {
        let limits = VerifierLimits::default();
        let mut canonical = Vec::with_capacity(limits.max_object_bytes * 2 + 8);
        canonical.extend_from_slice(br#"["\\x"#);
        for _ in 0..limits.max_object_bytes {
            canonical.extend_from_slice(b"00");
        }
        canonical.extend_from_slice(br#""]"#);
        assert!(canonical.len() > limits.max_object_bytes);
        assert!(canonical.len() <= max_native_restore_canonical_row_bytes(limits.max_object_bytes));

        let mut evidence = postgres_evidence();
        let domain = evidence
            .target
            .durable_domains
            .as_mut()
            .unwrap_or_else(|| unreachable!())
            .iter_mut()
            .find(|domain| domain.domain == NativeRestoreDurableDomainV1::DeploymentResourceBlobs)
            .unwrap_or_else(|| unreachable!());
        let rows = vec![NativeRestoreCanonicalRowV1::new(canonical)];
        *domain = NativeRestoreDurableDomainEvidenceV1::new(
            NativeRestoreDurableDomainV1::DeploymentResourceBlobs,
            rows.clone(),
            rows,
        );

        let report = verify_native_restore(&evidence, limits);
        assert!(report.is_ready(), "{:?}", report.diagnostics);
    }

    #[test]
    fn postgres_retry_reset_incident_and_zero_frame_member_rows_are_nonempty() {
        let evidence = postgres_evidence();
        let domains = evidence
            .target
            .durable_domains
            .as_ref()
            .unwrap_or_else(|| unreachable!());
        for required in [
            NativeRestoreDurableDomainV1::OperationGuards,
            NativeRestoreDurableDomainV1::ObservationConsequences,
            NativeRestoreDurableDomainV1::IntegrityIncidents,
            NativeRestoreDurableDomainV1::MemberDeliveryState,
        ] {
            let domain = domains
                .iter()
                .find(|domain| domain.domain == required)
                .unwrap_or_else(|| unreachable!());
            assert_eq!(domain.source_row_count, 1, "{}", required.as_str());
            assert_eq!(domain.restored_row_count, 1, "{}", required.as_str());
            assert_eq!(domain.source_rows, domain.restored_rows);
        }
        let member = domains
            .iter()
            .find(|domain| domain.domain == NativeRestoreDurableDomainV1::MemberDeliveryState)
            .unwrap_or_else(|| unreachable!());
        assert_eq!(
            member.source_rows[0].canonical_bytes,
            br#"["room","member","membership",0,1,1,null,null]"#
        );
        let guard = domains
            .iter()
            .find(|domain| domain.domain == NativeRestoreDurableDomainV1::OperationGuards)
            .unwrap_or_else(|| unreachable!());
        assert!(guard.source_rows[0].canonical_bytes.ends_with(b",null]"));
    }

    #[test]
    fn every_postgres_durable_domain_omission_fails_closed() {
        for omitted in POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1 {
            let mut evidence = postgres_evidence();
            evidence
                .target
                .durable_domains
                .as_mut()
                .unwrap_or_else(|| unreachable!())
                .retain(|domain| domain.domain != omitted);
            let report = verify_native_restore(&evidence, VerifierLimits::default());
            assert!(!report.is_ready(), "omitted {}", omitted.as_str());
            assert!(report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "native_restore_durable_domain_inventory_mismatch"
            }));
        }
    }

    #[test]
    fn every_postgres_durable_domain_row_tamper_fails_closed() {
        for tampered in POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1 {
            let mut evidence = postgres_evidence();
            let domain = evidence
                .target
                .durable_domains
                .as_mut()
                .unwrap_or_else(|| unreachable!())
                .iter_mut()
                .find(|domain| domain.domain == tampered)
                .unwrap_or_else(|| unreachable!());
            domain.source_rows[0].canonical_bytes = b"[\"tampered\"]".to_vec();
            let report = verify_native_restore(&evidence, VerifierLimits::default());
            assert!(!report.is_ready(), "tampered {}", tampered.as_str());
            assert!(
                report.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "native_restore_durable_domain_mismatch"
                })
            );
        }
    }

    #[test]
    fn postgres_durable_domain_counts_and_aggregate_digests_are_bound() {
        let mut count = postgres_evidence();
        count
            .target
            .durable_domains
            .as_mut()
            .unwrap_or_else(|| unreachable!())[0]
            .source_row_count += 1;
        let report = verify_native_restore(&count, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "native_restore_durable_domain_mismatch")
        );

        let mut digest = postgres_evidence();
        digest
            .target
            .durable_domains
            .as_mut()
            .unwrap_or_else(|| unreachable!())[0]
            .source_digest = DigestV1::hash(b"forged-domain-digest");
        let report = verify_native_restore(&digest, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "native_restore_durable_domain_mismatch")
        );
    }

    #[test]
    fn missing_target_metadata_fails_closed() {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        evidence.target.deployment_lineage = None;
        evidence.target.storage_epoch = None;
        evidence.target.migration_contract = None;
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "native_restore_metadata_missing"
                && diagnostic.class == ConsistencyClassV1::Manifest
        }));
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "native_restore_metadata_missing"
                && diagnostic.class == ConsistencyClassV1::Migration
        }));
    }

    #[test]
    fn deployment_lineage_mismatch_fails_closed() {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        evidence.target.deployment_lineage = Some("deployment/other".to_owned());
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "native_restore_deployment_lineage_mismatch"
                && diagnostic.class == ConsistencyClassV1::Manifest
        }));
    }

    #[test]
    fn canonical_byte_or_hash_mismatch_blocks_native_restore() {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        evidence.image.rooms[0].records[1].bytes = b"tampered".to_vec();
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.class == ConsistencyClassV1::CanonicalRecord
                && (diagnostic.code == "canonical_record_mismatch"
                    || diagnostic.code == "native_restore_canonical_evidence_mismatch")
        }));
    }

    #[test]
    fn duplicate_target_identity_fails_closed() {
        let mut evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        let mut packs = evidence.target.pack_identities.take().unwrap_or_default();
        assert_eq!(packs.len(), 1);
        let first = packs[0].clone();
        packs.push(first);
        evidence.target.pack_identities = Some(packs);
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "native_restore_duplicate_pack_identity")
        );
    }

    #[test]
    fn isolated_old_room_remains_isolated_without_poisoning_healthy_room() {
        let evidence = evidence(vec![
            room("healthy", IntegrityStatusV1::Healthy),
            room("old-isolated", IntegrityStatusV1::Quarantined),
        ]);
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(
            report.rooms.get("healthy"),
            Some(&crate::RoomDispositionV1::Verified)
        );
        assert_eq!(
            report.rooms.get("old-isolated"),
            Some(&crate::RoomDispositionV1::IsolatedPreExisting)
        );
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.disposition == crate::DiagnosticDispositionV1::PermittedPreExistingIsolation
        }));
    }

    #[test]
    fn isolated_raw_bytes_do_not_require_semantic_canonical_records() {
        let mut isolated = room("old-isolated", IntegrityStatusV1::Quarantined);
        isolated.records.clear();
        isolated.materialization.core_state_bytes.clear();
        isolated.materialization.activity_state_bytes.clear();
        isolated.materialization.authoritative_state_bytes.clear();
        let evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy), isolated]);
        let report = verify_native_restore(&evidence, VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(
            report.rooms.get("old-isolated"),
            Some(&crate::RoomDispositionV1::IsolatedPreExisting)
        );
        assert!(
            !report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "native_restore_canonical_evidence_mismatch"
            })
        );
    }

    #[test]
    fn lifecycle_requires_catch_up_and_keeps_isolated_rooms_quarantined()
    -> Result<(), RestoreLifecycleErrorV1> {
        let report = verify_native_restore(
            &evidence(vec![
                room("healthy", IntegrityStatusV1::Healthy),
                room("old-isolated", IntegrityStatusV1::Quarantined),
            ]),
            VerifierLimits::default(),
        );
        let mut projection = RestoreLifecycleProjectionV1::from_report(report, 2)?;
        assert_eq!(projection.state(), RestoreLifecycleStateV1::Loading);
        assert_eq!(
            projection.room_state("old-isolated"),
            Some(RestoreRoomLifecycleStateV1::Quarantined)
        );
        projection.begin_catch_up(2)?;
        assert_eq!(
            projection.complete_catch_up(2, false),
            Err(RestoreLifecycleErrorV1::CatchUpBarrierUnsatisfied)
        );
        assert_eq!(projection.state(), RestoreLifecycleStateV1::CatchingUp);
        projection.complete_catch_up(2, true)?;
        assert_eq!(projection.state(), RestoreLifecycleStateV1::Active);
        assert_eq!(
            projection.room_state("healthy"),
            Some(RestoreRoomLifecycleStateV1::Active)
        );
        assert_eq!(
            projection.room_state("old-isolated"),
            Some(RestoreRoomLifecycleStateV1::Quarantined)
        );
        Ok(())
    }

    #[test]
    fn lifecycle_faults_on_global_verification_failure() -> Result<(), RestoreLifecycleErrorV1> {
        let mut restore_evidence = evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]);
        restore_evidence.target.storage_epoch = None;
        let report = verify_native_restore(&restore_evidence, VerifierLimits::default());
        let mut projection = RestoreLifecycleProjectionV1::from_report(report, 3)?;
        assert_eq!(projection.state(), RestoreLifecycleStateV1::Faulted);
        assert_eq!(
            projection.begin_catch_up(3),
            Err(RestoreLifecycleErrorV1::InvalidTransition)
        );
        assert_eq!(
            projection.room_state("healthy"),
            Some(RestoreRoomLifecycleStateV1::Faulted)
        );
        Ok(())
    }

    #[test]
    fn passivation_reload_and_generation_fencing_are_deterministic()
    -> Result<(), RestoreLifecycleErrorV1> {
        let report = verify_native_restore(
            &evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]),
            VerifierLimits::default(),
        );
        let mut projection = RestoreLifecycleProjectionV1::from_report(report, 4)?;
        projection.begin_catch_up(4)?;
        projection.complete_catch_up(4, true)?;
        assert_eq!(
            projection.passivate(3, 5),
            Err(RestoreLifecycleErrorV1::GenerationFence)
        );
        projection.passivate(4, 5)?;
        assert_eq!(projection.state(), RestoreLifecycleStateV1::Loading);
        assert_eq!(projection.generation(), 5);
        assert_eq!(
            projection.begin_catch_up(4),
            Err(RestoreLifecycleErrorV1::GenerationFence)
        );
        projection.begin_catch_up(5)?;
        projection.complete_catch_up(5, true)?;
        assert_eq!(projection.state(), RestoreLifecycleStateV1::Active);
        Ok(())
    }

    #[test]
    fn install_after_commit_is_exactly_once() -> Result<(), RestoreLifecycleErrorV1> {
        let report = verify_native_restore(
            &evidence(vec![room("healthy", IntegrityStatusV1::Healthy)]),
            VerifierLimits::default(),
        );
        let mut projection = RestoreLifecycleProjectionV1::from_report(report, 6)?;
        assert_eq!(
            projection.install_after_commit(6, "commit-1"),
            Err(RestoreLifecycleErrorV1::CommitNotDurable)
        );
        projection.record_durable_commit(6, "commit-1")?;
        assert_eq!(
            projection.install_after_commit(6, "commit-1"),
            Ok(RestoreInstallOutcomeV1::Installed)
        );
        assert_eq!(
            projection.install_after_commit(6, "commit-1"),
            Ok(RestoreInstallOutcomeV1::AlreadyInstalled)
        );
        assert_eq!(
            projection.install_after_commit(6, "commit-2"),
            Err(RestoreLifecycleErrorV1::CommitNotDurable)
        );
        assert_eq!(
            projection.record_durable_commit(6, "commit-2"),
            Err(RestoreLifecycleErrorV1::CommitIdentityMismatch)
        );
        Ok(())
    }
}
