//! Current-state and guarded recovery qualification for both immutable formats.
use super::*;
use crate::room_commit_tests::canonical_tests::history_tests;
use crate::*;
use std::{error::Error, fmt::Display, str::FromStr, sync::Mutex};
type TestResult = Result<(), Box<dyn Error>>;
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
fn parsed<T: FromStr>(value: &str) -> T
where
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture parse: {error}"))
}

struct Recovery {
    candidate: RoomRecoveryCandidateV1,
    full: RoomRecoveryCandidateV1,
    installs: Mutex<Vec<RecoveredRoomMaterializationsV1>>,
    failures: Mutex<Vec<RecoveryIntegrityDispositionV1>>,
    fenced: bool,
}
impl Recovery {
    fn new(candidate: RoomRecoveryCandidateV1) -> Self {
        Self {
            full: candidate.clone(),
            candidate,
            installs: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            fenced: false,
        }
    }
}
impl CanonicalRoomRecoveryStorage for Recovery {
    fn inspect_recovery_candidate(
        &self,
        _: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        Ok(Some(self.candidate.clone()))
    }
    fn inspect_full_recovery_candidate(
        &self,
        _: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        Ok(Some(self.full.clone()))
    }
    fn guard_recovery_install(
        &self,
        room: &RoomId,
        head: &CompleteHeadV1,
        generation: IntegrityGenerationV1,
        materials: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        assert_eq!(room, self.candidate.head.room_id());
        assert_eq!(head, &self.candidate.head);
        assert_eq!(generation, self.candidate.integrity_generation);
        if self.fenced {
            return Err(RoomRecoveryErrorV1::ConcurrentChange);
        }
        self.installs
            .lock()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .push(materials.clone());
        Ok(())
    }
    fn record_recovery_failure(
        &self,
        _: &RoomId,
        _: &CompleteHeadV1,
        _: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        self.failures
            .lock()
            .map_err(|_| RoomRecoveryErrorV1::Corrupt)?
            .push(disposition);
        Ok(())
    }
}
fn candidate(
    trace: &CanonicalRoomTrace,
    present: bool,
) -> Result<RoomRecoveryCandidateV1, Box<dyn Error>> {
    Ok(RoomRecoveryCandidateV1::new(
        trace.head().clone(),
        IntegrityGenerationV1::new(1)?,
        trace.head().canonical_bytes()?,
        trace.retained_pack().revision_lock().canonical_bytes()?,
        trace.genesis_bytes()?,
        trace
            .transitions()
            .iter()
            .map(TransitionRecord::canonical_bytes)
            .collect::<Result<Vec<_>, _>>()?,
        present
            .then(|| trace.core_state().canonical_bytes())
            .transpose()?,
        present
            .then(|| trace.activity_state().to_bytes())
            .transpose()?,
    ))
}
fn checkpoint(
    candidate: &RoomRecoveryCandidateV1,
    registry: &PackRegistryV1,
    cut: usize,
) -> Result<RoomRecoveryCandidateV1, Box<dyn Error>> {
    let records = &candidate.canonical_transition_bytes;
    let prefix = CanonicalRoomTrace::replay(
        registry,
        &candidate.canonical_genesis_bytes,
        &records[..cut],
    )?;
    let report = CanonicalRoomTrace::replay_for_storage(
        registry,
        &prefix.final_head,
        &candidate.canonical_genesis_bytes,
        &records[..cut],
        None,
        None,
    )?;
    let witnesses = report.storage_verification()?;
    let checkpoint = RoomRecoveryCheckpointV1::new(
        report.final_head.clone(),
        prefix
            .steps
            .last()
            .ok_or("prefix cut")?
            .canonical_lineage_record_bytes
            .clone(),
        report.final_state().core_state().canonical_bytes()?,
        report.final_state().activity_state().to_bytes()?,
        witnesses.timers().to_vec(),
    )
    .with_operational_witnesses(
        witnesses.observation_frames().to_vec(),
        witnesses.observation_consequences().to_vec(),
        report.membership_generations().clone(),
    )
    .with_bounded_operational_witnesses(
        witnesses
            .observation_positions()
            .iter()
            .map(|position| (position.member_id().clone(), position.frame_head()))
            .collect(),
        witnesses
            .activation_decisions()
            .iter()
            .map(|decision| {
                RecoveredActivationDecisionV1::new(
                    decision.cause_room_seq(),
                    decision.decision_id().to_owned(),
                    decision.target_member_id().cloned(),
                    decision.canonical_decision_bytes().to_vec(),
                )
            })
            .collect(),
    );
    Ok(RoomRecoveryCandidateV1::new_with_checkpoint(
        candidate.head.clone(),
        candidate.integrity_generation,
        candidate.canonical_head_bytes.clone(),
        candidate.canonical_pack_revision_lock_bytes.clone(),
        candidate.canonical_genesis_bytes.clone(),
        records[cut..].to_vec(),
        candidate.canonical_core_state_bytes.clone(),
        candidate.canonical_activity_state_bytes.clone(),
        checkpoint,
    ))
}

