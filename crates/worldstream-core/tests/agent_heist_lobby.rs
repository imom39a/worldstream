use std::{fmt::Display, str::FromStr};

use worldstream_core::{
    AccessModeV1, AdministrationOperationIdentityV1, AdvanceDispositionV1, CORE_OPERATION_KIND,
    CanonicalJsonV1, CoreAuthorityAttributionV1, CoreAuthorityKindV1, CoreChangeSetV1,
    CoreProposedKindV1, CoreProposedV1, CoreRoomStateV1, CoreTraceV1, ExternalInputV1, InputId,
    MembershipChangeV1, MembershipStandingV1, MembershipV1, PackGenesisRequestV1, PackRegistryV1,
    PrincipalKindV1, RecordedStimulusV1, RoomStatusV1, SourceId, agent_heist_lobby_digest,
    builtin_agent_heist_registry,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA0";
const NAVIGATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA1";
const INSIDER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA2";
const BROKER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA3";
const OPERATOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA5";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const CONFIG: &[u8] = br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#;

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

fn core() -> CoreRoomStateV1 {
    let mut memberships = [
        (NAVIGATOR, "navigator"),
        (INSIDER, "insider"),
        (BROKER, "broker"),
    ]
    .map(|(member_id, role)| {
        MembershipV1::new(
            parsed(member_id),
            parsed(member_id),
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some(role.to_owned()),
        )
        .unwrap_or_else(|error| unreachable!("fixture Membership: {error}"))
    })
    .to_vec();
    memberships.push(
        MembershipV1::new(
            parsed(OPERATOR),
            parsed(OPERATOR),
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Operator,
            None,
        )
        .unwrap_or_else(|error| unreachable!("fixture operator Membership: {error}")),
    );
    CoreRoomStateV1::active(memberships)
        .unwrap_or_else(|error| unreachable!("fixture Core: {error}"))
}

fn trace_for(registry: &PackRegistryV1) -> CoreTraceV1 {
    let request = PackGenesisRequestV1 {
        room_id: parsed(ROOM),
        pack_digest: agent_heist_lobby_digest(),
        configuration: canonical(CONFIG),
        room_seed: parsed(SEED),
        created_at: parsed("2026-08-23T12:00:00Z"),
        initial_core_state: core(),
    };
    let prepared = registry
        .prepare_genesis_for_new_room(&request)
        .unwrap_or_else(|error| unreachable!("checked Heist Genesis: {error}"));
    CoreTraceV1::create_from_retained_for_conformance(prepared)
        .unwrap_or_else(|error| unreachable!("registry-bound trace: {error}"))
}

fn phase(trace: &CoreTraceV1) -> String {
    serde_json::to_value(trace.activity_state())
        .ok()
        .and_then(|value| {
            value
                .get("phase")
                .and_then(|phase| phase.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| unreachable!("Heist phase is absent"))
}

fn launch(input_id: &str, recorded_at: &str) -> RecordedStimulusV1 {
    RecordedStimulusV1::ExternalInput(ExternalInputV1 {
        source_id: parsed::<SourceId>(worldstream_core::HOST_LOBBY_LAUNCH_SOURCE),
        input_id: parsed::<InputId>(input_id),
        input_type: "host_launch".to_owned(),
        recorded_at: parsed(recorded_at),
        canonical_payload: canonical(br"{}"),
        immutable_resource_references: Vec::new(),
    })
}

fn core_proposal(
    trace: &CoreTraceV1,
    kind: CoreProposedKindV1,
    changeset: CoreChangeSetV1,
    idempotency_key: &str,
) -> RecordedStimulusV1 {
    RecordedStimulusV1::CoreProposed(CoreProposedV1::new(
        kind,
        CoreAuthorityAttributionV1 {
            principal_id: parsed(OPERATOR),
            authority_kind: CoreAuthorityKindV1::HostOperator,
        },
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(OPERATOR),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        },
        trace.head().room_seq(),
        "test.lobby_mandatory_core",
        parsed("2026-08-23T12:00:30Z"),
        changeset,
    ))
}

fn install(trace: &mut CoreTraceV1, stimulus: RecordedStimulusV1) {
    let prepared = trace
        .prepare(stimulus)
        .unwrap_or_else(|error| unreachable!("Lobby proposal preparation: {error}"));
    trace
        .install_prepared_for_conformance(prepared)
        .unwrap_or_else(|error| unreachable!("Lobby proposal install: {error}"));
}

#[test]
fn new_exact_revision_waits_in_lobby_without_timer_or_gameplay_side_effects() {
    let registry = builtin_agent_heist_registry()
        .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
    let trace = trace_for(&registry);

    assert_eq!(phase(&trace), "lobby");
    assert_eq!(
        serde_json::to_value(trace.activity_state())
            .ok()
            .and_then(|value| value["phase_deadline"].as_str().map(str::to_owned)),
        None
    );
    assert_eq!(trace.head().room_seq().get(), 0);
}

#[test]
fn lobby_preserves_mandatory_suspend_depart_and_archive_core_semantics() {
    let registry = builtin_agent_heist_registry()
        .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));

    let mut suspended = trace_for(&registry);
    let operator = suspended
        .core_state()
        .membership(&parsed(OPERATOR))
        .unwrap_or_else(|| unreachable!("operator Membership"))
        .clone();
    let proposal = core_proposal(
        &suspended,
        CoreProposedKindV1::Suspend,
        CoreChangeSetV1::one(MembershipChangeV1::suspend(operator)),
        "suspend",
    );
    install(&mut suspended, proposal);
    assert_eq!(phase(&suspended), "lobby");
    assert_eq!(
        suspended
            .core_state()
            .membership(&parsed(OPERATOR))
            .map(MembershipV1::standing),
        Some(MembershipStandingV1::Suspended)
    );

    let mut departed = trace_for(&registry);
    let navigator = departed
        .core_state()
        .membership(&parsed(NAVIGATOR))
        .unwrap_or_else(|| unreachable!("navigator Membership"))
        .clone();
    let proposal = core_proposal(
        &departed,
        CoreProposedKindV1::Depart,
        CoreChangeSetV1::one(MembershipChangeV1::depart(navigator)),
        "depart",
    );
    install(&mut departed, proposal);
    assert_eq!(phase(&departed), "lobby");
    assert_eq!(
        departed
            .core_state()
            .membership(&parsed(NAVIGATOR))
            .map(MembershipV1::standing),
        Some(MembershipStandingV1::Departed)
    );

    let mut archived = trace_for(&registry);
    let proposal = core_proposal(
        &archived,
        CoreProposedKindV1::Archive,
        CoreChangeSetV1::archive(archived.core_state().room_status()),
        "archive",
    );
    install(&mut archived, proposal);
    assert_eq!(phase(&archived), "lobby");
    assert_eq!(archived.core_state().room_status(), RoomStatusV1::Archived);
}

