//! Public canonical Replay and paging contracts, with exact retained executors.
use super::*;

pub(crate) fn history(
    format: CanonicalHistoryFormat,
    count: usize,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, CanonicalRoomTrace), Box<dyn Error>> {
    let mut trace = trace(format)?;
    let genesis = trace.genesis_bytes()?;
    for (index, id) in [ACTION, REPLAY_CAPABILITY, ACTION_CAPABILITY]
        .into_iter()
        .take(count)
        .enumerate()
    {
        let action_type = if index < 2 {
            "increment"
        } else {
            "private_ack"
        };
        let mut stimulus = action(&trace, id);
        if let RecordedStimulusV1::ParticipantAction(action) = &mut stimulus {
            action.action_type = action_type.into();
            action.payload_schema_digest = trace
                .retained_pack()
                .descriptor()
                .actions
                .iter()
                .find(|item| item.action_type == action_type)
                .ok_or("retained action")?
                .payload_schema
                .schema_digest
                .clone();
        }
        let request = ParticipantActionRequestV1::new(
            parsed(ROOM),
            parsed(MEMBER),
            parsed(id),
            trace.head().room_seq(),
            action_type,
            canonical(br"{}"),
        );
        let plan = PreparedCanonicalRoomCommit::for_action_for_conformance(
            &trace,
            &request,
            trace.prepare(stimulus)?,
            parsed(HOST_CAPABILITY),
            IntegrityGenerationV1::new(1)?,
            authority(),
            &BTreeMap::from([(parsed(MEMBER), 0)]),
        )?;
        assert_eq!(
            commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut trace, plan)
                .actor_installation(),
            ActorInstallationV1::Installed
        );
    }
    let records = trace
        .transitions()
        .iter()
        .map(TransitionRecord::canonical_bytes)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((genesis, records, trace))
}
fn request(sequence: u64) -> HistoricalReplayProjectionRequestV1 {
    HistoricalReplayProjectionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        RoomSequenceV1::new(sequence).unwrap_or_else(|_| unreachable!("safe sequence")),
        ReplayProjectionKindV1::HistoricalMembership,
        RoomIntegrityStateV1::new(
            RoomIntegrityStatusV1::Healthy,
            IntegrityGenerationV1::new(1).unwrap_or_else(|_| unreachable!("safe generation")),
        ),
    )
}
fn rehash(
    record: &TransitionRecord,
    state: &RoomTransitionStateV1,
    events: Vec<CanonicalJsonV1>,
    timers: Vec<TimerChangeV1>,
    attention: Vec<CanonicalJsonV1>,
    activity: &CanonicalJsonV1,
    stimulus: RecordedStimulusV1,
) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(TransitionV2::new(TransitionV2Input {
        room_id: record.room_id().clone(),
        room_seq: record.room_seq(),
        pack_digest: record.pack_digest().clone(),
        previous_lineage_hash: record.previous_lineage_hash().clone(),
        recorded_stimulus: stimulus,
        ordered_domain_events: events,
        ordered_timer_changes: timers,
        ordered_attention_signals: attention,
        resulting_core_state: state.core_state(),
        resulting_activity_state: activity,
    })?
    .canonical_bytes()?)
}

#[test]
fn canonical_replay_reproduces_every_cut_and_genuine_v1_continuation() -> TestResult {
    let registry = builtin_counter_registry()?;
    for format in [CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2] {
        let (genesis, records, expected) = history(format, 3)?;
        for cut in 0..=records.len() {
            let report = CanonicalRoomTrace::replay(&registry, &genesis, &records[..cut])?;
            assert_eq!(report.final_head.room_seq().get(), u64::try_from(cut)?);
            assert_eq!(report.activity_callback_count, cut);
            assert_eq!((report.external_effect_count, report.receipt_count), (0, 0));
            assert_eq!(report.steps.len(), cut + 1);
            assert_eq!(report.steps[0].canonical_lineage_record_bytes, genesis);
            for (step, bytes) in report.steps[1..].iter().zip(&records) {
                assert_eq!(&step.canonical_lineage_record_bytes, bytes);
            }
            if cut == records.len() {
                assert_eq!(&report.final_head, expected.head());
                assert_eq!(report.final_state().core_state(), expected.core_state());
                assert_eq!(
                    report.final_state().activity_state(),
                    expected.activity_state()
                );
                let trace = report.into_trace();
                assert_eq!(
                    trace.try_into_legacy().is_ok(),
                    format == CanonicalHistoryFormat::V1
                );
            }
        }
    }
    Ok(())
}

