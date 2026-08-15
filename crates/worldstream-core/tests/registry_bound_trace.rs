use std::{fmt::Display, str::FromStr};

use worldstream_core::{
    AccessModeV1, ActionAdmissionErrorV1, ActionId, ActivityObservationOutcomeV1,
    AdvanceDispositionV1, CanonicalJsonV1, CoreRoomStateV1, CoreTraceV1, MemberId,
    MembershipStandingV1, MembershipV1, PackGenesisRequestV1, PackRegistryV1, ParticipantActionV1,
    PrincipalKindV1, RecordedStimulusV1, RoomSeedV1, TraceErrorV1, builtin_counter_registry,
    counter_v2_digest,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PARTICIPANT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("fixture value {value}: {error}"))
}

fn canonical(bytes: &[u8]) -> CanonicalJsonV1 {
    CanonicalJsonV1::parse(bytes)
        .unwrap_or_else(|error| unreachable!("fixture canonical JSON: {error}"))
}

fn new_trace(initial_value: u32, maximum_value: u32) -> (PackRegistryV1, CoreTraceV1) {
    let registry = builtin_counter_registry()
        .unwrap_or_else(|error| unreachable!("Counter registry: {error}"));
    let participant = MembershipV1::new(
        parsed(PARTICIPANT),
        parsed(PRINCIPAL),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("participant fixture: {error}"));
    let core = CoreRoomStateV1::active([participant])
        .unwrap_or_else(|error| unreachable!("Core fixture: {error}"));
    let configuration = canonical(
        format!(r#"{{"initial_value":{initial_value},"maximum_value":{maximum_value}}}"#)
            .as_bytes(),
    );
    let request = PackGenesisRequestV1 {
        room_id: parsed(ROOM),
        pack_digest: counter_v2_digest(),
        configuration,
        room_seed: parsed(SEED),
        created_at: parsed("2026-08-15T12:00:00Z"),
        initial_core_state: core,
    };
    let prepared = registry
        .prepare_genesis_for_new_room(&request)
        .unwrap_or_else(|error| unreachable!("checked Counter Genesis: {error}"));
    let trace = CoreTraceV1::create_from_retained_for_conformance(prepared)
        .unwrap_or_else(|error| unreachable!("registry-bound trace: {error}"));
    (registry, trace)
}

fn participant_action(
    trace: &CoreTraceV1,
    action_type: &str,
    action_id: &str,
    admitted_at: &str,
) -> RecordedStimulusV1 {
    let retained = trace
        .retained_pack()
        .unwrap_or_else(|| unreachable!("production trace retains its exact pack"));
    let definition = retained
        .descriptor()
        .actions
        .iter()
        .find(|definition| definition.action_type == action_type)
        .unwrap_or_else(|| unreachable!("Counter action {action_type}"));
    RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed::<MemberId>(PARTICIPANT),
        action_id: parsed::<ActionId>(action_id),
        action_type: action_type.to_owned(),
        payload_schema_digest: definition.payload_schema.schema_digest.clone(),
        canonical_payload: canonical(br"{}"),
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed(admitted_at),
    })
}

#[test]
fn participant_admission_uses_the_exact_current_view_and_never_reduces_stale_work() {
    let (_registry, mut trace) = new_trace(0, 4);
    let first = participant_action(
        &trace,
        "private_ack",
        "01ARZ3NDEKTSV4RRFFQ69G5FC3",
        "2026-08-15T12:00:01Z",
    );
    let stale = participant_action(
        &trace,
        "increment",
        "01ARZ3NDEKTSV4RRFFQ69G5FC4",
        "2026-08-15T12:00:02Z",
    );
    let prepared = trace
        .prepare(first)
        .unwrap_or_else(|error| unreachable!("first preparation: {error}"));
    assert!(matches!(
        trace
            .install_prepared_for_conformance(prepared)
            .unwrap_or_else(|error| unreachable!("first install: {error}")),
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        }
    ));
    assert_eq!(trace.activity_callback_count(), 1);
    assert!(matches!(
        trace.prepare(stale),
        Err(TraceErrorV1::CompleteHeadMismatch)
    ));
    assert_eq!(trace.activity_callback_count(), 1);
    assert_eq!(trace.head().room_seq().get(), 1);
}

#[test]
fn absent_current_offer_is_admission_not_a_pack_reduce_invocation() {
    let (_registry, trace) = new_trace(1, 1);
    let increment = participant_action(
        &trace,
        "increment",
        "01ARZ3NDEKTSV4RRFFQ69G5FC5",
        "2026-08-15T12:00:01Z",
    );
    assert!(matches!(
        trace.prepare(increment),
        Err(TraceErrorV1::ActionAdmission(
            ActionAdmissionErrorV1::ActionNotAllowed(action_type)
        )) if action_type == "increment"
    ));
    assert_eq!(trace.activity_callback_count(), 0);
    assert_eq!(trace.head().room_seq().get(), 0);
}

#[test]
fn sealed_prepared_observation_and_replay_continuation_remain_registry_bound() {
    let (registry, mut trace) = new_trace(0, 4);
    let first = participant_action(
        &trace,
        "increment",
        "01ARZ3NDEKTSV4RRFFQ69G5FC6",
        "2026-08-15T12:00:01Z",
    );
    let prepared = trace
        .prepare(first)
        .unwrap_or_else(|error| unreachable!("first preparation: {error}"));
    assert!(matches!(
        trace
            .observe_prepared_for_conformance(
                &prepared,
                &worldstream_core::PackViewerV1::Participant(parsed(PARTICIPANT)),
            )
            .unwrap_or_else(|error| unreachable!("sealed observation: {error}")),
        ActivityObservationOutcomeV1::Observation(_)
    ));
    trace
        .install_prepared_for_conformance(prepared)
        .unwrap_or_else(|error| unreachable!("first install: {error}"));

    let genesis_bytes = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}"));
    let transition_bytes = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"));
    let report = CoreTraceV1::replay(&registry, &genesis_bytes, &transition_bytes)
        .unwrap_or_else(|failure| unreachable!("retained replay: {}", failure.detail));
    assert_eq!(report.activity_callback_count, 1);
    let mut restored = report.into_trace();
    assert_eq!(
        restored
            .retained_pack()
            .map(|pack| &pack.descriptor().revision_digest),
        Some(&counter_v2_digest())
    );
    assert_eq!(restored.room_seed(), &parsed::<RoomSeedV1>(SEED));

    let continuation = participant_action(
        &restored,
        "private_ack",
        "01ARZ3NDEKTSV4RRFFQ69G5FC7",
        "2026-08-15T12:00:02Z",
    );
    let prepared = restored
        .prepare(continuation)
        .unwrap_or_else(|error| unreachable!("continuation preparation: {error}"));
    restored
        .install_prepared_for_conformance(prepared)
        .unwrap_or_else(|error| unreachable!("continuation install: {error}"));
    assert_eq!(restored.head().room_seq().get(), 2);
}
