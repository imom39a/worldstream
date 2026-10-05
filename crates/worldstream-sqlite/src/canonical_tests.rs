//! Real-storage canonical lifecycle coverage. Retained legacy fixtures stay exact.
#![allow(clippy::panic, clippy::unwrap_used)]

use super::*;
use std::{error::Error, fmt::Display, str::FromStr};
use tempfile::NamedTempFile;
use worldstream_core::*;

type TestResult = Result<(), Box<dyn Error>>;
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T: FromStr>(value: &str) -> T
where
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("fixture {value}: {error}"))
}

fn creation(
    format: CanonicalHistoryFormat,
) -> Result<(PreparedCanonicalRoomCreation, PreparedAuthorityWitnessV1), Box<dyn Error>> {
    let member = MembershipV1::new(
        parsed(MEMBER),
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".into()),
    )?;
    let configuration = CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)?;
    let registry = builtin_counter_registry()?;
    let genesis = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed(ROOM),
        pack_digest: counter_v2_digest(),
        configuration: configuration.clone(),
        room_seed: parsed(SEED),
        created_at: parsed("2026-08-15T12:00:00Z"),
        initial_core_state: CoreRoomStateV1::active([member])?,
    })?;
    let request = RoomCreationRequestWithFormat::new(
        RoomCreationRequestV1::new(
            counter_v2_digest(),
            configuration,
            vec![InitialMembershipProposalV1::new(
                parsed(PRINCIPAL),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".into()),
            )?],
        ),
        format,
    );
    let witness = PreparedAuthorityWitnessV1::mint_for_conformance(
        "canonical-sqlite-authority",
        parsed(PRINCIPAL),
        1,
        &CanonicalJsonV1::parse(br#"{"scope":"create_room","revoked":false}"#)?,
    )?;
    let prepared = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
            idempotency_key: "canonical-create".into(),
        },
        &request,
        witness.clone(),
        genesis,
    )?;
    Ok((prepared, witness))
}

fn fixture(
    format: CanonicalHistoryFormat,
) -> Result<
    (
        NamedTempFile,
        SqliteRoomStore,
        CanonicalRoomTrace,
        PreparedAuthorityWitnessV1,
    ),
    Box<dyn Error>,
> {
    let file = NamedTempFile::new()?;
    let store = SqliteRoomStore::open(file.path())?;
    let (creation, witness) = creation(format)?;
    store
        .seed_authority(&witness, true)
        .map_err(std::io::Error::other)?;
    let outcome = commit_canonical_room_creation(&store, creation);
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let trace = outcome.into_parts().2.ok_or("creation trace missing")?;
    Ok((file, store, trace, witness))
}

fn increment_plan(
    store: &SqliteRoomStore,
    trace: &CanonicalRoomTrace,
    witness: &PreparedAuthorityWitnessV1,
    ordinal: usize,
) -> Result<PreparedCanonicalRoomCommit, Box<dyn Error>> {
    let action: ActionId = parsed(&format!("01ARZ3NDEKTSV4RRFFQ69G{ordinal:04}"));
    let transition: TransitionId = parsed(&format!("01ARZ3NDEKTSV4RRFFQ68G{ordinal:04}"));
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        action.clone(),
        trace.head().room_seq(),
        "increment",
        CanonicalJsonV1::parse(br"{}")?,
    );
    let payload_schema_digest = trace
        .retained_pack()
        .descriptor()
        .actions
        .iter()
        .find(|item| item.action_type == "increment")
        .ok_or("increment descriptor")?
        .payload_schema
        .schema_digest
        .clone();
    let prepared_transition =
        trace.prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: action,
            action_type: "increment".into(),
            payload_schema_digest,
            canonical_payload: CanonicalJsonV1::parse(br"{}")?,
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))?;
    let fence = store
        .current_room_serving_fence(&parsed(ROOM))?
        .ok_or("serving fence")?;
    Ok(PreparedCanonicalRoomCommit::for_action_for_conformance(
        trace,
        &request,
        prepared_transition,
        transition,
        fence.integrity().generation(),
        witness.clone(),
        fence.frame_heads(),
    )?)
}

