// Test-only checked host with generous descriptor limits. These limits let the
// selected Room policy fail independently of the retained descriptor contract.
struct BudgetProbePack {
    descriptor: &'static PackRevisionDescriptorV1,
    counts: Arc<CallbackCounts>,
    initial: CanonicalJsonV1,
    next: CanonicalJsonV1,
    events: Vec<CanonicalJsonV1>,
    projection: CanonicalJsonV1,
    observation: CanonicalJsonV1,
}

impl ActivityPackV1 for BudgetProbePack {
    fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        self.descriptor
    }

    fn initialize(
        &self,
        _input: &ActivityGenesisInputV1<'_>,
        _cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        self.counts.initialize.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(InitialOutputV1 {
            initial_activity_state: self.initial.clone(),
            timer_requests: Vec::new(),
        })
    }

    fn reduce(
        &self,
        _input: &ActivityReduceInputV1<'_>,
        _cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        self.counts.reduce.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
            next_activity_state: self.next.clone(),
            ordered_domain_events: self.events.clone(),
            timer_requests: Vec::new(),
            ordered_attention_signals: Vec::new(),
        }))
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        self.counts.view.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(PackViewV1 {
            projection_schema: self.descriptor.projection_schemas[&PackViewerClassV1::Participant]
                .schema_id
                .clone(),
            projection: self.projection.clone(),
            action_offers: if matches!(input.viewer, PackViewerV1::Participant(_)) {
                vec![ControlledPack::offer(&self.descriptor.actions[0])]
            } else {
                Vec::new()
            },
        })
    }

    fn observe(
        &self,
        _input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        self.counts.observe.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(Some(PackObservationV1::new(
            self.descriptor.observation_schemas[&PackViewerClassV1::Participant]
                .schema_id
                .clone(),
            self.observation.clone(),
        )))
    }
}

fn budget_probe_value(bytes: usize) -> Result<CanonicalJsonV1, CanonicalJsonError> {
    let value =
        CanonicalJsonV1::from_serialize(&serde_json::json!({"data": "x".repeat(bytes - 11)}))?;
    assert_eq!(value.to_bytes()?.len(), bytes);
    Ok(value)
}

fn budget_probe_host(
    initial: CanonicalJsonV1,
    next: CanonicalJsonV1,
    events: Vec<CanonicalJsonV1>,
    projection: CanonicalJsonV1,
    observation: CanonicalJsonV1,
) -> (ActivityPackHostV1, Arc<CallbackCounts>) {
    let counts = Arc::new(CallbackCounts::default());
    let mut host = checked_host_with_descriptor(
        {
            let mut descriptor = fixture().descriptor.clone();
            descriptor.limits.maximum_state_bytes = 1_048_576;
            descriptor.limits.maximum_projection_bytes = 1_048_576;
            descriptor.limits.maximum_observation_bytes = 65_536;
            descriptor.limits.maximum_events = 128;
            descriptor.limits.maximum_collection_items = 256;
            descriptor.limits.maximum_text_bytes = 1_048_576;
            descriptor
        },
        ControlledPack::good(Arc::clone(&counts)),
    );
    let mut entry = ValidatedPackEntryV1 {
        revision_lock: host.retained.0.revision_lock.clone(),
        descriptor: host.retained.0.descriptor.clone(),
        schemas: host.retained.0.schemas.clone(),
        codecs: host.retained.0.codecs.clone(),
        codec_implementation: host.retained.0.codec_implementation,
        executor_artifact_digest: host.retained.0.executor_artifact_digest.clone(),
        golden_corpus_digest: host.retained.0.golden_corpus_digest.clone(),
        executor: Arc::clone(&host.retained.0.executor),
        status: host.retained.0.status,
    };
    entry.executor = Arc::new(BudgetProbePack {
        descriptor: Box::leak(Box::new(entry.descriptor.clone())),
        counts: Arc::clone(&counts),
        initial,
        next,
        events,
        projection,
        observation,
    });
    host.retained = RetainedActivityPackV1(Arc::new(entry));
    (host, counts)
}