#[test]
fn current_verification_requires_genesis_and_exact_materializations_without_reduction() -> TestResult
{
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (genesis, records, trace) = history_tests::history(format, 2)?;
        let before = trace.activity_callback_count();
        let evidence = VerifiedCanonicalGenesis::from_canonical_bytes(&genesis)?;
        let head = trace.head();
        let bytes = records.last().ok_or("current record")?;
        let current = VerifiedCanonicalCurrentRoomMaterialization::verify_for_storage(
            &evidence,
            head,
            bytes,
            &trace.core_state().canonical_bytes()?,
            &trace.activity_state().to_bytes()?,
        )?;
        assert_eq!(current.head(), head);
        assert_eq!(current.activity_state(), trace.activity_state());
        assert_eq!(current.core_state(), trace.core_state());
        let record = VerifiedCanonicalLineageRecord::verify_for_storage(&evidence, head, bytes)?;
        let previous = evidence
            .record()
            .decode_transition(&records[0])?
            .complete_head();
        record.verify_successor(&previous)?;
        assert_eq!(
            record.verify_successor(&evidence.record().complete_head()),
            Err(RoomRecoveryErrorV1::Corrupt)
        );
        assert!(
            VerifiedCanonicalCurrentRoomMaterialization::verify_for_storage(
                &evidence,
                head,
                bytes,
                &trace.core_state().canonical_bytes()?,
                b"{}"
            )
            .is_err()
        );
        assert!(
            VerifiedCanonicalCurrentRoomMaterialization::verify_for_storage(
                &evidence,
                head,
                bytes,
                b"{}",
                &trace.activity_state().to_bytes()?
            )
            .is_err()
        );
        assert_eq!(trace.activity_callback_count(), before);
        let at_genesis = CanonicalRoomTrace::replay(&builtin_counter_registry()?, &genesis, &[])?;
        VerifiedCanonicalCurrentRoomMaterialization::verify_for_storage(
            &evidence,
            &at_genesis.final_head,
            &genesis,
            &at_genesis.final_state().core_state().canonical_bytes()?,
            &at_genesis.final_state().activity_state().to_bytes()?,
        )?;
    }
    Ok(())
}