fn increment(
    store: &SqliteRoomStore,
    trace: &mut CanonicalRoomTrace,
    witness: &PreparedAuthorityWitnessV1,
    ordinal: usize,
) -> Result<RoomCommitResolutionV1, Box<dyn Error>> {
    let prepared = increment_plan(store, trace, witness, ordinal)?;
    Ok(commit_canonical_existing_room(store, trace, prepared)
        .into_parts()
        .0)
}

#[test]
fn canonical_sqlite_formats_preserve_state_effects_and_durable_creation_retry() -> TestResult {
    let mut states = Vec::new();
    let mut effects = Vec::new();
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (file, store, mut trace, witness) = fixture(format)?;
        let (duplicate, _) = creation(format)?;
        assert!(matches!(
            CanonicalRoomCommitStorage::commit(&store, &duplicate.into()),
            RoomCommitResolutionV1::GenesisCreated {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        let (conflict, _) = creation(if format == CanonicalHistoryFormat::V1 {
            CanonicalHistoryFormat::V2
        } else {
            CanonicalHistoryFormat::V1
        })?;
        assert!(matches!(
            CanonicalRoomCommitStorage::commit(&store, &conflict.into()),
            RoomCommitResolutionV1::Conflict { .. }
        ));
        for ordinal in 1..=2 {
            assert!(matches!(
                increment(&store, &mut trace, &witness, ordinal)?,
                RoomCommitResolutionV1::TransitionCommitted {
                    status: ResolutionStatusV1::New,
                    ..
                }
            ));
        }
        states.push((
            trace.core_state().canonical_bytes()?,
            trace.activity_state().to_bytes()?,
        ));
        effects.push(
            trace
                .transitions()
                .iter()
                .map(|record| {
                    (
                        record.ordered_domain_events().to_vec(),
                        record.ordered_timer_changes().to_vec(),
                        record.ordered_attention_signals().to_vec(),
                    )
                })
                .collect::<Vec<_>>(),
        );
        let connection = Connection::open(file.path())?;
        let bytes: Vec<u8> = connection.query_row(
            "SELECT transition_bytes FROM transitions WHERE room_id=?1 AND room_seq=2",
            [ROOM],
            |row| row.get(0),
        )?;
        let object: serde_json::Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            object.get("resulting_core_state").is_none(),
            format == CanonicalHistoryFormat::V2
        );
        assert_eq!(
            object.get("resulting_activity_state").is_none(),
            format == CanonicalHistoryFormat::V2
        );
        assert_eq!(
            store
                .gateway_canonical_room_snapshot(&builtin_counter_registry()?, &parsed(ROOM))?
                .ok_or("canonical snapshot")?
                .trace()
                .head(),
            trace.head()
        );
    }
    assert_eq!(states[0], states[1]);
    assert_eq!(effects[0], effects[1]);
    Ok(())
}

#[test]
fn canonical_sqlite_rebuilds_absent_materializations_and_preserves_original_bytes() -> TestResult {
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (file, store, mut trace, witness) = fixture(format)?;
        increment(&store, &mut trace, &witness, 1)?;
        let connection = Connection::open(file.path())?;
        connection.execute("DELETE FROM room_snapshots WHERE room_id=?1", [ROOM])?;
        connection.execute("DELETE FROM room_materializations WHERE room_id=?1", [ROOM])?;
        let recovered = recover_canonical_room_from_full_storage(
            &store,
            &builtin_counter_registry()?,
            &parsed(ROOM),
        )?
        .ok_or("full recovery")?;
        assert_eq!(recovered.head(), trace.head());
        assert_eq!(recovered.activity_state(), trace.activity_state());
        let original: Vec<u8> = connection.query_row(
            "SELECT transition_bytes FROM transitions WHERE room_id=?1 AND room_seq=1",
            [ROOM],
            |row| row.get(0),
        )?;
        assert_eq!(original, trace.transitions()[0].canonical_bytes()?);
        assert_eq!(
            store
                .current_room_serving_fence(&parsed(ROOM))?
                .ok_or("rebuilt serving fence")?
                .head(),
            trace.head()
        );
    }
    Ok(())
}

