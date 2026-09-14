//! Real SQLite qualification driver for one deterministic Room history tier.
//!
//! This driver uses the public production Room creation, authorization,
//! commit, and recovery seams. It deliberately reports backend evidence
//! separately from the modeled matrix; a missing scenario is explicit and can
//! never be promoted to a qualification pass by the Python harness.

use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::Arc,
    time::Instant,
};

use rusqlite::Connection;
use serde::Serialize;
use worldstream_core::{
    AccessModeV1, AdministrationOperationIdentityV1, AuthorityBootstrapV1, AuthorityV1,
    Blake3DigestV1, CapabilityBearerV1, CompleteHeadV1, CoreAdministrationIngressV1,
    CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1, CoreRoomStateV1,
    InitialMembershipProposalV1, MembershipChangeV1, MembershipStandingV1, MembershipV1,
    PackGenesisRequestV1, PackViewerV1, PreparedRoomCommitV1, PreparedRoomCreationV1,
    PresentedCapabilityV1, PrincipalKindV1, RecoveredActivationDecisionV1,
    RecoveredObservationConsequenceV1, RecoveredObservationFrameV1,
    RecoveredTimerMaterializationV1, RecoveredTimerStateV1, ResolutionStatusV1,
    RoomCheckpointOperationalWitnessV1, RoomCheckpointOperationalWitnessV2, RoomCommitResolutionV1, RoomCreationIngressV1,
    RoomCreationRequestV1, RoomId, RoomRecoveryStorageV1, RoomSeedV1, RoomSequenceV1,
    TimerGenerationV1, ViewInputV1, authorize_core_administration_operation,
    authorize_room_creation_operation, builtin_counter_registry, commit_existing_room,
    commit_room_creation, counter_v2_digest, recover_room_from_storage_with_receipt,
};
use worldstream_sqlite::SqliteRoomStore;
use worldstream_transfer::{
    DeploymentIdentityV1, DigestV1, PackIdentityV1, ResourceKindV1, ResourcePayloadV1,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";
const HOST_BEARER: [u8; 32] = [0xA7; 32];
const ROOM_SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    source: &'static str,
    requested_transition_count: u64,
    counters: Counters,
    history: History,
    storage: Storage,
    snapshots: SnapshotMetrics,
    checkpoint: CheckpointEvidence,
    scenarios: Scenarios,
    transfer_metadata_initialized: bool,
    pass: bool,
}

#[derive(Serialize)]
struct Counters {
    models: u64,
    invocations: u64,
    attempts: u64,
    transitions: u64,
    frames: u64,
}

#[derive(Serialize)]
struct History {
    head_room_seq: u64,
    warm_path_reads: u64,
    reducer_callbacks: usize,
    recovery_ms: u128,
    context_bytes: usize,
}

#[derive(Serialize)]
struct Storage {
    db_bytes: u64,
    wal_bytes: u64,
    shm_bytes: u64,
    rss_bytes: Option<u64>,
    transitions: u64,
    frames: u64,
}

/// Counts captured by a transient observer trigger attached before the first
/// production Room commit. The trigger is dropped before this fixture exits,
/// so it never becomes source data or changes the retained backup contract.
#[derive(Serialize)]
struct SnapshotMetrics {
    /// Every successful post-commit snapshot preparation ends in one insert in
    /// this no-failpoint workload, so this is also the measured preparation
    /// count for the fixed-state cadence run.
    preparation_count: u64,
    /// Actual `room_snapshots` inserts observed while the production writer
    /// executed its post-commit cache work.
    write_count: u64,
    observed_snapshot_count: u64,
    retained_row_count: u64,
    last_snapshot_room_seq: u64,
    transitions_since_snapshot: u64,
    /// Bounded samples of serialized bytes measured from the exact bytes
    /// inserted by the production writer. The complete count is reported in
    /// `observed_snapshot_count`; this list never grows with history length.
    per_snapshot: Vec<SnapshotWriteAttribution>,
    /// SQLite does not expose writer CPU or WAL bytes for one post-commit
    /// callback independently from the surrounding process/connection. Keep
    /// those fields explicit and unavailable rather than attributing the
    /// whole transition transaction to the cache.
    cpu_attribution: AttributionUnavailable,
    wal_attribution: AttributionUnavailable,
}

#[derive(Serialize)]
struct SnapshotWriteAttribution {
    room_seq: u64,
    serialized_bytes: u64,
}

