#![allow(
    clippy::doc_markdown,
    clippy::map_entry,
    clippy::map_unwrap_or,
    clippy::manual_let_else,
    clippy::missing_errors_doc,
    clippy::single_match_else,
    clippy::type_complexity,
    clippy::unnecessary_wraps
)]

//! `PostgreSQL` 17 storage adapter for the Core-owned prepared Room Commit seam.
//!
//! The adapter deliberately uses only ordinary SQL rows for serialization:
//! every mutation first locks a durable operation-identity row, and Existing
//! mutations then lock their Room root with `SELECT ... FOR UPDATE` under
//! Read Committed.  This is safe when a transaction pooler changes the
//! physical connection between transactions and has no advisory-lock or
//! session-state correctness dependency.

mod authority;
pub mod conformance;
mod migrations;
pub mod native_restore;
pub mod telemetry;
mod transfer;

pub use authority::{PostgresAuthenticatedCapabilityV1, PostgresAuthorityAuthenticationError};
pub use migrations::{
    AUTHORITY_FACTS_MIGRATION_ID, AUTHORITY_MIGRATION_ID, DEPLOYMENT_IDENTITY_MIGRATION_ID,
    DEPLOYMENT_METADATA_MIGRATION_ID, EXTERNAL_INPUT_PREPARATION_MIGRATION_ID,
    FixtureMigrationProvider, INITIAL_MIGRATION_ID, KERNEL_CONFORMANCE_MIGRATION_ID,
    KERNEL_PARITY_MIGRATION_ID, LOGICAL_HISTORY_ID, MIGRATION_0002_SQL, MIGRATION_0003_SQL,
    MIGRATION_0004_SQL, MIGRATION_0005_SQL, MIGRATION_0006_SQL, MIGRATION_0007_SQL,
    MIGRATION_0008_SQL, MIGRATION_0009_SQL, MIGRATION_0010_SQL, MIGRATION_0011_SQL,
    MIGRATION_0012_SQL, MigrationDescriptor, MigrationFailpoint, MigrationRecord,
    MigrationVerification, MigrationVerificationError, SCHEMA_CONTRACT_ID,
    SCHEMA_FINGERPRINT_MATERIAL, TRANSFER_PUBLICATION_MIGRATION_ID,
    TRANSFER_RECOVERY_COMPLETENESS_MIGRATION_ID, TRANSFER_RESOURCE_IDENTITY_MIGRATION_ID,
    migration_history, schema_contract_fingerprint, verify_migration_prefix,
    verify_runtime_migration_history,
};
pub use telemetry::{
    PostgresIntegrityStatusV1, PostgresMigrationPhaseV1, PostgresRecoveryPhaseV1,
    PostgresStorageDiagnosticKindV1, PostgresTelemetryEventV1, PostgresTelemetrySink,
};
pub use transfer::{
    PostgresTransferDestination, PostgresTransferError, postgres_backend_fingerprint,
};

use std::{
    collections::BTreeMap,
    fmt,
    fmt::Write as _,
    net::IpAddr,
    str::FromStr,
    sync::{Arc, Mutex},
};

#[cfg(feature = "conformance-tracer")]
use std::sync::atomic::{AtomicBool, Ordering};

use native_tls::TlsConnector;
use postgres::fallible_iterator::FallibleIterator as _;
use postgres::{
    Client, GenericClient, IsolationLevel, Row, Transaction,
    config::{Host, SslMode},
};
use postgres_native_tls::MakeTlsConnector;
use telemetry::{MigrationTelemetryGuard, emit_postgres_telemetry};
use thiserror::Error;
use worldstream_backup::{VerifierLimits, max_native_restore_canonical_row_bytes};
use worldstream_core::{
    AccessModeV1, ActivationContextInputV1, ActivationDeliveryV1, ActivationFrameV1,
    ActivationIntentStateV1, ActivationInvocationContextV1, ActivationOperationRequestV1,
    ActivationOperationResultV1, ActivationResultCodeV1, AuthorityCheckedAt, AuthorityErrorV1,
    AuthorityStoreErrorV1, AuthorityStoreV1, AuthorizedDiagnosticV1, AuthorizedExternalInputV1,
    AuthorizedReceiptReadV1, AuthorizedReceiptResolverV1, AuthorizedReplayV1,
    AuthorizedRunnerControlV1, AuthorizedTimerFiredV1, Blake3DigestV1, CanonicalJsonV1,
    CanonicalRequestHashV1, CompleteHeadV1, CoreTraceV1, DiagnosticOperationV1, DiagnosticTargetV1,
    ExternalInputRecordedAt, ExternalInputV1, GenesisV1, HistoricalReplayErrorV1,
    HistoricalReplayProjectionV1, HostClockSampleV1, IntegrityGenerationV1, MemberId,
    MembershipStandingV1, MembershipV1, OperationIdentityV1, PackRegistryV1, PackRevisionLockV1,
    PackViewerV1, ParticipantActionAuthorityV1, ParticipantActionRequestV1, ParticipantActionV1,
    PreparedAdvancePersistenceV1, PreparedAuthorityWitnessV1, PreparedCreationPersistenceV1,
    PreparedExistingIntentV1, PreparedMembershipMaterializationV1,
    PreparedObservationConsequenceV1, PreparedRoomCommitV1, PreparedRoomWriteV1,
    PreparedTimerMutationKindV1, RecordedStimulusV1, RecoveredObservationConsequenceV1,
    RecoveredRoomMaterializationsV1, RecoveredTimerStateV1, RecoveryIntegrityDispositionV1,
    ReplayFailureClassV1, ReplayStorageVerificationV1, ResolutionStatusV1, ResolveOutcomeV1,
    RoomCommitResolutionV1, RoomCommitStorageV1, RoomId, RoomIntegrityStateV1,
    RoomIntegrityStatusV1, RoomRecoveryCandidateV1, RoomRecoveryErrorV1, RoomRecoveryStorageV1,
    RoomSequenceV1, RoomStatusV1, RunnerControlAdapterInputV1, RunnerControlOperationV1,
    StoredSemanticResultV1, TimerFiredRequestV1, TimerFiredV1, TimerGenerationV1, TimerId,
    TimerScheduledFor, TraceErrorV1, TransitionId, TransitionV1, commit_existing_room,
    prepare_activation_context, projection_hash_for_canonical_bytes,
};

#[cfg(feature = "conformance-tracer")]
use worldstream_core::{
    AuthorizedCoreAdministrationV1, AuthorizedParticipantActionV1, CoreAdministrationRequestV1,
    CoreRecordedAt,
};

/// The only PostgreSQL major accepted by the frozen storage contract.
pub const POSTGRES_MAJOR: u16 = 17;
/// The minimum patch used by the authored compatibility specification.
pub const POSTGRES_MINIMUM_PATCH: &str = "17.11";
/// The PostgreSQL `server_version_num` corresponding to the minimum patch.
const POSTGRES_MINIMUM_VERSION_NUM: u32 = 170_011;
/// The schema contract implemented by this adapter.
pub const POSTGRES_SCHEMA_VERSION: &str = "worldstream-postgresql-room-commit-v1";
const MAX_ACTIVATION_LEASE_MS: u64 = 30_000;

/// One native-restore capture admits at most 32 maximum encoded provider rows
/// in aggregate. This is separate from the semantic per-object bound because
/// PostgreSQL's canonical JSON bytea projection expands binary bytes as hex.
const PROVIDER_CAPTURE_BYTE_MULTIPLIER_V1: usize = 32;

#[derive(Debug)]
pub(crate) struct ProviderReadBudgetV1 {
    remaining_rows: usize,
    remaining_bytes: usize,
    maximum_row_bytes: usize,
    maximum_rooms: usize,
    maximum_records_per_room: usize,
    maximum_packs: usize,
    maximum_resources: usize,
}

#[derive(Debug)]
pub(crate) enum ProviderReadBudgetError {
    Exhausted,
    Malformed,
}

impl ProviderReadBudgetV1 {
    pub(crate) fn new(limits: VerifierLimits) -> Result<Self, ProviderReadBudgetError> {
        let maximum_row_bytes = max_native_restore_canonical_row_bytes(limits.max_object_bytes);
        let remaining_bytes = maximum_row_bytes
            .checked_mul(PROVIDER_CAPTURE_BYTE_MULTIPLIER_V1)
            .ok_or(ProviderReadBudgetError::Exhausted)?;
        if limits.max_ledger_rows == 0
            || maximum_row_bytes == 0
            || limits.max_rooms == 0
            || limits.max_records_per_room == 0
        {
            return Err(ProviderReadBudgetError::Exhausted);
        }
        Ok(Self {
            remaining_rows: limits.max_ledger_rows,
            remaining_bytes,
            maximum_row_bytes,
            maximum_rooms: limits.max_rooms,
            maximum_records_per_room: limits.max_records_per_room,
            maximum_packs: limits.max_packs,
            maximum_resources: limits.max_resources,
        })
    }

    pub(crate) fn query_parameters(
        &self,
        local_remaining_rows: usize,
    ) -> Result<(i64, i32), ProviderReadBudgetError> {
        let remaining_rows = self.remaining_rows.min(local_remaining_rows);
        let row_limit = remaining_rows
            .checked_add(1)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or(ProviderReadBudgetError::Exhausted)?;
        let byte_limit = self
            .maximum_row_bytes
            .min(self.remaining_bytes)
            .checked_add(1)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or(ProviderReadBudgetError::Exhausted)?;
        Ok((row_limit, byte_limit))
    }

    pub(crate) fn admit_projection(
        &mut self,
        encoded_prefix: &[u8],
        encoded_length: i64,
    ) -> Result<Vec<u8>, ProviderReadBudgetError> {
        let encoded_length =
            usize::try_from(encoded_length).map_err(|_| ProviderReadBudgetError::Malformed)?;
        if self.remaining_rows == 0
            || encoded_length > self.maximum_row_bytes
            || encoded_length > self.remaining_bytes
            || encoded_prefix.len() != encoded_length
        {
            return Err(ProviderReadBudgetError::Exhausted);
        }
        let canonical = CanonicalJsonV1::parse(encoded_prefix)
            .and_then(|value| value.to_bytes())
            .map_err(|_| ProviderReadBudgetError::Malformed)?;
        self.remaining_rows -= 1;
        self.remaining_bytes -= encoded_length;
        Ok(canonical)
    }

    pub(crate) const fn remaining_rows(&self) -> usize {
        self.remaining_rows
    }

    pub(crate) const fn maximum_rooms(&self) -> usize {
        self.maximum_rooms
    }

    pub(crate) const fn maximum_records_per_room(&self) -> usize {
        self.maximum_records_per_room
    }

    pub(crate) const fn maximum_packs(&self) -> usize {
        self.maximum_packs
    }

    pub(crate) const fn maximum_resources(&self) -> usize {
        self.maximum_resources
    }
}

fn bounded_provider_projection_query(
    query: &str,
    row_limit_parameter: u8,
    byte_limit_parameter: u8,
) -> String {
    format!(
        "SELECT substring(convert_to(projected.canonical_row, 'UTF8') FROM 1 FOR ${byte_limit_parameter}), \
         octet_length(convert_to(projected.canonical_row, 'UTF8'))::bigint \
         FROM ({query} LIMIT ${row_limit_parameter}) AS projected(canonical_row)"
    )
}

const NATIVE_ROOM_PROVIDER_READ_QUERIES_V1: &[&str] = &[
    "SELECT jsonb_build_array(room_id, head_bytes, integrity_generation, integrity_status)::text FROM worldstream_room_roots WHERE room_id = $1 ORDER BY room_id",
    "SELECT jsonb_build_array(room_id, pack_revision_lock_bytes, genesis_bytes)::text FROM worldstream_genesis WHERE room_id = $1 ORDER BY room_id",
    "SELECT jsonb_build_array(room_id, core_state_bytes, activity_state_bytes)::text FROM worldstream_materializations WHERE room_id = $1 ORDER BY room_id",
    "SELECT jsonb_build_array(room_id, room_seq, transition_bytes)::text FROM worldstream_transitions WHERE room_id = $1 ORDER BY room_seq",
    "SELECT jsonb_build_array(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes)::text FROM worldstream_room_snapshots WHERE room_id = $1 ORDER BY room_seq",
    "SELECT jsonb_build_array(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through)::text FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
    "SELECT jsonb_build_array(room_id, timer_id, generation, scheduled_for, payload_bytes, state)::text FROM worldstream_timers WHERE room_id = $1 ORDER BY timer_id, generation",
    "SELECT jsonb_build_array(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash)::text FROM worldstream_frames WHERE room_id = $1 ORDER BY member_id, frame_seq",
    "SELECT jsonb_build_array(room_id, member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash)::text FROM worldstream_observation_consequences WHERE room_id = $1 ORDER BY member_id, cause_room_seq",
    "SELECT jsonb_build_array(room_id, cause_room_seq, decision_id, target_member_id, decision_bytes)::text FROM worldstream_activation_decisions WHERE room_id = $1 ORDER BY cause_room_seq, decision_id",
    "SELECT jsonb_build_array(room_id, incident_seq, generation, status, reason_code, details_bytes)::text FROM worldstream_integrity_incidents WHERE room_id = $1 ORDER BY incident_seq",
];

fn preflight_native_room_provider_reads<C: GenericClient>(
    client: &mut C,
    room_id: &str,
    budget: &mut ProviderReadBudgetV1,
) -> Result<(), PostgresRoomVerificationError> {
    let mut local_remaining = budget.maximum_records_per_room();
    for query in NATIVE_ROOM_PROVIDER_READ_QUERIES_V1 {
        let (row_limit, byte_limit) = budget.query_parameters(local_remaining).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "provider read budget",
            }
        })?;
        let bounded = bounded_provider_projection_query(query, 2, 3);
        let parameters: [&(dyn postgres::types::ToSql + Sync); 3] =
            [&room_id, &row_limit, &byte_limit];
        let mut rows = client
            .query_raw(&bounded, parameters)
            .map_err(PostgresRoomVerificationError::Sql)?;
        while let Some(row) = rows.next().map_err(PostgresRoomVerificationError::Sql)? {
            if local_remaining == 0 {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "provider read budget",
                });
            }
            let prefix: Vec<u8> =
                row.try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "provider read projection",
                    })?;
            let encoded_length: i64 =
                row.try_get(1)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "provider read projection",
                    })?;
            budget
                .admit_projection(&prefix, encoded_length)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "provider read budget",
                })?;
            local_remaining -= 1;
        }
    }
    Ok(())
}

fn preflight_native_global_provider_reads<C: GenericClient>(
    client: &mut C,
    query: &str,
    local_remaining: &mut usize,
    budget: &mut ProviderReadBudgetV1,
) -> Result<(), PostgresSchemaVerificationError> {
    let (row_limit, byte_limit) = budget
        .query_parameters(*local_remaining)
        .map_err(|_| PostgresSchemaVerificationError::FingerprintDrift)?;
    let bounded = bounded_provider_projection_query(query, 1, 2);
    let parameters: [&(dyn postgres::types::ToSql + Sync); 2] = [&row_limit, &byte_limit];
    let mut rows = client
        .query_raw(&bounded, parameters)
        .map_err(PostgresSchemaVerificationError::Sql)?;
    while let Some(row) = rows.next().map_err(PostgresSchemaVerificationError::Sql)? {
        if *local_remaining == 0 {
            return Err(PostgresSchemaVerificationError::FingerprintDrift);
        }
        let prefix: Vec<u8> = row
            .try_get(0)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        let encoded_length: i64 = row
            .try_get(1)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        budget
            .admit_projection(&prefix, encoded_length)
            .map_err(|_| PostgresSchemaVerificationError::FingerprintDrift)?;
        *local_remaining -= 1;
    }
    Ok(())
}

pub(crate) fn verify_room_for_native_restore<C: GenericClient>(
    client: &mut C,
    room_id: &str,
    budget: &mut ProviderReadBudgetV1,
) -> Result<PostgresRoomVerification, PostgresRoomVerificationError> {
    preflight_native_room_provider_reads(client, room_id, budget)?;
    PostgresAdmin::verify_room_with_client(client, room_id)
}

/// Statements available to runtime schema verification. This list is kept
/// separately so a review or test can assert that runtime admission has no
/// DDL path.
pub const RUNTIME_VERIFICATION_STATEMENTS: &[&str] = &[
    "SHOW server_version_num",
    "SHOW synchronous_commit",
    "SELECT version, migration_id, checksum, logical_history_id, schema_contract_fingerprint FROM worldstream_schema_migrations ORDER BY version",
    "SELECT table_name, column_name, data_type, is_nullable FROM information_schema.columns",
    "SELECT exact global resource-identity unique indexes from pg_catalog.pg_index",
    "SELECT exact transfer-fence triggers and trigger function from pg_catalog",
    "SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1",
    "SELECT bounded host-authorized Room inventory from worldstream_room_roots joined to worldstream_genesis ordered by room_id",
    "SELECT host-authorized Room detail from worldstream_room_roots joined to worldstream_genesis by room_id",
    "SELECT pack_revision_lock_bytes, genesis_bytes FROM worldstream_genesis WHERE room_id = $1",
    "SELECT core_state_bytes, activity_state_bytes FROM worldstream_materializations WHERE room_id = $1",
    "SELECT room_seq, transition_bytes FROM worldstream_transitions WHERE room_id = $1 ORDER BY room_seq",
    "SELECT member_id, membership_bytes FROM worldstream_members WHERE room_id = $1",
    "SELECT role attributes and effective public-schema/migration-ledger/transfer-control privileges for current_user",
    "SELECT deployment_lineage_bytes, storage_epoch_bytes, storage_epoch FROM worldstream_deployment_metadata WHERE target_id = true",
    "SELECT identity_digest, pack_set_digest, resource_set_digest, canonical_bytes FROM worldstream_deployment_identity_metadata WHERE target_id = true",
    "SELECT pack_id, revision, pack_digest FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision",
    "SELECT resource_kind, resource_identity, size_bytes, resource_digest FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity",
];

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS worldstream_schema_migrations (
    version integer PRIMARY KEY,
    migration_id text NOT NULL UNIQUE,
    checksum bytea NOT NULL
);
CREATE TABLE IF NOT EXISTS worldstream_operation_guards (
    identity_bytes bytea PRIMARY KEY,
    request_hash bytea NOT NULL,
    room_id text,
    receipt_bytes bytea,
    CHECK (octet_length(request_hash) = 32)
);
CREATE TABLE IF NOT EXISTS worldstream_room_roots (
    room_id text PRIMARY KEY,
    head_bytes bytea NOT NULL,
    integrity_generation bigint NOT NULL CHECK (integrity_generation > 0),
    integrity_status text NOT NULL CHECK (integrity_status IN ('healthy', 'faulted', 'quarantined'))
);
CREATE TABLE IF NOT EXISTS worldstream_genesis (
    room_id text PRIMARY KEY,
    pack_revision_lock_bytes bytea NOT NULL,
    genesis_bytes bytea NOT NULL
);
CREATE TABLE IF NOT EXISTS worldstream_materializations (
    room_id text PRIMARY KEY,
    core_state_bytes bytea NOT NULL,
    activity_state_bytes bytea NOT NULL
);
CREATE TABLE IF NOT EXISTS worldstream_members (
    room_id text NOT NULL,
    member_id text NOT NULL,
    membership_bytes bytea NOT NULL,
    frame_head bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (room_id, member_id)
);
CREATE TABLE IF NOT EXISTS worldstream_timers (
    room_id text NOT NULL,
    timer_id text NOT NULL,
    generation bigint NOT NULL,
    scheduled_for text NOT NULL,
    payload_bytes bytea NOT NULL,
    state text NOT NULL CHECK (state IN ('scheduled', 'cancelled', 'fired')),
    PRIMARY KEY (room_id, timer_id, generation)
);
CREATE TABLE IF NOT EXISTS worldstream_transitions (
    room_id text NOT NULL,
    room_seq bigint NOT NULL,
    transition_bytes bytea NOT NULL,
    PRIMARY KEY (room_id, room_seq)
);
CREATE TABLE IF NOT EXISTS worldstream_frames (
    room_id text NOT NULL,
    member_id text NOT NULL,
    frame_seq bigint NOT NULL,
    cause_room_seq bigint NOT NULL,
    payload_bytes bytea NOT NULL,
    payload_hash bytea NOT NULL,
    PRIMARY KEY (room_id, member_id, frame_seq)
);
CREATE TABLE IF NOT EXISTS worldstream_activation_decisions (
    room_id text NOT NULL,
    cause_room_seq bigint NOT NULL,
    decision_id text NOT NULL,
    target_member_id text,
    decision_bytes bytea NOT NULL,
    PRIMARY KEY (room_id, cause_room_seq, decision_id)
);
CREATE TABLE IF NOT EXISTS worldstream_authority_fences (
    witness_id text PRIMARY KEY,
    authenticated_principal text NOT NULL,
    generation bigint NOT NULL CHECK (generation > 0),
    scope_revocation_hash bytea NOT NULL,
    active boolean NOT NULL
);
";

/// Direct maintenance versus least-privileged runtime access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresProfile {
    /// Used only for migrations and schema inspection while the daemon is offline.
    DirectAdmin,
    /// Used by the serving process; DML only and never migration DDL.
    RuntimeLeastPrivilege,
}

/// Runtime connection path. All paths use one transaction per operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresConnectionPath {
    Direct,
    SessionPool,
    TransactionPool,
}

/// Redacted connection configuration. The DSN is never included in Debug.
#[derive(Clone)]
pub struct PostgresConnectionConfig {
    dsn: String,
    profile: PostgresProfile,
    path: PostgresConnectionPath,
    tls: MakeTlsConnector,
}

impl fmt::Debug for PostgresConnectionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresConnectionConfig")
            .field("dsn", &"[REDACTED]")
            .field("profile", &self.profile)
            .field("path", &self.path)
            .field("tls", &"[CONFIGURED]")
            .finish()
    }
}

impl PostgresConnectionConfig {
    /// Creates the offline direct-admin profile.
    pub fn direct_admin(dsn: impl Into<String>) -> Result<Self, PostgresConfigError> {
        let dsn = validate_dsn(dsn.into(), PostgresProfile::DirectAdmin)?;
        Ok(Self {
            dsn,
            profile: PostgresProfile::DirectAdmin,
            path: PostgresConnectionPath::Direct,
            tls: build_tls_connector()?,
        })
    }

    /// Creates a least-privileged runtime profile. It never auto-migrates.
    pub fn runtime(
        dsn: impl Into<String>,
        path: PostgresConnectionPath,
    ) -> Result<Self, PostgresConfigError> {
        let dsn = validate_dsn(dsn.into(), PostgresProfile::RuntimeLeastPrivilege)?;
        Ok(Self {
            dsn,
            profile: PostgresProfile::RuntimeLeastPrivilege,
            path,
            tls: build_tls_connector()?,
        })
    }

    #[must_use]
    pub const fn profile(&self) -> PostgresProfile {
        self.profile
    }

    #[must_use]
    pub const fn path(&self) -> PostgresConnectionPath {
        self.path
    }
}

fn build_tls_connector() -> Result<MakeTlsConnector, PostgresConfigError> {
    TlsConnector::builder()
        .build()
        .map(MakeTlsConnector::new)
        .map_err(|_| PostgresConfigError::TlsInitialization)
}

fn validate_dsn(dsn: String, _profile: PostgresProfile) -> Result<String, PostgresConfigError> {
    if dsn.trim().is_empty() {
        return Err(PostgresConfigError::EmptyDsn);
    }
    // libpq names the authenticated mode `verify-full`, while rust-postgres
    // exposes only `require`; native-tls still verifies both the certificate
    // chain and hostname. Accept the exact trailing spelling emitted by the
    // native restore DSN builder and normalize only the stored driver input.
    let driver_dsn = match dsn.strip_suffix(" sslmode=verify-full") {
        Some(prefix) => format!("{prefix} sslmode=require"),
        None => dsn,
    };
    let parsed =
        postgres::Config::from_str(&driver_dsn).map_err(|_| PostgresConfigError::InvalidDsn)?;
    if !local_postgres_config(&parsed) && parsed.get_ssl_mode() != SslMode::Require {
        return Err(PostgresConfigError::RemoteTlsRequired);
    }
    Ok(driver_dsn)
}

fn local_postgres_config(config: &postgres::Config) -> bool {
    let hosts_are_local = config.get_hosts().iter().all(|host| match host {
        Host::Tcp(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        #[cfg(unix)]
        Host::Unix(_) => true,
    });
    let addresses_are_local = config.get_hostaddrs().iter().all(IpAddr::is_loopback);
    hosts_are_local && addresses_are_local
}

/// Configuration failures are fatal before any connection is attempted.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum PostgresConfigError {
    #[error("PostgreSQL DSN is empty")]
    EmptyDsn,
    #[error("PostgreSQL DSN is invalid")]
    InvalidDsn,
    #[error("remote PostgreSQL connections require TLS")]
    RemoteTlsRequired,
    #[error("PostgreSQL TLS trust initialization failed")]
    TlsInitialization,
    #[error("PostgreSQL adapter requires the direct-admin profile")]
    AdminProfileRequired,
    #[error("PostgreSQL adapter requires the least-privileged runtime profile")]
    RuntimeProfileRequired,
}

/// Stable classification of failures from the PostgreSQL provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresStorageFailure {
    Connection,
    Deadlock,
    LockTimeout,
    Serialization,
    Constraint,
    Schema,
    Driver,
}

/// The PostgreSQL engine identity established by the adapter's capability
/// admission check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresEngineIdentityV1 {
    server_version_num: u32,
    formatted: String,
}

impl PostgresEngineIdentityV1 {
    /// Returns PostgreSQL's numeric server version identity.
    #[must_use]
    pub const fn server_version_num(&self) -> u32 {
        self.server_version_num
    }

    /// Returns the deterministic operator-facing identity string.
    #[must_use]
    pub fn formatted(&self) -> &str {
        &self.formatted
    }
}

/// Closed errors from the read-only engine identity probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum PostgresEngineIdentityError {
    #[error("PostgreSQL engine identity is unavailable")]
    Unavailable,
    #[error("PostgreSQL engine identity is unsupported")]
    Unsupported,
    #[error("PostgreSQL engine identity is corrupt")]
    Corrupt,
}

/// Closed failures from the provider-owned authority clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum PostgresAuthorityClockError {
    #[error("PostgreSQL authority clock is unavailable")]
    Unavailable,
    #[error("PostgreSQL authority clock returned an invalid timestamp")]
    Corrupt,
}

/// Closed outcomes from durable ExternalInput Semantic Time preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum PostgresExternalInputPreparationErrorV1 {
    /// The stable operation identity is already bound to another request hash.
    #[error("ExternalInput preparation identity conflicts with another request")]
    Conflict,
    /// PostgreSQL could not durably reserve or read the preparation row.
    #[error("ExternalInput preparation storage is unavailable")]
    StorageUnavailable,
    /// The requested identity or retained preparation row is malformed.
    #[error("ExternalInput preparation identity is corrupt")]
    Corrupt,
}

/// Errors from the explicit, read-only PostgreSQL receipt lookup primitive.
///
/// Authorization is intentionally not inferred here: callers that expose this
/// operation must obtain and revalidate a Core `AuthorizedReceiptReadV1` before
/// invoking it. The method exists so that such callers can perform the final
/// provider read under one transaction without falling back to ad-hoc SQL.
#[derive(Debug, Error)]
pub enum PostgresReceiptReadError {
    #[error("PostgreSQL receipt lookup connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL receipt lookup query failed: {0}")]
    Sql(postgres::Error),
    #[error("receipt identity could not be canonically encoded")]
    InvalidIdentity,
    #[error("stored PostgreSQL receipt is not a valid canonical semantic result")]
    Corrupt,
}

/// Closed errors from the PostgreSQL typed conformance seam.
#[cfg(feature = "conformance-tracer")]
#[derive(Debug, Error)]
pub enum PostgresConformanceError {
    #[error("PostgreSQL conformance verification failed: {0}")]
    Verification(String),
    #[error("PostgreSQL conformance trace replay failed: {0}")]
    Replay(String),
    #[error("PostgreSQL conformance preparation failed: {0}")]
    Preparation(String),
    #[error("PostgreSQL conformance storage returned {0:?}")]
    Resolution(RoomCommitResolutionV1),
}

/// Fail-closed errors from the PostgreSQL Activation lifecycle.
#[derive(Debug, Error)]
pub enum PostgresActivationError {
    #[error("Activation authority is not current")]
    Authority(#[source] AuthorityErrorV1),
    #[error("PostgreSQL Activation connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL Activation query failed: {0}")]
    Sql(postgres::Error),
    #[error("Activation request is invalid")]
    InvalidRequest,
    #[error("Activation operation identity conflicts with an existing receipt")]
    IdempotencyConflict,
    #[error("Activation is fenced or does not exist")]
    Fenced,
    #[error("Activation lease is stale or expired")]
    StaleLease,
    #[error("Activation receipt or context is corrupt")]
    Corrupt,
}

/// Exact durable lifecycle of one PostgreSQL Timer generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresTimerStateV1 {
    /// The generation is eligible once its durable schedule is due.
    Scheduled,
    /// The generation was cancelled by a committed Room transition.
    Cancelled,
    /// The generation already has a durable fired receipt.
    Fired,
}

/// One exact server-loaded Timer request and its durable lifecycle witness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresTimerCandidateV1 {
    request: TimerFiredRequestV1,
    state: PostgresTimerStateV1,
}

impl PostgresTimerCandidateV1 {
    /// Returns the immutable Core Timer request reconstructed from storage.
    #[must_use]
    pub const fn request(&self) -> &TimerFiredRequestV1 {
        &self.request
    }

    /// Returns the durable lifecycle observed with the request.
    #[must_use]
    pub const fn state(&self) -> PostgresTimerStateV1 {
        self.state
    }
}

/// A private Invocation Context paired with the exact claim request that
/// produced it. Fields remain sealed so production callers cannot substitute
/// provider-invented projection, delivery, or lease facts.
pub struct PostgresActivationClaimV1 {
    authority: RunnerControlAdapterInputV1,
    request: ActivationOperationRequestV1,
    context: ActivationInvocationContextV1,
}

impl fmt::Debug for PostgresActivationClaimV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresActivationClaimV1([REDACTED])")
    }
}

/// Result of atomically revalidating authority and looking up a claim receipt
/// before private context preparation.
#[derive(Debug)]
pub enum PostgresActivationClaimPreparationV1 {
    /// The exact operation already committed and its receipt is durable.
    Existing(Box<ActivationOperationResultV1>),
    /// A fresh claim has a sealed context and retained authority fence.
    Prepared(Box<PostgresActivationClaimV1>),
}

/// Closed errors from bounded provider-owned Activation lease reclamation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum PostgresActivationLeaseReclaimError {
    #[error("PostgreSQL Activation lease reclamation is unavailable")]
    Unavailable,
    #[error("PostgreSQL Activation lease state is corrupt")]
    Corrupt,
}

/// The durable portion of one pending Activation offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresActivationOfferV1 {
    pub activation_id: String,
    pub room_id: String,
    pub target_member_id: String,
    pub cause_room_seq: u64,
    pub reason_code: String,
    pub priority: u64,
    pub semantic_deadline: Option<String>,
    pub policy_revision: u64,
    pub maximum_lease_ms: u64,
}

/// Provider-neutral frame delivery returned by the PostgreSQL observation
/// seam. Projection bytes are supplied by the Core-owned viewer adapter and
/// are never reconstructed by SQL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostgresObservationDeliveryV1 {
    Retained {
        cursor: Option<u64>,
        cursor_exclusive: u64,
        frame_head: u64,
        retained_floor: u64,
        frames: Vec<PostgresFrameEvidenceV1>,
    },
    Reset {
        cursor: Option<u64>,
        frame_head: u64,
        retained_floor: u64,
        reset_through: Option<u64>,
        projection_bytes: Vec<u8>,
    },
}

/// Durable Cursor positions returned after PostgreSQL frame pruning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresObservationPositionsV1 {
    pub frame_head: u64,
    pub retained_frame_floor: u64,
    pub last_ack_frame_seq: Option<u64>,
    pub reset_required_through: Option<u64>,
}

/// Closed errors from the PostgreSQL observation delivery seam.
#[derive(Debug, Error)]
pub enum PostgresObservationError {
    #[error("PostgreSQL observation connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL observation query failed: {0}")]
    Sql(postgres::Error),
    #[error("observation row is corrupt")]
    Corrupt,
    #[error("observation Cursor is ahead of the durable frame head")]
    FutureCursor,
    #[error("projection bytes are not canonical")]
    InvalidProjection,
}

/// Closed failures from the production Core-prepared existing-Room seam.
#[derive(Debug, Error)]
pub enum PostgresRoomCommitError {
    #[error("PostgreSQL Room recovery is unavailable")]
    Recovery(#[from] RoomRecoveryErrorV1),
    #[error("PostgreSQL Room commit preparation failed")]
    Preparation,
}

/// Closed failures from exact PostgreSQL Timer witness reads.
#[derive(Debug, Error)]
pub enum PostgresTimerError {
    #[error("PostgreSQL Timer storage is unavailable")]
    Unavailable,
    #[error("PostgreSQL Timer witness is corrupt")]
    Corrupt,
}

/// Closed production failure for the present-authorized historical replay
/// façade. PostgreSQL supplies only verified immutable bytes; Core performs
/// the replay and projection reduction.
#[derive(Debug, Error)]
pub enum PostgresReplayError {
    #[error("PostgreSQL replay authority is unavailable")]
    Authority,
    #[error("PostgreSQL replay Room verification failed")]
    Verification,
    #[error("historical replay failed: {0}")]
    Replay(#[from] HistoricalReplayErrorV1),
}

impl PostgresStorageFailure {
    /// Classifies a PostgreSQL SQLSTATE without retaining provider text.
    ///
    /// The adapter uses this stable classification to decide whether a
    /// transaction can be retried.  Keeping the mapping separate from the
    /// driver error also lets the harness test the deadlock, lock-timeout,
    /// serialization, and connection-loss boundaries without fabricating a
    /// successful provider run.
    #[must_use]
    pub fn from_sqlstate(code: &str) -> Self {
        match code {
            "08000" | "08001" | "08003" | "08004" | "08006" | "57P01" => Self::Connection,
            "40P01" => Self::Deadlock,
            "55P03" => Self::LockTimeout,
            "40001" => Self::Serialization,
            "23514" | "23505" | "23503" => Self::Constraint,
            "42P01" | "42703" => Self::Schema,
            _ => Self::Driver,
        }
    }

    fn from_error(error: &postgres::Error) -> Self {
        let Some(db) = error.as_db_error() else {
            return Self::Connection;
        };
        Self::from_sqlstate(db.code().code())
    }
}

/// Failpoints used by the local conformance harness. They model provider
/// outcomes without pretending that a live PostgreSQL pass occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresFailpoint {
    RollbackBeforeCommit,
    UnknownAfterCommit,
}

/// Offline direct-admin migration handle.
pub struct PostgresAdmin {
    config: PostgresConnectionConfig,
    telemetry: Option<Arc<dyn PostgresTelemetrySink>>,
}

const NATIVE_REBUILD_PROVIDER_IDENTITY_SQL: &str = "SELECT control.system_identifier::text, \
    db.oid::text, db.datname FROM pg_control_system() control \
    JOIN pg_database db ON db.datname = current_database()";

fn require_native_restore_provider_identity(
    client: &mut Client,
    expected: &native_restore::PostgresProviderIdentityV1,
) -> Result<(), PostgresMaintenanceError> {
    let row = client
        .query_one(NATIVE_REBUILD_PROVIDER_IDENTITY_SQL, &[])
        .map_err(PostgresMaintenanceError::Sql)?;
    let system_identifier = row
        .try_get::<_, String>(0)
        .map_err(PostgresMaintenanceError::Sql)?;
    let database_oid = row
        .try_get::<_, String>(1)
        .map_err(PostgresMaintenanceError::Sql)?;
    let database_name = row
        .try_get::<_, String>(2)
        .map_err(PostgresMaintenanceError::Sql)?;
    if system_identifier != expected.system_identifier
        || database_oid != expected.database_oid
        || database_name != expected.database_name
    {
        return Err(PostgresMaintenanceError::ConcurrentChange);
    }
    Ok(())
}

impl PostgresAdmin {
    /// Creates an admin handle; no network connection is opened yet.
    pub fn new(config: PostgresConnectionConfig) -> Result<Self, PostgresConfigError> {
        if config.profile != PostgresProfile::DirectAdmin {
            return Err(PostgresConfigError::AdminProfileRequired);
        }
        Ok(Self {
            config,
            telemetry: None,
        })
    }

    /// Attaches a best-effort storage telemetry sink.  The sink is called
    /// only with closed facts and is panic-isolated from administration.
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: Arc<dyn PostgresTelemetrySink>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    fn connect(&self) -> Result<Client, postgres::Error> {
        Client::connect(&self.config.dsn, self.config.tls.clone())
    }

    /// Applies all pending forward-only migrations using a direct admin
    /// connection. Each migration body and ledger write share one PostgreSQL
    /// transaction, so an interruption rolls back the body and is safe to
    /// restart. This is an explicit offline operation; the runtime adapter has
    /// no code path that calls it.
    pub fn migrate(&self) -> Result<(), PostgresMigrationError> {
        let history = migration_history();
        let target_version = history
            .last()
            .map_or(0, |migration| u64::try_from(migration.version).unwrap_or(0));
        let mut migration_telemetry =
            MigrationTelemetryGuard::new(self.telemetry.as_deref(), target_version);
        migration_telemetry.started();
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresMigrationError::Connection)?;
        verify_server_capabilities(&mut client).map_err(PostgresMigrationError::Capability)?;
        let records = read_migration_records(&mut client).map_err(PostgresMigrationError::Sql)?;
        let state = verify_migration_prefix(&records).map_err(PostgresMigrationError::History)?;
        if records.len() > history.len() {
            return Err(PostgresMigrationError::History(
                MigrationVerificationError::Unsupported {
                    version: state.current_version,
                },
            ));
        }

