//! PostgreSQL-specific native backup/restore orchestration.
//!
//! The provider-neutral image and verifier remain in `worldstream-backup`.
//! This module owns only PostgreSQL connection/tool handling and the typed
//! adaptation of restored rows into that existing verifier contract.

use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

use native_tls::TlsConnector;
use postgres::{Client, Row};
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_backup::{
    ActivationStateV1, ActivationV1, BackendNativePointV1, BackendProfileV1, BackupImageV1,
    BackupManifestV1, CanonicalRecordKindV1, CanonicalRecordV1, CompleteHeadV1, ContextRetentionV1,
    DigestV1, FrameV1, IntegrityStatusV1, IntegrityWitnessV1, MaterializationV1,
    MigrationContractV1, MigrationIdentityV1, NativeRestoreCanonicalRowV1,
    NativeRestoreDurableDomainEvidenceV1, NativeRestoreDurableDomainV1, NativeRestoreEvidenceV1,
    NativeRestoreRoomMembershipV1, NativeRestoreTargetEvidenceV1,
    POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1, PackIdentityV1, ReceiptKindV1, ReceiptV1,
    ResourceBlobV1, ResourceIdentityV1, RoomImageV1, TimerStateV1, TimerV1, VerificationReportV1,
    VerifierLimits, verify_native_restore,
};
use worldstream_core::{
    ActivationOperationResultV1, CanonicalJsonV1, CompleteHeadV1 as CoreCompleteHeadV1, GenesisV1,
    OperationIdentityV1, StoredSemanticResultV1, TransitionV1,
};
use worldstream_transfer::{
    DeploymentIdentityV1 as TransferDeploymentIdentityV1, DigestV1 as TransferDigestV1,
    PackIdentityV1 as TransferPackIdentityV1, ResourceIdentityV1 as TransferResourceIdentityV1,
    ResourceKindV1 as TransferResourceKindV1,
};

use crate::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore,
    migration_history, schema_contract_fingerprint,
};

/// Evidence schema emitted by this provider-native lane.
pub const NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1: &str =
    "worldstream/native-postgres-restore-evidence/v2";
const REQUIRED_POSTGRES_VERSION_NUM: u32 = 170_011;

/// Non-secret endpoint coordinates. Passwords are loaded only from `PGPASSFILE`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePostgresEndpointV1 {
    /// Host or socket directory.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Database name.
    pub database: String,
    /// Login role.
    pub username: String,
}

impl NativePostgresEndpointV1 {
    /// Creates an endpoint from non-secret coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error for empty/whitespace coordinates or port zero.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        database: impl Into<String>,
        username: impl Into<String>,
    ) -> Result<Self, NativePostgresError> {
        let endpoint = Self {
            host: host.into(),
            port,
            database: database.into(),
            username: username.into(),
        };
        if endpoint.port == 0
            || [&endpoint.host, &endpoint.database, &endpoint.username]
                .iter()
                .any(|value| value.trim().is_empty() || value.chars().any(char::is_whitespace))
        {
            return Err(NativePostgresError::Configuration(
                "invalid endpoint coordinates",
            ));
        }
        Ok(endpoint)
    }
}

/// Configuration for one native custom-format dump/restore.
#[derive(Clone)]
pub struct NativePostgresRestoreConfig {
    /// Source endpoint.
    pub source: NativePostgresEndpointV1,
    /// Isolated target endpoint.
    pub target: NativePostgresEndpointV1,
    /// Owner-only passfile path; its contents are never serialized.
    pub passfile: PathBuf,
    /// Provider `pg_dump` executable.
    pub pg_dump: PathBuf,
    /// Provider `pg_restore` executable.
    pub pg_restore: PathBuf,
    /// Provider `psql` executable.
    pub psql: PathBuf,
    /// New custom-format dump path.
    pub dump_path: PathBuf,
}

impl fmt::Debug for NativePostgresRestoreConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativePostgresRestoreConfig")
            .field("source", &self.source)
            .field("target", &self.target)
            .field("passfile", &self.passfile)
            .field("pg_dump", &self.pg_dump)
            .field("pg_restore", &self.pg_restore)
            .field("psql", &self.psql)
            .field("dump_path", &self.dump_path)
            .finish()
    }
}

impl NativePostgresRestoreConfig {
    /// Validates owner-only credentials, tools, and the disposable dump path.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials/tools are unsafe or the dump path is
    /// already present.
    pub fn new(
        source: NativePostgresEndpointV1,
        target: NativePostgresEndpointV1,
        passfile: impl Into<PathBuf>,
        pg_dump: impl Into<PathBuf>,
        pg_restore: impl Into<PathBuf>,
        psql: impl Into<PathBuf>,
        dump_path: impl Into<PathBuf>,
    ) -> Result<Self, NativePostgresError> {
        let config = Self {
            source,
            target,
            passfile: passfile.into(),
            pg_dump: pg_dump.into(),
            pg_restore: pg_restore.into(),
            psql: psql.into(),
            dump_path: dump_path.into(),
        };
        if !config.passfile.is_file() || config.passfile.is_symlink() {
            return Err(NativePostgresError::Configuration(
                "PGPASSFILE is not a regular file",
            ));
        }
        if config
            .passfile
            .metadata()
            .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?
            .permissions()
            .mode()
            & 0o777
            != 0o600
        {
            return Err(NativePostgresError::Configuration(
                "PGPASSFILE must be mode 0600",
            ));
        }
        for path in [&config.pg_dump, &config.pg_restore, &config.psql] {
            if !path.is_file() {
                return Err(NativePostgresError::ToolUnavailable);
            }
        }
        if config.dump_path.exists()
            || config.dump_path.is_symlink()
            || config
                .dump_path
                .parent()
                .is_none_or(|parent| !parent.is_dir())
        {
            return Err(NativePostgresError::Configuration(
                "dump path must be new in an existing directory",
            ));
        }
        Ok(config)
    }
}

/// Non-serializable witness minted only after native restore and semantic verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePostgresTrustedWitnessV1 {
    dump_digest: DigestV1,
    restored_global_digest: DigestV1,
}

impl NativePostgresTrustedWitnessV1 {
    /// Digest of the provider custom-format dump.
    #[must_use]
    pub fn dump_digest(&self) -> &DigestV1 {
        &self.dump_digest
    }

    /// Digest of the verified restored deployment identity.
    #[must_use]
    pub fn restored_global_digest(&self) -> &DigestV1 {
        &self.restored_global_digest
    }
}

/// Redacted provider-native report. `release_evidence` is permanently false.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct NativePostgresRestoreReportV1 {
    /// Evidence schema.
    pub schema: String,
    /// `ready` only when the unified verifier returned ready and snapshots were disposed.
    pub status: String,
    /// Stable redacted reason.
    pub reason: String,
    /// Always false for local Docker evidence.
    pub release_evidence: bool,
    /// Native custom-format dump/restore result.
    pub native_dump_restore: String,
    /// Unified provider-neutral verifier result.
    pub verifier: VerificationReportV1,
    /// Source remained byte-identical after dump/target cleanup.
    pub source_unchanged: bool,
    /// Target was exact before disposable snapshot deletion.
    pub exact_restored_row_set: bool,
    /// Durable canonical verification passed before snapshot deletion.
    pub snapshots_disposable: bool,
    /// Target was isolated and never published by this lane.
    pub target_isolated: bool,
    /// This lane never publishes target authority.
    pub target_published: bool,
    /// Caller-owned cleanup is required.
    pub cleanup_required: bool,
    /// Non-serializable witness was minted.
    pub native_witness_minted: bool,
    /// Secrets never entered argv, output, or this report.
    pub secrets_emitted: bool,
    /// Source/target provider version number.
    pub source_version_num: Option<u32>,
    /// Restored provider version number.
    pub restored_version_num: Option<u32>,
    /// Semantic receipts decoded and compared at both endpoints.
    pub semantic_receipts_verified: bool,
    /// Activation intents, generations, leases, runners, and contexts matched.
    pub activation_intents_verified: bool,
    /// Activation operation result/context rows and request hashes matched.
    pub activation_operation_receipts_verified: bool,
    /// Honest request evidence retained by this PostgreSQL schema.
    pub activation_request_evidence: String,
    /// Principals, capabilities, revocations, Runner targets, and fences matched.
    pub authority_state_verified: bool,
    /// Every fixed durable domain passed source/restored count and digest binding.
    pub durable_domains_verified: bool,
    /// Exact bounded fixture accepted by the typed semantic projection.
    pub verifier_scope: NativePostgresVerifierScopeV1,
    /// Aggregate source durable-domain inventory digest.
    pub source_durable_domains_digest: String,
    /// Aggregate restored durable-domain inventory digest.
    pub restored_durable_domains_digest: String,
    /// Fixed redacted domain inventory; exact rows remain inside verifier evidence.
    pub durable_domain_inventory: Vec<NativePostgresDurableDomainReportV1>,
    /// Snapshot rows observed before disposal.
    pub restored_snapshot_count_before: usize,
    /// Snapshot rows observed after the disposal attempt.
    pub restored_snapshot_count_after: usize,
}

/// Redacted count/digest summary for one exact source/restored durable domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativePostgresDurableDomainReportV1 {
    /// Stable provider-neutral domain label.
    pub domain: String,
    /// Exact source row count.
    pub source_row_count: u64,
    /// Exact restored row count.
    pub restored_row_count: u64,
    /// Exact source domain digest.
    pub source_digest: String,
    /// Exact restored domain digest.
    pub restored_digest: String,
}

/// Exact scope of the PostgreSQL-to-provider-neutral semantic projection.
/// All durable PostgreSQL rows and every healthy Room are verified without
/// sampling; pre-existing unhealthy Rooms are raw-byte compared and retained
/// as isolated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativePostgresVerifierScopeV1 {
    /// Stable verifier profile.
    pub profile: String,
    /// Exact source deployment Pack count.
    pub source_pack_identity_count: usize,
    /// Exact restored deployment Pack count.
    pub restored_pack_identity_count: usize,
    /// Exact source deployment resource count.
    pub source_resource_identity_count: usize,
    /// Exact restored deployment resource count.
    pub restored_resource_identity_count: usize,
    /// Fired Timers admitted by the source typed projection.
    pub source_fired_timer_count: usize,
    /// Fired Timers admitted by the restored typed projection.
    pub restored_fired_timer_count: usize,
    /// True only when complete multi-Pack/resource/fired-Timer support ran.
    pub general_deployment_support_verified: bool,
}