#[test]
fn canonical_sqlite_current_reader_rejects_compact_predecessor_without_execution() -> TestResult {
    let (file, store, mut trace, witness) = fixture(CanonicalHistoryFormat::V2)?;
    increment(&store, &mut trace, &witness, 1)?;
    increment(&store, &mut trace, &witness, 2)?;
    let before = trace.activity_callback_count();
    for _ in 0..5 {
        assert_eq!(
            store
                .current_room_serving_fence(&parsed(ROOM))?
                .ok_or("warm fence")?
                .head(),
            trace.head()
        );
    }
    assert_eq!(trace.activity_callback_count(), before);
    let connection = Connection::open(file.path())?;
    connection.execute_batch("DROP TRIGGER transitions_immutable_update")?;
    connection.execute(
        "UPDATE transitions SET transition_bytes=?2 WHERE room_id=?1 AND room_seq=1",
        params![ROOM, b"{}".as_slice()],
    )?;
    assert!(matches!(
        store.current_room_serving_fence(&parsed(ROOM)),
        Err(SqliteGatewayErrorV1::Corrupt)
    ));
    Ok(())
}

#[test]
fn canonical_sqlite_corruption_precedes_missing_exact_executor() -> TestResult {
    for corrupt in [false, true] {
        let (file, store, mut trace, witness) = fixture(CanonicalHistoryFormat::V2)?;
        increment(&store, &mut trace, &witness, 1)?;
        let connection = Connection::open(file.path())?;
        connection.execute("DELETE FROM room_snapshots WHERE room_id=?1", [ROOM])?;
        if corrupt {
            connection.execute_batch("DROP TRIGGER transitions_immutable_update")?;
            connection.execute(
                "UPDATE transitions SET transition_bytes=?2 WHERE room_id=?1",
                params![ROOM, b"{}".as_slice()],
            )?;
        }
        let result = recover_canonical_room_from_full_storage(
            &store,
            &counter_v1_only_registry_for_conformance()?,
            &parsed(ROOM),
        );
        assert!(if corrupt {
            matches!(result, Err(RoomRecoveryErrorV1::Corrupt))
        } else {
            matches!(result, Err(RoomRecoveryErrorV1::RuntimeUnavailable))
        });
        assert_eq!(
            store
                .room_integrity_state(&parsed(ROOM))?
                .ok_or("integrity")?
                .status(),
            if corrupt {
                RoomIntegrityStatusV1::Quarantined
            } else {
                RoomIntegrityStatusV1::Faulted
            }
        );
    }
    Ok(())
}

#[test]
fn canonical_sqlite_recovery_install_rejects_changed_integrity_fence() -> TestResult {
    let (file, store, _trace, _witness) = fixture(CanonicalHistoryFormat::V2)?;
    arm_recovery_install_pause(&store.writer.path);
    let recovery_store = store.clone();
    let recovery = std::thread::spawn(move || {
        recover_canonical_room_from_storage(
            &recovery_store,
            &builtin_counter_registry().unwrap(),
            &parsed(ROOM),
        )
    });
    wait_until_recovery_install_pauses();
    Connection::open(file.path())?.execute(
        "UPDATE room_integrity SET generation=2 WHERE room_id=?1",
        [ROOM],
    )?;
    release_recovery_install();
    assert!(matches!(
        recovery.join().unwrap(),
        Err(RoomRecoveryErrorV1::ConcurrentChange)
    ));
    Ok(())
}