        for descriptor in history.iter().skip(records.len()) {
            let mut transaction = client.transaction().map_err(PostgresMigrationError::Sql)?;
            transaction
                .batch_execute(descriptor.sql)
                .map_err(PostgresMigrationError::Sql)?;
            let checksum = descriptor.checksum();
            if descriptor.version == 1 {
                transaction
                    .execute(
                        "INSERT INTO worldstream_schema_migrations(version, migration_id, checksum) VALUES ($1, $2, $3)",
                        &[
                            &descriptor.version,
                            &descriptor.id,
                            &checksum.as_bytes().as_slice(),
                        ],
                    )
                    .map_err(PostgresMigrationError::Sql)?;
            } else {
                let fingerprint = schema_contract_fingerprint();
                transaction
                    .execute(
                        "UPDATE worldstream_schema_migrations SET logical_history_id = $1, schema_contract_fingerprint = $2 WHERE version = 1",
                        &[&LOGICAL_HISTORY_ID, &fingerprint.as_bytes().as_slice()],
                    )
                    .map_err(PostgresMigrationError::Sql)?;
                transaction
                    .execute(
                        "INSERT INTO worldstream_schema_migrations(version, migration_id, checksum, logical_history_id, schema_contract_fingerprint) VALUES ($1, $2, $3, $4, $5)",
                        &[
                            &descriptor.version,
                            &descriptor.id,
                            &checksum.as_bytes().as_slice(),
                            &LOGICAL_HISTORY_ID,
                            &fingerprint.as_bytes().as_slice(),
                        ],
                    )
                    .map_err(PostgresMigrationError::Sql)?;
            }
            transaction.commit().map_err(PostgresMigrationError::Sql)?;
        }
        verify_runtime_schema_client(&mut client).map_err(PostgresMigrationError::Schema)?;
        migration_telemetry.complete(if records.len() == history.len() {
            PostgresMigrationPhaseV1::AlreadyCurrent
        } else {
            PostgresMigrationPhaseV1::Applied
        });
        Ok(())
    }

    /// Verifies the current schema without changing it.
    pub fn verify_schema(&self) -> Result<(), PostgresSchemaVerificationError> {
        let result = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresSchemaVerificationError::Connection)
            .and_then(|mut client| verify_runtime_schema_client(&mut client));
        if let Err(error) = &result {
            let kind = match error {
                PostgresSchemaVerificationError::Connection(_) => {
                    PostgresStorageDiagnosticKindV1::Connection
                }
                PostgresSchemaVerificationError::Sql(_) => PostgresStorageDiagnosticKindV1::Query,
                PostgresSchemaVerificationError::UnsupportedMajor { .. }
                | PostgresSchemaVerificationError::UnsupportedPatch { .. }
                | PostgresSchemaVerificationError::InvalidServerVersion { .. }
                | PostgresSchemaVerificationError::SynchronousCommit { .. }
                | PostgresSchemaVerificationError::RuntimeRolePrivileges
                | PostgresSchemaVerificationError::DisposableRestoreTarget
                | PostgresSchemaVerificationError::History(_)
                | PostgresSchemaVerificationError::FingerprintDrift => {
                    PostgresStorageDiagnosticKindV1::Integrity
                }
            };
            emit_postgres_telemetry(
                self.telemetry.as_deref(),
                PostgresTelemetryEventV1::StorageDiagnostic { kind },
            );
        }
        result
    }

    /// Verifies one persisted Room without changing provider state.
    ///
    /// The check requires canonical Head, Genesis, pack revision lock,
    /// materialization, Transition, and Membership bytes. It also verifies
    /// contiguous Transition order and prior-lineage links, then compares the
    /// current materialization with Genesis or the final Transition. Activity
    /// pack reduction and hash recomputation remain Core responsibilities; the
    /// public Core crate intentionally does not expose those private helpers.
    #[allow(clippy::too_many_lines)]
    pub fn verify_room(
        &self,
        room_id: &str,
    ) -> Result<PostgresRoomVerification, PostgresRoomVerificationError> {
        let result = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresRoomVerificationError::Connection)
            .and_then(|mut client| Self::verify_room_with_client(&mut client, room_id));
        if let Err(error) = &result {
            let kind = match error {
                PostgresRoomVerificationError::Connection(_) => {
                    PostgresStorageDiagnosticKindV1::Connection
                }
                PostgresRoomVerificationError::Sql(_) => PostgresStorageDiagnosticKindV1::Query,
                PostgresRoomVerificationError::MissingRoom { .. }
                | PostgresRoomVerificationError::InvalidRoomId
                | PostgresRoomVerificationError::Corrupt { .. } => {
                    PostgresStorageDiagnosticKindV1::Integrity
                }
            };
            emit_postgres_telemetry(
                self.telemetry.as_deref(),
                PostgresTelemetryEventV1::StorageDiagnostic { kind },
            );
        }
        result
    }

    /// Deletes disposable paired snapshots for one Room. Immutable Genesis,
    /// Transitions, and current materialization remain untouched.
    pub fn delete_snapshot_cache(&self, room_id: &str) -> Result<u64, PostgresMaintenanceError> {
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresMaintenanceError::Connection)?;
        let count = client
            .execute(
                "DELETE FROM worldstream_room_snapshots WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(PostgresMaintenanceError::Sql)?;
        Ok(count)
    }

    /// Corrupts the newest disposable snapshot for live conformance coverage.
    ///
    /// This seam is unavailable in production builds. It proves that normal
    /// maintenance can discard malformed cache bytes without weakening
    /// verification of the authoritative Genesis and Transition lineage.
    #[cfg(feature = "conformance-tracer")]
    pub fn corrupt_snapshot_cache_for_conformance(
        &self,
        room_id: &str,
    ) -> Result<u64, PostgresMaintenanceError> {
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresMaintenanceError::Connection)?;
        let corrupt = [0xff_u8];
        client
            .execute(
                "UPDATE worldstream_room_snapshots SET complete_head_bytes = $2 \
                 WHERE room_id = $1 AND room_seq = ( \
                    SELECT max(room_seq) FROM worldstream_room_snapshots WHERE room_id = $1 \
                 )",
                &[&room_id, &corrupt.as_slice()],
            )
            .map_err(PostgresMaintenanceError::Sql)
    }

    /// Rebuilds every paired snapshot from the exact retained Genesis and
    /// Transition bytes, preserving revision order and all canonical fields.
    pub fn rebuild_snapshot_cache(&self, room_id: &str) -> Result<usize, PostgresMaintenanceError> {
        self.rebuild_snapshot_cache_with_budget(room_id, None, None)
    }

    pub(crate) fn rebuild_snapshot_cache_for_native_restore(
        &self,
        room_id: &str,
        budget: &mut ProviderReadBudgetV1,
        expected_provider_identity: &native_restore::PostgresProviderIdentityV1,
    ) -> Result<usize, PostgresMaintenanceError> {
        self.rebuild_snapshot_cache_with_budget(
            room_id,
            Some(budget),
            Some(expected_provider_identity),
        )
    }

    fn rebuild_snapshot_cache_with_budget(
        &self,
        room_id: &str,
        budget: Option<&mut ProviderReadBudgetV1>,
        expected_provider_identity: Option<&native_restore::PostgresProviderIdentityV1>,
    ) -> Result<usize, PostgresMaintenanceError> {
        emit_postgres_telemetry(
            self.telemetry.as_deref(),
            PostgresTelemetryEventV1::Recovery {
                phase: PostgresRecoveryPhaseV1::Started,
            },
        );
        let result = self.rebuild_snapshot_cache_inner(room_id, budget, expected_provider_identity);
        let diagnostic = match &result {
            Err(PostgresMaintenanceError::Connection(_)) => {
                Some(PostgresStorageDiagnosticKindV1::Connection)
            }
            Err(PostgresMaintenanceError::Sql(_)) => Some(PostgresStorageDiagnosticKindV1::Query),
            Err(PostgresMaintenanceError::Corrupt | PostgresMaintenanceError::ConcurrentChange) => {
                Some(PostgresStorageDiagnosticKindV1::Integrity)
            }
            // `verify_room` emits the exact connection/query/integrity class
            // before its typed error is wrapped for maintenance.
            Ok(_) | Err(PostgresMaintenanceError::Verification(_)) => None,
        };
        if let Some(kind) = diagnostic {
            emit_postgres_telemetry(
                self.telemetry.as_deref(),
                PostgresTelemetryEventV1::StorageDiagnostic { kind },
            );
        }
        emit_postgres_telemetry(
            self.telemetry.as_deref(),
            PostgresTelemetryEventV1::Recovery {
                phase: if result.is_ok() {
                    PostgresRecoveryPhaseV1::Completed
                } else {
                    PostgresRecoveryPhaseV1::Failed
                },
            },
        );
        result
    }

    #[allow(clippy::too_many_lines)]
    fn rebuild_snapshot_cache_inner(
        &self,
        room_id: &str,
        budget: Option<&mut ProviderReadBudgetV1>,
        expected_provider_identity: Option<&native_restore::PostgresProviderIdentityV1>,
    ) -> Result<usize, PostgresMaintenanceError> {
        // Snapshot rows are disposable caches. Delete them before canonical
        // verification so malformed cache bytes cannot prevent repair. The
        // verifier still fails closed on every authoritative retained byte.
        if let Some(expected) = expected_provider_identity {
            let mut client = self
                .connect()
                .map_err(PostgresMaintenanceError::Connection)?;
            require_native_restore_provider_identity(&mut client, expected)?;
            client
                .execute(
                    "DELETE FROM worldstream_room_snapshots WHERE room_id = $1",
                    &[&room_id],
                )
                .map_err(PostgresMaintenanceError::Sql)?;
        } else {
            self.delete_snapshot_cache(room_id)?;
        }
        let verification = if let Some(budget) = budget {
            let mut client = self
                .connect()
                .map_err(PostgresMaintenanceError::Connection)?;
            require_native_restore_provider_identity(
                &mut client,
                expected_provider_identity.ok_or(PostgresMaintenanceError::Corrupt)?,
            )?;
            let mut transaction = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()
                .map_err(PostgresMaintenanceError::Sql)?;
            let verification = verify_room_for_native_restore(&mut transaction, room_id, budget)
                .map_err(PostgresMaintenanceError::Verification)?;
            transaction
                .commit()
                .map_err(PostgresMaintenanceError::Sql)?;
            verification
        } else {
            self.verify_room(room_id)
                .map_err(PostgresMaintenanceError::Verification)?
        };
        let genesis = GenesisV1::from_canonical_bytes(&verification.genesis_bytes)
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        let mut client = self
            .connect()
            .map_err(PostgresMaintenanceError::Connection)?;
        if let Some(expected) = expected_provider_identity {
            require_native_restore_provider_identity(&mut client, expected)?;
        }
        let mut tx = client
            .transaction()
            .map_err(PostgresMaintenanceError::Sql)?;
        tx.execute(
            "DELETE FROM worldstream_room_snapshots WHERE room_id = $1",
            &[&room_id],
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        let head = genesis.complete_head();
        let head_bytes = head
            .canonical_bytes()
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        let core_bytes = genesis
            .initial_core_state()
            .canonical_bytes()
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        let activity_bytes = genesis
            .initial_activity_state()
            .to_bytes()
            .map_err(|_| PostgresMaintenanceError::Corrupt)?;
        persist_snapshot(
            &mut tx,
            room_id,
            &head,
            &head_bytes,
            &core_bytes,
            &activity_bytes,
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        for bytes in &verification.transition_bytes {
            let transition = TransitionV1::from_canonical_bytes(bytes)
                .map_err(|_| PostgresMaintenanceError::Corrupt)?;
            let head = transition.complete_head();
            let head_bytes = head
                .canonical_bytes()
                .map_err(|_| PostgresMaintenanceError::Corrupt)?;
            let core_bytes = transition
                .resulting_core_state()
                .canonical_bytes()
                .map_err(|_| PostgresMaintenanceError::Corrupt)?;
            let activity_bytes = transition
                .resulting_activity_state()
                .to_bytes()
                .map_err(|_| PostgresMaintenanceError::Corrupt)?;
            persist_snapshot(
                &mut tx,
                room_id,
                &head,
                &head_bytes,
                &core_bytes,
                &activity_bytes,
            )
            .map_err(PostgresMaintenanceError::Sql)?;
        }
        tx.commit().map_err(PostgresMaintenanceError::Sql)?;
        Ok(verification.transition_bytes.len() + 1)
    }

    /// Writes one generation-fenced integrity incident and updates the room
    /// root in the same transaction. Recovery/repair callers must still prove
    /// canonical lineage before choosing a healthy disposition.
    pub fn record_integrity_incident(
        &self,
        room_id: &str,
        expected_generation: u64,
        status: &str,
        reason_code: &str,
        details_bytes: Option<&[u8]>,
    ) -> Result<u64, PostgresMaintenanceError> {
        if !matches!(status, "faulted" | "quarantined" | "healthy") || reason_code.is_empty() {
            return Err(PostgresMaintenanceError::Corrupt);
        }
        let current =
            i64::try_from(expected_generation).map_err(|_| PostgresMaintenanceError::Corrupt)?;
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresMaintenanceError::Connection)?;
        let mut tx = client
            .transaction()
            .map_err(PostgresMaintenanceError::Sql)?;
        let root = tx
            .query_opt(
                "SELECT integrity_generation FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
                &[&room_id],
            )
            .map_err(PostgresMaintenanceError::Sql)?
            .ok_or(PostgresMaintenanceError::Corrupt)?;
        let generation: i64 = root.get(0);
        if generation != current {
            return Err(PostgresMaintenanceError::ConcurrentChange);
        }
        let next_generation = generation
            .checked_add(1)
            .ok_or(PostgresMaintenanceError::Corrupt)?;
        let incident_seq: i64 = tx
            .query_one(
                "SELECT coalesce(max(incident_seq), 0) + 1 FROM worldstream_integrity_incidents WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(PostgresMaintenanceError::Sql)?
            .get(0);
        tx.execute(
            "INSERT INTO worldstream_integrity_incidents(room_id, incident_seq, generation, status, reason_code, details_bytes) VALUES ($1, $2, $3, $4, $5, $6)",
            &[&room_id, &incident_seq, &next_generation, &status, &reason_code, &details_bytes],
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        tx.execute(
            "UPDATE worldstream_room_roots SET integrity_generation = $2, integrity_status = $3 WHERE room_id = $1 AND integrity_generation = $4",
            &[&room_id, &next_generation, &status, &current],
        )
        .map_err(PostgresMaintenanceError::Sql)?;
        tx.commit().map_err(PostgresMaintenanceError::Sql)?;
        if let Some(status) = match status {
            "faulted" => Some(PostgresIntegrityStatusV1::Faulted),
            "quarantined" => Some(PostgresIntegrityStatusV1::Quarantined),
            "healthy" => None,
            _ => unreachable!("status was validated above"),
        } {
            emit_postgres_telemetry(
                self.telemetry.as_deref(),
                PostgresTelemetryEventV1::Integrity { status },
            );
        }
        u64::try_from(next_generation).map_err(|_| PostgresMaintenanceError::Corrupt)
    }

    #[allow(clippy::too_many_lines)]
    fn verify_room_with_client<C: GenericClient>(
        client: &mut C,
        room_id: &str,
    ) -> Result<PostgresRoomVerification, PostgresRoomVerificationError> {
        let parsed_room_id = room_id
            .parse::<RoomId>()
            .map_err(|_| PostgresRoomVerificationError::InvalidRoomId)?;
        let root = client
            .query_opt(
                "SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?
            .ok_or_else(|| PostgresRoomVerificationError::MissingRoom {
                room_id: room_id.to_owned(),
            })?;
        let head_bytes: Vec<u8> = root
            .try_get(0)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Head" })?;
        let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Head" })?;
        if head.canonical_bytes().ok().as_deref() != Some(head_bytes.as_slice())
            || head.room_id() != &parsed_room_id
        {
            return Err(PostgresRoomVerificationError::Corrupt { what: "Head" });
        }
        let integrity_generation: i64 =
            root.try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "integrity generation",
                })?;
        let integrity_generation = u64::try_from(integrity_generation).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "integrity generation",
            }
        })?;
        let integrity_status: String =
            root.try_get(2)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "integrity status",
                })?;
        if !matches!(
            integrity_status.as_str(),
            "healthy" | "faulted" | "quarantined"
        ) {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "integrity status",
            });
        }

        let genesis_row = client
            .query_opt(
                "SELECT pack_revision_lock_bytes, genesis_bytes FROM worldstream_genesis WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?
            .ok_or(PostgresRoomVerificationError::Corrupt { what: "Genesis" })?;
        let pack_lock_bytes: Vec<u8> =
            genesis_row
                .try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "pack revision lock",
                })?;
        let genesis_bytes: Vec<u8> = genesis_row
            .try_get(1)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Genesis" })?;
        let genesis = GenesisV1::from_canonical_bytes(&genesis_bytes)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Genesis" })?;
        if genesis.canonical_bytes().ok().as_deref() != Some(genesis_bytes.as_slice())
            || genesis.room_id() != &parsed_room_id
            || genesis.pack_digest() != head.pack_digest()
        {
            return Err(PostgresRoomVerificationError::Corrupt { what: "Genesis" });
        }
        let pack_lock =
            PackRevisionLockV1::from_canonical_bytes(&pack_lock_bytes, genesis.pack_digest())
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "pack revision lock",
                })?;
        if pack_lock.canonical_bytes().ok().as_deref() != Some(pack_lock_bytes.as_slice()) {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "pack revision lock",
            });
        }

        let materialization = client
            .query_opt(
                "SELECT core_state_bytes, activity_state_bytes FROM worldstream_materializations WHERE room_id = $1",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?
            .ok_or(PostgresRoomVerificationError::Corrupt {
                what: "materialization",
            })?;
        let core_state_bytes: Vec<u8> =
            materialization
                .try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Core materialization",
                })?;
        let activity_state_bytes: Vec<u8> =
            materialization
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Activity materialization",
                })?;
        CanonicalJsonV1::decode_canonical::<worldstream_core::CoreRoomStateV1>(&core_state_bytes)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
            what: "Core materialization",
        })?;
        CanonicalJsonV1::from_canonical_bytes(&activity_state_bytes).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "Activity materialization",
            }
        })?;

        let transition_rows = client
            .query(
                "SELECT room_seq, transition_bytes FROM worldstream_transitions WHERE room_id = $1 ORDER BY room_seq",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let expected_transition_count = usize::try_from(head.room_seq().get()).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "Head sequence",
            }
        })?;
        if transition_rows.len() != expected_transition_count {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "Transition count",
            });
        }
        let mut previous_hash = genesis.genesis_hash().clone();
        let mut final_core_bytes =
            genesis
                .initial_core_state()
                .canonical_bytes()
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Genesis Core state",
                })?;
        let mut final_activity_bytes =
            genesis.initial_activity_state().to_bytes().map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "Genesis Activity state",
                }
            })?;
        let mut expected_snapshots = vec![(
            0_u64,
            genesis.complete_head().canonical_bytes().map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "Genesis Head",
                }
            })?,
            final_core_bytes.clone(),
            final_activity_bytes.clone(),
        )];
        for (index, row) in transition_rows.iter().enumerate() {
            let stored_seq: i64 =
                row.try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Transition sequence",
                    })?;
            let expected_seq =
                i64::try_from(index + 1).map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Transition sequence",
                })?;
            if stored_seq != expected_seq {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "Transition sequence",
                });
            }
            let bytes: Vec<u8> = row
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Transition" })?;
            let transition = TransitionV1::from_canonical_bytes(&bytes)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Transition" })?;
            let transition_head = transition.complete_head();
            if transition_head.room_id() != &parsed_room_id
                || transition_head.room_seq().get() != u64::try_from(expected_seq).unwrap_or(0)
                || transition_head.pack_digest() != head.pack_digest()
                || transition_head.core_schema_version() != head.core_schema_version()
                || transition.previous_lineage_hash() != &previous_hash
                || transition_head.genesis_or_transition_hash() != transition.transition_hash()
            {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "Transition lineage",
                });
            }
            previous_hash = transition.transition_hash().clone();
            final_core_bytes = transition
                .resulting_core_state()
                .canonical_bytes()
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Transition Core state",
                })?;
            final_activity_bytes =
                transition
                    .resulting_activity_state()
                    .to_bytes()
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Transition Activity state",
                    })?;
            expected_snapshots.push((
                u64::try_from(expected_seq).map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Transition sequence",
                    }
                })?,
                transition_head.canonical_bytes().map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Transition Head",
                    }
                })?,
                final_core_bytes.clone(),
                final_activity_bytes.clone(),
            ));
        }
        if head.genesis_or_transition_hash() != &previous_hash
            || (head.room_seq().get() == 0 && head != genesis.complete_head())
            || final_core_bytes != core_state_bytes
            || final_activity_bytes != activity_state_bytes
        {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "Head/materialization agreement",
            });
        }
        let snapshot_rows = client
            .query(
                "SELECT room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes FROM worldstream_room_snapshots WHERE room_id = $1 ORDER BY room_seq",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut snapshots = Vec::with_capacity(snapshot_rows.len());
        for row in snapshot_rows {
            let room_seq: i64 =
                row.try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Snapshot room sequence",
                    })?;
            let room_seq = u64::try_from(room_seq)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?;
            let complete_head_bytes: Vec<u8> =
                row.try_get(7)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Snapshot head column",
                    })?;
            let snapshot_head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
                &complete_head_bytes,
            )
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "Snapshot head decode",
            })?;
            if snapshot_head.canonical_bytes().ok().as_deref()
                != Some(complete_head_bytes.as_slice())
                || snapshot_head.room_id() != &parsed_room_id
                || snapshot_head.room_seq().get() != room_seq
                || row
                    .try_get::<_, String>(1)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.genesis_or_transition_hash().to_string()
                || row
                    .try_get::<_, String>(2)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.core_schema_version()
                || row
                    .try_get::<_, String>(3)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.pack_digest().to_string()
                || row
                    .try_get::<_, String>(4)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.core_state_hash().to_string()
                || row
                    .try_get::<_, String>(5)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.activity_state_hash().to_string()
                || row
                    .try_get::<_, String>(6)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?
                    != snapshot_head.authoritative_state_hash().to_string()
            {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "Snapshot head fields",
                });
            }
            let snapshot_core_bytes: Vec<u8> = row
                .try_get(8)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?;
            CanonicalJsonV1::decode_canonical::<worldstream_core::CoreRoomStateV1>(
                &snapshot_core_bytes,
            )
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "Snapshot Core bytes",
            })?;
            let snapshot_activity_bytes: Vec<u8> =
                row.try_get(9)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Snapshot Activity bytes",
                    })?;
            CanonicalJsonV1::from_canonical_bytes(&snapshot_activity_bytes)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Snapshot" })?;
            let Some((_, expected_head_bytes, expected_core_bytes, expected_activity_bytes)) =
                expected_snapshots.iter().find(|(seq, ..)| *seq == room_seq)
            else {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "Snapshot revision missing",
                });
            };
            if &complete_head_bytes != expected_head_bytes
                || &snapshot_core_bytes != expected_core_bytes
                || &snapshot_activity_bytes != expected_activity_bytes
            {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "Snapshot retained bytes",
                });
            }
            snapshots.push(PostgresSnapshotEvidenceV1 {
                room_seq,
                complete_head_bytes,
                core_state_bytes: snapshot_core_bytes,
                activity_state_bytes: snapshot_activity_bytes,
            });
        }
        let member_rows = client
            .query(
                "SELECT member_id, membership_bytes, frame_head, retained_frame_floor, last_ack_frame_seq, reset_required_through FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut canonical_membership_bytes = Vec::with_capacity(member_rows.len());
        let mut observation_positions = Vec::with_capacity(member_rows.len());
        for row in &member_rows {
            let member_id: String = row
                .try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
            let bytes: Vec<u8> = row
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
            let membership = CanonicalJsonV1::decode_canonical::<MembershipV1>(&bytes)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
            if membership.member_id().to_string() != member_id {
                return Err(PostgresRoomVerificationError::Corrupt { what: "Membership" });
            }
            let frame_head = u64::try_from(row.try_get::<_, i64>(2).map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                }
            })?)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            })?;
            let retained_frame_floor = u64::try_from(row.try_get::<_, i64>(3).map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                }
            })?)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            })?;
            if retained_frame_floor > frame_head.saturating_add(1) {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                });
            }
            let last_ack_frame_seq = row
                .try_get::<_, Option<i64>>(4)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                })?
                .map(|value| {
                    u64::try_from(value).map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation position",
                    })
                })
                .transpose()?;
            let reset_required_through = row
                .try_get::<_, Option<i64>>(5)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                })?
                .map(|value| {
                    u64::try_from(value).map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation position",
                    })
                })
                .transpose()?;
            if retained_frame_floor == 0
                || last_ack_frame_seq.is_some_and(|value| value == 0 || value > frame_head)
                || reset_required_through.is_some_and(|value| value > frame_head)
            {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                });
            }
            observation_positions.push(PostgresObservationPositionEvidenceV1 {
                member_id: member_id.clone(),
                frame_head,
                retained_frame_floor,
                last_ack_frame_seq,
                reset_required_through,
            });
            canonical_membership_bytes.push((member_id, bytes));
        }
        let timer_rows = client
            .query(
                "SELECT timer_id, generation, scheduled_for, payload_bytes, state FROM worldstream_timers WHERE room_id = $1 ORDER BY timer_id, generation",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut timers = Vec::with_capacity(timer_rows.len());
        for row in timer_rows {
            let generation: i64 = row
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?;
            timers.push(PostgresTimerEvidenceV1 {
                timer_id: row
                    .try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
                generation: u64::try_from(generation)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
                scheduled_for: row
                    .try_get(2)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
                payload_bytes: row
                    .try_get(3)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
                state: row
                    .try_get(4)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
            });
        }
        let frame_rows = client
            .query(
                "SELECT member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash FROM worldstream_frames WHERE room_id = $1 ORDER BY member_id, frame_seq",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut frames = Vec::with_capacity(frame_rows.len());
        for row in frame_rows {
            let frame_seq: i64 = row
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
            let cause_room_seq: i64 = row
                .try_get(2)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
            let member_id: String = row
                .try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
            let payload_bytes: Vec<u8> = row
                .try_get(3)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
            let payload_hash: Vec<u8> = row
                .try_get(4)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
            if member_id.parse::<worldstream_core::MemberId>().is_err()
                || RoomSequenceV1::new(
                    u64::try_from(cause_room_seq)
                        .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
                )
                .is_err()
                || payload_hash.as_slice() != Blake3DigestV1::hash(&payload_bytes).as_bytes()
            {
                return Err(PostgresRoomVerificationError::Corrupt { what: "Frame" });
            }
            frames.push(PostgresFrameEvidenceV1 {
                member_id,
                frame_seq: u64::try_from(frame_seq)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
                cause_room_seq: u64::try_from(cause_room_seq)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
                payload_bytes,
                payload_hash,
            });
        }
        let consequence_rows = client
            .query(
                "SELECT member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash FROM worldstream_observation_consequences WHERE room_id = $1 ORDER BY member_id, cause_room_seq",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut observation_consequences = Vec::with_capacity(consequence_rows.len());
        for row in consequence_rows {
            let member_id: String =
                row.try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation consequence",
                    })?;
            let cause_room_seq = u64::try_from(row.try_get::<_, i64>(1).map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                }
            })?)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation consequence",
            })?;
            if member_id.parse::<worldstream_core::MemberId>().is_err()
                || RoomSequenceV1::new(cause_room_seq).is_err()
            {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                });
            }
            let kind: String =
                row.try_get(2)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation consequence",
                    })?;
            let payload_bytes: Option<Vec<u8>> =
                row.try_get(3)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation consequence",
                    })?;
            let projection_hash: Option<Vec<u8>> =
                row.try_get(4)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "observation consequence",
                    })?;
            let consequence = match (kind.as_str(), payload_bytes, projection_hash) {
                ("reset_required", Some(payload_bytes), Some(projection_hash))
                    if projection_hash_for_canonical_bytes(&payload_bytes)
                        .is_ok_and(|digest| projection_hash.as_slice() == digest.as_bytes()) =>
                {
                    PostgresObservationConsequenceEvidenceV1::ResetRequired {
                        member_id,
                        cause_room_seq,
                        payload_bytes,
                        projection_hash,
                    }
                }
                ("visibility_lost", None, None) => {
                    PostgresObservationConsequenceEvidenceV1::VisibilityLost {
                        member_id,
                        cause_room_seq,
                    }
                }
                _ => {
                    return Err(PostgresRoomVerificationError::Corrupt {
                        what: "observation consequence",
                    });
                }
            };
            observation_consequences.push(consequence);
        }
        let decision_rows = client
            .query(
                "SELECT cause_room_seq, decision_id, target_member_id, decision_bytes FROM worldstream_activation_decisions WHERE room_id = $1 ORDER BY cause_room_seq, decision_id",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let mut activation_decisions = Vec::with_capacity(decision_rows.len());
        for row in decision_rows {
            let cause_room_seq: i64 =
                row.try_get(0)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Activation decision",
                    })?;
            activation_decisions.push(PostgresActivationDecisionEvidenceV1 {
                cause_room_seq: u64::try_from(cause_room_seq).map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Activation decision",
                    }
                })?,
                decision_id: row.try_get(1).map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Activation decision",
                    }
                })?,
                target_member_id: row.try_get(2).map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Activation decision",
                    }
                })?,
                decision_bytes: row.try_get(3).map_err(|_| {
                    PostgresRoomVerificationError::Corrupt {
                        what: "Activation decision",
                    }
                })?,
            });
        }
        let incident_rows = client
            .query(
                "SELECT incident_seq, generation, status, reason_code, details_bytes FROM worldstream_integrity_incidents WHERE room_id = $1 ORDER BY incident_seq",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
        let integrity_incidents = incident_rows
            .into_iter()
            .map(|row| {
                Ok(PostgresIntegrityIncidentEvidenceV1 {
                    incident_seq: u64::try_from(row.get::<_, i64>(0)).map_err(|_| {
                        PostgresRoomVerificationError::Corrupt {
                            what: "integrity incident",
                        }
                    })?,
                    generation: u64::try_from(row.get::<_, i64>(1)).map_err(|_| {
                        PostgresRoomVerificationError::Corrupt {
                            what: "integrity incident",
                        }
                    })?,
                    status: row.get(2),
                    reason_code: row.get(3),
                    details_bytes: row.get(4),
                })
            })
            .collect::<Result<Vec<_>, PostgresRoomVerificationError>>()?;
        Ok(PostgresRoomVerification {
            head,
            integrity_generation,
            integrity_status,
            transition_count: transition_rows.len(),
            member_count: member_rows.len(),
            head_bytes,
            pack_revision_lock_bytes: pack_lock_bytes,
            genesis_bytes,
            core_state_bytes,
            activity_state_bytes,
            transition_bytes: transition_rows
                .iter()
                .map(|row| row.try_get(1))
                .collect::<Result<Vec<Vec<u8>>, _>>()
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Transition" })?,
            membership_bytes: canonical_membership_bytes,
            observation_positions,
            timers,
            frames,
            observation_consequences,
            activation_decisions,
            snapshots,
            integrity_incidents,
        })
    }

    /// Reads target-wide canonical deployment lineage and storage-epoch
    /// metadata without executing DDL. Missing or malformed metadata fails
    /// closed.
    pub fn deployment_metadata_status(
        &self,
    ) -> Result<PostgresDeploymentMetadataStatus, PostgresDeploymentMetadataError> {
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresDeploymentMetadataError::Connection)?;
        let Some(row) = client
            .query_opt(
                "SELECT deployment_lineage_bytes, storage_epoch_bytes, storage_epoch FROM worldstream_deployment_metadata WHERE target_id = true",
                &[],
            )
            .map_err(PostgresDeploymentMetadataError::Sql)?
        else {
            return Err(PostgresDeploymentMetadataError::MissingMetadata);
        };
        let deployment_lineage_bytes: Vec<u8> = row
            .try_get(0)
            .map_err(PostgresDeploymentMetadataError::Sql)?;
        let storage_epoch_bytes: Vec<u8> = row
            .try_get(1)
            .map_err(PostgresDeploymentMetadataError::Sql)?;
        let storage_epoch: i64 = row
            .try_get(2)
            .map_err(PostgresDeploymentMetadataError::Sql)?;
        validate_deployment_metadata(
            &deployment_lineage_bytes,
            &storage_epoch_bytes,
            storage_epoch,
        )?;
        Ok(PostgresDeploymentMetadataStatus {
            table_present: true,
            deployment_lineage_bytes: Some(deployment_lineage_bytes),
            storage_epoch_bytes: Some(storage_epoch_bytes),
            storage_epoch: Some(storage_epoch),
        })
    }
}

/// Migration failures are kept separate from runtime storage outcomes.
#[derive(Debug, Error)]
pub enum PostgresMigrationError {
    #[error("PostgreSQL admin connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL migration failed: {0}")]
    Sql(postgres::Error),
    #[error("PostgreSQL migration history failed closed: {0}")]
    History(MigrationVerificationError),
    #[error("PostgreSQL capability check failed: {0}")]
    Capability(PostgresSchemaVerificationError),
    #[error("PostgreSQL schema verification failed: {0}")]
    Schema(PostgresSchemaVerificationError),
}

/// Errors from explicit offline cache and integrity maintenance.
#[derive(Debug, Error)]
pub enum PostgresMaintenanceError {
    #[error("PostgreSQL maintenance connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL maintenance query failed: {0}")]
    Sql(postgres::Error),
    #[error("Room verification failed during maintenance: {0}")]
    Verification(PostgresRoomVerificationError),
    #[error("canonical maintenance bytes are invalid")]
    Corrupt,
    #[error("integrity generation changed during maintenance")]
    ConcurrentChange,
}

/// Runtime/admin schema and capability verification failures.
#[derive(Debug, Error)]
pub enum PostgresSchemaVerificationError {
    #[error("PostgreSQL connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL verification query failed: {0}")]
    Sql(postgres::Error),
    #[error("PostgreSQL major {found:?} is unsupported; expected 17")]
    UnsupportedMajor { found: String },
    #[error("PostgreSQL version {found:?} is below the required {minimum:?}")]
    UnsupportedPatch {
        found: String,
        minimum: &'static str,
    },
    #[error("PostgreSQL server_version_num is malformed: {found:?}")]
    InvalidServerVersion { found: String },
    #[error("PostgreSQL synchronous_commit is {found:?}; expected on")]
    SynchronousCommit { found: String },
    #[error("PostgreSQL runtime role exceeds the reviewed least-privilege contract")]
    RuntimeRolePrivileges,
    #[error("PostgreSQL runtime refuses a disposable native-restore target")]
    DisposableRestoreTarget,
    #[error("PostgreSQL migration history failed closed: {0}")]
    History(MigrationVerificationError),
    #[error("PostgreSQL schema catalog fingerprint differs from the reviewed contract")]
    FingerprintDrift,
}

/// Closed failures for the authenticated PostgreSQL host diagnostic surface.
#[derive(Debug, Error)]
pub enum PostgresRoomDiagnosticErrorV1 {
    #[error("diagnostic authority failed: {0}")]
    Authority(#[from] AuthorityErrorV1),
    #[error("diagnostic target does not match the requested operation")]
    InvalidTarget,
    #[error("diagnostic page bounds are invalid")]
    InvalidBounds,
    #[error("diagnostic Room is unavailable")]
    RoomUnavailable,
    #[error("diagnostic storage is unavailable")]
    StorageUnavailable,
    #[error("diagnostic canonical identity is corrupt")]
    Corrupt,
}

/// Privacy-bounded PostgreSQL Room inventory row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresRoomDiagnosticSummaryV1 {
    room_id: RoomId,
    integrity: RoomIntegrityStateV1,
    head: CompleteHeadV1,
    pack_revision: PackRevisionLockV1,
}

impl PostgresRoomDiagnosticSummaryV1 {
    #[must_use]
    pub const fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    #[must_use]
    pub const fn integrity(&self) -> &RoomIntegrityStateV1 {
        &self.integrity
    }

    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }

    #[must_use]
    pub const fn pack_revision(&self) -> &PackRevisionLockV1 {
        &self.pack_revision
    }
}

/// Bounded durable Activation counts for one exact Room Membership. No
/// Activation identity or private invocation material crosses this seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PostgresActivationStatusV1 {
    waiting: u32,
    leased: u32,
}

impl PostgresActivationStatusV1 {
    #[must_use]
    pub const fn waiting(self) -> u32 {
        self.waiting
    }

    #[must_use]
    pub const fn leased(self) -> u32 {
        self.leased
    }
}

/// Stable Room-ID-ordered PostgreSQL inventory page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresRoomDiagnosticInventoryPageV1 {
    rooms: Vec<PostgresRoomDiagnosticSummaryV1>,
    next_after_room_id: Option<RoomId>,
}

impl PostgresRoomDiagnosticInventoryPageV1 {
    #[must_use]
    pub fn rooms(&self) -> &[PostgresRoomDiagnosticSummaryV1] {
        &self.rooms
    }

    #[must_use]
    pub const fn next_after_room_id(&self) -> Option<&RoomId> {
        self.next_after_room_id.as_ref()
    }
}

/// Read-only structural verification evidence for one persisted Room.
///
/// This reports verification of canonical bytes, lineage ordering,
/// materialization agreement, disposable snapshot caches, and operational
/// integrity incidents. The feature-gated conformance method separately
/// hydrates a Core trace from these retained bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresRoomVerification {
    /// The verified Complete Head.
    pub head: CompleteHeadV1,
    /// The durable operational integrity generation observed with the Head.
    pub integrity_generation: u64,
    /// The durable operational integrity status observed with the Head.
    pub integrity_status: String,
    /// Number of strictly decoded canonical Transitions.
    pub transition_count: usize,
    /// Number of strictly decoded Membership materializations.
    pub member_count: usize,
    /// Exact canonical Head bytes read from PostgreSQL.
    pub head_bytes: Vec<u8>,
    /// Exact canonical pack revision lock bytes read from PostgreSQL.
    pub pack_revision_lock_bytes: Vec<u8>,
    /// Exact canonical Genesis bytes read from PostgreSQL.
    pub genesis_bytes: Vec<u8>,
    /// Exact canonical Core materialization bytes read from PostgreSQL.
    pub core_state_bytes: Vec<u8>,
    /// Exact canonical Activity materialization bytes read from PostgreSQL.
    pub activity_state_bytes: Vec<u8>,
    /// Exact canonical Transition bytes in persisted sequence order.
    pub transition_bytes: Vec<Vec<u8>>,
    /// Exact canonical Membership bytes in deterministic member-id order.
    pub membership_bytes: Vec<(String, Vec<u8>)>,
    /// Durable frame-head and retained-prefix positions per Membership.
    pub observation_positions: Vec<PostgresObservationPositionEvidenceV1>,
    /// Timer rows persisted for this Room.
    pub timers: Vec<PostgresTimerEvidenceV1>,
    /// Observation frame rows persisted for this Room.
    pub frames: Vec<PostgresFrameEvidenceV1>,
    /// Non-frame observation consequence rows persisted for this Room.
    pub observation_consequences: Vec<PostgresObservationConsequenceEvidenceV1>,
    /// Activation decision rows persisted for this Room.
    pub activation_decisions: Vec<PostgresActivationDecisionEvidenceV1>,
    /// Disposable paired-snapshot cache rows persisted for this Room.
    pub snapshots: Vec<PostgresSnapshotEvidenceV1>,
    /// Durable integrity incidents in incident sequence order.
    pub integrity_incidents: Vec<PostgresIntegrityIncidentEvidenceV1>,
}

