use std::collections::BTreeMap;

use serde_json::Value;

use super::*;
use crate::{
    ActionId, ActivityDispositionV1, ActivityReduceInputV1, Blake3DigestV1, CanonicalJsonV1,
    CompleteHeadV1, CoreRoomStateV1, MembershipV1, PackDigestV1, ParticipantActionV1, PrincipalId,
    PrincipalKindV1, RecordedStimulusV1, RoomSequenceV1, ScheduledTimerV1, TimerFiredV1,
    TimerGenerationV1, TimerRequestV1, TimerScheduledFor,
};

const NAVIGATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const INSIDER_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
const BROKER_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD2";
const SPECTATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD3";
const OPERATOR_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD4";

fn member_id(value: &str) -> MemberId {
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("privacy fixture Member ID: {error}"))
}

fn membership(value: &str, access_mode: AccessModeV1, role: Option<&str>) -> MembershipV1 {
    membership_with_standing(value, access_mode, role, MembershipStandingV1::Enabled)
}

fn membership_with_standing(
    value: &str,
    access_mode: AccessModeV1,
    role: Option<&str>,
    standing: MembershipStandingV1,
) -> MembershipV1 {
    let principal_id: PrincipalId = value
        .parse()
        .unwrap_or_else(|error| unreachable!("privacy fixture Principal ID: {error}"));
    MembershipV1::new(
        member_id(value),
        principal_id,
        PrincipalKindV1::Agent,
        standing,
        access_mode,
        role.map(str::to_owned),
    )
    .unwrap_or_else(|error| unreachable!("privacy fixture Membership: {error}"))
}

fn core() -> CoreRoomStateV1 {
    CoreRoomStateV1::active([
        membership(NAVIGATOR_MEMBER, AccessModeV1::Participant, Some(NAVIGATOR)),
        membership(INSIDER_MEMBER, AccessModeV1::Participant, Some(INSIDER)),
        membership(BROKER_MEMBER, AccessModeV1::Participant, Some(BROKER)),
        membership(SPECTATOR_MEMBER, AccessModeV1::Spectator, None),
        membership(OPERATOR_MEMBER, AccessModeV1::Operator, None),
    ])
    .unwrap_or_else(|error| unreachable!("privacy fixture Core: {error}"))
}

fn state() -> StateV1 {
    StateV1 {
        phase: PhaseV1::Negotiation,
        phase_generation: 2,
        phase_start: "2026-08-15T12:00:30Z".to_owned(),
        phase_deadline: Some("2026-08-15T14:00:30Z".to_owned()),
        fixture_id: FIXTURE_ID_SERVICE.to_owned(),
        fixture: FixtureV1 {
            fixture_id: FIXTURE_ID_SERVICE.to_owned(),
            route: "service".to_owned(),
            entry_window: "early".to_owned(),
            required_tool: "thermal_key".to_owned(),
            extraction: "boat".to_owned(),
        },
        seats: vec![
            SeatV1 {
                role: NAVIGATOR.to_owned(),
                member_id: member_id(NAVIGATOR_MEMBER),
            },
            SeatV1 {
                role: INSIDER.to_owned(),
                member_id: member_id(INSIDER_MEMBER),
            },
            SeatV1 {
                role: BROKER.to_owned(),
                member_id: member_id(BROKER_MEMBER),
            },
        ],
        clues: vec![
            ClueV1 {
                clue_id: "route".to_owned(),
                owner_role: NAVIGATOR.to_owned(),
                claim_code: "route_service".to_owned(),
                inspected_by: vec![NAVIGATOR.to_owned()],
                disclosed_to: Vec::new(),
                published_claim_code: None,
            },
            ClueV1 {
                clue_id: "entry_window".to_owned(),
                owner_role: INSIDER.to_owned(),
                claim_code: "entry_window_early".to_owned(),
                inspected_by: Vec::new(),
                disclosed_to: Vec::new(),
                published_claim_code: None,
            },
        ],
        exchanges: vec![
            ExchangeV1 {
                offer_id: "offer_to_navigator".to_owned(),
                sender_role: INSIDER.to_owned(),
                recipient_role: NAVIGATOR.to_owned(),
                offered_clue_id: "entry_window".to_owned(),
                consideration_kind: "plan_endorsement".to_owned(),
                consideration_id: "plan-1".to_owned(),
                status: "open".to_owned(),
            },
            ExchangeV1 {
                offer_id: "offer_to_insider".to_owned(),
                sender_role: BROKER.to_owned(),
                recipient_role: INSIDER.to_owned(),
                offered_clue_id: "route".to_owned(),
                consideration_kind: "plan_endorsement".to_owned(),
                consideration_id: "plan-1".to_owned(),
                status: "open".to_owned(),
            },
        ],
        plans: vec![PlanV1 {
            plan_id: "plan-1".to_owned(),
            proposer_role: NAVIGATOR.to_owned(),
            created_room_seq: 3,
            route: "service".to_owned(),
            entry_window: "early".to_owned(),
            required_tool: "thermal_key".to_owned(),
            extraction: "boat".to_owned(),
        }],
        endorsements: BTreeMap::new(),
        challenges: Vec::new(),
        commitments: BTreeMap::from([(
            NAVIGATOR.to_owned(),
            CommitmentV1 {
                selected_plan_id: "plan-1".to_owned(),
                contribute_required_resource: true,
            },
        )]),
        result_acknowledgements: Vec::new(),
        outcome: None,
    }
}

fn json_projection(value: CanonicalJsonV1) -> serde_json::Value {
    serde_json::to_value(value)
        .unwrap_or_else(|error| unreachable!("privacy projection JSON: {error}"))
}

#[test]
fn exchange_details_are_absent_from_non_participant_projections() {
    let state = state();
    let core = core();
    for class in [
        PackViewerClassV1::Public,
        PackViewerClassV1::HistoricalPublic,
        PackViewerClassV1::Operator,
        PackViewerClassV1::HistoricalOperator,
    ] {
        let projection = json_projection(
            projection(&state, &core, None, false, class)
                .unwrap_or_else(|error| unreachable!("privacy projection: {error}")),
        );
        let object = projection
            .as_object()
            .unwrap_or_else(|| unreachable!("privacy projection must be an object"));
        assert!(!object.contains_key("exchanges"));
        assert!(!object.contains_key("addressed_offers"));
        assert!(!object.contains_key("own_commitment"));
        assert!(!object.contains_key("private_clues"));
    }

    let malformed_reveal = json_projection(
        projection(&state, &core, None, true, PackViewerClassV1::Public)
            .unwrap_or_else(|error| unreachable!("privacy projection: {error}")),
    );
    assert!(
        !malformed_reveal
            .as_object()
            .is_some_and(|object| object.contains_key("exchanges"))
    );
}

