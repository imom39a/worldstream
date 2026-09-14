#![allow(clippy::panic)]

use std::{
    collections::BTreeMap, fmt::Display, process::Command, str::FromStr, sync::Arc, thread,
    time::Instant,
};

#[cfg(feature = "conformance-tracer")]
use postgres::{Client, NoTls};

use worldstream_core::{
    AccessModeV1, AdministrationOperationIdentityV1, CanonicalJsonV1, CompleteHeadV1,
    CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1, CoreRoomStateV1, CoreTraceV1,
    InitialMembershipProposalV1, MembershipChangeV1, MembershipStandingV1, MembershipV1,
    PackGenesisRequestV1, ParticipantActionRequestV1, ParticipantActionV1,
    PreparedAuthorityWitnessV1, PreparedRoomCommitV1, PreparedRoomCreationV1, PreparedRoomWriteV1,
    PrincipalKindV1, RecordedStimulusV1, ResolutionStatusV1, RoomCheckpointOperationalWitnessV1,
    RoomCheckpointOperationalWitnessV2, RoomCheckpointOperationalWitnessV3,
    RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationRequestV1, RoomRecoveryStorageV1,
    RoomSeedV1, TransitionId, builtin_counter_registry, commit_existing_room, counter_v2_digest,
    recover_room_from_storage_with_receipt,
};
use worldstream_postgres::conformance::{
    ConformanceResolutionKind, ConformanceResolveKind, run_vector,
};
use worldstream_postgres::{
    FixtureMigrationProvider, FixturePostgresStore, MigrationFailpoint, MigrationRecord,
    PostgresConnectionConfig, PostgresConnectionPath, PostgresFailpoint, PostgresHarnessEvidence,
    PostgresProfile, RUNTIME_VERIFICATION_STATEMENTS, migration_history, migration_sql,
    probe_from_environment, schema_contract_fingerprint, verify_migration_prefix,
    verify_runtime_migration_history,
};
#[cfg(feature = "conformance-tracer")]
use worldstream_postgres::{PostgresAdmin, PostgresRoomStore};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const SERVING_FENCE_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const SERVING_FENCE_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const SERVING_FENCE_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const FULL_RECOVERY_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FY0";
const FULL_RECOVERY_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FY1";
const FULL_RECOVERY_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FY2";
const MALFORMED_HEAD_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FX0";
const MALFORMED_HEAD_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FX1";
const MALFORMED_HEAD_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FX2";
const MALFORMED_REBUILD_HEAD_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FW0";
const MALFORMED_REBUILD_HEAD_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FW1";
const MALFORMED_REBUILD_HEAD_PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FW2";
const RECOVERY_SCALE_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FZ0";
const SNAPSHOT_CADENCE_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FZ1";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("fixture {value}: {error}"))
}

fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(bytes).unwrap_or_else(|error| panic!("canonical fixture: {error}"))
}

fn creation_fixture_for(
    room_id: &str,
    member_id: &str,
    principal_id: &str,
    initial_value: u32,
    idempotency_key: &str,
) -> (
    worldstream_core::PreparedNewRoomGenesisV1,
    RoomCreationRequestV1,
    PreparedAuthorityWitnessV1,
    AdministrationOperationIdentityV1,
) {
    let registry =
        builtin_counter_registry().unwrap_or_else(|error| panic!("Counter registry: {error}"));
    let member = MembershipV1::new(
        parsed(member_id),
        parsed(principal_id),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .unwrap_or_else(|error| panic!("membership: {error}"));
    let configuration =
        canonical(format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes());
    let genesis = registry
        .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
            room_id: parsed(room_id),
            pack_digest: counter_v2_digest(),
            configuration: configuration.clone(),
            room_seed: parsed::<RoomSeedV1>(SEED),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: CoreRoomStateV1::active([member])
                .unwrap_or_else(|error| panic!("Core: {error}")),
        })
        .unwrap_or_else(|error| panic!("Genesis: {error}"));
    let request = RoomCreationRequestV1::new(
        counter_v2_digest(),
        configuration,
        vec![
            InitialMembershipProposalV1::new(
                parsed(principal_id),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".to_owned()),
            )
            .unwrap_or_else(|error| panic!("proposal: {error}")),
        ],
    );
    let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "postgres-fixture-authority",
        parsed(principal_id),
        1,
        &canonical(br#"{"scope":"create_room","revoked":false}"#),
    )
    .unwrap_or_else(|error| panic!("authority: {error}"));
    let identity = AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(principal_id),
        versioned_operation_kind: worldstream_core::CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: idempotency_key.to_owned(),
    };
    (genesis, request, witness, identity)
}

fn creation_fixture(
    initial_value: u32,
    idempotency_key: &str,
) -> (
    worldstream_core::PreparedNewRoomGenesisV1,
    RoomCreationRequestV1,
    PreparedAuthorityWitnessV1,
    AdministrationOperationIdentityV1,
) {
    creation_fixture_for(ROOM, MEMBER, PRINCIPAL, initial_value, idempotency_key)
}

fn prepared_creation(initial_value: u32, key: &str) -> PreparedRoomWriteV1 {
    let (genesis, request, witness, identity) = creation_fixture(initial_value, key);
    PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        identity, &request, witness, genesis,
    )
    .unwrap_or_else(|error| panic!("creation plan: {error}"))
    .into()
}

fn prepared_increment(
    trace: &CoreTraceV1,
    action_id: &str,
    transition_id: &str,
) -> PreparedRoomCommitV1 {
    prepared_increment_with_frame_head(trace, action_id, transition_id, 0)
}

fn prepared_increment_with_frame_head(
    trace: &CoreTraceV1,
    action_id: &str,
    transition_id: &str,
    previous_frame_head: u64,
) -> PreparedRoomCommitV1 {
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(action_id),
        trace.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    let definition = trace
        .retained_pack()
        .unwrap_or_else(|| panic!("retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|definition| definition.action_type == "increment")
        .unwrap_or_else(|| panic!("increment descriptor"));
    let transition = trace
        .prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(action_id),
            action_type: "increment".to_owned(),
            payload_schema_digest: definition.payload_schema.schema_digest.clone(),
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| panic!("prepare increment: {error}"));
    PreparedRoomCommitV1::for_action_for_conformance(
        trace,
        &request,
        transition,
        parsed::<TransitionId>(transition_id),
        worldstream_core::IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| panic!("integrity: {error}")),
        PreparedAuthorityWitnessV1::mint_for_conformance(
            "postgres-fixture-authority",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"action","revoked":false}"#),
        )
        .unwrap_or_else(|error| panic!("authority: {error}")),
        &BTreeMap::from([(parsed(MEMBER), previous_frame_head)]),
    )
    .unwrap_or_else(|error| panic!("seal increment: {error}"))
}

#[cfg(feature = "conformance-tracer")]
fn recovery_scale_transition_id(index: u64) -> TransitionId {
    let mut suffix = [b'0'; 8];
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = index;
    for byte in suffix.iter_mut().rev() {
        *byte = alphabet[usize::try_from(value & 31)
            .unwrap_or_else(|_| panic!("scale transition index exceeds platform capacity"))];
        value >>= 5;
    }
    parsed(&format!(
        "01ARZ3NDEKTSV4RRFF{}",
        std::str::from_utf8(&suffix)
            .unwrap_or_else(|_| panic!("scale transition suffix must be UTF-8"))
    ))
}