#[test]
fn canonical_sqlite_checkpoint_replays_only_tail_and_invalid_witness_falls_back() -> TestResult {
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        for force_v2_witness in [false, true] {
            let (file, store, mut trace, witness) = fixture(format)?;
            let connection = Connection::open(file.path())?;
            connection.execute(
            "UPDATE room_snapshot_schedules SET transitions_since_snapshot=249 WHERE room_id=?1",
            [ROOM],
        )?;
            increment(&store, &mut trace, &witness, 1)?;
            if force_v2_witness {
                connection.execute(
                    "DELETE FROM room_snapshot_operational_witnesses_v3 WHERE room_id=?1",
                    [ROOM],
                )?;
            }
            let at_checkpoint = recover_canonical_room_from_storage_with_receipt(
                &store,
                &builtin_counter_registry()?,
                &parsed(ROOM),
            )?
            .ok_or("zero-tail recovery")?;
            assert_eq!(at_checkpoint.trace().head(), trace.head());
            assert_eq!(at_checkpoint.trace().activity_callback_count(), 0);
            increment(&store, &mut trace, &witness, 2)?;
            connection.execute(
                "DELETE FROM room_snapshots WHERE room_id=?1 AND room_seq=2",
                [ROOM],
            )?;
            let candidate =
                CanonicalRoomRecoveryStorage::inspect_recovery_candidate(&store, &parsed(ROOM))?
                    .ok_or("checkpoint candidate")?;
            assert!(candidate.has_checkpoint());
            assert_eq!(candidate.tail_transition_count(), 1);
            let recovered = store
                .recover_canonical_room(&builtin_counter_registry()?, &parsed(ROOM))?
                .ok_or("checkpoint recovery")?;
            assert_eq!(recovered.head(), trace.head());
            assert_eq!(recovered.activity_state(), trace.activity_state());
            assert_eq!(recovered.activity_callback_count(), 1);

            // A damaged operational witness is a cache miss. The immutable
            // history remains authoritative and can rebuild complete state.
            connection.execute(
            "UPDATE room_snapshot_operational_witnesses_v3 SET witness_hash=?2 WHERE room_id=?1",
            params![ROOM, [0_u8; 32].as_slice()],
        )?;
            connection.execute(
            "UPDATE room_snapshot_operational_witnesses_v2 SET witness_hash=?2 WHERE room_id=?1",
            params![ROOM, [0_u8; 32].as_slice()],
        )?;
            let candidate =
                CanonicalRoomRecoveryStorage::inspect_recovery_candidate(&store, &parsed(ROOM))?
                    .ok_or("fallback candidate")?;
            assert!(!candidate.has_checkpoint());
            assert_eq!(candidate.tail_transition_count(), 2);
            let recovered = store
                .recover_canonical_room(&builtin_counter_registry()?, &parsed(ROOM))?
                .ok_or("full fallback")?;
            assert_eq!(recovered.head(), trace.head());
            assert_eq!(recovered.activity_state(), trace.activity_state());
            assert_eq!(recovered.activity_callback_count(), 2);
            assert_eq!(
                store
                    .room_integrity_state(&parsed(ROOM))?
                    .ok_or("integrity")?
                    .status(),
                RoomIntegrityStatusV1::Healthy
            );
        }
    }
    Ok(())
}

