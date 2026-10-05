//! Live canonical-format adapter coverage. Each run uses fresh operation and Room identities.
use super::*;
use worldstream_core::{
    CanonicalHistoryFormat, CanonicalRoomCommitStorage, CanonicalRoomTrace,
    PreparedCanonicalRoomCommit, PreparedCanonicalRoomCreation, RoomCreationRequestWithFormat,
    commit_canonical_existing_room, commit_canonical_room_creation,
};

fn fresh_id() -> String {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).unwrap();
    let mut value = u128::from_be_bytes(random);
    let mut encoded = [b'0'; 26];
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    for digit in encoded.iter_mut().rev() {
        *digit = alphabet[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).unwrap()
}
fn live_handles() -> Option<(PostgresAdmin, PostgresRoomStore, Client)> {
    let admin_dsn = std::env::var("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN").ok()?;
    let runtime_dsn = std::env::var("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN").ok()?;
    let admin =
        PostgresAdmin::new(PostgresConnectionConfig::direct_admin(admin_dsn).unwrap()).unwrap();
    admin.migrate().unwrap();
    let runtime = PostgresRoomStore::new(
        PostgresConnectionConfig::runtime(runtime_dsn.clone(), PostgresConnectionPath::Direct)
            .unwrap(),
    )
    .unwrap();
    Some((
        admin,
        runtime,
        Client::connect(&runtime_dsn, NoTls).unwrap(),
    ))
}
fn canonical_creation(
    room: &str,
    member: &str,
    key: &str,
    format: CanonicalHistoryFormat,
) -> PreparedCanonicalRoomCreation {
    let (genesis, legacy, witness, identity) =
        creation_fixture_for(room, member, PRINCIPAL, 0, key);
    PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
        identity,
        &RoomCreationRequestWithFormat::new(legacy, format),
        witness,
        genesis,
    )
    .unwrap()
}
fn canonical_increment(
    trace: &CanonicalRoomTrace,
    member: &str,
    frame_head: u64,
    action: &str,
    transition: &str,
) -> PreparedCanonicalRoomCommit {
    let request = ParticipantActionRequestV1::new(
        trace.head().room_id().clone(),
        parsed(member),
        parsed(action),
        trace.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    let schema = trace
        .retained_pack()
        .descriptor()
        .actions
        .iter()
        .find(|value| value.action_type == "increment")
        .unwrap()
        .payload_schema
        .schema_digest
        .clone();
    let prepared = trace
        .prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(member),
            action_id: parsed(action),
            action_type: "increment".to_owned(),
            payload_schema_digest: schema,
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap();
    PreparedCanonicalRoomCommit::for_action_for_conformance(
        trace,
        &request,
        prepared,
        parsed(transition),
        worldstream_core::IntegrityGenerationV1::new(1).unwrap(),
        PreparedAuthorityWitnessV1::mint_for_conformance(
            "postgres-fixture-authority",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"action","revoked":false}"#),
        )
        .unwrap(),
        &[(parsed(member), frame_head)].into_iter().collect(),
    )
    .unwrap()
}
#[test]
fn live_canonical_v2_creation_action_retry_and_missing_materialization_recovery() {
    let Some((admin, store, mut client)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL=SKIP reason=dsn_unset");
        return;
    };
    let registry = builtin_counter_registry().unwrap();
    let room = fresh_id();
    let member = fresh_id();
    let key = format!("canonical-v2-{}", fresh_id());
    let creation = canonical_creation(&room, &member, &key, CanonicalHistoryFormat::V2);
    store
        .seed_conformance_authority(creation.authority_witness(), true)
        .unwrap();
    let (resolution, installation, trace, _) =
        commit_canonical_room_creation(&store, creation).into_parts();
    assert!(matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    assert_eq!(
        installation,
        worldstream_core::ActorInstallationV1::Installed
    );
    let mut trace = trace.unwrap();
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(
            &store,
            &canonical_creation(&room, &member, &key, CanonicalHistoryFormat::V2).into()
        ),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(
            &store,
            &canonical_creation(&room, &member, &key, CanonicalHistoryFormat::V1).into()
        ),
        RoomCommitResolutionV1::Conflict { .. }
    ));
    let action = fresh_id();
    let transition = fresh_id();
    let prepared = canonical_increment(&trace, &member, 0, &action, &transition);
    store
        .seed_conformance_authority(prepared.authority_witness(), true)
        .unwrap();
    let duplicate = canonical_increment(&trace, &member, 0, &action, &transition);
    let outcome = commit_canonical_existing_room(&store, &mut trace, prepared);
    assert_eq!(
        outcome.actor_installation(),
        worldstream_core::ActorInstallationV1::Installed
    );
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(&store, &duplicate.into()),
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
    let verified = admin.verify_room(&room).unwrap();
    verified.verify_executable_replay(&registry).unwrap();
    assert_eq!(verified.head, *trace.head());
    assert!(
        !String::from_utf8_lossy(&verified.transition_bytes[0])
            .contains("\"resulting_core_state\":")
    );
    assert!(
        !String::from_utf8_lossy(&verified.transition_bytes[0])
            .contains("\"resulting_activity_state\":")
    );
    let fence = store
        .current_room_serving_fence(trace.head().room_id())
        .unwrap()
        .unwrap();
    assert_eq!(fence.head(), trace.head());
    admin.delete_snapshot_cache(&room).unwrap();
    client
        .execute(
            "DELETE FROM worldstream_materializations WHERE room_id=$1",
            &[&room],
        )
        .unwrap();
    let recovered = store
        .recover_canonical_room(&registry, &room)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.head(), trace.head());
    assert_eq!(recovered.activity_state(), trace.activity_state());
    assert_eq!(recovered.core_state(), trace.core_state());
    admin
        .verify_room(&room)
        .unwrap()
        .verify_executable_replay(&registry)
        .unwrap();
    println!(
        "LIVE_POSTGRES_CANONICAL=PASS v2+retry+conflict+action+operational-replay+missing-materialization"
    );
}