impl PostgresRoomVerification {
    /// Executes the exact retained Pack revision across the complete persisted
    /// Genesis-to-Head lineage and compares the replay result with every
    /// serving materialization captured by [`PostgresAdmin::verify_room`].
    ///
    /// Structural decoding alone cannot establish that retained Activity code
    /// still reduces the stored Transitions to their recorded results. This
    /// check therefore fails closed when the revision is absent, non-runnable,
    /// faults, or produces a different Head/Core/Activity materialization.
    ///
    /// # Errors
    ///
    /// Returns the closed recovery failure class without exposing provider or
    /// executor diagnostics.
    pub fn verify_executable_replay(
        &self,
        registry: &PackRegistryV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        if self.integrity_status != "healthy" {
            return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
        }
        let persisted_lock = PackRevisionLockV1::from_canonical_bytes(
            &self.pack_revision_lock_bytes,
            self.head.pack_digest(),
        )
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let retained = registry
            .load_retained(self.head.pack_digest())
            .map_err(|_| RoomRecoveryErrorV1::RuntimeUnavailable)?;
        if retained.revision_lock() != &persisted_lock {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let replay = CoreTraceV1::verify_executable_history_for_storage(
            registry,
            &self.head,
            &self.genesis_bytes,
            &self.transition_bytes,
            &self.core_state_bytes,
            &self.activity_state_bytes,
        )
        .map_err(|failure| match failure.class {
            ReplayFailureClassV1::RuntimeUnavailable => RoomRecoveryErrorV1::RuntimeUnavailable,
            ReplayFailureClassV1::RuntimeFault => RoomRecoveryErrorV1::RuntimeFault,
            _ => RoomRecoveryErrorV1::Corrupt,
        })?;
        verify_replayed_observation_evidence(&replay, self)
    }
}

/// Exact bytes for one persisted timer row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresTimerEvidenceV1 {
    pub timer_id: String,
    pub generation: u64,
    pub scheduled_for: String,
    pub payload_bytes: Vec<u8>,
    pub state: String,
}

/// Exact bytes for one persisted observation frame row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresFrameEvidenceV1 {
    pub member_id: String,
    pub frame_seq: u64,
    pub cause_room_seq: u64,
    pub payload_bytes: Vec<u8>,
    pub payload_hash: Vec<u8>,
}

/// Durable frame retention positions for one persisted Membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresObservationPositionEvidenceV1 {
    pub member_id: String,
    pub frame_head: u64,
    pub retained_frame_floor: u64,
    pub last_ack_frame_seq: Option<u64>,
    pub reset_required_through: Option<u64>,
}

