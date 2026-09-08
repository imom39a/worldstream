//! Heist 0.4.0: bounded, sequential attention for useful agent participation.
//! Retained 0.3.0 rules remain untouched. This revision changes attention only;
//! Actions, visibility, phase deadlines, scoring and strict Head admission are
//! unchanged. One phase-local reminder provides a bounded fresh opportunity.

use crate::agent_heist_lobby_v3::AgentHeistLobbyV3;
use crate::{
    AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, Blake3DigestV1, CanonicalJsonV1, CoreRoomStateV1,
    DeterministicContextV1, InitialOutputV1, MembershipStandingV1, ObserveInputV1,
    PackCodecBundleV1, PackDigestV1, PackFaultV1, PackObservationV1, PackRevisionDescriptorV1,
    PackRevisionLockV1, PackSchemaBundleV1, PackViewV1, PrincipalKindV1, RecordedStimulusV1,
    TimerId, TimerRequestV1, ViewInputV1,
};
use serde_json::{Value, json};
use std::{str::FromStr, sync::OnceLock};

pub const AGENT_HEIST_AGENT_READY_VERSION: &str = "0.4.0";
const ATTENTION_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ0";
#[derive(Clone, Copy)]
pub struct AgentHeistLobbyV4;

impl ActivityPackV1 for AgentHeistLobbyV4 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        revision().descriptor
    }
    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        AgentHeistLobbyV3.initialize(input, cx)
    }
    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        let reminder = matches!(input.recorded_stimulus,
            RecordedStimulusV1::TimerFired(timer) if timer.timer_id.as_str() == ATTENTION_TIMER);
        let disposition = if reminder {
            reminder_disposition(input)?
        } else {
            AgentHeistLobbyV3.reduce(input, cx)?
        };
        let ActivityDispositionV1::Apply(mut apply) = disposition else {
            return Ok(disposition);
        };
        let state = serde_json::to_value(&apply.next_activity_state).map_err(fault)?;
        let prior = serde_json::to_value(input.prior_activity_state).map_err(fault)?;
        if prior["phase_generation"] != state["phase_generation"] {
            schedule_phase_reminder(input, &state, &mut apply)?;
        }
        if let Some(signal) =
            next_attention(&state, input.proposed_core_after, input.next_room_seq.get())?
        {
            // A single useful phase invitation replaces broad simultaneous
            // invitations. Other targeted negotiations remain legal Actions.
            apply.ordered_attention_signals = vec![signal];
        }
        Ok(ActivityDispositionV1::Apply(apply))
    }
    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        AgentHeistLobbyV3.view(input)
    }
    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        AgentHeistLobbyV3.observe(input)
    }
}

fn reminder_disposition(
    input: &ActivityReduceInputV1<'_>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let RecordedStimulusV1::TimerFired(timer) = input.recorded_stimulus else {
        return Err(fault("attention reminder requires its recorded timer"));
    };
    let scheduled = input
        .scheduled_timers
        .get(&timer.timer_id)
        .ok_or_else(|| fault("attention reminder is not scheduled"))?;
    let state = serde_json::to_value(input.prior_activity_state).map_err(fault)?;
    let payload = serde_json::to_value(&timer.canonical_payload).map_err(fault)?;
    if scheduled.generation != timer.generation
        || scheduled.scheduled_for != timer.scheduled_for
        || scheduled.canonical_payload != timer.canonical_payload
        || payload["phase_generation"] != state["phase_generation"]
        || payload["phase"] != state["phase"]
        || payload["kind"] != "agent_attention_reminder"
    {
        return Err(fault("attention reminder witness mismatch"));
    }
    Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
        next_activity_state: input.prior_activity_state.clone(),
        ordered_domain_events: vec![
            CanonicalJsonV1::from_serialize(&json!({
                "event_type": "agent_attention_reminder", "phase": state["phase"],
                "phase_generation": state["phase_generation"],
            }))
            .map_err(fault)?,
        ],
        timer_requests: Vec::new(),
        ordered_attention_signals: Vec::new(),
    }))
}

fn schedule_phase_reminder(
    input: &ActivityReduceInputV1<'_>,
    state: &Value,
    apply: &mut ActivityApplyV1,
) -> Result<(), PackFaultV1> {
    let id = TimerId::from_str(ATTENTION_TIMER).map_err(fault)?;
    if let Some(current) = input.scheduled_timers.get(&id) {
        apply.timer_requests.push(TimerRequestV1::CancelCurrent {
            timer_id: id.clone(),
            expected_generation: current.generation,
        });
    }
    // Commitment already has its own reminder in the retained game rules.
    // These are one-shot timers: no automatic paid-request retry loop.
    let seconds = match state["phase"].as_str() {
        Some("briefing") => 15,
        Some("negotiation") => 45,
        Some("result") => 10,
        _ => return Ok(()),
    };
    let start = state["phase_start"]
        .as_str()
        .ok_or_else(|| fault("missing phase start"))?;
    let due = crate::agent_heist_clock_safe::add_seconds(start, seconds)?;
    apply.timer_requests.push(TimerRequestV1::ScheduleNext {
        timer_id: id,
        due: due.parse().map_err(fault)?,
        canonical_payload: CanonicalJsonV1::from_serialize(&json!({
            "kind": "agent_attention_reminder", "phase": state["phase"],
            "phase_generation": state["phase_generation"],
        }))
        .map_err(fault)?,
    });
    Ok(())
}