fn core_plan(
    store: &PostgresRoomStore,
    trace: &CanonicalRoomTrace,
    kind: CoreProposedKindV1,
    changes: CoreChangeSetV1,
) -> PreparedCanonicalRoomCommit {
    let request = CoreAdministrationRequestV1::new(
        trace.head().room_id().clone(),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: worldstream_core::CORE_OPERATION_KIND.into(),
            idempotency_key: fresh_id(),
        },
        kind,
        trace.head().room_seq(),
        "canonical_adapter_test",
        changes,
    )
    .unwrap();
    let frames = store
        .current_room_serving_fence(trace.head().room_id())
        .unwrap()
        .unwrap();
    let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "postgres-fixture-authority",
        parsed(PRINCIPAL),
        1,
        &canonical(br#"{"scope":"administration","revoked":false}"#),
    )
    .unwrap();
    store.seed_conformance_authority(&witness, true).unwrap();
    PreparedCanonicalRoomCommit::for_core_administration_for_conformance(
        trace,
        &request,
        parsed("2026-08-15T12:00:02Z"),
        parsed(&fresh_id()),
        worldstream_core::IntegrityGenerationV1::new(1).unwrap(),
        witness,
        frames.frame_heads(),
    )
    .unwrap()
}

#[test]
fn live_canonical_v2_atomic_outcomes_membership_checkpoint_repair_and_nochange() {
    let Some((admin, store, _)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL_OUTCOMES=SKIP reason=dsn_unset");
        return;
    };
    let registry = builtin_counter_registry().unwrap();
    let room = fresh_id();
    let member = fresh_id();
    let key = fresh_id();
    let creation = canonical_creation(&room, &member, &key, CanonicalHistoryFormat::V2);
    store
        .seed_conformance_authority(creation.authority_witness(), true)
        .unwrap();
    let (_, _, trace, _) = commit_canonical_room_creation(&store, creation).into_parts();
    let mut trace = trace.unwrap();
    let action = fresh_id();
    let transition = fresh_id();
    let rollback = canonical_increment(&trace, &member, 0, &action, &transition);
    store
        .seed_conformance_authority(rollback.authority_witness(), true)
        .unwrap();
    store.set_failpoint(Some(PostgresFailpoint::RollbackBeforeCommit));
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(&store, &rollback.into()),
        RoomCommitResolutionV1::RetryableKnownAbsent
    ));
    assert_eq!(
        store
            .current_room_serving_fence(trace.head().room_id())
            .unwrap()
            .unwrap()
            .head(),
        trace.head()
    );
    store.set_failpoint(None);
    let unknown = canonical_increment(&trace, &member, 0, &action, &transition);
    let identity = unknown.identity().clone();
    let request_hash = unknown.request_hash().clone();
    store.set_failpoint(Some(PostgresFailpoint::UnknownAfterCommit));
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(&store, &unknown.into()),
        RoomCommitResolutionV1::Indeterminate
    ));
    store.set_failpoint(None);
    assert!(matches!(
        CanonicalRoomCommitStorage::resolve(&store, &identity, &request_hash),
        worldstream_core::ResolveOutcomeV1::StoredResolution(_)
    ));
    trace = store
        .recover_canonical_room(&registry, &room)
        .unwrap()
        .unwrap();
    let before = trace.head().clone();
    let fenced = canonical_increment(&trace, &member, 1, &fresh_id(), &fresh_id());
    store
        .seed_conformance_authority(fenced.authority_witness(), false)
        .unwrap();
    assert!(matches!(
        CanonicalRoomCommitStorage::commit(&store, &fenced.into()),
        RoomCommitResolutionV1::Fenced
    ));
    assert_eq!(
        store
            .current_room_serving_fence(trace.head().room_id())
            .unwrap()
            .unwrap()
            .head(),
        &before
    );
    let suspend = core_plan(
        &store,
        &trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(
            trace
                .core_state()
                .membership(&parsed(&member))
                .unwrap()
                .clone(),
        )),
    );
    assert!(matches!(
        commit_canonical_existing_room(&store, &mut trace, suspend).resolution(),
        RoomCommitResolutionV1::TransitionCommitted { .. }
    ));
    let resume = core_plan(
        &store,
        &trace,
        CoreProposedKindV1::Resume,
        CoreChangeSetV1::one(MembershipChangeV1::resume(
            trace
                .core_state()
                .membership(&parsed(&member))
                .unwrap()
                .clone(),
        )),
    );
    assert!(matches!(
        commit_canonical_existing_room(&store, &mut trace, resume).resolution(),
        RoomCommitResolutionV1::TransitionCommitted { .. }
    ));
    assert_eq!(
        admin
            .rebuild_canonical_snapshot_cache(&registry, &room)
            .unwrap(),
        4
    );
    admin
        .verify_room(&room)
        .unwrap()
        .verify_executable_replay(&registry)
        .unwrap();
    let repaired = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(repaired.receipt().used_checkpoint());
    assert_eq!(
        repaired.receipt().checkpoint_room_seq(),
        Some(trace.head().room_seq())
    );
    store
        .rebuild_verified_canonical_recovery_checkpoint(&registry, &room)
        .unwrap()
        .unwrap();
    let execution = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(execution.receipt().used_checkpoint());
    assert_eq!(
        execution.receipt().checkpoint_room_seq(),
        Some(trace.head().room_seq())
    );
    assert_eq!(execution.receipt().tail_transition_records_delivered(), 0);
    assert_eq!(
        admin.corrupt_snapshot_cache_for_conformance(&room).unwrap(),
        1
    );
    let fallback = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(!fallback.receipt().used_checkpoint());
    assert_eq!(fallback.trace().head(), trace.head());
    assert_eq!(
        admin
            .rebuild_canonical_snapshot_cache(&registry, &room)
            .unwrap(),
        4
    );
    store
        .rebuild_verified_canonical_recovery_checkpoint(&registry, &room)
        .unwrap()
        .unwrap();
    let archive = core_plan(
        &store,
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(worldstream_core::RoomStatusV1::Active),
    );
    assert!(matches!(
        commit_canonical_existing_room(&store, &mut trace, archive).resolution(),
        RoomCommitResolutionV1::TransitionCommitted { .. }
    ));
    let head = trace.head().clone();
    let repeated = core_plan(
        &store,
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(worldstream_core::RoomStatusV1::Archived),
    );
    assert!(matches!(
        commit_canonical_existing_room(&store, &mut trace, repeated).resolution(),
        RoomCommitResolutionV1::NoChangeRecorded { .. }
    ));
    assert_eq!(trace.head(), &head);
    let execution = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(execution.receipt().used_checkpoint());
    assert_eq!(execution.receipt().tail_transition_records_delivered(), 1);
    assert_eq!(execution.trace().head(), trace.head());
    admin
        .verify_room(&room)
        .unwrap()
        .verify_executable_replay(&registry)
        .unwrap();
    println!(
        "LIVE_POSTGRES_CANONICAL_OUTCOMES=PASS rollback+unknown-resolution+authority+membership+checkpoint+repair+archive+NoChange"
    );
}