#[derive(Serialize)]
struct AttributionUnavailable {
    value: Option<u64>,
    source: &'static str,
}

#[derive(Serialize)]
struct WitnessCollectionEvidence {
    live_rows: usize,
    witness_entries: usize,
    exact: bool,
}

#[derive(Serialize)]
struct CheckpointEvidence {
    recovery_execution_path: &'static str,
    checkpoint_room_seq: u64,
    checkpoint_boundary_transition_records_read_by_adapter: u64,
    prefix_transition_range_reads: u64,
    prefix_transition_records_delivered_to_core: u64,
    prefix_transitions_skipped: u64,
    tail_transition_records_delivered_to_core: u64,
    transition_records_read_by_adapter_total: u64,
    witness_bytes: usize,
    witness_hash_exact: bool,
    witness_head_exact: bool,
    timer_ledger: WitnessCollectionEvidence,
    observation_frame_heads: WitnessCollectionEvidence,
    observation_frames: WitnessCollectionEvidence,
    observation_consequences: WitnessCollectionEvidence,
    membership_generations: WitnessCollectionEvidence,
    activation_decisions: WitnessCollectionEvidence,
    all_operational_witnesses_exact: bool,
}

#[derive(Serialize)]
struct Scenario {
    status: &'static str,
    source: &'static str,
}

#[derive(Serialize)]
struct Scenarios {
    crash_restart: Scenario,
    offline_runner: Scenario,
    timer_delivery: Scenario,
    backup: Scenario,
    transfer: Scenario,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn parsed<T: FromStr>(value: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("parse {value}: {error}").into())
}

fn transition_id(index: u64) -> Result<worldstream_core::TransitionId> {
    let mut suffix = [b'0'; 8];
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = index;
    for byte in suffix.iter_mut().rev() {
        *byte = alphabet[usize::try_from(value & 31)?];
        value >>= 5;
    }
    parsed(&format!(
        "01ARZ3NDEKTSV4RRFF{}",
        std::str::from_utf8(&suffix)?
    ))
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).map(|item| item.len()).unwrap_or(0)
}

fn rss_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kib = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

fn install_snapshot_observer(database: &Path) -> Result<()> {
    let connection = Connection::open(database)?;
    connection.execute_batch(
        r"
        CREATE TABLE fixture_snapshot_write_observations (
          room_seq INTEGER NOT NULL,
          serialized_bytes INTEGER NOT NULL
        );
        CREATE TRIGGER fixture_snapshot_write_observer
        AFTER INSERT ON room_snapshots
        BEGIN
          INSERT INTO fixture_snapshot_write_observations(room_seq, serialized_bytes)
          VALUES (
            NEW.room_seq,
            length(NEW.complete_head_bytes) + length(NEW.core_state_bytes) +
              length(NEW.activity_state_bytes)
          );
        END;
        ",
    )?;
    Ok(())
}

