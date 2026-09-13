//! Deterministic setup for the package-bound 100,000-Transition recovery measurement.
//!
//! This binary is setup tooling, not the measured recovery implementation. It
//! advances one already-created Room through production Core authorization and
//! commit APIs, then removes disposable materializations so a fresh packaged
//! daemon must recover from a recent paired snapshot plus a non-empty tail.

use std::{
    collections::BTreeMap, env, fs, fs::OpenOptions, io::Write as _, path::PathBuf, str::FromStr,
    sync::Arc, time::Instant,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

use rusqlite::{Connection, params};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use worldstream_core::{
    AdministrationOperationIdentityV1, AuthorityV1, CapabilityBearerV1,
    CoreAdministrationIngressV1, CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1,
    IntegrityGenerationV1, MembershipChangeV1, PackViewerV1, PreparedRoomCommitV1,
    ResolutionStatusV1, RoomCommitResolutionV1, RoomId, ViewInputV1,
    authorize_core_administration_operation, builtin_counter_registry, commit_existing_room,
};
use worldstream_sqlite::SqliteRoomStore;

const REPORT_SCHEMA: &str = "worldstream/reference-snapshot-tail-fixture/v1";
const TRANSITION_KIND: &str = "alternating_authorized_membership_suspend_resume";
const TAIL_TRANSITIONS: u64 = 2;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    source_revision: &'static str,
    transition_kind: &'static str,
    requested_transition_count: u64,
    accepted_transition_count: u64,
    setup_elapsed_ms: u128,
    head_room_seq: u64,
    newest_snapshot_room_seq: u64,
    snapshot_lag_transitions: u64,
    tail_recovery_transition_count: usize,
    tail_recovery_elapsed_ms: u128,
    tail_recovery_activity_callbacks: usize,
    complete_head: serde_json::Value,
    projection_hash: String,
    final_membership_standing: &'static str,
}

struct Arguments {
    database: PathBuf,
    authority_secret: PathBuf,
    output: PathBuf,
    room_id: RoomId,
    member_id: worldstream_core::MemberId,
    transition_count: u64,
}

fn argument_value(
    arguments: &mut impl Iterator<Item = String>,
    name: &str,
) -> FixtureResult<String> {
    arguments
        .next()
        .ok_or_else(|| format!("missing value for {name}").into())
}

fn arguments() -> FixtureResult<Arguments> {
    let mut values = env::args().skip(1);
    let mut database = None;
    let mut authority_secret = None;
    let mut output = None;
    let mut room_id = None;
    let mut member_id = None;
    let mut transition_count = None;
    while let Some(name) = values.next() {
        let value = argument_value(&mut values, &name)?;
        match name.as_str() {
            "--database" => database = Some(PathBuf::from(value)),
            "--authority-secret-file" => authority_secret = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--room-id" => room_id = Some(RoomId::from_str(&value)?),
            "--member-id" => member_id = Some(worldstream_core::MemberId::from_str(&value)?),
            "--transition-count" => transition_count = Some(value.parse::<u64>()?),
            _ => return Err(format!("unknown argument {name}").into()),
        }
    }
    let transition_count = transition_count.ok_or("missing --transition-count")?;
    if transition_count < TAIL_TRANSITIONS || transition_count % 2 != 0 {
        return Err("transition count must be even and at least two".into());
    }
    Ok(Arguments {
        database: database.ok_or("missing --database")?,
        authority_secret: authority_secret.ok_or("missing --authority-secret-file")?,
        output: output.ok_or("missing --output")?,
        room_id: room_id.ok_or("missing --room-id")?,
        member_id: member_id.ok_or("missing --member-id")?,
        transition_count,
    })
}

fn deterministic_ulid(sequence: u64) -> FixtureResult<String> {
    let mut encoded = [b'0'; 8];
    let mut remaining = sequence;
    for byte in encoded.iter_mut().rev() {
        *byte = CROCKFORD[usize::try_from(remaining & 31)?];
        remaining >>= 5;
    }
    if remaining != 0 {
        return Err("fixture Transition identity space exhausted".into());
    }
    Ok(format!(
        "01ARZ3NDEKTSV4RRFF{}",
        std::str::from_utf8(&encoded)?
    ))
}

fn read_secret(path: &PathBuf) -> FixtureResult<[u8; 32]> {
    let bytes = fs::read(path)?;
    <[u8; 32]>::try_from(bytes).map_err(|_| "authority secret must contain exactly 32 bytes".into())
}

