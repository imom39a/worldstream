//! Game decisions below consume participant views, never the hidden solution.
use crate::*;
use serde_json::{Value, json};
use std::{error::Error, str::FromStr};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[test]
fn agent_ready_revision_identity_is_fixed() {
    assert_eq!(
        agent_heist_agent_ready_digest().to_string(),
        "blake3:4455e4302bda695a5fc4aca150b5a8dac944775474930dafaef1539acb86c96e"
    );
}
const MEMBERS: [&str; 3] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FA1",
    "01ARZ3NDEKTSV4RRFFQ69G5FA2",
    "01ARZ3NDEKTSV4RRFFQ69G5FA3",
];
const ROLES: [&str; 3] = ["navigator", "insider", "broker"];
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
    let config = CanonicalJsonV1::from_serialize(&json!({
        "pack_id":"worldstream.agent-heist","pack_schema":1,"roles":ROLES,
        "briefing_duration_seconds":30,"negotiation_duration_seconds":90,
        "commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,
        "result_duration_seconds":20,"maximum_open_offers_per_role":4,"maximum_plans":12,
    }))?;
    let genesis = registry.prepare_genesis_for_new_room(&PackGenesisRequestV1 {
        room_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FA0"),
        pack_digest: agent_heist_agent_ready_digest(),
        configuration: config,
        room_seed: parsed("hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
        created_at: parsed("2026-08-23T12:00:00Z"),
        initial_core_state: core,
    })?;
    let mut trace = CoreTraceV1::create_uncommitted(genesis)?;
    trace.advance(RecordedStimulusV1::ExternalInput(ExternalInputV1 {
        source_id: parsed(HOST_LOBBY_LAUNCH_SOURCE),
        input_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0"),
        input_type: "host_launch".into(),
        recorded_at: parsed("2026-08-23T12:01:00Z"),
        canonical_payload: CanonicalJsonV1::from_serialize(&json!({}))?,
        immutable_resource_references: vec![],
    }))?;
    Ok((registry, trace))
}
fn view(trace: &CoreTraceV1, member: &str) -> Result<Value> {
    let view = AgentHeistLobbyV4.view(&ViewInputV1 {
        core: trace.core_state(),
        activity_state: trace.activity_state(),
        complete_head: trace.head(),
        viewer: &PackViewerV1::Participant(parsed(member)),
    })?;
    Ok(serde_json::to_value(view.projection)?)
}
fn attention(trace: &CoreTraceV1) -> Result<Vec<Value>> {
    trace
        .transitions()
        .last()
        .ok_or("missing transition")?
        .ordered_attention_signals()
        .iter()
        .map(|signal| serde_json::to_value(signal).map_err(Into::into))
        .collect()
}
#[allow(clippy::needless_pass_by_value)] // Test call sites pass literal JSON payloads.
fn action(
    trace: &CoreTraceV1,
    member: &str,
    kind: &str,
    payload: Value,
) -> Result<RecordedStimulusV1> {
    let definition = AgentHeistLobbyV4
        .descriptor()
        .actions
        .iter()
        .find(|a| a.action_type == kind)
        .ok_or("unknown action")?;
    let projection = view(trace, member)?;
    let time = projection["phase_start"]
        .as_str()
        .unwrap_or("2026-08-23T12:01:01Z");
    Ok(RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: parsed(member),
        action_id: parsed(&format!("{:026}", trace.head().room_seq().get() + 100)),
        action_type: kind.into(),
        payload_schema_digest: definition.payload_schema.schema_digest.clone(),
        canonical_payload: CanonicalJsonV1::from_serialize(&payload)?,
        exact_basis_head: trace.head().clone(),
        admitted_at: parsed(time),
    }))
}
fn act(trace: &mut CoreTraceV1, member: &str, kind: &str, payload: Value) -> Result<()> {
    let stimulus = action(trace, member, kind, payload)?;
    assert!(
        matches!(
            trace.advance(stimulus)?,
            AdvanceDispositionV1::TransitionAccepted { .. }
        ),
        "{kind} must be accepted"
    );
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
fn failed_initial_decision_gets_one_fresh_reminder_without_extending_briefing() -> Result<()> {
    let (_, mut trace) = launch()?;
    let initial = attention(&trace)?;
    // No Action: simulate a failed provider decision. Time, not a fabricated
    // successful move, must create another decision opportunity.
    fire_next(&mut trace)?;
    let fresh = attention(&trace)?;
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0]["target_member_id"], initial[0]["target_member_id"]);
    assert_ne!(
        fresh[0]["deduplication_key"],
        initial[0]["deduplication_key"]
    );
    assert_eq!(fresh[0]["deadline"], initial[0]["deadline"]);
    assert_eq!(
        trace.scheduled_timers().len(),
        1,
        "reminder must not reschedule itself"
    );
    fire_next(&mut trace)?;
    assert_eq!(view(&trace, MEMBERS[0])?["phase"], "negotiation");
    Ok(())
}