fn read_and_remove_snapshot_observer(database: &Path, room_id: &RoomId) -> Result<SnapshotMetrics> {
    let connection = Connection::open(database)?;
    let write_count = u64::try_from(connection.query_row::<i64, _, _>(
        "SELECT count(*) FROM fixture_snapshot_write_observations",
        [],
        |row| row.get(0),
    )?)?;
    let observed_snapshot_count = write_count;
    let retained_row_count = u64::try_from(connection.query_row::<i64, _, _>(
        "SELECT count(*) FROM room_snapshots WHERE room_id = ?1",
        [room_id.to_string()],
        |row| row.get(0),
    )?)?;
    let mut per_snapshot = Vec::new();
    let mut sample_offsets = vec![0_i64];
    if observed_snapshot_count > 2 {
        sample_offsets.push(i64::try_from(observed_snapshot_count / 2)?);
    }
    if observed_snapshot_count > 1 {
        sample_offsets.push(i64::try_from(observed_snapshot_count - 1)?);
    }
    for offset in sample_offsets {
        let (room_seq, serialized_bytes): (i64, i64) = connection.query_row(
            "SELECT room_seq, serialized_bytes \
             FROM fixture_snapshot_write_observations ORDER BY room_seq LIMIT 1 OFFSET ?1",
            [offset],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        per_snapshot.push(SnapshotWriteAttribution {
            room_seq: u64::try_from(room_seq)?,
            serialized_bytes: u64::try_from(serialized_bytes)?,
        });
    }
    let (last_snapshot_room_seq, transitions_since_snapshot): (i64, i64) = connection.query_row(
        "SELECT last_snapshot_room_seq, transitions_since_snapshot \
         FROM room_snapshot_schedules WHERE room_id = ?1",
        [room_id.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    connection.execute_batch(
        "DROP TRIGGER fixture_snapshot_write_observer;
         DROP TABLE fixture_snapshot_write_observations;",
    )?;
    Ok(SnapshotMetrics {
        preparation_count: write_count,
        write_count,
        observed_snapshot_count,
        retained_row_count,
        last_snapshot_room_seq: u64::try_from(last_snapshot_room_seq)?,
        transitions_since_snapshot: u64::try_from(transitions_since_snapshot)?,
        per_snapshot,
        cpu_attribution: AttributionUnavailable {
            value: None,
            source: "sqlite_writer_callback_not_separable_from_canonical_commit",
        },
        wal_attribution: AttributionUnavailable {
            value: None,
            source: "sqlite_wal_delta_not_separable_per_snapshot_without_checkpointing",
        },
    })
}

fn read_checkpoint_evidence(
    database: &Path,
    room_id: &RoomId,
    current_head: &CompleteHeadV1,
    receipt: worldstream_core::RoomRecoveryExecutionReceiptV1,
) -> Result<CheckpointEvidence> {
    let connection = Connection::open(database)?;
    let room_id_text = room_id.to_string();
    let (checkpoint_room_seq, checkpoint_head_bytes, witness_schema, witness_hash, witness_bytes): (
        i64,
        Vec<u8>,
        String,
        Vec<u8>,
        Vec<u8>,
    ) = connection.query_row(
        "SELECT room_seq, complete_head_bytes, witness_schema_version, witness_hash, witness_bytes FROM (\
           SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_schema_version, witness.witness_hash, witness.witness_bytes, 2 AS witness_version \
           FROM room_snapshots AS snapshots JOIN room_snapshot_operational_witnesses_v2 AS witness \
             ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq WHERE snapshots.room_id = ?1 \
           UNION ALL \
           SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_schema_version, witness.witness_hash, witness.witness_bytes, 1 AS witness_version \
           FROM room_snapshots AS snapshots JOIN room_snapshot_operational_witnesses AS witness \
             ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq WHERE snapshots.room_id = ?1 \
         ) ORDER BY room_seq DESC, witness_version DESC LIMIT 1",
        [&room_id_text],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;
    let checkpoint_head = worldstream_core::CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
        &checkpoint_head_bytes,
    )?;
    if witness_schema == worldstream_core::CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2 {
        let witness = RoomCheckpointOperationalWitnessV2::from_canonical_bytes(
            &witness_bytes, &checkpoint_head,
        ).map_err(|error| format!("decode V2 checkpoint witness: {error:?}"))?;
        let checkpoint_room_seq = u64::try_from(checkpoint_room_seq)?;
        if !receipt.used_checkpoint()
            || receipt.checkpoint_room_seq().map(|value| value.get()) != Some(checkpoint_room_seq)
            || receipt.prefix_transition_records_delivered() != 0
            || receipt.prefix_transitions_skipped() != checkpoint_room_seq {
            return Err("recovery receipt did not prove the selected checkpoint path".into());
        }
        let root_entries = |domain| -> Result<usize> {
            Ok(usize::try_from(witness.operational_history_roots()
                .get(domain).ok_or("missing V2 root")?.entry_count())?)
        };
        // These roots are snapshot-bound. Comparing them to the mutable
        // durable roots at the current Head is wrong when a checkpoint has a
        // tail: Core advances the roots while replaying that tail, and the
        // recovery install verifies those final roots before returning this
        // successful checkpoint execution.
        let roots_exact = witness.operational_history_roots().len() == 3
            && ["frames", "consequences", "activation_decisions"]
                .iter()
                .all(|domain| {
                    witness
                        .operational_history_roots()
                        .get(*domain)
                        .is_some_and(|root| root.domain() == *domain)
                });
        // The witness authenticates the selected snapshot boundary. It is
        // expected to differ from the current Room Head whenever recovery has
        // a tail to replay, so comparing it with `current_head` would make a
        // valid retained checkpoint look inexact.
        let exact = witness_hash.as_slice() == Blake3DigestV1::hash(&witness_bytes).as_bytes()
            && witness.checkpoint_head() == &checkpoint_head
            && checkpoint_head.room_seq().get() <= current_head.room_seq().get()
            && roots_exact;
        let collection = |live_rows, witness_entries| WitnessCollectionEvidence {
            live_rows, witness_entries, exact,
        };
        let tail = receipt.tail_transition_records_delivered();
        return Ok(CheckpointEvidence {
            recovery_execution_path: "checkpoint_v2",
            checkpoint_room_seq,
            checkpoint_boundary_transition_records_read_by_adapter: u64::from(checkpoint_room_seq > 0),
            prefix_transition_range_reads: 0,
            prefix_transition_records_delivered_to_core: receipt.prefix_transition_records_delivered(),
            prefix_transitions_skipped: receipt.prefix_transitions_skipped(),
            tail_transition_records_delivered_to_core: tail,
            transition_records_read_by_adapter_total: u64::from(checkpoint_room_seq > 0).checked_add(tail).ok_or("checkpoint Transition read count overflow")?,
            witness_bytes: witness_bytes.len(), witness_hash_exact: exact, witness_head_exact: exact,
            timer_ledger: collection(witness.timers().len(), witness.timers().len()),
            observation_frame_heads: collection(witness.observation_frame_heads().len(), witness.observation_frame_heads().len()),
            observation_frames: collection(root_entries("frames")?, 1),
            observation_consequences: collection(root_entries("consequences")?, 1),
            membership_generations: collection(witness.membership_generations().len(), witness.membership_generations().len()),
            activation_decisions: collection(root_entries("activation_decisions")?, 1),
            all_operational_witnesses_exact: exact,
        });
    }
    let witness =
        RoomCheckpointOperationalWitnessV1::from_canonical_bytes(&witness_bytes, &checkpoint_head)
            .map_err(|error| format!("decode checkpoint witness: {error:?}"))?;

    let mut timers = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT timer_id, generation, scheduled_for, payload_bytes, state FROM timers \
             WHERE room_id = ?1 ORDER BY timer_id, generation",
        )?;
        let rows = statement.query_map([&room_id_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (timer_id, generation, scheduled_for, payload, state) = row?;
            let state = match state.as_str() {
                "scheduled" => RecoveredTimerStateV1::Scheduled,
                "fired" => RecoveredTimerStateV1::Fired,
                "cancelled" => RecoveredTimerStateV1::Cancelled,
                _ => return Err(format!("unknown Timer state {state}").into()),
            };
            timers.push(RecoveredTimerMaterializationV1::new(
                parsed(&timer_id)?,
                TimerGenerationV1::new(u64::try_from(generation)?)?,
                parsed(&scheduled_for)?,
                payload,
                state,
            ));
        }
    }

    let mut observation_frame_heads = BTreeMap::new();
    let mut membership_generations = BTreeMap::new();
    {
        let mut statement = connection.prepare(
            "SELECT member_id, frame_head, membership_generation FROM room_members \
             WHERE room_id = ?1 ORDER BY member_id",
        )?;
        let rows = statement.query_map([&room_id_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (member_id, frame_head, generation) = row?;
            observation_frame_heads.insert(parsed(&member_id)?, u64::try_from(frame_head)?);
            membership_generations.insert(member_id, generation);
        }
    }

    let mut observation_frames = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT member_id, frame_seq, cause_room_seq, payload_hash FROM observation_frames \
             WHERE room_id = ?1 ORDER BY member_id, frame_seq",
        )?;
        let rows = statement.query_map([&room_id_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (member_id, frame_seq, cause_room_seq, payload_hash) = row?;
            observation_frames.push(RecoveredObservationFrameV1::from_replay(
                parsed(&member_id)?,
                u64::try_from(frame_seq)?,
                RoomSequenceV1::new(u64::try_from(cause_room_seq)?)?,
                parsed(&payload_hash)?,
            ));
        }
    }

    let mut observation_consequences = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT member_id, cause_room_seq, consequence_kind, projection_hash \
             FROM observation_consequences WHERE room_id = ?1 \
             ORDER BY member_id, cause_room_seq",
        )?;
        let rows = statement.query_map([&room_id_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (member_id, cause_room_seq, kind, projection_hash) = row?;
            let member_id = parsed(&member_id)?;
            let cause_room_seq = RoomSequenceV1::new(u64::try_from(cause_room_seq)?)?;
            observation_consequences.push(match kind.as_str() {
                "reset_required" => RecoveredObservationConsequenceV1::reset_required(
                    member_id,
                    cause_room_seq,
                    parsed(&projection_hash.ok_or("reset consequence lacks projection hash")?)?,
                ),
                "visibility_lost" if projection_hash.is_none() => {
                    RecoveredObservationConsequenceV1::visibility_lost(member_id, cause_room_seq)
                }
                _ => return Err(format!("invalid observation consequence {kind}").into()),
            });
        }
    }

    let mut activation_decisions = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT cause_room_seq, decision_id, target_member_id, decision_bytes \
             FROM activation_decisions WHERE room_id = ?1 \
             ORDER BY cause_room_seq, decision_id",
        )?;
        let rows = statement.query_map([&room_id_text], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        for row in rows {
            let (cause_room_seq, decision_id, target_member_id, decision_bytes) = row?;
            activation_decisions.push(RecoveredActivationDecisionV1::new(
                RoomSequenceV1::new(u64::try_from(cause_room_seq)?)?,
                decision_id,
                target_member_id.map(|value| parsed(&value)).transpose()?,
                decision_bytes,
            ));
        }
    }

    let timer_ledger_exact = witness.timers() == timers;
    let observation_frame_heads_exact =
        witness.observation_frame_heads() == &observation_frame_heads;
    let observation_frames_exact = witness.observation_frames() == observation_frames;
    let observation_consequences_exact =
        witness.observation_consequences() == observation_consequences;
    let membership_generations_exact = witness.membership_generations() == &membership_generations;
    let activation_decisions_exact = witness.activation_decisions() == activation_decisions;
    let witness_hash_exact =
        witness_hash.as_slice() == Blake3DigestV1::hash(&witness_bytes).as_bytes();
    // A V1 witness has the same boundary contract as V2: authenticate the
    // snapshot Head, while the caller separately verifies the recovered final
    // Head after replaying its tail.
    let witness_head_exact = witness.checkpoint_head() == &checkpoint_head
        && checkpoint_head.room_seq().get() <= current_head.room_seq().get();
    let all_operational_witnesses_exact = timer_ledger_exact
        && observation_frame_heads_exact
        && observation_frames_exact
        && observation_consequences_exact
        && membership_generations_exact
        && activation_decisions_exact;
    let checkpoint_room_seq = u64::try_from(checkpoint_room_seq)?;
    if !receipt.used_checkpoint()
        || receipt.checkpoint_room_seq().map(|value| value.get()) != Some(checkpoint_room_seq)
        || receipt.prefix_transition_records_delivered() != 0
        || receipt.prefix_transitions_skipped() != checkpoint_room_seq
    {
        return Err("recovery receipt did not prove the selected checkpoint path".into());
    }
    let checkpoint_boundary_transition_records_read_by_adapter = u64::from(checkpoint_room_seq > 0);
    let tail_transition_records_delivered_to_core = receipt.tail_transition_records_delivered();

    Ok(CheckpointEvidence {
        recovery_execution_path: "checkpoint",
        checkpoint_room_seq,
        checkpoint_boundary_transition_records_read_by_adapter,
        prefix_transition_range_reads: 0,
        prefix_transition_records_delivered_to_core: receipt.prefix_transition_records_delivered(),
        prefix_transitions_skipped: receipt.prefix_transitions_skipped(),
        tail_transition_records_delivered_to_core,
        transition_records_read_by_adapter_total:
            checkpoint_boundary_transition_records_read_by_adapter
                .checked_add(tail_transition_records_delivered_to_core)
                .ok_or("checkpoint Transition read count overflow")?,
        witness_bytes: witness_bytes.len(),
        witness_hash_exact,
        witness_head_exact,
        timer_ledger: WitnessCollectionEvidence {
            live_rows: timers.len(),
            witness_entries: witness.timers().len(),
            exact: timer_ledger_exact,
        },
        observation_frame_heads: WitnessCollectionEvidence {
            live_rows: observation_frame_heads.len(),
            witness_entries: witness.observation_frame_heads().len(),
            exact: observation_frame_heads_exact,
        },
        observation_frames: WitnessCollectionEvidence {
            live_rows: observation_frames.len(),
            witness_entries: witness.observation_frames().len(),
            exact: observation_frames_exact,
        },
        observation_consequences: WitnessCollectionEvidence {
            live_rows: observation_consequences.len(),
            witness_entries: witness.observation_consequences().len(),
            exact: observation_consequences_exact,
        },
        membership_generations: WitnessCollectionEvidence {
            live_rows: membership_generations.len(),
            witness_entries: witness.membership_generations().len(),
            exact: membership_generations_exact,
        },
        activation_decisions: WitnessCollectionEvidence {
            live_rows: activation_decisions.len(),
            witness_entries: witness.activation_decisions().len(),
            exact: activation_decisions_exact,
        },
        all_operational_witnesses_exact,
    })
}