// Keep the setup transaction sequence linear so reviewers can audit the exact
// production authorization/commit and measured-boundary ordering in one place.
#[allow(clippy::too_many_lines)]
fn run(arguments: &Arguments) -> FixtureResult<Report> {
    let started = Instant::now();
    let store = SqliteRoomStore::open(&arguments.database)?;
    let registry = builtin_counter_registry()?;
    let initial = store
        .gateway_room_snapshot(&registry, &arguments.room_id)?
        .ok_or("fixture Room is absent")?;
    if initial.trace().head().room_seq().get() != 0 {
        return Err("fixture Room must begin at Genesis".into());
    }
    let mut frame_heads: BTreeMap<_, _> = initial.frame_heads().clone();
    let integrity_generation = initial.integrity_generation();
    drop(initial);
    let authenticated = store.authenticate_bearer(CapabilityBearerV1::from_bytes(read_secret(
        &arguments.authority_secret,
    )?))?;
    let authenticated_principal = authenticated.principal_id().clone();
    let presented = authenticated.into_presented();
    let authority = AuthorityV1::new(Arc::new(store.clone()));
    let mut trace =
        worldstream_core::recover_room_from_storage(&store, &registry, &arguments.room_id)?
            .ok_or("fixture Room is absent during recovery")?;

    for index in 1..=arguments.transition_count {
        let membership = trace
            .core_state()
            .membership(&arguments.member_id)
            .ok_or("fixture Membership disappeared")?
            .clone();
        let suspend = index % 2 == 1;
        let kind = if suspend {
            CoreProposedKindV1::Suspend
        } else {
            CoreProposedKindV1::Resume
        };
        let change = if suspend {
            MembershipChangeV1::suspend(membership)
        } else {
            MembershipChangeV1::resume(membership)
        };
        let request = CoreAdministrationRequestV1::new(
            arguments.room_id.clone(),
            AdministrationOperationIdentityV1 {
                authenticated_principal: authenticated_principal.clone(),
                versioned_operation_kind: worldstream_core::CORE_OPERATION_KIND.to_owned(),
                idempotency_key: format!("snapshot-tail-{index}"),
            },
            kind,
            trace.head().room_seq(),
            if suspend {
                "reference_snapshot_tail_suspend"
            } else {
                "reference_snapshot_tail_resume"
            },
            CoreChangeSetV1::one(change),
        )?;
        let checked_at = OffsetDateTime::now_utc().format(&Rfc3339)?.parse()?;
        let grant = match authorize_core_administration_operation(
            &authority, &store, &presented, &request, checked_at,
        )? {
            CoreAdministrationIngressV1::Authorized(grant) => *grant,
            _ => return Err("fixture administration did not receive a fresh grant".into()),
        };
        let recorded_at = OffsetDateTime::now_utc().format(&Rfc3339)?.parse()?;
        let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
            &trace,
            &request,
            recorded_at,
            deterministic_ulid(index)?.parse()?,
            IntegrityGenerationV1::new(integrity_generation.get())?,
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
        let outcome = commit_existing_room(&store, &mut trace, prepared);
        if !matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ) {
            return Err(format!("fixture Transition {index} was not newly committed").into());
        }
    }

    let membership = trace
        .core_state()
        .membership(&arguments.member_id)
        .ok_or("fixture Membership disappeared after setup")?;
    if membership.standing() != worldstream_core::MembershipStandingV1::Enabled {
        return Err("fixture Membership did not finish enabled".into());
    }
    let retained = registry.load_retained(trace.head().pack_digest())?;
    let viewer = PackViewerV1::Participant(arguments.member_id.clone());
    let view = retained.host().view(&ViewInputV1 {
        core: trace.core_state(),
        activity_state: trace.activity_state(),
        complete_head: trace.head(),
        viewer: &viewer,
    })?;
    let head_room_seq = trace.head().room_seq().get();
    drop(store);

    let connection = Connection::open(&arguments.database)?;
    let retained_snapshot_seq = i64::try_from(head_room_seq - TAIL_TRANSITIONS)?;
    connection.execute(
        "DELETE FROM room_snapshots WHERE room_id = ?1 AND room_seq > ?2",
        params![arguments.room_id.to_string(), retained_snapshot_seq],
    )?;
    connection.execute(
        "DELETE FROM room_materializations WHERE room_id = ?1",
        [arguments.room_id.to_string()],
    )?;
    let newest_snapshot_room_seq = u64::try_from(connection.query_row(
        "SELECT max(room_seq) FROM room_snapshots WHERE room_id = ?1",
        [arguments.room_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?)?;
    let snapshot_lag_transitions = head_room_seq
        .checked_sub(newest_snapshot_room_seq)
        .ok_or("snapshot is ahead of the Room Head")?;
    if snapshot_lag_transitions != TAIL_TRANSITIONS {
        return Err("fixture did not retain the exact non-empty snapshot tail".into());
    }

    // Measure the actual bounded recovery boundary after the fixture has
    // removed current materializations. This is intentionally separate from
    // setup time: it reports the reducer callbacks and elapsed time paid by a
    // cold process that loads the retained paired snapshot.
    drop(connection);
    let recovery_store = SqliteRoomStore::open(&arguments.database)?;
    let recovery_started = Instant::now();
    let recovered = worldstream_core::recover_room_from_storage(
        &recovery_store,
        &registry,
        &arguments.room_id,
    )?
    .ok_or("fixture Room is absent during measured recovery")?;
    let tail_recovery_transition_count = recovered.transition_count();
    let tail_recovery_elapsed_ms = recovery_started.elapsed().as_millis();
    let tail_recovery_activity_callbacks = recovered.activity_callback_count();
    drop(recovery_store);

    Ok(Report {
        schema: REPORT_SCHEMA,
        source_revision: option_env!("WORLDSTREAM_BUILD_REVISION").unwrap_or("unbound"),
        transition_kind: TRANSITION_KIND,
        requested_transition_count: arguments.transition_count,
        accepted_transition_count: head_room_seq,
        setup_elapsed_ms: started.elapsed().as_millis(),
        head_room_seq,
        newest_snapshot_room_seq,
        snapshot_lag_transitions,
        tail_recovery_transition_count,
        tail_recovery_elapsed_ms,
        tail_recovery_activity_callbacks,
        complete_head: serde_json::to_value(trace.head())?,
        projection_hash: view.projection_hash()?.to_string(),
        final_membership_standing: "enabled",
    })
}

fn main() -> FixtureResult<()> {
    let arguments = arguments()?;
    let output = arguments.output.clone();
    let report = run(&arguments)?;
    let bytes = serde_json::to_vec(&report)?;
    if bytes.len() > 64 * 1024 {
        return Err("fixture report exceeded its byte limit".into());
    }
    let temporary = output.with_extension("json.tmp");
    if output.exists() || temporary.exists() {
        return Err("fixture output path already exists".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, output)?;
    Ok(())
}