#[test]
fn live_canonical_v2_structural_corruption_and_runtime_absence_are_distinct() {
    let Some((admin, store, mut client)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL_CLASS=SKIP reason=dsn_unset");
        return;
    };
    let registry = builtin_counter_registry().unwrap();
    let missing = worldstream_core::counter_v1_only_registry_for_conformance().unwrap();
    let room = fresh_id();
    let member = fresh_id();
    let creation = canonical_creation(&room, &member, &fresh_id(), CanonicalHistoryFormat::V2);
    store
        .seed_conformance_authority(creation.authority_witness(), true)
        .unwrap();
    let (_, _, trace, _) = commit_canonical_room_creation(&store, creation).into_parts();
    let mut trace = trace.unwrap();
    let plan = canonical_increment(&trace, &member, 0, &fresh_id(), &fresh_id());
    store
        .seed_conformance_authority(plan.authority_witness(), true)
        .unwrap();
    commit_canonical_existing_room(&store, &mut trace, plan);
    let verified = admin.verify_room(&room).unwrap();
    assert!(matches!(
        verified.verify_executable_replay(&missing),
        Err(worldstream_core::RoomRecoveryErrorV1::RuntimeUnavailable)
    ));
    let old_snapshots = verified.snapshots.clone();
    assert!(matches!(
        admin.rebuild_canonical_snapshot_cache(&missing, &room),
        Err(worldstream_postgres::PostgresMaintenanceError::RuntimeUnavailable)
    ));
    assert_eq!(admin.verify_room(&room).unwrap().snapshots, old_snapshots);
    client
        .execute(
            "UPDATE worldstream_materializations SET core_state_bytes='{}'::bytea WHERE room_id=$1",
            &[&room],
        )
        .unwrap();
    assert!(matches!(
        store.current_room_serving_fence(trace.head().room_id()),
        Err(worldstream_postgres::PostgresRoomVerificationError::Corrupt { .. })
    ));
    assert!(matches!(
        admin.verify_room(&room),
        Err(worldstream_postgres::PostgresRoomVerificationError::Corrupt { .. })
    ));
    client
        .execute(
            "UPDATE worldstream_materializations SET core_state_bytes=$2 WHERE room_id=$1",
            &[&room, &verified.core_state_bytes],
        )
        .unwrap();
    // A legacy-only return contract cannot quarantine a valid V2 Room.
    assert!(matches!(
        worldstream_core::recover_room_from_storage(&store, &registry, trace.head().room_id()),
        Err(worldstream_core::RoomRecoveryErrorV1::RuntimeUnavailable)
    ));
    assert_eq!(
        admin.verify_room(&room).unwrap().integrity_status,
        "healthy"
    );
    admin.delete_snapshot_cache(&room).unwrap();
    assert!(matches!(
        store.recover_canonical_room(&missing, &room),
        Err(worldstream_core::RoomRecoveryErrorV1::RuntimeUnavailable)
    ));
    let verification = admin.verify_room(&room).unwrap();
    assert_eq!(verification.integrity_status, "faulted");
    assert_eq!(verification.integrity_generation, 2);
    println!(
        "LIVE_POSTGRES_CANONICAL_CLASS=PASS strict-current+missing-runtime+atomic-repair+faulted-without-quarantine"
    );
}