#[test]
fn paged_operational_replay_matches_full_and_state_only_cannot_claim_witnesses() -> TestResult {
    let registry = builtin_counter_registry()?;
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (genesis, records, trace) = history_tests::history(format, 3)?;
        let full = CanonicalRoomTrace::replay_for_storage(
            &registry,
            trace.head(),
            &genesis,
            &records,
            None,
            None,
        )?;
        assert!(full.steps.is_empty());
        let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
        for page in records.chunks(1) {
            preflight.consume_transition_page(page)?;
        }
        let mut replay = preflight.begin_executable_with_observations(trace.head(), &registry)?;
        for page in records.chunks(1) {
            replay.consume_transition_page(page)?;
        }
        let paged = replay.finish(trace.head(), None, None)?;
        assert_eq!(paged.storage_verification()?, full.storage_verification()?);
        assert!(paged.into_trace().transitions().is_empty());
        let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
        preflight.consume_transition_page(&records)?;
        let mut replay = preflight.begin_executable(trace.head(), &registry)?;
        replay.consume_transition_page(&records)?;
        assert!(
            replay
                .finish(trace.head(), None, None)?
                .storage_verification()
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn full_and_checkpoint_recovery_rebuild_absent_materials_and_preserve_operational_witnesses()
-> TestResult {
    let registry = builtin_counter_registry()?;
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (_, _, expected) = history_tests::history(format, 3)?;
        let full = candidate(&expected, false)?;
        let storage = Recovery::new(full.clone());
        let execution =
            recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
                .ok_or("full recovery")?;
        assert!(!execution.receipt().used_checkpoint());
        assert_eq!(execution.trace().head(), expected.head());
        assert_eq!(
            execution.trace().activity_state(),
            expected.activity_state()
        );
        let expected_materials = storage.installs.lock().map_err(|_| "install lock")?[0].clone();
        let mut storage = Recovery::new(checkpoint(&full, &registry, 1)?);
        storage.full = full;
        let execution =
            recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
                .ok_or("checkpoint recovery")?;
        assert!(execution.receipt().used_checkpoint());
        assert_eq!(execution.trace().activity_callback_count(), 2);
        assert_eq!(
            execution.trace().activity_state(),
            expected.activity_state()
        );
        let actual = storage.installs.lock().map_err(|_| "install lock")?[0].clone();
        assert_eq!(
            actual.canonical_core_state_bytes(),
            expected_materials.canonical_core_state_bytes()
        );
        assert_eq!(
            actual.canonical_activity_state_bytes(),
            expected_materials.canonical_activity_state_bytes()
        );
        assert_eq!(actual.timers(), expected_materials.timers());
        assert_eq!(
            actual.observation_frames(),
            expected_materials.observation_frames()
        );
        assert_eq!(
            actual.observation_consequences(),
            expected_materials.observation_consequences()
        );
        assert_eq!(
            actual.observation_frame_heads(),
            expected_materials.observation_frame_heads()
        );
        assert_eq!(
            actual.membership_generations(),
            expected_materials.membership_generations()
        );
        assert_eq!(
            actual.activation_decisions(),
            expected_materials.activation_decisions()
        );
    }
    Ok(())
}

#[test]
fn malformed_checkpoint_falls_back_and_install_fences_prevent_actor_release() -> TestResult {
    let registry = builtin_counter_registry()?;
    let (_, _, expected) = history_tests::history(CanonicalHistoryFormat::V2, 2)?;
    let full = candidate(&expected, false)?;
    let mut storage = Recovery::new(checkpoint(&full, &registry, 1)?);
    storage.full = full;
    storage
        .candidate
        .checkpoint
        .as_mut()
        .ok_or("checkpoint")?
        .core_state_bytes = b"{}".to_vec();
    let execution =
        recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
            .ok_or("fallback")?;
    assert!(!execution.receipt().used_checkpoint());
    assert!(
        storage
            .failures
            .lock()
            .map_err(|_| "failure lock")?
            .is_empty()
    );
    storage.fenced = true;
    assert!(matches!(
        recover_canonical_room_from_storage(&storage, &registry, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::ConcurrentChange)
    ));
    assert_eq!(
        storage.installs.lock().map_err(|_| "install lock")?.len(),
        1
    );
    Ok(())
}

#[test]
fn missing_runtime_is_faulted_and_structural_corruption_is_quarantined_first() -> TestResult {
    let (_, _, trace) = history_tests::history(CanonicalHistoryFormat::V2, 2)?;
    let missing = counter_v1_only_registry_for_conformance()?;
    let mut storage = Recovery::new(candidate(&trace, false)?);
    assert!(matches!(
        recover_canonical_room_from_storage(&storage, &missing, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::RuntimeUnavailable)
    ));
    assert_eq!(
        *storage.failures.lock().map_err(|_| "failure lock")?,
        [RecoveryIntegrityDispositionV1::Faulted]
    );
    storage.failures.lock().map_err(|_| "failure lock")?.clear();
    let faulty =
        counter_v2_runtime_fault_registry_for_conformance(ActivityPackOperationV1::Reduce)?;
    assert!(matches!(
        recover_canonical_room_from_storage(&storage, &faulty, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::RuntimeFault)
    ));
    assert_eq!(
        *storage.failures.lock().map_err(|_| "failure lock")?,
        [RecoveryIntegrityDispositionV1::Faulted]
    );
    storage.failures.lock().map_err(|_| "failure lock")?.clear();
    storage.candidate.canonical_core_state_bytes = Some(b"{}".to_vec());
    assert!(matches!(
        recover_canonical_room_from_storage(&storage, &missing, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::Corrupt)
    ));
    assert_eq!(
        *storage.failures.lock().map_err(|_| "failure lock")?,
        [RecoveryIntegrityDispositionV1::Quarantined]
    );
    Ok(())
}

#[test]
fn canonical_timer_and_activation_witnesses_match_full_paged_and_checkpoint_recovery() -> TestResult
{
    let registry = builtin_agent_heist_registry()?;
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let mut trace = crate::room_commit_tests::canonical_tests::heist_trace(format)?;
        let stimulus = RecordedStimulusV1::ExternalInput(ExternalInputV1 {
            source_id: parsed(HOST_LOBBY_LAUNCH_SOURCE),
            input_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC9"),
            input_type: HOST_LAUNCH_INPUT_TYPE.into(),
            recorded_at: parsed("2026-08-23T12:01:00Z"),
            canonical_payload: CanonicalJsonV1::parse(b"{}")?,
            immutable_resource_references: vec![],
        });
        let prepared = trace.prepare(stimulus)?;
        trace.install_prepared(prepared)?;
        let timer = trace
            .scheduled_timers()
            .values()
            .min_by_key(|timer| timer.scheduled_for.as_str())
            .ok_or("Timer")?
            .clone();
        let prepared = trace.prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: timer.timer_id,
            generation: timer.generation,
            scheduled_for: timer.scheduled_for,
            canonical_payload: timer.canonical_payload,
        }))?;
        trace.install_prepared(prepared)?;
        let full = candidate(&trace, false)?;
        let report = CanonicalRoomTrace::replay_for_storage(
            &registry,
            trace.head(),
            &full.canonical_genesis_bytes,
            &full.canonical_transition_bytes,
            None,
            None,
        )?;
        let expected = report.storage_verification()?;
        assert!(
            expected
                .timers()
                .iter()
                .any(|timer| timer.state() == RecoveredTimerStateV1::Fired)
        );
        assert!(!expected.activation_decisions().is_empty());
        let mut preflight = CanonicalStorageHistoryPreflight::begin(&full.canonical_genesis_bytes)?;
        preflight.consume_transition_page(&full.canonical_transition_bytes)?;
        assert_eq!(
            preflight.structural_state().timer_materializations(),
            expected.timers()
        );
        assert_eq!(
            preflight.structural_state().activation_decisions().len(),
            expected.activation_decisions().len()
        );
        for (actual, expected) in preflight
            .structural_state()
            .activation_decisions()
            .iter()
            .zip(expected.activation_decisions())
        {
            assert_eq!(
                actual.canonical_decision_bytes(),
                expected.canonical_decision_bytes()
            );
        }
        assert!(
            preflight
                .structural_state()
                .commitment_checked_activity()
                .is_none()
        );
        let mut replay = preflight.begin_executable_with_observations(trace.head(), &registry)?;
        for page in full.canonical_transition_bytes.chunks(1) {
            replay.consume_transition_page(page)?;
        }
        assert_eq!(
            replay
                .finish(trace.head(), None, None)?
                .storage_verification()?,
            expected
        );
        let mut storage = Recovery::new(checkpoint(&full, &registry, 1)?);
        storage.full = full;
        let execution =
            recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
                .ok_or("Timer checkpoint")?;
        assert!(execution.receipt().used_checkpoint());
        let materials = &storage.installs.lock().map_err(|_| "install lock")?[0];
        assert_eq!(materials.timers(), expected.timers());
        assert_eq!(
            materials.activation_decisions().len(),
            expected.activation_decisions().len()
        );
        for (actual, expected) in materials
            .activation_decisions()
            .iter()
            .zip(expected.activation_decisions())
        {
            assert_eq!(
                actual.canonical_decision_bytes(),
                expected.canonical_decision_bytes()
            );
        }
    }
    Ok(())
}