fn next_attention(
    state: &Value,
    core: &CoreRoomStateV1,
    sequence: u64,
) -> Result<Option<CanonicalJsonV1>, PackFaultV1> {
    let phase = state["phase"]
        .as_str()
        .ok_or_else(|| fault("missing phase"))?;
    if matches!(phase, "lobby" | "resolution" | "complete") {
        return Ok(None);
    }
    let clues = state["clues"]
        .as_array()
        .ok_or_else(|| fault("missing clues"))?;
    let plans = state["plans"]
        .as_array()
        .ok_or_else(|| fault("missing plans"))?;
    let acknowledgements = state["result_acknowledgements"]
        .as_array()
        .ok_or_else(|| fault("missing acknowledgements"))?;
    let commitments = state["commitments"]
        .as_object()
        .ok_or_else(|| fault("missing commitments"))?;
    // The already-visible Room sequence identifies a new decision opportunity.
    // It allows a fresh invitation after a competing accepted move without
    // replaying a paid request or silently rebasing the old Action.
    for role in ["navigator", "insider", "broker"] {
        let Some(member) = core.memberships().values().find(|m| {
            m.role() == Some(role)
                && m.principal_kind() == PrincipalKindV1::Agent
                && m.standing() == MembershipStandingV1::Enabled
                && m.access_mode() == AccessModeV1::Participant
        }) else {
            continue;
        };
        let owns_unknown = clues.iter().any(|c| {
            c["owner_role"] == role
                && !c["inspected_by"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|v| v == role))
        });
        let owns_unpublished = clues.iter().any(|c| {
            c["owner_role"] == role
                && c["published_claim_code"].is_null()
                && c["inspected_by"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|v| v == role))
        });
        let selected = match phase {
            "briefing" if owns_unknown => Some(("clue_inspection_needed", "inspect_clue")),
            "negotiation" if owns_unpublished => Some(("clue_sharing_needed", "publish_clue")),
            "commitment" if !commitments.contains_key(role) && !plans.is_empty() => {
                Some(("commitment_opened", "commit_move"))
            }
            "result" if !acknowledgements.iter().any(|v| v == role) => {
                Some(("round_result_available", "acknowledge_result"))
            }
            _ => None,
        };
        if let Some((reason, action)) = selected {
            return signal(
                state,
                member.member_id().as_str(),
                role,
                reason,
                action,
                &sequence.to_string(),
            )
            .map(Some);
        }
    }
    if phase == "negotiation" && plans.is_empty() {
        for role in ["navigator", "insider", "broker"] {
            if let Some(member) = core.memberships().values().find(|m| {
                m.role() == Some(role)
                    && m.principal_kind() == PrincipalKindV1::Agent
                    && m.standing() == MembershipStandingV1::Enabled
                    && m.access_mode() == AccessModeV1::Participant
            }) {
                return signal(
                    state,
                    member.member_id().as_str(),
                    role,
                    "plan_needed",
                    "propose_plan",
                    &sequence.to_string(),
                )
                .map(Some);
            }
        }
    }
    Ok(None)
}

fn signal(
    state: &Value,
    member: &str,
    role: &str,
    reason: &str,
    action: &str,
    progress: &str,
) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(&json!({
        "target_member_id": member, "reason": reason, "priority": 1,
        "deduplication_key": format!("agent-ready:{reason}:{role}:{}:{progress}", state["phase_generation"]),
        "deadline": state["phase_deadline"], "action_types": [action],
    })).map_err(fault)
}

struct Revision {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact: Blake3DigestV1,
}
fn revision() -> &'static Revision {
    static VALUE: OnceLock<Revision> = OnceLock::new();
    VALUE.get_or_init(|| {
        let (prior, prior_lock, schemas, codecs, _) =
            crate::agent_heist_lobby_v3::agent_heist_lobby_revision();
        let mut descriptor = prior.clone();
        AGENT_HEIST_AGENT_READY_VERSION.clone_into(&mut descriptor.explanatory_version);
        let attention_schema = descriptor.output_schemas["attention:commitment_opened"].clone();
        descriptor.event_schemas.insert(
            "agent_attention_reminder".to_owned(),
            descriptor.event_schemas["commitment_reminder"].clone(),
        );
        for reason in [
            "clue_inspection_needed",
            "clue_sharing_needed",
            "plan_needed",
        ] {
            descriptor.attention_reasons.push(reason.to_owned());
            descriptor
                .output_schemas
                .insert(format!("attention:{reason}"), attention_schema.clone());
        }
        descriptor.revision_digest = PackDigestV1::from_str(&format!("blake3:{}", "0".repeat(64)))
            .unwrap_or_else(|error| unreachable!("fixed digest: {error}"));
        let artifact = Blake3DigestV1::hash(
            &include_bytes!("agent_heist_lobby_v4.rs")
                .iter()
                .chain(include_bytes!("agent_heist_lobby_v3.rs"))
                .chain(include_bytes!("agent_heist_clock_safe.rs"))
                .copied()
                .filter(|byte| *byte != b'\r')
                .collect::<Vec<_>>(),
        );
        let mut lock = prior_lock.clone();
        AGENT_HEIST_AGENT_READY_VERSION.clone_into(&mut lock.explanatory_version);
        lock.descriptor_digest = descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("agent-ready descriptor: {error}"));
        lock.rule_source_digest = artifact.clone();
        descriptor.revision_digest = lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("agent-ready revision: {error}"));
        Revision {
            descriptor: Box::leak(Box::new(descriptor)),
            lock,
            schemas: schemas.clone(),
            codecs: codecs.clone(),
            artifact,
        }
    })
}
pub(crate) fn agent_heist_agent_ready_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let r = revision();
    (r.descriptor, &r.lock, &r.schemas, &r.codecs, &r.artifact)
}
pub(crate) fn artifact_digest() -> Blake3DigestV1 {
    revision().artifact.clone()
}
#[must_use]
pub fn agent_heist_agent_ready_digest() -> PackDigestV1 {
    revision().descriptor.revision_digest.clone()
}
fn fault(error: impl std::fmt::Display) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}