#[test]
fn participant_and_historical_participant_keep_only_their_private_data() {
    let state = state();
    let core = core();
    let current = json_projection(
        projection(
            &state,
            &core,
            Some(NAVIGATOR),
            false,
            PackViewerClassV1::Participant,
        )
        .unwrap_or_else(|error| unreachable!("privacy projection: {error}")),
    );
    let historical = json_projection(
        projection(
            &state,
            &core,
            Some(NAVIGATOR),
            false,
            PackViewerClassV1::HistoricalParticipant,
        )
        .unwrap_or_else(|error| unreachable!("privacy projection: {error}")),
    );

    for projection in [current, historical] {
        let object = projection
            .as_object()
            .unwrap_or_else(|| unreachable!("privacy projection must be an object"));
        let offers = object
            .get("addressed_offers")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| unreachable!("participant offers are missing"));
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0]["offer_id"], "offer_to_navigator");
        assert_eq!(offers[0]["recipient_role"], NAVIGATOR);
        assert!(!offers[0].to_string().contains("offer_to_insider"));

        let clues = object
            .get("private_clues")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| unreachable!("participant clues are missing"));
        assert_eq!(clues.len(), 1);
        assert_eq!(clues[0]["clue_id"], "route");

        assert_eq!(object["own_commitment"]["selected_plan_id"], "plan-1");
        assert_eq!(
            object["own_commitment"]["contribute_required_resource"],
            true
        );
        assert!(!object.contains_key("exchanges"));
    }
}

#[test]
fn commitment_event_contains_no_sealed_commitment_values() {
    let event = commitment_accepted_event(
        NAVIGATOR,
        1,
        PhaseV1::Commitment,
        3,
        Some("2026-08-15T14:30:30Z"),
    );
    let object = event
        .as_object()
        .unwrap_or_else(|| unreachable!("commitment event must be an object"));
    assert!(!object.contains_key("selected_plan_id"));
    assert!(!object.contains_key("contribute_required_resource"));
    assert!(!object.contains_key("plan"));
    assert!(!object.contains_key("resource"));
}