#[test]
fn complete_cooperative_game_uses_authorized_clues_and_replays() -> Result<()> {
    let (registry, mut trace) = launch()?;
    let mut calls = [0_u8; 3];
    for (index, clues) in [
        (0, vec!["route"]),
        (1, vec!["entry_window"]),
        (2, vec!["required_tool", "extraction"]),
    ] {
        for clue in clues {
            assert_eq!(attention(&trace)?[0]["target_member_id"], MEMBERS[index]);
            act(
                &mut trace,
                MEMBERS[index],
                "inspect_clue",
                json!({"clue_id":clue}),
            )?;
            calls[index] += 1;
        }
    }
    assert!(
        attention(&trace)?.is_empty(),
        "no paid invitation when inspection work is done"
    );
    while view(&trace, MEMBERS[0])?["phase"] == "briefing" {
        fire_next(&mut trace)?;
    }
    for index in 0..3 {
        let projection = view(&trace, MEMBERS[index])?;
        for clue in projection["private_clues"]
            .as_array()
            .ok_or("private clues absent")?
        {
            assert_eq!(attention(&trace)?[0]["target_member_id"], MEMBERS[index]);
            act(
                &mut trace,
                MEMBERS[index],
                "publish_clue",
                json!({"clue_id":clue["clue_id"],"claim_code":clue["claim_code"]}),
            )?;
            calls[index] += 1;
        }
    }
    let projection = view(&trace, MEMBERS[0])?;
    let claims = projection["public_claims"]
        .as_array()
        .ok_or("public clues absent")?;
    let mut plan = serde_json::Map::new();
    for field in ["route", "entry_window", "required_tool", "extraction"] {
        let claim = claims
            .iter()
            .find(|c| c["clue_id"] == field)
            .and_then(|c| c["claim_code"].as_str())
            .ok_or("missing shared clue")?;
        let value = claim
            .strip_prefix(&format!("{field}_"))
            .ok_or("invalid claim")?;
        plan.insert(field.into(), json!(value));
    }
    act(&mut trace, MEMBERS[0], "propose_plan", Value::Object(plan))?;
    calls[0] += 1;
    while view(&trace, MEMBERS[0])?["phase"] == "negotiation" {
        fire_next(&mut trace)?;
    }
    let plan_id = view(&trace, MEMBERS[0])?["plans"][0]["plan_id"].clone();
    for index in 0..3 {
        assert_eq!(attention(&trace)?[0]["target_member_id"], MEMBERS[index]);
        act(
            &mut trace,
            MEMBERS[index],
            "commit_move",
            json!({"selected_plan_id":plan_id,"contribute_required_resource":true}),
        )?;
        calls[index] += 1;
    }
    while view(&trace, MEMBERS[0])?["phase"] != "result" {
        fire_next(&mut trace)?;
    }
    for index in 0..3 {
        assert_eq!(attention(&trace)?[0]["target_member_id"], MEMBERS[index]);
        act(&mut trace, MEMBERS[index], "acknowledge_result", json!({}))?;
        calls[index] += 1;
    }
    assert_eq!(view(&trace, MEMBERS[0])?["phase"], "complete");
    assert!(trace.scheduled_timers().is_empty());
    assert!(
        calls.iter().all(|count| *count <= 10),
        "existing House call caps must suffice"
    );
    let state = serde_json::to_value(trace.activity_state())?;
    assert_eq!(state["outcome"]["outcome"], "success");
    assert_eq!(state["outcome"]["score"], 5);
    let replay = CoreTraceV1::replay(
        &registry,
        &trace.genesis_bytes()?,
        &trace.transition_bytes()?,
    )
    .map_err(|e| e.detail)?;
    assert_eq!(replay.final_head, *trace.head());
    Ok(())
}

#[test]
fn competing_action_remains_stale_and_new_invitation_uses_new_head() -> Result<()> {
    let (_, mut trace) = launch()?;
    let stale = action(
        &trace,
        MEMBERS[1],
        "inspect_clue",
        json!({"clue_id":"entry_window"}),
    )?;
    act(
        &mut trace,
        MEMBERS[0],
        "inspect_clue",
        json!({"clue_id":"route"}),
    )?;
    let head = trace.head().clone();
    assert!(trace.advance(stale).is_err());
    assert_eq!(*trace.head(), head);
    assert_eq!(attention(&trace)?[0]["target_member_id"], MEMBERS[1]);
    act(
        &mut trace,
        MEMBERS[1],
        "inspect_clue",
        json!({"clue_id":"entry_window"}),
    )?;
    Ok(())
}