#[test]
fn canonical_sqlite_archive_and_no_change_preserve_receipts_through_full_recovery() -> TestResult {
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (file, store, mut trace, witness) = fixture(format)?;
        increment(&store, &mut trace, &witness, 1)?;
        for ordinal in 1..=2 {
            let request = CoreAdministrationRequestV1::new(
                parsed(ROOM),
                AdministrationOperationIdentityV1 {
                    authenticated_principal: parsed(PRINCIPAL),
                    versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                    idempotency_key: format!("canonical-archive-{ordinal}"),
                },
                CoreProposedKindV1::Archive,
                trace.head().room_seq(),
                "canonical_archive",
                CoreChangeSetV1::archive(trace.core_state().room_status()),
            )?;
            let fence = store
                .current_room_serving_fence(&parsed(ROOM))?
                .ok_or("archive fence")?;
            let prepared = PreparedCanonicalRoomCommit::for_core_administration_for_conformance(
                &trace,
                &request,
                parsed("2026-08-15T12:00:02Z"),
                parsed(&format!("01ARZ3NDEKTSV4RRFFQ67G{ordinal:04}")),
                fence.integrity().generation(),
                witness.clone(),
                fence.frame_heads(),
            )?;
            let expected = prepared
                .semantic_result()
                .canonical_receipt_bytes()
                .to_vec();
            let identity = prepared.identity().clone();
            let hash = prepared.request_hash().clone();
            let outcome = commit_canonical_existing_room(&store, &mut trace, prepared);
            assert!(if ordinal == 1 {
                matches!(
                    outcome.resolution(),
                    RoomCommitResolutionV1::TransitionCommitted { .. }
                )
            } else {
                matches!(
                    outcome.resolution(),
                    RoomCommitResolutionV1::NoChangeRecorded { .. }
                )
            });
            assert_eq!(
                outcome
                    .resolution()
                    .stored_result()
                    .ok_or("archive receipt")?
                    .canonical_receipt_bytes(),
                expected
            );
            let ResolveOutcomeV1::StoredResolution(resolved) =
                CanonicalRoomCommitStorage::resolve(&store, &identity, &hash)
            else {
                return Err("archive resolution missing".into());
            };
            assert_eq!(resolved.canonical_receipt_bytes(), expected);
        }
        assert_eq!(trace.head().room_seq().get(), 2);
        assert_eq!(trace.core_state().room_status(), RoomStatusV1::Archived);
        let connection = Connection::open(file.path())?;
        connection.execute("DELETE FROM room_materializations WHERE room_id=?1", [ROOM])?;
        connection.execute("DELETE FROM room_snapshots WHERE room_id=?1", [ROOM])?;
        let recovered = recover_canonical_room_from_full_storage(
            &store,
            &builtin_counter_registry()?,
            &parsed(ROOM),
        )?
        .ok_or("archived full replay")?;
        assert_eq!(recovered.head(), trace.head());
        assert_eq!(recovered.core_state(), trace.core_state());
        assert_eq!(recovered.activity_state(), trace.activity_state());
        let counts: (i64,i64,i64) = connection.query_row("SELECT (SELECT count(*) FROM transitions WHERE room_id=?1), (SELECT count(*) FROM semantic_receipts WHERE room_id=?1), (SELECT count(*) FROM observation_consequences WHERE room_id=?1 AND consequence_kind='visibility_lost')", [ROOM], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        assert_eq!(counts, (2, 4, 0));
    }
    Ok(())
}

#[test]
fn canonical_sqlite_unknown_commit_resolves_exact_original_without_install() -> TestResult {
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (_file, store, mut trace, witness) = fixture(format)?;
        let plan = increment_plan(&store, &trace, &witness, 1)?;
        let expected = plan.semantic_result().canonical_receipt_bytes().to_vec();
        store.set_failpoint(Some(WriteBoundary::AfterCommitUnknown));
        let outcome = commit_canonical_existing_room(&store, &mut trace, plan);
        assert!(matches!(
            outcome.resolution(),
            RoomCommitResolutionV1::Indeterminate
        ));
        assert_eq!(outcome.actor_installation(), ActorInstallationV1::Withheld);
        assert_eq!(trace.head().room_seq().get(), 0);
        let Some(CanonicalRoomPendingAttempt::ResolveOnly(pending)) = outcome.into_parts().3 else {
            return Err("unknown commit missing resolve capability".into());
        };
        store.set_failpoint(None);
        let resolved = pending.resolve(&store);
        assert_eq!(
            resolved.actor_installation(),
            ActorInstallationV1::ReloadRequired
        );
        assert_eq!(
            resolved
                .resolution()
                .stored_result()
                .ok_or("original receipt")?
                .canonical_receipt_bytes(),
            expected
        );
        assert_eq!(trace.head().room_seq().get(), 0);
        // Retry preparation still targets the same original identity/hash.
        // Durable resolution wins even after the original authority is revoked.
        store
            .seed_authority(&witness, false)
            .map_err(std::io::Error::other)?;
        let plan = increment_plan(&store, &trace, &witness, 1)?;
        let duplicate = commit_canonical_existing_room(&store, &mut trace, plan);
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
                .ok_or("duplicate receipt")?
                .canonical_receipt_bytes(),
            expected
        );
        let reloaded = store
            .recover_canonical_room(&builtin_counter_registry()?, &parsed(ROOM))?
            .ok_or("reloaded trace")?;
        assert_eq!(reloaded.head().room_seq().get(), 1);
    }
    Ok(())
}