#[test]
fn live_canonical_v2_timer_activation_facts_and_mmr_recover() {
    let Some((admin, store, _)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL_TIMER=SKIP reason=dsn_unset");
        return;
    };
    let registry = worldstream_core::builtin_agent_heist_registry().unwrap();
    let room = fresh_id();
    let members = [
        ("navigator", PrincipalKindV1::Agent),
        ("insider", PrincipalKindV1::Human),
        ("broker", PrincipalKindV1::Human),
    ]
    .into_iter()
    .map(|(role, kind)| {
        MembershipV1::new(
            parsed(&fresh_id()),
            parsed(&fresh_id()),
            kind,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some(role.into()),
        )
        .unwrap()
    })
    .collect::<Vec<_>>();
    let configuration=canonical(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#);
    let genesis = registry
        .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
            room_id: parsed(&room),
            pack_digest: worldstream_core::agent_heist_digest(),
            configuration: configuration.clone(),
            room_seed: parsed(SEED),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: CoreRoomStateV1::active(members.clone()).unwrap(),
        })
        .unwrap();
    let request = RoomCreationRequestWithFormat::new(
        RoomCreationRequestV1::new(
            worldstream_core::agent_heist_digest(),
            configuration,
            members
                .iter()
                .map(|member| {
                    InitialMembershipProposalV1::new(
                        member.principal_id().clone(),
                        member.principal_kind(),
                        member.standing(),
                        member.access_mode(),
                        member.role().map(str::to_owned),
                    )
                    .unwrap()
                })
                .collect(),
        ),
        CanonicalHistoryFormat::V2,
    );
    let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "postgres-fixture-authority",
        parsed(PRINCIPAL),
        1,
        &canonical(br#"{"scope":"create_room","revoked":false}"#),
    )
    .unwrap();
    store.seed_conformance_authority(&witness, true).unwrap();
    let creation = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: worldstream_core::CREATE_ROOM_OPERATION_KIND.into(),
            idempotency_key: fresh_id(),
        },
        &request,
        witness,
        genesis,
    )
    .unwrap();
    let (_, _, trace, _) = commit_canonical_room_creation(&store, creation).into_parts();
    let mut trace = trace.unwrap();
    let timer = trace.genesis().initial_timers()[0].clone();
    for generation in 1..=2 {
        let candidate = store
            .timer_candidate(
                trace.head().room_id(),
                &timer.timer_id,
                worldstream_core::TimerGenerationV1::new(generation).unwrap(),
            )
            .unwrap()
            .unwrap();
        let request = candidate.request().clone();
        let stimulus = RecordedStimulusV1::TimerFired(worldstream_core::TimerFiredV1 {
            timer_id: request.timer_id().clone(),
            generation: request.generation(),
            scheduled_for: request.scheduled_for().clone(),
            canonical_payload: request.canonical_payload().clone(),
        });
        let prepared = trace.prepare(stimulus).unwrap();
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            "postgres-fixture-authority",
            parsed(PRINCIPAL),
            1,
            &canonical(br#"{"scope":"timer","revoked":false}"#),
        )
        .unwrap();
        store.seed_conformance_authority(&witness, true).unwrap();
        let frames = store
            .current_room_serving_fence(trace.head().room_id())
            .unwrap()
            .unwrap();
        let plan = PreparedCanonicalRoomCommit::for_timer_fired_for_conformance(
            &trace,
            &request,
            prepared,
            parsed(&fresh_id()),
            worldstream_core::IntegrityGenerationV1::new(1).unwrap(),
            witness,
            frames.frame_heads(),
        )
        .unwrap();
        assert!(matches!(
            commit_canonical_existing_room(&store, &mut trace, plan).resolution(),
            RoomCommitResolutionV1::TransitionCommitted { .. }
        ));
    }
    let verified = admin.verify_room(&room).unwrap();
    assert!(verified.timers.iter().any(|timer| timer.state == "fired"));
    assert!(!verified.activation_decisions.is_empty());
    verified.verify_executable_replay(&registry).unwrap();
    admin.delete_snapshot_cache(&room).unwrap();
    let recovered = store
        .recover_canonical_room(&registry, &room)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.head(), trace.head());
    assert_eq!(recovered.activity_state(), trace.activity_state());
    store
        .rebuild_verified_canonical_recovery_checkpoint(&registry, &room)
        .unwrap()
        .unwrap();
    let execution = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(execution.receipt().used_checkpoint());
    println!(
        "LIVE_POSTGRES_CANONICAL_TIMER=PASS timer-generation+fired-ledger+Activation-decisions+MMR+full-and-checkpoint"
    );
}