#[cfg(feature = "conformance-tracer")]
fn current_rss_bytes() -> Option<u64> {
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

#[cfg(feature = "conformance-tracer")]
fn requested_recovery_scale_tiers() -> Vec<u64> {
    let Some(value) = std::env::var_os("WORLDSTREAM_POSTGRES_RECOVERY_SCALES") else {
        return Vec::new();
    };
    let tiers = value
        .to_string_lossy()
        .split(',')
        .map(|tier| {
            tier.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid WORLDSTREAM_POSTGRES_RECOVERY_SCALES tier"))
        })
        .collect::<Vec<_>>();
    assert!(
        tiers == vec![1_000, 10_000] || tiers == vec![1_000, 10_000, 100_000],
        "the direct scale lane accepts 1k/10k, with 100k supplied by the transfer-backed lane"
    );
    tiers
}

/// Runs the production PostgreSQL store through a long alternating Core
/// administration lineage.  The Counter fixture intentionally has a small
/// value domain, so alternating Suspend/Resume provides accepted transitions
/// without weakening the real retained-pack schema limits.
#[cfg(feature = "conformance-tracer")]
#[allow(clippy::too_many_lines)]
fn qualify_bounded_recovery_scales(
    admin_dsn: &str,
    runtime: &PostgresRoomStore,
    runtime_client: &mut Client,
    tiers: &[u64],
    room_id: &str,
) {
    if tiers.is_empty() {
        return;
    }

    // The audit relation is disposable test instrumentation.  The trigger
    // fires only when the production snapshot transaction advances the
    // durable last-snapshot sequence, so ordinary cadence-count updates are
    // not mistaken for snapshot writes.  PostgreSQL exposes WAL positions but
    // no portable per-transaction CPU counter; both limitations are reported
    // explicitly below rather than converted into fabricated attribution.
    let mut audit_admin = Client::connect(admin_dsn, NoTls)
        .unwrap_or_else(|error| panic!("snapshot audit admin connection: {error}"));
    audit_admin
        .batch_execute(
            r#"
            DROP TRIGGER IF EXISTS worldstream_imo223_snapshot_audit_trigger
                ON worldstream_room_snapshot_schedules;
            DROP FUNCTION IF EXISTS worldstream_imo223_snapshot_audit_fn();
            DROP TABLE IF EXISTS worldstream_imo223_snapshot_audit;
            CREATE TABLE worldstream_imo223_snapshot_audit (
                audit_id bigserial PRIMARY KEY,
                room_id text NOT NULL,
                room_seq bigint NOT NULL,
                serialized_bytes bigint NOT NULL,
                wal_lsn pg_lsn NOT NULL,
                captured_at timestamptz NOT NULL
            );
            CREATE FUNCTION worldstream_imo223_snapshot_audit_fn()
            RETURNS trigger
            LANGUAGE plpgsql
            SECURITY DEFINER
            SET search_path = pg_catalog, public
            AS $function$
            BEGIN
                INSERT INTO public.worldstream_imo223_snapshot_audit(
                    room_id, room_seq, serialized_bytes, wal_lsn, captured_at
                )
                SELECT NEW.room_id,
                       NEW.last_snapshot_room_seq,
                       octet_length(snapshot.complete_head_bytes) +
                         octet_length(snapshot.core_state_bytes) +
                         octet_length(snapshot.activity_state_bytes),
                       pg_current_wal_lsn(),
                       clock_timestamp()
                  FROM public.worldstream_room_snapshots AS snapshot
                 WHERE snapshot.room_id = NEW.room_id
                   AND snapshot.room_seq = NEW.last_snapshot_room_seq;
                RETURN NEW;
            END
            $function$;
            CREATE TRIGGER worldstream_imo223_snapshot_audit_trigger
            AFTER UPDATE OF last_snapshot_room_seq ON worldstream_room_snapshot_schedules
            FOR EACH ROW
            WHEN (OLD.last_snapshot_room_seq IS DISTINCT FROM NEW.last_snapshot_room_seq)
            EXECUTE FUNCTION worldstream_imo223_snapshot_audit_fn();
            GRANT SELECT ON worldstream_imo223_snapshot_audit TO PUBLIC;
            "#,
        )
        .unwrap_or_else(|error| panic!("install snapshot audit trigger: {error}"));

    let (genesis, request, creation_witness, identity) =
        creation_fixture_for(room_id, MEMBER, PRINCIPAL, 0, "live-recovery-scale");
    let (trace_genesis, _, _, _) =
        creation_fixture_for(room_id, MEMBER, PRINCIPAL, 0, "live-recovery-scale");
    runtime
        .seed_conformance_authority(&creation_witness, true)
        .unwrap_or_else(|error| panic!("scale creation authority seed: {error:?}"));
    let administration_witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "postgres-recovery-scale-administration",
        parsed(PRINCIPAL),
        1,
        &canonical(br#"{"scope":"core_administration","revoked":false}"#),
    )
    .unwrap_or_else(|error| panic!("scale administration authority: {error}"));
    runtime
        .seed_conformance_authority(&administration_witness, true)
        .unwrap_or_else(|error| panic!("scale administration authority seed: {error:?}"));
    let creation = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        identity,
        &request,
        creation_witness,
        genesis,
    )
    .unwrap_or_else(|error| panic!("scale creation plan: {error}"));
    assert!(matches!(
        runtime.commit(&PreparedRoomWriteV1::from(creation)),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let mut trace = CoreTraceV1::create_from_retained_for_conformance(trace_genesis)
        .unwrap_or_else(|error| panic!("scale trace: {error}"));
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| panic!("scale recovery registry: {error}"));
    let mut transitions_since_fixture_reset = 0_u64;

    for sequence in 1..=*tiers
        .last()
        .unwrap_or_else(|| panic!("scale tiers are nonempty"))
    {
        let membership = trace
            .core_state()
            .membership(&parsed(MEMBER))
            .unwrap_or_else(|| panic!("scale Member disappeared"))
            .clone();
        let suspend = sequence % 2 == 1;
        let request = CoreAdministrationRequestV1::new(
            parsed(room_id),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: worldstream_core::CORE_OPERATION_KIND.to_owned(),
                idempotency_key: format!("live-recovery-scale-{sequence}"),
            },
            if suspend {
                CoreProposedKindV1::Suspend
            } else {
                CoreProposedKindV1::Resume
            },
            trace.head().room_seq(),
            "bounded_recovery_qualification",
            CoreChangeSetV1::one(if suspend {
                MembershipChangeV1::suspend(membership)
            } else {
                MembershipChangeV1::resume(membership)
            }),
        )
        .unwrap_or_else(|error| panic!("scale administration request {sequence}: {error}"));
        let frame_heads = trace
            .core_state()
            .memberships()
            .keys()
            .cloned()
            .map(|member_id| (member_id, 0))
            .collect();
        if tiers.contains(&sequence) {
            // This qualification measures cold recovery at three exact cuts,
            // not the cost of producing 400 redundant growing cache rows.
            // Keep ordinary production commits and checkpoint persistence, but
            // make only the requested tier commits cadence-due. The schedule
            // row is disposable cache metadata and is not canonical history.
            runtime_client
                .execute(
                    "UPDATE worldstream_room_snapshot_schedules \
                     SET transitions_since_snapshot = 249, active_started_at = NULL \
                     WHERE room_id = $1",
                    &[&room_id],
                )
                .unwrap_or_else(|error| panic!("arm scale checkpoint at {sequence}: {error}"));
        }
        let prepared = PreparedRoomCommitV1::for_core_administration_for_conformance(
            &trace,
            &request,
            parsed("2026-08-15T12:00:00Z"),
            recovery_scale_transition_id(sequence),
            worldstream_core::IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("scale integrity generation: {error}")),
            administration_witness.clone(),
            &frame_heads,
        )
        .unwrap_or_else(|error| panic!("scale administration plan {sequence}: {error}"));
        let outcome = commit_existing_room(runtime, &mut trace, prepared);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                ..
            }
        ));

        if !tiers.contains(&sequence) {
            transitions_since_fixture_reset += 1;
            if transitions_since_fixture_reset == 249 {
                runtime_client
                    .execute(
                        "UPDATE worldstream_room_snapshot_schedules \
                         SET transitions_since_snapshot = 0, active_started_at = NULL \
                         WHERE room_id = $1",
                        &[&room_id],
                    )
                    .unwrap_or_else(|error| {
                        panic!("suppress intermediate scale checkpoint at {sequence}: {error}")
                    });
                transitions_since_fixture_reset = 0;
            }
            continue;
        }
        transitions_since_fixture_reset = 0;
        let candidate =
            RoomRecoveryStorageV1::inspect_recovery_candidate(runtime, &parsed(room_id))
                .unwrap_or_else(|error| panic!("scale bounded candidate {sequence}: {error:?}"))
                .unwrap_or_else(|| panic!("scale Room disappeared at {sequence}"));
        assert!(candidate.has_checkpoint());
        assert!(candidate.tail_transition_count() <= 250);
        assert_eq!(candidate.tail_transition_count(), 0);

        let recovery_started = Instant::now();
        let execution =
            recover_room_from_storage_with_receipt(runtime, &registry, &parsed(room_id))
                .unwrap_or_else(|error| panic!("scale recovery {sequence}: {error:?}"))
                .unwrap_or_else(|| panic!("scale Room disappeared during recovery {sequence}"));
        let recovery_ms = recovery_started.elapsed().as_millis();
        let receipt = execution.receipt();
        assert!(receipt.used_checkpoint());
        assert_eq!(
            receipt.checkpoint_room_seq().map(|value| value.get()),
            Some(sequence)
        );
        assert_eq!(receipt.prefix_transition_records_delivered(), 0);
        assert_eq!(receipt.prefix_transitions_skipped(), sequence);
        assert_eq!(
            receipt.tail_transition_records_delivered(),
            u64::try_from(candidate.tail_transition_count())
                .unwrap_or_else(|_| panic!("scale tail exceeds u64"))
        );
        let recovered = execution.into_trace();
        assert_eq!(recovered.head(), trace.head());
        assert_eq!(
            recovered
                .core_state()
                .canonical_bytes()
                .unwrap_or_else(|error| panic!("scale recovered Core bytes: {error}")),
            trace
                .core_state()
                .canonical_bytes()
                .unwrap_or_else(|error| panic!("scale expected Core bytes: {error}"))
        );
        assert_eq!(
            recovered
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| panic!("scale recovered Activity bytes: {error}")),
            trace
                .activity_state()
                .to_bytes()
                .unwrap_or_else(|error| panic!("scale expected Activity bytes: {error}"))
        );
        assert_eq!(
            recovered.activity_callback_count(),
            usize::try_from(receipt.tail_transition_records_delivered())
                .unwrap_or_else(|_| panic!("scale tail exceeds platform capacity"))
        );

        let checkpoint_row = runtime_client
            .query_one(
                "SELECT room_seq, complete_head_bytes, witness_schema_version, witness_hash, witness_bytes FROM (\
                   SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_schema_version, witness.witness_hash, witness.witness_bytes, 3 AS witness_version \
                   FROM worldstream_room_snapshots AS snapshots \
                   JOIN worldstream_room_snapshot_operational_witnesses_v3 AS witness \
                     ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq \
                   WHERE snapshots.room_id = $1 \
                   UNION ALL \
                   SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_schema_version, witness.witness_hash, witness.witness_bytes, 2 AS witness_version \
                   FROM worldstream_room_snapshots AS snapshots \
                   JOIN worldstream_room_snapshot_operational_witnesses_v2 AS witness \
                     ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq \
                   WHERE snapshots.room_id = $1 \
                   UNION ALL \
                   SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_schema_version, witness.witness_hash, witness.witness_bytes, 1 AS witness_version \
                   FROM worldstream_room_snapshots AS snapshots \
                   JOIN worldstream_room_snapshot_operational_witnesses AS witness \
                     ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq \
                   WHERE snapshots.room_id = $1 \
                 ) AS checkpoint ORDER BY room_seq DESC, witness_version DESC LIMIT 1",
                &[&room_id],
            )
            .unwrap_or_else(|error| panic!("scale checkpoint witness {sequence}: {error}"));
        let checkpoint_seq: i64 = checkpoint_row.get(0);
        let checkpoint_head_bytes: Vec<u8> = checkpoint_row.get(1);
        let witness_schema: String = checkpoint_row.get(2);
        let witness_hash: Vec<u8> = checkpoint_row.get(3);
        let witness_bytes: Vec<u8> = checkpoint_row.get(4);
        let checkpoint_head =
            CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&checkpoint_head_bytes)
                .unwrap_or_else(|error| panic!("scale checkpoint Head {sequence}: {error}"));
        assert_eq!(u64::try_from(checkpoint_seq).ok(), Some(sequence));
        assert_eq!(&checkpoint_head, trace.head());
        assert_eq!(
            witness_hash.as_slice(),
            worldstream_core::Blake3DigestV1::hash(&witness_bytes).as_bytes()
        );
        let (
            timer_witness_entries,
            frame_witness_entries,
            consequence_witness_entries,
            activation_witness_entries,
            membership_generations,
            observation_frame_heads,
        ) = if witness_schema == worldstream_core::CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V3 {
            let witness = RoomCheckpointOperationalWitnessV3::from_canonical_bytes(
                &witness_bytes,
                &checkpoint_head,
            )
            .unwrap_or_else(|error| panic!("scale V3 operational witness {sequence}: {error:?}"));
            assert_eq!(witness.checkpoint_head(), trace.head());
            assert_eq!(
                witness.canonical_bytes().unwrap_or_else(|error| {
                    panic!("scale V3 canonical witness {sequence}: {error:?}")
                }),
                witness_bytes
            );
            let root_entries = |domain| {
                usize::try_from(
                    witness
                        .operational_history_roots()
                        .get(domain)
                        .unwrap_or_else(|| panic!("scale V3 root {domain} missing"))
                        .entry_count(),
                )
                .unwrap_or_else(|_| panic!("scale V3 root {domain} exceeds platform capacity"))
            };
            (
                witness.timers().len(),
                root_entries("frames"),
                root_entries("consequences"),
                root_entries("activation_decisions"),
                witness.membership_generations().clone(),
                witness.observation_frame_heads().clone(),
            )
        } else if witness_schema == worldstream_core::CHECKPOINT_OPERATIONAL_WITNESS_SCHEMA_V2 {
            let witness = RoomCheckpointOperationalWitnessV2::from_canonical_bytes(
                &witness_bytes,
                &checkpoint_head,
            )
            .unwrap_or_else(|error| panic!("scale V2 operational witness {sequence}: {error:?}"));
            assert_eq!(witness.checkpoint_head(), trace.head());
            assert_eq!(
                witness.canonical_bytes().unwrap_or_else(|error| {
                    panic!("scale V2 canonical witness {sequence}: {error:?}")
                }),
                witness_bytes
            );
            let root_entries = |domain| {
                usize::try_from(
                    witness
                        .operational_history_roots()
                        .get(domain)
                        .unwrap_or_else(|| panic!("scale V2 root {domain} missing"))
                        .entry_count(),
                )
                .unwrap_or_else(|_| panic!("scale V2 root {domain} exceeds platform capacity"))
            };
            (
                witness.timers().len(),
                root_entries("frames"),
                root_entries("consequences"),
                root_entries("activation_decisions"),
                witness.membership_generations().clone(),
                witness.observation_frame_heads().clone(),
            )
        } else {
            let witness = RoomCheckpointOperationalWitnessV1::from_canonical_bytes(
                &witness_bytes,
                &checkpoint_head,
            )
            .unwrap_or_else(|error| panic!("scale operational witness {sequence}: {error:?}"));
            assert_eq!(witness.checkpoint_head(), trace.head());
            assert_eq!(
                witness.canonical_bytes().unwrap_or_else(|error| {
                    panic!("scale canonical witness {sequence}: {error:?}")
                }),
                witness_bytes
            );
            (
                witness.timers().len(),
                witness.observation_frames().len(),
                witness.observation_consequences().len(),
                witness.activation_decisions().len(),
                witness.membership_generations().clone(),
                witness.observation_frame_heads().clone(),
            )
        };

        let counts = runtime_client
            .query_one(
                "SELECT \
                   (SELECT count(*) FROM worldstream_transitions WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_timers WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_frames WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_observation_consequences WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_activation_decisions WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_members WHERE room_id = $1), \
                   (SELECT count(*) FROM worldstream_semantic_receipts WHERE room_id = $1)",
                &[&room_id],
            )
            .unwrap_or_else(|error| panic!("scale operational counts {sequence}: {error}"));
        let transition_rows: i64 = counts.get(0);
        let timer_rows: i64 = counts.get(1);
        let frame_rows: i64 = counts.get(2);
        let consequence_rows: i64 = counts.get(3);
        let activation_rows: i64 = counts.get(4);
        let membership_rows: i64 = counts.get(5);
        let semantic_receipt_rows: i64 = counts.get(6);
        assert_eq!(u64::try_from(transition_rows).ok(), Some(sequence));
        assert_eq!(
            usize::try_from(timer_rows).ok(),
            Some(timer_witness_entries)
        );
        assert_eq!(
            usize::try_from(frame_rows).ok(),
            Some(frame_witness_entries)
        );
        assert_eq!(
            usize::try_from(consequence_rows).ok(),
            Some(consequence_witness_entries)
        );
        assert_eq!(
            usize::try_from(activation_rows).ok(),
            Some(activation_witness_entries)
        );
        assert_eq!(
            usize::try_from(membership_rows).ok(),
            Some(membership_generations.len())
        );
        let membership_rows = runtime_client
            .query(
                "SELECT member_id, frame_head, membership_generation \
                 FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
                &[&room_id],
            )
            .unwrap_or_else(|error| panic!("scale membership witness rows {sequence}: {error}"));
        let membership_row_count = membership_rows.len();
        for membership in membership_rows {
            let member_id: String = membership.get(0);
            let frame_head: i64 = membership.get(1);
            let generation: i64 = membership.get(2);
            assert_eq!(
                membership_generations.get(&member_id),
                Some(&generation)
            );
            assert_eq!(
                observation_frame_heads.get(&parsed(&member_id)),
                Some(
                    &u64::try_from(frame_head)
                        .unwrap_or_else(|_| { panic!("scale frame Head cannot be negative") })
                )
            );
        }
        assert!(semantic_receipt_rows >= transition_rows);
        println!(
            "LIVE_POSTGRES_RECOVERY_SCALE={}",
            serde_json::json!({
                "history_transition_rows": transition_rows,
                "head_room_seq": sequence,
                "checkpoint_room_seq": checkpoint_seq,
                "checkpoint_tail_transition_count": candidate.tail_transition_count(),
                "recovery_execution_path": "checkpoint",
                "checkpoint_boundary_transition_records_read_by_adapter": 1,
                "prefix_transition_records_delivered_to_core": receipt.prefix_transition_records_delivered(),
                "tail_transition_records_delivered_to_core": receipt.tail_transition_records_delivered(),
                "prefix_transition_range_reads": 0,
                "prefix_transitions_skipped": receipt.prefix_transitions_skipped(),
                "transition_records_read_by_adapter_total": 1_u64
                    .checked_add(receipt.tail_transition_records_delivered())
                    .unwrap_or_else(|| panic!("scale Transition read count overflow")),
                "checkpoint_witness_bytes": witness_bytes.len(),
                "checkpoint_witness_under_16_mib": witness_bytes.len() <= 16 * 1024 * 1024,
                "reducer_callback_count": recovered.activity_callback_count(),
                "recovery_ms": recovery_ms,
                "rss_bytes": current_rss_bytes(),
                "state": {
                    "head_exact": true,
                    "core_exact": true,
                    "activity_exact": true,
                    "checkpoint_hash_exact": true,
                },
                "operational_witness": {
                    "timers": {"live_rows": timer_rows, "witness_entries": timer_witness_entries, "exact": true},
                    "frames": {"live_rows": frame_rows, "witness_entries": frame_witness_entries, "exact": true},
                    "consequences": {"live_rows": consequence_rows, "witness_entries": consequence_witness_entries, "exact": true},
                    "membership_generations": {"live_rows": membership_row_count, "witness_entries": membership_generations.len(), "exact": true},
                    "frame_heads": {"live_rows": membership_row_count, "witness_entries": observation_frame_heads.len(), "exact": true},
                    "activation_decisions": {"live_rows": activation_rows, "witness_entries": activation_witness_entries, "exact": true},
                },
                "semantic_receipts": {
                    "live_rows": semantic_receipt_rows,
                    "read_by_bounded_recovery": false,
                },
                "qualification_checkpoint_schedule": "requested_tiers_only",
            })
        );
    }
    // The audit relation is admin-owned disposable instrumentation. Read it
    // through that same direct-admin connection so cadence evidence does not
    // expand the runtime role's production grant surface.
    let audit_rows = audit_admin
        .query(
            r#"
                WITH ordered AS (
                    SELECT room_seq, serialized_bytes, captured_at,
                           pg_wal_lsn_diff(
                               wal_lsn,
                               lag(wal_lsn) OVER (ORDER BY audit_id)
                           )::bigint AS wal_bytes_since_prior_event
                      FROM worldstream_imo223_snapshot_audit
                     WHERE room_id = $1
                     ORDER BY audit_id
                )
                SELECT room_seq, serialized_bytes, captured_at::text,
                       COALESCE(wal_bytes_since_prior_event, 0)
                  FROM ordered
                 ORDER BY room_seq
            "#,
            &[&room_id],
        )
        .unwrap_or_else(|error| panic!("read snapshot cadence audit: {error:?}"));
    let snapshots = audit_rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "room_seq": row.get::<_, i64>(0),
                "serialized_bytes": row.get::<_, i64>(1),
                "captured_at": row.get::<_, String>(2),
                "wal_bytes_since_prior_snapshot_event": row.get::<_, i64>(3),
            })
        })
        .collect::<Vec<_>>();
    println!(
        "LIVE_POSTGRES_SNAPSHOT_CADENCE={}",
        serde_json::json!({
            "source": "production_postgresql_17",
            "requested_tiers": tiers,
            "cadence": {
                "transition_interval": 250,
                "time_interval": "5 minutes",
                "retained_rows": 3,
            },
            "snapshot_count": snapshots.len(),
            "snapshots": snapshots,
            "cpu_attribution": {
                "value": serde_json::Value::Null,
                "source": "postgres_standard_catalog_has_no_portable_per_snapshot_cpu_counter",
            },
            "wal_attribution": {
                "exact": false,
                "source": "pg_lsn_delta_between_snapshot_events_includes_intervening_canonical_writes",
            },
            "duplicate_and_concurrent_head": "covered_by_live_direct_same_identity_concurrency",
        })
    );
    audit_admin
        .batch_execute(
            "DROP TRIGGER IF EXISTS worldstream_imo223_snapshot_audit_trigger ON worldstream_room_snapshot_schedules;\
             DROP FUNCTION IF EXISTS worldstream_imo223_snapshot_audit_fn();\
             DROP TABLE IF EXISTS worldstream_imo223_snapshot_audit;",
        )
        .unwrap_or_else(|error| panic!("remove snapshot audit instrumentation: {error}"));
}