fn member_authority(
    store: &SqliteRoomStore,
) -> Result<(AuthorityV1, PresentedCapabilityV1), Box<dyn Error>> {
    let bearer = CapabilityBearerV1::from_bytes([0xB8; 32]);
    let capability_id: CapabilityId = parsed("01ARZ3NDEKTSV4RRFFQ69G5FH3");
    let principal = PrincipalAuthoritySnapshotV1::new(
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        PrincipalAuthorityStatusV1::Enabled,
        PrincipalGenerationV1::new(1)?,
    );
    let capability = CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
        capability_id: capability_id.clone(),
        token_hash: bearer.token_hash(),
        principal_id: parsed(PRINCIPAL),
        profile: CapabilityProfileV1::RoomMember {
            room_id: parsed(ROOM),
            member_id: parsed(MEMBER),
        },
        scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::RoomAct])?,
        generation: AuthorityGenerationV1::new(1)?,
        expires_at: None,
        revoked_at: None,
    })?;
    store
        .seed_authority_snapshot(principal, capability)
        .map_err(std::io::Error::other)?;
    Ok((
        AuthorityV1::new(Arc::new(store.clone())),
        PresentedCapabilityV1::new(capability_id, bearer),
    ))
}

#[test]
#[allow(clippy::too_many_lines)]
fn canonical_sqlite_direct_retry_after_restart_needs_no_executor() -> TestResult {
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (file, store, mut trace, _) = fixture(format)?;
        let (authority, presented) = member_authority(&store)?;
        let request = ParticipantActionRequestV1::new(
            parsed(ROOM),
            parsed(MEMBER),
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FE1"),
            trace.head().room_seq(),
            "increment",
            CanonicalJsonV1::parse(br"{}")?,
        );
        let changed = ParticipantActionRequestV1::new(
            parsed(ROOM),
            parsed(MEMBER),
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FE1"),
            trace.head().room_seq(),
            "increment",
            CanonicalJsonV1::parse(br#"{"changed":true}"#)?,
        );
        // Hold purpose-sealed grants before acceptance. The adapter must still
        // revalidate them against durable current authority on every retry.
        let grant = |request: &ParticipantActionRequestV1| -> Result<ParticipantActionAuthorityV1, Box<dyn Error>> {
            match authorize_participant_action_operation(
                &authority, &store, &presented, request, parsed("2026-08-15T12:00:01Z"),
            )? {
                ParticipantActionIngressV1::Authorized(grant) => Ok(*grant),
                _ => Err("expected fresh grant".into()),
            }
        };
        let first = grant(&request)?;
        let retry = grant(&request)?;
        let collision = grant(&changed)?;
        let mismatched = grant(&request)?;
        let revoked = grant(&request)?;
        let fence = store
            .current_room_serving_fence(&parsed(ROOM))?
            .ok_or("fence")?;
        let accepted = store.commit_authorized_participant_action_from_canonical_serving_trace(
            first,
            &request,
            parsed("2026-08-15T12:00:01Z"),
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FF1"),
            &mut trace,
            fence.integrity(),
            fence.frame_heads(),
        )?;
        let original = accepted
            .stored_result()
            .ok_or("accepted receipt")?
            .canonical_receipt_bytes()
            .to_vec();
        let head = trace.head().clone();
        drop(authority);
        drop(trace);
        drop(store);
        let reopened = SqliteRoomStore::open(file.path())?;
        let missing = counter_v1_only_registry_for_conformance()?;
        assert!(missing.load_retained(&counter_v2_digest()).is_err());
        let resolved = reopened.commit_authorized_canonical_participant_action(
            &missing,
            retry,
            &request,
            parsed("2026-08-15T12:00:02Z"),
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FF2"),
        )?;
        assert!(matches!(
            resolved,
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::Existing,
                ..
            }
        ));
        assert_eq!(
            resolved
                .stored_result()
                .ok_or("retry receipt")?
                .canonical_receipt_bytes(),
            original
        );
        assert!(matches!(
            reopened.commit_authorized_canonical_participant_action(
                &missing,
                collision,
                &changed,
                parsed("2026-08-15T12:00:02Z"),
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FF3"),
            )?,
            RoomCommitResolutionV1::Conflict { .. }
        ));
        assert!(matches!(
            reopened.commit_authorized_canonical_participant_action(
                &missing,
                mismatched,
                &changed,
                parsed("2026-08-15T12:00:02Z"),
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FF4"),
            ),
            Err(SqliteParticipantActionErrorV1::Rejected)
        ));
        Connection::open(file.path())?.execute(
            "UPDATE capabilities SET authority_generation=2, revoked_at='2026-08-15T12:00:02Z' WHERE capability_id=?1",
            ["01ARZ3NDEKTSV4RRFFQ69G5FH3"],
        )?;
        assert!(matches!(
            reopened.commit_authorized_canonical_participant_action(
                &missing,
                revoked,
                &request,
                parsed("2026-08-15T12:00:03Z"),
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FF5"),
            )?,
            RoomCommitResolutionV1::Fenced
        ));
        assert_eq!(
            reopened
                .current_room_serving_fence(&parsed(ROOM))?
                .ok_or("unchanged fence")?
                .head(),
            &head
        );
        assert_eq!(
            reopened
                .room_integrity_state(&parsed(ROOM))?
                .ok_or("healthy retry")?
                .status(),
            RoomIntegrityStatusV1::Healthy
        );
    }
    Ok(())
}

