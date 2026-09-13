#![allow(clippy::panic)]

use std::{collections::BTreeMap, fmt::Display, str::FromStr, sync::Arc, thread};

#[cfg(feature = "conformance-tracer")]
use postgres::{Client, NoTls};

use worldstream_core::{
    AccessModeV1, AdministrationOperationIdentityV1, CanonicalJsonV1, CoreRoomStateV1, CoreTraceV1,
    InitialMembershipProposalV1, MembershipStandingV1, MembershipV1, PackGenesisRequestV1,
    ParticipantActionRequestV1, ParticipantActionV1, PreparedAuthorityWitnessV1,
    PreparedRoomCommitV1, PreparedRoomCreationV1, PreparedRoomWriteV1, PrincipalKindV1,
    RecordedStimulusV1, ResolutionStatusV1, RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationRequestV1, RoomSeedV1, TransitionId, builtin_counter_registry, counter_v2_digest,
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
    assert_eq!(migration_history().len(), 17);
    assert_eq!(
        migration_history()[2].id,
        worldstream_postgres::KERNEL_CONFORMANCE_MIGRATION_ID
    );
    assert!(migration_sql().contains("worldstream_timers"));
    assert!(worldstream_postgres::MIGRATION_0003_SQL.contains("worldstream_activation_intents"));
    assert!(worldstream_postgres::MIGRATION_0003_SQL.contains("worldstream_room_snapshots"));
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
    assert_eq!(verified.snapshots.len(), 2);
    assert_eq!(
        verified.snapshots.last().map(|snapshot| snapshot.room_seq),
        Some(1)
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