#[test]
fn profiles_and_pooler_path_are_explicit_and_manifest_stays_fail_closed() {
    let admin = PostgresConnectionConfig::direct_admin("host=localhost user=admin sslmode=require")
        .unwrap_or_else(|error| panic!("admin profile: {error}"));
    assert_eq!(admin.profile(), PostgresProfile::DirectAdmin);
    let runtime = PostgresConnectionConfig::runtime(
        "host=localhost user=runtime sslmode=require",
        PostgresConnectionPath::TransactionPool,
    )
    .unwrap_or_else(|error| panic!("runtime profile: {error}"));
    assert_eq!(runtime.profile(), PostgresProfile::RuntimeLeastPrivilege);
    assert_eq!(runtime.path(), PostgresConnectionPath::TransactionPool);
    assert!(matches!(
        PostgresConnectionConfig::runtime(
            "host=database.internal user=runtime",
            PostgresConnectionPath::Direct,
        ),
        Err(worldstream_postgres::PostgresConfigError::RemoteTlsRequired)
    ));
    assert!(migration_sql().contains("worldstream_operation_guards"));
    assert_eq!(
        probe_from_environment(),
        PostgresHarnessEvidence::UnavailableEnvironment
    );
}

#[test]
fn fixture_proves_create_duplicate_conflict_and_guarded_resolution_parity() {
    let store = FixturePostgresStore::default();
    let first = prepared_creation(0, "create-parity");
    let duplicate = prepared_creation(0, "create-parity");
    let changed = prepared_creation(1, "create-parity");
    let created = store.commit(&first);
    assert!(matches!(
        created,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let duplicate = store.commit(&duplicate);
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
            .unwrap_or_else(|| panic!("duplicate receipt"))
            .canonical_receipt_bytes(),
        created
            .stored_result()
            .unwrap_or_else(|| panic!("created receipt"))
            .canonical_receipt_bytes(),
    );
    let changed_result = store.commit(&changed);
    assert!(
        matches!(changed_result, RoomCommitResolutionV1::Conflict { .. }),
        "changed result: {changed_result:?}"
    );
    assert!(matches!(
        store.resolve(first.identity(), first.request_hash()),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
}

#[test]
fn provider_neutral_vector_records_exact_fixture_receipts() {
    let store = FixturePostgresStore::default();
    let observations = run_vector(
        &store,
        [
            prepared_creation(0, "vector-create"),
            prepared_creation(0, "vector-create"),
            prepared_creation(1, "vector-create"),
        ],
    );

    assert_eq!(observations.len(), 3);
    assert!(observations[0].identity_bytes.is_some());
    assert_eq!(
        observations[0].identity_bytes,
        observations[1].identity_bytes
    );
    assert_eq!(
        observations[0].identity_bytes,
        observations[2].identity_bytes
    );
    assert_eq!(
        observations[0].request_hash_bytes,
        observations[1].request_hash_bytes
    );
    assert_ne!(
        observations[0].request_hash_bytes,
        observations[2].request_hash_bytes
    );
    assert_eq!(
        observations[0].resolution,
        ConformanceResolutionKind::GenesisCreated
    );
    assert!(!observations[0].duplicate);
    assert_eq!(
        observations[0].resolved,
        ConformanceResolveKind::StoredResolution
    );
    assert_eq!(
        observations[0].receipt_bytes,
        observations[0].resolved_receipt_bytes
    );
    assert_eq!(
        observations[1].resolution,
        ConformanceResolutionKind::GenesisCreated
    );
    assert!(observations[1].duplicate);
    assert_eq!(observations[1].receipt_bytes, observations[0].receipt_bytes);
    assert_eq!(
        observations[2].resolution,
        ConformanceResolutionKind::Conflict
    );
    assert_eq!(observations[2].resolved, ConformanceResolveKind::Conflict);
}

/// Direct cadence-only PostgreSQL lane. This intentionally bypasses the
/// broader adapter/full-gate harness so snapshot scheduling evidence remains
/// independently reviewable when an unrelated live gate fails.
#[cfg(feature = "conformance-tracer")]
#[test]
fn live_postgres_snapshot_cadence_direct() {
    let (Some(admin_dsn), Some(runtime_dsn)) = (
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN"),
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN"),
    ) else {
        println!("LIVE_POSTGRES_SNAPSHOT_CADENCE_DIRECT=SKIP reason=dsn_unset");
        return;
    };
    let admin_dsn = admin_dsn.to_string_lossy();
    let runtime_dsn = runtime_dsn.to_string_lossy();
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(admin_dsn.as_ref())
            .unwrap_or_else(|error| panic!("snapshot cadence admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("snapshot cadence admin handle: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("snapshot cadence migration: {error}"));
    let runtime = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(runtime_dsn.as_ref(), PostgresConnectionPath::Direct)
            .unwrap_or_else(|error| panic!("snapshot cadence runtime config: {error}")),
    )
    .unwrap_or_else(|error| panic!("snapshot cadence runtime handle: {error}"));
    let mut runtime_client = Client::connect(&runtime_dsn, NoTls)
        .unwrap_or_else(|error| panic!("snapshot cadence runtime connection: {error}"));
    qualify_bounded_recovery_scales(
        &admin_dsn,
        &runtime,
        &mut runtime_client,
        &[1_000, 10_000],
        SNAPSHOT_CADENCE_ROOM,
    );
    println!("LIVE_POSTGRES_SNAPSHOT_CADENCE_DIRECT=PASS");
}

#[cfg(feature = "conformance-tracer")]
#[test]
fn provider_neutral_transcript_covers_existing_write_and_stale_reprepare() {
    let store = FixturePostgresStore::default();
    let creation = prepared_creation(0, "vector-existing");
    let (genesis, _, _, _) = creation_fixture(0, "vector-existing");
    let trace = CoreTraceV1::create_from_retained_for_conformance(genesis)
        .unwrap_or_else(|error| panic!("transcript trace: {error}"));
    let advance = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    );
    let stale = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC4",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ8",
    );
    let observations = run_vector(&store, [creation, advance.into(), stale.into()]);

    assert_eq!(observations.len(), 3);
    assert_eq!(
        observations[0].resolution,
        ConformanceResolutionKind::GenesisCreated
    );
    assert_eq!(
        observations[1].resolution,
        ConformanceResolutionKind::TransitionCommitted
    );
    assert_eq!(
        observations[1].resolved,
        ConformanceResolveKind::StoredResolution
    );
    assert_eq!(
        observations[2].resolution,
        ConformanceResolutionKind::Reprepare
    );
    assert_eq!(
        observations[2].resolved,
        ConformanceResolveKind::KnownAbsent
    );
    assert!(observations[1].receipt_bytes.is_some());
    assert_eq!(
        observations[1].receipt_bytes,
        observations[1].resolved_receipt_bytes
    );
}

#[test]
fn postgres_sqlstate_failure_classes_are_stable_and_retryable_classes_are_explicit() {
    use worldstream_postgres::PostgresStorageFailure;

    assert_eq!(
        PostgresStorageFailure::from_sqlstate("08006"),
        PostgresStorageFailure::Connection
    );
    assert_eq!(
        PostgresStorageFailure::from_sqlstate("40P01"),
        PostgresStorageFailure::Deadlock
    );
    assert_eq!(
        PostgresStorageFailure::from_sqlstate("55P03"),
        PostgresStorageFailure::LockTimeout
    );
    assert_eq!(
        PostgresStorageFailure::from_sqlstate("40001"),
        PostgresStorageFailure::Serialization
    );
    assert_eq!(
        PostgresStorageFailure::from_sqlstate("23505"),
        PostgresStorageFailure::Constraint
    );
    assert_eq!(
        PostgresStorageFailure::from_sqlstate("XX000"),
        PostgresStorageFailure::Driver
    );
}

#[test]
fn fixture_corrupt_receipt_fails_closed_on_guarded_resolution() {
    let store = FixturePostgresStore::default();
    let creation = prepared_creation(0, "corrupt-receipt");
    assert!(matches!(
        store.commit(&creation),
        RoomCommitResolutionV1::GenesisCreated { .. }
    ));
    assert!(store.corrupt_receipt(creation.identity()));
    assert!(matches!(
        store.resolve(creation.identity(), creation.request_hash()),
        worldstream_core::ResolveOutcomeV1::ResolutionUnavailable
    ));
}

#[cfg(feature = "conformance-tracer")]
#[test]
fn fixture_authority_drift_fails_closed_before_commit() {
    let store = FixturePostgresStore::default();
    let (genesis, request, witness, identity) = creation_fixture(0, "authority-drift");
    let prepared = PreparedRoomWriteV1::from(
        worldstream_core::PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity,
            &request,
            witness.clone(),
            genesis,
        )
        .unwrap_or_else(|error| panic!("creation plan: {error}")),
    );
    store
        .seed_conformance_authority(&witness, false)
        .unwrap_or_else(|error| panic!("seed authority: {error:?}"));
    assert!(matches!(
        store.commit(&prepared),
        RoomCommitResolutionV1::Fenced
    ));
    assert!(matches!(
        store.resolve(prepared.identity(), prepared.request_hash()),
        worldstream_core::ResolveOutcomeV1::KnownAbsent
    ));
}