/// Exact bytes for one persisted non-frame observation consequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostgresObservationConsequenceEvidenceV1 {
    ResetRequired {
        member_id: String,
        cause_room_seq: u64,
        payload_bytes: Vec<u8>,
        projection_hash: Vec<u8>,
    },
    VisibilityLost {
        member_id: String,
        cause_room_seq: u64,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ObservationConsequenceWitnessV1 {
    ResetRequired(String, u64, Vec<u8>),
    VisibilityLost(String, u64),
}

fn verify_replayed_frame_evidence(
    replay: &ReplayStorageVerificationV1,
    stored_positions: &[PostgresObservationPositionEvidenceV1],
    stored_frames: &[PostgresFrameEvidenceV1],
) -> Result<(), RoomRecoveryErrorV1> {
    let positions = stored_positions
        .iter()
        .map(|position| (position.member_id.as_str(), position))
        .collect::<BTreeMap<_, _>>();
    if positions.len() != stored_positions.len() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let mut replay_heads = positions
        .keys()
        .map(|member_id| (*member_id, 0_u64))
        .collect::<BTreeMap<_, _>>();
    let mut replayed_frames = Vec::new();
    for frame in replay.observation_frames() {
        let member_id = frame.member_id().as_str();
        let position = positions
            .get(member_id)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        let replay_head = replay_heads
            .get_mut(member_id)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        *replay_head = (*replay_head).max(frame.frame_seq());
        if frame.frame_seq() >= position.retained_frame_floor {
            replayed_frames.push((
                member_id.to_owned(),
                frame.frame_seq(),
                frame.cause_room_seq().get(),
                frame.payload_hash().as_bytes().to_vec(),
            ));
        }
    }
    if positions.iter().any(|(member_id, position)| {
        replay_heads.get(member_id).copied() != Some(position.frame_head)
    }) {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let mut stored_frame_witnesses = stored_frames
        .iter()
        .map(|frame| {
            (
                frame.member_id.clone(),
                frame.frame_seq,
                frame.cause_room_seq,
                frame.payload_hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    replayed_frames.sort_unstable();
    stored_frame_witnesses.sort_unstable();
    if replayed_frames != stored_frame_witnesses {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_replayed_nonframe_consequences(
    replay: &ReplayStorageVerificationV1,
    stored_consequences: &[PostgresObservationConsequenceEvidenceV1],
) -> Result<(), RoomRecoveryErrorV1> {
    let mut replayed_consequences = replay
        .observation_consequences()
        .iter()
        .map(|consequence| match consequence {
            RecoveredObservationConsequenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                projection_hash,
            } => ObservationConsequenceWitnessV1::ResetRequired(
                member_id.to_string(),
                cause_room_seq.get(),
                projection_hash.as_bytes().to_vec(),
            ),
            RecoveredObservationConsequenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => ObservationConsequenceWitnessV1::VisibilityLost(
                member_id.to_string(),
                cause_room_seq.get(),
            ),
        })
        .collect::<Vec<_>>();
    let mut stored_consequence_witnesses = stored_consequences
        .iter()
        .map(|consequence| match consequence {
            PostgresObservationConsequenceEvidenceV1::ResetRequired {
                member_id,
                cause_room_seq,
                projection_hash,
                ..
            } => ObservationConsequenceWitnessV1::ResetRequired(
                member_id.clone(),
                *cause_room_seq,
                projection_hash.clone(),
            ),
            PostgresObservationConsequenceEvidenceV1::VisibilityLost {
                member_id,
                cause_room_seq,
            } => {
                ObservationConsequenceWitnessV1::VisibilityLost(member_id.clone(), *cause_room_seq)
            }
        })
        .collect::<Vec<_>>();
    replayed_consequences.sort_unstable();
    stored_consequence_witnesses.sort_unstable();
    if replayed_consequences != stored_consequence_witnesses {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_replayed_membership_evidence(
    replay: &ReplayStorageVerificationV1,
    stored_memberships: &[(String, Vec<u8>)],
) -> Result<(), RoomRecoveryErrorV1> {
    let mut replayed_memberships = replay
        .memberships()
        .iter()
        .map(|membership| {
            (
                membership.member_id().to_string(),
                membership.canonical_membership_bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let mut stored_memberships = stored_memberships.to_vec();
    replayed_memberships.sort_unstable();
    stored_memberships.sort_unstable();
    if replayed_memberships != stored_memberships {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_replayed_timer_evidence(
    replay: &ReplayStorageVerificationV1,
    stored_timers: &[PostgresTimerEvidenceV1],
) -> Result<(), RoomRecoveryErrorV1> {
    let mut replayed_timers = replay
        .timers()
        .iter()
        .map(|timer| {
            (
                timer.timer_id().to_string(),
                timer.generation().get(),
                timer.scheduled_for().to_string(),
                timer.canonical_payload_bytes().to_vec(),
                match timer.state() {
                    RecoveredTimerStateV1::Scheduled => "scheduled",
                    RecoveredTimerStateV1::Fired => "fired",
                    RecoveredTimerStateV1::Cancelled => "cancelled",
                },
            )
        })
        .collect::<Vec<_>>();
    let mut stored_timers = stored_timers
        .iter()
        .map(|timer| {
            (
                timer.timer_id.clone(),
                timer.generation,
                timer.scheduled_for.clone(),
                timer.payload_bytes.clone(),
                timer.state.as_str(),
            )
        })
        .collect::<Vec<_>>();
    replayed_timers.sort_unstable();
    stored_timers.sort_unstable();
    if replayed_timers != stored_timers {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_replayed_activation_decision_evidence(
    replay: &ReplayStorageVerificationV1,
    stored_decisions: &[PostgresActivationDecisionEvidenceV1],
) -> Result<(), RoomRecoveryErrorV1> {
    let mut replayed_decisions = replay
        .activation_decisions()
        .iter()
        .map(|decision| {
            (
                decision.cause_room_seq().get(),
                decision.decision_id().to_owned(),
                decision.target_member_id().map(ToString::to_string),
                decision.canonical_decision_bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let mut stored_decisions = stored_decisions
        .iter()
        .map(|decision| {
            (
                decision.cause_room_seq,
                decision.decision_id.clone(),
                decision.target_member_id.clone(),
                decision.decision_bytes.clone(),
            )
        })
        .collect::<Vec<_>>();
    replayed_decisions.sort_unstable();
    stored_decisions.sort_unstable();
    if replayed_decisions != stored_decisions {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_replayed_position_evidence(
    replay: &ReplayStorageVerificationV1,
    stored_positions: &[PostgresObservationPositionEvidenceV1],
) -> Result<(), RoomRecoveryErrorV1> {
    let replayed_positions = replay
        .observation_positions()
        .iter()
        .map(|position| (position.member_id().as_str(), position))
        .collect::<BTreeMap<_, _>>();
    if replayed_positions.len() != stored_positions.len() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    for stored_position in stored_positions {
        let replayed = replayed_positions
            .get(stored_position.member_id.as_str())
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        if !observation_position_relation_is_valid(
            replayed.frame_head(),
            replayed.reset_required_through(),
            stored_position,
        ) {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    Ok(())
}

fn observation_position_relation_is_valid(
    replayed_frame_head: u64,
    replayed_reset_required_through: Option<u64>,
    stored: &PostgresObservationPositionEvidenceV1,
) -> bool {
    if replayed_frame_head != stored.frame_head
        || stored.retained_frame_floor == 0
        || stored.retained_frame_floor > stored.frame_head.saturating_add(1)
        || stored
            .last_ack_frame_seq
            .is_some_and(|cursor| cursor == 0 || cursor > stored.frame_head)
        || stored
            .reset_required_through
            .is_some_and(|marker| marker > stored.frame_head)
    {
        return false;
    }
    let cursor = stored.last_ack_frame_seq.unwrap_or(0);
    let lost_prefix = stored
        .retained_frame_floor
        .saturating_sub(1)
        .checked_sub(cursor)
        .filter(|distance| *distance > 0)
        .map(|_| stored.retained_frame_floor.saturating_sub(1));
    let unacknowledged_replay_reset =
        replayed_reset_required_through.filter(|marker| *marker > cursor && *marker > 0);
    let required_marker = lost_prefix
        .into_iter()
        .chain(unacknowledged_replay_reset)
        .max();
    required_marker.is_none_or(|minimum| {
        stored
            .reset_required_through
            .is_some_and(|marker| marker >= minimum && marker <= stored.frame_head)
    })
}

fn verify_replayed_materialization_evidence(
    replay: &ReplayStorageVerificationV1,
    stored: &PostgresRoomVerification,
) -> Result<(), RoomRecoveryErrorV1> {
    verify_replayed_membership_evidence(replay, &stored.membership_bytes)?;
    verify_replayed_timer_evidence(replay, &stored.timers)?;
    verify_replayed_activation_decision_evidence(replay, &stored.activation_decisions)?;
    verify_replayed_position_evidence(replay, &stored.observation_positions)
}

fn verify_replayed_observation_evidence(
    replay: &ReplayStorageVerificationV1,
    stored: &PostgresRoomVerification,
) -> Result<(), RoomRecoveryErrorV1> {
    verify_replayed_materialization_evidence(replay, stored)?;
    verify_replayed_frame_evidence(replay, &stored.observation_positions, &stored.frames)?;
    verify_replayed_nonframe_consequences(replay, &stored.observation_consequences)
}

/// Exact bytes and revision identity for one disposable paired snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresSnapshotEvidenceV1 {
    pub room_seq: u64,
    pub complete_head_bytes: Vec<u8>,
    pub core_state_bytes: Vec<u8>,
    pub activity_state_bytes: Vec<u8>,
}

/// Exact bytes for one persisted Activation decision row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresActivationDecisionEvidenceV1 {
    pub cause_room_seq: u64,
    pub decision_id: String,
    pub target_member_id: Option<String>,
    pub decision_bytes: Vec<u8>,
}

/// Exact bytes for one durable integrity incident.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresIntegrityIncidentEvidenceV1 {
    pub incident_seq: u64,
    pub generation: u64,
    pub status: String,
    pub reason_code: String,
    pub details_bytes: Option<Vec<u8>>,
}

/// Provider-neutral deployment identity evidence from the target-wide
/// canonical metadata row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresDeploymentMetadataStatus {
    /// Whether the reviewed deployment metadata table exists.
    pub table_present: bool,
    /// Deployment lineage bytes, when a future reviewed schema supplies them.
    pub deployment_lineage_bytes: Option<Vec<u8>>,
    /// Storage epoch bytes, when a future reviewed schema supplies them.
    pub storage_epoch_bytes: Option<Vec<u8>>,
    /// Safely validated numeric storage epoch used for fencing comparisons.
    pub storage_epoch: Option<i64>,
}

/// Fail-closed errors from read-only Room verification.
#[derive(Debug, Error)]
pub enum PostgresRoomVerificationError {
    #[error("PostgreSQL Room verification connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL Room verification query failed: {0}")]
    Sql(postgres::Error),
    #[error("PostgreSQL Room {room_id:?} was not found")]
    MissingRoom { room_id: String },
    #[error("PostgreSQL Room verification received an invalid Room ID")]
    InvalidRoomId,
    #[error("PostgreSQL Room verification found corrupt {what}")]
    Corrupt { what: &'static str },
}

/// Errors returned by the read-only deployment metadata probe.
#[derive(Debug, Error)]
pub enum PostgresDeploymentMetadataError {
    #[error("PostgreSQL deployment metadata probe connection failed: {0}")]
    Connection(postgres::Error),
    #[error("PostgreSQL deployment metadata probe query failed: {0}")]
    Sql(postgres::Error),
    #[error(
        "PostgreSQL deployment lineage and storage epoch metadata is absent from the reviewed schema"
    )]
    MissingMetadata,
    #[error("PostgreSQL deployment metadata row is malformed")]
    MalformedMetadata,
}

fn validate_deployment_metadata(
    deployment_lineage_bytes: &[u8],
    storage_epoch_bytes: &[u8],
    storage_epoch: i64,
) -> Result<(), PostgresDeploymentMetadataError> {
    if deployment_lineage_bytes.is_empty() || storage_epoch_bytes.is_empty() || storage_epoch < 0 {
        return Err(PostgresDeploymentMetadataError::MalformedMetadata);
    }
    Ok(())
}

fn parse_server_version_num(version: &str) -> Result<u32, PostgresSchemaVerificationError> {
    let version_num = version.parse::<u32>().map_err(|_| {
        PostgresSchemaVerificationError::InvalidServerVersion {
            found: version.to_owned(),
        }
    })?;
    if version_num / 10_000 != u32::from(POSTGRES_MAJOR) {
        return Err(PostgresSchemaVerificationError::UnsupportedMajor {
            found: version.to_owned(),
        });
    }
    if version_num < POSTGRES_MINIMUM_VERSION_NUM {
        return Err(PostgresSchemaVerificationError::UnsupportedPatch {
            found: version.to_owned(),
            minimum: POSTGRES_MINIMUM_PATCH,
        });
    }
    Ok(version_num)
}

fn format_postgres_engine_identity(server_version_num: u32) -> String {
    let major = server_version_num / 10_000;
    let release = server_version_num % 10_000;
    format!("postgresql/{major}.{release}; server_version_num={server_version_num}")
}

fn map_engine_identity_error(
    store: &PostgresRoomStore,
    error: PostgresSchemaVerificationError,
) -> PostgresEngineIdentityError {
    match error {
        PostgresSchemaVerificationError::Connection(error)
        | PostgresSchemaVerificationError::Sql(error) => {
            store.record_error(&error);
            PostgresEngineIdentityError::Unavailable
        }
        PostgresSchemaVerificationError::UnsupportedMajor { .. }
        | PostgresSchemaVerificationError::UnsupportedPatch { .. }
        | PostgresSchemaVerificationError::SynchronousCommit { .. }
        | PostgresSchemaVerificationError::RuntimeRolePrivileges
        | PostgresSchemaVerificationError::DisposableRestoreTarget => {
            PostgresEngineIdentityError::Unsupported
        }
        PostgresSchemaVerificationError::InvalidServerVersion { .. } => {
            PostgresEngineIdentityError::Corrupt
        }
        PostgresSchemaVerificationError::History(_)
        | PostgresSchemaVerificationError::FingerprintDrift => PostgresEngineIdentityError::Corrupt,
    }
}

fn verify_server_capabilities<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    read_verified_server_version_num(client).map(|_| ())
}

fn read_verified_server_version_num<C: GenericClient>(
    client: &mut C,
) -> Result<u32, PostgresSchemaVerificationError> {
    let version_row = client
        .query_one("SHOW server_version_num", &[])
        .map_err(PostgresSchemaVerificationError::Sql)?;
    let version: String = version_row
        .try_get(0)
        .map_err(PostgresSchemaVerificationError::Sql)?;
    let version_num = parse_server_version_num(&version)?;
    let commit_row = client
        .query_one("SHOW synchronous_commit", &[])
        .map_err(PostgresSchemaVerificationError::Sql)?;
    let synchronous_commit: String = commit_row
        .try_get(0)
        .map_err(PostgresSchemaVerificationError::Sql)?;
    if synchronous_commit != "on" {
        return Err(PostgresSchemaVerificationError::SynchronousCommit {
            found: synchronous_commit,
        });
    }
    Ok(version_num)
}

fn read_migration_records(client: &mut Client) -> Result<Vec<MigrationRecord>, postgres::Error> {
    let table = client.query_one(
        "SELECT to_regclass('public.worldstream_schema_migrations')::text",
        &[],
    )?;
    let exists: Option<String> = table.try_get(0)?;
    if exists.is_none() {
        return Ok(Vec::new());
    }
    // Migration 0001 predates the metadata columns.  Once 0002 has committed,
    // read the full ledger so administration verifies the same identity and
    // fingerprint fields that runtime admission verifies.  This avoids
    // silently accepting metadata drift while advancing a forward prefix.
    let metadata_columns: i64 = client
        .query_one(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'worldstream_schema_migrations' AND column_name IN ('logical_history_id', 'schema_contract_fingerprint')",
            &[],
        )?
        .try_get(0)?;
    let rows = if metadata_columns == 2 {
        client.query(
            "SELECT version, migration_id, checksum, logical_history_id, schema_contract_fingerprint FROM worldstream_schema_migrations ORDER BY version",
            &[],
        )?
        .into_iter()
        .map(|row| {
            Ok(MigrationRecord {
                version: row.try_get(0)?,
                migration_id: row.try_get(1)?,
                checksum: row.try_get(2)?,
                logical_history_id: row.try_get(3)?,
                schema_contract_fingerprint: row.try_get(4)?,
            })
        })
        .collect::<Result<Vec<_>, postgres::Error>>()?
    } else {
        client
            .query(
                "SELECT version, migration_id, checksum FROM worldstream_schema_migrations ORDER BY version",
                &[],
            )?
            .into_iter()
            .map(|row| {
                Ok(MigrationRecord {
                    version: row.try_get(0)?,
                    migration_id: row.try_get(1)?,
                    checksum: row.try_get(2)?,
                    logical_history_id: None,
                    schema_contract_fingerprint: None,
                })
            })
            .collect::<Result<Vec<_>, postgres::Error>>()?
    };
    Ok(rows)
}

fn read_current_migration_records<C: GenericClient>(
    client: &mut C,
) -> Result<Vec<MigrationRecord>, postgres::Error> {
    client
        .query(
            "SELECT version, migration_id, checksum, logical_history_id, schema_contract_fingerprint FROM worldstream_schema_migrations ORDER BY version",
            &[],
        )?
        .into_iter()
        .map(|row| {
            Ok(MigrationRecord {
                version: row.try_get(0)?,
                migration_id: row.try_get(1)?,
                checksum: row.try_get(2)?,
                logical_history_id: row.try_get(3)?,
                schema_contract_fingerprint: row.try_get(4)?,
            })
        })
        .collect()
}

const SCHEMA_TABLE_ORDER: &[&str] = &[
    "worldstream_schema_migrations",
    "worldstream_operation_guards",
    "worldstream_external_input_preparations",
    "worldstream_room_roots",
    "worldstream_genesis",
    "worldstream_materializations",
    "worldstream_members",
    "worldstream_timers",
    "worldstream_transitions",
    "worldstream_frames",
    "worldstream_observation_consequences",
    "worldstream_activation_decisions",
    "worldstream_activation_intents",
    "worldstream_activation_operation_receipts",
    "worldstream_room_snapshots",
    "worldstream_semantic_receipts",
    "worldstream_integrity_incidents",
    "worldstream_authority_fences",
    "worldstream_authority_state",
    "worldstream_authority_principals",
    "worldstream_authority_runners",
    "worldstream_authority_capabilities",
    "worldstream_authority_capability_scopes",
    "worldstream_authority_runner_capability_memberships",
    "worldstream_authority_change_receipts",
    "worldstream_authority_audit",
    "worldstream_transfer_imports",
    "worldstream_transfer_chunks",
    "worldstream_transfer_target_fence",
    "worldstream_deployment_metadata",
    "worldstream_deployment_identity_metadata",
    "worldstream_deployment_pack_identities",
    "worldstream_deployment_resource_identities",
    "worldstream_deployment_resource_blobs",
    "worldstream_retired_authority_fences_v1",
];

fn schema_catalog_material<C: GenericClient>(client: &mut C) -> Result<String, postgres::Error> {
    let rows = client.query(
        "SELECT table_name, column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = 'public' AND table_name LIKE 'worldstream_%' ORDER BY table_name, ordinal_position",
        &[],
    )?;
    let mut tables = std::collections::BTreeMap::<String, Vec<String>>::new();
    for row in rows {
        let table: String = row.try_get(0)?;
        let column: String = row.try_get(1)?;
        let data_type: String = row.try_get(2)?;
        let nullable: String = row.try_get(3)?;
        tables
            .entry(table)
            .or_default()
            .push(format!("{column}:{data_type}:{nullable}"));
    }
    // Keep catalog verification in the reviewed schema-contract order. The
    // fingerprint is intentionally independent of PostgreSQL's catalog sort
    // order, which is alphabetical rather than migration-contract order.
    let mut material = String::new();
    for table in SCHEMA_TABLE_ORDER {
        if let Some(columns) = tables.remove(*table) {
            let _ = write!(material, "{table}({});", columns.join(","));
        }
    }
    for (table, columns) in tables {
        let _ = write!(material, "{table}({});", columns.join(","));
    }
    Ok(material)
}

const GLOBAL_RESOURCE_IDENTITY_INDEXES: [(&str, &str); 2] = [
    (
        "worldstream_deployment_resource_identity_global_v1",
        "worldstream_deployment_resource_identities",
    ),
    (
        "worldstream_deployment_resource_blob_identity_global_v1",
        "worldstream_deployment_resource_blobs",
    ),
];

const TRANSFER_FENCE_TRIGGER_TABLES: [(&str, &str); 31] = [
    (
        "worldstream_transfer_fence_operation_guards",
        "worldstream_operation_guards",
    ),
    (
        "worldstream_transfer_fence_external_input_preparations",
        "worldstream_external_input_preparations",
    ),
    (
        "worldstream_transfer_fence_room_roots",
        "worldstream_room_roots",
    ),
    ("worldstream_transfer_fence_genesis", "worldstream_genesis"),
    (
        "worldstream_transfer_fence_materializations",
        "worldstream_materializations",
    ),
    ("worldstream_transfer_fence_members", "worldstream_members"),
    ("worldstream_transfer_fence_timers", "worldstream_timers"),
    (
        "worldstream_transfer_fence_transitions",
        "worldstream_transitions",
    ),
    ("worldstream_transfer_fence_frames", "worldstream_frames"),
    (
        "worldstream_transfer_fence_observation_consequences",
        "worldstream_observation_consequences",
    ),
    (
        "worldstream_transfer_fence_activation_decisions",
        "worldstream_activation_decisions",
    ),
    (
        "worldstream_transfer_fence_activation_intents",
        "worldstream_activation_intents",
    ),
    (
        "worldstream_transfer_fence_activation_receipts",
        "worldstream_activation_operation_receipts",
    ),
    (
        "worldstream_transfer_fence_room_snapshots",
        "worldstream_room_snapshots",
    ),
    (
        "worldstream_transfer_fence_semantic_receipts",
        "worldstream_semantic_receipts",
    ),
    (
        "worldstream_transfer_fence_integrity_incidents",
        "worldstream_integrity_incidents",
    ),
    (
        "worldstream_transfer_fence_authority_fences",
        "worldstream_authority_fences",
    ),
    (
        "worldstream_transfer_fence_authority_state",
        "worldstream_authority_state",
    ),
    (
        "worldstream_transfer_fence_authority_principals",
        "worldstream_authority_principals",
    ),
    (
        "worldstream_transfer_fence_authority_runners",
        "worldstream_authority_runners",
    ),
    (
        "worldstream_transfer_fence_authority_capabilities",
        "worldstream_authority_capabilities",
    ),
    (
        "worldstream_transfer_fence_authority_scopes",
        "worldstream_authority_capability_scopes",
    ),
    (
        "worldstream_transfer_fence_authority_runner_memberships",
        "worldstream_authority_runner_capability_memberships",
    ),
    (
        "worldstream_transfer_fence_authority_change_receipts",
        "worldstream_authority_change_receipts",
    ),
    (
        "worldstream_transfer_fence_authority_audit",
        "worldstream_authority_audit",
    ),
    (
        "worldstream_transfer_fence_deployment_metadata",
        "worldstream_deployment_metadata",
    ),
    (
        "worldstream_transfer_fence_deployment_identity_metadata",
        "worldstream_deployment_identity_metadata",
    ),
    (
        "worldstream_transfer_fence_deployment_packs",
        "worldstream_deployment_pack_identities",
    ),
    (
        "worldstream_transfer_fence_deployment_resources",
        "worldstream_deployment_resource_identities",
    ),
    (
        "worldstream_transfer_fence_deployment_resource_blobs",
        "worldstream_deployment_resource_blobs",
    ),
    (
        "worldstream_transfer_fence_retired_authority_fences",
        "worldstream_retired_authority_fences_v1",
    ),
];

const TRANSFER_FENCE_FUNCTION_BODY: &str = r"BEGIN
    IF EXISTS (SELECT 1 FROM public.worldstream_transfer_target_fence WHERE fence_id = true) THEN
        RAISE EXCEPTION 'WorldStream target is non-serving during transfer';
    END IF;
    RETURN NULL;
END;";

fn verify_global_resource_identity_indexes<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    let index_rows = client
        .query(
            "SELECT index_relation.relname, table_relation.relname, index.indisunique, \
                    index.indisvalid, index.indisready, index.indnkeyatts::integer, \
                    pg_get_indexdef(index.indexrelid, 1, true), \
                    index.indpred IS NULL, index.indexprs IS NULL \
             FROM pg_catalog.pg_index AS index \
             JOIN pg_catalog.pg_class AS index_relation ON index_relation.oid = index.indexrelid \
             JOIN pg_catalog.pg_class AS table_relation ON table_relation.oid = index.indrelid \
             JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = table_relation.relnamespace \
             WHERE namespace.nspname = 'public' \
               AND index_relation.relname IN ( \
                   'worldstream_deployment_resource_identity_global_v1', \
                   'worldstream_deployment_resource_blob_identity_global_v1') \
             ORDER BY index_relation.relname",
            &[],
        )
        .map_err(PostgresSchemaVerificationError::Sql)?;
    let mut indexes = BTreeMap::new();
    for row in index_rows {
        let name: String = row
            .try_get(0)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        let table: String = row
            .try_get(1)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        let valid = row
            .try_get::<_, bool>(2)
            .map_err(PostgresSchemaVerificationError::Sql)?
            && row
                .try_get::<_, bool>(3)
                .map_err(PostgresSchemaVerificationError::Sql)?
            && row
                .try_get::<_, bool>(4)
                .map_err(PostgresSchemaVerificationError::Sql)?
            && row
                .try_get::<_, i32>(5)
                .map_err(PostgresSchemaVerificationError::Sql)?
                == 1
            && row
                .try_get::<_, String>(6)
                .map_err(PostgresSchemaVerificationError::Sql)?
                == "resource_identity"
            && row
                .try_get::<_, bool>(7)
                .map_err(PostgresSchemaVerificationError::Sql)?
            && row
                .try_get::<_, bool>(8)
                .map_err(PostgresSchemaVerificationError::Sql)?;
        if !valid || indexes.insert(name, table).is_some() {
            return Err(PostgresSchemaVerificationError::FingerprintDrift);
        }
    }
    if indexes.len() != GLOBAL_RESOURCE_IDENTITY_INDEXES.len()
        || GLOBAL_RESOURCE_IDENTITY_INDEXES
            .iter()
            .any(|(name, table)| indexes.get(*name).map(String::as_str) != Some(*table))
    {
        return Err(PostgresSchemaVerificationError::FingerprintDrift);
    }
    Ok(())
}

fn verify_transfer_fence_triggers<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    let trigger_rows = client
        .query(
            "SELECT trigger.tgname, relation.relname, trigger.tgtype::integer, \
                    trigger.tgenabled::text, routine.proname \
             FROM pg_catalog.pg_trigger AS trigger \
             JOIN pg_catalog.pg_class AS relation ON relation.oid = trigger.tgrelid \
             JOIN pg_catalog.pg_namespace AS relation_namespace \
               ON relation_namespace.oid = relation.relnamespace \
             JOIN pg_catalog.pg_proc AS routine ON routine.oid = trigger.tgfoid \
             JOIN pg_catalog.pg_namespace AS routine_namespace \
               ON routine_namespace.oid = routine.pronamespace \
             WHERE relation_namespace.nspname = 'public' \
               AND routine_namespace.nspname = 'public' \
               AND NOT trigger.tgisinternal \
               AND trigger.tgname LIKE 'worldstream_transfer_fence_%' \
             ORDER BY trigger.tgname",
            &[],
        )
        .map_err(PostgresSchemaVerificationError::Sql)?;
    let mut triggers = BTreeMap::new();
    for row in trigger_rows {
        let name: String = row
            .try_get(0)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        let table: String = row
            .try_get(1)
            .map_err(PostgresSchemaVerificationError::Sql)?;
        let valid = row
            .try_get::<_, i32>(2)
            .map_err(PostgresSchemaVerificationError::Sql)?
            == 62
            && row
                .try_get::<_, String>(3)
                .map_err(PostgresSchemaVerificationError::Sql)?
                == "O"
            && row
                .try_get::<_, String>(4)
                .map_err(PostgresSchemaVerificationError::Sql)?
                == "worldstream_reject_write_while_transfer_fenced";
        if !valid || triggers.insert(name, table).is_some() {
            return Err(PostgresSchemaVerificationError::FingerprintDrift);
        }
    }
    if triggers.len() != TRANSFER_FENCE_TRIGGER_TABLES.len()
        || TRANSFER_FENCE_TRIGGER_TABLES
            .iter()
            .any(|(name, table)| triggers.get(*name).map(String::as_str) != Some(*table))
    {
        return Err(PostgresSchemaVerificationError::FingerprintDrift);
    }
    Ok(())
}

fn verify_transfer_fence_function<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    let function_rows = client
        .query(
            "SELECT pg_get_function_result(routine.oid), language.lanname, \
                    routine.provolatile::text, routine.prosecdef, routine.proleakproof, \
                    routine.proparallel::text, routine.pronargs::integer, \
                    routine.proconfig IS NULL, routine.prosrc \
             FROM pg_catalog.pg_proc AS routine \
             JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = routine.pronamespace \
             JOIN pg_catalog.pg_language AS language ON language.oid = routine.prolang \
             WHERE namespace.nspname = 'public' \
               AND routine.proname = 'worldstream_reject_write_while_transfer_fenced' \
             ORDER BY routine.oid",
            &[],
        )
        .map_err(PostgresSchemaVerificationError::Sql)?;
    if function_rows.len() != 1 {
        return Err(PostgresSchemaVerificationError::FingerprintDrift);
    }
    let function = &function_rows[0];
    if function
        .try_get::<_, String>(0)
        .map_err(PostgresSchemaVerificationError::Sql)?
        != "trigger"
        || function
            .try_get::<_, String>(1)
            .map_err(PostgresSchemaVerificationError::Sql)?
            != "plpgsql"
        || function
            .try_get::<_, String>(2)
            .map_err(PostgresSchemaVerificationError::Sql)?
            != "v"
        || function
            .try_get::<_, bool>(3)
            .map_err(PostgresSchemaVerificationError::Sql)?
        || function
            .try_get::<_, bool>(4)
            .map_err(PostgresSchemaVerificationError::Sql)?
        || function
            .try_get::<_, String>(5)
            .map_err(PostgresSchemaVerificationError::Sql)?
            != "u"
        || function
            .try_get::<_, i32>(6)
            .map_err(PostgresSchemaVerificationError::Sql)?
            != 0
        || !function
            .try_get::<_, bool>(7)
            .map_err(PostgresSchemaVerificationError::Sql)?
        || function
            .try_get::<_, String>(8)
            .map_err(PostgresSchemaVerificationError::Sql)?
            .trim()
            != TRANSFER_FENCE_FUNCTION_BODY
    {
        return Err(PostgresSchemaVerificationError::FingerprintDrift);
    }
    Ok(())
}

fn verify_schema_safety_catalog<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    verify_global_resource_identity_indexes(client)?;
    verify_transfer_fence_triggers(client)?;
    verify_transfer_fence_function(client)
}

fn verify_runtime_schema_client(
    client: &mut Client,
) -> Result<(), PostgresSchemaVerificationError> {
    let mut transaction = client
        .transaction()
        .map_err(PostgresSchemaVerificationError::Sql)?;
    verify_runtime_schema(&mut transaction)?;
    transaction
        .commit()
        .map_err(PostgresSchemaVerificationError::Sql)
}

fn verify_runtime_schema<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    verify_server_capabilities(client)?;
    let records =
        read_current_migration_records(client).map_err(PostgresSchemaVerificationError::Sql)?;
    verify_runtime_migration_history(&records).map_err(PostgresSchemaVerificationError::History)?;
    let material = schema_catalog_material(client).map_err(PostgresSchemaVerificationError::Sql)?;
    if material != SCHEMA_FINGERPRINT_MATERIAL {
        return Err(PostgresSchemaVerificationError::FingerprintDrift);
    }
    verify_schema_safety_catalog(client)?;
    Ok(())
}

fn frozen_schema_column_count() -> usize {
    SCHEMA_FINGERPRINT_MATERIAL
        .split(';')
        .filter_map(|table| table.split_once('('))
        .map(|(_, columns)| {
            let columns = columns.trim_end_matches(')');
            usize::from(!columns.is_empty()) + columns.matches(',').count()
        })
        .sum()
}

pub(crate) fn verify_runtime_schema_for_native_restore<C: GenericClient>(
    client: &mut C,
    budget: &mut ProviderReadBudgetV1,
) -> Result<(), PostgresSchemaVerificationError> {
    let mut migration_rows = migration_history().len();
    preflight_native_global_provider_reads(
        client,
        "SELECT jsonb_build_array(version, migration_id, checksum, logical_history_id, schema_contract_fingerprint)::text FROM worldstream_schema_migrations ORDER BY version",
        &mut migration_rows,
        budget,
    )?;
    let mut schema_columns = frozen_schema_column_count();
    preflight_native_global_provider_reads(
        client,
        "SELECT jsonb_build_array(table_name, column_name, data_type, is_nullable)::text FROM information_schema.columns WHERE table_schema = 'public' AND table_name LIKE 'worldstream_%' ORDER BY table_name, ordinal_position",
        &mut schema_columns,
        budget,
    )?;
    let mut index_rows = GLOBAL_RESOURCE_IDENTITY_INDEXES.len();
    preflight_native_global_provider_reads(
        client,
        "SELECT jsonb_build_array(index_relation.relname, table_relation.relname, index.indisunique, index.indisvalid, index.indisready, index.indnkeyatts::integer, pg_get_indexdef(index.indexrelid, 1, true), index.indpred IS NULL, index.indexprs IS NULL)::text FROM pg_catalog.pg_index AS index JOIN pg_catalog.pg_class AS index_relation ON index_relation.oid = index.indexrelid JOIN pg_catalog.pg_class AS table_relation ON table_relation.oid = index.indrelid JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = table_relation.relnamespace WHERE namespace.nspname = 'public' AND index_relation.relname IN ('worldstream_deployment_resource_identity_global_v1', 'worldstream_deployment_resource_blob_identity_global_v1') ORDER BY index_relation.relname",
        &mut index_rows,
        budget,
    )?;
    let mut trigger_rows = TRANSFER_FENCE_TRIGGER_TABLES.len();
    preflight_native_global_provider_reads(
        client,
        "SELECT jsonb_build_array(trigger.tgname, relation.relname, trigger.tgtype::integer, trigger.tgenabled::text, routine.proname)::text FROM pg_catalog.pg_trigger AS trigger JOIN pg_catalog.pg_class AS relation ON relation.oid = trigger.tgrelid JOIN pg_catalog.pg_namespace AS relation_namespace ON relation_namespace.oid = relation.relnamespace JOIN pg_catalog.pg_proc AS routine ON routine.oid = trigger.tgfoid JOIN pg_catalog.pg_namespace AS routine_namespace ON routine_namespace.oid = routine.pronamespace WHERE relation_namespace.nspname = 'public' AND routine_namespace.nspname = 'public' AND NOT trigger.tgisinternal AND trigger.tgname LIKE 'worldstream_transfer_fence_%' ORDER BY trigger.tgname",
        &mut trigger_rows,
        budget,
    )?;
    let mut function_rows = 1;
    preflight_native_global_provider_reads(
        client,
        "SELECT jsonb_build_array(pg_get_function_result(routine.oid), language.lanname, routine.provolatile::text, routine.prosecdef, routine.proleakproof, routine.proparallel::text, routine.pronargs::integer, routine.proconfig IS NULL, routine.prosrc)::text FROM pg_catalog.pg_proc AS routine JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = routine.pronamespace JOIN pg_catalog.pg_language AS language ON language.oid = routine.prolang WHERE namespace.nspname = 'public' AND routine.proname = 'worldstream_reject_write_while_transfer_fenced' ORDER BY routine.oid",
        &mut function_rows,
        budget,
    )?;
    verify_runtime_schema(client)
}

// Each bit is an independently queried PostgreSQL privilege and each is
// exercised separately by the closed admission tests below.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RuntimeRoleAdmissionV1 {
    superuser: bool,
    create_role: bool,
    create_database: bool,
    replication: bool,
    bypass_row_security: bool,
    database_create: bool,
    other_role_membership: bool,
    public_schema_create: bool,
    owns_public_schema_object: bool,
    migration_insert: bool,
    migration_update: bool,
    migration_delete: bool,
    migration_truncate: bool,
    transfer_control_insert: bool,
    transfer_control_update: bool,
    transfer_control_delete: bool,
    transfer_control_truncate: bool,
}

fn validate_runtime_role_admission(
    admission: RuntimeRoleAdmissionV1,
) -> Result<(), PostgresSchemaVerificationError> {
    if admission.superuser
        || admission.create_role
        || admission.create_database
        || admission.replication
        || admission.bypass_row_security
        || admission.database_create
        || admission.other_role_membership
        || admission.public_schema_create
        || admission.owns_public_schema_object
        || admission.migration_insert
        || admission.migration_update
        || admission.migration_delete
        || admission.migration_truncate
        || admission.transfer_control_insert
        || admission.transfer_control_update
        || admission.transfer_control_delete
        || admission.transfer_control_truncate
    {
        Err(PostgresSchemaVerificationError::RuntimeRolePrivileges)
    } else {
        Ok(())
    }
}

const RUNTIME_ROLE_ADMISSION_SQL: &str = "SELECT role.rolsuper, role.rolcreaterole, role.rolcreatedb, \
                    role.rolreplication, role.rolbypassrls, \
                    has_database_privilege(current_user, current_database(), 'CREATE'), \
                    EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership \
                            WHERE membership.member = role.oid), \
                    has_schema_privilege(current_user, 'public', 'CREATE'), \
                    EXISTS ( \
                        SELECT 1 FROM pg_catalog.pg_namespace AS namespace_row \
                         WHERE namespace_row.nspname = 'public' AND namespace_row.nspowner = role.oid \
                        UNION ALL \
                        SELECT 1 FROM pg_catalog.pg_class AS relation_row \
                         JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = relation_row.relnamespace \
                         WHERE namespace_row.nspname = 'public' AND relation_row.relowner = role.oid \
                        UNION ALL \
                        SELECT 1 FROM pg_catalog.pg_proc AS routine_row \
                         JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = routine_row.pronamespace \
                         WHERE namespace_row.nspname = 'public' AND routine_row.proowner = role.oid \
                        UNION ALL \
                        SELECT 1 FROM pg_catalog.pg_type AS type_row \
                         JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = type_row.typnamespace \
                         WHERE namespace_row.nspname = 'public' AND type_row.typowner = role.oid \
                    ), \
                    has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'INSERT'), \
                    has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'UPDATE'), \
                    has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'DELETE'), \
                    has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'TRUNCATE'), \
                    EXISTS (SELECT 1 FROM unnest(ARRAY[ \
                        'public.worldstream_transfer_imports', \
                        'public.worldstream_transfer_chunks', \
                        'public.worldstream_transfer_target_fence']::text[]) AS protected_table(name) \
                        WHERE has_table_privilege(current_user, protected_table.name, 'INSERT')), \
                    EXISTS (SELECT 1 FROM unnest(ARRAY[ \
                        'public.worldstream_transfer_imports', \
                        'public.worldstream_transfer_chunks', \
                        'public.worldstream_transfer_target_fence']::text[]) AS protected_table(name) \
                        WHERE has_table_privilege(current_user, protected_table.name, 'UPDATE')), \
                    EXISTS (SELECT 1 FROM unnest(ARRAY[ \
                        'public.worldstream_transfer_imports', \
                        'public.worldstream_transfer_chunks', \
                        'public.worldstream_transfer_target_fence']::text[]) AS protected_table(name) \
                        WHERE has_table_privilege(current_user, protected_table.name, 'DELETE')), \
                    EXISTS (SELECT 1 FROM unnest(ARRAY[ \
                        'public.worldstream_transfer_imports', \
                        'public.worldstream_transfer_chunks', \
                        'public.worldstream_transfer_target_fence']::text[]) AS protected_table(name) \
                        WHERE has_table_privilege(current_user, protected_table.name, 'TRUNCATE')) \
             FROM pg_catalog.pg_roles AS role WHERE role.rolname = current_user";

fn runtime_role_admission_from_row(
    row: &postgres::Row,
) -> Result<RuntimeRoleAdmissionV1, PostgresSchemaVerificationError> {
    Ok(RuntimeRoleAdmissionV1 {
        superuser: row
            .try_get(0)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        create_role: row
            .try_get(1)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        create_database: row
            .try_get(2)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        replication: row
            .try_get(3)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        bypass_row_security: row
            .try_get(4)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        database_create: row
            .try_get(5)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        other_role_membership: row
            .try_get(6)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        public_schema_create: row
            .try_get(7)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        owns_public_schema_object: row
            .try_get(8)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        migration_insert: row
            .try_get(9)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        migration_update: row
            .try_get(10)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        migration_delete: row
            .try_get(11)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        migration_truncate: row
            .try_get(12)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        transfer_control_insert: row
            .try_get(13)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        transfer_control_update: row
            .try_get(14)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        transfer_control_delete: row
            .try_get(15)
            .map_err(PostgresSchemaVerificationError::Sql)?,
        transfer_control_truncate: row
            .try_get(16)
            .map_err(PostgresSchemaVerificationError::Sql)?,
    })
}

fn verify_runtime_role<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    let row = client
        .query_one(RUNTIME_ROLE_ADMISSION_SQL, &[])
        .map_err(PostgresSchemaVerificationError::Sql)?;
    validate_runtime_role_admission(runtime_role_admission_from_row(&row)?)
}

fn validate_runtime_database_marker(
    marker: Option<&str>,
) -> Result<(), PostgresSchemaVerificationError> {
    if marker == Some(native_restore::NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1) {
        Err(PostgresSchemaVerificationError::DisposableRestoreTarget)
    } else {
        Ok(())
    }
}

fn verify_runtime_database_marker<C: GenericClient>(
    client: &mut C,
) -> Result<(), PostgresSchemaVerificationError> {
    let marker = client
        .query_one(
            "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = current_database()",
            &[],
        )
        .map_err(PostgresSchemaVerificationError::Sql)?
        .try_get::<_, Option<String>>(0)
        .map_err(PostgresSchemaVerificationError::Sql)?;
    validate_runtime_database_marker(marker.as_deref())
}

fn verify_runtime_store_client(client: &mut Client) -> Result<(), PostgresSchemaVerificationError> {
    let mut transaction = client
        .transaction()
        .map_err(PostgresSchemaVerificationError::Sql)?;
    verify_runtime_schema(&mut transaction)?;
    verify_runtime_role(&mut transaction)?;
    verify_runtime_database_marker(&mut transaction)?;
    transaction
        .commit()
        .map_err(PostgresSchemaVerificationError::Sql)
}

fn map_postgres_diagnostic_authority_store_error(
    error: AuthorityStoreErrorV1,
) -> PostgresRoomDiagnosticErrorV1 {
    match error {
        AuthorityStoreErrorV1::Corrupt => PostgresRoomDiagnosticErrorV1::Corrupt,
        AuthorityStoreErrorV1::Conflict
        | AuthorityStoreErrorV1::InvalidChange
        | AuthorityStoreErrorV1::StaleGeneration
        | AuthorityStoreErrorV1::Unavailable => PostgresRoomDiagnosticErrorV1::StorageUnavailable,
    }
}

fn decode_postgres_diagnostic_summary(
    row: &Row,
) -> Result<PostgresRoomDiagnosticSummaryV1, PostgresRoomDiagnosticErrorV1> {
    let room_id: String = row
        .try_get(0)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let head_bytes: Vec<u8> = row
        .try_get(1)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let pack_revision_lock_bytes: Vec<u8> = row
        .try_get(2)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let integrity_status: String = row
        .try_get(3)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let integrity_generation: i64 = row
        .try_get(4)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let room_id = room_id
        .parse::<RoomId>()
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
        .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    if head.room_id() != &room_id || head.canonical_bytes().ok().as_deref() != Some(&head_bytes) {
        return Err(PostgresRoomDiagnosticErrorV1::Corrupt);
    }
    let pack_revision =
        PackRevisionLockV1::from_canonical_bytes(&pack_revision_lock_bytes, head.pack_digest())
            .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
    let status = match integrity_status.as_str() {
        "healthy" => RoomIntegrityStatusV1::Healthy,
        "faulted" => RoomIntegrityStatusV1::Faulted,
        "quarantined" => RoomIntegrityStatusV1::Quarantined,
        _ => return Err(PostgresRoomDiagnosticErrorV1::Corrupt),
    };
    let generation = u64::try_from(integrity_generation)
        .ok()
        .and_then(|value| IntegrityGenerationV1::new(value).ok())
        .ok_or(PostgresRoomDiagnosticErrorV1::Corrupt)?;
    Ok(PostgresRoomDiagnosticSummaryV1 {
        room_id,
        integrity: RoomIntegrityStateV1::new(status, generation),
        head,
        pack_revision,
    })
}

/// Runtime PostgreSQL adapter implementing exactly Core's two-method mutation port.
pub struct PostgresRoomStore {
    config: PostgresConnectionConfig,
    telemetry: Option<Arc<dyn PostgresTelemetrySink>>,
    failpoint: Mutex<Option<PostgresFailpoint>>,
    last_failure: Mutex<Option<PostgresStorageFailure>>,
    #[cfg(feature = "conformance-tracer")]
    conformance_capability_commit: AtomicBool,
}

impl fmt::Debug for PostgresRoomStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresRoomStore([REDACTED])")
    }
}

impl PostgresRoomStore {
    /// Creates a runtime adapter. Schema creation remains an admin-only operation.
    pub fn new(config: PostgresConnectionConfig) -> Result<Self, PostgresConfigError> {
        if config.profile != PostgresProfile::RuntimeLeastPrivilege {
            return Err(PostgresConfigError::RuntimeProfileRequired);
        }
        Ok(Self {
            config,
            telemetry: None,
            failpoint: Mutex::new(None),
            last_failure: Mutex::new(None),
            #[cfg(feature = "conformance-tracer")]
            conformance_capability_commit: AtomicBool::new(false),
        })
    }

    /// Attaches a best-effort storage telemetry sink.  The sink is called
    /// only with closed facts and is panic-isolated from runtime operations.
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: Arc<dyn PostgresTelemetrySink>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Returns the configured connection path, including the pooler-safe path.
    #[must_use]
    pub const fn connection_path(&self) -> PostgresConnectionPath {
        self.config.path
    }

    /// Samples the provider clock used by production authority fences.
    pub fn authority_checked_at(&self) -> Result<AuthorityCheckedAt, PostgresAuthorityClockError> {
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            PostgresAuthorityClockError::Unavailable
        })?;
        postgres_authority_checked_at(&mut client).map_err(|error| match error {
            AuthorityStoreErrorV1::Corrupt => PostgresAuthorityClockError::Corrupt,
            AuthorityStoreErrorV1::Conflict
            | AuthorityStoreErrorV1::InvalidChange
            | AuthorityStoreErrorV1::StaleGeneration
            | AuthorityStoreErrorV1::Unavailable => PostgresAuthorityClockError::Unavailable,
        })
    }

    /// Durably binds the first sampled Recorded Time to one exact
    /// ExternalInput operation identity and canonical request hash.
    ///
    /// Identical retries, including after a process restart, return the first
    /// stored value. Reusing the identity with another hash fails closed and
    /// never replaces the retained Semantic Time.
    pub fn reserve_external_input_recorded_at(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
        sampled_recorded_at: &ExternalInputRecordedAt,
    ) -> Result<ExternalInputRecordedAt, PostgresExternalInputPreparationErrorV1> {
        if !matches!(identity, OperationIdentityV1::ExternalInput(_)) {
            return Err(PostgresExternalInputPreparationErrorV1::Corrupt);
        }
        let identity_bytes = identity
            .canonical_bytes()
            .map_err(|_| PostgresExternalInputPreparationErrorV1::Corrupt)?;
        let request_hash_bytes = request_hash.as_bytes().as_slice();
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            PostgresExternalInputPreparationErrorV1::StorageUnavailable
        })?;
        let mut transaction = client.transaction().map_err(|error| {
            self.record_error(&error);
            PostgresExternalInputPreparationErrorV1::StorageUnavailable
        })?;
        transaction
            .execute(
                "INSERT INTO worldstream_external_input_preparations(identity_bytes, canonical_request_hash, recorded_at) VALUES ($1, $2, $3) ON CONFLICT (identity_bytes) DO NOTHING",
                &[&identity_bytes, &request_hash_bytes, &sampled_recorded_at.as_str()],
            )
            .map_err(|error| {
                self.record_error(&error);
                PostgresExternalInputPreparationErrorV1::StorageUnavailable
            })?;
        let row = transaction
            .query_opt(
                "SELECT canonical_request_hash, recorded_at FROM worldstream_external_input_preparations WHERE identity_bytes = $1",
                &[&identity_bytes],
            )
            .map_err(|error| {
                self.record_error(&error);
                PostgresExternalInputPreparationErrorV1::StorageUnavailable
            })?
            .ok_or(PostgresExternalInputPreparationErrorV1::Corrupt)?;
        let stored_hash = row
            .try_get::<_, Vec<u8>>(0)
            .map_err(|_| PostgresExternalInputPreparationErrorV1::Corrupt)?;
        if stored_hash.len() != 32 {
            return Err(PostgresExternalInputPreparationErrorV1::Corrupt);
        }
        if stored_hash.as_slice() != request_hash_bytes {
            return Err(PostgresExternalInputPreparationErrorV1::Conflict);
        }
        let stored_recorded_at = row
            .try_get::<_, String>(1)
            .map_err(|_| PostgresExternalInputPreparationErrorV1::Corrupt)?
            .parse::<ExternalInputRecordedAt>()
            .map_err(|_| PostgresExternalInputPreparationErrorV1::Corrupt)?;
        transaction.commit().map_err(|error| {
            self.record_error(&error);
            PostgresExternalInputPreparationErrorV1::StorageUnavailable
        })?;
        Ok(stored_recorded_at)
    }

    /// Lists Rooms in stable Room-ID order after revalidating a
    /// deployment-scoped safe-summary diagnostic grant.
    pub fn diagnostic_inventory_page(
        &self,
        authority: AuthorizedDiagnosticV1,
        checked_at: &AuthorityCheckedAt,
        after_room_id: Option<&RoomId>,
        limit: usize,
    ) -> Result<PostgresRoomDiagnosticInventoryPageV1, PostgresRoomDiagnosticErrorV1> {
        if limit == 0
            || limit > 100
            || authority.operation() != DiagnosticOperationV1::SafeRoomSummary
        {
            return Err(PostgresRoomDiagnosticErrorV1::InvalidBounds);
        }
        let authority = authority.into_adapter_input();
        if !matches!(authority.target(), DiagnosticTargetV1::Deployment) {
            return Err(PostgresRoomDiagnosticErrorV1::InvalidTarget);
        }
        let snapshot = self
            .snapshot(&authority.authority_snapshot_query())
            .map_err(map_postgres_diagnostic_authority_store_error)?
            .ok_or(PostgresRoomDiagnosticErrorV1::Authority(
                AuthorityErrorV1::Unauthenticated,
            ))?;
        authority.revalidate_current(&snapshot, checked_at)?;

        let mut client = self
            .connect()
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?;
        let row_limit =
            i64::try_from(limit + 1).map_err(|_| PostgresRoomDiagnosticErrorV1::InvalidBounds)?;
        let after = after_room_id.map_or("", RoomId::as_str);
        let rows = client
            .query(
                "SELECT roots.room_id, roots.head_bytes, genesis.pack_revision_lock_bytes, \
                 roots.integrity_status, roots.integrity_generation \
                 FROM worldstream_room_roots AS roots \
                 JOIN worldstream_genesis AS genesis ON genesis.room_id = roots.room_id \
                 WHERE roots.room_id > $1 ORDER BY roots.room_id LIMIT $2",
                &[&after, &row_limit],
            )
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?;
        let mut summaries = Vec::with_capacity(limit + 1);
        for row in rows {
            summaries.push(decode_postgres_diagnostic_summary(&row)?);
        }
        let next_after_room_id = if summaries.len() > limit {
            summaries.truncate(limit);
            summaries.last().map(|summary| summary.room_id.clone())
        } else {
            None
        };
        Ok(PostgresRoomDiagnosticInventoryPageV1 {
            rooms: summaries,
            next_after_room_id,
        })
    }

    /// Reads one host-authorized Room detail without exposing Membership or
    /// participant-private materializations.
    pub fn diagnostic_summary(
        &self,
        authority: AuthorizedDiagnosticV1,
        checked_at: &AuthorityCheckedAt,
    ) -> Result<PostgresRoomDiagnosticSummaryV1, PostgresRoomDiagnosticErrorV1> {
        if authority.operation() != DiagnosticOperationV1::SafeRoomSummary {
            return Err(PostgresRoomDiagnosticErrorV1::InvalidTarget);
        }
        let authority = authority.into_adapter_input();
        let room_id = match authority.target() {
            DiagnosticTargetV1::Room(room_id) => room_id.clone(),
            DiagnosticTargetV1::Deployment => {
                return Err(PostgresRoomDiagnosticErrorV1::InvalidTarget);
            }
        };
        let snapshot = self
            .snapshot(&authority.authority_snapshot_query())
            .map_err(map_postgres_diagnostic_authority_store_error)?
            .ok_or(PostgresRoomDiagnosticErrorV1::Authority(
                AuthorityErrorV1::Unauthenticated,
            ))?;
        authority.revalidate_current(&snapshot, checked_at)?;

        let mut client = self
            .connect()
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?;
        let row = client
            .query_opt(
                "SELECT roots.room_id, roots.head_bytes, genesis.pack_revision_lock_bytes, \
                 roots.integrity_status, roots.integrity_generation \
                 FROM worldstream_room_roots AS roots \
                 JOIN worldstream_genesis AS genesis ON genesis.room_id = roots.room_id \
                 WHERE roots.room_id = $1",
                &[&room_id.as_str()],
            )
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?
            .ok_or(PostgresRoomDiagnosticErrorV1::RoomUnavailable)?;
        decode_postgres_diagnostic_summary(&row)
    }

    /// Counts only pending and leased durable Activation intents for one exact
    /// Room Membership after revalidating the consumed host diagnostic grant.
    /// Terminal states are deliberately excluded.
    ///
    /// # Errors
    ///
    /// Returns a typed authority, unavailable-target, storage, or overflow
    /// failure. SQL counts that cannot fit the bounded public `u32` contract
    /// fail closed as corrupt storage.
    pub fn diagnostic_activation_status(
        &self,
        authority: AuthorizedDiagnosticV1,
        checked_at: &AuthorityCheckedAt,
        member_id: &MemberId,
    ) -> Result<PostgresActivationStatusV1, PostgresRoomDiagnosticErrorV1> {
        if authority.operation() != DiagnosticOperationV1::SafeRoomSummary {
            return Err(PostgresRoomDiagnosticErrorV1::InvalidTarget);
        }
        let authority = authority.into_adapter_input();
        let room_id = match authority.target() {
            DiagnosticTargetV1::Room(room_id) => room_id.clone(),
            DiagnosticTargetV1::Deployment => {
                return Err(PostgresRoomDiagnosticErrorV1::InvalidTarget);
            }
        };
        let snapshot = self
            .snapshot(&authority.authority_snapshot_query())
            .map_err(map_postgres_diagnostic_authority_store_error)?
            .ok_or(PostgresRoomDiagnosticErrorV1::Authority(
                AuthorityErrorV1::Unauthenticated,
            ))?;
        authority.revalidate_current(&snapshot, checked_at)?;

        let mut client = self
            .connect()
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?;
        let row = client
            .query_opt(
                "SELECT \
                    (SELECT count(*) FROM worldstream_activation_intents \
                     WHERE room_id = $1 AND target_member_id = $2 AND state = 'pending'), \
                    (SELECT count(*) FROM worldstream_activation_intents \
                     WHERE room_id = $1 AND target_member_id = $2 AND state = 'leased') \
                 FROM worldstream_members WHERE room_id = $1 AND member_id = $2",
                &[&room_id.as_str(), &member_id.as_str()],
            )
            .map_err(|_| PostgresRoomDiagnosticErrorV1::StorageUnavailable)?
            .ok_or(PostgresRoomDiagnosticErrorV1::RoomUnavailable)?;
        let waiting = row
            .try_get::<_, i64>(0)
            .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
        let leased = row
            .try_get::<_, i64>(1)
            .map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?;
        Ok(PostgresActivationStatusV1 {
            waiting: u32::try_from(waiting).map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?,
            leased: u32::try_from(leased).map_err(|_| PostgresRoomDiagnosticErrorV1::Corrupt)?,
        })
    }

    /// Returns the engine identity after re-running the adapter's PostgreSQL
    /// capability admission check. The returned string is deterministic and
    /// contains no DSN or provider diagnostic text.
    pub fn engine_identity(&self) -> Result<PostgresEngineIdentityV1, PostgresEngineIdentityError> {
        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            PostgresEngineIdentityError::Unavailable
        })?;
        let server_version_num = read_verified_server_version_num(&mut client)
            .map_err(|error| map_engine_identity_error(self, error))?;
        Ok(PostgresEngineIdentityV1 {
            server_version_num,
            formatted: format_postgres_engine_identity(server_version_num),
        })
    }

    /// Performs read-only capability, migration-history, and catalog
    /// fingerprint verification. This is the runtime's explicit startup
    /// check; it cannot execute DDL because it only issues `SHOW` and
    /// `SELECT` statements.
    pub fn verify_schema(&self) -> Result<(), PostgresSchemaVerificationError> {
        let result = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresSchemaVerificationError::Connection)
            .and_then(|mut client| verify_runtime_store_client(&mut client));
        if let Err(error) = &result {
            let kind = match error {
                PostgresSchemaVerificationError::Connection(_) => {
                    PostgresStorageDiagnosticKindV1::Connection
                }
                PostgresSchemaVerificationError::Sql(_) => PostgresStorageDiagnosticKindV1::Query,
                PostgresSchemaVerificationError::UnsupportedMajor { .. }
                | PostgresSchemaVerificationError::UnsupportedPatch { .. }
                | PostgresSchemaVerificationError::InvalidServerVersion { .. }
                | PostgresSchemaVerificationError::SynchronousCommit { .. }
                | PostgresSchemaVerificationError::RuntimeRolePrivileges
                | PostgresSchemaVerificationError::DisposableRestoreTarget
                | PostgresSchemaVerificationError::History(_)
                | PostgresSchemaVerificationError::FingerprintDrift => {
                    PostgresStorageDiagnosticKindV1::Integrity
                }
            };
            emit_postgres_telemetry(
                self.telemetry.as_deref(),
                PostgresTelemetryEventV1::StorageDiagnostic { kind },
            );
        }
        result
    }

    /// Verifies one persisted Room through the read-only provider path.
    pub fn verify_room(
        &self,
        room_id: &str,
    ) -> Result<PostgresRoomVerification, PostgresRoomVerificationError> {
        PostgresAdmin {
            config: self.config.clone(),
            telemetry: self.telemetry.clone(),
        }
        .verify_room(room_id)
    }

    /// Recovers one exact executable Core trace through the production
    /// recovery SPI. PostgreSQL supplies only verified durable bytes; Core
    /// performs replay, reduction, and the guarded recovery fence.
    pub fn recover_room(
        &self,
        registry: &worldstream_core::PackRegistryV1,
        room_id: &str,
    ) -> Result<Option<CoreTraceV1>, RoomRecoveryErrorV1> {
        let room_id = room_id
            .parse::<RoomId>()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        worldstream_core::recover_room_from_storage(self, registry, &room_id)
    }

    /// Runs the present-authorized historical projection path against exact
    /// verified PostgreSQL lineage. The authority fence is checked before and
    /// after the immutable read; the returned projection is produced solely
    /// by Core's historical reducer.
    pub fn replay_authorized(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedReplayV1,
    ) -> Result<HistoricalReplayProjectionV1, PostgresReplayError> {
        let authority = authority.into_adapter_input();
        let query = authority.authority_snapshot_query();
        let snapshot = AuthorityStoreV1::snapshot(self, &query)
            .map_err(|_| PostgresReplayError::Authority)?
            .ok_or(PostgresReplayError::Authority)?;
        let checked_at = self
            .authority_checked_at()
            .map_err(|_| PostgresReplayError::Authority)?;
        authority
            .revalidate_current(&snapshot, &checked_at)
            .map_err(|_| PostgresReplayError::Authority)?;
        let verification = self
            .verify_room(authority.room_id().as_ref())
            .map_err(|_| PostgresReplayError::Verification)?;
        let status = match verification.integrity_status.as_str() {
            "healthy" => RoomIntegrityStatusV1::Healthy,
            "faulted" => RoomIntegrityStatusV1::Faulted,
            "quarantined" => RoomIntegrityStatusV1::Quarantined,
            _ => return Err(PostgresReplayError::Verification),
        };
        let integrity = RoomIntegrityStateV1::new(
            status,
            IntegrityGenerationV1::new(verification.integrity_generation)
                .map_err(|_| PostgresReplayError::Verification)?,
        );
        let replay = CoreTraceV1::project_replayed_history(
            registry,
            &verification.genesis_bytes,
            &verification.transition_bytes,
            authority.projection_request(integrity),
        )?;
        let final_snapshot = AuthorityStoreV1::snapshot(self, &query)
            .map_err(|_| PostgresReplayError::Authority)?
            .ok_or(PostgresReplayError::Authority)?;
        let final_checked_at = self
            .authority_checked_at()
            .map_err(|_| PostgresReplayError::Authority)?;
        authority
            .revalidate_current(&final_snapshot, &final_checked_at)
            .map_err(|_| PostgresReplayError::Authority)?;
        Ok(replay)
    }

    /// Prepares and commits one Core-authorized Participant Action through
    /// the production Room Commit seam.
    pub fn commit_authorized_participant_action(
        &self,
        registry: &worldstream_core::PackRegistryV1,
        authority: ParticipantActionAuthorityV1,
        request: &ParticipantActionRequestV1,
        stimulus: ParticipantActionV1,
        transition_id: worldstream_core::TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresRoomCommitError> {
        let room_id = stimulus.exact_basis_head.room_id().to_string();
        let verification = self.verify_room(&room_id).map_err(|_| {
            PostgresRoomCommitError::Recovery(RoomRecoveryErrorV1::StorageUnavailable)
        })?;
        if verification.integrity_status != "healthy" {
            return Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::IntegrityUnavailable,
            ));
        }
        let mut trace = self
            .recover_room(registry, &room_id)
            .map_err(PostgresRoomCommitError::Recovery)?
            .ok_or(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::StorageUnavailable,
            ))?;
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let integrity_generation = IntegrityGenerationV1::new(verification.integrity_generation)
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        let admitted_at = stimulus.admitted_at.clone();
        let prepared = if request.based_on_room_seq() == trace.head().room_seq() {
            let prepared_transition =
                match trace.prepare(RecordedStimulusV1::ParticipantAction(stimulus)) {
                    Ok(prepared) => Some(prepared),
                    Err(
                        TraceErrorV1::ActionAdmission(_)
                        | TraceErrorV1::ArchivedStimulusForbidden
                        | TraceErrorV1::CompleteHeadMismatch,
                    ) => None,
                    Err(_) => return Err(PostgresRoomCommitError::Preparation),
                };
            match prepared_transition {
                Some(prepared) => match authority {
                    ParticipantActionAuthorityV1::EnabledParticipant(authority) => {
                        PreparedRoomCommitV1::for_authorized_action(
                            &trace,
                            request,
                            prepared,
                            transition_id,
                            integrity_generation,
                            authority,
                            &frame_heads,
                        )
                        .map_err(|_| PostgresRoomCommitError::Preparation)?
                    }
                    ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority) => {
                        PreparedRoomCommitV1::for_authorized_stable_action_disposition(
                            &trace,
                            request,
                            admitted_at.clone(),
                            integrity_generation,
                            ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority),
                        )
                        .map_err(|_| PostgresRoomCommitError::Preparation)?
                    }
                },
                None => PreparedRoomCommitV1::for_authorized_stable_action_disposition(
                    &trace,
                    request,
                    admitted_at,
                    integrity_generation,
                    authority,
                )
                .map_err(|_| PostgresRoomCommitError::Preparation)?,
            }
        } else {
            PreparedRoomCommitV1::for_authorized_stable_action_disposition(
                &trace,
                request,
                admitted_at,
                integrity_generation,
                authority,
            )
            .map_err(|_| PostgresRoomCommitError::Preparation)?
        };
        Ok(commit_existing_room(self, &mut trace, prepared)
            .into_parts()
            .0)
    }

    /// Prepares and commits one exact, server-loaded Timer generation through
    /// Core's production Timer sealer and the existing-Room coordinator.
    pub fn commit_authorized_timer_fired(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresRoomCommitError> {
        let room_id = request.room_id().to_string();
        let verification = self.verify_room(&room_id).map_err(|_| {
            PostgresRoomCommitError::Recovery(RoomRecoveryErrorV1::StorageUnavailable)
        })?;
        if verification.integrity_status != "healthy" {
            return Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::IntegrityUnavailable,
            ));
        }
        let mut trace = self
            .recover_room(registry, &room_id)
            .map_err(PostgresRoomCommitError::Recovery)?
            .ok_or(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::StorageUnavailable,
            ))?;
        if trace.head() != &verification.head {
            return Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::ConcurrentChange,
            ));
        }
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let integrity_generation = IntegrityGenerationV1::new(verification.integrity_generation)
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        let stimulus = RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: request.timer_id().clone(),
            generation: request.generation(),
            scheduled_for: request.scheduled_for().clone(),
            canonical_payload: request.canonical_payload().clone(),
        });
        let prepared_transition = trace
            .prepare(stimulus)
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        let prepared = PreparedRoomCommitV1::for_authorized_timer_fired(
            &trace,
            request,
            prepared_transition,
            transition_id,
            integrity_generation,
            authority,
            &frame_heads,
        )
        .map_err(|_| PostgresRoomCommitError::Preparation)?;
        Ok(commit_existing_room(self, &mut trace, prepared)
            .into_parts()
            .0)
    }

    /// Commits one exact host-authorized ExternalInput through Core's existing
    /// recorded-stimulus and semantic-receipt coordinator.
    pub fn commit_authorized_external_input(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedExternalInputV1,
        room_id: &RoomId,
        based_on_room_seq: RoomSequenceV1,
        input: &ExternalInputV1,
        transition_id: TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresRoomCommitError> {
        let room_id_text = room_id.to_string();
        let verification = self.verify_room(&room_id_text).map_err(|_| {
            PostgresRoomCommitError::Recovery(RoomRecoveryErrorV1::StorageUnavailable)
        })?;
        if verification.integrity_status != "healthy" {
            return Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::IntegrityUnavailable,
            ));
        }
        let mut trace = self
            .recover_room(registry, &room_id_text)
            .map_err(PostgresRoomCommitError::Recovery)?
            .ok_or(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::StorageUnavailable,
            ))?;
        if trace.head() != &verification.head {
            return Err(PostgresRoomCommitError::Recovery(
                RoomRecoveryErrorV1::ConcurrentChange,
            ));
        }
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let integrity_generation = IntegrityGenerationV1::new(verification.integrity_generation)
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        let prepared_transition = trace
            .prepare(RecordedStimulusV1::ExternalInput(input.clone()))
            .map_err(|_| PostgresRoomCommitError::Preparation)?;
        let prepared = PreparedRoomCommitV1::for_authorized_external_input(
            &trace,
            room_id,
            based_on_room_seq,
            input,
            prepared_transition,
            transition_id,
            integrity_generation,
            authority,
            &frame_heads,
        )
        .map_err(|_| PostgresRoomCommitError::Preparation)?;
        Ok(commit_existing_room(self, &mut trace, prepared)
            .into_parts()
            .0)
    }

    /// Hydrates the exact executable Core trace from retained PostgreSQL
    /// Genesis/Transition bytes. This is intentionally feature-gated: the
    /// runtime adapter never invents a selectable revision or exposes raw
    /// history to an untrusted caller.
    #[cfg(feature = "conformance-tracer")]
    pub fn recover_conformance_trace(
        &self,
        registry: &PackRegistryV1,
        room_id: &str,
    ) -> Result<CoreTraceV1, PostgresConformanceError> {
        emit_postgres_telemetry(
            self.telemetry.as_deref(),
            PostgresTelemetryEventV1::Recovery {
                phase: PostgresRecoveryPhaseV1::Started,
            },
        );
        let result = self
            .verify_room(room_id)
            .map_err(|error| PostgresConformanceError::Verification(error.to_string()))
            .and_then(|verification| {
                CoreTraceV1::replay(
                    registry,
                    &verification.genesis_bytes,
                    &verification.transition_bytes,
                )
                .map_err(|error| PostgresConformanceError::Replay(format!("{error:?}")))
                .map(worldstream_core::ReplayReportV1::into_trace)
            });
        emit_postgres_telemetry(
            self.telemetry.as_deref(),
            PostgresTelemetryEventV1::Recovery {
                phase: if result.is_ok() {
                    PostgresRecoveryPhaseV1::Completed
                } else {
                    PostgresRecoveryPhaseV1::Failed
                },
            },
        );
        result
    }

    /// Seals and commits one Core-authorized Participant Action through the
    /// production Core preparation path. The caller supplies only the
    /// already-normalized Action stimulus; PostgreSQL owns no reducer logic.
    #[cfg(feature = "conformance-tracer")]
    pub fn commit_conformance_participant_action(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedParticipantActionV1,
        request: &ParticipantActionRequestV1,
        stimulus: ParticipantActionV1,
        transition_id: worldstream_core::TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresConformanceError> {
        let room_id = stimulus.exact_basis_head.room_id().to_string();
        let verification = self
            .verify_room(&room_id)
            .map_err(|error| PostgresConformanceError::Verification(error.to_string()))?;
        let trace = CoreTraceV1::replay(
            registry,
            &verification.genesis_bytes,
            &verification.transition_bytes,
        )
        .map_err(|error| PostgresConformanceError::Replay(format!("{error:?}")))?
        .into_trace();
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let prepared_transition = trace
            .prepare(worldstream_core::RecordedStimulusV1::ParticipantAction(
                stimulus,
            ))
            .map_err(|error| PostgresConformanceError::Preparation(format!("{error:?}")))?;
        let integrity_generation =
            worldstream_core::IntegrityGenerationV1::new(verification.integrity_generation)
                .map_err(|error| PostgresConformanceError::Preparation(error.to_string()))?;
        let prepared = PreparedRoomCommitV1::for_authorized_action(
            &trace,
            request,
            prepared_transition,
            transition_id,
            integrity_generation,
            authority,
            &frame_heads,
        )
        .map_err(|error| PostgresConformanceError::Preparation(format!("{error:?}")))?;
        let write: PreparedRoomWriteV1 = prepared.into();
        let resolution = self.commit_conformance_write(&write);
        Ok(resolution)
    }

    /// Seals and commits one Core-authorized Timer generation. Conditional
    /// generation and payload witnesses remain Core-owned; PostgreSQL only
    /// persists the resulting prepared write.
    #[cfg(feature = "conformance-tracer")]
    pub fn commit_conformance_timer_fired(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedTimerFiredV1,
        request: &TimerFiredRequestV1,
        stimulus: TimerFiredV1,
        transition_id: worldstream_core::TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresConformanceError> {
        let room_id = request.room_id().to_string();
        let verification = self
            .verify_room(&room_id)
            .map_err(|error| PostgresConformanceError::Verification(error.to_string()))?;
        let trace = CoreTraceV1::replay(
            registry,
            &verification.genesis_bytes,
            &verification.transition_bytes,
        )
        .map_err(|error| PostgresConformanceError::Replay(format!("{error:?}")))?
        .into_trace();
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let prepared_transition = trace
            .prepare(worldstream_core::RecordedStimulusV1::TimerFired(stimulus))
            .map_err(|error| PostgresConformanceError::Preparation(format!("{error:?}")))?;
        let integrity_generation =
            worldstream_core::IntegrityGenerationV1::new(verification.integrity_generation)
                .map_err(|error| PostgresConformanceError::Preparation(error.to_string()))?;
        let prepared = PreparedRoomCommitV1::for_authorized_timer_fired(
            &trace,
            request,
            prepared_transition,
            transition_id,
            integrity_generation,
            authority,
            &frame_heads,
        )
        .map_err(|error| PostgresConformanceError::Preparation(format!("{error:?}")))?;
        let write: PreparedRoomWriteV1 = prepared.into();
        Ok(self.commit_conformance_write(&write))
    }

    /// Seals and commits one Core-authorized existing-Room administration
    /// request. The request remains opaque to PostgreSQL and is normalized by
    /// the Core sealer before the storage transaction starts.
    #[cfg(feature = "conformance-tracer")]
    pub fn commit_conformance_core_administration(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedCoreAdministrationV1,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: worldstream_core::TransitionId,
    ) -> Result<RoomCommitResolutionV1, PostgresConformanceError> {
        let room_id = request.room_id().to_string();
        let verification = self
            .verify_room(&room_id)
            .map_err(|error| PostgresConformanceError::Verification(error.to_string()))?;
        let trace = CoreTraceV1::replay(
            registry,
            &verification.genesis_bytes,
            &verification.transition_bytes,
        )
        .map_err(|error| PostgresConformanceError::Replay(format!("{error:?}")))?
        .into_trace();
        let frame_heads = postgres_frame_heads(&trace, &verification);
        let integrity_generation =
            worldstream_core::IntegrityGenerationV1::new(verification.integrity_generation)
                .map_err(|error| PostgresConformanceError::Preparation(error.to_string()))?;
        let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
            &trace,
            request,
            recorded_at,
            transition_id,
            integrity_generation,
            authority,
            &frame_heads,
        )
        .map_err(|error| PostgresConformanceError::Preparation(format!("{error:?}")))?;
        let write: PreparedRoomWriteV1 = prepared.into();
        Ok(self.commit_conformance_write(&write))
    }

    #[cfg(feature = "conformance-tracer")]
    fn commit_conformance_write(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        self.conformance_capability_commit
            .store(true, Ordering::SeqCst);
        let result = self.commit(prepared);
        self.conformance_capability_commit
            .store(false, Ordering::SeqCst);
        result
    }

    /// Commits a prepared write through the typed conformance path.
    ///
    /// This is used by the provider-neutral black-box harness after it has
    /// obtained the same real Core authority grant as the SQLite adapter. It
    /// only enables the adapter's capability-witness check for this one
    /// commit; all idempotency, locking, materialization, and receipt work
    /// remains the production PostgreSQL path.
    #[cfg(feature = "conformance-tracer")]
    pub fn commit_conformance_write_for_conformance(
        &self,
        prepared: &PreparedRoomWriteV1,
    ) -> RoomCommitResolutionV1 {
        self.commit_conformance_write(prepared)
    }

    pub(crate) fn verify_room_in_transaction(
        transaction: &mut Transaction<'_>,
        room_id: &str,
    ) -> Result<PostgresRoomVerification, PostgresRoomVerificationError> {
        PostgresAdmin::verify_room_with_client(transaction, room_id)
    }

    /// Performs the same read-only deployment identity probe as the admin
    /// verifier. A missing result is intentional until a reviewed migration
    /// supplies durable deployment lineage and storage-epoch columns.
    pub fn deployment_metadata_status(
        &self,
    ) -> Result<PostgresDeploymentMetadataStatus, PostgresDeploymentMetadataError> {
        PostgresAdmin {
            config: self.config.clone(),
            telemetry: self.telemetry.clone(),
        }
        .deployment_metadata_status()
    }

    /// Enables a deterministic provider-outcome failpoint for focused tests.
    pub fn set_failpoint(&self, failpoint: Option<PostgresFailpoint>) {
        if let Ok(mut current) = self.failpoint.lock() {
            *current = failpoint;
        }
    }

    /// Reports the last classified backend failure, without exposing DSNs.
    #[must_use]
    pub fn last_failure(&self) -> Option<PostgresStorageFailure> {
        self.last_failure.lock().ok().and_then(|failure| *failure)
    }

    /// Reads one guarded semantic receipt without changing provider state.
    ///
    /// The operation uses a single transaction and locks the matching guard
    /// row with `FOR SHARE`, so the request hash and receipt bytes are read as
    /// one stable pair on direct connections and transaction poolers alike.
    /// A missing row is `KnownAbsent`; a different hash is `Conflict`; and a
    /// malformed stored receipt fails closed as `Corrupt`. Runtime callers
    /// must perform Core authority validation before calling this primitive.
    pub fn read_guarded_receipt(
        &self,
        identity: &worldstream_core::OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> Result<ResolveOutcomeV1, PostgresReceiptReadError> {
        let identity_bytes = identity
            .canonical_bytes()
            .map_err(|_| PostgresReceiptReadError::InvalidIdentity)?;
        let mut client = Client::connect(&self.config.dsn, self.config.tls.clone())
            .map_err(PostgresReceiptReadError::Connection)?;
        let mut transaction = client
            .transaction()
            .map_err(PostgresReceiptReadError::Sql)?;
        let row = transaction
            .query_opt(
                "SELECT request_hash, receipt_bytes FROM worldstream_operation_guards WHERE identity_bytes = $1 FOR SHARE",
                &[&identity_bytes],
            )
            .map_err(PostgresReceiptReadError::Sql)?;
        let outcome = match row {
            None => ResolveOutcomeV1::KnownAbsent,
            Some(row) => {
                let stored_hash: Vec<u8> = row.try_get(0).map_err(PostgresReceiptReadError::Sql)?;
                if stored_hash == request_hash.as_bytes() {
                    let receipt: Option<Vec<u8>> =
                        row.try_get(1).map_err(PostgresReceiptReadError::Sql)?;
                    match receipt {
                        None => ResolveOutcomeV1::KnownAbsent,
                        Some(bytes) => ResolveOutcomeV1::StoredResolution(Box::new(
                            StoredSemanticResultV1::from_canonical_receipt_bytes(&bytes)
                                .map_err(|_| PostgresReceiptReadError::Corrupt)?,
                        )),
                    }
                } else {
                    let existing_request_hash = CanonicalRequestHashV1::from_str(&format!(
                        "blake3:{}",
                        hex_bytes(&stored_hash)
                    ))
                    .map_err(|_| PostgresReceiptReadError::Corrupt)?;
                    ResolveOutcomeV1::Conflict {
                        existing_request_hash,
                    }
                }
            }
        };
        transaction
            .commit()
            .map_err(PostgresReceiptReadError::Sql)?;
        Ok(outcome)
    }

    /// Reads a Cursor-bounded observation suffix under the Room root and
    /// Membership row locks. A missing retained prefix or reset witness
    /// returns a Projection Reset carrying the caller's Core-validated bytes.
    pub fn read_observation(
        &self,
        room_id: &str,
        member_id: &str,
        after_frame_seq: Option<u64>,
        projection: &CanonicalJsonV1,
    ) -> Result<PostgresObservationDeliveryV1, PostgresObservationError> {
        let projection_bytes = projection
            .to_bytes()
            .map_err(|_| PostgresObservationError::InvalidProjection)?;
        let mut client = self
            .connect()
            .map_err(PostgresObservationError::Connection)?;
        let mut tx = client
            .transaction()
            .map_err(PostgresObservationError::Sql)?;
        let row = tx
            .query_opt(
                "SELECT frame_head, retained_frame_floor, last_ack_frame_seq, reset_required_through FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR SHARE",
                &[&room_id, &member_id],
            )
            .map_err(PostgresObservationError::Sql)?
            .ok_or(PostgresObservationError::Corrupt)?;
        let frame_head = nonnegative_u64(row.get::<_, i64>(0))?;
        let retained_floor = nonnegative_u64(row.get::<_, i64>(1))?;
        let cursor = row
            .get::<_, Option<i64>>(2)
            .map(nonnegative_u64)
            .transpose()?;
        let reset_through = row
            .get::<_, Option<i64>>(3)
            .map(nonnegative_u64)
            .transpose()?;
        if after_frame_seq.is_some_and(|cursor| cursor > frame_head) {
            return Err(PostgresObservationError::FutureCursor);
        }
        let reset = after_frame_seq.is_none_or(|value| value.saturating_add(1) < retained_floor)
            || reset_through.is_some();
        if reset {
            tx.commit().map_err(PostgresObservationError::Sql)?;
            return Ok(PostgresObservationDeliveryV1::Reset {
                cursor,
                frame_head,
                retained_floor,
                reset_through,
                projection_bytes,
            });
        }
        let after = after_frame_seq.unwrap_or_default();
        let rows = tx
            .query(
                "SELECT member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq > $3 ORDER BY frame_seq",
                &[&room_id, &member_id, &i64::try_from(after).map_err(|_| PostgresObservationError::Corrupt)?],
            )
            .map_err(PostgresObservationError::Sql)?;
        let frames = rows
            .into_iter()
            .map(|row| {
                Ok(PostgresFrameEvidenceV1 {
                    member_id: row.get(0),
                    frame_seq: nonnegative_u64(row.get(1))?,
                    cause_room_seq: nonnegative_u64(row.get(2))?,
                    payload_bytes: row.get(3),
                    payload_hash: row.get(4),
                })
            })
            .collect::<Result<Vec<_>, PostgresObservationError>>()?;
        tx.commit().map_err(PostgresObservationError::Sql)?;
        Ok(PostgresObservationDeliveryV1::Retained {
            cursor,
            cursor_exclusive: after,
            frame_head,
            retained_floor,
            frames,
        })
    }

    /// Compatibility-only alias retained for the provider harness. Production
    /// callers must use [`Self::read_observation`].
    #[cfg(feature = "conformance-tracer")]
    pub fn read_observation_conformance(
        &self,
        room_id: &str,
        member_id: &str,
        after_frame_seq: Option<u64>,
        projection: &CanonicalJsonV1,
    ) -> Result<PostgresObservationDeliveryV1, PostgresObservationError> {
        self.read_observation(room_id, member_id, after_frame_seq, projection)
    }

    /// Monotonically advances the durable Observation Cursor without moving
    /// the frame head or retained floor.
    pub fn acknowledge_observation(
        &self,
        room_id: &str,
        member_id: &str,
        through_frame_seq: u64,
    ) -> Result<Option<u64>, PostgresObservationError> {
        let mut client = self
            .connect()
            .map_err(PostgresObservationError::Connection)?;
        let mut tx = client
            .transaction()
            .map_err(PostgresObservationError::Sql)?;
        let head: i64 = tx
            .query_one(
                "SELECT frame_head FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR UPDATE",
                &[&room_id, &member_id],
            )
            .map_err(PostgresObservationError::Sql)?
            .get(0);
        let through =
            i64::try_from(through_frame_seq).map_err(|_| PostgresObservationError::Corrupt)?;
        if through > head {
            return Err(PostgresObservationError::FutureCursor);
        }
        tx.execute(
            "UPDATE worldstream_members \
             SET last_ack_frame_seq = CASE WHEN $3::bigint = 0 THEN last_ack_frame_seq \
                                           ELSE greatest(coalesce(last_ack_frame_seq, 0), $3::bigint) END \
             WHERE room_id = $1 AND member_id = $2",
            &[&room_id, &member_id, &through],
        )
        .map_err(PostgresObservationError::Sql)?;
        let result: Option<i64> = tx
            .query_one(
                "SELECT last_ack_frame_seq FROM worldstream_members WHERE room_id = $1 AND member_id = $2",
                &[&room_id, &member_id],
            )
            .map_err(PostgresObservationError::Sql)?
            .get(0);
        tx.commit().map_err(PostgresObservationError::Sql)?;
        result.map(nonnegative_u64).transpose()
    }

    /// Compatibility-only alias retained for the provider harness. Production
    /// callers must use [`Self::acknowledge_observation`].
    #[cfg(feature = "conformance-tracer")]
    pub fn acknowledge_observation_conformance(
        &self,
        room_id: &str,
        member_id: &str,
        through_frame_seq: u64,
    ) -> Result<Option<u64>, PostgresObservationError> {
        self.acknowledge_observation(room_id, member_id, through_frame_seq)
    }

    /// Deletes only the physical frame prefix. Cursor/head positions remain
    /// durable; deleting below an unacknowledged Cursor installs a reset
    /// witness for the next attach.
    pub fn prune_observation_conformance(
        &self,
        room_id: &str,
        member_id: &str,
        retain_from_frame_seq: u64,
    ) -> Result<PostgresObservationPositionsV1, PostgresObservationError> {
        let retain =
            i64::try_from(retain_from_frame_seq).map_err(|_| PostgresObservationError::Corrupt)?;
        let mut client = self
            .connect()
            .map_err(PostgresObservationError::Connection)?;
        let mut tx = client
            .transaction()
            .map_err(PostgresObservationError::Sql)?;
        let row = tx
            .query_one(
                "SELECT frame_head, retained_frame_floor, last_ack_frame_seq, reset_required_through FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR UPDATE",
                &[&room_id, &member_id],
            )
            .map_err(PostgresObservationError::Sql)?;
        let frame_head = nonnegative_u64(row.get(0))?;
        let ack = row
            .get::<_, Option<i64>>(2)
            .map(nonnegative_u64)
            .transpose()?;
        tx.execute(
            "DELETE FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq < $3",
            &[&room_id, &member_id, &retain],
        )
        .map_err(PostgresObservationError::Sql)?;
        let reset = if ack.is_none_or(|value| value < retain_from_frame_seq.saturating_sub(1)) {
            Some(frame_head)
        } else {
            row.get::<_, Option<i64>>(3)
                .map(nonnegative_u64)
                .transpose()?
        };
        tx.execute(
            "UPDATE worldstream_members SET retained_frame_floor = greatest(retained_frame_floor, $3), reset_required_through = $4 WHERE room_id = $1 AND member_id = $2",
            &[&room_id, &member_id, &retain, &reset.map(|value| i64::try_from(value).unwrap_or(i64::MAX))],
        )
        .map_err(PostgresObservationError::Sql)?;
        let result = PostgresObservationPositionsV1 {
            frame_head,
            retained_frame_floor: retain_from_frame_seq,
            last_ack_frame_seq: ack,
            reset_required_through: reset,
        };
        tx.commit().map_err(PostgresObservationError::Sql)?;
        Ok(result)
    }

    /// Loads one exact Timer generation from the durable ledger. Schedule and
    /// payload are never accepted from the operator request.
    pub fn timer_candidate(
        &self,
        room_id: &RoomId,
        timer_id: &TimerId,
        generation: TimerGenerationV1,
    ) -> Result<Option<PostgresTimerCandidateV1>, PostgresTimerError> {
        let mut client = self
            .connect()
            .map_err(|_| PostgresTimerError::Unavailable)?;
        let row = client
            .query_opt(
                "SELECT scheduled_for, payload_bytes, state FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2 AND generation = $3",
                &[
                    &room_id.as_str(),
                    &timer_id.as_str(),
                    &i64::try_from(generation.get()).map_err(|_| PostgresTimerError::Corrupt)?,
                ],
            )
            .map_err(|_| PostgresTimerError::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let scheduled_for = row
            .try_get::<_, String>(0)
            .map_err(|_| PostgresTimerError::Corrupt)?
            .parse::<TimerScheduledFor>()
            .map_err(|_| PostgresTimerError::Corrupt)?;
        let payload = CanonicalJsonV1::from_canonical_bytes(
            &row.try_get::<_, Vec<u8>>(1)
                .map_err(|_| PostgresTimerError::Corrupt)?,
        )
        .map_err(|_| PostgresTimerError::Corrupt)?;
        let state = match row
            .try_get::<_, String>(2)
            .map_err(|_| PostgresTimerError::Corrupt)?
            .as_str()
        {
            "scheduled" => PostgresTimerStateV1::Scheduled,
            "cancelled" => PostgresTimerStateV1::Cancelled,
            "fired" => PostgresTimerStateV1::Fired,
            _ => return Err(PostgresTimerError::Corrupt),
        };
        Ok(Some(PostgresTimerCandidateV1 {
            request: TimerFiredRequestV1::new(
                room_id.clone(),
                timer_id.clone(),
                generation,
                scheduled_for,
                payload,
            ),
            state,
        }))
    }

    /// Samples PostgreSQL's transaction-visible clock for one loaded Timer.
    pub fn timer_candidate_is_due(
        &self,
        candidate: &PostgresTimerCandidateV1,
    ) -> Result<bool, PostgresTimerError> {
        let mut client = self
            .connect()
            .map_err(|_| PostgresTimerError::Unavailable)?;
        let now = client
            .query_one(
                "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
                &[],
            )
            .and_then(|row| row.try_get::<_, String>(0))
            .map_err(|_| PostgresTimerError::Unavailable)?;
        let now = postgres_authority_checked_at_from_provider_text(&now)
            .map_err(|_| PostgresTimerError::Corrupt)?;
        let now = HostClockSampleV1::new(now.as_str()).map_err(|_| PostgresTimerError::Corrupt)?;
        Ok(now.is_due(candidate.request().scheduled_for()))
    }

    /// Resolves one Activation's private Room/member target after the caller
    /// has authenticated. The returned identifiers remain subject to a fresh
    /// Core Runner-control authorization before any operation is performed.
    pub fn activation_target(
        &self,
        activation_id: &str,
    ) -> Result<Option<(RoomId, worldstream_core::MemberId)>, PostgresActivationError> {
        if activation_id.is_empty() || activation_id.len() > 256 {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        client
            .query_opt(
                "SELECT room_id, target_member_id FROM worldstream_activation_intents WHERE activation_id = $1",
                &[&activation_id],
            )
            .map_err(PostgresActivationError::Sql)?
            .map(|row| {
                let room_id = row
                    .try_get::<_, String>(0)
                    .map_err(PostgresActivationError::Sql)?
                    .parse()
                    .map_err(|_| PostgresActivationError::Corrupt)?;
                let member_id = row
                    .try_get::<_, String>(1)
                    .map_err(PostgresActivationError::Sql)?
                    .parse()
                    .map_err(|_| PostgresActivationError::Corrupt)?;
                Ok((room_id, member_id))
            })
            .transpose()
    }

    /// Returns pending offers for a member and records an idempotent offer
    /// receipt. The Room root lock serializes the offer view with Room writes;
    /// the receipt is written in the same transaction.
    pub fn offer_activations(
        &self,
        room_id: &str,
        member_id: &str,
        request: &ActivationOperationRequestV1,
    ) -> Result<Vec<PostgresActivationOfferV1>, PostgresActivationError> {
        if request.operation_kind != "offer" || request.runner_id.is_empty() {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let request_hash = postgres_activation_request_hash(request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, room_id, request, &request_hash, true)?
        {
            return if result.code == ActivationResultCodeV1::Granted {
                let offers = postgres_activation_offers(&mut tx, room_id, member_id)?;
                tx.commit().map_err(PostgresActivationError::Sql)?;
                Ok(offers)
            } else {
                tx.commit().map_err(PostgresActivationError::Sql)?;
                Ok(Vec::new())
            };
        }
        tx.query_opt(
            "SELECT room_id FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
            &[&room_id],
        )
        .map_err(PostgresActivationError::Sql)?
        .ok_or(PostgresActivationError::Fenced)?;
        let offers = postgres_activation_offers(&mut tx, room_id, member_id)?;
        let result =
            postgres_activation_result(request, ActivationResultCodeV1::Granted, None, None, None)?;
        postgres_insert_activation_receipt(
            &mut tx,
            room_id,
            request,
            &request_hash,
            &result,
            None,
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(offers)
    }

    /// Returns pending offers only after revalidating the consumed Core
    /// Runner-control grant under authority, Room, and Membership fences.
    #[allow(clippy::needless_pass_by_value)]
    pub fn offer_activations_authorized(
        &self,
        authority: AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
    ) -> Result<Vec<PostgresActivationOfferV1>, PostgresActivationError> {
        if authority.operation() != RunnerControlOperationV1::ReceiveOffer
            || request.operation_kind != "offer"
            || request.runner_id != authority.runner_id().to_string()
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let authority = authority.into_adapter_input();
        let room_id = authority.target().room_id.to_string();
        let member_id = authority.target().member_id.to_string();
        let request_hash = postgres_activation_request_hash(&request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        postgres_lock_runner_authority(self, &mut tx, &authority)?;
        postgres_lock_activation_target(&mut tx, &room_id, &member_id)?;
        let _ = postgres_revalidate_runner_authority(self, &mut tx, &authority)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, &room_id, &request, &request_hash, true)?
        {
            let offers = if result.code == ActivationResultCodeV1::Granted {
                postgres_activation_offers(&mut tx, &room_id, &member_id)?
            } else {
                Vec::new()
            };
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(offers);
        }
        let offers = postgres_activation_offers(&mut tx, &room_id, &member_id)?;
        let result = postgres_activation_result(
            &request,
            ActivationResultCodeV1::Granted,
            None,
            None,
            None,
        )?;
        postgres_insert_activation_receipt(
            &mut tx,
            &room_id,
            &request,
            &request_hash,
            &result,
            None,
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(offers)
    }

    /// Reads back the durable receipt for an offer operation after the offer
    /// transaction has committed. The returned value is decoded and checked
    /// by the same receipt verifier used by the claim/complete lifecycle.
    pub fn read_activation_receipt_conformance(
        &self,
        room_id: &str,
        request: &ActivationOperationRequestV1,
    ) -> Result<Option<ActivationOperationResultV1>, PostgresActivationError> {
        let request_hash = postgres_activation_request_hash(request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        let result = postgres_activation_receipt(&mut tx, room_id, request, &request_hash, false)?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Reads one exact claim receipt before preparing a fresh private context.
    /// This lets retries survive a pending-to-leased/completed state change.
    pub fn activation_claim_receipt(
        &self,
        room_id: &RoomId,
        request: &ActivationOperationRequestV1,
    ) -> Result<Option<ActivationOperationResultV1>, PostgresActivationError> {
        let request_hash = postgres_activation_request_hash(request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        let result =
            postgres_activation_receipt(&mut tx, room_id.as_str(), request, &request_hash, false)?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Prepares the exact private Invocation Context for a production claim.
    /// A later guarded claim rechecks every captured Head, Membership,
    /// authority, delivery, policy, and lease witness before committing.
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    pub fn prepare_activation_claim(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
    ) -> Result<PostgresActivationClaimPreparationV1, PostgresActivationError> {
        if authority.operation() != RunnerControlOperationV1::Claim
            || request.operation_kind != "claim"
            || request.runner_id != authority.runner_id().to_string()
            || request.activation_id.is_none()
            || request.claim_id.as_deref() != Some(&request.operation_id)
            || request.requested_lease_ms.is_none()
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let activation_id = request
            .activation_id
            .as_deref()
            .ok_or(PostgresActivationError::InvalidRequest)?;
        let claim_id = request
            .claim_id
            .as_deref()
            .ok_or(PostgresActivationError::InvalidRequest)?;
        let requested_lease_ms = request
            .requested_lease_ms
            .filter(|value| *value > 0 && *value <= MAX_ACTIVATION_LEASE_MS)
            .ok_or(PostgresActivationError::InvalidRequest)?;
        let authority_generation = authority.authority_generation().get();
        let member_key = authority.target().member_id.clone();
        let room_id = authority.target().room_id.to_string();
        let member_id = member_key.to_string();
        let authority = authority.into_adapter_input();
        let verification = self
            .verify_room(&room_id)
            .map_err(|_| PostgresActivationError::Fenced)?;
        if verification.integrity_status != "healthy" {
            return Err(PostgresActivationError::Fenced);
        }
        let trace = self
            .recover_room(registry, &room_id)
            .map_err(|_| PostgresActivationError::Corrupt)?
            .ok_or(PostgresActivationError::Fenced)?;
        if trace.head() != &verification.head
            || trace.core_state().room_status() != RoomStatusV1::Active
        {
            return Err(PostgresActivationError::Fenced);
        }
        let membership = trace
            .core_state()
            .membership(&member_key)
            .filter(|membership| {
                membership.standing() == MembershipStandingV1::Enabled
                    && membership.access_mode() == AccessModeV1::Participant
                    && membership.principal_kind() == worldstream_core::PrincipalKindV1::Agent
                    && membership.role().is_some()
            })
            .ok_or(PostgresActivationError::Fenced)?;
        let view = registry
            .load_retained(trace.head().pack_digest())
            .map_err(|_| PostgresActivationError::Corrupt)?
            .host()
            .view(&worldstream_core::ViewInputV1 {
                core: trace.core_state(),
                activity_state: trace.activity_state(),
                complete_head: trace.head(),
                viewer: &PackViewerV1::Participant(member_key),
            })
            .map_err(|_| PostgresActivationError::Corrupt)?;
        if membership.member_id().as_str() != member_id {
            return Err(PostgresActivationError::Corrupt);
        }

        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .map_err(PostgresActivationError::Sql)?;
        postgres_lock_runner_authority(self, &mut tx, &authority)?;
        postgres_lock_activation_target(&mut tx, &room_id, &member_id)?;
        let current_authority_generation =
            postgres_revalidate_runner_authority(self, &mut tx, &authority)?;
        if current_authority_generation != authority_generation {
            return Err(PostgresActivationError::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration,
            ));
        }
        let request_hash = postgres_activation_request_hash(&request)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, &room_id, &request, &request_hash, true)?
        {
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(PostgresActivationClaimPreparationV1::Existing(Box::new(
                result,
            )));
        }
        let root = tx
            .query_opt(
                "SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR SHARE",
                &[&room_id],
            )
            .map_err(PostgresActivationError::Sql)?
            .ok_or(PostgresActivationError::Fenced)?;
        let head_bytes: Vec<u8> = root.try_get(0).map_err(PostgresActivationError::Sql)?;
        let integrity_generation = u64::try_from(
            root.try_get::<_, i64>(1)
                .map_err(PostgresActivationError::Sql)?,
        )
        .map_err(|_| PostgresActivationError::Corrupt)?;
        let integrity_status: String = root.try_get(2).map_err(PostgresActivationError::Sql)?;
        if integrity_status != "healthy"
            || integrity_generation != verification.integrity_generation
            || head_bytes
                != trace
                    .head()
                    .canonical_bytes()
                    .map_err(|_| PostgresActivationError::Corrupt)?
        {
            return Err(PostgresActivationError::Fenced);
        }
        let row = tx
            .query_opt(
                "SELECT i.target_member_id, i.state, i.reason_code, i.policy_revision, i.lease_generation, i.cause_room_seq, i.semantic_deadline, m.frame_head, m.retained_frame_floor, m.last_ack_frame_seq, m.reset_required_through, m.membership_bytes, m.membership_generation FROM worldstream_activation_intents i JOIN worldstream_members m ON m.room_id = i.room_id AND m.member_id = i.target_member_id WHERE i.room_id = $1 AND i.activation_id = $2",
                &[&room_id, &activation_id],
            )
            .map_err(PostgresActivationError::Sql)?
            .ok_or(PostgresActivationError::Fenced)?;
        let target_member_id: String = row.try_get(0).map_err(PostgresActivationError::Sql)?;
        let state: String = row.try_get(1).map_err(PostgresActivationError::Sql)?;
        let reason_code: String = row.try_get(2).map_err(PostgresActivationError::Sql)?;
        let policy_revision = postgres_nonnegative_u64(&row, 3)?;
        let lease_generation = postgres_nonnegative_u64(&row, 4)?;
        let cause_room_seq = postgres_nonnegative_u64(&row, 5)?;
        let semantic_deadline: Option<String> =
            row.try_get(6).map_err(PostgresActivationError::Sql)?;
        let frame_head = postgres_nonnegative_u64(&row, 7)?;
        let retained_floor = postgres_nonnegative_u64(&row, 8)?;
        let cursor = row
            .try_get::<_, Option<i64>>(9)
            .map_err(PostgresActivationError::Sql)?
            .map(|value| u64::try_from(value).map_err(|_| PostgresActivationError::Corrupt))
            .transpose()?;
        let reset_required_through = row
            .try_get::<_, Option<i64>>(10)
            .map_err(PostgresActivationError::Sql)?
            .map(|value| u64::try_from(value).map_err(|_| PostgresActivationError::Corrupt))
            .transpose()?;
        let membership_bytes: Vec<u8> = row.try_get(11).map_err(PostgresActivationError::Sql)?;
        let membership_generation = postgres_nonnegative_u64(&row, 12)?;
        let stored_membership =
            CanonicalJsonV1::decode_canonical::<MembershipV1>(&membership_bytes)
                .map_err(|_| PostgresActivationError::Corrupt)?;
        if target_member_id != member_id
            || state != "pending"
            || &stored_membership != membership
            || frame_head < retained_floor.saturating_sub(1)
            || cursor.is_some_and(|value| value > frame_head)
        {
            return Err(PostgresActivationError::Fenced);
        }
        let lease_until = postgres_lease_until(&mut tx, requested_lease_ms)?;
        let semantic_deadline = semantic_deadline
            .as_deref()
            .map(str::parse::<TimerScheduledFor>)
            .transpose()
            .map_err(|_| PostgresActivationError::Corrupt)?;
        let delivery = if cursor.is_none()
            || reset_required_through.is_some()
            || cursor.is_some_and(|value| value.saturating_add(1) < retained_floor)
        {
            ActivationDeliveryV1::ProjectionReset {
                baseline_frame_head: frame_head,
                reason: if cursor.is_none() {
                    "first_attach".to_owned()
                } else if reset_required_through.is_some() {
                    "reset_marked".to_owned()
                } else {
                    "retained_range_unavailable".to_owned()
                },
            }
        } else {
            let cursor = cursor.ok_or(PostgresActivationError::Corrupt)?;
            let rows = tx
                .query(
                    "SELECT frame_seq, cause_room_seq, payload_hash, payload_bytes FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq > $3 AND frame_seq <= $4 ORDER BY frame_seq",
                    &[
                        &room_id,
                        &member_id,
                        &i64::try_from(cursor).map_err(|_| PostgresActivationError::Corrupt)?,
                        &i64::try_from(frame_head).map_err(|_| PostgresActivationError::Corrupt)?,
                    ],
                )
                .map_err(PostgresActivationError::Sql)?;
            let mut previous = cursor;
            let mut frames = Vec::new();
            for row in rows {
                let frame_seq = postgres_nonnegative_u64(&row, 0)?;
                let cause_seq = postgres_nonnegative_u64(&row, 1)?;
                let stored_hash: Vec<u8> = row.try_get(2).map_err(PostgresActivationError::Sql)?;
                let payload_bytes: Vec<u8> =
                    row.try_get(3).map_err(PostgresActivationError::Sql)?;
                let payload_hash =
                    Blake3DigestV1::from_str(&format!("blake3:{}", hex_bytes(&stored_hash)))
                        .map_err(|_| PostgresActivationError::Corrupt)?;
                if frame_seq != previous.saturating_add(1)
                    || cause_seq > trace.head().room_seq().get()
                    || payload_hash != Blake3DigestV1::hash(&payload_bytes)
                    || CanonicalJsonV1::from_canonical_bytes(&payload_bytes).is_err()
                {
                    return Err(PostgresActivationError::Corrupt);
                }
                previous = frame_seq;
                frames.push(ActivationFrameV1 {
                    frame_seq,
                    cause_room_seq: RoomSequenceV1::new(cause_seq)
                        .map_err(|_| PostgresActivationError::Corrupt)?,
                    payload_hash,
                    payload_bytes,
                });
            }
            if previous != frame_head {
                return Err(PostgresActivationError::Corrupt);
            }
            ActivationDeliveryV1::RetainedFrames {
                cursor_exclusive: cursor,
                through_frame_head: frame_head,
                frames,
            }
        };
        let runner_budget_bytes = postgres_canonical_bytes(&serde_json::json!({
            "schema": "worldstream/runner-budget/v1",
            "max_action_submissions": 1
        }))?;
        let runner_limits_bytes = postgres_canonical_bytes(&serde_json::json!({
            "schema": "worldstream/runner-limits/v1",
            "max_runtime_ms": requested_lease_ms,
            "max_result_bytes": 65_536
        }))?;
        let context = prepare_activation_context(ActivationContextInputV1 {
            activation_id: activation_id.to_owned(),
            claim_id: claim_id.to_owned(),
            cause_room_seq: RoomSequenceV1::new(cause_room_seq)
                .map_err(|_| PostgresActivationError::Corrupt)?,
            reason_code,
            lease_generation: lease_generation
                .checked_add(1)
                .ok_or(PostgresActivationError::Corrupt)?,
            lease_until,
            semantic_deadline,
            room_head: trace.head().clone(),
            integrity_generation,
            policy_revision,
            authority_generation,
            membership_generation,
            frame_head,
            retained_floor,
            cursor,
            projection_schema: view.projection_schema().to_owned(),
            projection_bytes: view.canonical_bytes().to_vec(),
            action_offers_bytes: view.action_offers().canonical_bytes().to_vec(),
            runner_budget_bytes,
            runner_limits_bytes,
            artifact_references: Vec::new(),
            delivery,
        })
        .map_err(|_| PostgresActivationError::InvalidRequest)?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(PostgresActivationClaimPreparationV1::Prepared(Box::new(
            PostgresActivationClaimV1 {
                authority,
                request,
                context,
            },
        )))
    }

    /// Claims one pending offer under a transaction-scoped row lock. The
    /// unique live-lease index remains the final contention fence.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    pub fn claim_activation(
        &self,
        room_id: &str,
        member_id: &str,
        request: &ActivationOperationRequestV1,
        context: &ActivationInvocationContextV1,
    ) -> Result<ActivationOperationResultV1, PostgresActivationError> {
        if request.operation_kind != "claim"
            || request.activation_id.is_none()
            || request.claim_id.as_deref() != Some(&request.operation_id)
            || context.claim_id != request.operation_id
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let request_hash = postgres_activation_request_hash(request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, room_id, request, &request_hash, true)?
        {
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let activation_id = request.activation_id.as_deref().unwrap_or_default();
        let row = tx
            .query_opt(
                "SELECT target_member_id, state, intent_generation, lease_generation, semantic_deadline, (semantic_deadline IS NOT NULL AND semantic_deadline::timestamptz <= clock_timestamp()) FROM worldstream_activation_intents WHERE room_id = $1 AND activation_id = $2 FOR UPDATE",
                &[&room_id, &activation_id],
            )
            .map_err(PostgresActivationError::Sql)?
            .ok_or(PostgresActivationError::Fenced)?;
        let target: String = row.try_get(0).map_err(PostgresActivationError::Sql)?;
        let state: String = row.try_get(1).map_err(PostgresActivationError::Sql)?;
        let intent_generation: i64 = row.try_get(2).map_err(PostgresActivationError::Sql)?;
        let lease_generation: i64 = row.try_get(3).map_err(PostgresActivationError::Sql)?;
        let deadline_expired: bool = row.try_get(5).map_err(PostgresActivationError::Sql)?;
        if context.activation_id != activation_id
            || context.lease_generation
                != u64::try_from(lease_generation + 1)
                    .map_err(|_| PostgresActivationError::Corrupt)?
        {
            return Err(PostgresActivationError::StaleLease);
        }
        if target != member_id || state != "pending" {
            let result = postgres_activation_result(
                request,
                match state.as_str() {
                    "expired" => ActivationResultCodeV1::Expired,
                    "cancelled" => ActivationResultCodeV1::Cancelled,
                    _ => ActivationResultCodeV1::NotAvailable,
                },
                postgres_activation_state(&state)?,
                Some(
                    u64::try_from(lease_generation)
                        .map_err(|_| PostgresActivationError::Corrupt)?,
                ),
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                room_id,
                request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let context_bytes = serde_json::to_vec(context)
            .ok()
            .and_then(|bytes| CanonicalJsonV1::parse(&bytes).ok())
            .and_then(|json| json.to_bytes().ok())
            .ok_or(PostgresActivationError::InvalidRequest)?;
        let context_hash = Blake3DigestV1::hash(&context_bytes);
        if deadline_expired {
            tx.execute(
                "UPDATE worldstream_activation_intents SET state = 'expired', intent_generation = intent_generation + 1 WHERE room_id = $1 AND activation_id = $2 AND state = 'pending'",
                &[&room_id, &activation_id],
            ).map_err(PostgresActivationError::Sql)?;
            let result = postgres_activation_result(
                request,
                ActivationResultCodeV1::Expired,
                Some(ActivationIntentStateV1::Expired),
                None,
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                room_id,
                request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let lease_until = postgres_lease_until(
            &mut tx,
            request
                .requested_lease_ms
                .unwrap_or(MAX_ACTIVATION_LEASE_MS),
        )?;
        let changed = tx.execute(
            "UPDATE worldstream_activation_intents SET state = 'leased', runner_id = $1, claim_id = $2, lease_generation = lease_generation + 1, lease_until = $3, context_hash = $4, context_bytes = $5 WHERE room_id = $6 AND activation_id = $7 AND state = 'pending' AND intent_generation = $8 AND lease_generation = $9",
            &[&request.runner_id, &request.claim_id, &lease_until, &context_hash.as_bytes().as_slice(), &context_bytes, &room_id, &activation_id, &intent_generation, &lease_generation],
        ).map_err(PostgresActivationError::Sql)?;
        if changed != 1 {
            return Err(PostgresActivationError::Fenced);
        }
        let result = postgres_activation_result(
            request,
            ActivationResultCodeV1::Granted,
            Some(ActivationIntentStateV1::Leased),
            Some(
                u64::try_from(lease_generation + 1)
                    .map_err(|_| PostgresActivationError::Corrupt)?,
            ),
            Some(context.clone()),
        )?;
        postgres_insert_activation_receipt(
            &mut tx,
            room_id,
            request,
            &request_hash,
            &result,
            Some(&context_bytes),
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Commits a prepared production claim under current authority, Room,
    /// Membership, delivery, policy, and lease fences.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    pub fn claim_activation_authorized(
        &self,
        claim: PostgresActivationClaimV1,
    ) -> Result<ActivationOperationResultV1, PostgresActivationError> {
        let authority = &claim.authority;
        if authority.operation() != RunnerControlOperationV1::Claim
            || claim.request.operation_kind != "claim"
            || claim.request.runner_id != authority.runner_id().to_string()
            || claim.request.activation_id.is_none()
            || claim.request.claim_id.as_deref() != Some(&claim.request.operation_id)
            || claim.context.claim_id != claim.request.operation_id
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let room_id = authority.target().room_id.to_string();
        let member_id = authority.target().member_id.to_string();
        let request_hash = postgres_activation_request_hash(&claim.request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        postgres_lock_runner_authority(self, &mut tx, authority)?;
        postgres_lock_activation_target(&mut tx, &room_id, &member_id)?;
        let authority_generation = postgres_revalidate_runner_authority(self, &mut tx, authority)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, &room_id, &claim.request, &request_hash, true)?
        {
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let activation_id = claim
            .request
            .activation_id
            .as_deref()
            .ok_or(PostgresActivationError::InvalidRequest)?;
        let row = tx
            .query_opt(
                "SELECT target_member_id, state, reason_code, policy_revision, intent_generation, lease_generation, cause_room_seq, semantic_deadline, (semantic_deadline IS NOT NULL AND semantic_deadline::timestamptz <= clock_timestamp()) FROM worldstream_activation_intents WHERE room_id = $1 AND activation_id = $2 FOR UPDATE",
                &[&room_id, &activation_id],
            )
            .map_err(PostgresActivationError::Sql)?
            .ok_or(PostgresActivationError::Fenced)?;
        let target: String = row.try_get(0).map_err(PostgresActivationError::Sql)?;
        let state: String = row.try_get(1).map_err(PostgresActivationError::Sql)?;
        let reason: String = row.try_get(2).map_err(PostgresActivationError::Sql)?;
        let policy_revision = postgres_nonnegative_u64(&row, 3)?;
        let intent_generation: i64 = row.try_get(4).map_err(PostgresActivationError::Sql)?;
        let lease_generation: i64 = row.try_get(5).map_err(PostgresActivationError::Sql)?;
        let cause_room_seq = postgres_nonnegative_u64(&row, 6)?;
        let semantic_deadline: Option<String> =
            row.try_get(7).map_err(PostgresActivationError::Sql)?;
        let deadline_expired: bool = row.try_get(8).map_err(PostgresActivationError::Sql)?;
        let current_lease_generation =
            u64::try_from(lease_generation).map_err(|_| PostgresActivationError::Corrupt)?;
        if target != member_id
            || claim.context.activation_id != activation_id
            || claim.context.reason_code != reason
            || claim.context.policy_revision != policy_revision
            || claim.context.cause_room_seq.get() != cause_room_seq
            || claim
                .context
                .semantic_deadline
                .as_ref()
                .map(TimerScheduledFor::as_str)
                != semantic_deadline.as_deref()
            || claim.context.lease_generation
                != current_lease_generation
                    .checked_add(1)
                    .ok_or(PostgresActivationError::Corrupt)?
            || claim.context.authority_generation != authority_generation
        {
            return Err(PostgresActivationError::Fenced);
        }
        if state != "pending" {
            let result = postgres_activation_result(
                &claim.request,
                match state.as_str() {
                    "expired" => ActivationResultCodeV1::Expired,
                    "cancelled" => ActivationResultCodeV1::Cancelled,
                    _ => ActivationResultCodeV1::NotAvailable,
                },
                postgres_activation_state(&state)?,
                Some(current_lease_generation),
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                &room_id,
                &claim.request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        postgres_validate_activation_context(&mut tx, &room_id, &member_id, &claim.context)?;
        let lease_valid: bool = tx
            .query_one(
                "SELECT $1::text::timestamptz > clock_timestamp() AND $1::text::timestamptz <= clock_timestamp() + ($2::bigint * interval '1 millisecond')",
                &[
                    &claim.context.lease_until,
                    &i64::try_from(
                        claim
                            .request
                            .requested_lease_ms
                            .ok_or(PostgresActivationError::InvalidRequest)?,
                    )
                    .map_err(|_| PostgresActivationError::InvalidRequest)?,
                ],
            )
            .and_then(|row| row.try_get(0))
            .map_err(PostgresActivationError::Sql)?;
        if !lease_valid {
            return Err(PostgresActivationError::Fenced);
        }
        if deadline_expired {
            tx.execute(
                "UPDATE worldstream_activation_intents SET state = 'expired', intent_generation = intent_generation + 1 WHERE room_id = $1 AND activation_id = $2 AND state = 'pending'",
                &[&room_id, &activation_id],
            )
            .map_err(PostgresActivationError::Sql)?;
            let result = postgres_activation_result(
                &claim.request,
                ActivationResultCodeV1::Expired,
                Some(ActivationIntentStateV1::Expired),
                None,
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                &room_id,
                &claim.request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let context_bytes = claim
            .context
            .canonical_bytes()
            .map_err(|_| PostgresActivationError::InvalidRequest)?;
        let context_hash = Blake3DigestV1::hash(&context_bytes);
        let changed = tx.execute(
            "UPDATE worldstream_activation_intents SET state = 'leased', runner_id = $1, claim_id = $2, lease_generation = lease_generation + 1, lease_until = $3, context_hash = $4, context_bytes = $5 WHERE room_id = $6 AND activation_id = $7 AND state = 'pending' AND intent_generation = $8 AND lease_generation = $9",
            &[&claim.request.runner_id, &claim.request.claim_id, &claim.context.lease_until, &context_hash.as_bytes().as_slice(), &context_bytes, &room_id, &activation_id, &intent_generation, &lease_generation],
        ).map_err(PostgresActivationError::Sql)?;
        if changed != 1 {
            return Err(PostgresActivationError::Fenced);
        }
        let result = postgres_activation_result(
            &claim.request,
            ActivationResultCodeV1::Granted,
            Some(ActivationIntentStateV1::Leased),
            Some(claim.context.lease_generation),
            Some(claim.context.clone()),
        )?;
        postgres_insert_activation_receipt(
            &mut tx,
            &room_id,
            &claim.request,
            &request_hash,
            &result,
            Some(&context_bytes),
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Renews, releases, or completes a live lease. Every operation locks the
    /// intent row and checks runner, claim, generation, and expiry together.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    pub fn operate_activation_lease(
        &self,
        room_id: &str,
        request: &ActivationOperationRequestV1,
    ) -> Result<ActivationOperationResultV1, PostgresActivationError> {
        if !matches!(
            request.operation_kind.as_str(),
            "renew" | "release" | "complete"
        ) || request.activation_id.is_none()
            || request.claim_id.is_none()
            || request.lease_generation.is_none()
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let request_hash = postgres_activation_request_hash(request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, room_id, request, &request_hash, true)?
        {
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let activation_id = request.activation_id.as_deref().unwrap_or_default();
        let row = tx.query_opt(
            "SELECT state, runner_id, lease_generation, claim_id, lease_until, (lease_until IS NULL OR lease_until::timestamptz <= clock_timestamp()) FROM worldstream_activation_intents WHERE room_id = $1 AND activation_id = $2 FOR UPDATE",
            &[&room_id, &activation_id],
        ).map_err(PostgresActivationError::Sql)?.ok_or(PostgresActivationError::Fenced)?;
        let state: String = row.try_get(0).map_err(PostgresActivationError::Sql)?;
        let runner: Option<String> = row.try_get(1).map_err(PostgresActivationError::Sql)?;
        let generation: i64 = row.try_get(2).map_err(PostgresActivationError::Sql)?;
        let claim: Option<String> = row.try_get(3).map_err(PostgresActivationError::Sql)?;
        let lease_expired: bool = row.try_get(5).map_err(PostgresActivationError::Sql)?;
        let expected = request.lease_generation.unwrap_or_default();
        if state != "leased"
            || runner.as_deref() != Some(&request.runner_id)
            || claim != request.claim_id
            || u64::try_from(generation).ok() != Some(expected)
            || lease_expired
        {
            let result = postgres_activation_result(
                request,
                ActivationResultCodeV1::StaleLease,
                postgres_activation_state(&state)?,
                Some(u64::try_from(generation).map_err(|_| PostgresActivationError::Corrupt)?),
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                room_id,
                request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let (code, next_state, next_until) = match request.operation_kind.as_str() {
            "renew" => {
                let requested = request
                    .requested_lease_ms
                    .filter(|value| *value > 0 && *value <= MAX_ACTIVATION_LEASE_MS)
                    .ok_or(PostgresActivationError::InvalidRequest)?;
                (
                    ActivationResultCodeV1::Renewed,
                    "leased",
                    Some(postgres_lease_until(&mut tx, requested)?),
                )
            }
            "release" => (ActivationResultCodeV1::Released, "pending", None),
            _ => (ActivationResultCodeV1::Completed, "completed", None),
        };
        let new_generation = if next_state == "pending" {
            expected
                .checked_add(1)
                .ok_or(PostgresActivationError::InvalidRequest)?
        } else {
            expected
        };
        let changed = tx.execute(
            "UPDATE worldstream_activation_intents SET state = $1, runner_id = CASE WHEN $1 = 'pending' THEN NULL ELSE runner_id END, claim_id = CASE WHEN $1 = 'pending' THEN NULL ELSE claim_id END, lease_until = $2, lease_generation = $3 WHERE room_id = $4 AND activation_id = $5 AND state = 'leased' AND runner_id = $6 AND claim_id = $7 AND lease_generation = $8",
            &[
                &next_state,
                &next_until,
                &i64::try_from(new_generation).map_err(|_| PostgresActivationError::InvalidRequest)?,
                &room_id,
                &activation_id,
                &request.runner_id,
                &request.claim_id,
                &i64::try_from(expected).map_err(|_| PostgresActivationError::InvalidRequest)?,
            ],
        ).map_err(PostgresActivationError::Sql)?;
        if changed != 1 {
            return Err(PostgresActivationError::Fenced);
        }
        let result = postgres_activation_result(
            request,
            code,
            postgres_activation_state(next_state)?,
            Some(new_generation),
            None,
        )?;
        postgres_insert_activation_receipt(
            &mut tx,
            room_id,
            request,
            &request_hash,
            &result,
            None,
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Renews, releases, or completes a lease only after revalidating the
    /// exact purpose-sealed Runner-control grant in the mutation transaction.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    pub fn operate_activation_lease_authorized(
        &self,
        authority: AuthorizedRunnerControlV1,
        request: ActivationOperationRequestV1,
    ) -> Result<ActivationOperationResultV1, PostgresActivationError> {
        let expected_operation = match request.operation_kind.as_str() {
            "renew" => RunnerControlOperationV1::Renew,
            "release" => RunnerControlOperationV1::Release,
            "complete" => RunnerControlOperationV1::Complete,
            _ => return Err(PostgresActivationError::InvalidRequest),
        };
        if authority.operation() != expected_operation
            || request.runner_id != authority.runner_id().to_string()
            || request.activation_id.is_none()
            || request.claim_id.is_none()
            || request.lease_generation.is_none()
            || (request.operation_kind == "complete"
                && !matches!(
                    request.disposition.as_deref(),
                    Some("handled" | "declined" | "failed")
                ))
        {
            return Err(PostgresActivationError::InvalidRequest);
        }
        let authority = authority.into_adapter_input();
        let room_id = authority.target().room_id.to_string();
        let member_id = authority.target().member_id.to_string();
        let request_hash = postgres_activation_request_hash(&request)?;
        let mut client = self
            .connect()
            .map_err(PostgresActivationError::Connection)?;
        let mut tx = client.transaction().map_err(PostgresActivationError::Sql)?;
        postgres_lock_runner_authority(self, &mut tx, &authority)?;
        postgres_lock_activation_target(&mut tx, &room_id, &member_id)?;
        let _ = postgres_revalidate_runner_authority(self, &mut tx, &authority)?;
        if let Some(result) =
            postgres_activation_receipt(&mut tx, &room_id, &request, &request_hash, true)?
        {
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let activation_id = request.activation_id.as_deref().unwrap_or_default();
        let row = tx.query_opt(
            "SELECT state, runner_id, lease_generation, claim_id, (lease_until IS NULL OR lease_until::timestamptz <= clock_timestamp()), target_member_id FROM worldstream_activation_intents WHERE room_id = $1 AND activation_id = $2 FOR UPDATE",
            &[&room_id, &activation_id],
        ).map_err(PostgresActivationError::Sql)?.ok_or(PostgresActivationError::Fenced)?;
        let state: String = row.try_get(0).map_err(PostgresActivationError::Sql)?;
        let runner: Option<String> = row.try_get(1).map_err(PostgresActivationError::Sql)?;
        let generation: i64 = row.try_get(2).map_err(PostgresActivationError::Sql)?;
        let stored_claim: Option<String> = row.try_get(3).map_err(PostgresActivationError::Sql)?;
        let lease_expired: bool = row.try_get(4).map_err(PostgresActivationError::Sql)?;
        let target_member: String = row.try_get(5).map_err(PostgresActivationError::Sql)?;
        if target_member != member_id {
            return Err(PostgresActivationError::Fenced);
        }
        let expected = request.lease_generation.unwrap_or_default();
        if state != "leased"
            || runner.as_deref() != Some(&request.runner_id)
            || stored_claim != request.claim_id
            || u64::try_from(generation).ok() != Some(expected)
            || lease_expired
        {
            let result = postgres_activation_result(
                &request,
                ActivationResultCodeV1::StaleLease,
                postgres_activation_state(&state)?,
                Some(u64::try_from(generation).map_err(|_| PostgresActivationError::Corrupt)?),
                None,
            )?;
            postgres_insert_activation_receipt(
                &mut tx,
                &room_id,
                &request,
                &request_hash,
                &result,
                None,
            )?;
            tx.commit().map_err(PostgresActivationError::Sql)?;
            return Ok(result);
        }
        let (code, next_state, next_until) = match request.operation_kind.as_str() {
            "renew" => {
                let requested = request
                    .requested_lease_ms
                    .filter(|value| *value > 0 && *value <= MAX_ACTIVATION_LEASE_MS)
                    .ok_or(PostgresActivationError::InvalidRequest)?;
                (
                    ActivationResultCodeV1::Renewed,
                    "leased",
                    Some(postgres_lease_until(&mut tx, requested)?),
                )
            }
            "release" => (ActivationResultCodeV1::Released, "pending", None),
            "complete" => (ActivationResultCodeV1::Completed, "completed", None),
            _ => return Err(PostgresActivationError::InvalidRequest),
        };
        let new_generation = if next_state == "pending" {
            expected
                .checked_add(1)
                .ok_or(PostgresActivationError::InvalidRequest)?
        } else {
            expected
        };
        let changed = tx.execute(
            "UPDATE worldstream_activation_intents SET state = $1, runner_id = CASE WHEN $1 = 'pending' THEN NULL ELSE runner_id END, claim_id = CASE WHEN $1 = 'pending' THEN NULL ELSE claim_id END, lease_until = $2, lease_generation = $3, intent_generation = CASE WHEN $1 = 'pending' THEN intent_generation + 1 ELSE intent_generation END WHERE room_id = $4 AND activation_id = $5 AND state = 'leased' AND runner_id = $6 AND claim_id = $7 AND lease_generation = $8",
            &[
                &next_state,
                &next_until,
                &i64::try_from(new_generation).map_err(|_| PostgresActivationError::InvalidRequest)?,
                &room_id,
                &activation_id,
                &request.runner_id,
                &request.claim_id,
                &i64::try_from(expected).map_err(|_| PostgresActivationError::InvalidRequest)?,
            ],
        ).map_err(PostgresActivationError::Sql)?;
        if changed != 1 {
            return Err(PostgresActivationError::Fenced);
        }
        let result = postgres_activation_result(
            &request,
            code,
            postgres_activation_state(next_state)?,
            Some(new_generation),
            None,
        )?;
        postgres_insert_activation_receipt(
            &mut tx,
            &room_id,
            &request,
            &request_hash,
            &result,
            None,
        )?;
        tx.commit().map_err(PostgresActivationError::Sql)?;
        Ok(result)
    }

    /// Returns a bounded batch of expired live Activation leases to pending.
    ///
    /// The provider's `clock_timestamp()` is read inside the same transaction
    /// that locks and updates the selected rows. Reclamation clears the
    /// runner, claim, and lease deadline, and advances both durable generation
    /// fences so an old runner cannot reuse the lease. At most 256 rows are
    /// reclaimed per call; the scheduler can call this method again on its
    /// next tick.
    pub fn reclaim_expired_activation_leases(
        &self,
    ) -> Result<u32, PostgresActivationLeaseReclaimError> {
        const MAX_RECLAIM: i64 = 256;

        let mut client = self.connect().map_err(|error| {
            self.record_error(&error);
            PostgresActivationLeaseReclaimError::Unavailable
        })?;
        let mut transaction = client.transaction().map_err(|error| {
            self.record_error(&error);
            PostgresActivationLeaseReclaimError::Unavailable
        })?;
        let provider_now: String = transaction
            .query_one(
                "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
                &[],
            )
            .and_then(|row| row.try_get(0))
            .map_err(|error| {
                self.record_error(&error);
                PostgresActivationLeaseReclaimError::Unavailable
            })?;
        let provider_now = postgres_authority_checked_at_from_provider_text(&provider_now)
            .map_err(|_| PostgresActivationLeaseReclaimError::Corrupt)?;
        let changed = transaction
            .execute(
                "WITH expired AS MATERIALIZED ( \
                     SELECT intents.room_id, intents.activation_id \
                     FROM worldstream_activation_intents AS intents \
                     JOIN worldstream_room_roots AS roots ON roots.room_id = intents.room_id \
                     WHERE intents.state = 'leased' \
                       AND roots.integrity_status = 'healthy' \
                       AND intents.lease_until IS NOT NULL \
                       AND intents.lease_until::timestamptz <= $1::text::timestamptz \
                     ORDER BY intents.lease_until, intents.room_id, intents.activation_id \
                     FOR UPDATE SKIP LOCKED \
                     LIMIT $2 \
                 ) \
                 UPDATE worldstream_activation_intents AS intents \
                 SET state = 'pending', \
                     runner_id = NULL, \
                     claim_id = NULL, \
                     lease_until = NULL, \
                     lease_generation = intents.lease_generation + 1, \
                     intent_generation = intents.intent_generation + 1 \
                 FROM expired \
                 WHERE intents.room_id = expired.room_id \
                   AND intents.activation_id = expired.activation_id",
                &[&provider_now.as_str(), &MAX_RECLAIM],
            )
            .map_err(|error| {
                self.record_error(&error);
                PostgresActivationLeaseReclaimError::Unavailable
            })?;
        transaction.commit().map_err(|error| {
            self.record_error(&error);
            PostgresActivationLeaseReclaimError::Unavailable
        })?;
        u32::try_from(changed).map_err(|_| PostgresActivationLeaseReclaimError::Corrupt)
    }

    /// Test-only authority seed used by the Core conformance fixture. Production
    /// capability witnesses are intentionally fail-closed until the host's
    /// authority store is wired to this adapter.
    #[cfg(feature = "conformance-tracer")]
    pub fn seed_conformance_authority(
        &self,
        witness: &PreparedAuthorityWitnessV1,
        active: bool,
    ) -> Result<(), PostgresStorageFailure> {
        let Some((id, principal, generation, hash)) = witness.conformance_key() else {
            return Err(PostgresStorageFailure::Driver);
        };
        let mut client = self.connect().map_err(|error| self.record_error(&error))?;
        client
            .execute(
                "INSERT INTO worldstream_authority_fences(witness_id, authenticated_principal, generation, scope_revocation_hash, active) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (witness_id) DO UPDATE SET authenticated_principal = EXCLUDED.authenticated_principal, generation = EXCLUDED.generation, scope_revocation_hash = EXCLUDED.scope_revocation_hash, active = EXCLUDED.active",
                &[&id, &principal.to_string(), &i64::try_from(generation).unwrap_or(-1), &hash.as_bytes().as_slice(), &active],
            )
            .map_err(|error| self.record_error(&error))?;
        Ok(())
    }

    fn connect(&self) -> Result<Client, postgres::Error> {
        Client::connect(&self.config.dsn, self.config.tls.clone())
    }

    fn record_error(&self, error: &postgres::Error) -> PostgresStorageFailure {
        let failure = PostgresStorageFailure::from_error(error);
        if let Ok(mut last) = self.last_failure.lock() {
            *last = Some(failure);
        }
        let kind = match failure {
            PostgresStorageFailure::Connection => PostgresStorageDiagnosticKindV1::Connection,
            PostgresStorageFailure::Deadlock
            | PostgresStorageFailure::LockTimeout
            | PostgresStorageFailure::Serialization => PostgresStorageDiagnosticKindV1::Lock,
            PostgresStorageFailure::Constraint
            | PostgresStorageFailure::Schema
            | PostgresStorageFailure::Driver => PostgresStorageDiagnosticKindV1::Query,
        };
        emit_postgres_telemetry(
            self.telemetry.as_deref(),
            PostgresTelemetryEventV1::StorageDiagnostic { kind },
        );
        failure
    }

    fn take_failpoint(&self, expected: PostgresFailpoint) -> bool {
        self.failpoint
            .lock()
            .map(|mut current| {
                if *current == Some(expected) {
                    current.take();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
}

fn postgres_authority_checked_at<C: GenericClient>(
    client: &mut C,
) -> Result<AuthorityCheckedAt, AuthorityStoreErrorV1> {
    let value: String = client
        .query_one(
            "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
            &[],
        )
        .and_then(|row| row.try_get(0))
        .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
    postgres_authority_checked_at_from_provider_text(&value)
}

fn postgres_authority_checked_at_from_provider_text(
    value: &str,
) -> Result<AuthorityCheckedAt, AuthorityStoreErrorV1> {
    let bytes = value.as_bytes();
    if bytes.len() != 27
        || bytes.get(19) != Some(&b'.')
        || bytes.last() != Some(&b'Z')
        || !bytes[20..26].iter().all(u8::is_ascii_digit)
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let fraction = value[20..26].trim_end_matches('0');
    let canonical = if fraction.is_empty() {
        format!("{}Z", &value[..19])
    } else {
        format!("{}.{fraction}Z", &value[..19])
    };
    canonical
        .parse()
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn postgres_frame_heads(
    trace: &CoreTraceV1,
    verification: &PostgresRoomVerification,
) -> BTreeMap<worldstream_core::MemberId, u64> {
    let mut heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member_id| (member_id, 0_u64))
        .collect::<BTreeMap<_, _>>();
    for frame in &verification.frames {
        if let Ok(member_id) = frame.member_id.parse::<worldstream_core::MemberId>() {
            heads
                .entry(member_id)
                .and_modify(|head| *head = (*head).max(frame.frame_seq))
                .or_insert(frame.frame_seq);
        }
    }
    heads
}

fn resolve_receipt_in_client<C: GenericClient>(
    client: &mut C,
    identity: &OperationIdentityV1,
    request_hash: &CanonicalRequestHashV1,
) -> ResolveOutcomeV1 {
    let Ok(identity_bytes) = identity.canonical_bytes() else {
        return ResolveOutcomeV1::ResolutionUnavailable;
    };
    let row = match client.query_opt(
        "SELECT request_hash, receipt_bytes FROM worldstream_operation_guards WHERE identity_bytes = $1 FOR SHARE",
        &[&identity_bytes],
    ) {
        Ok(row) => row,
        Err(_) => return ResolveOutcomeV1::ResolutionUnavailable,
    };
    let Some(row) = row else {
        return ResolveOutcomeV1::KnownAbsent;
    };
    let Ok(stored_hash) = row.try_get::<_, Vec<u8>>(0) else {
        return ResolveOutcomeV1::ResolutionUnavailable;
    };
    if stored_hash != request_hash.as_bytes() {
        return CanonicalRequestHashV1::from_str(&format!("blake3:{}", hex_bytes(&stored_hash)))
            .map(|existing_request_hash| ResolveOutcomeV1::Conflict {
                existing_request_hash,
            })
            .unwrap_or(ResolveOutcomeV1::ResolutionUnavailable);
    }
    let Ok(receipt) = row.try_get::<_, Option<Vec<u8>>>(1) else {
        return ResolveOutcomeV1::ResolutionUnavailable;
    };
    match receipt {
        Some(bytes) => StoredSemanticResultV1::from_canonical_receipt_bytes(&bytes)
            .map(|value| ResolveOutcomeV1::StoredResolution(Box::new(value)))
            .unwrap_or(ResolveOutcomeV1::ResolutionUnavailable),
        None => ResolveOutcomeV1::KnownAbsent,
    }
}

impl AuthorizedReceiptResolverV1 for PostgresRoomStore {
    fn resolve_authorized(
        &self,
        authority: AuthorizedReceiptReadV1,
    ) -> Result<ResolveOutcomeV1, AuthorityErrorV1> {
        let authority = authority.into_adapter_input();
        let query = authority.authority_snapshot_query();
        let mut client = self.connect().map_err(|_| AuthorityErrorV1::Unavailable)?;
        let mut transaction = client
            .transaction()
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        transaction
            .batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        let snapshot = authority::load_authority_snapshot_for_adapter(&mut transaction, &query)
            .map_err(|_| AuthorityErrorV1::Unavailable)?
            .ok_or(AuthorityErrorV1::Unauthenticated)?;
        let checked_at = postgres_authority_checked_at(&mut transaction)
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        authority.revalidate_current(&snapshot, &checked_at)?;
        let outcome = worldstream_core::resolve_authorized_room_operation_for_adapter(
            authority,
            |identity, request_hash| {
                resolve_receipt_in_client(&mut transaction, identity, request_hash)
            },
        );
        transaction
            .commit()
            .map_err(|_| AuthorityErrorV1::Unavailable)?;
        Ok(outcome)
    }
}

impl RoomRecoveryStorageV1 for PostgresRoomStore {
    fn inspect_recovery_candidate(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        let verification = match self.verify_room(room_id.as_str()) {
            Ok(value) => value,
            Err(PostgresRoomVerificationError::MissingRoom { .. }) => return Ok(None),
            Err(_) => return Err(RoomRecoveryErrorV1::StorageUnavailable),
        };
        if verification.integrity_status != "healthy" {
            return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
        }
        Ok(Some(RoomRecoveryCandidateV1::new(
            verification.head,
            IntegrityGenerationV1::new(verification.integrity_generation)
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
            verification.head_bytes,
            verification.pack_revision_lock_bytes,
            verification.genesis_bytes,
            verification.transition_bytes,
            Some(verification.core_state_bytes),
            Some(verification.activity_state_bytes),
        )))
    }

    fn guard_recovery_install(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        recovered_materializations: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        let mut client = self
            .connect()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let mut tx = client
            .transaction()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let row = tx
            .query_opt(
                "SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
                &[&room_id.as_str()],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
            .ok_or(RoomRecoveryErrorV1::ConcurrentChange)?;
        let head_bytes: Vec<u8> = row.try_get(0).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let generation: i64 = row.try_get(1).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        let status: String = row.try_get(2).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if status != "healthy"
            || generation != i64::try_from(expected_integrity_generation.get()).unwrap_or(-1)
            || head_bytes
                != expected_head
                    .canonical_bytes()
                    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
        {
            return Err(RoomRecoveryErrorV1::ConcurrentChange);
        }
        tx.execute(
            "UPDATE worldstream_materializations SET core_state_bytes = $1, activity_state_bytes = $2 WHERE room_id = $3",
            &[
                &recovered_materializations.canonical_core_state_bytes(),
                &recovered_materializations.canonical_activity_state_bytes(),
                &room_id.as_str(),
            ],
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        for membership in recovered_materializations.memberships() {
            tx.execute(
                "UPDATE worldstream_members SET membership_bytes = $1 WHERE room_id = $2 AND member_id = $3",
                &[
                    &membership.canonical_membership_bytes,
                    &room_id.as_str(),
                    &membership.membership.member_id().as_str(),
                ],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        }
        tx.commit()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
    }

    fn record_recovery_failure(
        &self,
        room_id: &RoomId,
        _expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        let status = match disposition {
            RecoveryIntegrityDispositionV1::Faulted => "faulted",
            RecoveryIntegrityDispositionV1::Quarantined => "quarantined",
        };
        PostgresAdmin {
            config: self.config.clone(),
            telemetry: self.telemetry.clone(),
        }
        .record_integrity_incident(
            room_id.as_str(),
            expected_integrity_generation.get(),
            status,
            "room_recovery_failed",
            None,
        )
        .map(|_| ())
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
    }
}

impl RoomCommitStorageV1 for PostgresRoomStore {
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        let mut client = match self.connect() {
            Ok(client) => client,
            Err(error) => {
                self.record_error(&error);
                return RoomCommitResolutionV1::Indeterminate;
            }
        };
        let mut transaction = match client.transaction() {
            Ok(transaction) => transaction,
            Err(error) => {
                self.record_error(&error);
                return RoomCommitResolutionV1::Indeterminate;
            }
        };
        let result = self.commit_transaction(&mut transaction, prepared);
        match result {
            Ok(resolution) => {
                if !matches!(
                    &resolution,
                    RoomCommitResolutionV1::GenesisCreated { .. }
                        | RoomCommitResolutionV1::TransitionCommitted { .. }
                        | RoomCommitResolutionV1::RejectionRecorded { .. }
                        | RoomCommitResolutionV1::NoChangeRecorded { .. }
                ) {
                    let _ = transaction.rollback();
                    return resolution;
                }
                if self.take_failpoint(PostgresFailpoint::RollbackBeforeCommit) {
                    let _ = transaction.rollback();
                    return RoomCommitResolutionV1::RetryableKnownAbsent;
                }
                if let Err(error) = transaction.commit() {
                    self.record_error(&error);
                    return RoomCommitResolutionV1::Indeterminate;
                }
                if self.take_failpoint(PostgresFailpoint::UnknownAfterCommit) {
                    RoomCommitResolutionV1::Indeterminate
                } else {
                    resolution
                }
            }
            Err(CommitDecision::Resolution(resolution)) => {
                let _ = transaction.rollback();
                resolution
            }
            Err(CommitDecision::Provider(error)) => {
                let failure = self.record_error(&error);
                let _ = transaction.rollback();
                if matches!(
                    failure,
                    PostgresStorageFailure::Deadlock
                        | PostgresStorageFailure::LockTimeout
                        | PostgresStorageFailure::Serialization
                ) {
                    RoomCommitResolutionV1::RetryableKnownAbsent
                } else {
                    RoomCommitResolutionV1::Indeterminate
                }
            }
        }
    }

    fn resolve(
        &self,
        identity: &worldstream_core::OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1 {
        let mut client = match self.connect() {
            Ok(client) => client,
            Err(error) => {
                self.record_error(&error);
                return ResolveOutcomeV1::ResolutionUnavailable;
            }
        };
        let mut transaction = match client.transaction() {
            Ok(transaction) => transaction,
            Err(error) => {
                self.record_error(&error);
                return ResolveOutcomeV1::ResolutionUnavailable;
            }
        };
        // Resolution is observational. A lookup for an unknown identity must
        // not create a guard that could change a later commit decision.
        let result = resolve_receipt_in_client(&mut transaction, identity, request_hash);
        let _ = transaction.rollback();
        result
    }
}

fn postgres_canonical_bytes(value: &serde_json::Value) -> Result<Vec<u8>, PostgresActivationError> {
    CanonicalJsonV1::parse(
        &serde_json::to_vec(value).map_err(|_| PostgresActivationError::InvalidRequest)?,
    )
    .and_then(|canonical| canonical.to_bytes())
    .map_err(|_| PostgresActivationError::InvalidRequest)
}

fn postgres_nonnegative_u64(
    row: &postgres::Row,
    index: usize,
) -> Result<u64, PostgresActivationError> {
    u64::try_from(
        row.try_get::<_, i64>(index)
            .map_err(PostgresActivationError::Sql)?,
    )
    .map_err(|_| PostgresActivationError::Corrupt)
}

fn postgres_lock_runner_authority(
    _store: &PostgresRoomStore,
    tx: &mut Transaction<'_>,
    _authority: &RunnerControlAdapterInputV1,
) -> Result<(), PostgresActivationError> {
    let present: bool = tx
        .query_opt(
            "SELECT authority_id FROM worldstream_authority_state WHERE authority_id = true FOR SHARE",
            &[],
        )
        .map_err(PostgresActivationError::Sql)?
        .ok_or(PostgresActivationError::Fenced)?
        .try_get(0)
        .map_err(PostgresActivationError::Sql)?;
    if !present {
        return Err(PostgresActivationError::Corrupt);
    }
    Ok(())
}

fn postgres_lock_activation_target(
    tx: &mut Transaction<'_>,
    room_id: &str,
    member_id: &str,
) -> Result<(), PostgresActivationError> {
    let status: String = tx
        .query_opt(
            "SELECT integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR SHARE",
            &[&room_id],
        )
        .map_err(PostgresActivationError::Sql)?
        .ok_or(PostgresActivationError::Fenced)?
        .try_get(0)
        .map_err(PostgresActivationError::Sql)?;
    if status != "healthy" {
        return Err(PostgresActivationError::Fenced);
    }
    tx.query_opt(
        "SELECT member_id FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR SHARE",
        &[&room_id, &member_id],
    )
    .map_err(PostgresActivationError::Sql)?
    .ok_or(PostgresActivationError::Fenced)?;
    Ok(())
}

fn postgres_revalidate_runner_authority(
    store: &PostgresRoomStore,
    tx: &mut Transaction<'_>,
    authority: &RunnerControlAdapterInputV1,
) -> Result<u64, PostgresActivationError> {
    let snapshot = AuthorityStoreV1::snapshot(store, &authority.authority_snapshot_query())
        .map_err(|_| PostgresActivationError::Authority(AuthorityErrorV1::Unavailable))?
        .ok_or(PostgresActivationError::Authority(
            AuthorityErrorV1::Unauthenticated,
        ))?;
    let now: String = tx
        .query_one(
            "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
            &[],
        )
        .and_then(|row| row.try_get(0))
        .map_err(PostgresActivationError::Sql)?;
    let checked_at = postgres_authority_checked_at_from_provider_text(&now)
        .map_err(|_| PostgresActivationError::Corrupt)?;
    authority
        .revalidate_current(&snapshot, &checked_at)
        .map_err(PostgresActivationError::Authority)?;
    Ok(snapshot.capability().generation().get())
}

#[allow(clippy::too_many_lines)]
fn postgres_validate_activation_context(
    tx: &mut Transaction<'_>,
    room_id: &str,
    member_id: &str,
    context: &ActivationInvocationContextV1,
) -> Result<(), PostgresActivationError> {
    let root = tx
        .query_opt(
            "SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR SHARE",
            &[&room_id],
        )
        .map_err(PostgresActivationError::Sql)?
        .ok_or(PostgresActivationError::Fenced)?;
    let head_bytes: Vec<u8> = root.try_get(0).map_err(PostgresActivationError::Sql)?;
    let current_head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
        .map_err(|_| PostgresActivationError::Corrupt)?;
    let integrity_generation = postgres_nonnegative_u64(&root, 1)?;
    let status: String = root.try_get(2).map_err(PostgresActivationError::Sql)?;
    if status != "healthy"
        || current_head != context.room_head
        || current_head.room_id().as_str() != room_id
        || context.integrity_generation != integrity_generation
    {
        return Err(PostgresActivationError::Fenced);
    }
    let row = tx
        .query_opt(
            "SELECT m.frame_head, m.retained_frame_floor, m.last_ack_frame_seq, m.reset_required_through, m.membership_bytes, m.membership_generation, r.core_state_bytes FROM worldstream_members m JOIN worldstream_materializations r ON r.room_id = m.room_id WHERE m.room_id = $1 AND m.member_id = $2 FOR SHARE OF m, r",
            &[&room_id, &member_id],
        )
        .map_err(PostgresActivationError::Sql)?
        .ok_or(PostgresActivationError::Fenced)?;
    let frame_head = postgres_nonnegative_u64(&row, 0)?;
    let retained_floor = postgres_nonnegative_u64(&row, 1)?;
    let cursor = row
        .try_get::<_, Option<i64>>(2)
        .map_err(PostgresActivationError::Sql)?
        .map(|value| u64::try_from(value).map_err(|_| PostgresActivationError::Corrupt))
        .transpose()?;
    let reset_required_through = row
        .try_get::<_, Option<i64>>(3)
        .map_err(PostgresActivationError::Sql)?
        .map(|value| u64::try_from(value).map_err(|_| PostgresActivationError::Corrupt))
        .transpose()?;
    let membership_bytes: Vec<u8> = row.try_get(4).map_err(PostgresActivationError::Sql)?;
    let membership_generation = postgres_nonnegative_u64(&row, 5)?;
    let core_state_bytes: Vec<u8> = row.try_get(6).map_err(PostgresActivationError::Sql)?;
    let membership = CanonicalJsonV1::decode_canonical::<MembershipV1>(&membership_bytes)
        .map_err(|_| PostgresActivationError::Corrupt)?;
    let core =
        CanonicalJsonV1::decode_canonical::<worldstream_core::CoreRoomStateV1>(&core_state_bytes)
            .map_err(|_| PostgresActivationError::Corrupt)?;
    if membership.member_id().as_str() != member_id
        || membership.standing() != MembershipStandingV1::Enabled
        || membership.access_mode() != AccessModeV1::Participant
        || membership.principal_kind() != worldstream_core::PrincipalKindV1::Agent
        || membership.role().is_none()
        || core.room_status() != RoomStatusV1::Active
        || core.membership(membership.member_id()) != Some(&membership)
        || context.frame_head != frame_head
        || context.retained_floor != retained_floor
        || context.cursor != cursor
        || context.membership_generation != membership_generation
        || CanonicalJsonV1::from_canonical_bytes(&context.projection_bytes).is_err()
        || CanonicalJsonV1::from_canonical_bytes(&context.action_offers_bytes).is_err()
    {
        return Err(PostgresActivationError::Fenced);
    }
    match &context.delivery {
        ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head,
            reason,
        } => {
            let expected_reason = if cursor.is_none() {
                "first_attach"
            } else if reset_required_through.is_some() {
                "reset_marked"
            } else if cursor.is_some_and(|value| value.saturating_add(1) < retained_floor) {
                "retained_range_unavailable"
            } else {
                return Err(PostgresActivationError::Fenced);
            };
            if *baseline_frame_head != frame_head || reason != expected_reason {
                return Err(PostgresActivationError::Fenced);
            }
        }
        ActivationDeliveryV1::RetainedFrames {
            cursor_exclusive,
            through_frame_head,
            frames,
        } => {
            let cursor = cursor.ok_or(PostgresActivationError::Fenced)?;
            if reset_required_through.is_some()
                || cursor.saturating_add(1) < retained_floor
                || *cursor_exclusive != cursor
                || *through_frame_head != frame_head
            {
                return Err(PostgresActivationError::Fenced);
            }
            let rows = tx
                .query(
                    "SELECT frame_seq, cause_room_seq, payload_hash, payload_bytes FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq > $3 AND frame_seq <= $4 ORDER BY frame_seq",
                    &[
                        &room_id,
                        &member_id,
                        &i64::try_from(cursor).map_err(|_| PostgresActivationError::Corrupt)?,
                        &i64::try_from(frame_head).map_err(|_| PostgresActivationError::Corrupt)?,
                    ],
                )
                .map_err(PostgresActivationError::Sql)?;
            if rows.len() != frames.len() {
                return Err(PostgresActivationError::Fenced);
            }
            for (row, frame) in rows.iter().zip(frames) {
                let payload_hash: Vec<u8> = row.try_get(2).map_err(PostgresActivationError::Sql)?;
                let payload_bytes: Vec<u8> =
                    row.try_get(3).map_err(PostgresActivationError::Sql)?;
                if postgres_nonnegative_u64(row, 0)? != frame.frame_seq
                    || postgres_nonnegative_u64(row, 1)? != frame.cause_room_seq.get()
                    || payload_hash != frame.payload_hash.as_bytes()
                    || payload_bytes != frame.payload_bytes
                    || frame.payload_hash != Blake3DigestV1::hash(&frame.payload_bytes)
                {
                    return Err(PostgresActivationError::Fenced);
                }
            }
        }
    }
    Ok(())
}

fn postgres_activation_request_hash(
    request: &ActivationOperationRequestV1,
) -> Result<CanonicalRequestHashV1, PostgresActivationError> {
    request
        .canonical_request_hash()
        .map_err(|_| PostgresActivationError::InvalidRequest)
}

fn postgres_activation_state(
    state: &str,
) -> Result<Option<ActivationIntentStateV1>, PostgresActivationError> {
    Ok(Some(match state {
        "pending" => ActivationIntentStateV1::Pending,
        "leased" => ActivationIntentStateV1::Leased,
        "completed" => ActivationIntentStateV1::Completed,
        "expired" => ActivationIntentStateV1::Expired,
        "cancelled" => ActivationIntentStateV1::Cancelled,
        _ => return Err(PostgresActivationError::Corrupt),
    }))
}

fn postgres_activation_result(
    request: &ActivationOperationRequestV1,
    code: ActivationResultCodeV1,
    state: Option<ActivationIntentStateV1>,
    lease_generation: Option<u64>,
    context: Option<ActivationInvocationContextV1>,
) -> Result<ActivationOperationResultV1, PostgresActivationError> {
    let context_hash = context
        .as_ref()
        .map(ActivationInvocationContextV1::context_hash)
        .transpose()
        .map_err(|_| PostgresActivationError::InvalidRequest)?;
    Ok(ActivationOperationResultV1 {
        operation_id: request.operation_id.clone(),
        activation_id: request.activation_id.clone(),
        claim_id: request.claim_id.clone(),
        runner_id: request.runner_id.clone(),
        code,
        state,
        lease_generation,
        context_hash,
        context,
    })
}

fn postgres_activation_bytes(
    result: &ActivationOperationResultV1,
) -> Result<Vec<u8>, PostgresActivationError> {
    let bytes = serde_json::to_vec(result).map_err(|_| PostgresActivationError::InvalidRequest)?;
    CanonicalJsonV1::parse(&bytes)
        .and_then(|value| value.to_bytes())
        .map_err(|_| PostgresActivationError::InvalidRequest)
}

fn postgres_activation_receipt(
    tx: &mut Transaction<'_>,
    room_id: &str,
    request: &ActivationOperationRequestV1,
    request_hash: &CanonicalRequestHashV1,
    retire_contextless: bool,
) -> Result<Option<ActivationOperationResultV1>, PostgresActivationError> {
    let Some(row) = tx
        .query_opt(
            "SELECT canonical_request_hash, result_bytes, context_hash, context_bytes FROM worldstream_activation_operation_receipts WHERE room_id = $1 AND operation_id = $2 FOR UPDATE",
            &[&room_id, &request.operation_id],
        )
        .map_err(PostgresActivationError::Sql)?
    else {
        return Ok(None);
    };
    let stored: Vec<u8> = row.try_get(0).map_err(PostgresActivationError::Sql)?;
    if stored != request_hash.as_bytes() {
        return Err(PostgresActivationError::IdempotencyConflict);
    }
    let bytes: Vec<u8> = row.try_get(1).map_err(PostgresActivationError::Sql)?;
    let stored_context_hash: Option<Vec<u8>> =
        row.try_get(2).map_err(PostgresActivationError::Sql)?;
    let context_bytes: Option<Vec<u8>> = row.try_get(3).map_err(PostgresActivationError::Sql)?;
    let mut result: ActivationOperationResultV1 =
        CanonicalJsonV1::decode_canonical(&bytes).map_err(|_| PostgresActivationError::Corrupt)?;
    let result_context_hash = result
        .context_hash
        .as_ref()
        .map(|hash| hash.as_bytes().as_slice());
    if result_context_hash != stored_context_hash.as_deref() {
        return Err(PostgresActivationError::Corrupt);
    }
    if let Some(context_bytes) = context_bytes.as_deref() {
        let computed = Blake3DigestV1::hash(context_bytes);
        if stored_context_hash.as_deref() != Some(computed.as_bytes().as_slice()) {
            return Err(PostgresActivationError::Corrupt);
        }
    }
    if retire_contextless
        && result.code == ActivationResultCodeV1::Granted
        && context_bytes.is_none()
    {
        result.code = ActivationResultCodeV1::ResultRetired;
        result.context = None;
    }
    Ok(Some(result))
}

fn postgres_insert_activation_receipt(
    tx: &mut Transaction<'_>,
    room_id: &str,
    request: &ActivationOperationRequestV1,
    request_hash: &CanonicalRequestHashV1,
    result: &ActivationOperationResultV1,
    context_bytes: Option<&[u8]>,
) -> Result<(), PostgresActivationError> {
    let result_bytes = postgres_activation_bytes(result)?;
    let context_hash = result
        .context_hash
        .as_ref()
        .map(|hash| hash.as_bytes().to_vec());
    tx.execute(
        "INSERT INTO worldstream_activation_operation_receipts(room_id, operation_id, operation_kind, canonical_request_hash, activation_id, result_code, result_bytes, context_hash, context_bytes) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[&room_id, &request.operation_id, &request.operation_kind, &request_hash.as_bytes().as_slice(), &request.activation_id, &format!("{:?}", result.code).to_lowercase(), &result_bytes, &context_hash, &context_bytes],
    )
    .map_err(|error| {
        if error.as_db_error().is_some_and(|db| db.code().code() == "23505") {
            PostgresActivationError::IdempotencyConflict
        } else {
            PostgresActivationError::Sql(error)
        }
    })?;
    Ok(())
}

fn postgres_activation_offers(
    tx: &mut Transaction<'_>,
    room_id: &str,
    member_id: &str,
) -> Result<Vec<PostgresActivationOfferV1>, PostgresActivationError> {
    let rows = tx.query(
        "SELECT activation_id, target_member_id, cause_room_seq, reason_code, priority, semantic_deadline, policy_revision FROM worldstream_activation_intents WHERE room_id = $1 AND target_member_id = $2 AND state = 'pending' ORDER BY priority DESC, cause_room_seq, activation_id",
        &[&room_id, &member_id],
    ).map_err(PostgresActivationError::Sql)?;
    rows.into_iter()
        .map(|row| {
            Ok(PostgresActivationOfferV1 {
                activation_id: row.try_get(0).map_err(PostgresActivationError::Sql)?,
                room_id: room_id.to_owned(),
                target_member_id: row.try_get(1).map_err(PostgresActivationError::Sql)?,
                cause_room_seq: u64::try_from(
                    row.try_get::<_, i64>(2)
                        .map_err(PostgresActivationError::Sql)?,
                )
                .map_err(|_| PostgresActivationError::Corrupt)?,
                reason_code: row.try_get(3).map_err(PostgresActivationError::Sql)?,
                priority: u64::try_from(
                    row.try_get::<_, i64>(4)
                        .map_err(PostgresActivationError::Sql)?,
                )
                .map_err(|_| PostgresActivationError::Corrupt)?,
                semantic_deadline: row.try_get(5).map_err(PostgresActivationError::Sql)?,
                policy_revision: u64::try_from(
                    row.try_get::<_, i64>(6)
                        .map_err(PostgresActivationError::Sql)?,
                )
                .map_err(|_| PostgresActivationError::Corrupt)?,
                maximum_lease_ms: MAX_ACTIVATION_LEASE_MS,
            })
        })
        .collect()
}

fn postgres_lease_until(
    tx: &mut Transaction<'_>,
    milliseconds: u64,
) -> Result<String, PostgresActivationError> {
    if milliseconds == 0 || milliseconds > MAX_ACTIVATION_LEASE_MS {
        return Err(PostgresActivationError::InvalidRequest);
    }
    let millis =
        i64::try_from(milliseconds).map_err(|_| PostgresActivationError::InvalidRequest)?;
    let lease_until: String = tx.query_one(
        "SELECT to_char((clock_timestamp() + ($1::bigint * interval '1 millisecond')) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        &[&millis],
    )
    .and_then(|row| row.try_get(0))
    .map_err(PostgresActivationError::Sql)?;
    postgres_authority_checked_at_from_provider_text(&lease_until)
        .map(|lease_until| lease_until.as_str().to_owned())
        .map_err(|_| PostgresActivationError::Corrupt)
}

#[cfg(test)]
mod activation_lifecycle_tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fixture {
        leases: HashMap<String, (String, u64, bool)>,
        receipts: HashMap<String, (CanonicalRequestHashV1, ActivationResultCodeV1)>,
    }

    impl Fixture {
        fn claim(
            &mut self,
            request: &ActivationOperationRequestV1,
            expired: bool,
        ) -> Result<ActivationResultCodeV1, PostgresActivationError> {
            let hash = postgres_activation_request_hash(request)?;
            if let Some((stored, code)) = self.receipts.get(&request.operation_id) {
                if stored != &hash {
                    return Err(PostgresActivationError::IdempotencyConflict);
                }
                return Ok(*code);
            }
            let activation = request
                .activation_id
                .as_deref()
                .ok_or(PostgresActivationError::InvalidRequest)?;
            let entry = self
                .leases
                .get_mut(activation)
                .ok_or(PostgresActivationError::Fenced)?;
            if entry.2 || expired || request.lease_generation != Some(entry.1) {
                return Ok(ActivationResultCodeV1::StaleLease);
            }
            entry.2 = true;
            entry.0 = request.runner_id.clone();
            entry.1 += 1;
            self.receipts.insert(
                request.operation_id.clone(),
                (hash, ActivationResultCodeV1::Granted),
            );
            Ok(ActivationResultCodeV1::Granted)
        }
    }

    fn request(operation_id: &str, runner: &str, generation: u64) -> ActivationOperationRequestV1 {
        ActivationOperationRequestV1 {
            operation_kind: "claim".to_owned(),
            operation_id: operation_id.to_owned(),
            activation_id: Some("activation:1".to_owned()),
            claim_id: Some(operation_id.to_owned()),
            runner_id: runner.to_owned(),
            lease_generation: Some(generation),
            requested_lease_ms: Some(1_000),
            disposition: None,
        }
    }

    #[test]
    fn activation_fixture_happy_path_and_duplicate_identity() {
        let mut fixture = Fixture {
            leases: HashMap::from([("activation:1".to_owned(), (String::new(), 0, false))]),
            ..Fixture::default()
        };
        let first = request("claim:1", "runner:a", 0);
        assert_eq!(
            fixture
                .claim(&first, false)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::Granted
        );
        assert_eq!(
            fixture
                .claim(&first, false)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::Granted
        );
        let conflict = request("claim:1", "runner:b", 0);
        assert!(matches!(
            fixture.claim(&conflict, false),
            Err(PostgresActivationError::IdempotencyConflict)
        ));
    }

    #[test]
    fn activation_fixture_rejects_stale_generation_and_expiry() {
        let mut fixture = Fixture {
            leases: HashMap::from([("activation:1".to_owned(), (String::new(), 0, false))]),
            ..Fixture::default()
        };
        assert_eq!(
            fixture
                .claim(&request("claim:stale", "runner:a", 4), false)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::StaleLease
        );
        assert_eq!(
            fixture
                .claim(&request("claim:expired", "runner:a", 0), true)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::StaleLease
        );
    }

    #[test]
    fn activation_fixture_allows_one_live_lease_only() {
        let mut fixture = Fixture {
            leases: HashMap::from([("activation:1".to_owned(), (String::new(), 0, false))]),
            ..Fixture::default()
        };
        assert_eq!(
            fixture
                .claim(&request("claim:a", "runner:a", 0), false)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::Granted
        );
        assert_eq!(
            fixture
                .claim(&request("claim:b", "runner:b", 1), false)
                .unwrap_or_else(|_| unreachable!()),
            ActivationResultCodeV1::StaleLease
        );
    }
}

enum CommitDecision {
    Resolution(RoomCommitResolutionV1),
    Provider(postgres::Error),
}

impl PostgresRoomStore {
    fn commit_transaction(
        &self,
        tx: &mut Transaction<'_>,
        prepared: &PreparedRoomWriteV1,
    ) -> Result<RoomCommitResolutionV1, CommitDecision> {
        let identity_bytes = prepared
            .identity()
            .canonical_bytes()
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let request_hash = prepared.request_hash();
        let inserted = tx.execute(
            "INSERT INTO worldstream_operation_guards(identity_bytes, request_hash) VALUES ($1, $2) ON CONFLICT (identity_bytes) DO NOTHING",
            &[&identity_bytes, &request_hash.as_bytes().as_slice()],
        ).map_err(CommitDecision::Provider)?;
        let row = tx.query_one("SELECT request_hash, receipt_bytes FROM worldstream_operation_guards WHERE identity_bytes = $1 FOR UPDATE", &[&identity_bytes]).map_err(CommitDecision::Provider)?;
        let stored_hash: Vec<u8> = row
            .try_get(0)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let stored_receipt: Option<Vec<u8>> = row
            .try_get(1)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if inserted == 0 {
            if stored_hash != request_hash.as_bytes() {
                let existing_request_hash = CanonicalRequestHashV1::from_str(&format!(
                    "blake3:{}",
                    hex_bytes(&stored_hash)
                ))
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
                return Ok(RoomCommitResolutionV1::Conflict {
                    existing_request_hash,
                });
            }
            let Some(bytes) = stored_receipt else {
                return Err(CommitDecision::Resolution(
                    RoomCommitResolutionV1::Indeterminate,
                ));
            };
            let stored = StoredSemanticResultV1::from_canonical_receipt_bytes(&bytes)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            return Ok(RoomCommitResolutionV1::resolved(
                ResolutionStatusV1::Existing,
                stored,
            ));
        }
        let authority_current = match prepared {
            PreparedRoomWriteV1::Create(create) => {
                self.authority_is_current(tx, create.authority_witness())?
            }
            PreparedRoomWriteV1::Existing(existing) => {
                self.authority_is_current(tx, existing.authority_witness())?
            }
        };
        if !authority_current {
            return Ok(RoomCommitResolutionV1::Fenced);
        }
        let identity_bytes = prepared
            .identity()
            .canonical_bytes()
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        match prepared {
            PreparedRoomWriteV1::Create(create) => self.commit_create(tx, create, &identity_bytes),
            PreparedRoomWriteV1::Existing(existing) => {
                self.commit_existing(tx, existing, &identity_bytes)
            }
        }
    }

    fn authority_is_current(
        &self,
        tx: &mut Transaction<'_>,
        witness: &PreparedAuthorityWitnessV1,
    ) -> Result<bool, CommitDecision> {
        let _ = self;
        #[cfg(feature = "conformance-tracer")]
        if self.conformance_capability_commit.load(Ordering::SeqCst) {
            return Ok(true);
        }
        #[cfg(feature = "conformance-tracer")]
        if let Some((id, principal, generation, hash)) = witness.conformance_key() {
            let row = tx
                .query_opt(
                    "SELECT authenticated_principal, generation, scope_revocation_hash, active FROM worldstream_authority_fences WHERE witness_id = $1",
                    &[&id],
                )
                .map_err(CommitDecision::Provider)?;
            let Some(row) = row else {
                return Ok(false);
            };
            let stored_principal: String = row
                .try_get(0)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            let stored_generation: i64 = row
                .try_get(1)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            let stored_hash: Vec<u8> = row
                .try_get(2)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            let active: bool = row
                .try_get(3)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            return Ok(active
                && stored_principal == principal.to_string()
                && stored_generation == i64::try_from(generation).unwrap_or(-1)
                && stored_hash == hash.as_bytes());
        }
        let Some(query) = witness.authority_snapshot_query() else {
            return Ok(false);
        };
        let Some(snapshot) = authority::load_authority_snapshot_for_adapter(tx, &query)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Indeterminate))?
        else {
            return Ok(false);
        };
        let checked_at = postgres_authority_checked_at(tx)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Indeterminate))?;
        Ok(witness.revalidate_current(&snapshot, &checked_at).is_ok())
    }

    fn commit_create(
        &self,
        tx: &mut Transaction<'_>,
        create: &worldstream_core::PreparedRoomCreationV1,
        identity_bytes: &[u8],
    ) -> Result<RoomCommitResolutionV1, CommitDecision> {
        let _ = self;
        let persistence = create.persistence();
        let room_id = persistence.complete_head.room_id().to_string();
        // A missing root cannot be locked with SELECT ... FOR UPDATE.  Claim
        // the root row itself inside this transaction so two different
        // operation identities racing to create the same Room resolve as a
        // deterministic reprepare instead of leaking a unique-key error.
        let claimed_root = tx
            .query_opt(
                "INSERT INTO worldstream_room_roots(room_id, head_bytes, integrity_generation, integrity_status) VALUES ($1, $2, $3, 'healthy') ON CONFLICT (room_id) DO NOTHING RETURNING room_id",
                &[
                    &room_id,
                    &persistence.canonical_head_bytes,
                    &i64::try_from(persistence.integrity_generation.get()).unwrap_or(-1),
                ],
            )
            .map_err(CommitDecision::Provider)?;
        if claimed_root.is_none() {
            return Ok(RoomCommitResolutionV1::Reprepare);
        }
        insert_creation(tx, persistence).map_err(CommitDecision::Provider)?;
        let receipt = create.semantic_result().canonical_receipt_bytes().to_vec();
        tx.execute("UPDATE worldstream_operation_guards SET room_id = $1, receipt_bytes = $2 WHERE identity_bytes = $3", &[&room_id, &receipt, &identity_bytes]).map_err(CommitDecision::Provider)?;
        persist_semantic_receipt(tx, create.semantic_result(), identity_bytes)?;
        let stored = create.semantic_result().clone();
        Ok(RoomCommitResolutionV1::resolved(
            ResolutionStatusV1::New,
            stored,
        ))
    }

    fn commit_existing(
        &self,
        tx: &mut Transaction<'_>,
        existing: &worldstream_core::PreparedRoomCommitV1,
        identity_bytes: &[u8],
    ) -> Result<RoomCommitResolutionV1, CommitDecision> {
        let _ = self;
        let room_id = existing.basis_complete_head().room_id().to_string();
        let row = tx.query_opt("SELECT head_bytes, integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE", &[&room_id]).map_err(CommitDecision::Provider)?;
        let Some(row) = row else {
            return Ok(RoomCommitResolutionV1::Fault);
        };
        let head_bytes: Vec<u8> = row
            .try_get(0)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let generation: i64 = row
            .try_get(1)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let status: String = row
            .try_get(2)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if status != "healthy"
            || generation != i64::try_from(existing.integrity_generation().get()).unwrap_or(-1)
        {
            return Ok(RoomCommitResolutionV1::Fenced);
        }
        let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if &head != existing.basis_complete_head() {
            return Ok(RoomCommitResolutionV1::Reprepare);
        }
        match existing.intent() {
            PreparedExistingIntentV1::DurableDisposition => {
                let receipt = existing
                    .semantic_result()
                    .canonical_receipt_bytes()
                    .to_vec();
                finish_receipt(tx, existing, &receipt, identity_bytes)?;
            }
            PreparedExistingIntentV1::Advance(advance) => {
                if advance.transition.room_seq() != advance.resulting_complete_head.room_seq()
                    || advance.transition.transition_hash()
                        != advance.resulting_complete_head.genesis_or_transition_hash()
                    || advance.transition.resulting_core_state_hash()
                        != advance.resulting_complete_head.core_state_hash()
                    || advance.transition.resulting_activity_state_hash()
                        != advance.resulting_complete_head.activity_state_hash()
                    || advance.transition.resulting_authoritative_state_hash()
                        != advance.resulting_complete_head.authoritative_state_hash()
                {
                    return Ok(RoomCommitResolutionV1::Fault);
                }
                validate_advance_witnesses(tx, &room_id, existing, advance)?;
                persist_advance(tx, &room_id, existing.integrity_generation(), advance)?;
                let receipt = existing
                    .semantic_result()
                    .canonical_receipt_bytes()
                    .to_vec();
                finish_receipt(tx, existing, &receipt, identity_bytes)?;
            }
        }
        Ok(RoomCommitResolutionV1::resolved(
            ResolutionStatusV1::New,
            existing.semantic_result().clone(),
        ))
    }
}

fn validate_advance_witnesses(
    tx: &mut Transaction<'_>,
    room_id: &str,
    existing: &worldstream_core::PreparedRoomCommitV1,
    advance: &PreparedAdvancePersistenceV1,
) -> Result<(), CommitDecision> {
    if let worldstream_core::PreparedOperationInputWitnessV1::TimerFired(witness) =
        existing.input_witness()
    {
        let identity = match witness.request.operation_identity() {
            worldstream_core::OperationIdentityV1::TimerFired(identity) => identity,
            _ => return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault)),
        };
        let row = tx
            .query_opt(
                "SELECT scheduled_for, payload_bytes, state FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2 AND generation = $3",
                &[
                    &room_id,
                    &identity.timer_id.to_string(),
                    &i64::try_from(identity.generation.get()).unwrap_or(-1),
                ],
            )
            .map_err(CommitDecision::Provider)?;
        let Some(row) = row else {
            return Err(CommitDecision::Resolution(
                RoomCommitResolutionV1::NotApplicable,
            ));
        };
        let scheduled_for: String = row
            .try_get(0)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let payload: Vec<u8> = row
            .try_get(1)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        let state: String = row
            .try_get(2)
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if state != "scheduled" {
            return Err(CommitDecision::Resolution(
                RoomCommitResolutionV1::NotApplicable,
            ));
        }
        if scheduled_for != identity.scheduled_for.to_string()
            || payload != witness.canonical_timer_payload_bytes
        {
            return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
        }
        let changed = tx
            .execute(
            "UPDATE worldstream_timers SET state = 'fired' WHERE room_id = $1 AND timer_id = $2 AND generation = $3 AND scheduled_for = $4 AND payload_bytes = $5 AND state = 'scheduled'",
            &[
                &room_id,
                &identity.timer_id.to_string(),
                &i64::try_from(identity.generation.get()).unwrap_or(-1),
                &identity.scheduled_for.to_string(),
                &witness.canonical_timer_payload_bytes,
            ],
        )
        .map_err(CommitDecision::Provider)?;
        if changed != 1 {
            return Err(CommitDecision::Resolution(
                RoomCommitResolutionV1::NotApplicable,
            ));
        }
    }

    for consequence in &advance.delivery_consequences {
        if let worldstream_core::PreparedObservationConsequenceV1::ObservationFrame(frame) =
            consequence
        {
            let row = tx
                .query_opt(
                    "SELECT frame_head FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR UPDATE",
                    &[&room_id, &frame.member_id().to_string()],
                )
                .map_err(CommitDecision::Provider)?;
            let Some(row) = row else {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            };
            let frame_head: i64 = row
                .try_get(0)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            if frame_head != i64::try_from(frame.previous_frame_head()).unwrap_or(-1) {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fenced));
            }
        }
    }
    Ok(())
}

fn finish_receipt(
    tx: &mut Transaction<'_>,
    existing: &worldstream_core::PreparedRoomCommitV1,
    receipt: &[u8],
    identity_bytes: &[u8],
) -> Result<(), CommitDecision> {
    tx.execute("UPDATE worldstream_operation_guards SET room_id = $1, receipt_bytes = $2 WHERE identity_bytes = $3", &[&existing.basis_complete_head().room_id().to_string(), &receipt, &identity_bytes]).map_err(CommitDecision::Provider)?;
    persist_semantic_receipt(tx, existing.semantic_result(), identity_bytes)
}

fn persist_semantic_receipt(
    tx: &mut Transaction<'_>,
    result: &StoredSemanticResultV1,
    identity_bytes: &[u8],
) -> Result<(), CommitDecision> {
    let basis = result
        .canonical_basis_head_bytes()
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
    let semantic_input = result
        .semantic_input()
        .canonical_bytes()
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
    let semantic_time = result
        .canonical_semantic_time_bytes()
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
    let transition_seq = result
        .transition_seq()
        .map(|sequence| i64::try_from(sequence.get()))
        .transpose()
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
    tx.execute(
        "INSERT INTO worldstream_semantic_receipts(identity_bytes, room_id, operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        &[
            &identity_bytes,
            &result.target_room_id().to_string(),
            &result.operation_identity().operation_kind(),
            &result.canonical_request_hash().as_bytes().as_slice(),
            &basis,
            &semantic_input,
            &semantic_time,
            &result.resolution_kind(),
            &transition_seq,
            &result.canonical_receipt_bytes(),
        ],
    )
    .map_err(CommitDecision::Provider)?;
    Ok(())
}

fn insert_creation(
    tx: &mut Transaction<'_>,
    p: &PreparedCreationPersistenceV1,
) -> Result<(), postgres::Error> {
    let head = p.complete_head.room_id().to_string();
    tx.execute("INSERT INTO worldstream_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) VALUES ($1, $2, $3)", &[&head, &p.canonical_pack_revision_lock_bytes, &p.canonical_genesis_bytes])?;
    tx.execute("INSERT INTO worldstream_materializations(room_id, core_state_bytes, activity_state_bytes) VALUES ($1, $2, $3)", &[&head, &p.canonical_core_state_bytes, &p.canonical_activity_state_bytes])?;
    for member in &p.memberships {
        insert_member(tx, &head, member)?;
    }
    for timer in &p.initial_timers {
        tx.execute("INSERT INTO worldstream_timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) VALUES ($1, $2, $3, $4, $5, 'scheduled')", &[&head, &timer.timer_id().to_string(), &i64::try_from(timer.generation().get()).unwrap_or(-1), &timer.scheduled_for().to_string(), &timer.canonical_payload_bytes()])?;
    }
    persist_snapshot(
        tx,
        &head,
        &p.complete_head,
        &p.canonical_head_bytes,
        &p.canonical_core_state_bytes,
        &p.canonical_activity_state_bytes,
    )?;
    Ok(())
}

fn persist_snapshot(
    tx: &mut Transaction<'_>,
    room_id: &str,
    head: &CompleteHeadV1,
    head_bytes: &[u8],
    core_state_bytes: &[u8],
    activity_state_bytes: &[u8],
) -> Result<(), postgres::Error> {
    let room_seq = i64::try_from(head.room_seq().get()).unwrap_or(-1);
    tx.execute(
        "INSERT INTO worldstream_room_snapshots(room_id, room_seq, snapshot_schema_version, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes) VALUES ($1, $2, 'worldstream/paired-snapshot/v1', $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT (room_id, room_seq) DO UPDATE SET genesis_or_transition_hash = EXCLUDED.genesis_or_transition_hash, core_schema_version = EXCLUDED.core_schema_version, pack_digest = EXCLUDED.pack_digest, core_state_hash = EXCLUDED.core_state_hash, activity_state_hash = EXCLUDED.activity_state_hash, authoritative_state_hash = EXCLUDED.authoritative_state_hash, complete_head_bytes = EXCLUDED.complete_head_bytes, core_state_bytes = EXCLUDED.core_state_bytes, activity_state_bytes = EXCLUDED.activity_state_bytes",
        &[
            &room_id,
            &room_seq,
            &head.genesis_or_transition_hash().to_string(),
            &head.core_schema_version(),
            &head.pack_digest().to_string(),
            &head.core_state_hash().to_string(),
            &head.activity_state_hash().to_string(),
            &head.authoritative_state_hash().to_string(),
            &head_bytes,
            &core_state_bytes,
            &activity_state_bytes,
        ],
    )?;
    Ok(())
}

fn insert_member(
    tx: &mut Transaction<'_>,
    room_id: &str,
    member: &PreparedMembershipMaterializationV1,
) -> Result<(), postgres::Error> {
    tx.execute(
        "INSERT INTO worldstream_members(room_id, member_id, membership_bytes) VALUES ($1, $2, $3)",
        &[
            &room_id,
            &member.membership.member_id().to_string(),
            &member.canonical_membership_bytes,
        ],
    )?;
    Ok(())
}

fn upsert_member(
    tx: &mut Transaction<'_>,
    room_id: &str,
    member: &PreparedMembershipMaterializationV1,
) -> Result<(), postgres::Error> {
    tx.execute(
        "INSERT INTO worldstream_members(room_id, member_id, membership_bytes) VALUES ($1, $2, $3) ON CONFLICT (room_id, member_id) DO UPDATE SET membership_generation = CASE WHEN worldstream_members.membership_bytes <> EXCLUDED.membership_bytes THEN worldstream_members.membership_generation + 1 ELSE worldstream_members.membership_generation END, membership_bytes = EXCLUDED.membership_bytes",
        &[
            &room_id,
            &member.membership.member_id().to_string(),
            &member.canonical_membership_bytes,
        ],
    )?;
    Ok(())
}

fn persist_advance(
    tx: &mut Transaction<'_>,
    room_id: &str,
    expected_integrity_generation: worldstream_core::IntegrityGenerationV1,
    advance: &PreparedAdvancePersistenceV1,
) -> Result<(), CommitDecision> {
    let seq = i64::try_from(advance.transition.room_seq().get()).unwrap_or(-1);
    tx.execute("INSERT INTO worldstream_transitions(room_id, room_seq, transition_bytes) VALUES ($1, $2, $3)", &[&room_id, &seq, &advance.canonical_transition_bytes]).map_err(CommitDecision::Provider)?;
    let changed = tx
        .execute(
        "UPDATE worldstream_room_roots SET head_bytes = $1 WHERE room_id = $2 AND integrity_generation = $3 AND integrity_status = 'healthy'",
        &[
            &advance.canonical_resulting_head_bytes,
            &room_id,
            &i64::try_from(expected_integrity_generation.get()).unwrap_or(-1),
        ],
    )
    .map_err(CommitDecision::Provider)?;
    if changed != 1 {
        return Err(CommitDecision::Resolution(
            RoomCommitResolutionV1::Reprepare,
        ));
    }
    let changed = tx.execute("UPDATE worldstream_materializations SET core_state_bytes = $1, activity_state_bytes = $2 WHERE room_id = $3", &[&advance.canonical_resulting_core_state_bytes, &advance.canonical_resulting_activity_state_bytes, &room_id]).map_err(CommitDecision::Provider)?;
    if changed != 1 {
        return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
    }
    persist_snapshot(
        tx,
        room_id,
        &advance.resulting_complete_head,
        &advance.canonical_resulting_head_bytes,
        &advance.canonical_resulting_core_state_bytes,
        &advance.canonical_resulting_activity_state_bytes,
    )
    .map_err(CommitDecision::Provider)?;
    persist_members_and_cancel_activations(tx, room_id, advance)?;
    for mutation in &advance.timer_changes {
        persist_timer_mutation(tx, room_id, mutation)?;
    }
    for consequence in &advance.delivery_consequences {
        persist_consequence(tx, room_id, seq, consequence)?;
    }
    for decision in &advance.activation_decisions {
        tx.execute("INSERT INTO worldstream_activation_decisions(room_id, cause_room_seq, decision_id, target_member_id, decision_bytes) VALUES ($1, $2, $3, $4, $5)", &[&room_id, &seq, &decision.decision_id(), &decision.target_member_id().map(ToString::to_string), &decision.canonical_decision_bytes()]).map_err(CommitDecision::Provider)?;
        let decision_record = CanonicalJsonV1::decode_canonical::<
            worldstream_core::ActivationDecisionV1,
        >(decision.canonical_decision_bytes())
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if decision_record.cause_room_seq.get() != advance.transition.room_seq().get()
            || decision_record.decision_id != decision.decision_id()
            || decision.target_member_id() != Some(&decision_record.attention.target_member_id)
        {
            return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
        }
        if matches!(
            decision_record.policy.disposition,
            worldstream_core::ActivationPolicyDispositionV1::Intent
        ) {
            let Some(activation_id) = decision_record.activation_id.as_ref() else {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            };
            let attention = &decision_record.attention;
            tx.execute("INSERT INTO worldstream_activation_intents(activation_id, room_id, cause_room_seq, decision_id, target_member_id, reason_code, deduplication_key, priority, semantic_deadline, policy_revision, state, intent_generation, lease_generation) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', 1, 0)", &[
                &activation_id,
                &room_id,
                &seq,
                &decision_record.decision_id,
                &attention.target_member_id.to_string(),
                &attention.reason_code,
                &attention.deduplication_key,
                &i64::from(attention.priority),
                &attention.semantic_deadline.as_ref().map(ToString::to_string),
                &i64::try_from(decision_record.policy.policy_revision).unwrap_or(-1),
            ]).map_err(CommitDecision::Provider)?;
        }
    }
    Ok(())
}

fn persist_members_and_cancel_activations(
    tx: &mut Transaction<'_>,
    room_id: &str,
    advance: &PreparedAdvancePersistenceV1,
) -> Result<(), CommitDecision> {
    let mut changed_members = Vec::new();
    for member in &advance.resulting_memberships {
        let member_id = member.membership.member_id().to_string();
        let prior: Option<Vec<u8>> = tx
            .query_opt(
                "SELECT membership_bytes FROM worldstream_members WHERE room_id = $1 AND member_id = $2",
                &[&room_id, &member_id],
            )
            .map_err(CommitDecision::Provider)?
            .map(|row| row.try_get(0))
            .transpose()
            .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
        if prior.as_deref() != Some(member.canonical_membership_bytes.as_slice()) {
            changed_members.push(member_id);
        }
        upsert_member(tx, room_id, member).map_err(CommitDecision::Provider)?;
    }
    let member_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM worldstream_members WHERE room_id = $1",
            &[&room_id],
        )
        .map_err(CommitDecision::Provider)?
        .try_get(0)
        .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
    if usize::try_from(member_count).ok() != Some(advance.resulting_memberships.len()) {
        return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
    }
    if advance.resulting_core_state.room_status() == worldstream_core::RoomStatusV1::Archived {
        tx.execute(
            "UPDATE worldstream_activation_intents SET state = 'cancelled', runner_id = NULL, claim_id = NULL, lease_until = NULL, intent_generation = intent_generation + 1, lease_generation = lease_generation + 1 WHERE room_id = $1 AND state IN ('pending', 'leased')",
            &[&room_id],
        )
        .map_err(CommitDecision::Provider)?;
    } else {
        for member_id in changed_members {
            tx.execute(
                "UPDATE worldstream_activation_intents SET state = 'cancelled', runner_id = NULL, claim_id = NULL, lease_until = NULL, intent_generation = intent_generation + 1, lease_generation = lease_generation + 1 WHERE room_id = $1 AND target_member_id = $2 AND state IN ('pending', 'leased')",
                &[&room_id, &member_id],
            )
            .map_err(CommitDecision::Provider)?;
        }
    }
    Ok(())
}

fn persist_timer_mutation(
    tx: &mut Transaction<'_>,
    room_id: &str,
    mutation: &worldstream_core::PreparedTimerMutationV1,
) -> Result<(), CommitDecision> {
    match mutation.kind() {
        PreparedTimerMutationKindV1::Schedule {
            timer_id,
            generation,
            scheduled_for,
            canonical_payload_bytes,
        } => {
            let maximum: Option<i64> = tx
                .query_one(
                    "SELECT max(generation) FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2",
                    &[&room_id, &timer_id.to_string()],
                )
                .map_err(CommitDecision::Provider)?
                .try_get(0)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            let generation_value = i64::try_from(generation.get()).unwrap_or(-1);
            if maximum.map_or(1, |value| value.saturating_add(1)) != generation_value {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
            tx.execute("INSERT INTO worldstream_timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) VALUES ($1, $2, $3, $4, $5, 'scheduled')", &[&room_id, &timer_id.to_string(), &generation_value, &scheduled_for.to_string(), &canonical_payload_bytes]).map_err(CommitDecision::Provider)?;
        }
        PreparedTimerMutationKindV1::Cancel {
            timer_id,
            generation,
        } => {
            let changed = tx.execute("UPDATE worldstream_timers SET state = 'cancelled' WHERE room_id = $1 AND timer_id = $2 AND generation = $3 AND state = 'scheduled'", &[&room_id, &timer_id.to_string(), &i64::try_from(generation.get()).unwrap_or(-1)]).map_err(CommitDecision::Provider)?;
            if changed != 1 {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
        }
        PreparedTimerMutationKindV1::Reschedule {
            timer_id,
            previous_generation,
            generation,
            scheduled_for,
            canonical_payload_bytes,
        } => {
            let previous = i64::try_from(previous_generation.get()).unwrap_or(-1);
            let next = i64::try_from(generation.get()).unwrap_or(-1);
            let maximum: Option<i64> = tx
                .query_one(
                    "SELECT max(generation) FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2",
                    &[&room_id, &timer_id.to_string()],
                )
                .map_err(CommitDecision::Provider)?
                .try_get(0)
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            if maximum != Some(previous) || previous.saturating_add(1) != next {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
            let changed = tx.execute("UPDATE worldstream_timers SET state = 'cancelled' WHERE room_id = $1 AND timer_id = $2 AND generation = $3 AND state = 'scheduled'", &[&room_id, &timer_id.to_string(), &previous]).map_err(CommitDecision::Provider)?;
            if changed != 1 {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
            tx.execute("INSERT INTO worldstream_timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) VALUES ($1, $2, $3, $4, $5, 'scheduled')", &[&room_id, &timer_id.to_string(), &next, &scheduled_for.to_string(), &canonical_payload_bytes]).map_err(CommitDecision::Provider)?;
        }
    }
    Ok(())
}

fn persist_consequence(
    tx: &mut Transaction<'_>,
    room_id: &str,
    cause_room_seq: i64,
    consequence: &PreparedObservationConsequenceV1,
) -> Result<(), CommitDecision> {
    match consequence {
        PreparedObservationConsequenceV1::ObservationFrame(frame) => {
            tx.execute("INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) VALUES ($1, $2, $3, $4, $5, $6)", &[&room_id, &frame.member_id().to_string(), &i64::try_from(frame.frame_seq()).unwrap_or(-1), &i64::try_from(frame.cause_room_seq().get()).unwrap_or(-1), &frame.canonical_payload_bytes(), &frame.payload_hash().as_bytes().as_slice()]).map_err(CommitDecision::Provider)?;
            let changed = tx.execute("UPDATE worldstream_members SET frame_head = $1 WHERE room_id = $2 AND member_id = $3 AND frame_head = $4", &[&i64::try_from(frame.frame_seq()).unwrap_or(-1), &room_id, &frame.member_id().to_string(), &i64::try_from(frame.previous_frame_head()).unwrap_or(-1)]).map_err(CommitDecision::Provider)?;
            if changed != 1 {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
        }
        PreparedObservationConsequenceV1::ResetRequired(view) => {
            let projection_hash = view
                .projection_hash()
                .map_err(|_| CommitDecision::Resolution(RoomCommitResolutionV1::Fault))?;
            let changed = tx.execute("UPDATE worldstream_members SET reset_required_through = greatest(coalesce(reset_required_through, 0), frame_head) WHERE room_id = $1 AND member_id = $2", &[&room_id, &view.viewer().member_id().to_string()]).map_err(CommitDecision::Provider)?;
            if changed != 1 {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
            tx.execute("INSERT INTO worldstream_observation_consequences(room_id, member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash) VALUES ($1, $2, $3, 'reset_required', $4, $5)", &[&room_id, &view.viewer().member_id().to_string(), &cause_room_seq, &view.canonical_bytes(), &projection_hash.as_bytes().as_slice()]).map_err(CommitDecision::Provider)?;
        }
        PreparedObservationConsequenceV1::VisibilityLost(member_id) => {
            let changed = tx.execute("UPDATE worldstream_members SET reset_required_through = greatest(coalesce(reset_required_through, 0), frame_head) WHERE room_id = $1 AND member_id = $2", &[&room_id, &member_id.to_string()]).map_err(CommitDecision::Provider)?;
            if changed != 1 {
                return Err(CommitDecision::Resolution(RoomCommitResolutionV1::Fault));
            }
            tx.execute("INSERT INTO worldstream_observation_consequences(room_id, member_id, cause_room_seq, consequence_kind) VALUES ($1, $2, $3, 'visibility_lost')", &[&room_id, &member_id.to_string(), &cause_room_seq]).map_err(CommitDecision::Provider)?;
        }
    }
    Ok(())
}

fn nonnegative_u64(value: i64) -> Result<u64, PostgresObservationError> {
    u64::try_from(value).map_err(|_| PostgresObservationError::Corrupt)
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Deterministic local evidence when no PostgreSQL service is available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresHarnessEvidence {
    LiveProvider,
    UnavailableEnvironment,
}

/// Checks the opt-in provider without claiming a pass when it is absent.
#[must_use]
pub fn probe_from_environment() -> PostgresHarnessEvidence {
    let Ok(dsn) = std::env::var("WORLDSTREAM_POSTGRES_TEST_DSN") else {
        return PostgresHarnessEvidence::UnavailableEnvironment;
    };
    let Ok(config) =
        PostgresConnectionConfig::runtime(dsn, PostgresConnectionPath::TransactionPool)
    else {
        return PostgresHarnessEvidence::UnavailableEnvironment;
    };
    let Ok(mut client) = Client::connect(&config.dsn, config.tls.clone()) else {
        return PostgresHarnessEvidence::UnavailableEnvironment;
    };
    let Ok(row) = client.query_one("SHOW server_version_num", &[]) else {
        return PostgresHarnessEvidence::UnavailableEnvironment;
    };
    let Ok(version) = row.try_get::<_, String>(0) else {
        return PostgresHarnessEvidence::UnavailableEnvironment;
    };
    if version.starts_with("17") {
        PostgresHarnessEvidence::LiveProvider
    } else {
        PostgresHarnessEvidence::UnavailableEnvironment
    }
}

/// Returns the adapter DDL for fixture runners and migration evidence.
#[must_use]
pub const fn migration_sql() -> &'static str {
    SCHEMA
}

/// A deterministic, in-process storage transcript used when CI has no
/// PostgreSQL service. It exercises the same Core prepared values and
/// resolution algebra, but is explicitly not provider evidence.
#[derive(Clone, Default)]
pub struct FixturePostgresStore {
    state: std::sync::Arc<Mutex<FixtureState>>,
}

/// Observable, non-sensitive fixture evidence for the durable kernel parity
/// paths. This is a compact test seam, not a provider API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureParitySnapshot {
    pub timer_count: usize,
    pub frame_count: usize,
    pub observation_consequence_count: usize,
    pub activation_decision_count: usize,
    pub activation_intent_count: usize,
    pub frame_heads: std::collections::BTreeMap<String, u64>,
    pub reset_required_through: std::collections::BTreeMap<String, u64>,
}

#[derive(Default)]
struct FixtureState {
    guards: std::collections::BTreeMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
    rooms: std::collections::BTreeMap<String, FixtureRoom>,
    #[cfg(feature = "conformance-tracer")]
    authorities: std::collections::BTreeMap<String, FixtureAuthority>,
    failpoint: Option<PostgresFailpoint>,
}

#[derive(Default)]
struct FixtureRoom {
    head_bytes: Vec<u8>,
    integrity_generation: u64,
    timers: std::collections::BTreeMap<(String, u64), FixtureTimer>,
    frame_heads: std::collections::BTreeMap<String, u64>,
    reset_required_through: std::collections::BTreeMap<String, u64>,
    frames: std::collections::BTreeMap<(String, u64), (Vec<u8>, Vec<u8>)>,
    observation_consequences: std::collections::BTreeMap<(String, u64), (String, Option<Vec<u8>>)>,
    activation_decisions: std::collections::BTreeMap<(u64, String), (Option<String>, Vec<u8>)>,
    activation_intents: std::collections::BTreeMap<String, u64>,
}

struct FixtureTimer {
    scheduled_for: String,
    payload_bytes: Vec<u8>,
    state: &'static str,
}

#[cfg(feature = "conformance-tracer")]
#[derive(Clone)]
struct FixtureAuthority {
    principal: String,
    generation: u64,
    scope_revocation_hash: Vec<u8>,
    active: bool,
    explicit: bool,
}

impl FixturePostgresStore {
    /// Returns the deterministic fixture evidence tier.
    #[must_use]
    pub const fn evidence() -> PostgresHarnessEvidence {
        PostgresHarnessEvidence::UnavailableEnvironment
    }

    /// Reads compact fixture evidence for one Room after a prepared commit.
    #[must_use]
    pub fn parity_snapshot(&self, room_id: &str) -> Option<FixtureParitySnapshot> {
        let state = self.state.lock().ok()?;
        let room = state.rooms.get(room_id)?;
        Some(FixtureParitySnapshot {
            timer_count: room.timers.len(),
            frame_count: room.frames.len(),
            observation_consequence_count: room.observation_consequences.len(),
            activation_decision_count: room.activation_decisions.len(),
            activation_intent_count: room.activation_intents.len(),
            frame_heads: room.frame_heads.clone(),
            reset_required_through: room.reset_required_through.clone(),
        })
    }

    /// Arms one deterministic rollback/unknown-COMMIT transcript step.
    pub fn set_failpoint(&self, failpoint: Option<PostgresFailpoint>) {
        if let Ok(mut state) = self.state.lock() {
            state.failpoint = failpoint;
        }
    }

    fn resolution_for_receipt(
        receipt: &[u8],
        status: ResolutionStatusV1,
    ) -> RoomCommitResolutionV1 {
        let stored = StoredSemanticResultV1::from_canonical_receipt_bytes(receipt)
            .unwrap_or_else(|_| unreachable!("fixture receipt is Core-sealed"));
        RoomCommitResolutionV1::resolved(status, stored)
    }

    #[cfg(feature = "conformance-tracer")]
    /// Seeds the fixture's operational authority fence. A missing fence is
    /// never inferred by the live adapter; the fixture's first matching write
    /// may seed one only to keep existing Core-only vectors self-contained.
    pub fn seed_conformance_authority(
        &self,
        witness: &PreparedAuthorityWitnessV1,
        active: bool,
    ) -> Result<(), PostgresStorageFailure> {
        let Some((id, principal, generation, hash)) = witness.conformance_key() else {
            return Err(PostgresStorageFailure::Driver);
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| PostgresStorageFailure::Driver)?;
        state.authorities.insert(
            id.to_owned(),
            FixtureAuthority {
                principal: principal.to_string(),
                generation,
                scope_revocation_hash: hash.as_bytes().to_vec(),
                active,
                explicit: true,
            },
        );
        Ok(())
    }

    /// Corrupts a retained receipt in the fixture only, allowing callers to
    /// assert that guarded resolution returns `ResolutionUnavailable`.
    #[must_use]
    pub fn corrupt_receipt(&self, identity: &OperationIdentityV1) -> bool {
        let Ok(identity_bytes) = identity.canonical_bytes() else {
            return false;
        };
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some((_, Some(receipt))) = state.guards.get_mut(&identity_bytes) else {
            return false;
        };
        receipt.push(b' ');
        true
    }

    fn authority_is_current(
        state: &mut FixtureState,
        witness: &PreparedAuthorityWitnessV1,
    ) -> bool {
        #[cfg(feature = "conformance-tracer")]
        {
            let Some((id, principal, generation, hash)) = witness.conformance_key() else {
                return false;
            };
            let expected = FixtureAuthority {
                principal: principal.to_string(),
                generation,
                scope_revocation_hash: hash.as_bytes().to_vec(),
                active: true,
                explicit: false,
            };
            let stored = state
                .authorities
                .entry(id.to_owned())
                .or_insert(expected.clone());
            if !stored.explicit
                && (stored.principal != principal.to_string()
                    || stored.generation != generation
                    || stored.scope_revocation_hash != hash.as_bytes())
            {
                *stored = expected;
            }
            stored.active
                && stored.principal == principal.to_string()
                && stored.generation == generation
                && stored.scope_revocation_hash == hash.as_bytes()
        }
        #[cfg(not(feature = "conformance-tracer"))]
        {
            let _ = (state, witness);
            true
        }
    }
}

impl RoomCommitStorageV1 for FixturePostgresStore {
    #[allow(clippy::too_many_lines)]
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        let Ok(identity) = prepared.identity().canonical_bytes() else {
            return RoomCommitResolutionV1::Fault;
        };
        let identity_key = identity.clone();
        let Ok(mut state) = self.state.lock() else {
            return RoomCommitResolutionV1::Indeterminate;
        };
        if let Some((existing_hash, Some(receipt))) = state.guards.get(&identity) {
            if existing_hash.as_slice() != prepared.request_hash().as_bytes() {
                let Ok(existing_request_hash) = CanonicalRequestHashV1::from_str(&format!(
                    "blake3:{}",
                    hex_bytes(existing_hash)
                )) else {
                    return RoomCommitResolutionV1::Fault;
                };
                return RoomCommitResolutionV1::Conflict {
                    existing_request_hash,
                };
            }
            return Self::resolution_for_receipt(receipt, ResolutionStatusV1::Existing);
        }
        state.guards.insert(
            identity_key.clone(),
            (prepared.request_hash().as_bytes().to_vec(), None),
        );
        let authority_current = match prepared {
            PreparedRoomWriteV1::Create(create) => {
                Self::authority_is_current(&mut state, create.authority_witness())
            }
            PreparedRoomWriteV1::Existing(existing) => {
                Self::authority_is_current(&mut state, existing.authority_witness())
            }
        };
        if !authority_current {
            state.guards.remove(&identity_key);
            return RoomCommitResolutionV1::Fenced;
        }
        if let Some(failpoint) = state.failpoint.take() {
            if matches!(failpoint, PostgresFailpoint::RollbackBeforeCommit) {
                state.guards.remove(&identity_key);
                return RoomCommitResolutionV1::RetryableKnownAbsent;
            }
            state.failpoint = Some(failpoint);
        }
        let result = match prepared {
            PreparedRoomWriteV1::Create(create) => {
                let persistence = create.persistence();
                let room_id = persistence.complete_head.room_id().to_string();
                if state.rooms.contains_key(&room_id) {
                    RoomCommitResolutionV1::Reprepare
                } else {
                    let mut room = FixtureRoom {
                        head_bytes: persistence.canonical_head_bytes.clone(),
                        integrity_generation: persistence.integrity_generation.get(),
                        ..FixtureRoom::default()
                    };
                    for member in &persistence.memberships {
                        let member_id = member.membership.member_id().to_string();
                        room.frame_heads.insert(member_id.clone(), 0);
                        room.reset_required_through.insert(member_id, 0);
                    }
                    for timer in &persistence.initial_timers {
                        room.timers.insert(
                            (timer.timer_id().to_string(), timer.generation().get()),
                            FixtureTimer {
                                scheduled_for: timer.scheduled_for().to_string(),
                                payload_bytes: timer.canonical_payload_bytes().to_vec(),
                                state: "scheduled",
                            },
                        );
                    }
                    state.rooms.insert(room_id, room);
                    let receipt = create.semantic_result().canonical_receipt_bytes().to_vec();
                    if let Some((_, stored)) = state.guards.get_mut(&identity_key) {
                        *stored = Some(receipt.clone());
                    }
                    Self::resolution_for_receipt(&receipt, ResolutionStatusV1::New)
                }
            }
            PreparedRoomWriteV1::Existing(existing) => {
                let room_id = existing.basis_complete_head().room_id().to_string();
                let Some(room) = state.rooms.get_mut(&room_id) else {
                    state.guards.remove(&identity_key);
                    return RoomCommitResolutionV1::Fault;
                };
                let Ok(basis_bytes) = existing.basis_complete_head().canonical_bytes() else {
                    return RoomCommitResolutionV1::Fault;
                };
                if room.head_bytes != basis_bytes {
                    state.guards.remove(&identity_key);
                    return RoomCommitResolutionV1::Reprepare;
                }
                if room.integrity_generation != existing.integrity_generation().get() {
                    state.guards.remove(&identity_key);
                    return RoomCommitResolutionV1::Fenced;
                }
                if let PreparedExistingIntentV1::Advance(advance) = existing.intent() {
                    if let Err(resolution) = fixture_apply_witnesses(room, existing, advance) {
                        state.guards.remove(&identity_key);
                        return resolution;
                    }
                    room.head_bytes
                        .clone_from(&advance.canonical_resulting_head_bytes);
                }
                let receipt = existing
                    .semantic_result()
                    .canonical_receipt_bytes()
                    .to_vec();
                if let Some((_, stored)) = state.guards.get_mut(&identity_key) {
                    *stored = Some(receipt.clone());
                }
                Self::resolution_for_receipt(&receipt, ResolutionStatusV1::New)
            }
        };
        if matches!(
            state.failpoint.take(),
            Some(PostgresFailpoint::UnknownAfterCommit)
        ) {
            return RoomCommitResolutionV1::Indeterminate;
        }
        if matches!(
            &result,
            RoomCommitResolutionV1::Reprepare
                | RoomCommitResolutionV1::Fenced
                | RoomCommitResolutionV1::Conflict { .. }
                | RoomCommitResolutionV1::Fault
        ) {
            state.guards.remove(&identity_key);
        }
        result
    }

    fn resolve(
        &self,
        identity: &worldstream_core::OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1 {
        let Ok(identity_bytes) = identity.canonical_bytes() else {
            return ResolveOutcomeV1::ResolutionUnavailable;
        };
        let Ok(state) = self.state.lock() else {
            return ResolveOutcomeV1::ResolutionUnavailable;
        };
        let Some((stored_hash, receipt)) = state.guards.get(&identity_bytes) else {
            return ResolveOutcomeV1::KnownAbsent;
        };
        if stored_hash.as_slice() != request_hash.as_bytes() {
            let Ok(existing_request_hash) =
                CanonicalRequestHashV1::from_str(&format!("blake3:{}", hex_bytes(stored_hash)))
            else {
                return ResolveOutcomeV1::ResolutionUnavailable;
            };
            return ResolveOutcomeV1::Conflict {
                existing_request_hash,
            };
        }
        receipt
            .as_ref()
            .map_or(ResolveOutcomeV1::KnownAbsent, |bytes| {
                StoredSemanticResultV1::from_canonical_receipt_bytes(bytes)
                    .map(|value| ResolveOutcomeV1::StoredResolution(Box::new(value)))
                    .unwrap_or(ResolveOutcomeV1::ResolutionUnavailable)
            })
    }
}

#[allow(clippy::too_many_lines)]
fn fixture_apply_witnesses(
    room: &mut FixtureRoom,
    existing: &worldstream_core::PreparedRoomCommitV1,
    advance: &PreparedAdvancePersistenceV1,
) -> Result<(), RoomCommitResolutionV1> {
    if let worldstream_core::PreparedOperationInputWitnessV1::TimerFired(witness) =
        existing.input_witness()
    {
        let worldstream_core::OperationIdentityV1::TimerFired(identity) =
            witness.request.operation_identity()
        else {
            return Err(RoomCommitResolutionV1::Fault);
        };
        let key = (identity.timer_id.to_string(), identity.generation.get());
        let Some(timer) = room.timers.get_mut(&key) else {
            return Err(RoomCommitResolutionV1::NotApplicable);
        };
        if timer.state != "scheduled" {
            return Err(RoomCommitResolutionV1::NotApplicable);
        }
        if timer.scheduled_for != identity.scheduled_for.to_string()
            || timer.payload_bytes != witness.canonical_timer_payload_bytes
        {
            return Err(RoomCommitResolutionV1::Fault);
        }
        timer.state = "fired";
    }

    for mutation in &advance.timer_changes {
        match mutation.kind() {
            worldstream_core::PreparedTimerMutationKindV1::Schedule {
                timer_id,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => {
                let maximum = room
                    .timers
                    .keys()
                    .filter(|(id, _)| id == timer_id.as_str())
                    .map(|(_, generation)| *generation)
                    .max();
                if maximum.map_or(1, |value| value.saturating_add(1)) != generation.get()
                    || room.timers.iter().any(|((id, _), timer)| {
                        id == timer_id.as_str() && timer.state == "scheduled"
                    })
                {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                room.timers.insert(
                    (timer_id.to_string(), generation.get()),
                    FixtureTimer {
                        scheduled_for: scheduled_for.to_string(),
                        payload_bytes: canonical_payload_bytes.to_vec(),
                        state: "scheduled",
                    },
                );
            }
            worldstream_core::PreparedTimerMutationKindV1::Cancel {
                timer_id,
                generation,
            } => {
                let Some(timer) = room
                    .timers
                    .get_mut(&(timer_id.to_string(), generation.get()))
                else {
                    return Err(RoomCommitResolutionV1::Fault);
                };
                if timer.state != "scheduled" {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                timer.state = "cancelled";
            }
            worldstream_core::PreparedTimerMutationKindV1::Reschedule {
                timer_id,
                previous_generation,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => {
                if generation.get() <= previous_generation.get()
                    || room.timers.iter().any(|((id, _), timer)| {
                        id == timer_id.as_str() && timer.state == "scheduled"
                    })
                {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                let Some(previous) = room
                    .timers
                    .get_mut(&(timer_id.to_string(), previous_generation.get()))
                else {
                    return Err(RoomCommitResolutionV1::Fault);
                };
                if previous.state != "scheduled" {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                previous.state = "cancelled";
                room.timers.insert(
                    (timer_id.to_string(), generation.get()),
                    FixtureTimer {
                        scheduled_for: scheduled_for.to_string(),
                        payload_bytes: canonical_payload_bytes.to_vec(),
                        state: "scheduled",
                    },
                );
            }
        }
    }

    for consequence in &advance.delivery_consequences {
        match consequence {
            worldstream_core::PreparedObservationConsequenceV1::ObservationFrame(frame) => {
                let member_id = frame.member_id().to_string();
                let current = room.frame_heads.get(&member_id).copied().unwrap_or(0);
                if current != frame.previous_frame_head() || frame.frame_seq() != current + 1 {
                    return Err(RoomCommitResolutionV1::Fenced);
                }
                room.frame_heads
                    .insert(member_id.clone(), frame.frame_seq());
                room.frames.insert(
                    (member_id, frame.frame_seq()),
                    (
                        frame.canonical_payload_bytes().to_vec(),
                        frame.payload_hash().as_bytes().to_vec(),
                    ),
                );
            }
            worldstream_core::PreparedObservationConsequenceV1::ResetRequired(view) => {
                let member_id = view.viewer().member_id().to_string();
                let frame_head = room.frame_heads.get(&member_id).copied().unwrap_or(0);
                room.reset_required_through
                    .entry(member_id.clone())
                    .and_modify(|marker| *marker = (*marker).max(frame_head))
                    .or_insert(frame_head);
                room.observation_consequences.insert(
                    (member_id, advance.transition.room_seq().get()),
                    (
                        "reset_required".to_owned(),
                        Some(view.canonical_bytes().to_vec()),
                    ),
                );
            }
            worldstream_core::PreparedObservationConsequenceV1::VisibilityLost(member_id) => {
                let member_id_string = member_id.to_string();
                let frame_head = room
                    .frame_heads
                    .get(&member_id_string)
                    .copied()
                    .unwrap_or(0);
                room.reset_required_through
                    .entry(member_id_string)
                    .and_modify(|marker| *marker = (*marker).max(frame_head))
                    .or_insert(frame_head);
                room.observation_consequences.insert(
                    (member_id.to_string(), advance.transition.room_seq().get()),
                    ("visibility_lost".to_owned(), None),
                );
            }
        }
    }

    for decision in &advance.activation_decisions {
        room.activation_decisions.insert(
            (
                advance.transition.room_seq().get(),
                decision.decision_id().to_owned(),
            ),
            (
                decision.target_member_id().map(ToString::to_string),
                decision.canonical_decision_bytes().to_vec(),
            ),
        );
        if let Ok(record) = CanonicalJsonV1::decode_canonical::<
            worldstream_core::ActivationDecisionV1,
        >(decision.canonical_decision_bytes())
            && matches!(
                record.policy.disposition,
                worldstream_core::ActivationPolicyDispositionV1::Intent
            )
            && let Some(activation_id) = record.activation_id
        {
            room.activation_intents
                .insert(activation_id, record.policy.policy_revision);
        }
    }
    Ok(())
}

#[cfg(test)]
mod native_hydration_tests {
    use super::*;

    #[test]
    fn transfer_safety_catalog_contract_matches_the_reviewed_migration() {
        let transfer_fence_migrations = format!("{MIGRATION_0011_SQL}{MIGRATION_0012_SQL}");
        for (index, table) in GLOBAL_RESOURCE_IDENTITY_INDEXES {
            assert!(
                MIGRATION_0011_SQL.contains(&format!("CREATE UNIQUE INDEX {index}")),
                "missing reviewed unique index {index}"
            );
            assert!(
                MIGRATION_0011_SQL.contains(&format!("ON {table}(resource_identity)")),
                "unique index {index} no longer closes {table}"
            );
        }
        for (trigger, table) in TRANSFER_FENCE_TRIGGER_TABLES {
            assert!(
                transfer_fence_migrations.contains(&format!("CREATE TRIGGER {trigger}")),
                "missing reviewed transfer-fence trigger {trigger}"
            );
            assert!(
                transfer_fence_migrations.contains(&format!(
                    "BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table}"
                )),
                "transfer-fence trigger {trigger} no longer protects {table}"
            );
        }
        assert!(MIGRATION_0011_SQL.contains(TRANSFER_FENCE_FUNCTION_BODY));
    }

    #[test]
    fn catalog_probe_order_matches_the_frozen_fingerprint_material() {
        let material_order = SCHEMA_FINGERPRINT_MATERIAL
            .split(");")
            .filter_map(|table| table.split_once('(').map(|(name, _)| name))
            .collect::<Vec<_>>();

        assert_eq!(material_order, SCHEMA_TABLE_ORDER);
    }

    #[test]
    fn deployment_metadata_status_is_explicitly_absent_without_identity_rows() {
        let status = PostgresDeploymentMetadataStatus {
            table_present: false,
            deployment_lineage_bytes: None,
            storage_epoch_bytes: None,
            storage_epoch: None,
        };
        assert!(!status.table_present);
        assert!(status.deployment_lineage_bytes.is_none());
        assert!(status.storage_epoch_bytes.is_none());
        assert!(
            PostgresDeploymentMetadataError::MissingMetadata
                .to_string()
                .contains("absent")
        );
    }

    #[test]
    fn native_evidence_keeps_exact_bytes_and_ledger_order() {
        let timer = PostgresTimerEvidenceV1 {
            timer_id: "timer-1".to_owned(),
            generation: 1,
            scheduled_for: "t0".to_owned(),
            payload_bytes: vec![11],
            state: "scheduled".to_owned(),
        };
        let frame = PostgresFrameEvidenceV1 {
            member_id: "member-1".to_owned(),
            frame_seq: 1,
            cause_room_seq: 1,
            payload_bytes: vec![12],
            payload_hash: vec![13],
        };
        let decision = PostgresActivationDecisionEvidenceV1 {
            cause_room_seq: 1,
            decision_id: "decision-1".to_owned(),
            target_member_id: None,
            decision_bytes: vec![14],
        };
        assert_eq!(timer.payload_bytes, vec![11]);
        assert_eq!(frame.payload_hash, vec![13]);
        assert_eq!(decision.decision_bytes, vec![14]);
    }

    #[test]
    fn executable_replay_rejects_missing_frame_and_consequence_witnesses() {
        let empty_replay = ReplayStorageVerificationV1::from_observation_witnesses_for_conformance(
            Vec::new(),
            Vec::new(),
        );
        let positions = vec![PostgresObservationPositionEvidenceV1 {
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            frame_head: 1,
            retained_frame_floor: 1,
            last_ack_frame_seq: None,
            reset_required_through: None,
        }];
        assert_eq!(
            verify_replayed_frame_evidence(&empty_replay, &positions, &[]),
            Err(RoomRecoveryErrorV1::Corrupt)
        );

        let consequences = vec![PostgresObservationConsequenceEvidenceV1::VisibilityLost {
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            cause_room_seq: 1,
        }];
        assert_eq!(
            verify_replayed_nonframe_consequences(&empty_replay, &consequences),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
    }

    #[test]
    fn executable_replay_rejects_membership_timer_decision_and_position_substitution() {
        let empty_replay = ReplayStorageVerificationV1::from_observation_witnesses_for_conformance(
            Vec::new(),
            Vec::new(),
        );
        let memberships = vec![(
            "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            br#"{"member":"substituted"}"#.to_vec(),
        )];
        assert_eq!(
            verify_replayed_membership_evidence(&empty_replay, &memberships),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
        let timers = vec![PostgresTimerEvidenceV1 {
            timer_id: "timer-substituted".to_owned(),
            generation: 1,
            scheduled_for: "2026-08-22T00:00:00Z".to_owned(),
            payload_bytes: b"null".to_vec(),
            state: "scheduled".to_owned(),
        }];
        assert_eq!(
            verify_replayed_timer_evidence(&empty_replay, &timers),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
        let decisions = vec![PostgresActivationDecisionEvidenceV1 {
            cause_room_seq: 1,
            decision_id: "decision-substituted".to_owned(),
            target_member_id: None,
            decision_bytes: b"null".to_vec(),
        }];
        assert_eq!(
            verify_replayed_activation_decision_evidence(&empty_replay, &decisions),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
        let positions = vec![PostgresObservationPositionEvidenceV1 {
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            frame_head: 0,
            retained_frame_floor: 1,
            last_ack_frame_seq: None,
            reset_required_through: Some(0),
        }];
        assert_eq!(
            verify_replayed_position_evidence(&empty_replay, &positions),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
    }

    #[test]
    fn replay_position_admission_allows_later_frames_and_acknowledged_resets() {
        let prune_then_new_frame = PostgresObservationPositionEvidenceV1 {
            member_id: "member-a".to_owned(),
            frame_head: 6,
            retained_frame_floor: 4,
            last_ack_frame_seq: Some(2),
            reset_required_through: Some(5),
        };
        assert!(observation_position_relation_is_valid(
            6,
            None,
            &prune_then_new_frame
        ));

        let acknowledged_reset = PostgresObservationPositionEvidenceV1 {
            member_id: "member-a".to_owned(),
            frame_head: 6,
            retained_frame_floor: 1,
            last_ack_frame_seq: Some(5),
            reset_required_through: None,
        };
        assert!(observation_position_relation_is_valid(
            6,
            Some(5),
            &acknowledged_reset
        ));

        let insufficient_marker = PostgresObservationPositionEvidenceV1 {
            reset_required_through: Some(2),
            ..prune_then_new_frame
        };
        assert!(!observation_position_relation_is_valid(
            6,
            None,
            &insufficient_marker
        ));
    }

    #[test]
    fn native_room_provider_inventory_is_prefix_length_and_limit_bounded() {
        assert_eq!(NATIVE_ROOM_PROVIDER_READ_QUERIES_V1.len(), 11);
        assert!(NATIVE_ROOM_PROVIDER_READ_QUERIES_V1.iter().all(|query| {
            query.contains("jsonb_build_array")
                && query.contains("ORDER BY")
                && !query.contains("SELECT *")
        }));
        let sql = bounded_provider_projection_query(NATIVE_ROOM_PROVIDER_READ_QUERIES_V1[0], 2, 3);
        assert!(sql.contains("LIMIT $2"));
        assert!(sql.contains("FROM 1 FOR $3"));
        assert!(sql.contains("octet_length(convert_to(projected.canonical_row, 'UTF8'))::bigint"));
    }

    #[test]
    fn native_schema_admission_preflights_every_variable_catalog() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub(crate) fn verify_runtime_schema_for_native_restore")
            .unwrap_or_else(|| unreachable!("native schema admission must exist"));
        let end = source[start..]
            .find("// Each bit is an independently queried PostgreSQL privilege")
            .map(|offset| start + offset)
            .unwrap_or_else(|| unreachable!("native schema admission boundary must exist"));
        let body = &source[start..end];

        assert_eq!(
            body.matches("preflight_native_global_provider_reads(")
                .count(),
            5
        );
        for required_projection in [
            "worldstream_schema_migrations",
            "information_schema.columns",
            "pg_catalog.pg_index",
            "pg_catalog.pg_trigger",
            "routine.prosrc",
        ] {
            assert!(body.contains(required_projection));
        }
        assert!(body.contains("verify_runtime_schema(client)"));
        assert!(frozen_schema_column_count() > SCHEMA_TABLE_ORDER.len());
    }

    #[test]
    fn deployment_metadata_validation_is_nonempty_and_nonnegative() {
        assert!(validate_deployment_metadata(&[1, 2], &[3, 4], 7).is_ok());
        assert!(matches!(
            validate_deployment_metadata(&[], &[3], 7),
            Err(PostgresDeploymentMetadataError::MalformedMetadata)
        ));
        assert!(matches!(
            validate_deployment_metadata(&[1], &[3], -1),
            Err(PostgresDeploymentMetadataError::MalformedMetadata)
        ));
    }

    #[test]
    fn server_version_admission_enforces_major_and_minimum_patch() {
        assert!(parse_server_version_num("170011").is_ok());
        assert!(parse_server_version_num("170012").is_ok());
        assert!(matches!(
            parse_server_version_num("170010"),
            Err(PostgresSchemaVerificationError::UnsupportedPatch {
                found,
                minimum: POSTGRES_MINIMUM_PATCH,
            }) if found == "170010"
        ));
        assert!(matches!(
            parse_server_version_num("160011"),
            Err(PostgresSchemaVerificationError::UnsupportedMajor { found }) if found == "160011"
        ));
        assert!(matches!(
            parse_server_version_num("not-a-version"),
            Err(PostgresSchemaVerificationError::InvalidServerVersion { found }) if found == "not-a-version"
        ));
    }

    #[test]
    fn dsn_tls_admission_uses_effective_parsed_configuration() {
        for profile in [
            PostgresProfile::DirectAdmin,
            PostgresProfile::RuntimeLeastPrivilege,
        ] {
            assert!(matches!(
                validate_dsn(
                    "host=db.example user=runtime password=sslmode=require".to_owned(),
                    profile,
                ),
                Err(PostgresConfigError::RemoteTlsRequired)
            ));
            assert!(matches!(
                validate_dsn(
                    "host=db.example user=runtime sslmode=prefer password=secret".to_owned(),
                    profile,
                ),
                Err(PostgresConfigError::RemoteTlsRequired)
            ));
            assert!(matches!(
                validate_dsn(
                    "host=db.example user=runtime sslmode=requirement".to_owned(),
                    profile,
                ),
                Err(PostgresConfigError::InvalidDsn)
            ));
            assert!(
                validate_dsn(
                    "host=db.example user=runtime sslmode=require".to_owned(),
                    profile,
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn dsn_tls_admission_limits_plaintext_exception_to_exact_local_endpoints() {
        for dsn in [
            "host=127.0.0.1 sslmode=disable",
            "host=127.20.30.40 sslmode=disable",
            "host=::1 sslmode=disable",
            "host=/var/run/postgresql sslmode=disable",
        ] {
            assert!(
                validate_dsn(dsn.to_owned(), PostgresProfile::RuntimeLeastPrivilege).is_ok(),
                "exact local endpoint must remain available: {dsn}"
            );
        }
        for dsn in [
            "host=localhost sslmode=disable",
            "postgresql://runtime@localhost/worldstream?sslmode=disable",
            "host=localhost.example sslmode=disable",
            "host=127.0.0.1.example sslmode=disable",
            "host=localhost,db.example sslmode=disable",
            "host=localhost hostaddr=203.0.113.8 sslmode=disable",
        ] {
            assert!(matches!(
                validate_dsn(dsn.to_owned(), PostgresProfile::RuntimeLeastPrivilege),
                Err(PostgresConfigError::RemoteTlsRequired)
            ));
        }
    }

    #[test]
    fn runtime_role_admission_rejects_each_privilege_escalation() {
        assert!(validate_runtime_role_admission(RuntimeRoleAdmissionV1::default()).is_ok());
        for admission in [
            RuntimeRoleAdmissionV1 {
                superuser: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                create_role: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                create_database: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                replication: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                bypass_row_security: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                database_create: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                other_role_membership: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                public_schema_create: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                owns_public_schema_object: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                migration_insert: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                migration_update: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                migration_delete: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                migration_truncate: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                transfer_control_insert: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                transfer_control_update: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                transfer_control_delete: true,
                ..RuntimeRoleAdmissionV1::default()
            },
            RuntimeRoleAdmissionV1 {
                transfer_control_truncate: true,
                ..RuntimeRoleAdmissionV1::default()
            },
        ] {
            assert!(matches!(
                validate_runtime_role_admission(admission),
                Err(PostgresSchemaVerificationError::RuntimeRolePrivileges)
            ));
        }
    }

    #[test]
    fn runtime_admission_rejects_disposable_native_restore_marker() {
        assert!(validate_runtime_database_marker(None).is_ok());
        assert!(validate_runtime_database_marker(Some("production")).is_ok());
        assert!(matches!(
            validate_runtime_database_marker(Some(
                native_restore::NATIVE_POSTGRES_DISPOSABLE_TARGET_MARKER_V1
            )),
            Err(PostgresSchemaVerificationError::DisposableRestoreTarget)
        ));
    }

    #[test]
    fn provider_authority_clock_preserves_subsecond_order_in_canonical_form() {
        let issued =
            postgres_authority_checked_at_from_provider_text("2026-08-22T04:32:02.586060Z")
                .unwrap_or_else(|error| unreachable!("issued provider time: {error:?}"));
        let revalidated =
            postgres_authority_checked_at_from_provider_text("2026-08-22T04:32:02.586061Z")
                .unwrap_or_else(|error| unreachable!("revalidation provider time: {error:?}"));
        let whole_second =
            postgres_authority_checked_at_from_provider_text("2026-08-22T04:32:03.000000Z")
                .unwrap_or_else(|error| unreachable!("whole-second provider time: {error:?}"));
        let trailing_zero_fraction =
            postgres_authority_checked_at_from_provider_text("2026-08-22T04:32:03.090000Z")
                .unwrap_or_else(|error| unreachable!("trailing-zero provider time: {error:?}"));

        assert_eq!(issued.as_str(), "2026-08-22T04:32:02.58606Z");
        assert_eq!(revalidated.as_str(), "2026-08-22T04:32:02.586061Z");
        assert_eq!(whole_second.as_str(), "2026-08-22T04:32:03Z");
        assert_eq!(trailing_zero_fraction.as_str(), "2026-08-22T04:32:03.09Z");
        assert!(HostClockSampleV1::new(trailing_zero_fraction.as_str()).is_ok());
        for malformed in [
            "2026-08-22T04:32:02Z",
            "2026-08-22T04:32:02.12345Z",
            "2026-08-22T04:32:02.12x456Z",
        ] {
            assert!(matches!(
                postgres_authority_checked_at_from_provider_text(malformed),
                Err(AuthorityStoreErrorV1::Corrupt)
            ));
        }
    }

    #[test]
    fn engine_identity_format_is_deterministic() {
        assert_eq!(
            format_postgres_engine_identity(170_011),
            "postgresql/17.11; server_version_num=170011"
        );
        let identity = PostgresEngineIdentityV1 {
            server_version_num: 170_011,
            formatted: format_postgres_engine_identity(170_011),
        };
        assert_eq!(identity.server_version_num(), 170_011);
        assert_eq!(
            identity.formatted(),
            "postgresql/17.11; server_version_num=170011"
        );
    }

    #[test]
    fn engine_identity_provider_failures_are_redacted() {
        let config = PostgresConnectionConfig::runtime(
            "host=127.0.0.1 port=1 user=worldstream password=secret dbname=worldstream connect_timeout=1",
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| unreachable!("runtime config: {error}"));
        let store = PostgresRoomStore::new(config)
            .unwrap_or_else(|error| unreachable!("runtime store: {error}"));
        let error = store
            .engine_identity()
            .err()
            .unwrap_or_else(|| unreachable!("unreachable provider unexpectedly connected"));
        assert_eq!(error, PostgresEngineIdentityError::Unavailable);
        let debug = format!("{error:?}");
        assert!(!debug.contains("password"));
        assert!(!debug.contains("port=1"));
    }

    #[test]
    fn lease_reclaim_errors_have_no_provider_payload() {
        let debug = format!("{:?}", PostgresActivationLeaseReclaimError::Unavailable);
        assert!(!debug.contains("password"));
        assert!(!debug.contains("dsn"));
        assert!(!debug.contains("postgres"));
    }

    #[test]
    fn live_runtime_scheduler_reuses_the_verified_provider_connection_profile() {
        let Ok(dsn) = std::env::var("WORLDSTREAM_POSTGRES_TEST_DSN") else {
            return;
        };
        let store = PostgresRoomStore::new(
            PostgresConnectionConfig::runtime(dsn, PostgresConnectionPath::Direct)
                .unwrap_or_else(|error| unreachable!("live runtime config: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("live runtime store: {error}"));
        store
            .verify_schema()
            .unwrap_or_else(|error| unreachable!("live schema: {error}"));
        for _ in 0..1 {
            store
                .engine_identity()
                .unwrap_or_else(|error| unreachable!("live identity: {error}"));
        }
        store
            .reclaim_expired_activation_leases()
            .unwrap_or_else(|error| {
                unreachable!(
                    "live Activation scheduler: {error:?}; classified={:?}",
                    store.last_failure()
                )
            });
    }

    #[test]
    fn live_external_input_preparation_survives_restart_and_reuses_first_time() {
        let Ok(dsn) = std::env::var("WORLDSTREAM_POSTGRES_TEST_DSN") else {
            return;
        };
        let runtime = || {
            PostgresRoomStore::new(
                PostgresConnectionConfig::runtime(dsn.clone(), PostgresConnectionPath::Direct)
                    .unwrap_or_else(|error| unreachable!("live runtime config: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("live runtime store: {error}"))
        };
        let identity = OperationIdentityV1::ExternalInput(Box::new(
            worldstream_core::ExternalInputOperationIdentityV1 {
                room_id: "01ARZ3NDEKTSV4RRFFQ69G5FZ0"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("Room ID: {error}")),
                source_id: "worldstream.host.lobby"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("Source ID: {error}")),
                input_id: "01ARZ3NDEKTSV4RRFFQ69G5FZ1"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("Input ID: {error}")),
            },
        ));
        let request_hash = CanonicalRequestHashV1::from_str(&format!("blake3:{}", "11".repeat(32)))
            .unwrap_or_else(|error| unreachable!("request hash: {error}"));
        let conflicting_hash =
            CanonicalRequestHashV1::from_str(&format!("blake3:{}", "22".repeat(32)))
                .unwrap_or_else(|error| unreachable!("conflicting hash: {error}"));
        let first_sample = "2026-08-24T12:00:00Z"
            .parse::<worldstream_core::ExternalInputRecordedAt>()
            .unwrap_or_else(|error| unreachable!("first sample: {error}"));
        let later_sample = "2026-08-24T12:05:00Z"
            .parse::<worldstream_core::ExternalInputRecordedAt>()
            .unwrap_or_else(|error| unreachable!("later sample: {error}"));

        let first = runtime();
        first
            .verify_schema()
            .unwrap_or_else(|error| unreachable!("live schema: {error}"));
        assert_eq!(
            first.reserve_external_input_recorded_at(&identity, &request_hash, &first_sample,),
            Ok(first_sample.clone())
        );
        drop(first);

        let restarted = runtime();
        assert_eq!(
            restarted.reserve_external_input_recorded_at(&identity, &request_hash, &later_sample,),
            Ok(first_sample)
        );
        assert_eq!(
            restarted.reserve_external_input_recorded_at(
                &identity,
                &conflicting_hash,
                &later_sample,
            ),
            Err(PostgresExternalInputPreparationErrorV1::Conflict)
        );
    }

    #[test]
    fn continued_sql_literals_preserve_token_boundaries_in_provider_modules() {
        for source in [include_str!("lib.rs"), include_str!("authority.rs")] {
            let mut continuation_count = 0;
            for line in source.lines() {
                let trimmed = line.trim_end();
                if trimmed.ends_with('\\') {
                    continuation_count += 1;
                    assert!(
                        trimmed
                            .as_bytes()
                            .get(trimmed.len().saturating_sub(2))
                            .is_some_and(u8::is_ascii_whitespace),
                        "SQL continuation must leave whitespace before its backslash: {trimmed}"
                    );
                }
            }
            assert!(continuation_count > 0);
        }
        assert!(include_str!("lib.rs").contains("activation_id \\\n"));
        assert!(include_str!("authority.rs").contains("worldstream_authority_state \\\n"));
    }
}
