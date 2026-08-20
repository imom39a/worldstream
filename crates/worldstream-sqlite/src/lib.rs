#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::needless_raw_string_hashes,
        clippy::panic,
        clippy::too_many_lines,
        clippy::unwrap_used
    )
)]

//! Bundled `SQLite` adapter for the core-owned prepared Room Commit seam.
//!
//! One private thread owns the sole writer connection for each database path.
//! Callers submit already-prepared semantic bundles through a bounded command
//! queue; SQL, transaction ordering, identity guards, and durable receipt
//! decoding remain private to this adapter.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(not(test))]
use std::time::SystemTime;

#[cfg(test)]
use std::sync::Condvar;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, ffi::ErrorCode,
    params,
};
use thiserror::Error;
#[cfg(not(test))]
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use worldstream_core::{
    AccessModeV1, AuthorityBootstrapStateV1, AuthorityChangeReceiptV1, AuthorityChangeResultV1,
    AuthorityChangeStatePartsV1, AuthorityChangeStateV1, AuthorityChangeTargetV1,
    AuthorityChangeV1, AuthorityCheckedAt, AuthorityErrorV1, AuthorityGenerationV1,
    AuthorityReasonCodeV1, AuthoritySnapshotQueryV1, AuthoritySnapshotV1, AuthorityStoreErrorV1,
    AuthorityStoreV1, AuthorizedReceiptReadV1, AuthorizedReceiptResolverV1, AuthorizedReplayV1,
    Blake3DigestV1, CanonicalRequestHashV1, CapabilityAuthoritySnapshotPartsV1,
    CapabilityAuthoritySnapshotV1, CapabilityExpiresAt, CapabilityId, CapabilityProfileV1,
    CapabilityRevokedAt, CapabilityScopeSetV1, CapabilityScopeV1, CapabilityTokenHashV1,
    CompleteHeadV1, CoreRoomStateV1, CoreTraceV1, GenesisV1, HistoricalReplayAccumulatorV1,
    HistoricalReplayErrorV1, HistoricalReplayProjectionRequestV1, HistoricalReplayProjectionV1,
    IntegrityGenerationV1, MemberId, MembershipAuthoritySnapshotV1, MembershipGenerationV1,
    MembershipStandingV1, MembershipV1, OperationIdentityV1, PackRegistryV1, PackRevisionLockV1,
    PreparedAdvancePersistenceV1, PreparedAuthorityBootstrapV1, PreparedAuthorityChangeV1,
    PreparedAuthorityWitnessV1, PreparedCreationPersistenceV1, PreparedExistingIntentV1,
    PreparedOperationInputWitnessV1, PreparedRoomWriteV1, PreparedTimerMutationKindV1,
    PrincipalAuthoritySnapshotV1, PrincipalAuthorityStatusV1, PrincipalGenerationV1, PrincipalId,
    PrincipalKindV1, ReceiptSemanticInputV1, RecordedStimulusV1, RecoveredRoomMaterializationsV1,
    RecoveredTimerStateV1, RecoveryIntegrityDispositionV1, ReplayAdapterInputV1,
    ReplayFailureClassV1, ReplayProjectionKindV1, ResolutionStatusV1, ResolveOutcomeV1,
    RoomCommitResolutionV1, RoomCommitStorageV1, RoomId, RoomIntegrityStateV1,
    RoomIntegrityStatusV1, RoomRecoveryCandidateV1, RoomRecoveryErrorV1, RoomRecoveryStorageV1,
    RoomSequenceV1, RoomStatusV1, RunnerAuthoritySnapshotV1, RunnerAuthorityStatusV1,
    RunnerGenerationV1, RunnerId, RunnerMembershipSetV1, SemanticResultV1, StoredSemanticResultV1,
    TransitionV1, ValidatedAuthorityBootstrapV1, ValidatedAuthorityChangeV1,
    VerifiedCurrentRoomMaterializationV1, recover_room_from_storage,
    resolve_authorized_room_operation_for_adapter,
};

/// Frozen `SQLite` engine selected by the authored compatibility manifest.
pub const SQLITE_VERSION: &str = "3.53.4";
/// Official source identity carried by the pinned bundled amalgamation.
pub const SQLITE_SOURCE_ID: &str =
    "2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc";
/// Exact upstream source revision supplying that amalgamation.
pub const RUSQLITE_BUNDLE_REVISION: &str = "229140734a4a60cc9fa34507fe79cb2277142f49";

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const INITIAL_MIGRATION_ID: &str = "0001-initial-storage-schema";
const AUTHORITY_MIGRATION_ID: &str = "0002-operational-authority-v1";
const OPERATION_RECEIPT_CODEC_ID: &str = "worldstream/operation-receipt/v1";
const WRITER_QUEUE_CAPACITY: usize = 32;
const REPLAY_PAGE_ROWS: usize = 64;
const REPLAY_MAX_ROWS_PER_SLICE: usize = 1_024;
const REPLAY_MAX_CAPTURE_DURATION: Duration = Duration::from_secs(5);
const MAX_IN_FLIGHT_REPLAY_SLICES: usize = 2;
const MAX_DEFERRED_REPLAY_SESSIONS: usize = 8;
const REPLAY_SESSION_IDLE_EXPIRY: Duration = Duration::from_secs(30);

static NEXT_REPLAY_SESSION_ID: AtomicU64 = AtomicU64::new(1);

const AUTHORITY_MIGRATION_SCHEMA: &str = r"
ALTER TABLE authority_fences RENAME TO retired_authority_fences_v1;
CREATE TRIGGER retired_authority_fences_v1_immutable_update
BEFORE UPDATE ON retired_authority_fences_v1 BEGIN
    SELECT RAISE(ABORT, 'retired authority fence is immutable');
END;
CREATE TRIGGER retired_authority_fences_v1_immutable_delete
BEFORE DELETE ON retired_authority_fences_v1 BEGIN
    SELECT RAISE(ABORT, 'retired authority fence is immutable');
END;
CREATE TABLE principals (
    principal_id TEXT PRIMARY KEY,
    principal_kind TEXT NOT NULL CHECK (principal_kind IN ('human', 'agent')),
    authority_status TEXT NOT NULL CHECK (authority_status IN ('enabled', 'disabled')),
    principal_generation INTEGER NOT NULL
        CHECK (principal_generation BETWEEN 1 AND 9007199254740991)
) STRICT;
CREATE TRIGGER principal_identity_immutable
BEFORE UPDATE OF principal_id, principal_kind ON principals BEGIN
    SELECT RAISE(ABORT, 'Principal identity is immutable');
END;
CREATE TABLE runners (
    runner_id TEXT PRIMARY KEY,
    owner_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    authority_status TEXT NOT NULL CHECK (authority_status IN ('enabled', 'revoked')),
    runner_generation INTEGER NOT NULL
        CHECK (runner_generation BETWEEN 1 AND 9007199254740991)
) STRICT;
CREATE TRIGGER runner_identity_immutable
BEFORE UPDATE OF runner_id, owner_principal_id ON runners BEGIN
    SELECT RAISE(ABORT, 'Runner identity is immutable');
END;
CREATE TABLE capabilities (
    capability_id TEXT PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    profile_kind TEXT NOT NULL
        CHECK (profile_kind IN ('room_member', 'host_operator', 'runner_control')),
    target_room_id TEXT,
    target_member_id TEXT,
    runner_id TEXT REFERENCES runners(runner_id),
    authority_generation INTEGER NOT NULL
        CHECK (authority_generation BETWEEN 1 AND 9007199254740991),
    expires_at TEXT,
    revoked_at TEXT,
    CHECK (
        (profile_kind = 'room_member'
            AND target_room_id IS NOT NULL
            AND target_member_id IS NOT NULL
            AND runner_id IS NULL)
        OR (profile_kind = 'host_operator'
            AND target_member_id IS NULL
            AND runner_id IS NULL)
        OR (profile_kind = 'runner_control'
            AND target_room_id IS NULL
            AND target_member_id IS NULL
            AND runner_id IS NOT NULL)
    )
) STRICT;
CREATE INDEX capabilities_by_principal ON capabilities(principal_id);
CREATE INDEX capabilities_by_runner ON capabilities(runner_id)
WHERE runner_id IS NOT NULL;
CREATE TRIGGER capability_identity_immutable
BEFORE UPDATE OF capability_id, token_hash, principal_id, profile_kind,
    target_room_id, target_member_id, runner_id ON capabilities BEGIN
    SELECT RAISE(ABORT, 'Capability identity and profile are immutable');
END;
CREATE TABLE capability_scopes (
    capability_id TEXT NOT NULL REFERENCES capabilities(capability_id) ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK (scope IN (
        'room:attach', 'room:act', 'room:observe_public', 'room:observe_member',
        'room:replay', 'activation:offer_receive', 'activation:claim',
        'activation:complete', 'operator:room_admin', 'operator:backup'
    )),
    PRIMARY KEY (capability_id, scope)
) STRICT;
CREATE TABLE runner_capability_memberships (
    capability_id TEXT NOT NULL REFERENCES capabilities(capability_id) ON DELETE CASCADE,
    room_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    PRIMARY KEY (capability_id, room_id, member_id)
) STRICT;
CREATE TABLE authority_change_receipts (
    change_id TEXT PRIMARY KEY,
    authenticated_principal TEXT REFERENCES principals(principal_id),
    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
    result_kind TEXT NOT NULL CHECK (result_kind IN (
        'authority_bootstrapped', 'principal_created', 'capability_registered',
        'capability_narrowed', 'capability_revoked', 'principal_status_changed',
        'runner_registered', 'runner_revoked'
    )),
    target_kind TEXT NOT NULL CHECK (
        target_kind IN ('bootstrap', 'principal', 'capability', 'runner')
    ),
    target_id TEXT NOT NULL,
    secondary_target_id TEXT,
    resulting_generation INTEGER NOT NULL
        CHECK (resulting_generation BETWEEN 1 AND 9007199254740991),
    checked_at TEXT NOT NULL,
    CHECK (
        (result_kind = 'authority_bootstrapped'
            AND authenticated_principal IS NULL
            AND target_kind = 'bootstrap'
            AND secondary_target_id IS NOT NULL
            AND resulting_generation = 1)
        OR (result_kind != 'authority_bootstrapped'
            AND authenticated_principal IS NOT NULL
            AND target_kind != 'bootstrap'
            AND secondary_target_id IS NULL)
    )
) STRICT;
CREATE TRIGGER authority_change_receipts_immutable_update
BEFORE UPDATE ON authority_change_receipts BEGIN
    SELECT RAISE(ABORT, 'authority change Receipt is immutable');
END;
CREATE TRIGGER authority_change_receipts_immutable_delete
BEFORE DELETE ON authority_change_receipts BEGIN
    SELECT RAISE(ABORT, 'authority change Receipt is immutable');
END;
CREATE TABLE authority_audit (
    audit_seq INTEGER PRIMARY KEY,
    change_id TEXT NOT NULL UNIQUE
        REFERENCES authority_change_receipts(change_id),
    actor_principal_id TEXT REFERENCES principals(principal_id),
    target_kind TEXT NOT NULL CHECK (
        target_kind IN ('bootstrap', 'principal', 'capability', 'runner')
    ),
    target_id TEXT NOT NULL,
    secondary_target_id TEXT,
    change_kind TEXT NOT NULL CHECK (change_kind IN (
        'bootstrap_authority', 'create_principal', 'register_capability',
        'register_runner', 'narrow_capability', 'revoke_capability',
        'revoke_runner', 'set_principal_status'
    )),
    prior_generation INTEGER
        CHECK (prior_generation BETWEEN 1 AND 9007199254740991),
    resulting_generation INTEGER NOT NULL
        CHECK (resulting_generation BETWEEN 1 AND 9007199254740991),
    checked_at TEXT NOT NULL,
    reason_code TEXT,
    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
    CHECK (
        (prior_generation IS NULL AND resulting_generation = 1)
        OR resulting_generation = prior_generation + 1
    ),
    CHECK (
        (change_kind = 'bootstrap_authority'
            AND actor_principal_id IS NULL
            AND target_kind = 'bootstrap'
            AND secondary_target_id IS NOT NULL)
        OR (change_kind != 'bootstrap_authority'
            AND actor_principal_id IS NOT NULL
            AND target_kind != 'bootstrap'
            AND secondary_target_id IS NULL)
    )
) STRICT;
CREATE TRIGGER authority_audit_immutable_update
BEFORE UPDATE ON authority_audit BEGIN
    SELECT RAISE(ABORT, 'authority audit is immutable');
END;
CREATE TRIGGER authority_audit_immutable_delete
BEFORE DELETE ON authority_audit BEGIN
    SELECT RAISE(ABORT, 'authority audit is immutable');
END;
CREATE UNIQUE INDEX semantic_receipts_by_transition
ON semantic_receipts(room_id, transition_seq)
WHERE transition_seq IS NOT NULL;
CREATE UNIQUE INDEX semantic_receipts_one_genesis_per_room
ON semantic_receipts(room_id)
WHERE resolution_kind = 'genesis_created';
ALTER TABLE room_members ADD COLUMN membership_generation INTEGER NOT NULL DEFAULT 1
    CHECK (membership_generation BETWEEN 1 AND 9007199254740991);
";

const INITIAL_MIGRATION_SCHEMA: &str = r"
CREATE TABLE authority_fences (
    witness_id TEXT PRIMARY KEY,
    authenticated_principal TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991),
    scope_revocation_bytes BLOB NOT NULL,
    scope_revocation_hash BLOB NOT NULL CHECK (length(scope_revocation_hash) = 32),
    active INTEGER NOT NULL CHECK (active IN (0, 1))
) STRICT;
CREATE TABLE rooms (
    room_id TEXT PRIMARY KEY,
    room_status TEXT NOT NULL CHECK (room_status IN ('active', 'archived')),
    room_seq INTEGER NOT NULL CHECK (room_seq BETWEEN 0 AND 9007199254740991),
    genesis_or_transition_hash TEXT NOT NULL,
    core_schema_version TEXT NOT NULL,
    pack_digest TEXT NOT NULL,
    core_state_hash TEXT NOT NULL,
    activity_state_hash TEXT NOT NULL,
    authoritative_state_hash TEXT NOT NULL,
    complete_head_bytes BLOB NOT NULL
) STRICT;
CREATE TABLE room_genesis (
    room_id TEXT PRIMARY KEY REFERENCES rooms(room_id) ON DELETE CASCADE,
    pack_revision_lock_bytes BLOB NOT NULL,
    genesis_bytes BLOB NOT NULL
) STRICT;
CREATE TRIGGER room_genesis_immutable_update
BEFORE UPDATE ON room_genesis BEGIN
    SELECT RAISE(ABORT, 'room Genesis is immutable');
END;
CREATE TRIGGER room_genesis_immutable_delete
BEFORE DELETE ON room_genesis BEGIN
    SELECT RAISE(ABORT, 'room Genesis is immutable');
END;
CREATE TABLE room_materializations (
    room_id TEXT PRIMARY KEY REFERENCES rooms(room_id) ON DELETE CASCADE,
    core_state_bytes BLOB NOT NULL,
    activity_state_bytes BLOB NOT NULL
) STRICT;
CREATE TABLE room_members (
    room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
    member_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    principal_kind TEXT NOT NULL CHECK (principal_kind IN ('human', 'agent')),
    standing TEXT NOT NULL CHECK (standing IN ('enabled', 'suspended', 'departed')),
    access_mode TEXT NOT NULL CHECK (access_mode IN ('participant', 'spectator', 'operator')),
    role TEXT,
    membership_bytes BLOB NOT NULL,
    frame_head INTEGER NOT NULL DEFAULT 0 CHECK (frame_head BETWEEN 0 AND 9007199254740991),
    CHECK (
        (access_mode = 'participant' AND role IS NOT NULL)
        OR (access_mode IN ('spectator', 'operator') AND role IS NULL)
    ),
    PRIMARY KEY (room_id, member_id)
) STRICT;
CREATE UNIQUE INDEX one_live_membership_per_principal
ON room_members(room_id, principal_id) WHERE standing != 'departed';
CREATE TABLE room_integrity (
    room_id TEXT PRIMARY KEY REFERENCES rooms(room_id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('healthy', 'faulted', 'quarantined')),
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991)
) STRICT;
CREATE TABLE transitions (
    room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
    transition_id TEXT NOT NULL UNIQUE,
    room_seq INTEGER NOT NULL CHECK (room_seq BETWEEN 1 AND 9007199254740991),
    transition_hash TEXT NOT NULL,
    previous_lineage_hash TEXT NOT NULL,
    core_schema_version TEXT NOT NULL,
    pack_digest TEXT NOT NULL,
    core_state_hash TEXT NOT NULL,
    activity_state_hash TEXT NOT NULL,
    authoritative_state_hash TEXT NOT NULL,
    transition_bytes BLOB NOT NULL,
    PRIMARY KEY (room_id, room_seq)
) STRICT;
CREATE TRIGGER transitions_immutable_update
BEFORE UPDATE ON transitions BEGIN
    SELECT RAISE(ABORT, 'Room Transition is immutable');
END;
CREATE TRIGGER transitions_immutable_delete
BEFORE DELETE ON transitions BEGIN
    SELECT RAISE(ABORT, 'Room Transition is immutable');
END;
CREATE TABLE timers (
    room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
    timer_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 9007199254740991),
    scheduled_for TEXT NOT NULL,
    payload_bytes BLOB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('scheduled', 'cancelled', 'fired')),
    PRIMARY KEY (room_id, timer_id, generation)
) STRICT;
CREATE UNIQUE INDEX one_scheduled_timer_generation
ON timers(room_id, timer_id) WHERE state = 'scheduled';
CREATE TABLE observation_frames (
    room_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    frame_seq INTEGER NOT NULL CHECK (frame_seq BETWEEN 1 AND 9007199254740991),
    cause_room_seq INTEGER NOT NULL CHECK (cause_room_seq BETWEEN 1 AND 9007199254740991),
    payload_hash TEXT NOT NULL,
    payload_bytes BLOB NOT NULL,
    PRIMARY KEY (room_id, member_id, frame_seq),
    FOREIGN KEY (room_id, member_id) REFERENCES room_members(room_id, member_id),
    FOREIGN KEY (room_id, cause_room_seq) REFERENCES transitions(room_id, room_seq)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE activation_decisions (
    room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
    cause_room_seq INTEGER NOT NULL CHECK (cause_room_seq BETWEEN 1 AND 9007199254740991),
    decision_id TEXT NOT NULL,
    target_member_id TEXT,
    decision_bytes BLOB NOT NULL,
    PRIMARY KEY (room_id, cause_room_seq, decision_id),
    FOREIGN KEY (room_id, cause_room_seq) REFERENCES transitions(room_id, room_seq)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE semantic_receipts (
    room_id TEXT NOT NULL REFERENCES rooms(room_id) DEFERRABLE INITIALLY DEFERRED,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN (
        'action', 'administration', 'timer_fired', 'external_input'
    )),
    operation_identity_bytes BLOB NOT NULL,
    codec_id TEXT NOT NULL CHECK (codec_id = 'worldstream/operation-receipt/v1'),
    canonical_request_hash BLOB NOT NULL CHECK (length(canonical_request_hash) = 32),
    basis_complete_head_bytes BLOB,
    semantic_input_bytes BLOB NOT NULL,
    semantic_time_bytes BLOB NOT NULL,
    resolution_kind TEXT NOT NULL CHECK (resolution_kind IN (
        'genesis_created', 'transition_committed', 'rejection_recorded', 'no_change_recorded'
    )),
    transition_seq INTEGER CHECK (transition_seq BETWEEN 1 AND 9007199254740991),
    stored_resolution_bytes BLOB NOT NULL,
    committed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK ((resolution_kind = 'transition_committed') = (transition_seq IS NOT NULL)),
    CHECK ((resolution_kind = 'genesis_created') = (basis_complete_head_bytes IS NULL)),
    PRIMARY KEY (operation_kind, operation_identity_bytes),
    FOREIGN KEY (room_id, transition_seq) REFERENCES transitions(room_id, room_seq)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TRIGGER semantic_receipts_immutable_update
BEFORE UPDATE ON semantic_receipts BEGIN
    SELECT RAISE(ABORT, 'Semantic Receipt is immutable');
END;
CREATE TRIGGER semantic_receipts_immutable_delete
BEFORE DELETE ON semantic_receipts BEGIN
    SELECT RAISE(ABORT, 'Semantic Receipt is immutable');
END;
";

static WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<WriterClient>>>> = OnceLock::new();

trait TrustedAuthorityClock: Send + Sync {
    fn checked_at(&self) -> Result<AuthorityCheckedAt, AuthorityStoreErrorV1>;
}

#[cfg(not(test))]
struct SystemAuthorityClock;

#[cfg(not(test))]
impl TrustedAuthorityClock for SystemAuthorityClock {
    fn checked_at(&self) -> Result<AuthorityCheckedAt, AuthorityStoreErrorV1> {
        OffsetDateTime::from(SystemTime::now())
            .format(&Rfc3339)
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?
            .parse()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)
    }
}

#[cfg(test)]
#[derive(Clone)]
struct TestAuthorityClock {
    current: Arc<Mutex<AuthorityCheckedAt>>,
}

#[cfg(test)]
impl TestAuthorityClock {
    fn at(value: &str) -> Self {
        Self {
            current: Arc::new(Mutex::new(
                value
                    .parse()
                    .unwrap_or_else(|_| panic!("fixed test authority time")),
            )),
        }
    }

    fn set(&self, value: &str) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|_| panic!("test authority clock lock"));
        *current = value
            .parse()
            .unwrap_or_else(|_| panic!("fixed test authority time"));
    }
}

#[cfg(test)]
impl TrustedAuthorityClock for TestAuthorityClock {
    fn checked_at(&self) -> Result<AuthorityCheckedAt, AuthorityStoreErrorV1> {
        self.current
            .lock()
            .map(|value| value.clone())
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)
    }
}

#[cfg(test)]
static GUARDED_COMMIT_PAUSE: OnceLock<(Mutex<GuardedCommitPause>, Condvar)> = OnceLock::new();

#[cfg(test)]
static RECOVERY_INSTALL_PAUSE: OnceLock<(Mutex<RecoveryInstallPause>, Condvar)> = OnceLock::new();

#[cfg(test)]
static WRITER_QUEUE_PAUSE: OnceLock<(Mutex<GuardedCommitPause>, Condvar)> = OnceLock::new();

#[cfg(test)]
static REPLAY_PROJECTION_PAUSE: OnceLock<(Mutex<ReplayProjectionPause>, Condvar)> = OnceLock::new();

#[cfg(test)]
static REPLAY_SLICE_BUDGET: OnceLock<Mutex<ReplaySliceBudget>> = OnceLock::new();

#[cfg(test)]
static REPLAY_PROJECTION_TEST_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
static AUTHORITY_CHANGE_ENQUEUED: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

#[cfg(test)]
#[derive(Default)]
struct GuardedCommitPause {
    reached: bool,
    released: bool,
}

#[cfg(test)]
#[derive(Default)]
struct RecoveryInstallPause {
    target_path: Option<PathBuf>,
    reached: bool,
    released: bool,
}

#[cfg(test)]
#[derive(Default)]
struct ReplayProjectionPause {
    target_path: Option<PathBuf>,
    reached: bool,
    released: bool,
}

#[cfg(test)]
#[derive(Default)]
struct ReplaySliceBudget {
    by_path: BTreeMap<PathBuf, usize>,
}

/// Cloneable handle to one controlled writer thread and connection.
#[derive(Clone)]
pub struct SqliteRoomStore {
    writer: Arc<WriterClient>,
}

/// Safe result of present-authorized `SQLite` Replay. The underlying pure Core
/// historical projection remains private so it cannot be confused with proof
/// that a current authority fence was checked.
pub struct SqliteAuthorizedReplayProjectionV1 {
    historical: HistoricalReplayProjectionV1,
    canonical_envelope: worldstream_core::CanonicalJsonV1,
}

/// Bounded progress from one present-authorized Replay slice.
pub enum SqliteAuthorizedReplayOutcomeV1 {
    /// The exact historical projection passed the final authority and
    /// operational release fence.
    Complete(Box<SqliteAuthorizedReplayProjectionV1>),
    /// More immutable prefix pages remain. The opaque continuation must be
    /// consumed by the same store through [`SqliteRoomStore::resume_replay_authorized`].
    Deferred(SqliteAuthorizedReplayContinuationV1),
}

impl fmt::Debug for SqliteAuthorizedReplayOutcomeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Complete(_) => "SqliteAuthorizedReplayOutcomeV1::Complete([REDACTED])",
            Self::Deferred(_) => "SqliteAuthorizedReplayOutcomeV1::Deferred([REDACTED])",
        })
    }
}

/// Opaque, store-bound progress token for bounded authorized Replay paging.
pub struct SqliteAuthorizedReplayContinuationV1 {
    session_id: u64,
}

impl fmt::Debug for SqliteAuthorizedReplayContinuationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SqliteAuthorizedReplayContinuationV1([REDACTED])")
    }
}

impl fmt::Debug for SqliteAuthorizedReplayProjectionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteAuthorizedReplayProjectionV1")
            .field("verified_head", self.historical.verified_head())
            .field("integrity", self.historical.integrity())
            .field(
                "historical_room_status",
                &self.historical.historical_room_status(),
            )
            .field(
                "historical_member_id",
                self.historical.historical_membership().member_id(),
            )
            .field("canonical_envelope", &"[REDACTED]")
            .finish()
    }
}

impl SqliteAuthorizedReplayProjectionV1 {
    fn from_historical(
        historical: HistoricalReplayProjectionV1,
        address: &AuthorizedReplayAddressV1,
    ) -> Result<Self, HistoricalReplayErrorV1> {
        if historical.verified_head().room_id() != &address.room_id
            || historical.verified_head().room_seq() != address.at_room_seq
            || historical.historical_membership().member_id() != &address.member_id
        {
            return Err(HistoricalReplayErrorV1::AddressMismatch);
        }
        let envelope_bytes = serde_json::to_vec(&SqliteAuthorizedReplayEnvelopeV1 {
            envelope: "worldstream/authorized-replay-projection/v1",
            room_id: &address.room_id,
            member_id: &address.member_id,
            at_room_seq: address.at_room_seq,
            projection_kind: address.projection_kind,
            integrity: historical.integrity(),
            historical_projection: historical.canonical_envelope(),
        })
        .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
        let canonical_envelope = worldstream_core::CanonicalJsonV1::parse(&envelope_bytes)
            .map_err(|_| HistoricalReplayErrorV1::ProjectionUnavailable)?;
        Ok(Self {
            historical,
            canonical_envelope,
        })
    }

    /// Returns the exact historical Head verified by retained-Pack Replay.
    #[must_use]
    pub const fn verified_head(&self) -> &CompleteHeadV1 {
        self.historical.verified_head()
    }

    /// Returns the exact operational integrity status and generation fenced
    /// in the writer transaction that admitted this Replay.
    #[must_use]
    pub const fn integrity(&self) -> &RoomIntegrityStateV1 {
        self.historical.integrity()
    }

    /// Returns the Core Room lifecycle status reconstructed at the requested
    /// historical sequence.
    #[must_use]
    pub const fn historical_room_status(&self) -> RoomStatusV1 {
        self.historical.historical_room_status()
    }

    /// Returns the exact Membership reconstructed at the requested sequence.
    #[must_use]
    pub const fn historical_membership(&self) -> &MembershipV1 {
        self.historical.historical_membership()
    }

    /// Returns the host-validated projection envelope. It contains no raw
    /// lineage, canonical Activity State, receipt, bearer, or continuation.
    #[must_use]
    pub const fn canonical_envelope(&self) -> &worldstream_core::CanonicalJsonV1 {
        &self.canonical_envelope
    }
}

#[derive(serde::Serialize)]
struct SqliteAuthorizedReplayEnvelopeV1<'a> {
    envelope: &'static str,
    room_id: &'a RoomId,
    member_id: &'a MemberId,
    at_room_seq: RoomSequenceV1,
    projection_kind: ReplayProjectionKindV1,
    integrity: &'a RoomIntegrityStateV1,
    historical_projection: &'a worldstream_core::CanonicalJsonV1,
}

/// Host-internal immutable capture used only to construct Core's recovery
/// candidate. Raw durable bytes never cross the Adapter's public surface.
#[derive(Clone)]
struct RoomHistoryInspectionV1 {
    head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    canonical_head_bytes: Vec<u8>,
    canonical_pack_revision_lock_bytes: Vec<u8>,
    canonical_genesis_bytes: Vec<u8>,
    canonical_transition_bytes: Vec<Vec<u8>>,
    canonical_core_state_bytes: Option<Vec<u8>>,
    canonical_activity_state_bytes: Option<Vec<u8>>,
}

/// Private authorization session retained across bounded immutable-prefix
/// paging and consumed by the final writer-owned release fence.
struct AuthorizedReplaySessionV1 {
    authority: ReplayAdapterInputV1,
    address: AuthorizedReplayAddressV1,
    request: Option<HistoricalReplayProjectionRequestV1>,
    disposition_fence: ReplayIntegrityDispositionFenceV1,
}

struct AuthorizedReplayPagingStateV1 {
    session: AuthorizedReplaySessionV1,
    accumulator: Option<HistoricalReplayAccumulatorV1>,
    receipt_head: Option<CompleteHeadV1>,
    prior_sequence: u64,
}

struct AuthorizedReplayAddressV1 {
    room_id: RoomId,
    member_id: MemberId,
    at_room_seq: RoomSequenceV1,
    projection_kind: ReplayProjectionKindV1,
}

struct ReplayIntegrityDispositionFenceV1 {
    room_id: RoomId,
    head: CompleteHeadV1,
    status: RoomIntegrityStatusV1,
    generation: IntegrityGenerationV1,
}

impl RoomHistoryInspectionV1 {
    fn recovery_candidate(&self) -> RoomRecoveryCandidateV1 {
        RoomRecoveryCandidateV1::new(
            self.head.clone(),
            self.integrity_generation,
            self.canonical_head_bytes.clone(),
            self.canonical_pack_revision_lock_bytes.clone(),
            self.canonical_genesis_bytes.clone(),
            self.canonical_transition_bytes.clone(),
            self.canonical_core_state_bytes.clone(),
            self.canonical_activity_state_bytes.clone(),
        )
    }
}

struct WriterClient {
    commands: SyncSender<WriterCommand>,
    path: PathBuf,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
    in_flight_replay_slices: Mutex<usize>,
    replay_sessions: Mutex<ReplaySessionRegistryV1>,
}

#[derive(Default)]
struct ReplaySessionRegistryV1 {
    by_id: BTreeMap<u64, ReplaySessionEntryV1>,
}

struct ReplaySessionEntryV1 {
    last_used_at: Instant,
    paging: Box<AuthorizedReplayPagingStateV1>,
}

struct ActiveReplaySessionV1 {
    writer: Arc<WriterClient>,
    session_id: u64,
    _slice_permit: ReplaySlicePermitV1,
}

struct ReplaySlicePermitV1 {
    writer: Arc<WriterClient>,
}

impl ActiveReplaySessionV1 {
    fn retain_deferred(
        self,
        paging: AuthorizedReplayPagingStateV1,
    ) -> Result<SqliteAuthorizedReplayContinuationV1, SqliteAuthorizedReplayErrorV1> {
        let now = Instant::now();
        let mut sessions = self
            .writer
            .replay_sessions
            .lock()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        cleanup_expired_replay_sessions(&mut sessions, now);
        if sessions.by_id.len() >= MAX_DEFERRED_REPLAY_SESSIONS {
            return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable);
        }
        if sessions
            .by_id
            .insert(
                self.session_id,
                ReplaySessionEntryV1 {
                    last_used_at: now,
                    paging: Box::new(paging),
                },
            )
            .is_some()
        {
            sessions.by_id.remove(&self.session_id);
            return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable);
        }
        Ok(SqliteAuthorizedReplayContinuationV1 {
            session_id: self.session_id,
        })
    }
}

impl Drop for ReplaySlicePermitV1 {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = self.writer.in_flight_replay_slices.lock() {
            *in_flight = in_flight.saturating_sub(1);
        }
    }
}

enum WriterCommand {
    ApplyAuthorityBootstrap(
        Box<PreparedAuthorityBootstrapV1>,
        mpsc::Sender<Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1>>,
    ),
    ApplyAuthorityChange(
        Box<PreparedAuthorityChangeV1>,
        mpsc::Sender<Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1>>,
    ),
    Commit(
        Box<SqlitePreparedWrite>,
        mpsc::Sender<RoomCommitResolutionV1>,
    ),
    Resolve {
        identity: OperationIdentityV1,
        identity_bytes: Vec<u8>,
        request_hash: CanonicalRequestHashV1,
        reply: mpsc::Sender<ResolveOutcomeV1>,
    },
    ResolveAuthorized {
        authority: AuthorizedReceiptReadV1,
        reply: mpsc::Sender<Result<ResolveOutcomeV1, AuthorityErrorV1>>,
    },
    BeginAuthorizedReplay {
        authority: AuthorizedReplayV1,
        reply: mpsc::Sender<Result<AuthorizedReplaySessionV1, SqliteAuthorizedReplayErrorV1>>,
    },
    ReleaseAuthorizedReplay {
        session: Box<AuthorizedReplaySessionV1>,
        require_operational_fence: bool,
        reply: mpsc::Sender<Result<AuthorizedReplaySessionV1, SqliteAuthorizedReplayErrorV1>>,
    },
    RecordReplayDisposition {
        fence: ReplayIntegrityDispositionFenceV1,
        disposition: RecoveryIntegrityDispositionV1,
        reply: mpsc::Sender<Result<(), RoomRecoveryErrorV1>>,
    },
    GuardRecoveryInstall {
        room_id: RoomId,
        expected_head: CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        recovered_materializations: Box<RecoveredRoomMaterializationsV1>,
        reply: mpsc::Sender<Result<(), RoomRecoveryErrorV1>>,
    },
    RecordRecoveryFailure {
        room_id: RoomId,
        expected_head: CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
        reply: mpsc::Sender<Result<(), RoomRecoveryErrorV1>>,
    },
    QuarantineObservedRecoveryCorruption {
        fence: Box<ObservedRecoveryFence>,
        reply: mpsc::Sender<Result<(), RoomRecoveryErrorV1>>,
    },
    #[cfg(test)]
    SeedAuthority {
        witness: PreparedAuthorityWitnessV1,
        active: bool,
        reply: mpsc::Sender<Result<(), String>>,
    },
    #[cfg(test)]
    SeedAuthoritySnapshot {
        principal: PrincipalAuthoritySnapshotV1,
        capability: CapabilityAuthoritySnapshotV1,
        reply: mpsc::Sender<Result<(), String>>,
    },
    #[cfg(test)]
    SetFailpoint(Option<WriteBoundary>, mpsc::Sender<()>),
    #[cfg(test)]
    SetQueryOnly(bool, mpsc::Sender<Result<(), String>>),
    #[cfg(test)]
    PauseQueue,
    Shutdown,
}

impl Drop for WriterClient {
    fn drop(&mut self) {
        let _ = self.commands.send(WriterCommand::Shutdown);
        if let Ok(thread) = self.thread.get_mut()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
}

struct SqlitePreparedWrite {
    identity: OperationIdentityV1,
    identity_bytes: Vec<u8>,
    request_hash: CanonicalRequestHashV1,
    authority: PreparedAuthorityWitnessV1,
    receipt: PreparedReceiptRow,
    branch: PreparedSqliteBranch,
}

struct PreparedReceiptRow {
    room_id: String,
    operation_kind: &'static str,
    basis_head_bytes: Option<Vec<u8>>,
    semantic_input_bytes: Vec<u8>,
    semantic_time_bytes: Vec<u8>,
    resolution_kind: &'static str,
    transition_seq: Option<i64>,
    stored_result: StoredSemanticResultV1,
}

enum PreparedSqliteBranch {
    Create(Box<PreparedCreationPersistenceV1>),
    Existing {
        basis: Box<worldstream_core::CompleteHeadV1>,
        integrity_generation: worldstream_core::IntegrityGenerationV1,
        input_witness: PreparedOperationInputWitnessV1,
        intent: PreparedExistingIntentV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredHeadProjection {
    room_status: String,
    room_seq: i64,
    lineage_hash: String,
    core_schema_version: String,
    pack_digest: String,
    core_state_hash: String,
    activity_state_hash: String,
    authoritative_state_hash: String,
    canonical_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedRecoveryFence {
    room_id: String,
    head: StoredHeadProjection,
    integrity_status: Option<String>,
    integrity_generation: Option<i64>,
}

type StoredMembershipProjection = (String, String, String, String, Option<String>, Vec<u8>);

struct StoredReceiptProjection {
    room_id: String,
    operation_kind: String,
    operation_identity_bytes: Vec<u8>,
    codec_id: String,
    canonical_request_hash: Vec<u8>,
    basis_complete_head_bytes: Option<Vec<u8>>,
    semantic_input_bytes: Vec<u8>,
    semantic_time_bytes: Vec<u8>,
    resolution_kind: String,
    transition_seq: Option<i64>,
    stored_resolution_bytes: Vec<u8>,
}

struct ReplayTransitionPageRowV1 {
    sequence: i64,
    transition_id: String,
    transition_hash: String,
    previous_lineage_hash: String,
    core_schema_version: String,
    pack_digest: String,
    core_state_hash: String,
    activity_state_hash: String,
    authoritative_state_hash: String,
    transition_bytes: Vec<u8>,
    receipt_count: i64,
    receipt: Option<StoredReceiptProjection>,
}

type CurrentTransitionProjection = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Vec<u8>,
);

struct StoredCapabilityAuthorityProjection {
    capability_id: String,
    token_hash: Vec<u8>,
    principal_id: String,
    profile_kind: String,
    target_room_id: Option<String>,
    target_member_id: Option<String>,
    runner_id: Option<String>,
    authority_generation: i64,
    expires_at: Option<String>,
    revoked_at: Option<String>,
}

struct StoredMembershipAuthorityProjection {
    principal_id: String,
    principal_kind: String,
    standing: String,
    access_mode: String,
    role: Option<String>,
    membership_bytes: Vec<u8>,
    membership_generation: i64,
}

struct StoredAuthorityChangeReceiptProjection {
    authenticated_principal: Option<String>,
    request_hash: Vec<u8>,
    result_kind: String,
    target_kind: String,
    target_id: String,
    secondary_target_id: Option<String>,
    resulting_generation: i64,
    checked_at: String,
}

struct StoredAuthorityAuditProjection {
    actor_principal_id: Option<String>,
    target_kind: String,
    target_id: String,
    secondary_target_id: Option<String>,
    change_kind: String,
    prior_generation: Option<i64>,
    resulting_generation: i64,
    checked_at: String,
    reason_code: Option<String>,
    request_hash: Vec<u8>,
}

impl SqlitePreparedWrite {
    fn from_core(prepared: &PreparedRoomWriteV1) -> Result<Self, ()> {
        let identity = prepared.identity().clone();
        let identity_bytes = identity.canonical_bytes().map_err(|_| ())?;
        let request_hash = prepared.request_hash().clone();
        let (authority, stored_result, branch) = match prepared {
            PreparedRoomWriteV1::Create(create) => (
                create.authority_witness().clone(),
                create.semantic_result().clone(),
                PreparedSqliteBranch::Create(Box::new(create.persistence().clone())),
            ),
            PreparedRoomWriteV1::Existing(existing) => (
                existing.authority_witness().clone(),
                existing.semantic_result().clone(),
                PreparedSqliteBranch::Existing {
                    basis: Box::new(existing.basis_complete_head().clone()),
                    integrity_generation: existing.integrity_generation(),
                    input_witness: existing.input_witness().clone(),
                    intent: existing.intent().clone(),
                },
            ),
        };
        if stored_result.operation_identity() != &identity
            || stored_result.canonical_request_hash() != &request_hash
        {
            return Err(());
        }
        let receipt = PreparedReceiptRow {
            room_id: stored_result.target_room_id().to_string(),
            operation_kind: identity.operation_kind(),
            basis_head_bytes: stored_result.canonical_basis_head_bytes().map_err(|_| ())?,
            semantic_input_bytes: stored_result
                .semantic_input()
                .canonical_bytes()
                .map_err(|_| ())?,
            semantic_time_bytes: stored_result
                .canonical_semantic_time_bytes()
                .map_err(|_| ())?,
            resolution_kind: stored_result.resolution_kind(),
            transition_seq: stored_result
                .transition_seq()
                .map(|sequence| i64::try_from(sequence.get()).map_err(|_| ()))
                .transpose()?,
            stored_result,
        };
        Ok(Self {
            identity,
            identity_bytes,
            request_hash,
            authority,
            receipt,
            branch,
        })
    }
}

impl SqliteRoomStore {
    /// Opens the local database and fails closed unless the linked engine is
    /// the exact release-selected bundled `SQLite` source. Repeated opens of the
    /// same path share one private bounded writer queue and writer connection.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is unsafe, another process owns the
    /// writer lease, the exact engine identity or schema does not match, or
    /// the writer connection cannot be initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteStoreOpenError> {
        #[cfg(not(test))]
        let clock: Arc<dyn TrustedAuthorityClock> = Arc::new(SystemAuthorityClock);
        #[cfg(test)]
        let clock: Arc<dyn TrustedAuthorityClock> =
            Arc::new(TestAuthorityClock::at("2026-08-15T12:00:10Z"));
        Self::open_with_clock(path, clock)
    }

    fn open_with_clock(
        path: impl AsRef<Path>,
        clock: Arc<dyn TrustedAuthorityClock>,
    ) -> Result<Self, SqliteStoreOpenError> {
        let path = normalized_path(path.as_ref())?;
        let registry = WRITERS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let mut registry = registry
            .lock()
            .map_err(|_| SqliteStoreOpenError::WriterRegistryPoisoned)?;
        if let Some(writer) = registry.get(&path).and_then(Weak::upgrade) {
            return Ok(Self { writer });
        }

        let (commands, receiver) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let (startup_send, startup_receive) = mpsc::channel();
        let thread_path = path.clone();
        let writer_thread = thread::Builder::new()
            .name("worldstream-sqlite-writer".to_owned())
            .spawn(move || writer_main(&thread_path, receiver, startup_send, clock.as_ref()))
            .map_err(SqliteStoreOpenError::ThreadSpawn)?;
        let startup_result = startup_receive
            .recv()
            .map_err(|_| SqliteStoreOpenError::WriterStartup)?;
        if let Err(error) = startup_result {
            let _ = writer_thread.join();
            return Err(error);
        }
        let writer = Arc::new(WriterClient {
            commands,
            path,
            thread: Mutex::new(Some(writer_thread)),
            in_flight_replay_slices: Mutex::new(0),
            replay_sessions: Mutex::new(ReplaySessionRegistryV1::default()),
        });
        registry.insert(writer.path.clone(), Arc::downgrade(&writer));
        Ok(Self { writer })
    }

    /// Returns the exact runtime engine identity verified at open.
    #[must_use]
    pub fn engine_identity(&self) -> (&'static str, &'static str) {
        (SQLITE_VERSION, SQLITE_SOURCE_ID)
    }

    /// Revalidates one receipt-read grant and resolves its exact operation in
    /// the same writer-owned transaction and trusted clock sample.
    ///
    /// # Errors
    ///
    /// Returns a closed authority failure if the current durable fence no
    /// longer matches the consumed grant or receipt resolution cannot be
    /// completed safely.
    pub fn resolve_authorized(
        &self,
        authority: AuthorizedReceiptReadV1,
    ) -> Result<ResolveOutcomeV1, AuthorityErrorV1> {
        AuthorizedReceiptResolverV1::resolve_authorized(self, authority)
    }

    /// Revalidates a held Replay grant on the controlled writer, reads its
    /// immutable prefix in bounded short pages, then revalidates authority and
    /// the operational fence on the writer before releasing a projection.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, storage, corruption, sequence, replay, or
    /// projection failure. Canonical Genesis/Transition bytes never escape.
    pub fn replay_authorized(
        &self,
        registry: &PackRegistryV1,
        authority: AuthorizedReplayV1,
    ) -> Result<SqliteAuthorizedReplayOutcomeV1, SqliteAuthorizedReplayErrorV1> {
        let active = self.reserve_replay_session()?;
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::BeginAuthorizedReplay { authority, reply })
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let session = receive
            .recv()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)??;
        self.advance_authorized_replay_slice(
            registry,
            AuthorizedReplayPagingStateV1 {
                session,
                accumulator: None,
                receipt_head: None,
                prior_sequence: 0,
            },
            active,
        )
    }

    /// Resumes one opaque bounded Replay slice on the exact store that
    /// produced it. Present authority and the operational fence are checked
    /// again before any additional immutable pages are read.
    ///
    /// # Errors
    ///
    /// Returns a closed denial when the token belongs to another store or any
    /// current authority, Head, integrity, paging, or projection fence fails.
    pub fn resume_replay_authorized(
        &self,
        registry: &PackRegistryV1,
        continuation: SqliteAuthorizedReplayContinuationV1,
    ) -> Result<SqliteAuthorizedReplayOutcomeV1, SqliteAuthorizedReplayErrorV1> {
        let Some(slice_permit) = self.try_acquire_replay_slice()? else {
            return Ok(SqliteAuthorizedReplayOutcomeV1::Deferred(continuation));
        };
        let (active, mut paging) =
            self.take_replay_session(continuation.session_id, slice_permit)?;
        paging.session = self.fence_authorized_replay(paging.session, true)?;
        self.advance_authorized_replay_slice(registry, paging, active)
    }

    fn advance_authorized_replay_slice(
        &self,
        registry: &PackRegistryV1,
        mut paging: AuthorizedReplayPagingStateV1,
        active: ActiveReplaySessionV1,
    ) -> Result<SqliteAuthorizedReplayOutcomeV1, SqliteAuthorizedReplayErrorV1> {
        match read_authorized_replay_slice(&self.writer.path, registry, &mut paging) {
            Err(error) => return self.finish_authorized_replay_error(paging.session, error),
            Ok(false) => {
                paging.session = self.fence_authorized_replay(paging.session, true)?;
                let continuation = active.retain_deferred(paging)?;
                return Ok(SqliteAuthorizedReplayOutcomeV1::Deferred(continuation));
            }
            Ok(true) => {}
        }
        let AuthorizedReplayPagingStateV1 {
            session,
            accumulator,
            receipt_head: _,
            prior_sequence: _,
        } = paging;
        let projection = match accumulator {
            Some(accumulator) => accumulator
                .finish()
                .map_err(SqliteAuthorizedReplayErrorV1::Replay),
            None => Err(SqliteAuthorizedReplayErrorV1::Corrupt),
        }
        .and_then(|historical| {
            SqliteAuthorizedReplayProjectionV1::from_historical(historical, &session.address)
                .map_err(SqliteAuthorizedReplayErrorV1::Replay)
        });
        let incident = projection.as_ref().err().copied().map_or(Ok(()), |error| {
            self.record_replay_incident_if_needed(&session.disposition_fence, error)
        });
        let _session = self.fence_authorized_replay(session, projection.is_ok())?;
        incident?;
        projection.map(|projection| SqliteAuthorizedReplayOutcomeV1::Complete(Box::new(projection)))
    }

    fn reserve_replay_session(
        &self,
    ) -> Result<ActiveReplaySessionV1, SqliteAuthorizedReplayErrorV1> {
        let now = Instant::now();
        let mut sessions = self
            .writer
            .replay_sessions
            .lock()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        cleanup_expired_replay_sessions(&mut sessions, now);
        drop(sessions);
        let slice_permit = self
            .try_acquire_replay_slice()?
            .ok_or(SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let session_id = NEXT_REPLAY_SESSION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        Ok(ActiveReplaySessionV1 {
            writer: Arc::clone(&self.writer),
            session_id,
            _slice_permit: slice_permit,
        })
    }

    fn try_acquire_replay_slice(
        &self,
    ) -> Result<Option<ReplaySlicePermitV1>, SqliteAuthorizedReplayErrorV1> {
        let mut in_flight = self
            .writer
            .in_flight_replay_slices
            .lock()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        if *in_flight >= MAX_IN_FLIGHT_REPLAY_SLICES {
            return Ok(None);
        }
        *in_flight += 1;
        drop(in_flight);
        Ok(Some(ReplaySlicePermitV1 {
            writer: Arc::clone(&self.writer),
        }))
    }

    fn take_replay_session(
        &self,
        session_id: u64,
        slice_permit: ReplaySlicePermitV1,
    ) -> Result<(ActiveReplaySessionV1, AuthorizedReplayPagingStateV1), SqliteAuthorizedReplayErrorV1>
    {
        let now = Instant::now();
        let mut sessions = self
            .writer
            .replay_sessions
            .lock()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        cleanup_expired_replay_sessions(&mut sessions, now);
        let Some(entry) = sessions.by_id.remove(&session_id) else {
            return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable);
        };
        drop(sessions);
        Ok((
            ActiveReplaySessionV1 {
                writer: Arc::clone(&self.writer),
                session_id,
                _slice_permit: slice_permit,
            },
            *entry.paging,
        ))
    }

    fn record_replay_incident_if_needed(
        &self,
        fence: &ReplayIntegrityDispositionFenceV1,
        error: SqliteAuthorizedReplayErrorV1,
    ) -> Result<(), SqliteAuthorizedReplayErrorV1> {
        let disposition = match error {
            SqliteAuthorizedReplayErrorV1::Corrupt => {
                Some(RecoveryIntegrityDispositionV1::Quarantined)
            }
            SqliteAuthorizedReplayErrorV1::Replay(error) => replay_integrity_disposition(error),
            SqliteAuthorizedReplayErrorV1::Authority(_)
            | SqliteAuthorizedReplayErrorV1::StorageUnavailable => None,
        };
        let Some(disposition) = disposition else {
            return Ok(());
        };
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::RecordReplayDisposition {
                fence: ReplayIntegrityDispositionFenceV1 {
                    room_id: fence.room_id.clone(),
                    head: fence.head.clone(),
                    status: fence.status,
                    generation: fence.generation,
                },
                disposition,
                reply,
            })
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        match receive
            .recv()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
        {
            Ok(())
            | Err(
                RoomRecoveryErrorV1::ConcurrentChange | RoomRecoveryErrorV1::IntegrityUnavailable,
            ) => Ok(()),
            Err(
                RoomRecoveryErrorV1::StorageUnavailable
                | RoomRecoveryErrorV1::Corrupt
                | RoomRecoveryErrorV1::RuntimeUnavailable
                | RoomRecoveryErrorV1::RuntimeFault,
            ) => Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable),
        }
    }

    fn finish_authorized_replay_error(
        &self,
        session: AuthorizedReplaySessionV1,
        error: SqliteAuthorizedReplayErrorV1,
    ) -> Result<SqliteAuthorizedReplayOutcomeV1, SqliteAuthorizedReplayErrorV1> {
        let incident = self.record_replay_incident_if_needed(&session.disposition_fence, error);
        let _session = self.fence_authorized_replay(session, false)?;
        incident?;
        Err(error)
    }

    fn fence_authorized_replay(
        &self,
        session: AuthorizedReplaySessionV1,
        require_operational_fence: bool,
    ) -> Result<AuthorizedReplaySessionV1, SqliteAuthorizedReplayErrorV1> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::ReleaseAuthorizedReplay {
                session: Box::new(session),
                require_operational_fence,
                reply,
            })
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        receive
            .recv()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
    }

    /// Runs trusted host recovery through Core's opaque recovery SPI. This is
    /// operational Room recovery, not caller-authorized historical Replay or
    /// a diagnostic raw-history surface.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery error when the immutable lineage, disposable
    /// projections, retained runtime, integrity fence, or guarded install
    /// cannot be verified.
    pub fn recover_room(
        &self,
        registry: &PackRegistryV1,
        room_id: &RoomId,
    ) -> Result<Option<CoreTraceV1>, RoomRecoveryErrorV1> {
        recover_room_from_storage(self, registry, room_id)
    }
}

impl AuthorizedReceiptResolverV1 for SqliteRoomStore {
    fn resolve_authorized(
        &self,
        authority: AuthorizedReceiptReadV1,
    ) -> Result<ResolveOutcomeV1, AuthorityErrorV1> {
        let (reply, receive) = mpsc::channel();
        if self
            .writer
            .commands
            .send(WriterCommand::ResolveAuthorized { authority, reply })
            .is_err()
        {
            return Err(AuthorityErrorV1::Unavailable);
        }
        receive.recv().unwrap_or(Err(AuthorityErrorV1::Unavailable))
    }
}

impl SqliteRoomStore {
    /// Captures one read-only Room history at a single `SQLite` snapshot and
    /// bounds Transition loading by the captured durable Head.
    ///
    /// # Errors
    ///
    /// Returns an error for `SQLite` I/O, a noncanonical/internally inconsistent
    /// row projection, missing lineage rows, or non-healthy integrity state.
    fn inspect_room(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomHistoryInspectionV1>, SqliteRoomInspectionErrorV1> {
        inspect_room_at_path(&self.writer.path, room_id)
    }

    #[cfg(test)]
    fn seed_authority(
        &self,
        witness: &PreparedAuthorityWitnessV1,
        active: bool,
    ) -> Result<(), String> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::SeedAuthority {
                witness: witness.clone(),
                active,
                reply,
            })
            .map_err(|_| "writer unavailable".to_owned())?;
        receive
            .recv()
            .map_err(|_| "writer unavailable".to_owned())?
    }

    #[cfg(test)]
    fn seed_authority_snapshot(
        &self,
        principal: PrincipalAuthoritySnapshotV1,
        capability: CapabilityAuthoritySnapshotV1,
    ) -> Result<(), String> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::SeedAuthoritySnapshot {
                principal,
                capability,
                reply,
            })
            .map_err(|_| "writer unavailable".to_owned())?;
        receive
            .recv()
            .map_err(|_| "writer unavailable".to_owned())?
    }

    #[cfg(test)]
    fn set_failpoint(&self, failpoint: Option<WriteBoundary>) {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::SetFailpoint(failpoint, reply))
            .unwrap_or_else(|_| panic!("writer available for failpoint"));
        receive
            .recv()
            .unwrap_or_else(|_| panic!("writer acknowledged failpoint"));
    }

    #[cfg(test)]
    fn set_query_only(&self, enabled: bool) -> Result<(), String> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::SetQueryOnly(enabled, reply))
            .map_err(|_| "writer unavailable".to_owned())?;
        receive
            .recv()
            .map_err(|_| "writer unavailable".to_owned())?
    }

    #[cfg(test)]
    fn pause_writer_queue(&self) {
        self.writer
            .commands
            .send(WriterCommand::PauseQueue)
            .unwrap_or_else(|_| panic!("writer available for queue pause"));
    }

    #[cfg(test)]
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        RoomCommitStorageV1::commit(self, prepared)
    }
}

impl AuthorityStoreV1 for SqliteRoomStore {
    fn snapshot(
        &self,
        query: &AuthoritySnapshotQueryV1,
    ) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
        snapshot_authority_at_path(&self.writer.path, query)
    }

    fn apply_bootstrap(
        &self,
        bootstrap: &PreparedAuthorityBootstrapV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::ApplyAuthorityBootstrap(
                Box::new(bootstrap.clone()),
                reply,
            ))
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        receive
            .recv()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?
    }

    fn apply_change(
        &self,
        change: &PreparedAuthorityChangeV1,
    ) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::ApplyAuthorityChange(
                Box::new(change.clone()),
                reply,
            ))
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?;
        #[cfg(test)]
        mark_authority_change_enqueued();
        receive
            .recv()
            .map_err(|_| AuthorityStoreErrorV1::Unavailable)?
    }
}

impl RoomCommitStorageV1 for SqliteRoomStore {
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        let Ok(prepared) = SqlitePreparedWrite::from_core(prepared) else {
            return RoomCommitResolutionV1::Fault;
        };
        let (reply, receive) = mpsc::channel();
        if self
            .writer
            .commands
            .send(WriterCommand::Commit(Box::new(prepared), reply))
            .is_err()
        {
            return RoomCommitResolutionV1::Indeterminate;
        }
        receive
            .recv()
            .unwrap_or(RoomCommitResolutionV1::Indeterminate)
    }

    fn resolve(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1 {
        let Ok(identity_bytes) = identity.canonical_bytes() else {
            return ResolveOutcomeV1::ResolutionUnavailable;
        };
        let (reply, receive) = mpsc::channel();
        if self
            .writer
            .commands
            .send(WriterCommand::Resolve {
                identity: identity.clone(),
                identity_bytes,
                request_hash: request_hash.clone(),
                reply,
            })
            .is_err()
        {
            return ResolveOutcomeV1::ResolutionUnavailable;
        }
        receive
            .recv()
            .unwrap_or(ResolveOutcomeV1::ResolutionUnavailable)
    }
}

impl RoomRecoveryStorageV1 for SqliteRoomStore {
    fn inspect_recovery_candidate(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        let observed_fence = capture_observed_recovery_fence(&self.writer.path, room_id)
            .map_err(|error| map_inspection_error(&error))?;
        match self.inspect_room(room_id) {
            Ok(inspection) => Ok(inspection.map(|value| value.recovery_candidate())),
            Err(SqliteRoomInspectionErrorV1::Corrupt) => {
                if let Some(fence) = observed_fence {
                    let (reply, receive) = mpsc::channel();
                    self.writer
                        .commands
                        .send(WriterCommand::QuarantineObservedRecoveryCorruption {
                            fence: Box::new(fence),
                            reply,
                        })
                        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
                    receive
                        .recv()
                        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)??;
                }
                Err(RoomRecoveryErrorV1::Corrupt)
            }
            Err(error) => Err(map_inspection_error(&error)),
        }
    }

    fn guard_recovery_install(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        recovered_materializations: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        #[cfg(test)]
        pause_before_recovery_install_guard(&self.writer.path);
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::GuardRecoveryInstall {
                room_id: room_id.clone(),
                expected_head: expected_head.clone(),
                expected_integrity_generation,
                recovered_materializations: Box::new(recovered_materializations.clone()),
                reply,
            })
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        receive
            .recv()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
    }

    fn record_recovery_failure(
        &self,
        room_id: &RoomId,
        expected_head: &CompleteHeadV1,
        expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        let (reply, receive) = mpsc::channel();
        self.writer
            .commands
            .send(WriterCommand::RecordRecoveryFailure {
                room_id: room_id.clone(),
                expected_head: expected_head.clone(),
                expected_integrity_generation,
                disposition,
                reply,
            })
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        receive
            .recv()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
    }
}

fn map_inspection_error(error: &SqliteRoomInspectionErrorV1) -> RoomRecoveryErrorV1 {
    match error {
        SqliteRoomInspectionErrorV1::Sqlite(_) | SqliteRoomInspectionErrorV1::Unavailable => {
            RoomRecoveryErrorV1::StorageUnavailable
        }
        SqliteRoomInspectionErrorV1::Corrupt => RoomRecoveryErrorV1::Corrupt,
        SqliteRoomInspectionErrorV1::IntegrityUnavailable => {
            RoomRecoveryErrorV1::IntegrityUnavailable
        }
    }
}

fn snapshot_authority_at_path(
    path: &Path,
    query: &AuthoritySnapshotQueryV1,
) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection = Connection::open_with_flags(path, flags).map_err(authority_sql_failure)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(authority_sql_failure)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(authority_sql_failure)?;
    let snapshot = load_authority_snapshot(&transaction, query)?;
    transaction.commit().map_err(authority_sql_failure)?;
    Ok(snapshot)
}

fn apply_authority_bootstrap(
    connection: &mut Connection,
    bootstrap: &PreparedAuthorityBootstrapV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(authority_sql_failure)?;
    let result = apply_authority_bootstrap_in_transaction(&transaction, bootstrap, clock);
    finish_authority_transaction(transaction, result)
}

fn apply_authority_bootstrap_in_transaction(
    transaction: &Transaction<'_>,
    bootstrap: &PreparedAuthorityBootstrapV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
    if let Some(receipt) = lookup_authority_bootstrap_receipt(transaction, bootstrap)? {
        return Ok(receipt);
    }

    let commit_checked_at = clock.checked_at()?;
    let occupied: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM principals UNION ALL SELECT 1 FROM capabilities \
             UNION ALL SELECT 1 FROM runners UNION ALL \
             SELECT 1 FROM authority_change_receipts)",
            (),
            |row| row.get(0),
        )
        .map_err(authority_sql_failure)?;
    let state = if occupied {
        AuthorityBootstrapStateV1::Occupied
    } else {
        AuthorityBootstrapStateV1::Empty
    };
    let validated = bootstrap.validate_install(state, &commit_checked_at)?;
    persist_validated_authority_bootstrap(transaction, &validated)?;
    let receipt = AuthorityChangeReceiptV1::from_applied_bootstrap(
        bootstrap,
        AuthorityChangeResultV1::AuthorityBootstrapped,
        1,
        commit_checked_at,
    )?;
    insert_authority_bootstrap_receipt(transaction, &receipt)?;
    Ok(receipt)
}

fn apply_authority_change(
    connection: &mut Connection,
    change: &PreparedAuthorityChangeV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(authority_sql_failure)?;
    let result = apply_authority_change_in_transaction(&transaction, change, clock);
    finish_authority_transaction(transaction, result)
}

fn finish_authority_transaction(
    transaction: Transaction<'_>,
    result: Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1>,
) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
    match result {
        Ok(receipt) => {
            transaction.commit().map_err(authority_sql_failure)?;
            Ok(receipt)
        }
        Err(error) => {
            if transaction.rollback().is_err() {
                Err(AuthorityStoreErrorV1::Unavailable)
            } else {
                Err(error)
            }
        }
    }
}

fn apply_authority_change_in_transaction(
    transaction: &Transaction<'_>,
    change: &PreparedAuthorityChangeV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorityChangeReceiptV1, AuthorityStoreErrorV1> {
    let actor_query = change.actor_snapshot_query();
    let actor_snapshot = load_authority_snapshot(transaction, &actor_query)?
        .ok_or(AuthorityStoreErrorV1::StaleGeneration)?;
    let commit_checked_at = clock.checked_at()?;
    change.revalidate_actor(&actor_snapshot, &commit_checked_at)?;
    if let Some(receipt) = lookup_authority_change_receipt(transaction, change)? {
        return Ok(receipt);
    }

    if let AuthorityChangeV1::RegisterCapability { capability, .. } = change.command() {
        let token_owner = transaction
            .query_row(
                "SELECT capability_id FROM capabilities WHERE token_hash = ?1",
                [capability.token_hash().storage_bytes().as_slice()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(authority_sql_failure)?;
        if token_owner.is_some() {
            return Err(AuthorityStoreErrorV1::Conflict);
        }
    }

    let target_state = load_authority_change_state(transaction, change)?;
    let validated = change.validate_target(&target_state, &commit_checked_at)?;
    persist_validated_authority_change(transaction, change, &validated)?;
    let receipt = AuthorityChangeReceiptV1::from_applied_change(
        change,
        validated.result(),
        validated.resulting_generation(),
        commit_checked_at,
    )?;
    insert_authority_change_receipt(transaction, change, &receipt)?;
    Ok(receipt)
}

fn lookup_authority_bootstrap_receipt(
    connection: &Connection,
    bootstrap: &PreparedAuthorityBootstrapV1,
) -> Result<Option<AuthorityChangeReceiptV1>, AuthorityStoreErrorV1> {
    let Some((row, audit)) =
        load_stored_authority_change_pair(connection, bootstrap.change_id().as_str())?
    else {
        return Ok(None);
    };
    validate_stored_authority_change_pair(&row, &audit)?;
    if row.request_hash.as_slice() != bootstrap.request_hash().as_bytes() {
        return Err(AuthorityStoreErrorV1::Conflict);
    }
    let result = authority_change_result_from_storage(&row.result_kind)?;
    let generation = parse_safe_authority_counter(row.resulting_generation)?;
    let changed_at = parse_authority_text::<AuthorityCheckedAt>(&row.checked_at)?;
    let receipt = AuthorityChangeReceiptV1::from_applied_bootstrap(
        bootstrap, result, generation, changed_at,
    )?;
    let (target_kind, target_id, secondary_target_id) =
        authority_change_target_storage(receipt.target());
    if row.authenticated_principal.is_some()
        || row.target_kind != target_kind
        || row.target_id != target_id
        || row.secondary_target_id != secondary_target_id
        || audit.change_kind != "bootstrap_authority"
        || audit.prior_generation.is_some()
        || audit.reason_code.is_some()
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(Some(receipt))
}

fn lookup_authority_change_receipt(
    connection: &Connection,
    change: &PreparedAuthorityChangeV1,
) -> Result<Option<AuthorityChangeReceiptV1>, AuthorityStoreErrorV1> {
    let Some((row, audit)) =
        load_stored_authority_change_pair(connection, change.command().change_id().as_str())?
    else {
        return Ok(None);
    };
    validate_stored_authority_change_pair(&row, &audit)?;
    if row.request_hash.as_slice() != change.request_hash().as_bytes() {
        return Err(AuthorityStoreErrorV1::Conflict);
    }

    let result = authority_change_result_from_storage(&row.result_kind)?;
    let generation = parse_safe_authority_counter(row.resulting_generation)?;
    let changed_at = parse_authority_text::<AuthorityCheckedAt>(&row.checked_at)?;
    let receipt =
        AuthorityChangeReceiptV1::from_applied_change(change, result, generation, changed_at)?;
    let (target_kind, target_id, secondary_target_id) =
        authority_change_target_storage(receipt.target());
    let (change_kind, prior_generation, reason_code) = authority_change_audit_facts(change);
    let prior_generation = prior_generation.map(to_i64_authority).transpose()?;
    if row.authenticated_principal.as_deref() != Some(change.actor_principal_id().as_str())
        || row.target_kind != target_kind
        || row.target_id != target_id
        || row.secondary_target_id != secondary_target_id
        || audit.change_kind != change_kind
        || audit.prior_generation != prior_generation
        || audit.reason_code.as_deref() != reason_code
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(Some(receipt))
}

fn load_stored_authority_change_pair(
    connection: &Connection,
    change_id: &str,
) -> Result<
    Option<(
        StoredAuthorityChangeReceiptProjection,
        StoredAuthorityAuditProjection,
    )>,
    AuthorityStoreErrorV1,
> {
    let row = connection
        .query_row(
            "SELECT authenticated_principal, request_hash, result_kind, target_kind, \
             target_id, secondary_target_id, resulting_generation, checked_at \
             FROM authority_change_receipts \
             WHERE change_id = ?1",
            [change_id],
            |row| {
                Ok(StoredAuthorityChangeReceiptProjection {
                    authenticated_principal: row.get(0)?,
                    request_hash: row.get(1)?,
                    result_kind: row.get(2)?,
                    target_kind: row.get(3)?,
                    target_id: row.get(4)?,
                    secondary_target_id: row.get(5)?,
                    resulting_generation: row.get(6)?,
                    checked_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some(receipt) = row else {
        return Ok(None);
    };
    let audit = connection
        .query_row(
            "SELECT actor_principal_id, target_kind, target_id, secondary_target_id, \
             change_kind, prior_generation, resulting_generation, checked_at, reason_code, \
             request_hash FROM authority_audit \
             WHERE change_id = ?1",
            [change_id],
            |row| {
                Ok(StoredAuthorityAuditProjection {
                    actor_principal_id: row.get(0)?,
                    target_kind: row.get(1)?,
                    target_id: row.get(2)?,
                    secondary_target_id: row.get(3)?,
                    change_kind: row.get(4)?,
                    prior_generation: row.get(5)?,
                    resulting_generation: row.get(6)?,
                    checked_at: row.get(7)?,
                    reason_code: row.get(8)?,
                    request_hash: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some((receipt, audit)))
}

#[allow(clippy::too_many_lines)]
fn validate_stored_authority_change_pair(
    receipt: &StoredAuthorityChangeReceiptProjection,
    audit: &StoredAuthorityAuditProjection,
) -> Result<(), AuthorityStoreErrorV1> {
    if let Some(actor) = &receipt.authenticated_principal {
        parse_authority_text::<PrincipalId>(actor)?;
    }
    if receipt.request_hash.len() != 32
        || audit.request_hash != receipt.request_hash
        || audit.actor_principal_id != receipt.authenticated_principal
        || audit.target_kind != receipt.target_kind
        || audit.target_id != receipt.target_id
        || audit.secondary_target_id != receipt.secondary_target_id
        || audit.resulting_generation != receipt.resulting_generation
        || audit.checked_at != receipt.checked_at
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    parse_authority_text::<AuthorityCheckedAt>(&receipt.checked_at)?;
    let generation = parse_safe_authority_counter(receipt.resulting_generation)?;
    let prior = audit
        .prior_generation
        .map(parse_safe_authority_counter)
        .transpose()?;
    let result = authority_change_result_from_storage(&receipt.result_kind)?;
    let expected = match result {
        AuthorityChangeResultV1::AuthorityBootstrapped => {
            ("bootstrap", "bootstrap_authority", None, false, false, true)
        }
        AuthorityChangeResultV1::PrincipalCreated => {
            ("principal", "create_principal", None, false, true, false)
        }
        AuthorityChangeResultV1::CapabilityRegistered => (
            "capability",
            "register_capability",
            None,
            false,
            true,
            false,
        ),
        AuthorityChangeResultV1::RunnerRegistered => {
            ("runner", "register_runner", None, false, true, false)
        }
        AuthorityChangeResultV1::CapabilityNarrowed => (
            "capability",
            "narrow_capability",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::CapabilityRevoked => (
            "capability",
            "revoke_capability",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::PrincipalStatusChanged => (
            "principal",
            "set_principal_status",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
        AuthorityChangeResultV1::RunnerRevoked => (
            "runner",
            "revoke_runner",
            generation.checked_sub(1),
            true,
            true,
            false,
        ),
    };
    if receipt.target_kind != expected.0
        || audit.change_kind != expected.1
        || prior != expected.2
        || audit.reason_code.is_some() != expected.3
        || receipt.authenticated_principal.is_some() != expected.4
        || receipt.secondary_target_id.is_some() != expected.5
        || (prior.is_none() && generation != 1)
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    match receipt.target_kind.as_str() {
        "principal" => {
            parse_authority_text::<PrincipalId>(&receipt.target_id)?;
        }
        "capability" => {
            parse_authority_text::<CapabilityId>(&receipt.target_id)?;
        }
        "runner" => {
            parse_authority_text::<RunnerId>(&receipt.target_id)?;
        }
        "bootstrap" => {
            parse_authority_text::<PrincipalId>(&receipt.target_id)?;
            parse_authority_text::<CapabilityId>(
                receipt
                    .secondary_target_id
                    .as_deref()
                    .ok_or(AuthorityStoreErrorV1::Corrupt)?,
            )?;
        }
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    }
    if let Some(reason) = &audit.reason_code {
        AuthorityReasonCodeV1::new(reason.clone()).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn persist_validated_authority_change(
    transaction: &Transaction<'_>,
    change: &PreparedAuthorityChangeV1,
    validated: &ValidatedAuthorityChangeV1,
) -> Result<(), AuthorityStoreErrorV1> {
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { .. } => {
            let principal = validated
                .principal()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            insert_principal(transaction, principal)?;
        }
        AuthorityChangeV1::SetPrincipalStatus {
            principal_id,
            expected_generation,
            ..
        } => {
            let principal = validated
                .principal()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            let changed = transaction
                .execute(
                    "UPDATE principals SET authority_status = ?1, principal_generation = ?2 \
                     WHERE principal_id = ?3 AND principal_generation = ?4",
                    params![
                        principal_authority_status(principal.status()),
                        to_i64_authority(principal.generation().get())?,
                        principal_id.as_str(),
                        to_i64_authority(expected_generation.get())?,
                    ],
                )
                .map_err(authority_write_failure)?;
            if changed != 1 {
                return Err(AuthorityStoreErrorV1::StaleGeneration);
            }
        }
        AuthorityChangeV1::RegisterCapability { .. } => {
            let capability = validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            insert_capability(transaction, capability)?;
        }
        AuthorityChangeV1::RegisterRunner { .. } => {
            let runner = validated.runner().ok_or(AuthorityStoreErrorV1::Corrupt)?;
            transaction
                .execute(
                    "INSERT INTO runners(\
                     runner_id, owner_principal_id, authority_status, runner_generation\
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        runner.runner_id().as_str(),
                        runner.owner_principal_id().as_str(),
                        runner_authority_status(runner.status()),
                        to_i64_authority(runner.generation().get())?,
                    ],
                )
                .map_err(authority_write_failure)?;
        }
        AuthorityChangeV1::NarrowCapability {
            capability_id,
            expected_generation,
            ..
        } => {
            let capability = validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            update_capability_mutable_facts(
                transaction,
                capability_id,
                expected_generation.get(),
                capability,
            )?;
            transaction
                .execute(
                    "DELETE FROM capability_scopes WHERE capability_id = ?1",
                    [capability_id.as_str()],
                )
                .map_err(authority_write_failure)?;
            insert_capability_scopes(transaction, capability)?;
        }
        AuthorityChangeV1::RevokeCapability {
            capability_id,
            expected_generation,
            ..
        } => {
            let capability = validated
                .capability()
                .ok_or(AuthorityStoreErrorV1::Corrupt)?;
            update_capability_mutable_facts(
                transaction,
                capability_id,
                expected_generation.get(),
                capability,
            )?;
        }
        AuthorityChangeV1::RevokeRunner {
            runner_id,
            expected_generation,
            ..
        } => {
            let runner = validated.runner().ok_or(AuthorityStoreErrorV1::Corrupt)?;
            if runner.runner_id() != runner_id {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            let changed = transaction
                .execute(
                    "UPDATE runners SET authority_status = ?1, runner_generation = ?2 \
                     WHERE runner_id = ?3 AND runner_generation = ?4",
                    params![
                        runner_authority_status(runner.status()),
                        to_i64_authority(runner.generation().get())?,
                        runner_id.as_str(),
                        to_i64_authority(expected_generation.get())?,
                    ],
                )
                .map_err(authority_write_failure)?;
            if changed != 1 {
                return Err(AuthorityStoreErrorV1::StaleGeneration);
            }
        }
    }
    Ok(())
}

fn persist_validated_authority_bootstrap(
    transaction: &Transaction<'_>,
    validated: &ValidatedAuthorityBootstrapV1,
) -> Result<(), AuthorityStoreErrorV1> {
    insert_principal(transaction, validated.principal())?;
    insert_capability(transaction, validated.capability())
}

fn insert_principal(
    transaction: &Transaction<'_>,
    principal: &PrincipalAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    transaction
        .execute(
            "INSERT INTO principals(\
             principal_id, principal_kind, authority_status, principal_generation\
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                principal.principal_id().as_str(),
                principal_kind(principal.kind()),
                principal_authority_status(principal.status()),
                to_i64_authority(principal.generation().get())?,
            ],
        )
        .map_err(authority_write_failure)?;
    Ok(())
}

fn insert_capability(
    transaction: &Transaction<'_>,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (profile_kind, target_room_id, target_member_id, runner_id) =
        capability_profile_storage(capability.profile());
    transaction
        .execute(
            "INSERT INTO capabilities(\
             capability_id, token_hash, principal_id, profile_kind, target_room_id, \
             target_member_id, runner_id, authority_generation, expires_at, revoked_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                capability.capability_id().as_str(),
                capability.token_hash().storage_bytes().as_slice(),
                capability.principal_id().as_str(),
                profile_kind,
                target_room_id,
                target_member_id,
                runner_id,
                to_i64_authority(capability.generation().get())?,
                capability.expires_at().map(CapabilityExpiresAt::as_str),
                capability.revoked_at().map(CapabilityRevokedAt::as_str),
            ],
        )
        .map_err(authority_write_failure)?;
    insert_capability_scopes(transaction, capability)?;
    if let CapabilityProfileV1::RunnerControl {
        permitted_memberships,
        ..
    } = capability.profile()
    {
        for membership in permitted_memberships.iter() {
            transaction
                .execute(
                    "INSERT INTO runner_capability_memberships(\
                     capability_id, room_id, member_id) VALUES (?1, ?2, ?3)",
                    params![
                        capability.capability_id().as_str(),
                        membership.room_id.as_str(),
                        membership.member_id.as_str(),
                    ],
                )
                .map_err(authority_write_failure)?;
        }
    }
    Ok(())
}

fn insert_capability_scopes(
    transaction: &Transaction<'_>,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    for scope in capability.scopes().iter() {
        transaction
            .execute(
                "INSERT INTO capability_scopes(capability_id, scope) VALUES (?1, ?2)",
                params![capability.capability_id().as_str(), capability_scope(scope),],
            )
            .map_err(authority_write_failure)?;
    }
    Ok(())
}

fn update_capability_mutable_facts(
    transaction: &Transaction<'_>,
    capability_id: &CapabilityId,
    expected_generation: u64,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    if capability.capability_id() != capability_id {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let changed = transaction
        .execute(
            "UPDATE capabilities SET authority_generation = ?1, expires_at = ?2, \
             revoked_at = ?3 WHERE capability_id = ?4 AND authority_generation = ?5",
            params![
                to_i64_authority(capability.generation().get())?,
                capability.expires_at().map(CapabilityExpiresAt::as_str),
                capability.revoked_at().map(CapabilityRevokedAt::as_str),
                capability_id.as_str(),
                to_i64_authority(expected_generation)?,
            ],
        )
        .map_err(authority_write_failure)?;
    if changed != 1 {
        return Err(AuthorityStoreErrorV1::StaleGeneration);
    }
    Ok(())
}

fn insert_authority_change_receipt(
    transaction: &Transaction<'_>,
    change: &PreparedAuthorityChangeV1,
    receipt: &AuthorityChangeReceiptV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (target_kind, target_id, secondary_target_id) =
        authority_change_target_storage(receipt.target());
    if secondary_target_id.is_some() {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let (change_kind, prior_generation, reason_code) = authority_change_audit_facts(change);
    insert_authority_receipt_rows(
        transaction,
        Some(change.actor_principal_id().as_str()),
        receipt,
        target_kind,
        &target_id,
        None,
        change_kind,
        prior_generation,
        reason_code,
    )
}

fn insert_authority_bootstrap_receipt(
    transaction: &Transaction<'_>,
    receipt: &AuthorityChangeReceiptV1,
) -> Result<(), AuthorityStoreErrorV1> {
    let (target_kind, target_id, secondary_target_id) =
        authority_change_target_storage(receipt.target());
    let secondary_target_id = secondary_target_id.ok_or(AuthorityStoreErrorV1::Corrupt)?;
    insert_authority_receipt_rows(
        transaction,
        None,
        receipt,
        target_kind,
        &target_id,
        Some(&secondary_target_id),
        "bootstrap_authority",
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_authority_receipt_rows(
    transaction: &Transaction<'_>,
    actor_principal_id: Option<&str>,
    receipt: &AuthorityChangeReceiptV1,
    target_kind: &str,
    target_id: &str,
    secondary_target_id: Option<&str>,
    change_kind: &str,
    prior_generation: Option<u64>,
    reason_code: Option<&str>,
) -> Result<(), AuthorityStoreErrorV1> {
    transaction
        .execute(
            "INSERT INTO authority_change_receipts(\
             change_id, authenticated_principal, request_hash, result_kind, target_kind, \
             target_id, secondary_target_id, resulting_generation, checked_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                receipt.change_id().as_str(),
                actor_principal_id,
                receipt.request_hash().as_bytes().as_slice(),
                authority_change_result_storage(receipt.result()),
                target_kind,
                target_id,
                secondary_target_id,
                to_i64_authority(receipt.resulting_generation())?,
                receipt.changed_at().as_str(),
            ],
        )
        .map_err(authority_write_failure)?;
    transaction
        .execute(
            "INSERT INTO authority_audit(\
             change_id, actor_principal_id, target_kind, target_id, secondary_target_id, \
             change_kind, prior_generation, resulting_generation, checked_at, reason_code, \
             request_hash\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                receipt.change_id().as_str(),
                actor_principal_id,
                target_kind,
                target_id,
                secondary_target_id,
                change_kind,
                prior_generation.map(to_i64_authority).transpose()?,
                to_i64_authority(receipt.resulting_generation())?,
                receipt.changed_at().as_str(),
                reason_code,
                receipt.request_hash().as_bytes().as_slice(),
            ],
        )
        .map_err(authority_write_failure)?;
    Ok(())
}

fn capability_profile_storage(
    profile: &CapabilityProfileV1,
) -> (&'static str, Option<String>, Option<String>, Option<String>) {
    match profile {
        CapabilityProfileV1::RoomMember { room_id, member_id } => (
            "room_member",
            Some(room_id.to_string()),
            Some(member_id.to_string()),
            None,
        ),
        CapabilityProfileV1::HostOperator { room_id } => (
            "host_operator",
            room_id.as_ref().map(ToString::to_string),
            None,
            None,
        ),
        CapabilityProfileV1::RunnerControl { runner_id, .. } => {
            ("runner_control", None, None, Some(runner_id.to_string()))
        }
    }
}

const fn capability_scope(scope: CapabilityScopeV1) -> &'static str {
    match scope {
        CapabilityScopeV1::RoomAttach => "room:attach",
        CapabilityScopeV1::RoomAct => "room:act",
        CapabilityScopeV1::RoomObservePublic => "room:observe_public",
        CapabilityScopeV1::RoomObserveMember => "room:observe_member",
        CapabilityScopeV1::RoomReplay => "room:replay",
        CapabilityScopeV1::ActivationOfferReceive => "activation:offer_receive",
        CapabilityScopeV1::ActivationClaim => "activation:claim",
        CapabilityScopeV1::ActivationComplete => "activation:complete",
        CapabilityScopeV1::OperatorRoomAdmin => "operator:room_admin",
        CapabilityScopeV1::OperatorBackup => "operator:backup",
    }
}

const fn principal_authority_status(status: PrincipalAuthorityStatusV1) -> &'static str {
    match status {
        PrincipalAuthorityStatusV1::Enabled => "enabled",
        PrincipalAuthorityStatusV1::Disabled => "disabled",
    }
}

const fn runner_authority_status(status: RunnerAuthorityStatusV1) -> &'static str {
    match status {
        RunnerAuthorityStatusV1::Enabled => "enabled",
        RunnerAuthorityStatusV1::Revoked => "revoked",
    }
}

const fn authority_change_result_storage(result: AuthorityChangeResultV1) -> &'static str {
    match result {
        AuthorityChangeResultV1::AuthorityBootstrapped => "authority_bootstrapped",
        AuthorityChangeResultV1::PrincipalCreated => "principal_created",
        AuthorityChangeResultV1::CapabilityRegistered => "capability_registered",
        AuthorityChangeResultV1::CapabilityNarrowed => "capability_narrowed",
        AuthorityChangeResultV1::CapabilityRevoked => "capability_revoked",
        AuthorityChangeResultV1::PrincipalStatusChanged => "principal_status_changed",
        AuthorityChangeResultV1::RunnerRegistered => "runner_registered",
        AuthorityChangeResultV1::RunnerRevoked => "runner_revoked",
    }
}

fn authority_change_result_from_storage(
    value: &str,
) -> Result<AuthorityChangeResultV1, AuthorityStoreErrorV1> {
    match value {
        "authority_bootstrapped" => Ok(AuthorityChangeResultV1::AuthorityBootstrapped),
        "principal_created" => Ok(AuthorityChangeResultV1::PrincipalCreated),
        "capability_registered" => Ok(AuthorityChangeResultV1::CapabilityRegistered),
        "capability_narrowed" => Ok(AuthorityChangeResultV1::CapabilityNarrowed),
        "capability_revoked" => Ok(AuthorityChangeResultV1::CapabilityRevoked),
        "principal_status_changed" => Ok(AuthorityChangeResultV1::PrincipalStatusChanged),
        "runner_registered" => Ok(AuthorityChangeResultV1::RunnerRegistered),
        "runner_revoked" => Ok(AuthorityChangeResultV1::RunnerRevoked),
        _ => Err(AuthorityStoreErrorV1::Corrupt),
    }
}

fn authority_change_target_storage(
    target: &AuthorityChangeTargetV1,
) -> (&'static str, String, Option<String>) {
    match target {
        AuthorityChangeTargetV1::Bootstrap {
            principal_id,
            capability_id,
        } => (
            "bootstrap",
            principal_id.to_string(),
            Some(capability_id.to_string()),
        ),
        AuthorityChangeTargetV1::Principal(principal_id) => {
            ("principal", principal_id.to_string(), None)
        }
        AuthorityChangeTargetV1::Capability(capability_id) => {
            ("capability", capability_id.to_string(), None)
        }
        AuthorityChangeTargetV1::Runner(runner_id) => ("runner", runner_id.to_string(), None),
    }
}

fn authority_change_audit_facts(
    change: &PreparedAuthorityChangeV1,
) -> (&'static str, Option<u64>, Option<&str>) {
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { .. } => ("create_principal", None, None),
        AuthorityChangeV1::RegisterCapability { .. } => ("register_capability", None, None),
        AuthorityChangeV1::RegisterRunner { .. } => ("register_runner", None, None),
        AuthorityChangeV1::NarrowCapability {
            expected_generation,
            reason_code,
            ..
        } => (
            "narrow_capability",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::RevokeCapability {
            expected_generation,
            reason_code,
            ..
        } => (
            "revoke_capability",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::RevokeRunner {
            expected_generation,
            reason_code,
            ..
        } => (
            "revoke_runner",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
        AuthorityChangeV1::SetPrincipalStatus {
            expected_generation,
            reason_code,
            ..
        } => (
            "set_principal_status",
            Some(expected_generation.get()),
            Some(reason_code.as_str()),
        ),
    }
}

fn to_i64_authority(value: u64) -> Result<i64, AuthorityStoreErrorV1> {
    i64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or(AuthorityStoreErrorV1::Corrupt)
}

fn load_authority_snapshot(
    connection: &Connection,
    query: &AuthoritySnapshotQueryV1,
) -> Result<Option<AuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let Some(capability) = load_capability_authority(connection, query.capability_id())? else {
        return Ok(None);
    };
    let principal = load_principal_authority(connection, capability.principal_id())?
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let membership = query
        .membership()
        .map(|key| load_membership_authority(connection, key))
        .transpose()?
        .flatten();
    let runner = query
        .runner_id()
        .map(|runner_id| load_runner_authority(connection, runner_id))
        .transpose()?
        .flatten();
    AuthoritySnapshotV1::new(capability, principal, membership, runner)
        .map(Some)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

#[allow(clippy::too_many_lines)]
fn load_capability_authority(
    connection: &Connection,
    requested_capability_id: &CapabilityId,
) -> Result<Option<CapabilityAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let stored = connection
        .query_row(
            "SELECT capability_id, token_hash, principal_id, profile_kind, target_room_id, \
             target_member_id, runner_id, authority_generation, expires_at, revoked_at \
             FROM capabilities WHERE capability_id = ?1",
            [requested_capability_id.as_str()],
            |row| {
                Ok(StoredCapabilityAuthorityProjection {
                    capability_id: row.get(0)?,
                    token_hash: row.get(1)?,
                    principal_id: row.get(2)?,
                    profile_kind: row.get(3)?,
                    target_room_id: row.get(4)?,
                    target_member_id: row.get(5)?,
                    runner_id: row.get(6)?,
                    authority_generation: row.get(7)?,
                    expires_at: row.get(8)?,
                    revoked_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some(stored) = stored else {
        return Ok(None);
    };

    let capability_id = parse_authority_text::<CapabilityId>(&stored.capability_id)?;
    if &capability_id != requested_capability_id {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let token_hash = CapabilityTokenHashV1::from_bytes(
        stored
            .token_hash
            .try_into()
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
    );
    let principal_id = parse_authority_text::<PrincipalId>(&stored.principal_id)?;
    let generation = parse_authority_generation(stored.authority_generation)?;
    let expires_at = stored
        .expires_at
        .as_deref()
        .map(parse_authority_text::<CapabilityExpiresAt>)
        .transpose()?;
    let revoked_at = stored
        .revoked_at
        .as_deref()
        .map(parse_authority_text::<CapabilityRevokedAt>)
        .transpose()?;
    let scopes = load_capability_scopes(connection, &capability_id)?;
    let runner_memberships = load_runner_capability_memberships(connection, &capability_id)?;
    let profile = match stored.profile_kind.as_str() {
        "room_member" => {
            if stored.runner_id.is_some() || !runner_memberships.is_empty() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::RoomMember {
                room_id: parse_authority_text(
                    stored
                        .target_room_id
                        .as_deref()
                        .ok_or(AuthorityStoreErrorV1::Corrupt)?,
                )?,
                member_id: parse_authority_text(
                    stored
                        .target_member_id
                        .as_deref()
                        .ok_or(AuthorityStoreErrorV1::Corrupt)?,
                )?,
            }
        }
        "host_operator" => {
            if stored.target_member_id.is_some()
                || stored.runner_id.is_some()
                || !runner_memberships.is_empty()
            {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::HostOperator {
                room_id: stored
                    .target_room_id
                    .as_deref()
                    .map(parse_authority_text::<RoomId>)
                    .transpose()?,
            }
        }
        "runner_control" => {
            if stored.target_room_id.is_some() || stored.target_member_id.is_some() {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
            CapabilityProfileV1::RunnerControl {
                runner_id: parse_authority_text(
                    stored
                        .runner_id
                        .as_deref()
                        .ok_or(AuthorityStoreErrorV1::Corrupt)?,
                )?,
                permitted_memberships: RunnerMembershipSetV1::new(runner_memberships)
                    .map_err(|_| AuthorityStoreErrorV1::Corrupt)?,
            }
        }
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
        capability_id,
        token_hash,
        principal_id,
        profile,
        scopes,
        generation,
        expires_at,
        revoked_at,
    })
    .map(Some)
    .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn load_capability_scopes(
    connection: &Connection,
    capability_id: &CapabilityId,
) -> Result<CapabilityScopeSetV1, AuthorityStoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT scope FROM capability_scopes \
             WHERE capability_id = ?1 ORDER BY scope",
        )
        .map_err(authority_sql_failure)?;
    let rows = statement
        .query_map([capability_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(authority_sql_failure)?;
    let mut scopes = Vec::new();
    for row in rows {
        let scope = match row.map_err(authority_sql_failure)?.as_str() {
            "room:attach" => CapabilityScopeV1::RoomAttach,
            "room:act" => CapabilityScopeV1::RoomAct,
            "room:observe_public" => CapabilityScopeV1::RoomObservePublic,
            "room:observe_member" => CapabilityScopeV1::RoomObserveMember,
            "room:replay" => CapabilityScopeV1::RoomReplay,
            "activation:offer_receive" => CapabilityScopeV1::ActivationOfferReceive,
            "activation:claim" => CapabilityScopeV1::ActivationClaim,
            "activation:complete" => CapabilityScopeV1::ActivationComplete,
            "operator:room_admin" => CapabilityScopeV1::OperatorRoomAdmin,
            "operator:backup" => CapabilityScopeV1::OperatorBackup,
            _ => return Err(AuthorityStoreErrorV1::Corrupt),
        };
        scopes.push(scope);
    }
    CapabilityScopeSetV1::new(scopes).map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn load_runner_capability_memberships(
    connection: &Connection,
    capability_id: &CapabilityId,
) -> Result<Vec<worldstream_core::RoomMembershipKeyV1>, AuthorityStoreErrorV1> {
    let mut statement = connection
        .prepare(
            "SELECT room_id, member_id FROM runner_capability_memberships \
             WHERE capability_id = ?1 ORDER BY room_id, member_id",
        )
        .map_err(authority_sql_failure)?;
    let rows = statement
        .query_map([capability_id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(authority_sql_failure)?;
    let mut memberships = Vec::new();
    for row in rows {
        let (room_id, member_id) = row.map_err(authority_sql_failure)?;
        memberships.push(worldstream_core::RoomMembershipKeyV1 {
            room_id: parse_authority_text(&room_id)?,
            member_id: parse_authority_text(&member_id)?,
        });
    }
    Ok(memberships)
}

fn load_principal_authority(
    connection: &Connection,
    principal_id: &PrincipalId,
) -> Result<Option<PrincipalAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let row = connection
        .query_row(
            "SELECT principal_kind, authority_status, principal_generation \
             FROM principals WHERE principal_id = ?1",
            [principal_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some((kind, status, generation)) = row else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "human" => PrincipalKindV1::Human,
        "agent" => PrincipalKindV1::Agent,
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    let status = match status.as_str() {
        "enabled" => PrincipalAuthorityStatusV1::Enabled,
        "disabled" => PrincipalAuthorityStatusV1::Disabled,
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    let generation = PrincipalGenerationV1::new(parse_safe_authority_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(PrincipalAuthoritySnapshotV1::new(
        principal_id.clone(),
        kind,
        status,
        generation,
    )))
}

fn load_membership_authority(
    connection: &Connection,
    key: &worldstream_core::RoomMembershipKeyV1,
) -> Result<Option<MembershipAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let current = load_authoritative_current_materialization(connection, &key.room_id)?;
    let Some(current) = current else {
        return Ok(None);
    };
    let authoritative_membership = current
        .memberships()
        .iter()
        .find(|projection| projection.membership.member_id() == &key.member_id);
    let row = connection
        .query_row(
            "SELECT principal_id, principal_kind, standing, access_mode, role, \
             membership_bytes, membership_generation FROM room_members \
             WHERE room_id = ?1 AND member_id = ?2",
            params![key.room_id.as_str(), key.member_id.as_str()],
            |row| {
                Ok(StoredMembershipAuthorityProjection {
                    principal_id: row.get(0)?,
                    principal_kind: row.get(1)?,
                    standing: row.get(2)?,
                    access_mode: row.get(3)?,
                    role: row.get(4)?,
                    membership_bytes: row.get(5)?,
                    membership_generation: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let (authoritative, row) = match (authoritative_membership, row) {
        (None, None) => return Ok(None),
        (Some(authoritative), Some(row)) => (authoritative, row),
        (None, Some(_)) | (Some(_), None) => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    let membership =
        worldstream_core::CanonicalJsonV1::decode_canonical::<MembershipV1>(&row.membership_bytes)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    if membership.member_id() != &key.member_id
        || membership != authoritative.membership
        || row.membership_bytes != authoritative.canonical_membership_bytes
        || row.principal_id != membership.principal_id().as_str()
        || row.principal_kind != principal_kind(membership.principal_kind())
        || row.standing != membership_standing(membership.standing())
        || row.access_mode != access_mode(membership.access_mode())
        || row.role.as_deref() != membership.role()
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let generation =
        MembershipGenerationV1::new(parse_safe_authority_counter(row.membership_generation)?)
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(MembershipAuthoritySnapshotV1::new(
        key.room_id.clone(),
        membership,
        generation,
    )))
}

/// Distinguishes corruption in the Room-serving Membership projection from
/// corruption in independent Principal, Capability, or Runner authority rows.
/// Membership generation is intentionally excluded: it is an operational
/// fence fact, not a canonical Core projection.
fn replay_room_authority_projection_is_corrupt(
    connection: &Connection,
    query: &AuthoritySnapshotQueryV1,
    replay_room_id: &RoomId,
) -> Result<bool, AuthorityStoreErrorV1> {
    let current = match load_authoritative_current_materialization(connection, replay_room_id) {
        Ok(Some(current)) => current,
        Ok(None) => return Ok(false),
        Err(AuthorityStoreErrorV1::Corrupt) => return Ok(true),
        Err(error) => return Err(error),
    };
    let Some(key) = query.membership() else {
        return Ok(false);
    };
    if &key.room_id != replay_room_id {
        return Ok(false);
    }
    let authoritative = current
        .memberships()
        .iter()
        .find(|projection| projection.membership.member_id() == &key.member_id);
    let row = connection
        .query_row(
            "SELECT principal_id, principal_kind, standing, access_mode, role, \
             membership_bytes, membership_generation FROM room_members \
             WHERE room_id = ?1 AND member_id = ?2",
            params![key.room_id.as_str(), key.member_id.as_str()],
            |row| {
                Ok(StoredMembershipAuthorityProjection {
                    principal_id: row.get(0)?,
                    principal_kind: row.get(1)?,
                    standing: row.get(2)?,
                    access_mode: row.get(3)?,
                    role: row.get(4)?,
                    membership_bytes: row.get(5)?,
                    membership_generation: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let (authoritative, row) = match (authoritative, row) {
        (Some(authoritative), Some(row)) => (authoritative, row),
        (None, None) => return Ok(false),
        (None, Some(_)) | (Some(_), None) => return Ok(true),
    };
    let Ok(membership) =
        worldstream_core::CanonicalJsonV1::decode_canonical::<MembershipV1>(&row.membership_bytes)
    else {
        return Ok(true);
    };
    Ok(membership.member_id() != &key.member_id
        || membership != authoritative.membership
        || row.membership_bytes != authoritative.canonical_membership_bytes
        || row.principal_id != membership.principal_id().as_str()
        || row.principal_kind != principal_kind(membership.principal_kind())
        || row.standing != membership_standing(membership.standing())
        || row.access_mode != access_mode(membership.access_mode())
        || row.role.as_deref() != membership.role())
}

#[allow(clippy::too_many_lines)]
fn load_authoritative_current_materialization(
    connection: &Connection,
    room_id: &RoomId,
) -> Result<Option<VerifiedCurrentRoomMaterializationV1>, AuthorityStoreErrorV1> {
    let room_id_text = room_id.to_string();
    let stored = connection
        .query_row(
            "SELECT room_status, room_seq, genesis_or_transition_hash, core_schema_version, \
             pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, \
             complete_head_bytes FROM rooms WHERE room_id = ?1",
            [&room_id_text],
            |row| {
                Ok(StoredHeadProjection {
                    room_status: row.get(0)?,
                    room_seq: row.get(1)?,
                    lineage_hash: row.get(2)?,
                    core_schema_version: row.get(3)?,
                    pack_digest: row.get(4)?,
                    core_state_hash: row.get(5)?,
                    activity_state_hash: row.get(6)?,
                    authoritative_state_hash: row.get(7)?,
                    canonical_bytes: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let head =
        decode_stored_head_projection(&stored, room_id).map_err(map_recovery_authority_error)?;
    let integrity: Option<(String, i64)> = connection
        .query_row(
            "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
            [&room_id_text],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some((integrity_status, integrity_generation)) = integrity else {
        return Err(AuthorityStoreErrorV1::Corrupt);
    };
    if !matches!(
        integrity_status.as_str(),
        "healthy" | "faulted" | "quarantined"
    ) || IntegrityGenerationV1::new(parse_safe_authority_counter(integrity_generation)?).is_err()
    {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let materializations: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT core_state_bytes, activity_state_bytes FROM room_materializations \
             WHERE room_id = ?1",
            [&room_id_text],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let (canonical_core_state_bytes, canonical_activity_state_bytes) =
        materializations.ok_or(AuthorityStoreErrorV1::Corrupt)?;

    let current_transition: Option<CurrentTransitionProjection> = if head.room_seq().get() == 0 {
        None
    } else {
        Some(
            connection
                .query_row(
                    "SELECT transition_hash, previous_lineage_hash, core_schema_version, \
                     pack_digest, core_state_hash, activity_state_hash, \
                     authoritative_state_hash, transition_bytes FROM transitions \
                     WHERE room_id = ?1 AND room_seq = ?2",
                    params![
                        room_id_text,
                        i64::try_from(head.room_seq().get())
                            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?
                    ],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                        ))
                    },
                )
                .optional()
                .map_err(authority_sql_failure)?
                .ok_or(AuthorityStoreErrorV1::Corrupt)?,
        )
    };
    let canonical_current_record_bytes = if let Some(current) = &current_transition {
        current.7.clone()
    } else {
        connection
            .query_row(
                "SELECT genesis_bytes FROM room_genesis WHERE room_id = ?1",
                [&room_id_text],
                |row| row.get(0),
            )
            .optional()
            .map_err(authority_sql_failure)?
            .ok_or(AuthorityStoreErrorV1::Corrupt)?
    };
    let verified = VerifiedCurrentRoomMaterializationV1::verify_for_storage(
        &head,
        &canonical_current_record_bytes,
        &canonical_core_state_bytes,
        &canonical_activity_state_bytes,
    )
    .map_err(map_recovery_authority_error)?;
    if stored.room_status != room_status(verified.room_status()) {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    match (current_transition, verified.previous_lineage_hash()) {
        (None, None) => {}
        (Some(current), Some(previous_lineage_hash)) => {
            let predecessor_head =
                load_verified_immediate_predecessor(connection, room_id, head.room_seq())?;
            let expected_successor = predecessor_head
                .room_seq()
                .checked_successor()
                .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
            if current.0 != head.genesis_or_transition_hash().to_string()
                || current.1 != previous_lineage_hash.to_string()
                || current.2 != head.core_schema_version()
                || current.3 != head.pack_digest().to_string()
                || current.4 != head.core_state_hash().to_string()
                || current.5 != head.activity_state_hash().to_string()
                || current.6 != head.authoritative_state_hash().to_string()
                || predecessor_head.room_id() != room_id
                || head.room_seq() != expected_successor
                || predecessor_head.core_schema_version() != head.core_schema_version()
                || predecessor_head.pack_digest() != head.pack_digest()
                || predecessor_head.genesis_or_transition_hash() != previous_lineage_hash
            {
                return Err(AuthorityStoreErrorV1::Corrupt);
            }
        }
        (None, Some(_)) | (Some(_), None) => return Err(AuthorityStoreErrorV1::Corrupt),
    }
    Ok(Some(verified))
}

fn load_verified_immediate_predecessor(
    connection: &Connection,
    room_id: &RoomId,
    current_sequence: RoomSequenceV1,
) -> Result<CompleteHeadV1, AuthorityStoreErrorV1> {
    let predecessor = current_sequence
        .get()
        .checked_sub(1)
        .ok_or(AuthorityStoreErrorV1::Corrupt)?;
    if predecessor == 0 {
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT genesis_bytes FROM room_genesis WHERE room_id = ?1",
                [room_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(authority_sql_failure)?
            .ok_or(AuthorityStoreErrorV1::Corrupt)?;
        let genesis =
            GenesisV1::from_canonical_bytes(&bytes).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        let predecessor_head = genesis.complete_head();
        if predecessor_head.room_id() != room_id || predecessor_head.room_seq().get() != 0 {
            return Err(AuthorityStoreErrorV1::Corrupt);
        }
        let core_bytes = genesis
            .initial_core_state()
            .canonical_bytes()
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        let activity_bytes = genesis
            .initial_activity_state()
            .to_bytes()
            .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
        VerifiedCurrentRoomMaterializationV1::verify_for_storage(
            &predecessor_head,
            &bytes,
            &core_bytes,
            &activity_bytes,
        )
        .map_err(map_recovery_authority_error)?;
        return Ok(predecessor_head);
    }
    let row: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT transition_hash, transition_bytes FROM transitions \
             WHERE room_id = ?1 AND room_seq = ?2",
            params![
                room_id.as_str(),
                i64::try_from(predecessor).map_err(|_| AuthorityStoreErrorV1::Corrupt)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let (stored_hash, bytes) = row.ok_or(AuthorityStoreErrorV1::Corrupt)?;
    let transition =
        TransitionV1::from_canonical_bytes(&bytes).map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let predecessor_head = transition.complete_head();
    if predecessor_head.room_id() != room_id || predecessor_head.room_seq().get() != predecessor {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    let core_bytes = transition
        .resulting_core_state()
        .canonical_bytes()
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    let activity_bytes = transition
        .resulting_activity_state()
        .to_bytes()
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    VerifiedCurrentRoomMaterializationV1::verify_for_storage(
        &predecessor_head,
        &bytes,
        &core_bytes,
        &activity_bytes,
    )
    .map_err(map_recovery_authority_error)?;
    if stored_hash != predecessor_head.genesis_or_transition_hash().to_string() {
        return Err(AuthorityStoreErrorV1::Corrupt);
    }
    Ok(predecessor_head)
}

const fn map_recovery_authority_error(error: RoomRecoveryErrorV1) -> AuthorityStoreErrorV1 {
    match error {
        RoomRecoveryErrorV1::StorageUnavailable => AuthorityStoreErrorV1::Unavailable,
        RoomRecoveryErrorV1::Corrupt
        | RoomRecoveryErrorV1::RuntimeUnavailable
        | RoomRecoveryErrorV1::RuntimeFault
        | RoomRecoveryErrorV1::IntegrityUnavailable
        | RoomRecoveryErrorV1::ConcurrentChange => AuthorityStoreErrorV1::Corrupt,
    }
}

fn load_runner_authority(
    connection: &Connection,
    runner_id: &RunnerId,
) -> Result<Option<RunnerAuthoritySnapshotV1>, AuthorityStoreErrorV1> {
    let row = connection
        .query_row(
            "SELECT owner_principal_id, authority_status, runner_generation \
             FROM runners WHERE runner_id = ?1",
            [runner_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(authority_sql_failure)?;
    let Some((owner_principal_id, status, generation)) = row else {
        return Ok(None);
    };
    let owner_principal_id = parse_authority_text(&owner_principal_id)?;
    let status = match status.as_str() {
        "enabled" => RunnerAuthorityStatusV1::Enabled,
        "revoked" => RunnerAuthorityStatusV1::Revoked,
        _ => return Err(AuthorityStoreErrorV1::Corrupt),
    };
    let generation = RunnerGenerationV1::new(parse_safe_authority_counter(generation)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)?;
    Ok(Some(RunnerAuthoritySnapshotV1::new(
        runner_id.clone(),
        owner_principal_id,
        status,
        generation,
    )))
}

fn load_authority_change_state(
    connection: &Connection,
    change: &PreparedAuthorityChangeV1,
) -> Result<AuthorityChangeStateV1, AuthorityStoreErrorV1> {
    let mut parts = AuthorityChangeStatePartsV1 {
        principal: None,
        capability: None,
        membership: None,
        runner: None,
        runner_memberships: Vec::new(),
    };
    match change.command() {
        AuthorityChangeV1::CreatePrincipal { principal_id, .. }
        | AuthorityChangeV1::SetPrincipalStatus { principal_id, .. } => {
            parts.principal = load_principal_authority(connection, principal_id)?;
        }
        AuthorityChangeV1::RegisterCapability { capability, .. } => {
            parts.principal = load_principal_authority(connection, capability.principal_id())?;
            parts.capability = load_capability_authority(connection, capability.capability_id())?;
            match capability.profile() {
                CapabilityProfileV1::RoomMember { room_id, member_id } => {
                    let key = worldstream_core::RoomMembershipKeyV1 {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                    };
                    parts.membership = load_membership_authority(connection, &key)?;
                }
                CapabilityProfileV1::HostOperator { .. } => {}
                CapabilityProfileV1::RunnerControl {
                    runner_id,
                    permitted_memberships,
                } => {
                    parts.runner = load_runner_authority(connection, runner_id)?;
                    for key in permitted_memberships.iter() {
                        if let Some(membership) = load_membership_authority(connection, key)? {
                            parts.runner_memberships.push(membership);
                        }
                    }
                }
            }
        }
        AuthorityChangeV1::RegisterRunner {
            runner_id,
            owner_principal_id,
            ..
        } => {
            parts.principal = load_principal_authority(connection, owner_principal_id)?;
            parts.runner = load_runner_authority(connection, runner_id)?;
        }
        AuthorityChangeV1::NarrowCapability { capability_id, .. }
        | AuthorityChangeV1::RevokeCapability { capability_id, .. } => {
            parts.capability = load_capability_authority(connection, capability_id)?;
        }
        AuthorityChangeV1::RevokeRunner { runner_id, .. } => {
            parts.runner = load_runner_authority(connection, runner_id)?;
        }
    }
    AuthorityChangeStateV1::new(parts).map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn parse_authority_generation(value: i64) -> Result<AuthorityGenerationV1, AuthorityStoreErrorV1> {
    AuthorityGenerationV1::new(parse_safe_authority_counter(value)?)
        .map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

fn parse_safe_authority_counter(value: i64) -> Result<u64, AuthorityStoreErrorV1> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER as u64)
        .ok_or(AuthorityStoreErrorV1::Corrupt)
}

fn parse_authority_text<T>(value: &str) -> Result<T, AuthorityStoreErrorV1>
where
    T: FromStr,
{
    value.parse().map_err(|_| AuthorityStoreErrorV1::Corrupt)
}

#[allow(clippy::needless_pass_by_value)]
fn authority_sql_failure(error: rusqlite::Error) -> AuthorityStoreErrorV1 {
    match &error {
        rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::InvalidColumnType(..) => AuthorityStoreErrorV1::Corrupt,
        rusqlite::Error::SqliteFailure(inner, _)
            if matches!(
                inner.code,
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
            ) =>
        {
            AuthorityStoreErrorV1::Corrupt
        }
        _ => AuthorityStoreErrorV1::Unavailable,
    }
}

fn authority_write_failure(error: rusqlite::Error) -> AuthorityStoreErrorV1 {
    if error.sqlite_error_code() == Some(ErrorCode::ConstraintViolation) {
        AuthorityStoreErrorV1::Corrupt
    } else {
        authority_sql_failure(error)
    }
}

fn capture_observed_recovery_fence(
    path: &Path,
    requested_room_id: &RoomId,
) -> Result<Option<ObservedRecoveryFence>, SqliteRoomInspectionErrorV1> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(path, flags)?;
    connection.pragma_update(None, "query_only", true)?;
    let room_id = requested_room_id.to_string();
    connection
        .query_row(
            "SELECT r.room_status, r.room_seq, r.genesis_or_transition_hash, \
             r.core_schema_version, r.pack_digest, r.core_state_hash, \
             r.activity_state_hash, r.authoritative_state_hash, r.complete_head_bytes, \
             i.status, i.generation FROM rooms AS r LEFT JOIN room_integrity AS i \
             ON i.room_id = r.room_id WHERE r.room_id = ?1",
            [&room_id],
            |row| {
                Ok(ObservedRecoveryFence {
                    room_id: room_id.clone(),
                    head: StoredHeadProjection {
                        room_status: row.get(0)?,
                        room_seq: row.get(1)?,
                        lineage_hash: row.get(2)?,
                        core_schema_version: row.get(3)?,
                        pack_digest: row.get(4)?,
                        core_state_hash: row.get(5)?,
                        activity_state_hash: row.get(6)?,
                        authoritative_state_hash: row.get(7)?,
                        canonical_bytes: row.get(8)?,
                    },
                    integrity_status: row.get(9)?,
                    integrity_generation: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(SqliteRoomInspectionErrorV1::Sqlite)
}

#[allow(clippy::too_many_lines)]
fn inspect_room_at_path(
    path: &Path,
    requested_room_id: &RoomId,
) -> Result<Option<RoomHistoryInspectionV1>, SqliteRoomInspectionErrorV1> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection = Connection::open_with_flags(path, flags)?;
    connection.pragma_update(None, "query_only", true)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let room_id = requested_room_id.to_string();
    let stored: Option<StoredHeadProjection> = transaction
        .query_row(
            "SELECT room_status, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, \
             core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes \
             FROM rooms WHERE room_id = ?1",
            [&room_id],
            |row| {
                Ok(StoredHeadProjection {
                    room_status: row.get(0)?,
                    room_seq: row.get(1)?,
                    lineage_hash: row.get(2)?,
                    core_schema_version: row.get(3)?,
                    pack_digest: row.get(4)?,
                    core_state_hash: row.get(5)?,
                    activity_state_hash: row.get(6)?,
                    authoritative_state_hash: row.get(7)?,
                    canonical_bytes: row.get(8)?,
                })
            },
        )
        .optional()?;
    let Some(stored) = stored else {
        transaction.commit()?;
        return Ok(None);
    };
    let head = worldstream_core::CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
        &stored.canonical_bytes,
    )
    .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
    let upper_sequence =
        i64::try_from(head.room_seq().get()).map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
    if head.room_id() != requested_room_id
        || stored.room_seq != upper_sequence
        || stored.lineage_hash != head.genesis_or_transition_hash().to_string()
        || stored.core_schema_version != head.core_schema_version()
        || stored.pack_digest != head.pack_digest().to_string()
        || stored.core_state_hash != head.core_state_hash().to_string()
        || stored.activity_state_hash != head.activity_state_hash().to_string()
        || stored.authoritative_state_hash != head.authoritative_state_hash().to_string()
        || head
            .canonical_bytes()
            .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?
            != stored.canonical_bytes
    {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    }
    let integrity: Option<(String, i64)> = transaction
        .query_row(
            "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
            [&room_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let integrity_generation = match integrity {
        Some((status, generation)) if status == "healthy" => {
            let generation =
                u64::try_from(generation).map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
            IntegrityGenerationV1::new(generation)
                .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?
        }
        Some(_) => return Err(SqliteRoomInspectionErrorV1::IntegrityUnavailable),
        None => return Err(SqliteRoomInspectionErrorV1::Corrupt),
    };
    let (canonical_pack_revision_lock_bytes, canonical_genesis_bytes): (Vec<u8>, Vec<u8>) =
        transaction
        .query_row(
            "SELECT pack_revision_lock_bytes, genesis_bytes FROM room_genesis WHERE room_id = ?1",
            [&room_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(SqliteRoomInspectionErrorV1::Corrupt)?;
    PackRevisionLockV1::from_canonical_bytes(
        &canonical_pack_revision_lock_bytes,
        head.pack_digest(),
    )
    .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
    let genesis = GenesisV1::from_canonical_bytes(&canonical_genesis_bytes)
        .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
    let receipts = inspect_room_receipts(&transaction, &room_id)?;
    let genesis_receipts = receipts
        .iter()
        .filter(|receipt| {
            matches!(
                receipt.result(),
                SemanticResultV1::GenesisCreated { room_id, .. }
                    if room_id == requested_room_id
            )
        })
        .collect::<Vec<_>>();
    let [genesis_receipt] = genesis_receipts.as_slice() else {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    };
    if !genesis_receipt_matches_genesis(genesis_receipt, &genesis) {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    }
    let materializations: Option<(Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT core_state_bytes, activity_state_bytes \
             FROM room_materializations WHERE room_id = ?1",
            [&room_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (canonical_core_state_bytes, canonical_activity_state_bytes) = match materializations {
        Some((core_bytes, activity_bytes)) => {
            let core =
                worldstream_core::CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(&core_bytes)
                    .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
            if core
                .canonical_bytes()
                .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?
                != core_bytes
                || room_status(core.room_status()) != stored.room_status
                || worldstream_core::CanonicalJsonV1::from_canonical_bytes(&activity_bytes).is_err()
            {
                return Err(SqliteRoomInspectionErrorV1::Corrupt);
            }
            (Some(core_bytes), Some(activity_bytes))
        }
        None => (None, None),
    };
    let has_transition_above_head: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM transitions WHERE room_id = ?1 AND room_seq > ?2)",
        params![room_id, upper_sequence],
        |row| row.get(0),
    )?;
    if has_transition_above_head {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    }
    let mut lineage_heads = vec![genesis.complete_head()];
    let canonical_transition_bytes = {
        let mut statement = transaction.prepare(
            "SELECT room_seq, transition_id, transition_hash, previous_lineage_hash, \
             core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
             authoritative_state_hash, transition_bytes FROM transitions \
             WHERE room_id = ?1 AND room_seq <= ?2 ORDER BY room_seq",
        )?;
        let rows = statement.query_map(params![room_id, upper_sequence], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Vec<u8>>(9)?,
            ))
        })?;
        let mut transitions = Vec::new();
        let mut expected_sequence = 1_i64;
        for row in rows {
            let (
                sequence,
                transition_id,
                transition_hash,
                previous_lineage_hash,
                core_schema_version,
                pack_digest,
                core_state_hash,
                activity_state_hash,
                authoritative_state_hash,
                bytes,
            ) = row?;
            let sequence_u64 =
                u64::try_from(sequence).map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
            let decoded = TransitionV1::from_canonical_bytes(&bytes)
                .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
            let decoded_head = decoded.complete_head();
            if sequence != expected_sequence
                || decoded.room_seq().get() != sequence_u64
                || decoded_head.room_id() != requested_room_id
                || transition_hash != decoded.transition_hash().to_string()
                || previous_lineage_hash != decoded.previous_lineage_hash().to_string()
                || core_schema_version != decoded_head.core_schema_version()
                || pack_digest != decoded_head.pack_digest().to_string()
                || core_state_hash != decoded.resulting_core_state_hash().to_string()
                || activity_state_hash != decoded.resulting_activity_state_hash().to_string()
                || authoritative_state_hash
                    != decoded.resulting_authoritative_state_hash().to_string()
            {
                return Err(SqliteRoomInspectionErrorV1::Corrupt);
            }
            let transition_receipts = receipts
                .iter()
                .filter(|receipt| {
                    receipt
                        .transition_seq()
                        .is_some_and(|value| value.get() == sequence_u64)
                })
                .collect::<Vec<_>>();
            let [receipt] = transition_receipts.as_slice() else {
                return Err(SqliteRoomInspectionErrorV1::Corrupt);
            };
            match receipt.result() {
                SemanticResultV1::TransitionCommitted {
                    room_id: receipt_room,
                    transition_id: receipt_transition_id,
                    room_seq,
                    complete_head,
                    ..
                } if receipt_room == requested_room_id
                    && room_seq.get() == sequence_u64
                    && receipt_transition_id.to_string() == transition_id
                    && complete_head == &decoded_head
                    && receipt.basis_complete_head() == lineage_heads.last()
                    && receipt_semantic_input_matches_transition(receipt, &decoded) => {}
                _ => return Err(SqliteRoomInspectionErrorV1::Corrupt),
            }
            lineage_heads.push(decoded_head);
            transitions.push(bytes);
            expected_sequence = expected_sequence
                .checked_add(1)
                .ok_or(SqliteRoomInspectionErrorV1::Corrupt)?;
        }
        if i64::try_from(transitions.len()).ok() != Some(upper_sequence) {
            return Err(SqliteRoomInspectionErrorV1::Corrupt);
        }
        transitions
    };
    if receipts
        .iter()
        .filter(|receipt| receipt.transition_seq().is_some())
        .count()
        != canonical_transition_bytes.len()
    {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    }
    if receipts.iter().any(|receipt| match receipt.result() {
        SemanticResultV1::GenesisCreated { .. } => receipt.basis_complete_head().is_some(),
        _ => receipt
            .basis_complete_head()
            .is_none_or(|basis| !lineage_heads.contains(basis)),
    }) {
        return Err(SqliteRoomInspectionErrorV1::Corrupt);
    }
    let recovered = RecoveredRoomMaterializationsV1::preflight_persisted_history_for_storage(
        &head,
        &canonical_genesis_bytes,
        &canonical_transition_bytes,
        canonical_core_state_bytes.as_deref(),
        canonical_activity_state_bytes.as_deref(),
    )
    .map_err(map_recovery_preflight_inspection_error)?;
    verify_inspection_projections(
        &transaction,
        &room_id,
        &stored.room_status,
        &head,
        &recovered,
    )
    .map_err(map_recovery_preflight_inspection_error)?;
    let inspection = RoomHistoryInspectionV1 {
        head,
        integrity_generation,
        canonical_head_bytes: stored.canonical_bytes.clone(),
        canonical_pack_revision_lock_bytes,
        canonical_genesis_bytes,
        canonical_transition_bytes,
        canonical_core_state_bytes,
        canonical_activity_state_bytes,
    };
    transaction.commit()?;
    Ok(Some(inspection))
}

const fn map_recovery_preflight_inspection_error(
    error: RoomRecoveryErrorV1,
) -> SqliteRoomInspectionErrorV1 {
    match error {
        RoomRecoveryErrorV1::Corrupt => SqliteRoomInspectionErrorV1::Corrupt,
        RoomRecoveryErrorV1::IntegrityUnavailable => {
            SqliteRoomInspectionErrorV1::IntegrityUnavailable
        }
        RoomRecoveryErrorV1::StorageUnavailable
        | RoomRecoveryErrorV1::ConcurrentChange
        | RoomRecoveryErrorV1::RuntimeUnavailable
        | RoomRecoveryErrorV1::RuntimeFault => SqliteRoomInspectionErrorV1::Unavailable,
    }
}

fn inspect_room_receipts(
    transaction: &Transaction<'_>,
    room_id: &str,
) -> Result<Vec<StoredSemanticResultV1>, SqliteRoomInspectionErrorV1> {
    let mut statement = transaction.prepare(
        "SELECT room_id, operation_kind, operation_identity_bytes, codec_id, \
         canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
         semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes \
         FROM semantic_receipts WHERE room_id = ?1 \
         ORDER BY operation_kind, operation_identity_bytes",
    )?;
    let rows = statement.query_map([room_id], |row| {
        Ok(StoredReceiptProjection {
            room_id: row.get(0)?,
            operation_kind: row.get(1)?,
            operation_identity_bytes: row.get(2)?,
            codec_id: row.get(3)?,
            canonical_request_hash: row.get(4)?,
            basis_complete_head_bytes: row.get(5)?,
            semantic_input_bytes: row.get(6)?,
            semantic_time_bytes: row.get(7)?,
            resolution_kind: row.get(8)?,
            transition_seq: row.get(9)?,
            stored_resolution_bytes: row.get(10)?,
        })
    })?;
    rows.map(|row| {
        let row = row.map_err(SqliteRoomInspectionErrorV1::Sqlite)?;
        validate_stored_receipt_projection(&row).map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)
    })
    .collect()
}

fn genesis_receipt_matches_genesis(receipt: &StoredSemanticResultV1, genesis: &GenesisV1) -> bool {
    let (
        ReceiptSemanticInputV1::RoomCreation {
            request,
            created_at,
        },
        SemanticResultV1::GenesisCreated {
            room_id,
            initial_member_ids,
            complete_head,
        },
    ) = (receipt.semantic_input(), receipt.result())
    else {
        return false;
    };
    request.pack_digest() == genesis.pack_digest()
        && request.configuration() == genesis.configuration()
        && created_at == genesis.created_at()
        && room_id == genesis.room_id()
        && complete_head == &genesis.complete_head()
        && initial_member_ids.len() == request.ordered_initial_memberships().len()
        && initial_member_ids.len() == genesis.initial_core_state().memberships().len()
        && request
            .ordered_initial_memberships()
            .iter()
            .zip(initial_member_ids)
            .all(|(proposal, member_id)| {
                genesis
                    .initial_core_state()
                    .memberships()
                    .get(member_id)
                    .is_some_and(|membership| proposal.matches_membership(membership))
            })
}

fn receipt_semantic_input_matches_transition(
    receipt: &StoredSemanticResultV1,
    transition: &TransitionV1,
) -> bool {
    match (receipt.semantic_input(), transition.recorded_stimulus()) {
        (
            ReceiptSemanticInputV1::ParticipantAction {
                normalized_action: Some(action),
                ..
            },
            RecordedStimulusV1::ParticipantAction(stimulus),
        ) => action.as_ref() == stimulus,
        (
            ReceiptSemanticInputV1::TimerFired { request },
            RecordedStimulusV1::TimerFired(stimulus),
        ) => {
            request.room_id() == transition.complete_head().room_id()
                && request.timer_id() == &stimulus.timer_id
                && request.generation() == stimulus.generation
                && request.scheduled_for() == &stimulus.scheduled_for
                && request.canonical_payload() == &stimulus.canonical_payload
        }
        (
            ReceiptSemanticInputV1::CoreAdministration { proposal },
            RecordedStimulusV1::CoreProposed(stimulus),
        ) => proposal == stimulus,
        (
            ReceiptSemanticInputV1::ExternalInput { room_id, input },
            RecordedStimulusV1::ExternalInput(stimulus),
        ) => room_id == transition.complete_head().room_id() && input == stimulus,
        _ => false,
    }
}

fn validate_replay_transition_receipt(
    room_id: &RoomId,
    prior_head: &CompleteHeadV1,
    sequence: u64,
    row: &ReplayTransitionPageRowV1,
) -> Result<CompleteHeadV1, SqliteAuthorizedReplayErrorV1> {
    let transition = TransitionV1::from_canonical_bytes(&row.transition_bytes)
        .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
    let complete_head = transition.complete_head();
    if transition.room_seq().get() != sequence
        || complete_head.room_id() != room_id
        || transition.previous_lineage_hash() != prior_head.genesis_or_transition_hash()
        || transition.transition_hash().to_string() != row.transition_hash
        || transition.previous_lineage_hash().to_string() != row.previous_lineage_hash
        || complete_head.core_schema_version() != row.core_schema_version
        || complete_head.pack_digest().to_string() != row.pack_digest
        || transition.resulting_core_state_hash().to_string() != row.core_state_hash
        || transition.resulting_activity_state_hash().to_string() != row.activity_state_hash
        || transition.resulting_authoritative_state_hash().to_string()
            != row.authoritative_state_hash
    {
        return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
    }
    let receipt = validate_stored_receipt_projection(
        row.receipt
            .as_ref()
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?,
    )
    .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
    match receipt.result() {
        SemanticResultV1::TransitionCommitted {
            room_id: receipt_room_id,
            transition_id: receipt_transition_id,
            room_seq,
            previous_lineage_hash,
            complete_head: receipt_head,
        } if receipt_room_id == room_id
            && receipt_transition_id.to_string() == row.transition_id
            && room_seq.get() == sequence
            && previous_lineage_hash == transition.previous_lineage_hash()
            && receipt_head == &complete_head
            && receipt.basis_complete_head() == Some(prior_head)
            && receipt_semantic_input_matches_transition(&receipt, &transition) =>
        {
            Ok(complete_head)
        }
        _ => Err(SqliteAuthorizedReplayErrorV1::Corrupt),
    }
}

fn normalized_path(path: &Path) -> Result<PathBuf, SqliteStoreOpenError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(SqliteStoreOpenError::Path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(SqliteStoreOpenError::UnsafePath);
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        ensure_same_owner(
            &metadata,
            &fs::metadata(parent).map_err(SqliteStoreOpenError::Path)?,
        )?;
        return fs::canonicalize(path).map_err(SqliteStoreOpenError::Path);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(SqliteStoreOpenError::Path)?;
    let file_name = path.file_name().ok_or_else(|| {
        SqliteStoreOpenError::Path(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SQLite path has no file name",
        ))
    })?;
    Ok(parent.join(file_name))
}

#[cfg(unix)]
fn ensure_same_owner(
    file: &fs::Metadata,
    parent: &fs::Metadata,
) -> Result<(), SqliteStoreOpenError> {
    use std::os::unix::fs::MetadataExt;
    if file.uid() != parent.uid() {
        return Err(SqliteStoreOpenError::UnsafePath);
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_same_owner(
    _file: &fs::Metadata,
    _parent: &fs::Metadata,
) -> Result<(), SqliteStoreOpenError> {
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn writer_main(
    path: &Path,
    receiver: Receiver<WriterCommand>,
    startup: mpsc::Sender<Result<(), SqliteStoreOpenError>>,
    clock: &dyn TrustedAuthorityClock,
) {
    let _writer_lock = match acquire_writer_lock(path) {
        Ok(lock) => lock,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let mut connection = match open_writer_connection(path) {
        Ok(connection) => {
            let _ = startup.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    drop(startup);
    #[cfg(test)]
    let mut failpoint = None;
    while let Ok(command) = receiver.recv() {
        match command {
            WriterCommand::ApplyAuthorityBootstrap(bootstrap, reply) => {
                let result = apply_authority_bootstrap(&mut connection, &bootstrap, clock);
                let _ = reply.send(result);
            }
            WriterCommand::ApplyAuthorityChange(change, reply) => {
                let result = apply_authority_change(&mut connection, &change, clock);
                let _ = reply.send(result);
            }
            WriterCommand::Commit(prepared, reply) => {
                #[cfg(test)]
                let resolution = commit_prepared(&mut connection, *prepared, clock, failpoint);
                #[cfg(not(test))]
                let resolution = commit_prepared(&mut connection, *prepared, clock);
                let _ = reply.send(resolution);
            }
            WriterCommand::Resolve {
                identity,
                identity_bytes,
                request_hash,
                reply,
            } => {
                let _ = reply.send(resolve_guarded(
                    &mut connection,
                    &identity,
                    &identity_bytes,
                    &request_hash,
                ));
            }
            WriterCommand::ResolveAuthorized { authority, reply } => {
                let _ = reply.send(resolve_authorized_guarded(
                    &mut connection,
                    authority,
                    clock,
                ));
            }
            WriterCommand::BeginAuthorizedReplay { authority, reply } => {
                let _ = reply.send(begin_authorized_replay(&mut connection, authority, clock));
            }
            WriterCommand::ReleaseAuthorizedReplay {
                session,
                require_operational_fence,
                reply,
            } => {
                let _ = reply.send(release_authorized_replay(
                    &mut connection,
                    *session,
                    require_operational_fence,
                    clock,
                ));
            }
            WriterCommand::RecordReplayDisposition {
                fence,
                disposition,
                reply,
            } => {
                let _ = reply.send(record_replay_disposition(
                    &mut connection,
                    &fence,
                    disposition,
                ));
            }
            WriterCommand::GuardRecoveryInstall {
                room_id,
                expected_head,
                expected_integrity_generation,
                recovered_materializations,
                reply,
            } => {
                let _ = reply.send(guard_recovery_install(
                    &mut connection,
                    &room_id,
                    &expected_head,
                    expected_integrity_generation,
                    &recovered_materializations,
                ));
            }
            WriterCommand::RecordRecoveryFailure {
                room_id,
                expected_head,
                expected_integrity_generation,
                disposition,
                reply,
            } => {
                let _ = reply.send(record_recovery_failure(
                    &mut connection,
                    &room_id,
                    &expected_head,
                    expected_integrity_generation,
                    disposition,
                ));
            }
            WriterCommand::QuarantineObservedRecoveryCorruption { fence, reply } => {
                let _ = reply.send(quarantine_observed_recovery_corruption(
                    &mut connection,
                    &fence,
                ));
            }
            #[cfg(test)]
            WriterCommand::SeedAuthority {
                witness,
                active,
                reply,
            } => {
                let result = seed_authority_fence(&connection, &witness, active)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            #[cfg(test)]
            WriterCommand::SeedAuthoritySnapshot {
                principal,
                capability,
                reply,
            } => {
                let result = seed_authority_snapshot(&mut connection, &principal, &capability)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            #[cfg(test)]
            WriterCommand::SetFailpoint(value, reply) => {
                failpoint = value;
                let _ = reply.send(());
            }
            #[cfg(test)]
            WriterCommand::SetQueryOnly(enabled, reply) => {
                let result = connection
                    .pragma_update(None, "query_only", enabled)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            #[cfg(test)]
            WriterCommand::PauseQueue => pause_writer_queue(),
            WriterCommand::Shutdown => break,
        }
    }
    drop(receiver);
}

fn record_recovery_failure(
    connection: &mut Connection,
    room_id: &RoomId,
    expected_head: &CompleteHeadV1,
    expected_integrity_generation: IntegrityGenerationV1,
    disposition: RecoveryIntegrityDispositionV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let room_id_text = room_id.to_string();
    let Some(current) = load_observed_recovery_fence(&transaction, &room_id_text)? else {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    };
    let expected_generation = i64::try_from(expected_integrity_generation.get())
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    if current.integrity_status.as_deref() != Some("healthy") {
        return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
    }
    if current.integrity_generation != Some(expected_generation) {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }
    let (disposition, current_head_is_corrupt) =
        match decode_stored_head_projection(&current.head, room_id) {
            Ok(current_head) if &current_head != expected_head => {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            }
            Ok(_) => (disposition, false),
            Err(RoomRecoveryErrorV1::Corrupt) => {
                (RecoveryIntegrityDispositionV1::Quarantined, true)
            }
            Err(error) => return Err(error),
        };
    record_integrity_disposition_in_transaction(
        &transaction,
        &room_id_text,
        expected_integrity_generation,
        disposition,
    )?;
    transaction
        .commit()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if current_head_is_corrupt {
        Err(RoomRecoveryErrorV1::Corrupt)
    } else {
        Ok(())
    }
}

fn record_replay_disposition(
    connection: &mut Connection,
    expected: &ReplayIntegrityDispositionFenceV1,
    disposition: RecoveryIntegrityDispositionV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let room_id = expected.room_id.to_string();
    let Some(current) = load_observed_recovery_fence(&transaction, &room_id)? else {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    };
    let current_generation = current
        .integrity_generation
        .and_then(|value| u64::try_from(value).ok())
        .and_then(|value| IntegrityGenerationV1::new(value).ok());
    let current_status = match current.integrity_status.as_deref() {
        Some("healthy") => Some(RoomIntegrityStatusV1::Healthy),
        Some("faulted") => Some(RoomIntegrityStatusV1::Faulted),
        Some("quarantined") => Some(RoomIntegrityStatusV1::Quarantined),
        Some(_) | None => None,
    };
    let (Some(current_status), Some(current_generation)) = (current_status, current_generation)
    else {
        quarantine_corrupt_replay_integrity_in_transaction(&transaction, &current)?;
        transaction
            .commit()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        return Err(RoomRecoveryErrorV1::Corrupt);
    };
    if current_status != expected.status || current_generation != expected.generation {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }
    let current_head = match decode_stored_head_projection(&current.head, &expected.room_id) {
        Ok(head) => head,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            update_replay_integrity_disposition_in_transaction(
                &transaction,
                &room_id,
                expected.status,
                expected.generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            transaction
                .commit()
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(error) => return Err(error),
    };
    let captured_head_is_ancestor = if current_head.room_seq() == expected.head.room_seq() {
        current_head == expected.head
    } else if expected.status == RoomIntegrityStatusV1::Healthy
        && current_head.room_seq().get() > expected.head.room_seq().get()
        && current_head.core_schema_version() == expected.head.core_schema_version()
        && current_head.pack_digest() == expected.head.pack_digest()
    {
        captured_replay_head_is_still_anchored(&transaction, &expected.head)?
    } else {
        false
    };
    if !captured_head_is_ancestor {
        update_replay_integrity_disposition_in_transaction(
            &transaction,
            &room_id,
            expected.status,
            expected.generation,
            RecoveryIntegrityDispositionV1::Quarantined,
        )?;
        transaction
            .commit()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    match (expected.status, disposition) {
        (RoomIntegrityStatusV1::Healthy, _)
        | (RoomIntegrityStatusV1::Faulted, RecoveryIntegrityDispositionV1::Quarantined) => {
            update_replay_integrity_disposition_in_transaction(
                &transaction,
                &room_id,
                expected.status,
                expected.generation,
                disposition,
            )?;
        }
        (RoomIntegrityStatusV1::Faulted, RecoveryIntegrityDispositionV1::Faulted) => {}
        (RoomIntegrityStatusV1::Quarantined, _) => {
            return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
        }
    }
    transaction
        .commit()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
}

fn update_replay_integrity_disposition_in_transaction(
    transaction: &Transaction<'_>,
    room_id: &str,
    current_status: RoomIntegrityStatusV1,
    current_generation: IntegrityGenerationV1,
    disposition: RecoveryIntegrityDispositionV1,
) -> Result<(), RoomRecoveryErrorV1> {
    if current_status == RoomIntegrityStatusV1::Quarantined {
        return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
    }
    let current =
        i64::try_from(current_generation.get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let next = i64::try_from(
        current_generation
            .checked_successor()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .get(),
    )
    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let changed = transaction
        .execute(
            "UPDATE room_integrity SET status = ?1, generation = ?2 \
             WHERE room_id = ?3 AND status = ?4 AND generation = ?5",
            params![
                recovery_integrity_status(disposition),
                next,
                room_id,
                room_integrity_status(current_status),
                current
            ],
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if changed != 1 {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }
    Ok(())
}

const fn room_integrity_status(status: RoomIntegrityStatusV1) -> &'static str {
    match status {
        RoomIntegrityStatusV1::Healthy => "healthy",
        RoomIntegrityStatusV1::Faulted => "faulted",
        RoomIntegrityStatusV1::Quarantined => "quarantined",
    }
}

fn quarantine_observed_recovery_corruption(
    connection: &mut Connection,
    expected: &ObservedRecoveryFence,
) -> Result<(), RoomRecoveryErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let Some(current) = load_observed_recovery_fence(&transaction, &expected.room_id)? else {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    };
    if &current != expected {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }
    match (
        current.integrity_status.as_deref(),
        current.integrity_generation,
    ) {
        (Some("healthy"), Some(generation)) => {
            let generation = u64::try_from(generation)
                .ok()
                .and_then(|value| IntegrityGenerationV1::new(value).ok())
                .ok_or(RoomRecoveryErrorV1::Corrupt)?;
            record_integrity_disposition_in_transaction(
                &transaction,
                &expected.room_id,
                generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
        }
        (None, None) => {
            let inserted = transaction
                .execute(
                    "INSERT INTO room_integrity(room_id, status, generation) \
                     SELECT ?1, 'quarantined', 1 WHERE NOT EXISTS(\
                         SELECT 1 FROM room_integrity WHERE room_id = ?1\
                     )",
                    [&expected.room_id],
                )
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            if inserted != 1 {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            }
        }
        (Some(_), Some(_)) => return Err(RoomRecoveryErrorV1::IntegrityUnavailable),
        _ => return Err(RoomRecoveryErrorV1::Corrupt),
    }
    transaction
        .commit()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
}

fn load_observed_recovery_fence(
    connection: &Connection,
    room_id: &str,
) -> Result<Option<ObservedRecoveryFence>, RoomRecoveryErrorV1> {
    connection
        .query_row(
            "SELECT r.room_status, r.room_seq, r.genesis_or_transition_hash, \
             r.core_schema_version, r.pack_digest, r.core_state_hash, \
             r.activity_state_hash, r.authoritative_state_hash, r.complete_head_bytes, \
             i.status, i.generation FROM rooms AS r LEFT JOIN room_integrity AS i \
             ON i.room_id = r.room_id WHERE r.room_id = ?1",
            [room_id],
            |row| {
                Ok(ObservedRecoveryFence {
                    room_id: room_id.to_owned(),
                    head: StoredHeadProjection {
                        room_status: row.get(0)?,
                        room_seq: row.get(1)?,
                        lineage_hash: row.get(2)?,
                        core_schema_version: row.get(3)?,
                        pack_digest: row.get(4)?,
                        core_state_hash: row.get(5)?,
                        activity_state_hash: row.get(6)?,
                        authoritative_state_hash: row.get(7)?,
                        canonical_bytes: row.get(8)?,
                    },
                    integrity_status: row.get(9)?,
                    integrity_generation: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
}

fn decode_stored_head_projection(
    stored: &StoredHeadProjection,
    room_id: &RoomId,
) -> Result<CompleteHeadV1, RoomRecoveryErrorV1> {
    let head = worldstream_core::CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
        &stored.canonical_bytes,
    )
    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let sequence =
        i64::try_from(head.room_seq().get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    if head.room_id() != room_id
        || stored.room_seq != sequence
        || stored.lineage_hash != head.genesis_or_transition_hash().to_string()
        || stored.core_schema_version != head.core_schema_version()
        || stored.pack_digest != head.pack_digest().to_string()
        || stored.core_state_hash != head.core_state_hash().to_string()
        || stored.activity_state_hash != head.activity_state_hash().to_string()
        || stored.authoritative_state_hash != head.authoritative_state_hash().to_string()
        || head
            .canonical_bytes()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            != stored.canonical_bytes
    {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(head)
}

fn record_integrity_disposition_in_transaction(
    transaction: &Transaction<'_>,
    room_id: &str,
    current_generation: IntegrityGenerationV1,
    disposition: RecoveryIntegrityDispositionV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let current =
        i64::try_from(current_generation.get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let next = i64::try_from(
        current_generation
            .checked_successor()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .get(),
    )
    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let changed = transaction
        .execute(
            "UPDATE room_integrity SET status = ?1, generation = ?2 \
             WHERE room_id = ?3 AND status = 'healthy' AND generation = ?4",
            params![
                recovery_integrity_status(disposition),
                next,
                room_id,
                current
            ],
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if changed != 1 {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }
    Ok(())
}

const fn recovery_integrity_status(disposition: RecoveryIntegrityDispositionV1) -> &'static str {
    match disposition {
        RecoveryIntegrityDispositionV1::Faulted => "faulted",
        RecoveryIntegrityDispositionV1::Quarantined => "quarantined",
    }
}

#[allow(clippy::too_many_lines)]
fn guard_recovery_install(
    connection: &mut Connection,
    room_id: &RoomId,
    expected_head: &CompleteHeadV1,
    expected_integrity_generation: IntegrityGenerationV1,
    recovered_materializations: &RecoveredRoomMaterializationsV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let room_id_text = room_id.to_string();
    let stored: Option<StoredHeadProjection> = transaction
        .query_row(
            "SELECT room_status, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, \
             core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes \
             FROM rooms WHERE room_id = ?1",
            [&room_id_text],
            |row| {
                Ok(StoredHeadProjection {
                    room_status: row.get(0)?,
                    room_seq: row.get(1)?,
                    lineage_hash: row.get(2)?,
                    core_schema_version: row.get(3)?,
                    pack_digest: row.get(4)?,
                    core_state_hash: row.get(5)?,
                    activity_state_hash: row.get(6)?,
                    authoritative_state_hash: row.get(7)?,
                    canonical_bytes: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let Some(stored) = stored else {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    };
    let integrity: Option<(String, i64)> = transaction
        .query_row(
            "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
            [&room_id_text],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let expected_generation = i64::try_from(expected_integrity_generation.get())
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    match integrity.as_ref() {
        Some((status, _)) if status != "healthy" => {
            return Err(RoomRecoveryErrorV1::IntegrityUnavailable);
        }
        Some((_, generation)) if *generation != expected_generation => {
            return Err(RoomRecoveryErrorV1::ConcurrentChange);
        }
        Some(_) => {}
        None => return Err(RoomRecoveryErrorV1::ConcurrentChange),
    }

    let durable_head = match decode_stored_head_projection(&stored, room_id) {
        Ok(head) => head,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            record_integrity_disposition_in_transaction(
                &transaction,
                &room_id_text,
                expected_integrity_generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            transaction
                .commit()
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(error) => return Err(error),
    };
    if &durable_head != expected_head {
        return Err(RoomRecoveryErrorV1::ConcurrentChange);
    }

    let verification = (|| {
        if stored.room_status != room_status(recovered_materializations.room_status()) {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let (rebuild_memberships, rebuild_timers) = verify_recovery_consequences(
            &transaction,
            &room_id_text,
            expected_head,
            recovered_materializations,
        )?;
        let materializations: Option<(Vec<u8>, Vec<u8>)> = transaction
            .query_row(
                "SELECT core_state_bytes, activity_state_bytes \
                 FROM room_materializations WHERE room_id = ?1",
                [&room_id_text],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let rebuild_materializations = match materializations {
            Some((core, activity))
                if core == recovered_materializations.canonical_core_state_bytes()
                    && activity == recovered_materializations.canonical_activity_state_bytes() =>
            {
                false
            }
            Some(_) => return Err(RoomRecoveryErrorV1::Corrupt),
            None => true,
        };
        Ok((
            rebuild_memberships,
            rebuild_timers,
            rebuild_materializations,
        ))
    })();

    let (rebuild_memberships, rebuild_timers, rebuild_materializations) = match verification {
        Ok(plan) => plan,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            record_integrity_disposition_in_transaction(
                &transaction,
                &room_id_text,
                expected_integrity_generation,
                RecoveryIntegrityDispositionV1::Quarantined,
            )?;
            transaction
                .commit()
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        Err(error) => return Err(error),
    };
    if rebuild_memberships {
        rebuild_recovery_memberships(&transaction, &room_id_text, recovered_materializations)?;
    }
    if rebuild_timers {
        rebuild_recovery_timers(&transaction, &room_id_text, recovered_materializations)?;
    }
    if rebuild_materializations {
        transaction
            .execute(
                "INSERT INTO room_materializations(\
                 room_id, core_state_bytes, activity_state_bytes\
                 ) VALUES (?1, ?2, ?3)",
                params![
                    room_id_text,
                    recovered_materializations.canonical_core_state_bytes(),
                    recovered_materializations.canonical_activity_state_bytes(),
                ],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    }
    transaction
        .commit()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)
}

fn verify_recovery_consequences(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(bool, bool), RoomRecoveryErrorV1> {
    let frame_heads = verify_recovery_frames(transaction, room_id, expected_head, recovered)?;
    let rebuild_memberships =
        verify_recovery_memberships(transaction, room_id, recovered, &frame_heads)?;
    let rebuild_timers = verify_recovery_timers(transaction, room_id, recovered)?;
    let activation_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM activation_decisions WHERE room_id = ?1",
            [room_id],
            |row| row.get(0),
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if activation_count != 0 {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok((rebuild_memberships, rebuild_timers))
}

fn verify_inspection_projections(
    transaction: &Transaction<'_>,
    room_id: &str,
    stored_room_status: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(), RoomRecoveryErrorV1> {
    if stored_room_status != room_status(recovered.room_status()) {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let (frame_heads, _) =
        inspect_recovery_frame_structure(transaction, room_id, expected_head, recovered)?;
    let _all_memberships_missing =
        verify_recovery_memberships(transaction, room_id, recovered, &frame_heads)?;
    let _all_timers_missing = verify_recovery_timers(transaction, room_id, recovered)?;
    let activation_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM activation_decisions WHERE room_id = ?1",
            [room_id],
            |row| row.get(0),
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    if activation_count != 0 {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(())
}

fn verify_recovery_frames(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<BTreeMap<String, i64>, RoomRecoveryErrorV1> {
    let expected_frames = recovered
        .observation_frames()
        .iter()
        .map(|frame| {
            let frame_seq =
                i64::try_from(frame.frame_seq()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
            let cause_room_seq = i64::try_from(frame.cause_room_seq().get())
                .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
            Ok((
                (frame.member_id().to_string(), frame_seq),
                (cause_room_seq, frame.payload_hash().to_string()),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, RoomRecoveryErrorV1>>()?;
    let (frame_heads, stored_frames) =
        inspect_recovery_frame_structure(transaction, room_id, expected_head, recovered)?;
    if stored_frames != expected_frames {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(frame_heads)
}

type StoredFrameIndex = BTreeMap<(String, i64), (i64, String)>;

fn inspect_recovery_frame_structure(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(BTreeMap<String, i64>, StoredFrameIndex), RoomRecoveryErrorV1> {
    let mut frame_heads = recovered
        .memberships()
        .iter()
        .map(|member| (member.membership.member_id().to_string(), 0_i64))
        .collect::<BTreeMap<_, _>>();
    let mut statement = transaction
        .prepare(
            "SELECT member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes \
             FROM observation_frames WHERE room_id = ?1 ORDER BY member_id, frame_seq",
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let rows = statement
        .query_map([room_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let maximum_cause =
        i64::try_from(expected_head.room_seq().get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let mut stored_frames = StoredFrameIndex::new();
    for row in rows {
        let (member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes) =
            row.map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let previous = frame_heads
            .get_mut(&member_id)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        if previous.checked_add(1) != Some(frame_seq)
            || cause_room_seq < 1
            || cause_room_seq > maximum_cause
            || worldstream_core::CanonicalJsonV1::from_canonical_bytes(&payload_bytes).is_err()
            || payload_hash != Blake3DigestV1::hash(&payload_bytes).to_string()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        *previous = frame_seq;
        if stored_frames
            .insert((member_id, frame_seq), (cause_room_seq, payload_hash))
            .is_some()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    Ok((frame_heads, stored_frames))
}

#[allow(clippy::too_many_lines)]
fn verify_recovery_memberships(
    transaction: &Transaction<'_>,
    room_id: &str,
    recovered: &RecoveredRoomMaterializationsV1,
    frame_heads: &BTreeMap<String, i64>,
) -> Result<bool, RoomRecoveryErrorV1> {
    type StoredMember = (
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Vec<u8>,
        i64,
        i64,
    );
    let expected_generations = recover_membership_generations(transaction, room_id)?;
    let stored = {
        let mut statement = transaction
            .prepare(
                "SELECT member_id, principal_id, principal_kind, standing, access_mode, role, \
                 membership_bytes, membership_generation, frame_head FROM room_members \
                 WHERE room_id = ?1 ORDER BY member_id",
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let rows = statement
            .query_map([room_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        rows.collect::<Result<Vec<StoredMember>, _>>()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
    };
    if stored.is_empty() && !recovered.memberships().is_empty() {
        return Ok(true);
    }
    if stored.len() != recovered.memberships().len() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    for (row, expected) in stored.iter().zip(recovered.memberships()) {
        let membership = &expected.membership;
        let expected_member_id = membership.member_id().to_string();
        if row.0 != expected_member_id
            || row.1 != membership.principal_id().to_string()
            || row.2 != principal_kind(membership.principal_kind())
            || row.3 != membership_standing(membership.standing())
            || row.4 != access_mode(membership.access_mode())
            || row.5.as_deref() != membership.role()
            || row.6 != expected.canonical_membership_bytes
            || Some(row.7) != expected_generations.get(&expected_member_id).copied()
            || Some(row.8) != frame_heads.get(&expected_member_id).copied()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    Ok(false)
}

fn rebuild_recovery_memberships(
    transaction: &Transaction<'_>,
    room_id: &str,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(), RoomRecoveryErrorV1> {
    let expected_generations = recover_membership_generations(transaction, room_id)?;
    let frame_heads = recovered.observation_frames().iter().fold(
        BTreeMap::<String, i64>::new(),
        |mut heads, frame| {
            if let Ok(frame_seq) = i64::try_from(frame.frame_seq()) {
                heads
                    .entry(frame.member_id().to_string())
                    .and_modify(|head| *head = (*head).max(frame_seq))
                    .or_insert(frame_seq);
            }
            heads
        },
    );
    for member in recovered.memberships() {
        let membership = &member.membership;
        let member_id = membership.member_id().to_string();
        transaction
            .execute(
                "INSERT INTO room_members(\
                 room_id, member_id, principal_id, principal_kind, standing, access_mode, role,\
                 membership_bytes, membership_generation, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    room_id,
                    member_id,
                    membership.principal_id().to_string(),
                    principal_kind(membership.principal_kind()),
                    membership_standing(membership.standing()),
                    access_mode(membership.access_mode()),
                    membership.role(),
                    member.canonical_membership_bytes,
                    expected_generations
                        .get(&member_id)
                        .copied()
                        .ok_or(RoomRecoveryErrorV1::Corrupt)?,
                    frame_heads.get(&member_id).copied().unwrap_or(0),
                ],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    }
    Ok(())
}

fn recover_membership_generations(
    transaction: &Connection,
    room_id: &str,
) -> Result<BTreeMap<String, i64>, RoomRecoveryErrorV1> {
    let genesis_bytes: Vec<u8> = transaction
        .query_row(
            "SELECT genesis_bytes FROM room_genesis WHERE room_id = ?1",
            [room_id],
            |row| row.get(0),
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let genesis = GenesisV1::from_canonical_bytes(&genesis_bytes)
        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    if genesis.room_id().to_string() != room_id {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let mut previous = genesis.initial_core_state().memberships().clone();
    let mut generations = previous
        .keys()
        .map(|member_id| (member_id.clone(), 1_i64))
        .collect::<BTreeMap<_, _>>();
    let mut statement = transaction
        .prepare(
            "SELECT room_seq, transition_bytes FROM transitions \
             WHERE room_id = ?1 ORDER BY room_seq",
        )
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let rows = statement
        .query_map([room_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let mut expected_room_seq = 1_i64;
    for row in rows {
        let (stored_room_seq, transition_bytes) =
            row.map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let transition = TransitionV1::from_canonical_bytes(&transition_bytes)
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
        if stored_room_seq != expected_room_seq
            || i64::try_from(transition.room_seq().get()).ok() != Some(stored_room_seq)
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        let resulting = transition.resulting_core_state().memberships();
        if previous
            .keys()
            .any(|member_id| !resulting.contains_key(member_id))
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        for (member_id, membership) in resulting {
            match previous.get(member_id) {
                Some(before) if before != membership => {
                    let generation = generations
                        .get_mut(member_id)
                        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
                    *generation = generation
                        .checked_add(1)
                        .filter(|value| *value <= MAX_SAFE_INTEGER)
                        .ok_or(RoomRecoveryErrorV1::Corrupt)?;
                }
                Some(_) => {}
                None => {
                    if generations.insert(member_id.clone(), 1).is_some() {
                        return Err(RoomRecoveryErrorV1::Corrupt);
                    }
                }
            }
        }
        previous = resulting.clone();
        expected_room_seq = expected_room_seq
            .checked_add(1)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    }
    Ok(generations
        .into_iter()
        .map(|(member_id, generation)| (member_id.to_string(), generation))
        .collect())
}

fn verify_recovery_timers(
    transaction: &Transaction<'_>,
    room_id: &str,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<bool, RoomRecoveryErrorV1> {
    type StoredTimer = (String, i64, String, Vec<u8>, String);
    let stored = {
        let mut statement = transaction
            .prepare(
                "SELECT timer_id, generation, scheduled_for, payload_bytes, state \
                 FROM timers WHERE room_id = ?1 ORDER BY timer_id, generation",
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let rows = statement
            .query_map([room_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        rows.collect::<Result<Vec<StoredTimer>, _>>()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?
    };
    if stored.is_empty() && !recovered.timers().is_empty() {
        return Ok(true);
    }
    if stored.len() != recovered.timers().len() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    for (row, expected) in stored.iter().zip(recovered.timers()) {
        if row.0 != expected.timer_id().to_string()
            || row.1
                != i64::try_from(expected.generation().get())
                    .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            || row.2 != expected.scheduled_for().as_str()
            || row.3 != expected.canonical_payload_bytes()
            || row.4 != recovered_timer_state(expected.state())
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
    }
    Ok(false)
}

fn rebuild_recovery_timers(
    transaction: &Transaction<'_>,
    room_id: &str,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(), RoomRecoveryErrorV1> {
    for timer in recovered.timers() {
        transaction
            .execute(
                "INSERT INTO timers(\
                 room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    room_id,
                    timer.timer_id().to_string(),
                    i64::try_from(timer.generation().get())
                        .map_err(|_| RoomRecoveryErrorV1::Corrupt)?,
                    timer.scheduled_for().as_str(),
                    timer.canonical_payload_bytes(),
                    recovered_timer_state(timer.state()),
                ],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    }
    Ok(())
}

const fn recovered_timer_state(state: RecoveredTimerStateV1) -> &'static str {
    match state {
        RecoveredTimerStateV1::Scheduled => "scheduled",
        RecoveredTimerStateV1::Fired => "fired",
        RecoveredTimerStateV1::Cancelled => "cancelled",
    }
}

fn acquire_writer_lock(path: &Path) -> Result<File, SqliteStoreOpenError> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(SqliteStoreOpenError::UnsafePath)?;
    let lock_path = path.with_file_name(format!(".{file_name}.worldstream-writer.lock"));
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(SqliteStoreOpenError::UnsafePath);
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(SqliteStoreOpenError::Path)?;
    lock.try_lock()
        .map_err(|_| SqliteStoreOpenError::WriterAlreadyOwned)?;
    Ok(lock)
}

fn open_writer_connection(path: &Path) -> Result<Connection, SqliteStoreOpenError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection =
        Connection::open_with_flags(path, flags).map_err(SqliteStoreOpenError::Sqlite)?;
    secure_database_file(path)?;
    let (version, source_id): (String, String) = connection
        .query_row("SELECT sqlite_version(), sqlite_source_id()", (), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if version != SQLITE_VERSION || source_id != SQLITE_SOURCE_ID {
        return Err(SqliteStoreOpenError::EngineIdentity {
            actual_version: version,
            actual_source_id: source_id,
        });
    }
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(SqliteStoreOpenError::Sqlite)?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(SqliteStoreOpenError::Sqlite)?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", (), |row| row.get(0))
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if journal_mode != "wal" {
        return Err(SqliteStoreOpenError::JournalMode(journal_mode));
    }
    let (foreign_keys, synchronous, busy_timeout): (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT foreign_keys FROM pragma_foreign_keys), \
             (SELECT synchronous FROM pragma_synchronous), \
             (SELECT timeout FROM pragma_busy_timeout)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if foreign_keys != 1 || synchronous != 2 || busy_timeout != 5_000 {
        return Err(SqliteStoreOpenError::PragmaMismatch {
            foreign_keys,
            synchronous,
            busy_timeout,
        });
    }
    migrate(&mut connection)?;
    #[cfg(test)]
    connection
        .execute_batch(
            "CREATE TEMP TABLE conformance_authority_fences (\
             witness_id TEXT PRIMARY KEY,\
             authenticated_principal TEXT NOT NULL,\
             generation INTEGER NOT NULL,\
             scope_revocation_hash BLOB NOT NULL CHECK(length(scope_revocation_hash) = 32),\
             active INTEGER NOT NULL CHECK(active IN (0, 1))\
             ) STRICT;",
        )
        .map_err(SqliteStoreOpenError::Sqlite)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", (), |row| row.get(0))
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if integrity != "ok" {
        return Err(SqliteStoreOpenError::IntegrityCheck(integrity));
    }
    let foreign_key_violation = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(SqliteStoreOpenError::Sqlite)?
        .exists(())
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if foreign_key_violation {
        return Err(SqliteStoreOpenError::ForeignKeyCheck);
    }
    Ok(connection)
}

#[cfg(unix)]
fn secure_database_file(path: &Path) -> Result<(), SqliteStoreOpenError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path).map_err(SqliteStoreOpenError::Path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(SqliteStoreOpenError::UnsafePath);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(SqliteStoreOpenError::Path)
}

#[cfg(not(unix))]
fn secure_database_file(path: &Path) -> Result<(), SqliteStoreOpenError> {
    let metadata = fs::symlink_metadata(path).map_err(SqliteStoreOpenError::Path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(SqliteStoreOpenError::UnsafePath);
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), SqliteStoreOpenError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(SqliteStoreOpenError::Sqlite)?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version INTEGER PRIMARY KEY, migration_id TEXT NOT NULL UNIQUE\
             ) STRICT;",
        )
        .map_err(SqliteStoreOpenError::Sqlite)?;
    let migrations = {
        let mut statement = transaction
            .prepare("SELECT version, migration_id FROM schema_migrations ORDER BY version")
            .map_err(SqliteStoreOpenError::Sqlite)?;
        let rows = statement
            .query_map((), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(SqliteStoreOpenError::Sqlite)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SqliteStoreOpenError::Sqlite)?
    };
    if migrations.len() > 2
        || migrations
            .iter()
            .enumerate()
            .any(|(index, (version, _))| usize::try_from(*version).ok() != Some(index + 1))
    {
        return Err(SqliteStoreOpenError::UnexpectedMigrationSet);
    }
    if let Some((_, found)) = migrations.first()
        && found != INITIAL_MIGRATION_ID
    {
        return Err(SqliteStoreOpenError::MigrationIdentity {
            version: 1,
            expected: INITIAL_MIGRATION_ID,
            found: found.clone(),
        });
    }
    if let Some((_, found)) = migrations.get(1)
        && found != AUTHORITY_MIGRATION_ID
    {
        return Err(SqliteStoreOpenError::MigrationIdentity {
            version: 2,
            expected: AUTHORITY_MIGRATION_ID,
            found: found.clone(),
        });
    }
    if migrations.is_empty() {
        transaction
            .execute_batch(INITIAL_MIGRATION_SCHEMA)
            .map_err(SqliteStoreOpenError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, migration_id) VALUES (1, ?1)",
                [INITIAL_MIGRATION_ID],
            )
            .map_err(SqliteStoreOpenError::Sqlite)?;
    }
    if migrations.len() < 2 {
        transaction
            .execute_batch(AUTHORITY_MIGRATION_SCHEMA)
            .map_err(SqliteStoreOpenError::Sqlite)?;
        backfill_membership_generations_for_v2(&transaction)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, migration_id) VALUES (2, ?1)",
                [AUTHORITY_MIGRATION_ID],
            )
            .map_err(SqliteStoreOpenError::Sqlite)?;
    }
    verify_schema(&transaction)?;
    transaction.commit().map_err(SqliteStoreOpenError::Sqlite)
}

fn backfill_membership_generations_for_v2(
    transaction: &Transaction<'_>,
) -> Result<(), SqliteStoreOpenError> {
    let rooms = {
        let mut statement = transaction
            .prepare("SELECT room_id, room_seq FROM rooms ORDER BY room_id")
            .map_err(SqliteStoreOpenError::Sqlite)?;
        let rows = statement
            .query_map((), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(SqliteStoreOpenError::Sqlite)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SqliteStoreOpenError::Sqlite)?
    };
    for (room_id, room_seq) in rooms {
        let stored_member_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM room_members WHERE room_id = ?1",
                [&room_id],
                |row| row.get(0),
            )
            .map_err(SqliteStoreOpenError::Sqlite)?;
        if stored_member_count == 0 {
            continue;
        }
        let transition_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM transitions WHERE room_id = ?1",
                [&room_id],
                |row| row.get(0),
            )
            .map_err(SqliteStoreOpenError::Sqlite)?;
        if room_seq < 0 || transition_count != room_seq {
            return Err(SqliteStoreOpenError::MigrationData);
        }
        let generations = recover_membership_generations(transaction, &room_id)
            .map_err(|_| SqliteStoreOpenError::MigrationData)?;
        if usize::try_from(stored_member_count).ok() != Some(generations.len()) {
            return Err(SqliteStoreOpenError::MigrationData);
        }
        for (member_id, generation) in generations {
            let changed = transaction
                .execute(
                    "UPDATE room_members SET membership_generation = ?1 \
                     WHERE room_id = ?2 AND member_id = ?3",
                    params![generation, room_id, member_id],
                )
                .map_err(SqliteStoreOpenError::Sqlite)?;
            if changed != 1 {
                return Err(SqliteStoreOpenError::MigrationData);
            }
        }
    }
    Ok(())
}

fn verify_schema(connection: &Connection) -> Result<(), SqliteStoreOpenError> {
    let actual = schema_corpus(connection)?;
    let reference = Connection::open_in_memory().map_err(SqliteStoreOpenError::Sqlite)?;
    reference
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version INTEGER PRIMARY KEY, migration_id TEXT NOT NULL UNIQUE\
             ) STRICT;",
        )
        .map_err(SqliteStoreOpenError::Sqlite)?;
    reference
        .execute_batch(INITIAL_MIGRATION_SCHEMA)
        .map_err(SqliteStoreOpenError::Sqlite)?;
    reference
        .execute_batch(AUTHORITY_MIGRATION_SCHEMA)
        .map_err(SqliteStoreOpenError::Sqlite)?;
    let expected = schema_corpus(&reference)?;
    if actual != expected {
        return Err(SqliteStoreOpenError::SchemaMismatch);
    }
    Ok(())
}

fn schema_corpus(
    connection: &Connection,
) -> Result<Vec<(String, String, String, String)>, SqliteStoreOpenError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name, tbl_name",
        )
        .map_err(SqliteStoreOpenError::Sqlite)?;
    let rows = statement
        .query_map((), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(SqliteStoreOpenError::Sqlite)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(SqliteStoreOpenError::Sqlite)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteBoundary {
    Receipt,
    Room,
    Genesis,
    Materializations,
    #[cfg(test)]
    AfterFirstCreateMember,
    Timers,
    Integrity,
    Transition,
    Head,
    ExistingMaterializations,
    Members,
    ExistingTimers,
    Frames,
    Activation,
    ExistingReceipt,
    #[cfg(test)]
    DriverReadOnlyAfterGuard,
    #[cfg(test)]
    PauseAfterGuard,
    AfterCommitUnknown,
}

#[cfg(not(test))]
fn commit_prepared(
    connection: &mut Connection,
    prepared: SqlitePreparedWrite,
    clock: &dyn TrustedAuthorityClock,
) -> RoomCommitResolutionV1 {
    commit_prepared_inner(connection, prepared, clock, None)
}

#[cfg(test)]
fn commit_prepared(
    connection: &mut Connection,
    prepared: SqlitePreparedWrite,
    clock: &dyn TrustedAuthorityClock,
    failpoint: Option<WriteBoundary>,
) -> RoomCommitResolutionV1 {
    commit_prepared_inner(connection, prepared, clock, failpoint)
}

fn commit_prepared_inner(
    connection: &mut Connection,
    prepared: SqlitePreparedWrite,
    clock: &dyn TrustedAuthorityClock,
    failpoint: Option<WriteBoundary>,
) -> RoomCommitResolutionV1 {
    let Ok(transaction) = connection.transaction_with_behavior(TransactionBehavior::Immediate)
    else {
        return RoomCommitResolutionV1::Indeterminate;
    };
    match lookup_receipt(&transaction, &prepared.identity, &prepared.identity_bytes) {
        Ok(Some(stored)) => {
            let resolution = if stored.canonical_request_hash() == &prepared.request_hash {
                RoomCommitResolutionV1::resolved(ResolutionStatusV1::Existing, stored)
            } else {
                RoomCommitResolutionV1::Conflict {
                    existing_request_hash: stored.canonical_request_hash().clone(),
                }
            };
            return if transaction.commit().is_ok() {
                resolution
            } else {
                RoomCommitResolutionV1::Indeterminate
            };
        }
        Ok(None) => {}
        Err(ReceiptLookupError::Database) => {
            let _ = transaction.rollback();
            return RoomCommitResolutionV1::Indeterminate;
        }
        Err(ReceiptLookupError::Corrupt) => {
            return if transaction.rollback().is_ok() {
                RoomCommitResolutionV1::Fault
            } else {
                RoomCommitResolutionV1::Indeterminate
            };
        }
    }
    #[cfg(test)]
    if failpoint == Some(WriteBoundary::PauseAfterGuard) {
        pause_after_guard();
    }
    #[cfg(test)]
    if failpoint == Some(WriteBoundary::DriverReadOnlyAfterGuard)
        && transaction.pragma_update(None, "query_only", true).is_err()
    {
        return if transaction.rollback().is_ok() {
            RoomCommitResolutionV1::RetryableKnownAbsent
        } else {
            RoomCommitResolutionV1::Indeterminate
        };
    }
    let result = match &prepared.branch {
        PreparedSqliteBranch::Create(persistence) => {
            commit_create(&transaction, &prepared, persistence, clock, failpoint)
        }
        PreparedSqliteBranch::Existing {
            basis,
            integrity_generation,
            input_witness,
            intent,
        } => commit_existing(
            &transaction,
            &prepared,
            basis,
            *integrity_generation,
            input_witness,
            intent,
            clock,
            failpoint,
        ),
    };
    if let Err(resolution) = result {
        return if transaction.rollback().is_ok() {
            resolution
        } else {
            RoomCommitResolutionV1::Indeterminate
        };
    }
    if transaction.commit().is_err() {
        return RoomCommitResolutionV1::Indeterminate;
    }
    if failpoint == Some(WriteBoundary::AfterCommitUnknown) {
        RoomCommitResolutionV1::Indeterminate
    } else {
        RoomCommitResolutionV1::resolved(ResolutionStatusV1::New, prepared.receipt.stored_result)
    }
}

#[allow(clippy::too_many_lines)]
fn commit_create(
    transaction: &Transaction<'_>,
    prepared: &SqlitePreparedWrite,
    persistence: &PreparedCreationPersistenceV1,
    clock: &dyn TrustedAuthorityClock,
    failpoint: Option<WriteBoundary>,
) -> Result<(), RoomCommitResolutionV1> {
    let head = &persistence.complete_head;
    let room_id = head.room_id().to_string();
    let authority_checked_at = clock
        .checked_at()
        .map_err(|_| RoomCommitResolutionV1::Indeterminate)?;
    match authority_matches(transaction, &prepared.authority, &authority_checked_at) {
        Ok(true) => {}
        Ok(false) => return Err(RoomCommitResolutionV1::Fenced),
        Err(resolution) => return Err(resolution),
    }
    let room_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rooms WHERE room_id = ?1)",
            [&room_id],
            |row| row.get(0),
        )
        .map_err(statement_failure)?;
    if room_exists {
        return Err(RoomCommitResolutionV1::Reprepare);
    }

    insert_receipt(transaction, prepared).map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::Receipt)?;

    transaction
        .execute(
            "INSERT INTO rooms(\
             room_id, room_status, room_seq, genesis_or_transition_hash, core_schema_version,\
             pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash,\
             complete_head_bytes\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                room_id,
                room_status(persistence.core_state.room_status()),
                to_i64(head.room_seq().get())?,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                persistence.canonical_head_bytes,
            ],
        )
        .map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::Room)?;

    transaction
        .execute(
            "INSERT INTO room_genesis(\
             room_id, pack_revision_lock_bytes, genesis_bytes\
             ) VALUES (?1, ?2, ?3)",
            params![
                room_id,
                persistence.canonical_pack_revision_lock_bytes,
                persistence.canonical_genesis_bytes,
            ],
        )
        .map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::Genesis)?;

    transaction
        .execute(
            "INSERT INTO room_materializations(\
             room_id, core_state_bytes, activity_state_bytes\
             ) VALUES (?1, ?2, ?3)",
            params![
                room_id,
                persistence.canonical_core_state_bytes,
                persistence.canonical_activity_state_bytes,
            ],
        )
        .map_err(statement_failure)?;
    #[cfg(test)]
    let mut wrote_first_member = false;
    for member in &persistence.memberships {
        let membership = &member.membership;
        transaction
            .execute(
                "INSERT INTO room_members(\
                 room_id, member_id, principal_id, principal_kind, standing, access_mode, role,\
                 membership_bytes, membership_generation, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 0)",
                params![
                    room_id,
                    membership.member_id().to_string(),
                    membership.principal_id().to_string(),
                    principal_kind(membership.principal_kind()),
                    membership_standing(membership.standing()),
                    access_mode(membership.access_mode()),
                    membership.role(),
                    member.canonical_membership_bytes,
                ],
            )
            .map_err(statement_failure)?;
        #[cfg(test)]
        if !wrote_first_member {
            wrote_first_member = true;
            fail_at(failpoint, WriteBoundary::AfterFirstCreateMember)?;
        }
    }
    fail_at(failpoint, WriteBoundary::Materializations)?;

    for timer in &persistence.initial_timers {
        transaction
            .execute(
                "INSERT INTO timers(\
                 room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'scheduled')",
                params![
                    room_id,
                    timer.timer_id().to_string(),
                    to_i64(timer.generation().get())?,
                    timer.scheduled_for().as_str(),
                    timer.canonical_payload_bytes(),
                ],
            )
            .map_err(statement_failure)?;
    }
    fail_at(failpoint, WriteBoundary::Timers)?;

    if persistence.integrity_generation.get() != 1 {
        return Err(RoomCommitResolutionV1::Fault);
    }
    transaction
        .execute(
            "INSERT INTO room_integrity(room_id, status, generation) VALUES (?1, 'healthy', 1)",
            [&room_id],
        )
        .map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::Integrity)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn commit_existing(
    transaction: &Transaction<'_>,
    prepared: &SqlitePreparedWrite,
    basis: &worldstream_core::CompleteHeadV1,
    integrity_generation: worldstream_core::IntegrityGenerationV1,
    input_witness: &PreparedOperationInputWitnessV1,
    intent: &PreparedExistingIntentV1,
    clock: &dyn TrustedAuthorityClock,
    failpoint: Option<WriteBoundary>,
) -> Result<(), RoomCommitResolutionV1> {
    let room_id = basis.room_id().to_string();
    let basis_head_bytes = prepared
        .receipt
        .basis_head_bytes
        .as_deref()
        .ok_or(RoomCommitResolutionV1::Fault)?;
    let room: Option<StoredHeadProjection> = transaction
        .query_row(
            "SELECT room_status, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, \
             core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes \
             FROM rooms WHERE room_id = ?1",
            [&room_id],
            |row| {
                Ok(StoredHeadProjection {
                    room_status: row.get(0)?,
                    room_seq: row.get(1)?,
                    lineage_hash: row.get(2)?,
                    core_schema_version: row.get(3)?,
                    pack_digest: row.get(4)?,
                    core_state_hash: row.get(5)?,
                    activity_state_hash: row.get(6)?,
                    authoritative_state_hash: row.get(7)?,
                    canonical_bytes: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(statement_failure)?;
    let Some(stored_head) = room else {
        return Err(RoomCommitResolutionV1::Fenced);
    };
    let durable_head = worldstream_core::CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
        &stored_head.canonical_bytes,
    )
    .map_err(|_| RoomCommitResolutionV1::Fault)?;
    let durable_room_seq = to_i64(durable_head.room_seq().get())?;
    if durable_head.room_id() != basis.room_id()
        || stored_head.room_seq != durable_room_seq
        || stored_head.lineage_hash != durable_head.genesis_or_transition_hash().to_string()
        || stored_head.core_schema_version != durable_head.core_schema_version()
        || stored_head.pack_digest != durable_head.pack_digest().to_string()
        || stored_head.core_state_hash != durable_head.core_state_hash().to_string()
        || stored_head.activity_state_hash != durable_head.activity_state_hash().to_string()
        || stored_head.authoritative_state_hash
            != durable_head.authoritative_state_hash().to_string()
        || durable_head
            .canonical_bytes()
            .map_err(|_| RoomCommitResolutionV1::Fault)?
            != stored_head.canonical_bytes
    {
        return Err(RoomCommitResolutionV1::Fault);
    }
    let expected_room_seq = to_i64(basis.room_seq().get())?;
    if durable_room_seq < expected_room_seq {
        return Err(RoomCommitResolutionV1::Fault);
    }
    if durable_room_seq > expected_room_seq {
        return Err(RoomCommitResolutionV1::Reprepare);
    }
    if durable_head != *basis || stored_head.canonical_bytes != basis_head_bytes {
        return Err(RoomCommitResolutionV1::Fault);
    }

    let integrity: Option<(String, i64)> = transaction
        .query_row(
            "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
            [&room_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(statement_failure)?;
    let expected_integrity = to_i64(integrity_generation.get())?;
    if !matches!(integrity, Some((ref status, generation)) if status == "healthy" && generation == expected_integrity)
    {
        return Err(RoomCommitResolutionV1::Fenced);
    }

    let authority_checked_at = clock
        .checked_at()
        .map_err(|_| RoomCommitResolutionV1::Indeterminate)?;
    match authority_matches(transaction, &prepared.authority, &authority_checked_at) {
        Ok(true) => {}
        Ok(false) => return Err(RoomCommitResolutionV1::Fenced),
        Err(resolution) => return Err(resolution),
    }

    let timer_is_applicable = match input_witness {
        PreparedOperationInputWitnessV1::ParticipantAction(_)
        | PreparedOperationInputWitnessV1::CoreAdministration(_) => true,
        PreparedOperationInputWitnessV1::TimerFired(witness) => {
            timer_witness_matches(transaction, &room_id, witness)?
        }
    };
    if !timer_is_applicable {
        return Err(RoomCommitResolutionV1::NotApplicable);
    }

    verify_existing_materializations(
        transaction,
        &room_id,
        &stored_head.room_status,
        input_witness,
    )?;
    match intent {
        PreparedExistingIntentV1::DurableDisposition => {
            insert_receipt(transaction, prepared).map_err(statement_failure)?;
            fail_at(failpoint, WriteBoundary::ExistingReceipt)
        }
        PreparedExistingIntentV1::Advance(advance) => commit_advance(
            transaction,
            prepared,
            basis_head_bytes,
            input_witness,
            advance,
            failpoint,
        ),
    }
}

fn timer_witness_matches(
    transaction: &Transaction<'_>,
    room_id: &str,
    witness: &worldstream_core::PreparedTimerInputWitnessV1,
) -> Result<bool, RoomCommitResolutionV1> {
    if witness.request.room_id().to_string() != room_id
        || witness.canonical_timer_payload_bytes
            != witness
                .request
                .canonical_payload()
                .to_bytes()
                .map_err(|_| RoomCommitResolutionV1::Fault)?
    {
        return Err(RoomCommitResolutionV1::Fault);
    }
    let row: Option<(String, Vec<u8>, String)> = transaction
        .query_row(
            "SELECT generation, scheduled_for, payload_bytes, state \
             FROM timers WHERE room_id = ?1 AND timer_id = ?2 AND generation = ?3",
            params![
                room_id,
                witness.request.timer_id().to_string(),
                to_i64(witness.request.generation().get())?,
            ],
            |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(statement_failure)?;
    let Some((scheduled_for, payload, state)) = row else {
        return Ok(false);
    };
    if state != "scheduled" {
        return Ok(false);
    }
    if scheduled_for != witness.request.scheduled_for().as_str()
        || payload != witness.canonical_timer_payload_bytes
    {
        return Err(RoomCommitResolutionV1::Fault);
    }
    Ok(true)
}

fn verify_existing_materializations(
    transaction: &Transaction<'_>,
    room_id: &str,
    stored_room_status: &str,
    input_witness: &PreparedOperationInputWitnessV1,
) -> Result<(), RoomCommitResolutionV1> {
    let expected = match input_witness {
        PreparedOperationInputWitnessV1::ParticipantAction(witness) => (
            witness.canonical_core_before_bytes.as_slice(),
            witness.canonical_activity_before_bytes.as_slice(),
        ),
        PreparedOperationInputWitnessV1::TimerFired(witness) => (
            witness.canonical_core_before_bytes.as_slice(),
            witness.canonical_activity_before_bytes.as_slice(),
        ),
        PreparedOperationInputWitnessV1::CoreAdministration(witness) => (
            witness.canonical_core_before_bytes.as_slice(),
            witness.canonical_activity_before_bytes.as_slice(),
        ),
    };
    let materializations: Option<(Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT core_state_bytes, activity_state_bytes \
             FROM room_materializations WHERE room_id = ?1",
            [room_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(statement_failure)?;
    if !matches!(materializations, Some((core, activity)) if core == expected.0 && activity == expected.1)
    {
        return Err(RoomCommitResolutionV1::Fault);
    }
    let core = worldstream_core::CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(expected.0)
        .map_err(|_| RoomCommitResolutionV1::Fault)?;
    if stored_room_status != room_status(core.room_status()) {
        return Err(RoomCommitResolutionV1::Fault);
    }
    let membership_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM room_members WHERE room_id = ?1",
            [room_id],
            |row| row.get(0),
        )
        .map_err(statement_failure)?;
    if usize::try_from(membership_count).ok() != Some(core.memberships().len()) {
        return Err(RoomCommitResolutionV1::Fault);
    }
    for membership in core.memberships().values() {
        let row: Option<StoredMembershipProjection> = transaction
            .query_row(
                "SELECT principal_id, principal_kind, standing, access_mode, role, membership_bytes \
                 FROM room_members WHERE room_id = ?1 AND member_id = ?2",
                params![room_id, membership.member_id().to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
        .map_err(statement_failure)?;
        let decoded_membership = row.as_ref().and_then(|(_, _, _, _, _, bytes)| {
            worldstream_core::CanonicalJsonV1::decode_canonical::<worldstream_core::MembershipV1>(
                bytes,
            )
            .ok()
        });
        if !matches!(
            row,
            Some((principal_id, kind, standing, mode, role, _bytes))
                if principal_id == membership.principal_id().to_string()
                    && kind == principal_kind(membership.principal_kind())
                    && standing == membership_standing(membership.standing())
                    && mode == access_mode(membership.access_mode())
                    && role.as_deref() == membership.role()
                    && decoded_membership.as_ref() == Some(membership)
        ) {
            return Err(RoomCommitResolutionV1::Fault);
        }
    }
    if let PreparedOperationInputWitnessV1::ParticipantAction(witness) = input_witness {
        let membership: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT membership_bytes FROM room_members \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![room_id, witness.membership_before.member_id().to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(statement_failure)?;
        if membership.as_deref() != Some(witness.canonical_membership_before_bytes.as_slice()) {
            return Err(RoomCommitResolutionV1::Fault);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn commit_advance(
    transaction: &Transaction<'_>,
    prepared: &SqlitePreparedWrite,
    basis_head_bytes: &[u8],
    input_witness: &PreparedOperationInputWitnessV1,
    advance: &PreparedAdvancePersistenceV1,
    failpoint: Option<WriteBoundary>,
) -> Result<(), RoomCommitResolutionV1> {
    let head = &advance.resulting_complete_head;
    let room_id = head.room_id().to_string();
    if advance.transition.room_seq() != head.room_seq()
        || advance.transition.transition_hash() != head.genesis_or_transition_hash()
        || advance.transition.resulting_core_state_hash() != head.core_state_hash()
        || advance.transition.resulting_activity_state_hash() != head.activity_state_hash()
        || advance.transition.resulting_authoritative_state_hash()
            != head.authoritative_state_hash()
    {
        return Err(RoomCommitResolutionV1::Fault);
    }
    transaction
        .execute(
            "INSERT INTO transitions(\
             room_id, transition_id, room_seq, transition_hash, previous_lineage_hash,\
             core_schema_version, pack_digest, core_state_hash, activity_state_hash,\
             authoritative_state_hash, transition_bytes\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                room_id,
                advance.transition_id.to_string(),
                to_i64(head.room_seq().get())?,
                head.genesis_or_transition_hash().to_string(),
                advance.transition.previous_lineage_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                advance.canonical_transition_bytes,
            ],
        )
        .map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::Transition)?;

    let changed = transaction
        .execute(
            "UPDATE rooms SET room_status = ?1, room_seq = ?2,\
             genesis_or_transition_hash = ?3, core_schema_version = ?4, pack_digest = ?5,\
             core_state_hash = ?6, activity_state_hash = ?7, authoritative_state_hash = ?8,\
             complete_head_bytes = ?9 WHERE room_id = ?10 AND complete_head_bytes = ?11",
            params![
                room_status(advance.resulting_core_state.room_status()),
                to_i64(head.room_seq().get())?,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                advance.canonical_resulting_head_bytes,
                room_id,
                basis_head_bytes,
            ],
        )
        .map_err(statement_failure)?;
    if changed != 1 {
        return Err(RoomCommitResolutionV1::Reprepare);
    }
    fail_at(failpoint, WriteBoundary::Head)?;

    let changed = transaction
        .execute(
            "UPDATE room_materializations SET core_state_bytes = ?1, activity_state_bytes = ?2 \
             WHERE room_id = ?3",
            params![
                advance.canonical_resulting_core_state_bytes,
                advance.canonical_resulting_activity_state_bytes,
                room_id,
            ],
        )
        .map_err(statement_failure)?;
    if changed != 1 {
        return Err(RoomCommitResolutionV1::Fault);
    }
    fail_at(failpoint, WriteBoundary::ExistingMaterializations)?;

    for member in &advance.resulting_memberships {
        let membership = &member.membership;
        transaction
            .execute(
                "INSERT INTO room_members(\
                 room_id, member_id, principal_id, principal_kind, standing, access_mode, role,\
                 membership_bytes, membership_generation, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 0) \
                 ON CONFLICT(room_id, member_id) DO UPDATE SET \
                 membership_generation = CASE \
                    WHEN room_members.membership_bytes != excluded.membership_bytes \
                    THEN room_members.membership_generation + 1 \
                    ELSE room_members.membership_generation END, \
                 standing = excluded.standing, access_mode = excluded.access_mode, \
                 role = excluded.role, membership_bytes = excluded.membership_bytes",
                params![
                    room_id,
                    membership.member_id().to_string(),
                    membership.principal_id().to_string(),
                    principal_kind(membership.principal_kind()),
                    membership_standing(membership.standing()),
                    access_mode(membership.access_mode()),
                    membership.role(),
                    member.canonical_membership_bytes,
                ],
            )
            .map_err(statement_failure)?;
    }
    let membership_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM room_members WHERE room_id = ?1",
            [&room_id],
            |row| row.get(0),
        )
        .map_err(statement_failure)?;
    if usize::try_from(membership_count).ok() != Some(advance.resulting_memberships.len()) {
        return Err(RoomCommitResolutionV1::Fault);
    }
    fail_at(failpoint, WriteBoundary::Members)?;

    if let PreparedOperationInputWitnessV1::TimerFired(witness) = input_witness {
        let changed = transaction
            .execute(
                "UPDATE timers SET state = 'fired' WHERE room_id = ?1 AND timer_id = ?2 \
                 AND generation = ?3 AND scheduled_for = ?4 AND payload_bytes = ?5 \
                 AND state = 'scheduled'",
                params![
                    room_id,
                    witness.request.timer_id().to_string(),
                    to_i64(witness.request.generation().get())?,
                    witness.request.scheduled_for().as_str(),
                    witness.canonical_timer_payload_bytes,
                ],
            )
            .map_err(statement_failure)?;
        if changed != 1 {
            return Err(RoomCommitResolutionV1::NotApplicable);
        }
    }
    apply_timer_mutations(transaction, &room_id, advance)?;
    fail_at(failpoint, WriteBoundary::ExistingTimers)?;

    for frame in &advance.observation_frames {
        transaction
            .execute(
                "INSERT INTO observation_frames(\
                 room_id, member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    room_id,
                    frame.member_id().to_string(),
                    to_i64(frame.frame_seq())?,
                    to_i64(frame.cause_room_seq().get())?,
                    frame.payload_hash().to_string(),
                    frame.canonical_payload_bytes(),
                ],
            )
            .map_err(statement_failure)?;
        let changed = transaction
            .execute(
                "UPDATE room_members SET frame_head = ?1 \
                 WHERE room_id = ?2 AND member_id = ?3 AND frame_head = ?4",
                params![
                    to_i64(frame.frame_seq())?,
                    room_id,
                    frame.member_id().to_string(),
                    to_i64(frame.previous_frame_head())?,
                ],
            )
            .map_err(statement_failure)?;
        if changed != 1 {
            return Err(RoomCommitResolutionV1::Fault);
        }
    }
    fail_at(failpoint, WriteBoundary::Frames)?;

    for decision in &advance.activation_decisions {
        transaction
            .execute(
                "INSERT INTO activation_decisions(\
                 room_id, cause_room_seq, decision_id, target_member_id, decision_bytes\
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    room_id,
                    to_i64(head.room_seq().get())?,
                    decision.decision_id(),
                    decision.target_member_id().map(ToString::to_string),
                    decision.canonical_decision_bytes(),
                ],
            )
            .map_err(statement_failure)?;
    }
    fail_at(failpoint, WriteBoundary::Activation)?;

    insert_receipt(transaction, prepared).map_err(statement_failure)?;
    fail_at(failpoint, WriteBoundary::ExistingReceipt)
}

fn apply_timer_mutations(
    transaction: &Transaction<'_>,
    room_id: &str,
    advance: &PreparedAdvancePersistenceV1,
) -> Result<(), RoomCommitResolutionV1> {
    for mutation in &advance.timer_changes {
        match mutation.kind() {
            PreparedTimerMutationKindV1::Schedule {
                timer_id,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => {
                require_next_timer_generation(
                    transaction,
                    room_id,
                    timer_id.as_ref(),
                    generation.get(),
                )?;
                transaction
                    .execute(
                        "INSERT INTO timers(\
                         room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                         ) VALUES (?1, ?2, ?3, ?4, ?5, 'scheduled')",
                        params![
                            room_id,
                            timer_id.to_string(),
                            to_i64(generation.get())?,
                            scheduled_for.as_str(),
                            canonical_payload_bytes,
                        ],
                    )
                    .map_err(statement_failure)?;
            }
            PreparedTimerMutationKindV1::Cancel {
                timer_id,
                generation,
            } => {
                let changed = transaction
                    .execute(
                        "UPDATE timers SET state = 'cancelled' \
                         WHERE room_id = ?1 AND timer_id = ?2 AND generation = ?3 \
                         AND state = 'scheduled'",
                        params![room_id, timer_id.to_string(), to_i64(generation.get())?],
                    )
                    .map_err(statement_failure)?;
                if changed != 1 {
                    return Err(RoomCommitResolutionV1::Fault);
                }
            }
            PreparedTimerMutationKindV1::Reschedule {
                timer_id,
                previous_generation,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => {
                let previous = to_i64(previous_generation.get())?;
                let next = to_i64(generation.get())?;
                let maximum: Option<i64> = transaction
                    .query_row(
                        "SELECT max(generation) FROM timers WHERE room_id = ?1 AND timer_id = ?2",
                        params![room_id, timer_id.to_string()],
                        |row| row.get(0),
                    )
                    .map_err(statement_failure)?;
                if maximum != Some(previous) || previous.checked_add(1) != Some(next) {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                let changed = transaction
                    .execute(
                        "UPDATE timers SET state = 'cancelled' \
                         WHERE room_id = ?1 AND timer_id = ?2 AND generation = ?3 \
                         AND state = 'scheduled'",
                        params![
                            room_id,
                            timer_id.to_string(),
                            to_i64(previous_generation.get())?,
                        ],
                    )
                    .map_err(statement_failure)?;
                if changed != 1 {
                    return Err(RoomCommitResolutionV1::Fault);
                }
                transaction
                    .execute(
                        "INSERT INTO timers(\
                         room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                         ) VALUES (?1, ?2, ?3, ?4, ?5, 'scheduled')",
                        params![
                            room_id,
                            timer_id.to_string(),
                            to_i64(generation.get())?,
                            scheduled_for.as_str(),
                            canonical_payload_bytes,
                        ],
                    )
                    .map_err(statement_failure)?;
            }
        }
    }
    Ok(())
}

fn require_next_timer_generation(
    transaction: &Transaction<'_>,
    room_id: &str,
    timer_id: &str,
    generation: u64,
) -> Result<(), RoomCommitResolutionV1> {
    let generation = to_i64(generation)?;
    let maximum: Option<i64> = transaction
        .query_row(
            "SELECT max(generation) FROM timers WHERE room_id = ?1 AND timer_id = ?2",
            params![room_id, timer_id],
            |row| row.get(0),
        )
        .map_err(statement_failure)?;
    let expected = maximum.map_or(Some(1), |value| value.checked_add(1));
    if expected != Some(generation) {
        return Err(RoomCommitResolutionV1::Fault);
    }
    Ok(())
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    prepared: &SqlitePreparedWrite,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO semantic_receipts(\
         room_id, operation_kind, operation_identity_bytes, codec_id, canonical_request_hash,\
         basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind,\
         transition_seq, stored_resolution_bytes\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            prepared.receipt.room_id,
            prepared.receipt.operation_kind,
            prepared.identity_bytes,
            OPERATION_RECEIPT_CODEC_ID,
            prepared.request_hash.as_bytes().as_slice(),
            prepared.receipt.basis_head_bytes,
            prepared.receipt.semantic_input_bytes,
            prepared.receipt.semantic_time_bytes,
            prepared.receipt.resolution_kind,
            prepared.receipt.transition_seq,
            prepared.receipt.stored_result.canonical_receipt_bytes(),
        ],
    )?;
    Ok(())
}

fn lookup_receipt(
    transaction: &Transaction<'_>,
    identity: &OperationIdentityV1,
    identity_bytes: &[u8],
) -> Result<Option<StoredSemanticResultV1>, ReceiptLookupError> {
    let row: Option<StoredReceiptProjection> = transaction
        .query_row(
            "SELECT room_id, operation_kind, operation_identity_bytes, codec_id, \
             canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
             semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes \
             FROM semantic_receipts \
             WHERE operation_kind = ?1 AND operation_identity_bytes = ?2",
            params![identity.operation_kind(), identity_bytes],
            |row| {
                Ok(StoredReceiptProjection {
                    room_id: row.get(0)?,
                    operation_kind: row.get(1)?,
                    operation_identity_bytes: row.get(2)?,
                    codec_id: row.get(3)?,
                    canonical_request_hash: row.get(4)?,
                    basis_complete_head_bytes: row.get(5)?,
                    semantic_input_bytes: row.get(6)?,
                    semantic_time_bytes: row.get(7)?,
                    resolution_kind: row.get(8)?,
                    transition_seq: row.get(9)?,
                    stored_resolution_bytes: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|_| ReceiptLookupError::Database)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored = validate_stored_receipt_projection(&row)?;
    if row.operation_identity_bytes != identity_bytes
        || stored.operation_identity() != identity
        || stored
            .operation_identity()
            .canonical_bytes()
            .map_err(|_| ReceiptLookupError::Corrupt)?
            != identity_bytes
    {
        return Err(ReceiptLookupError::Corrupt);
    }
    Ok(Some(stored))
}

fn validate_stored_receipt_projection(
    row: &StoredReceiptProjection,
) -> Result<StoredSemanticResultV1, ReceiptLookupError> {
    let stored = StoredSemanticResultV1::from_canonical_receipt_bytes(&row.stored_resolution_bytes)
        .map_err(|_| ReceiptLookupError::Corrupt)?;
    let stored_basis = stored
        .canonical_basis_head_bytes()
        .map_err(|_| ReceiptLookupError::Corrupt)?;
    let stored_input = stored
        .semantic_input()
        .canonical_bytes()
        .map_err(|_| ReceiptLookupError::Corrupt)?;
    let stored_time = stored
        .canonical_semantic_time_bytes()
        .map_err(|_| ReceiptLookupError::Corrupt)?;
    let stored_transition_seq = stored
        .transition_seq()
        .map(|sequence| i64::try_from(sequence.get()).map_err(|_| ReceiptLookupError::Corrupt))
        .transpose()?;
    if row.room_id != stored.target_room_id().to_string()
        || row.operation_kind != stored.operation_identity().operation_kind()
        || row.codec_id != OPERATION_RECEIPT_CODEC_ID
        || row.canonical_request_hash.as_slice() != stored.canonical_request_hash().as_bytes()
        || row.basis_complete_head_bytes != stored_basis
        || row.semantic_input_bytes != stored_input
        || row.semantic_time_bytes != stored_time
        || row.resolution_kind != stored.resolution_kind()
        || row.transition_seq != stored_transition_seq
        || row.stored_resolution_bytes != stored.canonical_receipt_bytes()
        || stored
            .operation_identity()
            .canonical_bytes()
            .map_err(|_| ReceiptLookupError::Corrupt)?
            != row.operation_identity_bytes
    {
        return Err(ReceiptLookupError::Corrupt);
    }
    Ok(stored)
}

enum ReceiptLookupError {
    Database,
    Corrupt,
}

fn resolve_authorized_guarded(
    connection: &mut Connection,
    authority: AuthorizedReceiptReadV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<ResolveOutcomeV1, AuthorityErrorV1> {
    let authority = authority.into_adapter_input();
    let query = authority.authority_snapshot_query();
    let Ok(transaction) = connection.transaction_with_behavior(TransactionBehavior::Immediate)
    else {
        return Err(AuthorityErrorV1::Unavailable);
    };
    let snapshot = load_authority_snapshot(&transaction, &query)
        .map_err(|_| AuthorityErrorV1::Unavailable)?
        .ok_or(AuthorityErrorV1::Unauthenticated)?;
    let checked_at = clock
        .checked_at()
        .map_err(|_| AuthorityErrorV1::Unavailable)?;
    authority.revalidate_current(&snapshot, &checked_at)?;
    let outcome =
        resolve_authorized_room_operation_for_adapter(authority, |identity, request_hash| {
            resolve_in_transaction(&transaction, identity, request_hash)
        });
    if transaction.commit().is_ok() {
        Ok(outcome)
    } else {
        Ok(ResolveOutcomeV1::ResolutionUnavailable)
    }
}

#[allow(clippy::too_many_lines)]
fn begin_authorized_replay(
    connection: &mut Connection,
    authority: AuthorizedReplayV1,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorizedReplaySessionV1, SqliteAuthorizedReplayErrorV1> {
    let authority = authority.into_adapter_input();
    let query = authority.authority_snapshot_query();
    let address = AuthorizedReplayAddressV1 {
        room_id: authority.room_id().clone(),
        member_id: authority.member_id().clone(),
        at_room_seq: authority.at_room_seq(),
        projection_kind: authority.projection_kind(),
    };
    let room_id = authority.room_id().to_string();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    let observed = load_observed_recovery_fence(&transaction, &room_id)
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    let snapshot = match load_authority_snapshot(&transaction, &query) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::Unauthenticated,
            ));
        }
        Err(AuthorityStoreErrorV1::Corrupt) => {
            if let Some(observed) = &observed {
                if !has_valid_replay_integrity_fence(observed) {
                    return quarantine_corrupt_replay_integrity(transaction, observed);
                }
                if parse_observed_replay_integrity_status(observed)?
                    == RoomIntegrityStatusV1::Quarantined
                {
                    return Err(SqliteAuthorizedReplayErrorV1::Replay(
                        HistoricalReplayErrorV1::IntegrityUnavailable,
                    ));
                }
                if replay_room_authority_projection_is_corrupt(
                    &transaction,
                    &query,
                    authority.room_id(),
                )
                .map_err(map_replay_authority_store_error)?
                {
                    let status = parse_observed_replay_integrity_status(observed)?;
                    let generation = parse_observed_replay_integrity_generation(observed)?;
                    return quarantine_corrupt_replay_capture(
                        transaction,
                        &room_id,
                        status,
                        generation,
                    );
                }
            }
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        Err(error) => return Err(map_replay_authority_store_error(error)),
    };
    let checked_at = clock
        .checked_at()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    authority
        .revalidate_current(&snapshot, &checked_at)
        .map_err(SqliteAuthorizedReplayErrorV1::Authority)?;

    let Some(observed) = observed else {
        return Err(SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::SequenceUnavailable,
        ));
    };
    if !has_valid_replay_integrity_fence(&observed) {
        return quarantine_corrupt_replay_integrity(transaction, &observed);
    }
    let integrity_status = parse_observed_replay_integrity_status(&observed)?;
    let integrity_generation = parse_observed_replay_integrity_generation(&observed)?;
    if integrity_status == RoomIntegrityStatusV1::Quarantined {
        return Err(SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::IntegrityUnavailable,
        ));
    }
    let head = match decode_stored_head_projection(&observed.head, authority.room_id()) {
        Ok(head) => head,
        Err(RoomRecoveryErrorV1::Corrupt) => {
            return quarantine_corrupt_replay_capture(
                transaction,
                &room_id,
                integrity_status,
                integrity_generation,
            );
        }
        Err(_) => return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable),
    };
    if head.room_seq().get() < authority.at_room_seq().get() {
        return Err(SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::SequenceUnavailable,
        ));
    }
    let request = authority.projection_request(RoomIntegrityStateV1::new(
        integrity_status,
        integrity_generation,
    ));
    transaction
        .commit()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    let disposition_fence = ReplayIntegrityDispositionFenceV1 {
        room_id: address.room_id.clone(),
        head,
        status: integrity_status,
        generation: integrity_generation,
    };
    Ok(AuthorizedReplaySessionV1 {
        authority,
        address,
        request: Some(request),
        disposition_fence,
    })
}

#[allow(clippy::too_many_lines)]
fn release_authorized_replay(
    connection: &mut Connection,
    session: AuthorizedReplaySessionV1,
    require_operational_fence: bool,
    clock: &dyn TrustedAuthorityClock,
) -> Result<AuthorizedReplaySessionV1, SqliteAuthorizedReplayErrorV1> {
    let expected = &session.disposition_fence;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    let room_id = expected.room_id.to_string();
    let observed = require_operational_fence
        .then(|| load_observed_recovery_fence(&transaction, &room_id))
        .transpose()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
        .flatten();
    let authority_query = session.authority.authority_snapshot_query();
    let snapshot = match load_authority_snapshot(&transaction, &authority_query) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::Unauthenticated,
            ));
        }
        Err(AuthorityStoreErrorV1::Corrupt) if require_operational_fence => {
            if let Some(observed) = &observed {
                if !has_valid_replay_integrity_fence(observed) {
                    return quarantine_corrupt_replay_integrity(transaction, observed);
                }
                let status = parse_observed_replay_integrity_status(observed)?;
                if status == RoomIntegrityStatusV1::Quarantined {
                    return Err(SqliteAuthorizedReplayErrorV1::Replay(
                        HistoricalReplayErrorV1::IntegrityUnavailable,
                    ));
                }
                if replay_room_authority_projection_is_corrupt(
                    &transaction,
                    &authority_query,
                    &expected.room_id,
                )
                .map_err(map_replay_authority_store_error)?
                {
                    let generation = parse_observed_replay_integrity_generation(observed)?;
                    return quarantine_corrupt_replay_capture(
                        transaction,
                        &room_id,
                        status,
                        generation,
                    );
                }
            }
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        Err(error) => return Err(map_replay_authority_store_error(error)),
    };
    let checked_at = clock
        .checked_at()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    session
        .authority
        .revalidate_current(&snapshot, &checked_at)
        .map_err(SqliteAuthorizedReplayErrorV1::Authority)?;
    if require_operational_fence {
        let observed = observed.ok_or(SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        if !has_valid_replay_integrity_fence(&observed) {
            return quarantine_corrupt_replay_integrity(transaction, &observed);
        }
        let status = parse_observed_replay_integrity_status(&observed)?;
        let generation = parse_observed_replay_integrity_generation(&observed)?;
        if status == RoomIntegrityStatusV1::Quarantined {
            return Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::IntegrityUnavailable,
            ));
        }
        let head = match decode_stored_head_projection(&observed.head, &expected.room_id) {
            Ok(head) => head,
            Err(RoomRecoveryErrorV1::Corrupt) => {
                return quarantine_corrupt_replay_capture(
                    transaction,
                    &room_id,
                    status,
                    generation,
                );
            }
            Err(_) => return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable),
        };
        if status != expected.status || generation != expected.generation {
            return Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable);
        }
        if head.room_seq().get() < expected.head.room_seq().get()
            || head.core_schema_version() != expected.head.core_schema_version()
            || head.pack_digest() != expected.head.pack_digest()
            || (head.room_seq() == expected.head.room_seq() && head != expected.head)
            || (expected.status != RoomIntegrityStatusV1::Healthy
                && head.room_seq() != expected.head.room_seq())
        {
            return quarantine_corrupt_replay_capture(transaction, &room_id, status, generation);
        }
        if head.room_seq().get() > expected.head.room_seq().get()
            && !captured_replay_head_is_still_anchored(&transaction, &expected.head)
                .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
        {
            return quarantine_corrupt_replay_capture(transaction, &room_id, status, generation);
        }
    }
    transaction
        .commit()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    Ok(session)
}

fn captured_replay_head_is_still_anchored(
    transaction: &Transaction<'_>,
    expected: &CompleteHeadV1,
) -> Result<bool, RoomRecoveryErrorV1> {
    if expected.room_seq().get() == 0 {
        let bytes: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT genesis_bytes FROM room_genesis WHERE room_id = ?1",
                [expected.room_id().as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let Some(bytes) = bytes else {
            return Ok(false);
        };
        let Ok(genesis) = GenesisV1::from_canonical_bytes(&bytes) else {
            return Ok(false);
        };
        let Ok(core_bytes) = genesis.initial_core_state().canonical_bytes() else {
            return Ok(false);
        };
        let Ok(activity_bytes) = genesis.initial_activity_state().to_bytes() else {
            return Ok(false);
        };
        return Ok(genesis.complete_head() == *expected
            && VerifiedCurrentRoomMaterializationV1::verify_for_storage(
                expected,
                &bytes,
                &core_bytes,
                &activity_bytes,
            )
            .is_ok());
    }
    let sequence =
        i64::try_from(expected.room_seq().get()).map_err(|_| RoomRecoveryErrorV1::Corrupt)?;
    let row: Option<(String, Vec<u8>)> = transaction
        .query_row(
            "SELECT transition_hash, transition_bytes FROM transitions \
             WHERE room_id = ?1 AND room_seq = ?2",
            params![expected.room_id().as_str(), sequence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    let Some((stored_hash, bytes)) = row else {
        return Ok(false);
    };
    let Ok(transition) = TransitionV1::from_canonical_bytes(&bytes) else {
        return Ok(false);
    };
    let Ok(core_bytes) = transition.resulting_core_state().canonical_bytes() else {
        return Ok(false);
    };
    let Ok(activity_bytes) = transition.resulting_activity_state().to_bytes() else {
        return Ok(false);
    };
    Ok(transition.room_seq() == expected.room_seq()
        && transition.complete_head() == *expected
        && transition.transition_hash().to_string() == stored_hash
        && transition.transition_hash() == expected.genesis_or_transition_hash()
        && VerifiedCurrentRoomMaterializationV1::verify_for_storage(
            expected,
            &bytes,
            &core_bytes,
            &activity_bytes,
        )
        .is_ok())
}

#[allow(clippy::too_many_lines)]
fn read_authorized_replay_slice(
    path: &Path,
    registry: &PackRegistryV1,
    paging: &mut AuthorizedReplayPagingStateV1,
) -> Result<bool, SqliteAuthorizedReplayErrorV1> {
    let room_id = &paging.session.address.room_id;
    let requested_sequence = paging.session.address.at_room_seq;
    let requested_sequence_i64 = i64::try_from(requested_sequence.get()).map_err(|_| {
        SqliteAuthorizedReplayErrorV1::Replay(HistoricalReplayErrorV1::SequenceUnavailable)
    })?;
    if paging.prior_sequence > requested_sequence.get()
        || (paging.accumulator.is_none() && paging.prior_sequence != 0)
        || (paging.accumulator.is_some() != paging.receipt_head.is_some())
    {
        return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
    }
    let started = Instant::now();
    let row_budget = replay_slice_row_budget(path);
    let mut rows_read = 0_usize;
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    connection
        .busy_timeout(Duration::from_millis(500))
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    if paging.accumulator.is_none() {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let (pack_revision_lock_bytes, bytes): (Vec<u8>, Vec<u8>) = transaction
            .query_row(
                "SELECT pack_revision_lock_bytes, genesis_bytes FROM room_genesis \
                 WHERE room_id = ?1",
                [room_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
        let genesis = GenesisV1::from_canonical_bytes(&bytes)
            .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
        if genesis.room_id() != room_id
            || genesis.complete_head().room_seq().get() != 0
            || genesis.complete_head().core_schema_version()
                != paging.session.disposition_fence.head.core_schema_version()
            || genesis.pack_digest() != paging.session.disposition_fence.head.pack_digest()
        {
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        let revision_lock = PackRevisionLockV1::from_canonical_bytes(
            &pack_revision_lock_bytes,
            genesis.pack_digest(),
        )
        .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
        if let Ok(retained) = registry.load_retained(genesis.pack_digest())
            && retained.revision_lock() != &revision_lock
        {
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        let genesis_receipt_count: i64 = transaction
            .query_row(
                "SELECT count(*) FROM semantic_receipts \
                 WHERE room_id = ?1 AND resolution_kind = 'genesis_created'",
                [room_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        if genesis_receipt_count != 1 {
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        let receipt_row = transaction
            .query_row(
                "SELECT room_id, operation_kind, operation_identity_bytes, codec_id, \
                 canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
                 semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes \
                 FROM semantic_receipts \
                 WHERE room_id = ?1 AND resolution_kind = 'genesis_created'",
                [room_id.as_str()],
                |row| {
                    Ok(StoredReceiptProjection {
                        room_id: row.get(0)?,
                        operation_kind: row.get(1)?,
                        operation_identity_bytes: row.get(2)?,
                        codec_id: row.get(3)?,
                        canonical_request_hash: row.get(4)?,
                        basis_complete_head_bytes: row.get(5)?,
                        semantic_input_bytes: row.get(6)?,
                        semantic_time_bytes: row.get(7)?,
                        resolution_kind: row.get(8)?,
                        transition_seq: row.get(9)?,
                        stored_resolution_bytes: row.get(10)?,
                    })
                },
            )
            .optional()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
        let genesis_receipt = validate_stored_receipt_projection(&receipt_row)
            .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
        if !genesis_receipt_matches_genesis(&genesis_receipt, &genesis) {
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        transaction
            .commit()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let request = paging
            .session
            .request
            .take()
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
        paging.accumulator = Some(
            HistoricalReplayAccumulatorV1::begin(registry, &bytes, request)
                .map_err(SqliteAuthorizedReplayErrorV1::Replay)?,
        );
        paging.receipt_head = Some(genesis.complete_head());
        rows_read = rows_read
            .checked_add(1)
            .ok_or(SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        #[cfg(test)]
        pause_before_replay_projection(path);
    }

    while paging.prior_sequence < requested_sequence.get() {
        if rows_read >= row_budget || started.elapsed() >= REPLAY_MAX_CAPTURE_DURATION {
            return Ok(false);
        }
        let remaining_budget = row_budget.saturating_sub(rows_read);
        let remaining_sequence = requested_sequence
            .get()
            .checked_sub(paging.prior_sequence)
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
        let page_limit = usize::try_from(remaining_sequence)
            .unwrap_or(usize::MAX)
            .min(REPLAY_PAGE_ROWS)
            .min(remaining_budget);
        if page_limit == 0 {
            return Ok(false);
        }
        let prior_sequence_i64 = i64::try_from(paging.prior_sequence)
            .map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let mut statement = transaction
            .prepare(
                "WITH page AS ( \
                     SELECT room_seq, transition_id, transition_hash, previous_lineage_hash, \
                         core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
                         authoritative_state_hash, transition_bytes FROM transitions \
                     WHERE room_id = ?1 AND room_seq > ?2 AND room_seq <= ?3 \
                     ORDER BY room_seq LIMIT ?4 \
                 ), receipt_counts AS ( \
                     SELECT page.room_seq, count(receipt.operation_identity_bytes) AS receipt_count \
                     FROM page \
                     LEFT JOIN semantic_receipts AS receipt \
                       ON receipt.room_id = ?1 AND receipt.transition_seq = page.room_seq \
                     GROUP BY page.room_seq \
                 ) \
                 SELECT page.room_seq, page.transition_id, page.transition_hash, \
                     page.previous_lineage_hash, page.core_schema_version, page.pack_digest, \
                     page.core_state_hash, page.activity_state_hash, \
                     page.authoritative_state_hash, page.transition_bytes, \
                     coalesce(receipt_counts.receipt_count, 0), \
                     receipt.room_id, receipt.operation_kind, receipt.operation_identity_bytes, \
                     receipt.codec_id, receipt.canonical_request_hash, \
                     receipt.basis_complete_head_bytes, receipt.semantic_input_bytes, \
                     receipt.semantic_time_bytes, receipt.resolution_kind, \
                     receipt.transition_seq, receipt.stored_resolution_bytes \
                 FROM page \
                 LEFT JOIN receipt_counts \
                   ON receipt_counts.room_seq = page.room_seq \
                 LEFT JOIN semantic_receipts AS receipt \
                   ON receipt.room_id = ?1 AND receipt.transition_seq = page.room_seq \
                  AND receipt_counts.receipt_count = 1 \
                 ORDER BY page.room_seq",
            )
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let rows = statement
            .query_map(
                params![
                    room_id.as_str(),
                    prior_sequence_i64,
                    requested_sequence_i64,
                    i64::try_from(page_limit)
                        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?
                ],
                |row| {
                    let receipt_count = row.get::<_, i64>(10)?;
                    let receipt = if receipt_count == 1 {
                        Some(StoredReceiptProjection {
                            room_id: row.get(11)?,
                            operation_kind: row.get(12)?,
                            operation_identity_bytes: row.get(13)?,
                            codec_id: row.get(14)?,
                            canonical_request_hash: row.get(15)?,
                            basis_complete_head_bytes: row.get(16)?,
                            semantic_input_bytes: row.get(17)?,
                            semantic_time_bytes: row.get(18)?,
                            resolution_kind: row.get(19)?,
                            transition_seq: row.get(20)?,
                            stored_resolution_bytes: row.get(21)?,
                        })
                    } else {
                        None
                    };
                    Ok(ReplayTransitionPageRowV1 {
                        sequence: row.get(0)?,
                        transition_id: row.get(1)?,
                        transition_hash: row.get(2)?,
                        previous_lineage_hash: row.get(3)?,
                        core_schema_version: row.get(4)?,
                        pack_digest: row.get(5)?,
                        core_state_hash: row.get(6)?,
                        activity_state_hash: row.get(7)?,
                        authoritative_state_hash: row.get(8)?,
                        transition_bytes: row.get(9)?,
                        receipt_count,
                        receipt,
                    })
                },
            )
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        let mut page = Vec::with_capacity(REPLAY_PAGE_ROWS);
        for row in rows {
            let row = row.map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
            let sequence =
                u64::try_from(row.sequence).map_err(|_| SqliteAuthorizedReplayErrorV1::Corrupt)?;
            let expected = paging
                .prior_sequence
                .checked_add(1)
                .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
            if sequence != expected || sequence > requested_sequence.get() || row.receipt_count != 1
            {
                return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
            }
            let prior_head = paging
                .receipt_head
                .as_ref()
                .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?;
            let next_head =
                validate_replay_transition_receipt(room_id, prior_head, sequence, &row)?;
            paging.prior_sequence = sequence;
            paging.receipt_head = Some(next_head);
            page.push(row.transition_bytes);
        }
        drop(statement);
        transaction
            .commit()
            .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        if page.is_empty() {
            return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
        }
        rows_read = rows_read
            .checked_add(page.len())
            .ok_or(SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
        paging
            .accumulator
            .as_mut()
            .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)?
            .consume_page(&page)
            .map_err(SqliteAuthorizedReplayErrorV1::Replay)?;
        #[cfg(test)]
        pause_before_replay_projection(path);
    }
    if paging.prior_sequence != requested_sequence.get()
        || !paging
            .accumulator
            .as_ref()
            .is_some_and(HistoricalReplayAccumulatorV1::is_complete)
    {
        return Err(SqliteAuthorizedReplayErrorV1::Corrupt);
    }
    Ok(true)
}

fn replay_slice_row_budget(path: &Path) -> usize {
    #[cfg(test)]
    if let Some(state) = REPLAY_SLICE_BUDGET.get()
        && let Ok(state) = state.lock()
        && let Some(max_rows) = state.by_path.get(path)
    {
        return (*max_rows).max(1);
    }
    let _ = path;
    REPLAY_MAX_ROWS_PER_SLICE
}

fn cleanup_expired_replay_sessions(sessions: &mut ReplaySessionRegistryV1, now: Instant) {
    sessions.by_id.retain(|_, entry| {
        now.saturating_duration_since(entry.last_used_at) < REPLAY_SESSION_IDLE_EXPIRY
    });
}

fn has_valid_replay_integrity_fence(observed: &ObservedRecoveryFence) -> bool {
    parse_observed_replay_integrity_status(observed).is_ok()
        && parse_observed_replay_integrity_generation(observed).is_ok()
}

fn parse_observed_replay_integrity_status(
    observed: &ObservedRecoveryFence,
) -> Result<RoomIntegrityStatusV1, SqliteAuthorizedReplayErrorV1> {
    match observed.integrity_status.as_deref() {
        Some("healthy") => Ok(RoomIntegrityStatusV1::Healthy),
        Some("faulted") => Ok(RoomIntegrityStatusV1::Faulted),
        Some("quarantined") => Ok(RoomIntegrityStatusV1::Quarantined),
        Some(_) | None => Err(SqliteAuthorizedReplayErrorV1::Corrupt),
    }
}

fn parse_observed_replay_integrity_generation(
    observed: &ObservedRecoveryFence,
) -> Result<IntegrityGenerationV1, SqliteAuthorizedReplayErrorV1> {
    observed
        .integrity_generation
        .and_then(|value| u64::try_from(value).ok())
        .and_then(|value| IntegrityGenerationV1::new(value).ok())
        .ok_or(SqliteAuthorizedReplayErrorV1::Corrupt)
}

fn quarantine_corrupt_replay_integrity<T>(
    transaction: Transaction<'_>,
    observed: &ObservedRecoveryFence,
) -> Result<T, SqliteAuthorizedReplayErrorV1> {
    quarantine_corrupt_replay_integrity_in_transaction(&transaction, observed)
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    transaction
        .commit()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    Err(SqliteAuthorizedReplayErrorV1::Corrupt)
}

fn quarantine_corrupt_replay_integrity_in_transaction(
    transaction: &Transaction<'_>,
    observed: &ObservedRecoveryFence,
) -> Result<(), RoomRecoveryErrorV1> {
    match (
        observed.integrity_status.as_deref(),
        observed.integrity_generation,
    ) {
        (None, None) => {
            let inserted = transaction
                .execute(
                    "INSERT INTO room_integrity(room_id, status, generation) \
                     SELECT ?1, 'quarantined', 1 WHERE NOT EXISTS(\
                         SELECT 1 FROM room_integrity WHERE room_id = ?1\
                     )",
                    [&observed.room_id],
                )
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            if inserted != 1 {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            }
        }
        (Some(status), Some(generation)) => {
            let next = generation
                .checked_add(1)
                .filter(|value| (1..=MAX_SAFE_INTEGER).contains(value))
                .ok_or(RoomRecoveryErrorV1::Corrupt)?;
            let changed = transaction
                .execute(
                    "UPDATE room_integrity SET status = 'quarantined', generation = ?1 \
                     WHERE room_id = ?2 AND status = ?3 AND generation = ?4",
                    params![next, observed.room_id, status, generation],
                )
                .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
            if changed != 1 {
                return Err(RoomRecoveryErrorV1::ConcurrentChange);
            }
        }
        _ => return Err(RoomRecoveryErrorV1::Corrupt),
    }
    Ok(())
}

fn quarantine_corrupt_replay_capture<T>(
    transaction: Transaction<'_>,
    room_id: &str,
    current_status: RoomIntegrityStatusV1,
    current_generation: IntegrityGenerationV1,
) -> Result<T, SqliteAuthorizedReplayErrorV1> {
    if current_status == RoomIntegrityStatusV1::Quarantined {
        return Err(SqliteAuthorizedReplayErrorV1::Replay(
            HistoricalReplayErrorV1::IntegrityUnavailable,
        ));
    }
    update_replay_integrity_disposition_in_transaction(
        &transaction,
        room_id,
        current_status,
        current_generation,
        RecoveryIntegrityDispositionV1::Quarantined,
    )
    .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    transaction
        .commit()
        .map_err(|_| SqliteAuthorizedReplayErrorV1::StorageUnavailable)?;
    Err(SqliteAuthorizedReplayErrorV1::Corrupt)
}

const fn replay_integrity_disposition(
    error: HistoricalReplayErrorV1,
) -> Option<RecoveryIntegrityDispositionV1> {
    match error {
        HistoricalReplayErrorV1::ReplayFailed(
            ReplayFailureClassV1::RuntimeUnavailable | ReplayFailureClassV1::RuntimeFault,
        )
        | HistoricalReplayErrorV1::ProjectionUnavailable => {
            Some(RecoveryIntegrityDispositionV1::Faulted)
        }
        HistoricalReplayErrorV1::ReplayFailed(_) | HistoricalReplayErrorV1::AddressMismatch => {
            Some(RecoveryIntegrityDispositionV1::Quarantined)
        }
        HistoricalReplayErrorV1::SequenceUnavailable
        | HistoricalReplayErrorV1::HistoricalMembershipUnavailable
        | HistoricalReplayErrorV1::IntegrityUnavailable => None,
    }
}

fn map_replay_authority_store_error(error: AuthorityStoreErrorV1) -> SqliteAuthorizedReplayErrorV1 {
    match error {
        AuthorityStoreErrorV1::Corrupt => SqliteAuthorizedReplayErrorV1::Corrupt,
        AuthorityStoreErrorV1::Unavailable
        | AuthorityStoreErrorV1::Conflict
        | AuthorityStoreErrorV1::StaleGeneration
        | AuthorityStoreErrorV1::InvalidChange => SqliteAuthorizedReplayErrorV1::StorageUnavailable,
    }
}

fn resolve_in_transaction(
    transaction: &Transaction<'_>,
    identity: &OperationIdentityV1,
    request_hash: &CanonicalRequestHashV1,
) -> ResolveOutcomeV1 {
    let Ok(identity_bytes) = identity.canonical_bytes() else {
        return ResolveOutcomeV1::ResolutionUnavailable;
    };
    match lookup_receipt(transaction, identity, &identity_bytes) {
        Ok(Some(stored)) if stored.canonical_request_hash() == request_hash => {
            ResolveOutcomeV1::StoredResolution(Box::new(stored))
        }
        Ok(Some(stored)) => ResolveOutcomeV1::Conflict {
            existing_request_hash: stored.canonical_request_hash().clone(),
        },
        Ok(None) => ResolveOutcomeV1::KnownAbsent,
        Err(ReceiptLookupError::Database | ReceiptLookupError::Corrupt) => {
            ResolveOutcomeV1::ResolutionUnavailable
        }
    }
}

fn resolve_guarded(
    connection: &mut Connection,
    identity: &OperationIdentityV1,
    identity_bytes: &[u8],
    request_hash: &CanonicalRequestHashV1,
) -> ResolveOutcomeV1 {
    let Ok(transaction) = connection.transaction_with_behavior(TransactionBehavior::Immediate)
    else {
        return ResolveOutcomeV1::ResolutionUnavailable;
    };
    let outcome = match lookup_receipt(&transaction, identity, identity_bytes) {
        Ok(Some(stored)) if stored.canonical_request_hash() == request_hash => {
            ResolveOutcomeV1::StoredResolution(Box::new(stored))
        }
        Ok(Some(stored)) => ResolveOutcomeV1::Conflict {
            existing_request_hash: stored.canonical_request_hash().clone(),
        },
        Ok(None) => ResolveOutcomeV1::KnownAbsent,
        Err(ReceiptLookupError::Database | ReceiptLookupError::Corrupt) => {
            ResolveOutcomeV1::ResolutionUnavailable
        }
    };
    if transaction.commit().is_ok() {
        outcome
    } else {
        ResolveOutcomeV1::ResolutionUnavailable
    }
}

fn authority_matches(
    transaction: &Transaction<'_>,
    witness: &PreparedAuthorityWitnessV1,
    checked_at: &AuthorityCheckedAt,
) -> Result<bool, RoomCommitResolutionV1> {
    if let Some(query) = witness.authority_snapshot_query() {
        let snapshot =
            load_authority_snapshot(transaction, &query).map_err(|error| match error {
                AuthorityStoreErrorV1::Unavailable => RoomCommitResolutionV1::RetryableKnownAbsent,
                AuthorityStoreErrorV1::Conflict
                | AuthorityStoreErrorV1::StaleGeneration
                | AuthorityStoreErrorV1::Corrupt
                | AuthorityStoreErrorV1::InvalidChange => RoomCommitResolutionV1::Fault,
            })?;
        let Some(snapshot) = snapshot else {
            return Ok(false);
        };
        return match witness.revalidate_current(&snapshot, checked_at) {
            Ok(()) => Ok(true),
            Err(AuthorityStoreErrorV1::StaleGeneration) => Ok(false),
            Err(
                AuthorityStoreErrorV1::Unavailable
                | AuthorityStoreErrorV1::Conflict
                | AuthorityStoreErrorV1::Corrupt
                | AuthorityStoreErrorV1::InvalidChange,
            ) => Err(RoomCommitResolutionV1::Fault),
        };
    }

    #[cfg(test)]
    {
        let Some((witness_id, principal_id, generation, scope_hash)) = witness.conformance_key()
        else {
            return Err(RoomCommitResolutionV1::Fault);
        };
        let generation = i64::try_from(generation)
            .ok()
            .filter(|value| *value <= MAX_SAFE_INTEGER)
            .ok_or(RoomCommitResolutionV1::Fault)?;
        transaction
            .query_row(
                "SELECT EXISTS( \
                 SELECT 1 FROM temp.conformance_authority_fences \
                 WHERE witness_id = ?1 AND authenticated_principal = ?2 AND generation = ?3 \
                 AND scope_revocation_hash = ?4 AND active = 1 \
                 )",
                params![
                    witness_id,
                    principal_id.as_str(),
                    generation,
                    scope_hash.as_bytes().as_slice(),
                ],
                |row| row.get(0),
            )
            .map_err(statement_failure)
    }

    #[cfg(not(test))]
    Err(RoomCommitResolutionV1::Fault)
}

#[cfg(test)]
fn seed_authority_fence(
    connection: &Connection,
    witness: &PreparedAuthorityWitnessV1,
    active: bool,
) -> rusqlite::Result<()> {
    let Some((witness_id, principal_id, generation, scope_hash)) = witness.conformance_key() else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    connection.execute(
        "INSERT INTO temp.conformance_authority_fences(\
         witness_id, authenticated_principal, generation, scope_revocation_hash, active\
         ) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(witness_id) DO UPDATE SET \
         authenticated_principal = excluded.authenticated_principal, \
         generation = excluded.generation, \
         scope_revocation_hash = excluded.scope_revocation_hash, \
         active = excluded.active",
        params![
            witness_id,
            principal_id.as_str(),
            i64::try_from(generation).unwrap_or(MAX_SAFE_INTEGER),
            scope_hash.as_bytes().as_slice(),
            i64::from(active),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
fn seed_authority_snapshot(
    connection: &mut Connection,
    principal: &PrincipalAuthoritySnapshotV1,
    capability: &CapabilityAuthoritySnapshotV1,
) -> Result<(), AuthorityStoreErrorV1> {
    if capability.principal_id() != principal.principal_id() {
        return Err(AuthorityStoreErrorV1::InvalidChange);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(authority_sql_failure)?;
    transaction
        .execute(
            "INSERT INTO principals(\
             principal_id, principal_kind, authority_status, principal_generation\
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                principal.principal_id().as_str(),
                principal_kind(principal.kind()),
                principal_authority_status(principal.status()),
                to_i64_authority(principal.generation().get())?,
            ],
        )
        .map_err(authority_write_failure)?;
    insert_capability(&transaction, capability)?;
    transaction.commit().map_err(authority_sql_failure)
}

#[cfg(test)]
fn guarded_commit_pause() -> &'static (Mutex<GuardedCommitPause>, Condvar) {
    GUARDED_COMMIT_PAUSE.get_or_init(|| (Mutex::new(GuardedCommitPause::default()), Condvar::new()))
}

#[cfg(test)]
fn arm_guarded_commit_pause() {
    let (state, _) = guarded_commit_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("guarded-commit pause mutex"));
    *state = GuardedCommitPause::default();
}

#[cfg(test)]
fn pause_after_guard() {
    let (state, changed) = guarded_commit_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("guarded-commit pause mutex"));
    state.reached = true;
    changed.notify_all();
    while !state.released {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("guarded-commit pause wait"));
        state = next;
        assert!(!timeout.timed_out(), "guarded commit was never released");
    }
}

#[cfg(test)]
fn wait_until_guarded_commit_pauses() {
    let (state, changed) = guarded_commit_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("guarded-commit pause mutex"));
    while !state.reached {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("guarded-commit reached wait"));
        state = next;
        assert!(!timeout.timed_out(), "writer never reached identity guard");
    }
}

#[cfg(test)]
fn release_guarded_commit() {
    let (state, changed) = guarded_commit_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("guarded-commit pause mutex"));
    state.released = true;
    changed.notify_all();
}

#[cfg(test)]
fn writer_queue_pause() -> &'static (Mutex<GuardedCommitPause>, Condvar) {
    WRITER_QUEUE_PAUSE.get_or_init(|| (Mutex::new(GuardedCommitPause::default()), Condvar::new()))
}

#[cfg(test)]
fn arm_writer_queue_pause() {
    let (state, _) = writer_queue_pause();
    *state
        .lock()
        .unwrap_or_else(|_| panic!("writer-queue pause mutex")) = GuardedCommitPause::default();
    let (enqueued, _) =
        AUTHORITY_CHANGE_ENQUEUED.get_or_init(|| (Mutex::new(false), Condvar::new()));
    *enqueued
        .lock()
        .unwrap_or_else(|_| panic!("authority enqueue mutex")) = false;
}

#[cfg(test)]
fn pause_writer_queue() {
    let (state, changed) = writer_queue_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("writer-queue pause mutex"));
    state.reached = true;
    changed.notify_all();
    while !state.released {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("writer-queue pause wait"));
        state = next;
        assert!(!timeout.timed_out(), "writer queue was never released");
    }
}

#[cfg(test)]
fn wait_until_writer_queue_pauses() {
    let (state, changed) = writer_queue_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("writer-queue pause mutex"));
    while !state.reached {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("writer-queue reached wait"));
        state = next;
        assert!(!timeout.timed_out(), "writer never paused its queue");
    }
}

#[cfg(test)]
fn release_writer_queue() {
    let (state, changed) = writer_queue_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("writer-queue pause mutex"));
    state.released = true;
    changed.notify_all();
}

#[cfg(test)]
fn mark_authority_change_enqueued() {
    let (state, changed) =
        AUTHORITY_CHANGE_ENQUEUED.get_or_init(|| (Mutex::new(false), Condvar::new()));
    *state
        .lock()
        .unwrap_or_else(|_| panic!("authority enqueue mutex")) = true;
    changed.notify_all();
}

#[cfg(test)]
fn wait_until_authority_change_enqueued() {
    let (state, changed) =
        AUTHORITY_CHANGE_ENQUEUED.get_or_init(|| (Mutex::new(false), Condvar::new()));
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("authority enqueue mutex"));
    while !*state {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("authority enqueue wait"));
        state = next;
        assert!(!timeout.timed_out(), "authority change was never enqueued");
    }
}

#[cfg(test)]
fn recovery_install_pause() -> &'static (Mutex<RecoveryInstallPause>, Condvar) {
    RECOVERY_INSTALL_PAUSE
        .get_or_init(|| (Mutex::new(RecoveryInstallPause::default()), Condvar::new()))
}

#[cfg(test)]
fn arm_recovery_install_pause(path: &Path) {
    let (state, _) = recovery_install_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("recovery-install pause mutex"));
    *state = RecoveryInstallPause {
        target_path: Some(path.to_path_buf()),
        reached: false,
        released: false,
    };
}

#[cfg(test)]
fn pause_before_recovery_install_guard(path: &Path) {
    let Some((state, changed)) = RECOVERY_INSTALL_PAUSE.get() else {
        return;
    };
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("recovery-install pause mutex"));
    if state.target_path.as_deref() != Some(path) || state.released {
        return;
    }
    state.reached = true;
    changed.notify_all();
    while !state.released {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("recovery-install pause wait"));
        state = next;
        assert!(!timeout.timed_out(), "recovery install was never released");
    }
}

#[cfg(test)]
fn wait_until_recovery_install_pauses() {
    let (state, changed) = recovery_install_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("recovery-install pause mutex"));
    while !state.reached {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("recovery-install reached wait"));
        state = next;
        assert!(!timeout.timed_out(), "recovery never reached install guard");
    }
}

#[cfg(test)]
fn release_recovery_install() {
    let (state, changed) = recovery_install_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("recovery-install pause mutex"));
    state.released = true;
    changed.notify_all();
}

#[cfg(test)]
fn replay_projection_pause() -> &'static (Mutex<ReplayProjectionPause>, Condvar) {
    REPLAY_PROJECTION_PAUSE
        .get_or_init(|| (Mutex::new(ReplayProjectionPause::default()), Condvar::new()))
}

#[cfg(test)]
fn arm_replay_projection_pause(path: &Path) {
    let (state, _) = replay_projection_pause();
    *state
        .lock()
        .unwrap_or_else(|_| panic!("Replay-projection pause mutex")) = ReplayProjectionPause {
        target_path: Some(path.to_path_buf()),
        reached: false,
        released: false,
    };
}

#[cfg(test)]
fn pause_before_replay_projection(path: &Path) {
    let Some((state, changed)) = REPLAY_PROJECTION_PAUSE.get() else {
        return;
    };
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("Replay-projection pause mutex"));
    if state.target_path.as_deref() != Some(path) || state.released {
        return;
    }
    state.reached = true;
    changed.notify_all();
    while !state.released {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("Replay-projection pause wait"));
        state = next;
        assert!(!timeout.timed_out(), "Replay never reached projection");
    }
}

#[cfg(test)]
fn wait_until_replay_projection_pauses() {
    let (state, changed) = replay_projection_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("Replay-projection pause mutex"));
    while !state.reached {
        let (next, timeout) = changed
            .wait_timeout(state, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("Replay-projection reached wait"));
        state = next;
        assert!(!timeout.timed_out(), "Replay never captured its prefix");
    }
}

#[cfg(test)]
fn release_replay_projection() {
    let (state, changed) = replay_projection_pause();
    let mut state = state
        .lock()
        .unwrap_or_else(|_| panic!("Replay-projection pause mutex"));
    state.released = true;
    changed.notify_all();
}

#[cfg(test)]
fn set_replay_slice_row_budget(path: &Path, max_rows: usize) {
    let state = REPLAY_SLICE_BUDGET.get_or_init(|| Mutex::new(ReplaySliceBudget::default()));
    state
        .lock()
        .unwrap_or_else(|_| panic!("Replay-slice budget mutex"))
        .by_path
        .insert(path.to_path_buf(), max_rows.max(1));
}

#[cfg(test)]
fn clear_replay_slice_row_budget(path: &Path) {
    if let Some(state) = REPLAY_SLICE_BUDGET.get() {
        state
            .lock()
            .unwrap_or_else(|_| panic!("Replay-slice budget mutex"))
            .by_path
            .remove(path);
    }
}

#[cfg(test)]
fn expire_deferred_replay_sessions(store: &SqliteRoomStore) {
    let expired_at = Instant::now()
        .checked_sub(REPLAY_SESSION_IDLE_EXPIRY + Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut sessions = store
        .writer
        .replay_sessions
        .lock()
        .unwrap_or_else(|_| panic!("Replay-session registry mutex"));
    for entry in sessions.by_id.values_mut() {
        entry.last_used_at = expired_at;
    }
}

#[cfg(test)]
fn serialize_replay_projection_test() -> std::sync::MutexGuard<'static, ()> {
    REPLAY_PROJECTION_TEST_SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|_| panic!("Replay-projection test serial mutex"))
}

#[allow(clippy::unnecessary_wraps)]
fn fail_at(
    failpoint: Option<WriteBoundary>,
    boundary: WriteBoundary,
) -> Result<(), RoomCommitResolutionV1> {
    #[cfg(test)]
    if failpoint == Some(boundary) {
        return Err(RoomCommitResolutionV1::RetryableKnownAbsent);
    }
    let _ = (failpoint, boundary);
    Ok(())
}

fn statement_failure(error: rusqlite::Error) -> RoomCommitResolutionV1 {
    let error_code = error.sqlite_error_code();
    drop(error);
    match error_code {
        Some(
            ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseLocked
            | ErrorCode::OutOfMemory
            | ErrorCode::ReadOnly
            | ErrorCode::OperationInterrupted
            | ErrorCode::SystemIoFailure
            | ErrorCode::DiskFull
            | ErrorCode::CannotOpen
            | ErrorCode::FileLockingProtocolFailed
            | ErrorCode::SchemaChanged,
        ) => RoomCommitResolutionV1::RetryableKnownAbsent,
        _ => RoomCommitResolutionV1::Fault,
    }
}

fn to_i64(value: u64) -> Result<i64, RoomCommitResolutionV1> {
    i64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or(RoomCommitResolutionV1::Fault)
}

const fn room_status(value: RoomStatusV1) -> &'static str {
    match value {
        RoomStatusV1::Active => "active",
        RoomStatusV1::Archived => "archived",
    }
}

const fn principal_kind(value: PrincipalKindV1) -> &'static str {
    match value {
        PrincipalKindV1::Human => "human",
        PrincipalKindV1::Agent => "agent",
    }
}

const fn membership_standing(value: MembershipStandingV1) -> &'static str {
    match value {
        MembershipStandingV1::Enabled => "enabled",
        MembershipStandingV1::Suspended => "suspended",
        MembershipStandingV1::Departed => "departed",
    }
}

const fn access_mode(value: AccessModeV1) -> &'static str {
    match value {
        AccessModeV1::Participant => "participant",
        AccessModeV1::Spectator => "spectator",
        AccessModeV1::Operator => "operator",
    }
}

/// Failure to capture a trustworthy immutable Room-history inspection.
#[derive(Debug, Error)]
enum SqliteRoomInspectionErrorV1 {
    #[error("SQLite inspection error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("SQLite inspection is unavailable")]
    Unavailable,
    #[error("durable Room history or a materialized projection is inconsistent")]
    Corrupt,
    #[error("Room integrity is not healthy and inspect/replay is withheld")]
    IntegrityUnavailable,
}

/// Safe caller-visible failure for the `SQLite` authorized-Replay facade.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SqliteAuthorizedReplayErrorV1 {
    #[error("authorized Replay authority was denied: {0}")]
    Authority(AuthorityErrorV1),
    #[error("authorized Replay storage is unavailable")]
    StorageUnavailable,
    #[error("durable authorized Replay history is inconsistent")]
    Corrupt,
    #[error("authorized Replay failed: {0}")]
    Replay(HistoricalReplayErrorV1),
}

/// Storage startup failure. A mismatched engine or migration never enters
/// Room serving.
#[derive(Debug, Error)]
pub enum SqliteStoreOpenError {
    #[error("SQLite error: {0}")]
    Sqlite(#[source] rusqlite::Error),
    #[error("SQLite path error: {0}")]
    Path(#[source] std::io::Error),
    #[error("failed to spawn SQLite writer thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    #[error("SQLite writer exited during startup")]
    WriterStartup,
    #[error("SQLite writer registry is poisoned")]
    WriterRegistryPoisoned,
    #[error("SQLite path is not a local regular owner-bound file")]
    UnsafePath,
    #[error("another process already owns the SQLite writer lease")]
    WriterAlreadyOwned,
    #[error(
        "bundled SQLite identity mismatch: version {actual_version}, source {actual_source_id}"
    )]
    EngineIdentity {
        actual_version: String,
        actual_source_id: String,
    },
    #[error("SQLite refused WAL journal mode and returned {0}")]
    JournalMode(String),
    #[error(
        "SQLite pragma mismatch: foreign_keys={foreign_keys}, synchronous={synchronous}, busy_timeout={busy_timeout}"
    )]
    PragmaMismatch {
        foreign_keys: i64,
        synchronous: i64,
        busy_timeout: i64,
    },
    #[error("SQLite integrity_check failed: {0}")]
    IntegrityCheck(String),
    #[error("SQLite foreign_key_check reported a violation")]
    ForeignKeyCheck,
    #[error("migration version {version} is {found}, expected {expected}")]
    MigrationIdentity {
        version: i64,
        expected: &'static str,
        found: String,
    },
    #[error("database has an unsupported migration set")]
    UnexpectedMigrationSet,
    #[error("stored Room history cannot be safely upgraded to migration version 2")]
    MigrationData,
    #[error("database schema differs from the frozen Room ledger DDL")]
    SchemaMismatch,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fmt::Display,
        str::FromStr,
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };

    use rusqlite::{Connection, TransactionBehavior, params, types::ValueRef};
    use tempfile::NamedTempFile;
    use worldstream_core::{
        AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityPackOperationV1,
        ActorInstallationV1, AdministrationOperationIdentityV1, AuthorityBootstrapV1,
        AuthorityChangeResultV1, AuthorityChangeTargetV1, AuthorityChangeV1, AuthorityCheckedAt,
        AuthorityErrorV1, AuthorityGenerationV1, AuthorityGrantV1, AuthorityReasonCodeV1,
        AuthorityUseV1, AuthorityV1, AuthorizedParticipantActionV1,
        AuthorizedStableActionDispositionV1, CORE_OPERATION_KIND, CREATE_ROOM_OPERATION_KIND,
        CanonicalJsonV1, CapabilityAuthoritySnapshotPartsV1, CapabilityAuthoritySnapshotV1,
        CapabilityBearerV1, CapabilityId, CapabilityProfileV1, CapabilityScopeSetV1,
        CapabilityScopeV1, CoreAdministrationIngressV1, CoreAdministrationRequestV1,
        CoreAuthorityAttributionV1, CoreAuthorityKindV1, CoreChangeSetV1, CoreProposedKindV1,
        CoreProposedV1, CoreRoomStateV1, CoreTraceV1, ExistingRoomPendingAttemptV1,
        ExistingRoomReprepareV1, GenesisInputV1, HistoricalReplayErrorV1,
        InitialMembershipProposalV1, IntegrityGenerationV1, MemberAuthorityUseV1,
        MembershipChangeV1, MembershipStandingV1, MembershipV1, NewCapabilityV1,
        OperationIdentityV1, PackGenesisRequestV1, PackRegistryV1, ParticipantActionAuthorityV1,
        ParticipantActionOperationIdentityV1, ParticipantActionRequestV1, ParticipantActionV1,
        PrepareRoomWriteErrorV1, PreparedAuthorityWitnessV1, PreparedExistingIntentV1,
        PreparedNewRoomGenesisV1, PreparedRoomCommitV1, PreparedRoomCreationV1,
        PreparedRoomWriteV1, PresentedCapabilityV1, PrincipalAuthoritySnapshotV1,
        PrincipalAuthorityStatusV1, PrincipalGenerationV1, PrincipalKindV1, RecordedStimulusV1,
        RecoveredRoomMaterializationsV1, RecoveryIntegrityDispositionV1, ReplayFailureClassV1,
        ReplayProjectionKindV1, ResolutionStatusV1, ResolveOutcomeV1, RoomCommitResolutionV1,
        RoomCommitStorageV1, RoomCreationIngressV1, RoomCreationPendingAttemptV1,
        RoomCreationRequestV1, RoomIntegrityStatusV1, RoomMembershipKeyV1, RoomRecoveryErrorV1,
        RoomRecoveryStorageV1, RoomSeedV1, RoomSequenceV1, RoomStatusV1, RunnerControlOperationV1,
        RunnerGenerationV1, RunnerMembershipSetV1, ScheduledTimerV1, SemanticResultV1,
        StoredSemanticResultV1, TimerFiredRequestV1, TimerFiredV1, TimerGenerationV1, TransitionId,
        TransitionV1, authorize_core_administration_operation, authorize_room_creation_operation,
        builtin_counter_registry, commit_existing_room as commit_existing_room_at,
        commit_room_creation as commit_room_creation_at, counter_v1_only_registry_for_conformance,
        counter_v2_digest, counter_v2_invalid_timer_output_registry_for_conformance,
        counter_v2_malformed_output_registry_for_conformance,
        counter_v2_returned_fault_registry_for_conformance,
        counter_v2_runtime_fault_registry_for_conformance,
        counter_v2_semantic_mismatch_registry_for_conformance, recover_room_from_storage,
    };

    use super::{
        AUTHORITY_MIGRATION_ID, INITIAL_MIGRATION_ID, INITIAL_MIGRATION_SCHEMA, SQLITE_SOURCE_ID,
        SQLITE_VERSION, SqliteAuthorizedReplayErrorV1, SqliteAuthorizedReplayOutcomeV1,
        SqliteAuthorizedReplayProjectionV1, SqliteRoomStore, TestAuthorityClock, WriteBoundary,
        arm_guarded_commit_pause, arm_recovery_install_pause, arm_replay_projection_pause,
        arm_writer_queue_pause, clear_replay_slice_row_budget, expire_deferred_replay_sessions,
        release_guarded_commit, release_recovery_install, release_replay_projection,
        release_writer_queue, serialize_replay_projection_test, set_replay_slice_row_budget,
        wait_until_authority_change_enqueued, wait_until_guarded_commit_pauses,
        wait_until_recovery_install_pauses, wait_until_replay_projection_pauses,
        wait_until_writer_queue_pauses,
    };

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ROOM_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const PARTICIPANT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    const PARTICIPANT_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const PRINCIPAL_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
    const ACTION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
    const ACTION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE1";
    const ACTION_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE2";
    const TRANSITION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF0";
    const TRANSITION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF1";
    const TRANSITION_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF2";
    const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG0";
    const TIMER_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG1";
    const TIMER_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG2";
    const TIMER_TRANSITION_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG3";
    const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";
    const MEMBER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH3";
    const REGISTER_MEMBER_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ4";
    const REVOKE_MEMBER_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ5";
    const NARROW_MEMBER_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ6";
    const DISABLE_MEMBER_PRINCIPAL_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ7";
    const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn parsed<T>(value: &str) -> T
    where
        T: FromStr,
        T::Err: Display,
    {
        value
            .parse()
            .unwrap_or_else(|error| panic!("fixture value {value}: {error}"))
    }

    fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
        CanonicalJsonV1::parse(bytes)
            .unwrap_or_else(|error| panic!("fixture canonical JSON: {error}"))
    }

    fn commit_room_creation(
        storage: &dyn RoomCommitStorageV1,
        prepared: PreparedRoomCreationV1,
    ) -> worldstream_core::RoomCreationCommitOutcomeV1 {
        commit_room_creation_at(storage, prepared)
    }

    fn commit_existing_room(
        storage: &dyn RoomCommitStorageV1,
        trace: &mut CoreTraceV1,
        prepared: PreparedRoomCommitV1,
    ) -> worldstream_core::ExistingRoomCommitOutcomeV1 {
        commit_existing_room_at(storage, trace, prepared)
    }

    fn creation_fixture(
        initial_value: u32,
    ) -> (
        PreparedNewRoomGenesisV1,
        RoomCreationRequestV1,
        PreparedAuthorityWitnessV1,
        AdministrationOperationIdentityV1,
    ) {
        creation_fixture_with_generated(
            initial_value,
            ROOM,
            PARTICIPANT,
            SEED,
            "2026-08-15T12:00:00Z",
            "create-fixture-1",
        )
    }

    fn creation_fixture_with_generated(
        initial_value: u32,
        room_id: &str,
        member_id: &str,
        room_seed: &str,
        created_at: &str,
        idempotency_key: &str,
    ) -> (
        PreparedNewRoomGenesisV1,
        RoomCreationRequestV1,
        PreparedAuthorityWitnessV1,
        AdministrationOperationIdentityV1,
    ) {
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Counter registry: {error}"));
        let participant = MembershipV1::new(
            parsed(member_id),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| panic!("participant fixture: {error}"));
        let core = CoreRoomStateV1::active([participant])
            .unwrap_or_else(|error| panic!("Core fixture: {error}"));
        let configuration = canonical(
            format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes(),
        );
        let request = PackGenesisRequestV1 {
            room_id: parsed(room_id),
            pack_digest: counter_v2_digest(),
            configuration: configuration.clone(),
            room_seed: parsed::<RoomSeedV1>(room_seed),
            created_at: parsed(created_at),
            initial_core_state: core,
        };
        let prepared = registry
            .prepare_genesis_for_new_room(&request)
            .unwrap_or_else(|error| panic!("checked Counter Genesis: {error}"));
        let creation_request = RoomCreationRequestV1::new(
            counter_v2_digest(),
            configuration,
            vec![
                InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )
                .unwrap_or_else(|error| panic!("initial proposal: {error}")),
            ],
        );
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "create-capability-fixture",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"create_room","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("authority witness: {error}"));
        let identity = AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        };
        (prepared, creation_request, witness, identity)
    }

    fn prepared_creation(
        initial_value: u32,
    ) -> (
        PreparedRoomWriteV1,
        PreparedAuthorityWitnessV1,
        AdministrationOperationIdentityV1,
    ) {
        let (genesis, request, witness, identity) = creation_fixture(initial_value);
        let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity.clone(),
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("prepared creation: {error}"));
        (prepared.into(), witness, identity)
    }

    fn prepared_two_member_creation() -> (PreparedRoomWriteV1, PreparedAuthorityWitnessV1) {
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Counter registry: {error}"));
        let participant = MembershipV1::new(
            parsed(PARTICIPANT),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| panic!("participant fixture: {error}"));
        let spectator = MembershipV1::new(
            parsed(PARTICIPANT_ALT),
            parsed(PRINCIPAL_ALT),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Spectator,
            None,
        )
        .unwrap_or_else(|error| panic!("spectator fixture: {error}"));
        let configuration = canonical(br#"{"initial_value":0,"maximum_value":4}"#);
        let genesis = registry
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id: parsed(ROOM),
                pack_digest: counter_v2_digest(),
                configuration: configuration.clone(),
                room_seed: parsed(SEED),
                created_at: parsed("2026-08-15T12:00:00Z"),
                initial_core_state: CoreRoomStateV1::active([participant, spectator])
                    .unwrap_or_else(|error| panic!("two-member Core fixture: {error}")),
            })
            .unwrap_or_else(|error| panic!("two-member Counter Genesis: {error}"));
        let request = RoomCreationRequestV1::new(
            counter_v2_digest(),
            configuration,
            vec![
                InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )
                .unwrap_or_else(|error| panic!("participant proposal: {error}")),
                InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL_ALT),
                    PrincipalKindV1::Agent,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Spectator,
                    None,
                )
                .unwrap_or_else(|error| panic!("spectator proposal: {error}")),
            ],
        );
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "create-two-member-capability",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"create_room","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("two-member authority: {error}"));
        let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
                idempotency_key: "create-two-member-fixture".to_owned(),
            },
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("prepare two-member creation: {error}"));
        (prepared.into(), witness)
    }

    fn committed_trace(store: &SqliteRoomStore) -> (CoreTraceV1, PreparedAuthorityWitnessV1) {
        let (genesis, request, witness, identity) = creation_fixture(0);
        let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity,
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("prepared creation: {error}"));
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed authority: {error}"));
        let outcome = commit_room_creation(store, prepared);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let trace = outcome
            .into_committed_trace()
            .unwrap_or_else(|| panic!("new durable Genesis releases trace"));
        (trace, witness)
    }

    fn presented_host_capability() -> PresentedCapabilityV1 {
        PresentedCapabilityV1::new(
            parsed(HOST_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xA7; 32]),
        )
    }

    fn presented_member_capability() -> PresentedCapabilityV1 {
        PresentedCapabilityV1::new(
            parsed(MEMBER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xB8; 32]),
        )
    }

    fn seed_real_host_authority(store: &SqliteRoomStore) -> AuthorityV1 {
        let principal = PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("host Principal generation: {error}")),
        );
        let capability = CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
            capability_id: parsed(HOST_CAPABILITY),
            token_hash: CapabilityBearerV1::from_bytes([0xA7; 32]).token_hash(),
            principal_id: parsed(PRINCIPAL),
            profile: CapabilityProfileV1::HostOperator { room_id: None },
            scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])
                .unwrap_or_else(|error| panic!("host Capability scopes: {error}")),
            generation: AuthorityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("host Capability generation: {error}")),
            expires_at: None,
            revoked_at: None,
        })
        .unwrap_or_else(|error| panic!("host Capability: {error}"));
        store
            .seed_authority_snapshot(principal, capability)
            .unwrap_or_else(|error| panic!("seed real host authority: {error}"));
        AuthorityV1::new(Arc::new(store.clone()))
    }

    fn creation_fixture_for_standing(
        standing: MembershipStandingV1,
    ) -> (
        PreparedNewRoomGenesisV1,
        RoomCreationRequestV1,
        AdministrationOperationIdentityV1,
    ) {
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Counter registry: {error}"));
        let configuration = canonical(br#"{"initial_value":0,"maximum_value":4}"#);
        let participant = MembershipV1::new(
            parsed(PARTICIPANT),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            standing,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| panic!("real-authority participant: {error}"));
        let prepared = registry
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id: parsed(ROOM),
                pack_digest: counter_v2_digest(),
                configuration: configuration.clone(),
                room_seed: parsed(SEED),
                created_at: parsed("2026-08-15T12:00:00Z"),
                initial_core_state: CoreRoomStateV1::active([participant])
                    .unwrap_or_else(|error| panic!("real-authority Core: {error}")),
            })
            .unwrap_or_else(|error| panic!("real-authority Genesis: {error}"));
        let request = RoomCreationRequestV1::new(
            counter_v2_digest(),
            configuration,
            vec![
                InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Human,
                    standing,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )
                .unwrap_or_else(|error| panic!("real-authority proposal: {error}")),
            ],
        );
        let identity = AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
            idempotency_key: "create-real-authority".to_owned(),
        };
        (prepared, request, identity)
    }

    fn register_member_capability(
        authority: &AuthorityV1,
        scopes: impl IntoIterator<Item = CapabilityScopeV1>,
        expires_at: Option<&str>,
    ) {
        let capability = NewCapabilityV1::new(
            parsed(MEMBER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xB8; 32]).token_hash(),
            parsed(PRINCIPAL),
            CapabilityProfileV1::RoomMember {
                room_id: parsed(ROOM),
                member_id: parsed(PARTICIPANT),
            },
            CapabilityScopeSetV1::new(scopes)
                .unwrap_or_else(|error| panic!("member Capability scopes: {error}")),
            expires_at.map(parsed),
        )
        .unwrap_or_else(|error| panic!("member Capability: {error}"));
        let receipt = authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RegisterCapability {
                    change_id: parsed(REGISTER_MEMBER_CAPABILITY_CHANGE),
                    capability,
                },
                parsed("2026-08-15T12:00:02Z"),
            )
            .unwrap_or_else(|error| panic!("register member Capability: {error}"));
        assert_eq!(
            receipt.result(),
            AuthorityChangeResultV1::CapabilityRegistered
        );
    }

    fn committed_trace_with_real_authority(
        store: &SqliteRoomStore,
        standing: MembershipStandingV1,
        member_scopes: impl IntoIterator<Item = CapabilityScopeV1>,
        member_expires_at: Option<&str>,
    ) -> (CoreTraceV1, AuthorityV1) {
        let authority = seed_real_host_authority(store);
        let (genesis, request, identity) = creation_fixture_for_standing(standing);
        let grant = match authorize_room_creation_operation(
            &authority,
            store,
            &presented_host_capability(),
            &identity,
            &request,
            parsed("2026-08-15T12:00:00Z"),
        )
        .unwrap_or_else(|error| panic!("authorize Room creation: {error}"))
        {
            RoomCreationIngressV1::Authorized(grant) => grant,
            other => panic!("new Room creation unexpectedly resolved a receipt: {other:?}"),
        };
        let prepared =
            PreparedRoomCreationV1::from_registry_genesis(identity, &request, *grant, genesis)
                .unwrap_or_else(|error| panic!("seal authorized Room creation: {error}"));
        let outcome = commit_room_creation_at(store, prepared);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let trace = outcome
            .into_committed_trace()
            .unwrap_or_else(|| panic!("durable authorized Genesis releases its trace"));
        register_member_capability(&authority, member_scopes, member_expires_at);
        (trace, authority)
    }

    fn commit_host_authorized_creation_receipt(
        store: &SqliteRoomStore,
        authority: &AuthorityV1,
        presented: &PresentedCapabilityV1,
    ) -> (
        OperationIdentityV1,
        worldstream_core::CanonicalRequestHashV1,
    ) {
        let (genesis, request, identity) =
            creation_fixture_for_standing(MembershipStandingV1::Enabled);
        let grant = match authorize_room_creation_operation(
            authority,
            store,
            presented,
            &identity,
            &request,
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap_or_else(|error| panic!("authorize receipt fixture Create: {error}"))
        {
            RoomCreationIngressV1::Authorized(grant) => grant,
            other => panic!("new receipt fixture unexpectedly resolved a receipt: {other:?}"),
        };
        let prepared =
            PreparedRoomCreationV1::from_registry_genesis(identity, &request, *grant, genesis)
                .unwrap_or_else(|error| panic!("prepare receipt fixture Create: {error}"));
        let operation_identity = prepared.semantic_result().operation_identity().clone();
        let request_hash = prepared.semantic_result().canonical_request_hash().clone();
        assert!(matches!(
            commit_room_creation_at(store, prepared).resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        (operation_identity, request_hash)
    }

    fn install_conformance_replay_access_change(
        connection: &Connection,
        trace: &mut CoreTraceV1,
        transition_id: &str,
    ) {
        let transition = advance_conformance_replay_access_change(trace);
        install_conformance_replay_transition(
            connection,
            trace,
            &transition,
            transition_id,
            &BTreeMap::from([(PARTICIPANT.to_owned(), 2_i64)]),
        );
    }

    fn advance_conformance_replay_access_change(trace: &mut CoreTraceV1) -> TransitionV1 {
        let before = trace
            .core_state()
            .membership(&parsed(PARTICIPANT))
            .unwrap_or_else(|| panic!("Replay Membership exists"))
            .clone();
        let stimulus = RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
            CoreProposedKindV1::AccessModeChange,
            CoreAuthorityAttributionV1 {
                principal_id: parsed(PRINCIPAL),
                authority_kind: CoreAuthorityKindV1::HostOperator,
            },
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "sqlite-replay-access-history".to_owned(),
            },
            trace.head().room_seq(),
            "sqlite_replay_spectator",
            parsed("2026-08-15T12:00:04Z"),
            CoreChangeSetV1::one(
                MembershipChangeV1::access_mode_change(before, AccessModeV1::Spectator, None)
                    .unwrap_or_else(|error| panic!("Replay Access change: {error}")),
            ),
        ));
        match trace
            .advance_for_conformance(stimulus)
            .unwrap_or_else(|error| panic!("advance conformance Replay Access history: {error}"))
        {
            worldstream_core::AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                transition,
            } => *transition,
            other => panic!("Replay Access history must advance, got {other:?}"),
        }
    }

    fn install_conformance_replay_member_join(
        connection: &Connection,
        trace: &mut CoreTraceV1,
        transition_id: &str,
    ) {
        let joined = MembershipV1::new(
            parsed(PARTICIPANT_ALT),
            parsed(PRINCIPAL_ALT),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Spectator,
            None,
        )
        .unwrap_or_else(|error| panic!("Replay joined Membership: {error}"));
        let stimulus = RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
            CoreProposedKindV1::Join,
            CoreAuthorityAttributionV1 {
                principal_id: parsed(PRINCIPAL),
                authority_kind: CoreAuthorityKindV1::HostOperator,
            },
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "sqlite-replay-member-join".to_owned(),
            },
            trace.head().room_seq(),
            "sqlite_replay_member_join",
            parsed("2026-08-15T12:00:05Z"),
            CoreChangeSetV1::one(MembershipChangeV1::join(joined)),
        ));
        let transition = match trace
            .advance_for_conformance(stimulus)
            .unwrap_or_else(|error| panic!("advance conformance Replay Join history: {error}"))
        {
            worldstream_core::AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                transition,
            } => *transition,
            other => panic!("Replay Join history must advance, got {other:?}"),
        };
        install_conformance_replay_transition(
            connection,
            trace,
            &transition,
            transition_id,
            &BTreeMap::from([
                (PARTICIPANT.to_owned(), 2_i64),
                (PARTICIPANT_ALT.to_owned(), 1_i64),
            ]),
        );
    }

    fn install_conformance_replay_neutral_membership_history(
        connection: &Connection,
        trace: &mut CoreTraceV1,
        join_transition_id: &str,
        suspend_transition_id: &str,
    ) {
        install_conformance_replay_member_join(connection, trace, join_transition_id);
        connection
            .execute(
                "UPDATE room_members SET membership_generation = 1 \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
            )
            .unwrap_or_else(|error| panic!("restore unchanged Membership generation: {error}"));
        let before = trace
            .core_state()
            .membership(&parsed(PARTICIPANT_ALT))
            .unwrap_or_else(|| panic!("joined Replay Membership exists"))
            .clone();
        let stimulus = RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
            CoreProposedKindV1::Suspend,
            CoreAuthorityAttributionV1 {
                principal_id: parsed(PRINCIPAL),
                authority_kind: CoreAuthorityKindV1::HostOperator,
            },
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "sqlite-replay-member-suspend".to_owned(),
            },
            trace.head().room_seq(),
            "sqlite_replay_member_suspend",
            parsed("2026-08-15T12:00:06Z"),
            CoreChangeSetV1::one(MembershipChangeV1::suspend(before)),
        ));
        let transition = match trace
            .advance_for_conformance(stimulus)
            .unwrap_or_else(|error| panic!("advance conformance Replay Suspend: {error}"))
        {
            worldstream_core::AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                transition,
            } => *transition,
            other => panic!("Replay Suspend history must advance, got {other:?}"),
        };
        install_conformance_replay_transition(
            connection,
            trace,
            &transition,
            suspend_transition_id,
            &BTreeMap::from([
                (PARTICIPANT.to_owned(), 1_i64),
                (PARTICIPANT_ALT.to_owned(), 2_i64),
            ]),
        );
    }

    fn install_conformance_replay_member_resume(
        connection: &Connection,
        trace: &mut CoreTraceV1,
        transition_id: &str,
    ) {
        let before = trace
            .core_state()
            .membership(&parsed(PARTICIPANT_ALT))
            .unwrap_or_else(|| panic!("suspended Replay Membership exists"))
            .clone();
        let stimulus = RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
            CoreProposedKindV1::Resume,
            CoreAuthorityAttributionV1 {
                principal_id: parsed(PRINCIPAL),
                authority_kind: CoreAuthorityKindV1::HostOperator,
            },
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "sqlite-replay-member-resume".to_owned(),
            },
            trace.head().room_seq(),
            "sqlite_replay_member_resume",
            parsed("2026-08-15T12:00:07Z"),
            CoreChangeSetV1::one(MembershipChangeV1::resume(before)),
        ));
        let transition = match trace
            .advance_for_conformance(stimulus)
            .unwrap_or_else(|error| panic!("advance conformance Replay Resume: {error}"))
        {
            worldstream_core::AdvanceDispositionV1::TransitionAccepted {
                existing: false,
                transition,
            } => *transition,
            other => panic!("Replay Resume history must advance, got {other:?}"),
        };
        install_conformance_replay_transition(
            connection,
            trace,
            &transition,
            transition_id,
            &BTreeMap::from([
                (PARTICIPANT.to_owned(), 1_i64),
                (PARTICIPANT_ALT.to_owned(), 3_i64),
            ]),
        );
    }

    fn install_conformance_replay_transition(
        connection: &Connection,
        trace: &CoreTraceV1,
        transition: &TransitionV1,
        transition_id: &str,
        membership_generations: &BTreeMap<String, i64>,
    ) {
        let head = transition.complete_head();
        let basis = if transition.room_seq().get() == 1 {
            trace.genesis().complete_head()
        } else {
            let previous_index = usize::try_from(transition.room_seq().get() - 2)
                .unwrap_or_else(|_| panic!("Replay prior Transition index"));
            trace
                .transitions()
                .get(previous_index)
                .unwrap_or_else(|| panic!("Replay prior Transition"))
                .complete_head()
        };
        let RecordedStimulusV1::CoreProposed(proposal) = transition.recorded_stimulus() else {
            panic!("Replay conformance fixture requires Core administration");
        };
        let receipt = StoredSemanticResultV1::from_core_transition_for_conformance(
            &basis,
            proposal,
            parsed(transition_id),
            transition,
        )
        .unwrap_or_else(|error| panic!("Replay conformance receipt: {error}"));
        let canonical_transition_bytes = transition
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("encode Replay Transition: {error}"));
        let canonical_core_state_bytes = trace
            .core_state()
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("encode Replay current Core: {error}"));
        let canonical_activity_state_bytes = trace
            .activity_state()
            .to_bytes()
            .unwrap_or_else(|error| panic!("encode Replay current Activity: {error}"));
        connection
            .execute(
                "INSERT INTO transitions( \
                     room_id, transition_id, room_seq, transition_hash, previous_lineage_hash, \
                     core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
                     authoritative_state_hash, transition_bytes \
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    ROOM,
                    transition_id,
                    i64::try_from(transition.room_seq().get())
                        .unwrap_or_else(|_| panic!("Replay Transition sequence")),
                    transition.transition_hash().to_string(),
                    transition.previous_lineage_hash().to_string(),
                    head.core_schema_version(),
                    head.pack_digest().to_string(),
                    transition.resulting_core_state_hash().to_string(),
                    transition.resulting_activity_state_hash().to_string(),
                    transition.resulting_authoritative_state_hash().to_string(),
                    canonical_transition_bytes,
                ],
            )
            .unwrap_or_else(|error| panic!("install Replay Transition: {error}"));
        connection
            .execute(
                "INSERT INTO semantic_receipts( \
                     room_id, operation_kind, operation_identity_bytes, codec_id, \
                     canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
                     semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes \
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    ROOM,
                    receipt.operation_identity().operation_kind(),
                    receipt
                        .operation_identity()
                        .canonical_bytes()
                        .unwrap_or_else(|error| panic!("Replay receipt identity: {error}")),
                    super::OPERATION_RECEIPT_CODEC_ID,
                    receipt.canonical_request_hash().as_bytes().as_slice(),
                    receipt
                        .basis_complete_head()
                        .and_then(|head| head.canonical_bytes().ok())
                        .unwrap_or_else(|| panic!("Replay receipt basis")),
                    receipt
                        .semantic_input()
                        .canonical_bytes()
                        .unwrap_or_else(|error| panic!("Replay receipt input: {error}")),
                    receipt
                        .canonical_semantic_time_bytes()
                        .unwrap_or_else(|error| panic!("Replay receipt time: {error}")),
                    receipt.resolution_kind(),
                    i64::try_from(
                        receipt
                            .transition_seq()
                            .unwrap_or_else(|| panic!("Replay receipt Transition sequence"))
                            .get()
                    )
                    .unwrap_or_else(|_| panic!("Replay receipt sequence")),
                    receipt.canonical_receipt_bytes(),
                ],
            )
            .unwrap_or_else(|error| panic!("install Replay Transition receipt: {error}"));
        connection
            .execute(
                "UPDATE rooms SET room_status = ?1, room_seq = ?2, \
                 genesis_or_transition_hash = ?3, core_schema_version = ?4, pack_digest = ?5, \
                 core_state_hash = ?6, activity_state_hash = ?7, authoritative_state_hash = ?8, \
                 complete_head_bytes = ?9 WHERE room_id = ?10",
                params![
                    super::room_status(trace.core_state().room_status()),
                    i64::try_from(head.room_seq().get())
                        .unwrap_or_else(|_| panic!("Replay Head sequence")),
                    head.genesis_or_transition_hash().to_string(),
                    head.core_schema_version(),
                    head.pack_digest().to_string(),
                    head.core_state_hash().to_string(),
                    head.activity_state_hash().to_string(),
                    head.authoritative_state_hash().to_string(),
                    head.canonical_bytes()
                        .unwrap_or_else(|error| panic!("encode Replay current Head: {error}")),
                    ROOM
                ],
            )
            .unwrap_or_else(|error| panic!("install Replay current Head: {error}"));
        connection
            .execute(
                "UPDATE room_materializations SET core_state_bytes = ?1, activity_state_bytes = ?2 \
                 WHERE room_id = ?3",
                params![
                    canonical_core_state_bytes,
                    canonical_activity_state_bytes,
                    ROOM
                ],
            )
            .unwrap_or_else(|error| panic!("install Replay current materializations: {error}"));
        for membership in trace.core_state().memberships().values() {
            let membership_json = serde_json::to_vec(membership)
                .unwrap_or_else(|error| panic!("encode Replay current Membership: {error}"));
            let canonical_membership_bytes = canonical(&membership_json)
                .to_bytes()
                .unwrap_or_else(|error| panic!("canonical Replay current Membership: {error}"));
            let generation = membership_generations
                .get(membership.member_id().as_str())
                .copied()
                .unwrap_or_else(|| panic!("Replay Membership generation"));
            connection
                .execute(
                    "INSERT INTO room_members( \
                         room_id, member_id, principal_id, principal_kind, standing, access_mode, \
                         role, membership_bytes, membership_generation, frame_head \
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0) \
                     ON CONFLICT(room_id, member_id) DO UPDATE SET \
                         principal_id = excluded.principal_id, \
                         principal_kind = excluded.principal_kind, standing = excluded.standing, \
                         access_mode = excluded.access_mode, role = excluded.role, \
                         membership_bytes = excluded.membership_bytes, \
                         membership_generation = excluded.membership_generation",
                    params![
                        ROOM,
                        membership.member_id().as_str(),
                        membership.principal_id().as_str(),
                        super::principal_kind(membership.principal_kind()),
                        super::membership_standing(membership.standing()),
                        super::access_mode(membership.access_mode()),
                        membership.role(),
                        canonical_membership_bytes,
                        generation,
                    ],
                )
                .unwrap_or_else(|error| panic!("install Replay current Membership: {error}"));
        }
    }

    fn increment_request(trace: &CoreTraceV1, action_id: &str) -> ParticipantActionRequestV1 {
        ParticipantActionRequestV1::new(
            parsed(ROOM),
            parsed(PARTICIPANT),
            parsed(action_id),
            trace.head().room_seq(),
            "increment",
            canonical(br"{}"),
        )
    }

    fn action_use(request: &ParticipantActionRequestV1, action_id: &str) -> AuthorityUseV1 {
        AuthorityUseV1::Member {
            room_id: parsed(ROOM),
            member_id: parsed(PARTICIPANT),
            operation: MemberAuthorityUseV1::SubmitAction {
                identity: ParticipantActionOperationIdentityV1 {
                    room_id: parsed(ROOM),
                    member_id: parsed(PARTICIPANT),
                    action_id: parsed(action_id),
                },
                request_hash: request
                    .canonical_request_hash()
                    .unwrap_or_else(|error| panic!("Action request hash: {error}")),
                action_type: "increment".to_owned(),
            },
        }
    }

    fn authorize_enabled_action(
        authority: &AuthorityV1,
        request: &ParticipantActionRequestV1,
        action_id: &str,
        checked_at: &str,
    ) -> AuthorizedParticipantActionV1 {
        match authority
            .authorize(
                &presented_member_capability(),
                action_use(request, action_id),
                parsed(checked_at),
            )
            .unwrap_or_else(|error| panic!("authorize participant Action: {error}"))
        {
            AuthorityGrantV1::ParticipantAction(
                ParticipantActionAuthorityV1::EnabledParticipant(grant),
            ) => grant,
            other => panic!("enabled participant Action grant, got {other:?}"),
        }
    }

    fn authorize_stable_action(
        authority: &AuthorityV1,
        request: &ParticipantActionRequestV1,
        action_id: &str,
        checked_at: &str,
    ) -> AuthorizedStableActionDispositionV1 {
        match authority
            .authorize(
                &presented_member_capability(),
                action_use(request, action_id),
                parsed(checked_at),
            )
            .unwrap_or_else(|error| panic!("authorize stable participant Action: {error}"))
        {
            AuthorityGrantV1::ParticipantAction(
                ParticipantActionAuthorityV1::StableMembershipNotEnabled(grant),
            ) => grant,
            other => panic!("stable disabled-Membership grant, got {other:?}"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn seal_authorized_increment(
        trace: &CoreTraceV1,
        request: &ParticipantActionRequestV1,
        action_id: &str,
        transition_id: &str,
        admitted_at: &str,
        frame_head: u64,
        grant: AuthorizedParticipantActionV1,
    ) -> Result<PreparedRoomCommitV1, PrepareRoomWriteErrorV1> {
        let definition = trace
            .retained_pack()
            .unwrap_or_else(|| panic!("registry-bound trace"))
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == "increment")
            .unwrap_or_else(|| panic!("Counter increment definition"));
        let transition = trace
            .prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: parsed(PARTICIPANT),
                action_id: parsed(action_id),
                action_type: "increment".to_owned(),
                payload_schema_digest: definition.payload_schema.schema_digest.clone(),
                canonical_payload: canonical(br"{}"),
                exact_basis_head: trace.head().clone(),
                admitted_at: parsed(admitted_at),
            }))
            .unwrap_or_else(|error| panic!("prepare authorized increment: {error}"));
        PreparedRoomCommitV1::for_authorized_action(
            trace,
            request,
            transition,
            parsed(transition_id),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("integrity generation: {error}")),
            grant,
            &trace
                .core_state()
                .memberships()
                .keys()
                .cloned()
                .map(|member_id| (member_id, frame_head))
                .collect(),
        )
    }

    fn complete_authorized_replay(
        store: &SqliteRoomStore,
        registry: &PackRegistryV1,
        authority: worldstream_core::AuthorizedReplayV1,
    ) -> Result<SqliteAuthorizedReplayProjectionV1, SqliteAuthorizedReplayErrorV1> {
        let mut outcome = store.replay_authorized(registry, authority)?;
        loop {
            match outcome {
                SqliteAuthorizedReplayOutcomeV1::Complete(projection) => return Ok(*projection),
                SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => {
                    outcome = store.resume_replay_authorized(registry, continuation)?;
                }
            }
        }
    }

    fn committed_history_fixture() -> (
        NamedTempFile,
        SqliteRoomStore,
        CoreTraceV1,
        PreparedAuthorityWitnessV1,
    ) {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp history DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open history SQLite: {error}"));
        let (mut trace, witness) = committed_trace(&store);
        let action = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let committed = commit_existing_room(&store, &mut trace, action);
        assert!(matches!(
            committed.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        (file, store, trace, witness)
    }

    fn assert_recovery_corruption_quarantines(mutate: impl FnOnce(&Connection)) {
        let (file, store, mut trace, witness) = committed_history_fixture();
        let candidate = prepared_increment(
            &trace,
            witness,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:02Z",
            1,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open recovery-corruption fixture: {error}"));
        mutate(&connection);
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Counter recovery registry: {error}"));
        assert!(matches!(
            recover_room_from_storage(&store, &registry, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read quarantined integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
        let fenced = commit_existing_room(&store, &mut trace, candidate);
        assert!(matches!(
            fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(fenced.actor_installation(), ActorInstallationV1::Unchanged);
    }

    fn assert_missing_runtime_does_not_mask_corruption(mutate: impl FnOnce(&Connection)) {
        let (file, store, _trace, _witness) = committed_history_fixture();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open missing-runtime corruption fixture: {error}"));
        mutate(&connection);
        let missing_runtime = counter_v1_only_registry_for_conformance()
            .unwrap_or_else(|error| panic!("v1-only registry: {error}"));
        let result = recover_room_from_storage(&store, &missing_runtime, &parsed(ROOM));
        match result {
            Err(RoomRecoveryErrorV1::Corrupt) => {}
            Err(error) => panic!("missing runtime masked structural corruption: {error:?}"),
            Ok(None) => panic!("missing runtime corruption unexpectedly returned no Room"),
            Ok(Some(_)) => panic!("missing runtime corruption unexpectedly recovered the Room"),
        }
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read masked-corruption integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    fn assert_runtime_recovery_faults(registry: &PackRegistryV1) {
        let (file, store, _trace, _witness) = committed_history_fixture();
        assert!(matches!(
            recover_room_from_storage(&store, registry, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::RuntimeFault)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open runtime execution fixture: {error}"));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read runtime execution integrity: {error}"));
        assert_eq!(integrity, ("faulted".to_owned(), 2));
    }

    fn assert_semantic_recovery_mismatch_quarantines(registry: &PackRegistryV1) {
        let (file, store, _trace, _witness) = committed_history_fixture();
        assert!(matches!(
            recover_room_from_storage(&store, registry, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open semantic mismatch fixture: {error}"));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read semantic mismatch integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    fn assert_timer_recovery_guard_quarantines(mutate: impl FnOnce(&Connection)) {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp Timer DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer SQLite: {error}"));
        let trace = timer_trace();
        seed_timer_room(file.path(), &trace);
        let recovered = RecoveredRoomMaterializationsV1::from_trace_for_conformance(&trace)
            .unwrap_or_else(|error| panic!("Timer recovery materializations: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer recovery mutator: {error}"));
        mutate(&connection);
        assert!(matches!(
            RoomRecoveryStorageV1::guard_recovery_install(
                &store,
                &parsed(ROOM),
                trace.head(),
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("integrity generation: {error}")),
                &recovered,
            ),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read Timer recovery integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    fn prepared_increment(
        trace: &CoreTraceV1,
        witness: PreparedAuthorityWitnessV1,
        action_id: &str,
        transition_id: &str,
        admitted_at: &str,
        frame_head: u64,
    ) -> PreparedRoomCommitV1 {
        let request = ParticipantActionRequestV1::new(
            parsed(ROOM),
            parsed(PARTICIPANT),
            parsed(action_id),
            trace.head().room_seq(),
            "increment",
            canonical(br"{}"),
        );
        let definition = trace
            .retained_pack()
            .unwrap_or_else(|| panic!("registry-bound trace"))
            .descriptor()
            .actions
            .iter()
            .find(|definition| definition.action_type == "increment")
            .unwrap_or_else(|| panic!("Counter increment definition"));
        let stimulus = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(PARTICIPANT),
            action_id: parsed(action_id),
            action_type: "increment".to_owned(),
            payload_schema_digest: definition.payload_schema.schema_digest.clone(),
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed(admitted_at),
        });
        let transition = trace
            .prepare(stimulus)
            .unwrap_or_else(|error| panic!("prepare increment: {error}"));
        PreparedRoomCommitV1::for_action_for_conformance(
            trace,
            &request,
            transition,
            parsed::<TransitionId>(transition_id),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("integrity generation: {error}")),
            witness,
            &BTreeMap::from([(parsed(PARTICIPANT), frame_head)]),
        )
        .unwrap_or_else(|error| panic!("seal increment: {error}"))
    }

    fn timer_trace() -> CoreTraceV1 {
        CoreTraceV1::create_for_conformance(
            GenesisInputV1::new(
                parsed(ROOM),
                counter_v2_digest(),
                canonical(br#"{"fixture":"timer"}"#),
                parsed(SEED),
                parsed("2026-08-15T12:00:00Z"),
                CoreRoomStateV1::active(std::iter::empty::<MembershipV1>())
                    .unwrap_or_else(|error| panic!("Timer Core fixture: {error}")),
                canonical(br#"{"fixture":"timer"}"#),
            )
            .with_initial_timers(vec![
                ScheduledTimerV1 {
                    timer_id: parsed(TIMER),
                    generation: TimerGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Timer generation: {error}")),
                    scheduled_for: parsed("2026-08-15T12:30:00Z"),
                    canonical_payload: canonical(br#"{"kind":"deadline"}"#),
                },
                ScheduledTimerV1 {
                    timer_id: parsed(TIMER_ALT),
                    generation: TimerGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Timer generation: {error}")),
                    scheduled_for: parsed("2026-08-15T12:45:00Z"),
                    canonical_payload: canonical(br#"{"kind":"secondary"}"#),
                },
            ]),
            |_| Ok(()),
            |input| {
                Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                    next_activity_state: input.prior_activity_state.clone(),
                    ordered_domain_events: Vec::new(),
                    timer_requests: Vec::new(),
                    ordered_attention_signals: Vec::new(),
                }))
            },
        )
        .unwrap_or_else(|error| panic!("Timer trace fixture: {error}"))
    }

    fn prepared_timer(
        trace: &CoreTraceV1,
        witness: PreparedAuthorityWitnessV1,
        transition_id: &str,
    ) -> PreparedRoomCommitV1 {
        prepared_timer_candidate(
            trace,
            witness,
            transition_id,
            TIMER,
            "2026-08-15T12:30:00Z",
            br#"{"kind":"deadline"}"#,
        )
    }

    fn prepared_timer_candidate(
        trace: &CoreTraceV1,
        witness: PreparedAuthorityWitnessV1,
        transition_id: &str,
        timer_id: &str,
        scheduled_for: &str,
        payload: &[u8],
    ) -> PreparedRoomCommitV1 {
        let request = TimerFiredRequestV1::new(
            parsed(ROOM),
            parsed(timer_id),
            TimerGenerationV1::new(1).unwrap_or_else(|error| panic!("Timer generation: {error}")),
            parsed(scheduled_for),
            canonical(payload),
        );
        let transition = trace
            .prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
                timer_id: parsed(timer_id),
                generation: TimerGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("Timer generation: {error}")),
                scheduled_for: parsed(scheduled_for),
                canonical_payload: canonical(payload),
            }))
            .unwrap_or_else(|error| panic!("prepare Timer firing: {error}"));
        PreparedRoomCommitV1::for_timer_fired_for_conformance(
            trace,
            &request,
            transition,
            parsed(transition_id),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("integrity generation: {error}")),
            witness,
            &BTreeMap::new(),
        )
        .unwrap_or_else(|error| panic!("seal Timer firing: {error}"))
    }

    fn seed_timer_room(path: &std::path::Path, trace: &CoreTraceV1) {
        let mut connection =
            Connection::open(path).unwrap_or_else(|error| panic!("open Timer fixture DB: {error}"));
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap_or_else(|error| panic!("begin Timer fixture seed: {error}"));
        let head = trace.head();
        let room_id = head.room_id().to_string();
        let head_bytes = head
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("canonical Timer Head: {error}"));
        let core_bytes = trace
            .core_state()
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("canonical Timer Core: {error}"));
        let activity_bytes = trace
            .activity_state()
            .to_bytes()
            .unwrap_or_else(|error| panic!("canonical Timer Activity: {error}"));
        transaction
            .execute(
                "INSERT INTO rooms(\
                 room_id, room_status, room_seq, genesis_or_transition_hash, core_schema_version,\
                 pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash,\
                 complete_head_bytes\
                 ) VALUES (?1, 'active', 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    room_id,
                    head.genesis_or_transition_hash().to_string(),
                    head.core_schema_version(),
                    head.pack_digest().to_string(),
                    head.core_state_hash().to_string(),
                    head.activity_state_hash().to_string(),
                    head.authoritative_state_hash().to_string(),
                    head_bytes,
                ],
            )
            .unwrap_or_else(|error| panic!("seed Timer room: {error}"));
        transaction
            .execute(
                "INSERT INTO room_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) \
                 VALUES (?1, ?2, ?3)",
                params![
                    room_id,
                    canonical(br"{}").to_bytes().unwrap_or_default(),
                    trace
                        .genesis_bytes()
                        .unwrap_or_else(|error| panic!("Timer Genesis bytes: {error}")),
                ],
            )
            .unwrap_or_else(|error| panic!("seed Timer Genesis: {error}"));
        transaction
            .execute(
                "INSERT INTO room_materializations(room_id, core_state_bytes, activity_state_bytes) \
                 VALUES (?1, ?2, ?3)",
                params![room_id, core_bytes, activity_bytes],
            )
            .unwrap_or_else(|error| panic!("seed Timer materializations: {error}"));
        for timer in trace.genesis().initial_timers() {
            transaction
                .execute(
                    "INSERT INTO timers(\
                     room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'scheduled')",
                    params![
                        room_id,
                        timer.timer_id.to_string(),
                        i64::try_from(timer.generation.get())
                            .unwrap_or_else(|error| panic!("Timer generation range: {error}")),
                        timer.scheduled_for.as_str(),
                        timer
                            .canonical_payload
                            .to_bytes()
                            .unwrap_or_else(|error| panic!("Timer payload bytes: {error}")),
                    ],
                )
                .unwrap_or_else(|error| panic!("seed Timer row: {error}"));
        }
        transaction
            .execute(
                "INSERT INTO room_integrity(room_id, status, generation) \
                 VALUES (?1, 'healthy', 1)",
                [&room_id],
            )
            .unwrap_or_else(|error| panic!("seed Timer integrity: {error}"));
        transaction
            .commit()
            .unwrap_or_else(|error| panic!("commit Timer fixture seed: {error}"));
    }

    fn authoritative_database_fingerprint(path: &std::path::Path) -> Vec<u8> {
        const QUERIES: &[(&str, &str, usize)] = &[
            (
                "principals",
                "SELECT principal_id, principal_kind, authority_status, principal_generation \
                 FROM principals ORDER BY principal_id",
                4,
            ),
            (
                "runners",
                "SELECT runner_id, owner_principal_id, authority_status, runner_generation \
                 FROM runners ORDER BY runner_id",
                4,
            ),
            (
                "capabilities",
                "SELECT capability_id, token_hash, principal_id, profile_kind, target_room_id, \
                 target_member_id, runner_id, authority_generation, expires_at, revoked_at \
                 FROM capabilities ORDER BY capability_id",
                10,
            ),
            (
                "capability_scopes",
                "SELECT capability_id, scope FROM capability_scopes \
                 ORDER BY capability_id, scope",
                2,
            ),
            (
                "runner_capability_memberships",
                "SELECT capability_id, room_id, member_id FROM runner_capability_memberships \
                 ORDER BY capability_id, room_id, member_id",
                3,
            ),
            (
                "authority_change_receipts",
                "SELECT change_id, authenticated_principal, request_hash, result_kind, \
                 target_kind, target_id, secondary_target_id, resulting_generation, checked_at \
                 FROM authority_change_receipts ORDER BY change_id",
                9,
            ),
            (
                "authority_audit",
                "SELECT audit_seq, change_id, actor_principal_id, target_kind, target_id, \
                 secondary_target_id, change_kind, prior_generation, resulting_generation, \
                 checked_at, reason_code, request_hash \
                 FROM authority_audit ORDER BY audit_seq",
                12,
            ),
            (
                "rooms",
                "SELECT room_id, room_status, room_seq, genesis_or_transition_hash, \
                 core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
                 authoritative_state_hash, complete_head_bytes FROM rooms ORDER BY room_id",
                10,
            ),
            (
                "room_genesis",
                "SELECT room_id, pack_revision_lock_bytes, genesis_bytes \
                 FROM room_genesis ORDER BY room_id",
                3,
            ),
            (
                "room_materializations",
                "SELECT room_id, core_state_bytes, activity_state_bytes \
                 FROM room_materializations ORDER BY room_id",
                3,
            ),
            (
                "room_members",
                "SELECT room_id, member_id, principal_id, principal_kind, standing, access_mode, \
                 role, membership_bytes, membership_generation, frame_head \
                 FROM room_members ORDER BY room_id, member_id",
                10,
            ),
            (
                "room_integrity",
                "SELECT room_id, status, generation FROM room_integrity ORDER BY room_id",
                3,
            ),
            (
                "transitions",
                "SELECT room_id, transition_id, room_seq, transition_hash, previous_lineage_hash, \
                 core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
                 authoritative_state_hash, transition_bytes \
                 FROM transitions ORDER BY room_id, room_seq",
                11,
            ),
            (
                "timers",
                "SELECT room_id, timer_id, generation, scheduled_for, payload_bytes, state \
                 FROM timers ORDER BY room_id, timer_id, generation",
                6,
            ),
            (
                "observation_frames",
                "SELECT room_id, member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes \
                 FROM observation_frames ORDER BY room_id, member_id, frame_seq",
                6,
            ),
            (
                "activation_decisions",
                "SELECT room_id, cause_room_seq, decision_id, target_member_id, decision_bytes \
                 FROM activation_decisions ORDER BY room_id, cause_room_seq, decision_id",
                5,
            ),
            (
                "semantic_receipts",
                "SELECT room_id, operation_kind, operation_identity_bytes, codec_id, \
                 canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
                 semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes, \
                 committed_at FROM semantic_receipts \
                 ORDER BY operation_kind, operation_identity_bytes",
                12,
            ),
        ];
        let connection = Connection::open(path)
            .unwrap_or_else(|error| panic!("open fingerprint reader: {error}"));
        connection
            .pragma_update(None, "query_only", true)
            .unwrap_or_else(|error| panic!("fingerprint query_only: {error}"));
        let mut fingerprint = Vec::new();
        for (table, query, columns) in QUERIES {
            fingerprint.extend_from_slice(&u64::try_from(table.len()).unwrap_or(0).to_be_bytes());
            fingerprint.extend_from_slice(table.as_bytes());
            let mut statement = connection
                .prepare(query)
                .unwrap_or_else(|error| panic!("prepare fingerprint {table}: {error}"));
            let mut rows = statement
                .query(())
                .unwrap_or_else(|error| panic!("query fingerprint {table}: {error}"));
            while let Some(row) = rows
                .next()
                .unwrap_or_else(|error| panic!("read fingerprint {table}: {error}"))
            {
                fingerprint.push(0xff);
                for column in 0..*columns {
                    match row
                        .get_ref(column)
                        .unwrap_or_else(|error| panic!("read {table} column {column}: {error}"))
                    {
                        ValueRef::Null => fingerprint.push(0),
                        ValueRef::Integer(value) => {
                            fingerprint.push(1);
                            fingerprint.extend_from_slice(&value.to_be_bytes());
                        }
                        ValueRef::Text(value) => {
                            fingerprint.push(2);
                            fingerprint.extend_from_slice(
                                &u64::try_from(value.len()).unwrap_or(0).to_be_bytes(),
                            );
                            fingerprint.extend_from_slice(value);
                        }
                        ValueRef::Blob(value) => {
                            fingerprint.push(3);
                            fingerprint.extend_from_slice(
                                &u64::try_from(value.len()).unwrap_or(0).to_be_bytes(),
                            );
                            fingerprint.extend_from_slice(value);
                        }
                        ValueRef::Real(_) => panic!("authoritative schema contains no REAL values"),
                    }
                }
            }
        }
        fingerprint
    }

    #[test]
    fn opens_only_the_exact_bundled_sqlite_engine() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("exact bundled SQLite: {error}"));
        assert_eq!(store.engine_identity(), (SQLITE_VERSION, SQLITE_SOURCE_ID));
    }

    #[test]
    fn opens_issued_v1_database_through_forward_authority_migration() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open v1 fixture: {error}"));
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (\
                 version INTEGER PRIMARY KEY, migration_id TEXT NOT NULL UNIQUE\
                 ) STRICT;",
            )
            .unwrap_or_else(|error| panic!("v1 migration table: {error}"));
        connection
            .execute_batch(INITIAL_MIGRATION_SCHEMA)
            .unwrap_or_else(|error| panic!("issued v1 schema: {error}"));
        connection
            .execute(
                "INSERT INTO schema_migrations(version, migration_id) VALUES (1, ?1)",
                [INITIAL_MIGRATION_ID],
            )
            .unwrap_or_else(|error| panic!("v1 migration identity: {error}"));
        connection
            .execute(
                "INSERT INTO authority_fences(\
                 witness_id, authenticated_principal, generation, scope_revocation_bytes, \
                 scope_revocation_hash, active) VALUES (?1, ?2, 1, ?3, ?4, 1)",
                params!["legacy-fence", PRINCIPAL, b"legacy-scope", [0xA5_u8; 32]],
            )
            .unwrap_or_else(|error| panic!("legacy authority fence: {error}"));
        let (prepared, _, _) = prepared_creation(0);
        let PreparedRoomWriteV1::Create(create) = prepared else {
            panic!("v1 fixture is a Room creation");
        };
        let persistence = create.persistence();
        let genesis_head = &persistence.complete_head;
        let trace_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("v1 transition trace DB: {error}"));
        let trace_store = SqliteRoomStore::open(trace_file.path())
            .unwrap_or_else(|error| panic!("open v1 transition trace SQLite: {error}"));
        let (mut transition_trace, _) = committed_trace(&trace_store);
        let transition = advance_conformance_replay_access_change(&mut transition_trace);
        assert_eq!(
            transition.previous_lineage_hash(),
            genesis_head.genesis_or_transition_hash()
        );
        let head = transition.complete_head();
        let room_id = head.room_id().to_string();
        connection
            .execute(
                "INSERT INTO rooms(\
                 room_id, room_status, room_seq, genesis_or_transition_hash, core_schema_version, \
                 pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, \
                 complete_head_bytes) VALUES (?1, 'active', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    room_id,
                    i64::try_from(head.room_seq().get()).unwrap_or(-1),
                    head.genesis_or_transition_hash().to_string(),
                    head.core_schema_version(),
                    head.pack_digest().to_string(),
                    head.core_state_hash().to_string(),
                    head.activity_state_hash().to_string(),
                    head.authoritative_state_hash().to_string(),
                    head.canonical_bytes()
                        .unwrap_or_else(|error| panic!("v1 current Head bytes: {error}")),
                ],
            )
            .unwrap_or_else(|error| panic!("v1 Room: {error}"));
        connection
            .execute(
                "INSERT INTO room_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) \
                 VALUES (?1, ?2, ?3)",
                params![
                    room_id,
                    persistence.canonical_pack_revision_lock_bytes,
                    persistence.canonical_genesis_bytes,
                ],
            )
            .unwrap_or_else(|error| panic!("v1 Genesis: {error}"));
        connection
            .execute(
                "INSERT INTO room_materializations(\
                 room_id, core_state_bytes, activity_state_bytes) VALUES (?1, ?2, ?3)",
                params![
                    room_id,
                    transition
                        .resulting_core_state()
                        .canonical_bytes()
                        .unwrap_or_else(|error| panic!("v1 resulting Core bytes: {error}")),
                    transition
                        .resulting_activity_state()
                        .to_bytes()
                        .unwrap_or_else(|error| panic!("v1 resulting Activity bytes: {error}")),
                ],
            )
            .unwrap_or_else(|error| panic!("v1 materializations: {error}"));
        for membership in transition.resulting_core_state().memberships().values() {
            let membership_json = serde_json::to_vec(membership)
                .unwrap_or_else(|error| panic!("encode v1 resulting Membership: {error}"));
            let membership_bytes = canonical(&membership_json)
                .to_bytes()
                .unwrap_or_else(|error| panic!("canonical v1 resulting Membership: {error}"));
            connection
                .execute(
                    "INSERT INTO room_members(\
                     room_id, member_id, principal_id, principal_kind, standing, access_mode, role, \
                     membership_bytes, frame_head) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)",
                    params![
                        room_id,
                        membership.member_id().to_string(),
                        membership.principal_id().to_string(),
                        super::principal_kind(membership.principal_kind()),
                        super::membership_standing(membership.standing()),
                        super::access_mode(membership.access_mode()),
                        membership.role(),
                        membership_bytes,
                    ],
                )
                .unwrap_or_else(|error| panic!("v1 Membership: {error}"));
        }
        connection
            .execute(
                "INSERT INTO transitions( \
                     room_id, transition_id, room_seq, transition_hash, previous_lineage_hash, \
                     core_schema_version, pack_digest, core_state_hash, activity_state_hash, \
                     authoritative_state_hash, transition_bytes \
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    room_id,
                    "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
                    transition.transition_hash().to_string(),
                    transition.previous_lineage_hash().to_string(),
                    head.core_schema_version(),
                    head.pack_digest().to_string(),
                    transition.resulting_core_state_hash().to_string(),
                    transition.resulting_activity_state_hash().to_string(),
                    transition.resulting_authoritative_state_hash().to_string(),
                    transition
                        .canonical_bytes()
                        .unwrap_or_else(|error| panic!("v1 Transition bytes: {error}")),
                ],
            )
            .unwrap_or_else(|error| panic!("v1 Membership-changing Transition: {error}"));
        connection
            .execute(
                "INSERT INTO room_integrity(room_id, status, generation) \
                 VALUES (?1, 'healthy', 1)",
                [&room_id],
            )
            .unwrap_or_else(|error| panic!("v1 integrity: {error}"));
        drop(connection);

        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("forward-open v1 database: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect migrated database: {error}"));
        let migrations = connection
            .prepare("SELECT version, migration_id FROM schema_migrations ORDER BY version")
            .and_then(|mut statement| {
                statement
                    .query_map((), |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_else(|error| panic!("migrated identities: {error}"));
        assert_eq!(
            migrations,
            [
                (1, INITIAL_MIGRATION_ID.to_owned()),
                (2, AUTHORITY_MIGRATION_ID.to_owned()),
            ]
        );
        let retired: (String, String, i64, Vec<u8>, Vec<u8>, i64) = connection
            .query_row(
                "SELECT witness_id, authenticated_principal, generation, \
                 scope_revocation_bytes, scope_revocation_hash, active \
                 FROM retired_authority_fences_v1 WHERE witness_id = 'legacy-fence'",
                (),
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("retired fence: {error}"));
        assert_eq!(retired.0, "legacy-fence");
        assert_eq!(retired.1, PRINCIPAL);
        assert_eq!(retired.2, 1);
        assert_eq!(retired.3, b"legacy-scope");
        assert_eq!(retired.4, [0xA5_u8; 32]);
        assert_eq!(retired.5, 1);
        let membership_generation_column: i64 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('room_members') \
                 WHERE name = 'membership_generation' AND type = 'INTEGER' \
                 AND \"notnull\" = 1 AND dflt_value = '1'",
                (),
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("membership generation column: {error}"));
        assert_eq!(membership_generation_column, 1);
        let migrated_generation: i64 = connection
            .query_row(
                "SELECT membership_generation FROM room_members \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("migrated Membership generation: {error}"));
        assert_eq!(migrated_generation, 2);
        let current_authority_rows: i64 = connection
            .query_row(
                "SELECT (SELECT count(*) FROM principals) \
                 + (SELECT count(*) FROM capabilities) + (SELECT count(*) FROM runners)",
                (),
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("current authority rows: {error}"));
        assert_eq!(current_authority_rows, 0);
        drop(store);
    }

    #[test]
    fn bootstrap_is_durable_idempotent_conflict_safe_and_empty_authority_only() {
        const BOOTSTRAP_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FK0";
        const OTHER_BOOTSTRAP_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FK1";
        const OTHER_BOOTSTRAP_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH4";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:01Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock))
            .unwrap_or_else(|error| panic!("open bootstrap SQLite: {error}"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer_bytes = [0xA9; 32];
        let bootstrap = AuthorityBootstrapV1::new(
            parsed(BOOTSTRAP_CHANGE),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            parsed(HOST_CAPABILITY),
            CapabilityBearerV1::from_bytes(bearer_bytes).token_hash(),
            None,
        )
        .unwrap_or_else(|error| panic!("bootstrap request: {error}"));
        let receipt = authority
            .bootstrap(
                bootstrap.clone(),
                parsed::<AuthorityCheckedAt>("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("fresh bootstrap: {error}"));
        assert_eq!(
            receipt.result(),
            AuthorityChangeResultV1::AuthorityBootstrapped
        );
        assert_eq!(receipt.resulting_generation(), 1);
        assert_eq!(
            receipt.target(),
            &AuthorityChangeTargetV1::Bootstrap {
                principal_id: parsed(PRINCIPAL),
                capability_id: parsed(HOST_CAPABILITY),
            }
        );
        let replay = authority
            .bootstrap(
                bootstrap,
                parsed::<AuthorityCheckedAt>("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("exact bootstrap replay: {error}"));
        assert_eq!(replay, receipt);

        let identity_conflict = authority.bootstrap(
            AuthorityBootstrapV1::new(
                parsed(BOOTSTRAP_CHANGE),
                parsed(PRINCIPAL_ALT),
                PrincipalKindV1::Agent,
                parsed(OTHER_BOOTSTRAP_CAPABILITY),
                CapabilityBearerV1::from_bytes([0xAA; 32]).token_hash(),
                None,
            )
            .unwrap_or_else(|error| panic!("conflicting bootstrap request: {error}")),
            parsed("2026-08-15T12:00:00Z"),
        );
        assert_eq!(identity_conflict, Err(AuthorityErrorV1::Conflict));
        let nonempty_denial = authority.bootstrap(
            AuthorityBootstrapV1::new(
                parsed(OTHER_BOOTSTRAP_CHANGE),
                parsed(PRINCIPAL_ALT),
                PrincipalKindV1::Agent,
                parsed(OTHER_BOOTSTRAP_CAPABILITY),
                CapabilityBearerV1::from_bytes([0xAB; 32]).token_hash(),
                None,
            )
            .unwrap_or_else(|error| panic!("second bootstrap request: {error}")),
            parsed("2026-08-15T12:00:00Z"),
        );
        assert_eq!(nonempty_denial, Err(AuthorityErrorV1::Conflict));

        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect bootstrap database: {error}"));
        let durable: (i64, i64, i64, Option<String>, String, String, Vec<u8>) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM principals), \
                 (SELECT count(*) FROM capabilities), \
                 (SELECT count(*) FROM authority_audit), authenticated_principal, target_kind, \
                 secondary_target_id, (SELECT token_hash FROM capabilities LIMIT 1) \
                 FROM authority_change_receipts WHERE change_id = ?1",
                [BOOTSTRAP_CHANGE],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("bootstrap durable facts: {error}"));
        assert_eq!(durable.0, 1);
        assert_eq!(durable.1, 1);
        assert_eq!(durable.2, 1);
        assert_eq!(durable.3, None);
        assert_eq!(durable.4, "bootstrap");
        assert_eq!(durable.5, HOST_CAPABILITY);
        assert_ne!(durable.6, bearer_bytes);
    }

    #[test]
    fn queued_authority_change_is_revalidated_at_writer_time_and_expiry_fences() {
        const BOOTSTRAP_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FN0";
        const CREATE_PRINCIPAL_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FN1";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:01Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock.clone()))
            .unwrap_or_else(|error| panic!("open queued-expiry SQLite: {error}"));
        let authority = Arc::new(AuthorityV1::new(Arc::new(store.clone())));
        let bearer_bytes = [0xBC; 32];
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    parsed(BOOTSTRAP_CHANGE),
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Human,
                    parsed(HOST_CAPABILITY),
                    CapabilityBearerV1::from_bytes(bearer_bytes).token_hash(),
                    Some(parsed("2026-08-15T12:00:05Z")),
                )
                .unwrap_or_else(|error| panic!("queued-expiry bootstrap request: {error}")),
                parsed("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("queued-expiry bootstrap: {error}"));

        arm_writer_queue_pause();
        store.pause_writer_queue();
        wait_until_writer_queue_pauses();
        let change_authority = Arc::clone(&authority);
        let change = thread::spawn(move || {
            change_authority.change(
                &PresentedCapabilityV1::new(
                    parsed(HOST_CAPABILITY),
                    CapabilityBearerV1::from_bytes(bearer_bytes),
                ),
                AuthorityChangeV1::CreatePrincipal {
                    change_id: parsed(CREATE_PRINCIPAL_CHANGE),
                    principal_id: parsed(PRINCIPAL_ALT),
                    kind: PrincipalKindV1::Agent,
                },
                parsed("2026-08-15T12:00:04Z"),
            )
        });
        wait_until_authority_change_enqueued();
        clock.set("2026-08-15T12:00:05Z");
        release_writer_queue();
        let result = change
            .join()
            .unwrap_or_else(|_| panic!("queued authority change thread"));
        assert_eq!(result, Err(AuthorityErrorV1::StaleAuthorityGeneration));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect queued-expiry database: {error}"));
        let counts: (i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM principals), \
                 (SELECT count(*) FROM authority_change_receipts), \
                 (SELECT count(*) FROM authority_audit)",
                (),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("queued-expiry counts: {error}"));
        assert_eq!(counts, (1, 1, 1));
    }

    #[test]
    fn authorized_receipt_resolution_revalidates_revocation_and_expiry_in_writer_transaction() {
        const REVOKE_HOST_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ0";

        let revoked_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("revoked receipt DB: {error}"));
        let revoked_clock = TestAuthorityClock::at("2026-08-15T12:00:01Z");
        let revoked_store =
            SqliteRoomStore::open_with_clock(revoked_file.path(), Arc::new(revoked_clock.clone()))
                .unwrap_or_else(|error| panic!("open revoked receipt SQLite: {error}"));
        let revoked_authority = seed_real_host_authority(&revoked_store);
        let presented_host = presented_host_capability();
        let (identity, request_hash) = commit_host_authorized_creation_receipt(
            &revoked_store,
            &revoked_authority,
            &presented_host,
        );
        let receipt_use = || AuthorityUseV1::ReadRoomOperationResult {
            identity: identity.clone(),
            request_hash: request_hash.clone(),
            target_room_id: None,
        };
        let fresh_grant = match revoked_authority
            .authorize(
                &presented_host,
                receipt_use(),
                parsed("2026-08-15T12:00:02Z"),
            )
            .unwrap_or_else(|error| panic!("authorize fresh receipt read: {error}"))
        {
            AuthorityGrantV1::ReceiptRead(grant) => grant,
            other => panic!("expected receipt grant, got {other:?}"),
        };
        revoked_clock.set("2026-08-15T12:00:02Z");
        assert!(matches!(
            revoked_store.resolve_authorized(fresh_grant),
            Ok(ResolveOutcomeV1::StoredResolution(_))
        ));
        let held_grant = match revoked_authority
            .authorize(
                &presented_host,
                receipt_use(),
                parsed("2026-08-15T12:00:02Z"),
            )
            .unwrap_or_else(|error| panic!("authorize held receipt read: {error}"))
        {
            AuthorityGrantV1::ReceiptRead(grant) => grant,
            other => panic!("expected receipt grant, got {other:?}"),
        };
        revoked_clock.set("2026-08-15T12:00:03Z");
        revoked_authority
            .change(
                &presented_host,
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_HOST_CHANGE),
                    capability_id: parsed(HOST_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Host generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("receipt_read_revoked")
                        .unwrap_or_else(|error| panic!("Host revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:03Z"),
            )
            .unwrap_or_else(|error| panic!("revoke receipt Host: {error}"));
        assert!(matches!(
            revoked_store.resolve_authorized(held_grant),
            Err(AuthorityErrorV1::StaleAuthorityGeneration)
        ));

        let expired_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("expired receipt DB: {error}"));
        let expired_clock = TestAuthorityClock::at("2026-08-15T12:00:01Z");
        let expired_store =
            SqliteRoomStore::open_with_clock(expired_file.path(), Arc::new(expired_clock.clone()))
                .unwrap_or_else(|error| panic!("open expired receipt SQLite: {error}"));
        let principal = PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("expired Host Principal generation: {error}")),
        );
        let capability = CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
            capability_id: parsed(HOST_CAPABILITY),
            token_hash: CapabilityBearerV1::from_bytes([0xA7; 32]).token_hash(),
            principal_id: parsed(PRINCIPAL),
            profile: CapabilityProfileV1::HostOperator { room_id: None },
            scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])
                .unwrap_or_else(|error| panic!("expired Host scopes: {error}")),
            generation: AuthorityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("expired Host generation: {error}")),
            expires_at: Some(parsed("2026-08-15T12:00:05Z")),
            revoked_at: None,
        })
        .unwrap_or_else(|error| panic!("expired Host Capability: {error}"));
        expired_store
            .seed_authority_snapshot(principal, capability)
            .unwrap_or_else(|error| panic!("seed expiring receipt Host: {error}"));
        let expired_authority = AuthorityV1::new(Arc::new(expired_store.clone()));
        let (identity, request_hash) = commit_host_authorized_creation_receipt(
            &expired_store,
            &expired_authority,
            &presented_host,
        );
        let held_grant = match expired_authority
            .authorize(
                &presented_host,
                AuthorityUseV1::ReadRoomOperationResult {
                    identity,
                    request_hash,
                    target_room_id: None,
                },
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize expiring receipt read: {error}"))
        {
            AuthorityGrantV1::ReceiptRead(grant) => grant,
            other => panic!("expected receipt grant, got {other:?}"),
        };
        expired_clock.set("2026-08-15T12:00:05Z");
        assert!(matches!(
            expired_store.resolve_authorized(held_grant),
            Err(AuthorityErrorV1::StaleAuthorityGeneration)
        ));
    }

    #[test]
    fn authorized_core_archive_uses_exact_materializations_and_atomic_advance_path() {
        const ARCHIVE_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ1";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Archive DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:05Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock))
            .unwrap_or_else(|error| panic!("open Archive SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let request = CoreAdministrationRequestV1::new(
            parsed(ROOM),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "sqlite-real-archive".to_owned(),
            },
            CoreProposedKindV1::Archive,
            trace.head().room_seq(),
            "sqlite_archive",
            CoreChangeSetV1::archive(trace.core_state().room_status()),
        )
        .unwrap_or_else(|error| panic!("Archive request: {error}"));
        let grant = match authorize_core_administration_operation(
            &authority,
            &store,
            &presented_host_capability(),
            &request,
            parsed("2026-08-15T12:00:03Z"),
        )
        .unwrap_or_else(|error| panic!("authorize Archive: {error}"))
        {
            CoreAdministrationIngressV1::Authorized(grant) => grant,
            other => panic!("new Archive operation unexpectedly resolved: {other:?}"),
        };
        let frame_heads = trace
            .core_state()
            .memberships()
            .keys()
            .cloned()
            .map(|member_id| (member_id, 0))
            .collect();
        let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
            &trace,
            &request,
            parsed("2026-08-15T12:00:04Z"),
            parsed(ARCHIVE_TRANSITION),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("Archive integrity generation: {error}")),
            *grant,
            &frame_heads,
        )
        .unwrap_or_else(|error| panic!("prepare Archive: {error}"));
        let outcome = commit_existing_room_at(&store, &mut trace, prepared);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        assert_eq!(trace.core_state().room_status(), RoomStatusV1::Archived);

        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect Archive SQL: {error}"));
        let durable: (String, i64, i64, i64, Vec<u8>) = connection
            .query_row(
                "SELECT r.room_status, r.room_seq, \
                 (SELECT count(*) FROM transitions WHERE room_id = r.room_id), \
                 (SELECT count(*) FROM semantic_receipts WHERE room_id = r.room_id), \
                 m.core_state_bytes FROM rooms AS r \
                 JOIN room_materializations AS m ON m.room_id = r.room_id \
                 WHERE r.room_id = ?1",
                [ROOM],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("durable Archive rows: {error}"));
        assert_eq!(durable.0, "archived");
        assert_eq!((durable.1, durable.2, durable.3), (1, 1, 2));
        let durable_core = CanonicalJsonV1::decode_canonical::<CoreRoomStateV1>(&durable.4)
            .unwrap_or_else(|error| panic!("durable archived Core: {error}"));
        assert_eq!(durable_core.room_status(), RoomStatusV1::Archived);
    }

    #[test]
    fn authorized_replay_reconstructs_historical_access_and_fences_integrity() {
        const REPLAY_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ2";
        const CREATE_ABSENT_PRINCIPAL_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ3";
        const ABSENT_MEMBER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ4";
        const REGISTER_ABSENT_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ5";
        const REPLAY_JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ6";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock))
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay conformance lineage: {error}"));
        install_conformance_replay_access_change(&connection, &mut trace, REPLAY_TRANSITION);
        install_conformance_replay_member_join(&connection, &mut trace, REPLAY_JOIN_TRANSITION);
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));

        let current: (String, Option<String>, i64) = connection
            .query_row(
                "SELECT access_mode, role, membership_generation FROM room_members \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("read current Replay Membership: {error}"));
        assert_eq!(current, ("spectator".to_owned(), None, 2));

        let historical = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("Replay Genesis sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:06Z"),
            )
            .unwrap_or_else(|error| panic!("authorize historical Replay: {error}"));
        let projection = complete_authorized_replay(&store, &registry, historical)
            .unwrap_or_else(|error| panic!("historical Replay: {error}"));
        assert_eq!(projection.verified_head().room_seq().get(), 0);
        assert_eq!(
            projection.integrity().status(),
            RoomIntegrityStatusV1::Healthy
        );
        assert_eq!(projection.integrity().generation().get(), 1);
        assert_eq!(projection.historical_room_status(), RoomStatusV1::Active);
        assert_eq!(
            projection.historical_membership().access_mode(),
            AccessModeV1::Participant
        );
        assert_eq!(projection.historical_membership().role(), Some("counter"));
        let envelope = projection
            .canonical_envelope()
            .to_bytes()
            .unwrap_or_else(|error| panic!("Replay envelope bytes: {error}"));
        let outer: serde_json::Value = serde_json::from_slice(&envelope)
            .unwrap_or_else(|error| panic!("decode authorized Replay envelope: {error}"));
        assert_eq!(
            outer["envelope"],
            "worldstream/authorized-replay-projection/v1"
        );
        assert_eq!(outer["room_id"], ROOM);
        assert_eq!(outer["member_id"], PARTICIPANT);
        assert_eq!(outer["at_room_seq"], 0);
        assert_eq!(outer["projection_kind"], "historical_membership");
        assert_eq!(outer["integrity"]["status"], "healthy");
        assert_eq!(outer["integrity"]["generation"], 1);
        assert_eq!(
            outer["historical_projection"]["envelope"],
            "worldstream/historical-replay-projection/v1"
        );
        assert!(
            envelope
                .windows(b"worldstream/authorized-replay-projection/v1".len())
                .any(|window| window == b"worldstream/authorized-replay-projection/v1")
        );
        assert!(
            envelope
                .windows(b"worldstream/historical-replay-projection/v1".len())
                .any(|window| window == b"worldstream/historical-replay-projection/v1")
        );
        assert!(
            envelope
                .windows(b"\"access_mode\":\"participant\"".len())
                .any(|window| window == b"\"access_mode\":\"participant\"")
        );
        assert!(
            envelope
                .windows(b"\"role\":\"counter\"".len())
                .any(|window| window == b"\"role\":\"counter\"")
        );

        // A distinct member can be authorized from the current materialization
        // yet remain absent from the immutable prefix at N.
        authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::CreatePrincipal {
                    change_id: parsed(CREATE_ABSENT_PRINCIPAL_CHANGE),
                    principal_id: parsed(PRINCIPAL_ALT),
                    kind: PrincipalKindV1::Agent,
                },
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("create absent-at-N Principal: {error}"));
        let absent_bearer = CapabilityBearerV1::from_bytes([0xD4; 32]);
        let absent_capability = NewCapabilityV1::new(
            parsed(ABSENT_MEMBER_CAPABILITY),
            absent_bearer.token_hash(),
            parsed(PRINCIPAL_ALT),
            CapabilityProfileV1::RoomMember {
                room_id: parsed(ROOM),
                member_id: parsed(PARTICIPANT_ALT),
            },
            CapabilityScopeSetV1::new([CapabilityScopeV1::RoomReplay])
                .unwrap_or_else(|error| panic!("absent-at-N Replay scope: {error}")),
            None,
        )
        .unwrap_or_else(|error| panic!("absent-at-N Capability: {error}"));
        authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RegisterCapability {
                    change_id: parsed(REGISTER_ABSENT_CAPABILITY_CHANGE),
                    capability: absent_capability,
                },
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("register absent-at-N Capability: {error}"));
        let absent = authority
            .authorize_replay(
                &PresentedCapabilityV1::new(parsed(ABSENT_MEMBER_CAPABILITY), absent_bearer),
                parsed(ROOM),
                parsed(PARTICIPANT_ALT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("absent-at-N Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:09Z"),
            )
            .unwrap_or_else(|error| panic!("authorize absent-at-N Replay: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, absent),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::HistoricalMembershipUnavailable
            ))
        ));

        connection
            .execute(
                "UPDATE room_integrity SET status = 'faulted', generation = 2 \
                 WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("fault Replay integrity: {error}"));
        let faulted = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("faulted Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize faulted Replay: {error}"));
        let faulted = complete_authorized_replay(&store, &registry, faulted)
            .unwrap_or_else(|error| panic!("faulted Replay remains readable: {error}"));
        assert_eq!(faulted.integrity().status(), RoomIntegrityStatusV1::Faulted);
        assert_eq!(faulted.integrity().generation().get(), 2);
        let faulted_envelope = faulted
            .canonical_envelope()
            .to_bytes()
            .unwrap_or_else(|error| panic!("faulted Replay envelope: {error}"));
        assert!(
            faulted_envelope
                .windows(b"\"status\":\"faulted\"".len())
                .any(|window| window == b"\"status\":\"faulted\"")
        );

        connection
            .execute(
                "UPDATE room_integrity SET status = 'quarantined', generation = 3 \
                 WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("quarantine Replay integrity: {error}"));
        let quarantined = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("quarantined Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("authorize quarantined Replay request: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, quarantined),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::IntegrityUnavailable
            ))
        ));
        let repeated = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("repeated quarantine sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:09Z"),
            )
            .unwrap_or_else(|error| panic!("authorize repeated quarantine Replay: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, repeated),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::IntegrityUnavailable
            ))
        ));
        let quarantined_integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read repeated quarantine fence: {error}"));
        assert_eq!(quarantined_integrity, ("quarantined".to_owned(), 3));
    }

    #[test]
    fn ordinary_replay_capability_cannot_request_final_reveal_or_fault_the_room() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        assert!(matches!(
            authority.authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("FinalReveal sequence: {error}")),
                ReplayProjectionKindV1::FinalReveal,
                parsed("2026-08-15T12:00:04Z"),
            ),
            Err(AuthorityErrorV1::Forbidden)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect FinalReveal denial: {error}"));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("FinalReveal integrity: {error}"));
        assert_eq!(integrity, ("healthy".to_owned(), 1));
        let request = increment_request(&trace, ACTION_A);
        let action = seal_authorized_increment(
            &trace,
            &request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:05Z",
            0,
            authorize_enabled_action(&authority, &request, ACTION_A, "2026-08-15T12:00:05Z"),
        )
        .unwrap_or_else(|error| panic!("seal post-FinalReveal Action: {error}"));
        assert!(matches!(
            commit_existing_room_at(&store, &mut trace, action).resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
    }

    #[test]
    fn authorized_replay_genesis_page_requires_lock_and_exact_creation_receipt() {
        #[derive(Clone, Copy)]
        enum GenesisCorruption {
            PackLockWithMissingRuntime,
            MissingReceipt,
            CorruptReceipt,
        }
        for corruption in [
            GenesisCorruption::PackLockWithMissingRuntime,
            GenesisCorruption::MissingReceipt,
            GenesisCorruption::CorruptReceipt,
        ] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
            let (_trace, authority) = committed_trace_with_real_authority(
                &store,
                MembershipStandingV1::Enabled,
                [CapabilityScopeV1::RoomReplay],
                None,
            );
            let held = authority
                .authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(0)
                        .unwrap_or_else(|error| panic!("Genesis Replay sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:04Z"),
                )
                .unwrap_or_else(|error| panic!("authorize Genesis Replay: {error}"));
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open Genesis Replay mutator: {error}"));

            if matches!(corruption, GenesisCorruption::PackLockWithMissingRuntime) {
                let query_plan = {
                    let mut statement = connection
                        .prepare(
                            "EXPLAIN QUERY PLAN SELECT operation_identity_bytes \
                             FROM semantic_receipts \
                             WHERE room_id = ?1 AND resolution_kind = 'genesis_created'",
                        )
                        .unwrap_or_else(|error| panic!("prepare Genesis receipt plan: {error}"));
                    statement
                        .query_map([ROOM], |row| row.get::<_, String>(3))
                        .unwrap_or_else(|error| panic!("query Genesis receipt plan: {error}"))
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap_or_else(|error| panic!("read Genesis receipt plan: {error}"))
                };
                assert!(
                    query_plan
                        .iter()
                        .any(|detail| detail.contains("semantic_receipts_one_genesis_per_room")),
                    "Genesis receipt lookup must use its bounded partial index: {query_plan:?}"
                );
                let duplicate = connection.execute(
                    "INSERT INTO semantic_receipts( \
                         room_id, operation_kind, operation_identity_bytes, codec_id, \
                         canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
                         semantic_time_bytes, resolution_kind, transition_seq, \
                         stored_resolution_bytes \
                     ) SELECT room_id, operation_kind, ?1, codec_id, canonical_request_hash, \
                         basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, \
                         resolution_kind, transition_seq, stored_resolution_bytes \
                     FROM semantic_receipts \
                     WHERE room_id = ?2 AND resolution_kind = 'genesis_created'",
                    params![b"duplicate-genesis-receipt".as_slice(), ROOM],
                );
                assert!(matches!(
                    duplicate,
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::ConstraintViolation
                ));
            }

            match corruption {
                GenesisCorruption::PackLockWithMissingRuntime => {
                    connection
                        .execute("DROP TRIGGER room_genesis_immutable_update", ())
                        .unwrap_or_else(|error| panic!("drop Genesis update trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE room_genesis SET pack_revision_lock_bytes = ?1 \
                             WHERE room_id = ?2",
                            params![b"{}".as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt retained Pack lock: {error}"));
                }
                GenesisCorruption::MissingReceipt => {
                    connection
                        .execute("DROP TRIGGER semantic_receipts_immutable_delete", ())
                        .unwrap_or_else(|error| panic!("drop receipt delete trigger: {error}"));
                    connection
                        .execute(
                            "DELETE FROM semantic_receipts \
                             WHERE room_id = ?1 AND resolution_kind = 'genesis_created'",
                            [ROOM],
                        )
                        .unwrap_or_else(|error| panic!("delete Genesis receipt: {error}"));
                }
                GenesisCorruption::CorruptReceipt => {
                    connection
                        .execute("DROP TRIGGER semantic_receipts_immutable_update", ())
                        .unwrap_or_else(|error| panic!("drop receipt update trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE semantic_receipts SET stored_resolution_bytes = ?1 \
                             WHERE room_id = ?2 AND resolution_kind = 'genesis_created'",
                            params![b"{}".as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Genesis receipt: {error}"));
                }
            }
            let result = match corruption {
                GenesisCorruption::PackLockWithMissingRuntime => {
                    let missing_runtime = counter_v1_only_registry_for_conformance()
                        .unwrap_or_else(|error| panic!("missing Replay runtime: {error}"));
                    store.replay_authorized(&missing_runtime, held)
                }
                GenesisCorruption::MissingReceipt | GenesisCorruption::CorruptReceipt => {
                    let registry = builtin_counter_registry()
                        .unwrap_or_else(|error| panic!("Replay registry: {error}"));
                    store.replay_authorized(&registry, held)
                }
            };
            assert!(matches!(
                result,
                Err(SqliteAuthorizedReplayErrorV1::Corrupt)
            ));
            let integrity: (String, i64) = connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("read Genesis-page quarantine: {error}"));
            assert_eq!(integrity, ("quarantined".to_owned(), 2));
        }
    }

    #[test]
    fn replay_pages_require_every_transition_row_projection_to_match_canonical_bytes() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        const RESUME_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQB";
        for column in [
            "transition_hash",
            "previous_lineage_hash",
            "core_schema_version",
            "pack_digest",
            "core_state_hash",
            "activity_state_hash",
            "authoritative_state_hash",
        ] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
            let (mut trace, authority) = committed_trace_with_real_authority(
                &store,
                MembershipStandingV1::Enabled,
                [CapabilityScopeV1::RoomReplay],
                None,
            );
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open projected-row lineage: {error}"));
            install_conformance_replay_neutral_membership_history(
                &connection,
                &mut trace,
                JOIN_TRANSITION,
                SUSPEND_TRANSITION,
            );
            install_conformance_replay_member_resume(&connection, &mut trace, RESUME_TRANSITION);
            let held = authority
                .authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(1)
                        .unwrap_or_else(|error| panic!("projected-row sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:08Z"),
                )
                .unwrap_or_else(|error| panic!("authorize projected-row Replay: {error}"));
            connection
                .execute("DROP TRIGGER transitions_immutable_update", ())
                .unwrap_or_else(|error| panic!("drop Transition update trigger: {error}"));
            connection
                .execute(
                    &format!(
                        "UPDATE transitions SET {column} = 'tampered' \
                         WHERE room_id = ?1 AND room_seq = 1"
                    ),
                    [ROOM],
                )
                .unwrap_or_else(|error| panic!("corrupt {column}: {error}"));
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            assert!(matches!(
                store.replay_authorized(&registry, held),
                Err(SqliteAuthorizedReplayErrorV1::Corrupt)
            ));
            let integrity: (String, i64) = connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("read projected-row quarantine: {error}"));
            assert_eq!(integrity, ("quarantined".to_owned(), 2), "{column}");
        }
    }

    #[test]
    fn replay_membership_projection_corruption_quarantines_at_begin_and_release() {
        const ACCESS_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
        let _serial = serialize_replay_projection_test();
        for corrupt_during_page in [false, true] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
            let (mut trace, authority) = committed_trace_with_real_authority(
                &store,
                MembershipStandingV1::Enabled,
                [CapabilityScopeV1::RoomReplay],
                None,
            );
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open Membership Replay mutator: {error}"));
            if corrupt_during_page {
                install_conformance_replay_access_change(
                    &connection,
                    &mut trace,
                    ACCESS_TRANSITION,
                );
            }
            let held = authority
                .authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(u64::from(corrupt_during_page))
                        .unwrap_or_else(|error| panic!("Membership Replay sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:07Z"),
                )
                .unwrap_or_else(|error| panic!("authorize Membership Replay: {error}"));
            let result = if corrupt_during_page {
                arm_replay_projection_pause(&store.writer.path);
                let replay_store = store.clone();
                let running = thread::spawn(move || {
                    let registry = builtin_counter_registry()
                        .unwrap_or_else(|error| panic!("Replay registry: {error}"));
                    replay_store.replay_authorized(&registry, held)
                });
                wait_until_replay_projection_pauses();
                connection
                    .execute(
                        "UPDATE room_members SET standing = 'suspended' \
                         WHERE room_id = ?1 AND member_id = ?2",
                        params![ROOM, PARTICIPANT],
                    )
                    .unwrap_or_else(|error| panic!("tamper paged Membership: {error}"));
                release_replay_projection();
                running
                    .join()
                    .unwrap_or_else(|_| panic!("Membership Replay thread"))
            } else {
                connection
                    .execute(
                        "UPDATE room_members SET standing = 'suspended' \
                         WHERE room_id = ?1 AND member_id = ?2",
                        params![ROOM, PARTICIPANT],
                    )
                    .unwrap_or_else(|error| panic!("tamper initial Membership: {error}"));
                let registry = builtin_counter_registry()
                    .unwrap_or_else(|error| panic!("Replay registry: {error}"));
                store.replay_authorized(&registry, held)
            };
            assert!(matches!(
                result,
                Err(SqliteAuthorizedReplayErrorV1::Corrupt)
            ));
            let integrity: (String, i64) = connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("read Membership quarantine: {error}"));
            assert_eq!(integrity, ("quarantined".to_owned(), 2));
        }
    }

    #[test]
    fn replay_non_room_authority_corruption_does_not_change_room_integrity() {
        for authority_row in ["capability", "principal"] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
            let (_trace, authority) = committed_trace_with_real_authority(
                &store,
                MembershipStandingV1::Enabled,
                [CapabilityScopeV1::RoomReplay],
                None,
            );
            let held = authority
                .authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(0)
                        .unwrap_or_else(|error| panic!("authority-corrupt sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:04Z"),
                )
                .unwrap_or_else(|error| panic!("authorize authority-corrupt Replay: {error}"));
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open authority mutator: {error}"));
            match authority_row {
                "capability" => {
                    connection
                        .execute("DROP TRIGGER capability_identity_immutable", ())
                        .unwrap_or_else(|error| panic!("drop Capability trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE capabilities SET target_member_id = ?1 \
                             WHERE capability_id = ?2",
                            params![PARTICIPANT_ALT, MEMBER_CAPABILITY],
                        )
                        .unwrap_or_else(|error| panic!("tamper Capability target: {error}"));
                }
                "principal" => {
                    connection
                        .execute("DROP TRIGGER principal_identity_immutable", ())
                        .unwrap_or_else(|error| panic!("drop Principal trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE principals SET principal_kind = 'agent' \
                             WHERE principal_id = ?1",
                            [PRINCIPAL],
                        )
                        .unwrap_or_else(|error| panic!("tamper Principal kind: {error}"));
                }
                _ => unreachable!("closed authority-row fixture"),
            }
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            assert!(store.replay_authorized(&registry, held).is_err());
            let integrity: (String, i64) = connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("read authority-only integrity: {error}"));
            assert_eq!(integrity, ("healthy".to_owned(), 1), "{authority_row}");
        }
    }

    #[test]
    fn replay_membership_generation_drift_fences_without_quarantine() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (_trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let held = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("generation-drift sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize generation-drift Replay: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open generation-drift mutator: {error}"));
        connection
            .execute(
                "UPDATE room_members SET membership_generation = 2 \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
            )
            .unwrap_or_else(|error| panic!("advance Membership generation: {error}"));
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, held),
            Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration
            ))
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read generation-drift integrity: {error}"));
        assert_eq!(integrity, ("healthy".to_owned(), 1));
    }

    #[test]
    fn authorized_replay_continuation_is_bounded_store_bound_and_expires_abandoned_state() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open paged Replay lineage: {error}"));
        let receipt_plan = {
            let mut statement = connection
                .prepare(
                    "EXPLAIN QUERY PLAN SELECT operation_identity_bytes \
                     FROM semantic_receipts WHERE room_id = ?1 AND transition_seq = ?2",
                )
                .unwrap_or_else(|error| panic!("prepare Replay receipt query plan: {error}"));
            statement
                .query_map(params![ROOM, 1_i64], |row| row.get::<_, String>(3))
                .unwrap_or_else(|error| panic!("query Replay receipt plan: {error}"))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|error| panic!("read Replay receipt plan: {error}"))
        };
        assert!(
            receipt_plan
                .iter()
                .any(|detail| detail.contains("semantic_receipts_by_transition")),
            "paged receipt lookup must use its bounded Room/sequence index: {receipt_plan:?}"
        );
        install_conformance_replay_neutral_membership_history(
            &connection,
            &mut trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        let duplicate_receipt = connection.execute(
            "INSERT INTO semantic_receipts( \
                 room_id, operation_kind, operation_identity_bytes, codec_id, \
                 canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, \
                 semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes \
             ) SELECT room_id, operation_kind, ?1, codec_id, canonical_request_hash, \
                 basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, \
                 resolution_kind, transition_seq, stored_resolution_bytes \
             FROM semantic_receipts WHERE room_id = ?2 AND transition_seq = 1",
            params![b"duplicate-replay-receipt".as_slice(), ROOM],
        );
        assert!(matches!(
            duplicate_receipt,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        let authorize = || {
            authority
                .authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(2)
                        .unwrap_or_else(|error| panic!("paged Replay sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:07Z"),
                )
                .unwrap_or_else(|error| panic!("authorize paged Replay: {error}"))
        };
        set_replay_slice_row_budget(&store.writer.path, 1);

        let first = match store
            .replay_authorized(&registry, authorize())
            .unwrap_or_else(|error| panic!("begin first paged Replay: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one-row slice cannot complete a two-Transition prefix")
            }
        };
        assert_eq!(
            format!("{first:?}"),
            "SqliteAuthorizedReplayContinuationV1([REDACTED])"
        );
        let second = match store
            .replay_authorized(&registry, authorize())
            .unwrap_or_else(|error| panic!("begin second paged Replay: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one-row slice cannot complete a two-Transition prefix")
            }
        };
        clear_replay_slice_row_budget(&store.writer.path);
        let third = complete_authorized_replay(&store, &registry, authorize())
            .unwrap_or_else(|error| panic!("third Replay must bypass deferred sessions: {error}"));
        assert_eq!(third.verified_head().room_seq().get(), 2);

        let other_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("other Replay DB: {error}"));
        let other_store = SqliteRoomStore::open(other_file.path())
            .unwrap_or_else(|error| panic!("open other Replay SQLite: {error}"));
        assert!(matches!(
            other_store.resume_replay_authorized(&registry, second),
            Err(SqliteAuthorizedReplayErrorV1::StorageUnavailable)
        ));

        expire_deferred_replay_sessions(&store);
        set_replay_slice_row_budget(&store.writer.path, 1);
        let after_expiry = match store
            .replay_authorized(&registry, authorize())
            .unwrap_or_else(|error| panic!("begin Replay after session expiry: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one-row slice cannot complete a two-Transition prefix")
            }
        };
        let after_expiry = match store
            .resume_replay_authorized(&registry, after_expiry)
            .unwrap_or_else(|error| panic!("resume first Replay page: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one Transition remains after the first resume")
            }
        };
        let projection = match store
            .resume_replay_authorized(&registry, after_expiry)
            .unwrap_or_else(|error| panic!("finish paged Replay: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Complete(projection) => projection,
            SqliteAuthorizedReplayOutcomeV1::Deferred(_) => {
                panic!("the final retained Transition must complete Replay")
            }
        };
        assert_eq!(projection.verified_head().room_seq().get(), 2);
        assert_eq!(
            projection.historical_membership().member_id().as_str(),
            PARTICIPANT
        );
        assert_eq!(
            store
                .writer
                .replay_sessions
                .lock()
                .unwrap_or_else(|_| panic!("Replay-session registry mutex"))
                .by_id
                .len(),
            0
        );
        clear_replay_slice_row_budget(&store.writer.path);
    }

    #[test]
    fn authorized_replay_continuation_revalidates_revocation_before_resume() {
        const ACCESS_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock))
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open paged Replay lineage: {error}"));
        install_conformance_replay_access_change(&connection, &mut trace, ACCESS_TRANSITION);
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        let grant = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(1)
                    .unwrap_or_else(|error| panic!("paged Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize paged Replay: {error}"));
        set_replay_slice_row_budget(&store.writer.path, 1);
        let continuation = match store
            .replay_authorized(&registry, grant)
            .unwrap_or_else(|error| panic!("begin paged Replay: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one-row slice must defer before Transition one")
            }
        };
        authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Replay Capability generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("replay_slice_revoked")
                        .unwrap_or_else(|error| panic!("Replay revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("revoke paged Replay Capability: {error}"));
        assert!(matches!(
            store.resume_replay_authorized(&registry, continuation),
            Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration
            ))
        ));
        assert_eq!(
            store
                .writer
                .replay_sessions
                .lock()
                .unwrap_or_else(|_| panic!("Replay-session registry mutex"))
                .by_id
                .len(),
            0
        );
        clear_replay_slice_row_budget(&store.writer.path);
    }

    #[test]
    fn authorized_replay_resume_quarantines_current_materialization_corruption() {
        const ACCESS_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open materialization Replay lineage: {error}"));
        install_conformance_replay_access_change(&connection, &mut trace, ACCESS_TRANSITION);
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(1)
                    .unwrap_or_else(|error| panic!("materialization Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize materialization Replay: {error}"));
        set_replay_slice_row_budget(&store.writer.path, 1);
        let continuation = match store
            .replay_authorized(&registry, replay)
            .unwrap_or_else(|error| panic!("begin materialization Replay: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => continuation,
            SqliteAuthorizedReplayOutcomeV1::Complete(_) => {
                panic!("one-row slice must defer before Transition one")
            }
        };
        connection
            .execute(
                "UPDATE room_materializations SET core_state_bytes = ?1 WHERE room_id = ?2",
                params![b"{}".as_slice(), ROOM],
            )
            .unwrap_or_else(|error| panic!("corrupt current Replay materialization: {error}"));
        assert!(matches!(
            store.resume_replay_authorized(&registry, continuation),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read materialization quarantine: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
        clear_replay_slice_row_budget(&store.writer.path);
    }

    #[test]
    fn authorized_replay_multislice_release_allows_an_append_after_captured_head() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock))
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open append Replay lineage: {error}"));
        install_conformance_replay_neutral_membership_history(
            &connection,
            &mut trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(2)
                    .unwrap_or_else(|error| panic!("append Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize append Replay: {error}"));
        let action_request = increment_request(&trace, ACTION_C);
        let action_grant = authorize_enabled_action(
            &authority,
            &action_request,
            ACTION_C,
            "2026-08-15T12:00:08Z",
        );
        let action = seal_authorized_increment(
            &trace,
            &action_request,
            ACTION_C,
            TRANSITION_C,
            "2026-08-15T12:00:08Z",
            0,
            action_grant,
        )
        .unwrap_or_else(|error| panic!("seal append Action: {error}"));
        set_replay_slice_row_budget(&store.writer.path, 1);
        let mut outcome = store
            .replay_authorized(&registry, replay)
            .unwrap_or_else(|error| panic!("begin append Replay: {error}"));
        assert!(matches!(
            &outcome,
            SqliteAuthorizedReplayOutcomeV1::Deferred(_)
        ));
        assert!(matches!(
            commit_existing_room_at(&store, &mut trace, action).resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        loop {
            outcome = match outcome {
                SqliteAuthorizedReplayOutcomeV1::Complete(projection) => {
                    assert_eq!(projection.verified_head().room_seq().get(), 2);
                    break;
                }
                SqliteAuthorizedReplayOutcomeV1::Deferred(continuation) => store
                    .resume_replay_authorized(&registry, continuation)
                    .unwrap_or_else(|error| panic!("resume append Replay: {error}")),
            };
        }
        let current_sequence: i64 = connection
            .query_row(
                "SELECT room_seq FROM rooms WHERE room_id = ?1",
                [ROOM],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("read appended Head: {error}"));
        assert_eq!(current_sequence, 3);
        clear_replay_slice_row_budget(&store.writer.path);
    }

    #[test]
    fn faulted_replay_rejects_and_quarantines_an_impossible_same_generation_append() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        const RESUME_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQB";
        let _serial = serialize_replay_projection_test();
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open faulted Replay lineage: {error}"));
        install_conformance_replay_neutral_membership_history(
            &connection,
            &mut trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        connection
            .execute(
                "UPDATE room_integrity SET status = 'faulted', generation = 2 \
                 WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("fault Replay integrity: {error}"));
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("faulted Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("authorize faulted Replay: {error}"));
        arm_replay_projection_pause(&store.writer.path);
        let replay_store = store.clone();
        let running = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            replay_store.replay_authorized(&registry, replay)
        });
        wait_until_replay_projection_pauses();
        install_conformance_replay_member_resume(&connection, &mut trace, RESUME_TRANSITION);
        release_replay_projection();
        assert!(matches!(
            running
                .join()
                .unwrap_or_else(|_| panic!("faulted Replay thread")),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read impossible faulted append: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 3));
    }

    #[test]
    fn replay_advanced_head_anchor_recomputes_captured_record_hashes() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        const RESUME_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQB";
        let _serial = serialize_replay_projection_test();
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open anchor Replay lineage: {error}"));
        install_conformance_replay_neutral_membership_history(
            &connection,
            &mut trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("anchor Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("authorize anchor Replay: {error}"));
        arm_replay_projection_pause(&store.writer.path);
        let replay_store = store.clone();
        let running = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            replay_store.replay_authorized(&registry, replay)
        });
        wait_until_replay_projection_pauses();
        install_conformance_replay_member_resume(&connection, &mut trace, RESUME_TRANSITION);
        connection
            .execute("DROP TRIGGER transitions_immutable_update", ())
            .unwrap_or_else(|error| panic!("drop Transition immutability trigger: {error}"));
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT transition_bytes FROM transitions WHERE room_id = ?1 AND room_seq = 2",
                [ROOM],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("read captured anchor bytes: {error}"));
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("decode captured anchor value: {error}"));
        assert_eq!(value["resulting_core_state"]["room_status"], "active");
        value["resulting_core_state"]["room_status"] =
            serde_json::Value::String("archived".to_owned());
        let mutated_json = serde_json::to_vec(&value)
            .unwrap_or_else(|error| panic!("serialize mutated anchor: {error}"));
        let mutated = CanonicalJsonV1::parse(&mutated_json)
            .unwrap_or_else(|error| panic!("canonical mutated anchor: {error}"))
            .to_bytes()
            .unwrap_or_else(|error| panic!("encode mutated anchor: {error}"));
        connection
            .execute(
                "UPDATE transitions SET transition_bytes = ?1 \
                 WHERE room_id = ?2 AND room_seq = 2",
                params![mutated, ROOM],
            )
            .unwrap_or_else(|error| panic!("mutate captured anchor state: {error}"));
        release_replay_projection();
        assert!(matches!(
            running
                .join()
                .unwrap_or_else(|_| panic!("anchor Replay thread")),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read anchor quarantine: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    #[test]
    fn authorized_replay_pages_release_the_writer_before_final_fencing() {
        const ACCESS_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
        let _serial = serialize_replay_projection_test();
        let revoke_file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let revoke_clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let revoke_store =
            SqliteRoomStore::open_with_clock(revoke_file.path(), Arc::new(revoke_clock))
                .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut revoke_trace, revoke_authority) = committed_trace_with_real_authority(
            &revoke_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(revoke_file.path())
            .unwrap_or_else(|error| panic!("open paged Replay lineage: {error}"));
        install_conformance_replay_access_change(&connection, &mut revoke_trace, ACCESS_TRANSITION);
        let replay = revoke_authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(1)
                    .unwrap_or_else(|error| panic!("paged Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize paged Replay: {error}"));
        arm_replay_projection_pause(&revoke_store.writer.path);
        let replay_store = revoke_store.clone();
        let running_replay = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            replay_store.replay_authorized(&registry, replay)
        });
        wait_until_replay_projection_pauses();
        let (observed_change, changed) = mpsc::channel();
        let changing = thread::spawn(move || {
            let result = revoke_authority.change(
                &presented_host_capability(),
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Replay Capability generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("replay_during_page_revoked")
                        .unwrap_or_else(|error| panic!("Replay revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:08Z"),
            );
            let _ = observed_change.send(result.is_ok());
            result
        });
        let changed_without_releasing_page = changed.recv_timeout(Duration::from_secs(2));
        release_replay_projection();
        let change = changing
            .join()
            .unwrap_or_else(|_| panic!("Replay revocation thread"));
        assert!(change.is_ok(), "Replay revocation must commit");
        assert_eq!(changed_without_releasing_page, Ok(true));
        assert!(matches!(
            running_replay
                .join()
                .unwrap_or_else(|_| panic!("paged Replay thread")),
            Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration
            ))
        ));

        let commit_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("commit Replay DB: {error}"));
        let commit_clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let commit_store =
            SqliteRoomStore::open_with_clock(commit_file.path(), Arc::new(commit_clock))
                .unwrap_or_else(|error| panic!("open commit Replay SQLite: {error}"));
        let (commit_trace, commit_authority) = committed_trace_with_real_authority(
            &commit_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let request = increment_request(&commit_trace, ACTION_A);
        let action = seal_authorized_increment(
            &commit_trace,
            &request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:04Z",
            0,
            authorize_enabled_action(
                &commit_authority,
                &request,
                ACTION_A,
                "2026-08-15T12:00:04Z",
            ),
        )
        .unwrap_or_else(|error| panic!("seal concurrent Action: {error}"));
        let replay = commit_authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("commit-race Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:05Z"),
            )
            .unwrap_or_else(|error| panic!("authorize commit-race Replay: {error}"));
        arm_replay_projection_pause(&commit_store.writer.path);
        let replay_store = commit_store.clone();
        let running_replay = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            replay_store.replay_authorized(&registry, replay)
        });
        wait_until_replay_projection_pauses();
        let commit_store_for_thread = commit_store.clone();
        let (observed_commit, committed) = mpsc::channel();
        let committing = thread::spawn(move || {
            let mut trace = commit_trace;
            let outcome = commit_existing_room_at(&commit_store_for_thread, &mut trace, action);
            let accepted = matches!(
                outcome.resolution(),
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::New,
                    ..
                }
            );
            let _ = observed_commit.send(accepted);
            accepted
        });
        let committed_without_releasing_page = committed.recv_timeout(Duration::from_secs(2));
        release_replay_projection();
        assert!(
            committing
                .join()
                .unwrap_or_else(|_| panic!("concurrent commit thread")),
            "Action must commit while Replay is paused between pages"
        );
        assert_eq!(committed_without_releasing_page, Ok(true));
        let projection = match running_replay
            .join()
            .unwrap_or_else(|_| panic!("commit-race Replay thread"))
            .unwrap_or_else(|error| panic!("append-safe Replay release: {error}"))
        {
            SqliteAuthorizedReplayOutcomeV1::Complete(projection) => projection,
            SqliteAuthorizedReplayOutcomeV1::Deferred(_) => {
                panic!("Genesis-only Replay completes in one slice")
            }
        };
        assert_eq!(projection.verified_head().room_seq().get(), 0);
    }

    #[test]
    fn authorized_replay_revalidates_revocation_and_expiry_before_sequence_lookup() {
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        let revoked_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("revoked Replay DB: {error}"));
        let revoked_clock = TestAuthorityClock::at("2026-08-15T12:00:10Z");
        let revoked_store =
            SqliteRoomStore::open_with_clock(revoked_file.path(), Arc::new(revoked_clock))
                .unwrap_or_else(|error| panic!("open revoked Replay SQLite: {error}"));
        let (_trace, revoked_authority) = committed_trace_with_real_authority(
            &revoked_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let unavailable_sequence = || {
            revoked_authority.authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(2)
                    .unwrap_or_else(|error| panic!("unavailable Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:06Z"),
            )
        };
        assert!(matches!(
            revoked_store.replay_authorized(
                &registry,
                unavailable_sequence()
                    .unwrap_or_else(|error| panic!("authorize absent-at-N Replay: {error}")),
            ),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::SequenceUnavailable
            ))
        ));
        let held = unavailable_sequence()
            .unwrap_or_else(|error| panic!("authorize held absent-at-N Replay: {error}"));
        revoked_authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Replay Capability generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("replay_revoked")
                        .unwrap_or_else(|error| panic!("Replay revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("revoke Replay Capability: {error}"));
        assert!(matches!(
            revoked_store.replay_authorized(&registry, held),
            Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration
            ))
        ));

        let expired_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("expired Replay DB: {error}"));
        let expired_clock = TestAuthorityClock::at("2026-08-15T12:00:04Z");
        let expired_store =
            SqliteRoomStore::open_with_clock(expired_file.path(), Arc::new(expired_clock.clone()))
                .unwrap_or_else(|error| panic!("open expired Replay SQLite: {error}"));
        let (_trace, expired_authority) = committed_trace_with_real_authority(
            &expired_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            Some("2026-08-15T12:00:05Z"),
        );
        let held = expired_authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("expiring Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize expiring Replay: {error}"));
        expired_clock.set("2026-08-15T12:00:05Z");
        assert!(matches!(
            expired_store.replay_authorized(&registry, held),
            Err(SqliteAuthorizedReplayErrorV1::Authority(
                AuthorityErrorV1::StaleAuthorityGeneration
            ))
        ));
    }

    #[test]
    fn replay_runtime_unavailable_faults_once_and_fences_later_mutation() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let action_request = increment_request(&trace, ACTION_A);
        let action_grant = authorize_enabled_action(
            &authority,
            &action_request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let held_action = seal_authorized_increment(
            &trace,
            &action_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            action_grant,
        )
        .unwrap_or_else(|error| panic!("seal Replay-fenced Action: {error}"));
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0).unwrap_or_else(|error| panic!("Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize runtime-absent Replay: {error}"));
        let missing_runtime = counter_v1_only_registry_for_conformance()
            .unwrap_or_else(|error| panic!("v1-only registry: {error}"));
        assert!(matches!(
            store.replay_authorized(&missing_runtime, replay),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::ReplayFailed(ReplayFailureClassV1::RuntimeUnavailable)
            ))
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect Replay runtime fault: {error}"));
        let integrity = || {
            connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap_or_else(|error| panic!("read Replay integrity: {error}"))
        };
        assert_eq!(integrity(), ("faulted".to_owned(), 2));
        let fenced = commit_existing_room_at(&store, &mut trace, held_action);
        assert!(matches!(
            fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));

        let repeated = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("repeated Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:05Z"),
            )
            .unwrap_or_else(|error| panic!("authorize repeated runtime-absent Replay: {error}"));
        assert!(matches!(
            store.replay_authorized(&missing_runtime, repeated),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::ReplayFailed(ReplayFailureClassV1::RuntimeUnavailable)
            ))
        ));
        assert_eq!(integrity(), ("faulted".to_owned(), 2));
    }

    #[test]
    fn replay_discovered_prefix_corruption_quarantines_and_fences_replay_and_mutation() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open corrupt-prefix lineage: {error}"));
        install_conformance_replay_neutral_membership_history(
            &connection,
            &mut trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        let action_request = increment_request(&trace, ACTION_C);
        let action_grant = authorize_enabled_action(
            &authority,
            &action_request,
            ACTION_C,
            "2026-08-15T12:00:05Z",
        );
        let held_action = seal_authorized_increment(
            &trace,
            &action_request,
            ACTION_C,
            TRANSITION_C,
            "2026-08-15T12:00:05Z",
            0,
            action_grant,
        )
        .unwrap_or_else(|error| panic!("seal corruption-fenced Action: {error}"));
        let held_replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("corrupt-prefix sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:06Z"),
            )
            .unwrap_or_else(|error| panic!("authorize corrupt-prefix Replay: {error}"));
        connection
            .execute("DROP TRIGGER room_genesis_immutable_update", ())
            .unwrap_or_else(|error| panic!("drop Genesis immutability trigger: {error}"));
        connection
            .execute(
                "UPDATE room_genesis SET genesis_bytes = ?1 WHERE room_id = ?2",
                params![b"{}".as_slice(), ROOM],
            )
            .unwrap_or_else(|error| panic!("corrupt Replay Genesis: {error}"));
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, held_replay),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read quarantined Replay integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
        assert!(matches!(
            commit_existing_room_at(&store, &mut trace, held_action).resolution(),
            RoomCommitResolutionV1::Fenced
        ));

        let later_replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("later Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:07Z"),
            )
            .unwrap_or_else(|error| panic!("authorize quarantined Replay: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, later_replay),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::IntegrityUnavailable
            ))
        ));
    }

    #[test]
    fn replay_incident_disposition_survives_a_valid_append_after_capture() {
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ9";
        const SUSPEND_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQA";
        let _serial = serialize_replay_projection_test();

        let runtime_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("runtime Replay DB: {error}"));
        let runtime_store = SqliteRoomStore::open(runtime_file.path())
            .unwrap_or_else(|error| panic!("open runtime Replay SQLite: {error}"));
        let (mut runtime_trace, runtime_authority) = committed_trace_with_real_authority(
            &runtime_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let request = increment_request(&runtime_trace, ACTION_A);
        let action = seal_authorized_increment(
            &runtime_trace,
            &request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:04Z",
            0,
            authorize_enabled_action(
                &runtime_authority,
                &request,
                ACTION_A,
                "2026-08-15T12:00:04Z",
            ),
        )
        .unwrap_or_else(|error| panic!("seal runtime append: {error}"));
        let replay = runtime_authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("runtime Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:05Z"),
            )
            .unwrap_or_else(|error| panic!("authorize runtime Replay: {error}"));
        arm_replay_projection_pause(&runtime_store.writer.path);
        let replay_store = runtime_store.clone();
        let running = thread::spawn(move || {
            let missing_runtime = counter_v1_only_registry_for_conformance()
                .unwrap_or_else(|error| panic!("missing Replay runtime: {error}"));
            replay_store.replay_authorized(&missing_runtime, replay)
        });
        wait_until_replay_projection_pauses();
        assert!(matches!(
            commit_existing_room_at(&runtime_store, &mut runtime_trace, action).resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        release_replay_projection();
        assert!(matches!(
            running
                .join()
                .unwrap_or_else(|_| panic!("runtime Replay thread")),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::ReplayFailed(ReplayFailureClassV1::RuntimeUnavailable)
            ))
        ));
        let runtime_connection = Connection::open(runtime_file.path())
            .unwrap_or_else(|error| panic!("inspect runtime Replay incident: {error}"));
        let runtime_integrity: (String, i64) = runtime_connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("runtime Replay integrity: {error}"));
        assert_eq!(runtime_integrity, ("faulted".to_owned(), 2));
        let later_request = increment_request(&runtime_trace, ACTION_B);
        let later = seal_authorized_increment(
            &runtime_trace,
            &later_request,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:06Z",
            0,
            authorize_enabled_action(
                &runtime_authority,
                &later_request,
                ACTION_B,
                "2026-08-15T12:00:06Z",
            ),
        )
        .unwrap_or_else(|error| panic!("seal runtime-fenced Action: {error}"));
        assert!(matches!(
            commit_existing_room_at(&runtime_store, &mut runtime_trace, later).resolution(),
            RoomCommitResolutionV1::Fenced
        ));

        let corrupt_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("corrupt Replay DB: {error}"));
        let corrupt_store = SqliteRoomStore::open(corrupt_file.path())
            .unwrap_or_else(|error| panic!("open corrupt Replay SQLite: {error}"));
        let (mut corrupt_trace, corrupt_authority) = committed_trace_with_real_authority(
            &corrupt_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomReplay],
            None,
        );
        let corrupt_connection = Connection::open(corrupt_file.path())
            .unwrap_or_else(|error| panic!("open corrupt Replay lineage: {error}"));
        install_conformance_replay_neutral_membership_history(
            &corrupt_connection,
            &mut corrupt_trace,
            JOIN_TRANSITION,
            SUSPEND_TRANSITION,
        );
        let request = increment_request(&corrupt_trace, ACTION_C);
        let action = seal_authorized_increment(
            &corrupt_trace,
            &request,
            ACTION_C,
            TRANSITION_C,
            "2026-08-15T12:00:07Z",
            0,
            authorize_enabled_action(
                &corrupt_authority,
                &request,
                ACTION_C,
                "2026-08-15T12:00:07Z",
            ),
        )
        .unwrap_or_else(|error| panic!("seal corrupt-prefix append: {error}"));
        let replay = corrupt_authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(1)
                    .unwrap_or_else(|error| panic!("corrupt Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:08Z"),
            )
            .unwrap_or_else(|error| panic!("authorize corrupt Replay: {error}"));
        arm_replay_projection_pause(&corrupt_store.writer.path);
        let replay_store = corrupt_store.clone();
        let running = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Replay registry: {error}"));
            replay_store.replay_authorized(&registry, replay)
        });
        wait_until_replay_projection_pauses();
        assert!(matches!(
            commit_existing_room_at(&corrupt_store, &mut corrupt_trace, action).resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        corrupt_connection
            .execute("DROP TRIGGER transitions_immutable_update", ())
            .unwrap_or_else(|error| panic!("drop Transition immutability trigger: {error}"));
        corrupt_connection
            .execute(
                "UPDATE transitions SET transition_bytes = ?1 \
                 WHERE room_id = ?2 AND room_seq = 1",
                params![b"{}".as_slice(), ROOM],
            )
            .unwrap_or_else(|error| panic!("corrupt captured Replay prefix: {error}"));
        release_replay_projection();
        assert!(matches!(
            running
                .join()
                .unwrap_or_else(|_| panic!("corrupt Replay thread")),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let corrupt_integrity: (String, i64) = corrupt_connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("corrupt Replay integrity: {error}"));
        assert_eq!(corrupt_integrity, ("quarantined".to_owned(), 2));
        let later_request = increment_request(&corrupt_trace, ACTION_B);
        let later = seal_authorized_increment(
            &corrupt_trace,
            &later_request,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:09Z",
            0,
            authorize_enabled_action(
                &corrupt_authority,
                &later_request,
                ACTION_B,
                "2026-08-15T12:00:09Z",
            ),
        )
        .unwrap_or_else(|error| panic!("seal corruption-fenced Action: {error}"));
        assert!(matches!(
            commit_existing_room_at(&corrupt_store, &mut corrupt_trace, later).resolution(),
            RoomCommitResolutionV1::Fenced
        ));
    }

    #[test]
    fn replay_failure_disposition_cannot_overwrite_a_newer_integrity_fence() {
        let _serial = serialize_replay_projection_test();
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (_trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let replay = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("racing Replay sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize racing Replay: {error}"));
        arm_replay_projection_pause(&store.writer.path);
        let replay_store = store.clone();
        let running = thread::spawn(move || {
            let missing_runtime = counter_v1_only_registry_for_conformance()
                .unwrap_or_else(|error| panic!("v1-only registry: {error}"));
            replay_store.replay_authorized(&missing_runtime, replay)
        });
        wait_until_replay_projection_pauses();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay race mutator: {error}"));
        connection
            .execute(
                "UPDATE room_integrity SET status = 'quarantined', generation = 2 \
                 WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("advance Replay integrity fence: {error}"));
        release_replay_projection();
        assert!(matches!(
            running
                .join()
                .unwrap_or_else(|_| panic!("Replay projection thread")),
            Err(SqliteAuthorizedReplayErrorV1::Replay(
                HistoricalReplayErrorV1::ReplayFailed(ReplayFailureClassV1::RuntimeUnavailable)
            ))
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read newer Replay integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    #[test]
    fn authority_current_anchor_rejects_tampered_or_misaddressed_seq_two_predecessor() {
        const ACCESS_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
        const JOIN_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ8";
        enum PredecessorCorruption {
            Scalar,
            Bytes,
            WrongSequence,
            WrongRoom,
        }
        for corruption in [
            PredecessorCorruption::Scalar,
            PredecessorCorruption::Bytes,
            PredecessorCorruption::WrongSequence,
            PredecessorCorruption::WrongRoom,
        ] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("authority DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open authority SQLite: {error}"));
            let (mut trace, authority) = committed_trace_with_real_authority(
                &store,
                MembershipStandingV1::Enabled,
                [CapabilityScopeV1::RoomReplay],
                None,
            );
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open authority lineage: {error}"));
            install_conformance_replay_access_change(&connection, &mut trace, ACCESS_TRANSITION);
            install_conformance_replay_member_join(&connection, &mut trace, JOIN_TRANSITION);
            assert_eq!(trace.head().room_seq().get(), 2);
            connection
                .execute("DROP TRIGGER transitions_immutable_update", ())
                .unwrap_or_else(|error| panic!("drop Transition immutability trigger: {error}"));
            match corruption {
                PredecessorCorruption::Scalar => {
                    connection
                        .execute(
                            "UPDATE transitions SET transition_hash = 'tampered' \
                             WHERE room_id = ?1 AND room_seq = 1",
                            [ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt predecessor scalar: {error}"));
                }
                PredecessorCorruption::Bytes => {
                    connection
                        .execute(
                            "UPDATE transitions SET transition_bytes = ?1 \
                             WHERE room_id = ?2 AND room_seq = 1",
                            params![b"{}".as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt predecessor bytes: {error}"));
                }
                PredecessorCorruption::WrongSequence => {
                    connection
                        .execute(
                            "UPDATE transitions SET \
                                 transition_hash = (SELECT transition_hash FROM transitions \
                                     WHERE room_id = ?1 AND room_seq = 2), \
                                 transition_bytes = (SELECT transition_bytes FROM transitions \
                                     WHERE room_id = ?1 AND room_seq = 2) \
                             WHERE room_id = ?1 AND room_seq = 1",
                            [ROOM],
                        )
                        .unwrap_or_else(|error| panic!("mis-sequence predecessor: {error}"));
                }
                PredecessorCorruption::WrongRoom => {
                    let (alt_genesis, _, _, _) = creation_fixture_with_generated(
                        0,
                        ROOM_ALT,
                        PARTICIPANT,
                        SEED,
                        "2026-08-15T12:00:00Z",
                        "wrong-room-predecessor",
                    );
                    let mut alt_trace =
                        CoreTraceV1::create_from_retained_for_conformance(alt_genesis)
                            .unwrap_or_else(|error| panic!("alternate predecessor trace: {error}"));
                    let alt_transition = advance_conformance_replay_access_change(&mut alt_trace);
                    connection
                        .execute(
                            "UPDATE transitions SET transition_hash = ?1, transition_bytes = ?2 \
                             WHERE room_id = ?3 AND room_seq = 1",
                            params![
                                alt_transition.transition_hash().to_string(),
                                alt_transition.canonical_bytes().unwrap_or_else(|error| {
                                    panic!("alternate predecessor bytes: {error}")
                                }),
                                ROOM,
                            ],
                        )
                        .unwrap_or_else(|error| panic!("wrong-Room predecessor: {error}"));
                }
            }
            assert!(matches!(
                authority.authorize_replay(
                    &presented_member_capability(),
                    parsed(ROOM),
                    parsed(PARTICIPANT),
                    RoomSequenceV1::new(0)
                        .unwrap_or_else(|error| panic!("anchor Replay sequence: {error}")),
                    ReplayProjectionKindV1::HistoricalMembership,
                    parsed("2026-08-15T12:00:07Z"),
                ),
                Err(AuthorityErrorV1::Unavailable)
            ));
        }
    }

    #[test]
    fn replay_missing_integrity_row_is_durably_replaced_by_quarantine() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("Replay DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Replay SQLite: {error}"));
        let (_trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomReplay],
            None,
        );
        let held = authority
            .authorize_replay(
                &presented_member_capability(),
                parsed(ROOM),
                parsed(PARTICIPANT),
                RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| panic!("missing-integrity sequence: {error}")),
                ReplayProjectionKindV1::HistoricalMembership,
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("authorize missing-integrity Replay: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open missing-integrity mutator: {error}"));
        connection
            .execute("DELETE FROM room_integrity WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("remove integrity row: {error}"));
        let registry =
            builtin_counter_registry().unwrap_or_else(|error| panic!("Replay registry: {error}"));
        assert!(matches!(
            store.replay_authorized(&registry, held),
            Err(SqliteAuthorizedReplayErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read rebuilt Replay integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 1));
    }

    #[test]
    fn registered_runner_controls_then_durable_revocation_denies_immediately() {
        const BOOTSTRAP_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FP0";
        const REGISTER_RUNNER_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FP1";
        const REGISTER_RUNNER_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FP2";
        const REVOKE_RUNNER_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FP3";
        const RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FM0";
        const RUNNER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH5";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:01Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock.clone()))
            .unwrap_or_else(|error| panic!("open Runner SQLite: {error}"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let host_bearer = CapabilityBearerV1::from_bytes([0xB1; 32]);
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    parsed(BOOTSTRAP_CHANGE),
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Agent,
                    parsed(HOST_CAPABILITY),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| panic!("Runner bootstrap request: {error}")),
                parsed("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("Runner bootstrap: {error}"));
        let presented_host = PresentedCapabilityV1::new(parsed(HOST_CAPABILITY), host_bearer);

        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Runner Counter registry: {error}"));
        let configuration = canonical(br#"{"initial_value":0,"maximum_value":4}"#);
        let participant = MembershipV1::new(
            parsed(PARTICIPANT),
            parsed(PRINCIPAL),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| panic!("Runner participant: {error}"));
        let genesis = registry
            .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
                room_id: parsed(ROOM),
                pack_digest: counter_v2_digest(),
                configuration: configuration.clone(),
                room_seed: parsed(SEED),
                created_at: parsed("2026-08-15T12:00:00Z"),
                initial_core_state: CoreRoomStateV1::active([participant])
                    .unwrap_or_else(|error| panic!("Runner Core state: {error}")),
            })
            .unwrap_or_else(|error| panic!("Runner Genesis: {error}"));
        let request = RoomCreationRequestV1::new(
            counter_v2_digest(),
            configuration,
            vec![
                InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Agent,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )
                .unwrap_or_else(|error| panic!("Runner initial Membership: {error}")),
            ],
        );
        let identity = AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
            idempotency_key: "create-runner-room".to_owned(),
        };
        let grant = match authorize_room_creation_operation(
            &authority,
            &store,
            &presented_host,
            &identity,
            &request,
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap_or_else(|error| panic!("authorize Runner Room: {error}"))
        {
            RoomCreationIngressV1::Authorized(grant) => grant,
            other => panic!("new Runner Room unexpectedly resolved a receipt: {other:?}"),
        };
        let prepared =
            PreparedRoomCreationV1::from_registry_genesis(identity, &request, *grant, genesis)
                .unwrap_or_else(|error| panic!("prepare Runner Room: {error}"));
        assert!(matches!(
            commit_room_creation_at(&store, prepared).resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));

        clock.set("2026-08-15T12:01:00Z");
        let registered = authority
            .change(
                &presented_host,
                AuthorityChangeV1::RegisterRunner {
                    change_id: parsed(REGISTER_RUNNER_CHANGE),
                    runner_id: parsed(RUNNER),
                    owner_principal_id: parsed(PRINCIPAL),
                },
                parsed("2026-08-15T12:01:00Z"),
            )
            .unwrap_or_else(|error| panic!("register Runner: {error}"));
        assert_eq!(
            registered.result(),
            AuthorityChangeResultV1::RunnerRegistered
        );
        let runner_bearer = CapabilityBearerV1::from_bytes([0xB2; 32]);
        let permitted_memberships = RunnerMembershipSetV1::new([RoomMembershipKeyV1 {
            room_id: parsed(ROOM),
            member_id: parsed(PARTICIPANT),
        }])
        .unwrap_or_else(|error| panic!("Runner Membership set: {error}"));
        let runner_capability = NewCapabilityV1::new(
            parsed(RUNNER_CAPABILITY),
            runner_bearer.token_hash(),
            parsed(PRINCIPAL),
            CapabilityProfileV1::RunnerControl {
                runner_id: parsed(RUNNER),
                permitted_memberships,
            },
            CapabilityScopeSetV1::new([CapabilityScopeV1::ActivationOfferReceive])
                .unwrap_or_else(|error| panic!("Runner scopes: {error}")),
            None,
        )
        .unwrap_or_else(|error| panic!("Runner Capability: {error}"));
        clock.set("2026-08-15T12:02:00Z");
        authority
            .change(
                &presented_host,
                AuthorityChangeV1::RegisterCapability {
                    change_id: parsed(REGISTER_RUNNER_CAPABILITY_CHANGE),
                    capability: runner_capability,
                },
                parsed("2026-08-15T12:02:00Z"),
            )
            .unwrap_or_else(|error| panic!("register Runner Capability: {error}"));
        let presented_runner = PresentedCapabilityV1::new(parsed(RUNNER_CAPABILITY), runner_bearer);
        let control_use = || AuthorityUseV1::RunnerControl {
            runner_id: parsed(RUNNER),
            operation: RunnerControlOperationV1::ReceiveOffer,
            target: Some(RoomMembershipKeyV1 {
                room_id: parsed(ROOM),
                member_id: parsed(PARTICIPANT),
            }),
        };
        assert!(matches!(
            authority.authorize(
                &presented_runner,
                control_use(),
                parsed("2026-08-15T12:03:00Z"),
            ),
            Ok(AuthorityGrantV1::RunnerControl(_))
        ));

        clock.set("2026-08-15T12:04:00Z");
        let revoked = authority
            .change(
                &presented_host,
                AuthorityChangeV1::RevokeRunner {
                    change_id: parsed(REVOKE_RUNNER_CHANGE),
                    runner_id: parsed(RUNNER),
                    expected_generation: RunnerGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Runner generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("runner_revoked")
                        .unwrap_or_else(|error| panic!("Runner revoke reason: {error}")),
                },
                parsed("2026-08-15T12:04:00Z"),
            )
            .unwrap_or_else(|error| panic!("revoke Runner: {error}"));
        assert_eq!(revoked.result(), AuthorityChangeResultV1::RunnerRevoked);
        assert!(matches!(
            authority.authorize(
                &presented_runner,
                control_use(),
                parsed("2026-08-15T12:04:00Z"),
            ),
            Err(AuthorityErrorV1::Forbidden)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect revoked Runner: {error}"));
        let runner: (String, i64) = connection
            .query_row(
                "SELECT authority_status, runner_generation FROM runners WHERE runner_id = ?1",
                [RUNNER],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("durable Runner: {error}"));
        assert_eq!(runner, ("revoked".to_owned(), 2));
    }

    #[test]
    fn authority_store_persists_validated_generations_revocation_receipts_and_audit() {
        const ADMIN_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH0";
        const TARGET_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH1";
        const CREATE_PRINCIPAL_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ0";
        const REGISTER_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ1";
        const NARROW_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ2";
        const REVOKE_CAPABILITY_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ3";

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let clock = TestAuthorityClock::at("2026-08-15T12:00:00Z");
        let store = SqliteRoomStore::open_with_clock(file.path(), Arc::new(clock.clone()))
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let admin_bearer_bytes = [0xA5; 32];
        let admin_capability_id: CapabilityId = parsed(ADMIN_CAPABILITY);
        let admin_principal = PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("Principal generation: {error}")),
        );
        let admin_capability =
            CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                capability_id: admin_capability_id.clone(),
                token_hash: CapabilityBearerV1::from_bytes(admin_bearer_bytes).token_hash(),
                principal_id: admin_principal.principal_id().clone(),
                profile: CapabilityProfileV1::HostOperator { room_id: None },
                scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])
                    .unwrap_or_else(|error| panic!("admin scopes: {error}")),
                generation: AuthorityGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("Capability generation: {error}")),
                expires_at: None,
                revoked_at: None,
            })
            .unwrap_or_else(|error| panic!("admin Capability: {error}"));
        store
            .seed_authority_snapshot(admin_principal, admin_capability)
            .unwrap_or_else(|error| panic!("seed authority snapshot: {error}"));
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let presented_admin = || {
            PresentedCapabilityV1::new(
                admin_capability_id.clone(),
                CapabilityBearerV1::from_bytes(admin_bearer_bytes),
            )
        };

        let create_principal = AuthorityChangeV1::CreatePrincipal {
            change_id: parsed(CREATE_PRINCIPAL_CHANGE),
            principal_id: parsed(PRINCIPAL_ALT),
            kind: PrincipalKindV1::Agent,
        };
        let created = authority
            .change(
                &presented_admin(),
                create_principal.clone(),
                parsed::<AuthorityCheckedAt>("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("create Principal: {error}"));
        assert_eq!(created.result(), AuthorityChangeResultV1::PrincipalCreated);
        assert_eq!(created.resulting_generation(), 1);
        let duplicate = authority
            .change(
                &presented_admin(),
                create_principal,
                parsed::<AuthorityCheckedAt>("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("duplicate Principal change: {error}"));
        assert_eq!(duplicate, created);

        let target_bearer = CapabilityBearerV1::from_bytes([0x5A; 32]);
        let target_capability = NewCapabilityV1::new(
            parsed(TARGET_CAPABILITY),
            target_bearer.token_hash(),
            parsed(PRINCIPAL_ALT),
            CapabilityProfileV1::HostOperator {
                room_id: Some(parsed(ROOM)),
            },
            CapabilityScopeSetV1::new([
                CapabilityScopeV1::OperatorRoomAdmin,
                CapabilityScopeV1::OperatorBackup,
            ])
            .unwrap_or_else(|error| panic!("target scopes: {error}")),
            None,
        )
        .unwrap_or_else(|error| panic!("target Capability: {error}"));
        let register = AuthorityChangeV1::RegisterCapability {
            change_id: parsed(REGISTER_CAPABILITY_CHANGE),
            capability: target_capability,
        };
        clock.set("2026-08-15T12:01:00Z");
        let registered = authority
            .change(
                &presented_admin(),
                register.clone(),
                parsed::<AuthorityCheckedAt>("2026-08-15T12:01:00Z"),
            )
            .unwrap_or_else(|error| panic!("register Capability: {error}"));
        assert_eq!(
            registered.result(),
            AuthorityChangeResultV1::CapabilityRegistered
        );
        assert_eq!(registered.resulting_generation(), 1);

        let narrow = AuthorityChangeV1::NarrowCapability {
            change_id: parsed(NARROW_CAPABILITY_CHANGE),
            capability_id: parsed(TARGET_CAPABILITY),
            expected_generation: AuthorityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("expected generation: {error}")),
            scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorBackup])
                .unwrap_or_else(|error| panic!("narrow scopes: {error}")),
            expires_at: Some(parsed("2026-08-16T00:00:00Z")),
            reason_code: AuthorityReasonCodeV1::new("least_privilege")
                .unwrap_or_else(|error| panic!("reason: {error}")),
        };
        clock.set("2026-08-15T12:02:00Z");
        let narrowed = authority
            .change(
                &presented_admin(),
                narrow,
                parsed::<AuthorityCheckedAt>("2026-08-15T12:02:00Z"),
            )
            .unwrap_or_else(|error| panic!("narrow Capability: {error}"));
        assert_eq!(narrowed.resulting_generation(), 2);

        let revoke = AuthorityChangeV1::RevokeCapability {
            change_id: parsed(REVOKE_CAPABILITY_CHANGE),
            capability_id: parsed(TARGET_CAPABILITY),
            expected_generation: AuthorityGenerationV1::new(2)
                .unwrap_or_else(|error| panic!("expected generation: {error}")),
            reason_code: AuthorityReasonCodeV1::new("operator_revoked")
                .unwrap_or_else(|error| panic!("reason: {error}")),
        };
        clock.set("2026-08-15T12:03:00Z");
        let revoked = authority
            .change(
                &presented_admin(),
                revoke,
                parsed::<AuthorityCheckedAt>("2026-08-15T12:03:00Z"),
            )
            .unwrap_or_else(|error| panic!("revoke Capability: {error}"));
        assert_eq!(revoked.result(), AuthorityChangeResultV1::CapabilityRevoked);
        assert_eq!(revoked.resulting_generation(), 3);

        let replayed_register = authority
            .change(
                &presented_admin(),
                register,
                parsed::<AuthorityCheckedAt>("2026-08-15T12:01:00Z"),
            )
            .unwrap_or_else(|error| panic!("replay Register change: {error}"));
        assert_eq!(replayed_register, registered);
        clock.set("2026-08-15T12:04:00Z");
        let conflict = authority.change(
            &presented_admin(),
            AuthorityChangeV1::CreatePrincipal {
                change_id: parsed(REGISTER_CAPABILITY_CHANGE),
                principal_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FD2"),
                kind: PrincipalKindV1::Human,
            },
            parsed::<AuthorityCheckedAt>("2026-08-15T12:04:00Z"),
        );
        assert_eq!(conflict, Err(AuthorityErrorV1::Conflict));

        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect authority database: {error}"));
        let target: (i64, Option<String>) = connection
            .query_row(
                "SELECT authority_generation, revoked_at FROM capabilities \
                 WHERE capability_id = ?1",
                [TARGET_CAPABILITY],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("target Capability row: {error}"));
        assert_eq!(target, (3, Some("2026-08-15T12:03:00Z".to_owned())));
        let scopes: Vec<String> = connection
            .prepare("SELECT scope FROM capability_scopes WHERE capability_id = ?1 ORDER BY scope")
            .and_then(|mut statement| {
                statement
                    .query_map([TARGET_CAPABILITY], |row| row.get(0))?
                    .collect()
            })
            .unwrap_or_else(|error| panic!("target Capability scopes: {error}"));
        assert_eq!(scopes, ["operator:backup"]);
        let counts: (i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM authority_change_receipts), \
                 (SELECT count(*) FROM authority_audit), (SELECT count(*) FROM rooms)",
                (),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|error| panic!("authority counts: {error}"));
        assert_eq!(counts, (4, 4, 0));
    }

    #[test]
    fn production_action_grants_are_request_purpose_sealed() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let request_a = increment_request(&trace, ACTION_A);
        let grant_a =
            authorize_enabled_action(&authority, &request_a, ACTION_A, "2026-08-15T12:00:03Z");
        let request_b = increment_request(&trace, ACTION_B);
        let mismatched = seal_authorized_increment(
            &trace,
            &request_b,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:03Z",
            0,
            grant_a,
        );
        assert!(matches!(
            mismatched,
            Err(PrepareRoomWriteErrorV1::AuthorityPurposeMismatch)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect purpose-sealed database: {error}"));
        let counts: (i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM semantic_receipts), \
                 (SELECT count(*) FROM transitions)",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("purpose-sealed counts: {error}"));
        assert_eq!(counts, (1, 0));
    }

    #[test]
    fn commit_time_expiry_and_revoke_first_fence_but_commit_first_duplicate_wins() {
        let expiry_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("expiry temp DB: {error}"));
        let expiry_clock = TestAuthorityClock::at("2026-08-15T12:00:04Z");
        let expiry_store =
            SqliteRoomStore::open_with_clock(expiry_file.path(), Arc::new(expiry_clock.clone()))
                .unwrap_or_else(|error| panic!("open expiry SQLite: {error}"));
        let (mut expiry_trace, expiry_authority) = committed_trace_with_real_authority(
            &expiry_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            Some("2026-08-15T12:00:05Z"),
        );
        let expiry_request = increment_request(&expiry_trace, ACTION_A);
        let expiry_grant = authorize_enabled_action(
            &expiry_authority,
            &expiry_request,
            ACTION_A,
            "2026-08-15T12:00:04Z",
        );
        let expiry_plan = seal_authorized_increment(
            &expiry_trace,
            &expiry_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:04Z",
            0,
            expiry_grant,
        )
        .unwrap_or_else(|error| panic!("seal expiring Action: {error}"));
        expiry_clock.set("2026-08-15T12:00:05Z");
        let expired = commit_existing_room_at(&expiry_store, &mut expiry_trace, expiry_plan);
        assert!(matches!(
            expired.resolution(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(expired.actor_installation(), ActorInstallationV1::Unchanged);
        assert_eq!(expiry_trace.head().room_seq().get(), 0);
        let expiry_connection = Connection::open(expiry_file.path())
            .unwrap_or_else(|error| panic!("inspect expiry database: {error}"));
        let expiry_counts: (i64, i64) = expiry_connection
            .query_row(
                "SELECT (SELECT count(*) FROM semantic_receipts), \
                 (SELECT count(*) FROM transitions)",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("expiry counts: {error}"));
        assert_eq!(expiry_counts, (1, 0));

        let revoke_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("revoke temp DB: {error}"));
        let revoke_store = SqliteRoomStore::open(revoke_file.path())
            .unwrap_or_else(|error| panic!("open revoke SQLite: {error}"));
        let (mut revoke_trace, revoke_authority) = committed_trace_with_real_authority(
            &revoke_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let revoke_request = increment_request(&revoke_trace, ACTION_A);
        let revoke_grant = authorize_enabled_action(
            &revoke_authority,
            &revoke_request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let revoke_plan = seal_authorized_increment(
            &revoke_trace,
            &revoke_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            revoke_grant,
        )
        .unwrap_or_else(|error| panic!("seal revoke-first Action: {error}"));
        let revoked = revoke_authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("member generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("revoke_first")
                        .unwrap_or_else(|error| panic!("revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("revoke before commit: {error}"));
        assert_eq!(revoked.result(), AuthorityChangeResultV1::CapabilityRevoked);
        let fenced = commit_existing_room_at(&revoke_store, &mut revoke_trace, revoke_plan);
        assert!(matches!(
            fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(revoke_trace.head().room_seq().get(), 0);

        let commit_first_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("commit-first temp DB: {error}"));
        let commit_first_store = SqliteRoomStore::open(commit_first_file.path())
            .unwrap_or_else(|error| panic!("open commit-first SQLite: {error}"));
        let (mut commit_first_trace, commit_first_authority) = committed_trace_with_real_authority(
            &commit_first_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let request = increment_request(&commit_first_trace, ACTION_A);
        let winner_grant = authorize_enabled_action(
            &commit_first_authority,
            &request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let duplicate_grant = authorize_enabled_action(
            &commit_first_authority,
            &request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let winner = seal_authorized_increment(
            &commit_first_trace,
            &request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            winner_grant,
        )
        .unwrap_or_else(|error| panic!("seal commit-first winner: {error}"));
        let duplicate = seal_authorized_increment(
            &commit_first_trace,
            &request,
            ACTION_A,
            TRANSITION_B,
            "2026-08-15T12:00:03Z",
            0,
            duplicate_grant,
        )
        .unwrap_or_else(|error| panic!("seal commit-first duplicate: {error}"));
        let winner_outcome =
            commit_existing_room_at(&commit_first_store, &mut commit_first_trace, winner);
        assert!(matches!(
            winner_outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        commit_first_authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::RevokeCapability {
                    change_id: parsed(REVOKE_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("member generation: {error}")),
                    reason_code: AuthorityReasonCodeV1::new("commit_first")
                        .unwrap_or_else(|error| panic!("revoke reason: {error}")),
                },
                parsed("2026-08-15T12:00:05Z"),
            )
            .unwrap_or_else(|error| panic!("revoke after commit: {error}"));
        let duplicate_outcome =
            commit_existing_room_at(&commit_first_store, &mut commit_first_trace, duplicate);
        assert!(matches!(
            duplicate_outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(commit_first_trace.head().room_seq().get(), 1);
        let commit_first_connection = Connection::open(commit_first_file.path())
            .unwrap_or_else(|error| panic!("inspect commit-first database: {error}"));
        let membership_generation: i64 = commit_first_connection
            .query_row(
                "SELECT membership_generation FROM room_members \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("read stable Membership generation: {error}"));
        assert_eq!(membership_generation, 1);
    }

    #[test]
    fn principal_scope_and_membership_generation_drift_each_fence_the_sealed_action() {
        let scope_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("scope temp DB: {error}"));
        let scope_store = SqliteRoomStore::open(scope_file.path())
            .unwrap_or_else(|error| panic!("open scope SQLite: {error}"));
        let (mut scope_trace, scope_authority) = committed_trace_with_real_authority(
            &scope_store,
            MembershipStandingV1::Enabled,
            [
                CapabilityScopeV1::RoomAct,
                CapabilityScopeV1::RoomObserveMember,
            ],
            None,
        );
        let scope_request = increment_request(&scope_trace, ACTION_A);
        let scope_grant = authorize_enabled_action(
            &scope_authority,
            &scope_request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let scope_plan = seal_authorized_increment(
            &scope_trace,
            &scope_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            scope_grant,
        )
        .unwrap_or_else(|error| panic!("seal scope-fenced Action: {error}"));
        scope_authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::NarrowCapability {
                    change_id: parsed(NARROW_MEMBER_CAPABILITY_CHANGE),
                    capability_id: parsed(MEMBER_CAPABILITY),
                    expected_generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("member generation: {error}")),
                    scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::RoomObserveMember])
                        .unwrap_or_else(|error| panic!("narrowed member scopes: {error}")),
                    expires_at: None,
                    reason_code: AuthorityReasonCodeV1::new("scope_changed")
                        .unwrap_or_else(|error| panic!("narrow reason: {error}")),
                },
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("narrow before commit: {error}"));
        let scope_fenced = commit_existing_room_at(&scope_store, &mut scope_trace, scope_plan);
        assert!(matches!(
            scope_fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));

        let principal_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("Principal temp DB: {error}"));
        let principal_store = SqliteRoomStore::open(principal_file.path())
            .unwrap_or_else(|error| panic!("open Principal SQLite: {error}"));
        let (mut principal_trace, principal_authority) = committed_trace_with_real_authority(
            &principal_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let principal_request = increment_request(&principal_trace, ACTION_A);
        let principal_grant = authorize_enabled_action(
            &principal_authority,
            &principal_request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let principal_plan = seal_authorized_increment(
            &principal_trace,
            &principal_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            principal_grant,
        )
        .unwrap_or_else(|error| panic!("seal Principal-fenced Action: {error}"));
        let disabled = principal_authority
            .change(
                &presented_host_capability(),
                AuthorityChangeV1::SetPrincipalStatus {
                    change_id: parsed(DISABLE_MEMBER_PRINCIPAL_CHANGE),
                    principal_id: parsed(PRINCIPAL),
                    expected_generation: PrincipalGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("Principal generation: {error}")),
                    status: PrincipalAuthorityStatusV1::Disabled,
                    reason_code: AuthorityReasonCodeV1::new("principal_disabled")
                        .unwrap_or_else(|error| panic!("disable reason: {error}")),
                },
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("disable Principal before commit: {error}"));
        assert_eq!(
            disabled.result(),
            AuthorityChangeResultV1::PrincipalStatusChanged
        );
        let principal_fenced =
            commit_existing_room_at(&principal_store, &mut principal_trace, principal_plan);
        assert!(matches!(
            principal_fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));

        let membership_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("Membership temp DB: {error}"));
        let membership_store = SqliteRoomStore::open(membership_file.path())
            .unwrap_or_else(|error| panic!("open Membership SQLite: {error}"));
        let (mut membership_trace, membership_authority) = committed_trace_with_real_authority(
            &membership_store,
            MembershipStandingV1::Enabled,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let membership_request = increment_request(&membership_trace, ACTION_A);
        let membership_grant = authorize_enabled_action(
            &membership_authority,
            &membership_request,
            ACTION_A,
            "2026-08-15T12:00:03Z",
        );
        let membership_plan = seal_authorized_increment(
            &membership_trace,
            &membership_request,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:03Z",
            0,
            membership_grant,
        )
        .unwrap_or_else(|error| panic!("seal Membership-fenced Action: {error}"));
        let membership_connection = Connection::open(membership_file.path())
            .unwrap_or_else(|error| panic!("open Membership fence fixture: {error}"));
        membership_connection
            .execute(
                "UPDATE room_members SET membership_generation = 2 \
                 WHERE room_id = ?1 AND member_id = ?2",
                params![ROOM, PARTICIPANT],
            )
            .unwrap_or_else(|error| panic!("advance Membership generation: {error}"));
        let membership_fenced =
            commit_existing_room_at(&membership_store, &mut membership_trace, membership_plan);
        assert!(matches!(
            membership_fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));
    }

    #[test]
    fn suspended_member_authority_records_the_stable_rejection_without_a_transition() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (mut trace, authority) = committed_trace_with_real_authority(
            &store,
            MembershipStandingV1::Suspended,
            [CapabilityScopeV1::RoomAct],
            None,
        );
        let request = increment_request(&trace, ACTION_A);
        let grant = authorize_stable_action(&authority, &request, ACTION_A, "2026-08-15T12:00:03Z");
        let plan = PreparedRoomCommitV1::for_authorized_stable_action_disposition(
            &trace,
            &request,
            parsed("2026-08-15T12:00:03Z"),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("integrity generation: {error}")),
            ParticipantActionAuthorityV1::StableMembershipNotEnabled(grant),
        )
        .unwrap_or_else(|error| panic!("seal stable disabled-Membership Action: {error}"));
        let outcome = commit_existing_room_at(&store, &mut trace, plan);
        match outcome.resolution() {
            RoomCommitResolutionV1::RejectionRecorded {
                status: ResolutionStatusV1::New,
                result,
            } => assert!(matches!(
                result.result(),
                SemanticResultV1::RejectionRecorded { code, .. }
                    if code == "membership_not_enabled"
            )),
            other => panic!("stable disabled-Membership receipt, got {other:?}"),
        }
        assert_eq!(outcome.actor_installation(), ActorInstallationV1::Unchanged);
        assert_eq!(trace.head().room_seq().get(), 0);
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect stable Action database: {error}"));
        let counts: (i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM semantic_receipts), \
                 (SELECT count(*) FROM transitions)",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("stable Action counts: {error}"));
        assert_eq!(counts, (2, 0));
    }

    #[test]
    fn create_duplicate_conflict_and_guarded_resolve_use_the_durable_receipt() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (prepared, witness, _identity) = prepared_creation(0);
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed authority: {error}"));

        let first = store.commit(&prepared);
        assert!(matches!(
            first,
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let original_bytes = first
            .stored_result()
            .unwrap_or_else(|| panic!("created result"))
            .canonical_receipt_bytes()
            .to_vec();
        let duplicate = store.commit(&prepared);
        assert!(matches!(
            duplicate,
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            duplicate
                .stored_result()
                .unwrap_or_else(|| panic!("duplicate result"))
                .canonical_receipt_bytes(),
            original_bytes
        );
        assert!(matches!(
            store.resolve(prepared.identity(), prepared.request_hash()),
            ResolveOutcomeV1::StoredResolution(_)
        ));

        let (changed, _, _) = prepared_creation(1);
        assert!(matches!(
            store.commit(&changed),
            RoomCommitResolutionV1::Conflict { .. }
        ));

        let reader = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("read committed DB: {error}"));
        for table in [
            "semantic_receipts",
            "rooms",
            "room_genesis",
            "room_materializations",
            "room_members",
            "room_integrity",
        ] {
            let count: i64 = reader
                .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("count {table}: {error}"));
            assert_eq!(count, 1, "{table}");
        }
        let frame_count: i64 = reader
            .query_row("SELECT count(*) FROM observation_frames", (), |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("count frames: {error}"));
        assert_eq!(frame_count, 0, "Genesis does not create frames");
    }

    #[test]
    fn create_unknown_commit_withholds_trace_until_durable_reload() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (genesis, request, witness, identity) = creation_fixture(0);
        let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity,
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("prepare unknown Create: {error}"));
        let expected_receipt = prepared
            .semantic_result()
            .canonical_receipt_bytes()
            .to_vec();
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed Create authority: {error}"));
        store.set_failpoint(Some(WriteBoundary::AfterCommitUnknown));
        let outcome = commit_room_creation(&store, prepared);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::Indeterminate
        ));
        let (_, trace, pending) = outcome.into_parts();
        assert!(
            trace.is_none(),
            "unknown Create cannot publish speculative Genesis"
        );
        let Some(RoomCreationPendingAttemptV1::ResolveOnly(resolve)) = pending else {
            panic!("unknown Create retains resolve-only capability")
        };
        store.set_failpoint(None);
        let resolved = resolve.resolve(&store);
        assert!(matches!(
            resolved.resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved
                .resolution()
                .stored_result()
                .unwrap_or_else(|| panic!("resolved Create receipt"))
                .canonical_receipt_bytes(),
            expected_receipt
        );
        assert!(
            resolved.into_committed_trace().is_none(),
            "resolved duplicate requires durable reload"
        );
    }

    #[test]
    fn generated_room_collision_reseals_only_create_values_and_lost_absence_can_regenerate() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (occupied_genesis, occupied_request, witness, occupied_identity) =
            creation_fixture_with_generated(
                1,
                ROOM,
                PARTICIPANT,
                SEED,
                "2026-08-15T12:00:00Z",
                "different-create-identity",
            );
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed collision authority: {error}"));
        let occupied = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            occupied_identity,
            &occupied_request,
            witness.clone(),
            occupied_genesis,
        )
        .unwrap_or_else(|error| panic!("prepare occupied Room: {error}"));
        assert!(matches!(
            commit_room_creation(&store, occupied).resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));

        let (revoked_genesis, revoked_request, _, revoked_identity) = creation_fixture(0);
        let revoked_candidate = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            revoked_identity,
            &revoked_request,
            witness.clone(),
            revoked_genesis,
        )
        .unwrap_or_else(|error| panic!("prepare revoked colliding Room: {error}"));
        store
            .seed_authority(&witness, false)
            .unwrap_or_else(|error| panic!("revoke collision authority: {error}"));
        let revoked_baseline = authoritative_database_fingerprint(file.path());
        assert!(matches!(
            commit_room_creation(&store, revoked_candidate).resolution(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(
            authoritative_database_fingerprint(file.path()),
            revoked_baseline,
            "authority denial precedes generated Room-ID collision"
        );
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("restore collision authority: {error}"));

        let (candidate_genesis, candidate_request, _, candidate_identity) = creation_fixture(0);
        let candidate = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            candidate_identity,
            &candidate_request,
            witness,
            candidate_genesis,
        )
        .unwrap_or_else(|error| panic!("prepare colliding Room: {error}"));
        let expected_identity = candidate.semantic_result().operation_identity().clone();
        let expected_hash = candidate.semantic_result().canonical_request_hash().clone();
        let baseline = authoritative_database_fingerprint(file.path());
        let collision = commit_room_creation(&store, candidate);
        assert!(matches!(
            collision.resolution(),
            RoomCommitResolutionV1::Reprepare
        ));
        let (_, trace, pending) = collision.into_parts();
        assert!(trace.is_none());
        assert_eq!(authoritative_database_fingerprint(file.path()), baseline);
        let Some(RoomCreationPendingAttemptV1::Reprepare(reprepare)) = pending else {
            panic!("Room-ID collision exposes Create reprepare capability")
        };
        let refreshed_authority = PreparedAuthorityWitnessV1::mint_for_conformance(
            "create-capability-refreshed",
            parsed(PRINCIPAL),
            2,
            &canonical(br#"{"scope":"create_room","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("refreshed Create authority: {error}"));
        store
            .seed_authority(&refreshed_authority, true)
            .unwrap_or_else(|error| panic!("seed refreshed authority: {error}"));
        let (regenerated_genesis, _, _, _) = creation_fixture_with_generated(
            0,
            ROOM_ALT,
            PARTICIPANT_ALT,
            "hex:101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f",
            "2026-08-15T13:00:00Z",
            "ignored-regenerated-identity",
        );
        let regenerated = reprepare
            .reseal_for_conformance(refreshed_authority, regenerated_genesis)
            .unwrap_or_else(|error| panic!("reseal collision: {error}"));
        assert_eq!(
            regenerated.semantic_result().operation_identity(),
            &expected_identity
        );
        assert_eq!(
            regenerated.semantic_result().canonical_request_hash(),
            &expected_hash
        );
        let created = commit_room_creation(&store, regenerated);
        assert!(matches!(
            created.resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        assert_eq!(
            created
                .into_committed_trace()
                .unwrap_or_else(|| panic!("regenerated Create trace"))
                .head()
                .room_id()
                .to_string(),
            ROOM_ALT
        );

        let absent_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("temp absent DB: {error}"));
        let absent_store = SqliteRoomStore::open(absent_file.path())
            .unwrap_or_else(|error| panic!("open absent SQLite: {error}"));
        let (lost_genesis, lost_request, lost_authority, lost_identity) = creation_fixture(0);
        let lost_plan = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            lost_identity.clone(),
            &lost_request,
            lost_authority,
            lost_genesis,
        )
        .unwrap_or_else(|error| panic!("prepare lost Create: {error}"));
        let lost_hash = lost_plan.semantic_result().canonical_request_hash().clone();
        drop(lost_plan);
        drop(absent_store);
        let absent_store = SqliteRoomStore::open(absent_file.path())
            .unwrap_or_else(|error| panic!("reopen absent SQLite: {error}"));
        let lost_operation = OperationIdentityV1::Administration(Box::new(lost_identity.clone()));
        assert!(matches!(
            absent_store.resolve(&lost_operation, &lost_hash),
            ResolveOutcomeV1::KnownAbsent
        ));
        let regenerated_authority = PreparedAuthorityWitnessV1::mint_for_conformance(
            "lost-create-regenerated-authority",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"create_room","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("regenerated absent authority: {error}"));
        absent_store
            .seed_authority(&regenerated_authority, true)
            .unwrap_or_else(|error| panic!("seed regenerated absent authority: {error}"));
        let (regenerated_genesis, _, _, _) = creation_fixture_with_generated(
            0,
            ROOM_ALT,
            PARTICIPANT_ALT,
            "hex:202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
            "2026-08-15T14:00:00Z",
            "ignored-lost-identity",
        );
        let regenerated = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            lost_identity,
            &lost_request,
            regenerated_authority,
            regenerated_genesis,
        )
        .unwrap_or_else(|error| panic!("regenerate guarded-absent Create: {error}"));
        assert_eq!(
            regenerated.semantic_result().canonical_request_hash(),
            &lost_hash
        );
        assert!(matches!(
            commit_room_creation(&absent_store, regenerated).resolution(),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
    }

    #[test]
    fn every_create_write_failpoint_rolls_back_the_entire_bundle() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (prepared, witness, _) = prepared_creation(0);
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed authority: {error}"));
        let reader =
            Connection::open(file.path()).unwrap_or_else(|error| panic!("read SQLite: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());

        for boundary in [
            WriteBoundary::Receipt,
            WriteBoundary::Room,
            WriteBoundary::Genesis,
            WriteBoundary::Materializations,
            WriteBoundary::Timers,
            WriteBoundary::Integrity,
        ] {
            store.set_failpoint(Some(boundary));
            assert!(matches!(
                store.commit(&prepared),
                RoomCommitResolutionV1::RetryableKnownAbsent
            ));
            for table in [
                "semantic_receipts",
                "rooms",
                "room_genesis",
                "room_materializations",
                "room_members",
                "room_integrity",
                "timers",
            ] {
                let count: i64 = reader
                    .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                        row.get(0)
                    })
                    .unwrap_or_else(|error| panic!("count {table}: {error}"));
                assert_eq!(count, 0, "{boundary:?} leaked {table}");
            }
            assert_eq!(
                authoritative_database_fingerprint(file.path()),
                baseline,
                "{boundary:?} changed an authoritative byte"
            );
        }
        store.set_failpoint(None);
        assert!(matches!(
            store.commit(&prepared),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
    }

    #[test]
    fn intra_membership_write_failure_rolls_back_the_first_collection_item() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (prepared, witness) = prepared_two_member_creation();
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed authority: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());
        store.set_failpoint(Some(WriteBoundary::AfterFirstCreateMember));
        assert!(matches!(
            store.commit(&prepared),
            RoomCommitResolutionV1::RetryableKnownAbsent
        ));
        assert_eq!(authoritative_database_fingerprint(file.path()), baseline);
        store.set_failpoint(None);
        assert!(matches!(
            store.commit(&prepared),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let reader = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("read two-member SQLite: {error}"));
        let member_count: i64 = reader
            .query_row("SELECT count(*) FROM room_members", (), |row| row.get(0))
            .unwrap_or_else(|error| panic!("count two-member rows: {error}"));
        assert_eq!(member_count, 2);
    }

    #[test]
    fn actual_read_only_driver_error_rolls_back_and_the_identical_plan_retries() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (prepared, witness, _) = prepared_creation(0);
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed authority: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());
        store
            .set_query_only(true)
            .unwrap_or_else(|error| panic!("enable query_only: {error}"));
        let failed = store.commit(&prepared);
        assert!(
            matches!(failed, RoomCommitResolutionV1::Indeterminate),
            "pre-guard query_only result: {failed:?}"
        );
        store
            .set_query_only(false)
            .unwrap_or_else(|error| panic!("disable query_only: {error}"));
        store.set_failpoint(Some(WriteBoundary::DriverReadOnlyAfterGuard));
        let failed = store.commit(&prepared);
        assert!(
            matches!(failed, RoomCommitResolutionV1::RetryableKnownAbsent),
            "post-guard query_only result: {failed:?}"
        );
        store.set_failpoint(None);
        store
            .set_query_only(false)
            .unwrap_or_else(|error| panic!("clear post-guard query_only: {error}"));

        let reader =
            Connection::open(file.path()).unwrap_or_else(|error| panic!("read SQLite: {error}"));
        for table in ["semantic_receipts", "rooms", "room_genesis"] {
            let count: i64 = reader
                .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("count {table}: {error}"));
            assert_eq!(count, 0, "driver error leaked {table}");
        }
        assert_eq!(authoritative_database_fingerprint(file.path()), baseline);
        assert!(matches!(
            store.commit(&prepared),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
    }

    #[test]
    fn action_advance_is_one_atomic_bundle_and_unknown_commit_resolves_original() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (mut trace, witness) = committed_trace(&store);
        let prepared = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let expected_receipt = prepared
            .semantic_result()
            .canonical_receipt_bytes()
            .to_vec();
        let PreparedExistingIntentV1::Advance(advance) = prepared.intent() else {
            panic!("Counter increment is an Advance")
        };
        let expected_frame = advance.observation_frames.first().map_or_else(
            || panic!("Counter increment prepares one addressed frame"),
            |frame| {
                (
                    frame.member_id().to_string(),
                    i64::try_from(frame.frame_seq()).unwrap_or(-1),
                    i64::try_from(frame.cause_room_seq().get()).unwrap_or(-1),
                    frame.payload_hash().to_string(),
                    frame.canonical_payload_bytes().to_vec(),
                )
            },
        );
        store.set_failpoint(Some(WriteBoundary::AfterCommitUnknown));
        let unknown = commit_existing_room(&store, &mut trace, prepared);
        assert!(
            matches!(unknown.resolution(), RoomCommitResolutionV1::Indeterminate),
            "unknown result: {:?}",
            unknown.resolution()
        );
        assert_eq!(unknown.actor_installation(), ActorInstallationV1::Withheld);
        assert_eq!(
            trace.head().room_seq().get(),
            0,
            "unknown cannot publish H1"
        );
        let (_, _, pending, _) = unknown.into_parts();
        let Some(ExistingRoomPendingAttemptV1::ResolveOnly(resolve)) = pending else {
            panic!("unknown Action COMMIT exposes resolve-only capability")
        };
        store.set_failpoint(None);
        let resolved = resolve.resolve(&store, &mut trace);
        let original = resolved
            .resolution()
            .stored_result()
            .unwrap_or_else(|| panic!("unknown COMMIT must resolve the durable original"));
        assert!(matches!(
            resolved.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved.actor_installation(),
            ActorInstallationV1::ReloadRequired
        );
        assert_eq!(
            trace.head().room_seq().get(),
            0,
            "resolve cannot install H1"
        );
        assert_eq!(original.canonical_receipt_bytes(), expected_receipt);
        assert!(matches!(
            original.result(),
            worldstream_core::SemanticResultV1::TransitionCommitted {
                transition_id,
                ..
            } if transition_id == &parsed::<TransitionId>(TRANSITION_A)
        ));

        store
            .seed_authority(&witness, false)
            .unwrap_or_else(|error| panic!("revoke authority: {error}"));
        let duplicate = prepared_increment(
            &trace,
            witness,
            ACTION_A,
            TRANSITION_B,
            "2026-08-15T12:00:01Z",
            0,
        );
        let duplicate = commit_existing_room(&store, &mut trace, duplicate);
        assert!(matches!(
            duplicate.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));

        let reader = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("read committed DB: {error}"));
        for (table, expected) in [
            ("rooms", 1_i64),
            ("transitions", 1),
            ("room_materializations", 1),
            ("room_members", 1),
            ("observation_frames", 1),
            ("semantic_receipts", 2),
        ] {
            let count: i64 = reader
                .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("count {table}: {error}"));
            assert_eq!(count, expected, "{table}");
        }
        let (room_seq, frame_head): (i64, i64) = reader
            .query_row(
                "SELECT rooms.room_seq, room_members.frame_head FROM rooms \
                 JOIN room_members USING(room_id) WHERE rooms.room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("advanced heads: {error}"));
        assert_eq!((room_seq, frame_head), (1, 1));
        let stored_frame: (String, i64, i64, String, Vec<u8>) = reader
            .query_row(
                "SELECT member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes \
                 FROM observation_frames WHERE room_id = ?1",
                [ROOM],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap_or_else(|error| panic!("stored Counter frame: {error}"));
        assert_eq!(stored_frame, expected_frame);
    }

    #[test]
    fn every_existing_write_failpoint_rolls_back_to_the_exact_genesis_bundle() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (trace, witness) = committed_trace(&store);
        let prepared = prepared_increment(
            &trace,
            witness,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let write: PreparedRoomWriteV1 = prepared.into();
        let reader =
            Connection::open(file.path()).unwrap_or_else(|error| panic!("read SQLite: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());

        for boundary in [
            WriteBoundary::Transition,
            WriteBoundary::Head,
            WriteBoundary::ExistingMaterializations,
            WriteBoundary::Members,
            WriteBoundary::ExistingTimers,
            WriteBoundary::Frames,
            WriteBoundary::Activation,
            WriteBoundary::ExistingReceipt,
        ] {
            store.set_failpoint(Some(boundary));
            assert!(matches!(
                store.commit(&write),
                RoomCommitResolutionV1::RetryableKnownAbsent
            ));
            let room_seq: i64 = reader
                .query_row(
                    "SELECT room_seq FROM rooms WHERE room_id = ?1",
                    [ROOM],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("Genesis Head after {boundary:?}: {error}"));
            assert_eq!(room_seq, 0, "{boundary:?} moved Head");
            for (table, expected) in [
                ("transitions", 0_i64),
                ("observation_frames", 0),
                ("activation_decisions", 0),
                ("semantic_receipts", 1),
            ] {
                let count: i64 = reader
                    .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                        row.get(0)
                    })
                    .unwrap_or_else(|error| panic!("count {table}: {error}"));
                assert_eq!(count, expected, "{boundary:?} leaked {table}");
            }
            assert_eq!(
                authoritative_database_fingerprint(file.path()),
                baseline,
                "{boundary:?} changed an authoritative byte"
            );
        }
        store.set_failpoint(None);
        let committed = store.commit(&write);
        assert!(
            matches!(
                committed,
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::New,
                    ..
                }
            ),
            "commit result: {committed:?}"
        );
    }

    #[test]
    fn two_candidates_share_one_head_then_loser_records_stable_stale_disposition() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (mut trace, witness) = committed_trace(&store);
        let winner = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let loser = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:02Z",
            0,
        );
        let original_hash = loser.semantic_result().canonical_request_hash().clone();
        let winner = commit_existing_room(&store, &mut trace, winner);
        assert!(
            matches!(
                winner.resolution(),
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::New,
                    ..
                }
            ),
            "winner result: {:?}",
            winner.resolution()
        );
        assert_eq!(winner.actor_installation(), ActorInstallationV1::Installed);
        assert_eq!(trace.head().room_seq().get(), 1);

        let loser = commit_existing_room(&store, &mut trace, loser);
        assert!(matches!(
            loser.resolution(),
            RoomCommitResolutionV1::Reprepare
        ));
        let (_, _, _, reprepare) = loser.into_parts();
        let Some(ExistingRoomReprepareV1::ParticipantAction(reprepare)) = reprepare else {
            panic!("losing Action retains immutable request and admission time")
        };
        let stale = reprepare
            .seal_stable_disposition_for_conformance(
                &trace,
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("integrity generation: {error}")),
                witness,
            )
            .unwrap_or_else(|error| panic!("seal stable stale disposition: {error}"));
        assert_eq!(
            stale.semantic_result().canonical_request_hash(),
            &original_hash
        );
        let stale = commit_existing_room(&store, &mut trace, stale);
        assert!(matches!(
            stale.resolution(),
            RoomCommitResolutionV1::RejectionRecorded {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        assert_eq!(stale.actor_installation(), ActorInstallationV1::Unchanged);
        assert_eq!(trace.head().room_seq().get(), 1);

        let reader =
            Connection::open(file.path()).unwrap_or_else(|error| panic!("read SQLite: {error}"));
        let (transitions, receipts): (i64, i64) = (
            reader
                .query_row("SELECT count(*) FROM transitions", (), |row| row.get(0))
                .unwrap_or_else(|error| panic!("transition count: {error}")),
            reader
                .query_row("SELECT count(*) FROM semantic_receipts", (), |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("receipt count: {error}")),
        );
        assert_eq!((transitions, receipts), (1, 3));
    }

    #[test]
    fn real_contention_serializes_duplicates_head_candidates_and_inflight_resolve() {
        let duplicate_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("temp duplicate DB: {error}"));
        let duplicate_store = SqliteRoomStore::open(duplicate_file.path())
            .unwrap_or_else(|error| panic!("open duplicate SQLite: {error}"));
        let (first, witness, _) = prepared_creation(0);
        let (second, _, _) = prepared_creation(0);
        duplicate_store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed duplicate authority: {error}"));
        let start = Arc::new(Barrier::new(3));
        let first_thread = {
            let store = duplicate_store.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                store.commit(&first)
            })
        };
        let second_thread = {
            let store = duplicate_store.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                store.commit(&second)
            })
        };
        start.wait();
        let first = first_thread
            .join()
            .unwrap_or_else(|_| panic!("first duplicate commit thread"));
        let second = second_thread
            .join()
            .unwrap_or_else(|_| panic!("second duplicate commit thread"));
        let statuses = [&first, &second]
            .into_iter()
            .map(|resolution| match resolution {
                RoomCommitResolutionV1::GenesisCreated { status, .. } => *status,
                other => panic!("duplicate contention result: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == ResolutionStatusV1::New)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == ResolutionStatusV1::Existing)
                .count(),
            1
        );
        assert_eq!(
            first
                .stored_result()
                .unwrap_or_else(|| panic!("first duplicate result"))
                .canonical_receipt_bytes(),
            second
                .stored_result()
                .unwrap_or_else(|| panic!("second duplicate result"))
                .canonical_receipt_bytes()
        );

        let head_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("temp Head DB: {error}"));
        let head_store = SqliteRoomStore::open(head_file.path())
            .unwrap_or_else(|error| panic!("open Head SQLite: {error}"));
        let (trace, witness) = committed_trace(&head_store);
        let first: PreparedRoomWriteV1 = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        )
        .into();
        let second: PreparedRoomWriteV1 = prepared_increment(
            &trace,
            witness,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:02Z",
            0,
        )
        .into();
        let start = Arc::new(Barrier::new(3));
        let first_thread = {
            let store = head_store.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                store.commit(&first)
            })
        };
        let second_thread = {
            let store = head_store.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                store.commit(&second)
            })
        };
        start.wait();
        let results = [
            first_thread
                .join()
                .unwrap_or_else(|_| panic!("first Head commit thread")),
            second_thread
                .join()
                .unwrap_or_else(|_| panic!("second Head commit thread")),
        ];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    RoomCommitResolutionV1::TransitionCommitted {
                        status: ResolutionStatusV1::New,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, RoomCommitResolutionV1::Reprepare))
                .count(),
            1
        );

        let resolve_file =
            NamedTempFile::new().unwrap_or_else(|error| panic!("temp resolve DB: {error}"));
        let resolve_store = SqliteRoomStore::open(resolve_file.path())
            .unwrap_or_else(|error| panic!("open resolve SQLite: {error}"));
        let (write, witness, _) = prepared_creation(0);
        let identity = write.identity().clone();
        let hash = write.request_hash().clone();
        resolve_store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed resolve authority: {error}"));
        arm_guarded_commit_pause();
        resolve_store.set_failpoint(Some(WriteBoundary::PauseAfterGuard));
        let commit_thread = {
            let store = resolve_store.clone();
            thread::spawn(move || store.commit(&write))
        };
        wait_until_guarded_commit_pauses();
        let (resolved_send, resolved_receive) = mpsc::channel();
        let resolve_thread = {
            let store = resolve_store.clone();
            thread::spawn(move || {
                let resolved = store.resolve(&identity, &hash);
                let _ = resolved_send.send(resolved);
            })
        };
        assert!(
            resolved_receive
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "resolve must wait behind the in-flight guarded commit"
        );
        release_guarded_commit();
        let committed = commit_thread
            .join()
            .unwrap_or_else(|_| panic!("guarded commit thread"));
        assert!(matches!(
            committed,
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let resolved = resolved_receive
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("guarded resolve result: {error}"));
        assert!(matches!(resolved, ResolveOutcomeV1::StoredResolution(_)));
        resolve_thread
            .join()
            .unwrap_or_else(|_| panic!("guarded resolve thread"));
        resolve_store.set_failpoint(None);
    }

    #[test]
    fn timer_advance_unknown_commit_duplicate_and_failpoint_rollbacks_are_exact() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let mut trace = timer_trace();
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "timer-capability-fixture",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"timer_fired","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("Timer authority: {error}"));
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed Timer authority: {error}"));
        seed_timer_room(file.path(), &trace);

        let retry_plan = prepared_timer(&trace, witness.clone(), TIMER_TRANSITION);
        let retry_write: PreparedRoomWriteV1 = retry_plan.into();
        let reader = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer reader: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());
        for boundary in [
            WriteBoundary::Transition,
            WriteBoundary::Head,
            WriteBoundary::ExistingMaterializations,
            WriteBoundary::Members,
            WriteBoundary::ExistingTimers,
            WriteBoundary::Frames,
            WriteBoundary::Activation,
            WriteBoundary::ExistingReceipt,
        ] {
            store.set_failpoint(Some(boundary));
            let failed = store.commit(&retry_write);
            assert!(
                matches!(failed, RoomCommitResolutionV1::RetryableKnownAbsent),
                "Timer boundary {boundary:?}: {failed:?}"
            );
            let state: String = reader
                .query_row(
                    "SELECT state FROM timers WHERE room_id = ?1 AND timer_id = ?2 AND generation = 1",
                    params![ROOM, TIMER],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("Timer state after {boundary:?}: {error}"));
            assert_eq!(state, "scheduled", "{boundary:?} consumed Timer");
            for table in ["transitions", "semantic_receipts"] {
                let count: i64 = reader
                    .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                        row.get(0)
                    })
                    .unwrap_or_else(|error| panic!("{table} after {boundary:?}: {error}"));
                assert_eq!(count, 0, "{boundary:?} leaked {table}");
            }
            assert_eq!(
                authoritative_database_fingerprint(file.path()),
                baseline,
                "Timer boundary {boundary:?} changed an authoritative byte"
            );
        }

        store.set_failpoint(Some(WriteBoundary::AfterCommitUnknown));
        let plan = prepared_timer(&trace, witness.clone(), TIMER_TRANSITION);
        let expected_receipt = plan.semantic_result().canonical_receipt_bytes().to_vec();
        let outcome = commit_existing_room(&store, &mut trace, plan);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::Indeterminate
        ));
        assert_eq!(outcome.actor_installation(), ActorInstallationV1::Withheld);
        let (_, _, pending, _) = outcome.into_parts();
        let Some(ExistingRoomPendingAttemptV1::ResolveOnly(resolve)) = pending else {
            panic!("unknown Timer COMMIT retains resolve-only capability")
        };
        store.set_failpoint(None);
        let resolved = resolve.resolve(&store, &mut trace);
        assert!(matches!(
            resolved.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved
                .resolution()
                .stored_result()
                .unwrap_or_else(|| panic!("resolved Timer receipt"))
                .canonical_receipt_bytes(),
            expected_receipt
        );
        assert_eq!(
            resolved.actor_installation(),
            ActorInstallationV1::ReloadRequired
        );
        assert_eq!(trace.head().room_seq().get(), 0, "unknown does not publish");

        let duplicate = prepared_timer(&trace, witness.clone(), "01ARZ3NDEKTSV4RRFFQ69G5FG2");
        store
            .seed_authority(&witness, false)
            .unwrap_or_else(|error| panic!("revoke Timer authority: {error}"));
        let duplicate = commit_existing_room(&store, &mut trace, duplicate);
        assert!(matches!(
            duplicate.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            duplicate
                .resolution()
                .stored_result()
                .unwrap_or_else(|| panic!("duplicate Timer receipt"))
                .canonical_receipt_bytes(),
            expected_receipt
        );
        let (state, transitions, receipts): (String, i64, i64) = (
            reader
                .query_row(
                    "SELECT state FROM timers WHERE room_id = ?1 AND timer_id = ?2 AND generation = 1",
                    params![ROOM, TIMER],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("durable Timer state: {error}")),
            reader
                .query_row("SELECT count(*) FROM transitions", (), |row| row.get(0))
                .unwrap_or_else(|error| panic!("Timer Transition count: {error}")),
            reader
                .query_row("SELECT count(*) FROM semantic_receipts", (), |row| row.get(0))
                .unwrap_or_else(|error| panic!("Timer receipt count: {error}")),
        );
        assert_eq!((state.as_str(), transitions, receipts), ("fired", 1, 1));
    }

    #[test]
    fn timer_obsolescence_head_precedence_and_corruption_classify_without_receipts() {
        for obsolete_state in [None, Some("fired"), Some("cancelled")] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open SQLite: {error}"));
            let mut trace = timer_trace();
            let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
                "timer-obsolete-capability",
                parsed(PRINCIPAL),
                1,
                &canonical(br#"{"scope":"timer_fired","revoked":false}"#),
            )
            .unwrap_or_else(|error| panic!("Timer authority: {error}"));
            store
                .seed_authority(&witness, true)
                .unwrap_or_else(|error| panic!("seed Timer authority: {error}"));
            seed_timer_room(file.path(), &trace);
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open Timer mutator: {error}"));
            match obsolete_state {
                None => {
                    connection
                        .execute(
                            "DELETE FROM timers WHERE room_id = ?1 AND timer_id = ?2",
                            params![ROOM, TIMER],
                        )
                        .unwrap_or_else(|error| panic!("delete obsolete Timer: {error}"));
                }
                Some(state) => {
                    connection
                        .execute(
                            "UPDATE timers SET state = ?1 WHERE room_id = ?2 AND timer_id = ?3",
                            params![state, ROOM, TIMER],
                        )
                        .unwrap_or_else(|error| panic!("mark obsolete Timer: {error}"));
                }
            }
            let plan = prepared_timer(&trace, witness, TIMER_TRANSITION);
            let outcome = commit_existing_room(&store, &mut trace, plan);
            assert!(matches!(
                outcome.resolution(),
                RoomCommitResolutionV1::NotApplicable
            ));
            assert_eq!(outcome.actor_installation(), ActorInstallationV1::Unchanged);
            for table in ["transitions", "semantic_receipts"] {
                let count: i64 = connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                        row.get(0)
                    })
                    .unwrap_or_else(|error| panic!("{table} after NotApplicable: {error}"));
                assert_eq!(count, 0, "NotApplicable wrote {table}");
            }
        }

        for changed_field in ["scheduled_for", "payload_bytes"] {
            let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
            let store = SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("open SQLite: {error}"));
            let mut trace = timer_trace();
            let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
                format!("timer-corrupt-{changed_field}"),
                parsed(PRINCIPAL),
                1,
                &canonical(br#"{"scope":"timer_fired","revoked":false}"#),
            )
            .unwrap_or_else(|error| panic!("Timer authority: {error}"));
            store
                .seed_authority(&witness, true)
                .unwrap_or_else(|error| panic!("seed Timer authority: {error}"));
            seed_timer_room(file.path(), &trace);
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open Timer corruptor: {error}"));
            match changed_field {
                "scheduled_for" => {
                    connection
                        .execute(
                            "UPDATE timers SET scheduled_for = '2026-08-15T12:31:00Z' \
                             WHERE room_id = ?1 AND timer_id = ?2",
                            params![ROOM, TIMER],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Timer schedule: {error}"));
                }
                "payload_bytes" => {
                    connection
                        .execute(
                            "UPDATE timers SET payload_bytes = ?1 \
                             WHERE room_id = ?2 AND timer_id = ?3",
                            params![br#"{"kind":"other"}"#.as_slice(), ROOM, TIMER],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Timer payload: {error}"));
                }
                _ => unreachable!("closed Timer corruption set"),
            }
            let baseline = authoritative_database_fingerprint(file.path());
            let plan = prepared_timer(&trace, witness, TIMER_TRANSITION);
            let outcome = commit_existing_room(&store, &mut trace, plan);
            assert!(matches!(
                outcome.resolution(),
                RoomCommitResolutionV1::Fault
            ));
            assert_eq!(
                outcome.actor_installation(),
                ActorInstallationV1::QuarantineRequired
            );
            assert_eq!(authoritative_database_fingerprint(file.path()), baseline);
            for table in ["transitions", "semantic_receipts"] {
                let count: i64 = connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), (), |row| {
                        row.get(0)
                    })
                    .unwrap_or_else(|error| panic!("{table} after Timer Fault: {error}"));
                assert_eq!(count, 0);
            }
        }

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let mut trace = timer_trace();
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "timer-precedence-capability",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"timer_fired","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("Timer authority: {error}"));
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed Timer authority: {error}"));
        seed_timer_room(file.path(), &trace);
        let stale_plan = prepared_timer(&trace, witness.clone(), TIMER_TRANSITION);
        let advancing_plan = prepared_timer_candidate(
            &trace,
            witness.clone(),
            TIMER_TRANSITION_ALT,
            TIMER_ALT,
            "2026-08-15T12:45:00Z",
            br#"{"kind":"secondary"}"#,
        );
        let advanced = commit_existing_room(&store, &mut trace, advancing_plan);
        assert!(matches!(
            advanced.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer mutator: {error}"));
        connection
            .execute(
                "DELETE FROM timers WHERE room_id = ?1 AND timer_id = ?2",
                [ROOM, TIMER],
            )
            .unwrap_or_else(|error| panic!("remove Timer after Head advance: {error}"));
        store
            .seed_authority(&witness, false)
            .unwrap_or_else(|error| panic!("revoke after Head advance: {error}"));
        let outcome = commit_existing_room(&store, &mut trace, stale_plan);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::Reprepare
        ));
        assert_eq!(outcome.actor_installation(), ActorInstallationV1::Unchanged);
        assert!(matches!(
            outcome.into_parts().3,
            Some(ExistingRoomReprepareV1::TimerFired(_))
        ));
        let receipt_count: i64 = connection
            .query_row("SELECT count(*) FROM semantic_receipts", (), |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("receipt count after Reprepare: {error}"));
        assert_eq!(receipt_count, 1);

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let mut trace = timer_trace();
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "timer-torn-head-capability",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"timer_fired","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("Timer authority: {error}"));
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed Timer authority: {error}"));
        seed_timer_room(file.path(), &trace);
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open torn-Head mutator: {error}"));
        connection
            .execute("UPDATE rooms SET room_seq = 1 WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("tear durable Head fixture: {error}"));
        let baseline = authoritative_database_fingerprint(file.path());
        let plan = prepared_timer(&trace, witness, TIMER_TRANSITION);
        let outcome = commit_existing_room(&store, &mut trace, plan);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::Fault
        ));
        assert_eq!(
            outcome.actor_installation(),
            ActorInstallationV1::QuarantineRequired
        );
        assert_eq!(authoritative_database_fingerprint(file.path()), baseline);
    }

    #[test]
    fn drop_reopen_resolves_exact_receipts_and_replays_durable_history_without_snapshots() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (genesis, request, witness, identity) = creation_fixture(0);
        let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity,
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("prepare restart Genesis: {error}"));
        let create_identity = prepared.semantic_result().operation_identity().clone();
        let create_hash = prepared.semantic_result().canonical_request_hash().clone();
        let create_receipt = prepared
            .semantic_result()
            .canonical_receipt_bytes()
            .to_vec();
        store
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("seed restart authority: {error}"));
        let creation = commit_room_creation(&store, prepared);
        let mut trace = creation
            .into_committed_trace()
            .unwrap_or_else(|| panic!("new durable Genesis releases trace"));
        let action = prepared_increment(
            &trace,
            witness.clone(),
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let action_identity = action.semantic_result().operation_identity().clone();
        let action_hash = action.semantic_result().canonical_request_hash().clone();
        let action_receipt = action.semantic_result().canonical_receipt_bytes().to_vec();
        let action = commit_existing_room(&store, &mut trace, action);
        assert!(matches!(
            action.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        assert_eq!(action.actor_installation(), ActorInstallationV1::Installed);
        let expected_head = trace.head().clone();

        drop(trace);
        drop(store);

        let reopened = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("deterministic restart: {error}"));
        for (identity, hash, expected_bytes) in [
            (&create_identity, &create_hash, create_receipt.as_slice()),
            (&action_identity, &action_hash, action_receipt.as_slice()),
        ] {
            let ResolveOutcomeV1::StoredResolution(result) = reopened.resolve(identity, hash)
            else {
                panic!("restart must resolve original durable receipt")
            };
            assert_eq!(result.canonical_receipt_bytes(), expected_bytes);
        }
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Counter registry for restart: {error}"));
        let recovered = recover_room_from_storage(&reopened, &registry, &parsed(ROOM))
            .unwrap_or_else(|error| panic!("guarded recovery: {error}"))
            .unwrap_or_else(|| panic!("durable Room remains present"));
        assert_eq!(recovered.head(), &expected_head);
        let reader = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("inspect schema after restart: {error}"));
        reader
            .execute(
                "DELETE FROM room_materializations WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("remove disposable materialization: {error}"));
        reader
            .pragma_update(None, "foreign_keys", false)
            .unwrap_or_else(|error| panic!("disable fixture foreign keys: {error}"));
        reader
            .execute("DELETE FROM room_members WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("remove disposable Membership projection: {error}"));
        let mut recovered = recover_room_from_storage(&reopened, &registry, &parsed(ROOM))
            .unwrap_or_else(|error| panic!("rebuild missing materialization: {error}"))
            .unwrap_or_else(|| panic!("durable Room remains present"));
        let rebuilt: (Vec<u8>, Vec<u8>) = reader
            .query_row(
                "SELECT core_state_bytes, activity_state_bytes \
                 FROM room_materializations WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read rebuilt materialization: {error}"));
        assert_eq!(
            rebuilt.0,
            recovered
                .core_state()
                .canonical_bytes()
                .unwrap_or_else(|error| panic!("recovered Core bytes: {error}"))
        );
        assert_eq!(
            rebuilt.1,
            recovered
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| panic!("recovered Activity bytes: {error}"))
        );
        let rebuilt_member_count: i64 = reader
            .query_row(
                "SELECT count(*) FROM room_members WHERE room_id = ?1",
                [ROOM],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("read rebuilt Membership projection: {error}"));
        assert_eq!(rebuilt_member_count, 1);
        reopened
            .seed_authority(&witness, true)
            .unwrap_or_else(|error| panic!("restore conformance authority after reopen: {error}"));
        let next = prepared_increment(
            &recovered,
            witness,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:02Z",
            1,
        );
        let next = commit_existing_room(&reopened, &mut recovered, next);
        assert!(matches!(
            next.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        assert_eq!(recovered.head().room_seq().get(), 2);
        let snapshot_objects: i64 = reader
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE lower(name) LIKE '%snapshot%'",
                (),
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("snapshot schema check: {error}"));
        assert_eq!(snapshot_objects, 0);
    }

    #[test]
    fn recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace() {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (_trace, _witness) = committed_trace(&store);
        arm_recovery_install_pause(&store.writer.path);
        let recovery_store = store.clone();
        let recovery = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Counter recovery registry: {error}"));
            recover_room_from_storage(&recovery_store, &registry, &parsed(ROOM))
        });
        wait_until_recovery_install_pauses();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open integrity-race mutator: {error}"));
        connection
            .execute(
                "UPDATE room_integrity SET generation = 2 WHERE room_id = ?1",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("advance integrity generation: {error}"));
        release_recovery_install();
        let result = recovery
            .join()
            .unwrap_or_else(|_| panic!("integrity-race recovery thread"));
        assert!(matches!(result, Err(RoomRecoveryErrorV1::ConcurrentChange)));

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open SQLite: {error}"));
        let (mut trace, witness) = committed_trace(&store);
        arm_recovery_install_pause(&store.writer.path);
        let recovery_store = store.clone();
        let recovery = thread::spawn(move || {
            let registry = builtin_counter_registry()
                .unwrap_or_else(|error| panic!("Counter recovery registry: {error}"));
            recover_room_from_storage(&recovery_store, &registry, &parsed(ROOM))
        });
        wait_until_recovery_install_pauses();
        let action = prepared_increment(
            &trace,
            witness,
            ACTION_A,
            TRANSITION_A,
            "2026-08-15T12:00:01Z",
            0,
        );
        let outcome = commit_existing_room(&store, &mut trace, action);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));
        release_recovery_install();
        let result = recovery
            .join()
            .unwrap_or_else(|_| panic!("Head-race recovery thread"));
        assert!(matches!(result, Err(RoomRecoveryErrorV1::ConcurrentChange)));
    }

    #[test]
    fn missing_integrity_row_is_conditionally_recreated_as_quarantined() {
        let (file, store, _trace, _witness) = committed_history_fixture();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open missing-integrity fixture: {error}"));
        connection
            .execute("DELETE FROM room_integrity WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("delete integrity row: {error}"));
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Counter recovery registry: {error}"));
        assert!(matches!(
            recover_room_from_storage(&store, &registry, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
        let recreated: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read recreated integrity row: {error}"));
        assert_eq!(recreated, ("quarantined".to_owned(), 1));

        let (file, store, _trace, _witness) = committed_history_fixture();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open missing-integrity race fixture: {error}"));
        connection
            .execute("DELETE FROM room_integrity WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("delete raced integrity row: {error}"));
        let missing = super::capture_observed_recovery_fence(&store.writer.path, &parsed(ROOM))
            .unwrap_or_else(|error| panic!("capture missing-integrity fence: {error}"))
            .unwrap_or_else(|| panic!("Room fence remains observable"));
        connection
            .execute(
                "INSERT INTO room_integrity(room_id, status, generation) \
                 VALUES (?1, 'healthy', 2)",
                [ROOM],
            )
            .unwrap_or_else(|error| panic!("concurrent integrity repair: {error}"));
        let mut writer = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open conditional quarantine writer: {error}"));
        assert!(matches!(
            super::quarantine_observed_recovery_corruption(&mut writer, &missing),
            Err(RoomRecoveryErrorV1::ConcurrentChange)
        ));
        let preserved: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read concurrent repair: {error}"));
        assert_eq!(preserved, ("healthy".to_owned(), 2));
    }

    #[test]
    fn replay_verification_rejects_each_corrupt_or_missing_durable_history_component() {
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Counter registry for corruption tests: {error}"));
        for corruption in [
            "activity_materialization",
            "pack_revision_lock",
            "genesis",
            "transition",
            "missing_transition",
        ] {
            let (file, store, mut trace, witness) = committed_history_fixture();
            let fenced_candidate = prepared_increment(
                &trace,
                witness,
                ACTION_B,
                TRANSITION_B,
                "2026-08-15T12:00:02Z",
                1,
            );
            let connection = Connection::open(file.path())
                .unwrap_or_else(|error| panic!("open corruption fixture: {error}"));
            match corruption {
                "activity_materialization" => {
                    connection
                        .execute(
                            "UPDATE room_materializations SET activity_state_bytes = ?1 \
                             WHERE room_id = ?2",
                            params![br#"{}"#.as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| {
                            panic!("corrupt Activity materialization: {error}")
                        });
                }
                "pack_revision_lock" => {
                    connection
                        .execute_batch("DROP TRIGGER room_genesis_immutable_update")
                        .unwrap_or_else(|error| panic!("drop Genesis update trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE room_genesis SET pack_revision_lock_bytes = ?1 \
                             WHERE room_id = ?2",
                            params![br#"{}"#.as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Pack lock: {error}"));
                }
                "genesis" => {
                    connection
                        .execute_batch("DROP TRIGGER room_genesis_immutable_update")
                        .unwrap_or_else(|error| panic!("drop Genesis update trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE room_genesis SET genesis_bytes = ?1 WHERE room_id = ?2",
                            params![br#"{}"#.as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Genesis bytes: {error}"));
                }
                "transition" => {
                    connection
                        .execute_batch("DROP TRIGGER transitions_immutable_update")
                        .unwrap_or_else(|error| panic!("drop Transition update trigger: {error}"));
                    connection
                        .execute(
                            "UPDATE transitions SET transition_bytes = ?1 WHERE room_id = ?2",
                            params![br#"{}"#.as_slice(), ROOM],
                        )
                        .unwrap_or_else(|error| panic!("corrupt Transition bytes: {error}"));
                }
                "missing_transition" => {
                    connection
                        .execute_batch(
                            "DROP TRIGGER transitions_immutable_delete; \
                             DROP TRIGGER semantic_receipts_immutable_delete; \
                             DELETE FROM semantic_receipts WHERE room_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV' \
                                 AND transition_seq = 1; \
                             DELETE FROM observation_frames WHERE room_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV'; \
                             DELETE FROM activation_decisions WHERE room_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV';",
                        )
                        .unwrap_or_else(|error| panic!("drop Transition delete trigger: {error}"));
                    connection
                        .execute("DELETE FROM transitions WHERE room_id = ?1", [ROOM])
                        .unwrap_or_else(|error| panic!("delete Transition: {error}"));
                }
                _ => unreachable!("closed corruption fixture set"),
            }
            assert!(
                matches!(
                    recover_room_from_storage(&store, &registry, &parsed(ROOM)),
                    Err(RoomRecoveryErrorV1::Corrupt)
                ),
                "{corruption} passed guarded recovery"
            );
            let integrity: (String, i64) = connection
                .query_row(
                    "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                    [ROOM],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("read quarantined integrity: {error}"));
            assert_eq!(integrity, ("quarantined".to_owned(), 2));
            let fenced = commit_existing_room(&store, &mut trace, fenced_candidate);
            assert!(matches!(
                fenced.resolution(),
                RoomCommitResolutionV1::Fenced
            ));
            assert_eq!(fenced.actor_installation(), ActorInstallationV1::Unchanged);
        }
    }

    #[test]
    fn unavailable_retained_runtime_faults_but_does_not_quarantine_intact_history() {
        let (file, store, mut trace, witness) = committed_history_fixture();
        let candidate = prepared_increment(
            &trace,
            witness,
            ACTION_B,
            TRANSITION_B,
            "2026-08-15T12:00:02Z",
            1,
        );
        let missing_runtime = counter_v1_only_registry_for_conformance()
            .unwrap_or_else(|error| panic!("v1-only registry: {error}"));
        assert!(matches!(
            recover_room_from_storage(&store, &missing_runtime, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::RuntimeUnavailable)
        ));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open runtime-fault fixture: {error}"));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read runtime-fault integrity: {error}"));
        assert_eq!(integrity, ("faulted".to_owned(), 2));
        let fenced = commit_existing_room(&store, &mut trace, candidate);
        assert!(matches!(
            fenced.resolution(),
            RoomCommitResolutionV1::Fenced
        ));
        let complete_registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("complete Counter registry: {error}"));
        assert!(matches!(
            recover_room_from_storage(&store, &complete_registry, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::IntegrityUnavailable)
        ));

        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute_batch("DROP TRIGGER room_genesis_immutable_update")
                .unwrap_or_else(|error| panic!("drop Genesis update trigger: {error}"));
            connection
                .execute(
                    "UPDATE room_genesis SET pack_revision_lock_bytes = ?1 WHERE room_id = ?2",
                    params![br#"{}"#.as_slice(), ROOM],
                )
                .unwrap_or_else(|error| panic!("replace retained Pack lock: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute("UPDATE rooms SET room_seq = 0 WHERE room_id = ?1", [ROOM])
                .unwrap_or_else(|error| panic!("tear captured top Head: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute(
                    "UPDATE room_materializations SET activity_state_bytes = ?1 \
                     WHERE room_id = ?2",
                    params![br"{}".as_slice(), ROOM],
                )
                .unwrap_or_else(|error| panic!("replace current Activity projection: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute_batch(
                    "DELETE FROM room_materializations \
                     WHERE room_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV'; \
                     UPDATE rooms SET room_status = 'archived' \
                     WHERE room_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV';",
                )
                .unwrap_or_else(|error| {
                    panic!("tear Room status without materialization: {error}")
                });
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute(
                    "UPDATE room_members SET standing = 'suspended' \
                     WHERE room_id = ?1 AND member_id = ?2",
                    params![ROOM, PARTICIPANT],
                )
                .unwrap_or_else(|error| panic!("tear Membership projection: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute(
                    "INSERT INTO timers(\
                     room_id, timer_id, generation, scheduled_for, payload_bytes, state\
                     ) VALUES (?1, ?2, 1, ?3, ?4, 'scheduled')",
                    params![ROOM, TIMER, "2026-08-15T12:01:00Z", br"{}".as_slice()],
                )
                .unwrap_or_else(|error| panic!("insert false Timer projection: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute(
                    "UPDATE observation_frames SET payload_bytes = ?1 WHERE room_id = ?2",
                    params![br"{}".as_slice(), ROOM],
                )
                .unwrap_or_else(|error| panic!("tear observation Frame payload: {error}"));
        });
        assert_missing_runtime_does_not_mask_corruption(|connection| {
            connection
                .execute(
                    "INSERT INTO activation_decisions(\
                     room_id, cause_room_seq, decision_id, target_member_id, decision_bytes\
                     ) VALUES (?1, 1, 'unexpected-pre-runtime', NULL, ?2)",
                    params![ROOM, br#"{}"#.as_slice()],
                )
                .unwrap_or_else(|error| panic!("insert pre-runtime Activation: {error}"));
        });

        let (file, store, trace, _witness) = committed_history_fixture();
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open fault-record race fixture: {error}"));
        connection
            .execute("UPDATE rooms SET room_seq = 0 WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("corrupt Head before fault record: {error}"));
        assert!(matches!(
            RoomRecoveryStorageV1::record_recovery_failure(
                &store,
                &parsed(ROOM),
                trace.head(),
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("integrity generation: {error}")),
                RecoveryIntegrityDispositionV1::Faulted,
            ),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
        let integrity: (String, i64) = connection
            .query_row(
                "SELECT status, generation FROM room_integrity WHERE room_id = ?1",
                [ROOM],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or_else(|error| panic!("read fault-record race integrity: {error}"));
        assert_eq!(integrity, ("quarantined".to_owned(), 2));
    }

    #[test]
    fn retained_runtime_panics_and_malformed_outputs_fault_but_semantic_mismatch_quarantines() {
        for operation in [
            ActivityPackOperationV1::Initialize,
            ActivityPackOperationV1::Reduce,
            ActivityPackOperationV1::View,
            ActivityPackOperationV1::Observe,
        ] {
            let panic_registry = counter_v2_runtime_fault_registry_for_conformance(operation)
                .unwrap_or_else(|error| panic!("runtime panic registry: {error}"));
            assert_runtime_recovery_faults(&panic_registry);

            let malformed_registry =
                counter_v2_malformed_output_registry_for_conformance(operation)
                    .unwrap_or_else(|error| panic!("malformed output registry: {error}"));
            assert_runtime_recovery_faults(&malformed_registry);

            let returned_fault_registry =
                counter_v2_returned_fault_registry_for_conformance(operation)
                    .unwrap_or_else(|error| panic!("returned fault registry: {error}"));
            assert_runtime_recovery_faults(&returned_fault_registry);
        }

        let mismatch_registry = counter_v2_semantic_mismatch_registry_for_conformance()
            .unwrap_or_else(|error| panic!("semantic mismatch registry: {error}"));
        assert_semantic_recovery_mismatch_quarantines(&mismatch_registry);

        let invalid_timer_registry = counter_v2_invalid_timer_output_registry_for_conformance()
            .unwrap_or_else(|error| panic!("invalid Timer output registry: {error}"));
        assert_runtime_recovery_faults(&invalid_timer_registry);
    }

    #[test]
    fn recovery_quarantines_corrupt_consequences_transition_indexes_and_receipt_projections() {
        assert_recovery_corruption_quarantines(|connection| {
            connection
                .pragma_update(None, "foreign_keys", false)
                .unwrap_or_else(|error| panic!("disable fixture foreign keys: {error}"));
            connection
                .execute(
                    "UPDATE room_members SET standing = 'suspended' \
                     WHERE room_id = ?1 AND member_id = ?2",
                    params![ROOM, PARTICIPANT],
                )
                .unwrap_or_else(|error| panic!("corrupt Membership standing: {error}"));
        });
        assert_recovery_corruption_quarantines(|connection| {
            connection
                .execute(
                    "UPDATE room_members SET membership_bytes = ?1 \
                     WHERE room_id = ?2 AND member_id = ?3",
                    params![br#"{}"#.as_slice(), ROOM, PARTICIPANT],
                )
                .unwrap_or_else(|error| panic!("corrupt Membership bytes: {error}"));
        });
        assert_recovery_corruption_quarantines(|connection| {
            let replacement = br#"{}"#;
            connection
                .execute(
                    "UPDATE observation_frames SET payload_bytes = ?1, payload_hash = ?2 \
                     WHERE room_id = ?3 AND member_id = ?4",
                    params![
                        replacement.as_slice(),
                        worldstream_core::Blake3DigestV1::hash(replacement).to_string(),
                        ROOM,
                        PARTICIPANT,
                    ],
                )
                .unwrap_or_else(|error| panic!("replace observation payload and hash: {error}"));
        });
        assert_recovery_corruption_quarantines(|connection| {
            connection
                .pragma_update(None, "foreign_keys", false)
                .unwrap_or_else(|error| panic!("disable fixture foreign keys: {error}"));
            connection
                .execute(
                    "UPDATE observation_frames SET cause_room_seq = 2 \
                     WHERE room_id = ?1 AND member_id = ?2",
                    params![ROOM, PARTICIPANT],
                )
                .unwrap_or_else(|error| panic!("crosswire observation cause: {error}"));
        });
        assert_recovery_corruption_quarantines(|connection| {
            connection
                .execute("DELETE FROM observation_frames WHERE room_id = ?1", [ROOM])
                .unwrap_or_else(|error| panic!("delete observation frame: {error}"));
            connection
                .execute(
                    "UPDATE room_members SET frame_head = 0 WHERE room_id = ?1",
                    [ROOM],
                )
                .unwrap_or_else(|error| panic!("hide missing frame behind frame Head: {error}"));
        });
        assert_recovery_corruption_quarantines(|connection| {
            connection
                .execute(
                    "INSERT INTO activation_decisions(\
                     room_id, cause_room_seq, decision_id, target_member_id, decision_bytes\
                     ) VALUES (?1, 1, 'unexpected', NULL, ?2)",
                    params![ROOM, br#"{}"#.as_slice()],
                )
                .unwrap_or_else(|error| panic!("insert unexpected Activation: {error}"));
        });
        for assignment in [
            "transition_hash = 'blake3:0000000000000000000000000000000000000000000000000000000000000000'",
            "transition_id = '01ARZ3NDEKTSV4RRFFQ69G5FBB'",
            "previous_lineage_hash = 'blake3:0000000000000000000000000000000000000000000000000000000000000000'",
            "core_state_hash = 'blake3:0000000000000000000000000000000000000000000000000000000000000000'",
        ] {
            assert_recovery_corruption_quarantines(|connection| {
                connection
                    .execute_batch("DROP TRIGGER transitions_immutable_update")
                    .unwrap_or_else(|error| panic!("drop Transition update trigger: {error}"));
                connection
                    .execute(
                        &format!("UPDATE transitions SET {assignment} WHERE room_id = ?1"),
                        [ROOM],
                    )
                    .unwrap_or_else(|error| panic!("corrupt Transition projection: {error}"));
            });
        }
        for assignment in [
            "canonical_request_hash = zeroblob(32)",
            "operation_identity_bytes = X'7B7D'",
            "semantic_input_bytes = X'7B7D'",
        ] {
            assert_recovery_corruption_quarantines(|connection| {
                connection
                    .execute_batch("DROP TRIGGER semantic_receipts_immutable_update")
                    .unwrap_or_else(|error| panic!("drop receipt update trigger: {error}"));
                connection
                    .execute(
                        &format!(
                            "UPDATE semantic_receipts SET {assignment} \
                             WHERE room_id = ?1 AND transition_seq = 1"
                        ),
                        [ROOM],
                    )
                    .unwrap_or_else(|error| panic!("corrupt receipt projection: {error}"));
            });
        }
    }

    #[test]
    fn recovery_verifies_or_rebuilds_the_exact_timer_generation_ledger() {
        assert_timer_recovery_guard_quarantines(|connection| {
            connection
                .execute(
                    "UPDATE timers SET scheduled_for = '2026-08-15T12:30:01Z' \
                     WHERE room_id = ?1 AND timer_id = ?2",
                    params![ROOM, TIMER],
                )
                .unwrap_or_else(|error| panic!("corrupt Timer schedule: {error}"));
        });
        assert_timer_recovery_guard_quarantines(|connection| {
            connection
                .execute(
                    "UPDATE timers SET payload_bytes = ?1 WHERE room_id = ?2 AND timer_id = ?3",
                    params![br#"{}"#.as_slice(), ROOM, TIMER],
                )
                .unwrap_or_else(|error| panic!("corrupt Timer payload: {error}"));
        });
        assert_timer_recovery_guard_quarantines(|connection| {
            connection
                .execute(
                    "UPDATE timers SET state = 'cancelled' WHERE room_id = ?1 AND timer_id = ?2",
                    params![ROOM, TIMER],
                )
                .unwrap_or_else(|error| panic!("corrupt Timer state: {error}"));
        });
        assert_timer_recovery_guard_quarantines(|connection| {
            connection
                .execute(
                    "DELETE FROM timers WHERE room_id = ?1 AND timer_id = ?2",
                    params![ROOM, TIMER_ALT],
                )
                .unwrap_or_else(|error| panic!("delete one Timer generation: {error}"));
        });

        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("temp Timer DB: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer SQLite: {error}"));
        let trace = timer_trace();
        seed_timer_room(file.path(), &trace);
        let recovered = RecoveredRoomMaterializationsV1::from_trace_for_conformance(&trace)
            .unwrap_or_else(|error| panic!("Timer recovery materializations: {error}"));
        let connection = Connection::open(file.path())
            .unwrap_or_else(|error| panic!("open Timer rebuild fixture: {error}"));
        connection
            .execute("DELETE FROM timers WHERE room_id = ?1", [ROOM])
            .unwrap_or_else(|error| panic!("delete disposable Timer ledger: {error}"));
        RoomRecoveryStorageV1::guard_recovery_install(
            &store,
            &parsed(ROOM),
            trace.head(),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("integrity generation: {error}")),
            &recovered,
        )
        .unwrap_or_else(|error| panic!("rebuild exact Timer ledger: {error}"));
        let rebuilt: Vec<(String, i64, String, Vec<u8>, String)> = {
            let mut statement = connection
                .prepare(
                    "SELECT timer_id, generation, scheduled_for, payload_bytes, state \
                     FROM timers WHERE room_id = ?1 ORDER BY timer_id, generation",
                )
                .unwrap_or_else(|error| panic!("prepare rebuilt Timer read: {error}"));
            statement
                .query_map([ROOM], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap_or_else(|error| panic!("query rebuilt Timers: {error}"))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|error| panic!("collect rebuilt Timers: {error}"))
        };
        assert_eq!(rebuilt.len(), 2);
        assert!(rebuilt.iter().all(|row| row.1 == 1 && row.4 == "scheduled"));
        assert_eq!(
            rebuilt
                .iter()
                .find(|row| row.0 == TIMER)
                .map(|row| (row.2.as_str(), row.3.as_slice())),
            Some(("2026-08-15T12:30:00Z", br#"{"kind":"deadline"}"#.as_slice(),))
        );
    }
}