#[test]
fn canonical_sqlite_fresh_input_rejects_before_reduction_and_retry_resolves_before_pack()
-> TestResult {
    let (file, store, mut trace, _witness) = fixture(CanonicalHistoryFormat::V2)?;
    let (authority, presented) = member_authority(&store)?;
    let payload = CanonicalJsonV1::parse(&serde_json::to_vec(
        &serde_json::json!({"padding":"x".repeat(32*1024)}),
    )?)?;
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FE0"),
        trace.head().room_seq(),
        "increment",
        payload,
    );
    let ParticipantActionIngressV1::Authorized(grant) = authorize_participant_action_operation(
        &authority,
        &store,
        &presented,
        &request,
        parsed("2026-08-15T12:00:01Z"),
    )?
    else {
        return Err("fresh input authorization".into());
    };
    let fence = store
        .current_room_serving_fence(&parsed(ROOM))?
        .ok_or("fresh fence")?;
    let before = trace.activity_callback_count();
    assert!(matches!(
        store.commit_authorized_participant_action_from_canonical_serving_trace(
            *grant,
            &request,
            parsed("2026-08-15T12:00:01Z"),
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FF0"),
            &mut trace,
            fence.integrity(),
            fence.frame_heads()
        ),
        Err(SqliteParticipantActionErrorV1::Rejected)
    ));
    assert_eq!(trace.activity_callback_count(), before);
    assert_eq!(trace.head().room_seq().get(), 0);
    let connection = Connection::open(file.path())?;
    assert_eq!(
        connection.query_row(
            "SELECT count(*) FROM semantic_receipts WHERE room_id=?1",
            [ROOM],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    assert_eq!(
        store
            .room_integrity_state(&parsed(ROOM))?
            .ok_or("healthy input rejection")?
            .status(),
        RoomIntegrityStatusV1::Healthy
    );

    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FE1"),
        trace.head().room_seq(),
        "increment",
        CanonicalJsonV1::parse(br"{}")?,
    );
    let ParticipantActionIngressV1::Authorized(grant) = authorize_participant_action_operation(
        &authority,
        &store,
        &presented,
        &request,
        parsed("2026-08-15T12:00:01Z"),
    )?
    else {
        return Err("normal input authorization".into());
    };
    let result = store.commit_authorized_participant_action_from_canonical_serving_trace(
        *grant,
        &request,
        parsed("2026-08-15T12:00:01Z"),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FF1"),
        &mut trace,
        fence.integrity(),
        fence.frame_heads(),
    )?;
    let original = result
        .stored_result()
        .ok_or("normal committed receipt")?
        .canonical_receipt_bytes()
        .to_vec();
    let before = trace.activity_callback_count();
    let ParticipantActionIngressV1::Existing(receipt) = authorize_participant_action_operation(
        &authority,
        &store,
        &presented,
        &request,
        parsed("2026-08-15T12:00:02Z"),
    )?
    else {
        return Err("durable retry did not resolve".into());
    };
    assert_eq!(receipt.canonical_receipt_bytes(), original);
    assert_eq!(trace.activity_callback_count(), before);
    Ok(())
}