fn budget_probe_trace(
    host: &ActivityPackHostV1,
    format: crate::CanonicalHistoryFormat,
) -> Result<crate::CanonicalRoomTrace, Box<dyn std::error::Error>> {
    let mut request = genesis_request();
    request.pack_digest = host.retained.descriptor().revision_digest.clone();
    let genesis_input = host.initialize(&request)?;
    Ok(crate::CanonicalRoomTrace::create_uncommitted(
        PreparedNewRoomGenesisV1 {
            genesis_input,
            retained_pack: host.retained.clone(),
        },
        format,
    )?)
}

#[test]
fn selected_payload_budget_rejects_fresh_creation_before_initialize()
-> Result<(), Box<dyn std::error::Error>> {
    let (host, counts) =
        budget_probe_host(json("{}"), json("{}"), Vec::new(), json("{}"), json("{}"));
    let registry = PackRegistryV1 {
        revisions: BTreeMap::from([(
            host.retained.descriptor().revision_digest.clone(),
            Arc::clone(&host.retained.0),
        )]),
    };
    let mut request = genesis_request();
    request.pack_digest = host.retained.descriptor().revision_digest.clone();
    request.configuration = budget_probe_value(32_769)?;
    assert!(matches!(
        registry
            .prepare_genesis_for_new_room_with_format(&request, crate::CanonicalHistoryFormat::V2),
        Err(crate::CanonicalGenesisPreparationError::Input(
            crate::PayloadBudgetErrorV1::Exceeded(crate::PayloadBudgetViolationV1 {
                kind: crate::PayloadKindV1::CreationConfiguration,
                actual_bytes: 32_769,
                maximum_bytes: 32_768
            })
        ))
    ));
    assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 0);
    // The same descriptor still admits the legacy configuration.
    let _ = registry
        .prepare_genesis_for_new_room_with_format(&request, crate::CanonicalHistoryFormat::V1)?;
    assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 1);
    counts.initialize.store(0, AtomicOrdering::Relaxed);
    request.configuration = json("{}");
    request.initial_core_state = CoreRoomStateV1::active(
        (1..=900)
            .map(|index| {
                MembershipV1::new(
                    parsed(&format!("{index:026}")),
                    parsed(&format!("{:026}", index + 2_000)),
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Spectator,
                    None,
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    assert!(matches!(
        registry
            .prepare_genesis_for_new_room_with_format(&request, crate::CanonicalHistoryFormat::V2),
        Err(crate::CanonicalGenesisPreparationError::Input(
            crate::PayloadBudgetErrorV1::Exceeded(crate::PayloadBudgetViolationV1 {
                kind: crate::PayloadKindV1::CoreState,
                ..
            })
        ))
    ));
    assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn selected_payload_budget_faults_initial_pack_state_before_genesis_acceptance()
-> Result<(), Box<dyn std::error::Error>> {
    let (host, counts) = budget_probe_host(
        budget_probe_value(262_145)?,
        json("{}"),
        Vec::new(),
        json("{}"),
        json("{}"),
    );
    assert!(matches!(
        budget_probe_trace(&host, crate::CanonicalHistoryFormat::V2)
            .map_err(|error| error.downcast::<crate::TraceErrorV1>()),
        Err(Ok(error)) if matches!(&*error, crate::TraceErrorV1::Pack(PackFaultV1::OperationFault {
            operation: ActivityPackOperationV1::Initialize, fault,
        }) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(detail) if detail.contains("ActivityState")))
    ));
    assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
    let legacy = budget_probe_trace(&host, crate::CanonicalHistoryFormat::V1)?;
    assert_eq!(legacy.activity_state().to_bytes()?.len(), 262_145);
    Ok(())
}

#[test]
fn selected_payload_budget_rejects_fresh_action_before_any_view_or_reduce()
-> Result<(), Box<dyn std::error::Error>> {
    for format in [
        crate::CanonicalHistoryFormat::V1,
        crate::CanonicalHistoryFormat::V2,
    ] {
        let (host, counts) =
            budget_probe_host(json("{}"), json("{}"), Vec::new(), json("{}"), json("{}"));
        let trace = budget_probe_trace(&host, format)?;
        let mut action = participant_action(trace.head());
        action.canonical_payload = budget_probe_value(32_769)?;
        let prepared = trace.prepare(RecordedStimulusV1::ParticipantAction(action));
        if format == crate::CanonicalHistoryFormat::V2 {
            assert!(matches!(
                prepared,
                Err(crate::TraceErrorV1::PayloadBudget(
                    crate::PayloadBudgetErrorV1::Exceeded(crate::PayloadBudgetViolationV1 {
                        kind: crate::PayloadKindV1::ActionPayload,
                        ..
                    })
                ))
            ));
            assert_eq!(trace.activity_callback_count(), 0);
            assert_eq!(counts.view.load(AtomicOrdering::Relaxed), 0);
            assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
        } else {
            assert!(matches!(
                prepared?.disposition(),
                crate::CanonicalAdvanceDisposition::TransitionAccepted { .. }
            ));
            assert_eq!(trace.activity_callback_count(), 1);
        }
        assert_eq!(trace.head().room_seq().get(), 0);
    }
    Ok(())
}

#[test]
fn selected_payload_budget_faults_pack_state_and_event_outputs_before_acceptance()
-> Result<(), Box<dyn std::error::Error>> {
    let large_event = CanonicalJsonV1::from_serialize(&serde_json::json!({
        "event_type": "incremented", "data": "x".repeat(8_193 - 38),
    }))?;
    let bounded_event = CanonicalJsonV1::from_serialize(&serde_json::json!({
        "event_type": "incremented", "data": "x".repeat(8_192 - 38),
    }))?;
    assert_eq!(large_event.to_bytes()?.len(), 8_193);
    assert_eq!(bounded_event.to_bytes()?.len(), 8_192);
    for (next, events, expected_kind) in [
        (budget_probe_value(262_145)?, Vec::new(), "ActivityState"),
        (json("{}"), vec![large_event], "DomainEventItem"),
        (json("{}"), vec![bounded_event; 33], "DomainEventsArray"),
    ] {
        let (host, _) = budget_probe_host(json("{}"), next, events, json("{}"), json("{}"));
        let trace = budget_probe_trace(&host, crate::CanonicalHistoryFormat::V2)?;
        let original_head = trace.head().clone();
        assert!(matches!(
            trace.prepare(RecordedStimulusV1::ParticipantAction(participant_action(trace.head()))),
            Err(crate::TraceErrorV1::Pack(PackFaultV1::OperationFault {
                operation: ActivityPackOperationV1::Reduce, fault,
            })) if matches!(&*fault, PackFaultV1::OutputBoundExceeded(detail) if detail.contains(expected_kind))
        ));
        assert_eq!(trace.head(), &original_head);
        assert_eq!(trace.activity_state(), &json("{}"));
        assert_eq!(trace.activity_callback_count(), 1);
        assert!(trace.transitions().is_empty());
    }
    Ok(())
}

#[test]
fn selected_payload_budget_accounts_for_complete_views_and_restored_offers()
-> Result<(), Box<dyn std::error::Error>> {
    let (host, _) = budget_probe_host(
        json("{}"),
        json("{}"),
        Vec::new(),
        budget_probe_value(262_144)?,
        json("{}"),
    );
    let legacy = budget_probe_trace(&host, crate::CanonicalHistoryFormat::V1)?;
    let viewer = PackViewerV1::Participant(parsed(MEMBER));
    assert!(legacy.view(&viewer)?.canonical_bytes().len() > 262_144);
    let compact = budget_probe_trace(&host, crate::CanonicalHistoryFormat::V2)?;
    assert!(
        matches!(compact.view(&viewer), Err(crate::TraceErrorV1::Pack(PackFaultV1::OperationFault { operation: ActivityPackOperationV1::View, fault })) if matches!(*fault, PackFaultV1::OutputBoundExceeded(_)))
    );

    let (initial_host, _) =
        budget_probe_host(json("{}"), json("{}"), Vec::new(), json("{}"), json("{}"));
    let initial_trace = budget_probe_trace(&initial_host, crate::CanonicalHistoryFormat::V1)?;
    let view = initial_trace.view(&viewer)?;
    let schema = &initial_host.retained.descriptor().observation_schemas
        [&PackViewerClassV1::Participant]
        .schema_id;
    let empty_observation_bytes = canonical_observation_bytes(schema, &json("{}"), None)?.len();
    let raw_bytes = 32_768 - empty_observation_bytes + 2;
    let (host, _) = budget_probe_host(
        json("{}"),
        json("{}"),
        Vec::new(),
        json("{}"),
        budget_probe_value(raw_bytes)?,
    );
    for format in [
        crate::CanonicalHistoryFormat::V1,
        crate::CanonicalHistoryFormat::V2,
    ] {
        let trace = budget_probe_trace(&host, format)?;
        let prepared = trace.prepare(RecordedStimulusV1::ParticipantAction(participant_action(
            trace.head(),
        )))?;
        let result = trace.observe_prepared(&prepared, &viewer);
        if format == crate::CanonicalHistoryFormat::V1 {
            let ActivityObservationOutcomeV1::Observation(observation) = result? else {
                return Err("expected observation".into());
            };
            assert_eq!(observation.canonical_bytes().len(), 32_768);
            assert!(observation.action_offers().is_none());
            assert!(
                canonical_observation_bytes(
                    schema,
                    observation.observation(),
                    Some(view.action_offers())
                )?
                .len()
                    > 32_768
            );
        } else {
            assert!(
                matches!(result, Err(crate::TraceErrorV1::Pack(PackFaultV1::OperationFault { operation: ActivityPackOperationV1::Observe, fault })) if matches!(*fault, PackFaultV1::OutputBoundExceeded(_)))
            );
        }
        assert_eq!(trace.head().room_seq().get(), 0);
    }
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)] // One matrix compares both sealed creation and exact stored Genesis admission.
fn initial_complete_view_budget_is_identical_for_creation_and_full_storage_replay()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::{
        AdministrationOperationIdentityV1, CREATE_ROOM_OPERATION_KIND, CanonicalHistoryFormat,
        CanonicalRoomTrace, InitialMembershipProposalV1, PrepareRoomWriteErrorV1,
        PreparedAuthorityWitnessV1, PreparedCanonicalRoomCreation, ReplayFailureClassV1,
        RoomCreationRequestV1, RoomCreationRequestWithFormat, TraceErrorV1,
    };

    // The raw Projection fits the policy. Its complete authorized envelope,
    // including unchanged ActionOffers, exceeds the same inclusive ceiling.
    let large_projection = budget_probe_value(262_144)?;
    let (reference_host, _) = budget_probe_host(
        json("{}"),
        json("{}"),
        Vec::new(),
        large_projection.clone(),
        json("{}"),
    );
    let reference = budget_probe_trace(&reference_host, CanonicalHistoryFormat::V1)?;
    let complete_size = reference
        .view(&PackViewerV1::Participant(parsed(MEMBER)))?
        .canonical_bytes()
        .len();
    assert!(complete_size > 262_144);
    let boundary_projection = budget_probe_value(262_144 - (complete_size - 262_144))?;

    for (format, standing, projection, rejected, exact_boundary) in [
        (
            CanonicalHistoryFormat::V2,
            MembershipStandingV1::Enabled,
            large_projection.clone(),
            true,
            false,
        ),
        (
            CanonicalHistoryFormat::V2,
            MembershipStandingV1::Enabled,
            json("{}"),
            false,
            false,
        ),
        (
            CanonicalHistoryFormat::V2,
            MembershipStandingV1::Enabled,
            boundary_projection,
            false,
            true,
        ),
        (
            CanonicalHistoryFormat::V2,
            MembershipStandingV1::Suspended,
            large_projection.clone(),
            false,
            false,
        ),
        (
            CanonicalHistoryFormat::V1,
            MembershipStandingV1::Enabled,
            large_projection,
            false,
            false,
        ),
    ] {
        let (host, counts) =
            budget_probe_host(json("{}"), json("{}"), Vec::new(), projection, json("{}"));
        let registry = PackRegistryV1 {
            revisions: BTreeMap::from([(
                host.retained.descriptor().revision_digest.clone(),
                Arc::clone(&host.retained.0),
            )]),
        };
        let mut request = genesis_request();
        request.pack_digest = host.retained.descriptor().revision_digest.clone();
        request.initial_core_state = CoreRoomStateV1::active([MembershipV1::new(
            parsed(MEMBER),
            parsed(PRINCIPAL),
            PrincipalKindV1::Agent,
            standing,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )?])?;
        let prepared = registry.prepare_genesis_for_new_room_with_format(&request, format)?;
        assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(prepared.genesis_input().initial_activity_state, json("{}"));
        assert!(prepared.genesis_input().initial_timers.is_empty());
        // Keep exact bytes from the low-level test seam. This bypasses the
        // production creation seal so Replay receives the invalid stored cut.
        let trace = CanonicalRoomTrace::create_from_retained_for_conformance(
            PreparedNewRoomGenesisV1 {
                genesis_input: prepared.genesis_input().clone(),
                retained_pack: prepared.retained_pack().clone(),
            },
            format,
        )?;
        let genesis_bytes = trace.genesis_bytes()?;
        let core_bytes = CanonicalJsonV1::from_serialize(trace.core_state())?.to_bytes()?;
        let activity_bytes = trace.activity_state().to_bytes()?;
        let creation_request = RoomCreationRequestWithFormat::new(
            RoomCreationRequestV1::new(
                request.pack_digest,
                request.configuration,
                vec![InitialMembershipProposalV1::new(
                    parsed(PRINCIPAL),
                    PrincipalKindV1::Agent,
                    standing,
                    AccessModeV1::Participant,
                    Some("counter".to_owned()),
                )?],
            ),
            format,
        );
        let creation = PreparedCanonicalRoomCreation::from_registry_genesis_for_conformance(
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(PRINCIPAL),
                versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
                idempotency_key: "initial-view-budget".to_owned(),
            },
            &creation_request,
            PreparedAuthorityWitnessV1::mint_for_conformance(
                "initial-view-budget",
                parsed(PRINCIPAL),
                1,
                &json("{}"),
            )?,
            prepared,
        );
        let replay = CanonicalRoomTrace::replay_for_storage(
            &registry,
            trace.head(),
            &genesis_bytes,
            &[],
            Some(&core_bytes),
            Some(&activity_bytes),
        );
        if rejected {
            assert!(matches!(creation, Err(PrepareRoomWriteErrorV1::Trace(
                TraceErrorV1::Pack(PackFaultV1::OperationFault {
                    operation: ActivityPackOperationV1::View, fault,
                })
            )) if matches!(fault.as_ref(), PackFaultV1::OutputBoundExceeded(detail) if detail.contains("Projection"))));
            let failure = match replay {
                Err(failure) => failure,
                Ok(_) => return Err("over-limit initial View accepted by storage Replay".into()),
            };
            assert_eq!(failure.class, ReplayFailureClassV1::RuntimeFault);
            assert!(failure.detail.contains("Projection"));
            assert!(failure.last_verified_head.is_none());
        } else {
            creation?;
            let replay = replay?;
            assert_eq!(replay.final_head, *trace.head());
            assert_eq!(replay.into_trace().genesis_bytes()?, genesis_bytes);
        }
        assert_eq!(counts.initialize.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(counts.reduce.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(counts.observe.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(
            counts.view.load(AtomicOrdering::Relaxed),
            if format == CanonicalHistoryFormat::V2 && standing == MembershipStandingV1::Enabled {
                2
            } else {
                0
            }
        );
        if exact_boundary {
            assert_eq!(
                trace
                    .view(&PackViewerV1::Participant(parsed(MEMBER)))?
                    .canonical_bytes()
                    .len(),
                262_144
            );
        }
    }
    Ok(())
}