#[test]
fn compact_checkpoint_extends_guard_only_rolling_and_mmr_proofs() -> TestResult {
    let registry = builtin_counter_registry()?;
    let (_, _, trace) = history_tests::history(CanonicalHistoryFormat::V2, 3)?;
    let full = candidate(&trace, false)?;
    let prefix = CanonicalRoomTrace::replay(
        &registry,
        &full.canonical_genesis_bytes,
        &full.canonical_transition_bytes[..1],
    )?;
    let prefix = CanonicalRoomTrace::replay_for_storage(
        &registry,
        &prefix.final_head,
        &full.canonical_genesis_bytes,
        &full.canonical_transition_bytes[..1],
        None,
        None,
    )?;
    let whole = CanonicalRoomTrace::replay_for_storage(
        &registry,
        trace.head(),
        &full.canonical_genesis_bytes,
        &full.canonical_transition_bytes,
        None,
        None,
    )?;
    let decisions = |report: &CanonicalReplayReport| -> Result<Vec<RecoveredActivationDecisionV1>, Box<dyn Error>> { Ok(report.storage_verification()?.activation_decisions().iter().map(|decision| RecoveredActivationDecisionV1::new(decision.cause_room_seq(), decision.decision_id().into(), decision.target_member_id().cloned(), decision.canonical_decision_bytes().to_vec())).collect()) };
    let rolling = OPERATIONAL_HISTORY_ROOT_DOMAINS_V2
        .into_iter()
        .map(|domain| {
            Ok((
                domain.to_owned(),
                OperationalHistoryRootV2::new(domain, 0, Blake3DigestV1::from_bytes([0; 32]))?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn Error>>>()?;
    let mmr = OPERATIONAL_HISTORY_ROOT_DOMAINS_V2
        .into_iter()
        .map(|domain| Ok((domain.to_owned(), OperationalMmrReceiptV1::empty(domain)?)))
        .collect::<Result<BTreeMap<_, _>, Box<dyn Error>>>()?;
    let mut accelerated = checkpoint(&full, &registry, 1)?;
    let checkpoint = accelerated.checkpoint.as_mut().ok_or("checkpoint")?;
    checkpoint.operational_history_roots = Some(advance_operational_history_roots(
        &rolling,
        prefix.observation_consequences(),
        &decisions(&prefix)?,
    )?);
    checkpoint.operational_mmr_receipts = Some(advance_operational_mmr_receipts(
        &mmr,
        prefix.observation_consequences(),
        &decisions(&prefix)?,
    )?);
    let mut storage = Recovery::new(accelerated);
    storage.full = full;
    assert!(
        recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
            .ok_or("proof recovery")?
            .receipt()
            .used_checkpoint()
    );
    let materials = &storage.installs.lock().map_err(|_| "install lock")?[0];
    assert_eq!(
        materials.operational_history_roots(),
        Some(&advance_operational_history_roots(
            &rolling,
            whole.observation_consequences(),
            &decisions(&whole)?
        )?)
    );
    assert_eq!(
        materials.operational_mmr_receipts(),
        Some(&advance_operational_mmr_receipts(
            &mmr,
            whole.observation_consequences(),
            &decisions(&whole)?
        )?)
    );
    Ok(())
}

#[cfg(feature = "conformance-tracer")]
#[test]
fn eligible_compact_checkpoint_executes_at_most_250_tail_records() -> TestResult {
    let registry = benchmark_state_registry_for_conformance()?;
    let digest = registry
        .catalog_revisions()
        .next()
        .ok_or("benchmark revision")?
        .revision_digest;
    let member: MemberId = parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0");
    let principal: PrincipalId = parsed("01ARZ3NDEKTSV4RRFFQ69G5FD0");
    let prepared = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed(ROOM),
        pack_digest: digest,
        configuration: CanonicalJsonV1::from_serialize(
            &serde_json::json!({"state_bytes":1024,"maximum_counter":251}),
        )?,
        room_seed: parsed("hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
        created_at: parsed("2026-08-15T12:00:00Z"),
        initial_core_state: CoreRoomStateV1::active([MembershipV1::new(
            member.clone(),
            principal,
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".into()),
        )?])?,
    })?;
    let mut trace = CanonicalRoomTrace::create_from_retained_for_conformance(
        prepared,
        CanonicalHistoryFormat::V2,
    )?;
    let schema = trace.retained_pack().descriptor().actions[0]
        .payload_schema
        .schema_digest
        .clone();
    for index in 0..251 {
        let prepared =
            trace.prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
                member_id: member.clone(),
                action_id: parsed(&format!("{index:026X}")),
                action_type: "increment".into(),
                payload_schema_digest: schema.clone(),
                canonical_payload: CanonicalJsonV1::parse(b"{}")?,
                exact_basis_head: trace.head().clone(),
                admitted_at: parsed("2026-08-15T12:00:01Z"),
            }))?;
        trace.install_prepared(prepared)?;
    }
    let full = candidate(&trace, false)?;
    let mut at_limit = Recovery::new(checkpoint(&full, &registry, 1)?);
    at_limit.full = full.clone();
    let execution =
        recover_canonical_room_from_storage_with_receipt(&at_limit, &registry, &parsed(ROOM))?
            .ok_or("250 tail")?;
    assert!(execution.receipt().used_checkpoint());
    assert_eq!(execution.trace().activity_callback_count(), 250);
    let mut over_limit = Recovery::new(checkpoint(&full, &registry, 0)?);
    over_limit.full = full;
    let execution =
        recover_canonical_room_from_storage_with_receipt(&over_limit, &registry, &parsed(ROOM))?
            .ok_or("251 full fallback")?;
    assert!(!execution.receipt().used_checkpoint());
    assert_eq!(execution.trace().activity_callback_count(), 251);
    assert!(
        over_limit
            .failures
            .lock()
            .map_err(|_| "failure lock")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn membership_generation_and_visibility_witnesses_survive_compact_checkpoint_tail() -> TestResult {
    let registry = builtin_counter_registry()?;
    let (_, _, mut trace) = history_tests::history(CanonicalHistoryFormat::V2, 1)?;
    let member: MemberId = parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0");
    let principal: PrincipalId = parsed("01ARZ3NDEKTSV4RRFFQ69G5FD0");
    let before = trace
        .core_state()
        .membership(&member)
        .ok_or("Membership")?
        .clone();
    let prepared = trace.prepare(RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        CoreProposedKindV1::Suspend,
        CoreAuthorityAttributionV1 {
            principal_id: principal.clone(),
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: principal,
            versioned_operation_kind: CORE_OPERATION_KIND.into(),
            idempotency_key: "recovery-suspend".into(),
        },
        trace.head().room_seq(),
        "fixture_suspend",
        parsed("2026-08-15T12:00:03Z"),
        CoreChangeSetV1::one(MembershipChangeV1::suspend(before)),
    )))?;
    trace.install_prepared(prepared)?;
    let full = candidate(&trace, false)?;
    let full_storage = Recovery::new(full.clone());
    recover_canonical_room_from_storage(&full_storage, &registry, &parsed(ROOM))?
        .ok_or("full Membership recovery")?;
    let mut storage = Recovery::new(checkpoint(&full, &registry, 1)?);
    storage.full = full;
    let execution =
        recover_canonical_room_from_storage_with_receipt(&storage, &registry, &parsed(ROOM))?
            .ok_or("Membership checkpoint")?;
    assert!(execution.receipt().used_checkpoint());
    let actual = &storage.installs.lock().map_err(|_| "install lock")?[0];
    let expected = &full_storage.installs.lock().map_err(|_| "install lock")?[0];
    assert_eq!(
        actual
            .membership_generations()
            .and_then(|generations| generations.get(member.as_str())),
        Some(&2)
    );
    assert_eq!(
        actual.membership_generations(),
        expected.membership_generations()
    );
    assert_eq!(
        actual.observation_consequences(),
        expected.observation_consequences()
    );
    assert_eq!(
        actual.observation_frame_heads(),
        expected.observation_frame_heads()
    );
    assert!(!actual.observation_consequences().is_empty());
    Ok(())
}