#[test]
fn canonical_preflight_has_no_opaque_activity_and_pages_emit_verified_materializations()
-> TestResult {
    let (genesis, records, expected) = history(CanonicalHistoryFormat::V2, 3)?;
    let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    assert!(
        preflight
            .structural_state()
            .commitment_checked_activity()
            .is_none()
    );
    for page in records.chunks(1) {
        preflight.consume_transition_page(page)?;
    }
    assert_eq!(preflight.final_head(), expected.head());
    assert_eq!(
        preflight.structural_state().core_state(),
        expected.core_state()
    );
    assert_eq!(
        preflight
            .structural_state()
            .membership_generations()
            .get(MEMBER),
        Some(&1)
    );
    assert!(
        preflight
            .structural_state()
            .commitment_checked_activity()
            .is_none()
    );
    let structure = preflight
        .finish()
        .with_activity_materialization(&expected.activity_state().to_bytes()?)?;
    assert_eq!(
        structure.commitment_checked_activity(),
        Some(expected.activity_state())
    );
    let mut wrong = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    wrong.consume_transition_page(&records)?;
    assert!(wrong.finish().with_activity_materialization(b"{}").is_err());
    let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    preflight.consume_transition_page(&records)?;
    let registry = builtin_counter_registry()?;
    let mut executable = preflight.begin_executable(expected.head(), &registry)?;
    let mut cuts = vec![executable.genesis_materialization()?];
    for page in records.chunks(1) {
        executable.consume_transition_page_with_materializations(page, &mut |step| {
            cuts.push(step.clone());
            Ok(())
        })?;
    }
    assert!(executable.genesis_materialization().is_err());
    let report = executable.finish(expected.head(), None, None)?;
    assert_eq!(report.activity_callback_count, 3);
    assert!(report.into_trace().transitions().is_empty());
    assert_eq!(cuts.len(), 4);
    for (cut, step) in cuts.iter().enumerate() {
        let replay = CanonicalRoomTrace::replay(&registry, &genesis, &records[..cut])?;
        assert_eq!(&step.head, &replay.final_head);
        assert_eq!(
            step.canonical_core_bytes,
            replay.final_state().core_state().canonical_bytes()?
        );
        assert_eq!(
            step.canonical_activity_bytes,
            replay.final_state().activity_state().to_bytes()?
        );
    }
    Ok(())
}