fn no_checkpoint_evidence() -> CheckpointEvidence {
    let empty = || WitnessCollectionEvidence {
        live_rows: 0,
        witness_entries: 0,
        exact: false,
    };
    CheckpointEvidence {
        recovery_execution_path: "no_eligible_checkpoint",
        checkpoint_room_seq: 0,
        checkpoint_boundary_transition_records_read_by_adapter: 0,
        prefix_transition_range_reads: 0,
        prefix_transition_records_delivered_to_core: 0,
        prefix_transitions_skipped: 0,
        tail_transition_records_delivered_to_core: 0,
        transition_records_read_by_adapter_total: 0,
        witness_bytes: 0,
        witness_hash_exact: false,
        witness_head_exact: false,
        timer_ledger: empty(),
        observation_frame_heads: empty(),
        observation_frames: empty(),
        observation_consequences: empty(),
        membership_generations: empty(),
        activation_decisions: empty(),
        all_operational_witnesses_exact: false,
    }
}

fn initialize_stream_metadata(
    store: &SqliteRoomStore,
    trace: &worldstream_core::CoreTraceV1,
) -> Result<()> {
    let retained = trace
        .retained_pack()
        .ok_or("fixture retained Pack is absent")?;
    let lock = retained.revision_lock();
    let pack = PackIdentityV1::new(
        lock.pack_id.clone(),
        lock.explanatory_version.clone(),
        DigestV1::from_bytes(trace.head().pack_digest().digest().as_bytes())?,
    )?;
    let resource = ResourcePayloadV1::from_bytes(
        ResourceKindV1::Artifact,
        "worldstream.history-qualification.fixture",
        b"history qualification stream resource v1\n",
    )?;
    let identity = DeploymentIdentityV1::new(vec![pack], vec![resource.identity().clone()])?;
    store.initialize_canonical_metadata("deployment/history-qualification", 7)?;
    store.initialize_deployment_identity_with_resources(identity, vec![resource])?;
    Ok(())
}