#[test]
fn recorded_host_launch_enters_briefing_once_and_replays_deterministically() {
    let registry = builtin_agent_heist_registry()
        .unwrap_or_else(|error| unreachable!("Agent Heist registry: {error}"));
    let mut trace = trace_for(&registry);
    let prepared = trace
        .prepare(launch("01ARZ3NDEKTSV4RRFFQ69G5FA4", "2026-08-23T12:01:00Z"))
        .unwrap_or_else(|error| unreachable!("host launch preparation: {error}"));
    assert!(matches!(
        trace
            .install_prepared_for_conformance(prepared)
            .unwrap_or_else(|error| unreachable!("host launch install: {error}")),
        AdvanceDispositionV1::TransitionAccepted {
            existing: false,
            ..
        }
    ));
    assert_eq!(phase(&trace), "briefing");
    assert_eq!(trace.head().room_seq().get(), 1);
    assert_eq!(
        serde_json::to_value(trace.activity_state())
            .ok()
            .and_then(|value| value["phase_deadline"].as_str().map(str::to_owned)),
        Some("2026-08-23T12:01:30Z".to_owned())
    );

    let genesis = trace
        .genesis_bytes()
        .unwrap_or_else(|error| unreachable!("Genesis bytes: {error}"));
    let transitions = trace
        .transition_bytes()
        .unwrap_or_else(|error| unreachable!("Transition bytes: {error}"));
    let replay = CoreTraceV1::replay(&registry, &genesis, &transitions)
        .unwrap_or_else(|failure| unreachable!("Lobby replay: {}", failure.detail));
    assert_eq!(phase(&replay.into_trace()), "briefing");
}