/// Native restore result; the trusted witness cannot be serialized.
#[derive(Debug)]
pub struct NativePostgresRestoreOutcome {
    /// Redacted operational report.
    pub report: NativePostgresRestoreReportV1,
    /// Trusted witness, present only on a fully verified restore.
    pub witness: Option<NativePostgresTrustedWitnessV1>,
}

/// Rebuilds the reviewed disposable snapshot cache for every healthy seeded
/// source Room using the existing PostgreSQL adapter recovery path. Existing
/// faulted or quarantined Rooms remain byte-preserved and isolated: their
/// disposable snapshots are deliberately not decoded or rewritten.
///
/// # Errors
///
/// Returns an error when the passfile/database is unavailable or a canonical
/// Room cannot be rebuilt without mutation of durable history.
pub fn rebuild_native_snapshot_cache(
    endpoint: &NativePostgresEndpointV1,
    passfile: &Path,
) -> Result<usize, NativePostgresError> {
    let password = password_for(endpoint, passfile)?;
    let mut client = connect(endpoint, &password)?;
    let room_ids = client
        .query(
            "SELECT room_id, integrity_generation, integrity_status \
             FROM worldstream_room_roots ORDER BY room_id",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?;
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(dsn_for(endpoint, &password))
            .map_err(|_| NativePostgresError::Configuration("source admin configuration failed"))?,
    )
    .map_err(|_| NativePostgresError::Configuration("source admin adapter failed"))?;
    let mut rebuilt = 0;
    for row in room_ids {
        let room_id: String = row
            .try_get(0)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let integrity_generation: i64 = row
            .try_get(1)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let integrity_status: String = row
            .try_get(2)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        if integrity_generation <= 0
            || !matches!(
                integrity_status.as_str(),
                "healthy" | "faulted" | "quarantined"
            )
        {
            return Err(NativePostgresError::MalformedRow);
        }
        if integrity_status != "healthy" {
            continue;
        }
        rebuilt += admin
            .rebuild_snapshot_cache(&room_id)
            .map_err(|_| NativePostgresError::Database)?;
    }
    Ok(rebuilt)
}

/// Closed errors from the provider-native boundary.
#[derive(Debug, Error)]
pub enum NativePostgresError {
    #[error("native PostgreSQL configuration is invalid: {0}")]
    Configuration(&'static str),
    #[error("required PostgreSQL tool is unavailable")]
    ToolUnavailable,
    #[error("native PostgreSQL provider command failed")]
    ProviderCommand,
    #[error("native PostgreSQL provider row is malformed")]
    MalformedRow,
    #[error("native PostgreSQL provider version is not 17.11")]
    WrongVersion,
    #[error("native PostgreSQL semantic evidence is incomplete")]
    Incomplete,
    #[error("native PostgreSQL database operation failed")]
    Database,
}

struct Capture {
    version_num: u32,
    deployment_lineage: String,
    storage_epoch: u64,
    global_digest: DigestV1,
    migration_contract: MigrationContractV1,
    packs: Vec<PackIdentityV1>,
    resources: Vec<ResourceIdentityV1>,
    resource_blobs: Vec<ResourceBlobV1>,
    rooms: Vec<RoomCapture>,
    receipts: Vec<ReceiptV1>,
    activations: Vec<ActivationV1>,
    activation_receipts: Vec<ReceiptV1>,
    durable_domains: BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
    durable_digest: DigestV1,
    snapshot_count: usize,
}

struct RoomCapture {
    room: RoomImageV1,
    timers: Vec<TimerV1>,
    frames: Vec<FrameV1>,
    snapshot_count: usize,
}

/// Runs provider-native dump/restore, adapts both endpoints, and invokes the
/// existing provider-neutral semantic verifier.
///
/// # Errors
///
/// Returns an error for unavailable/malformed provider operations. A valid
/// provider operation with incomplete semantic evidence returns an incomplete
/// outcome and never mints a witness.
#[allow(clippy::too_many_lines)]
pub fn run_native_postgres_restore(
    config: &NativePostgresRestoreConfig,
) -> Result<NativePostgresRestoreOutcome, NativePostgresError> {
    let source_before = capture(config, &config.source)?;
    let dump_digest = native_dump_restore(config)?;
    let source_after_dump = capture(config, &config.source)?;
    let restored_before_disposal = capture(config, &config.target)?;
    let evidence = make_evidence(&source_after_dump, &restored_before_disposal, &dump_digest)?;
    let durable_domain_inventory = evidence
        .target
        .durable_domains
        .as_deref()
        .ok_or(NativePostgresError::Incomplete)?
        .iter()
        .map(|domain| NativePostgresDurableDomainReportV1 {
            domain: domain.domain.as_str().to_owned(),
            source_row_count: domain.source_row_count,
            restored_row_count: domain.restored_row_count,
            source_digest: domain.source_digest.as_str().to_owned(),
            restored_digest: domain.restored_digest.as_str().to_owned(),
        })
        .collect::<Vec<_>>();
    let verifier = verify_native_restore(&evidence, VerifierLimits::default());
    let source_unchanged = source_before.durable_digest == source_after_dump.durable_digest;
    let exact_restored_row_set =
        source_after_dump.durable_digest == restored_before_disposal.durable_digest;
    let (snapshots_disposable, restored_snapshot_count_after) = dispose_snapshots(
        config,
        &restored_before_disposal,
        verifier.is_ready() && source_unchanged && exact_restored_row_set,
    )?;
    let ready =
        verifier.is_ready() && source_unchanged && exact_restored_row_set && snapshots_disposable;
    let witness = ready.then(|| NativePostgresTrustedWitnessV1 {
        dump_digest,
        restored_global_digest: restored_before_disposal.durable_digest.clone(),
    });
    Ok(NativePostgresRestoreOutcome {
        report: NativePostgresRestoreReportV1 {
            schema: NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1.to_owned(),
            status: if ready { "ready" } else { "incomplete" }.to_owned(),
            reason: if ready {
                "postgres_native_restore_verified_by_unified_verifier"
            } else if !verifier.is_ready() {
                "unified_semantic_verifier_not_ready"
            } else {
                "native_restore_boundary_incomplete"
            }
            .to_owned(),
            release_evidence: false,
            native_dump_restore: "pass".to_owned(),
            verifier,
            source_unchanged,
            exact_restored_row_set,
            snapshots_disposable,
            target_isolated: true,
            target_published: false,
            cleanup_required: true,
            native_witness_minted: witness.is_some(),
            secrets_emitted: false,
            source_version_num: Some(source_before.version_num),
            restored_version_num: Some(restored_before_disposal.version_num),
            semantic_receipts_verified: source_before.receipts == restored_before_disposal.receipts,
            activation_intents_verified: source_before.activations
                == restored_before_disposal.activations,
            activation_operation_receipts_verified: source_before.activation_receipts
                == restored_before_disposal.activation_receipts,
            activation_request_evidence: "stored_canonical_hash_only_verified".to_owned(),
            authority_state_verified: authority_domains_equal(
                &source_after_dump.durable_domains,
                &restored_before_disposal.durable_domains,
            ),
            durable_domains_verified: source_after_dump.durable_domains
                == restored_before_disposal.durable_domains,
            verifier_scope: NativePostgresVerifierScopeV1 {
                profile: "full_deployment_all_durable_domains".to_owned(),
                source_pack_identity_count: source_after_dump.packs.len(),
                restored_pack_identity_count: restored_before_disposal.packs.len(),
                source_resource_identity_count: source_after_dump.resources.len(),
                restored_resource_identity_count: restored_before_disposal.resources.len(),
                source_fired_timer_count: fired_timer_count(&source_after_dump),
                restored_fired_timer_count: fired_timer_count(&restored_before_disposal),
                general_deployment_support_verified: ready
                    && source_after_dump.packs.len() >= 2
                    && source_after_dump.packs == restored_before_disposal.packs
                    && !source_after_dump.resources.is_empty()
                    && source_after_dump.resources == restored_before_disposal.resources
                    && source_after_dump.resource_blobs == restored_before_disposal.resource_blobs
                    && fired_timer_count(&source_after_dump) > 0
                    && fired_timer_count(&source_after_dump)
                        == fired_timer_count(&restored_before_disposal),
            },
            source_durable_domains_digest: durable_domains_digest(
                &source_after_dump.durable_domains,
            )
            .as_str()
            .to_owned(),
            restored_durable_domains_digest: durable_domains_digest(
                &restored_before_disposal.durable_domains,
            )
            .as_str()
            .to_owned(),
            durable_domain_inventory,
            restored_snapshot_count_before: restored_before_disposal.snapshot_count,
            restored_snapshot_count_after,
        },
        witness,
    })
}

fn native_dump_restore(
    config: &NativePostgresRestoreConfig,
) -> Result<DigestV1, NativePostgresError> {
    let dump = config
        .dump_path
        .to_str()
        .ok_or(NativePostgresError::Configuration("dump path is not UTF-8"))?;
    run_tool(
        config,
        &config.pg_dump,
        &config.source,
        [
            "--format=custom",
            "--no-owner",
            "--no-privileges",
            "--file",
            dump,
        ],
    )?;
    let dump_digest = DigestV1::hash(
        &fs::read(&config.dump_path).map_err(|_| NativePostgresError::ProviderCommand)?,
    );
    run_tool(
        config,
        &config.pg_restore,
        &config.target,
        [
            "--exit-on-error",
            "--single-transaction",
            "--clean",
            "--if-exists",
            "--no-owner",
            "--no-privileges",
            dump,
        ],
    )?;
    Ok(dump_digest)
}

fn dispose_snapshots(
    config: &NativePostgresRestoreConfig,
    restored: &Capture,
    eligible: bool,
) -> Result<(bool, usize), NativePostgresError> {
    if !eligible {
        return Ok((false, restored.snapshot_count));
    }
    if restored.snapshot_count == 0 {
        return Ok((true, 0));
    }
    let password = password_for(&config.target, &config.passfile)?;
    let target_admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(dsn_for(&config.target, &password))
            .map_err(|_| NativePostgresError::Configuration("target admin configuration failed"))?,
    )
    .map_err(|_| NativePostgresError::Configuration("target admin adapter failed"))?;
    for room in &restored.rooms {
        target_admin
            .delete_snapshot_cache(&room.room.room_id)
            .map_err(|_| NativePostgresError::Database)?;
    }
    let after = capture(config, &config.target)?;
    Ok((
        after.snapshot_count == 0 && after.durable_digest == restored.durable_digest,
        after.snapshot_count,
    ))
}