#[test]
fn genesis_selected_policy_rejects_oversized_recorded_facts_before_runtime_lookup() -> TestResult {
    let missing = counter_v1_only_registry_for_conformance()?;
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (genesis, _, trace) = history_tests::history(format, 1)?;
        let original = trace.transitions().last().ok_or("Transition")?;
        for (payload, events) in [
            (
                Some(CanonicalJsonV1::from_serialize(&"x".repeat(32_767))?),
                original.ordered_domain_events().to_vec(),
            ),
            (
                None,
                vec![CanonicalJsonV1::from_serialize(&"x".repeat(8_191))?],
            ),
            (
                None,
                vec![CanonicalJsonV1::from_serialize(&"x".repeat(4_998))?; 55],
            ),
        ] {
            let mut stimulus = original.recorded_stimulus().clone();
            if let (Some(payload), RecordedStimulusV1::ParticipantAction(action)) =
                (payload, &mut stimulus)
            {
                action.canonical_payload = payload;
            }
            let record = match original {
                TransitionRecord::V1(original) => {
                    let mut record = original.clone();
                    record.recorded_stimulus = stimulus;
                    record.ordered_domain_events = events;
                    record.transition_hash = record.calculate_hash()?;
                    TransitionRecord::V1(record)
                }
                TransitionRecord::V2(_) => {
                    TransitionRecord::V2(TransitionV2::new(TransitionV2Input {
                        room_id: original.room_id().clone(),
                        room_seq: original.room_seq(),
                        pack_digest: original.pack_digest().clone(),
                        previous_lineage_hash: original.previous_lineage_hash().clone(),
                        recorded_stimulus: stimulus,
                        ordered_domain_events: events,
                        ordered_timer_changes: original.ordered_timer_changes().to_vec(),
                        ordered_attention_signals: original.ordered_attention_signals().to_vec(),
                        resulting_core_state: trace.core_state(),
                        resulting_activity_state: trace.activity_state(),
                    })?)
                }
            };
            let bytes = record.canonical_bytes()?;
            let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
            let result = preflight.consume_transition_page(std::slice::from_ref(&bytes));
            if format == CanonicalHistoryFormat::V1 {
                result?;
                continue;
            }
            assert!(result.is_err());
            let evidence = VerifiedCanonicalGenesis::from_canonical_bytes(&genesis)?;
            assert!(
                VerifiedCanonicalLineageRecord::verify_for_storage(
                    &evidence,
                    &record.complete_head(),
                    &bytes
                )
                .is_err()
            );
            let storage = Recovery::new(RoomRecoveryCandidateV1::new(
                record.complete_head(),
                IntegrityGenerationV1::new(1)?,
                record.complete_head().canonical_bytes()?,
                trace.retained_pack().revision_lock().canonical_bytes()?,
                genesis.clone(),
                vec![bytes],
                None,
                None,
            ));
            assert!(matches!(
                recover_canonical_room_from_storage(&storage, &missing, &parsed(ROOM)),
                Err(RoomRecoveryErrorV1::Corrupt)
            ));
            assert_eq!(
                *storage.failures.lock().map_err(|_| "failure lock")?,
                [RecoveryIntegrityDispositionV1::Quarantined]
            );
        }
    }
    Ok(())
}