#[test]
fn canonical_replay_classifies_semantic_tampering_even_with_valid_rehashed_records() -> TestResult {
    let registry = builtin_counter_registry()?;
    let (genesis, records, _) = history(CanonicalHistoryFormat::V2, 1)?;
    let report = CanonicalRoomTrace::replay(&registry, &genesis, &records)?;
    let genesis_record = GenesisRecord::from_canonical_bytes(&genesis)?;
    let record = genesis_record.decode_transition(&records[0])?;
    let state = report.final_state();
    let cases = [
        (
            vec![canonical(br#"{"event":"tampered"}"#)],
            record.ordered_timer_changes().to_vec(),
            record.ordered_attention_signals().to_vec(),
            state.activity_state().clone(),
            ReplayFailureClassV1::DomainEvents,
        ),
        (
            record.ordered_domain_events().to_vec(),
            record.ordered_timer_changes().to_vec(),
            record.ordered_attention_signals().to_vec(),
            canonical(br#"{"maximum_value":4,"value":3}"#),
            ReplayFailureClassV1::ActivityState,
        ),
        (
            record.ordered_domain_events().to_vec(),
            record.ordered_timer_changes().to_vec(),
            vec![canonical(br#"{"attention":"tampered"}"#)],
            state.activity_state().clone(),
            ReplayFailureClassV1::AttentionSignals,
        ),
        (
            record.ordered_domain_events().to_vec(),
            vec![TimerChangeV1::Cancel {
                timer_id: parsed(TIMER),
                generation: TimerGenerationV1::new(1)?,
            }],
            record.ordered_attention_signals().to_vec(),
            state.activity_state().clone(),
            ReplayFailureClassV1::TimerChanges,
        ),
    ];
    for (events, timers, attention, activity, class) in cases {
        let bytes = rehash(
            &record,
            state,
            events,
            timers,
            attention,
            &activity,
            record.recorded_stimulus().clone(),
        )?;
        assert!(genesis_record.decode_transition(&bytes).is_ok());
        let failure = match CanonicalRoomTrace::replay(&registry, &genesis, &[bytes]) {
            Err(failure) => failure,
            Ok(_) => unreachable!("semantic mutation must fail"),
        };
        assert_eq!(failure.class, class);
        assert_eq!(
            failure.last_verified_head.as_deref(),
            Some(&genesis_record.complete_head())
        );
    }
    Ok(())
}

#[test]
fn canonical_missing_runtime_never_masks_structural_corruption_or_missing_history() -> TestResult {
    let missing = counter_v1_only_registry_for_conformance()?;
    let (genesis, records, expected) = history(CanonicalHistoryFormat::V2, 2)?;
    let failure = match CanonicalRoomTrace::replay(&missing, &genesis, &records) {
        Err(failure) => failure,
        Ok(_) => unreachable!("missing exact CounterV2"),
    };
    assert_eq!(failure.class, ReplayFailureClassV1::RuntimeUnavailable);
    assert_eq!(failure.last_verified_head.as_deref(), Some(expected.head()));
    let mut paged = CanonicalHistoricalReplayAccumulator::begin(&missing, &genesis, request(2))?;
    paged.consume_page(&records)?;
    assert!(matches!(
        paged.finish(),
        Err(HistoricalReplayErrorV1::ReplayFailed(
            ReplayFailureClassV1::RuntimeUnavailable
        ))
    ));
    let mut paged = CanonicalHistoricalReplayAccumulator::begin(&missing, &genesis, request(2))?;
    paged.consume_page(&records[..1])?;
    assert!(matches!(
        paged.consume_page(&[b"{}".to_vec()]),
        Err(HistoricalReplayErrorV1::ReplayFailed(_))
    ));
    let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    preflight.consume_transition_page(&records[..1])?;
    assert!(matches!(
        preflight.begin_executable(expected.head(), &missing),
        Err(ReplayFailureV1 {
            class: ReplayFailureClassV1::LineageHash,
            ..
        })
    ));
    let (_, legacy, _) = history(CanonicalHistoryFormat::V1, 1)?;
    assert!(matches!(
        CanonicalRoomTrace::replay(&missing, &genesis, &legacy),
        Err(ReplayFailureV1 {
            class: ReplayFailureClassV1::VersionOrDigest,
            ..
        })
    ));
    assert!(matches!(
        CanonicalRoomTrace::project_replayed_history(&missing, &genesis, &records[..1], request(2)),
        Err(HistoricalReplayErrorV1::SequenceUnavailable)
    ));
    Ok(())
}

#[test]
fn canonical_historical_membership_and_integrity_match_full_and_paged_projection() -> TestResult {
    let registry = builtin_counter_registry()?;
    let mut trace = trace(CanonicalHistoryFormat::V2)?;
    let genesis = trace.genesis_bytes()?;
    let membership = trace
        .core_state()
        .membership(&parsed(MEMBER))
        .ok_or("member")?;
    let request_suspend = CoreAdministrationRequestV1::new(
        parsed(ROOM),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.into(),
            idempotency_key: "history-suspend".into(),
        },
        CoreProposedKindV1::Suspend,
        trace.head().room_seq(),
        "fixture_suspend",
        CoreChangeSetV1::one(MembershipChangeV1::suspend(membership.clone())),
    )?;
    let (host, capability) = room_host_authority(ROOM);
    let grant = host.authorize_core_administration(
        &capability,
        &request_suspend,
        parsed("2026-08-15T12:00:01Z"),
    )?;
    let plan = PreparedCanonicalRoomCommit::for_authorized_core_administration(
        &trace,
        &request_suspend,
        parsed("2026-08-15T12:00:02Z"),
        parsed(HOST_CAPABILITY),
        IntegrityGenerationV1::new(1)?,
        grant,
        &BTreeMap::from([(parsed(MEMBER), 0)]),
    )?;
    assert_eq!(
        commit_canonical_existing_room(&CanonicalStorage::new([Reply::New]), &mut trace, plan)
            .actor_installation(),
        ActorInstallationV1::Installed
    );
    let records = trace
        .transitions()
        .iter()
        .map(TransitionRecord::canonical_bytes)
        .collect::<Result<Vec<_>, _>>()?;
    let historical =
        CanonicalRoomTrace::project_replayed_history(&registry, &genesis, &records, request(0))?;
    assert_eq!(
        historical.historical_membership().standing(),
        MembershipStandingV1::Enabled
    );
    let paged =
        CanonicalHistoricalReplayAccumulator::begin(&registry, &genesis, request(0))?.finish()?;
    assert_eq!(historical.canonical_envelope(), paged.canonical_envelope());
    assert!(matches!(
        CanonicalRoomTrace::project_replayed_history(&registry, &genesis, &records, request(1)),
        Err(HistoricalReplayErrorV1::HistoricalMembershipUnavailable)
    ));
    let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    preflight.consume_transition_page(&records)?;
    assert_eq!(
        preflight
            .structural_state()
            .membership_generations()
            .get(MEMBER),
        Some(&2)
    );
    let quarantined = HistoricalReplayProjectionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        RoomSequenceV1::new(0)?,
        ReplayProjectionKindV1::HistoricalMembership,
        RoomIntegrityStateV1::new(
            RoomIntegrityStatusV1::Quarantined,
            IntegrityGenerationV1::new(1)?,
        ),
    );
    assert!(matches!(
        CanonicalHistoricalReplayAccumulator::begin(&registry, &genesis, quarantined),
        Err(HistoricalReplayErrorV1::IntegrityUnavailable)
    ));
    Ok(())
}

#[test]
fn canonical_structural_timer_facts_and_rehashed_core_tampering_precede_missing_runtime()
-> TestResult {
    let mut trace = heist_trace(CanonicalHistoryFormat::V2)?;
    let genesis = trace.genesis_bytes()?;
    let input = ExternalInputV1 {
        source_id: parsed(HOST_LOBBY_LAUNCH_SOURCE),
        input_id: parsed(REPLAY_CAPABILITY),
        input_type: HOST_LAUNCH_INPUT_TYPE.into(),
        recorded_at: parsed("2026-08-23T12:01:00Z"),
        canonical_payload: canonical(br"{}"),
        immutable_resource_references: vec![],
    };
    let prepared = trace.prepare(RecordedStimulusV1::ExternalInput(input))?;
    trace.install_prepared(prepared)?;
    let timer = trace
        .scheduled_timers()
        .values()
        .min_by_key(|item| item.scheduled_for.as_str())
        .ok_or("scheduled Timer")?
        .clone();
    let prepared = trace.prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: timer.timer_id.clone(),
        generation: timer.generation,
        scheduled_for: timer.scheduled_for.clone(),
        canonical_payload: timer.canonical_payload.clone(),
    }))?;
    trace.install_prepared(prepared)?;
    let records = trace
        .transitions()
        .iter()
        .map(TransitionRecord::canonical_bytes)
        .collect::<Result<Vec<_>, _>>()?;
    let mut preflight = CanonicalStorageHistoryPreflight::begin(&genesis)?;
    preflight.consume_transition_page(&records)?;
    assert_eq!(
        preflight.structural_state().scheduled_timers(),
        trace.scheduled_timers()
    );
    assert_eq!(
        preflight
            .structural_state()
            .timer_generations()
            .get(&timer.timer_id),
        Some(&timer.generation)
    );
    assert!(
        !preflight
            .structural_state()
            .scheduled_timers()
            .contains_key(&timer.timer_id)
    );
    let registry = builtin_agent_heist_registry()?;
    let replay = CanonicalRoomTrace::replay(&registry, &genesis, &records)?;
    assert_eq!(
        replay.final_state().activity_state(),
        trace.activity_state()
    );
    let genesis_record = GenesisRecord::from_canonical_bytes(&genesis)?;
    let first = genesis_record.decode_transition(&records[0])?;
    let first_report = CanonicalRoomTrace::replay(&registry, &genesis, &records[..1])?;
    let wrong_core = CoreRoomStateV1::active(std::iter::empty::<MembershipV1>())?;
    let invalid_host = TransitionV2::new(TransitionV2Input {
        room_id: first.room_id().clone(),
        room_seq: first.room_seq(),
        pack_digest: first.pack_digest().clone(),
        previous_lineage_hash: first.previous_lineage_hash().clone(),
        recorded_stimulus: first.recorded_stimulus().clone(),
        ordered_domain_events: first.ordered_domain_events().to_vec(),
        ordered_timer_changes: first.ordered_timer_changes().to_vec(),
        ordered_attention_signals: first.ordered_attention_signals().to_vec(),
        resulting_core_state: &wrong_core,
        resulting_activity_state: first_report.final_state().activity_state(),
    })?
    .canonical_bytes()?;
    let missing = counter_v1_only_registry_for_conformance()?;
    assert!(matches!(
        CanonicalRoomTrace::replay(&missing, &genesis, &[invalid_host]),
        Err(ReplayFailureV1 {
            class: ReplayFailureClassV1::CoreState,
            ..
        })
    ));
    Ok(())
}

