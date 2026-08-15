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
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

#[cfg(test)]
use std::sync::Condvar;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, ffi::ErrorCode,
    params,
};
use thiserror::Error;
use worldstream_core::{
    AccessModeV1, Blake3DigestV1, CanonicalRequestHashV1, CompleteHeadV1, CoreRoomStateV1,
    CoreTraceV1, GenesisV1, IntegrityGenerationV1, MembershipStandingV1, OperationIdentityV1,
    PackRegistryV1, PreparedAdvancePersistenceV1, PreparedAuthorityWitnessV1,
    PreparedCreationPersistenceV1, PreparedExistingIntentV1, PreparedOperationInputWitnessV1,
    PreparedRoomWriteV1, PreparedTimerMutationKindV1, PrincipalKindV1, ReceiptSemanticInputV1,
    RecordedStimulusV1, RecoveredRoomMaterializationsV1, RecoveredTimerStateV1,
    RecoveryIntegrityDispositionV1, ReplayReportV1, ResolutionStatusV1, ResolveOutcomeV1,
    RoomCommitResolutionV1, RoomCommitStorageV1, RoomId, RoomRecoveryCandidateV1,
    RoomRecoveryErrorV1, RoomRecoveryStorageV1, RoomStatusV1, SemanticResultV1,
    StoredSemanticResultV1, TransitionV1, recover_room_from_storage,
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
const OPERATION_RECEIPT_CODEC_ID: &str = "worldstream/operation-receipt/v1";
const WRITER_QUEUE_CAPACITY: usize = 32;

const MIGRATION_SCHEMA: &str = r"
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

#[cfg(test)]
static GUARDED_COMMIT_PAUSE: OnceLock<(Mutex<GuardedCommitPause>, Condvar)> = OnceLock::new();

#[cfg(test)]
static RECOVERY_INSTALL_PAUSE: OnceLock<(Mutex<RecoveryInstallPause>, Condvar)> = OnceLock::new();

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

/// Cloneable handle to one controlled writer thread and connection.
#[derive(Clone)]
pub struct SqliteRoomStore {
    writer: Arc<WriterClient>,
}

/// One immutable, transactionally bounded Room-history inspection. Its exact
/// bytes remain untrusted until [`Self::verify_replay`] succeeds; it carries
/// no snapshot or write ability.
#[derive(Clone, Debug)]
pub struct RoomHistoryInspectionV1 {
    head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    canonical_head_bytes: Vec<u8>,
    canonical_pack_revision_lock_bytes: Vec<u8>,
    canonical_genesis_bytes: Vec<u8>,
    canonical_transition_bytes: Vec<Vec<u8>>,
    canonical_core_state_bytes: Option<Vec<u8>>,
    canonical_activity_state_bytes: Option<Vec<u8>>,
}

impl RoomHistoryInspectionV1 {
    #[must_use]
    pub const fn head(&self) -> &CompleteHeadV1 {
        &self.head
    }

    #[must_use]
    pub fn canonical_head_bytes(&self) -> &[u8] {
        &self.canonical_head_bytes
    }

    #[must_use]
    pub fn canonical_genesis_bytes(&self) -> &[u8] {
        &self.canonical_genesis_bytes
    }

    #[must_use]
    pub fn canonical_pack_revision_lock_bytes(&self) -> &[u8] {
        &self.canonical_pack_revision_lock_bytes
    }

    #[must_use]
    pub fn canonical_transition_bytes(&self) -> &[Vec<u8>] {
        &self.canonical_transition_bytes
    }

    #[must_use]
    pub fn canonical_core_state_bytes(&self) -> Option<&[u8]> {
        self.canonical_core_state_bytes.as_deref()
    }

    #[must_use]
    pub fn canonical_activity_state_bytes(&self) -> Option<&[u8]> {
        self.canonical_activity_state_bytes.as_deref()
    }

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

    /// Registry-replays the exact lineage bytes and verifies that the replayed
    /// final Head/Core/Activity values equal the captured durable projections.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` if retained-pack replay fails or any captured final
    /// projection differs from replay.
    pub fn verify_replay(
        &self,
        registry: &PackRegistryV1,
    ) -> Result<ReplayReportV1, SqliteRoomInspectionErrorV1> {
        let report = CoreTraceV1::replay(
            registry,
            &self.canonical_genesis_bytes,
            &self.canonical_transition_bytes,
        )
        .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
        worldstream_core::CanonicalJsonV1::from_canonical_bytes(
            &self.canonical_pack_revision_lock_bytes,
        )
        .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
        let replayed_revision_lock_bytes = report
            .retained_pack_revision_lock()
            .ok_or(SqliteRoomInspectionErrorV1::Corrupt)?
            .canonical_bytes()
            .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
        let replayed_core_state_bytes = report
            .final_state()
            .core_state()
            .canonical_bytes()
            .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
        let replayed_activity_state_bytes = report
            .final_state()
            .activity_state()
            .to_bytes()
            .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
        if report.final_head != self.head
            || replayed_revision_lock_bytes != self.canonical_pack_revision_lock_bytes
            || report
                .final_head
                .canonical_bytes()
                .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?
                != self.canonical_head_bytes
            || self
                .canonical_core_state_bytes
                .as_ref()
                .is_some_and(|stored| stored != &replayed_core_state_bytes)
            || self
                .canonical_activity_state_bytes
                .as_ref()
                .is_some_and(|stored| stored != &replayed_activity_state_bytes)
        {
            return Err(SqliteRoomInspectionErrorV1::Corrupt);
        }
        Ok(report)
    }
}

struct WriterClient {
    commands: SyncSender<WriterCommand>,
    path: PathBuf,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

enum WriterCommand {
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
    SetFailpoint(Option<WriteBoundary>, mpsc::Sender<()>),
    #[cfg(test)]
    SetQueryOnly(bool, mpsc::Sender<Result<(), String>>),
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
            .spawn(move || writer_main(&thread_path, receiver, startup_send))
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
        });
        registry.insert(writer.path.clone(), Arc::downgrade(&writer));
        Ok(Self { writer })
    }

    /// Returns the exact runtime engine identity verified at open.
    #[must_use]
    pub fn engine_identity(&self) -> (&'static str, &'static str) {
        (SQLITE_VERSION, SQLITE_SOURCE_ID)
    }

    /// Captures one read-only Room history at a single `SQLite` snapshot and
    /// bounds Transition loading by the captured durable Head.
    ///
    /// # Errors
    ///
    /// Returns an error for `SQLite` I/O, a noncanonical/internally inconsistent
    /// row projection, missing lineage rows, or non-healthy integrity state.
    /// Call [`RoomHistoryInspectionV1::verify_replay`] before serving it.
    pub fn inspect_room(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomHistoryInspectionV1>, SqliteRoomInspectionErrorV1> {
        inspect_room_at_path(&self.writer.path, room_id)
    }

    /// Recovers one executable Room only after bounded registry replay and a
    /// writer-serialized reread of the exact captured Head and integrity generation.
    ///
    /// # Errors
    ///
    /// Returns a closed recovery error if storage is unavailable, integrity is
    /// unhealthy, durable bytes are corrupt, or the recovery fence changes.
    pub fn recover_room(
        &self,
        registry: &PackRegistryV1,
        room_id: &RoomId,
    ) -> Result<Option<CoreTraceV1>, RoomRecoveryErrorV1> {
        recover_room_from_storage(self, registry, room_id)
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
        SqliteRoomInspectionErrorV1::Sqlite(_) => RoomRecoveryErrorV1::StorageUnavailable,
        SqliteRoomInspectionErrorV1::Corrupt => RoomRecoveryErrorV1::Corrupt,
        SqliteRoomInspectionErrorV1::IntegrityUnavailable => {
            RoomRecoveryErrorV1::IntegrityUnavailable
        }
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
    worldstream_core::CanonicalJsonV1::from_canonical_bytes(&canonical_pack_revision_lock_bytes)
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
    let preflight = inspection
        .recovery_candidate()
        .preflight_lineage_materializations()
        .map_err(|_| SqliteRoomInspectionErrorV1::Corrupt)?;
    verify_pre_runtime_recovery_projections(
        &transaction,
        &room_id,
        inspection.head(),
        &stored.room_status,
        &preflight,
    )
    .map_err(map_pre_runtime_recovery_error)?;
    transaction.commit()?;
    Ok(Some(inspection))
}

fn map_pre_runtime_recovery_error(error: RoomRecoveryErrorV1) -> SqliteRoomInspectionErrorV1 {
    match error {
        RoomRecoveryErrorV1::StorageUnavailable => {
            SqliteRoomInspectionErrorV1::Sqlite(rusqlite::Error::InvalidQuery)
        }
        RoomRecoveryErrorV1::Corrupt
        | RoomRecoveryErrorV1::RuntimeUnavailable
        | RoomRecoveryErrorV1::RuntimeFault
        | RoomRecoveryErrorV1::IntegrityUnavailable
        | RoomRecoveryErrorV1::ConcurrentChange => SqliteRoomInspectionErrorV1::Corrupt,
    }
}

fn verify_pre_runtime_recovery_projections(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    stored_room_status: &str,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<(), RoomRecoveryErrorV1> {
    if stored_room_status != room_status(recovered.room_status()) {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    let frame_heads =
        verify_recovery_frame_structure(transaction, room_id, expected_head, recovered)?;
    verify_recovery_memberships(transaction, room_id, recovered, &frame_heads)?;
    verify_recovery_timers(transaction, room_id, recovered)?;
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

fn verify_recovery_frame_structure(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<BTreeMap<String, i64>, RoomRecoveryErrorV1> {
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
    }
    Ok(frame_heads)
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
            WriterCommand::Commit(prepared, reply) => {
                #[cfg(test)]
                let resolution = commit_prepared(&mut connection, *prepared, failpoint);
                #[cfg(not(test))]
                let resolution = commit_prepared(&mut connection, *prepared);
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

fn verify_recovery_frames(
    transaction: &Transaction<'_>,
    room_id: &str,
    expected_head: &CompleteHeadV1,
    recovered: &RecoveredRoomMaterializationsV1,
) -> Result<BTreeMap<String, i64>, RoomRecoveryErrorV1> {
    let mut frame_heads = recovered
        .memberships()
        .iter()
        .map(|member| (member.membership.member_id().to_string(), 0_i64))
        .collect::<BTreeMap<_, _>>();
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
    let mut stored_count = 0_usize;
    for row in rows {
        let (member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes) =
            row.map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
        let expected = expected_frames
            .get(&(member_id.clone(), frame_seq))
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        let previous = frame_heads
            .get_mut(&member_id)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
        if cause_room_seq != expected.0
            || payload_hash != expected.1
            || previous.checked_add(1) != Some(frame_seq)
            || cause_room_seq < 1
            || cause_room_seq > maximum_cause
            || worldstream_core::CanonicalJsonV1::from_canonical_bytes(&payload_bytes).is_err()
            || payload_hash != Blake3DigestV1::hash(&payload_bytes).to_string()
        {
            return Err(RoomRecoveryErrorV1::Corrupt);
        }
        *previous = frame_seq;
        stored_count = stored_count
            .checked_add(1)
            .ok_or(RoomRecoveryErrorV1::Corrupt)?;
    }
    if stored_count != expected_frames.len() {
        return Err(RoomRecoveryErrorV1::Corrupt);
    }
    Ok(frame_heads)
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
    );
    let stored = {
        let mut statement = transaction
            .prepare(
                "SELECT member_id, principal_id, principal_kind, standing, access_mode, role, \
                 membership_bytes, frame_head FROM room_members \
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
            || Some(row.7) != frame_heads.get(&expected_member_id).copied()
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
                 membership_bytes, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    room_id,
                    member_id,
                    membership.principal_id().to_string(),
                    principal_kind(membership.principal_kind()),
                    membership_standing(membership.standing()),
                    access_mode(membership.access_mode()),
                    membership.role(),
                    member.canonical_membership_bytes,
                    frame_heads.get(&member_id).copied().unwrap_or(0),
                ],
            )
            .map_err(|_| RoomRecoveryErrorV1::StorageUnavailable)?;
    }
    Ok(())
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
    let existing: Option<String> = transaction
        .query_row(
            "SELECT migration_id FROM schema_migrations WHERE version = 1",
            (),
            |row| row.get(0),
        )
        .optional()
        .map_err(SqliteStoreOpenError::Sqlite)?;
    match existing {
        Some(found) if found != INITIAL_MIGRATION_ID => {
            return Err(SqliteStoreOpenError::MigrationIdentity {
                expected: INITIAL_MIGRATION_ID,
                found,
            });
        }
        Some(_) => {}
        None => {
            transaction
                .execute_batch(MIGRATION_SCHEMA)
                .map_err(SqliteStoreOpenError::Sqlite)?;
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version, migration_id) VALUES (1, ?1)",
                    [INITIAL_MIGRATION_ID],
                )
                .map_err(SqliteStoreOpenError::Sqlite)?;
        }
    }
    let migration_count: i64 = transaction
        .query_row("SELECT count(*) FROM schema_migrations", (), |row| {
            row.get(0)
        })
        .map_err(SqliteStoreOpenError::Sqlite)?;
    if migration_count != 1 {
        return Err(SqliteStoreOpenError::UnexpectedMigrationSet);
    }
    verify_schema(&transaction)?;
    transaction.commit().map_err(SqliteStoreOpenError::Sqlite)
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
        .execute_batch(MIGRATION_SCHEMA)
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
) -> RoomCommitResolutionV1 {
    commit_prepared_inner(connection, prepared, None)
}

#[cfg(test)]
fn commit_prepared(
    connection: &mut Connection,
    prepared: SqlitePreparedWrite,
    failpoint: Option<WriteBoundary>,
) -> RoomCommitResolutionV1 {
    commit_prepared_inner(connection, prepared, failpoint)
}

fn commit_prepared_inner(
    connection: &mut Connection,
    prepared: SqlitePreparedWrite,
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
            commit_create(&transaction, &prepared, persistence, failpoint)
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
    failpoint: Option<WriteBoundary>,
) -> Result<(), RoomCommitResolutionV1> {
    match authority_matches(transaction, &prepared.authority) {
        Ok(true) => {}
        Ok(false) => return Err(RoomCommitResolutionV1::Fenced),
        Err(resolution) => return Err(resolution),
    }
    let head = &persistence.complete_head;
    let room_id = head.room_id().to_string();
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
                 membership_bytes, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)",
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

#[allow(clippy::too_many_lines)]
fn commit_existing(
    transaction: &Transaction<'_>,
    prepared: &SqlitePreparedWrite,
    basis: &worldstream_core::CompleteHeadV1,
    integrity_generation: worldstream_core::IntegrityGenerationV1,
    input_witness: &PreparedOperationInputWitnessV1,
    intent: &PreparedExistingIntentV1,
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

    match authority_matches(transaction, &prepared.authority) {
        Ok(true) => {}
        Ok(false) => return Err(RoomCommitResolutionV1::Fenced),
        Err(resolution) => return Err(resolution),
    }

    let timer_is_applicable = match input_witness {
        PreparedOperationInputWitnessV1::ParticipantAction(_) => true,
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
                 membership_bytes, frame_head\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0) \
                 ON CONFLICT(room_id, member_id) DO UPDATE SET \
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
) -> Result<bool, RoomCommitResolutionV1> {
    let generation = i64::try_from(witness.generation())
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or(RoomCommitResolutionV1::Fault)?;
    transaction
        .query_row(
            "SELECT EXISTS( \
             SELECT 1 FROM authority_fences \
             WHERE witness_id = ?1 AND authenticated_principal = ?2 AND generation = ?3 \
             AND scope_revocation_bytes = ?4 AND scope_revocation_hash = ?5 AND active = 1 \
             )",
            params![
                witness.witness_id(),
                witness.authenticated_principal().to_string(),
                generation,
                witness.canonical_scope_revocation_bytes(),
                witness.scope_revocation_hash().as_bytes().as_slice(),
            ],
            |row| row.get(0),
        )
        .map_err(statement_failure)
}

#[cfg(test)]
fn seed_authority_fence(
    connection: &Connection,
    witness: &PreparedAuthorityWitnessV1,
    active: bool,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO authority_fences(\
         witness_id, authenticated_principal, generation, scope_revocation_bytes,\
         scope_revocation_hash, active\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(witness_id) DO UPDATE SET \
         authenticated_principal = excluded.authenticated_principal, \
         generation = excluded.generation, \
         scope_revocation_bytes = excluded.scope_revocation_bytes, \
         scope_revocation_hash = excluded.scope_revocation_hash, \
         active = excluded.active",
        params![
            witness.witness_id(),
            witness.authenticated_principal().to_string(),
            i64::try_from(witness.generation()).unwrap_or(MAX_SAFE_INTEGER),
            witness.canonical_scope_revocation_bytes(),
            witness.scope_revocation_hash().as_bytes().as_slice(),
            i64::from(active),
        ],
    )?;
    Ok(())
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
pub enum SqliteRoomInspectionErrorV1 {
    #[error("SQLite inspection error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("durable Room history or a materialized projection is inconsistent")]
    Corrupt,
    #[error("Room integrity is not healthy and inspect/replay is withheld")]
    IntegrityUnavailable,
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
    #[error("migration version 1 is {found}, expected {expected}")]
    MigrationIdentity {
        expected: &'static str,
        found: String,
    },
    #[error("database has an unsupported migration set")]
    UnexpectedMigrationSet,
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
        ActorInstallationV1, AdministrationOperationIdentityV1, CREATE_ROOM_OPERATION_KIND,
        CanonicalJsonV1, CoreRoomStateV1, CoreTraceV1, ExistingRoomPendingAttemptV1,
        ExistingRoomReprepareV1, GenesisInputV1, InitialMembershipProposalV1,
        IntegrityGenerationV1, MembershipStandingV1, MembershipV1, OperationIdentityV1,
        PackGenesisRequestV1, PackRegistryV1, ParticipantActionRequestV1, ParticipantActionV1,
        PreparedAuthorityWitnessV1, PreparedExistingIntentV1, PreparedNewRoomGenesisV1,
        PreparedRoomCommitV1, PreparedRoomCreationV1, PreparedRoomWriteV1, PrincipalKindV1,
        RecordedStimulusV1, RecoveredRoomMaterializationsV1, RecoveryIntegrityDispositionV1,
        ResolutionStatusV1, ResolveOutcomeV1, RoomCommitResolutionV1, RoomCommitStorageV1,
        RoomCreationPendingAttemptV1, RoomCreationRequestV1, RoomRecoveryErrorV1,
        RoomRecoveryStorageV1, RoomSeedV1, ScheduledTimerV1, TimerFiredRequestV1, TimerFiredV1,
        TimerGenerationV1, TransitionId, builtin_counter_registry, commit_existing_room,
        commit_room_creation, counter_v1_only_registry_for_conformance, counter_v2_digest,
        counter_v2_invalid_timer_output_registry_for_conformance,
        counter_v2_malformed_output_registry_for_conformance,
        counter_v2_returned_fault_registry_for_conformance,
        counter_v2_runtime_fault_registry_for_conformance,
        counter_v2_semantic_mismatch_registry_for_conformance,
    };

    use super::{
        SQLITE_SOURCE_ID, SQLITE_VERSION, SqliteRoomInspectionErrorV1, SqliteRoomStore,
        WriteBoundary, arm_guarded_commit_pause, arm_recovery_install_pause,
        release_guarded_commit, release_recovery_install, wait_until_guarded_commit_pauses,
        wait_until_recovery_install_pauses,
    };

    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ROOM_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const PARTICIPANT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    const PARTICIPANT_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    const PRINCIPAL_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
    const ACTION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
    const ACTION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE1";
    const TRANSITION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF0";
    const TRANSITION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF1";
    const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG0";
    const TIMER_TRANSITION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG1";
    const TIMER_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG2";
    const TIMER_TRANSITION_ALT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FG3";
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
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
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
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
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
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
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
            store.recover_room(&registry, &parsed(ROOM)),
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
        assert!(matches!(
            store.recover_room(&missing_runtime, &parsed(ROOM)),
            Err(RoomRecoveryErrorV1::Corrupt)
        ));
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
            store.recover_room(registry, &parsed(ROOM)),
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
            store.recover_room(registry, &parsed(ROOM)),
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
                "authority_fences",
                "SELECT witness_id, authenticated_principal, generation, scope_revocation_bytes, \
                 scope_revocation_hash, active FROM authority_fences ORDER BY witness_id",
                6,
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
                 role, membership_bytes, frame_head FROM room_members ORDER BY room_id, member_id",
                9,
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
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
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
        let occupied = PreparedRoomCreationV1::from_registry_genesis(
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

        let (candidate_genesis, candidate_request, _, candidate_identity) = creation_fixture(0);
        let candidate = PreparedRoomCreationV1::from_registry_genesis(
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
            .reseal(refreshed_authority, regenerated_genesis)
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
        let lost_plan = PreparedRoomCreationV1::from_registry_genesis(
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
        let regenerated = PreparedRoomCreationV1::from_registry_genesis(
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
            .seal_stable_disposition(
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
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
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
        let inspection = reopened
            .inspect_room(&parsed(ROOM))
            .unwrap_or_else(|error| panic!("inspect restarted Room: {error}"))
            .unwrap_or_else(|| panic!("restarted Room exists"));
        assert_eq!(inspection.head(), &expected_head);
        assert_eq!(inspection.canonical_transition_bytes().len(), 1);
        assert_eq!(
            inspection.canonical_head_bytes(),
            expected_head
                .canonical_bytes()
                .unwrap_or_else(|error| panic!("expected Head bytes: {error}"))
        );
        let registry = builtin_counter_registry()
            .unwrap_or_else(|error| panic!("Counter registry for restart: {error}"));
        let report = inspection
            .verify_replay(&registry)
            .unwrap_or_else(|error| panic!("replay durable Room history: {error}"));
        assert_eq!(&report.final_head, inspection.head());
        assert_eq!(
            report
                .final_state()
                .core_state()
                .canonical_bytes()
                .unwrap_or_else(|error| panic!("replayed Core bytes: {error}")),
            inspection
                .canonical_core_state_bytes()
                .unwrap_or_else(|| panic!("stored Core materialization exists"))
        );
        assert_eq!(
            report
                .final_state()
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| panic!("replayed Activity bytes: {error}")),
            inspection
                .canonical_activity_state_bytes()
                .unwrap_or_else(|| panic!("stored Activity materialization exists"))
        );
        let recovered = reopened
            .recover_room(&registry, &parsed(ROOM))
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
        let mut recovered = reopened
            .recover_room(&registry, &parsed(ROOM))
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
            recovery_store.recover_room(&registry, &parsed(ROOM))
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
            recovery_store.recover_room(&registry, &parsed(ROOM))
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
            store.recover_room(&registry, &parsed(ROOM)),
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
            match store.inspect_room(&parsed(ROOM)) {
                Ok(Some(inspection)) => assert!(
                    matches!(
                        inspection.verify_replay(&registry),
                        Err(SqliteRoomInspectionErrorV1::Corrupt)
                    ),
                    "{corruption} passed registry replay"
                ),
                Err(SqliteRoomInspectionErrorV1::Corrupt)
                    if matches!(
                        corruption,
                        "activity_materialization"
                            | "genesis"
                            | "transition"
                            | "missing_transition"
                    ) => {}
                other => panic!("unexpected {corruption} inspection: {other:?}"),
            }
            assert!(matches!(
                store.recover_room(&registry, &parsed(ROOM)),
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
            store.recover_room(&missing_runtime, &parsed(ROOM)),
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
            store.recover_room(&complete_registry, &parsed(ROOM)),
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