fn run(database: &Path, count: u64, stream_metadata: bool) -> Result<Report> {
    if count == 0 {
        return Err("transition count must be positive".into());
    }
    let store = SqliteRoomStore::open(database)?;
    install_snapshot_observer(database)?;
    let authority = AuthorityV1::new(Arc::new(store.clone()));
    let host_bearer = CapabilityBearerV1::from_bytes(HOST_BEARER);
    authority.bootstrap(
        AuthorityBootstrapV1::new(
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FJ0")?,
            parsed(PRINCIPAL)?,
            PrincipalKindV1::Human,
            parsed(HOST_CAPABILITY)?,
            host_bearer.token_hash(),
            None,
        )?,
        parsed("2026-08-15T12:00:00Z")?,
    )?;
    let presented = PresentedCapabilityV1::new(parsed(HOST_CAPABILITY)?, host_bearer);
    let registry = builtin_counter_registry()?;
    let configuration =
        worldstream_core::CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)?;
    let membership = MembershipV1::new(
        parsed(MEMBER)?,
        parsed(PRINCIPAL)?,
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )?;
    let genesis = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed(ROOM)?,
        pack_digest: counter_v2_digest(),
        configuration: configuration.clone(),
        room_seed: parsed::<RoomSeedV1>(ROOM_SEED)?,
        created_at: parsed("2026-08-15T12:00:00Z")?,
        initial_core_state: CoreRoomStateV1::active([membership.clone()])?,
    })?;
    let creation = RoomCreationRequestV1::new(
        counter_v2_digest(),
        configuration,
        vec![InitialMembershipProposalV1::new(
            parsed(PRINCIPAL)?,
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )?],
    );
    let identity = AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(PRINCIPAL)?,
        versioned_operation_kind: worldstream_core::CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: "history-qualification-create".to_owned(),
    };
    let grant = match authorize_room_creation_operation(
        &authority,
        &store,
        &presented,
        &identity,
        &creation,
        parsed("2026-08-15T12:00:00Z")?,
    )? {
        RoomCreationIngressV1::Authorized(grant) => *grant,
        other => return Err(format!("unexpected creation resolution: {other:?}").into()),
    };
    let prepared =
        PreparedRoomCreationV1::from_registry_genesis(identity, &creation, grant, genesis)?;
    let outcome = commit_room_creation(&store, prepared);
    if !matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ) {
        return Err("Genesis was not newly committed".into());
    }
    let mut trace = outcome
        .into_committed_trace()
        .ok_or("Genesis did not release trace")?;
    let gateway_snapshot = store
        .gateway_room_snapshot(&registry, &parsed(ROOM)?)?
        .ok_or("missing snapshot")?;
    let mut frame_heads = gateway_snapshot.frame_heads().clone();
    let integrity = gateway_snapshot.integrity_generation();
    for index in 1..=count {
        let current = trace
            .core_state()
            .membership(&parsed(MEMBER)?)
            .ok_or("member disappeared")?
            .clone();
        let suspend = index % 2 == 1;
        let request = CoreAdministrationRequestV1::new(
            parsed(ROOM)?,
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL)?,
                versioned_operation_kind: worldstream_core::CORE_OPERATION_KIND.to_owned(),
                idempotency_key: format!("history-qualification-{index}"),
            },
            if suspend {
                CoreProposedKindV1::Suspend
            } else {
                CoreProposedKindV1::Resume
            },
            trace.head().room_seq(),
            "history_qualification",
            CoreChangeSetV1::one(if suspend {
                MembershipChangeV1::suspend(current)
            } else {
                MembershipChangeV1::resume(current)
            }),
        )?;
        let grant = match authorize_core_administration_operation(
            &authority,
            &store,
            &presented,
            &request,
            parsed("2026-08-15T12:00:00Z")?,
        )? {
            CoreAdministrationIngressV1::Authorized(grant) => *grant,
            other => return Err(format!("unexpected admin resolution: {other:?}").into()),
        };
        let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
            &trace,
            &request,
            parsed("2026-08-15T12:00:00Z")?,
            transition_id(index)?,
            integrity,
            grant,
            &frame_heads,
        )?;
        if let worldstream_core::PreparedExistingIntentV1::Advance(persistence) = prepared.intent()
        {
            for consequence in &persistence.delivery_consequences {
                if let worldstream_core::PreparedObservationConsequenceV1::ObservationFrame(frame) =
                    consequence
                {
                    frame_heads.insert(frame.member_id().clone(), frame.frame_seq());
                }
            }
        }
        let committed = commit_existing_room(&store, &mut trace, prepared);
        if !matches!(
            committed.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ) {
            return Err(format!("Transition {index} did not commit").into());
        }
    }
    let room_id: RoomId = parsed(ROOM)?;
    // The alternating administration workload can end with the measured
    // Membership suspended. A final participant projection is useful when it
    // is authorized, but it is not a prerequisite for the durable history
    // measurement and must not turn an otherwise valid odd-length run into a
    // fixture failure.
    let context_bytes = if trace
        .core_state()
        .membership(&parsed(MEMBER)?)
        .is_some_and(|membership| membership.standing() == MembershipStandingV1::Enabled)
    {
        registry
            .load_retained(trace.head().pack_digest())?
            .host()
            .view(&ViewInputV1 {
                core: trace.core_state(),
                activity_state: trace.activity_state(),
                complete_head: trace.head(),
                viewer: &PackViewerV1::Participant(parsed(MEMBER)?),
            })?
            .canonical_bytes()
            .len()
    } else {
        0
    };
    let snapshots = read_and_remove_snapshot_observer(database, &room_id)?;
    if stream_metadata {
        initialize_stream_metadata(&store, &trace)?;
    }
    drop(store);
    let db_bytes = size(database);
    let wal_bytes = size(&PathBuf::from(format!("{}-wal", database.display())));
    let shm_bytes = size(&PathBuf::from(format!("{}-shm", database.display())));
    let connection = Connection::open(database)?;
    let transition_rows: u64 = u64::try_from(connection.query_row::<i64, _, _>(
        "SELECT count(*) FROM transitions WHERE room_id = ?1",
        [room_id.to_string()],
        |row| row.get(0),
    )?)?;
    let frame_rows: u64 = u64::try_from(connection.query_row::<i64, _, _>(
        "SELECT count(*) FROM observation_frames WHERE room_id = ?1",
        [room_id.to_string()],
        |row| row.get(0),
    )?)?;
    drop(connection);
    let reopened = SqliteRoomStore::open(database)?;
    let candidate = <SqliteRoomStore as RoomRecoveryStorageV1>::inspect_recovery_candidate(
        &reopened, &room_id,
    )?
    .ok_or("recovery candidate returned no Room")?;
    let (recovery_ms, callbacks, checkpoint) = if !candidate.has_checkpoint() {
        // Keep cadence evidence usable at scales where the bounded operational
        // witness collection cannot yet qualify a checkpoint. This is an
        // explicit recovery limitation, not a reason to discard the measured
        // production snapshot writes.
        (0, 0, no_checkpoint_evidence())
    } else {
        let recovery_started = Instant::now();
        let execution = recover_room_from_storage_with_receipt(&reopened, &registry, &room_id)?
            .ok_or("recovery returned no Room")?;
        let recovery_ms = recovery_started.elapsed().as_millis();
        let receipt = execution.receipt();
        if receipt.tail_transition_records_delivered()
            != u64::try_from(candidate.tail_transition_count())?
        {
            return Err("recovery receipt and inspected checkpoint tail disagree".into());
        }
        let recovered = execution.into_trace();
        if recovered.head() != trace.head() {
            return Err("checkpoint recovery final Head differs from the current Head".into());
        }
        let callbacks = recovered.activity_callback_count();
        let checkpoint = read_checkpoint_evidence(database, &room_id, trace.head(), receipt)?;
        (recovery_ms, callbacks, checkpoint)
    };
    drop(reopened);
    let runner_scenarios = Scenarios {
        crash_restart: Scenario {
            status: "completed",
            source: "production_sqlite_reopen",
        },
        offline_runner: Scenario {
            status: "not_exercised",
            source: "driver_scope",
        },
        timer_delivery: Scenario {
            status: "not_exercised",
            source: "driver_scope",
        },
        backup: Scenario {
            status: "not_exercised",
            source: "driver_scope",
        },
        transfer: Scenario {
            status: "not_exercised",
            source: "driver_scope",
        },
    };
    let pass = transition_rows == count
        && u64::try_from(callbacks)? == checkpoint.tail_transition_records_delivered_to_core
        && matches!(
            checkpoint.recovery_execution_path,
            "checkpoint" | "checkpoint_v2"
        )
        && checkpoint.prefix_transition_range_reads == 0
        && checkpoint.prefix_transition_records_delivered_to_core == 0
        && checkpoint.witness_hash_exact
        && checkpoint.witness_head_exact
        && checkpoint.all_operational_witnesses_exact;
    Ok(Report {
        schema: "worldstream/room-history-qualification/sqlite-v1",
        source: "production_sqlite_core_storage",
        requested_transition_count: count,
        counters: Counters {
            models: count,
            invocations: count,
            attempts: count,
            transitions: transition_rows,
            frames: frame_rows,
        },
        history: History {
            head_room_seq: trace.head().room_seq().get(),
            warm_path_reads: 1,
            reducer_callbacks: callbacks,
            recovery_ms,
            context_bytes,
        },
        storage: Storage {
            db_bytes,
            wal_bytes,
            shm_bytes,
            rss_bytes: rss_bytes(),
            transitions: transition_rows,
            frames: frame_rows,
        },
        snapshots,
        checkpoint,
        scenarios: runner_scenarios,
        transfer_metadata_initialized: stream_metadata,
        // The counter Pack intentionally emits no observation frames for this
        // administration-only workload.  A zero frame count is measured
        // evidence, while transfer/backup and Runner scenarios remain
        // explicitly not exercised below.
        pass,
    })
}

fn main() -> Result<()> {
    let mut database = None;
    let mut output = None;
    let mut count = None;
    let mut stream_metadata = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--database" => database = Some(PathBuf::from(args.next().ok_or("missing database")?)),
            "--output" => output = Some(PathBuf::from(args.next().ok_or("missing output")?)),
            "--transition-count" => {
                count = Some(args.next().ok_or("missing transition count")?.parse()?)
            }
            "--stream-metadata" => stream_metadata = true,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let report = run(
        &database.ok_or("missing --database")?,
        count.ok_or("missing --transition-count")?,
        stream_metadata,
    )?;
    let bytes = serde_json::to_vec_pretty(&report)?;
    if bytes.len() > 64 * 1024 {
        return Err("report exceeded bound".into());
    }
    if let Some(path) = output {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    println!("{}", std::str::from_utf8(&bytes)?);
    Ok(())
}
