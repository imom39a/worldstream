//! PostgreSQL-specific native backup/restore orchestration.
//!
//! The provider-neutral image and verifier remain in `worldstream-backup`.
//! This module owns only PostgreSQL connection/tool handling and the typed
//! adaptation of restored rows into that existing verifier contract.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;

#[cfg(windows)]
use fs_id::FileID;

#[cfg(unix)]
use command_group::{CommandGroup as _, GroupChild};
use native_tls::TlsConnector;
use postgres::fallible_iterator::FallibleIterator as _;
use postgres::{Client, IsolationLevel, Row, Transaction, config::SslMode};
use postgres_native_tls::MakeTlsConnector;
use postgres_protocol::password::scram_sha_256;
#[cfg(windows)]
use processkit::{Mechanism, ProcessGroup};
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
    VerifierLimits, max_native_restore_canonical_row_bytes, verify_native_restore,
};
use worldstream_core::{
    AccessModeV1, ActivationOperationResultV1, CanonicalJsonV1,
    CompleteHeadV1 as CoreCompleteHeadV1, GenesisV1, OperationIdentityV1, PrincipalKindV1,
    StoredSemanticResultV1, TransitionV1,
};
use worldstream_runtime::prepare_data_directory;
#[cfg(windows)]
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, validate_owner_only_file,
    validate_sqlite_data_filesystem,
};
use worldstream_transfer::{
    DeploymentIdentityV1 as TransferDeploymentIdentityV1, DigestV1 as TransferDigestV1,
    PackIdentityV1 as TransferPackIdentityV1, ResourceIdentityV1 as TransferResourceIdentityV1,
    ResourceKindV1 as TransferResourceKindV1,
};
use zeroize::Zeroizing;

use crate::{
    PostgresAdmin, PostgresConnectionConfig, ProviderReadBudgetV1, migration_history,
    schema_contract_fingerprint,
};

/// Evidence schema emitted by this provider-native lane.
pub const NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1: &str =
    "worldstream/native-postgres-restore-evidence/v2";
/// Exact database comment an operator must place on a new, non-serving
/// database before this lane may invoke destructive `pg_restore --clean`.
pub const NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1: &str =
    "worldstream/native-postgres-disposable-target/v1";
const REQUIRED_POSTGRES_VERSION_NUM: u32 = 170_011;
const TARGET_ISOLATION_ADVISORY_LOCK_CLASS_V1: i32 = 0x5753_4e52;
const TARGET_ISOLATION_ADVISORY_LOCK_OBJECT_V1: i32 = 1;
const NATIVE_DUMP_BYTE_MULTIPLIER_V1: usize = 64;
const MAX_PGPASSFILE_BYTES_V1: usize = 64 * 1024;
const MAX_NATIVE_WORKER_REQUEST_BYTES_V1: usize = 64 * 1024;
const MAX_NATIVE_WORKER_RESULT_BYTES_V1: usize = 32 * 1024 * 1024;
const MAX_NATIVE_COMMIT_REQUEST_BYTES_V1: usize =
    MAX_NATIVE_WORKER_RESULT_BYTES_V1 + MAX_NATIVE_WORKER_REQUEST_BYTES_V1;
const MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1: u64 = 512 * 1024 * 1024;
const NATIVE_POSTGRES_WORKER_REQUEST_SCHEMA_V1: &str =
    "worldstream/native-postgres-worker-request/v1";
const NATIVE_POSTGRES_WORKER_RESULT_SCHEMA_V1: &str =
    "worldstream/native-postgres-worker-result/v1";
const NATIVE_POSTGRES_REPAIR_REQUEST_SCHEMA_V1: &str =
    "worldstream/native-postgres-repair-request/v1";
const NATIVE_POSTGRES_REPAIR_RESULT_SCHEMA_V1: &str =
    "worldstream/native-postgres-repair-result/v1";
const NATIVE_POSTGRES_ADMISSION_REQUEST_SCHEMA_V1: &str =
    "worldstream/native-postgres-admission-request/v1";
const NATIVE_POSTGRES_ADMISSION_RESULT_SCHEMA_V1: &str =
    "worldstream/native-postgres-admission-result/v1";
const NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1: &str =
    "worldstream/native-postgres-commit-request/v1";
const NATIVE_POSTGRES_RECOVERY_RECORD_SCHEMA_V1: &str =
    "worldstream/native-postgres-recovery-record/v1";
/// Hidden same-executable entry selected by the supervised operator API.
pub const NATIVE_POSTGRES_WORKER_ARG_V1: &str = "--worldstream-native-postgres-worker-v1";
/// Hidden same-executable repair entry selected only by the supervisor.
pub const NATIVE_POSTGRES_REPAIR_ARG_V1: &str = "--worldstream-native-postgres-repair-v1";
/// Hidden same-executable read-only admission entry selected by the supervisor.
pub const NATIVE_POSTGRES_ADMISSION_ARG_V1: &str = "--worldstream-native-postgres-admission-v1";
/// Hidden same-executable publication entry selected only by the supervisor.
pub const NATIVE_POSTGRES_COMMIT_ARG_V1: &str = "--worldstream-native-postgres-commit-v1";
const NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1: &str =
    "WORLDSTREAM_NATIVE_POSTGRES_WORKER_CONTAINED_V1";
const PROVIDER_TOOL_TIMEOUT_V1: Duration = Duration::from_mins(5);
const PROVIDER_TERMINATION_GRACE_V1: Duration = Duration::from_secs(2);
const MIN_NATIVE_SUPERVISOR_TIMEOUT_V1: Duration = Duration::from_secs(12);
const POSTGRES_PROVIDER_OBSERVATION_SQL: &str = "SELECT control.system_identifier::text, \
        db.oid::text, db.datname, shobj_description(db.oid, 'pg_database'), \
        (SELECT count(*) FROM pg_stat_activity activity \
         WHERE activity.datid = db.oid AND activity.pid <> pg_backend_pid()), \
        db.datconnlimit, role.rolsuper \
    FROM pg_control_system() control \
    JOIN pg_database db ON db.datname = current_database() \
    JOIN pg_roles role ON role.rolname = current_user";
const TERMINATE_RESTORE_ROLE_BACKENDS_SQL: &str = "SELECT COALESCE(bool_and(pg_terminate_backend(pid, 5000)), true) \
     FROM pg_stat_activity WHERE usename = $1 AND pid <> pg_backend_pid()";
const COUNT_RESTORE_ROLE_BACKENDS_SQL: &str =
    "SELECT count(*) FROM pg_stat_activity WHERE usename = $1 AND pid <> pg_backend_pid()";
const TARGET_USER_OBJECT_COUNT_SQL: &str = r"
WITH public_namespace AS (
    SELECT oid FROM pg_namespace WHERE nspname = 'public'
), unexpected_schemas AS (
    SELECT oid FROM pg_namespace
    WHERE nspname NOT IN ('public', 'pg_catalog', 'information_schema', 'pg_toast')
      AND nspname NOT LIKE 'pg_temp_%'
      AND nspname NOT LIKE 'pg_toast_temp_%'
), schema_objects AS (
    SELECT c.oid FROM pg_class c
      JOIN public_namespace n ON n.oid = c.relnamespace
    UNION ALL SELECT p.oid FROM pg_proc p
      JOIN public_namespace n ON n.oid = p.pronamespace
    UNION ALL SELECT t.oid FROM pg_type t
      JOIN public_namespace n ON n.oid = t.typnamespace
    UNION ALL SELECT o.oid FROM pg_operator o
      JOIN public_namespace n ON n.oid = o.oprnamespace
    UNION ALL SELECT co.oid FROM pg_collation co
      JOIN public_namespace n ON n.oid = co.collnamespace
    UNION ALL SELECT cv.oid FROM pg_conversion cv
      JOIN public_namespace n ON n.oid = cv.connamespace
    UNION ALL SELECT cfg.oid FROM pg_ts_config cfg
      JOIN public_namespace n ON n.oid = cfg.cfgnamespace
    UNION ALL SELECT d.oid FROM pg_ts_dict d
      JOIN public_namespace n ON n.oid = d.dictnamespace
    UNION ALL SELECT prs.oid FROM pg_ts_parser prs
      JOIN public_namespace n ON n.oid = prs.prsnamespace
    UNION ALL SELECT tmpl.oid FROM pg_ts_template tmpl
      JOIN public_namespace n ON n.oid = tmpl.tmplnamespace
    UNION ALL SELECT opc.oid FROM pg_opclass opc
      JOIN public_namespace n ON n.oid = opc.opcnamespace
    UNION ALL SELECT opf.oid FROM pg_opfamily opf
      JOIN public_namespace n ON n.oid = opf.opfnamespace
    UNION ALL SELECT stx.oid FROM pg_statistic_ext stx
      JOIN public_namespace n ON n.oid = stx.stxnamespace
), database_objects AS (
    SELECT ext.oid FROM pg_extension ext WHERE ext.extname <> 'plpgsql'
    UNION ALL SELECT evt.oid FROM pg_event_trigger evt
    UNION ALL SELECT fdw.oid FROM pg_foreign_data_wrapper fdw
    UNION ALL SELECT srv.oid FROM pg_foreign_server srv
    UNION ALL SELECT um.oid FROM pg_user_mapping um
    UNION ALL SELECT pub.oid FROM pg_publication pub
    UNION ALL SELECT sub.oid FROM pg_subscription sub
    UNION ALL SELECT lom.oid FROM pg_largeobject_metadata lom
    UNION ALL SELECT da.oid FROM pg_default_acl da
    UNION ALL SELECT ca.oid FROM pg_cast ca WHERE ca.oid >= 16384
    UNION ALL SELECT lan.oid FROM pg_language lan WHERE lan.oid >= 16384
    UNION ALL SELECT trf.oid FROM pg_transform trf WHERE trf.oid >= 16384
    UNION ALL SELECT am.oid FROM pg_am am WHERE am.oid >= 16384
    UNION ALL SELECT sl.objoid FROM pg_seclabel sl
    UNION ALL SELECT ssl.objoid FROM pg_shseclabel ssl
      JOIN pg_database db ON db.oid = ssl.objoid
      WHERE ssl.classoid = 'pg_database'::regclass
        AND db.datname = current_database()
    UNION ALL SELECT setting.setdatabase FROM pg_db_role_setting setting
      JOIN pg_database db ON db.oid = setting.setdatabase
      WHERE db.datname = current_database()
)
SELECT (SELECT count(*) FROM unexpected_schemas)
     + (SELECT count(*) FROM schema_objects)
     + (SELECT count(*) FROM database_objects)";

/// Explicit TLS policy for one provider-native PostgreSQL endpoint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePostgresTlsModeV1 {
    /// Require an authenticated PostgreSQL TLS connection using system trust
    /// roots and hostname verification.
    Require,
    /// Permit plaintext only for an exact loopback or Unix-socket endpoint.
    Disable,
}

impl NativePostgresTlsModeV1 {
    const fn as_libpq(self) -> &'static str {
        match self {
            Self::Require => "verify-full",
            Self::Disable => "disable",
        }
    }

    const fn as_driver(self) -> SslMode {
        match self {
            Self::Require => SslMode::Require,
            Self::Disable => SslMode::Disable,
        }
    }
}

impl FromStr for NativePostgresTlsModeV1 {
    type Err = NativePostgresError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "require" => Ok(Self::Require),
            "disable" => Ok(Self::Disable),
            _ => Err(NativePostgresError::Configuration(
                "TLS mode must be require or disable",
            )),
        }
    }
}

/// Non-secret endpoint coordinates. Passwords are loaded only from `PGPASSFILE`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativePostgresEndpointV1 {
    /// Host or socket directory.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Database name.
    pub database: String,
    /// Login role.
    pub username: String,
    /// Explicit TLS policy shared by the Rust driver and provider tools.
    pub tls_mode: NativePostgresTlsModeV1,
}

impl NativePostgresEndpointV1 {
    /// Creates an endpoint from non-secret coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error for empty/whitespace coordinates, port zero, or a
    /// plaintext policy aimed at a non-local endpoint.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        database: impl Into<String>,
        username: impl Into<String>,
        tls_mode: NativePostgresTlsModeV1,
    ) -> Result<Self, NativePostgresError> {
        let endpoint = Self {
            host: host.into(),
            port,
            database: database.into(),
            username: username.into(),
            tls_mode,
        };
        validate_native_postgres_endpoint(&endpoint)?;
        Ok(endpoint)
    }
}

fn endpoint_host_is_local(host: &str) -> bool {
    if host
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
    {
        return true;
    }
    #[cfg(unix)]
    if Path::new(host).is_absolute() {
        return true;
    }
    false
}

fn validate_native_postgres_endpoint(
    endpoint: &NativePostgresEndpointV1,
) -> Result<(), NativePostgresError> {
    if endpoint.port == 0
        || [&endpoint.host, &endpoint.database, &endpoint.username]
            .iter()
            .any(|value| value.trim().is_empty() || value.chars().any(char::is_whitespace))
    {
        return Err(NativePostgresError::Configuration(
            "invalid endpoint coordinates",
        ));
    }
    if endpoint.tls_mode == NativePostgresTlsModeV1::Disable
        && !endpoint_host_is_local(&endpoint.host)
    {
        return Err(NativePostgresError::Configuration(
            "remote PostgreSQL endpoint requires TLS",
        ));
    }
    #[cfg(unix)]
    if endpoint.tls_mode == NativePostgresTlsModeV1::Require
        && Path::new(&endpoint.host).is_absolute()
    {
        return Err(NativePostgresError::Configuration(
            "authenticated TLS requires a TCP PostgreSQL endpoint",
        ));
    }
    Ok(())
}

/// Configuration for one native custom-format dump/restore.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// Exact caller-observed identity of the protected dump/report/journal
    /// directory. Native admission must open and match this identity before
    /// creating any child or artifact.
    pub artifact_directory_identity: NativePostgresArtifactDirectoryIdentityV1,
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
            .field(
                "artifact_directory_identity",
                &self.artifact_directory_identity,
            )
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: NativePostgresEndpointV1,
        target: NativePostgresEndpointV1,
        passfile: impl Into<PathBuf>,
        pg_dump: impl Into<PathBuf>,
        pg_restore: impl Into<PathBuf>,
        psql: impl Into<PathBuf>,
        dump_path: impl Into<PathBuf>,
        artifact_directory_identity: NativePostgresArtifactDirectoryIdentityV1,
    ) -> Result<Self, NativePostgresError> {
        let config = Self {
            source,
            target,
            passfile: passfile.into(),
            pg_dump: pg_dump.into(),
            pg_restore: pg_restore.into(),
            psql: psql.into(),
            dump_path: dump_path.into(),
            artifact_directory_identity,
        };
        validate_native_restore_configuration(&config)?;
        Ok(config)
    }
}

/// Versioned, non-secret request consumed only by the contained native worker.
/// Credential bytes are never serialized; this carries only the already
/// validated owner-only passfile path and non-secret endpoint/tool paths.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresWorkerRequestV1 {
    schema: String,
    config: NativePostgresRestoreConfig,
    recovery: NativePostgresWorkerRecoveryPlanV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresWorkerRecoveryPlanV1 {
    role_name: String,
    repair_passfile: PathBuf,
    operator_passfile_identity: NativeWorkerFileIdentityV1,
    operator_passfile_is_anonymous: bool,
    expected_target: PostgresProviderIdentityV1,
    role_passfile: PathBuf,
    role_passfile_identity: NativeWorkerFileIdentityV1,
    role_passfile_is_anonymous: bool,
    private_dump_path: PathBuf,
    private_dump_identity: NativeWorkerFileIdentityV1,
    private_dump_is_anonymous: bool,
}

/// Exact filesystem identity of the retained native restore artifact root.
/// Values are fixed-width lowercase hexadecimal: the filesystem/volume ID is
/// 64 bits and the file ID is 128 bits (with Unix inode values zero-padded).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativePostgresArtifactDirectoryIdentityV1 {
    /// Filesystem device or Windows volume serial number.
    pub storage_id: String,
    /// Unix inode or full Windows `FILE_ID_INFO` 128-bit identifier.
    pub file_id: String,
}

type NativeWorkerFileIdentityV1 = NativePostgresArtifactDirectoryIdentityV1;

impl NativePostgresArtifactDirectoryIdentityV1 {
    /// Constructs one canonical fixed-width artifact-directory identity.
    ///
    /// # Errors
    ///
    /// Returns a closed configuration error unless both coordinates are
    /// lowercase hexadecimal with the exact platform-independent widths.
    pub fn new(
        storage_id: impl Into<String>,
        file_id: impl Into<String>,
    ) -> Result<Self, NativePostgresError> {
        let identity = Self {
            storage_id: storage_id.into(),
            file_id: file_id.into(),
        };
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> Result<(), NativePostgresError> {
        fn fixed_lower_hex(value: &str, width: usize) -> bool {
            value.len() == width
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }

        if !fixed_lower_hex(&self.storage_id, 16) || !fixed_lower_hex(&self.file_id, 32) {
            return Err(NativePostgresError::Configuration(
                "artifact directory identity is not canonical",
            ));
        }
        Ok(())
    }
}

/// Observes the exact identity that must be supplied to native restore or
/// recovery. The caller can retain its own directory handle between this
/// observation and invocation; native admission independently opens and
/// matches the same identity before creating any child or artifact.
///
/// # Errors
///
/// Returns a closed error unless `path` is an existing supported owner-only
/// native restore directory.
pub fn native_postgres_artifact_directory_identity(
    path: &Path,
) -> Result<NativePostgresArtifactDirectoryIdentityV1, NativePostgresError> {
    let authority = NativeRestoreDirectoryAuthorityV1::open(path)?;
    authority.verify_named_path()?;
    Ok(authority.identity.clone())
}

impl NativePostgresWorkerRequestV1 {
    /// Builds a worker request from an already validated restore config.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration no longer passes admission.
    fn new(
        config: &NativePostgresRestoreConfig,
        recovery: NativePostgresWorkerRecoveryPlanV1,
    ) -> Result<Self, NativePostgresError> {
        validate_native_restore_configuration(config)?;
        recovery.validate()?;
        Ok(Self {
            schema: NATIVE_POSTGRES_WORKER_REQUEST_SCHEMA_V1.to_owned(),
            config: config.clone(),
            recovery,
        })
    }

    fn into_parts(
        self,
    ) -> Result<
        (
            NativePostgresRestoreConfig,
            NativePostgresWorkerRecoveryPlanV1,
        ),
        NativePostgresError,
    > {
        if self.schema != NATIVE_POSTGRES_WORKER_REQUEST_SCHEMA_V1 {
            return Err(NativePostgresError::Configuration(
                "native worker request schema is unsupported",
            ));
        }
        validate_native_restore_configuration(&self.config)?;
        self.recovery.validate()?;
        Ok((self.config, self.recovery))
    }
}

impl NativePostgresWorkerRecoveryPlanV1 {
    fn validate(&self) -> Result<(), NativePostgresError> {
        if !restore_role_name_is_valid(&self.role_name) {
            return Err(NativePostgresError::Incomplete);
        }
        if self.role_passfile == self.private_dump_path
            || self.role_passfile == self.repair_passfile
            || self.private_dump_path == self.repair_passfile
            || !self.role_passfile.is_absolute()
            || !self.private_dump_path.is_absolute()
            || !self.repair_passfile.is_absolute()
            || self.expected_target.database_name.is_empty()
            || !self.operator_passfile_identity.is_canonical()
            || self.operator_passfile_is_anonymous != cfg!(target_os = "linux")
            || !self.role_passfile_identity.is_canonical()
            || !self.private_dump_identity.is_canonical()
            || self.role_passfile_is_anonymous != cfg!(target_os = "linux")
            || self.private_dump_is_anonymous != cfg!(target_os = "linux")
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }
}

impl NativePostgresCommitRequestV1 {
    fn validate(&self) -> Result<(), NativePostgresError> {
        if self.schema != NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1
            || !self.artifact_directory_path.is_absolute()
            || !self.recovery_path.is_absolute()
            || !self.report_staging_path.is_absolute()
            || !self.report_path.is_absolute()
            || !paths_are_pairwise_distinct(&[
                &self.recovery_path,
                &self.report_staging_path,
                &self.report_path,
                &self.recovery_record.private_dump_path,
                &self.recovery_record.publication_path,
                &self.recovery_record.role_passfile_path,
            ])
            || self.recovery_path.parent() != Some(self.artifact_directory_path.as_path())
            || self.report_staging_path.parent() != Some(self.artifact_directory_path.as_path())
            || self.report_path.parent() != Some(self.artifact_directory_path.as_path())
            || !self.recovery_identity.is_canonical()
            || !self.report_staging_identity.is_canonical()
            || DigestV1::parse(self.restored_global_digest.clone()).is_err()
        {
            return Err(NativePostgresError::Incomplete);
        }
        self.artifact_directory_identity.validate()?;
        self.recovery_record.validate()?;
        if self.recovery_record.artifact_directory_identity != self.artifact_directory_identity
            || self.recovery_record.report_path.is_some()
            || self.recovery_record.report_digest.is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
        match self.phase {
            NativePostgresCommitPhaseV1::Dump => {
                if self.recovery_record.disposition != "repair_required_if_primary_interrupted"
                    || self.recovery_record.native_dump_digest.is_some()
                    || !native_worker_checkpoint_is_verified(&self.report)
                {
                    return Err(NativePostgresError::Incomplete);
                }
            }
            NativePostgresCommitPhaseV1::ReportAndAcknowledge => {
                if self.recovery_record.disposition != "publication_committed_pending_report_ack"
                    || self.recovery_record.native_dump_digest.as_deref()
                        != Some(self.report.native_dump_digest.as_str())
                    || self.recovery_record.native_dump_size_bytes
                        != Some(self.report.native_dump_size_bytes)
                    || !native_worker_report_is_ready(&self.report)
                {
                    return Err(NativePostgresError::Incomplete);
                }
            }
        }
        Ok(())
    }
}

impl NativeWorkerFileIdentityV1 {
    fn is_canonical(&self) -> bool {
        let lowercase_hex = |value: &str, length: usize| {
            value.len() == length
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        lowercase_hex(&self.storage_id, 16) && lowercase_hex(&self.file_id, 32)
    }
}

fn paths_are_pairwise_distinct(paths: &[&Path]) -> bool {
    paths.iter().enumerate().all(|(index, path)| {
        paths[index.saturating_add(1)..]
            .iter()
            .all(|other| path != other)
    })
}

fn restore_role_name_is_valid(role_name: &str) -> bool {
    role_name
        .strip_prefix("worldstream_restore_")
        .is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn native_recovery_record_name_is_valid(name: &str) -> bool {
    name.strip_prefix(".worldstream_native_recovery_")
        .and_then(|suffix| suffix.strip_suffix(".json"))
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresWorkerResultV1 {
    schema: String,
    record: String,
    status: String,
    report: Option<NativePostgresRestoreReportV1>,
    restored_global_digest: Option<String>,
    reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum NativePostgresCommitPhaseV1 {
    Dump,
    ReportAndAcknowledge,
}

/// Canonical non-secret state consumed only through the contained commit
/// helper's inherited private staging handle. The helper never accepts paths
/// or semantic claims through argv or the environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresCommitRequestV1 {
    schema: String,
    phase: NativePostgresCommitPhaseV1,
    artifact_directory_path: PathBuf,
    artifact_directory_identity: NativePostgresArtifactDirectoryIdentityV1,
    recovery_path: PathBuf,
    recovery_identity: NativeWorkerFileIdentityV1,
    recovery_record: NativePostgresRecoveryRecordV1,
    report_staging_path: PathBuf,
    report_staging_identity: NativeWorkerFileIdentityV1,
    report_path: PathBuf,
    report: NativePostgresRestoreReportV1,
    restored_global_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresAdmissionRequestV1 {
    schema: String,
    target: NativePostgresEndpointV1,
    operator_passfile: PathBuf,
    operator_passfile_identity: NativeWorkerFileIdentityV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresAdmissionResultV1 {
    schema: String,
    status: String,
    target: PostgresProviderIdentityV1,
    target_marker: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresRecoveryRecordV1 {
    schema: String,
    disposition: String,
    target: PostgresProviderIdentityV1,
    target_marker: String,
    role_name: String,
    artifact_directory_identity: NativeWorkerFileIdentityV1,
    role_passfile_path: PathBuf,
    role_passfile_identity: NativeWorkerFileIdentityV1,
    role_passfile_is_anonymous: bool,
    private_dump_path: PathBuf,
    private_dump_identity: NativeWorkerFileIdentityV1,
    private_dump_is_anonymous: bool,
    publication_path: PathBuf,
    native_dump_digest: Option<String>,
    native_dump_size_bytes: Option<u64>,
    private_report_path: Option<PathBuf>,
    report_path: Option<PathBuf>,
    report_identity: Option<NativeWorkerFileIdentityV1>,
    report_digest: Option<String>,
    report_size_bytes: Option<u64>,
    operator_action: String,
}

impl NativePostgresRecoveryRecordV1 {
    fn validate(&self) -> Result<(), NativePostgresError> {
        let digest_and_size_are_consistent = match (
            self.native_dump_digest.as_deref(),
            self.native_dump_size_bytes,
        ) {
            (None, None) => true,
            (Some(digest), Some(size)) => {
                size > 0
                    && DigestV1::parse(digest.to_owned())
                        .is_ok_and(|parsed| parsed.as_str() == digest)
            }
            _ => false,
        };
        let report_binding_is_consistent = match (
            self.private_report_path.as_deref(),
            self.report_path.as_deref(),
            self.report_identity.as_ref(),
            self.report_digest.as_deref(),
            self.report_size_bytes,
        ) {
            (None, None, None, None, None) => true,
            (Some(private), Some(public), Some(identity), Some(digest), Some(size)) => {
                private.is_absolute()
                    && public.is_absolute()
                    && private != public
                    && private != self.private_dump_path
                    && private != self.publication_path
                    && private != self.role_passfile_path
                    && public != self.private_dump_path
                    && public != self.publication_path
                    && public != self.role_passfile_path
                    && identity.is_canonical()
                    && size > 0
                    && DigestV1::parse(digest.to_owned())
                        .is_ok_and(|parsed| parsed.as_str() == digest)
            }
            _ => false,
        };
        if self.schema != NATIVE_POSTGRES_RECOVERY_RECORD_SCHEMA_V1
            || !matches!(
                self.disposition.as_str(),
                "repair_required_if_primary_interrupted"
                    | "publication_commit_in_progress"
                    | "publication_committed_pending_report_ack"
                    | "report_commit_in_progress"
                    | "report_committed_pending_recovery_ack"
                    | "publication_acknowledged"
                    | "recovery_completed_artifacts_scrubbed"
            )
            || self.target_marker != NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1
            || !restore_role_name_is_valid(&self.role_name)
            || !self.artifact_directory_identity.is_canonical()
            || !self.role_passfile_path.is_absolute()
            || !self.role_passfile_identity.is_canonical()
            || self.role_passfile_is_anonymous != cfg!(target_os = "linux")
            || !self.private_dump_path.is_absolute()
            || !self.publication_path.is_absolute()
            || self.private_dump_path == self.publication_path
            || self.role_passfile_path == self.private_dump_path
            || self.role_passfile_path == self.publication_path
            || !self.private_dump_identity.is_canonical()
            || self.private_dump_is_anonymous != cfg!(target_os = "linux")
            || self.target.system_identifier.is_empty()
            || self.target.database_oid.is_empty()
            || self.target.database_name.is_empty()
            || !digest_and_size_are_consistent
            || !report_binding_is_consistent
            || self.operator_action.is_empty()
            || self.operator_action.len() > 256
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn has_same_recovery_authority(&self, first: &Self) -> bool {
        self.schema == first.schema
            && self.target == first.target
            && self.target_marker == first.target_marker
            && self.role_name == first.role_name
            && self.artifact_directory_identity == first.artifact_directory_identity
            && self.role_passfile_path == first.role_passfile_path
            && self.role_passfile_identity == first.role_passfile_identity
            && self.role_passfile_is_anonymous == first.role_passfile_is_anonymous
            && self.private_dump_path == first.private_dump_path
            && self.private_dump_identity == first.private_dump_identity
            && self.private_dump_is_anonymous == first.private_dump_is_anonymous
            && self.publication_path == first.publication_path
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresRepairRequestV1 {
    schema: String,
    target: NativePostgresEndpointV1,
    operator_passfile: PathBuf,
    operator_passfile_identity: NativeWorkerFileIdentityV1,
    expected_target: PostgresProviderIdentityV1,
    target_marker: String,
    role_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativePostgresRepairResultV1 {
    schema: String,
    status: String,
    reason: String,
}

enum NativePostgresRepairWorkerCompletionV1 {
    /// The worker Job/group is conclusively empty; target/artifact cleanup may
    /// now proceed or report the contained operation failure.
    Drained(Result<(), NativePostgresError>),
    /// A child was spawned but its Job/group could not be proven empty. No
    /// target, credential, executable, transport, or artifact cleanup is safe.
    ContainmentUncertain(NativePostgresError),
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
    /// Stable backup identifier derived only from the verified dump bytes.
    pub backup_id: String,
    /// Digest of the canonical provider-native backup point metadata.
    pub native_point_digest: String,
    /// BLAKE3 digest of the exact provider custom-format dump bytes.
    pub native_dump_digest: String,
    /// Exact byte length of the provider custom-format dump.
    pub native_dump_size_bytes: u64,
    /// Exact admitted source cluster/database identity used for the dump.
    pub source_provider_identity: PostgresProviderIdentityV1,
    /// Exact admitted target cluster/database identity authorized for cleanup.
    pub target_provider_identity: PostgresProviderIdentityV1,
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

/// Committed native restore result available only after durable report
/// persistence and recovery-record acknowledgement.
#[derive(Debug)]
pub struct NativePostgresRestoreOutcome {
    report: NativePostgresRestoreReportV1,
    witness: NativePostgresTrustedWitnessV1,
    native_dump_identity: NativePostgresArtifactDirectoryIdentityV1,
    report_identity: NativePostgresArtifactDirectoryIdentityV1,
    recovery_record_identity: NativePostgresArtifactDirectoryIdentityV1,
    recovery_record_name: String,
}

impl NativePostgresRestoreOutcome {
    /// Returns the immutable redacted report whose exact bytes were persisted.
    #[must_use]
    pub fn report(&self) -> &NativePostgresRestoreReportV1 {
        &self.report
    }

    /// Returns the non-serializable trusted witness minted by the supervisor.
    #[must_use]
    pub fn witness(&self) -> &NativePostgresTrustedWitnessV1 {
        &self.witness
    }

    /// Returns the exact retained-handle identity of the committed dump.
    #[must_use]
    pub fn native_dump_identity(&self) -> &NativePostgresArtifactDirectoryIdentityV1 {
        &self.native_dump_identity
    }

    /// Returns the exact retained-handle identity of the committed report.
    #[must_use]
    pub fn report_identity(&self) -> &NativePostgresArtifactDirectoryIdentityV1 {
        &self.report_identity
    }

    /// Returns the exact retained-handle identity of the acknowledged journal.
    #[must_use]
    pub fn recovery_record_identity(&self) -> &NativePostgresArtifactDirectoryIdentityV1 {
        &self.recovery_record_identity
    }

    /// Returns the validated basename of the acknowledged recovery journal.
    #[must_use]
    pub fn recovery_record_name(&self) -> &str {
        &self.recovery_record_name
    }
}

struct NativePostgresVerifiedWorkerCheckpointV1 {
    report: NativePostgresRestoreReportV1,
    restored_global_digest: DigestV1,
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
) -> Result<NativePostgresSnapshotRebuildResultV1, NativePostgresError> {
    validate_native_postgres_endpoint(endpoint)?;
    let password = password_for(endpoint, passfile)?;
    let mut client = connect(endpoint, &password)?;
    let source_provider_identity = provider_identity(&mut client)?.identity;
    let mut budget = ProviderReadBudgetV1::new(VerifierLimits::default())
        .map_err(|_| NativePostgresError::Incomplete)?;
    let mut transaction = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .map_err(|_| NativePostgresError::Database)?;
    crate::verify_runtime_schema_for_native_restore(&mut transaction, &mut budget)
        .map_err(|_| NativePostgresError::Incomplete)?;
    let mut room_remaining = budget.maximum_rooms();
    bounded_global_provider_rows(
        &mut transaction,
        &mut budget,
        "SELECT jsonb_build_array(room_id, integrity_generation, integrity_status)::text FROM worldstream_room_roots ORDER BY room_id",
        &mut room_remaining,
        false,
    )?;
    let room_limit = i64::try_from(budget.maximum_rooms().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let room_rows = transaction
        .query(
            "SELECT room_id, integrity_generation, integrity_status \
             FROM worldstream_room_roots ORDER BY room_id LIMIT $1",
            &[&room_limit],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if room_rows.len() > budget.maximum_rooms() {
        return Err(NativePostgresError::Incomplete);
    }
    let room_ids = room_rows
        .into_iter()
        .map(|row| {
            let room_id: String = row
                .try_get(0)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let integrity_generation: i64 = row
                .try_get(1)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            let integrity_status: String = row
                .try_get(2)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            Ok((room_id, integrity_generation, integrity_status))
        })
        .collect::<Result<Vec<_>, NativePostgresError>>()?;
    transaction
        .commit()
        .map_err(|_| NativePostgresError::Database)?;
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(dsn_for(endpoint, &password))
            .map_err(|_| NativePostgresError::Configuration("source admin configuration failed"))?,
    )
    .map_err(|_| NativePostgresError::Configuration("source admin adapter failed"))?;
    let mut rebuilt = 0;
    for (room_id, integrity_generation, integrity_status) in room_ids {
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
            .rebuild_snapshot_cache_for_native_restore(
                &room_id,
                &mut budget,
                &source_provider_identity,
            )
            .map_err(|_| NativePostgresError::Database)?;
    }
    if provider_identity(&mut client)?.identity != source_provider_identity {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(NativePostgresSnapshotRebuildResultV1 {
        rebuilt_snapshot_count: rebuilt,
        source_provider_identity,
    })
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresProviderIdentityV1 {
    /// Cluster system identifier returned by `pg_control_system()`.
    pub system_identifier: String,
    /// OID of the admitted database inside that exact cluster.
    pub database_oid: String,
    /// Exact admitted database name.
    pub database_name: String,
}

/// Identity-bound result of rebuilding disposable native snapshot caches.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NativePostgresSnapshotRebuildResultV1 {
    /// Total snapshot rows rebuilt across healthy Rooms.
    pub rebuilt_snapshot_count: usize,
    /// Exact source cluster/database admitted before enumeration and required
    /// again immediately before every destructive cache operation.
    pub source_provider_identity: PostgresProviderIdentityV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativePostgresTargetIsolationWitnessV1 {
    source: PostgresProviderIdentityV1,
    target: PostgresProviderIdentityV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PostgresProviderObservationV1 {
    identity: PostgresProviderIdentityV1,
    marker: Option<String>,
    other_client_backends: i64,
    connection_limit: i32,
    current_user_is_superuser: bool,
}

struct ExclusiveFileIdentityV1 {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    handle: FileID,
}

impl ExclusiveFileIdentityV1 {
    fn for_file(file: &fs::File) -> Result<Self, NativePostgresError> {
        #[cfg(unix)]
        {
            let metadata = file
                .metadata()
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            // `fs-id` reads FILE_ID_INFO from this exact open handle. Its
            // volume serial plus full 128-bit ID remains authoritative on
            // both NTFS and ReFS; the legacy 64-bit file index is not used.
            let handle = FileID::new(file).map_err(|_| NativePostgresError::ProviderCommand)?;
            Ok(Self { handle })
        }
    }

    fn still_names_file(&self, path: &Path) -> Result<bool, NativePostgresError> {
        #[cfg(unix)]
        {
            match fs::symlink_metadata(path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                        return Ok(false);
                    }
                    Ok(metadata.dev() == self.device && metadata.ino() == self.inode)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(_) => Err(NativePostgresError::ProviderCommand),
            }
        }
        #[cfg(windows)]
        {
            match validate_owner_only_file(path) {
                Ok(()) => FileID::new(path)
                    .map(|candidate| candidate == self.handle)
                    .map_err(|_| NativePostgresError::ProviderCommand),
                Err(_) if !path.exists() => Ok(false),
                Err(_) => Ok(false),
            }
        }
    }

    fn worker_identity(&self) -> NativeWorkerFileIdentityV1 {
        #[cfg(unix)]
        {
            NativeWorkerFileIdentityV1 {
                storage_id: format!("{:016x}", self.device),
                file_id: format!("{:032x}", u128::from(self.inode)),
            }
        }
        #[cfg(windows)]
        {
            NativeWorkerFileIdentityV1 {
                storage_id: format!("{:016x}", self.handle.storage_id()),
                file_id: format!("{:032x}", self.handle.internal_file_id()),
            }
        }
    }

    fn matches_worker_identity(&self, expected: &NativeWorkerFileIdentityV1) -> bool {
        self.worker_identity() == *expected
    }
}

#[derive(Debug)]
struct NativeRestoreDirectoryAuthorityV1 {
    requested_path: PathBuf,
    path: PathBuf,
    file: fs::File,
    identity: NativeWorkerFileIdentityV1,
}

impl NativeRestoreDirectoryAuthorityV1 {
    fn open(path: &Path) -> Result<Arc<Self>, NativePostgresError> {
        let prepared = prepare_data_directory(path)
            .map_err(|_| NativePostgresError::Configuration("restore parent is unsafe"))?;
        #[cfg(unix)]
        let file = {
            let flags = rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            let flags =
                i32::try_from(flags.bits()).map_err(|_| NativePostgresError::ProviderCommand)?;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&prepared)
                .map_err(|_| NativePostgresError::ProviderCommand)?
        };
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            };

            fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&prepared)
                .map_err(|_| NativePostgresError::ProviderCommand)?
        };
        #[cfg(not(any(unix, windows)))]
        return Err(NativePostgresError::Configuration(
            "native restore parent authority is unsupported",
        ));
        let metadata = file
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if !metadata.is_dir() {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(unix)]
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(NativePostgresError::Incomplete);
        }
        let identity = ExclusiveFileIdentityV1::for_file(&file)?.worker_identity();
        let authority = Arc::new(Self {
            requested_path: path.to_path_buf(),
            path: prepared,
            file,
            identity,
        });
        authority.verify_named_path()?;
        Ok(authority)
    }

    fn open_expected(
        path: &Path,
        expected: &NativePostgresArtifactDirectoryIdentityV1,
    ) -> Result<Arc<Self>, NativePostgresError> {
        expected.validate()?;
        let authority = Self::open(path)?;
        if authority.identity != *expected {
            return Err(NativePostgresError::Configuration(
                "native restore artifact directory identity changed before admission",
            ));
        }
        Ok(authority)
    }

    fn verify_named_path(&self) -> Result<(), NativePostgresError> {
        let metadata =
            fs::symlink_metadata(&self.path).map_err(|_| NativePostgresError::ProviderCommand)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(unix)]
        let named_identity = {
            let named =
                fs::File::open(&self.path).map_err(|_| NativePostgresError::ProviderCommand)?;
            ExclusiveFileIdentityV1::for_file(&named)?.worker_identity()
        };
        #[cfg(windows)]
        let named_identity = {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            };

            let named = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&self.path)
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            let identity = FileID::new(&named).map_err(|_| NativePostgresError::ProviderCommand)?;
            NativeWorkerFileIdentityV1 {
                storage_id: format!("{:016x}", identity.storage_id()),
                file_id: format!("{:032x}", identity.internal_file_id()),
            }
        };
        if named_identity != self.identity
            || ExclusiveFileIdentityV1::for_file(&self.file)?.worker_identity() != self.identity
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn path_for(&self, name: &std::ffi::OsStr) -> Result<PathBuf, NativePostgresError> {
        if name.is_empty()
            || Path::new(name).components().count() != 1
            || matches!(name.to_str(), Some("." | ".."))
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(self.path.join(name))
    }

    fn name_for(&self, path: &Path) -> Result<std::ffi::OsString, NativePostgresError> {
        let parent = path.parent().ok_or(NativePostgresError::Incomplete)?;
        if parent != self.requested_path && parent != self.path {
            return Err(NativePostgresError::Incomplete);
        }
        let name = path
            .file_name()
            .ok_or(NativePostgresError::Incomplete)?
            .to_owned();
        self.path_for(&name)?;
        Ok(name)
    }

    fn create_owner_only(
        &self,
        name: &std::ffi::OsStr,
        renameable: bool,
    ) -> Result<fs::File, NativePostgresError> {
        self.path_for(name)?;
        #[cfg(windows)]
        let path = self.path_for(name)?;
        #[cfg(unix)]
        let file = {
            let _ = renameable;
            let descriptor = rustix::fs::openat(
                &self.file,
                name,
                rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|_| NativePostgresError::ProviderCommand)?;
            fs::File::from(descriptor)
        };
        #[cfg(windows)]
        let file = if renameable {
            create_owner_only_renameable_file(&path)
        } else {
            create_owner_only_file(&path)
        }
        .map_err(|_| NativePostgresError::ProviderCommand)?;
        #[cfg(not(any(unix, windows)))]
        return Err(NativePostgresError::Configuration(
            "native restore parent authority is unsupported",
        ));
        let metadata = file
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if !metadata.is_file() {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(unix)]
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(windows)]
        validate_owner_only_file(&path).map_err(|_| NativePostgresError::ProviderCommand)?;
        self.verify_named_path()?;
        Ok(file)
    }

    fn create_dump_file(&self, name: &std::ffi::OsStr) -> Result<fs::File, NativePostgresError> {
        #[cfg(target_os = "linux")]
        {
            let descriptor = rustix::fs::openat(
                &self.file,
                ".",
                rustix::fs::OFlags::TMPFILE
                    | rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|_| NativePostgresError::ProviderCommand)?;
            let file = fs::File::from(descriptor);
            let metadata = file
                .metadata()
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            if !metadata.is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(NativePostgresError::Incomplete);
            }
            return Ok(file);
        }
        #[cfg(not(target_os = "linux"))]
        self.create_owner_only(name, cfg!(windows))
    }

    fn open_existing(
        &self,
        name: &std::ffi::OsStr,
    ) -> Result<Option<fs::File>, NativePostgresError> {
        self.path_for(name)?;
        #[cfg(windows)]
        let path = self.path_for(name)?;
        #[cfg(unix)]
        let file = {
            rustix::fs::openat(
                &self.file,
                name,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map(fs::File::from)
            .map_err(std::io::Error::from)
        };
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
        };
        #[cfg(not(any(unix, windows)))]
        return Err(NativePostgresError::Configuration(
            "native restore parent authority is unsupported",
        ));
        let file = match file {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(NativePostgresError::ProviderCommand),
        };
        self.verify_named_path()?;
        Ok(Some(file))
    }

    fn still_names(
        &self,
        name: &std::ffi::OsStr,
        expected: &ExclusiveFileIdentityV1,
    ) -> Result<bool, NativePostgresError> {
        self.path_for(name)?;
        #[cfg(windows)]
        let path = self.path_for(name)?;
        #[cfg(unix)]
        let opened = rustix::fs::openat(
            &self.file,
            name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map(fs::File::from)
        .map_err(std::io::Error::from);
        #[cfg(windows)]
        let opened = {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
        };
        #[cfg(not(any(unix, windows)))]
        return Err(NativePostgresError::Configuration(
            "native restore parent authority is unsupported",
        ));
        let file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(NativePostgresError::ProviderCommand),
        };
        let observed = ExclusiveFileIdentityV1::for_file(&file)?;
        self.verify_named_path()?;
        Ok(observed.matches_worker_identity(&expected.worker_identity()))
    }

    #[cfg(unix)]
    fn sync(&self) -> Result<(), NativePostgresError> {
        self.file
            .sync_all()
            .map_err(|_| NativePostgresError::ProviderCommand)
    }
}

fn retained_named_file_identity(
    directory: &NativeRestoreDirectoryAuthorityV1,
    path: &Path,
    file: &fs::File,
    expected_identity: &ExclusiveFileIdentityV1,
    expected_size: Option<u64>,
) -> Result<NativePostgresArtifactDirectoryIdentityV1, NativePostgresError> {
    let name = directory.name_for(path)?;
    let observed = ExclusiveFileIdentityV1::for_file(file)?;
    let metadata = file
        .metadata()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let size_is_valid = expected_size.map_or(metadata.len() > 0, |size| metadata.len() == size);
    if !metadata.is_file()
        || !size_is_valid
        || !observed.matches_worker_identity(&expected_identity.worker_identity())
        || !directory.still_names(&name, expected_identity)?
    {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(observed.worker_identity())
}

#[cfg(test)]
fn open_exclusive_private_file(path: &Path) -> Result<fs::File, NativePostgresError> {
    let parent = path.parent().ok_or(NativePostgresError::Incomplete)?;
    let directory = NativeRestoreDirectoryAuthorityV1::open(parent)?;
    open_exclusive_private_file_in(&directory, path)
}

fn open_exclusive_private_file_in(
    directory: &NativeRestoreDirectoryAuthorityV1,
    path: &Path,
) -> Result<fs::File, NativePostgresError> {
    let name = directory.name_for(path)?;
    directory.create_owner_only(&name, false)
}

#[cfg(test)]
fn open_exclusive_dump_file(path: &Path) -> Result<fs::File, NativePostgresError> {
    let parent = path.parent().ok_or(NativePostgresError::Incomplete)?;
    let directory = NativeRestoreDirectoryAuthorityV1::open(parent)?;
    open_exclusive_dump_file_in(&directory, path)
}

fn open_exclusive_dump_file_in(
    directory: &NativeRestoreDirectoryAuthorityV1,
    path: &Path,
) -> Result<fs::File, NativePostgresError> {
    let name = directory.name_for(path)?;
    directory.create_dump_file(&name)
}

fn scrub_private_file(
    file: &mut Option<fs::File>,
    identity: &ExclusiveFileIdentityV1,
    path: &Path,
) -> Result<(), NativePostgresError> {
    scrub_private_file_with(file, identity, path, |retained| {
        retained.set_len(0).and_then(|()| retained.sync_all())
    })
}

fn scrub_private_file_with(
    file: &mut Option<fs::File>,
    _identity: &ExclusiveFileIdentityV1,
    _path: &Path,
    scrub: impl FnOnce(&fs::File) -> std::io::Result<()>,
) -> Result<(), NativePostgresError> {
    let retained = file.as_ref().ok_or(NativePostgresError::Incomplete)?;
    scrub(retained).map_err(|_| NativePostgresError::ProviderCommand)?;
    // The retained open handle is the authority: scrub it even if its original
    // pathname was renamed or replaced. Never delete or truncate by pathname,
    // because that would re-open a compare/action race against another file.
    drop(file.take());
    Ok(())
}

struct EphemeralPgPassfileV1 {
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: PathBuf,
    file: Option<fs::File>,
    identity: ExclusiveFileIdentityV1,
    binding: PrivateFileBindingV1,
    removed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateFileBindingV1 {
    Anonymous,
    OwnedPath,
    #[cfg(windows)]
    BorrowedPath,
}

struct PinnedNativeWorkerV1 {
    path: PathBuf,
    file: Option<fs::File>,
    identity: ExclusiveFileIdentityV1,
    expected_digest: DigestV1,
    borrowed: bool,
    removed: bool,
}

impl PinnedNativeWorkerV1 {
    fn verify(&self) -> Result<(), NativePostgresError> {
        self.verify_retained_immutable()?;
        let file = self.file.as_ref().ok_or(NativePostgresError::Incomplete)?;
        let mut exact = file
            .try_clone()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        exact
            .seek(SeekFrom::Start(0))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if hash_bounded_reader(&mut exact, MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1)?
            != self.expected_digest
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn verify_retained_immutable(&self) -> Result<(), NativePostgresError> {
        let file = self.file.as_ref().ok_or(NativePostgresError::Incomplete)?;
        let observed_identity = ExclusiveFileIdentityV1::for_file(file)?;
        if !observed_identity.matches_worker_identity(&self.identity.worker_identity()) {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(target_os = "linux")]
        {
            let required = rustix::fs::SealFlags::SEAL
                | rustix::fs::SealFlags::SHRINK
                | rustix::fs::SealFlags::GROW
                | rustix::fs::SealFlags::WRITE;
            let seals = rustix::fs::fcntl_get_seals(file)
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            if !seals.contains(required) {
                return Err(NativePostgresError::Incomplete);
            }
        }
        #[cfg(not(target_os = "linux"))]
        if !self.identity.still_names_file(&self.path)? {
            return Err(NativePostgresError::Incomplete);
        }
        let metadata = file
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn remove(&mut self) -> Result<(), NativePostgresError> {
        if self.removed {
            return Ok(());
        }
        if self.borrowed {
            drop(self.file.take());
            self.removed = true;
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            // The sealed anonymous executable has no directory entry. Closing
            // the last retained descriptor is exact, race-free retirement.
            drop(self.file.take());
            self.removed = true;
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            #[cfg(unix)]
            self.file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?
                .set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            drop(self.file.take());
            #[cfg(unix)]
            let candidate = {
                let flags = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
                    .map_err(|_| NativePostgresError::ProviderCommand)?;
                fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(flags)
                    .open(&self.path)
                    .map_err(|_| NativePostgresError::ProviderCommand)?
            };
            #[cfg(windows)]
            let candidate = {
                use std::os::windows::fs::OpenOptionsExt as _;
                use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
                fs::OpenOptions::new()
                    .write(true)
                    .share_mode(FILE_SHARE_READ)
                    .open(&self.path)
                    .map_err(|_| NativePostgresError::ProviderCommand)?
            };
            let candidate_identity = ExclusiveFileIdentityV1::for_file(&candidate)?;
            if !candidate_identity.matches_worker_identity(&self.identity.worker_identity()) {
                return Err(NativePostgresError::Incomplete);
            }
            let mut candidate = Some(candidate);
            scrub_private_file(&mut candidate, &candidate_identity, &self.path)?;
            self.removed = true;
            Ok(())
        }
    }
}

impl Drop for PinnedNativeWorkerV1 {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}

impl EphemeralPgPassfileV1 {
    fn verify(&self) -> Result<(), NativePostgresError> {
        let file = self.file.as_ref().ok_or(NativePostgresError::Incomplete)?;
        let observed = ExclusiveFileIdentityV1::for_file(file)?;
        if observed.matches_worker_identity(&self.identity.worker_identity()) {
            match self.binding {
                PrivateFileBindingV1::Anonymous => Ok(()),
                PrivateFileBindingV1::OwnedPath => {
                    let name = self
                        .path
                        .file_name()
                        .ok_or(NativePostgresError::Incomplete)?;
                    if self.directory.still_names(name, &self.identity)? {
                        Ok(())
                    } else {
                        Err(NativePostgresError::Incomplete)
                    }
                }
                #[cfg(windows)]
                PrivateFileBindingV1::BorrowedPath => {
                    if self.identity.still_names_file(&self.path)? {
                        Ok(())
                    } else {
                        Err(NativePostgresError::Incomplete)
                    }
                }
            }
        } else {
            Err(NativePostgresError::Incomplete)
        }
    }

    fn remove(&mut self) -> Result<(), NativePostgresError> {
        if self.removed {
            return Ok(());
        }
        match self.binding {
            PrivateFileBindingV1::Anonymous => {
                drop(self.file.take());
                self.removed = true;
                return Ok(());
            }
            #[cfg(windows)]
            PrivateFileBindingV1::BorrowedPath => {
                drop(self.file.take());
                self.removed = true;
                return Ok(());
            }
            PrivateFileBindingV1::OwnedPath => {}
        }
        scrub_private_file(&mut self.file, &self.identity, &self.path)?;
        self.removed = true;
        Ok(())
    }

    fn preserve_on_drop(&mut self) -> Result<(), NativePostgresError> {
        if self.binding != PrivateFileBindingV1::OwnedPath || self.file.is_none() {
            return Err(NativePostgresError::Incomplete);
        }
        // Set this before any later fallible work. Once the first durable
        // recovery generation exists, unwind/Drop must never truncate it.
        self.removed = true;
        Ok(())
    }

    fn scrub_preserved_before_spawn(&mut self) -> Result<(), NativePostgresError> {
        if self.binding != PrivateFileBindingV1::OwnedPath {
            return Err(NativePostgresError::Incomplete);
        }
        if self.file.is_none() {
            return Ok(());
        }
        scrub_private_file(&mut self.file, &self.identity, &self.path)?;
        self.removed = true;
        Ok(())
    }

    fn freeze_for_provider(&mut self) -> Result<(), NativePostgresError> {
        self.verify()?;
        let file = self.file.as_ref().ok_or(NativePostgresError::Incomplete)?;
        file.sync_all()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        #[cfg(target_os = "linux")]
        if self.binding != PrivateFileBindingV1::OwnedPath {
            rustix::fs::fcntl_add_seals(
                file,
                rustix::fs::SealFlags::SEAL
                    | rustix::fs::SealFlags::SHRINK
                    | rustix::fs::SealFlags::GROW
                    | rustix::fs::SealFlags::WRITE,
            )
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        }
        Ok(())
    }

    fn preserve(&mut self) -> Result<(), NativePostgresError> {
        self.preserve_with_sync(fs::File::sync_all)
    }

    fn preserve_with_sync(
        &mut self,
        sync: impl FnOnce(&fs::File) -> std::io::Result<()>,
    ) -> Result<(), NativePostgresError> {
        if self.binding != PrivateFileBindingV1::OwnedPath {
            return Err(NativePostgresError::Incomplete);
        }
        // A failed target repair makes this the only durable, nonsecret
        // operator recovery record. Disarm Drop/epilogue scrubbing before any
        // fallible verification or sync: even a storage fault must leave the
        // retained bytes/name available for best-effort operator recovery.
        self.removed = true;
        let verification = self.verify();
        let synchronization = self
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)
            .and_then(|file| sync(file).map_err(|_| NativePostgresError::ProviderCommand));
        verification?;
        synchronization
    }

    fn seal_anonymous_transport(&mut self) -> Result<(), NativePostgresError> {
        self.verify()?;
        let file = self.file.as_ref().ok_or(NativePostgresError::Incomplete)?;
        file.sync_all()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        #[cfg(target_os = "linux")]
        if self.binding == PrivateFileBindingV1::Anonymous {
            rustix::fs::fcntl_add_seals(
                file,
                rustix::fs::SealFlags::SEAL
                    | rustix::fs::SealFlags::SHRINK
                    | rustix::fs::SealFlags::GROW
                    | rustix::fs::SealFlags::WRITE,
            )
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        }
        Ok(())
    }
}

impl Drop for EphemeralPgPassfileV1 {
    fn drop(&mut self) {
        if !self.removed {
            match self.binding {
                PrivateFileBindingV1::OwnedPath => {
                    let _ = scrub_private_file(&mut self.file, &self.identity, &self.path);
                }
                PrivateFileBindingV1::Anonymous => {
                    drop(self.file.take());
                }
                #[cfg(windows)]
                PrivateFileBindingV1::BorrowedPath => {
                    drop(self.file.take());
                }
            }
        }
    }
}

struct NativePostgresRestoreCredentialV1 {
    role_name: String,
    endpoint: NativePostgresEndpointV1,
    passfile_path: PathBuf,
    passfile_identity: NativeWorkerFileIdentityV1,
}

fn open_verified_pgpassfile(path: &Path) -> Result<fs::File, NativePostgresError> {
    #[cfg(unix)]
    let file = {
        #[cfg(target_os = "linux")]
        if linux_inherited_fd_path(path) {
            // The packaged supervisor deliberately supplies an already-open
            // credential descriptor as the canonical procfs magic-link form.
            // Admit only `/proc/self/fd/<canonical-decimal>` and immediately
            // duplicate that exact descriptor. General symlinks remain
            // rejected by O_NOFOLLOW below.
            fs::File::open(path)
                .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?
        } else {
            open_nofollow_pgpassfile(path)?
        }
        #[cfg(not(target_os = "linux"))]
        open_nofollow_pgpassfile(path)?
    };

    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        validate_owner_only_file(path).map_err(|_| {
            NativePostgresError::Configuration("PGPASSFILE is not an owner-only regular file")
        })?;
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
            .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?;
        // Re-check after opening. The no-delete-sharing handle prevents the
        // validated pathname from being replaced for the rest of this copy.
        validate_owner_only_file(path).map_err(|_| {
            NativePostgresError::Configuration("PGPASSFILE is not an owner-only regular file")
        })?;
        file
    };

    #[cfg(not(any(unix, windows)))]
    let file = {
        let _ = path;
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE cannot be verified on this platform",
        ));
    };

    let metadata = file
        .metadata()
        .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?;
    if !metadata.is_file() {
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE is not a regular file",
        ));
    }
    #[cfg(unix)]
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE must be owner-owned mode 0600",
        ));
    }
    let identity = ExclusiveFileIdentityV1::for_file(&file).map_err(|_| {
        NativePostgresError::Configuration("PGPASSFILE identity cannot be verified")
    })?;
    #[cfg(target_os = "linux")]
    let retained_descriptor = linux_inherited_fd_path(path);
    #[cfg(not(target_os = "linux"))]
    let retained_descriptor = false;
    if !retained_descriptor
        && !identity.still_names_file(path).map_err(|_| {
            NativePostgresError::Configuration("PGPASSFILE identity cannot be verified")
        })?
    {
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE changed while it was opened",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_nofollow_pgpassfile(path: &Path) -> Result<fs::File, NativePostgresError> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let flags = i32::try_from(flags.bits()).map_err(|_| {
        NativePostgresError::Configuration("PGPASSFILE flags cannot be represented")
    })?;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))
}

fn open_worker_owned_file(
    path: &Path,
    expected: &NativeWorkerFileIdentityV1,
) -> Result<fs::File, NativePostgresError> {
    #[cfg(target_os = "linux")]
    let file = if linux_inherited_fd_path(path) {
        // `/proc/self/fd/N` is a kernel-owned magic link to the inherited
        // descriptor. `O_NOFOLLOW` would reject it, so open the descriptor
        // duplicate and bind it to the exact full identity carried in the
        // canonical worker request before reading any credential bytes.
        let file = fs::File::open(path).map_err(|_| NativePostgresError::ProviderCommand)?;
        let metadata = file
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(NativePostgresError::Incomplete);
        }
        file
    } else {
        open_verified_pgpassfile(path)?
    };
    #[cfg(not(target_os = "linux"))]
    let file = open_verified_pgpassfile(path)?;
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    if !identity.matches_worker_identity(expected) {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn linux_inherited_fd_path(path: &Path) -> bool {
    path.to_str()
        .and_then(|path| path.strip_prefix("/proc/self/fd/"))
        .and_then(|descriptor| descriptor.parse::<i32>().ok().map(|fd| (descriptor, fd)))
        .is_some_and(|(descriptor, fd)| fd >= 0 && descriptor == fd.to_string())
}

fn create_provider_passfile_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    directory.name_for(&path)?;
    #[cfg(target_os = "linux")]
    {
        let passfile = create_native_worker_transport_file_in(directory, path)?;
        passfile
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        Ok(passfile)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let file = open_exclusive_private_file_in(&directory, &path)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        Ok(EphemeralPgPassfileV1 {
            directory,
            path,
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::OwnedPath,
            removed: false,
        })
    }
}

#[cfg(all(test, windows))]
fn pin_pgpassfile(
    directory: &Path,
    source: &Path,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let authority = NativeRestoreDirectoryAuthorityV1::open(directory)?;
    pin_pgpassfile_in(&authority, source)
}

fn pin_pgpassfile_in(
    directory: &Arc<NativeRestoreDirectoryAuthorityV1>,
    source: &Path,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    #[cfg(windows)]
    {
        // Keep the operator's original owner-only passfile pinned with a
        // no-write/no-delete-sharing handle instead of creating a second
        // named credential. A whole-process termination therefore cannot
        // strand a copied administrator secret before the recovery journal.
        let file = open_verified_pgpassfile(source)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        let passfile = EphemeralPgPassfileV1 {
            directory: directory.clone(),
            path: source.to_path_buf(),
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::BorrowedPath,
            removed: false,
        };
        passfile.verify()?;
        Ok(passfile)
    }
    #[cfg(not(windows))]
    {
        let content = Zeroizing::new(read_bounded_utf8(
            open_verified_pgpassfile(source)?,
            MAX_PGPASSFILE_BYTES_V1,
        )?);
        let path = directory.path.join(format!(
            ".worldstream_credentials_{}.pgpass",
            random_hex(16)?
        ));
        let mut passfile = create_provider_passfile_in(directory.clone(), path)?;
        let file = passfile
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        passfile.freeze_for_provider()?;
        passfile.verify()?;
        Ok(passfile)
    }
}

#[allow(clippy::too_many_lines)]
fn pin_native_worker_executable_in(
    directory: &Arc<NativeRestoreDirectoryAuthorityV1>,
    source: &Path,
) -> Result<PinnedNativeWorkerV1, NativePostgresError> {
    #[cfg(unix)]
    let mut source_file = {
        let flags = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(source)
            .map_err(|_| NativePostgresError::ProviderCommand)?
    };
    #[cfg(windows)]
    let mut source_file = {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(source)
            .map_err(|_| NativePostgresError::ProviderCommand)?
    };
    let source_metadata = source_file
        .metadata()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    if !source_metadata.is_file()
        || source_metadata.len() == 0
        || source_metadata.len() > MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1
    {
        return Err(NativePostgresError::Configuration(
            "native worker executable is invalid",
        ));
    }
    let source_identity = ExclusiveFileIdentityV1::for_file(&source_file)?;
    if !source_identity.still_names_file(source)? {
        return Err(NativePostgresError::ProviderCommand);
    }
    let expected_digest =
        hash_bounded_reader(&mut source_file, MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1)?;
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    #[cfg(windows)]
    {
        let _ = directory;
        // The packaged executable is already the exact product authority.
        // Retain its no-write/no-delete-sharing handle instead of making a
        // named private copy that could survive a pre-journal hard kill.
        let pinned = PinnedNativeWorkerV1 {
            path: source.to_path_buf(),
            file: Some(source_file),
            identity: source_identity,
            expected_digest,
            borrowed: true,
            removed: false,
        };
        pinned.verify()?;
        Ok(pinned)
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;

        let owned = rustix::fs::memfd_create(
            "worldstream-native-worker-v1",
            rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .map_err(|_| NativePostgresError::ProviderCommand)?;
        let mut destination = fs::File::from(owned);
        let mut bounded_source = std::io::Read::take(
            &mut source_file,
            MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1.saturating_add(1),
        );
        let copied = std::io::copy(&mut bounded_source, &mut destination)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if copied != source_metadata.len() || !source_identity.still_names_file(source)? {
            return Err(NativePostgresError::Incomplete);
        }
        destination
            .sync_all()
            .and_then(|()| destination.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if hash_bounded_reader(&mut destination, MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1)?
            != expected_digest
        {
            return Err(NativePostgresError::Incomplete);
        }
        destination
            .set_permissions(fs::Permissions::from_mode(0o500))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let required_seals = rustix::fs::SealFlags::SEAL
            | rustix::fs::SealFlags::SHRINK
            | rustix::fs::SealFlags::GROW
            | rustix::fs::SealFlags::WRITE;
        rustix::fs::fcntl_add_seals(&destination, required_seals)
            .map_err(|_| NativePostgresError::ProviderCommand)?;

        // Reopen read-only before dropping the writer to avoid ETXTBSY. Keep
        // this descriptor inherited so shebang-based test workers can reopen
        // `/proc/self/fd/N` after exec; the sealed bytes are nonsecret.
        let writer_path = PathBuf::from(format!("/proc/self/fd/{}", destination.as_raw_fd()));
        let read_handle = fs::OpenOptions::new()
            .read(true)
            .open(&writer_path)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        rustix::io::fcntl_setfd(&read_handle, rustix::io::FdFlags::empty())
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        drop(destination);
        let path = PathBuf::from(format!("/proc/self/fd/{}", read_handle.as_raw_fd()));
        let identity = ExclusiveFileIdentityV1::for_file(&read_handle)?;
        let pinned = PinnedNativeWorkerV1 {
            path,
            file: Some(read_handle),
            identity,
            expected_digest,
            borrowed: false,
            removed: false,
        };
        pinned.verify()?;
        Ok(pinned)
    }
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    {
        let path = directory.path.join(format!(
            ".worldstream_native_worker_{}{}",
            random_hex(16)?,
            if cfg!(windows) { ".exe" } else { "" }
        ));
        let file = open_exclusive_private_file_in(directory, &path)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        let mut pinned = EphemeralPgPassfileV1 {
            directory: directory.clone(),
            path,
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::OwnedPath,
            removed: false,
        };
        let destination = pinned
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?;
        let mut bounded_source = std::io::Read::take(
            &mut source_file,
            MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1.saturating_add(1),
        );
        let copied = std::io::copy(&mut bounded_source, destination)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if copied != source_metadata.len() || !source_identity.still_names_file(source)? {
            return Err(NativePostgresError::Incomplete);
        }
        destination
            .sync_all()
            .and_then(|()| destination.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let copied_digest =
            hash_bounded_reader(&mut *destination, MAX_NATIVE_WORKER_EXECUTABLE_BYTES_V1)?;
        if copied_digest != expected_digest {
            return Err(NativePostgresError::Incomplete);
        }
        #[cfg(unix)]
        fs::set_permissions(&pinned.path, fs::Permissions::from_mode(0o500))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        pinned.freeze_for_provider()?;
        drop(pinned.file.take());
        #[cfg(unix)]
        let read_handle = {
            let flags = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&pinned.path)
                .map_err(|_| NativePostgresError::ProviderCommand)?
        };
        #[cfg(windows)]
        let read_handle = {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&pinned.path)
                .map_err(|_| NativePostgresError::ProviderCommand)?
        };
        let read_identity = ExclusiveFileIdentityV1::for_file(&read_handle)?;
        if !read_identity.matches_worker_identity(&pinned.identity.worker_identity())
            || !read_identity.still_names_file(&pinned.path)?
        {
            return Err(NativePostgresError::Incomplete);
        }
        pinned.removed = true;
        let pinned_worker = PinnedNativeWorkerV1 {
            path: pinned.path.clone(),
            file: Some(read_handle),
            identity: read_identity,
            expected_digest,
            borrowed: false,
            removed: false,
        };
        pinned_worker.verify()?;
        Ok(pinned_worker)
    }
}

#[cfg(unix)]
fn retained_native_worker_invocation_path(
    worker: &PinnedNativeWorkerV1,
    production_worker: bool,
) -> Result<PathBuf, NativePostgresError> {
    use std::os::fd::AsRawFd as _;

    if !production_worker {
        return Ok(worker.path.clone());
    }
    let descriptor = worker
        .file
        .as_ref()
        .ok_or(NativePostgresError::Incomplete)?
        .as_raw_fd();
    #[cfg(target_os = "linux")]
    let path = format!("/proc/self/fd/{descriptor}");
    #[cfg(not(target_os = "linux"))]
    let path = format!("/dev/fd/{descriptor}");
    Ok(PathBuf::from(path))
}

#[cfg(windows)]
fn retained_native_worker_invocation_path(
    worker: &PinnedNativeWorkerV1,
    _production_worker: bool,
) -> Result<PathBuf, NativePostgresError> {
    worker.verify()?;
    Ok(worker.path.clone())
}

// The keeper session admits target A and creates a cryptographically random,
// one-use login that exists only on A. `pg_restore` authenticates as that role,
// so DNS or routing changes to target B fail authentication before libpq can
// issue `--clean`. PostgreSQL database connection limits are global rather
// than reserved; observations therefore require zero competing backends before
// admission and again after the child exits and after limit-zero sealing. Any
// ordinary-role race fails the run before a witness can be minted. Advisory
// locks and connection limits remain voluntary for cluster administrators, who
// stay inside the operational trust boundary.
struct NativePostgresTargetIsolationLeaseV1 {
    witness: NativePostgresTargetIsolationWitnessV1,
    marker: String,
    target_owner_role: String,
    source_client: Client,
    target_client: Client,
    active_connection_limit: i32,
    target_user_is_superuser: bool,
    outstanding_restore_role: Option<String>,
    finalized: bool,
}

fn validate_distinct_endpoint_coordinates(
    source: &NativePostgresEndpointV1,
    target: &NativePostgresEndpointV1,
) -> Result<(), NativePostgresError> {
    let same_host = if source.host.starts_with('/') || target.host.starts_with('/') {
        source.host == target.host
    } else {
        source.host.eq_ignore_ascii_case(&target.host)
    };
    if same_host && source.port == target.port && source.database == target.database {
        return Err(NativePostgresError::Configuration(
            "source and target coordinates identify the same database",
        ));
    }
    Ok(())
}

fn validate_native_restore_configuration(
    config: &NativePostgresRestoreConfig,
) -> Result<(), NativePostgresError> {
    config.artifact_directory_identity.validate()?;
    validate_native_postgres_endpoint(&config.source)?;
    validate_native_postgres_endpoint(&config.target)?;
    validate_distinct_endpoint_coordinates(&config.source, &config.target)?;
    drop(open_verified_pgpassfile(&config.passfile)?);
    for path in [&config.pg_dump, &config.pg_restore, &config.psql] {
        if !path.is_file() {
            return Err(NativePostgresError::ToolUnavailable);
        }
    }
    if config.dump_path.exists() || config.dump_path.is_symlink() {
        return Err(NativePostgresError::Configuration(
            "dump path must be new in an existing directory",
        ));
    }
    let parent = config
        .dump_path
        .parent()
        .ok_or(NativePostgresError::Configuration(
            "dump path must have a protected parent directory",
        ))?;
    let protected_parent = prepare_data_directory(parent).map_err(|_| {
        NativePostgresError::Configuration("dump parent directory is not owner-only")
    })?;
    #[cfg(not(windows))]
    let _ = protected_parent;
    #[cfg(windows)]
    validate_sqlite_data_filesystem(&protected_parent).map_err(|_| {
        NativePostgresError::Configuration("dump parent must use a local fixed NTFS or ReFS volume")
    })?;
    Ok(())
}

fn provider_identity(
    client: &mut Client,
) -> Result<PostgresProviderObservationV1, NativePostgresError> {
    let row = client
        .query_one(POSTGRES_PROVIDER_OBSERVATION_SQL, &[])
        .map_err(|_| NativePostgresError::Database)?;
    Ok(PostgresProviderObservationV1 {
        identity: PostgresProviderIdentityV1 {
            system_identifier: row
                .try_get(0)
                .map_err(|_| NativePostgresError::MalformedRow)?,
            database_oid: row
                .try_get(1)
                .map_err(|_| NativePostgresError::MalformedRow)?,
            database_name: row
                .try_get(2)
                .map_err(|_| NativePostgresError::MalformedRow)?,
        },
        marker: row
            .try_get(3)
            .map_err(|_| NativePostgresError::MalformedRow)?,
        other_client_backends: row
            .try_get(4)
            .map_err(|_| NativePostgresError::MalformedRow)?,
        connection_limit: row
            .try_get(5)
            .map_err(|_| NativePostgresError::MalformedRow)?,
        current_user_is_superuser: row
            .try_get(6)
            .map_err(|_| NativePostgresError::MalformedRow)?,
    })
}

fn server_version_num(client: &mut Client) -> Result<u32, NativePostgresError> {
    client
        .query_one("SHOW server_version_num", &[])
        .map_err(|_| NativePostgresError::Database)?
        .try_get::<_, String>(0)
        .map_err(|_| NativePostgresError::MalformedRow)?
        .parse::<u32>()
        .map_err(|_| NativePostgresError::MalformedRow)
}

fn target_user_object_count(client: &mut Client) -> Result<i64, NativePostgresError> {
    client
        .query_one(TARGET_USER_OBJECT_COUNT_SQL, &[])
        .and_then(|row| row.try_get(0))
        .map_err(|_| NativePostgresError::Database)
}

fn postgres_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn postgres_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn random_hex(byte_count: usize) -> Result<String, NativePostgresError> {
    let mut random = vec![0_u8; byte_count];
    getrandom::fill(&mut random).map_err(|_| NativePostgresError::Incomplete)?;
    let mut encoded = String::with_capacity(byte_count.saturating_mul(2));
    for byte in &random {
        write!(&mut encoded, "{byte:02x}").map_err(|_| NativePostgresError::Incomplete)?;
    }
    random.fill(0);
    Ok(encoded)
}

fn pgpass_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

fn restore_role_create_sql(role: &str, password: &str, expires_at: &str) -> String {
    // PostgreSQL's protocol crate implements the server-compatible verifier
    // format and deliberately keeps plaintext out of DDL/log_statement.
    let password_verifier = scram_sha_256(password.as_bytes());
    format!(
        "CREATE ROLE {role} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE \
         NOINHERIT NOREPLICATION NOBYPASSRLS CONNECTION LIMIT 1 \
         PASSWORD {} VALID UNTIL {}",
        postgres_literal(&password_verifier),
        postgres_literal(expires_at),
    )
}

#[cfg(all(test, unix))]
fn create_ephemeral_pgpassfile(
    directory: &Path,
    endpoint: &NativePostgresEndpointV1,
    password: &str,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let authority = NativeRestoreDirectoryAuthorityV1::open(directory)?;
    let mut passfile = create_empty_ephemeral_pgpassfile_in(&authority, endpoint)?;
    populate_ephemeral_pgpassfile(&mut passfile, endpoint, password)?;
    Ok(passfile)
}

fn create_empty_ephemeral_pgpassfile_in(
    directory: &Arc<NativeRestoreDirectoryAuthorityV1>,
    endpoint: &NativePostgresEndpointV1,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let path = directory
        .path
        .join(format!(".{}.pgpass", endpoint.username));
    create_provider_passfile_in(directory.clone(), path)
}

fn populate_ephemeral_pgpassfile(
    passfile: &mut EphemeralPgPassfileV1,
    endpoint: &NativePostgresEndpointV1,
    password: &str,
) -> Result<(), NativePostgresError> {
    passfile.verify()?;
    let file = passfile
        .file
        .as_mut()
        .ok_or(NativePostgresError::Incomplete)?;
    if file
        .metadata()
        .map_err(|_| NativePostgresError::ProviderCommand)?
        .len()
        != 0
    {
        return Err(NativePostgresError::Incomplete);
    }
    writeln!(
        file,
        "{}:{}:{}:{}:{}",
        pgpass_field(&endpoint.host),
        endpoint.port,
        pgpass_field(&endpoint.database),
        pgpass_field(&endpoint.username),
        pgpass_field(password),
    )
    .map_err(|_| NativePostgresError::ProviderCommand)?;
    file.sync_all()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    passfile.freeze_for_provider()?;
    Ok(())
}

fn target_observation_is_admissible(
    observation: &PostgresProviderObservationV1,
    expected_identity: &PostgresProviderIdentityV1,
    expected_marker: &str,
    expected_connection_limit: i32,
) -> bool {
    observation.identity == *expected_identity
        && observation.marker.as_deref() == Some(expected_marker)
        && observation.other_client_backends == 0
        && observation.connection_limit == expected_connection_limit
}

const fn target_restore_connection_limit() -> i32 {
    // PostgreSQL counts both the already-connected keeper and the newly
    // admitted non-superuser restore backend when it evaluates datconnlimit.
    // The direct-superuser keeper bypasses its own admission check but is still
    // counted for the restore-role check, so keeper plus restore requires two.
    2
}

fn source_snapshot_is_safe(snapshot: &str) -> bool {
    !snapshot.is_empty()
        && snapshot.len() <= 256
        && snapshot
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

trait RestoreRoleCleanupBackendV1 {
    fn disable_login(&mut self) -> Result<(), NativePostgresError>;
    fn terminate_backends(&mut self) -> Result<bool, NativePostgresError>;
    fn remaining_backends(&mut self) -> Result<i64, NativePostgresError>;
    fn reassign_owned(&mut self) -> Result<(), NativePostgresError>;
    fn drop_owned(&mut self) -> Result<(), NativePostgresError>;
    fn drop_role(&mut self) -> Result<(), NativePostgresError>;
    fn role_exists(&mut self) -> Result<bool, NativePostgresError>;
}

fn run_restore_role_cleanup_stages(
    backend: &mut impl RestoreRoleCleanupBackendV1,
) -> Result<(), NativePostgresError> {
    match backend.role_exists() {
        Ok(false) => return Ok(()),
        Ok(true) => {}
        Err(_) => return Err(NativePostgresError::Database),
    }
    let login_disabled = backend.disable_login().is_ok();
    let mut database_error = !login_disabled;
    let terminated = match backend.terminate_backends() {
        Ok(value) => value,
        Err(_) => {
            database_error = true;
            false
        }
    };
    let remaining = match backend.remaining_backends() {
        Ok(value) => Some(value),
        Err(_) => {
            database_error = true;
            None
        }
    };
    // Never drop the catalog role until termination succeeded and the
    // admitted keeper proves no session for that login remains. PostgreSQL
    // permits an already-connected backend to survive DROP ROLE, after which
    // it could no longer be safely targeted by role identity.
    if !login_disabled || !terminated || remaining != Some(0) {
        return if database_error {
            Err(NativePostgresError::Database)
        } else {
            Err(NativePostgresError::Incomplete)
        };
    }
    for result in [
        backend.reassign_owned(),
        backend.drop_owned(),
        backend.drop_role(),
    ] {
        if result.is_err() {
            database_error = true;
        }
    }
    let role_still_exists = backend
        .role_exists()
        .map_err(|_| NativePostgresError::Database)?;
    if database_error {
        Err(NativePostgresError::Database)
    } else if role_still_exists {
        Err(NativePostgresError::Incomplete)
    } else {
        Ok(())
    }
}

struct PostgresRestoreRoleCleanupBackendV1<'a> {
    client: &'a mut Client,
    role_name: &'a str,
    role: String,
    target_owner_role: &'a str,
}

impl RestoreRoleCleanupBackendV1 for PostgresRestoreRoleCleanupBackendV1<'_> {
    fn disable_login(&mut self) -> Result<(), NativePostgresError> {
        self.client
            .batch_execute(&format!(
                "ALTER ROLE {} NOLOGIN PASSWORD NULL VALID UNTIL 'epoch'",
                self.role
            ))
            .map_err(|_| NativePostgresError::Database)
    }

    fn terminate_backends(&mut self) -> Result<bool, NativePostgresError> {
        self.client
            .query_one(TERMINATE_RESTORE_ROLE_BACKENDS_SQL, &[&self.role_name])
            .and_then(|row| row.try_get(0))
            .map_err(|_| NativePostgresError::Database)
    }

    fn remaining_backends(&mut self) -> Result<i64, NativePostgresError> {
        self.client
            .query_one(COUNT_RESTORE_ROLE_BACKENDS_SQL, &[&self.role_name])
            .and_then(|row| row.try_get(0))
            .map_err(|_| NativePostgresError::Database)
    }

    fn reassign_owned(&mut self) -> Result<(), NativePostgresError> {
        self.client
            .batch_execute(&format!(
                "REASSIGN OWNED BY {} TO {}",
                self.role,
                postgres_identifier(self.target_owner_role)
            ))
            .map_err(|_| NativePostgresError::Database)
    }

    fn drop_owned(&mut self) -> Result<(), NativePostgresError> {
        self.client
            .batch_execute(&format!("DROP OWNED BY {}", self.role))
            .map_err(|_| NativePostgresError::Database)
    }

    fn drop_role(&mut self) -> Result<(), NativePostgresError> {
        self.client
            .batch_execute(&format!("DROP ROLE {}", self.role))
            .map_err(|_| NativePostgresError::Database)
    }

    fn role_exists(&mut self) -> Result<bool, NativePostgresError> {
        self.client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)",
                &[&self.role_name],
            )
            .and_then(|row| row.try_get(0))
            .map_err(|_| NativePostgresError::Database)
    }
}

impl NativePostgresTargetIsolationLeaseV1 {
    fn set_connection_limit(&mut self, connection_limit: i32) -> Result<(), NativePostgresError> {
        self.target_client
            .batch_execute(&format!(
                "ALTER DATABASE {} CONNECTION LIMIT {connection_limit}",
                postgres_identifier(&self.witness.target.database_name)
            ))
            .map_err(|_| NativePostgresError::Database)?;
        self.active_connection_limit = connection_limit;
        Ok(())
    }

    fn verify_target(&mut self, require_empty: bool) -> Result<(), NativePostgresError> {
        let observation = provider_identity(&mut self.target_client)?;
        if !target_observation_is_admissible(
            &observation,
            &self.witness.target,
            &self.marker,
            self.active_connection_limit,
        ) || observation.current_user_is_superuser != self.target_user_is_superuser
            || (require_empty && target_user_object_count(&mut self.target_client)? != 0)
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn verify_source(&mut self) -> Result<(), NativePostgresError> {
        let source = provider_identity(&mut self.source_client)?;
        if source.identity != self.witness.source || source.identity == self.witness.target {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn export_source_snapshot(&mut self) -> Result<String, NativePostgresError> {
        self.verify_source()?;
        self.source_client
            .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .map_err(|_| NativePostgresError::Database)?;
        let result = self
            .source_client
            .query_one("SELECT pg_export_snapshot()", &[])
            .and_then(|row| row.try_get::<_, String>(0))
            .map_err(|_| NativePostgresError::Database)
            .and_then(|snapshot| {
                if source_snapshot_is_safe(&snapshot) {
                    self.verify_source()?;
                    Ok(snapshot)
                } else {
                    Err(NativePostgresError::Incomplete)
                }
            });
        if result.is_err() {
            let _ = self.source_client.batch_execute("ROLLBACK");
        }
        result
    }

    fn release_source_snapshot(&mut self) -> Result<(), NativePostgresError> {
        self.source_client
            .batch_execute("ROLLBACK")
            .map_err(|_| NativePostgresError::Database)?;
        self.verify_source()
    }

    fn create_restore_credential(
        &mut self,
        config: &NativePostgresRestoreConfig,
        recovery: &NativePostgresWorkerRecoveryPlanV1,
    ) -> Result<NativePostgresRestoreCredentialV1, NativePostgresError> {
        self.verify_source()?;
        self.verify_target(true)?;
        let restore_connection_limit = target_restore_connection_limit();
        if self.active_connection_limit != restore_connection_limit {
            self.set_connection_limit(restore_connection_limit)?;
        }
        self.verify_target(true)?;

        recovery.validate()?;
        let role_name = recovery.role_name.clone();
        let endpoint = NativePostgresEndpointV1 {
            host: config.target.host.clone(),
            port: config.target.port,
            database: config.target.database.clone(),
            username: role_name.clone(),
            tls_mode: config.target.tls_mode,
        };
        let password = Zeroizing::new(password_for_reader(
            &endpoint,
            open_worker_owned_file(&recovery.role_passfile, &recovery.role_passfile_identity)?,
        )?);
        let expires_at: String = self
            .target_client
            .query_one(
                "SELECT (clock_timestamp() + interval '15 minutes')::text",
                &[],
            )
            .and_then(|row| row.try_get(0))
            .map_err(|_| NativePostgresError::Database)?;
        let role = postgres_identifier(&role_name);
        // PostgreSQL utility statements do not accept a bind parameter for a
        // role password. Derive the exact PostgreSQL SCRAM verifier in this
        // process and place only that verifier in the DDL; the random
        // plaintext remains confined to the retained owner-only passfile and
        // zeroizing client memory, so `log_statement = 'ddl'/'all'` cannot
        // disclose a usable client credential.
        let create_sql = restore_role_create_sql(&role, &password, &expires_at);
        // Track the unguessable identity before sending CREATE so an
        // ambiguous transport failure cannot leave a valid login outside the
        // lease. A definite CREATE failure is cleaned as an idempotent absent
        // role; grants are a separate stage and never precede tracking.
        self.outstanding_restore_role = Some(role_name.clone());
        let create_result = self
            .target_client
            .batch_execute(&create_sql)
            .map_err(|_| NativePostgresError::Database);
        drop(create_sql);
        if let Err(error) = create_result {
            self.close_restore_role_admission()?;
            self.retire_outstanding_restore_role()?;
            return Err(error);
        }
        let grant_sql = format!(
            "GRANT {role} TO CURRENT_USER WITH ADMIN OPTION; \
             GRANT CONNECT, CREATE, TEMPORARY ON DATABASE {} TO {role}; \
             GRANT USAGE, CREATE ON SCHEMA public TO {role};",
            postgres_identifier(&self.witness.target.database_name),
        );
        if self.target_client.batch_execute(&grant_sql).is_err() {
            self.close_restore_role_admission()?;
            self.retire_outstanding_restore_role()?;
            return Err(NativePostgresError::Database);
        }

        let credential = NativePostgresRestoreCredentialV1 {
            role_name,
            endpoint,
            passfile_path: recovery.role_passfile.clone(),
            passfile_identity: recovery.role_passfile_identity.clone(),
        };
        let admitted = self.verify_restore_credential(&credential);
        if let Err(error) = admitted {
            self.close_restore_role_admission()?;
            let cleanup = self.retire_outstanding_restore_role();
            cleanup?;
            return Err(error);
        }
        Ok(credential)
    }

    fn verify_restore_credential(
        &mut self,
        credential: &NativePostgresRestoreCredentialV1,
    ) -> Result<(), NativePostgresError> {
        drop(open_worker_owned_file(
            &credential.passfile_path,
            &credential.passfile_identity,
        )?);
        self.verify_target(true)?;
        let admitted = self
            .target_client
            .query_opt(
                "SELECT rolcanlogin AND NOT rolsuper AND NOT rolcreatedb \
                     AND NOT rolcreaterole AND NOT rolinherit AND NOT rolreplication \
                     AND NOT rolbypassrls AND rolconnlimit = 1 \
                     AND rolvaliduntil > clock_timestamp() \
                 FROM pg_roles WHERE rolname = $1",
                &[&credential.role_name],
            )
            .map_err(|_| NativePostgresError::Database)?
            .and_then(|row| row.try_get::<_, bool>(0).ok())
            .unwrap_or(false);
        if admitted {
            Ok(())
        } else {
            Err(NativePostgresError::Incomplete)
        }
    }

    fn retire_outstanding_restore_role(&mut self) -> Result<(), NativePostgresError> {
        let Some(role_name) = self.outstanding_restore_role.clone() else {
            return Ok(());
        };
        let mut backend = PostgresRestoreRoleCleanupBackendV1 {
            client: &mut self.target_client,
            role_name: &role_name,
            role: postgres_identifier(&role_name),
            target_owner_role: &self.target_owner_role,
        };
        run_restore_role_cleanup_stages(&mut backend)?;
        self.outstanding_restore_role = None;
        Ok(())
    }

    fn close_restore_role_admission(&mut self) -> Result<(), NativePostgresError> {
        if self.active_connection_limit != 0 {
            self.set_connection_limit(0)?;
        }
        let observation = provider_identity(&mut self.target_client)?;
        if observation.identity != self.witness.target
            || observation.connection_limit != 0
            || observation.current_user_is_superuser != self.target_user_is_superuser
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn retire_restore_credential(
        &mut self,
        credential: &mut NativePostgresRestoreCredentialV1,
    ) -> Result<(), NativePostgresError> {
        // The database limit of two admits the already-connected keeper plus
        // exactly one restore backend. A superuser keeper is exempt only when
        // it connects; PostgreSQL still counts it when checking the ordinary
        // restore role. Close that child slot immediately after pg_restore.
        let mut cleanup_error = None;
        if let Err(error) = self.close_restore_role_admission() {
            cleanup_error = Some(error);
        }
        if self.outstanding_restore_role.as_deref() != Some(&credential.role_name) {
            cleanup_error.get_or_insert(NativePostgresError::Incomplete);
        }
        if cleanup_error.is_none()
            && let Err(error) = self.retire_outstanding_restore_role()
        {
            cleanup_error = Some(error);
        }
        if let Err(error) = self.verify_target(false) {
            cleanup_error.get_or_insert(error);
        }
        match cleanup_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn capture_target(&mut self) -> Result<Capture, NativePostgresError> {
        self.verify_target(false)?;
        let capture = capture_with_client(&mut self.target_client)?;
        self.verify_target(false)?;
        Ok(capture)
    }

    fn capture_source(&mut self) -> Result<Capture, NativePostgresError> {
        self.verify_source()?;
        let capture = capture_with_client(&mut self.source_client)?;
        self.verify_source()?;
        Ok(capture)
    }

    fn dispose_target_snapshots(
        &mut self,
        restored: &Capture,
        eligible: bool,
    ) -> Result<(bool, usize), NativePostgresError> {
        if !eligible {
            return Ok((false, restored.snapshot_count));
        }
        if restored.snapshot_count == 0 {
            return Ok((true, 0));
        }
        // This keeper session is already bound to admitted target A. Recheck
        // the marker, identity, connection limit, and absence of other clients
        // immediately before the first destructive statement.
        self.verify_target(false)?;
        for room in &restored.rooms {
            self.target_client
                .execute(
                    "DELETE FROM worldstream_room_snapshots WHERE room_id = $1",
                    &[&room.room.room_id],
                )
                .map_err(|_| NativePostgresError::Database)?;
        }
        let after = self.capture_target()?;
        Ok((
            after.snapshot_count == 0 && after.durable_digest == restored.durable_digest,
            after.snapshot_count,
        ))
    }

    fn seal_fail_closed(&mut self) -> Result<(), NativePostgresError> {
        let observation = provider_identity(&mut self.target_client)?;
        if observation.identity != self.witness.target
            || observation.marker.as_deref() != Some(self.marker.as_str())
            || observation.current_user_is_superuser != self.target_user_is_superuser
        {
            return Err(NativePostgresError::Incomplete);
        }
        // Always issue and commit the limit-zero DDL, even when our cached
        // observation already says zero. An administrator can change the
        // catalog between observations; the unpublished target is not sealed
        // until this keeper has durably rewritten the admitted database row.
        self.set_connection_limit(0)?;
        self.retire_outstanding_restore_role()?;
        let sealed = provider_identity(&mut self.target_client)?;
        if !target_observation_is_admissible(&sealed, &self.witness.target, &self.marker, 0)
            || sealed.current_user_is_superuser != self.target_user_is_superuser
        {
            return Err(NativePostgresError::Incomplete);
        }
        self.finalized = true;
        Ok(())
    }
}

impl Drop for NativePostgresTargetIsolationLeaseV1 {
    fn drop(&mut self) {
        if !self.finalized {
            // Error paths never reopen an unverified database. If the keeper
            // session still works and remains bound to the admitted identity,
            // prevent new ordinary-role connections before dropping it.
            if self.active_connection_limit != 0 {
                let _ = self.set_connection_limit(0);
            }
            if self.active_connection_limit == 0 {
                let _ = self.retire_outstanding_restore_role();
            }
            let _ = self.seal_fail_closed();
        }
    }
}

fn verify_disposable_target_preflight(
    config: &NativePostgresRestoreConfig,
    admitted: impl FnOnce(&NativePostgresTargetIsolationWitnessV1) -> Result<(), NativePostgresError>,
) -> Result<NativePostgresTargetIsolationLeaseV1, NativePostgresError> {
    validate_distinct_endpoint_coordinates(&config.source, &config.target)?;
    let source_password = password_for(&config.source, &config.passfile)?;
    let target_password = password_for(&config.target, &config.passfile)?;
    let mut source_client = connect(&config.source, &source_password)?;
    let source = provider_identity(&mut source_client)?;
    let mut target_client = connect(&config.target, &target_password)?;
    let target = provider_identity(&mut target_client)?;
    if source.identity == target.identity {
        return Err(NativePostgresError::Configuration(
            "source and target resolve to the same provider database",
        ));
    }
    if target.marker.as_deref() != Some(NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1) {
        return Err(NativePostgresError::Configuration(
            "target lacks the disposable restore marker",
        ));
    }
    if target.other_client_backends != 0 {
        return Err(NativePostgresError::Configuration(
            "target has another serving client",
        ));
    }
    if !target.current_user_is_superuser || target.connection_limit != 0 {
        return Err(NativePostgresError::Configuration(
            "target native restore requires a sealed direct-superuser administration endpoint",
        ));
    }
    let lock_acquired = target_client
        .query_one(
            "SELECT pg_try_advisory_lock($1, $2)",
            &[
                &TARGET_ISOLATION_ADVISORY_LOCK_CLASS_V1,
                &TARGET_ISOLATION_ADVISORY_LOCK_OBJECT_V1,
            ],
        )
        .and_then(|row| row.try_get::<_, bool>(0))
        .map_err(|_| NativePostgresError::Database)?;
    if !lock_acquired {
        return Err(NativePostgresError::Configuration(
            "target isolation lease is already held",
        ));
    }
    let mut lease = NativePostgresTargetIsolationLeaseV1 {
        witness: NativePostgresTargetIsolationWitnessV1 {
            source: source.identity,
            target: target.identity,
        },
        marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
        target_owner_role: config.target.username.clone(),
        source_client,
        target_client,
        active_connection_limit: target.connection_limit,
        target_user_is_superuser: target.current_user_is_superuser,
        outstanding_restore_role: None,
        finalized: false,
    };
    // The parent has already synced an identity-bound recovery record before
    // spawning this worker. Rebind the live keeper to that exact target before
    // the first connection-limit or role mutation.
    admitted(&lease.witness)?;
    lease
        .set_connection_limit(target_restore_connection_limit())
        .map_err(|_| NativePostgresError::Configuration("target isolation lease cannot be set"))?;
    lease.verify_target(true).map_err(|_| {
        NativePostgresError::Configuration("target database is not isolated and empty")
    })?;
    Ok(lease)
}

fn verify_disposable_target_postflight(
    lease: &mut NativePostgresTargetIsolationLeaseV1,
) -> Result<bool, NativePostgresError> {
    lease.verify_source()?;
    lease.verify_target(false)?;
    Ok(true)
}

/// Serializes one strict bounded worker request to a caller-owned protected
/// file or inherited handle.
///
/// # Errors
///
/// Returns an error when serialization fails or the fixed request bound would
/// be exceeded.
fn write_native_postgres_worker_request(
    request: &NativePostgresWorkerRequestV1,
    mut writer: impl Write,
) -> Result<(), NativePostgresError> {
    let bytes = serde_json::to_vec(request).map_err(|_| NativePostgresError::Incomplete)?;
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_WORKER_REQUEST_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    writer
        .write_all(&bytes)
        .and_then(|()| writer.flush())
        .map_err(|_| NativePostgresError::ProviderCommand)
}

fn read_native_worker_bytes(
    reader: impl Read,
    maximum_bytes: usize,
) -> Result<Vec<u8>, NativePostgresError> {
    let read_limit = maximum_bytes
        .checked_add(1)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(NativePostgresError::Incomplete)?;
    let mut bytes = Vec::with_capacity(maximum_bytes.min(64 * 1024).saturating_add(1));
    reader
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(bytes)
}

/// Executes one contained native worker request from an inherited protected
/// input handle and writes a strict redacted result to an inherited protected
/// output handle. The outer operator process owns the absolute deadline and
/// must not accept this result until the worker Job/group is drained.
///
/// # Errors
///
/// Returns an error for malformed input or an output write failure. Provider
/// failures are encoded as a closed worker result and return `Ok(false)`.
pub fn execute_native_postgres_restore_worker(
    reader: impl Read,
    writer: impl Write,
    dump_writer: impl Write,
) -> Result<bool, NativePostgresError> {
    if std::env::var_os(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1).as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err(NativePostgresError::Configuration(
            "native worker requires supervised containment",
        ));
    }
    execute_contained_native_postgres_restore_worker(reader, writer, dump_writer)
}

fn execute_contained_native_postgres_restore_worker(
    reader: impl Read,
    mut writer: impl Write,
    mut dump_writer: impl Write,
) -> Result<bool, NativePostgresError> {
    let request_bytes = read_native_worker_bytes(reader, MAX_NATIVE_WORKER_REQUEST_BYTES_V1)?;
    let request: NativePostgresWorkerRequestV1 =
        serde_json::from_slice(&request_bytes).map_err(|_| NativePostgresError::Incomplete)?;
    if serde_json::to_vec(&request).map_err(|_| NativePostgresError::Incomplete)? != request_bytes {
        return Err(NativePostgresError::Incomplete);
    }
    let (config, recovery) = request.into_parts()?;
    let (result, accepted) =
        match run_native_postgres_restore_in_worker(&config, &recovery, &mut dump_writer) {
            Ok(checkpoint) => {
                let accepted = native_worker_checkpoint_is_verified(&checkpoint.report);
                (
                    NativePostgresWorkerResultV1 {
                        schema: NATIVE_POSTGRES_WORKER_RESULT_SCHEMA_V1.to_owned(),
                        record: "result".to_owned(),
                        status: "complete".to_owned(),
                        report: Some(checkpoint.report),
                        restored_global_digest: Some(
                            checkpoint.restored_global_digest.as_str().to_owned(),
                        ),
                        reason: "native_worker_completed".to_owned(),
                    },
                    accepted,
                )
            }
            Err(_) => (
                NativePostgresWorkerResultV1 {
                    schema: NATIVE_POSTGRES_WORKER_RESULT_SCHEMA_V1.to_owned(),
                    record: "result".to_owned(),
                    status: "failed".to_owned(),
                    report: None,
                    restored_global_digest: None,
                    reason: "native_worker_provider_operation_failed".to_owned(),
                },
                false,
            ),
        };
    write_native_worker_record(&mut writer, &result)?;
    Ok(accepted)
}

fn write_native_worker_record(
    writer: &mut impl Write,
    record: &impl Serialize,
) -> Result<(), NativePostgresError> {
    let bytes = serde_json::to_vec(record).map_err(|_| NativePostgresError::Incomplete)?;
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_WORKER_RESULT_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    writer
        .write_all(&bytes)
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|_| NativePostgresError::ProviderCommand)
}

/// Executes one of the two deadline-contained native publication phases from
/// exact inherited standard handles. Stdin is the private report/request
/// staging file, stdout is the append-only recovery journal, and stderr is the
/// private/native dump. No path is reopened for those three authorities.
/// This helper never constructs a trusted witness; only the parent may do so
/// after the helper Job/group has been conclusively drained.
///
/// # Errors
///
/// Returns a closed error for an unsupervised invocation, a noncanonical
/// request, any handle/identity mismatch, or incomplete durable publication.
pub fn execute_native_postgres_restore_commit_worker() -> Result<(), NativePostgresError> {
    if std::env::var_os(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1).as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err(NativePostgresError::Configuration(
            "native worker requires supervised containment",
        ));
    }
    let (report_staging, recovery, dump) = duplicate_native_commit_standard_files()?;
    execute_contained_native_postgres_commit_worker(report_staging, recovery, dump)
}

fn duplicate_native_commit_standard_files()
-> Result<(fs::File, fs::File, fs::File), NativePostgresError> {
    #[cfg(target_os = "linux")]
    {
        let duplicate = |descriptor: u8| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("/proc/self/fd/{descriptor}"))
                .map_err(|_| NativePostgresError::ProviderCommand)
        };
        Ok((duplicate(0)?, duplicate(1)?, duplicate(2)?))
    }
    #[cfg(windows)]
    {
        use worldstream_windows_handle::{StandardHandle, duplicate_standard_handle};

        Ok((
            duplicate_standard_handle(StandardHandle::Stdin)
                .map_err(|_| NativePostgresError::ProviderCommand)?,
            duplicate_standard_handle(StandardHandle::Stdout)
                .map_err(|_| NativePostgresError::ProviderCommand)?,
            duplicate_standard_handle(StandardHandle::Stderr)
                .map_err(|_| NativePostgresError::ProviderCommand)?,
        ))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    Err(NativePostgresError::Configuration(
        "native PostgreSQL commit is supported only on Linux and Windows",
    ))
}

#[allow(clippy::too_many_lines)]
fn execute_contained_native_postgres_commit_worker(
    mut report_staging: fs::File,
    mut recovery: fs::File,
    mut dump: fs::File,
) -> Result<(), NativePostgresError> {
    report_staging
        .seek(SeekFrom::Start(0))
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let request_bytes =
        read_native_worker_bytes(&mut report_staging, MAX_NATIVE_COMMIT_REQUEST_BYTES_V1)?;
    let request: NativePostgresCommitRequestV1 =
        serde_json::from_slice(&request_bytes).map_err(|_| NativePostgresError::Incomplete)?;
    if serde_json::to_vec(&request).map_err(|_| NativePostgresError::Incomplete)? != request_bytes {
        return Err(NativePostgresError::Incomplete);
    }
    request.validate()?;
    let directory = NativeRestoreDirectoryAuthorityV1::open_expected(
        &request.artifact_directory_path,
        &request.artifact_directory_identity,
    )?;
    let recovery_identity = ExclusiveFileIdentityV1::for_file(&recovery)?;
    let report_identity = ExclusiveFileIdentityV1::for_file(&report_staging)?;
    let dump_identity = ExclusiveFileIdentityV1::for_file(&dump)?;
    if !recovery_identity.matches_worker_identity(&request.recovery_identity)
        || !report_identity.matches_worker_identity(&request.report_staging_identity)
        || !dump_identity.matches_worker_identity(&request.recovery_record.private_dump_identity)
    {
        return Err(NativePostgresError::Incomplete);
    }
    let recovery_name = directory.name_for(&request.recovery_path)?;
    if !directory.still_names(&recovery_name, &recovery_identity)? {
        return Err(NativePostgresError::Incomplete);
    }
    let expected_dump_digest = DigestV1::parse(request.report.native_dump_digest.clone())
        .map_err(|_| NativePostgresError::Incomplete)?;
    let _restored_global_digest = DigestV1::parse(request.restored_global_digest.clone())
        .map_err(|_| NativePostgresError::Incomplete)?;
    recovery
        .seek(SeekFrom::End(0))
        .map_err(|_| NativePostgresError::ProviderCommand)?;

    match request.phase {
        NativePostgresCommitPhaseV1::Dump => {
            dump.sync_all()
                .and_then(|()| dump.seek(SeekFrom::Start(0)).map(|_| ()))
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            let dump_len = dump
                .metadata()
                .map_err(|_| NativePostgresError::ProviderCommand)?
                .len();
            let observed_dump_digest =
                hash_bounded_reader(&mut dump, request.report.native_dump_size_bytes)?;
            if dump_len != request.report.native_dump_size_bytes
                || observed_dump_digest != expected_dump_digest
            {
                return Err(NativePostgresError::Incomplete);
            }
            let mut publication = NativeDumpFileV1 {
                directory: Some(directory.clone()),
                file: Some(dump),
                path: request.recovery_record.private_dump_path.clone(),
                publication_path: request.recovery_record.publication_path.clone(),
                identity: dump_identity,
                digest: Some(expected_dump_digest.clone()),
                byte_count: Some(dump_len),
                retained: false,
            };
            let mut record = request.recovery_record;
            "publication_commit_in_progress".clone_into(&mut record.disposition);
            record.native_dump_digest = Some(expected_dump_digest.as_str().to_owned());
            record.native_dump_size_bytes = Some(dump_len);
            "repair_target_then_scrub_or_validate_the_identity_bound_native_dump"
                .clone_into(&mut record.operator_action);
            append_durable_native_recovery_record(
                &record,
                &mut recovery,
                &recovery_identity,
                &request.recovery_path,
                &directory,
            )?;
            publication.retain()?;
            "publication_committed_pending_report_ack".clone_into(&mut record.disposition);
            "persist_the_redacted_report_then_acknowledge_the_publication"
                .clone_into(&mut record.operator_action);
            append_durable_native_recovery_record(
                &record,
                &mut recovery,
                &recovery_identity,
                &request.recovery_path,
                &directory,
            )?;
        }
        NativePostgresCommitPhaseV1::ReportAndAcknowledge => {
            let mut published_dump = NativeDumpFileV1 {
                directory: Some(directory.clone()),
                file: Some(dump),
                path: request.recovery_record.publication_path.clone(),
                publication_path: request.recovery_record.publication_path.clone(),
                identity: dump_identity,
                digest: Some(expected_dump_digest.clone()),
                byte_count: Some(request.report.native_dump_size_bytes),
                retained: true,
            };
            published_dump.verify_retained_publication(&expected_dump_digest)?;

            let mut report_bytes =
                serde_json::to_vec(&request.report).map_err(|_| NativePostgresError::Incomplete)?;
            if report_bytes.is_empty() || report_bytes.len() >= MAX_NATIVE_WORKER_RESULT_BYTES_V1 {
                return Err(NativePostgresError::Incomplete);
            }
            report_bytes.push(b'\n');
            report_staging
                .set_len(0)
                .and_then(|()| report_staging.seek(SeekFrom::Start(0)).map(|_| ()))
                .and_then(|()| report_staging.write_all(&report_bytes))
                .and_then(|()| report_staging.sync_all())
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            let report_digest = DigestV1::hash(&report_bytes);
            let report_worker_identity = report_identity.worker_identity();
            let mut report_publication = NativeDumpFileV1 {
                directory: Some(directory.clone()),
                file: Some(report_staging),
                path: request.report_staging_path.clone(),
                publication_path: request.report_path.clone(),
                identity: report_identity,
                digest: Some(report_digest.clone()),
                byte_count: Some(report_bytes.len() as u64),
                retained: false,
            };
            let mut record = request.recovery_record;
            "report_commit_in_progress".clone_into(&mut record.disposition);
            record.private_report_path = Some(request.report_staging_path);
            record.report_path = Some(request.report_path);
            record.report_identity = Some(report_worker_identity);
            record.report_digest = Some(report_digest.as_str().to_owned());
            record.report_size_bytes = Some(report_bytes.len() as u64);
            "validate_or_scrub_the_identity_bound_dump_and_report_then_repair_target"
                .clone_into(&mut record.operator_action);
            append_durable_native_recovery_record(
                &record,
                &mut recovery,
                &recovery_identity,
                &request.recovery_path,
                &directory,
            )?;
            report_publication.retain()?;
            report_publication.verify_retained_publication(&report_digest)?;
            published_dump.verify_retained_publication(&expected_dump_digest)?;
            "report_committed_pending_recovery_ack".clone_into(&mut record.disposition);
            "validate_the_durable_report_and_dump_then_acknowledge_publication"
                .clone_into(&mut record.operator_action);
            append_durable_native_recovery_record(
                &record,
                &mut recovery,
                &recovery_identity,
                &request.recovery_path,
                &directory,
            )?;
            // The helper never constructs the non-serializable witness. The
            // parent placed the exact immutable Ready report in this second
            // phase only after the dump phase was conclusively drained.
            "publication_acknowledged".clone_into(&mut record.disposition);
            "publication_and_report_are_durably_committed".clone_into(&mut record.operator_action);
            append_durable_native_recovery_record(
                &record,
                &mut recovery,
                &recovery_identity,
                &request.recovery_path,
                &directory,
            )?;
        }
    }
    Ok(())
}

/// Executes the supervisor's idempotent fail-closed target repair request.
/// This entry is hidden behind the same outer Job/group and absolute deadline
/// as the main worker and never emits a public Ready report.
///
/// # Errors
///
/// Returns a closed error unless the admitted target is identity-bound,
/// connection-limited to zero, free of the one-use role and its sessions, and
/// still marked disposable.
pub fn execute_native_postgres_restore_repair_worker(
    reader: impl Read,
    mut writer: impl Write,
) -> Result<(), NativePostgresError> {
    if std::env::var_os(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1).as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err(NativePostgresError::Configuration(
            "native worker requires supervised containment",
        ));
    }
    let bytes = read_native_worker_bytes(reader, MAX_NATIVE_WORKER_REQUEST_BYTES_V1)?;
    let request: NativePostgresRepairRequestV1 =
        serde_json::from_slice(&bytes).map_err(|_| NativePostgresError::Incomplete)?;
    if serde_json::to_vec(&request).map_err(|_| NativePostgresError::Incomplete)? != bytes
        || request.schema != NATIVE_POSTGRES_REPAIR_REQUEST_SCHEMA_V1
        || request.target_marker != NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1
        || !restore_role_name_is_valid(&request.role_name)
    {
        return Err(NativePostgresError::Incomplete);
    }
    repair_native_postgres_target(&request)?;
    let result = NativePostgresRepairResultV1 {
        schema: NATIVE_POSTGRES_REPAIR_RESULT_SCHEMA_V1.to_owned(),
        status: "repaired".to_owned(),
        reason: "native_target_identity_bound_sealed_and_role_absent".to_owned(),
    };
    let result_bytes = serde_json::to_vec(&result).map_err(|_| NativePostgresError::Incomplete)?;
    writer
        .write_all(&result_bytes)
        .and_then(|()| writer.flush())
        .map_err(|_| NativePostgresError::ProviderCommand)
}

fn repair_native_postgres_target(
    request: &NativePostgresRepairRequestV1,
) -> Result<(), NativePostgresError> {
    validate_native_postgres_endpoint(&request.target)?;
    if request.expected_target.database_name != request.target.database {
        return Err(NativePostgresError::Incomplete);
    }
    let password = Zeroizing::new(password_for_reader(
        &request.target,
        open_worker_owned_file(
            &request.operator_passfile,
            &request.operator_passfile_identity,
        )?,
    )?);
    let mut client = connect(&request.target, &password)?;
    let admitted = provider_identity(&mut client)?;
    if admitted.identity != request.expected_target
        || admitted.marker.as_deref() != Some(request.target_marker.as_str())
        || !admitted.current_user_is_superuser
    {
        return Err(NativePostgresError::Incomplete);
    }
    let lock_acquired = client
        .query_one(
            "SELECT pg_try_advisory_lock($1, $2)",
            &[
                &TARGET_ISOLATION_ADVISORY_LOCK_CLASS_V1,
                &TARGET_ISOLATION_ADVISORY_LOCK_OBJECT_V1,
            ],
        )
        .and_then(|row| row.try_get::<_, bool>(0))
        .map_err(|_| NativePostgresError::Database)?;
    if !lock_acquired {
        return Err(NativePostgresError::Incomplete);
    }
    client
        .batch_execute(&format!(
            "ALTER DATABASE {} CONNECTION LIMIT 0",
            postgres_identifier(&request.expected_target.database_name)
        ))
        .map_err(|_| NativePostgresError::Database)?;
    let mut backend = PostgresRestoreRoleCleanupBackendV1 {
        client: &mut client,
        role_name: &request.role_name,
        role: postgres_identifier(&request.role_name),
        target_owner_role: &request.target.username,
    };
    run_restore_role_cleanup_stages(&mut backend)?;
    let sealed = provider_identity(&mut client)?;
    if !target_observation_is_admissible(
        &sealed,
        &request.expected_target,
        &request.target_marker,
        0,
    ) {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(())
}

fn native_worker_report_is_ready(report: &NativePostgresRestoreReportV1) -> bool {
    let digest_is_canonical = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    let expected_domains = POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
        .into_iter()
        .map(NativeRestoreDurableDomainV1::as_str)
        .collect::<BTreeSet<_>>();
    let actual_domains = report
        .durable_domain_inventory
        .iter()
        .map(|domain| domain.domain.as_str())
        .collect::<BTreeSet<_>>();
    let dump_digest = DigestV1::parse(report.native_dump_digest.clone()).ok();
    let backup_point_is_bound = dump_digest.as_ref().is_some_and(|dump_digest| {
        report.backup_id == postgres_native_backup_id(dump_digest)
            && postgres_native_point_digest(dump_digest)
                .is_ok_and(|digest| report.native_point_digest == digest.as_str())
    });
    let provider_identity_is_canonical = |identity: &PostgresProviderIdentityV1| {
        !identity.database_name.is_empty()
            && identity.database_name.len() <= 63
            && identity
                .system_identifier
                .parse::<u64>()
                .is_ok_and(|value| value > 0)
            && identity
                .database_oid
                .parse::<u32>()
                .is_ok_and(|value| value > 0)
    };
    report.schema == NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1
        && report.status == "ready"
        && report.reason == "postgres_native_restore_verified_by_unified_verifier"
        && !report.release_evidence
        && report.native_dump_restore == "pass"
        && backup_point_is_bound
        && report.native_dump_size_bytes > 0
        && provider_identity_is_canonical(&report.source_provider_identity)
        && provider_identity_is_canonical(&report.target_provider_identity)
        && report.source_provider_identity != report.target_provider_identity
        && report.verifier.is_ready()
        && report.source_unchanged
        && report.exact_restored_row_set
        && report.snapshots_disposable
        && report.target_isolated
        && !report.target_published
        && report.cleanup_required
        && report.native_witness_minted
        && !report.secrets_emitted
        && report.source_version_num == Some(REQUIRED_POSTGRES_VERSION_NUM)
        && report.restored_version_num == Some(REQUIRED_POSTGRES_VERSION_NUM)
        && report.semantic_receipts_verified
        && report.activation_intents_verified
        && report.activation_operation_receipts_verified
        && report.activation_request_evidence == "stored_canonical_hash_only_verified"
        && report.authority_state_verified
        && report.durable_domains_verified
        && report.verifier_scope.profile == "full_deployment_all_durable_domains"
        && report.verifier_scope.source_pack_identity_count
            == report.verifier_scope.restored_pack_identity_count
        && report.verifier_scope.source_resource_identity_count
            == report.verifier_scope.restored_resource_identity_count
        && report.verifier_scope.source_fired_timer_count
            == report.verifier_scope.restored_fired_timer_count
        && digest_is_canonical(&report.source_durable_domains_digest)
        && report.source_durable_domains_digest == report.restored_durable_domains_digest
        && report.durable_domain_inventory.len() == POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1.len()
        && actual_domains == expected_domains
        && report.durable_domain_inventory.iter().all(|domain| {
            digest_is_canonical(&domain.source_digest)
                && domain.source_row_count == domain.restored_row_count
                && domain.source_digest == domain.restored_digest
        })
        && report.restored_snapshot_count_after == 0
}

fn native_worker_checkpoint_is_verified(report: &NativePostgresRestoreReportV1) -> bool {
    if report.status != "verified_pending_supervisor"
        || report.reason != "native_worker_verified_pending_containment_repair_and_publication"
        || report.native_dump_restore != "verified_pending_publication"
        || report.native_witness_minted
    {
        return false;
    }
    let mut final_report = report.clone();
    "ready".clone_into(&mut final_report.status);
    "postgres_native_restore_verified_by_unified_verifier".clone_into(&mut final_report.reason);
    "pass".clone_into(&mut final_report.native_dump_restore);
    final_report.native_witness_minted = true;
    native_worker_report_is_ready(&final_report)
}

/// Decodes and validates a bounded result only after the caller has proven the
/// native worker containment is empty and reread the same protected output
/// handle.
///
/// # Errors
///
/// Returns a closed error for a malformed, failed, or internally inconsistent
/// worker result.
struct NativePostgresObservedWorkerOutputV1 {
    result: Option<NativePostgresWorkerResultV1>,
    stream_complete: bool,
}

fn read_native_postgres_worker_output(
    reader: impl Read,
) -> Result<NativePostgresObservedWorkerOutputV1, NativePostgresError> {
    let read_limit = MAX_NATIVE_WORKER_RESULT_BYTES_V1
        .checked_add(1)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(NativePostgresError::Incomplete)?;
    let mut bytes = Vec::new();
    reader
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    if bytes.len() > MAX_NATIVE_WORKER_RESULT_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    let stream_complete = bytes.last() == Some(&b'\n');
    let complete_length = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index.saturating_add(1));
    let mut result = None;
    let complete_records = complete_length
        .checked_sub(1)
        .map_or(&bytes[0..0], |length| &bytes[..length]);
    for line in complete_records.split(|byte| *byte == b'\n') {
        if complete_length == 0 {
            break;
        }
        if line.is_empty() {
            return Err(NativePostgresError::Incomplete);
        }
        let value: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| NativePostgresError::Incomplete)?;
        match value.get("record").and_then(serde_json::Value::as_str) {
            Some("result") if result.is_none() => {
                let record: NativePostgresWorkerResultV1 =
                    serde_json::from_value(value).map_err(|_| NativePostgresError::Incomplete)?;
                if record.schema != NATIVE_POSTGRES_WORKER_RESULT_SCHEMA_V1
                    || serde_json::to_vec(&record).map_err(|_| NativePostgresError::Incomplete)?
                        != line
                {
                    return Err(NativePostgresError::Incomplete);
                }
                result = Some(record);
            }
            _ => return Err(NativePostgresError::Incomplete),
        }
    }
    Ok(NativePostgresObservedWorkerOutputV1 {
        result,
        stream_complete,
    })
}

fn verified_native_worker_checkpoint(
    result: NativePostgresWorkerResultV1,
) -> Result<NativePostgresVerifiedWorkerCheckpointV1, NativePostgresError> {
    if result.record != "result" {
        return Err(NativePostgresError::Incomplete);
    }
    if result.status == "failed"
        && result.reason == "native_worker_provider_operation_failed"
        && result.report.is_none()
        && result.restored_global_digest.is_none()
    {
        return Err(NativePostgresError::ProviderCommand);
    }
    if result.status != "complete" || result.reason != "native_worker_completed" {
        return Err(NativePostgresError::Incomplete);
    }
    let report = result.report.ok_or(NativePostgresError::Incomplete)?;
    if !native_worker_checkpoint_is_verified(&report) {
        return Err(NativePostgresError::Incomplete);
    }
    let restored_global_digest = DigestV1::parse(
        result
            .restored_global_digest
            .ok_or(NativePostgresError::Incomplete)?,
    )
    .map_err(|_| NativePostgresError::Incomplete)?;
    Ok(NativePostgresVerifiedWorkerCheckpointV1 {
        report,
        restored_global_digest,
    })
}

#[cfg(all(test, target_os = "linux"))]
fn create_native_worker_transport_file(
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let directory = NativeRestoreDirectoryAuthorityV1::open(
        path.parent().ok_or(NativePostgresError::Incomplete)?,
    )?;
    create_native_worker_transport_file_in(directory, path)
}

fn create_native_worker_transport_file_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    directory.name_for(&path)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;

        let owned = rustix::fs::memfd_create(
            "worldstream-native-worker-transport-v1",
            rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .map_err(|_| NativePostgresError::ProviderCommand)?;
        let file = fs::File::from(owned);
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        Ok(EphemeralPgPassfileV1 {
            directory,
            path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::Anonymous,
            removed: false,
        })
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let parent = path.parent().ok_or(NativePostgresError::Incomplete)?;
        prepare_data_directory(parent).map_err(|_| NativePostgresError::ProviderCommand)?;
        let file =
            tempfile::tempfile_in(parent).map_err(|_| NativePostgresError::ProviderCommand)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        Ok(EphemeralPgPassfileV1 {
            directory,
            path,
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::Anonymous,
            removed: false,
        })
    }
    #[cfg(windows)]
    {
        let file = open_exclusive_private_file_in(&directory, &path)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        Ok(EphemeralPgPassfileV1 {
            directory,
            path,
            file: Some(file),
            identity,
            binding: PrivateFileBindingV1::OwnedPath,
            removed: false,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(NativePostgresError::Configuration(
            "native worker transport is unsupported on this platform",
        ))
    }
}

/// Creates the primary worker result as a parent-retained, rename-capable
/// staging file. Unlike request/passfile transports it must remain writable by
/// the later contained commit helper and eventually becomes the exact durable
/// public report through retained-handle publication.
fn create_native_commit_staging_file_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let name = directory.name_for(&path)?;
    let file = directory.create_dump_file(&name)?;
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    #[cfg(target_os = "linux")]
    let binding = PrivateFileBindingV1::Anonymous;
    #[cfg(not(target_os = "linux"))]
    let binding = PrivateFileBindingV1::OwnedPath;
    Ok(EphemeralPgPassfileV1 {
        directory,
        path,
        file: Some(file),
        identity,
        binding,
        removed: false,
    })
}

#[cfg(test)]
fn create_native_recovery_record_file(
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let directory = NativeRestoreDirectoryAuthorityV1::open(
        path.parent().ok_or(NativePostgresError::Incomplete)?,
    )?;
    create_native_recovery_record_file_in(directory, path)
}

fn create_native_recovery_record_file_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: PathBuf,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let file = open_exclusive_private_file_in(&directory, &path)?;
    #[cfg(unix)]
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    Ok(EphemeralPgPassfileV1 {
        directory,
        path,
        file: Some(file),
        identity,
        binding: PrivateFileBindingV1::OwnedPath,
        removed: false,
    })
}

#[cfg(test)]
fn open_existing_native_recovery_record_file(
    path: &Path,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let directory = NativeRestoreDirectoryAuthorityV1::open(
        path.parent().ok_or(NativePostgresError::Incomplete)?,
    )?;
    open_existing_native_recovery_record_file_in(directory, path)
}

fn open_existing_native_recovery_record_file_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: &Path,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    let name = directory.name_for(path)?;
    let file = directory
        .open_existing(&name)?
        .ok_or(NativePostgresError::ProviderCommand)?;
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    #[cfg(unix)]
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    if !identity.still_names_file(path)? {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(EphemeralPgPassfileV1 {
        directory,
        path: path.to_path_buf(),
        file: Some(file),
        identity,
        binding: PrivateFileBindingV1::OwnedPath,
        // An existing recovery journal is never a cleanup temporary. Every
        // early return must preserve it; successful recovery appends a durable
        // tombstone instead of deleting history.
        removed: true,
    })
}

fn open_expected_native_recovery_record_file_in(
    directory: Arc<NativeRestoreDirectoryAuthorityV1>,
    path: &Path,
    expected_identity: &NativePostgresArtifactDirectoryIdentityV1,
) -> Result<EphemeralPgPassfileV1, NativePostgresError> {
    expected_identity.validate()?;
    let recovery = open_existing_native_recovery_record_file_in(directory, path)?;
    if recovery.identity.worker_identity() != *expected_identity {
        return Err(NativePostgresError::Configuration(
            "native recovery journal identity changed before repair admission",
        ));
    }
    Ok(recovery)
}

fn read_native_recovery_record(
    file: &mut fs::File,
) -> Result<NativePostgresRecoveryRecordV1, NativePostgresError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let mut bytes = Vec::new();
    Read::take(
        &mut *file,
        (MAX_NATIVE_WORKER_REQUEST_BYTES_V1 as u64).saturating_add(1),
    )
    .read_to_end(&mut bytes)
    .map_err(|_| NativePostgresError::ProviderCommand)?;
    if bytes.len() > MAX_NATIVE_WORKER_REQUEST_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    let complete_length = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index.saturating_add(1));
    if complete_length == 0 {
        return Err(NativePostgresError::Incomplete);
    }
    let mut first = None;
    let mut previous_rank = None;
    let mut previous = None::<NativePostgresRecoveryRecordV1>;
    for line in bytes[..complete_length.saturating_sub(1)].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            return Err(NativePostgresError::Incomplete);
        }
        let record: NativePostgresRecoveryRecordV1 =
            serde_json::from_slice(line).map_err(|_| NativePostgresError::Incomplete)?;
        if serde_json::to_vec(&record).map_err(|_| NativePostgresError::Incomplete)? != line {
            return Err(NativePostgresError::Incomplete);
        }
        record.validate()?;
        let rank = match record.disposition.as_str() {
            "repair_required_if_primary_interrupted" => 0_u8,
            "publication_commit_in_progress" => 1,
            "publication_committed_pending_report_ack" => 2,
            "report_commit_in_progress" => 3,
            "report_committed_pending_recovery_ack" => 4,
            "publication_acknowledged" => 5,
            "recovery_completed_artifacts_scrubbed" => 6,
            _ => return Err(NativePostgresError::Incomplete),
        };
        if first.is_none() {
            if rank != 0 || record.native_dump_digest.is_some() || record.report_digest.is_some() {
                return Err(NativePostgresError::Incomplete);
            }
            first = Some(record.clone());
        }
        let first_record = first.as_ref().ok_or(NativePostgresError::Incomplete)?;
        if !record.has_same_recovery_authority(first_record)
            || previous_rank.is_some_and(|previous| rank < previous)
        {
            return Err(NativePostgresError::Incomplete);
        }
        if let Some(prior) = previous.as_ref() {
            if prior.native_dump_digest.is_some()
                && (record.native_dump_digest != prior.native_dump_digest
                    || record.native_dump_size_bytes != prior.native_dump_size_bytes)
            {
                return Err(NativePostgresError::Incomplete);
            }
            if prior.report_digest.is_some()
                && (record.private_report_path != prior.private_report_path
                    || record.report_path != prior.report_path
                    || record.report_identity != prior.report_identity
                    || record.report_digest != prior.report_digest
                    || record.report_size_bytes != prior.report_size_bytes)
            {
                return Err(NativePostgresError::Incomplete);
            }
        }
        previous_rank = Some(rank);
        previous = Some(record);
    }
    let last = previous.ok_or(NativePostgresError::Incomplete)?;
    if complete_length != bytes.len() {
        file.set_len(complete_length as u64)
            .and_then(|()| file.sync_all())
            .map_err(|_| NativePostgresError::ProviderCommand)?;
    }
    Ok(last)
}

enum NativeRecoveryArtifactCandidateV1 {
    Absent,
    Foreign,
    Exact {
        file: fs::File,
        identity: ExclusiveFileIdentityV1,
    },
}

fn open_native_recovery_artifact(
    directory: &NativeRestoreDirectoryAuthorityV1,
    path: &Path,
    expected: &NativeWorkerFileIdentityV1,
) -> Result<NativeRecoveryArtifactCandidateV1, NativePostgresError> {
    let name = path.file_name().ok_or(NativePostgresError::Incomplete)?;
    directory.path_for(name)?;
    let file = match directory.open_existing(name)? {
        Some(file) => file,
        None => return Ok(NativeRecoveryArtifactCandidateV1::Absent),
    };
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    if !identity.matches_worker_identity(expected) {
        return Ok(NativeRecoveryArtifactCandidateV1::Foreign);
    }
    if !directory.still_names(name, &identity)? {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(NativeRecoveryArtifactCandidateV1::Exact { file, identity })
}

fn scrub_native_recovery_artifact(
    directory: &NativeRestoreDirectoryAuthorityV1,
    private_path: &Path,
    private_is_anonymous: bool,
    publication_path: &Path,
    expected: &NativeWorkerFileIdentityV1,
    publication_must_exist: bool,
) -> Result<(), NativePostgresError> {
    let private = if private_is_anonymous {
        NativeRecoveryArtifactCandidateV1::Absent
    } else {
        open_native_recovery_artifact(directory, private_path, expected)?
    };
    let public = open_native_recovery_artifact(directory, publication_path, expected)?;
    let private_exact = matches!(&private, NativeRecoveryArtifactCandidateV1::Exact { .. });
    let public_exact = matches!(&public, NativeRecoveryArtifactCandidateV1::Exact { .. });
    let private_foreign = matches!(&private, NativeRecoveryArtifactCandidateV1::Foreign);
    let public_foreign = matches!(&public, NativeRecoveryArtifactCandidateV1::Foreign);
    if private_foreign
        || (publication_must_exist && !public_exact)
        || (!private_exact && !public_exact && !private_is_anonymous)
        || (public_foreign && !private_exact && !private_is_anonymous)
    {
        return Err(NativePostgresError::Incomplete);
    }
    for (candidate, path) in [(private, private_path), (public, publication_path)] {
        if let NativeRecoveryArtifactCandidateV1::Exact { file, identity } = candidate {
            file.set_len(0)
                .and_then(|()| file.sync_all())
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            let name = path.file_name().ok_or(NativePostgresError::Incomplete)?;
            if !directory.still_names(name, &identity)? {
                return Err(NativePostgresError::Incomplete);
            }
        }
    }
    Ok(())
}

fn scrub_native_recovery_secret(
    directory: &NativeRestoreDirectoryAuthorityV1,
    path: &Path,
    expected: &NativeWorkerFileIdentityV1,
    was_anonymous: bool,
) -> Result<(), NativePostgresError> {
    if was_anonymous {
        // Linux memfd credentials have no directory entry and disappear when
        // the interrupted supervisor's last inherited descriptor closes. The
        // recorded `/proc/self/fd/N` coordinate is diagnostic only and must
        // never be reopened by a later process because that descriptor number
        // may have been recycled for an unrelated file.
        return if cfg!(target_os = "linux") {
            Ok(())
        } else {
            Err(NativePostgresError::Incomplete)
        };
    }
    let (file, identity) = match open_native_recovery_artifact(directory, path, expected)? {
        NativeRecoveryArtifactCandidateV1::Absent => return Ok(()),
        NativeRecoveryArtifactCandidateV1::Foreign => {
            return Err(NativePostgresError::Incomplete);
        }
        NativeRecoveryArtifactCandidateV1::Exact { file, identity } => (file, identity),
    };
    file.set_len(0)
        .and_then(|()| file.sync_all())
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let name = path.file_name().ok_or(NativePostgresError::Incomplete)?;
    if !directory.still_names(name, &identity)? {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(())
}

fn write_durable_native_recovery_record(
    record: &NativePostgresRecoveryRecordV1,
    record_file: &mut EphemeralPgPassfileV1,
) -> Result<(), NativePostgresError> {
    record_file.verify()?;
    append_durable_native_recovery_record(
        record,
        record_file
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?,
        &record_file.identity,
        &record_file.path,
        &record_file.directory,
    )
}

fn append_durable_native_recovery_record(
    record: &NativePostgresRecoveryRecordV1,
    file: &mut fs::File,
    identity: &ExclusiveFileIdentityV1,
    path: &Path,
    directory: &NativeRestoreDirectoryAuthorityV1,
) -> Result<(), NativePostgresError> {
    record.validate()?;
    let name = path.file_name().ok_or(NativePostgresError::Incomplete)?;
    if !directory.still_names(name, identity)? {
        return Err(NativePostgresError::Incomplete);
    }
    let mut bytes = serde_json::to_vec(record).map_err(|_| NativePostgresError::Incomplete)?;
    if bytes.is_empty() || bytes.len() >= MAX_NATIVE_WORKER_REQUEST_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    bytes.push(b'\n');
    let current_len = file
        .metadata()
        .map_err(|_| NativePostgresError::ProviderCommand)?
        .len();
    let append_len = u64::try_from(bytes.len()).map_err(|_| NativePostgresError::Incomplete)?;
    if current_len
        .checked_add(append_len)
        .is_none_or(|length| length > MAX_NATIVE_WORKER_REQUEST_BYTES_V1 as u64)
    {
        return Err(NativePostgresError::Incomplete);
    }
    file.seek(SeekFrom::End(0))
        .and_then(|_| file.write_all(&bytes))
        .and_then(|()| file.sync_all())
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    if !directory.still_names(name, identity)? {
        return Err(NativePostgresError::Incomplete);
    }
    #[cfg(unix)]
    directory.sync()?;
    directory.verify_named_path()?;
    Ok(())
}

fn process_deadline_before_cleanup(phase_end: Instant) -> Result<Instant, NativePostgresError> {
    let process_deadline = phase_end
        .checked_sub(PROVIDER_TERMINATION_GRACE_V1)
        .ok_or(NativePostgresError::Incomplete)?;
    if Instant::now() >= process_deadline {
        return Err(NativePostgresError::ProviderCommand);
    }
    Ok(process_deadline)
}

fn run_bounded_native_file_task<T: Send + 'static>(
    file: &fs::File,
    deadline: Instant,
    task: impl FnOnce(fs::File) -> Result<T, NativePostgresError> + Send + 'static,
) -> Result<T, NativePostgresError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(NativePostgresError::ProviderCommand)?;
    let retained = file
        .try_clone()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("worldstream-native-bounded-file".to_owned())
        .spawn(move || {
            let _ = sender.send(task(retained));
        })
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    match receiver.recv_timeout(remaining) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
            // The task owns no target/database/process/publication authority.
            // A stuck regular-file read/write may finish later, but no commit
            // helper is spawned after this deadline failure and the durable
            // recovery record remains actionable.
            Err(NativePostgresError::ProviderCommand)
        }
    }
}

fn write_native_commit_request_bounded(
    request: &NativePostgresCommitRequestV1,
    staging: &fs::File,
    deadline: Instant,
) -> Result<(), NativePostgresError> {
    request.validate()?;
    let bytes = serde_json::to_vec(request).map_err(|_| NativePostgresError::Incomplete)?;
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_COMMIT_REQUEST_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    run_bounded_native_file_task(staging, deadline, move |mut file| {
        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| file.write_all(&bytes))
            .and_then(|()| file.sync_all())
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)
    })
}

fn read_native_worker_checkpoint_bounded(
    staging: &fs::File,
    deadline: Instant,
) -> Result<NativePostgresVerifiedWorkerCheckpointV1, NativePostgresError> {
    run_bounded_native_file_task(staging, deadline, |mut file| {
        file.sync_all()
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let observed = read_native_postgres_worker_output(&mut file)?;
        if !observed.stream_complete {
            return Err(NativePostgresError::Incomplete);
        }
        verified_native_worker_checkpoint(observed.result.ok_or(NativePostgresError::Incomplete)?)
    })
}

fn run_native_postgres_commit_worker(
    worker_executable: &Path,
    request: &NativePostgresCommitRequestV1,
    report_staging: &fs::File,
    recovery: &fs::File,
    dump: &fs::File,
    deadline: Instant,
) -> Result<ProviderChildCompletionV1, NativePostgresError> {
    write_native_commit_request_bounded(request, report_staging, deadline)?;
    let request_stdin = report_staging
        .try_clone()
        .map(Stdio::from)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let recovery_stdout = recovery
        .try_clone()
        .map(Stdio::from)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let dump_stderr = dump
        .try_clone()
        .map(Stdio::from)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let mut command = Command::new(worker_executable);
    command
        .arg(NATIVE_POSTGRES_COMMIT_ARG_V1)
        .env_clear()
        .env(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1, "1")
        .stdin(request_stdin)
        .stdout(recovery_stdout)
        .stderr(dump_stderr);
    preserve_windows_runtime_environment(&mut command);
    let spawned = spawn_provider_group_with_mode(command, false, false)?;
    ProviderChildGuardV1::new(spawned).wait_until_with_drain_proof(deadline)
}

/// Executes the supervisor's read-only target admission probe. It never
/// changes the target and returns only the exact provider identity that the
/// parent will durably bind into the recovery record before spawning the
/// destructive worker.
///
/// # Errors
///
/// Returns a closed error unless the target is marked disposable, empty,
/// connection-limited to zero, and the retained passfile identity is exact.
pub fn execute_native_postgres_restore_admission_worker(
    reader: impl Read,
    mut writer: impl Write,
) -> Result<(), NativePostgresError> {
    if std::env::var_os(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1).as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err(NativePostgresError::Configuration(
            "native worker requires supervised containment",
        ));
    }
    let bytes = read_native_worker_bytes(reader, MAX_NATIVE_WORKER_REQUEST_BYTES_V1)?;
    let request: NativePostgresAdmissionRequestV1 =
        serde_json::from_slice(&bytes).map_err(|_| NativePostgresError::Incomplete)?;
    if serde_json::to_vec(&request).map_err(|_| NativePostgresError::Incomplete)? != bytes
        || request.schema != NATIVE_POSTGRES_ADMISSION_REQUEST_SCHEMA_V1
        || !request.operator_passfile.is_absolute()
        || !request.operator_passfile_identity.is_canonical()
    {
        return Err(NativePostgresError::Incomplete);
    }
    validate_native_postgres_endpoint(&request.target)?;
    let password = Zeroizing::new(password_for_reader(
        &request.target,
        open_worker_owned_file(
            &request.operator_passfile,
            &request.operator_passfile_identity,
        )?,
    )?);
    let mut client = connect(&request.target, &password)?;
    let version = server_version_num(&mut client)?;
    let observation = provider_identity(&mut client)?;
    if version != REQUIRED_POSTGRES_VERSION_NUM
        || observation.marker.as_deref() != Some(NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1)
        || observation.other_client_backends != 0
        || observation.connection_limit != 0
        || !observation.current_user_is_superuser
        || target_user_object_count(&mut client)? != 0
    {
        return Err(NativePostgresError::Incomplete);
    }
    let result = NativePostgresAdmissionResultV1 {
        schema: NATIVE_POSTGRES_ADMISSION_RESULT_SCHEMA_V1.to_owned(),
        status: "admitted".to_owned(),
        target: observation.identity,
        target_marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
        reason: "native_target_read_only_identity_admission_complete".to_owned(),
    };
    let result_bytes = serde_json::to_vec(&result).map_err(|_| NativePostgresError::Incomplete)?;
    writer
        .write_all(&result_bytes)
        .and_then(|()| writer.flush())
        .map_err(|_| NativePostgresError::ProviderCommand)
}

fn run_native_postgres_admission_worker(
    worker_executable: &Path,
    request: &NativePostgresAdmissionRequestV1,
    directory: &Arc<NativeRestoreDirectoryAuthorityV1>,
    process_deadline: Instant,
) -> Result<NativePostgresAdmissionResultV1, NativePostgresError> {
    if Instant::now() >= process_deadline {
        return Err(NativePostgresError::ProviderCommand);
    }
    let mut request_file = create_native_worker_transport_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_admission_request_{}.json",
            random_hex(16)?
        )),
    )?;
    let mut result_file = create_native_worker_transport_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_admission_result_{}.json",
            random_hex(16)?
        )),
    )?;
    let operation = (|| {
        let bytes = serde_json::to_vec(request).map_err(|_| NativePostgresError::Incomplete)?;
        if bytes.is_empty() || bytes.len() > MAX_NATIVE_WORKER_REQUEST_BYTES_V1 {
            return Err(NativePostgresError::Incomplete);
        }
        {
            let request_handle = request_file
                .file
                .as_mut()
                .ok_or(NativePostgresError::Incomplete)?;
            request_handle
                .write_all(&bytes)
                .and_then(|()| request_handle.sync_all())
                .and_then(|()| request_handle.seek(SeekFrom::Start(0)).map(|_| ()))
                .map_err(|_| NativePostgresError::ProviderCommand)?;
        }
        request_file.seal_anonymous_transport()?;
        let request_stdin = request_file
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let result_stdout = result_file
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let mut command = Command::new(worker_executable);
        command
            .arg(NATIVE_POSTGRES_ADMISSION_ARG_V1)
            .env_clear()
            .env(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1, "1")
            .stdin(request_stdin)
            .stdout(result_stdout)
            .stderr(Stdio::null());
        preserve_windows_runtime_environment(&mut command);
        let spawned = spawn_provider_group_with_mode(command, false, false)?;
        let mut worker = ProviderChildGuardV1::new(spawned);
        let status = worker.wait_until(process_deadline)?;
        if !status.success() {
            return Err(NativePostgresError::ProviderCommand);
        }
        result_file.seal_anonymous_transport()?;
        let result_handle = result_file
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?;
        result_handle
            .sync_all()
            .and_then(|()| result_handle.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let result_bytes =
            read_native_worker_bytes(&mut *result_handle, MAX_NATIVE_WORKER_REQUEST_BYTES_V1)?;
        let result: NativePostgresAdmissionResultV1 =
            serde_json::from_slice(&result_bytes).map_err(|_| NativePostgresError::Incomplete)?;
        if serde_json::to_vec(&result).map_err(|_| NativePostgresError::Incomplete)? != result_bytes
            || result.schema != NATIVE_POSTGRES_ADMISSION_RESULT_SCHEMA_V1
            || result.status != "admitted"
            || result.target_marker != NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1
            || result.reason != "native_target_read_only_identity_admission_complete"
            || result.target.database_name != request.target.database
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(result)
    })();
    let _ = request_file.remove();
    let _ = result_file.remove();
    operation
}

#[allow(clippy::too_many_lines)]
fn run_native_postgres_repair_worker(
    worker_executable: &Path,
    request: &NativePostgresRepairRequestV1,
    directory: &Arc<NativeRestoreDirectoryAuthorityV1>,
    deadline: Instant,
) -> Result<NativePostgresRepairWorkerCompletionV1, NativePostgresError> {
    if Instant::now() >= deadline {
        return Err(NativePostgresError::ProviderCommand);
    }
    let mut request_file = create_native_worker_transport_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_repair_request_{}.json",
            random_hex(16)?
        )),
    )?;
    let mut result_file = create_native_worker_transport_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_repair_result_{}.json",
            random_hex(16)?
        )),
    )?;
    let bytes = serde_json::to_vec(request).map_err(|_| NativePostgresError::Incomplete)?;
    if bytes.is_empty() || bytes.len() > MAX_NATIVE_WORKER_REQUEST_BYTES_V1 {
        return Err(NativePostgresError::Incomplete);
    }
    {
        let request_handle = request_file
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?;
        request_handle
            .write_all(&bytes)
            .and_then(|()| request_handle.sync_all())
            .and_then(|()| request_handle.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
    }
    request_file.seal_anonymous_transport()?;
    let request_stdin = request_file
        .file
        .as_ref()
        .ok_or(NativePostgresError::Incomplete)?
        .try_clone()
        .map(Stdio::from)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let result_stdout = result_file
        .file
        .as_ref()
        .ok_or(NativePostgresError::Incomplete)?
        .try_clone()
        .map(Stdio::from)
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let mut command = Command::new(worker_executable);
    command
        .arg(NATIVE_POSTGRES_REPAIR_ARG_V1)
        .env_clear()
        .env(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1, "1")
        .stdin(request_stdin)
        .stdout(result_stdout)
        .stderr(Stdio::null());
    preserve_windows_runtime_environment(&mut command);
    let spawned = spawn_provider_group_with_mode(command, false, false)?;
    let completion = ProviderChildGuardV1::new(spawned).wait_until_with_drain_proof(deadline);
    let status = match completion {
        Ok(ProviderChildCompletionV1::Completed(status)) => status,
        Ok(ProviderChildCompletionV1::TimedOutAfterDrain(_)) => {
            let _ = request_file.remove();
            let _ = result_file.remove();
            return Ok(NativePostgresRepairWorkerCompletionV1::Drained(Err(
                NativePostgresError::ProviderCommand,
            )));
        }
        Err(error) => {
            // The child may still hold and mutate both transports. Preserve
            // their exact handles/names and let the caller preserve every
            // higher-level authority instead of racing cleanup.
            request_file.removed = true;
            result_file.removed = true;
            return Ok(NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(error));
        }
    };
    let operation = (|| {
        if !status.success() {
            return Err(NativePostgresError::ProviderCommand);
        }
        result_file.seal_anonymous_transport()?;
        let result_handle = result_file
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?;
        result_handle
            .sync_all()
            .and_then(|()| result_handle.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let result_bytes =
            read_native_worker_bytes(&mut *result_handle, MAX_NATIVE_WORKER_REQUEST_BYTES_V1)?;
        let result: NativePostgresRepairResultV1 =
            serde_json::from_slice(&result_bytes).map_err(|_| NativePostgresError::Incomplete)?;
        if serde_json::to_vec(&result).map_err(|_| NativePostgresError::Incomplete)? != result_bytes
            || result.schema != NATIVE_POSTGRES_REPAIR_RESULT_SCHEMA_V1
            || result.status != "repaired"
            || result.reason != "native_target_identity_bound_sealed_and_role_absent"
        {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    })();
    let _ = request_file.remove();
    let _ = result_file.remove();
    Ok(NativePostgresRepairWorkerCompletionV1::Drained(operation))
}

fn finish_native_worker_supervision(
    operation: Result<NativePostgresRestoreOutcome, NativePostgresError>,
    request_cleanup: Result<(), NativePostgresError>,
    result_cleanup: Result<(), NativePostgresError>,
) -> Result<NativePostgresRestoreOutcome, NativePostgresError> {
    // These retained transport files contain only the canonical nonsecret
    // request and redacted report. Their empty-placeholder cleanup is best
    // effort and cannot retroactively invalidate a verified publication. In
    // particular, returning an error after worker success would misclassify a
    // nonempty final dump as failed/untrusted even though all security and
    // semantic acceptance checks already completed. Credential cleanup stays
    // mandatory inside the worker before dump publication.
    drop(request_cleanup);
    drop(result_cleanup);
    operation
}

/// Supervises the complete native restore in a contained same-executable
/// worker. The worker receives only canonical non-secret request bytes through
/// inherited stdin and writes its canonical redacted result through inherited
/// stdout. This call applies one absolute deadline, always terminates and
/// drains the outer Job/group after apparent worker exit, rereads both retained
/// protected handles, and never reconstructs the worker's trusted witness.
///
/// # Errors
///
/// Returns a closed error on timeout, containment/transport failure, a
/// noncanonical or incomplete report, or any provider failure.
pub fn run_native_postgres_restore(
    config: &NativePostgresRestoreConfig,
    report_path: &Path,
    timeout: Duration,
) -> Result<NativePostgresRestoreOutcome, NativePostgresError> {
    if !report_path.is_absolute()
        || report_path == config.dump_path
        || report_path == config.passfile
    {
        return Err(NativePostgresError::Configuration(
            "native report path must be absolute and distinct from dump and credential paths",
        ));
    }
    let worker_executable = std::env::current_exe().map_err(|_| {
        NativePostgresError::Configuration("native worker executable is unavailable")
    })?;
    run_native_postgres_restore_with_worker(
        config,
        report_path,
        &worker_executable,
        &[NATIVE_POSTGRES_WORKER_ARG_V1],
        timeout,
    )
}

/// Repairs an interrupted native restore from one exact durable recovery
/// journal using freshly supplied operator credentials. The target is repaired
/// and identity-bound before any dump/report/credential artifact is scrubbed.
/// A completed recovery is appended as a durable tombstone; malformed, torn,
/// substituted, or already-acknowledged journals fail closed.
///
/// # Errors
///
/// Returns an error unless the journal, target identity/marker, role cleanup,
/// process containment, and every recorded artifact disposition are proven.
#[allow(clippy::too_many_lines)]
pub fn recover_native_postgres_restore(
    target: &NativePostgresEndpointV1,
    passfile: &Path,
    recovery_path: &Path,
    recovery_record_identity: &NativePostgresArtifactDirectoryIdentityV1,
    artifact_directory_identity: &NativePostgresArtifactDirectoryIdentityV1,
    timeout: Duration,
) -> Result<(), NativePostgresError> {
    if timeout < MIN_NATIVE_SUPERVISOR_TIMEOUT_V1
        || !recovery_path.is_absolute()
        || cfg!(all(unix, not(target_os = "linux")))
    {
        return Err(NativePostgresError::Configuration(
            "native PostgreSQL recovery requires Linux or Windows and a bounded absolute journal",
        ));
    }
    validate_native_postgres_endpoint(target)?;
    recovery_record_identity.validate()?;
    artifact_directory_identity.validate()?;
    let directory_path = recovery_path
        .parent()
        .ok_or(NativePostgresError::Incomplete)?;
    let directory = NativeRestoreDirectoryAuthorityV1::open_expected(
        directory_path,
        artifact_directory_identity,
    )?;
    let mut recovery_file = open_expected_native_recovery_record_file_in(
        directory.clone(),
        recovery_path,
        recovery_record_identity,
    )?;
    let mut record = read_native_recovery_record(
        recovery_file
            .file
            .as_mut()
            .ok_or(NativePostgresError::Incomplete)?,
    )?;
    if record.artifact_directory_identity != directory.identity
        || record.target.database_name != target.database
        || matches!(
            record.disposition.as_str(),
            "publication_acknowledged" | "recovery_completed_artifacts_scrubbed"
        )
    {
        let _ = recovery_file.preserve();
        return Err(NativePostgresError::Configuration(
            "native recovery journal is not actionable for this target",
        ));
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(NativePostgresError::Incomplete)?;
    let worker_executable = std::env::current_exe().map_err(|_| {
        NativePostgresError::Configuration("native worker executable is unavailable")
    })?;
    let mut pinned_worker = pin_native_worker_executable_in(&directory, &worker_executable)?;
    let worker_invocation = retained_native_worker_invocation_path(&pinned_worker, true)?;
    let mut pinned_operator = pin_pgpassfile_in(&directory, passfile)?;
    let repair_completion = process_deadline_before_cleanup(deadline).and_then(|repair_deadline| {
        run_native_postgres_repair_worker(
            &worker_invocation,
            &NativePostgresRepairRequestV1 {
                schema: NATIVE_POSTGRES_REPAIR_REQUEST_SCHEMA_V1.to_owned(),
                target: target.clone(),
                operator_passfile: pinned_operator.path.clone(),
                operator_passfile_identity: pinned_operator.identity.worker_identity(),
                expected_target: record.target.clone(),
                target_marker: record.target_marker.clone(),
                role_name: record.role_name.clone(),
            },
            &directory,
            repair_deadline,
        )
    });
    let repair_result = match repair_completion {
        Ok(NativePostgresRepairWorkerCompletionV1::Drained(result)) => result,
        Ok(NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(error)) => {
            // The repair child may still be using the exact passfile and
            // pinned executable. Do not scrub, remove, or perform artifact
            // recovery concurrently with it. The journal remains actionable.
            pinned_operator.removed = true;
            pinned_worker.removed = true;
            let _ = recovery_file.preserve();
            return Err(error);
        }
        Err(error) => Err(error),
    };
    let operation = (|| {
        repair_result?;
        pinned_worker.verify()?;
        scrub_native_recovery_secret(
            &directory,
            &record.role_passfile_path,
            &record.role_passfile_identity,
            record.role_passfile_is_anonymous,
        )?;
        let dump_publication_must_exist = matches!(
            record.disposition.as_str(),
            "publication_committed_pending_report_ack"
                | "report_commit_in_progress"
                | "report_committed_pending_recovery_ack"
        );
        scrub_native_recovery_artifact(
            &directory,
            &record.private_dump_path,
            record.private_dump_is_anonymous,
            &record.publication_path,
            &record.private_dump_identity,
            dump_publication_must_exist,
        )?;
        if let (
            Some(private_report_path),
            Some(report_path),
            Some(report_identity),
            Some(_),
            Some(_),
        ) = (
            record.private_report_path.as_deref(),
            record.report_path.as_deref(),
            record.report_identity.as_ref(),
            record.report_digest.as_deref(),
            record.report_size_bytes,
        ) {
            scrub_native_recovery_artifact(
                &directory,
                private_report_path,
                cfg!(target_os = "linux"),
                report_path,
                report_identity,
                record.disposition == "report_committed_pending_recovery_ack",
            )?;
        }
        Ok(())
    })();
    let operator_cleanup = pinned_operator.remove();
    let worker_cleanup = pinned_worker.remove();
    let completed = operation
        .and(operator_cleanup)
        .and(worker_cleanup)
        .and_then(|()| {
            "recovery_completed_artifacts_scrubbed".clone_into(&mut record.disposition);
            "target_repaired_and_recorded_artifacts_scrubbed"
                .clone_into(&mut record.operator_action);
            write_durable_native_recovery_record(&record, &mut recovery_file)?;
            let observed = read_native_recovery_record(
                recovery_file
                    .file
                    .as_mut()
                    .ok_or(NativePostgresError::Incomplete)?,
            )?;
            if observed != record {
                return Err(NativePostgresError::Incomplete);
            }
            Ok(())
        });
    let preservation = recovery_file.preserve();
    completed.and(preservation)
}

#[allow(clippy::too_many_lines)]
fn run_native_postgres_restore_with_worker(
    config: &NativePostgresRestoreConfig,
    report_path: &Path,
    worker_executable: &Path,
    worker_args: &[&str],
    timeout: Duration,
) -> Result<NativePostgresRestoreOutcome, NativePostgresError> {
    validate_native_restore_configuration(config)?;
    if !report_path.is_absolute()
        || report_path == config.dump_path
        || report_path == config.passfile
        || report_path.parent() != config.dump_path.parent()
    {
        return Err(NativePostgresError::Configuration(
            "native report path must be absolute, distinct, and share the retained dump parent",
        ));
    }
    if cfg!(all(unix, not(target_os = "linux"))) && worker_args == [NATIVE_POSTGRES_WORKER_ARG_V1] {
        return Err(NativePostgresError::Configuration(
            "native PostgreSQL restore is supported only on Linux and Windows",
        ));
    }
    if timeout < MIN_NATIVE_SUPERVISOR_TIMEOUT_V1
        || worker_args.is_empty()
        || !worker_executable.is_file()
    {
        return Err(NativePostgresError::Configuration(
            "native worker executable or deadline is invalid",
        ));
    }
    let started_at = Instant::now();
    let deadline = started_at
        .checked_add(timeout)
        .ok_or(NativePostgresError::Incomplete)?;
    let tenth = timeout
        .checked_div(10)
        .ok_or(NativePostgresError::Incomplete)?;
    let admission_phase_end = started_at
        .checked_add(
            tenth
                .checked_mul(2)
                .ok_or(NativePostgresError::Incomplete)?,
        )
        .ok_or(NativePostgresError::Incomplete)?;
    let primary_phase_end = started_at
        .checked_add(
            tenth
                .checked_mul(6)
                .ok_or(NativePostgresError::Incomplete)?,
        )
        .ok_or(NativePostgresError::Incomplete)?;
    let repair_phase_end = started_at
        .checked_add(
            tenth
                .checked_mul(7)
                .ok_or(NativePostgresError::Incomplete)?,
        )
        .ok_or(NativePostgresError::Incomplete)?;
    let dump_commit_phase_end = started_at
        .checked_add(
            tenth
                .checked_mul(8)
                .ok_or(NativePostgresError::Incomplete)?,
        )
        .ok_or(NativePostgresError::Incomplete)?;
    let admission_deadline = process_deadline_before_cleanup(admission_phase_end)?;
    let worker_deadline = process_deadline_before_cleanup(primary_phase_end)?;
    let repair_deadline = process_deadline_before_cleanup(repair_phase_end)?;
    let dump_commit_deadline = process_deadline_before_cleanup(dump_commit_phase_end)?;
    let report_commit_deadline = process_deadline_before_cleanup(deadline)?;
    let directory_path = config
        .dump_path
        .parent()
        .ok_or(NativePostgresError::Configuration(
            "dump path has no parent directory",
        ))?;
    let directory = NativeRestoreDirectoryAuthorityV1::open_expected(
        directory_path,
        &config.artifact_directory_identity,
    )?;
    let production_worker = worker_args == [NATIVE_POSTGRES_WORKER_ARG_V1];
    let mut pinned_worker = pin_native_worker_executable_in(&directory, worker_executable)?;
    let worker_invocation =
        retained_native_worker_invocation_path(&pinned_worker, production_worker)?;
    let mut pinned_operator = pin_pgpassfile_in(&directory, &config.passfile)?;
    let admission = run_native_postgres_admission_worker(
        &worker_invocation,
        &NativePostgresAdmissionRequestV1 {
            schema: NATIVE_POSTGRES_ADMISSION_REQUEST_SCHEMA_V1.to_owned(),
            target: config.target.clone(),
            operator_passfile: pinned_operator.path.clone(),
            operator_passfile_identity: pinned_operator.identity.worker_identity(),
        },
        &directory,
        admission_deadline,
    )?;
    let role_name = format!("worldstream_restore_{}", random_hex(16)?);
    let password = Zeroizing::new(random_hex(32)?);
    let mut role_endpoint = config.target.clone();
    role_endpoint.username.clone_from(&role_name);
    let mut role_passfile = create_empty_ephemeral_pgpassfile_in(&directory, &role_endpoint)?;
    let private_dump_path = directory
        .path
        .join(format!(".worldstream_dump_{}.partial", random_hex(16)?));
    let private_dump_file = open_exclusive_dump_file_in(&directory, &private_dump_path)?;
    let private_dump_identity = ExclusiveFileIdentityV1::for_file(&private_dump_file)?;
    let mut private_dump = NativeDumpFileV1 {
        directory: Some(directory.clone()),
        file: Some(private_dump_file),
        path: private_dump_path.clone(),
        publication_path: config.dump_path.clone(),
        identity: private_dump_identity,
        digest: None,
        byte_count: None,
        retained: false,
    };
    let recovery = NativePostgresWorkerRecoveryPlanV1 {
        role_name,
        repair_passfile: pinned_operator.path.clone(),
        operator_passfile_identity: pinned_operator.identity.worker_identity(),
        operator_passfile_is_anonymous: pinned_operator.binding == PrivateFileBindingV1::Anonymous,
        expected_target: admission.target.clone(),
        role_passfile: role_passfile.path.clone(),
        role_passfile_identity: role_passfile.identity.worker_identity(),
        role_passfile_is_anonymous: role_passfile.binding == PrivateFileBindingV1::Anonymous,
        private_dump_path,
        private_dump_identity: private_dump.identity.worker_identity(),
        private_dump_is_anonymous: cfg!(target_os = "linux"),
    };
    recovery.validate()?;
    let mut worker_config = config.clone();
    worker_config.passfile.clone_from(&pinned_operator.path);
    let mut request_file = create_native_worker_transport_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_worker_request_{}.json",
            random_hex(16)?
        )),
    )?;
    let mut result_file = create_native_commit_staging_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_worker_result_{}.json",
            random_hex(16)?
        )),
    )?;
    let mut recovery_file = create_native_recovery_record_file_in(
        directory.clone(),
        directory.path.join(format!(
            ".worldstream_native_recovery_{}.json",
            random_hex(16)?
        )),
    )?;
    if !paths_are_pairwise_distinct(&[
        &recovery.repair_passfile,
        &recovery.role_passfile,
        &recovery.private_dump_path,
        &request_file.path,
        &result_file.path,
        &recovery_file.path,
        &pinned_worker.path,
        &config.dump_path,
    ]) {
        return Err(NativePostgresError::Incomplete);
    }
    let mut recovery_record = NativePostgresRecoveryRecordV1 {
        schema: NATIVE_POSTGRES_RECOVERY_RECORD_SCHEMA_V1.to_owned(),
        disposition: "repair_required_if_primary_interrupted".to_owned(),
        target: admission.target.clone(),
        target_marker: admission.target_marker.clone(),
        role_name: recovery.role_name.clone(),
        artifact_directory_identity: directory.identity.clone(),
        role_passfile_path: recovery.role_passfile.clone(),
        role_passfile_identity: recovery.role_passfile_identity.clone(),
        role_passfile_is_anonymous: recovery.role_passfile_is_anonymous,
        private_dump_path: recovery.private_dump_path.clone(),
        private_dump_identity: recovery.private_dump_identity.clone(),
        private_dump_is_anonymous: recovery.private_dump_is_anonymous,
        publication_path: config.dump_path.clone(),
        native_dump_digest: None,
        native_dump_size_bytes: None,
        private_report_path: None,
        report_path: None,
        report_identity: None,
        report_digest: None,
        report_size_bytes: None,
        operator_action:
            "keep_target_non_serving_and_run_identity_bound_native_repair_with_fresh_credentials"
                .to_owned(),
    };
    write_durable_native_recovery_record(&recovery_record, &mut recovery_file)?;
    recovery_file.preserve_on_drop()?;
    populate_ephemeral_pgpassfile(&mut role_passfile, &role_endpoint, password.as_str())?;
    drop(password);

    let mut primary_spawn_attempted = false;
    let mut containment_uncertain = false;
    let operation = (|| {
        let request = NativePostgresWorkerRequestV1::new(&worker_config, recovery.clone())?;
        let mut expected_request = Vec::new();
        write_native_postgres_worker_request(&request, &mut expected_request)?;
        {
            let request_handle = request_file
                .file
                .as_mut()
                .ok_or(NativePostgresError::Incomplete)?;
            request_handle
                .write_all(&expected_request)
                .and_then(|()| request_handle.sync_all())
                .and_then(|()| request_handle.seek(SeekFrom::Start(0)).map(|_| ()))
                .map_err(|_| NativePostgresError::ProviderCommand)?;
        }
        request_file.seal_anonymous_transport()?;
        let request_stdin = request_file
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let result_stdout = result_file
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let dump_stderr = private_dump
            .file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)?;

        pinned_worker.verify()?;
        let mut command = Command::new(&worker_invocation);
        command
            .args(worker_args)
            .env_clear()
            .env(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1, "1")
            .stdin(request_stdin)
            .stdout(result_stdout)
            .stderr(dump_stderr);
        preserve_windows_runtime_environment(&mut command);
        // Force a new outer containment even if a caller accidentally invokes
        // this API from a process carrying the worker marker.
        primary_spawn_attempted = true;
        let worker_completion = match spawn_provider_group_with_mode(command, false, false) {
            Ok(spawned) => {
                ProviderChildGuardV1::new(spawned).wait_until_with_drain_proof(worker_deadline)
            }
            Err(error) => Err(error),
        };
        let (worker_status, worker_timed_out) = match worker_completion {
            Ok(ProviderChildCompletionV1::Completed(status)) => (status, false),
            Ok(ProviderChildCompletionV1::TimedOutAfterDrain(status)) => (status, true),
            Err(error) => {
                // A kill/Job/wait/quiescence failure means the destructive
                // worker tree may still be running. Do not race it with target
                // repair, role retirement, credential scrubbing, dump
                // truncation, or transport cleanup. Preserve every named
                // recovery authority and return the containment failure.
                containment_uncertain = true;
                role_passfile.removed = true;
                pinned_operator.removed = true;
                pinned_worker.removed = true;
                request_file.removed = true;
                result_file.removed = true;
                private_dump.retained = true;
                let _ = recovery_file.preserve();
                return Err(error);
            }
        };
        let worker_identity_after_primary = pinned_worker.verify_retained_immutable();

        // Once spawn has been attempted, fail-closed target repair is the first
        // fallible operation after containment drain. Transport/checkpoint
        // errors are deliberately observed only after repair completes.
        let repair_result = match run_native_postgres_repair_worker(
            &worker_invocation,
            &NativePostgresRepairRequestV1 {
                schema: NATIVE_POSTGRES_REPAIR_REQUEST_SCHEMA_V1.to_owned(),
                target: worker_config.target.clone(),
                operator_passfile: recovery.repair_passfile.clone(),
                operator_passfile_identity: recovery.operator_passfile_identity.clone(),
                expected_target: recovery.expected_target.clone(),
                target_marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
                role_name: recovery.role_name.clone(),
            },
            &directory,
            repair_deadline,
        ) {
            Ok(NativePostgresRepairWorkerCompletionV1::Drained(result)) => result,
            Ok(NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(error)) => {
                containment_uncertain = true;
                role_passfile.removed = true;
                pinned_operator.removed = true;
                pinned_worker.removed = true;
                request_file.removed = true;
                result_file.removed = true;
                private_dump.retained = true;
                let _ = recovery_file.preserve();
                return Err(error);
            }
            Err(error) => Err(error),
        };
        let worker_identity_after_repair = pinned_worker.verify_retained_immutable();
        let role_passfile_cleanup = role_passfile.remove();
        let operator_passfile_cleanup = pinned_operator.remove();
        let mut first_security_error =
            worker_timed_out.then_some(NativePostgresError::ProviderCommand);
        for result in [
            repair_result,
            worker_identity_after_primary,
            worker_identity_after_repair,
            role_passfile_cleanup,
            operator_passfile_cleanup,
        ] {
            if let Err(error) = result
                && first_security_error.is_none()
            {
                first_security_error = Some(error);
            }
        }
        if let Some(error) = first_security_error {
            // Repair is only final once the primary containment is proven
            // drained, the exact worker remained pinned through repair, and
            // every credential handle is scrubbed. Any uncertainty retains
            // the durable record; preserve() disarms later cleanup even if its
            // own verification/sync reports an error.
            let _ = recovery_file.preserve();
            return Err(error);
        }
        request_file.verify()?;
        if !worker_status.success() {
            return Err(NativePostgresError::ProviderCommand);
        }
        let checkpoint = read_native_worker_checkpoint_bounded(
            result_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            dump_commit_deadline,
        )?;
        let mut report = checkpoint.report;
        if report.target_provider_identity != recovery.expected_target {
            return Err(NativePostgresError::Incomplete);
        }
        let expected_digest = DigestV1::parse(report.native_dump_digest.clone())
            .map_err(|_| NativePostgresError::Incomplete)?;
        let restored_global_digest = checkpoint.restored_global_digest;
        let dump_commit_request = NativePostgresCommitRequestV1 {
            schema: NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1.to_owned(),
            phase: NativePostgresCommitPhaseV1::Dump,
            artifact_directory_path: directory.path.clone(),
            artifact_directory_identity: directory.identity.clone(),
            recovery_path: recovery_file.path.clone(),
            recovery_identity: recovery_file.identity.worker_identity(),
            recovery_record: recovery_record.clone(),
            report_staging_path: result_file.path.clone(),
            report_staging_identity: result_file.identity.worker_identity(),
            report_path: report_path.to_path_buf(),
            report: report.clone(),
            restored_global_digest: restored_global_digest.as_str().to_owned(),
        };
        let dump_completion = run_native_postgres_commit_worker(
            &worker_invocation,
            &dump_commit_request,
            result_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            recovery_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            private_dump
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            dump_commit_deadline,
        );
        let dump_status = match dump_completion {
            Ok(ProviderChildCompletionV1::Completed(status)) => status,
            Ok(ProviderChildCompletionV1::TimedOutAfterDrain(_)) => {
                return Err(NativePostgresError::ProviderCommand);
            }
            Err(error) => {
                containment_uncertain = true;
                request_file.removed = true;
                result_file.removed = true;
                private_dump.retained = true;
                pinned_worker.removed = true;
                let _ = recovery_file.preserve();
                return Err(error);
            }
        };
        if !dump_status.success() {
            return Err(NativePostgresError::ProviderCommand);
        }
        pinned_worker.verify_retained_immutable()?;
        private_dump.digest = Some(expected_digest.clone());
        private_dump.byte_count = Some(report.native_dump_size_bytes);
        private_dump.path.clone_from(&config.dump_path);
        private_dump.retained = true;
        "publication_committed_pending_report_ack".clone_into(&mut recovery_record.disposition);
        recovery_record.native_dump_digest = Some(report.native_dump_digest.clone());
        recovery_record.native_dump_size_bytes = Some(report.native_dump_size_bytes);
        "persist_the_redacted_report_then_acknowledge_the_publication"
            .clone_into(&mut recovery_record.operator_action);

        // Only the parent transitions the verified worker checkpoint to Ready.
        // The first contained commit phase is now conclusively drained, target
        // repair and credential cleanup are proven, and the exact dump handle
        // remains retained by this process through the second phase.
        "ready".clone_into(&mut report.status);
        "postgres_native_restore_verified_by_unified_verifier".clone_into(&mut report.reason);
        "pass".clone_into(&mut report.native_dump_restore);
        report.native_witness_minted = true;
        if !native_worker_report_is_ready(&report) {
            return Err(NativePostgresError::Incomplete);
        }
        let report_commit_request = NativePostgresCommitRequestV1 {
            schema: NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1.to_owned(),
            phase: NativePostgresCommitPhaseV1::ReportAndAcknowledge,
            artifact_directory_path: directory.path.clone(),
            artifact_directory_identity: directory.identity.clone(),
            recovery_path: recovery_file.path.clone(),
            recovery_identity: recovery_file.identity.worker_identity(),
            recovery_record: recovery_record.clone(),
            report_staging_path: result_file.path.clone(),
            report_staging_identity: result_file.identity.worker_identity(),
            report_path: report_path.to_path_buf(),
            report: report.clone(),
            restored_global_digest: restored_global_digest.as_str().to_owned(),
        };
        let report_completion = run_native_postgres_commit_worker(
            &worker_invocation,
            &report_commit_request,
            result_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            recovery_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            private_dump
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            report_commit_deadline,
        );
        let report_status = match report_completion {
            Ok(ProviderChildCompletionV1::Completed(status)) => status,
            Ok(ProviderChildCompletionV1::TimedOutAfterDrain(_)) => {
                private_dump.retained = false;
                return Err(NativePostgresError::ProviderCommand);
            }
            Err(error) => {
                containment_uncertain = true;
                request_file.removed = true;
                result_file.removed = true;
                private_dump.retained = true;
                pinned_worker.removed = true;
                let _ = recovery_file.preserve();
                return Err(error);
            }
        };
        if !report_status.success() {
            private_dump.retained = false;
            return Err(NativePostgresError::ProviderCommand);
        }
        pinned_worker.verify_retained_immutable()?;
        result_file.removed = true;
        private_dump.retained = true;
        let report_size_bytes = serde_json::to_vec(&report)
            .map_err(|_| NativePostgresError::Incomplete)?
            .len()
            .checked_add(1)
            .and_then(|size| u64::try_from(size).ok())
            .ok_or(NativePostgresError::Incomplete)?;
        let native_dump_identity = retained_named_file_identity(
            &directory,
            &config.dump_path,
            private_dump
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            &private_dump.identity,
            Some(report.native_dump_size_bytes),
        )?;
        let report_identity = retained_named_file_identity(
            &directory,
            report_path,
            result_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            &result_file.identity,
            Some(report_size_bytes),
        )?;
        let recovery_record_identity = retained_named_file_identity(
            &directory,
            &recovery_file.path,
            recovery_file
                .file
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?,
            &recovery_file.identity,
            None,
        )?;
        let recovery_record_name = recovery_file
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .filter(|name| native_recovery_record_name_is_valid(name))
            .ok_or(NativePostgresError::Incomplete)?
            .to_owned();
        let witness = NativePostgresTrustedWitnessV1 {
            dump_digest: expected_digest,
            restored_global_digest,
        };
        Ok(NativePostgresRestoreOutcome {
            report,
            witness,
            native_dump_identity,
            report_identity,
            recovery_record_identity,
            recovery_record_name,
        })
    })();
    if !containment_uncertain && !role_passfile.removed {
        let _ = role_passfile.remove();
    }
    if !containment_uncertain && !pinned_operator.removed {
        let _ = pinned_operator.remove();
    }
    if !containment_uncertain && !pinned_worker.removed {
        let _ = pinned_worker.remove();
    }
    let request_cleanup = if containment_uncertain {
        Ok(())
    } else {
        request_file.remove()
    };
    let result_cleanup = if containment_uncertain {
        Ok(())
    } else {
        result_file.remove()
    };
    if recovery_file.file.is_some() {
        if primary_spawn_attempted {
            let _ = recovery_file.preserve();
        } else {
            let _ = recovery_file.scrub_preserved_before_spawn();
        }
    }
    finish_native_worker_supervision(operation, request_cleanup, result_cleanup)
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
fn run_native_postgres_restore_in_worker(
    config: &NativePostgresRestoreConfig,
    recovery: &NativePostgresWorkerRecoveryPlanV1,
    dump_writer: &mut impl Write,
) -> Result<NativePostgresVerifiedWorkerCheckpointV1, NativePostgresError> {
    // Public configuration fields permit direct construction, so repeat every
    // destructive-boundary check here immediately before any source capture or
    validate_native_restore_configuration(config)?;
    if config.dump_path.exists() || config.dump_path.is_symlink() {
        return Err(NativePostgresError::ProviderCommand);
    }
    drop(open_worker_owned_file(
        &config.passfile,
        &recovery.operator_passfile_identity,
    )?);
    run_native_postgres_restore_with_pinned(config, recovery, dump_writer)
}

#[allow(clippy::too_many_lines)]
fn run_native_postgres_restore_with_pinned(
    config: &NativePostgresRestoreConfig,
    recovery: &NativePostgresWorkerRecoveryPlanV1,
    dump_writer: &mut impl Write,
) -> Result<NativePostgresVerifiedWorkerCheckpointV1, NativePostgresError> {
    drop(open_worker_owned_file(
        &config.passfile,
        &recovery.operator_passfile_identity,
    )?);
    let mut isolation = verify_disposable_target_preflight(config, |witness| {
        if witness.target == recovery.expected_target {
            Ok(())
        } else {
            Err(NativePostgresError::Incomplete)
        }
    })?;
    let operation =
        (|| -> Result<NativePostgresVerifiedWorkerCheckpointV1, NativePostgresError> {
            let source_before = isolation.capture_source()?;
            let dump = native_dump_restore(config, &mut isolation, recovery, dump_writer)?;
            let dump_digest = dump
                .digest
                .as_ref()
                .ok_or(NativePostgresError::Incomplete)?
                .clone();
            let dump_byte_count = dump.byte_count.ok_or(NativePostgresError::Incomplete)?;
            let native_point_digest = postgres_native_point_digest(&dump_digest)?;
            let source_after_dump = isolation.capture_source()?;
            let restored_before_disposal = isolation.capture_target()?;
            let evidence =
                make_evidence(&source_after_dump, &restored_before_disposal, &dump_digest)?;
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
            let (snapshots_disposable, restored_snapshot_count_after) = isolation
                .dispose_target_snapshots(
                    &restored_before_disposal,
                    verifier.is_ready() && source_unchanged && exact_restored_row_set,
                )?;
            let target_isolated = verify_disposable_target_postflight(&mut isolation)?;
            let ready = verifier.is_ready()
                && source_unchanged
                && exact_restored_row_set
                && snapshots_disposable;
            let source_provider_identity = isolation.witness.source.clone();
            let target_provider_identity = isolation.witness.target.clone();
            Ok(NativePostgresVerifiedWorkerCheckpointV1 {
                report: NativePostgresRestoreReportV1 {
                    schema: NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1.to_owned(),
                    status: if ready {
                        "verified_pending_supervisor"
                    } else {
                        "incomplete"
                    }
                    .to_owned(),
                    reason: if ready {
                        "native_worker_verified_pending_containment_repair_and_publication"
                    } else if !verifier.is_ready() {
                        "unified_semantic_verifier_not_ready"
                    } else {
                        "native_restore_boundary_incomplete"
                    }
                    .to_owned(),
                    release_evidence: false,
                    native_dump_restore: if ready {
                        "verified_pending_publication"
                    } else {
                        "incomplete"
                    }
                    .to_owned(),
                    backup_id: postgres_native_backup_id(&dump_digest),
                    native_point_digest: native_point_digest.as_str().to_owned(),
                    native_dump_digest: dump_digest.as_str().to_owned(),
                    native_dump_size_bytes: dump_byte_count,
                    source_provider_identity,
                    target_provider_identity,
                    verifier,
                    source_unchanged,
                    exact_restored_row_set,
                    snapshots_disposable,
                    target_isolated,
                    target_published: false,
                    cleanup_required: true,
                    native_witness_minted: false,
                    secrets_emitted: false,
                    source_version_num: Some(source_before.version_num),
                    restored_version_num: Some(restored_before_disposal.version_num),
                    semantic_receipts_verified: source_before.receipts
                        == restored_before_disposal.receipts,
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
                            && source_after_dump.resource_blobs
                                == restored_before_disposal.resource_blobs
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
                restored_global_digest: restored_before_disposal.durable_digest,
            })
        })();
    // Verification never publishes a restored target. Explicitly and durably
    // close the admitted database after both successful and incomplete runs;
    // Drop remains only a last-resort retry for unwinding or a failed seal.
    let sealed = isolation.seal_fail_closed();
    match (operation, sealed) {
        (Ok(checkpoint), Ok(())) => Ok(checkpoint),
        (Err(error), Ok(())) | (_, Err(error)) => Err(error),
    }
}

fn native_dump_restore(
    config: &NativePostgresRestoreConfig,
    isolation: &mut NativePostgresTargetIsolationLeaseV1,
    recovery: &NativePostgresWorkerRecoveryPlanV1,
    dump_writer: &mut impl Write,
) -> Result<NativeDumpFileV1, NativePostgresError> {
    let maximum_dump_bytes =
        max_native_restore_canonical_row_bytes(VerifierLimits::default().max_object_bytes)
            .checked_mul(NATIVE_DUMP_BYTE_MULTIPLIER_V1)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(NativePostgresError::Incomplete)?;
    // The exported snapshot exists only on admitted source A and remains
    // valid while the keeper transaction is open. If DNS/routing changes to
    // source B, pg_dump authentication may succeed but snapshot import fails
    // before archive data can be accepted.
    let source_snapshot = isolation.export_source_snapshot()?;
    let dump_result = stream_native_dump_to_writer_with_timeout(
        config,
        maximum_dump_bytes,
        &source_snapshot,
        PROVIDER_TOOL_TIMEOUT_V1,
        dump_writer,
    );
    let source_snapshot_cleanup = isolation.release_source_snapshot();
    source_snapshot_cleanup?;
    let (dump_digest, dump_byte_count) = dump_result?;
    #[cfg(target_os = "linux")]
    let mut dump_file = {
        // The supervising parent installs its anonymous O_TMPFILE as this
        // worker's stderr. Reopen that exact inherited descriptor for reading;
        // no pathname can be substituted and the provider receives only a
        // clone of this same inode.
        let file = fs::OpenOptions::new()
            .read(true)
            .open("/proc/self/fd/2")
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let identity = ExclusiveFileIdentityV1::for_file(&file)?;
        if !identity.matches_worker_identity(&recovery.private_dump_identity) {
            return Err(NativePostgresError::Incomplete);
        }
        file
    };
    #[cfg(not(target_os = "linux"))]
    let mut dump_file =
        open_worker_owned_file(&recovery.private_dump_path, &recovery.private_dump_identity)?;
    let dump_length = dump_file
        .metadata()
        .map_err(|_| NativePostgresError::ProviderCommand)?
        .len();
    let reread_digest = hash_bounded_reader(&mut dump_file, dump_byte_count)?;
    if dump_length != dump_byte_count || reread_digest != dump_digest {
        return Err(NativePostgresError::Incomplete);
    }
    dump_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let dump_identity = ExclusiveFileIdentityV1::for_file(&dump_file)?;
    let mut dump = NativeDumpFileV1 {
        directory: None,
        file: Some(dump_file),
        path: recovery.private_dump_path.clone(),
        publication_path: config.dump_path.clone(),
        identity: dump_identity,
        digest: Some(dump_digest),
        byte_count: Some(dump_byte_count),
        // The supervising parent retains the authoritative write handle and
        // owns cleanup/publication. This worker's read handle must never scrub
        // that shared sink on return.
        retained: true,
    };
    // The keeper session has held the advisory lease and restrictive database
    // connection limit since preflight. The one-use login is created through
    // that keeper on admitted target A. A route change to B cannot authenticate
    // this child and therefore cannot reach the first `--clean` statement.
    let mut credential = isolation.create_restore_credential(config, recovery)?;
    let restore_result = isolation
        .verify_restore_credential(&credential)
        .and_then(|()| {
            run_provider_tool(
                &config.pg_restore,
                &credential.endpoint,
                &credential.passfile_path,
                [
                    "--exit-on-error",
                    "--single-transaction",
                    "--clean",
                    "--if-exists",
                    "--no-owner",
                    "--no-privileges",
                ],
                dump.restore_input()?,
            )
        });
    let credential_cleanup = isolation.retire_restore_credential(&mut credential);
    credential_cleanup?;
    restore_result?;
    Ok(dump)
}

enum ProviderProcessV1 {
    #[cfg(unix)]
    Group(GroupChild),
    Direct(std::process::Child),
    #[cfg(windows)]
    Job(Box<WindowsProviderJobV1>),
}

#[cfg(windows)]
struct WindowsProviderJobV1 {
    child: tokio::process::Child,
    group: ProcessGroup,
    runtime: tokio::runtime::Runtime,
}

struct SpawnedProviderGroupV1 {
    process: ProviderProcessV1,
    stdout: Option<Box<dyn Read + Send>>,
}

struct ProviderChildGuardV1 {
    process: Option<ProviderProcessV1>,
    stdout: Option<Box<dyn Read + Send>>,
}

fn spawn_provider_group(
    command: Command,
    capture_stdout: bool,
) -> Result<SpawnedProviderGroupV1, NativePostgresError> {
    let inherit_outer_containment = std::env::var_os(NATIVE_POSTGRES_WORKER_CONTAINED_ENV_V1)
        .as_deref()
        == Some(std::ffi::OsStr::new("1"));
    spawn_provider_group_with_mode(command, capture_stdout, inherit_outer_containment)
}

fn spawn_provider_group_with_mode(
    mut command: Command,
    capture_stdout: bool,
    inherit_outer_containment: bool,
) -> Result<SpawnedProviderGroupV1, NativePostgresError> {
    // Provider executable paths are trusted operator configuration. The group
    // / Job Object contains ordinary child helpers, but an intentionally
    // detached POSIX descendant can escape that boundary. Deadlines therefore
    // never synchronously join a stdout reader that such a process could keep
    // open; a configured provider that detaches remains a configuration/trust
    // violation rather than a way to block the restore caller indefinitely.
    let stdout = if capture_stdout {
        let (reader, writer) = os_pipe::pipe().map_err(|_| NativePostgresError::ProviderCommand)?;
        command.stdout(Stdio::from(writer));
        Some(Box::new(reader) as Box<dyn Read + Send>)
    } else {
        None
    };
    if inherit_outer_containment {
        let child = command
            .spawn()
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        return Ok(SpawnedProviderGroupV1 {
            process: ProviderProcessV1::Direct(child),
            stdout,
        });
    }
    #[cfg(unix)]
    let child = command
        .group_spawn()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    #[cfg(windows)]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    #[cfg(windows)]
    let group = ProcessGroup::new().map_err(|_| NativePostgresError::ProviderCommand)?;
    #[cfg(windows)]
    if group.mechanism() != Mechanism::JobObject {
        return Err(NativePostgresError::ProviderCommand);
    }
    #[cfg(windows)]
    let child = {
        let _runtime_guard = runtime.enter();
        group
            .spawn(tokio::process::Command::from(command))
            .map_err(|_| NativePostgresError::ProviderCommand)?
    };
    #[cfg(unix)]
    let process = ProviderProcessV1::Group(child);
    #[cfg(windows)]
    let process = ProviderProcessV1::Job(Box::new(WindowsProviderJobV1 {
        child,
        group,
        runtime,
    }));
    Ok(SpawnedProviderGroupV1 { process, stdout })
}

impl ProviderChildGuardV1 {
    fn new(spawned: SpawnedProviderGroupV1) -> Self {
        Self {
            process: Some(spawned.process),
            stdout: spawned.stdout,
        }
    }

    fn stdout(&mut self) -> Result<Box<dyn Read + Send>, NativePostgresError> {
        self.stdout
            .take()
            .ok_or(NativePostgresError::ProviderCommand)
    }

    fn wait_until(
        &mut self,
        deadline: Instant,
    ) -> Result<std::process::ExitStatus, NativePostgresError> {
        match self.wait_until_with_drain_proof(deadline)? {
            ProviderChildCompletionV1::Completed(status) => Ok(status),
            ProviderChildCompletionV1::TimedOutAfterDrain(_) => {
                Err(NativePostgresError::ProviderCommand)
            }
        }
    }

    /// Waits for the provider leader while preserving the distinction between
    /// an operation timeout whose containment was conclusively drained and a
    /// failure to prove containment. Callers that may perform destructive
    /// target repair must use this form: an `Err` means no repair or artifact
    /// cleanup is safe because a provider descendant may still be live.
    fn wait_until_with_drain_proof(
        &mut self,
        deadline: Instant,
    ) -> Result<ProviderChildCompletionV1, NativePostgresError> {
        loop {
            let process = self
                .process
                .as_mut()
                .ok_or(NativePostgresError::ProviderCommand)?;
            let observed_status = match process {
                #[cfg(unix)]
                ProviderProcessV1::Group(child) => {
                    unix_provider_leader_status_without_reaping(child)?
                }
                ProviderProcessV1::Direct(child) => child
                    .try_wait()
                    .map_err(|_| NativePostgresError::ProviderCommand)?,
                #[cfg(windows)]
                ProviderProcessV1::Job(job) => job
                    .child
                    .try_wait()
                    .map_err(|_| NativePostgresError::ProviderCommand)?,
            };
            if let Some(status) = observed_status {
                // Group leaders stay WNOWAIT-pinned until the one final group
                // signal; Job identity survives leader reaping. Direct worker
                // children are already inside the outer containment.
                self.terminate_and_wait_until(provider_cleanup_deadline(deadline), Some(status))?;
                #[cfg(test)]
                if FORCE_PROVIDER_CONTAINMENT_UNCERTAIN_AFTER_DRAIN_V1
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    return Err(NativePostgresError::ProviderCommand);
                }
                return Ok(ProviderChildCompletionV1::Completed(status));
            }
            if Instant::now() >= deadline {
                let status =
                    self.terminate_and_wait_until(provider_cleanup_deadline(deadline), None)?;
                #[cfg(test)]
                if FORCE_PROVIDER_CONTAINMENT_UNCERTAIN_AFTER_DRAIN_V1
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    return Err(NativePostgresError::ProviderCommand);
                }
                return Ok(ProviderChildCompletionV1::TimedOutAfterDrain(status));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate_and_wait_until(
        &mut self,
        deadline: Instant,
        observed_status: Option<std::process::ExitStatus>,
    ) -> Result<std::process::ExitStatus, NativePostgresError> {
        let Some(process) = self.process.take() else {
            return Err(NativePostgresError::ProviderCommand);
        };
        match process {
            #[cfg(unix)]
            ProviderProcessV1::Group(child) => {
                terminate_unix_provider_child(child, deadline, observed_status)
            }
            ProviderProcessV1::Direct(mut child) => {
                if observed_status.is_none()
                    && let Err(error) = child.kill()
                    && error.kind() != std::io::ErrorKind::InvalidInput
                {
                    return Err(NativePostgresError::ProviderCommand);
                }
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => return Ok(observed_status.unwrap_or(status)),
                        Ok(None) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Ok(None) | Err(_) => {
                            retain_direct_provider_reaper(child);
                            return Err(NativePostgresError::ProviderCommand);
                        }
                    }
                }
            }
            #[cfg(windows)]
            ProviderProcessV1::Job(mut job) => {
                job.group
                    .kill_all()
                    .map_err(|_| NativePostgresError::ProviderCommand)?;
                loop {
                    let active = job
                        .group
                        .stats()
                        .map_err(|_| NativePostgresError::ProviderCommand)?
                        .active_process_count;
                    if active == 0 {
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Err(NativePostgresError::ProviderCommand);
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                let status = job
                    .runtime
                    .block_on(job.child.wait())
                    .map_err(|_| NativePostgresError::ProviderCommand)?;
                drop(job.group);
                Ok(observed_status.unwrap_or(status))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ProviderChildCompletionV1 {
    Completed(std::process::ExitStatus),
    TimedOutAfterDrain(std::process::ExitStatus),
}

#[cfg(test)]
static FORCE_PROVIDER_CONTAINMENT_UNCERTAIN_AFTER_DRAIN_V1: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
fn terminate_unix_provider_child(
    mut child: GroupChild,
    deadline: Instant,
    mut observed_status: Option<std::process::ExitStatus>,
) -> Result<std::process::ExitStatus, NativePostgresError> {
    let raw_group = i32::try_from(child.id()).map_err(|_| NativePostgresError::ProviderCommand)?;
    let group =
        rustix::process::Pid::from_raw(raw_group).ok_or(NativePostgresError::ProviderCommand)?;
    // Provider executable paths are trusted operator configuration. Exact
    // PostgreSQL tools may create ordinary helpers in this cooperative group,
    // but a deliberately detached POSIX process requires an external sandbox.
    // Keep the leader WNOWAIT-unreaped so this one checked killpg cannot target
    // a recycled group. Reap next and never operate on the numeric PGID again.
    match rustix::process::kill_process_group(group, rustix::process::Signal::KILL) {
        Ok(()) => {}
        Err(rustix::io::Errno::SRCH) => {
            if observed_status.is_none() {
                observed_status = match unix_provider_leader_status_without_reaping(&mut child) {
                    Ok(status) => status,
                    Err(error) => {
                        retain_unix_provider_reaper(child);
                        return Err(error);
                    }
                };
            }
            if observed_status.is_none() {
                retain_unix_provider_reaper(child);
                return Err(NativePostgresError::ProviderCommand);
            }
        }
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::PERM) => {
            // Darwin can report EPERM after the provider closes its pipe and
            // becomes an unreaped zombie between the pre-signal observation
            // and killpg. Re-observe the still-pinned leader first. Darwin's
            // waitid WNOWAIT can lag this state transition, so a final
            // try_wait is permitted only after the one group operation has
            // already failed and no later numeric-PGID operation can occur.
            // EPERM with a live/unknown leader remains a containment failure.
            let Some(observation_deadline) = Instant::now().checked_add(Duration::from_millis(50))
            else {
                retain_unix_provider_reaper(child);
                return Err(NativePostgresError::ProviderCommand);
            };
            while observed_status.is_none() {
                observed_status = match unix_provider_leader_status_without_reaping(&mut child) {
                    Ok(status) => status,
                    Err(error) => {
                        retain_unix_provider_reaper(child);
                        return Err(error);
                    }
                };
                if observed_status.is_none() {
                    observed_status = match child.inner().try_wait() {
                        Ok(status) => status,
                        Err(_) => {
                            retain_unix_provider_reaper(child);
                            return Err(NativePostgresError::ProviderCommand);
                        }
                    };
                }
                if observed_status.is_some() || Instant::now() >= observation_deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            if observed_status.is_none() {
                retain_unix_provider_reaper(child);
                return Err(NativePostgresError::ProviderCommand);
            }
        }
        Err(_) => {
            retain_unix_provider_reaper(child);
            return Err(NativePostgresError::ProviderCommand);
        }
    }
    loop {
        if observed_status.is_none() {
            observed_status = match unix_provider_leader_status_without_reaping(&mut child) {
                Ok(status) => status,
                Err(error) => {
                    retain_unix_provider_reaper(child);
                    return Err(error);
                }
            };
        }
        let group_is_quiescent = if observed_status.is_some() {
            match unix_provider_group_has_no_other_members(raw_group) {
                Ok(quiescent) => quiescent,
                Err(error) => {
                    retain_unix_provider_reaper(child);
                    return Err(error);
                }
            }
        } else {
            false
        };
        if group_is_quiescent {
            match child.inner().try_wait() {
                Ok(Some(status)) => return Ok(observed_status.unwrap_or(status)),
                Ok(None) => {}
                Err(_) => {
                    retain_unix_provider_reaper(child);
                    return Err(NativePostgresError::ProviderCommand);
                }
            }
        }
        if Instant::now() >= deadline {
            retain_unix_provider_reaper(child);
            return Err(NativePostgresError::ProviderCommand);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn retain_direct_provider_reaper(mut child: std::process::Child) {
    if let Err(error) = thread::Builder::new()
        .name("worldstream-provider-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
        })
    {
        // The process has already received its final termination request. If
        // the host cannot allocate a reaper thread, closing the local handle is
        // still preferable to an unbounded wait on the deadline-sensitive
        // caller; the operation remains failed and its recovery record stays.
        drop(error);
    }
}

#[cfg(unix)]
fn retain_unix_provider_reaper(mut child: GroupChild) {
    if let Err(error) = thread::Builder::new()
        .name("worldstream-provider-group-reaper".to_owned())
        .spawn(move || {
            let _ = child.inner().wait();
        })
    {
        drop(error);
    }
}

#[cfg(target_os = "linux")]
fn unix_provider_group_has_no_other_members(group: i32) -> Result<bool, NativePostgresError> {
    let proc = fs::read_dir("/proc").map_err(|_| NativePostgresError::ProviderCommand)?;
    let mut inspected = 0_usize;
    for entry in proc {
        inspected = inspected.saturating_add(1);
        if inspected > 1_048_576 {
            return Err(NativePostgresError::ProviderCommand);
        }
        let entry = entry.map_err(|_| NativePostgresError::ProviderCommand)?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == group {
            continue;
        }
        let stat = match fs::read(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(NativePostgresError::ProviderCommand),
        };
        let stat = std::str::from_utf8(&stat).map_err(|_| NativePostgresError::ProviderCommand)?;
        let fields = stat
            .rfind(')')
            .and_then(|end| stat.get(end.saturating_add(1)..))
            .ok_or(NativePostgresError::ProviderCommand)?
            .split_whitespace()
            .collect::<Vec<_>>();
        let process_group = fields
            .get(2)
            .and_then(|value| value.parse::<i32>().ok())
            .ok_or(NativePostgresError::ProviderCommand)?;
        if process_group == group {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unix_provider_group_has_no_other_members(_group: i32) -> Result<bool, NativePostgresError> {
    // Product native restore fails closed before spawn on non-Linux Unix. This
    // branch exists only so portable provider-unit fixtures compile; Linux is
    // the shipped Unix path and proves exact group quiescence from `/proc`.
    Ok(true)
}

#[cfg(unix)]
fn unix_provider_leader_status_without_reaping(
    child: &mut GroupChild,
) -> Result<Option<std::process::ExitStatus>, NativePostgresError> {
    let pid = rustix::process::Pid::from_child(child.inner());
    let options = rustix::process::WaitIdOptions::EXITED
        | rustix::process::WaitIdOptions::NOHANG
        | rustix::process::WaitIdOptions::NOWAIT;
    rustix::process::waitid(rustix::process::WaitId::Pid(pid), options)
        .map(|status| status.and_then(unix_exit_status_from_waitid))
        .map_err(|_| NativePostgresError::ProviderCommand)
}

#[cfg(unix)]
fn unix_exit_status_from_waitid(
    status: rustix::process::WaitIdStatus,
) -> Option<std::process::ExitStatus> {
    if let Some(code) = status.exit_status() {
        return code.checked_shl(8).map(std::process::ExitStatus::from_raw);
    }
    status.terminating_signal().map(|signal| {
        let core_dumped = if status.dumped() { 0x80 } else { 0 };
        std::process::ExitStatus::from_raw(signal | core_dumped)
    })
}

fn provider_cleanup_deadline(deadline: Instant) -> Instant {
    let grace = Instant::now()
        .checked_add(PROVIDER_TERMINATION_GRACE_V1)
        .unwrap_or(deadline);
    deadline.max(grace)
}

impl Drop for ProviderChildGuardV1 {
    fn drop(&mut self) {
        let deadline = provider_cleanup_deadline(Instant::now());
        let _ = self.terminate_and_wait_until(deadline, None);
    }
}

struct NativeDumpFileV1 {
    directory: Option<Arc<NativeRestoreDirectoryAuthorityV1>>,
    file: Option<fs::File>,
    path: PathBuf,
    publication_path: PathBuf,
    identity: ExclusiveFileIdentityV1,
    digest: Option<DigestV1>,
    byte_count: Option<u64>,
    retained: bool,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsPublicationFaultV1 {
    BeforeRename,
    AfterRenameBeforeIdentity,
    AfterIdentityBeforeAcl,
    AfterAclBeforeSync,
    AfterSyncBeforeHash,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnixPublicationFaultV1 {
    ParentSync,
}

impl NativeDumpFileV1 {
    fn verify_retained_publication(
        &mut self,
        expected_digest: &DigestV1,
    ) -> Result<(), NativePostgresError> {
        let directory = self
            .directory
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?;
        let publication_name = directory.name_for(&self.publication_path)?;
        if !self.retained
            || self.path != self.publication_path
            || self.digest.as_ref() != Some(expected_digest)
            || !directory.still_names(&publication_name, &self.identity)?
        {
            return Err(NativePostgresError::Incomplete);
        }
        let expected_bytes = self.byte_count.ok_or(NativePostgresError::Incomplete)?;
        let file = self.file.as_mut().ok_or(NativePostgresError::Incomplete)?;
        let observed_bytes = file
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        file.seek(SeekFrom::Start(0))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let observed_digest = hash_bounded_reader(&mut *file, expected_bytes)?;
        if observed_bytes != expected_bytes || observed_digest != *expected_digest {
            return Err(NativePostgresError::Incomplete);
        }
        Ok(())
    }

    fn restore_input(&mut self) -> Result<Stdio, NativePostgresError> {
        #[cfg(not(target_os = "linux"))]
        if !self.identity.still_names_file(&self.path)? {
            return Err(NativePostgresError::Incomplete);
        }
        let file = self.file.as_mut().ok_or(NativePostgresError::Incomplete)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        file.try_clone()
            .map(Stdio::from)
            .map_err(|_| NativePostgresError::ProviderCommand)
    }

    fn retain(&mut self) -> Result<(), NativePostgresError> {
        if self.digest.is_none() || self.byte_count.is_none() {
            return Err(NativePostgresError::Incomplete);
        }
        let expected_digest = self
            .digest
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .clone();
        let expected_byte_count = self.byte_count.ok_or(NativePostgresError::Incomplete)?;
        #[cfg(not(target_os = "linux"))]
        if !self.identity.still_names_file(&self.path)? {
            return Err(NativePostgresError::Incomplete);
        }
        self.directory
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .verify_named_path()?;
        #[cfg(windows)]
        {
            self.retain_windows(&expected_digest, expected_byte_count)
        }
        #[cfg(target_os = "linux")]
        {
            self.retain_linux(&expected_digest, expected_byte_count)
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            self.retain_unix_with_fault(&expected_digest, expected_byte_count, None)
        }
    }

    #[cfg(target_os = "linux")]
    fn retain_linux(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
    ) -> Result<(), NativePostgresError> {
        self.retain_linux_with_fault(expected_digest, expected_byte_count, None)
    }

    #[cfg(target_os = "linux")]
    fn retain_linux_with_fault(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
        fault: Option<UnixPublicationFaultV1>,
    ) -> Result<(), NativePostgresError> {
        use std::os::fd::AsRawFd as _;

        let source = self.file.as_mut().ok_or(NativePostgresError::Incomplete)?;
        source
            .sync_all()
            .and_then(|()| source.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let source_len = source
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        if source_len != expected_byte_count
            || hash_bounded_reader(&mut *source, expected_byte_count)? != *expected_digest
        {
            return Err(NativePostgresError::Incomplete);
        }
        let directory = self
            .directory
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?;
        let file_name = directory.name_for(&self.publication_path)?;
        let published_path = directory.path_for(&file_name)?;
        let descriptor_path = format!("/proc/self/fd/{}", source.as_raw_fd());
        rustix::fs::linkat(
            rustix::fs::CWD,
            descriptor_path.as_str(),
            &directory.file,
            file_name.as_os_str(),
            rustix::fs::AtFlags::SYMLINK_FOLLOW,
        )
        .map_err(|_| NativePostgresError::ProviderCommand)?;
        self.path.clone_from(&published_path);
        self.publication_path = published_path;
        if !directory.still_names(&file_name, &self.identity)? {
            return Err(NativePostgresError::Incomplete);
        }
        if fault == Some(UnixPublicationFaultV1::ParentSync) || directory.sync().is_err() {
            return Err(NativePostgresError::ProviderCommand);
        }
        directory.verify_named_path()?;
        source
            .seek(SeekFrom::Start(0))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let published_len = source
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        let published_digest = hash_bounded_reader(&mut *source, expected_byte_count)?;
        if published_len != expected_byte_count || published_digest != *expected_digest {
            return Err(NativePostgresError::Incomplete);
        }
        self.retained = true;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn retain_unix_with_fault(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
        fault: Option<UnixPublicationFaultV1>,
    ) -> Result<(), NativePostgresError> {
        self.retain_linux_with_fault(expected_digest, expected_byte_count, fault)
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn retain_unix_with_fault(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
        fault: Option<UnixPublicationFaultV1>,
    ) -> Result<(), NativePostgresError> {
        self.file
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?
            .sync_all()
            .map_err(|_| NativePostgresError::ProviderCommand)?;

        // Supported Unix kernels provide RENAME_NOREPLACE through tempfile's
        // persist_noclobber implementation. Cleanup is disabled so even an
        // error path can never unlink a concurrently installed publication.
        let private_path = self.path.clone();
        let mut staging = tempfile::TempPath::try_from_path(private_path.clone())
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        staging.disable_cleanup(true);
        if staging.persist_noclobber(&self.publication_path).is_err() {
            let _ = scrub_private_file(&mut self.file, &self.identity, &self.path);
            return Err(NativePostgresError::ProviderCommand);
        }

        // tempfile has a hard-link fallback on unsupported Unix kernels. Do
        // not accept publication if that fallback left the private name: an
        // error scrubs both links through the still-open authoritative handle
        // and leaves only empty owner-only placeholders.
        if self.identity.still_names_file(&private_path)? {
            scrub_private_file(&mut self.file, &self.identity, &private_path)?;
            return Err(NativePostgresError::Incomplete);
        }
        if !self.identity.still_names_file(&self.publication_path)? {
            return Err(NativePostgresError::Incomplete);
        }
        let parent = self
            .publication_path
            .parent()
            .ok_or(NativePostgresError::Incomplete)?;
        let parent_sync = if fault == Some(UnixPublicationFaultV1::ParentSync) {
            Err(std::io::Error::other("injected parent sync failure"))
        } else {
            fs::File::open(parent).and_then(|directory| directory.sync_all())
        };
        if parent_sync.is_err() {
            let _ = scrub_private_file(&mut self.file, &self.identity, &self.publication_path);
            return Err(NativePostgresError::ProviderCommand);
        }
        self.path.clone_from(&self.publication_path);

        let published = self.file.as_mut().ok_or(NativePostgresError::Incomplete)?;
        let published_len = published
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        published
            .seek(SeekFrom::Start(0))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let published_digest = hash_bounded_reader(&mut *published, expected_byte_count)?;
        if published_len != expected_byte_count || published_digest != *expected_digest {
            published
                .set_len(0)
                .and_then(|()| published.sync_all())
                .map_err(|_| NativePostgresError::ProviderCommand)?;
            return Err(NativePostgresError::Incomplete);
        }
        self.retained = true;
        Ok(())
    }

    #[cfg(windows)]
    fn retain_windows(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
    ) -> Result<(), NativePostgresError> {
        self.retain_windows_with_fault(expected_digest, expected_byte_count, None)
    }

    #[cfg(windows)]
    fn retain_windows_with_fault(
        &mut self,
        expected_digest: &DigestV1,
        expected_byte_count: u64,
        fault: Option<WindowsPublicationFaultV1>,
    ) -> Result<(), NativePostgresError> {
        let source = self.file.as_mut().ok_or(NativePostgresError::Incomplete)?;
        source
            .sync_all()
            .and_then(|()| source.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let source_len = source
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        let source_digest = hash_bounded_reader(&mut *source, expected_byte_count)?;
        if source_len != expected_byte_count || source_digest != *expected_digest {
            return Err(NativePostgresError::Incomplete);
        }

        if fault == Some(WindowsPublicationFaultV1::BeforeRename) {
            return Err(NativePostgresError::ProviderCommand);
        }
        // The handle was created with DELETE access but shares reads only, so
        // the source name cannot be moved/replaced by another process. The
        // shim issues FileRenameInfo with ReplaceIfExists=FALSE against this
        // exact retained handle: destination collision is atomic and there is
        // no pathname source lookup or private alias after success.
        let directory = self
            .directory
            .as_ref()
            .ok_or(NativePostgresError::Incomplete)?;
        let publication_name = directory.name_for(&self.publication_path)?;
        let publication_path = directory.path_for(&publication_name)?;
        worldstream_windows_handle::rename_noreplace_at(source, &directory.file, &publication_name)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        self.path.clone_from(&publication_path);
        self.publication_path = publication_path;
        if fault == Some(WindowsPublicationFaultV1::AfterRenameBeforeIdentity) {
            return Err(NativePostgresError::ProviderCommand);
        }
        if !directory.still_names(&publication_name, &self.identity)? {
            return Err(NativePostgresError::Incomplete);
        }
        if fault == Some(WindowsPublicationFaultV1::AfterIdentityBeforeAcl) {
            return Err(NativePostgresError::ProviderCommand);
        }
        validate_owner_only_file(&self.publication_path)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if fault == Some(WindowsPublicationFaultV1::AfterAclBeforeSync) {
            return Err(NativePostgresError::ProviderCommand);
        }
        source
            .sync_all()
            .and_then(|()| source.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if fault == Some(WindowsPublicationFaultV1::AfterSyncBeforeHash) {
            return Err(NativePostgresError::ProviderCommand);
        }
        let published_len = source
            .metadata()
            .map_err(|_| NativePostgresError::ProviderCommand)?
            .len();
        let published_digest = hash_bounded_reader(&mut *source, expected_byte_count)?;
        if published_len != expected_byte_count || published_digest != *expected_digest {
            return Err(NativePostgresError::Incomplete);
        }
        directory.verify_named_path()?;
        self.retained = true;
        Ok(())
    }
}

impl Drop for NativeDumpFileV1 {
    fn drop(&mut self) {
        if !self.retained {
            let _ = scrub_private_file(&mut self.file, &self.identity, &self.path);
        }
    }
}

#[cfg(all(test, unix))]
fn stream_native_dump(
    config: &NativePostgresRestoreConfig,
    maximum_bytes: u64,
    source_snapshot: &str,
) -> Result<NativeDumpFileV1, NativePostgresError> {
    stream_native_dump_with_timeout(
        config,
        maximum_bytes,
        source_snapshot,
        PROVIDER_TOOL_TIMEOUT_V1,
    )
}

enum ProviderStdoutEventV1 {
    Chunk(Vec<u8>),
    Eof,
    Failed,
}

fn write_bounded_provider_stream(
    receiver: &mpsc::Receiver<ProviderStdoutEventV1>,
    sink: &mut impl Write,
    maximum_bytes: u64,
    deadline: Instant,
) -> Result<(blake3::Hasher, u64), NativePostgresError> {
    let mut hasher = blake3::Hasher::new();
    let mut observed = 0_u64;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(NativePostgresError::ProviderCommand);
        }
        let remaining = deadline.duration_since(now);
        let event = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected => {
                    NativePostgresError::ProviderCommand
                }
            })?;
        if Instant::now() >= deadline {
            return Err(NativePostgresError::ProviderCommand);
        }
        let chunk = match event {
            ProviderStdoutEventV1::Chunk(chunk) => chunk,
            ProviderStdoutEventV1::Eof => return Ok((hasher, observed)),
            ProviderStdoutEventV1::Failed => {
                return Err(NativePostgresError::ProviderCommand);
            }
        };
        let chunk_bytes =
            u64::try_from(chunk.len()).map_err(|_| NativePostgresError::Incomplete)?;
        observed = observed
            .checked_add(chunk_bytes)
            .ok_or(NativePostgresError::Incomplete)?;
        if observed > maximum_bytes {
            return Err(NativePostgresError::Incomplete);
        }
        sink.write_all(&chunk)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        hasher.update(&chunk);
    }
}

#[cfg(all(test, unix))]
fn stream_native_dump_with_timeout(
    config: &NativePostgresRestoreConfig,
    maximum_bytes: u64,
    source_snapshot: &str,
    timeout: Duration,
) -> Result<NativeDumpFileV1, NativePostgresError> {
    let directory = config
        .dump_path
        .parent()
        .ok_or(NativePostgresError::Configuration(
            "dump path has no parent directory",
        ))?;
    if config.dump_path.exists() || config.dump_path.is_symlink() {
        return Err(NativePostgresError::ProviderCommand);
    }
    let private_path = directory.join(format!(".worldstream_dump_{}.partial", random_hex(16)?));
    let file = open_exclusive_dump_file(&private_path)?;
    let identity = ExclusiveFileIdentityV1::for_file(&file)?;
    let mut dump = NativeDumpFileV1 {
        directory: Some(NativeRestoreDirectoryAuthorityV1::open(directory)?),
        file: Some(file),
        path: private_path,
        publication_path: config.dump_path.clone(),
        identity,
        digest: None,
        byte_count: None,
        retained: false,
    };
    let (digest, observed) = stream_native_dump_to_writer_with_timeout(
        config,
        maximum_bytes,
        source_snapshot,
        timeout,
        dump.file.as_mut().ok_or(NativePostgresError::Incomplete)?,
    )?;
    dump.file
        .as_ref()
        .ok_or(NativePostgresError::Incomplete)?
        .sync_all()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    dump.digest = Some(digest);
    dump.byte_count = Some(observed);
    Ok(dump)
}

fn stream_native_dump_to_writer_with_timeout(
    config: &NativePostgresRestoreConfig,
    maximum_bytes: u64,
    source_snapshot: &str,
    timeout: Duration,
    sink: &mut impl Write,
) -> Result<(DigestV1, u64), NativePostgresError> {
    if !source_snapshot_is_safe(source_snapshot) {
        return Err(NativePostgresError::Incomplete);
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(NativePostgresError::Incomplete)?;
    let mut command = provider_command(&config.pg_dump, &config.source, &config.passfile);
    command
        .args(["--format=custom", "--no-owner", "--no-privileges"])
        .arg(format!("--snapshot={source_snapshot}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = spawn_provider_group(command, true)?;
    let mut child = ProviderChildGuardV1::new(child);
    let mut stdout = child.stdout()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        loop {
            let mut chunk = vec![0_u8; 64 * 1024];
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    let _ = sender.send(ProviderStdoutEventV1::Eof);
                    return;
                }
                Ok(read) => {
                    chunk.truncate(read);
                    if sender.send(ProviderStdoutEventV1::Chunk(chunk)).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = sender.send(ProviderStdoutEventV1::Failed);
                    return;
                }
            }
        }
    });
    let (hasher, observed) =
        match write_bounded_provider_stream(&receiver, sink, maximum_bytes, deadline) {
            Ok(result) => result,
            Err(error) => {
                drop(receiver);
                child.terminate_and_wait_until(provider_cleanup_deadline(deadline), None)?;
                // The leader may have spawned a detached process that inherited
                // this pipe. It is outside the process-group trust boundary and
                // can keep the reader blocked after the deadline, so dropping the
                // JoinHandle is deliberate. The reader exits when that inherited
                // descriptor closes.
                drop(reader);
                return Err(error);
            }
        };
    drop(receiver);
    let reader_result = reader.join();
    let child_status = child.wait_until(deadline)?;
    if reader_result.is_err() || !child_status.success() {
        return Err(NativePostgresError::ProviderCommand);
    }
    sink.flush()
        .map_err(|_| NativePostgresError::ProviderCommand)?;
    let digest = DigestV1::parse(hasher.finalize().to_hex().to_string())
        .map_err(|_| NativePostgresError::Incomplete)?;
    Ok((digest, observed))
}

fn hash_bounded_reader<R: Read>(
    mut reader: R,
    maximum_bytes: u64,
) -> Result<DigestV1, NativePostgresError> {
    let mut hasher = blake3::Hasher::new();
    let mut observed = 0_u64;
    let mut chunk = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        if read == 0 {
            break;
        }
        observed = observed
            .checked_add(u64::try_from(read).map_err(|_| NativePostgresError::Incomplete)?)
            .ok_or(NativePostgresError::Incomplete)?;
        if observed > maximum_bytes {
            return Err(NativePostgresError::Incomplete);
        }
        hasher.update(&chunk[..read]);
    }
    DigestV1::parse(hasher.finalize().to_hex().to_string())
        .map_err(|_| NativePostgresError::Incomplete)
}

fn postgres_native_point(dump_digest: &DigestV1) -> BackendNativePointV1 {
    BackendNativePointV1::PostgresNative {
        major: 17,
        engine_identity: "postgresql-17.11".to_owned(),
        point_id: dump_digest.as_str().to_owned(),
        mechanism: worldstream_backup::PostgresNativeMechanismV1::Dump,
    }
}

fn postgres_native_backup_id(dump_digest: &DigestV1) -> String {
    format!("postgres-native-{}", dump_digest.as_str())
}

fn postgres_native_point_digest(dump_digest: &DigestV1) -> Result<DigestV1, NativePostgresError> {
    let bytes = serde_json::to_vec(&postgres_native_point(dump_digest))
        .map_err(|_| NativePostgresError::Incomplete)?;
    Ok(DigestV1::hash(&bytes))
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
    let native_point = postgres_native_point(dump_digest);
    let manifest = BackupManifestV1 {
        schema: worldstream_backup::BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
        backup_id: postgres_native_backup_id(dump_digest),
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
fn capture_with_client(client: &mut Client) -> Result<Capture, NativePostgresError> {
    let mut budget = ProviderReadBudgetV1::new(VerifierLimits::default())
        .map_err(|_| NativePostgresError::Incomplete)?;
    let mut client = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .map_err(|_| NativePostgresError::Database)?;
    crate::verify_runtime_schema_for_native_restore(&mut client, &mut budget)
        .map_err(|_| NativePostgresError::Incomplete)?;
    let version_num = client
        .query_one("SHOW server_version_num", &[])
        .map_err(|_| NativePostgresError::Database)?
        .get::<_, String>(0)
        .parse::<u32>()
        .map_err(|_| NativePostgresError::MalformedRow)?;
    let migration_contract = migration_contract(&mut client, &mut budget)?;
    let mut metadata_remaining = 1;
    bounded_global_provider_rows(
        &mut client,
        &mut budget,
        "SELECT jsonb_build_array(deployment_lineage_bytes, storage_epoch)::text FROM worldstream_deployment_metadata WHERE target_id = true ORDER BY target_id",
        &mut metadata_remaining,
        false,
    )?;
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
    let mut room_remaining = budget.maximum_rooms();
    bounded_global_provider_rows(
        &mut client,
        &mut budget,
        "SELECT jsonb_build_array(room_id, integrity_generation, integrity_status)::text FROM worldstream_room_roots ORDER BY room_id",
        &mut room_remaining,
        false,
    )?;
    let room_limit = i64::try_from(budget.maximum_rooms().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let room_ids = client
        .query(
            "SELECT room_id, integrity_generation, integrity_status \
             FROM worldstream_room_roots ORDER BY room_id LIMIT $1",
            &[&room_limit],
        )
        .map_err(|_| NativePostgresError::Database)?;
    if room_ids.is_empty() || room_ids.len() > budget.maximum_rooms() {
        return Err(NativePostgresError::Incomplete);
    }
    let registry = worldstream_core::builtin_worldstream_registry()
        .map_err(|_| NativePostgresError::Incomplete)?;
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
            let verification =
                crate::verify_room_for_native_restore(&mut client, &room_id, &mut budget)
                    .map_err(|_| NativePostgresError::Database)?;
            verification
                .verify_executable_replay(&registry)
                .map_err(|_| NativePostgresError::Incomplete)?;
            rooms.push(room_capture(&mut client, &mut budget, &verification)?);
        } else {
            rooms.push(isolated_room_capture(
                &mut client,
                &mut budget,
                &room_id,
                generation,
                &status,
            )?);
        }
    }
    let (packs, resources, resource_blobs, _identity_digest) =
        identities(&mut client, &mut budget)?;
    let receipts = semantic_receipts(&mut client, &mut budget)?;
    let activations = activation_intents(&mut client, &mut budget)?;
    let activation_receipts =
        activation_operation_receipts(&mut client, &mut budget, &activations)?;
    let durable_domains = durable_domains(&mut client, &mut budget)?;
    validate_global_authority_relations(&durable_domains)?;
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
    let capture = Capture {
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
    };
    client.commit().map_err(|_| NativePostgresError::Database)?;
    Ok(capture)
}

fn isolated_room_capture(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
    room_id: &str,
    generation: u64,
    status: &str,
) -> Result<RoomCapture, NativePostgresError> {
    let integrity_status = integrity_status(status)?;
    if integrity_status == IntegrityStatusV1::Healthy {
        return Err(NativePostgresError::MalformedRow);
    }
    let fingerprint = isolated_room_bytes_digest(client, budget, room_id)?;
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

fn bounded_canonical_projection_query(
    query: &str,
    row_limit_parameter: u8,
    byte_limit_parameter: u8,
) -> String {
    format!(
        "SELECT substring(convert_to(projected.canonical_row, 'UTF8') \
         FROM 1 FOR ${byte_limit_parameter}), \
         octet_length(convert_to(projected.canonical_row, 'UTF8'))::bigint \
         FROM ({query} LIMIT ${row_limit_parameter}) AS projected(canonical_row)"
    )
}

fn bounded_global_provider_rows(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
    query: &str,
    local_remaining: &mut usize,
    retain_rows: bool,
) -> Result<Vec<Vec<u8>>, NativePostgresError> {
    let (row_limit, byte_limit) = budget
        .query_parameters(*local_remaining)
        .map_err(|_| NativePostgresError::Incomplete)?;
    let bounded_query = bounded_canonical_projection_query(query, 1, 2);
    let parameters: [&(dyn postgres::types::ToSql + Sync); 2] = [&row_limit, &byte_limit];
    let mut rows = client
        .query_raw(&bounded_query, parameters)
        .map_err(|_| NativePostgresError::Database)?;
    let mut accepted = Vec::new();
    while let Some(row) = rows.next().map_err(|_| NativePostgresError::Database)? {
        if *local_remaining == 0 {
            return Err(NativePostgresError::Incomplete);
        }
        let prefix: Vec<u8> = row
            .try_get(0)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let encoded_length: i64 = row
            .try_get(1)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let canonical = budget
            .admit_projection(&prefix, encoded_length)
            .map_err(|_| NativePostgresError::Incomplete)?;
        if retain_rows {
            accepted.push(canonical);
        }
        *local_remaining -= 1;
    }
    Ok(accepted)
}

fn bounded_room_provider_rows(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
    room_id: &str,
    query: &str,
    local_remaining: &mut usize,
    retain_rows: bool,
) -> Result<Vec<Vec<u8>>, NativePostgresError> {
    let (row_limit, byte_limit) = budget
        .query_parameters(*local_remaining)
        .map_err(|_| NativePostgresError::Incomplete)?;
    let bounded_query = bounded_canonical_projection_query(query, 2, 3);
    let parameters: [&(dyn postgres::types::ToSql + Sync); 3] = [&room_id, &row_limit, &byte_limit];
    let mut rows = client
        .query_raw(&bounded_query, parameters)
        .map_err(|_| NativePostgresError::Database)?;
    let mut accepted = Vec::new();
    while let Some(row) = rows.next().map_err(|_| NativePostgresError::Database)? {
        if *local_remaining == 0 {
            return Err(NativePostgresError::Incomplete);
        }
        let prefix: Vec<u8> = row
            .try_get(0)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let encoded_length: i64 = row
            .try_get(1)
            .map_err(|_| NativePostgresError::MalformedRow)?;
        let canonical = budget
            .admit_projection(&prefix, encoded_length)
            .map_err(|_| NativePostgresError::Incomplete)?;
        if retain_rows {
            accepted.push(canonical);
        }
        *local_remaining -= 1;
    }
    Ok(accepted)
}

fn isolated_room_bytes_digest(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
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
            "room_snapshots",
            "SELECT jsonb_build_array(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes)::text FROM worldstream_room_snapshots WHERE room_id = $1 ORDER BY room_seq",
        ),
        (
            "members",
            "SELECT jsonb_build_array(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through, reset_generation)::text FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
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
            "SELECT jsonb_build_array(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash, retained_at)::text FROM worldstream_frames WHERE room_id = $1 ORDER BY member_id, frame_seq",
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
    let mut remaining_rows = budget.maximum_records_per_room();
    let mut room_hasher = blake3::Hasher::new();
    room_hasher.update(b"worldstream/isolated-room-raw/v2\0");
    for (domain, query) in QUERIES {
        let rows =
            bounded_room_provider_rows(client, budget, room_id, query, &mut remaining_rows, true)?;
        let mut domain_hasher = blake3::Hasher::new();
        domain_hasher.update(b"worldstream/isolated-room-domain-rows/v1\0");
        let mut domain_count = 0_u64;
        for canonical in rows {
            domain_hasher.update(&(canonical.len() as u64).to_be_bytes());
            domain_hasher.update(&canonical);
            domain_count = domain_count
                .checked_add(1)
                .ok_or(NativePostgresError::Incomplete)?;
        }
        room_hasher.update(&(domain.len() as u64).to_be_bytes());
        room_hasher.update(domain.as_bytes());
        room_hasher.update(&domain_count.to_be_bytes());
        room_hasher.update(domain_hasher.finalize().as_bytes());
    }
    DigestV1::parse(room_hasher.finalize().to_hex().to_string())
        .map_err(|_| NativePostgresError::Incomplete)
}

fn room_capture(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
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
    let (timers, frames) = operational_rows(client, budget, verification, &room.room_id)?;
    Ok(RoomCapture {
        room,
        timers,
        frames,
        snapshot_count: verification.snapshots.len(),
    })
}

#[allow(clippy::too_many_lines)]
fn operational_rows(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
    verification: &crate::PostgresRoomVerification,
    room_id: &str,
) -> Result<(Vec<TimerV1>, Vec<FrameV1>), NativePostgresError> {
    let mut local_remaining = budget.maximum_records_per_room();
    bounded_room_provider_rows(
        client,
        budget,
        room_id,
        "SELECT jsonb_build_array(receipt_bytes)::text FROM worldstream_semantic_receipts WHERE room_id = $1 AND operation_kind = 'timer_fired' ORDER BY identity_bytes",
        &mut local_remaining,
        false,
    )?;
    bounded_room_provider_rows(
        client,
        budget,
        room_id,
        "SELECT jsonb_build_array(member_id, frame_head, last_ack_frame_seq)::text FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
        &mut local_remaining,
        false,
    )?;
    let row_limit = i64::try_from(budget.maximum_records_per_room().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let fired_receipts = client
        .query(
            "SELECT receipt_bytes FROM worldstream_semantic_receipts \
             WHERE room_id = $1 AND operation_kind = 'timer_fired' ORDER BY identity_bytes LIMIT $2",
            &[&room_id, &row_limit],
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
            "SELECT member_id, frame_head, last_ack_frame_seq FROM worldstream_members WHERE room_id = $1 ORDER BY member_id LIMIT $2",
            &[&room_id, &row_limit],
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
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<
    (
        Vec<PackIdentityV1>,
        Vec<ResourceIdentityV1>,
        Vec<ResourceBlobV1>,
        DigestV1,
    ),
    NativePostgresError,
> {
    let mut pack_remaining = budget.maximum_packs();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(pack_id, revision, pack_digest)::text FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision",
        &mut pack_remaining,
        false,
    )?;
    let mut resource_identity_remaining = budget.maximum_resources();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(resource_kind, resource_identity, size_bytes, resource_digest)::text FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity",
        &mut resource_identity_remaining,
        false,
    )?;
    let mut identity_remaining = 1;
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(identity_digest, pack_set_digest, resource_set_digest, canonical_bytes)::text FROM worldstream_deployment_identity_metadata WHERE target_id = true ORDER BY target_id",
        &mut identity_remaining,
        false,
    )?;
    let mut blob_remaining = budget.maximum_resources();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(resource_kind, resource_identity, resource_bytes, resource_digest)::text FROM worldstream_deployment_resource_blobs ORDER BY resource_kind, resource_identity",
        &mut blob_remaining,
        false,
    )?;
    let mut pack_lock_remaining = budget.maximum_rooms();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(room_id, pack_revision_lock_bytes)::text FROM worldstream_genesis ORDER BY room_id",
        &mut pack_lock_remaining,
        false,
    )?;
    let pack_limit = i64::try_from(budget.maximum_packs().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let resource_limit = i64::try_from(budget.maximum_resources().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let room_limit = i64::try_from(budget.maximum_rooms().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let pack_rows = client
        .query("SELECT pack_id, revision, pack_digest FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision LIMIT $1", &[&pack_limit])
        .map_err(|_| NativePostgresError::Database)?;
    let resource_rows = client
        .query("SELECT resource_kind, resource_identity, size_bytes, resource_digest FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity LIMIT $1", &[&resource_limit])
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
            "SELECT resource_kind, resource_identity, resource_bytes, resource_digest FROM worldstream_deployment_resource_blobs ORDER BY resource_kind, resource_identity LIMIT $1",
            &[&resource_limit],
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
            "SELECT room_id, pack_revision_lock_bytes FROM worldstream_genesis ORDER BY room_id LIMIT $1",
            &[&room_limit],
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
            (lock.pack_id == pack.pack_id() && lock.explanatory_version == pack.revision())
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

const DURABLE_DOMAIN_QUERIES: [(NativeRestoreDurableDomainV1, &str); 34] = [
    (
        NativeRestoreDurableDomainV1::SchemaMigrations,
        "SELECT jsonb_build_array(version, migration_id, checksum, logical_history_id, schema_contract_fingerprint)::text FROM worldstream_schema_migrations ORDER BY version",
    ),
    (
        NativeRestoreDurableDomainV1::OperationGuards,
        "SELECT jsonb_build_array(identity_bytes, request_hash, room_id, receipt_bytes)::text FROM worldstream_operation_guards ORDER BY identity_bytes",
    ),
    (
        NativeRestoreDurableDomainV1::ExternalInputPreparations,
        "SELECT jsonb_build_array(identity_bytes, canonical_request_hash, recorded_at)::text FROM worldstream_external_input_preparations ORDER BY identity_bytes",
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
        "SELECT jsonb_build_array(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through, reset_generation)::text FROM worldstream_members ORDER BY room_id, member_id",
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
        "SELECT jsonb_build_array(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash, retained_at)::text FROM worldstream_frames ORDER BY room_id, member_id, frame_seq",
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
        "SELECT jsonb_build_array(fence_id, bundle_hash, target_fingerprint, state)::text FROM worldstream_transfer_target_fence ORDER BY fence_id",
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
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<
    BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
    NativePostgresError,
> {
    let mut remaining = budget.remaining_rows();
    let mut result = BTreeMap::new();
    for (domain, query) in DURABLE_DOMAIN_QUERIES {
        let canonical = bounded_global_provider_rows(client, budget, query, &mut remaining, true)?
            .into_iter()
            .map(NativeRestoreCanonicalRowV1::new)
            .collect();
        if result.insert(domain, canonical).is_some() {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if result.len() != POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1.len() {
        return Err(NativePostgresError::Incomplete);
    }
    Ok(result)
}

fn preflight_activation_intents(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<(), NativePostgresError> {
    let mut remaining = budget.remaining_rows();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(a.activation_id, a.room_id, a.cause_room_seq, a.target_member_id, a.state, a.intent_generation, a.lease_generation, a.runner_id, a.claim_id, a.lease_until, a.context_hash, a.context_bytes, a.context_retired)::text FROM worldstream_activation_intents a JOIN worldstream_room_roots r ON r.room_id = a.room_id WHERE r.integrity_status = 'healthy' ORDER BY a.activation_id",
        &mut remaining,
        false,
    )?;
    Ok(())
}

fn activation_intents(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<Vec<ActivationV1>, NativePostgresError> {
    let limits = VerifierLimits::default();
    preflight_activation_intents(client, budget)?;
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

fn preflight_activation_operation_receipts(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<(), NativePostgresError> {
    let mut remaining = budget.remaining_rows();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(a.room_id, a.operation_id, a.operation_kind, a.canonical_request_hash, a.activation_id, a.result_code, a.result_bytes, a.context_hash, a.context_bytes)::text FROM worldstream_activation_operation_receipts a JOIN worldstream_room_roots r ON r.room_id = a.room_id WHERE r.integrity_status = 'healthy' ORDER BY a.room_id, a.operation_id",
        &mut remaining,
        false,
    )?;
    Ok(())
}

fn activation_operation_receipts(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
    activations: &[ActivationV1],
) -> Result<Vec<ReceiptV1>, NativePostgresError> {
    let limits = VerifierLimits::default();
    preflight_activation_operation_receipts(client, budget)?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthorityCapabilityRelationV1 {
    principal_id: String,
    profile: String,
    target_room_id: Option<String>,
    runner_id: Option<String>,
    generation: i64,
    revoked_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthorityPrincipalRelationV1 {
    principal_kind: PrincipalKindV1,
    status: String,
    generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthorityRunnerRelationV1 {
    owner_principal_id: String,
    status: String,
    generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthorityMemberRelationV1 {
    principal_id: String,
    principal_kind: PrincipalKindV1,
    access_mode: AccessModeV1,
    has_role: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthorityLineageEventKindV1 {
    Bootstrap,
    PrincipalCreated,
    PrincipalStatusChanged,
    CapabilityRegistered,
    CapabilityNarrowed,
    CapabilityRevoked,
    RunnerRegistered,
    RunnerRevoked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthorityLineageEventV1 {
    audit_seq: i64,
    generation: i64,
    kind: AuthorityLineageEventKindV1,
    checked_at: String,
}

fn authority_lineage_is_contiguous(
    events: &mut [AuthorityLineageEventV1],
    current_generation: i64,
) -> bool {
    events.sort_unstable_by_key(|event| (event.generation, event.audit_seq));
    i64::try_from(events.len()) == Ok(current_generation)
        && events.iter().enumerate().all(|(index, event)| {
            i64::try_from(index + 1) == Ok(event.generation)
                && (index == 0 || events[index - 1].audit_seq < event.audit_seq)
        })
}

fn authority_scope_matches_profile(profile: &str, scope: &str) -> bool {
    match profile {
        "room_member" => matches!(
            scope,
            "room:attach"
                | "room:act"
                | "room:observe_public"
                | "room:observe_member"
                | "room:replay"
        ),
        "host_operator" => matches!(scope, "operator:room_admin" | "operator:backup"),
        "runner_control" => matches!(
            scope,
            "activation:offer_receive" | "activation:claim" | "activation:complete"
        ),
        _ => false,
    }
}

fn durable_domain_arrays(
    domains: &BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
    domain: NativeRestoreDurableDomainV1,
    expected_columns: usize,
) -> Result<Vec<Vec<serde_json::Value>>, NativePostgresError> {
    domains
        .get(&domain)
        .ok_or(NativePostgresError::Incomplete)?
        .iter()
        .map(|row| {
            let values = serde_json::from_slice::<Vec<serde_json::Value>>(&row.canonical_bytes)
                .map_err(|_| NativePostgresError::MalformedRow)?;
            if values.len() != expected_columns {
                return Err(NativePostgresError::MalformedRow);
            }
            Ok(values)
        })
        .collect()
}

fn authority_text(row: &[serde_json::Value], index: usize) -> Result<&str, NativePostgresError> {
    row.get(index)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(NativePostgresError::MalformedRow)
}

fn authority_optional_text(
    row: &[serde_json::Value],
    index: usize,
) -> Result<Option<&str>, NativePostgresError> {
    match row.get(index) {
        Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(Some)
            .ok_or(NativePostgresError::MalformedRow),
        None => Err(NativePostgresError::MalformedRow),
    }
}

fn authority_integer(row: &[serde_json::Value], index: usize) -> Result<i64, NativePostgresError> {
    row.get(index)
        .and_then(serde_json::Value::as_i64)
        .ok_or(NativePostgresError::MalformedRow)
}

fn authority_optional_integer(
    row: &[serde_json::Value],
    index: usize,
) -> Result<Option<i64>, NativePostgresError> {
    match row.get(index) {
        Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or(NativePostgresError::MalformedRow),
        None => Err(NativePostgresError::MalformedRow),
    }
}

fn authority_bytea_len(
    row: &[serde_json::Value],
    index: usize,
) -> Result<usize, NativePostgresError> {
    Ok(authority_bytea(row, index)?.len())
}

fn authority_bytea(
    row: &[serde_json::Value],
    index: usize,
) -> Result<Vec<u8>, NativePostgresError> {
    let encoded = authority_text(row, index)?;
    let hex = encoded
        .strip_prefix("\\x")
        .ok_or(NativePostgresError::MalformedRow)?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NativePostgresError::MalformedRow);
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .ok_or(NativePostgresError::MalformedRow)?;
            let low = (pair[1] as char)
                .to_digit(16)
                .ok_or(NativePostgresError::MalformedRow)?;
            u8::try_from((high << 4) | low).map_err(|_| NativePostgresError::MalformedRow)
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn validate_global_authority_relations(
    domains: &BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
) -> Result<(), NativePostgresError> {
    let transfer_fence = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::TransferTargetFence,
        4,
    )?;
    if transfer_fence.len() > 1 {
        return Err(NativePostgresError::Incomplete);
    }
    if let Some(row) = transfer_fence.first()
        && (row.first().and_then(serde_json::Value::as_bool) != Some(true)
            || authority_bytea_len(row, 1)? != 32
            || authority_bytea_len(row, 2)? != 32
            || !matches!(authority_text(row, 3)?, "importing" | "aborted"))
    {
        return Err(NativePostgresError::Incomplete);
    }

    let authority_state =
        durable_domain_arrays(domains, NativeRestoreDurableDomainV1::AuthorityState, 1)?;
    if authority_state.len() != 1
        || authority_state[0]
            .first()
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return Err(NativePostgresError::Incomplete);
    }

    let room_rows = durable_domain_arrays(domains, NativeRestoreDurableDomainV1::RoomRoots, 4)?;
    let mut rooms = BTreeSet::new();
    for row in &room_rows {
        if !rooms.insert(authority_text(row, 0)?.to_owned()) {
            return Err(NativePostgresError::Incomplete);
        }
    }
    let member_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::MemberDeliveryState,
        8,
    )?;
    let mut members = BTreeMap::new();
    for row in &member_rows {
        let room_id = authority_text(row, 0)?.to_owned();
        let member_id = authority_text(row, 1)?.to_owned();
        let membership = CanonicalJsonV1::decode_canonical::<worldstream_core::MembershipV1>(
            &authority_bytea(row, 2)?,
        )
        .map_err(|_| NativePostgresError::MalformedRow)?;
        if !rooms.contains(&room_id)
            || membership.member_id().as_str() != member_id
            || members
                .insert(
                    (room_id, member_id),
                    AuthorityMemberRelationV1 {
                        principal_id: membership.principal_id().to_string(),
                        principal_kind: membership.principal_kind(),
                        access_mode: membership.access_mode(),
                        has_role: membership.role().is_some(),
                    },
                )
                .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
    }

    let principal_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::AuthorityPrincipals,
        4,
    )?;
    let mut principals = BTreeMap::new();
    for row in &principal_rows {
        let principal_kind = match authority_text(row, 1)? {
            "human" => PrincipalKindV1::Human,
            "agent" => PrincipalKindV1::Agent,
            _ => return Err(NativePostgresError::Incomplete),
        };
        let status = authority_text(row, 2)?.to_owned();
        let generation = authority_integer(row, 3)?;
        if !matches!(status.as_str(), "enabled" | "disabled")
            || generation <= 0
            || principals
                .insert(
                    authority_text(row, 0)?.to_owned(),
                    AuthorityPrincipalRelationV1 {
                        principal_kind,
                        status,
                        generation,
                    },
                )
                .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
    }

    let runner_rows =
        durable_domain_arrays(domains, NativeRestoreDurableDomainV1::AuthorityRunners, 4)?;
    let mut runners = BTreeMap::new();
    for row in &runner_rows {
        let runner_id = authority_text(row, 0)?.to_owned();
        let owner = authority_text(row, 1)?.to_owned();
        let status = authority_text(row, 2)?.to_owned();
        let generation = authority_integer(row, 3)?;
        if !principals.contains_key(&owner)
            || !matches!(status.as_str(), "enabled" | "revoked")
            || generation <= 0
            || runners
                .insert(
                    runner_id,
                    AuthorityRunnerRelationV1 {
                        owner_principal_id: owner,
                        status,
                        generation,
                    },
                )
                .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
    }

    let capability_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::AuthorityCapabilities,
        10,
    )?;
    let mut capabilities = BTreeMap::new();
    for row in &capability_rows {
        let capability_id = authority_text(row, 0)?.to_owned();
        let principal_id = authority_text(row, 2)?.to_owned();
        let profile = authority_text(row, 3)?.to_owned();
        let target_room_id = authority_optional_text(row, 4)?.map(str::to_owned);
        let target_member_id = authority_optional_text(row, 5)?.map(str::to_owned);
        let runner_id = authority_optional_text(row, 6)?.map(str::to_owned);
        let generation = authority_integer(row, 7)?;
        let revoked_at = authority_optional_text(row, 9)?.map(str::to_owned);
        if authority_bytea_len(row, 1)? != 32
            || !principals.contains_key(&principal_id)
            || generation <= 0
        {
            return Err(NativePostgresError::Incomplete);
        }
        let target_valid = match profile.as_str() {
            "room_member" => target_room_id
                .as_ref()
                .zip(target_member_id.as_ref())
                .is_some_and(|(room_id, member_id)| {
                    let relation = members.get(&(room_id.clone(), member_id.clone()));
                    runner_id.is_none()
                        && rooms.contains(room_id)
                        && relation.is_some_and(|member| {
                            member.principal_id == principal_id
                                && principals
                                    .get(&principal_id)
                                    .map(|principal| principal.principal_kind)
                                    == Some(member.principal_kind)
                        })
                }),
            "host_operator" => {
                target_member_id.is_none()
                    && runner_id.is_none()
                    && target_room_id
                        .as_ref()
                        .is_none_or(|room_id| rooms.contains(room_id))
            }
            "runner_control" => {
                target_room_id.is_none()
                    && target_member_id.is_none()
                    && runner_id.as_ref().is_some_and(|runner_id| {
                        runners
                            .get(runner_id)
                            .is_some_and(|runner| runner.owner_principal_id == principal_id)
                    })
            }
            _ => false,
        };
        if !target_valid
            || capabilities
                .insert(
                    capability_id,
                    AuthorityCapabilityRelationV1 {
                        principal_id,
                        profile,
                        target_room_id,
                        runner_id,
                        generation,
                        revoked_at,
                    },
                )
                .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
        let _ = authority_optional_text(row, 8)?;
    }

    let scope_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::AuthorityCapabilityScopes,
        2,
    )?;
    let mut scopes = BTreeMap::<String, BTreeSet<String>>::new();
    for row in &scope_rows {
        let capability_id = authority_text(row, 0)?.to_owned();
        let scope = authority_text(row, 1)?.to_owned();
        let Some(capability) = capabilities.get(&capability_id) else {
            return Err(NativePostgresError::Incomplete);
        };
        if !authority_scope_matches_profile(&capability.profile, &scope)
            || !scopes.entry(capability_id).or_default().insert(scope)
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if capabilities
        .keys()
        .any(|capability_id| !scopes.contains_key(capability_id))
    {
        return Err(NativePostgresError::Incomplete);
    }

    let membership_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::AuthorityRunnerCapabilityMemberships,
        3,
    )?;
    let mut runner_memberships = BTreeSet::new();
    for row in &membership_rows {
        let capability_id = authority_text(row, 0)?.to_owned();
        let room_id = authority_text(row, 1)?.to_owned();
        let member_id = authority_text(row, 2)?.to_owned();
        let member = members.get(&(room_id.clone(), member_id.clone()));
        if capabilities
            .get(&capability_id)
            .map(|capability| capability.profile.as_str())
            != Some("runner_control")
            || !rooms.contains(&room_id)
            || !member.is_some_and(|member| {
                member.principal_kind == PrincipalKindV1::Agent
                    && member.access_mode == AccessModeV1::Participant
                    && member.has_role
            })
            || !runner_memberships.insert((capability_id, room_id, member_id))
        {
            return Err(NativePostgresError::Incomplete);
        }
    }

    let receipt_rows = durable_domain_arrays(
        domains,
        NativeRestoreDurableDomainV1::AuthorityChangeReceipts,
        9,
    )?;
    let audit_rows =
        durable_domain_arrays(domains, NativeRestoreDurableDomainV1::AuthorityAudit, 12)?;
    let mut receipts = BTreeMap::new();
    for row in &receipt_rows {
        if receipts
            .insert(authority_text(row, 0)?.to_owned(), row)
            .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    let mut audits = BTreeMap::new();
    let mut audit_sequences = BTreeSet::new();
    for row in &audit_rows {
        let audit_seq = authority_integer(row, 0)?;
        if audit_seq <= 0
            || !audit_sequences.insert(audit_seq)
            || audits
                .insert(authority_text(row, 1)?.to_owned(), row)
                .is_some()
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if receipts.keys().ne(audits.keys()) {
        return Err(NativePostgresError::Incomplete);
    }

    let mut principal_lineages = BTreeMap::<String, Vec<AuthorityLineageEventV1>>::new();
    let mut runner_lineages = BTreeMap::<String, Vec<AuthorityLineageEventV1>>::new();
    let mut capability_lineages = BTreeMap::<String, Vec<AuthorityLineageEventV1>>::new();
    let mut actor_uses = Vec::<(String, i64)>::new();
    let mut bootstrap_count = 0_usize;
    let mut bootstrap_audit_seq = None;
    for (change_id, receipt) in receipts {
        let audit = audits
            .get(&change_id)
            .ok_or(NativePostgresError::Incomplete)?;
        let audit_seq = authority_integer(audit, 0)?;
        let receipt_actor = authority_optional_text(receipt, 1)?;
        let result_kind = authority_text(receipt, 3)?;
        let target_kind = authority_text(receipt, 4)?;
        let target_id = authority_text(receipt, 5)?;
        let secondary_target = authority_optional_text(receipt, 6)?;
        let resulting_generation = authority_integer(receipt, 7)?;
        let checked_at = authority_text(receipt, 8)?;
        let audit_actor = authority_optional_text(audit, 2)?;
        let audit_prior_generation = authority_optional_integer(audit, 7)?;
        let audit_reason = authority_optional_text(audit, 10)?;
        if authority_bytea_len(receipt, 2)? != 32
            || authority_bytea_len(audit, 11)? != 32
            || receipt_actor != audit_actor
            || authority_text(receipt, 2)? != authority_text(audit, 11)?
            || target_kind != authority_text(audit, 3)?
            || target_id != authority_text(audit, 4)?
            || secondary_target != authority_optional_text(audit, 5)?
            || resulting_generation != authority_integer(audit, 8)?
            || checked_at != authority_text(audit, 9)?
            || resulting_generation <= 0
        {
            return Err(NativePostgresError::Incomplete);
        }
        let target_exists = match target_kind {
            "bootstrap" => secondary_target.is_some_and(|capability_id| {
                principals.contains_key(target_id)
                    && capabilities
                        .get(capability_id)
                        .is_some_and(|capability| capability.principal_id == target_id)
            }),
            "principal" => secondary_target.is_none() && principals.contains_key(target_id),
            "capability" => secondary_target.is_none() && capabilities.contains_key(target_id),
            "runner" => secondary_target.is_none() && runners.contains_key(target_id),
            _ => false,
        };
        let (
            expected_target_kind,
            expected_change_kind,
            expected_prior,
            reason_required,
            event_kind,
        ) = match result_kind {
            "authority_bootstrapped" => (
                "bootstrap",
                "bootstrap_authority",
                None,
                false,
                AuthorityLineageEventKindV1::Bootstrap,
            ),
            "principal_created" => (
                "principal",
                "create_principal",
                None,
                false,
                AuthorityLineageEventKindV1::PrincipalCreated,
            ),
            "capability_registered" => (
                "capability",
                "register_capability",
                None,
                false,
                AuthorityLineageEventKindV1::CapabilityRegistered,
            ),
            "runner_registered" => (
                "runner",
                "register_runner",
                None,
                false,
                AuthorityLineageEventKindV1::RunnerRegistered,
            ),
            "capability_narrowed" => (
                "capability",
                "narrow_capability",
                resulting_generation.checked_sub(1),
                true,
                AuthorityLineageEventKindV1::CapabilityNarrowed,
            ),
            "capability_revoked" => (
                "capability",
                "revoke_capability",
                resulting_generation.checked_sub(1),
                true,
                AuthorityLineageEventKindV1::CapabilityRevoked,
            ),
            "principal_status_changed" => (
                "principal",
                "set_principal_status",
                resulting_generation.checked_sub(1),
                true,
                AuthorityLineageEventKindV1::PrincipalStatusChanged,
            ),
            "runner_revoked" => (
                "runner",
                "revoke_runner",
                resulting_generation.checked_sub(1),
                true,
                AuthorityLineageEventKindV1::RunnerRevoked,
            ),
            _ => return Err(NativePostgresError::Incomplete),
        };
        let bootstrap = result_kind == "authority_bootstrapped";
        if !target_exists
            || target_kind != expected_target_kind
            || authority_text(audit, 6)? != expected_change_kind
            || audit_prior_generation != expected_prior
            || audit_reason.is_some() != reason_required
            || (expected_prior.is_none() && resulting_generation != 1)
            || if bootstrap {
                receipt_actor.is_some() || secondary_target.is_none()
            } else {
                receipt_actor.is_none_or(|actor| !principals.contains_key(actor))
                    || secondary_target.is_some()
            }
        {
            return Err(NativePostgresError::Incomplete);
        }
        if let Some(actor) = receipt_actor {
            actor_uses.push((actor.to_owned(), audit_seq));
        }
        let event = AuthorityLineageEventV1 {
            audit_seq,
            generation: resulting_generation,
            kind: event_kind,
            checked_at: checked_at.to_owned(),
        };
        match event_kind {
            AuthorityLineageEventKindV1::Bootstrap => {
                let capability_id = secondary_target.ok_or(NativePostgresError::Incomplete)?;
                bootstrap_count = bootstrap_count.saturating_add(1);
                bootstrap_audit_seq = Some(audit_seq);
                principal_lineages
                    .entry(target_id.to_owned())
                    .or_default()
                    .push(event.clone());
                capability_lineages
                    .entry(capability_id.to_owned())
                    .or_default()
                    .push(event);
            }
            AuthorityLineageEventKindV1::PrincipalCreated
            | AuthorityLineageEventKindV1::PrincipalStatusChanged => principal_lineages
                .entry(target_id.to_owned())
                .or_default()
                .push(event),
            AuthorityLineageEventKindV1::CapabilityRegistered
            | AuthorityLineageEventKindV1::CapabilityNarrowed
            | AuthorityLineageEventKindV1::CapabilityRevoked => capability_lineages
                .entry(target_id.to_owned())
                .or_default()
                .push(event),
            AuthorityLineageEventKindV1::RunnerRegistered
            | AuthorityLineageEventKindV1::RunnerRevoked => runner_lineages
                .entry(target_id.to_owned())
                .or_default()
                .push(event),
        }
    }

    let authority_occupied =
        !principals.is_empty() || !runners.is_empty() || !capabilities.is_empty();
    if (authority_occupied && bootstrap_count != 1)
        || (!authority_occupied
            && (bootstrap_count != 0
                || !principal_lineages.is_empty()
                || !runner_lineages.is_empty()
                || !capability_lineages.is_empty()))
        || bootstrap_audit_seq
            .zip(audit_sequences.first().copied())
            .is_some_and(|(bootstrap, first)| bootstrap != first)
    {
        return Err(NativePostgresError::Incomplete);
    }

    for (principal_id, principal) in &principals {
        let events = principal_lineages
            .get_mut(principal_id)
            .ok_or(NativePostgresError::Incomplete)?;
        if !authority_lineage_is_contiguous(events, principal.generation)
            || !matches!(
                events.first().map(|event| event.kind),
                Some(
                    AuthorityLineageEventKindV1::Bootstrap
                        | AuthorityLineageEventKindV1::PrincipalCreated
                )
            )
            || events
                .iter()
                .skip(1)
                .any(|event| event.kind != AuthorityLineageEventKindV1::PrincipalStatusChanged)
        {
            return Err(NativePostgresError::Incomplete);
        }
        let expected_status = if (principal.generation - 1) % 2 == 0 {
            "enabled"
        } else {
            "disabled"
        };
        if principal.status != expected_status {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if principal_lineages.len() != principals.len() {
        return Err(NativePostgresError::Incomplete);
    }

    for (runner_id, runner) in &runners {
        let events = runner_lineages
            .get_mut(runner_id)
            .ok_or(NativePostgresError::Incomplete)?;
        if !authority_lineage_is_contiguous(events, runner.generation)
            || events.first().map(|event| event.kind)
                != Some(AuthorityLineageEventKindV1::RunnerRegistered)
            || events.len() > 2
            || events
                .get(1)
                .is_some_and(|event| event.kind != AuthorityLineageEventKindV1::RunnerRevoked)
            || (runner.status == "revoked")
                != (events.last().map(|event| event.kind)
                    == Some(AuthorityLineageEventKindV1::RunnerRevoked))
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if runner_lineages.len() != runners.len() {
        return Err(NativePostgresError::Incomplete);
    }

    for (capability_id, capability) in &capabilities {
        let events = capability_lineages
            .get_mut(capability_id)
            .ok_or(NativePostgresError::Incomplete)?;
        if !authority_lineage_is_contiguous(events, capability.generation)
            || !matches!(
                events.first().map(|event| event.kind),
                Some(
                    AuthorityLineageEventKindV1::Bootstrap
                        | AuthorityLineageEventKindV1::CapabilityRegistered
                )
            )
            || events.iter().enumerate().skip(1).any(|(index, event)| {
                event.kind != AuthorityLineageEventKindV1::CapabilityNarrowed
                    && !(event.kind == AuthorityLineageEventKindV1::CapabilityRevoked
                        && index + 1 == events.len())
            })
            || capability.revoked_at.is_some()
                != (events.last().map(|event| event.kind)
                    == Some(AuthorityLineageEventKindV1::CapabilityRevoked))
            || capability.revoked_at.as_deref()
                != events
                    .last()
                    .filter(|event| event.kind == AuthorityLineageEventKindV1::CapabilityRevoked)
                    .map(|event| event.checked_at.as_str())
        {
            return Err(NativePostgresError::Incomplete);
        }
        if events.first().map(|event| event.kind) == Some(AuthorityLineageEventKindV1::Bootstrap)
            && (capability.profile != "host_operator"
                || capability.target_room_id.is_some()
                || capability.runner_id.is_some()
                || capability.revoked_at.is_some()
                || scopes.get(capability_id).is_none_or(|capability_scopes| {
                    capability_scopes.len() != 1
                        || !capability_scopes.contains("operator:room_admin")
                }))
        {
            return Err(NativePostgresError::Incomplete);
        }
        if capability.profile == "runner_control"
            && !runner_memberships
                .iter()
                .any(|(membership_capability_id, _, _)| membership_capability_id == capability_id)
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    if capability_lineages.len() != capabilities.len() {
        return Err(NativePostgresError::Incomplete);
    }

    for (actor, audit_seq) in actor_uses {
        if principal_lineages
            .get(&actor)
            .and_then(|events| events.first())
            .is_none_or(|created| created.audit_seq >= audit_seq)
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    for (runner_id, runner) in &runners {
        let registered = runner_lineages
            .get(runner_id)
            .and_then(|events| events.first())
            .ok_or(NativePostgresError::Incomplete)?;
        if principal_lineages
            .get(&runner.owner_principal_id)
            .and_then(|events| events.first())
            .is_none_or(|created| created.audit_seq >= registered.audit_seq)
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    for (capability_id, capability) in &capabilities {
        let registered = capability_lineages
            .get(capability_id)
            .and_then(|events| events.first())
            .ok_or(NativePostgresError::Incomplete)?;
        let principal_created = principal_lineages
            .get(&capability.principal_id)
            .and_then(|events| events.first())
            .ok_or(NativePostgresError::Incomplete)?;
        if (registered.kind == AuthorityLineageEventKindV1::Bootstrap
            && principal_created.audit_seq != registered.audit_seq)
            || (registered.kind != AuthorityLineageEventKindV1::Bootstrap
                && principal_created.audit_seq >= registered.audit_seq)
            || capability.runner_id.as_ref().is_some_and(|runner_id| {
                runner_lineages
                    .get(runner_id)
                    .and_then(|events| events.first())
                    .is_none_or(|runner_registered| {
                        runner_registered.audit_seq >= registered.audit_seq
                    })
            })
        {
            return Err(NativePostgresError::Incomplete);
        }
    }
    Ok(())
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

fn semantic_receipts(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<Vec<ReceiptV1>, NativePostgresError> {
    let mut remaining = budget.remaining_rows();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(s.identity_bytes, s.operation_kind, s.canonical_request_hash, s.basis_complete_head_bytes, s.semantic_input_bytes, s.semantic_time_bytes, s.resolution_kind, s.transition_seq, s.receipt_bytes, s.room_id)::text FROM worldstream_semantic_receipts s JOIN worldstream_room_roots r ON r.room_id = s.room_id WHERE r.integrity_status = 'healthy' ORDER BY s.identity_bytes",
        &mut remaining,
        false,
    )?;
    let row_limit = i64::try_from(VerifierLimits::default().max_ledger_rows.saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let rows = client
        .query("SELECT s.identity_bytes, s.operation_kind, s.canonical_request_hash, s.basis_complete_head_bytes, s.semantic_input_bytes, s.semantic_time_bytes, s.resolution_kind, s.transition_seq, s.receipt_bytes, s.room_id \
                FROM worldstream_semantic_receipts s \
                JOIN worldstream_room_roots r ON r.room_id = s.room_id \
                WHERE r.integrity_status = 'healthy' ORDER BY s.identity_bytes LIMIT $1", &[&row_limit])
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

fn migration_contract(
    client: &mut Transaction<'_>,
    budget: &mut ProviderReadBudgetV1,
) -> Result<MigrationContractV1, NativePostgresError> {
    let history = migration_history();
    let mut remaining = history.len();
    bounded_global_provider_rows(
        client,
        budget,
        "SELECT jsonb_build_array(version, migration_id, checksum)::text FROM worldstream_schema_migrations ORDER BY version",
        &mut remaining,
        false,
    )?;
    let row_limit = i64::try_from(history.len().saturating_add(1))
        .map_err(|_| NativePostgresError::Incomplete)?;
    let rows = client
        .query("SELECT version, migration_id, checksum FROM worldstream_schema_migrations ORDER BY version LIMIT $1", &[&row_limit])
        .map_err(|_| NativePostgresError::Database)?;
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

fn append_length_prefixed(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn append_membership_bytes(bytes: &mut Vec<u8>, memberships: &[(String, Vec<u8>)]) {
    bytes.extend_from_slice(b"worldstream/postgres-membership-replay-witness/v1\0");
    bytes.extend_from_slice(
        &u64::try_from(memberships.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for (member_id, membership_bytes) in memberships {
        append_length_prefixed(bytes, member_id.as_bytes());
        append_length_prefixed(bytes, membership_bytes);
    }
}

fn append_observation_positions_bytes(
    bytes: &mut Vec<u8>,
    positions: &[crate::PostgresObservationPositionEvidenceV1],
) {
    bytes.extend_from_slice(b"worldstream/postgres-observation-positions/v2\0");
    bytes.extend_from_slice(
        &u64::try_from(positions.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for position in positions {
        append_length_prefixed(bytes, position.member_id.as_bytes());
        bytes.extend_from_slice(&position.frame_head.to_be_bytes());
        bytes.extend_from_slice(&position.retained_frame_floor.to_be_bytes());
        bytes.extend_from_slice(&position.reset_generation.to_be_bytes());
        for optional in [position.last_ack_frame_seq, position.reset_required_through] {
            match optional {
                Some(value) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&value.to_be_bytes());
                }
                None => bytes.push(0),
            }
        }
    }
}

fn append_timer_bytes(bytes: &mut Vec<u8>, timers: &[crate::PostgresTimerEvidenceV1]) {
    bytes.extend_from_slice(b"worldstream/postgres-timer-replay-witness/v1\0");
    bytes.extend_from_slice(
        &u64::try_from(timers.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for timer in timers {
        append_length_prefixed(bytes, timer.timer_id.as_bytes());
        bytes.extend_from_slice(&timer.generation.to_be_bytes());
        append_length_prefixed(bytes, timer.scheduled_for.as_bytes());
        append_length_prefixed(bytes, &timer.payload_bytes);
        append_length_prefixed(bytes, timer.state.as_bytes());
    }
}

fn append_frame_bytes(bytes: &mut Vec<u8>, frames: &[crate::PostgresFrameEvidenceV1]) {
    bytes.extend_from_slice(b"worldstream/postgres-frame-replay-witness/v1\0");
    bytes.extend_from_slice(
        &u64::try_from(frames.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for frame in frames {
        append_length_prefixed(bytes, frame.member_id.as_bytes());
        bytes.extend_from_slice(&frame.frame_seq.to_be_bytes());
        bytes.extend_from_slice(&frame.cause_room_seq.to_be_bytes());
        append_length_prefixed(bytes, &frame.payload_bytes);
        append_length_prefixed(bytes, &frame.payload_hash);
    }
}

fn append_observation_consequences_bytes(
    bytes: &mut Vec<u8>,
    consequences: &[crate::PostgresObservationConsequenceEvidenceV1],
) {
    bytes.extend_from_slice(b"worldstream/postgres-observation-consequences/v1\0");
    bytes.extend_from_slice(
        &u64::try_from(consequences.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for consequence in consequences {
        match consequence {
            crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                payload_bytes,
                projection_hash,
            } => {
                bytes.push(1);
                append_length_prefixed(bytes, member_id.as_bytes());
                bytes.extend_from_slice(&cause_room_seq.to_be_bytes());
                append_length_prefixed(bytes, payload_bytes);
                append_length_prefixed(bytes, projection_hash);
            }
            crate::PostgresObservationConsequenceEvidenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => {
                bytes.push(2);
                append_length_prefixed(bytes, member_id.as_bytes());
                bytes.extend_from_slice(&cause_room_seq.to_be_bytes());
            }
        }
    }
}

fn append_activation_decision_bytes(
    bytes: &mut Vec<u8>,
    decisions: &[crate::PostgresActivationDecisionEvidenceV1],
) {
    bytes.extend_from_slice(b"worldstream/postgres-activation-decision-replay-witness/v1\0");
    bytes.extend_from_slice(
        &u64::try_from(decisions.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for decision in decisions {
        bytes.extend_from_slice(&decision.cause_room_seq.to_be_bytes());
        append_length_prefixed(bytes, decision.decision_id.as_bytes());
        match &decision.target_member_id {
            Some(member_id) => {
                bytes.push(1);
                append_length_prefixed(bytes, member_id.as_bytes());
            }
            None => bytes.push(0),
        }
        append_length_prefixed(bytes, &decision.decision_bytes);
    }
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
    append_membership_bytes(&mut bytes, &verification.membership_bytes);
    append_observation_positions_bytes(&mut bytes, &verification.observation_positions);
    append_timer_bytes(&mut bytes, &verification.timers);
    append_frame_bytes(&mut bytes, &verification.frames);
    append_observation_consequences_bytes(&mut bytes, &verification.observation_consequences);
    append_activation_decision_bytes(&mut bytes, &verification.activation_decisions);
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

fn provider_command(tool: &Path, endpoint: &NativePostgresEndpointV1, passfile: &Path) -> Command {
    let mut command = Command::new(tool);
    command
        .env_clear()
        .env("PGPASSFILE", passfile)
        .env("PGSSLMODE", endpoint.tls_mode.as_libpq())
        .env("PGGSSENCMODE", "disable")
        .arg("--host")
        .arg(&endpoint.host)
        .arg("--port")
        .arg(endpoint.port.to_string())
        .arg("--dbname")
        .arg(&endpoint.database)
        .arg("--username")
        .arg(&endpoint.username)
        .arg("--no-password");
    preserve_windows_runtime_environment(&mut command);
    if endpoint.tls_mode == NativePostgresTlsModeV1::Require {
        command.env("PGSSLROOTCERT", "system");
    }
    command
}

fn preserve_windows_runtime_environment(command: &mut Command) {
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn run_provider_tool<const N: usize>(
    tool: &Path,
    endpoint: &NativePostgresEndpointV1,
    passfile: &Path,
    args: [&str; N],
    stdin: Stdio,
) -> Result<(), NativePostgresError> {
    run_provider_tool_with_timeout(
        tool,
        endpoint,
        passfile,
        args,
        stdin,
        PROVIDER_TOOL_TIMEOUT_V1,
    )
}

fn run_provider_tool_with_timeout<const N: usize>(
    tool: &Path,
    endpoint: &NativePostgresEndpointV1,
    passfile: &Path,
    args: [&str; N],
    stdin: Stdio,
    timeout: Duration,
) -> Result<(), NativePostgresError> {
    let mut command = provider_command(tool, endpoint, passfile);
    command
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(NativePostgresError::Incomplete)?;
    let child = spawn_provider_group(command, false)?;
    let mut child = ProviderChildGuardV1::new(child);
    if child.wait_until(deadline)?.success() {
        Ok(())
    } else {
        Err(NativePostgresError::ProviderCommand)
    }
}

fn password_for(
    endpoint: &NativePostgresEndpointV1,
    passfile: &Path,
) -> Result<String, NativePostgresError> {
    password_for_reader(endpoint, open_verified_pgpassfile(passfile)?)
}

fn password_for_reader(
    endpoint: &NativePostgresEndpointV1,
    passfile: impl Read,
) -> Result<String, NativePostgresError> {
    let content = read_bounded_utf8(passfile, MAX_PGPASSFILE_BYTES_V1)?;
    let entries = content
        .lines()
        .map(parse_pgpass_line)
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>, NativePostgresError>>()?;
    let exact = first_matching_pgpass_password(&entries, endpoint, &endpoint.host);
    if !endpoint.host.starts_with('/') {
        return exact.ok_or(NativePostgresError::Configuration(
            "PGPASSFILE has no matching credential",
        ));
    }
    // libpq matches an explicit Unix-socket directory, except that a directory
    // equal to its build-time default is searched as `localhost`. That default
    // varies across supported provider tool builds, so require both possible
    // selections to resolve to the same secret (a wildcard naturally does).
    let localhost = first_matching_pgpass_password(&entries, endpoint, "localhost");
    match (exact, localhost) {
        (Some(exact), Some(localhost)) if exact == localhost => Ok(exact),
        _ => Err(NativePostgresError::Configuration(
            "Unix-socket PGPASSFILE entries do not select one credential",
        )),
    }
}

fn parse_pgpass_line(line: &str) -> Result<Option<[String; 5]>, NativePostgresError> {
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let mut fields = Vec::with_capacity(5);
    let mut field = String::new();
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            field.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ':' {
            fields.push(std::mem::take(&mut field));
            if fields.len() >= 5 {
                return Err(NativePostgresError::Configuration(
                    "PGPASSFILE line does not have five fields",
                ));
            }
        } else {
            field.push(character);
        }
    }
    if escaped {
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE line ends with an incomplete escape",
        ));
    }
    fields.push(field);
    fields.try_into().map(Some).map_err(|_| {
        NativePostgresError::Configuration("PGPASSFILE line does not have five fields")
    })
}

fn first_matching_pgpass_password(
    entries: &[[String; 5]],
    endpoint: &NativePostgresEndpointV1,
    host: &str,
) -> Option<String> {
    let port = endpoint.port.to_string();
    for fields in entries {
        if (fields[0] == "*" || fields[0] == host)
            && (fields[1] == "*" || fields[1] == port)
            && (fields[2] == "*" || fields[2] == endpoint.database)
            && (fields[3] == "*" || fields[3] == endpoint.username)
        {
            return Some(fields[4].clone());
        }
    }
    None
}

fn read_bounded_utf8<R: Read>(
    reader: R,
    maximum_bytes: usize,
) -> Result<String, NativePostgresError> {
    let read_limit = maximum_bytes
        .checked_add(1)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(NativePostgresError::Configuration(
            "PGPASSFILE is too large",
        ))?;
    let mut bytes = Vec::with_capacity(maximum_bytes.min(8 * 1024).saturating_add(1));
    reader
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| NativePostgresError::Configuration("PGPASSFILE cannot be read"))?;
    if bytes.len() > maximum_bytes {
        return Err(NativePostgresError::Configuration(
            "PGPASSFILE is too large",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| NativePostgresError::Configuration("PGPASSFILE is not UTF-8"))
}

fn connect(
    endpoint: &NativePostgresEndpointV1,
    password: &str,
) -> Result<Client, NativePostgresError> {
    validate_native_postgres_endpoint(endpoint)?;
    let mut config = postgres::Config::new();
    config
        .host(&endpoint.host)
        .port(endpoint.port)
        .dbname(&endpoint.database)
        .user(&endpoint.username)
        .password(password)
        .ssl_mode(endpoint.tls_mode.as_driver());
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
        "host={} port={} dbname={} user={} password={} sslmode={}",
        dsn_value(&endpoint.host),
        endpoint.port,
        dsn_value(&endpoint.database),
        dsn_value(&endpoint.username),
        dsn_value(password),
        endpoint.tls_mode.as_libpq(),
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

    fn provider_process_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    const AUTHORITY_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const AUTHORITY_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    const AUTHORITY_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const AUTHORITY_OTHER_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";

    fn canonical_domain_row(value: &serde_json::Value) -> NativeRestoreCanonicalRowV1 {
        let encoded = serde_json::to_vec(&value)
            .unwrap_or_else(|error| unreachable!("fixture JSON encoding: {error}"));
        let canonical = CanonicalJsonV1::parse(&encoded)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("fixture canonical JSON: {error}"));
        NativeRestoreCanonicalRowV1::new(canonical)
    }

    fn authority_membership_bytea() -> String {
        let membership = worldstream_core::MembershipV1::new(
            AUTHORITY_MEMBER
                .parse()
                .unwrap_or_else(|error| unreachable!("member fixture: {error}")),
            AUTHORITY_PRINCIPAL
                .parse()
                .unwrap_or_else(|error| unreachable!("principal fixture: {error}")),
            PrincipalKindV1::Agent,
            worldstream_core::MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("agent".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("Membership fixture: {error}"));
        let encoded = serde_json::to_vec(&membership)
            .unwrap_or_else(|error| unreachable!("Membership JSON: {error}"));
        let canonical = CanonicalJsonV1::parse(&encoded)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("Membership canonical bytes: {error}"));
        let mut hex = String::with_capacity(canonical.len() * 2 + 2);
        hex.push_str("\\x");
        for byte in canonical {
            write!(&mut hex, "{byte:02x}")
                .unwrap_or_else(|error| unreachable!("Membership hex: {error}"));
        }
        hex
    }

    #[allow(clippy::needless_pass_by_value)]
    fn insert_domain_rows(
        domains: &mut BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
        domain: NativeRestoreDurableDomainV1,
        rows: Vec<serde_json::Value>,
    ) {
        assert!(
            domains
                .insert(domain, rows.iter().map(canonical_domain_row).collect(),)
                .is_none()
        );
    }

    #[allow(clippy::too_many_lines)]
    fn authority_relation_fixture()
    -> BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>> {
        let hash = format!("\\x{}", "00".repeat(32));
        let mut domains = BTreeMap::new();
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityState,
            vec![serde_json::json!([true])],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::TransferTargetFence,
            Vec::new(),
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::RoomRoots,
            vec![serde_json::json!([AUTHORITY_ROOM, "head", 1, "healthy"])],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::MemberDeliveryState,
            vec![serde_json::json!([
                AUTHORITY_ROOM,
                AUTHORITY_MEMBER,
                authority_membership_bytea(),
                0,
                1,
                1,
                null,
                null
            ])],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityPrincipals,
            vec![
                serde_json::json!([AUTHORITY_PRINCIPAL, "agent", "enabled", 1]),
                serde_json::json!([AUTHORITY_OTHER_PRINCIPAL, "human", "enabled", 1]),
            ],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityRunners,
            vec![serde_json::json!([
                "runner-1",
                AUTHORITY_PRINCIPAL,
                "enabled",
                1
            ])],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityCapabilities,
            vec![
                serde_json::json!([
                    "cap-bootstrap",
                    hash,
                    AUTHORITY_PRINCIPAL,
                    "host_operator",
                    null,
                    null,
                    null,
                    1,
                    null,
                    null
                ]),
                serde_json::json!([
                    "cap-runner",
                    format!("\\x{}", "11".repeat(32)),
                    AUTHORITY_PRINCIPAL,
                    "runner_control",
                    null,
                    null,
                    "runner-1",
                    1,
                    null,
                    null
                ]),
                serde_json::json!([
                    "cap-room",
                    format!("\\x{}", "22".repeat(32)),
                    AUTHORITY_PRINCIPAL,
                    "room_member",
                    AUTHORITY_ROOM,
                    AUTHORITY_MEMBER,
                    null,
                    1,
                    null,
                    null
                ]),
            ],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityCapabilityScopes,
            vec![
                serde_json::json!(["cap-bootstrap", "operator:room_admin"]),
                serde_json::json!(["cap-runner", "activation:claim"]),
                serde_json::json!(["cap-room", "room:act"]),
            ],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityRunnerCapabilityMemberships,
            vec![serde_json::json!([
                "cap-runner",
                AUTHORITY_ROOM,
                AUTHORITY_MEMBER
            ])],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityChangeReceipts,
            vec![
                serde_json::json!([
                    "change-1",
                    null,
                    format!("\\x{}", "33".repeat(32)),
                    "authority_bootstrapped",
                    "bootstrap",
                    AUTHORITY_PRINCIPAL,
                    "cap-bootstrap",
                    1,
                    "2026-08-22T00:00:00Z"
                ]),
                serde_json::json!([
                    "change-2",
                    AUTHORITY_PRINCIPAL,
                    format!("\\x{}", "44".repeat(32)),
                    "principal_created",
                    "principal",
                    AUTHORITY_OTHER_PRINCIPAL,
                    null,
                    1,
                    "2026-08-22T00:00:01Z"
                ]),
                serde_json::json!([
                    "change-3",
                    AUTHORITY_PRINCIPAL,
                    format!("\\x{}", "55".repeat(32)),
                    "runner_registered",
                    "runner",
                    "runner-1",
                    null,
                    1,
                    "2026-08-22T00:00:02Z"
                ]),
                serde_json::json!([
                    "change-4",
                    AUTHORITY_PRINCIPAL,
                    format!("\\x{}", "66".repeat(32)),
                    "capability_registered",
                    "capability",
                    "cap-runner",
                    null,
                    1,
                    "2026-08-22T00:00:03Z"
                ]),
                serde_json::json!([
                    "change-5",
                    AUTHORITY_PRINCIPAL,
                    format!("\\x{}", "77".repeat(32)),
                    "capability_registered",
                    "capability",
                    "cap-room",
                    null,
                    1,
                    "2026-08-22T00:00:04Z"
                ]),
            ],
        );
        insert_domain_rows(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityAudit,
            vec![
                serde_json::json!([
                    1,
                    "change-1",
                    null,
                    "bootstrap",
                    AUTHORITY_PRINCIPAL,
                    "cap-bootstrap",
                    "bootstrap_authority",
                    null,
                    1,
                    "2026-08-22T00:00:00Z",
                    null,
                    format!("\\x{}", "33".repeat(32))
                ]),
                serde_json::json!([
                    2,
                    "change-2",
                    AUTHORITY_PRINCIPAL,
                    "principal",
                    AUTHORITY_OTHER_PRINCIPAL,
                    null,
                    "create_principal",
                    null,
                    1,
                    "2026-08-22T00:00:01Z",
                    null,
                    format!("\\x{}", "44".repeat(32))
                ]),
                serde_json::json!([
                    3,
                    "change-3",
                    AUTHORITY_PRINCIPAL,
                    "runner",
                    "runner-1",
                    null,
                    "register_runner",
                    null,
                    1,
                    "2026-08-22T00:00:02Z",
                    null,
                    format!("\\x{}", "55".repeat(32))
                ]),
                serde_json::json!([
                    4,
                    "change-4",
                    AUTHORITY_PRINCIPAL,
                    "capability",
                    "cap-runner",
                    null,
                    "register_capability",
                    null,
                    1,
                    "2026-08-22T00:00:03Z",
                    null,
                    format!("\\x{}", "66".repeat(32))
                ]),
                serde_json::json!([
                    5,
                    "change-5",
                    AUTHORITY_PRINCIPAL,
                    "capability",
                    "cap-room",
                    null,
                    "register_capability",
                    null,
                    1,
                    "2026-08-22T00:00:04Z",
                    null,
                    format!("\\x{}", "77".repeat(32))
                ]),
            ],
        );
        domains
    }

    #[allow(clippy::needless_pass_by_value)]
    fn replace_first_domain_row(
        domains: &mut BTreeMap<NativeRestoreDurableDomainV1, Vec<NativeRestoreCanonicalRowV1>>,
        domain: NativeRestoreDurableDomainV1,
        row: serde_json::Value,
    ) {
        domains
            .get_mut(&domain)
            .unwrap_or_else(|| unreachable!("fixture domain"))[0] = canonical_domain_row(&row);
    }

    #[test]
    fn global_authority_relations_accept_complete_exact_graph() {
        assert!(validate_global_authority_relations(&authority_relation_fixture()).is_ok());
    }

    #[test]
    fn transfer_target_fence_state_is_validated_and_digest_bound() {
        let hash = format!("\\x{}", "88".repeat(32));
        let mut importing = authority_relation_fixture();
        *importing
            .get_mut(&NativeRestoreDurableDomainV1::TransferTargetFence)
            .unwrap_or_else(|| unreachable!("transfer fence fixture")) = vec![
            canonical_domain_row(&serde_json::json!([true, hash, hash, "importing"])),
        ];
        assert!(validate_global_authority_relations(&importing).is_ok());

        let mut aborted = importing.clone();
        replace_first_domain_row(
            &mut aborted,
            NativeRestoreDurableDomainV1::TransferTargetFence,
            serde_json::json!([true, hash, hash, "aborted"]),
        );
        assert!(validate_global_authority_relations(&aborted).is_ok());
        assert_ne!(
            durable_domains_digest(&importing),
            durable_domains_digest(&aborted)
        );
        assert!(!authority_domains_equal(&importing, &aborted));

        replace_first_domain_row(
            &mut aborted,
            NativeRestoreDurableDomainV1::TransferTargetFence,
            serde_json::json!([true, hash, hash, "published"]),
        );
        assert!(matches!(
            validate_global_authority_relations(&aborted),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn transfer_target_fence_capture_includes_lifecycle_state() {
        let query = DURABLE_DOMAIN_QUERIES
            .iter()
            .find_map(|(domain, query)| {
                (*domain == NativeRestoreDurableDomainV1::TransferTargetFence).then_some(*query)
            })
            .unwrap_or_else(|| unreachable!("transfer target fence domain"));
        assert!(query.contains("fence_id, bundle_hash, target_fingerprint, state"));
    }

    #[test]
    fn external_input_preparations_are_exact_native_restore_evidence() {
        assert!(
            POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
                .contains(&NativeRestoreDurableDomainV1::ExternalInputPreparations)
        );
        let query = DURABLE_DOMAIN_QUERIES
            .iter()
            .find_map(|(domain, query)| {
                (*domain == NativeRestoreDurableDomainV1::ExternalInputPreparations)
                    .then_some(*query)
            })
            .unwrap_or_else(|| unreachable!("ExternalInput preparation domain"));
        assert!(query.contains("identity_bytes, canonical_request_hash, recorded_at"));
        assert!(
            query.contains("FROM worldstream_external_input_preparations ORDER BY identity_bytes")
        );
    }

    #[test]
    fn global_authority_relations_reject_missing_creation_lineage() {
        let mut domains = authority_relation_fixture();
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityChangeReceipts)
            .unwrap_or_else(|| unreachable!("receipt fixture"))
            .remove(1);
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityAudit)
            .unwrap_or_else(|| unreachable!("audit fixture"))
            .remove(1);

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_generation_gap() {
        let mut domains = authority_relation_fixture();
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityCapabilities)
            .unwrap_or_else(|| unreachable!("capability fixture"))[2] =
            canonical_domain_row(&serde_json::json!([
                "cap-room",
                format!("\\x{}", "22".repeat(32)),
                AUTHORITY_PRINCIPAL,
                "room_member",
                AUTHORITY_ROOM,
                AUTHORITY_MEMBER,
                null,
                3,
                null,
                null
            ]));

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_runner_terminal_mismatch() {
        let mut domains = authority_relation_fixture();
        replace_first_domain_row(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityRunners,
            serde_json::json!(["runner-1", AUTHORITY_PRINCIPAL, "revoked", 1]),
        );

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_capability_terminal_mismatch() {
        let mut domains = authority_relation_fixture();
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityCapabilities)
            .unwrap_or_else(|| unreachable!("capability fixture"))[2] =
            canonical_domain_row(&serde_json::json!([
                "cap-room",
                format!("\\x{}", "22".repeat(32)),
                AUTHORITY_PRINCIPAL,
                "room_member",
                AUTHORITY_ROOM,
                AUTHORITY_MEMBER,
                null,
                1,
                null,
                "2026-08-22T00:00:05Z"
            ]));

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_cross_profile_scope() {
        let mut domains = authority_relation_fixture();
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityCapabilityScopes)
            .unwrap_or_else(|| unreachable!("scope fixture"))[2] =
            canonical_domain_row(&serde_json::json!(["cap-room", "operator:backup"]));

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_missing_room_member_target() {
        let mut domains = authority_relation_fixture();
        let hash = format!("\\x{}", "22".repeat(32));
        domains
            .get_mut(&NativeRestoreDurableDomainV1::AuthorityCapabilities)
            .unwrap_or_else(|| unreachable!("capability fixture"))[2] =
            canonical_domain_row(&serde_json::json!([
                "cap-room",
                hash,
                AUTHORITY_PRINCIPAL,
                "room_member",
                AUTHORITY_ROOM,
                "missing-member",
                null,
                1,
                null,
                null
            ]));

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_wrong_runner_membership_profile() {
        let mut domains = authority_relation_fixture();
        replace_first_domain_row(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityRunnerCapabilityMemberships,
            serde_json::json!(["cap-room", AUTHORITY_ROOM, AUTHORITY_MEMBER]),
        );

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_receipt_audit_disagreement() {
        let mut domains = authority_relation_fixture();
        replace_first_domain_row(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityAudit,
            serde_json::json!([
                1,
                "change-1",
                null,
                "bootstrap",
                AUTHORITY_PRINCIPAL,
                "cap-bootstrap",
                "bootstrap_authority",
                null,
                1,
                "2026-08-22T00:00:01Z",
                null,
                format!("\\x{}", "33".repeat(32))
            ]),
        );

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn global_authority_relations_reject_bootstrap_capability_owned_by_another_principal() {
        let mut domains = authority_relation_fixture();
        let hash = format!("\\x{}", "00".repeat(32));
        replace_first_domain_row(
            &mut domains,
            NativeRestoreDurableDomainV1::AuthorityCapabilities,
            serde_json::json!([
                "cap-bootstrap",
                hash,
                AUTHORITY_OTHER_PRINCIPAL,
                "host_operator",
                null,
                null,
                null,
                1,
                null,
                null
            ]),
        );

        assert!(matches!(
            validate_global_authority_relations(&domains),
            Err(NativePostgresError::Incomplete)
        ));
    }

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
                    lock.explanatory_version.clone(),
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
    fn retained_pack_locks_preserve_two_revisions_of_one_pack() -> Result<(), NativePostgresError> {
        let registry = worldstream_core::builtin_counter_registry()
            .map_err(|_| NativePostgresError::Incomplete)?;
        let mut packs = Vec::new();
        let mut lock_rows = Vec::new();
        for digest in [
            worldstream_core::counter_v1_digest(),
            worldstream_core::counter_v2_digest(),
        ] {
            let lock = registry
                .load_retained(&digest)
                .map_err(|_| NativePostgresError::Incomplete)?
                .revision_lock()
                .clone();
            packs.push(
                TransferPackIdentityV1::new(
                    lock.pack_id.clone(),
                    lock.explanatory_version.clone(),
                    TransferDigestV1::from_bytes(digest.digest().as_bytes())
                        .map_err(|_| NativePostgresError::Incomplete)?,
                )
                .map_err(|_| NativePostgresError::Incomplete)?,
            );
            lock_rows.push(
                lock.canonical_bytes()
                    .map_err(|_| NativePostgresError::Incomplete)?,
            );
        }
        let identity = TransferDeploymentIdentityV1::new(packs, Vec::new())
            .map_err(|_| NativePostgresError::Incomplete)?;

        let locks = retained_pack_locks(&identity, &lock_rows)?;

        assert_eq!(locks.len(), 2);
        assert_eq!(locks[0].pack_id, locks[1].pack_id);
        assert_ne!(locks[0].explanatory_version, locks[1].explanatory_version);
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
        let Ok(endpoint) = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        ) else {
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
            artifact_directory_identity: NativePostgresArtifactDirectoryIdentityV1 {
                storage_id: "0000000000000001".to_owned(),
                file_id: "00000000000000000000000000000002".to_owned(),
            },
        };
        assert!(!format!("{config:?}").contains("TOP_SECRET"));
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_artifacts_are_exclusive_protected_and_identity_bound() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let directory = parent.path().join("native-restore");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let passfile_path = directory.join("operator.pgpass");
        let mut operator_passfile = create_owner_only_file(&passfile_path)
            .unwrap_or_else(|error| unreachable!("protected passfile: {error}"));
        operator_passfile
            .write_all(
                b"127.0.0.1:15432:source:source_admin:source-secret\n\
                  127.0.0.1:25432:target:target_admin:target-secret\n",
            )
            .unwrap_or_else(|error| unreachable!("write passfile: {error}"));
        operator_passfile
            .flush()
            .unwrap_or_else(|error| unreachable!("flush passfile: {error}"));
        drop(operator_passfile);

        let source = NativePostgresEndpointV1::new(
            "127.0.0.1",
            15432,
            "source",
            "source_admin",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("source endpoint: {error}"));
        let target = NativePostgresEndpointV1::new(
            "127.0.0.1",
            25432,
            "target",
            "target_admin",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("target endpoint: {error}"));
        let executable = std::env::current_exe()
            .unwrap_or_else(|error| unreachable!("current executable: {error}"));
        let dump_path = directory.join("source.dump");
        let artifact_directory_identity = native_postgres_artifact_directory_identity(&directory)
            .unwrap_or_else(|error| unreachable!("artifact directory identity: {error}"));
        let config = NativePostgresRestoreConfig {
            source,
            target,
            passfile: passfile_path.clone(),
            pg_dump: executable.clone(),
            pg_restore: executable.clone(),
            psql: executable,
            dump_path: dump_path.clone(),
            artifact_directory_identity,
        };
        validate_native_restore_configuration(&config)
            .unwrap_or_else(|error| unreachable!("valid native Windows config: {error}"));

        let file = open_exclusive_private_file(&dump_path)
            .unwrap_or_else(|error| unreachable!("exclusive dump: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("dump identity: {error}"));
        assert!(validate_owner_only_file(&dump_path).is_ok());
        assert!(open_exclusive_private_file(&dump_path).is_err());
        let moved = directory.join("moved.dump");
        assert!(fs::rename(&dump_path, &moved).is_err());
        assert!(
            identity
                .still_names_file(&dump_path)
                .unwrap_or_else(|error| unreachable!("locked identity: {error}"))
        );
        drop(file);
        fs::rename(&dump_path, &moved)
            .unwrap_or_else(|error| unreachable!("rename released dump: {error}"));
        let replacement = open_exclusive_private_file(&dump_path)
            .unwrap_or_else(|error| unreachable!("replacement dump: {error}"));
        assert!(
            !identity
                .still_names_file(&dump_path)
                .unwrap_or_else(|error| unreachable!("replacement identity: {error}"))
        );
        drop(replacement);
        fs::remove_file(&dump_path)
            .unwrap_or_else(|error| unreachable!("remove replacement: {error}"));
        fs::remove_file(&moved).unwrap_or_else(|error| unreachable!("remove moved dump: {error}"));

        let mut pinned = pin_pgpassfile(&directory, &passfile_path)
            .unwrap_or_else(|error| unreachable!("pinned passfile: {error}"));
        assert!(pinned.verify().is_ok());
        assert!(validate_owner_only_file(&pinned.path).is_ok());
        pinned
            .remove()
            .unwrap_or_else(|error| unreachable!("pinned cleanup: {error}"));
        assert_eq!(
            fs::metadata(&pinned.path)
                .unwrap_or_else(|error| unreachable!("scrubbed passfile: {error}"))
                .len(),
            0
        );
        assert!(validate_owner_only_file(&pinned.path).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_dump_publication_faults_leave_no_nonempty_public_artifact() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let bytes = b"bounded-native-dump";
        let digest = DigestV1::hash(bytes);
        for (index, fault) in [
            WindowsPublicationFaultV1::BeforeRename,
            WindowsPublicationFaultV1::AfterRenameBeforeIdentity,
            WindowsPublicationFaultV1::AfterIdentityBeforeAcl,
            WindowsPublicationFaultV1::AfterAclBeforeSync,
            WindowsPublicationFaultV1::AfterSyncBeforeHash,
        ]
        .into_iter()
        .enumerate()
        {
            let directory = parent.path().join(format!("publication-fault-{index}"));
            prepare_data_directory(&directory)
                .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
            let authority = NativeRestoreDirectoryAuthorityV1::open(&directory)
                .unwrap_or_else(|error| unreachable!("publication directory: {error}"));
            let private_path = directory.join("private.dump");
            let publication_path = directory.join("public.dump");
            let mut file = open_exclusive_dump_file_in(&authority, &private_path)
                .unwrap_or_else(|error| unreachable!("private dump: {error}"));
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .unwrap_or_else(|error| unreachable!("private dump bytes: {error}"));
            let identity = ExclusiveFileIdentityV1::for_file(&file)
                .unwrap_or_else(|error| unreachable!("private dump identity: {error}"));
            let mut dump = NativeDumpFileV1 {
                directory: Some(authority),
                file: Some(file),
                path: private_path.clone(),
                publication_path: publication_path.clone(),
                identity,
                digest: Some(digest.clone()),
                byte_count: Some(bytes.len() as u64),
                retained: false,
            };
            assert!(
                dump.retain_windows_with_fault(&digest, bytes.len() as u64, Some(fault))
                    .is_err()
            );
            drop(dump);
            if fault == WindowsPublicationFaultV1::BeforeRename {
                assert!(!publication_path.exists());
                assert_eq!(
                    fs::metadata(&private_path)
                        .unwrap_or_else(|error| unreachable!("private metadata: {error}"))
                        .len(),
                    0
                );
                assert!(validate_owner_only_file(&private_path).is_ok());
            } else {
                assert!(!private_path.exists());
                assert_eq!(
                    fs::metadata(&publication_path)
                        .unwrap_or_else(|error| unreachable!("publication metadata: {error}"))
                        .len(),
                    0
                );
                assert!(validate_owner_only_file(&publication_path).is_ok());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_dump_publication_collision_preserves_the_exact_sentinel() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let directory = parent.path().join("publication-collision");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let private_path = directory.join("private.dump");
        let publication_path = directory.join("public.dump");
        let bytes = b"bounded-native-dump";
        let sentinel = b"must-not-be-replaced";
        let mut file = open_exclusive_dump_file(&private_path)
            .unwrap_or_else(|error| unreachable!("private dump: {error}"));
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| unreachable!("private dump bytes: {error}"));
        fs::write(&publication_path, sentinel)
            .unwrap_or_else(|error| unreachable!("publication sentinel: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("private dump identity: {error}"));
        let digest = DigestV1::hash(bytes);
        let mut dump = NativeDumpFileV1 {
            directory: Some(
                NativeRestoreDirectoryAuthorityV1::open(&directory)
                    .unwrap_or_else(|error| unreachable!("test dump directory: {error}")),
            ),
            file: Some(file),
            path: private_path.clone(),
            publication_path: publication_path.clone(),
            identity,
            digest: Some(digest.clone()),
            byte_count: Some(bytes.len() as u64),
            retained: false,
        };
        assert!(
            dump.retain_windows_with_fault(&digest, bytes.len() as u64, None)
                .is_err()
        );
        drop(dump);
        assert_eq!(
            fs::read(&publication_path)
                .unwrap_or_else(|error| unreachable!("publication sentinel bytes: {error}")),
            sentinel
        );
        assert_eq!(
            fs::metadata(&private_path)
                .unwrap_or_else(|error| unreachable!("scrubbed private metadata: {error}"))
                .len(),
            0
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_dump_publication_success_keeps_exact_verified_bytes() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let directory = parent.path().join("publication-success");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&directory)
            .unwrap_or_else(|error| unreachable!("publication directory: {error}"));
        let private_path = directory.join("private.dump");
        let publication_path = directory.join("public.dump");
        let bytes = b"bounded-native-dump";
        let mut file = open_exclusive_dump_file_in(&authority, &private_path)
            .unwrap_or_else(|error| unreachable!("private dump: {error}"));
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| unreachable!("private dump bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("private dump identity: {error}"));
        let digest = DigestV1::hash(bytes);
        let mut dump = NativeDumpFileV1 {
            directory: Some(authority),
            file: Some(file),
            path: private_path.clone(),
            publication_path: publication_path.clone(),
            identity,
            digest: Some(digest),
            byte_count: Some(bytes.len() as u64),
            retained: false,
        };
        let moved_private = directory.join("moved-private.dump");
        assert!(fs::rename(&private_path, &moved_private).is_err());
        assert!(fs::remove_file(&private_path).is_err());
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&private_path)
                .is_err()
        );
        dump.retain()
            .unwrap_or_else(|error| unreachable!("Windows publication: {error}"));
        let moved_publication = directory.join("moved-public.dump");
        assert!(fs::rename(&publication_path, &moved_publication).is_err());
        assert!(fs::remove_file(&publication_path).is_err());
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&publication_path)
                .is_err()
        );
        drop(dump);
        assert_eq!(
            fs::read(&publication_path)
                .unwrap_or_else(|error| unreachable!("published dump: {error}")),
            bytes
        );
        assert!(validate_owner_only_file(&publication_path).is_ok());
        assert!(!private_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_retained_parent_prevents_substitution_during_publication() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let ancestor = parent.path().join("retained-ancestor");
        let directory = ancestor.join("retained-publication");
        prepare_data_directory(&directory)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&directory)
            .unwrap_or_else(|error| unreachable!("retained publication directory: {error}"));
        let private_path = directory.join("private.dump");
        let publication_path = directory.join("public.dump");
        let bytes = b"identity-bound-parent-publication";
        let mut file = open_exclusive_dump_file_in(&authority, &private_path)
            .unwrap_or_else(|error| unreachable!("private dump: {error}"));
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| unreachable!("private dump bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("private dump identity: {error}"));
        let digest = DigestV1::hash(bytes);
        let mut publication = NativeDumpFileV1 {
            directory: Some(authority),
            file: Some(file),
            path: private_path,
            publication_path: publication_path.clone(),
            identity,
            digest: Some(digest),
            byte_count: Some(bytes.len() as u64),
            retained: false,
        };
        let moved_directory = ancestor.join("moved-publication");
        let moved_ancestor = parent.path().join("moved-ancestor");
        assert!(fs::rename(&directory, &moved_directory).is_err());
        assert!(fs::rename(&ancestor, &moved_ancestor).is_err());
        publication
            .retain()
            .unwrap_or_else(|error| unreachable!("retained-parent publication: {error}"));
        assert_eq!(
            fs::read(&publication_path)
                .unwrap_or_else(|error| unreachable!("published bytes: {error}")),
            bytes
        );
        drop(publication);
        fs::rename(&ancestor, &moved_ancestor)
            .unwrap_or_else(|error| unreachable!("released parent rename: {error}"));
        assert_eq!(
            fs::read(moved_ancestor.join("retained-publication/public.dump"))
                .unwrap_or_else(|error| unreachable!("moved published bytes: {error}")),
            bytes
        );
    }

    #[test]
    fn endpoint_tls_policy_rejects_remote_plaintext_and_reaches_every_client() {
        assert!(matches!(
            NativePostgresEndpointV1::new(
                "db.example",
                5432,
                "worldstream",
                "postgres",
                NativePostgresTlsModeV1::Disable,
            ),
            Err(NativePostgresError::Configuration(
                "remote PostgreSQL endpoint requires TLS"
            ))
        ));
        assert!(matches!(
            NativePostgresEndpointV1::new(
                "localhost",
                5432,
                "worldstream",
                "postgres",
                NativePostgresTlsModeV1::Disable,
            ),
            Err(NativePostgresError::Configuration(
                "remote PostgreSQL endpoint requires TLS"
            ))
        ));
        #[cfg(unix)]
        assert!(matches!(
            NativePostgresEndpointV1::new(
                "/var/run/postgresql",
                5432,
                "worldstream",
                "postgres",
                NativePostgresTlsModeV1::Require,
            ),
            Err(NativePostgresError::Configuration(
                "authenticated TLS requires a TCP PostgreSQL endpoint"
            ))
        ));
        #[cfg(unix)]
        assert!(
            NativePostgresEndpointV1::new(
                "/var/run/postgresql",
                5432,
                "worldstream",
                "postgres",
                NativePostgresTlsModeV1::Disable,
            )
            .is_ok()
        );

        let remote = NativePostgresEndpointV1::new(
            "db.example",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Require,
        )
        .unwrap_or_else(|error| unreachable!("remote TLS endpoint: {error}"));
        let local = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("local plaintext endpoint: {error}"));

        assert!(dsn_for(&remote, "secret").ends_with("sslmode=verify-full"));
        assert!(dsn_for(&local, "secret").ends_with("sslmode=disable"));
        assert!(PostgresConnectionConfig::direct_admin(dsn_for(&remote, "secret")).is_ok());
        assert_eq!(remote.tls_mode.as_driver(), SslMode::Require);
        assert_eq!(local.tls_mode.as_driver(), SslMode::Disable);
        assert!("prefer".parse::<NativePostgresTlsModeV1>().is_err());
    }

    #[test]
    fn destructive_restore_rejects_same_database_coordinates_for_different_roles() {
        let source = NativePostgresEndpointV1::new(
            "DB.EXAMPLE",
            5432,
            "worldstream",
            "source_admin",
            NativePostgresTlsModeV1::Require,
        )
        .unwrap_or_else(|error| unreachable!("source endpoint: {error}"));
        let target = NativePostgresEndpointV1::new(
            "db.example",
            5432,
            "worldstream",
            "target_admin",
            NativePostgresTlsModeV1::Require,
        )
        .unwrap_or_else(|error| unreachable!("target endpoint: {error}"));

        assert!(matches!(
            validate_distinct_endpoint_coordinates(&source, &target),
            Err(NativePostgresError::Configuration(
                "source and target coordinates identify the same database"
            ))
        ));
    }

    #[test]
    fn provider_identity_detects_host_aliases_to_the_same_database() {
        let source = PostgresProviderIdentityV1 {
            system_identifier: "7612345678901234567".to_owned(),
            database_oid: "16384".to_owned(),
            database_name: "worldstream".to_owned(),
        };
        let target = source.clone();

        assert_eq!(source, target);
    }

    fn isolated_target_observation() -> PostgresProviderObservationV1 {
        PostgresProviderObservationV1 {
            identity: PostgresProviderIdentityV1 {
                system_identifier: "7612345678901234567".to_owned(),
                database_oid: "16385".to_owned(),
                database_name: "worldstream_restore".to_owned(),
            },
            marker: Some(NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned()),
            other_client_backends: 0,
            connection_limit: 2,
            current_user_is_superuser: false,
        }
    }

    #[test]
    fn target_isolation_lease_rejects_identity_marker_session_and_limit_changes() {
        let admitted = isolated_target_observation();
        assert!(target_observation_is_admissible(
            &admitted,
            &admitted.identity,
            NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1,
            2
        ));

        let mut swapped_identity = admitted.clone();
        swapped_identity.identity.database_oid = "16386".to_owned();
        let mut removed_marker = admitted.clone();
        removed_marker.marker = None;
        let mut competing_session = admitted.clone();
        competing_session.other_client_backends = 1;
        let mut widened_limit = admitted.clone();
        widened_limit.connection_limit = -1;

        for observation in [
            swapped_identity,
            removed_marker,
            competing_session,
            widened_limit,
        ] {
            assert!(!target_observation_is_admissible(
                &observation,
                &admitted.identity,
                NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1,
                2
            ));
        }
    }

    #[test]
    fn target_isolation_counts_every_database_bound_backend() {
        assert!(
            POSTGRES_PROVIDER_OBSERVATION_SQL.contains("shobj_description(db.oid, 'pg_database')")
        );
        assert!(!POSTGRES_PROVIDER_OBSERVATION_SQL.contains("db.datname, obj_description"));
        assert!(POSTGRES_PROVIDER_OBSERVATION_SQL.contains("activity.datid = db.oid"));
        assert!(POSTGRES_PROVIDER_OBSERVATION_SQL.contains("activity.pid <> pg_backend_pid()"));
        assert!(!POSTGRES_PROVIDER_OBSERVATION_SQL.contains("backend_type"));
    }

    #[test]
    fn target_restore_limit_counts_the_direct_superuser_keeper_and_restore_backend() {
        assert_eq!(target_restore_connection_limit(), 2);
    }

    #[test]
    fn restore_role_cleanup_targets_sessions_across_the_cluster() {
        for sql in [
            TERMINATE_RESTORE_ROLE_BACKENDS_SQL,
            COUNT_RESTORE_ROLE_BACKENDS_SQL,
        ] {
            assert!(sql.contains("FROM pg_stat_activity"));
            assert!(sql.contains("usename = $1"));
            assert!(!sql.contains("datid"));
            assert!(!sql.contains("current_database"));
        }
    }

    #[test]
    fn restore_role_ddl_contains_only_a_scram_verifier() {
        let plaintext = "native_restore_plaintext_must_never_reach_sql_or_server_logs";
        let sql = restore_role_create_sql(
            "\"worldstream_restore_test\"",
            plaintext,
            "2030-01-01 00:00:00+00",
        );
        assert!(!sql.contains(plaintext));
        assert!(sql.contains("PASSWORD 'SCRAM-SHA-256$4096:"));
        assert!(sql.contains("CONNECTION LIMIT 1"));
    }

    struct FakeRestoreRoleCleanupV1 {
        failed_stage: Option<&'static str>,
        terminated: bool,
        remaining: i64,
        initially_exists: bool,
        finally_exists: bool,
        role_exists_calls: usize,
        calls: Vec<&'static str>,
    }

    impl FakeRestoreRoleCleanupV1 {
        fn stage(&mut self, stage: &'static str) -> Result<(), NativePostgresError> {
            self.calls.push(stage);
            if self.failed_stage == Some(stage) {
                Err(NativePostgresError::Database)
            } else {
                Ok(())
            }
        }
    }

    impl RestoreRoleCleanupBackendV1 for FakeRestoreRoleCleanupV1 {
        fn disable_login(&mut self) -> Result<(), NativePostgresError> {
            self.stage("disable_login")
        }

        fn terminate_backends(&mut self) -> Result<bool, NativePostgresError> {
            self.stage("terminate_backends")?;
            Ok(self.terminated)
        }

        fn remaining_backends(&mut self) -> Result<i64, NativePostgresError> {
            self.stage("remaining_backends")?;
            Ok(self.remaining)
        }

        fn reassign_owned(&mut self) -> Result<(), NativePostgresError> {
            self.stage("reassign_owned")
        }

        fn drop_owned(&mut self) -> Result<(), NativePostgresError> {
            self.stage("drop_owned")
        }

        fn drop_role(&mut self) -> Result<(), NativePostgresError> {
            self.stage("drop_role")
        }

        fn role_exists(&mut self) -> Result<bool, NativePostgresError> {
            let stage = if self.role_exists_calls == 0 {
                "role_exists_initial"
            } else {
                "role_exists_final"
            };
            self.role_exists_calls = self.role_exists_calls.saturating_add(1);
            self.stage(stage)?;
            Ok(if self.role_exists_calls == 1 {
                self.initially_exists
            } else {
                self.finally_exists
            })
        }
    }

    fn fake_restore_role_cleanup() -> FakeRestoreRoleCleanupV1 {
        FakeRestoreRoleCleanupV1 {
            failed_stage: None,
            terminated: true,
            remaining: 0,
            initially_exists: true,
            finally_exists: false,
            role_exists_calls: 0,
            calls: Vec::new(),
        }
    }

    #[test]
    fn restore_role_cleanup_faults_never_skip_later_safe_stages() {
        let every_stage = [
            "role_exists_initial",
            "disable_login",
            "terminate_backends",
            "remaining_backends",
            "reassign_owned",
            "drop_owned",
            "drop_role",
            "role_exists_final",
        ];
        for failed_stage in [
            "reassign_owned",
            "drop_owned",
            "drop_role",
            "role_exists_final",
        ] {
            let mut cleanup = fake_restore_role_cleanup();
            cleanup.failed_stage = Some(failed_stage);
            assert!(run_restore_role_cleanup_stages(&mut cleanup).is_err());
            assert_eq!(cleanup.calls, every_stage);
        }

        let mut cleanup = fake_restore_role_cleanup();
        cleanup.initially_exists = false;
        run_restore_role_cleanup_stages(&mut cleanup)
            .unwrap_or_else(|error| unreachable!("absent role is fully cleaned: {error}"));
        assert_eq!(cleanup.calls, ["role_exists_initial"]);

        let mut cleanup = fake_restore_role_cleanup();
        cleanup.failed_stage = Some("role_exists_initial");
        assert!(matches!(
            run_restore_role_cleanup_stages(&mut cleanup),
            Err(NativePostgresError::Database)
        ));
        assert_eq!(cleanup.calls, ["role_exists_initial"]);
    }

    #[test]
    fn restore_role_cleanup_never_drops_a_role_until_backends_are_proven_zero() {
        for (failed_stage, terminated, remaining, database_error) in [
            (Some("disable_login"), true, 0, true),
            (None, false, 1, false),
            (Some("terminate_backends"), true, 0, true),
            (Some("remaining_backends"), true, 0, true),
            (None, true, 1, false),
        ] {
            let mut cleanup = fake_restore_role_cleanup();
            cleanup.failed_stage = failed_stage;
            cleanup.terminated = terminated;
            cleanup.remaining = remaining;
            let result = run_restore_role_cleanup_stages(&mut cleanup);
            assert!(
                matches!(
                    result,
                    Err(NativePostgresError::Database) if database_error
                ) || matches!(
                    result,
                    Err(NativePostgresError::Incomplete) if !database_error
                )
            );
            assert_eq!(
                cleanup.calls,
                [
                    "role_exists_initial",
                    "disable_login",
                    "terminate_backends",
                    "remaining_backends"
                ]
            );
        }
    }

    #[test]
    fn restore_role_is_tracked_before_any_grant_or_admission_failure() {
        let source = include_str!("native_restore.rs");
        let start = source
            .find("fn create_restore_credential(")
            .unwrap_or_else(|| unreachable!("restore credential function"));
        let end = source[start..]
            .find("fn verify_restore_credential(")
            .map(|offset| start + offset)
            .unwrap_or_else(|| unreachable!("restore credential boundary"));
        let body = &source[start..end];
        let tracked = body
            .find("self.outstanding_restore_role = Some(role_name.clone())")
            .unwrap_or_else(|| unreachable!("tracked restore role"));
        let grant = body
            .find("GRANT {role} TO CURRENT_USER")
            .unwrap_or_else(|| unreachable!("restore grants"));
        assert!(tracked < grant);
        let admission_closed = body
            .find("self.close_restore_role_admission()?")
            .unwrap_or_else(|| unreachable!("restore admission closes before cleanup"));
        let retirement = body
            .find("self.retire_outstanding_restore_role()?")
            .unwrap_or_else(|| unreachable!("tracked role cleanup"));
        assert!(admission_closed < retirement);
        assert!(body.contains("self.retire_outstanding_restore_role()?"));
    }

    #[test]
    fn durable_domain_capture_bounds_rows_before_driver_materialization() {
        let base = "SELECT payload::text FROM durable_rows ORDER BY row_id";
        let bounded = bounded_canonical_projection_query(base, 1, 2);
        assert!(
            bounded
                .contains("substring(convert_to(projected.canonical_row, 'UTF8') FROM 1 FOR $2)")
        );
        assert!(
            bounded.contains("octet_length(convert_to(projected.canonical_row, 'UTF8'))::bigint")
        );
        assert!(bounded.contains(&format!("FROM ({base} LIMIT $1)")));
    }

    #[test]
    fn native_capture_never_enters_runtime_marker_admission() {
        let source = include_str!("native_restore.rs");
        let runtime_store_type = ["PostgresRoom", "Store"].concat();
        let runtime_constructor = ["PostgresConnectionConfig::", "runtime("].concat();
        assert!(!source.contains(&runtime_store_type));
        assert!(!source.contains(&runtime_constructor));
    }

    #[test]
    fn native_snapshot_rebuild_uses_the_bounded_admin_path() {
        let source = include_str!("native_restore.rs");
        let start = source
            .find("pub fn rebuild_native_snapshot_cache(")
            .unwrap_or_else(|| unreachable!("native rebuild entry point must exist"));
        let end = source[start..]
            .find("/// Closed errors from the provider-native boundary.")
            .map(|offset| start + offset)
            .unwrap_or_else(|| unreachable!("native rebuild boundary must exist"));
        let body = &source[start..end];

        assert!(body.contains("verify_runtime_schema_for_native_restore"));
        assert!(body.contains("bounded_global_provider_rows"));
        assert!(body.contains("ORDER BY room_id LIMIT $1"));
        assert!(body.contains("rebuild_snapshot_cache_for_native_restore"));
        assert!(!body.contains(".rebuild_snapshot_cache("));
    }

    #[test]
    fn provider_budget_accepts_exact_aggregate_and_rejects_next_byte_or_row() {
        let mut exact = ProviderReadBudgetV1 {
            remaining_rows: 1,
            remaining_bytes: 4,
            maximum_row_bytes: 4,
            maximum_rooms: 1,
            maximum_records_per_room: 1,
            maximum_packs: 1,
            maximum_resources: 1,
        };
        assert_eq!(
            exact
                .admit_projection(b"null", 4)
                .unwrap_or_else(|_| unreachable!("exact budget must admit")),
            b"null".to_vec()
        );
        assert!(exact.admit_projection(b"0", 1).is_err());

        let mut oversized = ProviderReadBudgetV1 {
            remaining_rows: 1,
            remaining_bytes: 5,
            maximum_row_bytes: 4,
            maximum_rooms: 1,
            maximum_records_per_room: 1,
            maximum_packs: 1,
            maximum_resources: 1,
        };
        assert!(oversized.admit_projection(b"null ", 5).is_err());
    }

    #[test]
    fn dump_hash_stream_accepts_max_and_rejects_max_plus_one() {
        let exact = vec![7_u8; 65_537];
        assert_eq!(
            hash_bounded_reader(std::io::Cursor::new(&exact), 65_537)
                .unwrap_or_else(|_| unreachable!("exact dump bound must admit")),
            DigestV1::hash(&exact)
        );
        assert!(matches!(
            hash_bounded_reader(std::io::Cursor::new(&exact), 65_536),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[cfg(unix)]
    fn fake_dump_config(
        directory: &Path,
        script_body: &str,
    ) -> Result<NativePostgresRestoreConfig, NativePostgresError> {
        let directory = directory.join("native-restore");
        prepare_data_directory(&directory).map_err(|_| NativePostgresError::ProviderCommand)?;
        let tool = directory.join("fake-pg-dump");
        fs::write(&tool, format!("#!/bin/sh\n{script_body}\n"))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let passfile = directory.join("pgpass");
        fs::write(&passfile, "*:*:*:*:unused\n")
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        fs::set_permissions(&passfile, fs::Permissions::from_mode(0o600))
            .map_err(|_| NativePostgresError::ProviderCommand)?;
        let artifact_directory_identity = native_postgres_artifact_directory_identity(&directory)?;
        Ok(NativePostgresRestoreConfig {
            source: NativePostgresEndpointV1::new(
                "127.0.0.1",
                15432,
                "source",
                "source",
                NativePostgresTlsModeV1::Disable,
            )?,
            target: NativePostgresEndpointV1::new(
                "127.0.0.1",
                25432,
                "target",
                "target",
                NativePostgresTlsModeV1::Disable,
            )?,
            passfile,
            pg_dump: tool.clone(),
            pg_restore: tool.clone(),
            psql: tool,
            dump_path: directory.join("source.dump"),
            artifact_directory_identity,
        })
    }

    #[cfg(unix)]
    fn fake_worker_recovery_plan(
        config: &NativePostgresRestoreConfig,
    ) -> NativePostgresWorkerRecoveryPlanV1 {
        let directory = config
            .dump_path
            .parent()
            .unwrap_or_else(|| unreachable!("dump parent"));
        let identity = NativeWorkerFileIdentityV1 {
            storage_id: "0000000000000001".to_owned(),
            file_id: "00000000000000000000000000000001".to_owned(),
        };
        NativePostgresWorkerRecoveryPlanV1 {
            role_name: "worldstream_restore_0123456789abcdef0123456789abcdef".to_owned(),
            repair_passfile: config.passfile.clone(),
            operator_passfile_identity: identity.clone(),
            operator_passfile_is_anonymous: cfg!(target_os = "linux"),
            expected_target: PostgresProviderIdentityV1 {
                system_identifier: "1".to_owned(),
                database_oid: "2".to_owned(),
                database_name: config.target.database.clone(),
            },
            role_passfile: directory.join(".worldstream_restore_test.pgpass"),
            role_passfile_identity: identity.clone(),
            role_passfile_is_anonymous: cfg!(target_os = "linux"),
            private_dump_path: directory.join(".worldstream_dump_test.partial"),
            private_dump_identity: identity,
            private_dump_is_anonymous: cfg!(target_os = "linux"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_worker_request_is_canonical_bounded_and_contains_no_credential_bytes() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "exit 0")
            .unwrap_or_else(|error| unreachable!("fake worker config: {error}"));
        let request =
            NativePostgresWorkerRequestV1::new(&config, fake_worker_recovery_plan(&config))
                .unwrap_or_else(|error| unreachable!("worker request: {error}"));
        let mut bytes = Vec::new();
        write_native_postgres_worker_request(&request, &mut bytes)
            .unwrap_or_else(|error| unreachable!("worker request bytes: {error}"));
        assert_eq!(
            bytes,
            serde_json::to_vec(&request)
                .unwrap_or_else(|error| unreachable!("canonical request: {error}"))
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("unused"));

        let mut noncanonical = bytes;
        noncanonical.push(b'\n');
        assert!(matches!(
            execute_contained_native_postgres_restore_worker(
                &noncanonical[..],
                Vec::new(),
                Vec::new(),
            ),
            Err(NativePostgresError::Incomplete)
        ));
        assert!(matches!(
            read_native_worker_bytes(
                std::io::Cursor::new(vec![b'x'; MAX_NATIVE_WORKER_REQUEST_BYTES_V1 + 1]),
                MAX_NATIVE_WORKER_REQUEST_BYTES_V1,
            ),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[test]
    fn direct_native_worker_invocation_is_rejected_without_supervisor_marker() {
        assert!(matches!(
            execute_native_postgres_restore_worker(&b"{}"[..], Vec::new(), Vec::new()),
            Err(NativePostgresError::Configuration(
                "native worker requires supervised containment"
            ))
        ));
        assert!(matches!(
            execute_native_postgres_restore_commit_worker(),
            Err(NativePostgresError::Configuration(
                "native worker requires supervised containment"
            ))
        ));
    }

    #[test]
    fn native_worker_result_rejects_noncanonical_and_failed_bytes_without_minting_a_witness() {
        let result = NativePostgresWorkerResultV1 {
            schema: NATIVE_POSTGRES_WORKER_RESULT_SCHEMA_V1.to_owned(),
            record: "result".to_owned(),
            status: "failed".to_owned(),
            report: None,
            restored_global_digest: None,
            reason: "native_worker_provider_operation_failed".to_owned(),
        };
        let mut bytes = serde_json::to_vec(&result)
            .unwrap_or_else(|error| unreachable!("failed worker result: {error}"));
        bytes.push(b'\n');
        let observed = read_native_postgres_worker_output(&bytes[..])
            .unwrap_or_else(|error| unreachable!("failed worker output: {error}"));
        assert!(matches!(
            verified_native_worker_checkpoint(
                observed
                    .result
                    .unwrap_or_else(|| unreachable!("failed worker result record")),
            ),
            Err(NativePostgresError::ProviderCommand)
        ));
        let mut noncanonical = bytes;
        noncanonical.push(b'\n');
        assert!(matches!(
            read_native_postgres_worker_output(&noncanonical[..]),
            Err(NativePostgresError::Incomplete)
        ));
    }

    fn minimal_ready_native_worker_report() -> NativePostgresRestoreReportV1 {
        let verifier: VerificationReportV1 = serde_json::from_value(serde_json::json!({
            "readiness": "Ready",
            "rooms": {},
            "diagnostics": []
        }))
        .unwrap_or_else(|error| unreachable!("minimal verifier report: {error}"));
        let empty_digest = DigestV1::hash(&[]).as_str().to_owned();
        let dump_digest = DigestV1::hash(b"native-dump");
        NativePostgresRestoreReportV1 {
            schema: NATIVE_POSTGRES_RESTORE_EVIDENCE_SCHEMA_V1.to_owned(),
            status: "ready".to_owned(),
            reason: "postgres_native_restore_verified_by_unified_verifier".to_owned(),
            release_evidence: false,
            native_dump_restore: "pass".to_owned(),
            backup_id: postgres_native_backup_id(&dump_digest),
            native_point_digest: postgres_native_point_digest(&dump_digest)
                .unwrap_or_else(|error| unreachable!("native point digest: {error}"))
                .as_str()
                .to_owned(),
            native_dump_digest: dump_digest.as_str().to_owned(),
            native_dump_size_bytes: 11,
            source_provider_identity: PostgresProviderIdentityV1 {
                system_identifier: "100".to_owned(),
                database_oid: "200".to_owned(),
                database_name: "source".to_owned(),
            },
            target_provider_identity: PostgresProviderIdentityV1 {
                system_identifier: "101".to_owned(),
                database_oid: "201".to_owned(),
                database_name: "target".to_owned(),
            },
            verifier,
            source_unchanged: true,
            exact_restored_row_set: true,
            snapshots_disposable: true,
            target_isolated: true,
            target_published: false,
            cleanup_required: true,
            native_witness_minted: true,
            secrets_emitted: false,
            source_version_num: Some(REQUIRED_POSTGRES_VERSION_NUM),
            restored_version_num: Some(REQUIRED_POSTGRES_VERSION_NUM),
            semantic_receipts_verified: true,
            activation_intents_verified: true,
            activation_operation_receipts_verified: true,
            activation_request_evidence: "stored_canonical_hash_only_verified".to_owned(),
            authority_state_verified: true,
            durable_domains_verified: true,
            verifier_scope: NativePostgresVerifierScopeV1 {
                profile: "full_deployment_all_durable_domains".to_owned(),
                source_pack_identity_count: 0,
                restored_pack_identity_count: 0,
                source_resource_identity_count: 0,
                restored_resource_identity_count: 0,
                source_fired_timer_count: 0,
                restored_fired_timer_count: 0,
                general_deployment_support_verified: false,
            },
            source_durable_domains_digest: empty_digest.clone(),
            restored_durable_domains_digest: empty_digest.clone(),
            durable_domain_inventory: POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1
                .into_iter()
                .map(|domain| NativePostgresDurableDomainReportV1 {
                    domain: domain.as_str().to_owned(),
                    source_row_count: 0,
                    restored_row_count: 0,
                    source_digest: empty_digest.clone(),
                    restored_digest: empty_digest.clone(),
                })
                .collect(),
            restored_snapshot_count_before: 0,
            restored_snapshot_count_after: 0,
        }
    }

    fn test_published_dump(directory: &Path, label: &str) -> (NativeDumpFileV1, DigestV1) {
        let private_path = directory.join(format!("{label}.private"));
        let publication_path = directory.join(format!("{label}.public"));
        let bytes = b"native-dump";
        let directory = NativeRestoreDirectoryAuthorityV1::open(directory)
            .unwrap_or_else(|error| unreachable!("test directory authority: {error}"));
        let mut file = open_exclusive_dump_file_in(&directory, &private_path)
            .unwrap_or_else(|error| unreachable!("test dump: {error}"));
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| unreachable!("test dump bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("test dump identity: {error}"));
        let digest = DigestV1::hash(bytes);
        let mut dump = NativeDumpFileV1 {
            directory: Some(directory),
            file: Some(file),
            path: private_path,
            publication_path,
            identity,
            digest: Some(digest.clone()),
            byte_count: Some(bytes.len() as u64),
            retained: false,
        };
        dump.retain()
            .unwrap_or_else(|error| unreachable!("test dump publication: {error}"));
        (dump, digest)
    }

    fn test_recovery_record(
        directory: &Path,
        label: &str,
        dump: &NativeDumpFileV1,
        digest: &DigestV1,
    ) -> NativePostgresRecoveryRecordV1 {
        NativePostgresRecoveryRecordV1 {
            schema: NATIVE_POSTGRES_RECOVERY_RECORD_SCHEMA_V1.to_owned(),
            disposition: "publication_committed_pending_report_ack".to_owned(),
            target: PostgresProviderIdentityV1 {
                system_identifier: "1".to_owned(),
                database_oid: "2".to_owned(),
                database_name: "target".to_owned(),
            },
            target_marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
            role_name: "worldstream_restore_0123456789abcdef0123456789abcdef".to_owned(),
            artifact_directory_identity: NativeRestoreDirectoryAuthorityV1::open(directory)
                .unwrap_or_else(|error| unreachable!("recovery directory: {error}"))
                .identity
                .clone(),
            role_passfile_path: directory.join(format!("{label}.role.pgpass")),
            role_passfile_identity: dump.identity.worker_identity(),
            role_passfile_is_anonymous: cfg!(target_os = "linux"),
            private_dump_path: directory.join(format!("{label}.logical-private")),
            private_dump_identity: dump.identity.worker_identity(),
            private_dump_is_anonymous: cfg!(target_os = "linux"),
            publication_path: dump.publication_path.clone(),
            native_dump_digest: Some(digest.as_str().to_owned()),
            native_dump_size_bytes: Some(11),
            private_report_path: None,
            report_path: None,
            report_identity: None,
            report_digest: None,
            report_size_bytes: None,
            operator_action: "persist_the_redacted_report_then_acknowledge_the_publication"
                .to_owned(),
        }
    }

    #[test]
    fn public_native_worker_readiness_accepts_minimal_deployments_and_exact_domains() {
        let report = minimal_ready_native_worker_report();
        assert!(native_worker_report_is_ready(&report));

        let mut duplicate_domain = report.clone();
        duplicate_domain.durable_domain_inventory[1].domain =
            duplicate_domain.durable_domain_inventory[0].domain.clone();
        assert!(!native_worker_report_is_ready(&duplicate_domain));

        let mut uppercase_digest = report;
        uppercase_digest
            .source_durable_domains_digest
            .make_ascii_uppercase();
        uppercase_digest.restored_durable_domains_digest =
            uppercase_digest.source_durable_domains_digest.clone();
        assert!(!native_worker_report_is_ready(&uppercase_digest));
    }

    #[test]
    fn redacted_transport_cleanup_fault_does_not_reclassify_verified_publication() {
        let report = minimal_ready_native_worker_report();
        let committed_identity =
            NativePostgresArtifactDirectoryIdentityV1::new("0".repeat(16), "0".repeat(32))
                .unwrap_or_else(|error| unreachable!("committed artifact identity: {error}"));
        let outcome = NativePostgresRestoreOutcome {
            report: report.clone(),
            witness: NativePostgresTrustedWitnessV1 {
                dump_digest: DigestV1::parse(report.native_dump_digest.clone())
                    .unwrap_or_else(|error| unreachable!("dump digest: {error}")),
                restored_global_digest: DigestV1::hash(b"restored-global"),
            },
            native_dump_identity: committed_identity.clone(),
            report_identity: committed_identity.clone(),
            recovery_record_identity: committed_identity,
            recovery_record_name:
                ".worldstream_native_recovery_00000000000000000000000000000000.json".to_owned(),
        };
        let accepted = finish_native_worker_supervision(
            Ok(outcome),
            Err(NativePostgresError::ProviderCommand),
            Err(NativePostgresError::ProviderCommand),
        )
        .unwrap_or_else(|error| unreachable!("verified publication remains accepted: {error}"));
        assert_eq!(accepted.report, report);
        assert_eq!(
            accepted.witness.restored_global_digest,
            DigestV1::hash(b"restored-global")
        );
        assert!(native_recovery_record_name_is_valid(
            accepted.recovery_record_name()
        ));

        assert!(matches!(
            finish_native_worker_supervision(
                Err(NativePostgresError::Incomplete),
                Err(NativePostgresError::ProviderCommand),
                Err(NativePostgresError::ProviderCommand),
            ),
            Err(NativePostgresError::Incomplete)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn contained_worker_provider_inherits_the_outer_process_group() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let tool = directory.path().join("contained-provider");
        fs::write(
            &tool,
            "#!/bin/sh\nps -o pgid= -p $$ | tr -d ' ' > \"$0.pgid\"\nexit 0\n",
        )
        .unwrap_or_else(|error| unreachable!("contained provider fixture: {error}"));
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("contained provider mode: {error}"));
        let mut command = Command::new(&tool);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let spawned = spawn_provider_group_with_mode(command, false, true)
            .unwrap_or_else(|error| unreachable!("contained provider spawn: {error}"));
        assert!(matches!(&spawned.process, ProviderProcessV1::Direct(_)));
        let mut guard = ProviderChildGuardV1::new(spawned);
        let status = guard
            .wait_until(
                Instant::now()
                    .checked_add(Duration::from_secs(5))
                    .unwrap_or_else(|| unreachable!("provider deadline")),
            )
            .unwrap_or_else(|error| unreachable!("contained provider wait: {error}"));
        assert!(status.success());
        let child_group = fs::read_to_string(tool.with_extension("pgid"))
            .unwrap_or_else(|error| unreachable!("contained provider pgid: {error}"));
        assert_eq!(
            child_group.trim().parse::<i32>().ok(),
            Some(rustix::process::getpgrp().as_raw_pid())
        );
    }

    #[cfg(unix)]
    #[test]
    fn injected_repair_drain_failure_is_typed_as_containment_uncertain() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("uncertain-repair");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("repair directory: {error}"));
        let worker = protected.join("repair-worker");
        fs::write(
            &worker,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                serde_json::to_string(&NativePostgresRepairResultV1 {
                    schema: NATIVE_POSTGRES_REPAIR_RESULT_SCHEMA_V1.to_owned(),
                    status: "repaired".to_owned(),
                    reason: "native_target_identity_bound_sealed_and_role_absent".to_owned(),
                })
                .unwrap_or_else(|error| unreachable!("repair result fixture: {error}")),
            ),
        )
        .unwrap_or_else(|error| unreachable!("repair worker fixture: {error}"));
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("repair worker mode: {error}"));
        let request = NativePostgresRepairRequestV1 {
            schema: NATIVE_POSTGRES_REPAIR_REQUEST_SCHEMA_V1.to_owned(),
            target: NativePostgresEndpointV1::new(
                "127.0.0.1",
                5432,
                "target",
                "postgres",
                NativePostgresTlsModeV1::Disable,
            )
            .unwrap_or_else(|error| unreachable!("repair endpoint: {error}")),
            operator_passfile: protected.join("operator.pgpass"),
            operator_passfile_identity: NativeWorkerFileIdentityV1 {
                storage_id: "0000000000000001".to_owned(),
                file_id: "00000000000000000000000000000002".to_owned(),
            },
            expected_target: PostgresProviderIdentityV1 {
                system_identifier: "1".to_owned(),
                database_oid: "2".to_owned(),
                database_name: "target".to_owned(),
            },
            target_marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
            role_name: "worldstream_restore_0123456789abcdef0123456789abcdef".to_owned(),
        };
        FORCE_PROVIDER_CONTAINMENT_UNCERTAIN_AFTER_DRAIN_V1
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let completion = run_native_postgres_repair_worker(
            &worker,
            &request,
            &authority,
            Instant::now()
                .checked_add(Duration::from_secs(5))
                .unwrap_or_else(|| unreachable!("repair deadline")),
        )
        .unwrap_or_else(|error| unreachable!("repair supervision: {error}"));
        assert!(matches!(
            completion,
            NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(
                NativePostgresError::ProviderCommand
            )
        ));
    }

    #[test]
    fn containment_uncertainty_skips_cleanup_in_restore_and_recovery_supervisors() {
        let source = include_str!("native_restore.rs");
        let recover_start = source
            .find("pub fn recover_native_postgres_restore(")
            .unwrap_or_else(|| unreachable!("public recovery entry"));
        let recover_end = source[recover_start..]
            .find("fn run_native_postgres_restore_with_worker(")
            .map(|offset| recover_start + offset)
            .unwrap_or_else(|| unreachable!("recovery boundary"));
        let recovery = &source[recover_start..recover_end];
        let uncertain = recovery
            .find("NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(error)")
            .unwrap_or_else(|| unreachable!("typed recovery containment failure"));
        let credential_preserved = recovery[uncertain..]
            .find("pinned_operator.removed = true")
            .map(|offset| uncertain + offset)
            .unwrap_or_else(|| unreachable!("recovery credential preservation"));
        let artifact_scrub = recovery
            .find("scrub_native_recovery_artifact(")
            .unwrap_or_else(|| unreachable!("recovery artifact scrub"));
        assert!(uncertain < credential_preserved && credential_preserved < artifact_scrub);
        assert!(recovery[uncertain..artifact_scrub].contains("return Err(error)"));

        let restore_start = source
            .find("fn run_native_postgres_restore_with_worker(")
            .unwrap_or_else(|| unreachable!("restore supervisor"));
        let restore_end = source[restore_start..]
            .find("fn run_native_postgres_restore_in_worker(")
            .map(|offset| restore_start + offset)
            .unwrap_or_else(|| unreachable!("restore worker boundary"));
        let restore = &source[restore_start..restore_end];
        let uncertain = restore
            .rfind("NativePostgresRepairWorkerCompletionV1::ContainmentUncertain(error)")
            .unwrap_or_else(|| unreachable!("typed restore repair containment failure"));
        let return_error = restore[uncertain..]
            .find("return Err(error)")
            .map(|offset| uncertain + offset)
            .unwrap_or_else(|| unreachable!("restore containment return"));
        assert!(restore[uncertain..return_error].contains("role_passfile.removed = true"));
        assert!(restore[uncertain..return_error].contains("private_dump.retained = true"));
        assert!(restore[uncertain..return_error].contains("recovery_file.preserve()"));
    }

    #[cfg(unix)]
    #[test]
    fn native_worker_deadline_kills_worker_tree_and_preserves_unrelated_child() {
        let _provider_process_test_lock = provider_process_test_lock();

        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel: {error}"));
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "exit 0")
            .unwrap_or_else(|error| unreachable!("fake restore config: {error}"));
        let worker = directory.path().join("hanging-native-worker");
        let leader_path = worker.with_extension("leader");
        let descendant_path = worker.with_extension("descendant");
        let admission = serde_json::to_string(&NativePostgresAdmissionResultV1 {
            schema: NATIVE_POSTGRES_ADMISSION_RESULT_SCHEMA_V1.to_owned(),
            status: "admitted".to_owned(),
            target: PostgresProviderIdentityV1 {
                system_identifier: "1".to_owned(),
                database_oid: "2".to_owned(),
                database_name: config.target.database.clone(),
            },
            target_marker: NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1.to_owned(),
            reason: "native_target_read_only_identity_admission_complete".to_owned(),
        })
        .unwrap_or_else(|error| unreachable!("admission fixture: {error}"));
        let repair = serde_json::to_string(&NativePostgresRepairResultV1 {
            schema: NATIVE_POSTGRES_REPAIR_RESULT_SCHEMA_V1.to_owned(),
            status: "repaired".to_owned(),
            reason: "native_target_identity_bound_sealed_and_role_absent".to_owned(),
        })
        .unwrap_or_else(|error| unreachable!("repair fixture: {error}"));
        fs::write(
            &worker,
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                   {admission_arg}) printf '%s' '{admission}'; exit 0 ;;\n\
                   {repair_arg}) printf '%s' '{repair}'; exit 0 ;;\n\
                 esac\n\
                 printf '%s' \"$$\" > '{leader}'\n\
                 sleep 60 </dev/null >/dev/null 2>&1 &\n\
                 printf '%s' \"$!\" > '{descendant}'\n\
                 wait\n",
                admission_arg = NATIVE_POSTGRES_ADMISSION_ARG_V1,
                repair_arg = NATIVE_POSTGRES_REPAIR_ARG_V1,
                leader = leader_path.display(),
                descendant = descendant_path.display(),
            ),
        )
        .unwrap_or_else(|error| unreachable!("worker fixture: {error}"));
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("worker fixture mode: {error}"));

        let started = Instant::now();
        assert!(matches!(
            run_native_postgres_restore_with_worker(
                &config,
                &config
                    .dump_path
                    .parent()
                    .unwrap_or_else(|| unreachable!("dump parent"))
                    .join("native-report.json"),
                &worker,
                &["worker-fixture"],
                MIN_NATIVE_SUPERVISOR_TIMEOUT_V1,
            ),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(started.elapsed() <= MIN_NATIVE_SUPERVISOR_TIMEOUT_V1);
        for (label, path) in [("leader", leader_path), ("descendant", descendant_path)] {
            let pid = fs::read_to_string(path)
                .unwrap_or_else(|error| unreachable!("{label} pid: {error}"));
            assert!(
                !Command::new("/bin/kill")
                    .args(["-0", pid.trim()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap_or_else(|error| unreachable!("{label} probe: {error}"))
                    .success()
            );
        }
        assert!(
            unrelated
                .try_wait()
                .unwrap_or_else(|error| unreachable!("unrelated sentinel probe: {error}"))
                .is_none()
        );
        unrelated
            .kill()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel cleanup: {error}"));
        unrelated
            .wait()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel reap: {error}"));
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_dump_sink_rejects_symlinks_without_clobbering_the_referent() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("protected");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let victim = protected.join("victim");
        let dump = protected.join("source.dump");
        fs::write(&victim, b"do-not-clobber")
            .unwrap_or_else(|error| unreachable!("victim fixture: {error}"));
        symlink(&victim, &dump)
            .unwrap_or_else(|error| unreachable!("dump symlink fixture: {error}"));

        let rejected = open_exclusive_private_file(&dump);
        assert!(
            matches!(rejected, Err(NativePostgresError::ProviderCommand)),
            "unexpected exclusive-open result: {rejected:?}"
        );
        assert_eq!(
            fs::read(&victim).unwrap_or_else(|error| unreachable!("victim read: {error}")),
            b"do-not-clobber"
        );
    }

    #[test]
    fn artifact_directory_identity_rejects_substitution_before_native_admission() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let requested = parent.path().join("artifact-root");
        prepare_data_directory(&requested)
            .unwrap_or_else(|error| unreachable!("artifact root: {error}"));
        let expected = native_postgres_artifact_directory_identity(&requested)
            .unwrap_or_else(|error| unreachable!("artifact identity: {error}"));
        let original = parent.path().join("original-artifact-root");
        fs::rename(&requested, &original)
            .unwrap_or_else(|error| unreachable!("rename artifact root: {error}"));
        prepare_data_directory(&requested)
            .unwrap_or_else(|error| unreachable!("replacement artifact root: {error}"));
        let sentinel = requested.join("unrelated-sentinel");
        fs::write(&sentinel, b"must-survive")
            .unwrap_or_else(|error| unreachable!("replacement sentinel: {error}"));

        assert!(matches!(
            NativeRestoreDirectoryAuthorityV1::open_expected(&requested, &expected),
            Err(NativePostgresError::Configuration(_))
        ));
        assert_eq!(
            fs::read(&sentinel)
                .unwrap_or_else(|error| unreachable!("replacement sentinel read: {error}")),
            b"must-survive"
        );
        assert!(original.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn committed_receipt_identity_is_derived_from_the_retained_exact_file() {
        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let requested = parent.path().join("receipt-identity-root");
        prepare_data_directory(&requested)
            .unwrap_or_else(|error| unreachable!("artifact root: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&requested)
            .unwrap_or_else(|error| unreachable!("directory authority: {error}"));
        let committed = requested.join("committed.dump");
        let moved = requested.join("moved.dump");
        let bytes = b"identity-bound-artifact";
        let mut exact = open_exclusive_private_file_in(&authority, &committed)
            .unwrap_or_else(|error| unreachable!("committed file: {error}"));
        exact
            .write_all(bytes)
            .and_then(|()| exact.sync_all())
            .unwrap_or_else(|error| unreachable!("committed bytes: {error}"));
        let expected = ExclusiveFileIdentityV1::for_file(&exact)
            .unwrap_or_else(|error| unreachable!("committed identity: {error}"));
        assert_eq!(
            retained_named_file_identity(
                &authority,
                &committed,
                &exact,
                &expected,
                Some(bytes.len() as u64),
            )
            .unwrap_or_else(|error| unreachable!("retained identity: {error}")),
            expected.worker_identity()
        );

        fs::rename(&committed, &moved)
            .unwrap_or_else(|error| unreachable!("move retained file: {error}"));
        let mut replacement = open_exclusive_private_file_in(&authority, &committed)
            .unwrap_or_else(|error| unreachable!("replacement file: {error}"));
        replacement
            .write_all(bytes)
            .and_then(|()| replacement.sync_all())
            .unwrap_or_else(|error| unreachable!("replacement bytes: {error}"));
        assert!(matches!(
            retained_named_file_identity(
                &authority,
                &committed,
                &exact,
                &expected,
                Some(bytes.len() as u64),
            ),
            Err(NativePostgresError::Incomplete)
        ));
        assert_eq!(
            fs::read(&committed)
                .unwrap_or_else(|error| unreachable!("replacement remains intact: {error}")),
            bytes
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn retained_restore_directory_rejects_parent_substitution_for_dump_and_report_publication() {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

        let parent = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let requested = parent.path().join("publication-root");
        prepare_data_directory(&requested)
            .unwrap_or_else(|error| unreachable!("publication root: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&requested)
            .unwrap_or_else(|error| unreachable!("directory authority: {error}"));
        let private_path = requested.join(".worldstream_dump_test.partial");
        let publication_path = requested.join("committed.json");
        let bytes = b"complete-identity-bound-publication";
        let mut file = open_exclusive_dump_file_in(&authority, &private_path)
            .unwrap_or_else(|error| unreachable!("anonymous source: {error}"));
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| unreachable!("source bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(&file)
            .unwrap_or_else(|error| unreachable!("source identity: {error}"));
        let digest = DigestV1::hash(bytes);
        let mut publication = NativeDumpFileV1 {
            directory: Some(authority),
            file: Some(file),
            path: private_path,
            publication_path: publication_path.clone(),
            identity,
            digest: Some(digest),
            byte_count: Some(bytes.len() as u64),
            retained: false,
        };

        let original = parent.path().join("true-publication-root");
        fs::rename(&requested, &original)
            .unwrap_or_else(|error| unreachable!("rename retained root: {error}"));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&requested)
            .unwrap_or_else(|error| unreachable!("replacement root: {error}"));
        fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("replacement permissions: {error}"));
        let sentinel = requested.join("victim-sentinel");
        fs::write(&sentinel, b"must-survive")
            .unwrap_or_else(|error| unreachable!("replacement sentinel: {error}"));

        assert!(matches!(
            publication.retain(),
            Err(NativePostgresError::Incomplete) | Err(NativePostgresError::ProviderCommand)
        ));
        assert!(!publication_path.exists());
        assert_eq!(
            fs::read(&sentinel)
                .unwrap_or_else(|error| unreachable!("replacement sentinel read: {error}")),
            b"must-survive"
        );
        drop(publication);
        assert_eq!(
            fs::metadata(original.join("committed.json"))
                .unwrap_or_else(|error| unreachable!("scrubbed retained publication: {error}"))
                .len(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_scrub_uses_retained_handle_after_path_replacement() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("private-scrub");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let original = protected.join("credential");
        let renamed = protected.join("renamed-credential");
        let mut file = Some(
            open_exclusive_private_file(&original)
                .unwrap_or_else(|error| unreachable!("private file: {error}")),
        );
        file.as_mut()
            .unwrap_or_else(|| unreachable!("private handle"))
            .write_all(b"secret-material")
            .unwrap_or_else(|error| unreachable!("private bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(
            file.as_ref()
                .unwrap_or_else(|| unreachable!("private handle")),
        )
        .unwrap_or_else(|error| unreachable!("private identity: {error}"));
        fs::rename(&original, &renamed)
            .unwrap_or_else(|error| unreachable!("rename private file: {error}"));
        fs::write(&original, b"replacement-must-survive")
            .unwrap_or_else(|error| unreachable!("replacement fixture: {error}"));

        scrub_private_file(&mut file, &identity, &original)
            .unwrap_or_else(|error| unreachable!("retained-handle scrub: {error}"));
        assert_eq!(
            fs::metadata(&renamed)
                .unwrap_or_else(|error| unreachable!("renamed metadata: {error}"))
                .len(),
            0
        );
        assert_eq!(
            fs::read(&original).unwrap_or_else(|error| unreachable!("replacement read: {error}")),
            b"replacement-must-survive"
        );
    }

    #[test]
    fn failed_recovery_record_preservation_never_truncates_the_only_record() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("native-recovery");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let record_path = protected.join("recovery.json");
        let mut record = create_native_recovery_record_file(record_path.clone())
            .unwrap_or_else(|error| unreachable!("recovery record: {error}"));
        let recovery_bytes = b"{\"schema\":\"native-recovery-test-v1\"}\n";
        record
            .file
            .as_mut()
            .unwrap_or_else(|| unreachable!("recovery handle"))
            .write_all(recovery_bytes)
            .unwrap_or_else(|error| unreachable!("recovery bytes: {error}"));

        assert!(matches!(
            record.preserve_with_sync(|_| Err(std::io::Error::other("injected sync fault"))),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(record.removed);
        // The unconditional supervisor epilogue is now harmless after a
        // preservation fault, and Drop must only close the retained handle.
        record
            .remove()
            .unwrap_or_else(|error| unreachable!("disarmed recovery cleanup: {error}"));
        drop(record);
        assert_eq!(
            fs::read(&record_path)
                .unwrap_or_else(|error| unreachable!("retained recovery record: {error}")),
            recovery_bytes
        );
    }

    #[test]
    fn durable_recovery_record_survives_unwind_after_the_spawn_boundary() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("panic-native-recovery");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let recovery_path = protected.join("recovery.json");
        let (dump, digest) = test_published_dump(&protected, "panic-recovery-dump");
        let mut initial = test_recovery_record(&protected, "panic-recovery", &dump, &digest);
        "repair_required_if_primary_interrupted".clone_into(&mut initial.disposition);
        initial.native_dump_digest = None;
        initial.native_dump_size_bytes = None;
        "keep_target_non_serving_and_run_identity_bound_native_repair_with_fresh_credentials"
            .clone_into(&mut initial.operator_action);
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut recovery = create_native_recovery_record_file(recovery_path.clone())
                .unwrap_or_else(|error| unreachable!("recovery record: {error}"));
            write_durable_native_recovery_record(&initial, &mut recovery)
                .unwrap_or_else(|error| unreachable!("initial recovery generation: {error}"));
            recovery
                .preserve_on_drop()
                .unwrap_or_else(|error| unreachable!("arm recovery preservation: {error}"));
            std::panic::resume_unwind(Box::new("injected post-spawn unwind"));
        }));
        assert!(unwind.is_err());
        let mut recovery = open_existing_native_recovery_record_file(&recovery_path)
            .unwrap_or_else(|error| unreachable!("retained recovery record: {error}"));
        let observed = read_native_recovery_record(
            recovery
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("recovery handle")),
        )
        .unwrap_or_else(|error| unreachable!("actionable recovery record: {error}"));
        assert_eq!(observed, initial);
    }

    #[test]
    fn private_scrub_failure_retains_the_exact_handle_for_retry() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("retry-private-scrub");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let original = protected.join("credential");
        let renamed = protected.join("renamed-credential");
        let mut file = Some(
            open_exclusive_private_file(&original)
                .unwrap_or_else(|error| unreachable!("private file: {error}")),
        );
        file.as_mut()
            .unwrap_or_else(|| unreachable!("private handle"))
            .write_all(b"secret-material")
            .unwrap_or_else(|error| unreachable!("private bytes: {error}"));
        let identity = ExclusiveFileIdentityV1::for_file(
            file.as_ref()
                .unwrap_or_else(|| unreachable!("private handle")),
        )
        .unwrap_or_else(|error| unreachable!("private identity: {error}"));
        fs::rename(&original, &renamed)
            .unwrap_or_else(|error| unreachable!("rename private file: {error}"));
        fs::write(&original, b"replacement-must-survive")
            .unwrap_or_else(|error| unreachable!("replacement fixture: {error}"));

        assert!(matches!(
            scrub_private_file_with(&mut file, &identity, &original, |_| {
                Err(std::io::Error::other("injected truncate failure"))
            }),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(file.is_some());
        assert_eq!(
            fs::read(&renamed).unwrap_or_else(|error| unreachable!("secret remains: {error}")),
            b"secret-material"
        );
        scrub_private_file(&mut file, &identity, &original)
            .unwrap_or_else(|error| unreachable!("scrub retry: {error}"));
        assert!(file.is_none());
        assert_eq!(
            fs::metadata(&renamed)
                .unwrap_or_else(|error| unreachable!("renamed metadata: {error}"))
                .len(),
            0
        );
        assert_eq!(
            fs::read(&original).unwrap_or_else(|error| unreachable!("replacement read: {error}")),
            b"replacement-must-survive"
        );
    }

    #[test]
    fn torn_recovery_tail_is_durably_removed_before_a_completion_append() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("torn-native-recovery");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let recovery_path = protected.join("recovery.json");
        let mut recovery = create_native_recovery_record_file(recovery_path.clone())
            .unwrap_or_else(|error| unreachable!("recovery record: {error}"));
        let (dump, digest) = test_published_dump(&protected, "torn-recovery-dump");
        let mut initial = test_recovery_record(&protected, "torn-recovery", &dump, &digest);
        "repair_required_if_primary_interrupted".clone_into(&mut initial.disposition);
        initial.native_dump_digest = None;
        initial.native_dump_size_bytes = None;
        "keep_target_non_serving_and_run_identity_bound_native_repair_with_fresh_credentials"
            .clone_into(&mut initial.operator_action);
        write_durable_native_recovery_record(&initial, &mut recovery)
            .unwrap_or_else(|error| unreachable!("initial recovery generation: {error}"));
        let exact_initial_length = recovery
            .file
            .as_ref()
            .unwrap_or_else(|| unreachable!("recovery handle"))
            .metadata()
            .unwrap_or_else(|error| unreachable!("initial recovery metadata: {error}"))
            .len();
        recovery
            .file
            .as_mut()
            .unwrap_or_else(|| unreachable!("recovery handle"))
            .seek(SeekFrom::End(0))
            .and_then(|_| {
                recovery
                    .file
                    .as_mut()
                    .unwrap_or_else(|| unreachable!("recovery handle"))
                    .write_all(b"{\"schema\":\"torn")
            })
            .and_then(|()| {
                recovery
                    .file
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("recovery handle"))
                    .sync_all()
            })
            .unwrap_or_else(|error| unreachable!("torn recovery tail: {error}"));

        let observed = read_native_recovery_record(
            recovery
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("recovery handle")),
        )
        .unwrap_or_else(|error| unreachable!("repair torn recovery tail: {error}"));
        assert_eq!(observed, initial);
        assert_eq!(
            recovery
                .file
                .as_ref()
                .unwrap_or_else(|| unreachable!("recovery handle"))
                .metadata()
                .unwrap_or_else(|error| unreachable!("trimmed recovery metadata: {error}"))
                .len(),
            exact_initial_length
        );

        let mut completed = initial;
        "recovery_completed_artifacts_scrubbed".clone_into(&mut completed.disposition);
        "target_repaired_and_recorded_artifacts_scrubbed"
            .clone_into(&mut completed.operator_action);
        write_durable_native_recovery_record(&completed, &mut recovery)
            .unwrap_or_else(|error| unreachable!("completion generation: {error}"));
        let final_record = read_native_recovery_record(
            recovery
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("recovery handle")),
        )
        .unwrap_or_else(|error| unreachable!("final recovery record: {error}"));
        assert_eq!(final_record, completed);
        assert!(
            fs::read(&recovery_path)
                .unwrap_or_else(|error| unreachable!("recovery bytes: {error}"))
                .ends_with(b"\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_a_swapped_journal_identity_before_target_repair() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("identity-bound-recovery");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("directory authority: {error}"));
        let recovery_path =
            protected.join(".worldstream_native_recovery_00000000000000000000000000000000.json");
        let mut recovery = open_exclusive_private_file_in(&authority, &recovery_path)
            .unwrap_or_else(|error| unreachable!("recovery journal: {error}"));
        recovery
            .write_all(b"must-not-be-read-or-scrubbed")
            .and_then(|()| recovery.sync_all())
            .unwrap_or_else(|error| unreachable!("recovery bytes: {error}"));
        let mut substituted_identity = ExclusiveFileIdentityV1::for_file(&recovery)
            .unwrap_or_else(|error| unreachable!("recovery identity: {error}"))
            .worker_identity();
        substituted_identity.file_id.replace_range(
            ..1,
            if substituted_identity.file_id.starts_with('0') {
                "1"
            } else {
                "0"
            },
        );
        let result = open_expected_native_recovery_record_file_in(
            authority.clone(),
            &recovery_path,
            &substituted_identity,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => unreachable!("substituted recovery identity was accepted"),
        };
        assert!(matches!(
            error,
            NativePostgresError::Configuration(
                "native recovery journal identity changed before repair admission"
            )
        ));
        assert_eq!(
            fs::read(&recovery_path)
                .unwrap_or_else(|error| unreachable!("preserved recovery bytes: {error}")),
            b"must-not-be-read-or-scrubbed"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn anonymous_recovery_secret_never_scrubs_a_reused_proc_fd() {
        use std::os::fd::AsRawFd as _;

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("anonymous-recovery-secret");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("directory authority: {error}"));
        let sentinel_path = protected.join("unrelated-sentinel");
        let mut sentinel = open_exclusive_private_file(&sentinel_path)
            .unwrap_or_else(|error| unreachable!("sentinel: {error}"));
        sentinel
            .write_all(b"unrelated-bytes-must-survive")
            .and_then(|()| sentinel.sync_all())
            .unwrap_or_else(|error| unreachable!("sentinel bytes: {error}"));
        let recycled_coordinate = PathBuf::from(format!("/proc/self/fd/{}", sentinel.as_raw_fd()));
        let recycled_identity = ExclusiveFileIdentityV1::for_file(&sentinel)
            .unwrap_or_else(|error| unreachable!("sentinel identity: {error}"))
            .worker_identity();

        scrub_native_recovery_secret(&authority, &recycled_coordinate, &recycled_identity, true)
            .unwrap_or_else(|error| unreachable!("anonymous secret retirement: {error}"));
        assert_eq!(
            fs::read(&sentinel_path)
                .unwrap_or_else(|error| unreachable!("sentinel reread: {error}")),
            b"unrelated-bytes-must-survive"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn retained_procfd_pgpassfile_is_admitted_for_every_native_entry_path() {
        use std::os::fd::AsRawFd as _;

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let passfile_path = directory.path().join("operator.pgpass");
        let mut passfile = open_exclusive_private_file(&passfile_path)
            .unwrap_or_else(|error| unreachable!("operator passfile: {error}"));
        passfile
            .write_all(b"127.0.0.1:5432:worldstream:postgres:retained-secret\n")
            .and_then(|()| passfile.sync_all())
            .unwrap_or_else(|error| unreachable!("operator passfile bytes: {error}"));
        let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", passfile.as_raw_fd()));
        let endpoint = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("endpoint: {error}"));

        // `password_for` is the shared credential admission used by snapshot
        // rebuild, restore admission, and explicit recovery.
        assert_eq!(
            password_for(&endpoint, &descriptor_path)
                .unwrap_or_else(|error| unreachable!("retained credential: {error}")),
            "retained-secret"
        );
        let protected = directory.path().join("native-artifacts");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("artifact directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("artifact authority: {error}"));
        let mut pinned = pin_pgpassfile_in(&authority, &descriptor_path)
            .unwrap_or_else(|error| unreachable!("pinned retained credential: {error}"));
        assert_eq!(
            password_for(&endpoint, &pinned.path)
                .unwrap_or_else(|error| unreachable!("pinned credential selection: {error}")),
            "retained-secret"
        );
        pinned
            .remove()
            .unwrap_or_else(|error| unreachable!("pinned credential close: {error}"));

        assert!(!linux_inherited_fd_path(Path::new("/proc/self/fd/01")));
        assert!(!linux_inherited_fd_path(Path::new("/proc/self/fd/-1")));
        assert!(!linux_inherited_fd_path(Path::new("/proc/self/fd/1/extra")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn provider_passfile_is_anonymous_sealed_and_identity_bound() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("anonymous-provider-passfile");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let endpoint = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "worldstream_restore_0123456789abcdef0123456789abcdef",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("endpoint: {error}"));
        let mut passfile = create_ephemeral_pgpassfile(&protected, &endpoint, "secret-value")
            .unwrap_or_else(|error| unreachable!("anonymous passfile: {error}"));
        assert_eq!(passfile.binding, PrivateFileBindingV1::Anonymous);
        assert!(linux_inherited_fd_path(&passfile.path));
        assert!(
            fs::read_dir(&protected)
                .unwrap_or_else(|error| unreachable!("protected directory: {error}"))
                .next()
                .is_none()
        );
        let exact = open_worker_owned_file(&passfile.path, &passfile.identity.worker_identity())
            .unwrap_or_else(|error| unreachable!("exact inherited passfile: {error}"));
        assert_eq!(
            password_for_reader(&endpoint, exact)
                .unwrap_or_else(|error| unreachable!("passfile selection: {error}")),
            "secret-value"
        );
        assert!(
            passfile
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("passfile handle"))
                .write_all(b"forged")
                .is_err()
        );
        passfile
            .remove()
            .unwrap_or_else(|error| unreachable!("anonymous passfile close: {error}"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_worker_transports_are_anonymous_and_sealed_before_acceptance() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let requested_path = directory.path().join("must-never-exist.json");
        let mut transport = create_native_worker_transport_file(requested_path.clone())
            .unwrap_or_else(|error| unreachable!("anonymous transport: {error}"));
        assert_eq!(transport.binding, PrivateFileBindingV1::Anonymous);
        assert!(!requested_path.exists());
        transport
            .file
            .as_mut()
            .unwrap_or_else(|| unreachable!("transport handle"))
            .write_all(b"canonical")
            .unwrap_or_else(|error| unreachable!("transport bytes: {error}"));
        transport
            .seal_anonymous_transport()
            .unwrap_or_else(|error| unreachable!("seal transport: {error}"));
        assert!(
            transport
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("transport handle"))
                .write_all(b"forged")
                .is_err()
        );
        let handle = transport
            .file
            .as_mut()
            .unwrap_or_else(|| unreachable!("transport handle"));
        handle
            .seek(SeekFrom::Start(0))
            .unwrap_or_else(|error| unreachable!("transport rewind: {error}"));
        let mut observed = Vec::new();
        handle
            .read_to_end(&mut observed)
            .unwrap_or_else(|error| unreachable!("transport read: {error}"));
        assert_eq!(observed, b"canonical");
    }

    #[test]
    fn contained_report_commit_publishes_exact_bytes_and_appends_acknowledgement() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("publication-commit");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("publication directory: {error}"));
        let recovery_path = protected.join("recovery.json");
        let mut recovery = create_native_recovery_record_file(recovery_path.clone())
            .unwrap_or_else(|error| unreachable!("recovery record: {error}"));
        let report = minimal_ready_native_worker_report();
        let dump_digest = DigestV1::parse(report.native_dump_digest.clone())
            .unwrap_or_else(|error| unreachable!("dump digest: {error}"));
        let restored_global_digest = DigestV1::hash(b"restored-global");
        let (dump, observed_dump_digest) = test_published_dump(&protected, "native-dump");
        assert_eq!(observed_dump_digest, dump_digest);
        let committed_record = test_recovery_record(&protected, "record", &dump, &dump_digest);
        let mut initial_record = committed_record.clone();
        "repair_required_if_primary_interrupted".clone_into(&mut initial_record.disposition);
        initial_record.native_dump_digest = None;
        initial_record.native_dump_size_bytes = None;
        "repair_target_before_artifact_cleanup".clone_into(&mut initial_record.operator_action);
        write_durable_native_recovery_record(&initial_record, &mut recovery)
            .unwrap_or_else(|error| unreachable!("initial recovery record: {error}"));
        write_durable_native_recovery_record(&committed_record, &mut recovery)
            .unwrap_or_else(|error| unreachable!("committed recovery record: {error}"));

        let staging_path = protected.join("report.private");
        let mut staging =
            create_native_commit_staging_file_in(authority.clone(), staging_path.clone())
                .unwrap_or_else(|error| unreachable!("report staging: {error}"));
        let report_path = protected.join("report.json");
        let request = NativePostgresCommitRequestV1 {
            schema: NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1.to_owned(),
            phase: NativePostgresCommitPhaseV1::ReportAndAcknowledge,
            artifact_directory_path: protected.clone(),
            artifact_directory_identity: authority.identity.clone(),
            recovery_path: recovery_path.clone(),
            recovery_identity: recovery.identity.worker_identity(),
            recovery_record: committed_record,
            report_staging_path: staging_path,
            report_staging_identity: staging.identity.worker_identity(),
            report_path: report_path.clone(),
            report: report.clone(),
            restored_global_digest: restored_global_digest.as_str().to_owned(),
        };
        let request_bytes = serde_json::to_vec(&request)
            .unwrap_or_else(|error| unreachable!("commit request: {error}"));
        staging
            .file
            .as_mut()
            .unwrap_or_else(|| unreachable!("staging handle"))
            .write_all(&request_bytes)
            .and_then(|()| {
                staging
                    .file
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("staging handle"))
                    .sync_all()
            })
            .unwrap_or_else(|error| unreachable!("commit request bytes: {error}"));
        execute_contained_native_postgres_commit_worker(
            staging
                .file
                .as_ref()
                .unwrap_or_else(|| unreachable!("staging handle"))
                .try_clone()
                .unwrap_or_else(|error| unreachable!("staging clone: {error}")),
            recovery
                .file
                .as_ref()
                .unwrap_or_else(|| unreachable!("recovery handle"))
                .try_clone()
                .unwrap_or_else(|error| unreachable!("recovery clone: {error}")),
            dump.file
                .as_ref()
                .unwrap_or_else(|| unreachable!("dump handle"))
                .try_clone()
                .unwrap_or_else(|error| unreachable!("dump clone: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("contained report commit: {error}"));
        let mut expected_report = serde_json::to_vec(&report)
            .unwrap_or_else(|error| unreachable!("report bytes: {error}"));
        expected_report.push(b'\n');
        assert_eq!(
            fs::read(&report_path).unwrap_or_else(|error| unreachable!("durable report: {error}")),
            expected_report
        );
        let final_record = read_native_recovery_record(
            recovery
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("recovery handle")),
        )
        .unwrap_or_else(|error| unreachable!("recovery tombstone: {error}"));
        assert_eq!(final_record.disposition, "publication_acknowledged");
        staging.removed = true;
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::too_many_lines)]
    fn contained_commit_deadline_preserves_journal_and_prevents_late_publication() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected = directory.path().join("deadline-contained-commit");
        prepare_data_directory(&protected)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let authority = NativeRestoreDirectoryAuthorityV1::open(&protected)
            .unwrap_or_else(|error| unreachable!("publication directory: {error}"));
        let mut report = minimal_ready_native_worker_report();
        "verified_pending_supervisor".clone_into(&mut report.status);
        "native_worker_verified_pending_containment_repair_and_publication"
            .clone_into(&mut report.reason);
        "verified_pending_publication".clone_into(&mut report.native_dump_restore);
        report.native_witness_minted = false;
        let restored_global_digest = DigestV1::hash(b"restored-global");
        let dump_digest = DigestV1::parse(report.native_dump_digest.clone())
            .unwrap_or_else(|error| unreachable!("dump digest: {error}"));
        let private_dump_path = protected.join("dump.private");
        let publication_path = protected.join("dump.public");
        let mut dump_file = open_exclusive_dump_file_in(&authority, &private_dump_path)
            .unwrap_or_else(|error| unreachable!("private dump: {error}"));
        dump_file
            .write_all(b"native-dump")
            .and_then(|()| dump_file.sync_all())
            .unwrap_or_else(|error| unreachable!("private dump bytes: {error}"));
        let dump_identity = ExclusiveFileIdentityV1::for_file(&dump_file)
            .unwrap_or_else(|error| unreachable!("private dump identity: {error}"));
        let dump = NativeDumpFileV1 {
            directory: Some(authority.clone()),
            file: Some(dump_file),
            path: private_dump_path.clone(),
            publication_path: publication_path.clone(),
            identity: dump_identity,
            digest: Some(dump_digest.clone()),
            byte_count: Some(11),
            retained: false,
        };
        let recovery_path = protected.join("recovery.json");
        let mut recovery = create_native_recovery_record_file(recovery_path.clone())
            .unwrap_or_else(|error| unreachable!("recovery journal: {error}"));
        let mut initial_record = test_recovery_record(&protected, "deadline", &dump, &dump_digest);
        "repair_required_if_primary_interrupted".clone_into(&mut initial_record.disposition);
        initial_record.native_dump_digest = None;
        initial_record.native_dump_size_bytes = None;
        "repair_target_before_artifact_cleanup".clone_into(&mut initial_record.operator_action);
        write_durable_native_recovery_record(&initial_record, &mut recovery)
            .unwrap_or_else(|error| unreachable!("initial recovery generation: {error}"));
        let report_staging_path = protected.join("report.private");
        let staging =
            create_native_commit_staging_file_in(authority.clone(), report_staging_path.clone())
                .unwrap_or_else(|error| unreachable!("report staging: {error}"));
        let report_path = protected.join("report.json");
        let request = NativePostgresCommitRequestV1 {
            schema: NATIVE_POSTGRES_COMMIT_REQUEST_SCHEMA_V1.to_owned(),
            phase: NativePostgresCommitPhaseV1::Dump,
            artifact_directory_path: protected.clone(),
            artifact_directory_identity: authority.identity.clone(),
            recovery_path: recovery_path.clone(),
            recovery_identity: recovery.identity.worker_identity(),
            recovery_record: initial_record.clone(),
            report_staging_path,
            report_staging_identity: staging.identity.worker_identity(),
            report_path: report_path.clone(),
            report,
            restored_global_digest: restored_global_digest.as_str().to_owned(),
        };

        let worker = protected.join("hanging-commit-worker");
        let leader_path = protected.join("commit.leader");
        let descendant_path = protected.join("commit.descendant");
        fs::write(
            &worker,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nsleep 60 </dev/null >/dev/null 2>&1 &\nprintf '%s' \"$!\" > '{}'\nwait\n",
                leader_path.display(),
                descendant_path.display(),
            ),
        )
        .unwrap_or_else(|error| unreachable!("commit worker fixture: {error}"));
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("commit worker mode: {error}"));
        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel: {error}"));
        let started = Instant::now();
        let completion = run_native_postgres_commit_worker(
            &worker,
            &request,
            staging
                .file
                .as_ref()
                .unwrap_or_else(|| unreachable!("staging handle")),
            recovery
                .file
                .as_ref()
                .unwrap_or_else(|| unreachable!("recovery handle")),
            dump.file
                .as_ref()
                .unwrap_or_else(|| unreachable!("dump handle")),
            Instant::now()
                // Leave enough scheduling room for the worker to publish both
                // process witnesses on a loaded CI host. The 60-second fixture
                // still guarantees this exercises the deadline/drain path.
                .checked_add(Duration::from_secs(2))
                .unwrap_or_else(|| unreachable!("commit deadline")),
        )
        .unwrap_or_else(|error| unreachable!("drained commit timeout: {error}"));
        assert!(matches!(
            completion,
            ProviderChildCompletionV1::TimedOutAfterDrain(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(100));
        assert!(!publication_path.exists());
        assert!(!report_path.exists());
        let observed_record = read_native_recovery_record(
            recovery
                .file
                .as_mut()
                .unwrap_or_else(|| unreachable!("recovery handle")),
        )
        .unwrap_or_else(|error| unreachable!("preserved recovery journal: {error}"));
        assert_eq!(observed_record, initial_record);
        for path in [leader_path, descendant_path] {
            let pid = fs::read_to_string(&path)
                .unwrap_or_else(|error| unreachable!("contained pid: {error}"));
            assert!(
                !Command::new("/bin/kill")
                    .args(["-0", pid.trim()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap_or_else(|error| unreachable!("contained process probe: {error}"))
                    .success()
            );
        }
        assert!(
            unrelated
                .try_wait()
                .unwrap_or_else(|error| unreachable!("unrelated process probe: {error}"))
                .is_none()
        );
        unrelated
            .kill()
            .and_then(|()| unrelated.wait().map(|_| ()))
            .unwrap_or_else(|error| unreachable!("unrelated process cleanup: {error}"));
    }

    #[cfg(unix)]
    #[test]
    fn dump_overflow_terminates_and_reaps_the_provider_child_and_scrubs_the_sink() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(
            directory.path(),
            "printf '%s' \"$$\" > \"$0.pid\"\n\
             while :; do printf '0123456789abcdef0123456789abcdef'; done",
        )
        .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));

        let result = stream_native_dump(&config, 32, "00000001-1");
        let error = match result {
            Ok(_) => unreachable!("overflow unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(
            matches!(error, NativePostgresError::Incomplete),
            "overflow result: {error:?}"
        );
        assert!(!config.dump_path.exists());
        let pid = fs::read_to_string(config.pg_dump.with_extension("pid"))
            .unwrap_or_else(|error| unreachable!("provider pid: {error}"));
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap_or_else(|error| unreachable!("process probe: {error}"))
                .success()
        );
        let scrubbed = fs::read_dir(
            config
                .dump_path
                .parent()
                .unwrap_or_else(|| unreachable!("dump parent")),
        )
        .unwrap_or_else(|error| unreachable!("scrubbed dump directory: {error}"))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".worldstream_dump_")
        })
        .collect::<Vec<_>>();
        assert_eq!(scrubbed.len(), 1);
        assert_eq!(
            scrubbed[0]
                .metadata()
                .unwrap_or_else(|error| unreachable!("scrubbed dump metadata: {error}"))
                .len(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn dump_deadline_does_not_join_a_detached_inherited_stdout_holder() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(
            directory.path(),
            "/usr/bin/python3 -c 'import os,sys,time; os.setsid(); open(sys.argv[1], \"w\").write(str(os.getpid())); time.sleep(60)' \"$0.detached\" &\n\
             while [ ! -s \"$0.detached\" ]; do sleep 0.05; done\n\
             exit 0",
        )
        .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));

        let started = Instant::now();
        assert!(matches!(
            stream_native_dump_with_timeout(&config, 32, "00000001-1", Duration::from_secs(10),),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(started.elapsed() < Duration::from_secs(14));
        assert!(!config.dump_path.exists());

        let pid = fs::read_to_string(config.pg_dump.with_extension("detached"))
            .unwrap_or_else(|error| unreachable!("detached pid: {error}"));
        let _ = Command::new("/bin/kill")
            .args(["-TERM", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(unix)]
    #[test]
    fn continuous_dump_output_cannot_extend_the_deadline() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(
            directory.path(),
            "printf '%s' \"$$\" > \"$0.pid\"\n\
             while :; do printf x; done",
        )
        .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));

        let started = Instant::now();
        assert!(matches!(
            stream_native_dump_with_timeout(
                &config,
                u64::MAX,
                "00000001-1",
                Duration::from_secs(2),
            ),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(!config.dump_path.exists());
        let pid = fs::read_to_string(config.pg_dump.with_extension("pid"))
            .unwrap_or_else(|error| unreachable!("provider pid: {error}"));
        let status = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output();
        assert!(
            status.is_err() || {
                let state = String::from_utf8_lossy(
                    &status
                        .unwrap_or_else(|error| unreachable!("provider probe: {error}"))
                        .stdout,
                )
                .trim()
                .to_owned();
                state.is_empty() || state.starts_with('Z')
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn dump_stream_retains_only_a_successful_bounded_exclusive_artifact() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "printf 'native-dump'")
            .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));
        let mut dump = stream_native_dump(&config, 11, "00000001-1")
            .unwrap_or_else(|error| unreachable!("bounded dump: {error}"));

        assert_eq!(dump.digest, Some(DigestV1::hash(b"native-dump")));
        assert!(!config.dump_path.exists());
        assert!(dump.path.exists());
        dump.retain()
            .unwrap_or_else(|error| unreachable!("retain dump: {error}"));
        drop(dump);
        assert_eq!(
            fs::read(&config.dump_path)
                .unwrap_or_else(|error| unreachable!("retained dump: {error}")),
            b"native-dump"
        );
        assert!(matches!(
            stream_native_dump(&config, 11, "00000001-1"),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert_eq!(
            fs::read(&config.dump_path)
                .unwrap_or_else(|error| unreachable!("unchanged dump: {error}")),
            b"native-dump"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unpublished_parent_owned_dump_is_scrubbed_through_its_retained_handle() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "printf 'native-dump'")
            .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));
        let dump = stream_native_dump(&config, 11, "00000001-1")
            .unwrap_or_else(|error| unreachable!("bounded dump: {error}"));
        let private_path = dump.path.clone();

        assert!(!config.dump_path.exists());
        drop(dump);
        assert_eq!(
            fs::metadata(&private_path)
                .unwrap_or_else(|error| unreachable!("scrubbed private dump: {error}"))
                .len(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_parent_sync_failure_leaves_only_an_empty_protected_publication() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "printf 'native-dump'")
            .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));
        let mut dump = stream_native_dump(&config, 11, "00000001-1")
            .unwrap_or_else(|error| unreachable!("bounded dump: {error}"));
        let digest = dump
            .digest
            .clone()
            .unwrap_or_else(|| unreachable!("dump digest"));

        assert!(matches!(
            dump.retain_unix_with_fault(&digest, 11, Some(UnixPublicationFaultV1::ParentSync)),
            Err(NativePostgresError::ProviderCommand)
        ));
        drop(dump);
        assert_eq!(
            fs::metadata(&config.dump_path)
                .unwrap_or_else(|error| unreachable!("empty publication: {error}"))
                .len(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsuccessful_dump_child_removes_the_untrusted_partial_artifact() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let config = fake_dump_config(directory.path(), "printf 'partial'; exit 7")
            .unwrap_or_else(|error| unreachable!("fake dump config: {error}"));

        assert!(matches!(
            stream_native_dump(&config, 64, "00000001-1"),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(!config.dump_path.exists());
    }

    #[test]
    fn restore_identity_material_is_random_and_restricted_to_safe_sql_tokens() {
        let first = random_hex(32)
            .unwrap_or_else(|error| unreachable!("first restore credential: {error}"));
        let second = random_hex(32)
            .unwrap_or_else(|error| unreachable!("second restore credential: {error}"));

        assert_eq!(first.len(), 64);
        assert_eq!(second.len(), 64);
        assert_ne!(first, second);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(second.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(pgpass_field("host:name\\socket"), "host\\:name\\\\socket");
    }

    #[test]
    fn pgpass_parser_matches_libpq_escapes_comments_wildcards_and_first_match() {
        let lines = [
            "# ignored",
            "",
            r"\:\:1:5432:worldstream:postgres:first\:secret\\suffix",
            r"*:5432:worldstream:postgres:second",
        ];
        let entries = lines
            .into_iter()
            .map(parse_pgpass_line)
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, NativePostgresError>>()
            .unwrap_or_else(|error| unreachable!("parse pgpass: {error}"));
        let endpoint = NativePostgresEndpointV1::new(
            "::1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("IPv6 endpoint: {error}"));
        assert_eq!(
            first_matching_pgpass_password(&entries, &endpoint, &endpoint.host).as_deref(),
            Some("first:secret\\suffix")
        );
        assert!(parse_pgpass_line("host:5432:db:user:secret\\").is_err());
        assert!(parse_pgpass_line("host:5432:db:user:secret:extra").is_err());
    }

    #[test]
    fn unix_socket_pgpass_requires_exact_and_localhost_to_select_same_secret() {
        let endpoint = NativePostgresEndpointV1::new(
            "/var/run/postgresql",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("socket endpoint: {error}"));
        let wildcard = parse_pgpass_line("*:*:*:*:shared")
            .unwrap_or_else(|error| unreachable!("wildcard parse: {error}"))
            .unwrap_or_else(|| unreachable!("wildcard entry"));
        assert_eq!(
            first_matching_pgpass_password(
                std::slice::from_ref(&wildcard),
                &endpoint,
                &endpoint.host
            ),
            first_matching_pgpass_password(std::slice::from_ref(&wildcard), &endpoint, "localhost")
        );
        let exact = parse_pgpass_line("/var/run/postgresql:5432:worldstream:postgres:exact")
            .unwrap_or_else(|error| unreachable!("exact parse: {error}"))
            .unwrap_or_else(|| unreachable!("exact entry"));
        let localhost = parse_pgpass_line("localhost:5432:worldstream:postgres:different")
            .unwrap_or_else(|error| unreachable!("localhost parse: {error}"))
            .unwrap_or_else(|| unreachable!("localhost entry"));
        assert_ne!(
            first_matching_pgpass_password(std::slice::from_ref(&exact), &endpoint, &endpoint.host),
            first_matching_pgpass_password(
                std::slice::from_ref(&localhost),
                &endpoint,
                "localhost"
            )
        );
    }

    #[test]
    fn destructive_restore_uses_one_use_identity_and_never_gives_pg_dump_a_path() {
        let source = include_str!("native_restore.rs");
        let start = source
            .find("fn native_dump_restore(")
            .unwrap_or_else(|| unreachable!("native dump restore function"));
        let end = source[start..]
            .find("struct ProviderChildGuardV1")
            .map(|offset| start + offset)
            .unwrap_or_else(|| unreachable!("provider child guard boundary"));
        let body = &source[start..end];

        assert!(body.contains("create_restore_credential"));
        assert!(body.contains("verify_restore_credential"));
        assert!(body.contains("credential.passfile_path"));
        assert!(!body.contains("&config.target,"));
        assert!(!source.contains("\"--file\""));
        assert!(source.contains("NOSUPERUSER NOCREATEDB NOCREATEROLE"));
        assert!(source.contains("CONNECTION LIMIT 1"));
        assert!(source.contains("ALTER ROLE {role} NOLOGIN PASSWORD NULL"));
    }

    #[test]
    fn source_dump_is_snapshot_bound_and_publication_follows_seal() {
        assert!(source_snapshot_is_safe("00000003-0000001A-1"));
        assert!(!source_snapshot_is_safe("snapshot with whitespace"));
        let source = include_str!("native_restore.rs");
        let dump_start = source
            .find("fn native_dump_restore(")
            .unwrap_or_else(|| unreachable!("native dump restore function"));
        let dump_end = source[dump_start..]
            .find("struct ProviderChildGuardV1")
            .map(|offset| dump_start + offset)
            .unwrap_or_else(|| unreachable!("provider guard boundary"));
        let dump_body = &source[dump_start..dump_end];
        let export = dump_body
            .find("export_source_snapshot()")
            .unwrap_or_else(|| unreachable!("source snapshot export"));
        let stream = dump_body
            .find("stream_native_dump_to_writer_with_timeout(")
            .unwrap_or_else(|| unreachable!("snapshot-bound pg_dump"));
        let release = dump_body
            .find("release_source_snapshot()")
            .unwrap_or_else(|| unreachable!("source snapshot release"));
        assert!(export < stream && stream < release);
        assert!(source.contains(".arg(format!(\"--snapshot={source_snapshot}\"))"));

        let run_start = source
            .find("fn run_native_postgres_restore_with_worker(")
            .unwrap_or_else(|| unreachable!("supervised restore function"));
        let run_end = source[run_start..]
            .find("fn run_native_postgres_restore_in_worker(")
            .map(|offset| run_start + offset)
            .unwrap_or_else(|| unreachable!("worker boundary"));
        let run_body = &source[run_start..run_end];
        let repair = run_body
            .find("run_native_postgres_repair_worker(")
            .unwrap_or_else(|| unreachable!("target repair"));
        let publish = run_body
            .find("phase: NativePostgresCommitPhaseV1::Dump")
            .unwrap_or_else(|| unreachable!("contained dump publication"));
        assert!(repair < publish);
    }

    #[test]
    fn unpublished_worker_target_is_explicitly_and_durably_sealed_on_every_exit() {
        let source = include_str!("native_restore.rs");
        let seal_start = source
            .find("fn seal_fail_closed(&mut self)")
            .unwrap_or_else(|| unreachable!("target seal function"));
        let seal_end = source[seal_start..]
            .find("impl Drop for NativePostgresTargetIsolationLeaseV1")
            .map(|offset| seal_start + offset)
            .unwrap_or_else(|| unreachable!("target seal boundary"));
        let seal_body = &source[seal_start..seal_end];
        let durable_limit = seal_body
            .find("self.set_connection_limit(0)?")
            .unwrap_or_else(|| unreachable!("durable limit-zero statement"));
        let role_retirement = seal_body
            .find("self.retire_outstanding_restore_role()?")
            .unwrap_or_else(|| unreachable!("restore-role retirement"));
        let final_observation = seal_body
            .rfind("provider_identity(&mut self.target_client)?")
            .unwrap_or_else(|| unreachable!("final sealed observation"));
        assert!(durable_limit < role_retirement && role_retirement < final_observation);
        assert!(!seal_body.contains("if self.active_connection_limit != 0"));

        let run_start = source
            .find("fn run_native_postgres_restore_with_pinned(")
            .unwrap_or_else(|| unreachable!("native worker operation"));
        let run_end = source[run_start..]
            .find("fn native_dump_restore(")
            .map(|offset| run_start + offset)
            .unwrap_or_else(|| unreachable!("native worker boundary"));
        let run_body = &source[run_start..run_end];
        let operation = run_body
            .find("let operation =")
            .unwrap_or_else(|| unreachable!("fallible worker operation"));
        let explicit_seal = run_body
            .find("let sealed = isolation.seal_fail_closed()")
            .unwrap_or_else(|| unreachable!("explicit worker seal"));
        let combined_result = run_body
            .find("match (operation, sealed)")
            .unwrap_or_else(|| unreachable!("seal-aware worker result"));
        assert!(operation < explicit_seal && explicit_seal < combined_result);
    }

    #[cfg(unix)]
    #[test]
    fn rerouted_restore_target_rejects_the_one_use_identity_before_clean() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let protected_directory = directory.path().join("native-restore");
        prepare_data_directory(&protected_directory)
            .unwrap_or_else(|error| unreachable!("protected directory: {error}"));
        let tool = protected_directory.join("fake-target-b");
        fs::write(
            &tool,
            "#!/bin/sh\n\
             username=\n\
             clean=0\n\
             while [ \"$#\" -gt 0 ]; do\n\
               case \"$1\" in\n\
                 --username) username=$2; shift 2 ;;\n\
                 --clean) clean=1; shift ;;\n\
                 *) shift ;;\n\
               esac\n\
             done\n\
             if [ \"$username\" = target_admin ]; then\n\
               if [ \"$clean\" = 1 ]; then : > \"$0.clean\"; fi\n\
               exit 0\n\
             fi\n\
             exit 41\n",
        )
        .unwrap_or_else(|error| unreachable!("fake target B: {error}"));
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("fake target B mode: {error}"));
        let role_name = format!(
            "worldstream_restore_{}",
            random_hex(16).unwrap_or_else(|error| unreachable!("restore role: {error}"))
        );
        let endpoint = NativePostgresEndpointV1::new(
            "127.0.0.1",
            25432,
            "target",
            role_name,
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("one-use endpoint: {error}"));
        let mut passfile =
            create_ephemeral_pgpassfile(&protected_directory, &endpoint, "random-secret")
                .unwrap_or_else(|error| unreachable!("one-use passfile: {error}"));
        assert_eq!(
            fs::metadata(&passfile.path)
                .unwrap_or_else(|error| unreachable!("one-use passfile mode: {error}"))
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        assert!(matches!(
            run_provider_tool(
                &tool,
                &endpoint,
                &passfile.path,
                ["--single-transaction", "--clean"],
                Stdio::null(),
            ),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(!tool.with_extension("clean").exists());
        passfile
            .remove()
            .unwrap_or_else(|error| unreachable!("one-use passfile cleanup: {error}"));
    }

    #[cfg(unix)]
    #[test]
    fn provider_timeout_kills_and_reaps_same_group_descendants() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let tool = directory.path().join("hanging-provider");
        fs::write(
            &tool,
            "#!/bin/sh\n\
             printf '%s' \"$$\" > \"$0.leader\"\n\
             sleep 60 &\n\
             descendant=$!\n\
             printf '%s' \"$descendant\" > \"$0.descendant\"\n\
             wait\n",
        )
        .unwrap_or_else(|error| unreachable!("provider fixture: {error}"));
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("provider mode: {error}"));
        let passfile = directory.path().join("pgpass");
        fs::write(&passfile, "*:*:*:*:secret\n")
            .unwrap_or_else(|error| unreachable!("passfile fixture: {error}"));
        fs::set_permissions(&passfile, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("passfile mode: {error}"));
        let endpoint = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("endpoint: {error}"));

        let started = Instant::now();
        assert!(matches!(
            run_provider_tool_with_timeout(
                &tool,
                &endpoint,
                &passfile,
                [],
                Stdio::null(),
                Duration::from_secs(1),
            ),
            Err(NativePostgresError::ProviderCommand)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        for suffix in ["leader", "descendant"] {
            let pid = fs::read_to_string(tool.with_extension(suffix))
                .unwrap_or_else(|error| unreachable!("{suffix} pid: {error}"));
            assert!(
                !Command::new("/bin/kill")
                    .args(["-0", pid.trim()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap_or_else(|error| unreachable!("{suffix} probe: {error}"))
                    .success()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn successful_provider_exit_terminates_and_quiesces_same_group_descendant() {
        let _provider_process_test_lock = provider_process_test_lock();

        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel: {error}"));
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let tool = directory.path().join("successful-provider");
        fs::write(
            &tool,
            "#!/bin/sh\n\
             sleep 60 </dev/null >/dev/null 2>&1 &\n\
             descendant=$!\n\
             printf '%s' \"$descendant\" > \"$0.descendant\"\n\
             exit 0\n",
        )
        .unwrap_or_else(|error| unreachable!("provider fixture: {error}"));
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("provider mode: {error}"));
        let passfile = directory.path().join("pgpass");
        fs::write(&passfile, "*:*:*:*:secret\n")
            .unwrap_or_else(|error| unreachable!("passfile fixture: {error}"));
        fs::set_permissions(&passfile, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("passfile mode: {error}"));
        let endpoint = NativePostgresEndpointV1::new(
            "127.0.0.1",
            5432,
            "worldstream",
            "postgres",
            NativePostgresTlsModeV1::Disable,
        )
        .unwrap_or_else(|error| unreachable!("endpoint: {error}"));

        let started = Instant::now();
        run_provider_tool_with_timeout(
            &tool,
            &endpoint,
            &passfile,
            [],
            Stdio::null(),
            Duration::from_secs(5),
        )
        .unwrap_or_else(|error| unreachable!("successful provider: {error}"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid = fs::read_to_string(tool.with_extension("descendant"))
            .unwrap_or_else(|error| unreachable!("descendant pid: {error}"));
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap_or_else(|error| unreachable!("descendant probe: {error}"))
                .success()
        );
        assert!(
            unrelated
                .try_wait()
                .unwrap_or_else(|error| unreachable!("unrelated sentinel probe: {error}"))
                .is_none(),
            "provider cleanup must not reap or signal an unrelated child"
        );
        unrelated
            .kill()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel cleanup: {error}"));
        unrelated
            .wait()
            .unwrap_or_else(|error| unreachable!("unrelated sentinel reap: {error}"));
    }

    #[cfg(unix)]
    #[test]
    fn successful_provider_exit_without_descendants_preserves_status_without_pgid_reuse() {
        let _provider_process_test_lock = provider_process_test_lock();

        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let tool = directory.path().join("sole-successful-provider");
        fs::write(&tool, "#!/bin/sh\nexit 0\n")
            .unwrap_or_else(|error| unreachable!("provider fixture: {error}"));
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| unreachable!("provider mode: {error}"));
        let mut command = Command::new(&tool);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_provider_group(command, false)
            .unwrap_or_else(|error| unreachable!("spawn sole provider: {error}"));
        let mut guard = ProviderChildGuardV1::new(child);

        let status = guard
            .wait_until(
                Instant::now()
                    .checked_add(Duration::from_secs(5))
                    .unwrap_or_else(|| unreachable!("provider deadline")),
            )
            .unwrap_or_else(|error| unreachable!("sole provider status: {error}"));
        assert!(status.success());
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_provider_guard_accepts_a_sole_successful_job() {
        let _provider_process_test_lock = provider_process_test_lock();
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/S", "/C", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_provider_group(command, false)
            .unwrap_or_else(|error| unreachable!("spawn Windows provider: {error}"));
        let mut guard = ProviderChildGuardV1::new(child);

        let status = guard
            .wait_until(
                Instant::now()
                    .checked_add(Duration::from_secs(5))
                    .unwrap_or_else(|| unreachable!("provider deadline")),
            )
            .unwrap_or_else(|error| unreachable!("Windows provider status: {error}"));
        assert!(status.success());
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_provider_guard_waits_for_background_job_descendant_exit() {
        let _provider_process_test_lock = provider_process_test_lock();
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary directory: {error}"));
        let script = directory.path().join("provider-with-child.ps1");
        let pid_path = directory.path().join("descendant.pid");
        fs::write(
            &script,
            "param([string]$PidPath)\n\
             $child = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 60') -PassThru\n\
             [IO.File]::WriteAllText($PidPath, [string]$child.Id)\n\
             exit 0\n",
        )
        .unwrap_or_else(|error| unreachable!("Windows provider fixture: {error}"));
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .arg(&pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_provider_group(command, false)
            .unwrap_or_else(|error| unreachable!("spawn Windows provider tree: {error}"));
        let mut guard = ProviderChildGuardV1::new(child);
        let status = guard
            .wait_until(
                Instant::now()
                    .checked_add(Duration::from_secs(10))
                    .unwrap_or_else(|| unreachable!("provider deadline")),
            )
            .unwrap_or_else(|error| unreachable!("Windows provider tree status: {error}"));
        assert!(status.success());
        let pid = fs::read_to_string(&pid_path)
            .unwrap_or_else(|error| unreachable!("Windows descendant pid: {error}"));
        let probe = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}",
                    pid.trim()
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap_or_else(|error| unreachable!("Windows descendant probe: {error}"));
        assert!(probe.success());
    }

    #[test]
    fn bounded_utf8_read_accepts_max_and_rejects_max_plus_one() {
        let exact = vec![b'x'; 257];
        assert_eq!(
            read_bounded_utf8(std::io::Cursor::new(&exact), exact.len())
                .unwrap_or_else(|_| unreachable!("exact UTF-8 bound must admit"))
                .len(),
            exact.len()
        );
        assert!(matches!(
            read_bounded_utf8(std::io::Cursor::new(&exact), exact.len() - 1),
            Err(NativePostgresError::Configuration(
                "PGPASSFILE is too large"
            ))
        ));
    }

    fn replay_witness_digest(
        memberships: &[(String, Vec<u8>)],
        timers: &[crate::PostgresTimerEvidenceV1],
        frames: &[crate::PostgresFrameEvidenceV1],
        decisions: &[crate::PostgresActivationDecisionEvidenceV1],
    ) -> DigestV1 {
        let mut bytes = Vec::new();
        append_membership_bytes(&mut bytes, memberships);
        append_timer_bytes(&mut bytes, timers);
        append_frame_bytes(&mut bytes, frames);
        append_activation_decision_bytes(&mut bytes, decisions);
        DigestV1::hash(&bytes)
    }

    fn replay_witness_fixtures() -> (
        Vec<(String, Vec<u8>)>,
        Vec<crate::PostgresTimerEvidenceV1>,
        Vec<crate::PostgresFrameEvidenceV1>,
        Vec<crate::PostgresActivationDecisionEvidenceV1>,
    ) {
        (
            vec![
                ("member-a".to_owned(), b"membership-a".to_vec()),
                ("member-b".to_owned(), b"membership-b".to_vec()),
            ],
            vec![
                crate::PostgresTimerEvidenceV1 {
                    timer_id: "timer-a".to_owned(),
                    generation: 1,
                    scheduled_for: "2030-01-01T00:00:00Z".to_owned(),
                    payload_bytes: b"timer-payload-a".to_vec(),
                    state: "scheduled".to_owned(),
                },
                crate::PostgresTimerEvidenceV1 {
                    timer_id: "timer-b".to_owned(),
                    generation: 2,
                    scheduled_for: "2030-01-02T00:00:00Z".to_owned(),
                    payload_bytes: b"timer-payload-b".to_vec(),
                    state: "fired".to_owned(),
                },
            ],
            vec![
                crate::PostgresFrameEvidenceV1 {
                    member_id: "member-a".to_owned(),
                    frame_seq: 1,
                    cause_room_seq: 4,
                    payload_bytes: b"frame-payload-a".to_vec(),
                    payload_hash: vec![0x11; 32],
                },
                crate::PostgresFrameEvidenceV1 {
                    member_id: "member-b".to_owned(),
                    frame_seq: 2,
                    cause_room_seq: 5,
                    payload_bytes: b"frame-payload-b".to_vec(),
                    payload_hash: vec![0x22; 32],
                },
            ],
            vec![
                crate::PostgresActivationDecisionEvidenceV1 {
                    cause_room_seq: 6,
                    decision_id: "decision-a".to_owned(),
                    target_member_id: Some("member-a".to_owned()),
                    decision_bytes: b"decision-bytes-a".to_vec(),
                },
                crate::PostgresActivationDecisionEvidenceV1 {
                    cause_room_seq: 7,
                    decision_id: "decision-b".to_owned(),
                    target_member_id: None,
                    decision_bytes: b"decision-bytes-b".to_vec(),
                },
            ],
        )
    }

    #[test]
    fn room_digest_binds_every_membership_replay_witness_field() {
        let (memberships, timers, frames, decisions) = replay_witness_fixtures();
        let expected = replay_witness_digest(&memberships, &timers, &frames, &decisions);
        let mut changed = memberships.clone();
        changed[0].0.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&changed, &timers, &frames, &decisions)
        );
        changed = memberships.clone();
        changed[0].1.push(0);
        assert_ne!(
            expected,
            replay_witness_digest(&changed, &timers, &frames, &decisions)
        );
        changed = memberships.clone();
        changed.reverse();
        assert_ne!(
            expected,
            replay_witness_digest(&changed, &timers, &frames, &decisions)
        );
        assert_ne!(
            expected,
            replay_witness_digest(&[], &timers, &frames, &decisions)
        );
    }

    #[test]
    fn room_digest_binds_every_timer_replay_witness_field() {
        let (memberships, timers, frames, decisions) = replay_witness_fixtures();
        let expected = replay_witness_digest(&memberships, &timers, &frames, &decisions);
        let mut changed = timers.clone();
        changed[0].timer_id.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        changed = timers.clone();
        changed[0].generation += 1;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        changed = timers.clone();
        changed[0].scheduled_for.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        changed = timers.clone();
        changed[0].payload_bytes.push(0);
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        changed = timers.clone();
        changed[0].state.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        changed = timers.clone();
        changed.reverse();
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &changed, &frames, &decisions)
        );
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &[], &frames, &decisions)
        );
    }

    #[test]
    fn room_digest_binds_every_frame_replay_witness_field() {
        let (memberships, timers, frames, decisions) = replay_witness_fixtures();
        let expected = replay_witness_digest(&memberships, &timers, &frames, &decisions);
        let mut changed = frames.clone();
        changed[0].member_id.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        changed = frames.clone();
        changed[0].frame_seq += 1;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        changed = frames.clone();
        changed[0].cause_room_seq += 1;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        changed = frames.clone();
        changed[0].payload_bytes.push(0);
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        changed = frames.clone();
        changed[0].payload_hash[0] ^= 0xff;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        changed = frames.clone();
        changed.reverse();
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &changed, &decisions)
        );
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &[], &decisions)
        );
    }

    #[test]
    fn room_digest_binds_every_activation_decision_replay_witness_field() {
        let (memberships, timers, frames, decisions) = replay_witness_fixtures();
        let expected = replay_witness_digest(&memberships, &timers, &frames, &decisions);
        let mut changed = decisions.clone();
        changed[0].cause_room_seq += 1;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &changed)
        );
        changed = decisions.clone();
        changed[0].decision_id.push('x');
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &changed)
        );
        changed = decisions.clone();
        changed[0].target_member_id = None;
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &changed)
        );
        changed = decisions.clone();
        changed[0].decision_bytes.push(0);
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &changed)
        );
        changed = decisions.clone();
        changed.reverse();
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &changed)
        );
        assert_ne!(
            expected,
            replay_witness_digest(&memberships, &timers, &frames, &[])
        );
    }

    fn observation_digest(
        positions: &[crate::PostgresObservationPositionEvidenceV1],
        consequences: &[crate::PostgresObservationConsequenceEvidenceV1],
    ) -> DigestV1 {
        let mut bytes = Vec::new();
        append_observation_positions_bytes(&mut bytes, positions);
        append_observation_consequences_bytes(&mut bytes, consequences);
        DigestV1::hash(&bytes)
    }

    fn observation_positions_fixture() -> Vec<crate::PostgresObservationPositionEvidenceV1> {
        vec![
            crate::PostgresObservationPositionEvidenceV1 {
                member_id: "member-a".to_owned(),
                frame_head: 2,
                retained_frame_floor: 1,
                last_ack_frame_seq: None,
                reset_required_through: None,
                reset_generation: 0,
            },
            crate::PostgresObservationPositionEvidenceV1 {
                member_id: "member-b".to_owned(),
                frame_head: 4,
                retained_frame_floor: 3,
                last_ack_frame_seq: Some(2),
                reset_required_through: None,
                reset_generation: 0,
            },
        ]
    }

    fn observation_consequences_fixture() -> Vec<crate::PostgresObservationConsequenceEvidenceV1> {
        vec![
            crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
                member_id: "member-a".to_owned(),
                cause_room_seq: 7,
                payload_bytes: b"projection".to_vec(),
                projection_hash: vec![0x11; 32],
            },
            crate::PostgresObservationConsequenceEvidenceV1::VisibilityLost {
                member_id: "member-b".to_owned(),
                cause_room_seq: 8,
            },
        ]
    }

    #[test]
    fn room_digest_binds_every_observation_position_field() {
        let positions = observation_positions_fixture();
        let consequences = observation_consequences_fixture();
        let expected = observation_digest(&positions, &consequences);

        let mut changed_positions = positions.clone();
        changed_positions[0].member_id.push('x');
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        changed_positions = positions.clone();
        changed_positions[0].frame_head += 1;
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        changed_positions = positions.clone();
        changed_positions[0].retained_frame_floor += 1;
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        changed_positions = positions.clone();
        changed_positions[0].last_ack_frame_seq = Some(1);
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        changed_positions = positions.clone();
        changed_positions[0].reset_required_through = Some(2);
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        changed_positions = positions.clone();
        changed_positions.reverse();
        assert_ne!(
            expected,
            observation_digest(&changed_positions, &consequences)
        );
        assert_ne!(expected, observation_digest(&[], &consequences));
    }

    #[test]
    fn room_digest_binds_every_observation_consequence_field() {
        let positions = observation_positions_fixture();
        let consequences = observation_consequences_fixture();
        let expected = observation_digest(&positions, &consequences);
        let mut changed_consequences = consequences.clone();
        if let crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
            member_id, ..
        } = &mut changed_consequences[0]
        {
            member_id.push('x');
        }
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        changed_consequences = consequences.clone();
        if let crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
            cause_room_seq,
            ..
        } = &mut changed_consequences[0]
        {
            *cause_room_seq += 1;
        }
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        changed_consequences = consequences.clone();
        if let crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
            payload_bytes,
            ..
        } = &mut changed_consequences[0]
        {
            payload_bytes.push(0);
        }
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        changed_consequences = consequences.clone();
        if let crate::PostgresObservationConsequenceEvidenceV1::ResetRequired {
            projection_hash,
            ..
        } = &mut changed_consequences[0]
        {
            projection_hash[0] ^= 0xff;
        }
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        changed_consequences = consequences.clone();
        changed_consequences[0] = crate::PostgresObservationConsequenceEvidenceV1::VisibilityLost {
            member_id: "member-a".to_owned(),
            cause_room_seq: 7,
        };
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        changed_consequences = consequences.clone();
        changed_consequences.reverse();
        assert_ne!(
            expected,
            observation_digest(&positions, &changed_consequences)
        );
        assert_ne!(expected, observation_digest(&positions, &[]));
    }

    #[test]
    fn target_catalog_admission_covers_schema_and_database_object_families() {
        for catalog in [
            "pg_namespace",
            "pg_class",
            "pg_proc",
            "pg_type",
            "pg_operator",
            "pg_collation",
            "pg_conversion",
            "pg_ts_config",
            "pg_ts_dict",
            "pg_ts_parser",
            "pg_ts_template",
            "pg_opclass",
            "pg_opfamily",
            "pg_statistic_ext",
            "pg_extension",
            "pg_event_trigger",
            "pg_foreign_data_wrapper",
            "pg_foreign_server",
            "pg_user_mapping",
            "pg_publication",
            "pg_subscription",
            "pg_largeobject_metadata",
            "pg_default_acl",
            "pg_cast",
            "pg_language",
            "pg_transform",
            "pg_am",
            "pg_seclabel",
            "pg_shseclabel",
            "pg_db_role_setting",
        ] {
            assert!(
                TARGET_USER_OBJECT_COUNT_SQL.contains(catalog),
                "catalog admission omitted {catalog}"
            );
        }
        assert!(TARGET_USER_OBJECT_COUNT_SQL.contains("ext.extname <> 'plpgsql'"));
        assert!(TARGET_USER_OBJECT_COUNT_SQL.contains("nspname NOT LIKE 'pg_temp_%'"));
    }

    #[test]
    fn database_identifier_quoting_cannot_inject_restore_lease_sql() {
        assert_eq!(
            postgres_identifier("restore\" CONNECTION LIMIT -1; --"),
            "\"restore\"\" CONNECTION LIMIT -1; --\""
        );
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