#[test]
fn fixture_proves_advance_stale_head_disposition_and_unknown_resolution() {
    let store = FixturePostgresStore::default();
    let creation = prepared_creation(0, "advance-room");
    let (genesis, _, _, _) = creation_fixture(0, "advance-room");
    let trace = CoreTraceV1::create_from_retained_for_conformance(genesis)
        .unwrap_or_else(|error| panic!("trace: {error}"));
    assert!(matches!(
        store.commit(&creation),
        RoomCommitResolutionV1::GenesisCreated { .. }
    ));
    let winner = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    );
    let loser = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC4",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ8",
    );
    assert!(matches!(
        store.commit(&winner.into()),
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let snapshot = store
        .parity_snapshot(ROOM)
        .unwrap_or_else(|| panic!("fixture parity snapshot"));
    assert_eq!(
        snapshot.frame_count,
        usize::try_from(snapshot.frame_heads.values().sum::<u64>())
            .unwrap_or_else(|_| panic!("frame count exceeds platform capacity"))
    );
    assert!(snapshot.frame_heads.contains_key(MEMBER));
    let stale = store.commit(&loser.into());
    assert!(matches!(stale, RoomCommitResolutionV1::Reprepare));
    let _ = trace;
    assert_eq!(
        FixturePostgresStore::evidence(),
        PostgresHarnessEvidence::UnavailableEnvironment
    );
}

#[test]
fn fixture_contention_has_one_new_winner_and_one_existing_duplicate() {
    let store = Arc::new(FixturePostgresStore::default());
    let first = prepared_creation(0, "contention");
    let second = prepared_creation(0, "contention");
    let left = Arc::clone(&store);
    let right = Arc::clone(&store);
    let first_thread = thread::spawn(move || left.commit(&first));
    let second_thread = thread::spawn(move || right.commit(&second));
    let results = [
        first_thread
            .join()
            .unwrap_or_else(|_| panic!("first contention worker")),
        second_thread
            .join()
            .unwrap_or_else(|_| panic!("second contention worker")),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                RoomCommitResolutionV1::GenesisCreated {
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
            .filter(|result| matches!(
                result,
                RoomCommitResolutionV1::GenesisCreated {
                    status: ResolutionStatusV1::Existing,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn fixture_classifies_rollback_and_unknown_commit_without_blind_retry() {
    let store = FixturePostgresStore::default();
    let rollback = prepared_creation(0, "rollback");
    store.set_failpoint(Some(PostgresFailpoint::RollbackBeforeCommit));
    assert!(matches!(
        store.commit(&rollback),
        RoomCommitResolutionV1::RetryableKnownAbsent
    ));
    assert!(matches!(
        store.resolve(rollback.identity(), rollback.request_hash()),
        worldstream_core::ResolveOutcomeV1::KnownAbsent
    ));
    assert!(matches!(
        store.commit(&rollback),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));

    let unknown_store = FixturePostgresStore::default();
    let unknown = prepared_creation(0, "unknown");
    unknown_store.set_failpoint(Some(PostgresFailpoint::UnknownAfterCommit));
    assert!(matches!(
        unknown_store.commit(&unknown),
        RoomCommitResolutionV1::Indeterminate
    ));
    assert!(matches!(
        unknown_store.resolve(unknown.identity(), unknown.request_hash()),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    assert!(matches!(
        unknown_store.commit(&unknown),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
}

#[cfg(feature = "conformance-tracer")]
#[test]
fn storage_connection_failure_is_indeterminate_and_classified_without_retry() {
    let store = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            "host=127.0.0.1 port=1 user=worldstream dbname=worldstream connect_timeout=1",
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| panic!("local failure probe config: {error}")),
    )
    .unwrap_or_else(|error| panic!("local failure probe handle: {error}"));

    let prepared = prepared_creation(0, "storage-error-classification");
    assert!(matches!(
        store.commit(&prepared),
        RoomCommitResolutionV1::Indeterminate
    ));
    assert_eq!(
        store.last_failure(),
        Some(worldstream_postgres::PostgresStorageFailure::Connection)
    );
    assert!(matches!(
        store.resolve(prepared.identity(), prepared.request_hash()),
        worldstream_core::ResolveOutcomeV1::ResolutionUnavailable
    ));
}

fn complete_migration_records() -> Vec<MigrationRecord> {
    let fingerprint = schema_contract_fingerprint();
    migration_history()
        .into_iter()
        .map(|migration| MigrationRecord {
            version: migration.version,
            migration_id: migration.id.to_owned(),
            checksum: migration.checksum().as_bytes().to_vec(),
            logical_history_id: Some(worldstream_postgres::LOGICAL_HISTORY_ID.to_owned()),
            schema_contract_fingerprint: Some(fingerprint.as_bytes().to_vec()),
        })
        .collect()
}

#[test]
fn migration_identity_and_checksum_drift_fail_closed() {
    let mut identity_drift = complete_migration_records();
    identity_drift[0].migration_id = "0001-unreviewed".to_owned();
    assert!(matches!(
        verify_migration_prefix(&identity_drift),
        Err(worldstream_postgres::MigrationVerificationError::IdentityDrift { .. })
    ));

    let mut checksum_drift = complete_migration_records();
    checksum_drift[1].checksum[0] ^= 0xff;
    assert!(matches!(
        verify_runtime_migration_history(&checksum_drift),
        Err(worldstream_postgres::MigrationVerificationError::ChecksumDrift { version: 2 })
    ));
}

#[test]
fn unsupported_downgraded_and_mixed_histories_fail_closed() {
    let mut unsupported = complete_migration_records();
    unsupported.push(MigrationRecord::legacy(5, "0005-future", &[0; 32]));
    assert!(matches!(
        verify_migration_prefix(&unsupported),
        Err(
            worldstream_postgres::MigrationVerificationError::NonContiguous { version: 5 }
                | worldstream_postgres::MigrationVerificationError::Unsupported { version: 5 }
        )
    ));

    let downgraded = vec![MigrationRecord::legacy(
        2,
        worldstream_postgres::AUTHORITY_MIGRATION_ID,
        migration_history()[1].checksum().as_bytes(),
    )];
    assert!(matches!(
        verify_migration_prefix(&downgraded),
        Err(worldstream_postgres::MigrationVerificationError::NonContiguous { version: 2 })
    ));

    let mut mixed = complete_migration_records();
    mixed[1].logical_history_id = Some("worldstream-storage-v0".to_owned());
    assert!(matches!(
        verify_runtime_migration_history(&mixed),
        Err(worldstream_postgres::MigrationVerificationError::LogicalHistoryDrift { .. })
    ));
}

#[test]
fn runtime_verification_is_read_only_and_contains_no_ddl() {
    for statement in RUNTIME_VERIFICATION_STATEMENTS {
        let upper = statement.trim_start().to_ascii_uppercase();
        assert!(!upper.starts_with("CREATE"));
        assert!(!upper.starts_with("ALTER"));
        assert!(!upper.starts_with("DROP"));
        assert!(!upper.starts_with("TRUNCATE"));
    }
}

#[test]
fn interrupted_migration_restarts_without_duplicate_history() {
    let provider = FixtureMigrationProvider::default();
    provider.set_failpoint(Some(MigrationFailpoint::AfterMigrationBodyBeforeRecord));
    assert!(provider.migrate().is_err());
    assert!(provider.records().is_empty());

    let verified = provider
        .migrate()
        .unwrap_or_else(|error| panic!("restart migration: {error}"));
    assert!(verified.complete);
    assert!(verified.fingerprint_verified);
    assert_eq!(provider.records().len(), migration_history().len());
    assert_eq!(provider.body_attempts(), migration_history().len() + 1);
}

#[test]
fn kernel_conformance_migration_is_reviewed_and_forward_only() {
    assert_eq!(migration_history().len(), 23);
    assert_eq!(
        migration_history()[2].id,
        worldstream_postgres::KERNEL_CONFORMANCE_MIGRATION_ID
    );
    assert!(migration_sql().contains("worldstream_timers"));
    assert!(worldstream_postgres::MIGRATION_0003_SQL.contains("worldstream_activation_intents"));
    assert!(worldstream_postgres::MIGRATION_0003_SQL.contains("worldstream_room_snapshots"));
    assert!(
        worldstream_postgres::MIGRATION_0018_SQL
            .contains("worldstream_room_snapshot_operational_witnesses")
    );
    assert!(worldstream_postgres::MIGRATION_0003_SQL.contains("worldstream_semantic_receipts"));
    assert!(
        worldstream_postgres::MIGRATION_0004_SQL
            .contains("worldstream_semantic_receipts_by_transition")
    );
    assert!(worldstream_postgres::MIGRATION_0005_SQL.contains("worldstream_transfer_imports"));
    assert!(worldstream_postgres::MIGRATION_0005_SQL.contains("worldstream_transfer_chunks"));
    assert!(worldstream_postgres::MIGRATION_0006_SQL.contains("worldstream_transfer_target_fence"));
    assert!(worldstream_postgres::MIGRATION_0007_SQL.contains("worldstream_deployment_metadata"));
    assert!(worldstream_postgres::MIGRATION_0009_SQL.contains("worldstream_authority_principals"));
    assert!(
        worldstream_postgres::MIGRATION_0010_SQL.contains("worldstream_deployment_resource_blobs")
    );
    assert!(
        worldstream_postgres::MIGRATION_0010_SQL
            .contains("worldstream_retired_authority_fences_v1")
    );
    assert!(
        worldstream_postgres::MIGRATION_0011_SQL
            .contains("CREATE UNIQUE INDEX worldstream_deployment_resource_identity_global_v1")
    );
    assert!(
        worldstream_postgres::MIGRATION_0011_SQL
            .contains("public.worldstream_transfer_target_fence")
    );
    assert!(
        worldstream_postgres::MIGRATION_0019_SQL
            .contains("worldstream_room_operational_history_roots_v2")
    );
    assert!(
        worldstream_postgres::MIGRATION_0020_SQL
            .contains("worldstream_room_current_timers_v2")
    );
    assert!(
        worldstream_postgres::MIGRATION_0021_SQL
            .contains("worldstream_room_snapshot_operational_witnesses_v2")
    );
    assert!(
        worldstream_postgres::MIGRATION_0022_SQL
            .contains("worldstream_room_operational_mmr_receipts_v1")
    );
    assert!(
        worldstream_postgres::MIGRATION_0023_SQL
            .contains("worldstream_room_snapshot_operational_witnesses_v3")
    );
}

#[cfg(feature = "conformance-tracer")]
#[test]
#[allow(clippy::too_many_lines)]
fn live_serving_fence_rejects_corrupt_current_materialization_and_membership() {
    let (Some(admin_dsn), Some(runtime_dsn)) = (
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN"),
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN"),
    ) else {
        println!("LIVE_POSTGRES_SERVING_FENCE=SKIP reason=dsn_unset");
        return;
    };
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(admin_dsn.to_string_lossy())
            .unwrap_or_else(|error| panic!("serving-fence admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("serving-fence admin handle: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("serving-fence migration: {error}"));
    let runtime = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            runtime_dsn.to_string_lossy(),
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| panic!("serving-fence runtime config: {error}")),
    )
    .unwrap_or_else(|error| panic!("serving-fence runtime handle: {error}"));
    let (genesis, request, witness, identity) = creation_fixture_for(
        SERVING_FENCE_ROOM,
        SERVING_FENCE_MEMBER,
        SERVING_FENCE_PRINCIPAL,
        0,
        "serving-fence-current-materialization",
    );
    runtime
        .seed_conformance_authority(&witness, true)
        .unwrap_or_else(|error| panic!("serving-fence authority seed: {error:?}"));
    let prepared = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        identity, &request, witness, genesis,
    )
    .unwrap_or_else(|error| panic!("serving-fence creation plan: {error}"));
    assert!(matches!(
        runtime.commit(&prepared.into()),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let room = parsed(SERVING_FENCE_ROOM);
    assert!(
        runtime
            .current_room_serving_fence(&room)
            .unwrap_or_else(|error| panic!("valid serving fence: {error}"))
            .is_some()
    );

    let mut client = Client::connect(&admin_dsn.to_string_lossy(), NoTls)
        .unwrap_or_else(|error| panic!("serving-fence admin client: {error}"));
    let membership_bytes: Vec<u8> = client
        .query_one(
            "SELECT membership_bytes FROM worldstream_members WHERE room_id = $1 AND member_id = $2",
            &[&SERVING_FENCE_ROOM, &SERVING_FENCE_MEMBER],
        )
        .unwrap_or_else(|error| panic!("read serving membership: {error}"))
        .get(0);
    client
        .execute(
            "UPDATE worldstream_members SET membership_bytes = '{}'::bytea WHERE room_id = $1 AND member_id = $2",
            &[&SERVING_FENCE_ROOM, &SERVING_FENCE_MEMBER],
        )
        .unwrap_or_else(|error| panic!("corrupt serving membership: {error}"));
    assert!(matches!(
        runtime.current_room_serving_fence(&room),
        Err(worldstream_postgres::PostgresRoomVerificationError::Corrupt { .. })
    ));
    client
        .execute(
            "UPDATE worldstream_members SET membership_bytes = $1 WHERE room_id = $2 AND member_id = $3",
            &[&membership_bytes, &SERVING_FENCE_ROOM, &SERVING_FENCE_MEMBER],
        )
        .unwrap_or_else(|error| panic!("restore serving membership: {error}"));
    let core_state_bytes: Vec<u8> = client
        .query_one(
            "SELECT core_state_bytes FROM worldstream_materializations WHERE room_id = $1",
            &[&SERVING_FENCE_ROOM],
        )
        .unwrap_or_else(|error| panic!("read current materialization: {error}"))
        .get(0);
    client
        .execute(
            "UPDATE worldstream_materializations SET core_state_bytes = '{}'::bytea WHERE room_id = $1",
            &[&SERVING_FENCE_ROOM],
        )
        .unwrap_or_else(|error| panic!("corrupt current materialization: {error}"));
    assert!(matches!(
        runtime.current_room_serving_fence(&room),
        Err(worldstream_postgres::PostgresRoomVerificationError::Corrupt { .. })
    ));
    client
        .execute(
            "UPDATE worldstream_materializations SET core_state_bytes = $1 WHERE room_id = $2",
            &[&core_state_bytes, &SERVING_FENCE_ROOM],
        )
        .unwrap_or_else(|error| panic!("restore current materialization: {error}"));
    println!("LIVE_POSTGRES_SERVING_FENCE=PASS current-record+materialization+membership");
}

#[cfg(feature = "conformance-tracer")]
#[test]
#[allow(clippy::too_many_lines)]
fn live_full_recovery_corruption_quarantines_and_stale_head_is_fenced() {
    let (Some(admin_dsn), Some(runtime_dsn)) = (
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN"),
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN"),
    ) else {
        println!("LIVE_POSTGRES_RECOVERY_FALLBACK=SKIP reason=dsn_unset");
        return;
    };
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(admin_dsn.to_string_lossy())
            .unwrap_or_else(|error| panic!("recovery-fallback admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("recovery-fallback admin handle: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("recovery-fallback migration: {error}"));
    let runtime = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            runtime_dsn.to_string_lossy(),
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| panic!("recovery-fallback runtime config: {error}")),
    )
    .unwrap_or_else(|error| panic!("recovery-fallback runtime handle: {error}"));
    let (genesis, request, witness, identity) = creation_fixture_for(
        FULL_RECOVERY_ROOM,
        FULL_RECOVERY_MEMBER,
        FULL_RECOVERY_PRINCIPAL,
        0,
        "live-recovery-fallback",
    );
    runtime
        .seed_conformance_authority(&witness, true)
        .unwrap_or_else(|error| panic!("recovery-fallback authority: {error:?}"));
    let creation = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        identity, &request, witness, genesis,
    )
    .unwrap_or_else(|error| panic!("recovery-fallback creation plan: {error}"));
    assert!(matches!(
        runtime.commit(&PreparedRoomWriteV1::from(creation)),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));

    let (stale_genesis, _, _, _) = creation_fixture_for(
        FULL_RECOVERY_ROOM,
        FULL_RECOVERY_MEMBER,
        FULL_RECOVERY_PRINCIPAL,
        1,
        "live-recovery-fallback-stale",
    );
    let stale_trace = CoreTraceV1::create_from_retained_for_conformance(stale_genesis)
        .unwrap_or_else(|error| panic!("recovery-fallback stale trace: {error}"));
    assert!(matches!(
        worldstream_core::RoomRecoveryStorageV1::record_recovery_failure(
            &runtime,
            &parsed(FULL_RECOVERY_ROOM),
            stale_trace.head(),
            worldstream_core::IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("recovery-fallback generation: {error}")),
            worldstream_core::RecoveryIntegrityDispositionV1::Quarantined,
        ),
        Err(worldstream_core::RoomRecoveryErrorV1::ConcurrentChange)
    ));

    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| panic!("recovery-fallback registry: {error}"));
    let mut client = Client::connect(&admin_dsn.to_string_lossy(), NoTls)
        .unwrap_or_else(|error| panic!("recovery-fallback admin client: {error}"));

    assert_eq!(
        admin
            .corrupt_snapshot_cache_for_conformance(FULL_RECOVERY_ROOM)
            .unwrap_or_else(|error| panic!("corrupt disposable snapshot: {error}")),
        1
    );
    let snapshot_fallback = runtime
        .recover_room(&registry, FULL_RECOVERY_ROOM)
        .unwrap_or_else(|error| panic!("recover through corrupt snapshot fallback: {error}"))
        .unwrap_or_else(|| panic!("corrupt snapshot fallback returned no room"));
    assert_eq!(snapshot_fallback.head().room_seq().get(), 0);
    let integrity_after_snapshot_fallback = client
        .query_one(
            "SELECT integrity_status, integrity_generation \
             FROM worldstream_room_roots WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("read snapshot-fallback integrity: {error}"));
    assert_eq!(
        integrity_after_snapshot_fallback.get::<_, String>(0),
        "healthy"
    );
    assert_eq!(integrity_after_snapshot_fallback.get::<_, i64>(1), 1);

    client
        .execute(
            "DELETE FROM worldstream_room_snapshot_operational_witnesses WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("delete recovery-fallback snapshot witness: {error}"));
    client
        .execute(
            "DELETE FROM worldstream_room_snapshots WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("delete recovery-fallback snapshot: {error}"));
    client
        .execute(
            "DELETE FROM worldstream_room_snapshot_schedules WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("delete recovery-fallback snapshot schedule: {error}"));
    client
        .execute(
            "DELETE FROM worldstream_materializations WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("delete recovery-fallback materialization: {error}"));
    let rebuilt = runtime
        .recover_room(&registry, FULL_RECOVERY_ROOM)
        .unwrap_or_else(|error| panic!("recover missing materialization: {error}"))
        .unwrap_or_else(|| panic!("recover missing materialization returned no room"));
    let rebuilt_materialization = client
        .query_one(
            "SELECT core_state_bytes, activity_state_bytes \
             FROM worldstream_materializations WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("read rebuilt materialization: {error}"));
    assert_eq!(
        rebuilt_materialization.get::<_, Vec<u8>>(0),
        rebuilt
            .core_state()
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("encode rebuilt Core state: {error}"))
    );
    assert_eq!(
        rebuilt_materialization.get::<_, Vec<u8>>(1),
        rebuilt
            .activity_state()
            .to_bytes()
            .unwrap_or_else(|error| panic!("encode rebuilt Activity state: {error}"))
    );

    let genesis_bytes: Vec<u8> = client
        .query_one(
            "SELECT genesis_bytes FROM worldstream_genesis WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("read recovery-fallback Genesis: {error}"))
        .get(0);
    let malformed_genesis = b"{}".to_vec();
    client
        .execute(
            "UPDATE worldstream_genesis SET genesis_bytes = $1 WHERE room_id = $2",
            &[&malformed_genesis, &FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("corrupt recovery-fallback Genesis: {error}"));
    assert!(matches!(
        runtime.recover_room(&registry, FULL_RECOVERY_ROOM),
        Err(worldstream_core::RoomRecoveryErrorV1::Corrupt)
    ));
    let integrity = client
        .query_one(
            "SELECT integrity_status, integrity_generation FROM worldstream_room_roots WHERE room_id = $1",
            &[&FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("read recovery-fallback integrity: {error}"));
    assert_eq!(integrity.get::<_, String>(0), "quarantined");
    assert_eq!(integrity.get::<_, i64>(1), 2);
    client
        .execute(
            "UPDATE worldstream_genesis SET genesis_bytes = $1 WHERE room_id = $2",
            &[&genesis_bytes, &FULL_RECOVERY_ROOM],
        )
        .unwrap_or_else(|error| panic!("restore recovery-fallback Genesis: {error}"));

    let (malformed_genesis, malformed_request, malformed_witness, malformed_identity) =
        creation_fixture_for(
            MALFORMED_HEAD_ROOM,
            MALFORMED_HEAD_MEMBER,
            MALFORMED_HEAD_PRINCIPAL,
            0,
            "live-recovery-malformed-head",
        );
    runtime
        .seed_conformance_authority(&malformed_witness, true)
        .unwrap_or_else(|error| panic!("malformed-head authority: {error:?}"));
    let malformed_creation = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        malformed_identity,
        &malformed_request,
        malformed_witness,
        malformed_genesis,
    )
    .unwrap_or_else(|error| panic!("malformed-head creation plan: {error}"));
    assert!(matches!(
        runtime.commit(&PreparedRoomWriteV1::from(malformed_creation)),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    client
        .execute(
            "UPDATE worldstream_room_roots SET head_bytes = '{}'::bytea WHERE room_id = $1",
            &[&MALFORMED_HEAD_ROOM],
        )
        .unwrap_or_else(|error| panic!("corrupt recovery Head: {error}"));
    assert!(matches!(
        runtime.recover_room(&registry, MALFORMED_HEAD_ROOM),
        Err(worldstream_core::RoomRecoveryErrorV1::Corrupt)
    ));
    let malformed_integrity = client
        .query_one(
            "SELECT integrity_status, integrity_generation \
             FROM worldstream_room_roots WHERE room_id = $1",
            &[&MALFORMED_HEAD_ROOM],
        )
        .unwrap_or_else(|error| panic!("read malformed-head integrity: {error}"));
    assert_eq!(malformed_integrity.get::<_, String>(0), "quarantined");
    assert_eq!(malformed_integrity.get::<_, i64>(1), 2);

    println!(
        "LIVE_POSTGRES_RECOVERY_FALLBACK=PASS \
         corrupt-snapshot-full-fallback+missing-materialization-rebuild+malformed-head-quarantine+corrupt-full-history+stale-head-fence"
    );
}

#[cfg(feature = "conformance-tracer")]
#[test]
fn live_checkpoint_rebuild_malformed_head_quarantines() {
    let (Some(admin_dsn), Some(runtime_dsn)) = (
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN"),
        std::env::var_os("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN"),
    ) else {
        println!("LIVE_POSTGRES_REBUILD_MALFORMED_HEAD=SKIP reason=dsn_unset");
        return;
    };
    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(admin_dsn.to_string_lossy())
            .unwrap_or_else(|error| panic!("rebuild-malformed-head admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("rebuild-malformed-head admin handle: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("rebuild-malformed-head migration: {error}"));
    let runtime = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            runtime_dsn.to_string_lossy(),
            PostgresConnectionPath::Direct,
        )
        .unwrap_or_else(|error| panic!("rebuild-malformed-head runtime config: {error}")),
    )
    .unwrap_or_else(|error| panic!("rebuild-malformed-head runtime handle: {error}"));
    let (genesis, request, witness, identity) = creation_fixture_for(
        MALFORMED_REBUILD_HEAD_ROOM,
        MALFORMED_REBUILD_HEAD_MEMBER,
        MALFORMED_REBUILD_HEAD_PRINCIPAL,
        0,
        "live-rebuild-malformed-head",
    );
    runtime
        .seed_conformance_authority(&witness, true)
        .unwrap_or_else(|error| panic!("rebuild-malformed-head authority: {error:?}"));
    let creation = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        identity, &request, witness, genesis,
    )
    .unwrap_or_else(|error| panic!("rebuild-malformed-head creation plan: {error}"));
    assert!(matches!(
        runtime.commit(&PreparedRoomWriteV1::from(creation)),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let mut client = Client::connect(&admin_dsn.to_string_lossy(), NoTls)
        .unwrap_or_else(|error| panic!("rebuild-malformed-head admin client: {error}"));
    client
        .execute(
            "UPDATE worldstream_room_roots SET head_bytes = '{}'::bytea WHERE room_id = $1",
            &[&MALFORMED_REBUILD_HEAD_ROOM],
        )
        .unwrap_or_else(|error| panic!("corrupt checkpoint-rebuild Head: {error}"));
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| panic!("rebuild-malformed-head registry: {error}"));
    assert!(matches!(
        runtime.rebuild_verified_recovery_checkpoint(&registry, MALFORMED_REBUILD_HEAD_ROOM),
        Err(worldstream_core::RoomRecoveryErrorV1::Corrupt)
    ));
    let integrity = client
        .query_one(
            "SELECT integrity_status, integrity_generation \
             FROM worldstream_room_roots WHERE room_id = $1",
            &[&MALFORMED_REBUILD_HEAD_ROOM],
        )
        .unwrap_or_else(|error| panic!("read rebuild-malformed-head integrity: {error}"));
    assert_eq!(integrity.get::<_, String>(0), "quarantined");
    assert_eq!(integrity.get::<_, i64>(1), 2);
    println!("LIVE_POSTGRES_REBUILD_MALFORMED_HEAD=PASS exact-raw-fence+quarantine");
}

#[cfg(feature = "conformance-tracer")]
#[test]
#[allow(clippy::too_many_lines)]
fn live_direct_runtime_and_optional_pooler_conformance() {
    let Some(admin_dsn) = std::env::var_os("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN") else {
        println!("LIVE_POSTGRES=SKIP reason=admin_dsn_unset");
        return;
    };
    let Some(runtime_dsn) = std::env::var_os("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN") else {
        println!("LIVE_POSTGRES=SKIP reason=runtime_dsn_unset");
        return;
    };

    let admin = PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(admin_dsn.to_string_lossy())
            .unwrap_or_else(|error| panic!("live admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("live admin handle: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("live direct-admin migration: {error}"));
    admin
        .verify_schema()
        .unwrap_or_else(|error| panic!("live direct-admin schema verification: {error}"));
    println!("LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17");
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("live restart migration: {error}"));
    println!("LIVE_POSTGRES=PASS direct_admin=restart-idempotent");

    let runtime_config = PostgresConnectionConfig::runtime(
        runtime_dsn.to_string_lossy(),
        PostgresConnectionPath::Direct,
    )
    .unwrap_or_else(|error| panic!("live runtime config: {error}"));
    let runtime = PostgresRoomStore::new(runtime_config.clone())
        .unwrap_or_else(|error| panic!("live runtime handle: {error}"));
    runtime
        .verify_schema()
        .unwrap_or_else(|error| panic!("live runtime schema verification: {error}"));
    let (_, _, witness, _) = creation_fixture(0, "live-runtime-create");
    runtime
        .seed_conformance_authority(&witness, true)
        .unwrap_or_else(|error| panic!("live authority seed: {error:?}"));
    let mut runtime_client = Client::connect(&runtime_dsn.to_string_lossy(), NoTls)
        .unwrap_or_else(|error| panic!("live runtime permission probe: {error}"));
    assert!(
        runtime_client
            .batch_execute(
                "CREATE TABLE worldstream_runtime_must_not_create (probe integer NOT NULL)"
            )
            .is_err()
    );
    println!("LIVE_POSTGRES=PASS runtime_ddl=create_denied");

    let observations = run_vector(
        &runtime,
        [
            prepared_creation(0, "live-runtime-create"),
            prepared_creation(0, "live-runtime-create"),
            prepared_creation(1, "live-runtime-create"),
        ],
    );
    assert_eq!(observations.len(), 3);
    assert_eq!(
        observations[0].resolution,
        ConformanceResolutionKind::GenesisCreated
    );
    assert!(!observations[0].duplicate);
    assert_eq!(
        observations[1].resolution,
        ConformanceResolutionKind::GenesisCreated
    );
    assert!(observations[1].duplicate);
    assert_eq!(observations[1].receipt_bytes, observations[0].receipt_bytes);
    assert_eq!(
        observations[2].resolution,
        ConformanceResolutionKind::Conflict
    );
    assert_eq!(observations[2].resolved, ConformanceResolveKind::Conflict);
    let different_identity = prepared_creation(0, "live-runtime-create-second-identity");
    assert!(matches!(
        runtime.commit(&different_identity),
        RoomCommitResolutionV1::Reprepare
    ));
    println!("LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve");
    println!("LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare");

    let receipt_probe = prepared_creation(0, "live-runtime-create");
    assert!(matches!(
        runtime
            .read_guarded_receipt(receipt_probe.identity(), receipt_probe.request_hash())
            .unwrap_or_else(|error| panic!("live guarded receipt read: {error}")),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    let conflict_probe = prepared_creation(1, "live-runtime-create");
    assert!(matches!(
        runtime
            .read_guarded_receipt(conflict_probe.identity(), conflict_probe.request_hash())
            .unwrap_or_else(|error| panic!("live guarded receipt conflict read: {error}")),
        worldstream_core::ResolveOutcomeV1::Conflict { .. }
    ));
    println!("LIVE_POSTGRES=PASS runtime=direct guarded-receipt-read");

    let unknown_probe = prepared_creation(0, "live-runtime-unknown-resolution");
    let unknown_identity_bytes = unknown_probe
        .identity()
        .canonical_bytes()
        .unwrap_or_else(|error| panic!("live unknown identity bytes: {error}"));
    let before_unknown_guard: i64 = runtime_client
        .query_one(
            "SELECT count(*) FROM worldstream_operation_guards WHERE identity_bytes = $1",
            &[&unknown_identity_bytes],
        )
        .unwrap_or_else(|error| panic!("live unknown guard before lookup: {error}"))
        .get(0);
    assert!(matches!(
        runtime.resolve(unknown_probe.identity(), unknown_probe.request_hash()),
        worldstream_core::ResolveOutcomeV1::KnownAbsent
    ));
    let after_unknown_guard: i64 = runtime_client
        .query_one(
            "SELECT count(*) FROM worldstream_operation_guards WHERE identity_bytes = $1",
            &[&unknown_identity_bytes],
        )
        .unwrap_or_else(|error| panic!("live unknown guard after lookup: {error}"))
        .get(0);
    assert_eq!(before_unknown_guard, after_unknown_guard);
    println!("LIVE_POSTGRES=PASS runtime=direct unknown-resolve-is-read-only");

    let (genesis, _, _, _) = creation_fixture(0, "live-runtime-create");
    let trace = CoreTraceV1::create_from_retained_for_conformance(genesis)
        .unwrap_or_else(|error| panic!("live trace: {error}"));
    let action_witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "postgres-fixture-authority",
        parsed(PRINCIPAL),
        1,
        &canonical(br#"{"scope":"action","revoked":false}"#),
    )
    .unwrap_or_else(|error| panic!("live action authority: {error}"));
    runtime
        .seed_conformance_authority(&action_witness, true)
        .unwrap_or_else(|error| panic!("live action authority seed: {error:?}"));
    let direct_advance_left = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    );
    let direct_advance_right = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    );
    let direct_advance_left: PreparedRoomWriteV1 = direct_advance_left.into();
    let direct_advance_right: PreparedRoomWriteV1 = direct_advance_right.into();
    assert_eq!(
        direct_advance_left.identity(),
        direct_advance_right.identity()
    );
    let left_store = PostgresRoomStore::new(runtime_config.clone())
        .unwrap_or_else(|error| panic!("live left runtime handle: {error}"));
    let right_store = PostgresRoomStore::new(runtime_config.clone())
        .unwrap_or_else(|error| panic!("live right runtime handle: {error}"));
    let left = thread::spawn(move || left_store.commit(&direct_advance_left));
    let right = thread::spawn(move || right_store.commit(&direct_advance_right));
    let concurrent = [
        left.join()
            .unwrap_or_else(|_| panic!("live direct concurrency left worker")),
        right
            .join()
            .unwrap_or_else(|_| panic!("live direct concurrency right worker")),
    ];
    assert_eq!(
        concurrent
            .iter()
            .filter(|resolution| matches!(
                resolution,
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::New,
                    ..
                }
            ))
            .count(),
        1,
        "same-identity concurrency must have one durable winner: {concurrent:?}"
    );
    assert_eq!(
        concurrent
            .iter()
            .filter(|resolution| matches!(
                resolution,
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::Existing,
                    ..
                }
            ))
            .count(),
        1,
        "same-identity concurrency must have one idempotent duplicate: {concurrent:?}"
    );
    let row = runtime_client
        .query_one(
            "SELECT (SELECT count(*) FROM worldstream_transitions WHERE room_id = $1), (SELECT count(*) FROM worldstream_frames WHERE room_id = $1), (SELECT count(*) FROM worldstream_semantic_receipts WHERE room_id = $1)",
            &[&ROOM],
        )
        .unwrap_or_else(|error| panic!("live persisted advance rows: {error}"));
    let transition_count: i64 = row.get(0);
    let frame_count: i64 = row.get(1);
    let receipt_count: i64 = row.get(2);
    assert_eq!((transition_count, receipt_count), (1, 2));
    assert!(frame_count >= 1);
    let verified = runtime
        .verify_room(ROOM)
        .unwrap_or_else(|error| panic!("live Room verification: {error}"));
    assert_eq!(verified.transition_count, 1);
    assert_eq!(verified.member_count, 1);
    assert_eq!(verified.integrity_generation, 1);
    assert_eq!(verified.snapshots.len(), 1);
    assert_eq!(
        verified.snapshots.last().map(|snapshot| snapshot.room_seq),
        Some(0)
    );
    let cadence = runtime_client
        .query_one(
            "SELECT last_snapshot_room_seq, transitions_since_snapshot, active_started_at IS NOT NULL FROM worldstream_room_snapshot_schedules WHERE room_id = $1",
            &[&ROOM],
        )
        .unwrap_or_else(|error| panic!("live snapshot cadence row: {error}"));
    assert_eq!(
        (
            cadence.get::<_, i64>(0),
            cadence.get::<_, i64>(1),
            cadence.get::<_, bool>(2),
        ),
        (0, 1, true)
    );
    println!(
        "LIVE_POSTGRES=PASS runtime=direct advance+frame+semantic-receipt+snapshot-cache+same-identity-concurrency"
    );

    let stale = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC4",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ8",
    );
    let stale: PreparedRoomWriteV1 = stale.into();
    let stale_identity = stale.identity().clone();
    let stale_request_hash = stale.request_hash().clone();
    assert!(matches!(
        runtime.commit(&stale),
        RoomCommitResolutionV1::Reprepare
    ));
    assert!(matches!(
        runtime.resolve(&stale_identity, &stale_request_hash),
        worldstream_core::ResolveOutcomeV1::KnownAbsent
    ));
    println!("LIVE_POSTGRES=PASS runtime=direct stale-head=reprepare+known-absent");

    let rollback_duplicate = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    );
    let rollback_duplicate: PreparedRoomWriteV1 = rollback_duplicate.into();
    let rollback_identity = rollback_duplicate.identity().clone();
    let rollback_request_hash = rollback_duplicate.request_hash().clone();
    runtime.set_failpoint(Some(PostgresFailpoint::RollbackBeforeCommit));
    assert!(matches!(
        runtime.commit(&rollback_duplicate),
        RoomCommitResolutionV1::RetryableKnownAbsent
    ));
    assert!(matches!(
        runtime.resolve(&rollback_identity, &rollback_request_hash),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));

    // Build a distinct valid transition from the durable head. Reusing the
    // already committed operation identity would exercise idempotent receipt
    // lookup, not an uncertain post-commit result.
    let (post_genesis, _, _, _) = creation_fixture(0, "live-post-trace");
    let mut post_trace = CoreTraceV1::create_from_retained_for_conformance(post_genesis)
        .unwrap_or_else(|error| panic!("live post trace: {error}"));
    let post_action_id = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
    let post_definition = post_trace
        .retained_pack()
        .unwrap_or_else(|| panic!("live retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|definition| definition.action_type == "increment")
        .unwrap_or_else(|| panic!("live increment descriptor"));
    post_trace
        .advance_for_conformance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(post_action_id),
            action_type: "increment".to_owned(),
            payload_schema_digest: post_definition.payload_schema.schema_digest.clone(),
            canonical_payload: canonical(br"{}"),
            exact_basis_head: post_trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| panic!("live post trace advance: {error}"));
    let unknown = prepared_increment_with_frame_head(
        &post_trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC4",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ8",
        1,
    );
    let unknown: PreparedRoomWriteV1 = unknown.into();
    let unknown_identity = unknown.identity().clone();
    let unknown_request_hash = unknown.request_hash().clone();
    runtime
        .seed_conformance_authority(&action_witness, true)
        .unwrap_or_else(|error| panic!("live unknown authority refresh: {error:?}"));
    runtime.set_failpoint(Some(PostgresFailpoint::UnknownAfterCommit));
    let unknown_commit = runtime.commit(&unknown);
    assert!(
        matches!(unknown_commit, RoomCommitResolutionV1::Indeterminate),
        "unknown resolution: {:?}",
        runtime.resolve(&unknown_identity, &unknown_request_hash)
    );
    assert!(matches!(
        runtime.resolve(&unknown_identity, &unknown_request_hash),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    println!("LIVE_POSTGRES=PASS runtime=direct rollback+unknown-commit=guarded-resolution");

    let (_, _, fence_witness, _) = creation_fixture(0, "live-fence");
    runtime
        .seed_conformance_authority(&fence_witness, false)
        .unwrap_or_else(|error| panic!("live authority fence: {error:?}"));
    let fenced = prepared_creation(0, "live-fence");
    assert!(matches!(
        runtime.commit(&fenced),
        RoomCommitResolutionV1::Fenced
    ));
    assert!(matches!(
        runtime.resolve(fenced.identity(), fenced.request_hash()),
        worldstream_core::ResolveOutcomeV1::KnownAbsent
    ));
    println!("LIVE_POSTGRES=PASS runtime=direct authority-fence=known-absent");
    println!(
        "LIVE_POSTGRES=PASS normalized=create+commit+duplicate+conflict+stale+fence+rollback+unknown-resolution"
    );

    let bounded =
        <PostgresRoomStore as worldstream_core::RoomRecoveryStorageV1>::inspect_recovery_candidate(
            &runtime,
            &parsed(ROOM),
        )
        .unwrap_or_else(|error| panic!("live bounded recovery candidate: {error:?}"))
        .unwrap_or_else(|| panic!("live bounded recovery Room disappeared"));
    assert!(bounded.has_checkpoint());
    assert_eq!(bounded.tail_transition_count(), 2);
    let recovery_registry = builtin_counter_registry()
        .unwrap_or_else(|error| panic!("live recovery registry: {error}"));
    let recovered = runtime
        .recover_room(&recovery_registry, ROOM)
        .unwrap_or_else(|error| panic!("live bounded checkpoint recovery: {error:?}"))
        .unwrap_or_else(|| panic!("live bounded checkpoint Room disappeared"));
    assert_eq!(recovered.head().room_seq().get(), 2);
    assert!(recovered.activity_callback_count() > 0);

    let witness_row = runtime_client
        .query_one(
            "SELECT witness_hash, witness_bytes \
             FROM worldstream_room_snapshot_operational_witnesses_v3 \
             WHERE room_id = $1 AND room_seq = 0",
            &[&ROOM],
        )
        .unwrap_or_else(|error| panic!("read live checkpoint witness: {error}"));
    let witness_hash: Vec<u8> = witness_row.get(0);
    let witness_bytes: Vec<u8> = witness_row.get(1);
    let malformed_witness = br"{}".to_vec();
    let malformed_hash = worldstream_core::Blake3DigestV1::hash(&malformed_witness);
    runtime_client
        .execute(
            "UPDATE worldstream_room_snapshot_operational_witnesses_v3 \
             SET witness_hash = $1, witness_bytes = $2 \
             WHERE room_id = $3 AND room_seq = 0",
            &[
                &malformed_hash.as_bytes().as_slice(),
                &malformed_witness,
                &ROOM,
            ],
        )
        .unwrap_or_else(|error| panic!("tamper live checkpoint witness: {error}"));
    let fallback =
        <PostgresRoomStore as worldstream_core::RoomRecoveryStorageV1>::inspect_recovery_candidate(
            &runtime,
            &parsed(ROOM),
        )
        .unwrap_or_else(|error| panic!("live malformed-witness fallback: {error:?}"))
        .unwrap_or_else(|| panic!("live malformed-witness Room disappeared"));
    assert!(!fallback.has_checkpoint());
    assert_eq!(fallback.tail_transition_count(), 2);
    runtime_client
        .execute(
            "UPDATE worldstream_room_snapshot_operational_witnesses_v3 \
             SET witness_hash = $1, witness_bytes = $2 \
             WHERE room_id = $3 AND room_seq = 0",
            &[&witness_hash, &witness_bytes, &ROOM],
        )
        .unwrap_or_else(|error| panic!("restore live checkpoint witness: {error}"));

    let mut forged_witness: serde_json::Value = serde_json::from_slice(&witness_bytes)
        .unwrap_or_else(|error| panic!("decode live checkpoint witness: {error}"));
    assert_eq!(
        forged_witness["membership_generations"][MEMBER].as_i64(),
        Some(1)
    );
    forged_witness["membership_generations"][MEMBER] = serde_json::Value::from(2);
    let forged_json = serde_json::to_vec(&forged_witness)
        .unwrap_or_else(|error| panic!("serialize forged live checkpoint witness: {error}"));
    let forged_bytes = CanonicalJsonV1::parse(&forged_json)
        .unwrap_or_else(|error| panic!("canonicalize forged live checkpoint witness: {error}"))
        .to_bytes()
        .unwrap_or_else(|error| panic!("encode forged live checkpoint witness: {error}"));
    let forged_hash = worldstream_core::Blake3DigestV1::hash(&forged_bytes);
    runtime_client
        .execute(
            "UPDATE worldstream_room_snapshot_operational_witnesses_v3 \
             SET witness_hash = $1, witness_bytes = $2 \
             WHERE room_id = $3 AND room_seq = 0",
            &[&forged_hash.as_bytes().as_slice(), &forged_bytes, &ROOM],
        )
        .unwrap_or_else(|error| panic!("install forged live checkpoint witness: {error}"));
    let forged_candidate =
        <PostgresRoomStore as worldstream_core::RoomRecoveryStorageV1>::inspect_recovery_candidate(
            &runtime,
            &parsed(ROOM),
        )
        .unwrap_or_else(|error| panic!("inspect forged live checkpoint: {error:?}"))
        .unwrap_or_else(|| panic!("live forged-witness Room disappeared"));
    assert!(forged_candidate.has_checkpoint());
    let recovered_after_forgery = runtime
        .recover_room(&recovery_registry, ROOM)
        .unwrap_or_else(|error| panic!("live forged-witness full fallback: {error:?}"))
        .unwrap_or_else(|| panic!("live forged-witness fallback Room disappeared"));
    assert_eq!(recovered_after_forgery.head().room_seq().get(), 2);
    let integrity = runtime_client
        .query_one(
            "SELECT integrity_status, integrity_generation \
             FROM worldstream_room_roots WHERE room_id = $1",
            &[&ROOM],
        )
        .unwrap_or_else(|error| panic!("read post-fallback live integrity: {error}"));
    assert_eq!(integrity.get::<_, String>(0), "healthy");
    assert_eq!(integrity.get::<_, i64>(1), 1);
    runtime_client
        .execute(
            "UPDATE worldstream_room_snapshot_operational_witnesses_v3 \
             SET witness_hash = $1, witness_bytes = $2 \
             WHERE room_id = $3 AND room_seq = 0",
            &[&witness_hash, &witness_bytes, &ROOM],
        )
        .unwrap_or_else(|error| panic!("restore forged live checkpoint witness: {error}"));

    let stale_recovery_head = trace.head().clone();
    assert_eq!(stale_recovery_head.room_seq().get(), 0);
    assert!(matches!(
        worldstream_core::RoomRecoveryStorageV1::record_recovery_failure(
            &runtime,
            &parsed(ROOM),
            &stale_recovery_head,
            worldstream_core::IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| panic!("live integrity generation: {error}")),
            worldstream_core::RecoveryIntegrityDispositionV1::Quarantined,
        ),
        Err(worldstream_core::RoomRecoveryErrorV1::ConcurrentChange)
    ));
    let post_race_integrity = runtime_client
        .query_one(
            "SELECT head_bytes, integrity_status, integrity_generation \
             FROM worldstream_room_roots WHERE room_id = $1",
            &[&ROOM],
        )
        .unwrap_or_else(|error| panic!("read post-race live integrity: {error}"));
    assert_eq!(
        post_race_integrity.get::<_, Vec<u8>>(0),
        recovered_after_forgery
            .head()
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("post-race Head bytes: {error}"))
    );
    assert_eq!(post_race_integrity.get::<_, String>(1), "healthy");
    assert_eq!(post_race_integrity.get::<_, i64>(2), 1);
    println!(
        "LIVE_POSTGRES=PASS recovery=checkpoint-tail-2+operational-guard+malformed-witness-fallback+canonical-witness-full-fallback+stale-head-failure-fence"
    );

    qualify_bounded_recovery_scales(
        &admin_dsn.to_string_lossy(),
        &runtime,
        &mut runtime_client,
        &requested_recovery_scale_tiers(),
        RECOVERY_SCALE_ROOM,
    );

    let Some(pooler_dsn) = std::env::var_os("WORLDSTREAM_POSTGRES_TEST_POOLER_DSN") else {
        println!("LIVE_POSTGRES_POOLER=SKIP reason=pooler_dsn_unset");
        return;
    };
    let pooler = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(
            pooler_dsn.to_string_lossy(),
            PostgresConnectionPath::TransactionPool,
        )
        .unwrap_or_else(|error| panic!("live pooler config: {error}")),
    )
    .unwrap_or_else(|error| panic!("live pooler handle: {error}"));
    pooler
        .verify_schema()
        .unwrap_or_else(|error| panic!("live pooler schema verification: {error}"));
    assert!(matches!(
        pooler
            .read_guarded_receipt(receipt_probe.identity(), receipt_probe.request_hash())
            .unwrap_or_else(|error| panic!("live pooler guarded receipt read: {error}")),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    let pooled_retry: PreparedRoomWriteV1 = prepared_increment(
        &trace,
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "01ARZ3NDEKTSV4RRFFQ69G5FQ7",
    )
    .into();
    let pooled_identity = pooled_retry.identity().clone();
    let pooled_request_hash = pooled_retry.request_hash().clone();
    let duplicate = pooler.commit(&pooled_retry);
    assert!(matches!(
        duplicate,
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
    assert!(matches!(
        pooler.resolve(&pooled_identity, &pooled_request_hash),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    println!("LIVE_POSTGRES_POOLER=PASS path=transaction_pool duplicate+resolve");
}
