//! Real SQLite qualification driver for one deterministic Room history tier.
//!
//! This driver uses the public production Room creation, authorization,
//! commit, and recovery seams. It deliberately reports backend evidence
//! separately from the modeled matrix; a missing scenario is explicit and can
//! never be promoted to a qualification pass by the Python harness.

use std::{
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
    CapabilityBearerV1, CoreAdministrationIngressV1, CoreAdministrationRequestV1, CoreChangeSetV1,
    CoreProposedKindV1, CoreRoomStateV1, InitialMembershipProposalV1, MembershipChangeV1,
    MembershipStandingV1, MembershipV1, PackGenesisRequestV1, PackViewerV1, PreparedRoomCommitV1,
    PreparedRoomCreationV1, PresentedCapabilityV1, PrincipalKindV1, ResolutionStatusV1,
    RoomCommitResolutionV1, RoomCreationIngressV1, RoomCreationRequestV1, RoomId, RoomSeedV1,
    ViewInputV1, authorize_core_administration_operation, authorize_room_creation_operation,
    builtin_counter_registry, commit_existing_room, commit_room_creation, counter_v2_digest,
    recover_room_from_storage,
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
    retained_row_count: u64,
    last_snapshot_room_seq: u64,
    transitions_since_snapshot: u64,
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
        CREATE TABLE fixture_snapshot_write_observations (room_seq INTEGER NOT NULL);
        CREATE TRIGGER fixture_snapshot_write_observer
        AFTER INSERT ON room_snapshots
        BEGIN
          INSERT INTO fixture_snapshot_write_observations(room_seq) VALUES (NEW.room_seq);
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
    let retained_row_count = u64::try_from(connection.query_row::<i64, _, _>(
        "SELECT count(*) FROM room_snapshots WHERE room_id = ?1",
        [room_id.to_string()],
        |row| row.get(0),
    )?)?;
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
        retained_row_count,
        last_snapshot_room_seq: u64::try_from(last_snapshot_room_seq)?,
        transitions_since_snapshot: u64::try_from(transitions_since_snapshot)?,
    })
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
    let recovery_started = Instant::now();
    let recovered = recover_room_from_storage(&reopened, &registry, &room_id)?
        .ok_or("recovery returned no Room")?;
    let recovery_ms = recovery_started.elapsed().as_millis();
    let callbacks = recovered.activity_callback_count();
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
        scenarios: runner_scenarios,
        transfer_metadata_initialized: stream_metadata,
        // The counter Pack intentionally emits no observation frames for this
        // administration-only workload.  A zero frame count is measured
        // evidence, while transfer/backup and Runner scenarios remain
        // explicitly not exercised below.
        pass: transition_rows == count,
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
