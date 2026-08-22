use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Display,
    str::FromStr,
    sync::{Arc, Mutex},
};

use super::*;

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC4";
const ROOM_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC6";
const REPLAY_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC9";
const ACTION_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FCA";
const LATER_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture {value}: {error}"))
}

fn canonical(value: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(value)
        .unwrap_or_else(|error| unreachable!("fixture canonical JSON: {error}"))
}

fn commit_room_creation(
    storage: &dyn RoomCommitStorageV1,
    prepared: PreparedRoomCreationV1,
) -> RoomCreationCommitOutcomeV1 {
    super::commit_room_creation(storage, prepared)
}

fn commit_existing_room(
    storage: &dyn RoomCommitStorageV1,
    trace: &mut CoreTraceV1,
    prepared: PreparedRoomCommitV1,
) -> ExistingRoomCommitOutcomeV1 {
    super::commit_existing_room(storage, trace, prepared)
}

fn participant_with(member_id: &str) -> MembershipV1 {
    MembershipV1::new(
        parsed(member_id),
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("fixture membership: {error}"))
}

fn creation_request(initial_value: u32) -> RoomCreationRequestV1 {
    RoomCreationRequestV1::new(
        counter_v2_digest(),
        canonical(format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes()),
        vec![
            InitialMembershipProposalV1::new(
                parsed(PRINCIPAL),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".to_owned()),
            )
            .unwrap_or_else(|error| unreachable!("fixture proposal: {error}")),
        ],
    )
}

fn authority() -> PreparedAuthorityWitnessV1 {
    authority_with("room-commit-test-authority", parsed(PRINCIPAL), 1)
}

fn authority_with(
    witness_id: &str,
    authenticated_principal: PrincipalId,
    generation: u64,
) -> PreparedAuthorityWitnessV1 {
    PreparedAuthorityWitnessV1::new(
        witness_id,
        authenticated_principal,
        generation,
        &canonical(br#"{"scope":"room-commit-test","revoked":false}"#),
    )
    .unwrap_or_else(|error| unreachable!("fixture authority: {error}"))
}

fn creation_identity() -> AdministrationOperationIdentityV1 {
    AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(PRINCIPAL),
        versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: "room-commit-test-create".to_owned(),
    }
}

fn counter_genesis(initial_value: u32) -> PreparedNewRoomGenesisV1 {
    counter_genesis_with(initial_value, ROOM, MEMBER, SEED, "2026-08-15T12:00:00Z")
}