#[test]
fn attention_precedence_keeps_one_signal_per_agent_target() {
    let state = state();
    let core = core();
    let signals = attention_for_candidates(
        &state,
        &core,
        [
            (NAVIGATOR, ATTENTION_ROUND_RESULT_AVAILABLE),
            (NAVIGATOR, ATTENTION_REQUIRED_ACTION_DEADLINE),
            (INSIDER, ATTENTION_OFFER_RECEIVED),
            (INSIDER, ATTENTION_ENDORSEMENT_REQUESTED),
        ],
    );
    assert_eq!(signals.len(), 2);
    let values = signals
        .into_iter()
        .map(|signal| {
            serde_json::to_value(signal)
                .unwrap_or_else(|error| unreachable!("Attention JSON: {error}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(values[0]["target_member_id"], NAVIGATOR_MEMBER);
    assert_eq!(values[0]["reason"], ATTENTION_REQUIRED_ACTION_DEADLINE);
    assert_eq!(values[1]["target_member_id"], INSIDER_MEMBER);
    assert_eq!(values[1]["reason"], ATTENTION_OFFER_RECEIVED);
}

fn phase_state(phase: PhaseV1, generation: u32, deadline: &str) -> StateV1 {
    let mut state = state();
    state.phase = phase;
    state.phase_generation = generation;
    state.phase_start = "2026-08-15T12:00:30Z".to_owned();
    state.phase_deadline = Some(deadline.to_owned());
    state.outcome = None;
    state
}

fn timer_disposition(
    mut state: StateV1,
    timer_name: &str,
    kind: &str,
) -> Result<(StateV1, ActivityDispositionV1), PackFaultV1> {
    let timer_id_value = timer_id(timer_name);
    let scheduled_for = state
        .phase_deadline
        .clone()
        .unwrap_or_else(|| "2026-08-15T14:00:30Z".to_owned());
    let generation = TimerGenerationV1::new(u64::from(state.phase_generation))
        .unwrap_or_else(|error| unreachable!("timer generation fixture: {error}"));
    let payload = canonical_value(&timer_payload(kind, state.phase, state.phase_generation))?;
    let timer = TimerFiredV1 {
        timer_id: timer_id_value.clone(),
        generation,
        scheduled_for: scheduled_for
            .parse()
            .unwrap_or_else(|error| unreachable!("timer timestamp fixture: {error}")),
        canonical_payload: payload.clone(),
    };
    let mut scheduled = BTreeMap::new();
    scheduled.insert(
        timer_id_value.clone(),
        ScheduledTimerV1 {
            timer_id: timer_id_value,
            generation,
            scheduled_for: timer.scheduled_for.clone(),
            canonical_payload: payload,
        },
    );
    let stimulus = RecordedStimulusV1::TimerFired(timer.clone());
    let core = core();
    let prior = canonical(&state)?;
    let input = ActivityReduceInputV1 {
        prior_activity_state: &prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers: &scheduled,
        next_room_seq: RoomSequenceV1::new(17)
            .unwrap_or_else(|error| unreachable!("room sequence fixture: {error}")),
        recorded_stimulus: &stimulus,
    };
    let disposition = reduce_timer(&mut state, &input, &timer)?;
    Ok((state, disposition))
}

fn action_disposition(
    mut state: StateV1,
    member: &str,
    action_type: &str,
    action_id_value: &str,
    admitted_at: &str,
    payload: &Value,
    scheduled_timers: &BTreeMap<TimerId, ScheduledTimerV1>,
) -> Result<(StateV1, ActivityDispositionV1), PackFaultV1> {
    let action = ParticipantActionV1 {
        member_id: member_id(member),
        action_id: action_id_value
            .parse()
            .unwrap_or_else(|error| unreachable!("Heist action ID: {error}")),
        action_type: action_type.to_owned(),
        payload_schema_digest: Blake3DigestV1::hash(action_type.as_bytes()),
        canonical_payload: canonical_value(payload)?,
        exact_basis_head: dummy_head(),
        admitted_at: admitted_at
            .parse()
            .unwrap_or_else(|error| unreachable!("Heist action timestamp: {error}")),
    };
    let stimulus = RecordedStimulusV1::ParticipantAction(action.clone());
    let core = core();
    let prior = canonical(&state)?;
    let input = ActivityReduceInputV1 {
        prior_activity_state: &prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers,
        next_room_seq: RoomSequenceV1::new(17)
            .unwrap_or_else(|error| unreachable!("action room sequence: {error}")),
        recorded_stimulus: &stimulus,
    };
    let disposition = reduce_action(&mut state, &input, &action)?;
    Ok((state, disposition))
}

#[test]
#[allow(clippy::too_many_lines)]
fn briefing_and_negotiation_actions_exercise_declared_success_and_rejections() {
    let no_timers = BTreeMap::new();

    let mut briefing = phase_state(PhaseV1::Briefing, 1, "2026-08-15T14:00:30Z");
    briefing.clues[0].inspected_by.clear();
    let (_, inspect) = action_disposition(
        briefing,
        NAVIGATOR_MEMBER,
        INSPECT_CLUE,
        "01ARZ3NDEKTSV4RRFFQ69G5FD6",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({"clue_id":"route"}),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Briefing inspect: {error:?}"));
    assert!(matches!(inspect, ActivityDispositionV1::Apply(_)));

    let (_, publish) = action_disposition(
        state(),
        NAVIGATOR_MEMBER,
        PUBLISH_CLUE,
        "01ARZ3NDEKTSV4RRFFQ69G5FD7",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({"clue_id":"route","claim_code":"route_service"}),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation publish: {error:?}"));
    assert!(matches!(publish, ActivityDispositionV1::Apply(_)));

    let (_, offer) = action_disposition(
        state(),
        NAVIGATOR_MEMBER,
        OFFER_EXCHANGE,
        "01ARZ3NDEKTSV4RRFFQ69G5FD8",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({
            "recipient_role":"insider",
            "offered_clue_id":"route",
            "consideration":{"kind":"plan_endorsement","plan_id":"plan-1"}
        }),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation offer: {error:?}"));
    assert!(matches!(offer, ActivityDispositionV1::Apply(_)));

    let mut accept_state = state();
    accept_state.clues[1].inspected_by.push(INSIDER.to_owned());
    accept_state.exchanges = vec![ExchangeV1 {
        offer_id: "offer_to_navigator".to_owned(),
        sender_role: INSIDER.to_owned(),
        recipient_role: NAVIGATOR.to_owned(),
        offered_clue_id: "entry_window".to_owned(),
        consideration_kind: "plan_endorsement".to_owned(),
        consideration_id: "plan-1".to_owned(),
        status: "open".to_owned(),
    }];
    let (_, accept) = action_disposition(
        accept_state,
        NAVIGATOR_MEMBER,
        ACCEPT_EXCHANGE,
        "01ARZ3NDEKTSV4RRFFQ69G5FD9",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({"offer_id":"offer_to_navigator"}),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation accept: {error:?}"));
    assert!(matches!(accept, ActivityDispositionV1::Apply(_)));

    let (_, propose) = action_disposition(
        state(),
        NAVIGATOR_MEMBER,
        PROPOSE_PLAN,
        "01ARZ3NDEKTSV4RRFFQ69G5FE0",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({
            "route":"canal",
            "entry_window":"late",
            "required_tool":"disguise",
            "extraction":"van"
        }),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation propose: {error:?}"));
    assert!(matches!(propose, ActivityDispositionV1::Apply(_)));

    let (_, endorse) = action_disposition(
        state(),
        INSIDER_MEMBER,
        ENDORSE_PLAN,
        "01ARZ3NDEKTSV4RRFFQ69G5FE1",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({"plan_id":"plan-1"}),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation endorse: {error:?}"));
    assert!(matches!(endorse, ActivityDispositionV1::Apply(_)));

    let mut challenge_state = state();
    challenge_state.plans[0].route = "canal".to_owned();
    let (_, challenge) = action_disposition(
        challenge_state,
        NAVIGATOR_MEMBER,
        CHALLENGE_PLAN,
        "01ARZ3NDEKTSV4RRFFQ69G5FE2",
        "2026-08-15T14:00:00Z",
        &serde_json::json!({"plan_id":"plan-1","reason":"route_conflict"}),
        &no_timers,
    )
    .unwrap_or_else(|error| unreachable!("Negotiation challenge: {error:?}"));
    assert!(matches!(challenge, ActivityDispositionV1::Apply(_)));

    for (index, action_type) in [
        INSPECT_CLUE,
        PUBLISH_CLUE,
        OFFER_EXCHANGE,
        ACCEPT_EXCHANGE,
        PROPOSE_PLAN,
        ENDORSE_PLAN,
        CHALLENGE_PLAN,
    ]
    .into_iter()
    .enumerate()
    {
        let rejected_state = phase_state(PhaseV1::Result, 5, "2026-08-15T14:00:30Z");
        let before = canonical(&rejected_state)
            .unwrap_or_else(|error| unreachable!("rejection state: {error}"));
        let (after, disposition) = action_disposition(
            rejected_state,
            NAVIGATOR_MEMBER,
            action_type,
            &format!("01ARZ3NDEKTSV4RRFFQ69G5F{:02X}", index + 3),
            "2026-08-15T14:00:00Z",
            &serde_json::json!({}),
            &no_timers,
        )
        .unwrap_or_else(|error| unreachable!("stable {action_type} rejection: {error:?}"));
        match disposition {
            ActivityDispositionV1::Reject(rejection) => {
                assert_eq!(rejection.declared_code, "wrong_phase");
            }
            ActivityDispositionV1::Apply(_) => {
                unreachable!("{action_type} unexpectedly applied in Result")
            }
        }
        assert_eq!(
            canonical(&after).unwrap_or_else(|error| unreachable!("rejected state: {error}")),
            before
        );
    }
}

#[test]
fn absent_broker_outcome_matrix_is_deterministic_and_fail_closed() {
    let success = BTreeMap::from([
        (NAVIGATOR.to_owned(), ("correct".to_owned(), true)),
        (INSIDER.to_owned(), ("correct".to_owned(), false)),
    ]);
    assert_eq!(
        outcome_for_matrix(&success, Some(FIXTURE_ID_SERVICE)),
        ("success".to_owned(), 5)
    );

    let split = BTreeMap::from([
        (NAVIGATOR.to_owned(), ("correct".to_owned(), true)),
        (INSIDER.to_owned(), ("other".to_owned(), true)),
    ]);
    assert_eq!(
        outcome_for_matrix(&split, Some(FIXTURE_ID_SERVICE)),
        ("failure".to_owned(), 0)
    );

    let majority_with_no_resource = BTreeMap::from([
        (NAVIGATOR.to_owned(), ("correct".to_owned(), false)),
        (INSIDER.to_owned(), ("correct".to_owned(), false)),
        (BROKER.to_owned(), ("correct".to_owned(), false)),
    ]);
    assert_eq!(
        outcome_for_matrix(&majority_with_no_resource, Some(FIXTURE_ID_SERVICE)),
        ("partial_failure".to_owned(), 4)
    );

    // A missing Broker is represented by an absent commitment, never by a
    // synthetic vote. Repeating the same semantic input is a replay oracle.
    assert_eq!(
        outcome_for_matrix(&success, Some(FIXTURE_ID_SERVICE)),
        outcome_for_matrix(&success, Some(FIXTURE_ID_SERVICE))
    );
}

#[test]
fn golden_majority_and_score_matrix_covers_missing_split_and_partial_votes() {
    let commitment = |plan: &str, resource: bool| (plan.to_owned(), resource);
    let cases = [
        (BTreeMap::new(), "failure", 0),
        (
            BTreeMap::from([("navigator".to_owned(), commitment("plan-1", true))]),
            "failure",
            0,
        ),
        (
            BTreeMap::from([
                ("navigator".to_owned(), commitment("correct", true)),
                ("insider".to_owned(), commitment("correct", false)),
            ]),
            "success",
            5,
        ),
        (
            BTreeMap::from([
                ("navigator".to_owned(), commitment("plan-1", true)),
                ("insider".to_owned(), commitment("plan-2", true)),
            ]),
            "failure",
            0,
        ),
        (
            BTreeMap::from([
                ("navigator".to_owned(), commitment("correct", true)),
                ("insider".to_owned(), commitment("correct", false)),
                ("broker".to_owned(), commitment("correct", true)),
            ]),
            "success",
            5,
        ),
        (
            BTreeMap::from([
                ("navigator".to_owned(), commitment("correct", true)),
                ("insider".to_owned(), commitment("correct", false)),
                ("broker".to_owned(), commitment("wrong", true)),
            ]),
            "success",
            5,
        ),
        (
            BTreeMap::from([
                ("navigator".to_owned(), commitment("plan-1", true)),
                ("insider".to_owned(), commitment("plan-2", true)),
                ("broker".to_owned(), commitment("plan-3", true)),
            ]),
            "failure",
            0,
        ),
    ];
    for (commitments, expected_outcome, expected_score) in cases {
        assert_eq!(
            outcome_for_matrix(&commitments, Some(FIXTURE_ID_SERVICE)),
            (expected_outcome.to_owned(), expected_score),
            "matrix case must be deterministic"
        );
    }
}

#[test]
fn resolve_timer_records_explicit_three_of_five_partial_score() {
    let mut resolution = phase_state(PhaseV1::Resolution, 4, "2026-08-15T14:00:31Z");
    let plan = resolution
        .plans
        .first_mut()
        .unwrap_or_else(|| unreachable!("privacy fixture must contain a plan"));
    plan.route = "wrong-route".to_owned();
    resolution.commitments = BTreeMap::from([
        (
            NAVIGATOR.to_owned(),
            CommitmentV1 {
                selected_plan_id: "plan-1".to_owned(),
                contribute_required_resource: false,
            },
        ),
        (
            INSIDER.to_owned(),
            CommitmentV1 {
                selected_plan_id: "plan-1".to_owned(),
                contribute_required_resource: false,
            },
        ),
    ]);

    let (result, disposition) = timer_disposition(resolution, RESOLVE_TIMER, "resolve_now")
        .unwrap_or_else(|error| unreachable!("three-of-five resolve: {error:?}"));
    let outcome = result
        .outcome
        .as_ref()
        .unwrap_or_else(|| unreachable!("resolve timer must record an outcome"));

    assert_eq!(result.phase, PhaseV1::Result);
    assert_eq!(outcome.outcome, "partial_failure");
    assert_eq!(outcome.score, 3);
    assert_eq!(outcome.selected_plan_id.as_deref(), Some("plan-1"));
    assert_eq!(outcome.vote_counts.get("plan-1"), Some(&2));
    assert_eq!(outcome.missing_roles, vec![BROKER.to_owned()]);
    assert!(matches!(disposition, ActivityDispositionV1::Apply(_)));
}

#[test]
#[allow(clippy::too_many_lines)]
fn early_third_commit_emits_phase_and_reminder_cancellation_and_resolve_timer() {
    let mut state = phase_state(PhaseV1::Commitment, 3, "2026-08-15T14:00:30Z");
    state.plans.push(PlanV1 {
        plan_id: "plan-1".to_owned(),
        proposer_role: NAVIGATOR.to_owned(),
        created_room_seq: 1,
        route: "service".to_owned(),
        entry_window: "early".to_owned(),
        required_tool: "thermal_key".to_owned(),
        extraction: "boat".to_owned(),
    });
    state.commitments.insert(
        NAVIGATOR.to_owned(),
        CommitmentV1 {
            selected_plan_id: "plan-1".to_owned(),
            contribute_required_resource: true,
        },
    );
    state.commitments.insert(
        INSIDER.to_owned(),
        CommitmentV1 {
            selected_plan_id: "plan-1".to_owned(),
            contribute_required_resource: false,
        },
    );

    let payload = canonical_value(&serde_json::json!({
        "selected_plan_id": "plan-1",
        "contribute_required_resource": true,
    }))
    .unwrap_or_else(|error| unreachable!("commit payload: {error:?}"));
    let action = ParticipantActionV1 {
        member_id: member_id(BROKER_MEMBER),
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD5"
            .parse()
            .unwrap_or_else(|error| unreachable!("commit action: {error}")),
        action_type: COMMIT_MOVE.to_owned(),
        payload_schema_digest: Blake3DigestV1::hash(b"heist-commit"),
        canonical_payload: payload,
        exact_basis_head: dummy_head(),
        admitted_at: "2026-08-15T14:00:00Z"
            .parse()
            .unwrap_or_else(|error| unreachable!("commit time: {error}")),
    };
    let stimulus = RecordedStimulusV1::ParticipantAction(action.clone());
    let prior =
        canonical(&state).unwrap_or_else(|error| unreachable!("commit prior state: {error:?}"));
    let timer = |name: &str, kind: &str, generation: u64, due: &str| ScheduledTimerV1 {
        timer_id: timer_id(name),
        generation: TimerGenerationV1::new(generation)
            .unwrap_or_else(|error| unreachable!("timer generation: {error}")),
        scheduled_for: due
            .parse()
            .unwrap_or_else(|error| unreachable!("timer due: {error}")),
        canonical_payload: canonical_value(&timer_payload(
            kind,
            PhaseV1::Commitment,
            u32::try_from(generation).unwrap_or_else(|error| unreachable!("generation: {error}")),
        ))
        .unwrap_or_else(|error| unreachable!("timer payload: {error:?}")),
    };
    let scheduled = BTreeMap::from([
        (
            timer_id(PHASE_TIMER),
            timer(PHASE_TIMER, "phase_deadline", 11, "2026-08-15T14:00:30Z"),
        ),
        (
            timer_id(REMINDER_TIMER),
            timer(
                REMINDER_TIMER,
                "commitment_reminder",
                9,
                "2026-08-15T14:00:20Z",
            ),
        ),
    ]);
    let core = core();
    let input = ActivityReduceInputV1 {
        prior_activity_state: &prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers: &scheduled,
        next_room_seq: RoomSequenceV1::new(18)
            .unwrap_or_else(|error| unreachable!("commit room sequence: {error}")),
        recorded_stimulus: &stimulus,
    };
    let disposition = reduce_action(&mut state, &input, &action)
        .unwrap_or_else(|error| unreachable!("early third commitment: {error:?}"));
    assert_eq!(state.phase, PhaseV1::Resolution);
    assert_eq!(state.phase_generation, 4);
    let ActivityDispositionV1::Apply(output) = disposition else {
        unreachable!("third commitment must apply")
    };
    assert!(output.timer_requests.iter().any(|request| matches!(
        request,
        TimerRequestV1::CancelCurrent { timer_id, expected_generation }
            if timer_id.as_str() == PHASE_TIMER && expected_generation.get() == 11
    )));
    assert!(output.timer_requests.iter().any(|request| matches!(
        request,
        TimerRequestV1::CancelCurrent { timer_id, expected_generation }
            if timer_id.as_str() == REMINDER_TIMER && expected_generation.get() == 9
    )));
    assert!(output.timer_requests.iter().any(|request| {
        matches!(request, TimerRequestV1::ScheduleNext { timer_id, .. } if timer_id.as_str() == RESOLVE_TIMER)
    }));
}

#[test]
#[allow(clippy::too_many_lines)]
fn timer_matrix_advances_all_six_phases_and_fences_duplicates() {
    let phases = [
        (
            PhaseV1::Briefing,
            1,
            PHASE_TIMER,
            "phase_deadline",
            PhaseV1::Negotiation,
        ),
        (
            PhaseV1::Negotiation,
            2,
            PHASE_TIMER,
            "phase_deadline",
            PhaseV1::Commitment,
        ),
        (
            PhaseV1::Commitment,
            3,
            PHASE_TIMER,
            "phase_deadline",
            PhaseV1::Resolution,
        ),
        (
            PhaseV1::Result,
            5,
            PHASE_TIMER,
            "phase_deadline",
            PhaseV1::Complete,
        ),
    ];

    for (phase, generation, timer_name, kind, expected_phase) in phases {
        let deadline = if phase == PhaseV1::Resolution {
            "2026-08-15T14:00:31Z"
        } else {
            "2026-08-15T14:00:30Z"
        };
        let (next_state, disposition) =
            timer_disposition(phase_state(phase, generation, deadline), timer_name, kind)
                .unwrap_or_else(|error| unreachable!("phase timer reduction: {error:?}"));
        assert_eq!(next_state.phase, expected_phase);
        match disposition {
            ActivityDispositionV1::Apply(output) => {
                let reduced: StateV1 = decode(&output.next_activity_state)
                    .unwrap_or_else(|error| unreachable!("reduced Heist state: {error:?}"));
                assert_eq!(reduced.phase, expected_phase);
                assert_eq!(output.ordered_domain_events.len(), 1);
            }
            ActivityDispositionV1::Reject(rejection) => {
                unreachable!("timer unexpectedly rejected: {}", rejection.declared_code)
            }
        }
    }

    let state = phase_state(PhaseV1::Negotiation, 2, "2026-08-15T14:00:30Z");
    let (mut advanced, _) = timer_disposition(state, PHASE_TIMER, "phase_deadline")
        .unwrap_or_else(|error| unreachable!("first phase timer reduction: {error:?}"));
    let prior = canonical(&advanced)
        .unwrap_or_else(|error| unreachable!("advanced Heist state: {error:?}"));
    let core = core();
    let timer_id_value = timer_id(PHASE_TIMER);
    let generation = TimerGenerationV1::new(2)
        .unwrap_or_else(|error| unreachable!("duplicate timer generation: {error}"));
    let payload = canonical_value(&timer_payload("phase_deadline", PhaseV1::Negotiation, 2))
        .unwrap_or_else(|error| unreachable!("duplicate timer payload: {error:?}"));
    let scheduled_for = "2026-08-15T14:00:30Z"
        .parse()
        .unwrap_or_else(|error| unreachable!("duplicate timer timestamp: {error}"));
    let timer = TimerFiredV1 {
        timer_id: timer_id_value.clone(),
        generation,
        scheduled_for,
        canonical_payload: payload.clone(),
    };
    let mut scheduled = BTreeMap::new();
    scheduled.insert(
        timer_id_value.clone(),
        ScheduledTimerV1 {
            timer_id: timer_id_value,
            generation,
            scheduled_for: timer.scheduled_for.clone(),
            canonical_payload: payload,
        },
    );
    let stimulus = RecordedStimulusV1::TimerFired(timer.clone());
    let input = ActivityReduceInputV1 {
        prior_activity_state: &prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers: &scheduled,
        next_room_seq: RoomSequenceV1::new(18)
            .unwrap_or_else(|error| unreachable!("duplicate room sequence: {error}")),
        recorded_stimulus: &stimulus,
    };
    let Err(error) = reduce_timer(&mut advanced, &input, &timer) else {
        unreachable!("a consumed phase timer must reject replay")
    };
    assert!(matches!(error, PackFaultV1::InvalidTimerOutput(_)));
}

#[test]
fn phase_transition_matrix_emits_the_six_phase_timer_contract() {
    let cases = [
        (PhaseV1::Briefing, 1, PhaseV1::Negotiation, PHASE_TIMER, 1),
        (PhaseV1::Negotiation, 2, PhaseV1::Commitment, PHASE_TIMER, 2),
        (
            PhaseV1::Commitment,
            3,
            PhaseV1::Resolution,
            RESOLVE_TIMER,
            1,
        ),
        (PhaseV1::Resolution, 4, PhaseV1::Result, PHASE_TIMER, 1),
        (PhaseV1::Result, 5, PhaseV1::Complete, PHASE_TIMER, 0),
    ];
    let core = core();
    for (phase, generation, next, expected_timer, request_count) in cases {
        let mut state = phase_state(phase, generation, "2026-08-15T14:00:30Z");
        let mut requests = Vec::new();
        enter_phase(
            &mut state,
            next,
            "2026-08-15T14:00:30Z",
            &core,
            &mut requests,
        )
        .unwrap_or_else(|error| unreachable!("phase transition: {error:?}"));
        assert_eq!(state.phase, next);
        assert_eq!(state.phase_generation, generation + 1);
        assert_eq!(requests.len(), request_count);
        if request_count == 0 {
            assert!(state.phase_deadline.is_none());
        } else {
            assert!(
                requests
                    .iter()
                    .any(|request| request.timer_id().as_str() == expected_timer)
            );
        }
    }
}

#[test]
fn timer_replay_after_canonical_restart_has_identical_output() {
    let original = phase_state(PhaseV1::Briefing, 1, "2026-08-15T14:00:30Z");
    let serialized = canonical(&original)
        .unwrap_or_else(|error| unreachable!("restart serialization: {error:?}"));
    let restarted: StateV1 = decode(&serialized)
        .unwrap_or_else(|error| unreachable!("restart deserialization: {error:?}"));
    assert_eq!(original.phase, restarted.phase);

    let (state_a, disposition_a) = timer_disposition(original, PHASE_TIMER, "phase_deadline")
        .unwrap_or_else(|error| unreachable!("original timer reduction: {error:?}"));
    let (state_b, disposition_b) = timer_disposition(restarted, PHASE_TIMER, "phase_deadline")
        .unwrap_or_else(|error| unreachable!("restarted timer reduction: {error:?}"));
    assert_eq!(
        canonical(&state_a).unwrap_or_else(|error| unreachable!("replayed state A: {error:?}")),
        canonical(&state_b).unwrap_or_else(|error| unreachable!("replayed state B: {error:?}"))
    );
    assert_eq!(disposition_a, disposition_b);
}

#[test]
fn absent_broker_timer_path_reaches_result_and_complete_with_golden_outcome() {
    let mut commitment = phase_state(PhaseV1::Commitment, 3, "2026-08-15T14:00:30Z");
    commitment.commitments.insert(
        INSIDER.to_owned(),
        CommitmentV1 {
            selected_plan_id: "plan-1".to_owned(),
            contribute_required_resource: false,
        },
    );
    let (resolution, commitment_disposition) =
        timer_disposition(commitment, PHASE_TIMER, "phase_deadline")
            .unwrap_or_else(|error| unreachable!("D3 closure: {error:?}"));
    assert_eq!(resolution.phase, PhaseV1::Resolution);
    assert_eq!(resolution.phase_generation, 4);
    assert_eq!(resolution.commitments.len(), 2);
    assert!(matches!(
        commitment_disposition,
        ActivityDispositionV1::Apply(_)
    ));

    let (result, result_disposition) = timer_disposition(resolution, RESOLVE_TIMER, "resolve_now")
        .unwrap_or_else(|error| unreachable!("resolve_now: {error:?}"));
    assert_eq!(result.phase, PhaseV1::Result);
    assert_eq!(result.phase_generation, 5);
    let outcome = result
        .outcome
        .as_ref()
        .unwrap_or_else(|| unreachable!("Result must contain an Outcome"));
    assert_eq!(outcome.outcome, "success");
    assert_eq!(outcome.score, 5);
    assert_eq!(outcome.selected_plan_id.as_deref(), Some("plan-1"));
    assert_eq!(outcome.missing_roles, vec![BROKER.to_owned()]);
    assert_eq!(outcome.vote_counts.get("plan-1"), Some(&2));
    assert!(matches!(
        result_disposition,
        ActivityDispositionV1::Apply(_)
    ));

    let (complete, complete_disposition) = timer_disposition(result, PHASE_TIMER, "phase_deadline")
        .unwrap_or_else(|error| unreachable!("D4 closure: {error:?}"));
    assert_eq!(complete.phase, PhaseV1::Complete);
    assert_eq!(complete.phase_generation, 6);
    assert!(complete.phase_deadline.is_none());
    assert!(matches!(
        complete_disposition,
        ActivityDispositionV1::Apply(_)
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn result_to_complete_cancels_timer_and_rejects_duplicate_or_obsolete_generations() {
    let generation = TimerGenerationV1::new(5)
        .unwrap_or_else(|error| unreachable!("Result timer generation: {error}"));
    let scheduled_for: TimerScheduledFor = "2026-08-15T14:00:50Z"
        .parse()
        .unwrap_or_else(|error| unreachable!("Result timer due time: {error}"));
    let payload = canonical_value(&timer_payload("phase_deadline", PhaseV1::Result, 5))
        .unwrap_or_else(|error| unreachable!("Result timer payload: {error}"));
    let scheduled = BTreeMap::from([(
        timer_id(PHASE_TIMER),
        ScheduledTimerV1 {
            timer_id: timer_id(PHASE_TIMER),
            generation,
            scheduled_for: scheduled_for.clone(),
            canonical_payload: payload.clone(),
        },
    )]);

    let result = phase_state(PhaseV1::Result, 5, "2026-08-15T14:00:50Z");
    let (result, first) = action_disposition(
        result,
        NAVIGATOR_MEMBER,
        ACKNOWLEDGE_RESULT,
        "01ARZ3NDEKTSV4RRFFQ69G5FE3",
        "2026-08-15T14:00:31Z",
        &serde_json::json!({}),
        &scheduled,
    )
    .unwrap_or_else(|error| unreachable!("first Result acknowledgement: {error:?}"));
    assert!(matches!(first, ActivityDispositionV1::Apply(_)));

    let before_duplicate =
        canonical(&result).unwrap_or_else(|error| unreachable!("duplicate Result state: {error}"));
    let (duplicate_state, duplicate) = action_disposition(
        result.clone(),
        NAVIGATOR_MEMBER,
        ACKNOWLEDGE_RESULT,
        "01ARZ3NDEKTSV4RRFFQ69G5FE3",
        "2026-08-15T14:00:32Z",
        &serde_json::json!({}),
        &scheduled,
    )
    .unwrap_or_else(|error| unreachable!("duplicate Result acknowledgement: {error:?}"));
    match duplicate {
        ActivityDispositionV1::Reject(rejection) => {
            assert_eq!(rejection.declared_code, "prior_acknowledgement");
        }
        ActivityDispositionV1::Apply(_) => unreachable!("duplicate acknowledgement applied"),
    }
    assert_eq!(
        canonical(&duplicate_state)
            .unwrap_or_else(|error| unreachable!("duplicate state after rejection: {error}")),
        before_duplicate
    );

    let (result, second) = action_disposition(
        result,
        INSIDER_MEMBER,
        ACKNOWLEDGE_RESULT,
        "01ARZ3NDEKTSV4RRFFQ69G5FE4",
        "2026-08-15T14:00:33Z",
        &serde_json::json!({}),
        &scheduled,
    )
    .unwrap_or_else(|error| unreachable!("second Result acknowledgement: {error:?}"));
    assert!(matches!(second, ActivityDispositionV1::Apply(_)));

    let (complete, final_ack) = action_disposition(
        result,
        BROKER_MEMBER,
        ACKNOWLEDGE_RESULT,
        "01ARZ3NDEKTSV4RRFFQ69G5FE5",
        "2026-08-15T14:00:34Z",
        &serde_json::json!({}),
        &scheduled,
    )
    .unwrap_or_else(|error| unreachable!("final Result acknowledgement: {error:?}"));
    let ActivityDispositionV1::Apply(output) = final_ack else {
        unreachable!("final acknowledgement must apply")
    };
    assert_eq!(complete.phase, PhaseV1::Complete);
    assert_eq!(complete.phase_generation, 6);
    assert!(complete.phase_deadline.is_none());
    assert_eq!(
        output.timer_requests,
        vec![TimerRequestV1::CancelCurrent {
            timer_id: timer_id(PHASE_TIMER),
            expected_generation: generation,
        }]
    );
    assert_eq!(output.ordered_domain_events.len(), 1);

    let prior_complete =
        canonical(&complete).unwrap_or_else(|error| unreachable!("complete state: {error}"));
    let obsolete = TimerFiredV1 {
        timer_id: timer_id(PHASE_TIMER),
        generation,
        scheduled_for,
        canonical_payload: payload,
    };
    let stimulus = RecordedStimulusV1::TimerFired(obsolete.clone());
    let complete_core = core();
    let input = ActivityReduceInputV1 {
        prior_activity_state: &prior_complete,
        core_before: &complete_core,
        proposed_core_after: &complete_core,
        scheduled_timers: &scheduled,
        next_room_seq: RoomSequenceV1::new(17)
            .unwrap_or_else(|error| unreachable!("obsolete timer sequence: {error}")),
        recorded_stimulus: &stimulus,
    };
    let mut obsolete_state = complete.clone();
    let Err(error) = reduce_timer(&mut obsolete_state, &input, &obsolete) else {
        unreachable!("obsolete Result timer unexpectedly applied")
    };
    assert!(matches!(
        error,
        PackFaultV1::InvalidTimerOutput(detail) if detail == "obsolete phase timer generation"
    ));
    assert_eq!(
        canonical(&obsolete_state)
            .unwrap_or_else(|error| unreachable!("obsolete state after rejection: {error}")),
        prior_complete
    );
}

fn dummy_head() -> CompleteHeadV1 {
    CompleteHeadV1 {
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FC5"
            .parse()
            .unwrap_or_else(|error| unreachable!("Heist room ID: {error}")),
        room_seq: RoomSequenceV1::new(0)
            .unwrap_or_else(|error| unreachable!("Heist head sequence: {error}")),
        genesis_or_transition_hash: Blake3DigestV1::hash(b"heist-test-head"),
        core_schema_version: "worldstream/core/v1".to_owned(),
        pack_digest: PackDigestV1::from_str(
            "blake3:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_or_else(|error| unreachable!("Heist pack digest: {error}")),
        core_state_hash: Blake3DigestV1::hash(b"heist-test-core"),
        activity_state_hash: Blake3DigestV1::hash(b"heist-test-activity"),
        authoritative_state_hash: Blake3DigestV1::hash(b"heist-test-authoritative"),
    }
}

#[test]
fn duplicate_commitment_and_exact_deadline_are_rejected_without_mutation() {
    let core = core();
    let payload = serde_json::Map::from_iter([
        (
            "selected_plan_id".to_owned(),
            serde_json::Value::String("plan-1".to_owned()),
        ),
        (
            "contribute_required_resource".to_owned(),
            serde_json::Value::Bool(true),
        ),
    ]);
    let action = |member: &str, admitted_at: &str| ParticipantActionV1 {
        member_id: member_id(member),
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD5"
            .parse::<ActionId>()
            .unwrap_or_else(|error| unreachable!("Heist action ID: {error}")),
        action_type: COMMIT_MOVE.to_owned(),
        payload_schema_digest: Blake3DigestV1::hash(b"heist-commit-payload"),
        canonical_payload: canonical_value(&serde_json::Value::Object(payload.clone()))
            .unwrap_or_else(|error| unreachable!("Heist action payload: {error}")),
        exact_basis_head: dummy_head(),
        admitted_at: admitted_at
            .parse()
            .unwrap_or_else(|error| unreachable!("Heist admitted time: {error}")),
    };

    let mut duplicate_state = phase_state(PhaseV1::Commitment, 3, "2026-08-15T14:00:30Z");
    let duplicate_action = action(NAVIGATOR_MEMBER, "2026-08-15T14:00:00Z");
    let duplicate_stimulus = RecordedStimulusV1::ParticipantAction(duplicate_action.clone());
    let duplicate_prior = canonical(&duplicate_state)
        .unwrap_or_else(|error| unreachable!("duplicate prior state: {error:?}"));
    let empty_timers = BTreeMap::new();
    let duplicate_input = ActivityReduceInputV1 {
        prior_activity_state: &duplicate_prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers: &empty_timers,
        next_room_seq: RoomSequenceV1::new(18)
            .unwrap_or_else(|error| unreachable!("duplicate Room sequence: {error}")),
        recorded_stimulus: &duplicate_stimulus,
    };
    let duplicate_before = canonical(&duplicate_state)
        .unwrap_or_else(|error| unreachable!("duplicate state snapshot: {error:?}"));
    let duplicate_result = reduce_action(&mut duplicate_state, &duplicate_input, &duplicate_action)
        .unwrap_or_else(|error| unreachable!("duplicate commitment result: {error:?}"));
    assert!(matches!(
        duplicate_result,
        ActivityDispositionV1::Reject(ActivityRejectionV1 {
            declared_code,
            ..
        }) if declared_code == "prior_commitment"
    ));
    assert_eq!(
        canonical(&duplicate_state)
            .unwrap_or_else(|error| unreachable!("duplicate state after rejection: {error:?}")),
        duplicate_before
    );

    let mut stale_state = phase_state(PhaseV1::Commitment, 3, "2026-08-15T14:00:30Z");
    let stale_action = action(INSIDER_MEMBER, "2026-08-15T14:00:30Z");
    let stale_stimulus = RecordedStimulusV1::ParticipantAction(stale_action.clone());
    let stale_prior = canonical(&stale_state)
        .unwrap_or_else(|error| unreachable!("stale prior state: {error:?}"));
    let stale_input = ActivityReduceInputV1 {
        prior_activity_state: &stale_prior,
        core_before: &core,
        proposed_core_after: &core,
        scheduled_timers: &empty_timers,
        next_room_seq: RoomSequenceV1::new(19)
            .unwrap_or_else(|error| unreachable!("stale Room sequence: {error}")),
        recorded_stimulus: &stale_stimulus,
    };
    let stale_result = reduce_action(&mut stale_state, &stale_input, &stale_action)
        .unwrap_or_else(|error| unreachable!("stale commitment result: {error:?}"));
    assert!(matches!(
        stale_result,
        ActivityDispositionV1::Reject(ActivityRejectionV1 {
            declared_code,
            ..
        }) if declared_code == "wrong_phase"
    ));
}

#[test]
fn public_and_operator_views_never_reveal_sealed_commitments_or_fixture_truth() {
    let state = state();
    let core = core();
    for class in [
        PackViewerClassV1::Public,
        PackViewerClassV1::HistoricalPublic,
        PackViewerClassV1::Operator,
        PackViewerClassV1::HistoricalOperator,
    ] {
        let value = json_projection(
            projection(&state, &core, None, false, class)
                .unwrap_or_else(|error| unreachable!("public Heist projection: {error:?}")),
        );
        let text = value.to_string();
        assert!(!text.contains("commitments"));
        assert!(!text.contains("selected_plan_id"));
        assert!(!text.contains("offer_to_insider"));
    }
}

#[test]
fn final_reveal_is_terminal_only_and_separate_from_historical_views() {
    let core = core();
    let pre_complete = state();
    let result = projection(
        &pre_complete,
        &core,
        Some(NAVIGATOR),
        true,
        PackViewerClassV1::FinalReveal,
    );
    assert!(matches!(
        &result,
        Err(PackFaultV1::PrivacyContract(message))
            if message.contains("unavailable before completion")
    ));
    let Err(denied) = result else {
        unreachable!("pre-Complete FinalReveal must fail closed")
    };
    assert!(matches!(
        denied,
        PackFaultV1::PrivacyContract(message)
            if message.contains("unavailable before completion")
    ));

    let mut complete = pre_complete;
    complete.phase = PhaseV1::Complete;
    let reveal = json_projection(
        projection(
            &complete,
            &core,
            Some(NAVIGATOR),
            true,
            PackViewerClassV1::FinalReveal,
        )
        .unwrap_or_else(|error| unreachable!("terminal FinalReveal: {error:?}")),
    );
    assert!(reveal.get("fixture").is_some());
    assert!(reveal.get("clues").is_some());
    assert!(reveal.get("commitments").is_some());

    let historical = json_projection(
        projection(
            &complete,
            &core,
            Some(NAVIGATOR),
            false,
            PackViewerClassV1::HistoricalParticipant,
        )
        .unwrap_or_else(|error| unreachable!("historical participant: {error:?}")),
    );
    assert!(!historical.to_string().contains("offer_to_insider"));
    assert!(historical.get("fixture").is_none());
    assert!(historical.get("commitments").is_none());
}

#[test]
fn suspended_or_departed_seat_stays_immutable_but_is_not_enabled_or_replaced() {
    let state = state();
    for standing in [
        MembershipStandingV1::Suspended,
        MembershipStandingV1::Departed,
    ] {
        let changed_core = CoreRoomStateV1::active([
            membership(NAVIGATOR_MEMBER, AccessModeV1::Participant, Some(NAVIGATOR)),
            membership(INSIDER_MEMBER, AccessModeV1::Participant, Some(INSIDER)),
            membership_with_standing(
                BROKER_MEMBER,
                AccessModeV1::Participant,
                Some(BROKER),
                standing,
            ),
            membership(SPECTATOR_MEMBER, AccessModeV1::Spectator, None),
            membership(OPERATOR_MEMBER, AccessModeV1::Operator, None),
        ])
        .unwrap_or_else(|error| unreachable!("changed-standing Heist Core: {error}"));
        assert_eq!(state.seats.len(), 3);
        assert!(fixed_seats_match_core(&state, &changed_core));
        assert!(!enabled_seat(&changed_core, &state, BROKER));
        assert!(!agent_target(&changed_core, &state, BROKER));
        assert_eq!(
            role_for_member(&state, &member_id(BROKER_MEMBER)),
            Some(BROKER)
        );

        let public = json_projection(
            projection(
                &state,
                &changed_core,
                None,
                false,
                PackViewerClassV1::Public,
            )
            .unwrap_or_else(|error| unreachable!("changed-standing projection: {error}")),
        );
        assert_eq!(public["seats"][2]["role"], BROKER);
        assert_eq!(public["seats"][2]["present"], false);
        assert!(!public.to_string().contains("private_clues"));

        let broker_view = json_projection(
            projection(
                &state,
                &changed_core,
                Some(BROKER),
                false,
                PackViewerClassV1::Participant,
            )
            .unwrap_or_else(|error| unreachable!("missing-seat participant projection: {error}")),
        );
        assert!(
            broker_view["private_clues"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );

        let mut outcome_state = state.clone();
        outcome_state.phase = PhaseV1::Resolution;
        outcome_state.commitments.insert(
            INSIDER.to_owned(),
            CommitmentV1 {
                selected_plan_id: "plan-1".to_owned(),
                contribute_required_resource: false,
            },
        );
        let outcome = resolve_outcome(&outcome_state);
        assert_eq!(outcome.missing_roles, vec![BROKER.to_owned()]);
        assert_eq!(outcome.vote_counts.get("plan-1"), Some(&2));
        assert_eq!(outcome.score, 5);
    }
}
