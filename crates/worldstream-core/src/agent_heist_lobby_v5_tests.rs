use crate::*;
use serde_json::{Value, json};
use std::{error::Error, str::FromStr};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MEMBERS: [&str; 3] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FA1",
    "01ARZ3NDEKTSV4RRFFQ69G5FA2",
    "01ARZ3NDEKTSV4RRFFQ69G5FA3",
];
const ROLES: [&str; 3] = ["navigator", "insider", "broker"];

#[test]
fn schema_safe_revision_identity_is_fixed() {
    assert_eq!(
        agent_heist_schema_safe_digest().to_string(),
        "blake3:56449d0830d1137d69b1b7c11ed25e8f0d9b7188d40e8290c58e5a2caff2bef9"
    );
}

fn parsed<T: FromStr>(value: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("test value: {error:?}"))
}

fn launch() -> Result<(PackRegistryV1, CoreTraceV1)> {
    let registry = builtin_agent_heist_registry()?;
    let core = CoreRoomStateV1::active(
        MEMBERS
            .iter()
            .zip(ROLES)
            .map(|(id, role)| {
                MembershipV1::new(
                    parsed(id),
                    parsed(id),
                    PrincipalKindV1::Agent,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Participant,
                    Some(role.to_owned()),
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
    )?;
    let configuration = CanonicalJsonV1::from_serialize(&json!({
        "pack_id":"worldstream.agent-heist","pack_schema":1,"roles":ROLES,
        "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
        "commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,
        "result_duration_seconds":20,"maximum_open_offers_per_role":4,"maximum_plans":12,
    }))?;
    let genesis = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FA0"),
        pack_digest: agent_heist_schema_safe_digest(),
        configuration,
        room_seed: parsed("hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
        created_at: parsed("2026-08-23T12:00:00Z"),
        initial_core_state: core,
    })?;
    let mut trace = CoreTraceV1::create_uncommitted(genesis)?;
    trace.advance(RecordedStimulusV1::ExternalInput(ExternalInputV1 {
        source_id: parsed(HOST_LOBBY_LAUNCH_SOURCE),
        input_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0"),
        input_type: HOST_LAUNCH_INPUT_TYPE.into(),
        recorded_at: parsed("2026-08-23T12:01:00Z"),
        canonical_payload: CanonicalJsonV1::from_serialize(&json!({}))?,
        immutable_resource_references: vec![],
    }))?;
    Ok((registry, trace))
}

fn view(trace: &CoreTraceV1, member: &str) -> Result<Value> {
    let view = AgentHeistLobbyV5.view(&ViewInputV1 {
        core: trace.core_state(),
        activity_state: trace.activity_state(),
        complete_head: trace.head(),
        viewer: &PackViewerV1::Participant(parsed(member)),
    })?;
    Ok(serde_json::to_value(view.projection)?)
}

fn action(
    trace: &CoreTraceV1,
    member: &str,
    action_type: &str,
    payload: Value,
) -> Result<RecordedStimulusV1> {
    let definition = AgentHeistLobbyV5
        .descriptor()
        .actions
        .iter()
        .find(|definition| definition.action_type == action_type)
        .ok_or("unknown action")?;
    let admitted_at = view(trace, member)?["phase_start"]
        .as_str()
        .unwrap_or("2026-08-23T12:01:01Z")
        .to_owned();
    Ok(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(member),
        action_id: parsed(&format!("{:026}", trace.head().room_seq().get() + 100)),
        action_type: action_type.into(),
        payload_schema_digest: definition.payload_schema.schema_digest.clone(),
        canonical_payload: CanonicalJsonV1::from_serialize(&payload)?,
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed(&admitted_at),
    }))
}

fn act(trace: &mut CoreTraceV1, member: &str, action_type: &str, payload: Value) -> Result<()> {
    let stimulus = action(trace, member, action_type, payload)?;
    assert!(matches!(
        trace.advance(stimulus)?,
        AdvanceDispositionV1::TransitionAccepted { .. }
    ));
    Ok(())
}

fn fire_next(trace: &mut CoreTraceV1) -> Result<()> {
    let scheduled = trace
        .scheduled_timers()
        .values()
        .min_by_key(|timer| timer.scheduled_for.as_str())
        .ok_or("missing timer")?
        .clone();
    trace.advance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: scheduled.timer_id,
        generation: scheduled.generation,
        scheduled_for: scheduled.scheduled_for,
        canonical_payload: scheduled.canonical_payload,
    }))?;
    Ok(())
}

#[test]
fn schema_safe_action_contract_rejects_empty_publish_payload_before_reduce() -> Result<()> {
    let (_, mut trace) = launch()?;
    while view(&trace, MEMBERS[0])?["phase"] == "briefing" {
        fire_next(&mut trace)?;
    }
    let result = trace.advance(action(&trace, MEMBERS[0], PUBLISH_CLUE, json!({}))?);
    assert!(matches!(
        result,
        Err(TraceErrorV1::ActionAdmission(
            ActionAdmissionErrorV1::InvalidPayload(_)
        ))
    ));
    Ok(())
}

#[test]
fn schema_safe_action_contract_accepts_exact_publish_and_commit_payloads() -> Result<()> {
    let (_, mut trace) = launch()?;
    act(
        &mut trace,
        MEMBERS[0],
        INSPECT_CLUE,
        json!({"clue_id":"route"}),
    )?;
    while view(&trace, MEMBERS[0])?["phase"] == "briefing" {
        fire_next(&mut trace)?;
    }
    let clue = view(&trace, MEMBERS[0])?["private_clues"][0].clone();
    act(
        &mut trace,
        MEMBERS[0],
        PUBLISH_CLUE,
        json!({"clue_id":clue["clue_id"],"claim_code":clue["claim_code"]}),
    )?;
    act(
        &mut trace,
        MEMBERS[0],
        PROPOSE_PLAN,
        json!({"route":"canal","entry_window":"late","required_tool":"disguise","extraction":"van"}),
    )?;
    while view(&trace, MEMBERS[0])?["phase"] == "negotiation" {
        fire_next(&mut trace)?;
    }
    let plan_id = view(&trace, MEMBERS[0])?["plans"][0]["plan_id"].clone();
    act(
        &mut trace,
        MEMBERS[0],
        COMMIT_MOVE,
        json!({"selected_plan_id":plan_id,"contribute_required_resource":true}),
    )?;
    Ok(())
}