fn counter_genesis_with(
    initial_value: u32,
    room_id: &str,
    member_id: &str,
    room_seed: &str,
    created_at: &str,
) -> PreparedNewRoomGenesisV1 {
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
    let request = PackGenesisRequestV1 {
        room_id: parsed(room_id),
        pack_digest: counter_v2_digest(),
        configuration: canonical(
            format!(r#"{{"initial_value":{initial_value},"maximum_value":4}}"#).as_bytes(),
        ),
        room_seed: parsed(room_seed),
        created_at: parsed(created_at),
        initial_core_state: CoreRoomStateV1::active([participant_with(member_id)])
            .unwrap_or_else(|error| unreachable!("fixture Core state: {error}")),
    };
    registry
        .prepare_genesis_for_new_room(&request)
        .unwrap_or_else(|error| unreachable!("fixture Counter Genesis: {error}"))
}

fn creation_plan(initial_value: u32) -> PreparedRoomCreationV1 {
    PreparedRoomCreationV1::from_registry_genesis_for_conformance(
        creation_identity(),
        &creation_request(initial_value),
        authority(),
        counter_genesis(initial_value),
    )
    .unwrap_or_else(|error| unreachable!("fixture Create plan: {error}"))
}

fn counter_trace() -> CoreTraceV1 {
    CoreTraceV1::create_uncommitted(counter_genesis(0))
        .unwrap_or_else(|error| unreachable!("fixture Counter trace: {error}"))
}

fn host_authority(room_id: Option<&str>) -> (AuthorityV1, PresentedCapabilityV1) {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    store
        .seed_principal(PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("fixture Principal: {error}"));
    let bearer = CapabilityBearerV1::from_bytes([19; 32]);
    let scopes = CapabilityScopeSetV1::new([CapabilityScopeV1::OperatorRoomAdmin])
        .unwrap_or_else(|error| unreachable!("fixture scope: {error}"));
    store
        .seed_capability(
            CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                capability_id: parsed(HOST_CAPABILITY),
                token_hash: bearer.token_hash(),
                principal_id: parsed(PRINCIPAL),
                profile: CapabilityProfileV1::HostOperator {
                    room_id: room_id.map(parsed),
                },
                scopes,
                generation: AuthorityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
                expires_at: None,
                revoked_at: None,
            })
            .unwrap_or_else(|error| unreachable!("fixture Capability: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("seed Capability: {error}"));
    (
        AuthorityV1::new(store),
        PresentedCapabilityV1::new(parsed(HOST_CAPABILITY), bearer),
    )
}

fn room_host_authority(room_id: &str) -> (AuthorityV1, PresentedCapabilityV1) {
    host_authority(Some(room_id))
}

fn global_host_authority() -> (AuthorityV1, PresentedCapabilityV1) {
    host_authority(None)
}

fn replay_authority(
    member_id: &str,
    expires_at: Option<&str>,
) -> (AuthorityV1, PresentedCapabilityV1) {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    store
        .seed_principal(PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("fixture Principal: {error}"));
    let membership = if member_id == MEMBER {
        participant_with(member_id)
    } else {
        MembershipV1::new(
            parsed(member_id),
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("fixture later Membership: {error}"))
    };
    store
        .seed_membership(MembershipAuthoritySnapshotV1::new(
            parsed(ROOM),
            membership,
            MembershipGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("fixture Membership: {error}"));
    let bearer = CapabilityBearerV1::from_bytes([29; 32]);
    store
        .seed_capability(
            CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                capability_id: parsed(REPLAY_CAPABILITY),
                token_hash: bearer.token_hash(),
                principal_id: parsed(PRINCIPAL),
                profile: CapabilityProfileV1::RoomMember {
                    room_id: parsed(ROOM),
                    member_id: parsed(member_id),
                },
                scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::RoomReplay])
                    .unwrap_or_else(|error| unreachable!("fixture scope: {error}")),
                generation: AuthorityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
                expires_at: expires_at.map(parsed),
                revoked_at: None,
            })
            .unwrap_or_else(|error| unreachable!("fixture Capability: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("seed Capability: {error}"));
    (
        AuthorityV1::new(store),
        PresentedCapabilityV1::new(parsed(REPLAY_CAPABILITY), bearer),
    )
}

fn action_ingress_authority(access_mode: AccessModeV1) -> (AuthorityV1, PresentedCapabilityV1) {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    store
        .seed_principal(PrincipalAuthoritySnapshotV1::new(
            parsed(PRINCIPAL),
            PrincipalKindV1::Human,
            PrincipalAuthorityStatusV1::Enabled,
            PrincipalGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("fixture Principal: {error}"));
    let role = (access_mode == AccessModeV1::Participant).then(|| "counter".to_owned());
    let membership = MembershipV1::new(
        parsed(MEMBER),
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        access_mode,
        role,
    )
    .unwrap_or_else(|error| unreachable!("fixture Membership: {error}"));
    store
        .seed_membership(MembershipAuthoritySnapshotV1::new(
            parsed(ROOM),
            membership,
            MembershipGenerationV1::new(2)
                .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("seed Membership: {error}"));
    let bearer = CapabilityBearerV1::from_bytes([31; 32]);
    store
        .seed_capability(
            CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                capability_id: parsed(ACTION_CAPABILITY),
                token_hash: bearer.token_hash(),
                principal_id: parsed(PRINCIPAL),
                profile: CapabilityProfileV1::RoomMember {
                    room_id: parsed(ROOM),
                    member_id: parsed(MEMBER),
                },
                scopes: CapabilityScopeSetV1::new([CapabilityScopeV1::RoomAct])
                    .unwrap_or_else(|error| unreachable!("fixture scope: {error}")),
                generation: AuthorityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture generation: {error}")),
                expires_at: None,
                revoked_at: None,
            })
            .unwrap_or_else(|error| unreachable!("fixture Capability: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("seed Capability: {error}"));
    (
        AuthorityV1::new(store),
        PresentedCapabilityV1::new(parsed(ACTION_CAPABILITY), bearer),
    )
}

fn administrative_archive_plan(
    trace: &CoreTraceV1,
    room_id: &str,
    idempotency_key: &str,
    transition_id: &str,
) -> (CoreAdministrationRequestV1, PreparedRoomCommitV1) {
    let request = CoreAdministrationRequestV1::new(
        parsed(room_id),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        CoreProposedKindV1::Archive,
        trace.head().room_seq(),
        "fixture_archive",
        CoreChangeSetV1::archive(trace.core_state().room_status()),
    )
    .unwrap_or_else(|error| unreachable!("fixture admin request: {error}"));
    let (authority, presented) = room_host_authority(room_id);
    let grant = authority
        .authorize_core_administration(&presented, &request, parsed("2026-08-15T12:00:01Z"))
        .unwrap_or_else(|error| unreachable!("fixture admin authority: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member_id| (member_id, 0))
        .collect();
    let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
        trace,
        &request,
        parsed("2026-08-15T12:00:02Z"),
        parsed(transition_id),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity: {error}")),
        grant,
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture admin plan: {error}"));
    (request, prepared)
}

fn administrative_membership_plan(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    idempotency_key: &str,
    transition_id: &str,
) -> PreparedRoomCommitV1 {
    let request = CoreAdministrationRequestV1::new(
        trace.head().room_id().clone(),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        kind,
        trace.head().room_seq(),
        "fixture_membership_change",
        changeset,
    )
    .unwrap_or_else(|error| unreachable!("fixture Membership request: {error}"));
    let (authority, presented) = room_host_authority(trace.head().room_id().to_string().as_str());
    let grant = authority
        .authorize_core_administration(&presented, &request, parsed("2026-08-15T12:00:01Z"))
        .unwrap_or_else(|error| unreachable!("fixture Membership authority: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member_id| (member_id, 0))
        .collect();
    PreparedRoomCommitV1::for_authorized_core_administration(
        trace,
        &request,
        parsed("2026-08-15T12:00:02Z"),
        parsed(transition_id),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity: {error}")),
        grant,
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture Membership plan: {error}"))
}

fn advance_persistence(plan: &PreparedRoomCommitV1) -> &PreparedAdvancePersistenceV1 {
    let PreparedExistingIntentV1::Advance(persistence) = plan.intent() else {
        unreachable!("fixture plan advances the Room")
    };
    persistence
}

#[test]
fn prepared_advance_classifies_frames_resets_and_visibility_loss_as_delivery_consequences() {
    let (_action_trace, action_plan) = action_plan();
    let action_persistence = advance_persistence(&action_plan);
    assert!(matches!(
        action_persistence.delivery_consequences.as_slice(),
        [PreparedObservationConsequenceV1::ObservationFrame(frame)]
            if frame.member_id() == &parsed(MEMBER)
    ));

    let reset_trace = counter_trace();
    let member = reset_trace
        .core_state()
        .membership(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("fixture Membership"))
        .clone();
    let reset_plan = administrative_membership_plan(
        &reset_trace,
        CoreProposedKindV1::AccessModeChange,
        CoreChangeSetV1::one(
            MembershipChangeV1::access_mode_change(member, AccessModeV1::Spectator, None)
                .unwrap_or_else(|error| unreachable!("fixture Access change: {error}")),
        ),
        "room-commit-test-reset-consequence",
        "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    );
    let reset_persistence = advance_persistence(&reset_plan);
    assert!(matches!(
        reset_persistence.delivery_consequences.as_slice(),
        [PreparedObservationConsequenceV1::ResetRequired(view)]
            if view.viewer() == &PackViewerV1::Public(parsed(MEMBER))
    ));

    let loss_trace = counter_trace();
    let member = loss_trace
        .core_state()
        .membership(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("fixture Membership"))
        .clone();
    let loss_plan = administrative_membership_plan(
        &loss_trace,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(member)),
        "room-commit-test-loss-consequence",
        "01ARZ3NDEKTSV4RRFFQ69G5FC8",
    );
    let loss_persistence = advance_persistence(&loss_plan);
    assert!(matches!(
        loss_persistence.delivery_consequences.as_slice(),
        [PreparedObservationConsequenceV1::VisibilityLost(member_id)]
            if member_id == &parsed(MEMBER)
    ));
}

fn action_plan() -> (CoreTraceV1, PreparedRoomCommitV1) {
    let trace = counter_trace();
    let action_schema = trace
        .retained_pack()
        .unwrap_or_else(|| unreachable!("retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .unwrap_or_else(|| unreachable!("Counter increment action"))
        .payload_schema
        .schema_digest
        .clone();
    let request = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        trace.head().room_seq(),
        "increment",
        canonical(br"{}"),
    );
    let prepared = trace
        .prepare(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(ACTION),
            action_type: "increment".to_owned(),
            payload_schema_digest: action_schema,
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Action preparation: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0))
        .collect::<BTreeMap<_, _>>();
    let plan = PreparedRoomCommitV1::for_action(
        &trace,
        &request,
        prepared,
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC5"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
        authority(),
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture Action plan: {error}"));
    (trace, plan)
}

fn counter_action_trace() -> CoreTraceV1 {
    let mut trace = counter_trace();
    let action_schema = trace
        .retained_pack()
        .unwrap_or_else(|| unreachable!("retained Counter pack"))
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .unwrap_or_else(|| unreachable!("Counter increment action"))
        .payload_schema
        .schema_digest
        .clone();
    trace
        .advance(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: parsed(MEMBER),
            action_id: parsed(ACTION),
            action_type: "increment".to_owned(),
            payload_schema_digest: action_schema,
            canonical_payload: canonical(br"{}"),
            exact_basis_head: trace.head().clone(),
            admitted_at: parsed("2026-08-15T12:00:01Z"),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Action advance: {error}"));
    trace
}

fn lifecycle_stimulus(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    idempotency_key: &str,
    recorded_at: &str,
) -> RecordedStimulusV1 {
    RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        kind,
        CoreAuthorityAttributionV1 {
            principal_id: parsed(PRINCIPAL),
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        trace.head().room_seq(),
        "room-commit-test-lifecycle",
        parsed(recorded_at),
        changeset,
    ))
}

fn install_lifecycle_transition(trace: &mut CoreTraceV1, stimulus: RecordedStimulusV1) {
    assert!(matches!(
        trace
            .advance(stimulus)
            .unwrap_or_else(|error| unreachable!("fixture lifecycle transition: {error}")),
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        }
    ));
}

fn archive_trace(suspended: bool) -> CoreTraceV1 {
    let mut trace = counter_trace();
    if suspended {
        let membership = trace
            .core_state()
            .membership(&parsed(MEMBER))
            .unwrap_or_else(|| unreachable!("fixture membership"))
            .clone();
        let stimulus = lifecycle_stimulus(
            &trace,
            CoreProposedKindV1::Suspend,
            CoreChangeSetV1::one(MembershipChangeV1::suspend(membership)),
            "room-commit-test-suspend",
            "2026-08-15T12:00:01Z",
        );
        install_lifecycle_transition(&mut trace, stimulus);
    }
    let stimulus = lifecycle_stimulus(
        &trace,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(trace.core_state().room_status()),
        "room-commit-test-archive",
        "2026-08-15T12:00:02Z",
    );
    install_lifecycle_transition(&mut trace, stimulus);
    trace
}

fn action_request(
    trace: &CoreTraceV1,
    based_on_room_seq: RoomSequenceV1,
    payload: &[u8],
) -> ParticipantActionRequestV1 {
    ParticipantActionRequestV1::new(
        trace.head().room_id().clone(),
        parsed(MEMBER),
        parsed(ACTION),
        based_on_room_seq,
        "increment",
        canonical(payload),
    )
}

fn timer_plan() -> (CoreTraceV1, PreparedRoomCommitV1) {
    let trace = CoreTraceV1::create_for_conformance(
        GenesisInputV1::new(
            parsed(ROOM),
            counter_v2_digest(),
            canonical(br#"{"fixture":"timer"}"#),
            parsed(SEED),
            parsed("2026-08-15T12:00:00Z"),
            CoreRoomStateV1::active(std::iter::empty::<MembershipV1>())
                .unwrap_or_else(|error| unreachable!("fixture Core state: {error}")),
            canonical(br#"{"fixture":"timer"}"#),
        )
        .with_initial_timers(vec![ScheduledTimerV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }]),
        |_| Ok(()),
        |input| {
            Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
                next_activity_state: input.prior_activity_state.clone(),
                ordered_domain_events: Vec::new(),
                timer_requests: Vec::new(),
                ordered_attention_signals: Vec::new(),
            }))
        },
    )
    .unwrap_or_else(|error| unreachable!("fixture Timer trace: {error}"));
    let request = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:30:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    let prepared = trace
        .prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Timer preparation: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0))
        .collect::<BTreeMap<_, _>>();
    let plan = PreparedRoomCommitV1::for_timer_fired(
        &trace,
        &request,
        prepared,
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
        authority(),
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture Timer plan: {error}"));
    (trace, plan)
}

#[test]
fn authorized_timer_fired_sealer_delegates_exact_timer_witness() {
    let (trace, plan) = timer_plan();
    let request = match plan.input_witness() {
        PreparedOperationInputWitnessV1::TimerFired(witness) => witness.request.clone(),
        _ => unreachable!("fixture Timer plan retained a non-Timer witness"),
    };
    let (authority, presented) = global_host_authority();
    let grant = authority
        .authorize_timer_fired(&presented, &request, parsed("2026-08-15T12:00:00Z"))
        .unwrap_or_else(|error| unreachable!("fixture Timer authority: {error}"));
    let prepared_transition = trace
        .prepare(RecordedStimulusV1::TimerFired(request.recorded_stimulus()))
        .unwrap_or_else(|error| unreachable!("fixture Timer preparation: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0))
        .collect::<BTreeMap<_, _>>();
    let sealed = PreparedRoomCommitV1::for_authorized_timer_fired(
        &trace,
        &request,
        prepared_transition,
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC7"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
        grant,
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture authorized Timer plan: {error}"));
    assert!(matches!(
        sealed.input_witness(),
        PreparedOperationInputWitnessV1::TimerFired(_)
    ));
    assert!(
        sealed
            .authority_witness()
            .authority_snapshot_query()
            .is_some()
    );
}

enum CommitReply {
    Exact,
    Fixed(RoomCommitResolutionV1),
    Indeterminate,
    NotApplicable,
}

struct ScriptedStorage {
    commits: Mutex<VecDeque<CommitReply>>,
    resolutions: Mutex<VecDeque<ResolveOutcomeV1>>,
    observed: Mutex<Vec<(Vec<u8>, CanonicalRequestHashV1)>>,
}

struct RecoveryStorage {
    candidate: RoomRecoveryCandidateV1,
    recorded: Mutex<Vec<RecoveryIntegrityDispositionV1>>,
}

impl RecoveryStorage {
    fn new(candidate: RoomRecoveryCandidateV1) -> Self {
        Self {
            candidate,
            recorded: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<RecoveryIntegrityDispositionV1> {
        self.recorded
            .lock()
            .unwrap_or_else(|_| unreachable!("recovery storage mutex"))
            .clone()
    }
}

impl RoomRecoveryStorageV1 for RecoveryStorage {
    fn inspect_recovery_candidate(
        &self,
        _room_id: &RoomId,
    ) -> Result<Option<RoomRecoveryCandidateV1>, RoomRecoveryErrorV1> {
        Ok(Some(self.candidate.clone()))
    }

    fn guard_recovery_install(
        &self,
        _room_id: &RoomId,
        _expected_head: &CompleteHeadV1,
        _expected_integrity_generation: IntegrityGenerationV1,
        _recovered_materializations: &RecoveredRoomMaterializationsV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        Ok(())
    }

    fn record_recovery_failure(
        &self,
        _room_id: &RoomId,
        _expected_head: &CompleteHeadV1,
        _expected_integrity_generation: IntegrityGenerationV1,
        disposition: RecoveryIntegrityDispositionV1,
    ) -> Result<(), RoomRecoveryErrorV1> {
        self.recorded
            .lock()
            .unwrap_or_else(|_| unreachable!("recovery storage mutex"))
            .push(disposition);
        Ok(())
    }
}

fn recovery_candidate(
    genesis: &GenesisV1,
    transitions: &[TransitionV1],
) -> RoomRecoveryCandidateV1 {
    let head = transitions
        .last()
        .map_or_else(|| genesis.complete_head(), TransitionV1::complete_head);
    let (core, activity) = transitions.last().map_or_else(
        || {
            (
                genesis.initial_core_state().clone(),
                genesis.initial_activity_state().clone(),
            )
        },
        |transition| {
            (
                transition.resulting_core_state().clone(),
                transition.resulting_activity_state().clone(),
            )
        },
    );
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
    let lock = registry
        .load_retained(head.pack_digest())
        .unwrap_or_else(|error| unreachable!("retained Counter revision: {error}"))
        .revision_lock()
        .canonical_bytes()
        .unwrap_or_else(|error| unreachable!("revision lock bytes: {error}"));
    RoomRecoveryCandidateV1::new(
        head.clone(),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("integrity generation: {error}")),
        head.canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Head bytes: {error}")),
        lock,
        genesis
            .canonical_bytes()
            .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}")),
        transitions
            .iter()
            .map(|transition| {
                transition
                    .canonical_bytes()
                    .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"))
            })
            .collect(),
        Some(
            core.canonical_bytes()
                .unwrap_or_else(|error| unreachable!("Core bytes: {error}")),
        ),
        Some(
            activity
                .to_bytes()
                .unwrap_or_else(|error| unreachable!("Activity bytes: {error}")),
        ),
    )
}

fn assert_candidate_quarantines(candidate: RoomRecoveryCandidateV1, registry: &PackRegistryV1) {
    let storage = RecoveryStorage::new(candidate);
    assert!(matches!(
        recover_room_from_storage(&storage, registry, &parsed(ROOM)),
        Err(RoomRecoveryErrorV1::Corrupt)
    ));
    assert_eq!(
        storage.recorded(),
        vec![RecoveryIntegrityDispositionV1::Quarantined]
    );
}

fn assert_candidate_quarantines_before_missing_runtime(candidate: RoomRecoveryCandidateV1) {
    let missing_runtime = counter_v1_only_registry_for_conformance()
        .unwrap_or_else(|error| unreachable!("v1-only registry: {error}"));
    assert_candidate_quarantines(candidate, &missing_runtime);
}

fn refresh_genesis_hashes(genesis: &mut GenesisV1) {
    genesis.initial_core_state_hash = crate::lineage::hash_core_state(&genesis.initial_core_state)
        .unwrap_or_else(|error| unreachable!("Core hash: {error}"));
    genesis.initial_activity_state_hash =
        crate::lineage::hash_activity_state(&genesis.pack_digest, &genesis.initial_activity_state)
            .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
    genesis.initial_authoritative_state_hash = crate::lineage::hash_authoritative_state(
        &genesis.pack_digest,
        &genesis.initial_core_state_hash,
        &genesis.initial_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
    genesis.genesis_hash = genesis
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Genesis hash: {error}"));
}

fn refresh_transition_hashes(transition: &mut TransitionV1) {
    transition.resulting_core_state_hash =
        crate::lineage::hash_core_state(&transition.resulting_core_state)
            .unwrap_or_else(|error| unreachable!("Core hash: {error}"));
    transition.resulting_activity_state_hash = crate::lineage::hash_activity_state(
        &transition.pack_digest,
        &transition.resulting_activity_state,
    )
    .unwrap_or_else(|error| unreachable!("Activity hash: {error}"));
    transition.resulting_authoritative_state_hash = crate::lineage::hash_authoritative_state(
        &transition.pack_digest,
        &transition.resulting_core_state_hash,
        &transition.resulting_activity_state_hash,
    )
    .unwrap_or_else(|error| unreachable!("authoritative hash: {error}"));
    transition.transition_hash = transition
        .calculate_hash()
        .unwrap_or_else(|error| unreachable!("Transition hash: {error}"));
}

impl ScriptedStorage {
    fn new(
        commits: impl IntoIterator<Item = CommitReply>,
        resolutions: impl IntoIterator<Item = ResolveOutcomeV1>,
    ) -> Self {
        Self {
            commits: Mutex::new(commits.into_iter().collect()),
            resolutions: Mutex::new(resolutions.into_iter().collect()),
            observed: Mutex::new(Vec::new()),
        }
    }

    fn observed(&self) -> Vec<(Vec<u8>, CanonicalRequestHashV1)> {
        self.observed
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .clone()
    }

    fn record(&self, identity: &OperationIdentityV1, hash: &CanonicalRequestHashV1) {
        self.observed
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .push((
                identity
                    .canonical_bytes()
                    .unwrap_or_else(|error| unreachable!("canonical identity: {error}")),
                hash.clone(),
            ));
    }
}

impl RoomCommitStorageV1 for ScriptedStorage {
    fn commit(&self, prepared: &PreparedRoomWriteV1) -> RoomCommitResolutionV1 {
        self.record(prepared.identity(), prepared.request_hash());
        let reply = self
            .commits
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .pop_front()
            .unwrap_or_else(|| unreachable!("unexpected commit"));
        match reply {
            CommitReply::Exact => match prepared {
                PreparedRoomWriteV1::Create(create) => RoomCommitResolutionV1::resolved(
                    ResolutionStatusV1::New,
                    create.semantic_result().clone(),
                ),
                PreparedRoomWriteV1::Existing(existing) => RoomCommitResolutionV1::resolved(
                    ResolutionStatusV1::New,
                    existing.semantic_result().clone(),
                ),
            },
            CommitReply::Fixed(resolution) => resolution,
            CommitReply::Indeterminate => RoomCommitResolutionV1::Indeterminate,
            CommitReply::NotApplicable => RoomCommitResolutionV1::NotApplicable,
        }
    }

    fn resolve(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1 {
        self.record(identity, request_hash);
        self.resolutions
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .pop_front()
            .unwrap_or_else(|| unreachable!("unexpected resolve"))
    }
}

impl AuthorizedReceiptResolverV1 for ScriptedStorage {
    fn resolve_authorized(
        &self,
        _authority: AuthorizedReceiptReadV1,
    ) -> Result<ResolveOutcomeV1, AuthorityErrorV1> {
        Ok(self
            .resolutions
            .lock()
            .unwrap_or_else(|_| unreachable!("test storage mutex"))
            .pop_front()
            .unwrap_or(ResolveOutcomeV1::ResolutionUnavailable))
    }
}

#[test]
fn create_releases_only_the_exact_new_genesis_receipt() {
    let plan = creation_plan(0);
    let expected_head = plan.persistence().complete_head.clone();
    let storage = ScriptedStorage::new([CommitReply::Exact], []);
    let outcome = commit_room_creation(&storage, plan);
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        }
    ));
    let trace = outcome
        .into_committed_trace()
        .unwrap_or_else(|| unreachable!("exact new Create releases trace"));
    assert_eq!(trace.head(), &expected_head);

    let mutated = creation_plan(1).semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            result: Box::new(mutated),
        })],
        [],
    );
    let outcome = commit_room_creation(&storage, creation_plan(0));
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::Fault
    ));
    assert!(outcome.into_committed_trace().is_none());
}

#[test]
fn create_reprepare_reseals_new_genesis_without_changing_caller_identity() {
    let prepared = creation_plan(0);
    let expected_identity = prepared.semantic_result().operation_identity().clone();
    let expected_hash = prepared.semantic_result().canonical_request_hash().clone();
    let expected_lock = prepared.persistence().pack_revision_lock.clone();
    let old_room_id = prepared.persistence().complete_head.room_id().clone();
    let old_member_id = prepared.persistence().memberships[0]
        .membership
        .member_id()
        .clone();
    let old_seed = prepared.persistence().genesis.room_seed().clone();
    let old_created_at = prepared.persistence().genesis.created_at().clone();
    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let (_, trace, pending) = commit_room_creation(&storage, prepared).into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::Reprepare(reprepare)) = pending else {
        unreachable!("Create collision exposes only the opaque reprepare token");
    };
    assert_eq!(reprepare.selected_pack_revision_lock(), &expected_lock);

    let resealed = reprepare
        .reseal_for_conformance(
            authority_with("room-commit-test-authority-refreshed", parsed(PRINCIPAL), 2),
            counter_genesis_with(
                0,
                "01ARZ3NDEKTSV4RRFFQ69G5FAW",
                "01ARZ3NDEKTSV4RRFFQ69G5FC1",
                "hex:101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f",
                "2026-08-15T13:00:00Z",
            ),
        )
        .unwrap_or_else(|error| unreachable!("same-principal reseal: {error}"));
    assert_eq!(
        resealed.authority_witness().authenticated_principal(),
        &parsed(PRINCIPAL)
    );
    assert_eq!(resealed.authority_witness().generation(), 2);
    assert_eq!(resealed.persistence().pack_revision_lock, expected_lock);
    assert_ne!(resealed.persistence().complete_head.room_id(), &old_room_id);
    assert_ne!(
        resealed.persistence().memberships[0].membership.member_id(),
        &old_member_id
    );
    assert_ne!(resealed.persistence().genesis.room_seed(), &old_seed);
    assert_ne!(resealed.persistence().genesis.created_at(), &old_created_at);
    let resealed = PreparedRoomWriteV1::from(resealed);
    assert_eq!(resealed.identity(), &expected_identity);
    assert_eq!(resealed.request_hash(), &expected_hash);

    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let (_, _, pending) = commit_room_creation(&storage, creation_plan(0)).into_parts();
    let Some(RoomCreationPendingAttemptV1::Reprepare(reprepare)) = pending else {
        unreachable!("second Create collision exposes reprepare token");
    };
    assert!(matches!(
        reprepare.reseal_for_conformance(
            authority_with(
                "room-commit-test-authority-wrong-principal",
                parsed("01ARZ3NDEKTSV4RRFFQ69G5FD1"),
                2,
            ),
            counter_genesis_with(
                0,
                "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                "hex:202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
                "2026-08-15T13:01:00Z",
            ),
        ),
        Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch)
    ));
}

#[test]
fn impossible_storage_result_variants_are_rejected_by_operation_kind() {
    let create_result = creation_plan(0).semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(
            RoomCommitResolutionV1::TransitionCommitted {
                status: ResolutionStatusV1::New,
                result: Box::new(create_result),
            },
        )],
        [],
    );
    assert!(matches!(
        commit_room_creation(&storage, creation_plan(0)).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, action) = action_plan();
    let action_result = action.semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            result: Box::new(action_result),
        })],
        [],
    );
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, action).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, action) = action_plan();
    let storage = ScriptedStorage::new([CommitReply::NotApplicable], []);
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, action).resolution(),
        RoomCommitResolutionV1::Fault
    ));

    let (mut trace, timer) = timer_plan();
    let storage = ScriptedStorage::new([CommitReply::NotApplicable], []);
    assert!(matches!(
        commit_existing_room(&storage, &mut trace, timer).resolution(),
        RoomCommitResolutionV1::NotApplicable
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn invalid_action_payload_never_becomes_stable_but_archival_is_lifecycle_first() {
    let mut suspended = counter_trace();
    let membership = suspended
        .core_state()
        .membership(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("fixture membership"))
        .clone();
    let stimulus = lifecycle_stimulus(
        &suspended,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(membership)),
        "room-commit-test-invalid-payload-suspend",
        "2026-08-15T12:00:01Z",
    );
    install_lifecycle_transition(&mut suspended, stimulus);

    let active = counter_trace();
    let archived_enabled = archive_trace(false);
    let archived_suspended = archive_trace(true);
    for (label, trace, based_on_room_seq) in [
        (
            "active-stale",
            &active,
            RoomSequenceV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        ("active-disabled", &suspended, suspended.head().room_seq()),
        (
            "archived-enabled-current",
            &archived_enabled,
            archived_enabled.head().room_seq(),
        ),
        (
            "archived-enabled-stale",
            &archived_enabled,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        (
            "archived-suspended-current",
            &archived_suspended,
            archived_suspended.head().room_seq(),
        ),
        (
            "archived-suspended-stale",
            &archived_suspended,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
    ] {
        let request = action_request(trace, based_on_room_seq, br"[]");
        assert!(
            PreparedRoomCommitV1::for_stable_action_disposition(
                trace,
                &request,
                parsed("2026-08-15T12:01:00Z"),
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
                authority(),
            )
            .is_err(),
            "strict invalid payload unexpectedly sealed a stable receipt for {label}"
        );
    }

    for (label, trace, based_on_room_seq) in [
        (
            "archived-enabled-current",
            &archived_enabled,
            archived_enabled.head().room_seq(),
        ),
        (
            "archived-enabled-stale",
            &archived_enabled,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
        (
            "archived-suspended-current",
            &archived_suspended,
            archived_suspended.head().room_seq(),
        ),
        (
            "archived-suspended-stale",
            &archived_suspended,
            RoomSequenceV1::new(0)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        ),
    ] {
        let request = action_request(trace, based_on_room_seq, br"{}");
        let prepared = PreparedRoomCommitV1::for_stable_action_disposition(
            trace,
            &request,
            parsed("2026-08-15T12:01:00Z"),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
            authority(),
        )
        .unwrap_or_else(|error| unreachable!("{label} archived stable receipt: {error}"));
        assert!(matches!(
            prepared.semantic_result().result(),
            SemanticResultV1::RejectionRecorded { code, .. } if code == "room_archived"
        ));
        let decoded = StoredSemanticResultV1::from_canonical_receipt_bytes(
            prepared.semantic_result().canonical_receipt_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("{label} archived receipt decode: {error}"));
        assert_eq!(decoded, *prepared.semantic_result());
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn authorized_administration_is_persistable_and_receipt_reads_are_room_sealed() {
    let trace_a = counter_trace();
    let trace_b = CoreTraceV1::create_uncommitted(counter_genesis_with(
        0,
        ROOM_B,
        MEMBER,
        SEED,
        "2026-08-15T12:00:00Z",
    ))
    .unwrap_or_else(|error| unreachable!("fixture Room B trace: {error}"));
    let (request_a, plan_a) = administrative_archive_plan(
        &trace_a,
        ROOM,
        "same-administration-identity",
        "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    );
    let (_, plan_b) = administrative_archive_plan(
        &trace_b,
        ROOM_B,
        "same-administration-identity",
        "01ARZ3NDEKTSV4RRFFQ69G5FC8",
    );
    assert!(matches!(
        plan_a.intent(),
        PreparedExistingIntentV1::Advance(_)
    ));
    assert!(matches!(
        plan_a.semantic_result().semantic_input(),
        ReceiptSemanticInputV1::CoreAdministration { proposal }
            if proposal.authority_attribution().principal_id == parsed(PRINCIPAL)
    ));

    let (authority, presented) = room_host_authority(ROOM);
    let identity =
        OperationIdentityV1::Administration(Box::new(request_a.operation_identity().clone()));
    let request_hash = request_a
        .canonical_request_hash()
        .unwrap_or_else(|error| unreachable!("fixture request hash: {error}"));
    let grant = match authority
        .authorize(
            &presented,
            AuthorityUseV1::ReadRoomOperationResult {
                identity: identity.clone(),
                request_hash: request_hash.clone(),
                target_room_id: Some(parsed(ROOM)),
            },
            parsed("2026-08-15T12:00:03Z"),
        )
        .unwrap_or_else(|error| unreachable!("fixture receipt authority: {error}"))
    {
        AuthorityGrantV1::ReceiptRead(grant) => grant,
        grant => unreachable!("expected receipt grant, got {grant:?}"),
    };
    let same_room = ScriptedStorage::new(
        [],
        [ResolveOutcomeV1::StoredResolution(Box::new(
            plan_a.semantic_result().clone(),
        ))],
    );
    let grant = authority
        .receipt_adapter_input_for_test(grant, &parsed("2026-08-15T12:00:03Z"))
        .unwrap_or_else(|error| unreachable!("fixture receipt revalidation: {error}"));
    assert!(matches!(
        resolve_authorized_room_operation_for_adapter(grant, |identity, request_hash| {
            same_room.resolve(identity, request_hash)
        }),
        ResolveOutcomeV1::StoredResolution(_)
    ));

    let grant = match authority
        .authorize(
            &presented,
            AuthorityUseV1::ReadRoomOperationResult {
                identity,
                request_hash,
                target_room_id: Some(parsed(ROOM)),
            },
            parsed("2026-08-15T12:00:04Z"),
        )
        .unwrap_or_else(|error| unreachable!("fixture receipt authority: {error}"))
    {
        AuthorityGrantV1::ReceiptRead(grant) => grant,
        grant => unreachable!("expected receipt grant, got {grant:?}"),
    };
    let cross_room_hash = plan_b.semantic_result().canonical_request_hash().clone();
    let cross_room = ScriptedStorage::new(
        [],
        [
            ResolveOutcomeV1::Conflict {
                existing_request_hash: cross_room_hash,
            },
            ResolveOutcomeV1::StoredResolution(Box::new(plan_b.semantic_result().clone())),
        ],
    );
    let grant = authority
        .receipt_adapter_input_for_test(grant, &parsed("2026-08-15T12:00:04Z"))
        .unwrap_or_else(|error| unreachable!("fixture receipt revalidation: {error}"));
    assert_eq!(
        resolve_authorized_room_operation_for_adapter(grant, |identity, request_hash| {
            cross_room.resolve(identity, request_hash)
        }),
        ResolveOutcomeV1::ResolutionUnavailable
    );
    assert_eq!(
        format!("{:?}", plan_a.semantic_result()),
        "StoredSemanticResultV1([REDACTED])"
    );
}

#[test]
fn action_ingress_resolves_the_receipt_before_current_access_eligibility() {
    let (trace, plan) = action_plan();
    let request = action_request(&trace, trace.head().room_seq(), br"{}");
    let (authority, presented) = action_ingress_authority(AccessModeV1::Spectator);
    assert!(matches!(
        authority.authorize_action(&presented, &request, parsed("2026-08-15T12:00:03Z")),
        Err(AuthorityErrorV1::Forbidden)
    ));

    let stored = ScriptedStorage::new(
        [],
        [ResolveOutcomeV1::StoredResolution(Box::new(
            plan.semantic_result().clone(),
        ))],
    );
    assert!(matches!(
        authorize_participant_action_operation(
            &authority,
            &stored,
            &presented,
            &request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Ok(ParticipantActionIngressV1::Existing(_))
    ));

    let absent = ScriptedStorage::new([], [ResolveOutcomeV1::KnownAbsent]);
    assert!(matches!(
        authorize_participant_action_operation(
            &authority,
            &absent,
            &presented,
            &request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Err(ParticipantActionIngressErrorV1::Authority(
            AuthorityErrorV1::Forbidden
        ))
    ));
}

#[test]
fn creation_and_administration_ingress_resolve_before_speculative_work() {
    let create_plan = creation_plan(0);
    let create_request = creation_request(0);
    let create_identity = creation_identity();
    let (create_authority, create_presented) = global_host_authority();
    let existing_create = ScriptedStorage::new(
        [],
        [ResolveOutcomeV1::StoredResolution(Box::new(
            create_plan.semantic_result().clone(),
        ))],
    );
    assert!(matches!(
        authorize_room_creation_operation(
            &create_authority,
            &existing_create,
            &create_presented,
            &create_identity,
            &create_request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Ok(RoomCreationIngressV1::Existing(_))
    ));
    let absent_create = ScriptedStorage::new([], [ResolveOutcomeV1::KnownAbsent]);
    assert!(matches!(
        authorize_room_creation_operation(
            &create_authority,
            &absent_create,
            &create_presented,
            &create_identity,
            &create_request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Ok(RoomCreationIngressV1::Authorized(_))
    ));

    let trace = counter_trace();
    let (admin_request, admin_plan) = administrative_archive_plan(
        &trace,
        ROOM,
        "receipt-first-admin",
        "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    );
    let (admin_authority, admin_presented) = room_host_authority(ROOM);
    let existing_admin = ScriptedStorage::new(
        [],
        [ResolveOutcomeV1::StoredResolution(Box::new(
            admin_plan.semantic_result().clone(),
        ))],
    );
    assert!(matches!(
        authorize_core_administration_operation(
            &admin_authority,
            &existing_admin,
            &admin_presented,
            &admin_request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Ok(CoreAdministrationIngressV1::Existing(_))
    ));
    let absent_admin = ScriptedStorage::new([], [ResolveOutcomeV1::KnownAbsent]);
    assert!(matches!(
        authorize_core_administration_operation(
            &admin_authority,
            &absent_admin,
            &admin_presented,
            &admin_request,
            parsed("2026-08-15T12:00:03Z"),
        ),
        Ok(CoreAdministrationIngressV1::Authorized(_))
    ));
}

#[test]
fn administration_receipt_rejects_cross_principal_attribution_without_a_transition() {
    let mut trace = counter_trace();
    let first_identity = AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(PRINCIPAL),
        versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
        idempotency_key: "first-archive".to_owned(),
    };
    let first = CoreProposedV1::new(
        CoreProposedKindV1::Archive,
        CoreAuthorityAttributionV1 {
            principal_id: parsed(PRINCIPAL),
            authority_kind: CoreAuthorityKindV1::RoomAdministrator,
        },
        first_identity,
        trace.head().room_seq(),
        "first_archive",
        parsed("2026-08-15T12:00:01Z"),
        CoreChangeSetV1::archive(RoomStatusV1::Active),
    );
    trace
        .advance(RecordedStimulusV1::CoreProposed(first))
        .unwrap_or_else(|error| unreachable!("fixture archive: {error}"));

    let request = CoreAdministrationRequestV1::new(
        parsed(ROOM),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(PRINCIPAL),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: "repeat-archive".to_owned(),
        },
        CoreProposedKindV1::Archive,
        trace.head().room_seq(),
        "repeat_archive",
        CoreChangeSetV1::archive(RoomStatusV1::Archived),
    )
    .unwrap_or_else(|error| unreachable!("fixture repeat request: {error}"));
    let (authority, presented) = room_host_authority(ROOM);
    let grant = authority
        .authorize_core_administration(&presented, &request, parsed("2026-08-15T12:00:02Z"))
        .unwrap_or_else(|error| unreachable!("fixture admin authority: {error}"));
    let frame_heads = trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member_id| (member_id, 0))
        .collect();
    let plan = PreparedRoomCommitV1::for_authorized_core_administration(
        &trace,
        &request,
        parsed("2026-08-15T12:00:03Z"),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC8"),
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity: {error}")),
        grant,
        &frame_heads,
    )
    .unwrap_or_else(|error| unreachable!("fixture repeat plan: {error}"));
    assert!(matches!(
        plan.semantic_result().result(),
        SemanticResultV1::NoChangeRecorded { .. }
    ));

    let mut receipt: serde_json::Value =
        serde_json::from_slice(plan.semantic_result().canonical_receipt_bytes())
            .unwrap_or_else(|error| unreachable!("fixture receipt JSON: {error}"));
    receipt["semantic_input"]["proposal"]["canonical_authority_attribution"]["principal_id"] =
        serde_json::Value::String("01ARZ3NDEKTSV4RRFFQ69G5FE0".to_owned());
    let bytes = serde_json::to_vec(&receipt)
        .unwrap_or_else(|error| unreachable!("fixture mutated receipt: {error}"));
    let canonical_bytes = CanonicalJsonV1::parse(&bytes)
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| unreachable!("fixture canonical receipt: {error}"));
    assert!(StoredSemanticResultV1::from_canonical_receipt_bytes(&canonical_bytes).is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn replay_requires_current_authority_and_reconstructs_the_historical_viewer() {
    let trace = counter_trace();
    let genesis_bytes = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("fixture Genesis bytes: {error}"));
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("fixture Counter registry: {error}"));

    let (authority, presented) = replay_authority(MEMBER, None);
    let grant = match authority
        .authorize(
            &presented,
            AuthorityUseV1::Replay {
                room_id: parsed(ROOM),
                member_id: parsed(MEMBER),
                at_room_seq: RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
                projection_kind: ReplayProjectionKindV1::HistoricalMembership,
            },
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap_or_else(|error| unreachable!("fixture Replay authority: {error}"))
    {
        AuthorityGrantV1::Replay(grant) => grant,
        grant => unreachable!("expected Replay grant, got {grant:?}"),
    };
    let grant = authority
        .replay_adapter_input_for_test(grant, &parsed("2026-08-15T12:00:02Z"))
        .unwrap_or_else(|error| unreachable!("fixture Replay revalidation: {error}"));
    let request = grant.projection_request(RoomIntegrityStateV1::new(
        RoomIntegrityStatusV1::Healthy,
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
    ));
    let projection = CoreTraceV1::project_replayed_history(&registry, &genesis_bytes, &[], request)
        .unwrap_or_else(|error| unreachable!("fixture historical Replay: {error}"));
    assert_eq!(projection.verified_head(), trace.head());
    assert_eq!(
        projection.historical_membership().member_id(),
        &parsed::<MemberId>(MEMBER)
    );
    assert_eq!(projection.historical_room_status(), RoomStatusV1::Active);
    let view_bytes = projection
        .canonical_envelope()
        .to_bytes()
        .unwrap_or_else(|error| unreachable!("fixture view bytes: {error}"));
    assert!(
        view_bytes
            .windows(b"\"authorized_core\"".len())
            .any(|window| { window == b"\"authorized_core\"" })
    );
    assert!(
        view_bytes
            .windows(b"\"private_ack_count\":0".len())
            .any(|window| { window == b"\"private_ack_count\":0" })
    );
    let debug_projection = format!("{projection:?}");
    assert!(debug_projection.contains("[REDACTED]"));
    assert!(!debug_projection.contains("private_ack_count"));

    let (later_authority, later_presented) = replay_authority(LATER_MEMBER, None);
    let later_grant = match later_authority
        .authorize(
            &later_presented,
            AuthorityUseV1::Replay {
                room_id: parsed(ROOM),
                member_id: parsed(LATER_MEMBER),
                at_room_seq: RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
                projection_kind: ReplayProjectionKindV1::HistoricalMembership,
            },
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap_or_else(|error| unreachable!("fixture later Replay authority: {error}"))
    {
        AuthorityGrantV1::Replay(grant) => grant,
        grant => unreachable!("expected Replay grant, got {grant:?}"),
    };
    let later_grant = later_authority
        .replay_adapter_input_for_test(later_grant, &parsed("2026-08-15T12:00:02Z"))
        .unwrap_or_else(|error| unreachable!("fixture later Replay revalidation: {error}"));
    let later_request = later_grant.projection_request(RoomIntegrityStateV1::new(
        RoomIntegrityStatusV1::Healthy,
        IntegrityGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture integrity generation: {error}")),
    ));
    assert!(matches!(
        CoreTraceV1::project_replayed_history(&registry, &genesis_bytes, &[], later_request),
        Err(HistoricalReplayErrorV1::HistoricalMembershipUnavailable)
    ));

    let (expiring_authority, expiring_presented) =
        replay_authority(MEMBER, Some("2026-08-15T12:00:02Z"));
    let expiring_grant = match expiring_authority
        .authorize(
            &expiring_presented,
            AuthorityUseV1::Replay {
                room_id: parsed(ROOM),
                member_id: parsed(MEMBER),
                at_room_seq: RoomSequenceV1::new(0)
                    .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
                projection_kind: ReplayProjectionKindV1::HistoricalMembership,
            },
            parsed("2026-08-15T12:00:01Z"),
        )
        .unwrap_or_else(|error| unreachable!("fixture expiring Replay authority: {error}"))
    {
        AuthorityGrantV1::Replay(grant) => grant,
        grant => unreachable!("expected Replay grant, got {grant:?}"),
    };
    assert!(
        expiring_authority
            .replay_adapter_input_for_test(expiring_grant, &parsed("2026-08-15T12:00:02Z"))
            .is_err()
    );
}

#[test]
fn incremental_historical_replay_folds_pages_and_preflights_before_runtime_failure() {
    let trace = counter_action_trace();
    let genesis_bytes = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("fixture Genesis bytes: {error}"));
    let transition_bytes = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("fixture Transition bytes: {error}"));
    let request = || {
        HistoricalReplayProjectionRequestV1::new(
            parsed(ROOM),
            parsed(MEMBER),
            RoomSequenceV1::new(1)
                .unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
            ReplayProjectionKindV1::HistoricalMembership,
            RoomIntegrityStateV1::new(
                RoomIntegrityStatusV1::Healthy,
                IntegrityGenerationV1::new(1)
                    .unwrap_or_else(|error| unreachable!("fixture integrity: {error}")),
            ),
        )
    };

    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("fixture Counter registry: {error}"));
    let mut incremental =
        HistoricalReplayAccumulatorV1::begin(&registry, &genesis_bytes, request())
            .unwrap_or_else(|error| unreachable!("begin incremental Replay: {error}"));
    assert!(!incremental.is_complete());
    assert!(format!("{incremental:?}").contains("[REDACTED]"));
    incremental
        .consume_page(&transition_bytes)
        .unwrap_or_else(|error| unreachable!("fold incremental page: {error}"));
    assert!(incremental.is_complete());
    let projection = incremental
        .finish()
        .unwrap_or_else(|error| unreachable!("finish incremental Replay: {error}"));
    assert_eq!(projection.verified_head(), trace.head());

    let missing_runtime = counter_v1_only_registry_for_conformance()
        .unwrap_or_else(|error| unreachable!("fixture v1-only registry: {error}"));
    let mut runtime_absent =
        HistoricalReplayAccumulatorV1::begin(&missing_runtime, &genesis_bytes, request())
            .unwrap_or_else(|error| unreachable!("begin runtime-absent Replay: {error}"));
    runtime_absent
        .consume_page(&transition_bytes)
        .unwrap_or_else(|error| unreachable!("preflight intact runtime-absent page: {error}"));
    assert!(matches!(
        runtime_absent.finish(),
        Err(HistoricalReplayErrorV1::ReplayFailed(
            ReplayFailureClassV1::RuntimeUnavailable
        ))
    ));

    let mut corrupt_after_missing_runtime =
        HistoricalReplayAccumulatorV1::begin(&missing_runtime, &genesis_bytes, request())
            .unwrap_or_else(|error| unreachable!("begin corrupt runtime-absent Replay: {error}"));
    assert!(matches!(
        corrupt_after_missing_runtime.consume_page(&[b"{}".to_vec()]),
        Err(HistoricalReplayErrorV1::ReplayFailed(
            ReplayFailureClassV1::NonCanonicalRecord
        ))
    ));
}

#[test]
fn existing_winner_never_installs_speculative_transition() {
    let (mut trace, plan) = action_plan();
    let before = trace.head().clone();
    let winner = plan.semantic_result().clone();
    let storage = ScriptedStorage::new(
        [CommitReply::Fixed(RoomCommitResolutionV1::resolved(
            ResolutionStatusV1::Existing,
            winner,
        ))],
        [],
    );
    let outcome = commit_existing_room(&storage, &mut trace, plan);
    assert!(matches!(
        outcome.resolution(),
        RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        }
    ));
    assert_eq!(
        outcome.actor_installation(),
        ActorInstallationV1::ReloadRequired
    );
    assert_eq!(trace.head(), &before);
}

#[test]
fn timer_reprepare_obsolescence_is_typed_not_applicable_without_pack_entry() {
    let (mut trace, timer) = timer_plan();
    let storage = ScriptedStorage::new([CommitReply::Fixed(RoomCommitResolutionV1::Reprepare)], []);
    let outcome = commit_existing_room(&storage, &mut trace, timer);
    let (_, _, _, reprepare) = outcome.into_parts();
    let Some(ExistingRoomReprepareV1::TimerFired(reprepare)) = reprepare else {
        unreachable!("changed Head retains an opaque Timer reprepare capability")
    };
    let consumed = RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: parsed(TIMER),
        generation: TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        scheduled_for: parsed("2026-08-15T12:30:00Z"),
        canonical_payload: canonical(br#"{"kind":"deadline"}"#),
    });
    trace
        .advance(consumed)
        .unwrap_or_else(|error| unreachable!("consume Timer fixture: {error}"));
    let callbacks_after_consumption = trace.activity_callback_count();
    let resealed = reprepare
        .seal_against(
            &trace,
            parsed("01ARZ3NDEKTSV4RRFFQ69G5FC8"),
            IntegrityGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("integrity generation: {error}")),
            authority(),
            &BTreeMap::new(),
        )
        .unwrap_or_else(|error| unreachable!("obsolete Timer classification: {error}"));
    assert!(matches!(resealed, TimerReprepareOutcomeV1::NotApplicable));
    assert_eq!(trace.activity_callback_count(), callbacks_after_consumption);
}

#[test]
fn create_resolution_tokens_retain_the_identical_sealed_plan() {
    let storage = ScriptedStorage::new(
        [CommitReply::Indeterminate, CommitReply::Exact],
        [
            ResolveOutcomeV1::ResolutionUnavailable,
            ResolveOutcomeV1::KnownAbsent,
        ],
    );
    let first = commit_room_creation(&storage, creation_plan(0));
    let (_, trace, pending) = first.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::ResolveOnly(resolve)) = pending else {
        unreachable!("indeterminate Create exposes resolve-only token");
    };
    let second = resolve.resolve(&storage);
    let (_, trace, pending) = second.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::ResolveOnly(resolve)) = pending else {
        unreachable!("unavailable resolution retains resolve-only token");
    };
    let third = resolve.resolve(&storage);
    let (_, trace, pending) = third.into_parts();
    assert!(trace.is_none());
    let Some(RoomCreationPendingAttemptV1::Retryable(retry)) = pending else {
        unreachable!("known absence exposes retry token");
    };
    let final_outcome = retry.retry(&storage);
    assert!(final_outcome.into_committed_trace().is_some());

    let observed = storage.observed();
    assert_eq!(observed.len(), 4);
    assert!(observed.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
#[allow(clippy::too_many_lines)]
fn canonical_request_hash_vectors_bind_semantics_and_not_identity_only_fields() {
    let create = RoomCreationRequestV1::new(
        parsed("blake3:1111111111111111111111111111111111111111111111111111111111111111"),
        canonical(br#"{"initial_value":0,"maximum_value":4}"#),
        vec![
            InitialMembershipProposalV1::new(
                parsed(PRINCIPAL),
                PrincipalKindV1::Human,
                MembershipStandingV1::Enabled,
                AccessModeV1::Participant,
                Some("counter".to_owned()),
            )
            .unwrap_or_else(|error| unreachable!("fixture proposal: {error}")),
        ],
    );
    let action = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br"{}"),
    );
    let timer = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:30:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    assert_eq!(
        (
            create
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Create hash: {error}"))
                .to_string(),
            action
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Action hash: {error}"))
                .to_string(),
            timer
                .canonical_request_hash()
                .unwrap_or_else(|error| unreachable!("Timer hash: {error}"))
                .to_string(),
        ),
        (
            "blake3:80546edf7c213bc86e25c05f13ea1344e3ba66b66259aa3764d2981c1523a6a2".to_owned(),
            "blake3:70943dd000ed7c711e4c428bb06a60c481e13706233b7750f97d9947e2ab91bf".to_owned(),
            "blake3:02970bc922b8161be9ec42baef43cf69a757c249056e7c5d755846cfc9927d69".to_owned(),
        )
    );

    let changed_create = RoomCreationRequestV1::new(
        parsed("blake3:1111111111111111111111111111111111111111111111111111111111111111"),
        canonical(br#"{"initial_value":1,"maximum_value":4}"#),
        create.ordered_initial_memberships().to_vec(),
    );
    assert_ne!(
        create
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Create hash: {error}")),
        changed_create
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Create mutation hash: {error}")),
    );
    let changed_action_id = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC7"),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br"{}"),
    );
    assert_eq!(
        action
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action hash: {error}")),
        changed_action_id
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action identity-only hash: {error}")),
    );
    let changed_action_payload = ParticipantActionRequestV1::new(
        parsed(ROOM),
        parsed(MEMBER),
        parsed(ACTION),
        RoomSequenceV1::new(0).unwrap_or_else(|error| unreachable!("fixture sequence: {error}")),
        "increment",
        canonical(br#"{"delta":1}"#),
    );
    assert_ne!(
        action
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action hash: {error}")),
        changed_action_payload
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Action mutation hash: {error}")),
    );
    let changed_timer = TimerFiredRequestV1::new(
        parsed(ROOM),
        parsed(TIMER),
        TimerGenerationV1::new(1)
            .unwrap_or_else(|error| unreachable!("fixture Timer generation: {error}")),
        parsed("2026-08-15T12:31:00Z"),
        canonical(br#"{"kind":"deadline"}"#),
    );
    assert_ne!(
        timer
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer hash: {error}")),
        changed_timer
            .canonical_request_hash()
            .unwrap_or_else(|error| unreachable!("Timer mutation hash: {error}")),
    );
}

#[test]
fn recovery_quarantines_rehashed_host_core_disagreement_before_missing_runtime() {
    let action_trace = counter_action_trace();
    let mut action_transition = action_trace.transitions()[0].clone();
    action_transition
        .resulting_core_state
        .memberships
        .get_mut(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("Action result Membership"))
        .standing = MembershipStandingV1::Suspended;
    refresh_transition_hashes(&mut action_transition);
    assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
        action_trace.genesis(),
        &[action_transition],
    ));

    let proposal_trace = archive_trace(false);
    let mut proposal_transition = proposal_trace.transitions()[0].clone();
    proposal_transition.resulting_core_state.room_status = RoomStatusV1::Active;
    refresh_transition_hashes(&mut proposal_transition);
    assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
        proposal_trace.genesis(),
        &[proposal_transition],
    ));
}

#[test]
fn recovery_quarantines_rehashed_invalid_timer_history_before_missing_runtime() {
    let (mut trace, _plan) = timer_plan();
    trace
        .advance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
            timer_id: parsed(TIMER),
            generation: TimerGenerationV1::new(1)
                .unwrap_or_else(|error| unreachable!("Timer generation: {error}")),
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br#"{"kind":"deadline"}"#),
        }))
        .unwrap_or_else(|error| unreachable!("fixture Timer advance: {error}"));
    let original = trace.transitions()[0].clone();
    let generation_two =
        TimerGenerationV1::new(2).unwrap_or_else(|error| unreachable!("Timer generation: {error}"));
    let generation_three =
        TimerGenerationV1::new(3).unwrap_or_else(|error| unreachable!("Timer generation: {error}"));
    for changes in [
        vec![TimerChangeV1::Schedule {
            timer_id: parsed(TIMER),
            generation: generation_three,
            scheduled_for: parsed("2026-08-15T13:00:00Z"),
            canonical_payload: canonical(br"{}"),
        }],
        vec![TimerChangeV1::Schedule {
            timer_id: parsed(TIMER),
            generation: generation_two,
            scheduled_for: parsed("2026-08-15T12:30:00Z"),
            canonical_payload: canonical(br"{}"),
        }],
        vec![
            TimerChangeV1::Schedule {
                timer_id: parsed(TIMER),
                generation: generation_two,
                scheduled_for: parsed("2026-08-15T13:00:00Z"),
                canonical_payload: canonical(br"{}"),
            },
            TimerChangeV1::Cancel {
                timer_id: parsed(TIMER),
                generation: generation_two,
            },
        ],
    ] {
        let mut transition = original.clone();
        transition.ordered_timer_changes = changes;
        refresh_transition_hashes(&mut transition);
        assert_candidate_quarantines_before_missing_runtime(recovery_candidate(
            trace.genesis(),
            &[transition],
        ));
    }
}

#[test]
fn recovery_quarantines_rehashed_invalid_genesis_inputs_not_runtime_faults() {
    let trace = counter_trace();
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));

    let mut invalid_configuration = trace.genesis().clone();
    invalid_configuration.configuration = canonical(br"{}");
    refresh_genesis_hashes(&mut invalid_configuration);
    assert_candidate_quarantines(recovery_candidate(&invalid_configuration, &[]), &registry);

    let mut invalid_role = trace.genesis().clone();
    invalid_role
        .initial_core_state
        .memberships
        .get_mut(&parsed(MEMBER))
        .unwrap_or_else(|| unreachable!("Genesis Membership"))
        .role = Some("not-a-counter-role".to_owned());
    refresh_genesis_hashes(&mut invalid_role);
    assert_candidate_quarantines(recovery_candidate(&invalid_role, &[]), &registry);
}