fn make_evidence(
    source: &Capture,
    restored: &Capture,
    dump_digest: &DigestV1,
) -> Result<NativeRestoreEvidenceV1, NativePostgresError> {
    if source.version_num != REQUIRED_POSTGRES_VERSION_NUM
        || restored.version_num != REQUIRED_POSTGRES_VERSION_NUM
        || source.packs != restored.packs
        || source.resources != restored.resources
        || source.migration_contract != restored.migration_contract
        || source.deployment_lineage != restored.deployment_lineage
        || source.storage_epoch != restored.storage_epoch
        || source.global_digest != restored.global_digest
        || source.activations != restored.activations
        || source.activation_receipts != restored.activation_receipts
        || source.durable_domains != restored.durable_domains
    {
        return Err(NativePostgresError::Incomplete);
    }
    let native_point = BackendNativePointV1::PostgresNative {
        major: 17,
        engine_identity: "postgresql-17.11".to_owned(),
        point_id: dump_digest.as_str().to_owned(),
        mechanism: worldstream_backup::PostgresNativeMechanismV1::Dump,
    };
    let manifest = BackupManifestV1 {
        schema: worldstream_backup::BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
        backup_id: format!("postgres-native-{}", dump_digest.as_str()),
        deployment_lineage: source.deployment_lineage.clone(),
        storage_epoch: source.storage_epoch,
        backend: BackendProfileV1::PostgresPrimary,
        native_point: native_point.clone(),
        migration_contract: source.migration_contract.clone(),
        expected_packs: source.packs.clone(),
        expected_resources: source.resources.clone(),
        expected_global_digest: source.global_digest.clone(),
    };
    let source_rooms = source
        .rooms
        .iter()
        .map(|item| (&item.room.room_id, &item.room))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut rooms = Vec::with_capacity(restored.rooms.len());
    for restored_room in &restored.rooms {
        let Some(source_room) = source_rooms.get(&restored_room.room.room_id) else {
            return Err(NativePostgresError::Incomplete);
        };
        let mut room = restored_room.room.clone();
        room.source_bytes_digest = source_room.source_bytes_digest.clone();
        rooms.push(room);
    }
    let image = BackupImageV1 {
        manifest: manifest.clone(),
        restored_native_point: native_point.clone(),
        migrations: restored.migration_contract.clone(),
        resources: restored.resource_blobs.clone(),
        rooms,
        receipts: restored.receipts.clone(),
        timers: restored
            .rooms
            .iter()
            .flat_map(|room| room.timers.clone())
            .collect(),
        frames: restored
            .rooms
            .iter()
            .flat_map(|room| room.frames.clone())
            .collect(),
        activations: restored.activations.clone(),
        activation_receipts: restored.activation_receipts.clone(),
        source_global_digest: source.global_digest.clone(),
        restored_global_digest: restored.global_digest.clone(),
    };
    let target = NativeRestoreTargetEvidenceV1 {
        backend: Some(BackendProfileV1::PostgresPrimary),
        deployment_lineage: Some(restored.deployment_lineage.clone()),
        storage_epoch: Some(restored.storage_epoch),
        native_point: Some(native_point),
        migration_contract: Some(restored.migration_contract.clone()),
        pack_identities: Some(restored.packs.clone()),
        resource_identities: Some(restored.resources.clone()),
        room_membership: Some(
            restored
                .rooms
                .iter()
                .map(|item| NativeRestoreRoomMembershipV1 {
                    room_id: item.room.room_id.clone(),
                    integrity: item.room.integrity.clone(),
                })
                .collect(),
        ),
        durable_domains: Some(native_durable_domain_evidence(source, restored)?),
    };
    Ok(NativeRestoreEvidenceV1::new(image, target))
}