#[test]
fn live_canonical_v2_paged_replay_and_checkpoint_tail_ceiling() {
    let Some((admin, store, mut client)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL_PAGES=SKIP reason=dsn_unset");
        return;
    };
    let registry = builtin_counter_registry().unwrap();
    let room = fresh_id();
    let member = fresh_id();
    let creation = canonical_creation(&room, &member, &fresh_id(), CanonicalHistoryFormat::V2);
    store
        .seed_conformance_authority(creation.authority_witness(), true)
        .unwrap();
    let (_, _, trace, _) = commit_canonical_room_creation(&store, creation).into_parts();
    let mut trace = trace.unwrap();
    for index in 1..=257 {
        let membership = trace
            .core_state()
            .membership(&parsed(&member))
            .unwrap()
            .clone();
        let (kind, change) = if index % 2 == 1 {
            (
                CoreProposedKindV1::Suspend,
                MembershipChangeV1::suspend(membership),
            )
        } else {
            (
                CoreProposedKindV1::Resume,
                MembershipChangeV1::resume(membership),
            )
        };
        let plan = core_plan(&store, &trace, kind, CoreChangeSetV1::one(change));
        assert!(matches!(
            commit_canonical_existing_room(&store, &mut trace, plan).resolution(),
            RoomCommitResolutionV1::TransitionCommitted { .. }
        ));
    }
    // Retain the authentic Genesis snapshot. Its 257-record tail is ineligible.
    client
        .execute(
            "DELETE FROM worldstream_room_snapshots WHERE room_id=$1 AND room_seq>0",
            &[&room],
        )
        .unwrap();
    let candidate = worldstream_core::CanonicalRoomRecoveryStorage::inspect_recovery_candidate(
        &store,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(!candidate.has_checkpoint());
    assert_eq!(candidate.tail_transition_count(), 257);
    let recovered = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(!recovered.receipt().used_checkpoint());
    assert_eq!(
        recovered.receipt().prefix_transition_records_delivered(),
        257
    );
    assert_eq!(recovered.trace().head(), trace.head());
    assert_eq!(recovered.trace().core_state(), trace.core_state());
    store
        .verify_canonical_stream_room_for_conformance(&registry, &room)
        .unwrap();
    assert!(matches!(
        store.verify_canonical_stream_room_for_conformance(
            &worldstream_core::counter_v1_only_registry_for_conformance().unwrap(),
            &room
        ),
        Err(worldstream_postgres::PostgresRoomVerificationError::RuntimeUnavailable)
    ));
    store
        .rebuild_verified_canonical_recovery_checkpoint(&registry, &room)
        .unwrap()
        .unwrap();
    let fast = worldstream_core::recover_canonical_room_from_storage_with_receipt(
        &store,
        &registry,
        trace.head().room_id(),
    )
    .unwrap()
    .unwrap();
    assert!(fast.receipt().used_checkpoint());
    assert_eq!(fast.receipt().prefix_transitions_skipped(), 257);
    assert_eq!(fast.receipt().tail_transition_records_delivered(), 0);
    admin
        .verify_room(&room)
        .unwrap()
        .verify_executable_replay(&registry)
        .unwrap();
    println!(
        "LIVE_POSTGRES_CANONICAL_PAGES=PASS two-keyset-pages+257-tail-full-fallback+exact-operational-replay+checkpoint"
    );
}

#[test]
fn live_canonical_both_formats_exact_v1_and_concurrent_duplicates() {
    let Some((admin, _, _)) = live_handles() else {
        println!("LIVE_POSTGRES_CANONICAL_V1=SKIP reason=dsn_unset");
        return;
    };
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (_, store, _) = live_handles().unwrap();
        let room = fresh_id();
        let member = fresh_id();
        let key = fresh_id();
        let creation = canonical_creation(&room, &member, &key, format);
        let (genesis, request, witness, identity) =
            creation_fixture_for(&room, &member, PRINCIPAL, 0, &key);
        let legacy = PreparedRoomCreationV1::from_registry_genesis_for_conformance(
            identity, &request, witness, genesis,
        )
        .unwrap();
        if format == CanonicalHistoryFormat::V1 {
            assert_eq!(
                creation.persistence().canonical_genesis_bytes(),
                legacy.persistence().canonical_genesis_bytes.as_slice()
            );
        }
        store
            .seed_conformance_authority(creation.authority_witness(), true)
            .unwrap();
        let (_, _, trace, _) = commit_canonical_room_creation(&store, creation).into_parts();
        let trace = trace.unwrap();
        let action = fresh_id();
        let transition = fresh_id();
        let first = canonical_increment(&trace, &member, 0, &action, &transition);
        let second = canonical_increment(&trace, &member, 0, &action, &transition);
        let (old_genesis, _, _, _) = creation_fixture_for(&room, &member, PRINCIPAL, 0, &key);
        let old_trace = CoreTraceV1::create_from_retained_for_conformance(old_genesis).unwrap();
        let old_plan = prepared_increment_for(&old_trace, &action, &transition, 0, &room, &member);
        let worldstream_core::PreparedCanonicalExistingIntent::Advance(canonical) = first.intent()
        else {
            panic!("expected Advance");
        };
        let worldstream_core::PreparedExistingIntentV1::Advance(legacy) = old_plan.intent() else {
            panic!("expected V1 Advance");
        };
        if format == CanonicalHistoryFormat::V1 {
            assert_eq!(
                canonical.canonical_transition_bytes(),
                legacy.canonical_transition_bytes.as_slice()
            );
            assert_eq!(
                first.semantic_result().canonical_receipt_bytes(),
                old_plan.semantic_result().canonical_receipt_bytes()
            );
        }
        store
            .seed_conformance_authority(first.authority_witness(), true)
            .unwrap();
        let shared_store = Arc::new(store);
        let left = Arc::clone(&shared_store);
        let right = Arc::clone(&shared_store);
        let one =
            thread::spawn(move || CanonicalRoomCommitStorage::commit(left.as_ref(), &first.into()));
        let two = thread::spawn(move || {
            CanonicalRoomCommitStorage::commit(right.as_ref(), &second.into())
        });
        let results = [one.join().unwrap(), two.join().unwrap()];
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
                .filter(|result| matches!(
                    result,
                    RoomCommitResolutionV1::TransitionCommitted {
                        status: ResolutionStatusV1::Existing,
                        ..
                    }
                ))
                .count(),
            1
        );
        let verified = admin.verify_room(&room).unwrap();
        assert_eq!(verified.transition_count, 1);
        verified
            .verify_executable_replay(&builtin_counter_registry().unwrap())
            .unwrap();
        shared_store
            .verify_canonical_stream_room_for_conformance(
                &builtin_counter_registry().unwrap(),
                &room,
            )
            .unwrap();
    }
    println!(
        "LIVE_POSTGRES_CANONICAL_V1=PASS exact-Genesis+Transition+receipt-bytes+concurrent-duplicate+selected-paged-replay"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn live_canonical_direct_authorized_retries_precede_execution() {
    use worldstream_core::*;
    let Some((_admin, store, mut client)) = live_handles() else {
        println!("LIVE_POSTGRES_DIRECT_RETRY=SKIP reason=dsn_unset");
        return;
    };
    let registry = builtin_counter_registry().unwrap();
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let room = fresh_id();
        let member = fresh_id();
        let principal = fresh_id();
        let host_cap = fresh_id();
        let member_cap = fresh_id();
        let (genesis, legacy, _, identity) =
            creation_fixture_for(&room, &member, &principal, 0, &fresh_id());
        let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
            &format!("direct-retry-{room}"),
            parsed(&principal),
            1,
            &canonical(br#"{"scope":"create_room","revoked":false}"#),
        )
        .unwrap();
        let creation = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
            identity,
            &RoomCreationRequestWithFormat::new(legacy, format),
            witness,
            genesis,
        )
        .unwrap();
        store
            .seed_conformance_authority(creation.authority_witness(), true)
            .unwrap();
        let mut trace = commit_canonical_room_creation(&store, creation)
            .into_parts()
            .2
            .unwrap();
        // Seed only fresh authority fixture rows. This test does not reuse a
        // deployment bootstrap or another test's capability/operation lane.
        let mut host_bytes = [0_u8; 32];
        let mut member_bytes = [0_u8; 32];
        getrandom::fill(&mut host_bytes).unwrap();
        getrandom::fill(&mut member_bytes).unwrap();
        let host_bearer = CapabilityBearerV1::from_bytes(host_bytes);
        let member_bearer = CapabilityBearerV1::from_bytes(member_bytes);
        client.execute("INSERT INTO worldstream_authority_principals (principal_id,principal_kind,authority_status,principal_generation) VALUES ($1,'human','enabled',1)", &[&principal]).unwrap();
        client.execute("INSERT INTO worldstream_authority_capabilities (capability_id,token_hash,principal_id,profile_kind,authority_generation) VALUES ($1,$2,$3,'host_operator',1)", &[&host_cap,&host_bearer.token_hash().storage_bytes().as_slice(),&principal]).unwrap();
        client.execute("INSERT INTO worldstream_authority_capability_scopes (capability_id,scope) VALUES ($1,'operator:room_admin')", &[&host_cap]).unwrap();
        client.execute("INSERT INTO worldstream_authority_capabilities (capability_id,token_hash,principal_id,profile_kind,target_room_id,target_member_id,authority_generation) VALUES ($1,$2,$3,'room_member',$4,$5,1)", &[&member_cap,&member_bearer.token_hash().storage_bytes().as_slice(),&principal,&room,&member]).unwrap();
        client.execute("INSERT INTO worldstream_authority_capability_scopes (capability_id,scope) VALUES ($1,'room:act')", &[&member_cap]).unwrap();
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let member_presented = PresentedCapabilityV1::new(parsed(&member_cap), member_bearer);
        let host_presented = PresentedCapabilityV1::new(parsed(&host_cap), host_bearer);
        let action = fresh_id();
        let request = ParticipantActionRequestV1::new(
            parsed(&room),
            parsed(&member),
            parsed(&action),
            trace.head().room_seq(),
            "increment",
            canonical(br"{}"),
        );
        let changed = ParticipantActionRequestV1::new(
            parsed(&room),
            parsed(&member),
            parsed(&action),
            trace.head().room_seq(),
            "increment",
            canonical(br#"{"changed":true}"#),
        );
        let action_grant =
            |request: &ParticipantActionRequestV1| match authorize_participant_action_operation(
                &authority,
                &store,
                &member_presented,
                request,
                parsed("2026-08-15T12:00:01Z"),
            )
            .unwrap()
            {
                ParticipantActionIngressV1::Authorized(grant) => *grant,
                _ => panic!("fresh action grant"),
            };
        let first = action_grant(&request);
        let retry = action_grant(&request);
        let conflict = action_grant(&changed);
        let mismatched = action_grant(&request);
        let revoked = action_grant(&request);
        let stimulus = ParticipantActionV1 {
            member_id: parsed(&member),
            action_id: parsed(&action),
            action_type: "increment".into(),
            payload_schema_digest: trace
                .retained_pack()
                .descriptor()
                .actions
                .iter()
                .find(|v| v.action_type == "increment")
                .unwrap()
                .payload_schema
                .schema_digest
                .clone(),
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        };
        let frames = [(parsed(&member), 0)].into_iter().collect();
        let accepted = store
            .commit_authorized_participant_action_from_canonical_serving_trace(
                first,
                &request,
                stimulus.clone(),
                parsed(&fresh_id()),
                &mut trace,
                IntegrityGenerationV1::new(1).unwrap(),
                &frames,
            )
            .unwrap();
        let original = accepted
            .stored_result()
            .unwrap()
            .canonical_receipt_bytes()
            .to_vec();
        let before = trace.activity_callback_count();
        let resolved = store
            .commit_authorized_participant_action_from_canonical_serving_trace(
                retry,
                &request,
                stimulus.clone(),
                parsed(&fresh_id()),
                &mut trace,
                IntegrityGenerationV1::new(1).unwrap(),
                &frames,
            )
            .unwrap();
        assert!(matches!(
            resolved,
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved.stored_result().unwrap().canonical_receipt_bytes(),
            original
        );
        assert_eq!(trace.activity_callback_count(), before);
        assert!(matches!(
            store
                .commit_authorized_participant_action_from_canonical_serving_trace(
                    conflict,
                    &changed,
                    stimulus.clone(),
                    parsed(&fresh_id()),
                    &mut trace,
                    IntegrityGenerationV1::new(1).unwrap(),
                    &frames
                )
                .unwrap(),
            RoomCommitResolutionV1::Conflict { .. }
        ));
        assert!(matches!(
            store.commit_authorized_participant_action_from_canonical_serving_trace(
                mismatched,
                &changed,
                stimulus.clone(),
                parsed(&fresh_id()),
                &mut trace,
                IntegrityGenerationV1::new(1).unwrap(),
                &frames,
            ),
            Err(worldstream_postgres::PostgresRoomCommitError::Preparation)
        ));
        client.execute("UPDATE worldstream_authority_capabilities SET authority_generation=2, revoked_at='2026-08-15T12:00:02Z' WHERE capability_id=$1", &[&member_cap]).unwrap();
        assert!(matches!(
            store
                .commit_authorized_participant_action_from_canonical_serving_trace(
                    revoked,
                    &request,
                    stimulus,
                    parsed(&fresh_id()),
                    &mut trace,
                    IntegrityGenerationV1::new(1).unwrap(),
                    &frames,
                )
                .unwrap(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(trace.activity_callback_count(), before);
        let core_request = CoreAdministrationRequestV1::new(
            parsed(&room),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(&principal),
                versioned_operation_kind: CORE_OPERATION_KIND.into(),
                idempotency_key: fresh_id(),
            },
            CoreProposedKindV1::Archive,
            trace.head().room_seq(),
            "direct_retry",
            CoreChangeSetV1::archive(RoomStatusV1::Active),
        )
        .unwrap();
        let core_grant = || match authorize_core_administration_operation(
            &authority,
            &store,
            &host_presented,
            &core_request,
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap()
        {
            CoreAdministrationIngressV1::Authorized(grant) => *grant,
            _ => panic!("fresh administration grant"),
        };
        let first = core_grant();
        let retry = core_grant();
        let revoked_core = core_grant();
        let accepted = store
            .commit_authorized_core_administration(
                &registry,
                first,
                &core_request,
                parsed("2026-08-15T12:00:01Z"),
                parsed(&fresh_id()),
            )
            .unwrap();
        let original = accepted
            .stored_result()
            .unwrap()
            .canonical_receipt_bytes()
            .to_vec();
        drop(trace);
        let reopened = PostgresRoomStore::new(
            PostgresConnectionConfig::runtime(
                std::env::var("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN").unwrap(),
                PostgresConnectionPath::Direct,
            )
            .unwrap(),
        )
        .unwrap();
        let resolved = reopened
            .commit_authorized_core_administration(
                &counter_v1_only_registry_for_conformance().unwrap(),
                retry,
                &core_request,
                parsed("2026-08-15T12:00:02Z"),
                parsed(&fresh_id()),
            )
            .unwrap();
        assert!(matches!(
            resolved,
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved.stored_result().unwrap().canonical_receipt_bytes(),
            original
        );
        client.execute("UPDATE worldstream_authority_capabilities SET authority_generation=2, revoked_at='2026-08-15T12:00:02Z' WHERE capability_id=$1", &[&host_cap]).unwrap();
        assert!(matches!(
            reopened
                .commit_authorized_core_administration(
                    &counter_v1_only_registry_for_conformance().unwrap(),
                    revoked_core,
                    &core_request,
                    parsed("2026-08-15T12:00:03Z"),
                    parsed(&fresh_id()),
                )
                .unwrap(),
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(
            reopened.verify_room(&room).unwrap().integrity_status,
            "healthy"
        );
    }
    println!(
        "LIVE_POSTGRES_DIRECT_RETRY=PASS formats=v1,v2 cached-action+conflict+mismatch+revocation+cold-core-missing-executor"
    );
}
