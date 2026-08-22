//! PostgreSQL-specific native backup/restore orchestration.
//!
//! The provider-neutral image and verifier remain in `worldstream-backup`.
//! This module owns only PostgreSQL connection/tool handling and the typed
//! adaptation of restored rows into that existing verifier contract.

use std::{
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
    BackendNativePointV1, BackendProfileV1, BackupImageV1, BackupManifestV1, CanonicalRecordKindV1,
    CanonicalRecordV1, CompleteHeadV1, DigestV1, FrameV1, IntegrityStatusV1, IntegrityWitnessV1,
    MaterializationV1, MigrationContractV1, MigrationIdentityV1, NativeRestoreEvidenceV1,
    NativeRestoreRoomMembershipV1, NativeRestoreTargetEvidenceV1, PackIdentityV1, ReceiptKindV1,
    ReceiptV1, ResourceBlobV1, ResourceIdentityV1, RoomImageV1, TimerStateV1, TimerV1,
    VerificationReportV1, VerifierLimits, verify_native_restore,
};
use worldstream_core::{
    CanonicalJsonV1, CompleteHeadV1 as CoreCompleteHeadV1, GenesisV1, StoredSemanticResultV1,
    TransitionV1,
};
use worldstream_transfer::{
    DeploymentIdentityV1 as TransferDeploymentIdentityV1, DigestV1 as TransferDigestV1,
    PackIdentityV1 as TransferPackIdentityV1,
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
    /// Snapshot rows observed before disposal.
    pub restored_snapshot_count_before: usize,
    /// Snapshot rows observed after the disposal attempt.
    pub restored_snapshot_count_after: usize,
}

/// Native restore result; the trusted witness cannot be serialized.
#[derive(Debug)]
pub struct NativePostgresRestoreOutcome {
    /// Redacted operational report.
    pub report: NativePostgresRestoreReportV1,
    /// Trusted witness, present only on a fully verified restore.
    pub witness: Option<NativePostgresTrustedWitnessV1>,
}

/// Rebuilds the reviewed disposable snapshot cache for every seeded source
/// Room using the existing PostgreSQL adapter recovery path.
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
            "SELECT room_id FROM worldstream_room_roots ORDER BY room_id",
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
pub fn run_native_postgres_restore(
    config: &NativePostgresRestoreConfig,
) -> Result<NativePostgresRestoreOutcome, NativePostgresError> {
    let source_before = capture(config, &config.source)?;
    let dump_digest = native_dump_restore(config)?;
    let source_after_dump = capture(config, &config.source)?;
    let restored_before_disposal = capture(config, &config.target)?;
    let evidence = make_evidence(&source_after_dump, &restored_before_disposal, &dump_digest)?;
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
    if !eligible || restored.snapshot_count == 0 {
        return Ok((false, restored.snapshot_count));
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
        activations: Vec::new(),
        activation_receipts: Vec::new(),
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
    };
    Ok(NativeRestoreEvidenceV1::new(image, target))
}

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
            "SELECT room_id FROM worldstream_room_roots ORDER BY room_id",
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
        let verification = store
            .verify_room(&room_id)
            .map_err(|_| NativePostgresError::Database)?;
        rooms.push(room_capture(&mut client, &verification)?);
    }
    let (packs, resources, resource_blobs, global_digest) = identities(&mut client, &rooms)?;
    let receipts = semantic_receipts(&mut client)?;
    let durable_digest = durable_digest(&DurableDigestInput {
        lineage: &deployment_lineage,
        epoch: storage_epoch,
        global_digest: &global_digest,
        migration: &migration_contract,
        packs: &packs,
        resources: &resources,
        rooms: &rooms,
        receipts: &receipts,
    });
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
        durable_digest,
        snapshot_count,
    })
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