fn native_durable_domain_evidence(
    source: &Capture,
    restored: &Capture,
) -> Result<Vec<NativeRestoreDurableDomainEvidenceV1>, NativePostgresError> {
    POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
        .into_iter()
        .map(|domain| {
            let source_rows = source
                .durable_domains
                .get(&domain)
                .cloned()
                .ok_or(NativePostgresError::Incomplete)?;
            let restored_rows = restored
                .durable_domains
                .get(&domain)
                .cloned()
                .ok_or(NativePostgresError::Incomplete)?;
            Ok(NativeRestoreDurableDomainEvidenceV1::new(
                domain,
                source_rows,
                restored_rows,
            ))
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn capture(
    config: &NativePostgresRestoreConfig,
    endpoint: &NativePostgresEndpointV1,
) -> Result<Capture, NativePostgresError> {
    let password = password_for(endpoint, &config.passfile)?;
    let mut client = connect(endpoint, &password)?;
    let version_num = client
        .query_one("SHOW server_version_num", &[])
        .map_err(|_| NativePostgresError::Database)?
        .get::<_, String>(0)
        .parse::<u32>()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let migration_contract = migration_contract(&mut client)?;
    let metadata = client
        .query_opt(
            "SELECT deployment_lineage_bytes, storage_epoch FROM worldstream_deployment_metadata WHERE target_id = true",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?
        .ok_or(NativePostgresError::Incomplete)?;
    let deployment_lineage = String::from_utf8(metadata.get::<_, Vec<u8>>(0))
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let storage_epoch =
        u64::try_from(metadata.get::<_, i64>(1)).map_err(|_| NativePostgresError::MalformedRow)?;
    let room_ids = client
        .query(
            "SELECT room_id, integrity_generation, integrity_status \
             FROM worldstream_room_roots ORDER BY room_id",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if room_ids.is_empty() {
        return Err(NativePostgresError::Incomplete);
    }
    let store = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            dsn_for(endpoint, &password),
            PostgresConnectionPath::Direct,
        )
        .map_err(|_| NativePostgresError::Configuration("runtime configuration failed"))?,
    )
    .map_err(|_| NativePostgresError::Configuration("runtime adapter failed"))?;
    let mut rooms = Vec::with_capacity(room_ids.len());
    for row in room_ids {
        let room_id: String = row
            .try_get(0)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let generation = u64::try_from(
            row.try_get::<_, i64>(1)
                .map_err(|_| NativePostgresError::MalformedRow)?,
        )
        .map_err(|_| NativePostgresError::MalformedRow)?;
        let status: String = row
            .try_get(2)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        if status == "healthy" {
            let verification = store
                .verify_room(&room_id)
                .map_err(|_| NativePostgresError::Database)?;
            rooms.push(room_capture(&mut client, &verification)?);
        } else {
            rooms.push(isolated_room_capture(
                &mut client,
                &room_id,
                generation,
                &status,
            )?);
        }
    }
    let (packs, resources, resource_blobs, _identity_digest) = identities(&mut client)?;
    let receipts = semantic_receipts(&mut client)?;
    let activations = activation_intents(&mut client)?;
    let activation_receipts = activation_operation_receipts(&mut client, &activations)?;
    let durable_domains = durable_domains(&mut client)?;
    let global_digest = durable_domains_digest(&durable_domains);
    let durable_digest = durable_digest(&DurableDigestInput {
        lineage: &deployment_lineage,
        epoch: storage_epoch,
        global_digest: &global_digest,
        migration: &migration_contract,
        packs: &packs,
        resources: &resources,
        rooms: &rooms,
        receipts: &receipts,
        activations: &activations,
        activation_receipts: &activation_receipts,
        durable_domains: &durable_domains,
    })?;
    let snapshot_count = rooms.iter().map(|room| room.snapshot_count).sum();
    Ok(Capture {
        version_num,
        deployment_lineage,
        storage_epoch,
        global_digest,
        migration_contract,
        packs,
        resources,
        resource_blobs,
        rooms,
        receipts,
        activations,
        activation_receipts,
        durable_domains,
        durable_digest,
        snapshot_count,
    })
}

fn isolated_room_capture(
    client: &mut Client,
    room_id: &str,
    generation: u64,
    status: &str,
) -> Result<RoomCapture, NativePostgresError> {
    let integrity_status = integrity_status(status)?;
    if integrity_status == IntegrityStatusV1::Healthy {
        return Err(NativePostgresError::MalformedRow);
    }
    let fingerprint = isolated_room_bytes_digest(client, room_id)?;
    let empty_digest = DigestV1::hash(&[]);
    let snapshot_count = usize::try_from(
        client
            .query_one(
                "SELECT count(*) FROM worldstream_room_snapshots WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(|_| NativePostgresError::Database)?
            .try_get::<_, i64>(0)
            .map_err(|_| NativePostgresError::MalformedRow)?,
    )
    .map_err(|_| NativePostgresError::MalformedRow)?;
    Ok(RoomCapture {
        room: RoomImageV1 {
            room_id: room_id.to_owned(),
            integrity: IntegrityWitnessV1 {
                source_status: integrity_status,
                restored_status: integrity_status,
                source_generation: generation,
                restored_generation: generation,
                source_isolated: true,
                restored_isolated: true,
            },
            head: CompleteHeadV1 {
                room_id: room_id.to_owned(),
                room_seq: 0,
                lineage_digest: empty_digest.clone(),
                core_schema_version: "isolated/raw".to_owned(),
                pack_revision_digest: empty_digest.clone(),
                core_state_digest: empty_digest.clone(),
                activity_state_digest: empty_digest.clone(),
                authoritative_state_digest: empty_digest,
            },
            records: Vec::new(),
            materialization: MaterializationV1 {
                core_state_bytes: Vec::new(),
                activity_state_bytes: Vec::new(),
                authoritative_state_bytes: Vec::new(),
            },
            source_bytes_digest: fingerprint.clone(),
            restored_bytes_digest: fingerprint,
        },
        timers: Vec::new(),
        frames: Vec::new(),
        snapshot_count,
    })
}

fn isolated_room_bytes_digest(
    client: &mut Client,
    room_id: &str,
) -> Result<DigestV1, NativePostgresError> {
    const QUERIES: &[(&str, &str)] = &[
        (
            "operation_guards",
            "SELECT jsonb_build_array(identity_bytes, request_hash, room_id, receipt_bytes)::text FROM worldstream_operation_guards WHERE room_id = $1 ORDER BY identity_bytes",
        ),
        (
            "room_roots",
            "SELECT jsonb_build_array(room_id, head_bytes, integrity_generation, integrity_status)::text FROM worldstream_room_roots WHERE room_id = $1 ORDER BY room_id",
        ),
        (
            "genesis",
            "SELECT jsonb_build_array(room_id, pack_revision_lock_bytes, genesis_bytes)::text FROM worldstream_genesis WHERE room_id = $1 ORDER BY room_id",
        ),
        (
            "materializations",
            "SELECT jsonb_build_array(room_id, core_state_bytes, activity_state_bytes)::text FROM worldstream_materializations WHERE room_id = $1 ORDER BY room_id",
        ),
        (
            "members",
            "SELECT jsonb_build_array(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through)::text FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
        ),
        (
            "timers",
            "SELECT jsonb_build_array(room_id, timer_id, generation, scheduled_for, payload_bytes, state)::text FROM worldstream_timers WHERE room_id = $1 ORDER BY timer_id, generation",
        ),
        (
            "transitions",
            "SELECT jsonb_build_array(room_id, room_seq, transition_bytes)::text FROM worldstream_transitions WHERE room_id = $1 ORDER BY room_seq",
        ),
        (
            "frames",
            "SELECT jsonb_build_array(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash)::text FROM worldstream_frames WHERE room_id = $1 ORDER BY member_id, frame_seq",
        ),
        (
            "observation_consequences",
            "SELECT jsonb_build_array(room_id, member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash)::text FROM worldstream_observation_consequences WHERE room_id = $1 ORDER BY member_id, cause_room_seq",
        ),
        (
            "activation_decisions",
            "SELECT jsonb_build_array(room_id, cause_room_seq, decision_id, target_member_id, decision_bytes)::text FROM worldstream_activation_decisions WHERE room_id = $1 ORDER BY cause_room_seq, decision_id",
        ),
        (
            "activation_intents",
            "SELECT jsonb_build_array(activation_id, room_id, cause_room_seq, decision_id, target_member_id, reason_code, deduplication_key, priority, semantic_deadline, policy_revision, state, intent_generation, lease_generation, runner_id, claim_id, lease_until, context_hash, context_bytes, context_retired)::text FROM worldstream_activation_intents WHERE room_id = $1 ORDER BY activation_id",
        ),
        (
            "activation_receipts",
            "SELECT jsonb_build_array(room_id, operation_id, operation_kind, canonical_request_hash, activation_id, result_code, result_bytes, context_hash, context_bytes)::text FROM worldstream_activation_operation_receipts WHERE room_id = $1 ORDER BY operation_id",
        ),
        (
            "semantic_receipts",
            "SELECT jsonb_build_array(identity_bytes, operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes, room_id)::text FROM worldstream_semantic_receipts WHERE room_id = $1 ORDER BY identity_bytes",
        ),
        (
            "integrity_incidents",
            "SELECT jsonb_build_array(room_id, incident_seq, generation, status, reason_code, details_bytes)::text FROM worldstream_integrity_incidents WHERE room_id = $1 ORDER BY incident_seq",
        ),
    ];
    let limits = VerifierLimits::default();
    let mut bytes = b"worldstream/isolated-room-raw/v1\0".to_vec();
    let mut count = 0usize;
    for (domain, query) in QUERIES {
        bytes.extend_from_slice(domain.as_bytes());
        bytes.push(0);
        let rows = client
            .query(*query, &[&room_id])
            .map_err(|_| NativePostgresError::Database)?;
        count = count
            .checked_add(rows.len())
            .ok_or(NativePostgresError::Incomplete)?;
        if count > limits.max_ledger_rows {
            return Err(NativePostgresError::Incomplete);
        }
        bytes.extend_from_slice(&(rows.len() as u64).to_be_bytes());
        for row in rows {
            let text: String = row
                .try_get(0)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let canonical = CanonicalJsonV1::parse(text.as_bytes())
                .and_then(|value| value.to_bytes())
                .map_err(|_| NativePostgresError::MalformedRow)?;
            if canonical.len() > limits.max_object_bytes {
                return Err(NativePostgresError::Incomplete);
            }
            bytes.extend_from_slice(&(canonical.len() as u64).to_be_bytes());
            bytes.extend_from_slice(&canonical);
        }
    }
    Ok(DigestV1::hash(&bytes))
}

fn room_capture(
    client: &mut Client,
    verification: &crate::PostgresRoomVerification,
) -> Result<RoomCapture, NativePostgresError> {
    let genesis = GenesisV1::from_canonical_bytes(&verification.genesis_bytes)
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let mut records = vec![CanonicalRecordV1 {
        kind: CanonicalRecordKindV1::Genesis,
        room_seq: 0,
        bytes: verification.genesis_bytes.clone(),
        digest: core_digest(&genesis.genesis_hash())?,
        previous_digest: None,
    }];
    for (index, bytes) in verification.transition_bytes.iter().enumerate() {
        let transition = TransitionV1::from_canonical_bytes(bytes)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        records.push(CanonicalRecordV1 {
            kind: CanonicalRecordKindV1::Transition,
            room_seq: u64::try_from(index + 1).map_err(|_| NativePostgresError::MalformedRow)?,
            bytes: bytes.clone(),
            digest: core_digest(&transition.transition_hash())?,
            previous_digest: Some(core_digest(&transition.previous_lineage_hash())?),
        });
    }
    let head = convert_head(&verification.head)?;
    let authoritative = authoritative_bytes(&verification.head)?;
    let fingerprint = room_bytes_digest(verification);
    let integrity = IntegrityWitnessV1 {
        source_status: integrity_status(&verification.integrity_status)?,
        restored_status: integrity_status(&verification.integrity_status)?,
        source_generation: verification.integrity_generation,
        restored_generation: verification.integrity_generation,
        source_isolated: !verification
            .integrity_status
            .eq_ignore_ascii_case("healthy"),
        restored_isolated: !verification
            .integrity_status
            .eq_ignore_ascii_case("healthy"),
    };
    let room = RoomImageV1 {
        room_id: verification.head.room_id().to_string(),
        integrity,
        head,
        records,
        materialization: MaterializationV1 {
            core_state_bytes: verification.core_state_bytes.clone(),
            activity_state_bytes: verification.activity_state_bytes.clone(),
            authoritative_state_bytes: authoritative,
        },
        source_bytes_digest: fingerprint.clone(),
        restored_bytes_digest: fingerprint,
    };
    let (timers, frames) = operational_rows(client, verification, &room.room_id)?;
    Ok(RoomCapture {
        room,
        timers,
        frames,
        snapshot_count: verification.snapshots.len(),
    })
}

#[allow(clippy::too_many_lines)]
fn operational_rows(
    client: &mut Client,
    verification: &crate::PostgresRoomVerification,
    room_id: &str,
) -> Result<(Vec<TimerV1>, Vec<FrameV1>), NativePostgresError> {
    let fired_receipts = client
        .query(
            "SELECT receipt_bytes FROM worldstream_semantic_receipts \
             WHERE room_id = $1 AND operation_kind = 'timer_fired' ORDER BY identity_bytes",
            &[&room_id],
        )
        .map_err(|_| NativePostgresError::Database)?
        .into_iter()
        .map(|row| {
            let bytes: Vec<u8> = row
                .try_get(0)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let receipt = StoredSemanticResultV1::from_canonical_receipt_bytes(&bytes)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let OperationIdentityV1::TimerFired(identity) = receipt.operation_identity() else {
                return Err(NativePostgresError::MalformedRow);
            };
            let transition_seq = receipt
                .transition_seq()
                .map(worldstream_core::RoomSequenceV1::get)
                .ok_or(NativePostgresError::Incomplete)?;
            Ok((
                (
                    identity.timer_id.to_string(),
                    identity.generation.get(),
                    identity.scheduled_for.as_str().to_owned(),
                ),
                transition_seq,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, NativePostgresError>>()?;
    let member_rows = client
        .query(
            "SELECT member_id, frame_head, last_ack_frame_seq FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
            &[&room_id],
        )
        .map_err(|_| NativePostgresError::Database)?;
    let frame_heads = member_rows
        .into_iter()
        .map(|row| {
            let member_id: String = row.get(0);
            let frame_head = u64::try_from(row.get::<_, i64>(1))
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let cursor = row
                .get::<_, Option<i64>>(2)
                .map(|value| u64::try_from(value).map_err(|_| NativePostgresError::MalformedRow))
                .transpose()?;
            Ok((member_id, (frame_head, cursor)))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, NativePostgresError>>()?;
    let timers = verification
        .timers
        .iter()
        .map(|timer| {
            let (state, fired_transition_seq) = match timer.state.as_str() {
                "scheduled" => (TimerStateV1::Scheduled, None),
                "cancelled" => (TimerStateV1::Cancelled, None),
                "fired" => {
                    let key = (
                        timer.timer_id.clone(),
                        timer.generation,
                        timer.scheduled_for.clone(),
                    );
                    (
                        TimerStateV1::Fired,
                        Some(
                            *fired_receipts
                                .get(&key)
                                .ok_or(NativePostgresError::Incomplete)?,
                        ),
                    )
                }
                _ => return Err(NativePostgresError::MalformedRow),
            };
            Ok(TimerV1 {
                room_id: room_id.to_owned(),
                timer_id: timer.timer_id.clone(),
                generation: timer.generation,
                scheduled_for: timer.scheduled_for.clone(),
                payload_digest: DigestV1::hash(&timer.payload_bytes),
                payload_bytes: timer.payload_bytes.clone(),
                state,
                fired_transition_seq,
            })
        })
        .collect::<Result<Vec<_>, NativePostgresError>>()?;
    let frames = verification
        .frames
        .iter()
        .map(|frame| {
            let Some((frame_head, cursor)) = frame_heads.get(&frame.member_id) else {
                return Err(NativePostgresError::Incomplete);
            };
            if digest_from_bytes(&frame.payload_hash)? != DigestV1::hash(&frame.payload_bytes) {
                return Err(NativePostgresError::Incomplete);
            }
            Ok(FrameV1 {
                room_id: room_id.to_owned(),
                member_id: frame.member_id.clone(),
                frame_seq: frame.frame_seq,
                cause_room_seq: frame.cause_room_seq,
                payload_bytes: frame.payload_bytes.clone(),
                payload_digest: DigestV1::hash(&frame.payload_bytes),
                frame_head: *frame_head,
                cursor: *cursor,
            })
        })
        .collect::<Result<Vec<_>, NativePostgresError>>()?;
    Ok((timers, frames))
}

#[allow(clippy::too_many_lines)]
fn identities(
    client: &mut Client,
) -> Result<
    (
        Vec<PackIdentityV1>,
        Vec<ResourceIdentityV1>,
        Vec<ResourceBlobV1>,
        DigestV1,
    ),
    NativePostgresError,
> {
    let pack_rows = client
        .query("SELECT pack_id, revision, pack_digest FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision", &[])
        .map_err(|_| NativePostgresError::Database)?;
    let resource_rows = client
        .query("SELECT resource_kind, resource_identity, size_bytes, resource_digest FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity", &[])
        .map_err(|_| NativePostgresError::Database)?;
    let identity_rows = client
        .query(
            "SELECT identity_digest, pack_set_digest, resource_set_digest, canonical_bytes FROM worldstream_deployment_identity_metadata WHERE target_id = true",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if identity_rows.len() != 1 {
        return Err(NativePostgresError::Incomplete);
    }
    let identity_row = &identity_rows[0];
    let transfer_packs = pack_rows
        .iter()
        .map(|row| {
            TransferPackIdentityV1::new(
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                TransferDigestV1::from_bytes(&row.get::<_, Vec<u8>>(2))
                    .map_err(|_| NativePostgresError::MalformedRow)?,
            )
            .map_err(|_| NativePostgresError::MalformedRow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let transfer_resources = resource_rows
        .iter()
        .map(|row| {
            let kind = match row.get::<_, String>(0).as_str() {
                "artifact" => TransferResourceKindV1::Artifact,
                "codec" => TransferResourceKindV1::Codec,
                "schema" => TransferResourceKindV1::Schema,
                _ => return Err(NativePostgresError::MalformedRow),
            };
            TransferResourceIdentityV1::from_persisted_parts(
                kind,
                row.get::<_, String>(1),
                u64::try_from(row.get::<_, i64>(2))
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                TransferDigestV1::from_bytes(&row.get::<_, Vec<u8>>(3))
                    .map_err(|_| NativePostgresError::MalformedRow)?,
            )
            .map_err(|_| NativePostgresError::MalformedRow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let transfer_identity = TransferDeploymentIdentityV1::new(transfer_packs, transfer_resources)
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let canonical_identity = transfer_identity
        .canonical_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let pack_set_digest = transfer_pack_set_digest(&transfer_identity);
    let resource_set_digest = transfer_resource_set_digest(&transfer_identity);
    if identity_row.get::<_, Vec<u8>>(0) != transfer_identity.digest().as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(1) != pack_set_digest.as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(2) != resource_set_digest.as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(3) != canonical_identity
    {
        return Err(NativePostgresError::Incomplete);
    }
    let blob_rows = client
        .query(
            "SELECT resource_kind, resource_identity, resource_bytes, resource_digest FROM worldstream_deployment_resource_blobs ORDER BY resource_kind, resource_identity",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if blob_rows.len() != transfer_identity.resources().len() {
        return Err(NativePostgresError::Incomplete);
    }
    let mut resources = Vec::with_capacity(blob_rows.len());
    let mut resource_blobs = Vec::with_capacity(blob_rows.len());
    for (row, expected) in blob_rows.iter().zip(transfer_identity.resources()) {
        let kind: String = row.get(0);
        let resource_id: String = row.get(1);
        let bytes: Vec<u8> = row.get(2);
        let digest_bytes: Vec<u8> = row.get(3);
        let expected_kind = match expected.kind() {
            TransferResourceKindV1::Artifact => "artifact",
            TransferResourceKindV1::Codec => "codec",
            TransferResourceKindV1::Schema => "schema",
        };
        if kind != expected_kind
            || resource_id != expected.identity()
            || digest_bytes != expected.digest().as_bytes()
            || expected.verify_bytes(&bytes).is_err()
        {
            return Err(NativePostgresError::Incomplete);
        }
        resources.push(ResourceIdentityV1 {
            resource_id: resource_id.clone(),
            kind,
            byte_len: u64::try_from(bytes.len()).map_err(|_| NativePostgresError::Incomplete)?,
            digest: DigestV1::hash(&bytes),
        });
        resource_blobs.push(ResourceBlobV1 { resource_id, bytes });
    }
    // Pack component identities come from the exact persisted revision lock,
    // not from semantic replay. That makes an isolated-only retained Pack
    // recoverable without decoding or serving the isolated Room, while a
    // missing/malformed lock still fails readiness closed.
    let pack_lock_rows = client
        .query(
            "SELECT room_id, pack_revision_lock_bytes FROM worldstream_genesis ORDER BY room_id",
            &[],
        )
        .map_err(|_| NativePostgresError::Database)?;
    let pack_lock_bytes = pack_lock_rows
        .iter()
        .map(|row| {
            row.try_get::<_, Vec<u8>>(1)
                .map_err(|_| NativePostgresError::MalformedRow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let retained_locks = retained_pack_locks(&transfer_identity, &pack_lock_bytes)?;
    let mut packs = Vec::with_capacity(retained_locks.len());
    for (transfer_pack, lock) in transfer_identity.packs().iter().zip(retained_locks) {
        let revision_digest = DigestV1::parse(transfer_pack.digest().to_string())
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let executor_digest = core_digest(&lock.rule_source_digest)?;
        let schema_bundle_digest = core_digest(&lock.schema_bundle_digest)?;
        let codec_bundle_digest = core_digest(&lock.codec_bundle_digest)?;
        let resource_ids = resources
            .iter()
            .filter(|resource| {
                resource.digest == executor_digest
                    || resource.digest == schema_bundle_digest
                    || resource.digest == codec_bundle_digest
            })
            .map(|resource| resource.resource_id.clone())
            .collect();
        packs.push(PackIdentityV1 {
            pack_id: transfer_pack.pack_id().to_owned(),
            revision_digest,
            executor_digest,
            schema_bundle_digest,
            codec_bundle_digest,
            resource_ids,
        });
    }
    Ok((
        packs,
        resources,
        resource_blobs,
        DigestV1::parse(transfer_identity.digest().to_string())
            .map_err(|_| NativePostgresError::MalformedRow)?,
    ))
}

/// Resolves every retained Pack from exact persisted revision-lock bytes.
///
/// Every Genesis row must itself be a valid lock for exactly one published
/// deployment Pack. This is intentionally stronger than finding one usable
/// lock per Pack: a malformed lock owned only by an isolated Room is durable
/// corruption and must fail restore readiness rather than being ignored when
/// another Room happens to use the same Pack.
fn retained_pack_locks(
    identity: &TransferDeploymentIdentityV1,
    lock_rows: &[Vec<u8>],
) -> Result<Vec<worldstream_core::PackRevisionLockV1>, NativePostgresError> {
    let expected = identity
        .packs()
        .iter()
        .map(|pack| {
            let digest = DigestV1::parse(pack.digest().to_string())
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let pack_digest =
                worldstream_core::PackDigestV1::from_str(&format!("blake3:{}", digest.as_str()))
                    .map_err(|_| NativePostgresError::MalformedRow)?;
            Ok((pack, pack_digest))
        })
        .collect::<Result<Vec<_>, NativePostgresError>>()?;
    let mut observed = BTreeMap::<(String, String), worldstream_core::PackRevisionLockV1>::new();
    for bytes in lock_rows {
        let mut matches = expected.iter().filter_map(|(pack, digest)| {
            let lock =
                worldstream_core::PackRevisionLockV1::from_canonical_bytes(bytes, digest).ok()?;
            (lock.pack_id == pack.pack_id() && lock.revision_lock_id == pack.revision())
                .then_some((pack, lock))
        });
        let Some((pack, lock)) = matches.next() else {
            return Err(NativePostgresError::Incomplete);
        };
        if matches.next().is_some() {
            return Err(NativePostgresError::Incomplete);
        }
        let key = (pack.pack_id().to_owned(), pack.revision().to_owned());
        if let Some(prior) = observed.get(&key) {
            if prior
                .canonical_bytes()
                .map_err(|_| NativePostgresError::MalformedRow)?
                != lock
                    .canonical_bytes()
                    .map_err(|_| NativePostgresError::MalformedRow)?
            {
                return Err(NativePostgresError::Incomplete);
            }
        } else {
            observed.insert(key, lock);
        }
    }
    identity
        .packs()
        .iter()
        .map(|pack| {
            observed
                .remove(&(pack.pack_id().to_owned(), pack.revision().to_owned()))
                .ok_or(NativePostgresError::Incomplete)
        })
        .collect()
}

fn transfer_resource_set_digest(identity: &TransferDeploymentIdentityV1) -> TransferDigestV1 {
    let mut bytes = b"worldstream/deployment-resource-set/v1".to_vec();
    for resource in identity.resources() {
        bytes.push(match resource.kind() {
            TransferResourceKindV1::Artifact => 1,
            TransferResourceKindV1::Codec => 2,
            TransferResourceKindV1::Schema => 3,
        });
        bytes.extend_from_slice(resource.identity().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&resource.size_bytes().to_le_bytes());
        bytes.extend_from_slice(&resource.digest().as_bytes());
    }
    TransferDigestV1::hash(&bytes)
}

fn transfer_pack_set_digest(identity: &TransferDeploymentIdentityV1) -> TransferDigestV1 {
    let mut bytes = b"worldstream/deployment-pack-set/v1".to_vec();
    for pack in identity.packs() {
        bytes.extend_from_slice(pack.pack_id().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(pack.revision().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&pack.digest().as_bytes());
    }
    TransferDigestV1::hash(&bytes)
}

fn digest_from_bytes(bytes: &[u8]) -> Result<DigestV1, NativePostgresError> {
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(&mut text, "{byte:02x}").map_err(|_| NativePostgresError::MalformedRow)?;
    }
    DigestV1::parse(text).map_err(|_| NativePostgresError::MalformedRow)
}

const DURABLE_DOMAIN_QUERIES: [(NativeRestoreDurableDomainV1, &str); 33] = [
    (
        NativeRestoreDurableDomainV1::SchemaMigrations,
        "SELECT jsonb_build_array(version, migration_id, checksum, logical_history_id, schema_contract_fingerprint)::text FROM worldstream_schema_migrations ORDER BY version",
    ),
    (
        NativeRestoreDurableDomainV1::OperationGuards,
        "SELECT jsonb_build_array(identity_bytes, request_hash, room_id, receipt_bytes)::text FROM worldstream_operation_guards ORDER BY identity_bytes",
    ),
    (
        NativeRestoreDurableDomainV1::RoomRoots,
        "SELECT jsonb_build_array(room_id, head_bytes, integrity_generation, integrity_status)::text FROM worldstream_room_roots ORDER BY room_id",
    ),
    (
        NativeRestoreDurableDomainV1::Genesis,
        "SELECT jsonb_build_array(room_id, pack_revision_lock_bytes, genesis_bytes)::text FROM worldstream_genesis ORDER BY room_id",
    ),
    (
        NativeRestoreDurableDomainV1::Materializations,
        "SELECT jsonb_build_array(room_id, core_state_bytes, activity_state_bytes)::text FROM worldstream_materializations ORDER BY room_id",
    ),
    (
        NativeRestoreDurableDomainV1::MemberDeliveryState,
        "SELECT jsonb_build_array(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through)::text FROM worldstream_members ORDER BY room_id, member_id",
    ),
    (
        NativeRestoreDurableDomainV1::Timers,
        "SELECT jsonb_build_array(room_id, timer_id, generation, scheduled_for, payload_bytes, state)::text FROM worldstream_timers ORDER BY room_id, timer_id, generation",
    ),
    (
        NativeRestoreDurableDomainV1::Transitions,
        "SELECT jsonb_build_array(room_id, room_seq, transition_bytes)::text FROM worldstream_transitions ORDER BY room_id, room_seq",
    ),
    (
        NativeRestoreDurableDomainV1::Frames,
        "SELECT jsonb_build_array(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash)::text FROM worldstream_frames ORDER BY room_id, member_id, frame_seq",
    ),
    (
        NativeRestoreDurableDomainV1::ObservationConsequences,
        "SELECT jsonb_build_array(room_id, member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash)::text FROM worldstream_observation_consequences ORDER BY room_id, member_id, cause_room_seq",
    ),
    (
        NativeRestoreDurableDomainV1::ActivationDecisions,
        "SELECT jsonb_build_array(room_id, cause_room_seq, decision_id, target_member_id, decision_bytes)::text FROM worldstream_activation_decisions ORDER BY room_id, cause_room_seq, decision_id",
    ),
    (
        NativeRestoreDurableDomainV1::ActivationIntents,
        "SELECT jsonb_build_array(activation_id, room_id, cause_room_seq, decision_id, target_member_id, reason_code, deduplication_key, priority, semantic_deadline, policy_revision, state, intent_generation, lease_generation, runner_id, claim_id, lease_until, context_hash, context_bytes, context_retired)::text FROM worldstream_activation_intents ORDER BY activation_id",
    ),
    (
        NativeRestoreDurableDomainV1::ActivationOperationReceipts,
        "SELECT jsonb_build_array(room_id, operation_id, operation_kind, canonical_request_hash, activation_id, result_code, result_bytes, context_hash, context_bytes)::text FROM worldstream_activation_operation_receipts ORDER BY room_id, operation_id",
    ),
    (
        NativeRestoreDurableDomainV1::SemanticReceipts,
        "SELECT jsonb_build_array(identity_bytes, operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes, room_id)::text FROM worldstream_semantic_receipts ORDER BY identity_bytes",
    ),
    (
        NativeRestoreDurableDomainV1::IntegrityIncidents,
        "SELECT jsonb_build_array(room_id, incident_seq, generation, status, reason_code, details_bytes)::text FROM worldstream_integrity_incidents ORDER BY room_id, incident_seq",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityFences,
        "SELECT jsonb_build_array(witness_id, authenticated_principal, generation, scope_revocation_hash, active)::text FROM worldstream_authority_fences ORDER BY witness_id",
    ),
    (
        NativeRestoreDurableDomainV1::RetiredAuthorityFences,
        "SELECT jsonb_build_array(witness_id, authenticated_principal, generation, scope_revocation_bytes, scope_revocation_hash, active)::text FROM worldstream_retired_authority_fences_v1 ORDER BY witness_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityState,
        "SELECT jsonb_build_array(authority_id)::text FROM worldstream_authority_state ORDER BY authority_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityPrincipals,
        "SELECT jsonb_build_array(principal_id, principal_kind, authority_status, principal_generation)::text FROM worldstream_authority_principals ORDER BY principal_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityRunners,
        "SELECT jsonb_build_array(runner_id, owner_principal_id, authority_status, runner_generation)::text FROM worldstream_authority_runners ORDER BY runner_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityCapabilities,
        "SELECT jsonb_build_array(capability_id, token_hash, principal_id, profile_kind, target_room_id, target_member_id, runner_id, authority_generation, expires_at, revoked_at)::text FROM worldstream_authority_capabilities ORDER BY capability_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityCapabilityScopes,
        "SELECT jsonb_build_array(capability_id, scope)::text FROM worldstream_authority_capability_scopes ORDER BY capability_id, scope",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityRunnerCapabilityMemberships,
        "SELECT jsonb_build_array(capability_id, room_id, member_id)::text FROM worldstream_authority_runner_capability_memberships ORDER BY capability_id, room_id, member_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityChangeReceipts,
        "SELECT jsonb_build_array(change_id, authenticated_principal, request_hash, result_kind, target_kind, target_id, secondary_target_id, resulting_generation, checked_at)::text FROM worldstream_authority_change_receipts ORDER BY change_id",
    ),
    (
        NativeRestoreDurableDomainV1::AuthorityAudit,
        "SELECT jsonb_build_array(audit_seq, change_id, actor_principal_id, target_kind, target_id, secondary_target_id, change_kind, prior_generation, resulting_generation, checked_at, reason_code, request_hash)::text FROM worldstream_authority_audit ORDER BY audit_seq",
    ),
    (
        NativeRestoreDurableDomainV1::TransferImports,
        "SELECT jsonb_build_array(bundle_hash, target_fingerprint, state)::text FROM worldstream_transfer_imports ORDER BY bundle_hash",
    ),
    (
        NativeRestoreDurableDomainV1::TransferChunks,
        "SELECT jsonb_build_array(bundle_hash, chunk_start, chunk_end, chunk_digest, records_bytes)::text FROM worldstream_transfer_chunks ORDER BY bundle_hash, chunk_start",
    ),
    (
        NativeRestoreDurableDomainV1::TransferTargetFence,
        "SELECT jsonb_build_array(fence_id, bundle_hash, target_fingerprint)::text FROM worldstream_transfer_target_fence ORDER BY fence_id",
    ),
    (
        NativeRestoreDurableDomainV1::DeploymentMetadata,
        "SELECT jsonb_build_array(target_id, deployment_lineage_bytes, storage_epoch_bytes, storage_epoch)::text FROM worldstream_deployment_metadata ORDER BY target_id",
    ),
    (
        NativeRestoreDurableDomainV1::DeploymentIdentityMetadata,
        "SELECT jsonb_build_array(target_id, identity_digest, pack_set_digest, resource_set_digest, canonical_bytes)::text FROM worldstream_deployment_identity_metadata ORDER BY target_id",
    ),
    (
        NativeRestoreDurableDomainV1::DeploymentPackIdentities,
        "SELECT jsonb_build_array(pack_id, revision, pack_digest)::text FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision",
    ),
    (
        NativeRestoreDurableDomainV1::DeploymentResourceIdentities,
        "SELECT jsonb_build_array(resource_kind, resource_identity, size_bytes, resource_digest)::text FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity",
    ),
    (
        NativeRestoreDurableDomainV1::DeploymentResourceBlobs,
        "SELECT jsonb_build_array(resource_kind, resource_identity, resource_bytes, resource_digest)::text FROM worldstream_deployment_resource_blobs ORDER BY resource_kind, resource_identity",
    ),
];

fn durable_domains(
    client: &mut Client,
) -> Result<
    BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
    NativePostgresError,
> {
    let limits = VerifierLimits::default();
    let mut remaining = limits.max_ledger_rows;
    let mut result = BTreeMap::new();
    for (domain, query) in DURABLE_DOMAIN_QUERIES {
        let row_limit = remaining
            .checked_add(1)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or(NativePostgresError::Incomplete)?;
        let rows = client
            .query(&format!("{query} LIMIT $1"), &[&row_limit])
            .map_err(|_| NativePostgresError::Database)?;
        if rows.len() > remaining {
            return Err(NativePostgresError::Incomplete);
        }
        remaining -= rows.len();
        let canonical = rows
            .into_iter()
            .map(|row| {
                let text: String = row
                    .try_get(0)
                    .map_err(|_| NativePostgresError::MalformedRow)?;
                if text.len() > limits.max_object_bytes {
                    return Err(NativePostgresError::Incomplete);
                }
                let bytes = CanonicalJsonV1::parse(text.as_bytes())
                    .and_then(|value| value.to_bytes())
                    .map_err(|_| NativePostgresError::MalformedRow)?;
                Ok(NativeRestoreCanonicalRowV1::new(bytes))
            })
            .collect::<Result<Vec<_>, NativePostgresError>>()?;
        if result.insert(domain, canonical).is_some() {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if result.len() != POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1.len() {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(result)
}

fn activation_intents(client: &mut Client) -> Result<Vec<ActivationV1>, NativePostgresError> {
    let limits = VerifierLimits::default();
    let row_limit = i64::try_from(limits.max_ledger_rows.saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let rows = client
        .query(
            "SELECT a.activation_id, a.room_id, a.cause_room_seq, a.target_member_id, a.state, a.intent_generation, a.lease_generation, a.runner_id, a.claim_id, a.lease_until, a.context_hash, a.context_bytes, a.context_retired \
             FROM worldstream_activation_intents a \
             JOIN worldstream_room_roots r ON r.room_id = a.room_id \
             WHERE r.integrity_status = 'healthy' ORDER BY a.activation_id LIMIT $1",
            &[&row_limit],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if rows.len() > limits.max_ledger_rows {
        return Err(NativePostgresError::Incomplete);
    }
    rows.into_iter()
        .map(|row| {
            let context_hash = row
                .try_get::<_, Option<Vec<u8>>>(10)
                .map_err(|_| NativePostgresError::MalformedRow)?
                .map(|bytes| digest_from_bytes(&bytes))
                .transpose()?;
            let context_bytes = row
                .try_get::<_, Option<Vec<u8>>>(11)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            if context_bytes
                .as_ref()
                .is_some_and(|bytes| bytes.len() > limits.max_object_bytes)
            {
                return Err(NativePostgresError::Incomplete);
            }
            let retired = row
                .try_get::<_, bool>(12)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let context = match (context_hash.as_ref(), context_bytes) {
                (None, None) if !retired => ContextRetentionV1::None,
                (Some(digest), Some(bytes)) if !retired && DigestV1::hash(&bytes) == *digest => {
                    ContextRetentionV1::Retained(bytes)
                }
                (Some(_), None) if retired => ContextRetentionV1::Tombstone,
                _ => return Err(NativePostgresError::Incomplete),
            };
            let state = match row
                .try_get::<_, String>(4)
                .map_err(|_| NativePostgresError::MalformedRow)?
                .as_str()
            {
                "pending" => ActivationStateV1::Pending,
                "leased" => ActivationStateV1::Leased,
                "completed" => ActivationStateV1::Completed,
                "expired" => ActivationStateV1::Expired,
                "cancelled" => ActivationStateV1::Cancelled,
                _ => return Err(NativePostgresError::MalformedRow),
            };
            Ok(ActivationV1 {
                activation_id: row
                    .try_get(0)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                room_id: row
                    .try_get(1)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                cause_room_seq: u64::try_from(
                    row.try_get::<_, i64>(2)
                        .map_err(|_| NativePostgresError::MalformedRow)?,
                )
                .map_err(|_| NativePostgresError::MalformedRow)?,
                target_member_id: row
                    .try_get(3)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                state,
                intent_generation: u64::try_from(
                    row.try_get::<_, i64>(5)
                        .map_err(|_| NativePostgresError::MalformedRow)?,
                )
                .map_err(|_| NativePostgresError::MalformedRow)?,
                lease_generation: u64::try_from(
                    row.try_get::<_, i64>(6)
                        .map_err(|_| NativePostgresError::MalformedRow)?,
                )
                .map_err(|_| NativePostgresError::MalformedRow)?,
                runner_id: row
                    .try_get(7)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                claim_id: row
                    .try_get(8)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                lease_until: row
                    .try_get(9)
                    .map_err(|_| NativePostgresError::MalformedRow)?,
                context_digest: context_hash,
                context,
            })
        })
        .collect()
}

fn activation_operation_receipts(
    client: &mut Client,
    activations: &[ActivationV1],
) -> Result<Vec<ReceiptV1>, NativePostgresError> {
    let limits = VerifierLimits::default();
    let row_limit = i64::try_from(limits.max_ledger_rows.saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let rows = client
        .query(
            "SELECT a.room_id, a.operation_id, a.operation_kind, a.canonical_request_hash, a.activation_id, a.result_code, a.result_bytes, a.context_hash, a.context_bytes \
             FROM worldstream_activation_operation_receipts a \
             JOIN worldstream_room_roots r ON r.room_id = a.room_id \
             WHERE r.integrity_status = 'healthy' ORDER BY a.room_id, a.operation_id LIMIT $1",
            &[&row_limit],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if rows.len() > limits.max_ledger_rows {
        return Err(NativePostgresError::Incomplete);
    }
    rows.into_iter()
        .map(|row| {
            let room_id: String = row
                .try_get(0)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let operation_id: String = row
                .try_get(1)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let operation_kind: String = row
                .try_get(2)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let request_hash: Vec<u8> = row
                .try_get(3)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let activation_id: Option<String> = row
                .try_get(4)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let result_code: String = row
                .try_get(5)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let result_bytes: Vec<u8> = row
                .try_get(6)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let context_hash: Option<Vec<u8>> = row
                .try_get(7)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let context_digest = context_hash.as_deref().map(digest_from_bytes).transpose()?;
            let context_bytes: Option<Vec<u8>> = row
                .try_get(8)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            if result_bytes.len() > limits.max_object_bytes
                || context_bytes
                    .as_ref()
                    .is_some_and(|bytes| bytes.len() > limits.max_object_bytes)
            {
                return Err(NativePostgresError::Incomplete);
            }
            let result =
                CanonicalJsonV1::decode_canonical::<ActivationOperationResultV1>(&result_bytes)
                    .map_err(|_| NativePostgresError::MalformedRow)?;
            let expected_context_bytes = result
                .context
                .as_ref()
                .map(worldstream_core::ActivationInvocationContextV1::canonical_bytes)
                .transpose()
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let expected_context_hash = result
                .context_hash
                .as_ref()
                .map(|digest| digest.as_bytes().to_vec());
            let relation_valid = match (operation_kind.as_str(), activation_id.as_ref()) {
                ("offer", None) => true,
                ("claim" | "renew" | "complete" | "release", Some(id)) => activations
                    .iter()
                    .any(|activation| &activation.activation_id == id),
                _ => false,
            };
            if result.operation_id != operation_id
                || result.activation_id != activation_id
                || format!("{:?}", result.code).to_lowercase() != result_code
                || expected_context_hash != context_hash
                || expected_context_bytes != context_bytes
                || context_bytes
                    .as_ref()
                    .is_some_and(|bytes| context_digest.as_ref() != Some(&DigestV1::hash(bytes)))
                || !relation_valid
            {
                return Err(NativePostgresError::Incomplete);
            }
            Ok(ReceiptV1 {
                kind: ReceiptKindV1::ActivationOperation,
                identity_bytes: activation_operation_receipt_identity(&room_id, &operation_id)?,
                request_bytes: Vec::new(),
                request_bytes_available: false,
                request_digest: digest_from_bytes(&request_hash)?,
                result_digest: DigestV1::hash(&result_bytes),
                result_bytes,
                room_id: Some(room_id),
                transition_seq: None,
                activation_id,
                operation_kind: Some(operation_kind),
            })
        })
        .collect()
}

fn durable_domains_digest(
    domains: &BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
) -> DigestV1 {
    let mut bytes = b"worldstream/native-postgres-durable-domains/v1\0".to_vec();
    for domain in POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1 {
        bytes.extend_from_slice(domain.as_str().as_bytes());
        bytes.push(0);
        let rows = domains.get(&domain).map(Vec::as_slice).unwrap_or_default();
        bytes.extend_from_slice(&(rows.len() as u64).to_be_bytes());
        for row in rows {
            bytes.extend_from_slice(row.digest.as_str().as_bytes());
        }
    }
    DigestV1::hash(&bytes)
}

fn fired_timer_count(capture: &Capture) -> usize {
    capture
        .rooms
        .iter()
        .flat_map(|room| &room.timers)
        .filter(|timer| timer.state == TimerStateV1::Fired)
        .count()
}

fn authority_domains_equal(
    source: &BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
    restored: &BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
) -> bool {
    const AUTHORITY_DOMAINS: [NativeRestoreDurableDomainV1; 11] = [
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
        NativeRestoreDurableDomainV1::TransferTargetFence,
    ];
    AUTHORITY_DOMAINS
        .iter()
        .all(|domain| source.get(domain) == restored.get(domain))
}

#[derive(Serialize)]
struct ActivationOperationReceiptIdentity<'a> {
    operation_id: &'a str,
    room_id: &'a str,
}

fn activation_operation_receipt_identity(
    room_id: &str,
    operation_id: &str,
) -> Result<Vec<u8>, NativePostgresError> {
    let bytes = serde_json::to_vec(&ActivationOperationReceiptIdentity {
        operation_id,
        room_id,
    })
    .map_err(|_| NativePostgresError::Incomplete)?;
    CanonicalJsonV1::from_canonical_bytes(&bytes).map_err(|_| NativePostgresError::Incomplete)?;
    Ok(bytes)
}

fn semantic_receipts(client: &mut Client) -> Result<Vec<ReceiptV1>, NativePostgresError> {
    let rows = client
        .query("SELECT s.identity_bytes, s.operation_kind, s.canonical_request_hash, s.basis_complete_head_bytes, s.semantic_input_bytes, s.semantic_time_bytes, s.resolution_kind, s.transition_seq, s.receipt_bytes, s.room_id \
                FROM worldstream_semantic_receipts s \
                JOIN worldstream_room_roots r ON r.room_id = s.room_id \
                WHERE r.integrity_status = 'healthy' ORDER BY s.identity_bytes", &[])
        .map_err(|_| NativePostgresError::Database)?;
    rows.iter().map(receipt).collect()
}

fn receipt(row: &Row) -> Result<ReceiptV1, NativePostgresError> {
    let identity_bytes: Vec<u8> = row.get(0);
    let operation_kind: String = row.get(1);
    let canonical_request_hash: Vec<u8> = row.get(2);
    let basis_complete_head_bytes: Option<Vec<u8>> = row.get(3);
    let semantic_input_bytes: Vec<u8> = row.get(4);
    let semantic_time_bytes: Vec<u8> = row.get(5);
    let resolution_kind: String = row.get(6);
    let transition_seq = row
        .get::<_, Option<i64>>(7)
        .map(|value| u64::try_from(value).map_err(|_| NativePostgresError::MalformedRow))
        .transpose()?;
    let receipt_bytes: Vec<u8> = row.get(8);
    let room_id: String = row.get(9);
    let stored = StoredSemanticResultV1::from_canonical_receipt_bytes(&receipt_bytes)
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let expected_identity = stored
        .operation_identity()
        .canonical_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let expected_basis = stored
        .canonical_basis_head_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let expected_input = stored
        .semantic_input()
        .canonical_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let expected_time = stored
        .canonical_semantic_time_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let expected_transition_seq = stored
        .transition_seq()
        .map(worldstream_core::RoomSequenceV1::get);
    if identity_bytes != expected_identity
        || operation_kind != stored.operation_identity().operation_kind()
        || canonical_request_hash != stored.canonical_request_hash().as_bytes()
        || basis_complete_head_bytes != expected_basis
        || semantic_input_bytes != expected_input
        || semantic_time_bytes != expected_time
        || resolution_kind != stored.resolution_kind()
        || transition_seq != expected_transition_seq
        || room_id != stored.target_room_id().to_string()
    {
        return Err(NativePostgresError::Incomplete);
    }
    let request_bytes = semantic_input_bytes;
    Ok(ReceiptV1 {
        kind: ReceiptKindV1::Semantic,
        identity_bytes,
        request_bytes: request_bytes.clone(),
        request_bytes_available: true,
        request_digest: DigestV1::hash(&request_bytes),
        result_bytes: receipt_bytes.clone(),
        result_digest: DigestV1::hash(&receipt_bytes),
        room_id: Some(room_id),
        transition_seq,
        activation_id: None,
        operation_kind: None,
    })
}

fn migration_contract(client: &mut Client) -> Result<MigrationContractV1, NativePostgresError> {
    let rows = client
        .query("SELECT version, migration_id, checksum FROM worldstream_schema_migrations ORDER BY version", &[])
        .map_err(|_| NativePostgresError::Database)?;
    let history = migration_history();
    if rows.len() != history.len() {
        return Err(NativePostgresError::Incomplete);
    }
    let mut records = Vec::with_capacity(rows.len());
    for (row, expected) in rows.into_iter().zip(history) {
        if row.get::<_, i32>(0) != expected.version
            || row.get::<_, String>(1) != expected.id
            || row.get::<_, Vec<u8>>(2) != expected.checksum().as_bytes()
        {
            return Err(NativePostgresError::Incomplete);
        }
        records.push(MigrationIdentityV1 {
            version: u32::try_from(expected.version)
                .map_err(|_| NativePostgresError::MalformedRow)?,
            migration_id: expected.id.to_owned(),
            checksum: core_digest(&expected.checksum())?,
        });
    }
    Ok(MigrationContractV1 {
        logical_history_id: crate::LOGICAL_HISTORY_ID.to_owned(),
        schema_contract_fingerprint: core_digest(&schema_contract_fingerprint())?,
        records,
    })
}

struct DurableDigestInput<'a> {
    lineage: &'a str,
    epoch: u64,
    global_digest: &'a DigestV1,
    migration: &'a MigrationContractV1,
    packs: &'a [PackIdentityV1],
    resources: &'a [ResourceIdentityV1],
    rooms: &'a [RoomCapture],
    receipts: &'a [ReceiptV1],
    activations: &'a [ActivationV1],
    activation_receipts: &'a [ReceiptV1],
    durable_domains: &'a BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
}

fn durable_digest(input: &DurableDigestInput<'_>) -> Result<DigestV1, NativePostgresError> {
    let mut bytes = input.lineage.as_bytes().to_vec();
    bytes.extend_from_slice(&input.epoch.to_be_bytes());
    bytes.extend_from_slice(input.global_digest.as_str().as_bytes());
    bytes.extend_from_slice(
        &serde_json::to_vec(input.migration).map_err(|_| NativePostgresError::Incomplete)?,
    );
    bytes.extend_from_slice(
        &serde_json::to_vec(input.packs).map_err(|_| NativePostgresError::Incomplete)?,
    );
    bytes.extend_from_slice(
        &serde_json::to_vec(input.resources).map_err(|_| NativePostgresError::Incomplete)?,
    );
    for room in input.rooms {
        bytes.extend_from_slice(room.room.source_bytes_digest.as_str().as_bytes());
        bytes.extend_from_slice(
            &serde_json::to_vec(&room.timers).map_err(|_| NativePostgresError::Incomplete)?,
        );
        bytes.extend_from_slice(
            &serde_json::to_vec(&room.frames).map_err(|_| NativePostgresError::Incomplete)?,
        );
    }
    for receipt in input.receipts {
        bytes.extend_from_slice(&receipt.result_bytes);
    }
    bytes.extend_from_slice(
        &serde_json::to_vec(input.activations).map_err(|_| NativePostgresError::Incomplete)?,
    );
    bytes.extend_from_slice(
        &serde_json::to_vec(input.activation_receipts)
            .map_err(|_| NativePostgresError::Incomplete)?,
    );
    bytes.extend_from_slice(
        durable_domains_digest(input.durable_domains)
            .as_str()
            .as_bytes(),
    );
    Ok(DigestV1::hash(&bytes))
}

fn room_bytes_digest(verification: &crate::PostgresRoomVerification) -> DigestV1 {
    let mut bytes = verification.head_bytes.clone();
    bytes.extend_from_slice(&verification.pack_revision_lock_bytes);
    bytes.extend_from_slice(&verification.genesis_bytes);
    bytes.extend_from_slice(&verification.core_state_bytes);
    bytes.extend_from_slice(&verification.activity_state_bytes);
    for transition in &verification.transition_bytes {
        bytes.extend_from_slice(transition);
    }
    for (member, value) in &verification.membership_bytes {
        bytes.extend_from_slice(member.as_bytes());
        bytes.extend_from_slice(value);
    }
    for timer in &verification.timers {
        bytes.extend_from_slice(timer.timer_id.as_bytes());
        bytes.extend_from_slice(&timer.payload_bytes);
    }
    for frame in &verification.frames {
        bytes.extend_from_slice(frame.member_id.as_bytes());
        bytes.extend_from_slice(&frame.payload_bytes);
    }
    DigestV1::hash(&bytes)
}

fn convert_head(head: &CoreCompleteHeadV1) -> Result<CompleteHeadV1, NativePostgresError> {
    Ok(CompleteHeadV1 {
        room_id: head.room_id().to_string(),
        room_seq: head.room_seq().get(),
        lineage_digest: core_digest(head.genesis_or_transition_hash())?,
        core_schema_version: head.core_schema_version().to_owned(),
        pack_revision_digest: core_digest(head.pack_digest())?,
        core_state_digest: core_digest(head.core_state_hash())?,
        activity_state_digest: core_digest(head.activity_state_hash())?,
        authoritative_state_digest: core_digest(head.authoritative_state_hash())?,
    })
}

fn authoritative_bytes(head: &CoreCompleteHeadV1) -> Result<Vec<u8>, NativePostgresError> {
    let value = serde_json::json!({
        "domain": "worldstream/authoritative-state/v1",
        "core_schema": head.core_schema_version(),
        "pack_digest": head.pack_digest().to_string(),
        "core_state_hash": head.core_state_hash().to_string(),
        "activity_state_hash": head.activity_state_hash().to_string(),
    });
    CanonicalJsonV1::parse(
        &serde_json::to_vec(&value).map_err(|_| NativePostgresError::MalformedRow)?,
    )
    .and_then(|json| json.to_bytes())
    .map_err(|_| NativePostgresError::MalformedRow)
}

fn integrity_status(value: &str) -> Result<IntegrityStatusV1, NativePostgresError> {
    match value {
        "healthy" => Ok(IntegrityStatusV1::Healthy),
        "faulted" => Ok(IntegrityStatusV1::Faulted),
        "quarantined" => Ok(IntegrityStatusV1::Quarantined),
        _ => Err(NativePostgresError::MalformedRow),
    }
}

fn core_digest<T: ToString>(value: &T) -> Result<DigestV1, NativePostgresError> {
    DigestV1::parse(value.to_string().trim_start_matches("blake3:").to_owned())
        .map_err(|_| NativePostgresError::MalformedRow)
}

fn run_tool<const N: usize>(
    config: &NativePostgresRestoreConfig,
    tool: &Path,
    endpoint: &NativePostgresEndpointV1,
    args: [&str; N],
) -> Result<(), NativePostgresError> {
    let mut command = Command::new(tool);
    command
        .env("PGPASSFILE", &config.passfile)
        .arg("--host")
        .arg(&endpoint.host)
        .arg("--port")
        .arg(endpoint.port.to_string())
        .arg("--dbname")
        .arg(&endpoint.database)
        .arg("--username")
        .arg(&endpoint.username)
        .arg("--no-password")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if command
        .status()
        .map_err(|_| NativePostgresError::ProviderCommand)?
        .success()
    {
        Ok(())
    } else {
        Err(NativePostgresError::ProviderCommand)
    }
}

fn password_for(
    endpoint: &NativePostgresEndpointV1,
    passfile: &Path,
) -> Result<String, NativePostgresError> {
    let content = fs::read_to_string(passfile)
        .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?;
    for line in content.lines() {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.len() == 5
            && (fields[0] == "*" || fields[0] == endpoint.host)
            && (fields[1] == "*" || fields[1] == endpoint.port.to_string())
            && (fields[2] == "*" || fields[2] == endpoint.database)
            && (fields[3] == "*" || fields[3] == endpoint.username)
        {
            return Ok(fields[4].to_owned());
        }
    }
    Err(NativePostgresError::Configuration(
        "PGPASSFILE has no matching credential",
    ))
}

fn connect(
    endpoint: &NativePostgresEndpointV1,
    password: &str,
) -> Result<Client, NativePostgresError> {
    let mut config = postgres::Config::new();
    config
        .host(&endpoint.host)
        .port(endpoint.port)
        .dbname(&endpoint.database)
        .user(&endpoint.username)
        .password(password);
    config
        .connect(MakeTlsConnector::new(
            TlsConnector::builder().build().map_err(|_| {
                NativePostgresError::Configuration("TLS trust initialization failed")
            })?,
        ))
        .map_err(|_| NativePostgresError::Database)
}

fn dsn_for(endpoint: &NativePostgresEndpointV1, password: &str) -> String {
    format!(
        "host={} port={} dbname={} user={} password={}",
        dsn_value(&endpoint.host),
        endpoint.port,
        dsn_value(&endpoint.database),
        dsn_value(&endpoint.username),
        dsn_value(password)
    )
}

fn dsn_value(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter_identity_and_lock()
    -> Result<(TransferDeploymentIdentityV1, Vec<u8>), NativePostgresError> {
        let digest = worldstream_core::counter_v2_digest();
        let registry = worldstream_core::builtin_counter_registry()
            .map_err(|_| NativePostgresError::Incomplete)?;
        let lock = registry
            .load_retained(&digest)
            .map_err(|_| NativePostgresError::Incomplete)?
            .revision_lock()
            .clone();
        let identity = TransferDeploymentIdentityV1::new(
            vec![
                TransferPackIdentityV1::new(
                    lock.pack_id.clone(),
                    lock.revision_lock_id.clone(),
                    TransferDigestV1::from_bytes(digest.digest().as_bytes())
                        .map_err(|_| NativePostgresError::Incomplete)?,
                )
                .map_err(|_| NativePostgresError::Incomplete)?,
            ],
            Vec::new(),
        )
        .map_err(|_| NativePostgresError::Incomplete)?;
        Ok((
            identity,
            lock.canonical_bytes()
                .map_err(|_| NativePostgresError::Incomplete)?,
        ))
    }

    #[test]
    fn isolated_only_pack_lock_can_supply_retained_executor_identity()
    -> Result<(), NativePostgresError> {
        let (identity, lock) = counter_identity_and_lock()?;

        let locks = retained_pack_locks(&identity, &[lock.clone(), lock])?;

        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].pack_id, identity.packs()[0].pack_id());
        Ok(())
    }

    #[test]
    fn malformed_isolated_pack_lock_fails_even_when_another_room_lock_is_valid()
    -> Result<(), NativePostgresError> {
        let (identity, lock) = counter_identity_and_lock()?;
        let mut malformed = lock.clone();
        malformed[0] ^= 0xff;

        assert!(matches!(
            retained_pack_locks(&identity, &[lock, malformed]),
            Err(NativePostgresError::Incomplete)
        ));
        Ok(())
    }

    #[test]
    fn dsn_and_debug_boundary_is_explicit() {
        let Ok(endpoint) =
            NativePostgresEndpointV1::new("127.0.0.1", 5432, "worldstream", "postgres")
        else {
            return;
        };
        assert!(dsn_for(&endpoint, "TOP_SECRET").contains("TOP_SECRET"));
        let config = NativePostgresRestoreConfig {
            source: endpoint.clone(),
            target: endpoint,
            passfile: PathBuf::from("/owner/pgpass"),
            pg_dump: PathBuf::from("/bin/pg_dump"),
            pg_restore: PathBuf::from("/bin/pg_restore"),
            psql: PathBuf::from("/bin/psql"),
            dump_path: PathBuf::from("/tmp/dump"),
        };
        assert!(!format!("{config:?}").contains("TOP_SECRET"));
    }

    #[test]
    fn activation_receipt_identity_binds_room_and_operation() -> Result<(), NativePostgresError> {
        let first = activation_operation_receipt_identity("room-a", "shared-operation")?;
        let second = activation_operation_receipt_identity("room-b", "shared-operation")?;

        assert_ne!(first, second);
        assert_eq!(
            first,
            br#"{"operation_id":"shared-operation","room_id":"room-a"}"#
        );
        assert!(CanonicalJsonV1::from_canonical_bytes(&first).is_ok());
        assert!(CanonicalJsonV1::from_canonical_bytes(&second).is_ok());
        Ok(())
    }
}