#[test]
fn canonical_replay_checks_ordered_attention_and_runtime_fault_class() -> TestResult {
    let registry = builtin_counter_registry()?;
    let core = CoreRoomStateV1::active([
        participant_with(MEMBER),
        MembershipV1::new(
            parsed(LATER_MEMBER),
            parsed(LATER_MEMBER),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".into()),
        )?,
        MembershipV1::new(
            parsed(ACTION),
            parsed(ACTION),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".into()),
        )?,
    ])?;
    let initial = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed(ROOM),
        pack_digest: counter_v4_digest(),
        configuration: canonical(br#"{"initial_value":0,"maximum_value":4}"#),
        room_seed: parsed(SEED),
        created_at: parsed("2026-08-15T12:00:00Z"),
        initial_core_state: core,
    })?;
    let trace = CanonicalRoomTrace::create_from_retained_for_conformance(
        initial,
        CanonicalHistoryFormat::V2,
    )?;
    let schema = trace
        .retained_pack()
        .descriptor()
        .actions
        .iter()
        .find(|item| item.action_type == "private_ack")
        .ok_or("private acknowledgement")?
        .payload_schema
        .schema_digest
        .clone();
    let prepared = trace.prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(MEMBER),
        action_id: parsed(REPLAY_CAPABILITY),
        action_type: "private_ack".into(),
        payload_schema_digest: schema,
        canonical_payload: canonical(br"{}"),
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed("2026-08-15T12:00:02Z"),
    }))?;
    let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } = prepared.disposition()
    else {
        unreachable!("accepted acknowledgement")
    };
    let state = prepared.resulting_state().ok_or("checked state")?;
    let mut attention = transition.ordered_attention_signals().to_vec();
    assert_eq!(attention.len(), 2);
    attention.reverse();
    let reordered = rehash(
        transition,
        state,
        transition.ordered_domain_events().to_vec(),
        transition.ordered_timer_changes().to_vec(),
        attention,
        state.activity_state(),
        transition.recorded_stimulus().clone(),
    )?;
    assert!(matches!(
        CanonicalRoomTrace::replay(&registry, &trace.genesis_bytes()?, &[reordered]),
        Err(ReplayFailureV1 {
            class: ReplayFailureClassV1::AttentionSignals,
            ..
        })
    ));
    let (genesis, records, _) = history(CanonicalHistoryFormat::V2, 1)?;
    let faulty =
        counter_v2_runtime_fault_registry_for_conformance(ActivityPackOperationV1::Reduce)?;
    assert!(matches!(
        CanonicalRoomTrace::replay(&faulty, &genesis, &records),
        Err(ReplayFailureV1 {
            class: ReplayFailureClassV1::RuntimeFault,
            ..
        })
    ));
    Ok(())
}