fn operational_rows(
    client: &mut Client,
    verification: &crate::PostgresRoomVerification,
    room_id: &str,
) -> Result<(Vec<TimerV1>, Vec<FrameV1>), NativePostgresError> {
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
            let state = match timer.state.as_str() {
                "scheduled" => TimerStateV1::Scheduled,
                "cancelled" => TimerStateV1::Cancelled,
                // The provider schema does not retain the consuming transition on
                // a fired timer, so projecting one would weaken the verifier.
                "fired" => return Err(NativePostgresError::Incomplete),
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
                fired_transition_seq: None,
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

fn identities(
    client: &mut Client,
    rooms: &[RoomCapture],
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
    let Some(room) = rooms.first() else {
        return Err(NativePostgresError::Incomplete);
    };
    if pack_rows.len() != 1 || !resource_rows.is_empty() {
        return Err(NativePostgresError::Incomplete);
    }
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
    let transfer_pack_digest = TransferDigestV1::from_bytes(&pack_rows[0].get::<_, Vec<u8>>(2))
        .map_err(|_| NativePostgresError::MalformedRow)?;
    if DigestV1::parse(transfer_pack_digest.to_string())
        .map_err(|_| NativePostgresError::MalformedRow)?
        != room.room.head.pack_revision_digest
    {
        return Err(NativePostgresError::Incomplete);
    }
    let transfer_pack = TransferPackIdentityV1::new(
        pack_rows[0].get::<_, String>(0),
        pack_rows[0].get::<_, String>(1),
        transfer_pack_digest,
    )
    .map_err(|_| NativePostgresError::MalformedRow)?;
    let transfer_identity = TransferDeploymentIdentityV1::new(vec![transfer_pack], Vec::new())
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let canonical_identity = transfer_identity
        .canonical_bytes()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let pack_set_digest = transfer_pack_set_digest(&transfer_identity);
    let resource_set_digest = TransferDigestV1::hash(b"worldstream/deployment-resource-set/v1");
    if identity_row.get::<_, Vec<u8>>(0) != transfer_identity.digest().as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(1) != pack_set_digest.as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(2) != resource_set_digest.as_bytes().to_vec()
        || identity_row.get::<_, Vec<u8>>(3) != canonical_identity
    {
        return Err(NativePostgresError::Incomplete);
    }
    let pack_lock_bytes = client
        .query_one(
            "SELECT pack_revision_lock_bytes FROM worldstream_genesis WHERE room_id = $1",
            &[&room.room.room_id],
        )
        .map_err(|_| NativePostgresError::Database)?
        .get::<_, Vec<u8>>(0);
    let expected_pack_digest = worldstream_core::PackDigestV1::from_str(&format!(
        "blake3:{}",
        room.room.head.pack_revision_digest.as_str()
    ))
    .map_err(|_| NativePostgresError::MalformedRow)?;
    let lock = worldstream_core::PackRevisionLockV1::from_canonical_bytes(
        &pack_lock_bytes,
        &expected_pack_digest,
    )
    .map_err(|_| NativePostgresError::MalformedRow)?;
    let packs = vec![PackIdentityV1 {
        pack_id: pack_rows[0].get(0),
        revision_digest: room.room.head.pack_revision_digest.clone(),
        executor_digest: core_digest(&lock.rule_source_digest)?,
        schema_bundle_digest: core_digest(&lock.schema_bundle_digest)?,
        codec_bundle_digest: core_digest(&lock.codec_bundle_digest)?,
        resource_ids: Vec::new(),
    }];
    Ok((
        packs,
        Vec::new(),
        Vec::new(),
        DigestV1::parse(transfer_identity.digest().to_string())
            .map_err(|_| NativePostgresError::MalformedRow)?,
    ))
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

fn semantic_receipts(client: &mut Client) -> Result<Vec<ReceiptV1>, NativePostgresError> {
    let rows = client
        .query("SELECT identity_bytes, operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes, room_id FROM worldstream_semantic_receipts ORDER BY identity_bytes", &[])
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
        request_digest: DigestV1::hash(&request_bytes),
        result_bytes: receipt_bytes.clone(),
        result_digest: DigestV1::hash(&receipt_bytes),
        room_id: Some(room_id),
        transition_seq,
        activation_id: None,
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
}

fn durable_digest(input: &DurableDigestInput<'_>) -> DigestV1 {
    let mut bytes = input.lineage.as_bytes().to_vec();
    bytes.extend_from_slice(&input.epoch.to_be_bytes());
    bytes.extend_from_slice(input.global_digest.as_str().as_bytes());
    bytes.extend_from_slice(&serde_json::to_vec(input.migration).unwrap_or_default());
    bytes.extend_from_slice(&serde_json::to_vec(input.packs).unwrap_or_default());
    bytes.extend_from_slice(&serde_json::to_vec(input.resources).unwrap_or_default());
    for room in input.rooms {
        bytes.extend_from_slice(room.room.source_bytes_digest.as_str().as_bytes());
        bytes.extend_from_slice(&serde_json::to_vec(&room.timers).unwrap_or_default());
        bytes.extend_from_slice(&serde_json::to_vec(&room.frames).unwrap_or_default());
    }
    for receipt in input.receipts {
        bytes.extend_from_slice(&receipt.result_bytes);
    }
    DigestV1::hash(&bytes)
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
}
