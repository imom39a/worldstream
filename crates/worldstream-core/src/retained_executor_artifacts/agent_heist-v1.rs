//! The trusted, compiled Agent Heist v0.1 Activity Pack.
//!
//! This module deliberately contains only Activity State, reduction, views,
//! observations, and generation-free timer requests. Ordering, timer
//! generations, Core membership, persistence, and delivery remain owned by
//! the generic `WorldStream` host.

use std::{collections::BTreeMap, str::FromStr, sync::OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ACTION_OFFER_DOMAIN, ACTIVITY_PACK_HOST_CONTRACT_ID, AccessModeV1, ActionDefinitionV1,
    ActionOfferV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, ActivityRejectionV1, Blake3DigestV1, CANONICAL_CODEC_ID,
    CanonicalJsonV1, DeterministicContextV1, EligibilityWindowV1, InitialOutputV1, ObserveInputV1,
    PACK_REVISION_LOCK_ID, PackCodecBundleV1, PackDigestV1, PackFaultV1, PackLimitsV1,
    PackObservationV1, PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1,
    PackSchemaV1, PackViewV1, PackViewerClassV1, PackViewerV1, RoleDefinitionV1, TimerRequestV1,
    ViewInputV1,
};

use crate::{
    AccessModeV1 as Access, MemberId, MembershipStandingV1, PrincipalKindV1, RecordedStimulusV1,
    TimerId,
};

pub const AGENT_HEIST_PACK_ID: &str = "worldstream.agent-heist";
pub const AGENT_HEIST_VERSION: &str = "0.1.0";
pub const AGENT_HEIST_RETAINED_VERSION: &str = "0.0.1";

pub const NAVIGATOR: &str = "navigator";
pub const INSIDER: &str = "insider";
pub const BROKER: &str = "broker";
pub const ROLES: [&str; 3] = [NAVIGATOR, INSIDER, BROKER];

pub const INSPECT_CLUE: &str = "inspect_clue";
pub const PUBLISH_CLUE: &str = "publish_clue";
pub const OFFER_EXCHANGE: &str = "offer_exchange";
pub const ACCEPT_EXCHANGE: &str = "accept_exchange";
pub const PROPOSE_PLAN: &str = "propose_plan";
pub const ENDORSE_PLAN: &str = "endorse_plan";
pub const CHALLENGE_PLAN: &str = "challenge_plan";
pub const COMMIT_MOVE: &str = "commit_move";
pub const ACKNOWLEDGE_RESULT: &str = "acknowledge_result";

const ATTENTION_OFFER_RECEIVED: &str = "offer_received";
const ATTENTION_ENDORSEMENT_REQUESTED: &str = "endorsement_requested";
const ATTENTION_COMMITMENT_OPENED: &str = "commitment_opened";
const ATTENTION_REQUIRED_ACTION_DEADLINE: &str = "required_action_deadline";
const ATTENTION_ROUND_RESULT_AVAILABLE: &str = "round_result_available";

const PHASE_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH0";
const REMINDER_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH1";
const RESOLVE_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH2";

const FIXTURE_LABEL: &str = "agent-heist/fixture/v1";
const FIXTURE_ID_CANAL: &str = "canal_shift";
const FIXTURE_ID_SERVICE: &str = "service_window";
const FIXTURE_ID_ROOF: &str = "roof_signal";

#[derive(Clone, Copy)]
pub struct AgentHeistV1;

/// The previous executable Heist revision. It intentionally remains
/// runnable for retained Rooms but is not selectable for new Rooms.
///
/// The implementation is kept on the Activity Pack seam and shares the
/// already-frozen reducer behavior. Its distinct descriptor/lock identity is
/// what lets stored Genesis bytes resolve to the exact historical revision.
#[derive(Clone, Copy)]
pub struct AgentHeistV0;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationV1 {
    pack_id: String,
    pack_schema: u32,
    roles: Vec<String>,
    briefing_duration_seconds: u64,
    negotiation_duration_seconds: u64,
    commitment_duration_seconds: u64,
    commitment_reminder_seconds_before_deadline: u64,
    result_duration_seconds: u64,
    maximum_plans: u32,
    maximum_open_offers_per_role: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PhaseV1 {
    Briefing,
    Negotiation,
    Commitment,
    Resolution,
    Result,
    Complete,
}

impl PhaseV1 {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FixtureV1 {
    fixture_id: String,
    route: String,
    entry_window: String,
    required_tool: String,
    extraction: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SeatV1 {
    role: String,
    member_id: MemberId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClueV1 {
    clue_id: String,
    owner_role: String,
    claim_code: String,
    inspected_by: Vec<String>,
    disclosed_to: Vec<String>,
    published_claim_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExchangeV1 {
    offer_id: String,
    sender_role: String,
    recipient_role: String,
    offered_clue_id: String,
    consideration_kind: String,
    consideration_id: String,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanV1 {
    plan_id: String,
    proposer_role: String,
    created_room_seq: u64,
    route: String,
    entry_window: String,
    required_tool: String,
    extraction: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChallengeV1 {
    role: String,
    plan_id: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommitmentV1 {
    selected_plan_id: String,
    contribute_required_resource: bool,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChecksV1 {
    route: bool,
    entry_window: bool,
    required_tool: bool,
    extraction: bool,
    resource_contributed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OutcomeV1 {
    outcome: String,
    selected_plan_id: Option<String>,
    vote_counts: BTreeMap<String, u32>,
    missing_roles: Vec<String>,
    checks: Option<ChecksV1>,
    score: u8,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateV1 {
    phase: PhaseV1,
    phase_generation: u32,
    phase_start: String,
    phase_deadline: Option<String>,
    fixture_id: String,
    fixture: FixtureV1,
    seats: Vec<SeatV1>,
    clues: Vec<ClueV1>,
    exchanges: Vec<ExchangeV1>,
    plans: Vec<PlanV1>,
    endorsements: BTreeMap<String, String>,
    challenges: Vec<ChallengeV1>,
    commitments: BTreeMap<String, CommitmentV1>,
    result_acknowledgements: Vec<String>,
    outcome: Option<OutcomeV1>,
}

#[derive(Clone, Copy)]
struct FixtureRow {
    id: &'static str,
    route: &'static str,
    entry_window: &'static str,
    required_tool: &'static str,
    extraction: &'static str,
}

const FIXTURES: [FixtureRow; 3] = [
    FixtureRow {
        id: FIXTURE_ID_CANAL,
        route: "canal",
        entry_window: "late",
        required_tool: "disguise",
        extraction: "van",
    },
    FixtureRow {
        id: FIXTURE_ID_SERVICE,
        route: "service",
        entry_window: "early",
        required_tool: "thermal_key",
        extraction: "boat",
    },
    FixtureRow {
        id: FIXTURE_ID_ROOF,
        route: "roof",
        entry_window: "middle",
        required_tool: "jammer",
        extraction: "motorbike",
    },
];

impl ActivityPackV1 for AgentHeistV1 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        initialize(input, cx)
    }

    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        _cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        reduce(input)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        view(input)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        observe(input)
    }
}

impl ActivityPackV1 for AgentHeistV0 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        legacy_revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        AgentHeistV1.initialize(input, cx)
    }

    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        AgentHeistV1.reduce(input, cx)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        AgentHeistV1.view(input)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        AgentHeistV1.observe(input)
    }
}

fn initialize(
    input: &ActivityGenesisInputV1<'_>,
    cx: &DeterministicContextV1<'_>,
) -> Result<InitialOutputV1, PackFaultV1> {
    let config: ConfigurationV1 = decode(input.configuration)?;
    validate_configuration(&config)?;
    let seats = fixed_seats(input.initial_core_state)?;
    let fixture_index = usize::try_from(
        cx.uniform_index(
            FIXTURE_LABEL,
            0,
            u64::try_from(FIXTURES.len())
                .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
        )
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    let fixture = FIXTURES[fixture_index];
    let phase_start = input.created_at.as_str().to_owned();
    let phase_deadline = add_seconds(&phase_start, config.briefing_duration_seconds)?;
    let state = StateV1 {
        phase: PhaseV1::Briefing,
        phase_generation: 1,
        phase_start,
        phase_deadline: Some(phase_deadline.clone()),
        fixture_id: fixture.id.to_owned(),
        fixture: FixtureV1 {
            fixture_id: fixture.id.to_owned(),
            route: fixture.route.to_owned(),
            entry_window: fixture.entry_window.to_owned(),
            required_tool: fixture.required_tool.to_owned(),
            extraction: fixture.extraction.to_owned(),
        },
        seats,
        clues: initial_clues(&fixture),
        exchanges: Vec::new(),
        plans: Vec::new(),
        endorsements: BTreeMap::new(),
        challenges: Vec::new(),
        commitments: BTreeMap::new(),
        result_acknowledgements: Vec::new(),
        outcome: None,
    };
    Ok(InitialOutputV1 {
        initial_activity_state: canonical(&state)?,
        timer_requests: vec![TimerRequestV1::ScheduleNext {
            timer_id: timer_id(PHASE_TIMER),
            due: phase_deadline.parse().map_err(timestamp_fault)?,
            canonical_payload: canonical(&timer_payload("phase_deadline", PhaseV1::Briefing, 1))?,
        }],
    })
}

fn reduce(input: &ActivityReduceInputV1<'_>) -> Result<ActivityDispositionV1, PackFaultV1> {
    let mut state: StateV1 = decode(input.prior_activity_state)?;
    match input.recorded_stimulus {
        RecordedStimulusV1::ParticipantAction(action) => reduce_action(&mut state, input, action),
        RecordedStimulusV1::TimerFired(timer) => reduce_timer(&mut state, input, timer),
        RecordedStimulusV1::CoreProposed(_) => {
            if fixed_seats_match_core(&state, input.proposed_core_after) {
                apply(&state, Vec::new(), Vec::new())
            } else {
                reject("fixed_genesis_seat")
            }
        }
        RecordedStimulusV1::ExternalInput(_) => apply(&state, Vec::new(), Vec::new()),
    }
}

fn reduce_action(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    action: &crate::ParticipantActionV1,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let role = role_for_member(state, &action.member_id)
        .map(str::to_owned)
        .ok_or_else(|| {
            PackFaultV1::InvalidOutput("Action Member is not an immutable Heist seat".to_owned())
        })?;
    let membership = input
        .core_before
        .membership(&action.member_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("Action Membership is absent".to_owned()))?;
    if membership.standing() != MembershipStandingV1::Enabled
        || membership.access_mode() != Access::Participant
        || membership.role() != Some(role.as_str())
    {
        return reject("unavailable_seat");
    }
    let payload = payload_object(&action.canonical_payload)?;
    match action.action_type.as_str() {
        INSPECT_CLUE => inspect_clue(state, &role, &payload),
        PUBLISH_CLUE => publish_clue(state, &role, &payload),
        OFFER_EXCHANGE => offer_exchange(state, input, &role, &payload),
        ACCEPT_EXCHANGE => accept_exchange(state, input, &role, &payload),
        PROPOSE_PLAN => propose_plan(state, input, &role, action, &payload),
        ENDORSE_PLAN => endorse_plan(state, &role, &payload),
        CHALLENGE_PLAN => challenge_plan(state, &role, &payload),
        COMMIT_MOVE => commit_move(state, input, &role, &payload),
        ACKNOWLEDGE_RESULT => acknowledge_result(state, input, &role),
        _ => Err(PackFaultV1::InvalidOutput(format!(
            "host admitted undeclared Heist Action {}",
            action.action_type
        ))),
    }
}

fn inspect_clue(
    state: &mut StateV1,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if !matches!(
        state.phase,
        PhaseV1::Briefing | PhaseV1::Negotiation | PhaseV1::Commitment
    ) {
        return reject("wrong_phase");
    }
    let clue_id = required_string(payload, "clue_id")?;
    let clue = state
        .clues
        .iter_mut()
        .find(|clue| clue.clue_id == clue_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown clue".to_owned()))?;
    if clue.owner_role != role || clue.inspected_by.iter().any(|item| item == role) {
        return reject("clue_ownership_or_knowledge");
    }
    clue.inspected_by.push(role.to_owned());
    event_apply(
        state,
        "clue_inspected",
        serde_json::json!({"event_type":"clue_inspected","role":role,"clue_id":clue_id}),
    )
}

fn publish_clue(
    state: &mut StateV1,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    let clue_id = required_string(payload, "clue_id")?;
    let claim_code = required_string(payload, "claim_code")?;
    let clue = state
        .clues
        .iter_mut()
        .find(|clue| clue.clue_id == clue_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown clue".to_owned()))?;
    if !known(clue, role) {
        return reject("clue_ownership_or_knowledge");
    }
    if clue.claim_code != claim_code {
        return reject("invalid_claim_code");
    }
    if clue.published_claim_code.is_some() {
        return reject("invalid_claim_code");
    }
    clue.published_claim_code = Some(claim_code.clone());
    event_apply(
        state,
        "clue_published",
        serde_json::json!({"event_type":"clue_published","role":role,"clue_id":clue_id,"claim_code":claim_code}),
    )
}

fn offer_exchange(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    let recipient_role = required_string(payload, "recipient_role")?;
    let offered_clue_id = required_string(payload, "offered_clue_id")?;
    if !ROLES.contains(&recipient_role.as_str())
        || recipient_role == role
        || !enabled_seat(input.proposed_core_after, state, &recipient_role)
    {
        return reject("invalid_or_bounded_offer");
    }
    let clue = state
        .clues
        .iter()
        .find(|clue| clue.clue_id == offered_clue_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown offered clue".to_owned()))?;
    if !known(clue, role) {
        return reject("clue_ownership_or_knowledge");
    }
    let consideration = payload
        .get("consideration")
        .and_then(Value::as_object)
        .ok_or_else(|| PackFaultV1::InvalidOutput("consideration must be an object".to_owned()))?;
    let consideration_kind = required_string(consideration, "kind")?;
    let consideration_id = if consideration_kind == "clue_disclosure" {
        required_string(consideration, "clue_id")?
    } else if consideration_kind == "plan_endorsement" {
        required_string(consideration, "plan_id")?
    } else {
        return reject("invalid_or_bounded_offer");
    };
    if consideration_kind == "clue_disclosure" {
        let target = state
            .clues
            .iter()
            .find(|clue| clue.clue_id == consideration_id)
            .ok_or_else(|| PackFaultV1::InvalidOutput("unknown consideration clue".to_owned()))?;
        if target.owner_role != recipient_role || !known(target, &recipient_role) {
            return reject("invalid_or_bounded_offer");
        }
    } else if !state
        .plans
        .iter()
        .any(|plan| plan.plan_id == consideration_id)
    {
        return reject("missing_or_duplicate_plan");
    }
    let open = state
        .exchanges
        .iter()
        .filter(|offer| offer.sender_role == role && offer.status == "open")
        .count();
    if open >= 4 {
        return reject("invalid_or_bounded_offer");
    }
    state.exchanges.push(ExchangeV1 {
        offer_id: action_id(input)?,
        sender_role: role.to_owned(),
        recipient_role: recipient_role.clone(),
        offered_clue_id: offered_clue_id.clone(),
        consideration_kind: consideration_kind.clone(),
        consideration_id: consideration_id.clone(),
        status: "open".to_owned(),
    });
    let attention = attention_for_candidates(
        state,
        input.proposed_core_after,
        [(recipient_role.as_str(), ATTENTION_OFFER_RECEIVED)],
    );
    event_apply_with_attention(
        state,
        "exchange_offered",
        serde_json::json!({"event_type":"exchange_offered","sender_role":role,"recipient_role":recipient_role,"offer_id":action_id(input)?}),
        attention,
    )
}

fn accept_exchange(
    state: &mut StateV1,
    _input: &ActivityReduceInputV1<'_>,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    let offer_id = required_string(payload, "offer_id")?;
    let index = state
        .exchanges
        .iter()
        .position(|offer| offer.offer_id == offer_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown offer".to_owned()))?;
    let offer = state.exchanges[index].clone();
    if offer.recipient_role != role || offer.status != "open" {
        return reject("invalid_or_bounded_offer");
    }
    let offered = state
        .clues
        .iter()
        .find(|clue| clue.clue_id == offer.offered_clue_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown offered clue".to_owned()))?;
    if !known(offered, &offer.sender_role) {
        return reject("invalid_or_bounded_offer");
    }
    if offer.consideration_kind == "clue_disclosure" {
        let consideration = state
            .clues
            .iter()
            .find(|clue| clue.clue_id == offer.consideration_id)
            .ok_or_else(|| PackFaultV1::InvalidOutput("unknown consideration clue".to_owned()))?;
        if consideration.owner_role != role || !known(consideration, role) {
            return reject("invalid_or_bounded_offer");
        }
    } else if !state
        .plans
        .iter()
        .any(|plan| plan.plan_id == offer.consideration_id)
    {
        return reject("invalid_or_bounded_offer");
    }
    "accepted".clone_into(&mut state.exchanges[index].status);
    if let Some(clue) = state
        .clues
        .iter_mut()
        .find(|clue| clue.clue_id == offer.offered_clue_id)
    {
        insert_role(&mut clue.disclosed_to, role);
    }
    if offer.consideration_kind == "clue_disclosure" {
        if let Some(clue) = state
            .clues
            .iter_mut()
            .find(|clue| clue.clue_id == offer.consideration_id)
        {
            insert_role(&mut clue.disclosed_to, &offer.sender_role);
        }
    } else {
        state
            .endorsements
            .insert(role.to_owned(), offer.consideration_id.clone());
    }
    event_apply(
        state,
        "exchange_accepted",
        serde_json::json!({"event_type":"exchange_accepted","offer_id":offer_id,"recipient_role":role}),
    )
}

fn propose_plan(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    role: &str,
    action: &crate::ParticipantActionV1,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    if state.plans.len() >= 12 {
        return reject("missing_or_duplicate_plan");
    }
    let route = required_string(payload, "route")?;
    let entry_window = required_string(payload, "entry_window")?;
    let required_tool = required_string(payload, "required_tool")?;
    let extraction = required_string(payload, "extraction")?;
    if !valid_plan_values(&route, &entry_window, &required_tool, &extraction) {
        return reject("missing_or_duplicate_plan");
    }
    if state.plans.iter().any(|plan| {
        plan.route == route
            && plan.entry_window == entry_window
            && plan.required_tool == required_tool
            && plan.extraction == extraction
    }) {
        return reject("missing_or_duplicate_plan");
    }
    let plan_id = action.action_id.to_string();
    state.plans.push(PlanV1 {
        plan_id: plan_id.clone(),
        proposer_role: role.to_owned(),
        created_room_seq: input.next_room_seq.get(),
        route,
        entry_window,
        required_tool,
        extraction,
    });
    let attention = ROLES
        .iter()
        .filter(|target| **target != role && agent_target(input.proposed_core_after, state, target))
        .map(|target| (*target, ATTENTION_ENDORSEMENT_REQUESTED))
        .collect::<Vec<_>>();
    let attention = attention_for_candidates(state, input.proposed_core_after, attention);
    event_apply_with_attention(
        state,
        "plan_proposed",
        serde_json::json!({"event_type":"plan_proposed","plan_id":plan_id,"proposer_role":role}),
        attention,
    )
}

fn endorse_plan(
    state: &mut StateV1,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    let plan_id = required_string(payload, "plan_id")?;
    if !state.plans.iter().any(|plan| plan.plan_id == plan_id) {
        return reject("missing_or_duplicate_plan");
    }
    state.endorsements.insert(role.to_owned(), plan_id.clone());
    event_apply(
        state,
        "plan_endorsed",
        serde_json::json!({"event_type":"plan_endorsed","role":role,"plan_id":plan_id}),
    )
}

fn challenge_plan(
    state: &mut StateV1,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Negotiation {
        return reject("wrong_phase");
    }
    let plan_id = required_string(payload, "plan_id")?;
    let reason = required_string(payload, "reason")?;
    if ![
        "route_conflict",
        "timing_conflict",
        "tool_conflict",
        "extraction_conflict",
    ]
    .contains(&reason.as_str())
    {
        return reject("unsupported_challenge");
    }
    if !state.plans.iter().any(|plan| plan.plan_id == plan_id)
        || state.challenges.iter().any(|challenge| {
            challenge.role == role && challenge.plan_id == plan_id && challenge.reason == reason
        })
    {
        return reject("unsupported_challenge");
    }
    let clue_id = match reason.as_str() {
        "route_conflict" => "route",
        "timing_conflict" => "entry_window",
        "tool_conflict" => "required_tool",
        _ => "extraction",
    };
    let clue = state
        .clues
        .iter()
        .find(|clue| clue.clue_id == clue_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown challenge clue".to_owned()))?;
    let plan = state
        .plans
        .iter()
        .find(|plan| plan.plan_id == plan_id)
        .ok_or_else(|| PackFaultV1::InvalidOutput("unknown challenge plan".to_owned()))?;
    let value = match clue_id {
        "route" => &plan.route,
        "entry_window" => &plan.entry_window,
        "required_tool" => &plan.required_tool,
        _ => &plan.extraction,
    };
    let truth = fixture_field(&state.fixture, clue_id);
    if !known(clue, role) || value == truth {
        return reject("unsupported_challenge");
    }
    state.challenges.push(ChallengeV1 {
        role: role.to_owned(),
        plan_id: plan_id.clone(),
        reason: reason.clone(),
    });
    event_apply(
        state,
        "plan_challenged",
        serde_json::json!({"event_type":"plan_challenged","role":role,"plan_id":plan_id,"reason":reason}),
    )
}

fn commit_move(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    role: &str,
    payload: &serde_json::Map<String, Value>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Commitment {
        return reject("wrong_phase");
    }
    if state.commitments.contains_key(role) {
        return reject("prior_commitment");
    }
    let plan_id = required_string(payload, "selected_plan_id")?;
    let contribute = payload
        .get("contribute_required_resource")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            PackFaultV1::InvalidOutput("contribute_required_resource must be boolean".to_owned())
        })?;
    if !state.plans.iter().any(|plan| plan.plan_id == plan_id) {
        return reject("missing_or_duplicate_plan");
    }
    let admitted_at = input.recorded_stimulus.semantic_time();
    if state.phase_start.as_str() > admitted_at
        || state
            .phase_deadline
            .as_deref()
            .is_none_or(|deadline| admitted_at >= deadline)
    {
        return reject("wrong_phase");
    }
    state.commitments.insert(
        role.to_owned(),
        CommitmentV1 {
            selected_plan_id: plan_id.clone(),
            contribute_required_resource: contribute,
        },
    );
    let mut requests = Vec::new();
    let mut phase_after = state.phase;
    if state.commitments.len() == ROLES.len() {
        cancel_current(&mut requests, input, REMINDER_TIMER);
        cancel_current(&mut requests, input, PHASE_TIMER);
        enter_phase(
            state,
            PhaseV1::Resolution,
            admitted_at,
            input.proposed_core_after,
            &mut requests,
        )?;
        phase_after = state.phase;
    }
    let event = commitment_accepted_event(
        role,
        state.commitments.len(),
        phase_after,
        state.phase_generation,
        state.phase_deadline.as_deref(),
    );
    apply_with_requests(state, vec![canonical_value(&event)?], requests, Vec::new())
}

fn acknowledge_result(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    role: &str,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    if state.phase != PhaseV1::Result {
        return reject("wrong_phase");
    }
    if state
        .result_acknowledgements
        .iter()
        .any(|item| item == role)
    {
        return reject("prior_acknowledgement");
    }
    state.result_acknowledgements.push(role.to_owned());
    let mut requests = Vec::new();
    if state.result_acknowledgements.len() == ROLES.len() {
        cancel_current(&mut requests, input, PHASE_TIMER);
        state.phase = PhaseV1::Complete;
        state.phase_generation = state.phase_generation.saturating_add(1);
        input
            .recorded_stimulus
            .semantic_time()
            .clone_into(&mut state.phase_start);
        state.phase_deadline = None;
    }
    let event = serde_json::json!({"event_type":"result_acknowledged","role":role,"acknowledgement_count":state.result_acknowledgements.len(),"phase_after":state.phase,"phase_generation_after":state.phase_generation,"phase_deadline_after":state.phase_deadline});
    apply_with_requests(state, vec![canonical_value(&event)?], requests, Vec::new())
}

#[allow(clippy::too_many_lines)]
fn reduce_timer(
    state: &mut StateV1,
    input: &ActivityReduceInputV1<'_>,
    timer: &crate::TimerFiredV1,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let scheduled = input.scheduled_timers.get(&timer.timer_id).ok_or_else(|| {
        PackFaultV1::InvalidTimerOutput("timer witness is not scheduled".to_owned())
    })?;
    if scheduled.generation != timer.generation
        || scheduled.scheduled_for != timer.scheduled_for
        || scheduled.canonical_payload != timer.canonical_payload
    {
        return Err(PackFaultV1::InvalidTimerOutput(
            "timer witness mismatch".to_owned(),
        ));
    }
    let payload = payload_object(&timer.canonical_payload)?;
    let kind = required_string(&payload, "kind")?;
    let phase_generation = u32::try_from(
        payload
            .get("phase_generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PackFaultV1::InvalidOutput("timer payload lacks phase_generation".to_owned())
            })?,
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    if phase_generation != state.phase_generation {
        return Err(PackFaultV1::InvalidTimerOutput(
            "obsolete phase timer generation".to_owned(),
        ));
    }
    if kind == "commitment_reminder" {
        if timer.timer_id != timer_id(REMINDER_TIMER) || state.phase != PhaseV1::Commitment {
            return Err(PackFaultV1::InvalidTimerOutput(
                "reminder is not applicable".to_owned(),
            ));
        }
        let attention = ROLES
            .iter()
            .filter(|role| {
                !state.commitments.contains_key(**role)
                    && agent_target(input.proposed_core_after, state, role)
            })
            .map(|role| (*role, ATTENTION_REQUIRED_ACTION_DEADLINE))
            .collect::<Vec<_>>();
        let attention = attention_for_candidates(state, input.proposed_core_after, attention);
        let missing = ROLES
            .iter()
            .filter(|role| !state.commitments.contains_key(**role))
            .map(|role| (*role).to_owned())
            .collect::<Vec<_>>();
        return event_apply_with_attention(
            state,
            "commitment_reminder",
            serde_json::json!({"event_type":"commitment_reminder","missing_roles":missing,"phase_after":state.phase,"phase_generation_after":state.phase_generation,"phase_deadline_after":state.phase_deadline}),
            attention,
        );
    }
    if timer.timer_id != timer_id(PHASE_TIMER)
        && !(timer.timer_id == timer_id(RESOLVE_TIMER) && kind == "resolve_now")
    {
        return Err(PackFaultV1::InvalidTimerOutput(
            "unknown Heist timer".to_owned(),
        ));
    }
    let scheduled_for = timer.scheduled_for.as_str();
    let mut requests = Vec::new();
    let (next, event_type) = match kind.as_str() {
        "phase_deadline" if state.phase == PhaseV1::Briefing => {
            (PhaseV1::Negotiation, "phase_timer_fired")
        }
        "phase_deadline" if state.phase == PhaseV1::Negotiation => {
            (PhaseV1::Commitment, "phase_timer_fired")
        }
        "phase_deadline" if state.phase == PhaseV1::Commitment => {
            cancel_current(&mut requests, input, REMINDER_TIMER);
            (PhaseV1::Resolution, "phase_timer_fired")
        }
        "resolve_now" if state.phase == PhaseV1::Resolution => {
            (PhaseV1::Result, "resolution_timer_fired")
        }
        "phase_deadline" if state.phase == PhaseV1::Result => {
            (PhaseV1::Complete, "phase_timer_fired")
        }
        _ => {
            return Err(PackFaultV1::InvalidTimerOutput(
                "phase timer purpose is not applicable".to_owned(),
            ));
        }
    };
    enter_phase(
        state,
        next,
        scheduled_for,
        input.proposed_core_after,
        &mut requests,
    )?;
    let mut extra = serde_json::json!({"event_type":event_type,"fired_kind":kind,"phase_after":state.phase,"phase_generation_after":state.phase_generation,"phase_deadline_after":state.phase_deadline});
    if next == PhaseV1::Result {
        state.outcome = Some(resolve_outcome(state));
        extra["outcome"] = serde_json::to_value(
            state
                .outcome
                .as_ref()
                .ok_or_else(|| PackFaultV1::InvalidOutput("outcome missing".to_owned()))?,
        )
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    }
    apply_with_requests(
        state,
        vec![canonical_value(&extra)?],
        requests,
        attention_for_phase(state, input.proposed_core_after),
    )
}

fn enter_phase(
    state: &mut StateV1,
    next: PhaseV1,
    effective: &str,
    core: &crate::CoreRoomStateV1,
    requests: &mut Vec<TimerRequestV1>,
) -> Result<(), PackFaultV1> {
    state.phase = next;
    state.phase_generation = state
        .phase_generation
        .checked_add(1)
        .ok_or_else(|| PackFaultV1::InvalidOutput("phase generation overflow".to_owned()))?;
    effective.clone_into(&mut state.phase_start);
    let config = default_configuration();
    match next {
        PhaseV1::Negotiation => {
            let due = add_seconds(effective, config.negotiation_duration_seconds)?;
            state.phase_deadline = Some(due.clone());
            requests.push(schedule(
                PHASE_TIMER,
                &due,
                &timer_payload("phase_deadline", next, state.phase_generation),
            )?);
        }
        PhaseV1::Commitment => {
            let due = add_seconds(effective, config.commitment_duration_seconds)?;
            let reminder =
                subtract_seconds(&due, config.commitment_reminder_seconds_before_deadline)?;
            state.phase_deadline = Some(due.clone());
            requests.push(schedule(
                PHASE_TIMER,
                &due,
                &timer_payload("phase_deadline", next, state.phase_generation),
            )?);
            requests.push(schedule(
                REMINDER_TIMER,
                &reminder,
                &timer_payload("commitment_reminder", next, state.phase_generation),
            )?);
        }
        PhaseV1::Resolution => {
            let due = successor(effective)?;
            state.phase_deadline = Some(due.clone());
            requests.push(schedule(
                RESOLVE_TIMER,
                &due,
                &timer_payload("resolve_now", next, state.phase_generation),
            )?);
        }
        PhaseV1::Result => {
            let due = add_seconds(effective, config.result_duration_seconds)?;
            state.phase_deadline = Some(due.clone());
            requests.push(schedule(
                PHASE_TIMER,
                &due,
                &timer_payload("phase_deadline", next, state.phase_generation),
            )?);
        }
        PhaseV1::Complete => {
            state.phase_deadline = None;
        }
        PhaseV1::Briefing => {
            return Err(PackFaultV1::InvalidOutput(
                "cannot enter Briefing".to_owned(),
            ));
        }
    }
    let _ = core;
    Ok(())
}

fn attention_for_phase(state: &StateV1, core: &crate::CoreRoomStateV1) -> Vec<CanonicalJsonV1> {
    let reason = match state.phase {
        PhaseV1::Commitment => ATTENTION_COMMITMENT_OPENED,
        PhaseV1::Result => ATTENTION_ROUND_RESULT_AVAILABLE,
        _ => return Vec::new(),
    };
    let candidates = ROLES
        .iter()
        .filter(|role| {
            (reason != ATTENTION_COMMITMENT_OPENED || !state.commitments.contains_key(**role))
                && agent_target(core, state, role)
        })
        .map(|role| (*role, reason))
        .collect::<Vec<_>>();
    attention_for_candidates(state, core, candidates)
}

fn resolve_outcome(state: &StateV1) -> OutcomeV1 {
    let mut counts = BTreeMap::new();
    for commitment in state.commitments.values() {
        *counts
            .entry(commitment.selected_plan_id.clone())
            .or_insert(0) += 1;
    }
    let missing_roles = ROLES
        .iter()
        .filter(|role| !state.commitments.contains_key(**role))
        .map(|role| (*role).to_owned())
        .collect();
    let majority = counts
        .iter()
        .filter(|(_, count)| **count >= 2)
        .map(|(plan, _)| plan.clone())
        .collect::<Vec<_>>();
    if majority.len() != 1 {
        return OutcomeV1 {
            outcome: "failure".to_owned(),
            selected_plan_id: None,
            vote_counts: counts,
            missing_roles,
            checks: None,
            score: 0,
            reason: "no_strict_majority".to_owned(),
        };
    }
    let selected = &majority[0];
    let plan = state.plans.iter().find(|plan| &plan.plan_id == selected);
    let Some(plan) = plan else {
        return OutcomeV1 {
            outcome: "failure".to_owned(),
            selected_plan_id: Some(selected.clone()),
            vote_counts: counts,
            missing_roles,
            checks: None,
            score: 0,
            reason: "no_strict_majority".to_owned(),
        };
    };
    let checks = ChecksV1 {
        route: plan.route == state.fixture.route,
        entry_window: plan.entry_window == state.fixture.entry_window,
        required_tool: plan.required_tool == state.fixture.required_tool,
        extraction: plan.extraction == state.fixture.extraction,
        resource_contributed: state.commitments.values().any(|commitment| {
            commitment.selected_plan_id == *selected && commitment.contribute_required_resource
        }),
    };
    let score = [
        checks.route,
        checks.entry_window,
        checks.required_tool,
        checks.extraction,
        checks.resource_contributed,
    ]
    .into_iter()
    .filter(|value| *value)
    .count()
    .try_into()
    .unwrap_or(0);
    let outcome = if score == 5 {
        "success"
    } else if score >= 3 {
        "partial_failure"
    } else {
        "failure"
    };
    OutcomeV1 {
        outcome: outcome.to_owned(),
        selected_plan_id: Some(selected.clone()),
        vote_counts: counts,
        missing_roles,
        checks: Some(checks),
        score,
        reason: "scored_selected_plan".to_owned(),
    }
}

#[must_use]
pub fn outcome_for_matrix(
    commitments: &BTreeMap<String, (String, bool)>,
    fixture: Option<&str>,
) -> (String, u8) {
    let fixture = fixture.unwrap_or(FIXTURE_ID_SERVICE);
    let row = FIXTURES
        .iter()
        .find(|row| row.id == fixture)
        .copied()
        .unwrap_or(FIXTURES[1]);
    let state = StateV1 {
        phase: PhaseV1::Resolution,
        phase_generation: 4,
        phase_start: String::new(),
        phase_deadline: None,
        fixture_id: row.id.to_owned(),
        fixture: FixtureV1 {
            fixture_id: row.id.to_owned(),
            route: row.route.to_owned(),
            entry_window: row.entry_window.to_owned(),
            required_tool: row.required_tool.to_owned(),
            extraction: row.extraction.to_owned(),
        },
        seats: Vec::new(),
        clues: Vec::new(),
        exchanges: Vec::new(),
        plans: vec![PlanV1 {
            plan_id: "correct".to_owned(),
            proposer_role: NAVIGATOR.to_owned(),
            created_room_seq: 1,
            route: row.route.to_owned(),
            entry_window: row.entry_window.to_owned(),
            required_tool: row.required_tool.to_owned(),
            extraction: row.extraction.to_owned(),
        }],
        endorsements: BTreeMap::new(),
        challenges: Vec::new(),
        commitments: commitments
            .iter()
            .map(|(role, (plan, resource))| {
                (
                    role.clone(),
                    CommitmentV1 {
                        selected_plan_id: plan.clone(),
                        contribute_required_resource: *resource,
                    },
                )
            })
            .collect(),
        result_acknowledgements: Vec::new(),
        outcome: None,
    };
    let outcome = resolve_outcome(&state);
    (outcome.outcome, outcome.score)
}

fn view(input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
    let state: StateV1 = decode(input.activity_state)?;
    let class = viewer_class(input.core, input.viewer)?;
    if matches!(input.viewer, PackViewerV1::FinalReveal(_)) && !state.phase.is_terminal() {
        return Err(PackFaultV1::PrivacyContract(
            "Agent Heist final reveal is unavailable before completion".to_owned(),
        ));
    }
    let role = role_for_member(&state, input.viewer.member_id());
    let projection = projection(
        &state,
        input.core,
        role,
        matches!(input.viewer, PackViewerV1::FinalReveal(_)),
        class,
    )?;
    let offers = if matches!(input.viewer, PackViewerV1::Participant(_)) {
        action_offers(&state, input.viewer.member_id(), role, input.core)?
    } else {
        Vec::new()
    };
    Ok(PackViewV1 {
        projection_schema: revision()
            .descriptor
            .projection_schemas
            .get(&class)
            .ok_or_else(|| PackFaultV1::PrivacyContract("missing projection schema".to_owned()))?
            .schema_id
            .clone(),
        projection,
        action_offers: offers,
    })
}

fn observe(input: &ObserveInputV1<'_>) -> Result<Option<PackObservationV1>, PackFaultV1> {
    let before: StateV1 = decode(input.activity_before)?;
    let after: StateV1 = decode(input.activity_after)?;
    let class = viewer_class(input.core_after, input.viewer)?;
    let role = role_for_member(&after, input.viewer.member_id());
    let final_reveal = matches!(input.viewer, PackViewerV1::FinalReveal(_));
    if final_reveal && !after.phase.is_terminal() {
        return Err(PackFaultV1::PrivacyContract(
            "Agent Heist final reveal is unavailable before completion".to_owned(),
        ));
    }
    let before_projection = projection(
        &before,
        input.core_before,
        role_for_member(&before, input.viewer.member_id()),
        final_reveal && before.phase.is_terminal(),
        viewer_class(input.core_before, input.viewer)?,
    )?;
    let after_projection = projection(&after, input.core_after, role, final_reveal, class)?;
    let before_offers = if matches!(input.viewer, PackViewerV1::Participant(_)) {
        action_offers(
            &before,
            input.viewer.member_id(),
            role_for_member(&before, input.viewer.member_id()),
            input.core_before,
        )?
    } else {
        Vec::new()
    };
    let offers = if matches!(input.viewer, PackViewerV1::Participant(_)) {
        action_offers(&after, input.viewer.member_id(), role, input.core_after)?
    } else {
        Vec::new()
    };
    if before_projection == after_projection
        && before_offers == offers
        && input.core_before == input.core_after
    {
        return Ok(None);
    }
    let schema = revision()
        .descriptor
        .observation_schemas
        .get(&class)
        .ok_or_else(|| PackFaultV1::PrivacyContract("missing observation schema".to_owned()))?
        .schema_id
        .clone();
    let mut observation = PackObservationV1::new(schema, after_projection);
    if before_offers != offers {
        observation = observation.with_action_offers(input.after_view.action_offers().clone());
    }
    Ok(Some(observation))
}

#[allow(clippy::too_many_lines)]
fn projection(
    state: &StateV1,
    core: &crate::CoreRoomStateV1,
    role: Option<&str>,
    final_reveal: bool,
    class: PackViewerClassV1,
) -> Result<CanonicalJsonV1, PackFaultV1> {
    let final_reveal = final_reveal && class == PackViewerClassV1::FinalReveal;
    if final_reveal && !state.phase.is_terminal() {
        return Err(PackFaultV1::PrivacyContract(
            "Agent Heist final reveal is unavailable before completion".to_owned(),
        ));
    }
    let seats = ROLES
        .iter()
        .map(|seat| serde_json::json!({"role":seat,"present":enabled_seat(core, state, seat)}))
        .collect::<Vec<_>>();
    let public_clues = state
        .clues
        .iter()
        .filter_map(|clue| {
            clue.published_claim_code
                .as_ref()
                .map(|claim| serde_json::json!({"clue_id":clue.clue_id,"claim_code":claim}))
        })
        .collect::<Vec<_>>();
    let plans = state.plans.iter().map(|plan| serde_json::json!({"plan_id":plan.plan_id,"proposer_role":plan.proposer_role,"created_room_seq":plan.created_room_seq,"route":plan.route,"entry_window":plan.entry_window,"required_tool":plan.required_tool,"extraction":plan.extraction})).collect::<Vec<_>>();
    let challenges = state.challenges.iter().map(|challenge| serde_json::json!({"role":challenge.role,"plan_id":challenge.plan_id,"reason":challenge.reason})).collect::<Vec<_>>();
    let mut object = serde_json::Map::new();
    object.insert(
        "phase".to_owned(),
        serde_json::to_value(state.phase).map_err(json_fault)?,
    );
    object.insert(
        "phase_generation".to_owned(),
        Value::from(state.phase_generation),
    );
    object.insert(
        "phase_start".to_owned(),
        Value::from(state.phase_start.clone()),
    );
    object.insert(
        "phase_deadline".to_owned(),
        serde_json::to_value(&state.phase_deadline).map_err(json_fault)?,
    );
    object.insert("seats".to_owned(), Value::Array(seats));
    object.insert("public_claims".to_owned(), Value::Array(public_clues));
    object.insert("plans".to_owned(), Value::Array(plans));
    object.insert(
        "endorsements".to_owned(),
        serde_json::to_value(&state.endorsements).map_err(json_fault)?,
    );
    object.insert("challenges".to_owned(), Value::Array(challenges));
    object.insert(
        "commitment_count".to_owned(),
        Value::from(
            u64::try_from(state.commitments.len())
                .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
        ),
    );
    object.insert(
        "outcome".to_owned(),
        serde_json::to_value(&state.outcome).map_err(json_fault)?,
    );
    // Final reveal is a separately typed, post-completion surface. Keep its
    // private fields tied to that class as well as the typed-view flag so a
    // malformed internal call cannot turn a public/operator projection into
    // a reveal.
    if final_reveal {
        object.insert("exchanges".to_owned(), serde_json::to_value(state.exchanges.iter().map(|offer| serde_json::json!({"offer_id":offer.offer_id,"sender_role":offer.sender_role,"recipient_role":offer.recipient_role,"offered_clue_id":offer.offered_clue_id,"consideration_kind":offer.consideration_kind,"consideration_id":offer.consideration_id,"status":offer.status})).collect::<Vec<_>>()).map_err(json_fault)?);
    }
    if final_reveal {
        object.insert(
            "fixture".to_owned(),
            serde_json::to_value(&state.fixture).map_err(json_fault)?,
        );
        object.insert(
            "clues".to_owned(),
            serde_json::to_value(&state.clues).map_err(json_fault)?,
        );
        object.insert(
            "commitments".to_owned(),
            serde_json::to_value(&state.commitments).map_err(json_fault)?,
        );
    } else if matches!(
        class,
        PackViewerClassV1::Participant | PackViewerClassV1::HistoricalParticipant
    ) {
        let private_clues = state.clues.iter().filter(|clue| role.is_some_and(|role| known(clue, role))).map(|clue| serde_json::json!({"clue_id":clue.clue_id,"known":true,"owner_role":clue.owner_role,"claim_code":clue.claim_code})).collect::<Vec<_>>();
        object.insert("private_clues".to_owned(), Value::Array(private_clues));
        object.insert(
            "own_commitment".to_owned(),
            serde_json::to_value(role.and_then(|role| state.commitments.get(role)))
                .map_err(json_fault)?,
        );
        let addressed_offers = role
            .map(|role| {
                state
                    .exchanges
                    .iter()
                    .filter(|offer| offer.recipient_role == role)
                    .map(|offer| {
                        serde_json::json!({
                            "offer_id": offer.offer_id,
                            "sender_role": offer.sender_role,
                            "recipient_role": offer.recipient_role,
                            "offered_clue_id": offer.offered_clue_id,
                            "consideration_kind": offer.consideration_kind,
                            "consideration_id": offer.consideration_id,
                            "status": offer.status,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        object.insert(
            "addressed_offers".to_owned(),
            Value::Array(addressed_offers),
        );
    }
    let projection = Value::Object(object);
    canonical_value(&projection)
}

fn action_offers(
    state: &StateV1,
    _member_id: &MemberId,
    role: Option<&str>,
    core: &crate::CoreRoomStateV1,
) -> Result<Vec<ActionOfferV1>, PackFaultV1> {
    let role = role.ok_or_else(|| {
        PackFaultV1::PrivacyContract("participant is not a fixed Heist seat".to_owned())
    })?;
    if !enabled_seat(core, state, role) {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let descriptor = revision().descriptor;
    let add = |result: &mut Vec<ActionOfferV1>,
               action_type: &str,
               window: Option<EligibilityWindowV1>|
     -> Result<(), PackFaultV1> {
        let definition = descriptor
            .actions
            .iter()
            .find(|definition| definition.action_type == action_type)
            .ok_or_else(|| PackFaultV1::InvalidOutput("missing Action descriptor".to_owned()))?;
        result.push(ActionOfferV1 {
            domain: ACTION_OFFER_DOMAIN.to_owned(),
            action_type: action_type.to_owned(),
            payload_schema_digest: definition.payload_schema.schema_digest.clone(),
            eligibility_window: window,
        });
        Ok(())
    };
    if matches!(
        state.phase,
        PhaseV1::Briefing | PhaseV1::Negotiation | PhaseV1::Commitment
    ) && state
        .clues
        .iter()
        .any(|clue| clue.owner_role == role && !known(clue, role))
    {
        add(&mut result, INSPECT_CLUE, None)?;
    }
    if state.phase == PhaseV1::Negotiation {
        add(&mut result, PUBLISH_CLUE, None)?;
        add(&mut result, OFFER_EXCHANGE, None)?;
        add(&mut result, ACCEPT_EXCHANGE, None)?;
        add(&mut result, PROPOSE_PLAN, None)?;
        add(&mut result, ENDORSE_PLAN, None)?;
        add(&mut result, CHALLENGE_PLAN, None)?;
    }
    if state.phase == PhaseV1::Commitment && !state.commitments.contains_key(role) {
        let window = EligibilityWindowV1 {
            opens_at: state.phase_start.parse().map_err(timestamp_fault)?,
            deadline: state
                .phase_deadline
                .clone()
                .ok_or_else(|| PackFaultV1::InvalidOutput("Commitment has no deadline".to_owned()))?
                .parse()
                .map_err(timestamp_fault)?,
        };
        add(&mut result, COMMIT_MOVE, Some(window))?;
    }
    if state.phase == PhaseV1::Result
        && !state
            .result_acknowledgements
            .iter()
            .any(|item| item == role)
    {
        add(&mut result, ACKNOWLEDGE_RESULT, None)?;
    }
    Ok(result)
}

fn viewer_class(
    core: &crate::CoreRoomStateV1,
    viewer: &PackViewerV1,
) -> Result<PackViewerClassV1, PackFaultV1> {
    let membership = core
        .membership(viewer.member_id())
        .ok_or_else(|| PackFaultV1::PrivacyContract("viewer Membership is absent".to_owned()))?;
    if membership.standing() != MembershipStandingV1::Enabled {
        return Err(PackFaultV1::PrivacyContract(
            "viewer Membership is not enabled".to_owned(),
        ));
    }
    match (viewer, membership.access_mode()) {
        (PackViewerV1::Public(_), AccessModeV1::Spectator) => Ok(PackViewerClassV1::Public),
        (PackViewerV1::Participant(_), AccessModeV1::Participant) => {
            Ok(PackViewerClassV1::Participant)
        }
        (PackViewerV1::Operator(_), AccessModeV1::Operator) => Ok(PackViewerClassV1::Operator),
        (PackViewerV1::Historical(_), AccessModeV1::Spectator) => {
            Ok(PackViewerClassV1::HistoricalPublic)
        }
        (PackViewerV1::Historical(_), AccessModeV1::Participant) => {
            Ok(PackViewerClassV1::HistoricalParticipant)
        }
        (PackViewerV1::Historical(_), AccessModeV1::Operator) => {
            Ok(PackViewerClassV1::HistoricalOperator)
        }
        (PackViewerV1::FinalReveal(_), _) => Ok(PackViewerClassV1::FinalReveal),
        _ => Err(PackFaultV1::PrivacyContract(
            "typed viewer disagrees with Membership Access Mode".to_owned(),
        )),
    }
}

fn fixed_seats(core: &crate::CoreRoomStateV1) -> Result<Vec<SeatV1>, PackFaultV1> {
    let mut result = Vec::new();
    for role in ROLES {
        let matches = core
            .memberships()
            .values()
            .filter(|membership| {
                membership.standing() == MembershipStandingV1::Enabled
                    && membership.access_mode() == AccessModeV1::Participant
                    && membership.role() == Some(role)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(PackFaultV1::InvalidOutput(format!(
                "Genesis requires exactly one enabled seat for {role}"
            )));
        }
        result.push(SeatV1 {
            role: role.to_owned(),
            member_id: matches[0].member_id().clone(),
        });
    }
    Ok(result)
}

fn fixed_seats_match_core(state: &StateV1, core: &crate::CoreRoomStateV1) -> bool {
    state.seats.iter().all(|seat| {
        core.membership(&seat.member_id).is_some_and(|membership| {
            membership.access_mode() == AccessModeV1::Participant
                && membership.role() == Some(seat.role.as_str())
        })
    })
}
fn role_for_member<'a>(state: &'a StateV1, member_id: &MemberId) -> Option<&'a str> {
    state
        .seats
        .iter()
        .find(|seat| &seat.member_id == member_id)
        .map(|seat| seat.role.as_str())
}
fn enabled_seat(core: &crate::CoreRoomStateV1, state: &StateV1, role: &str) -> bool {
    state
        .seats
        .iter()
        .find(|seat| seat.role == role)
        .and_then(|seat| core.membership(&seat.member_id))
        .is_some_and(|membership| {
            membership.standing() == MembershipStandingV1::Enabled
                && membership.access_mode() == AccessModeV1::Participant
                && membership.role() == Some(role)
        })
}
fn agent_target(core: &crate::CoreRoomStateV1, state: &StateV1, role: &str) -> bool {
    state
        .seats
        .iter()
        .find(|seat| seat.role == role)
        .and_then(|seat| core.membership(&seat.member_id))
        .is_some_and(|membership| {
            enabled_seat(core, state, role) && membership.principal_kind() == PrincipalKindV1::Agent
        })
}
fn known(clue: &ClueV1, role: &str) -> bool {
    clue.inspected_by.iter().any(|item| item == role)
        || clue.disclosed_to.iter().any(|item| item == role)
}
fn insert_role(values: &mut Vec<String>, role: &str) {
    if !values.iter().any(|item| item == role) {
        values.push(role.to_owned());
        values.sort_by_key(|item| {
            ROLES
                .iter()
                .position(|known| known == item)
                .unwrap_or(usize::MAX)
        });
    }
}
fn fixture_field<'a>(fixture: &'a FixtureV1, clue_id: &str) -> &'a str {
    match clue_id {
        "route" => &fixture.route,
        "entry_window" => &fixture.entry_window,
        "required_tool" => &fixture.required_tool,
        _ => &fixture.extraction,
    }
}
fn valid_plan_values(route: &str, entry_window: &str, tool: &str, extraction: &str) -> bool {
    ["canal", "service", "roof"].contains(&route)
        && ["late", "early", "middle"].contains(&entry_window)
        && ["disguise", "thermal_key", "jammer"].contains(&tool)
        && ["van", "boat", "motorbike"].contains(&extraction)
}
fn initial_clues(fixture: &FixtureRow) -> Vec<ClueV1> {
    [
        ("route", NAVIGATOR, fixture.route),
        ("entry_window", INSIDER, fixture.entry_window),
        ("required_tool", BROKER, fixture.required_tool),
        ("extraction", BROKER, fixture.extraction),
    ]
    .into_iter()
    .map(|(clue_id, owner_role, value)| ClueV1 {
        clue_id: clue_id.to_owned(),
        owner_role: owner_role.to_owned(),
        claim_code: format!("{clue_id}_{value}"),
        inspected_by: Vec::new(),
        disclosed_to: Vec::new(),
        published_claim_code: None,
    })
    .collect()
}

fn event_apply(
    state: &StateV1,
    event_type: &str,
    event: Value,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let event = event_type_with(event, event_type);
    apply(state, vec![canonical_value(&event)?], Vec::new())
}
fn event_apply_with_attention(
    state: &StateV1,
    event_type: &str,
    event: Value,
    attention: Vec<CanonicalJsonV1>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let event = event_type_with(event, event_type);
    apply(state, vec![canonical_value(&event)?], attention)
}
fn event_type_with(mut event: Value, event_type: &str) -> Value {
    if let Value::Object(ref mut object) = event {
        object.insert(
            "event_type".to_owned(),
            Value::String(event_type.to_owned()),
        );
    }
    event
}
fn apply(
    state: &StateV1,
    events: Vec<CanonicalJsonV1>,
    attention: Vec<CanonicalJsonV1>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    apply_with_requests(state, events, Vec::new(), attention)
}
fn apply_with_requests(
    state: &StateV1,
    events: Vec<CanonicalJsonV1>,
    timer_requests: Vec<TimerRequestV1>,
    attention: Vec<CanonicalJsonV1>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    Ok(ActivityDispositionV1::Apply(crate::ActivityApplyV1 {
        next_activity_state: canonical(&state)?,
        ordered_domain_events: events,
        timer_requests,
        ordered_attention_signals: attention,
    }))
}
fn reject(code: &str) -> Result<ActivityDispositionV1, PackFaultV1> {
    Ok(ActivityDispositionV1::Reject(ActivityRejectionV1 {
        declared_code: code.to_owned(),
        bounded_safe_details: canonical(&BTreeMap::<String, String>::new())?,
    }))
}
fn action_id(input: &ActivityReduceInputV1<'_>) -> Result<String, PackFaultV1> {
    match input.recorded_stimulus {
        RecordedStimulusV1::ParticipantAction(action) => Ok(action.action_id.to_string()),
        _ => Err(PackFaultV1::InvalidOutput(
            "Action identity required".to_owned(),
        )),
    }
}
fn attention_signal(state: &StateV1, role: &str, reason: &str) -> CanonicalJsonV1 {
    let action_types = match reason {
        ATTENTION_OFFER_RECEIVED => vec![ACCEPT_EXCHANGE],
        ATTENTION_ENDORSEMENT_REQUESTED => vec![ENDORSE_PLAN, CHALLENGE_PLAN],
        ATTENTION_COMMITMENT_OPENED | ATTENTION_REQUIRED_ACTION_DEADLINE => vec![COMMIT_MOVE],
        ATTENTION_ROUND_RESULT_AVAILABLE => vec![ACKNOWLEDGE_RESULT],
        _ => Vec::new(),
    };
    let signal = serde_json::json!({"target_member_id":state.seats.iter().find(|seat| seat.role == role).map(|seat| seat.member_id.to_string()).unwrap_or_default(),"reason":reason,"priority":1,"deduplication_key":format!("{reason}:{role}:{}",state.phase_generation),"deadline":state.phase_deadline,"action_types":action_types});
    canonical_value(&signal)
        .unwrap_or_else(|_| CanonicalJsonV1::parse(br"{}").unwrap_or_else(|_| unreachable!()))
}

fn commitment_accepted_event(
    role: &str,
    commitment_count: usize,
    phase_after: PhaseV1,
    phase_generation_after: u32,
    phase_deadline_after: Option<&str>,
) -> Value {
    serde_json::json!({
        "event_type": "commitment_accepted",
        "role": role,
        "commitment_count": commitment_count,
        "phase_after": phase_after,
        "phase_generation_after": phase_generation_after,
        "phase_deadline_after": phase_deadline_after,
        "sealed": true,
    })
}

fn attention_reason_precedence(reason: &str) -> u8 {
    match reason {
        ATTENTION_REQUIRED_ACTION_DEADLINE => 0,
        ATTENTION_COMMITMENT_OPENED => 1,
        ATTENTION_OFFER_RECEIVED => 2,
        ATTENTION_ENDORSEMENT_REQUESTED => 3,
        ATTENTION_ROUND_RESULT_AVAILABLE => 4,
        _ => u8::MAX,
    }
}

fn attention_for_candidates<'a>(
    state: &StateV1,
    core: &crate::CoreRoomStateV1,
    candidates: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<CanonicalJsonV1> {
    let mut selected = BTreeMap::<&str, &str>::new();
    for (role, reason) in candidates {
        if !agent_target(core, state, role) {
            continue;
        }
        let replace = selected.get(role).is_none_or(|current| {
            attention_reason_precedence(reason) < attention_reason_precedence(current)
        });
        if replace {
            selected.insert(role, reason);
        }
    }
    ROLES
        .iter()
        .filter_map(|role| {
            selected
                .get(role)
                .map(|reason| attention_signal(state, role, reason))
        })
        .collect()
}
fn cancel_current(requests: &mut Vec<TimerRequestV1>, input: &ActivityReduceInputV1<'_>, id: &str) {
    if let Some(timer) = input.scheduled_timers.get(&timer_id(id)) {
        requests.push(TimerRequestV1::CancelCurrent {
            timer_id: timer.timer_id.clone(),
            expected_generation: timer.generation,
        });
    }
}
fn schedule(id: &str, due: &str, payload: &Value) -> Result<TimerRequestV1, PackFaultV1> {
    Ok(TimerRequestV1::ScheduleNext {
        timer_id: timer_id(id),
        due: due.parse().map_err(timestamp_fault)?,
        canonical_payload: canonical_value(payload)?,
    })
}
fn timer_payload(kind: &str, phase: PhaseV1, generation: u32) -> Value {
    serde_json::json!({"kind":kind,"phase":phase,"phase_generation":generation})
}
fn timer_id(value: &str) -> TimerId {
    value
        .parse()
        .unwrap_or_else(|error| unreachable!("authored Heist timer ID {value}: {error}"))
}
fn canonical<T: Serialize>(value: &T) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}
fn canonical_value(value: &Value) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(&value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}
fn decode<T: for<'de> Deserialize<'de>>(value: &CanonicalJsonV1) -> Result<T, PackFaultV1> {
    serde_json::from_value(serde_json::to_value(value).map_err(json_fault)?).map_err(json_fault)
}
fn required_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, PackFaultV1> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| PackFaultV1::InvalidOutput(format!("payload lacks string {key}")))
}
fn json_fault(error: impl std::fmt::Display) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}

// Canonical JSON owns its allocation, so action parsing uses this owned helper.
fn payload_object(value: &CanonicalJsonV1) -> Result<serde_json::Map<String, Value>, PackFaultV1> {
    match serde_json::to_value(value).map_err(json_fault)? {
        Value::Object(object) => Ok(object),
        _ => Err(PackFaultV1::InvalidOutput(
            "payload must be an object".to_owned(),
        )),
    }
}

fn default_configuration() -> ConfigurationV1 {
    ConfigurationV1 {
        pack_id: AGENT_HEIST_PACK_ID.to_owned(),
        pack_schema: 1,
        roles: ROLES.iter().map(|role| (*role).to_owned()).collect(),
        briefing_duration_seconds: 30,
        negotiation_duration_seconds: 90,
        commitment_duration_seconds: 30,
        commitment_reminder_seconds_before_deadline: 10,
        result_duration_seconds: 20,
        maximum_plans: 12,
        maximum_open_offers_per_role: 4,
    }
}
fn validate_configuration(config: &ConfigurationV1) -> Result<(), PackFaultV1> {
    let expected = default_configuration();
    if config.pack_id != expected.pack_id
        || config.pack_schema != expected.pack_schema
        || config.roles != expected.roles
        || config.briefing_duration_seconds != expected.briefing_duration_seconds
        || config.negotiation_duration_seconds != expected.negotiation_duration_seconds
        || config.commitment_duration_seconds != expected.commitment_duration_seconds
        || config.commitment_reminder_seconds_before_deadline
            != expected.commitment_reminder_seconds_before_deadline
        || config.result_duration_seconds != expected.result_duration_seconds
        || config.maximum_plans != expected.maximum_plans
        || config.maximum_open_offers_per_role != expected.maximum_open_offers_per_role
    {
        return Err(PackFaultV1::InvalidOutput(
            "Agent Heist configuration is not the frozen v0.1 configuration".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Timestamp {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    micros: i64,
}
fn parse_timestamp(value: &str) -> Result<Timestamp, PackFaultV1> {
    let value = value
        .strip_suffix('Z')
        .ok_or_else(|| json_fault("timestamp lacks Z"))?;
    let (date, time) = value
        .split_once('T')
        .ok_or_else(|| json_fault("timestamp lacks T"))?;
    let mut date = date.split('-');
    let year = date
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid year"))?;
    let month = date
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid month"))?;
    let day = date
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid day"))?;
    let (clock, fraction) = time.split_once('.').map_or((time, "0"), |parts| parts);
    let mut clock = clock.split(':');
    let hour = clock
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid hour"))?;
    let minute = clock
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid minute"))?;
    let second = clock
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| json_fault("invalid second"))?;
    let fraction = fraction.chars().take(6).collect::<String>();
    let fraction_len = u32::try_from(fraction.len()).map_err(json_fault)?;
    let micros =
        fraction.parse::<i64>().unwrap_or(0) * 10_i64.pow(6_u32.saturating_sub(fraction_len));
    Ok(Timestamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
        micros,
    })
}
fn format_timestamp(time: Timestamp) -> String {
    if time.micros == 0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            time.year, time.month, time.day, time.hour, time.minute, time.second
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            time.year, time.month, time.day, time.hour, time.minute, time.second, time.micros
        )
    }
}
fn add_seconds(value: &str, seconds: u64) -> Result<String, PackFaultV1> {
    add_micros(value, i128::from(seconds) * 1_000_000)
}
fn subtract_seconds(value: &str, seconds: u64) -> Result<String, PackFaultV1> {
    add_micros(value, -(i128::from(seconds) * 1_000_000))
}
fn successor(value: &str) -> Result<String, PackFaultV1> {
    add_micros(value, 1)
}
fn add_micros(value: &str, delta: i128) -> Result<String, PackFaultV1> {
    let mut time = parse_timestamp(value)?;
    let total = i128::from(time.hour * 3_600 + time.minute * 60 + time.second) * 1_000_000
        + i128::from(time.micros)
        + delta;
    let day_delta = total.div_euclid(86_400_000_000);
    let remainder = total.rem_euclid(86_400_000_000);
    let days = days_from_civil(time.year, time.month, time.day)
        + i64::try_from(day_delta).map_err(json_fault)?;
    let (year, month, day) = civil_from_days(days);
    time.year = year;
    time.month = month;
    time.day = day;
    let seconds = remainder / 1_000_000;
    time.hour = i64::try_from(seconds / 3_600).map_err(json_fault)?;
    time.minute = i64::try_from((seconds % 3_600) / 60).map_err(json_fault)?;
    time.second = i64::try_from(seconds % 60).map_err(json_fault)?;
    time.micros = i64::try_from(remainder % 1_000_000).map_err(json_fault)?;
    Ok(format_timestamp(time))
}

#[cfg(test)]
#[path = "agent_heist_privacy_tests.rs"]
mod agent_heist_privacy_tests;
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }).div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = (if days >= 0 { days } else { days - 146_096 }).div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month + 2) / 5 + 1;
    let month = month + if month < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}
fn timestamp_fault(error: impl std::fmt::Display) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}

struct RevisionV1 {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact_digest: Blake3DigestV1,
}
fn revision() -> &'static RevisionV1 {
    static REVISION: OnceLock<RevisionV1> = OnceLock::new();
    REVISION.get_or_init(build_revision)
}

fn legacy_revision() -> &'static RevisionV1 {
    static REVISION: OnceLock<RevisionV1> = OnceLock::new();
    REVISION.get_or_init(build_legacy_revision)
}
pub(crate) fn agent_heist_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let revision = revision();
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact_digest,
    )
}
pub(crate) fn agent_heist_artifact_digest() -> Blake3DigestV1 {
    revision().artifact_digest.clone()
}

pub(crate) fn agent_heist_legacy_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let revision = legacy_revision();
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact_digest,
    )
}

pub(crate) fn agent_heist_legacy_artifact_digest() -> Blake3DigestV1 {
    legacy_revision().artifact_digest.clone()
}

fn build_legacy_revision() -> RevisionV1 {
    let current = revision();
    let mut descriptor = current.descriptor.clone();
    descriptor
        .explanatory_version
        .clone_from(&AGENT_HEIST_RETAINED_VERSION.to_owned());
    descriptor.revision_digest = PackDigestV1::from_str(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| unreachable!("Heist legacy placeholder digest: {error}"));

    let mut lock = current.lock.clone();
    lock.explanatory_version
        .clone_from(&AGENT_HEIST_RETAINED_VERSION.to_owned());
    lock.descriptor_digest = descriptor
        .content_digest()
        .unwrap_or_else(|error| unreachable!("Heist legacy descriptor digest: {error}"));
    descriptor.revision_digest = lock
        .revision_digest()
        .unwrap_or_else(|error| unreachable!("Heist legacy revision digest: {error}"));

    RevisionV1 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas: current.schemas.clone(),
        codecs: current.codecs.clone(),
        artifact_digest: current.artifact_digest.clone(),
    }
}

#[allow(clippy::too_many_lines)]
fn build_revision() -> RevisionV1 {
    let schema = |id: &str, source: &[u8]| {
        PackSchemaV1::new(
            id,
            CanonicalJsonV1::parse(source)
                .unwrap_or_else(|error| unreachable!("Heist schema JSON: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Heist schema: {error}"))
    };
    let config = schema("agent-heist/configuration/v1", br#"{"additionalProperties":false,"properties":{"briefing_duration_seconds":{"const":30,"type":"integer"},"commitment_duration_seconds":{"const":30,"type":"integer"},"commitment_reminder_seconds_before_deadline":{"const":10,"type":"integer"},"maximum_open_offers_per_role":{"const":4,"type":"integer"},"maximum_plans":{"const":12,"type":"integer"},"negotiation_duration_seconds":{"const":90,"type":"integer"},"pack_id":{"const":"worldstream.agent-heist","type":"string"},"pack_schema":{"const":1,"type":"integer"},"result_duration_seconds":{"const":20,"type":"integer"},"roles":{"items":{"enum":["navigator","insider","broker"],"type":"string"},"maxItems":3,"minItems":3,"type":"array"}},"required":["pack_id","pack_schema","roles","briefing_duration_seconds","negotiation_duration_seconds","commitment_duration_seconds","commitment_reminder_seconds_before_deadline","result_duration_seconds","maximum_plans","maximum_open_offers_per_role"],"type":"object"}"#);
    let state = schema(
        "agent-heist/state/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let empty = schema(
        "agent-heist/empty-action/v1",
        br#"{"additionalProperties":false,"type":"object"}"#,
    );
    let object = schema(
        "agent-heist/action-object/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let event = schema(
        "agent-heist/event/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let timer = schema(
        "agent-heist/timer-request/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let attention = schema(
        "agent-heist/attention/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let projection = schema(
        "agent-heist/projection/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let rejection = schema(
        "agent-heist/rejection/v1",
        br#"{"additionalProperties":true,"type":"object"}"#,
    );
    let action_refs = [
        object.reference(),
        object.reference(),
        object.reference(),
        object.reference(),
        object.reference(),
        object.reference(),
        object.reference(),
        object.reference(),
        empty.reference(),
    ];
    let schemas = PackSchemaBundleV1::new([
        config.clone(),
        state.clone(),
        empty.clone(),
        object.clone(),
        event.clone(),
        timer.clone(),
        attention.clone(),
        projection.clone(),
        rejection.clone(),
    ])
    .unwrap_or_else(|error| unreachable!("Heist schema bundle: {error}"));
    let placeholder = PackDigestV1::from_str(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| unreachable!("Heist placeholder digest: {error}"));
    let classes = [
        PackViewerClassV1::Public,
        PackViewerClassV1::Participant,
        PackViewerClassV1::Operator,
        PackViewerClassV1::HistoricalPublic,
        PackViewerClassV1::HistoricalParticipant,
        PackViewerClassV1::HistoricalOperator,
        PackViewerClassV1::FinalReveal,
    ];
    let mut descriptor = PackRevisionDescriptorV1 {
        pack_id: AGENT_HEIST_PACK_ID.to_owned(),
        name: "Agent Heist".to_owned(),
        explanatory_version: AGENT_HEIST_VERSION.to_owned(),
        revision_digest: placeholder,
        host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        configuration_schema: config.reference(),
        state_schema: state.reference(),
        roles: ROLES
            .iter()
            .map(|role| RoleDefinitionV1 {
                role: (*role).to_owned(),
                minimum: 1,
                maximum: 1,
            })
            .collect(),
        actions: [
            INSPECT_CLUE,
            PUBLISH_CLUE,
            OFFER_EXCHANGE,
            ACCEPT_EXCHANGE,
            PROPOSE_PLAN,
            ENDORSE_PLAN,
            CHALLENGE_PLAN,
            COMMIT_MOVE,
            ACKNOWLEDGE_RESULT,
        ]
        .into_iter()
        .zip(action_refs)
        .map(|(action_type, payload_schema)| ActionDefinitionV1 {
            action_type: action_type.to_owned(),
            payload_schema,
        })
        .collect(),
        rejection_codes: [
            "wrong_phase",
            "unavailable_seat",
            "clue_ownership_or_knowledge",
            "invalid_claim_code",
            "invalid_or_bounded_offer",
            "missing_or_duplicate_plan",
            "unsupported_challenge",
            "prior_commitment",
            "prior_acknowledgement",
            "fixed_genesis_seat",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        attention_reasons: [
            "offer_received",
            "endorsement_requested",
            "commitment_opened",
            "required_action_deadline",
            "round_result_available",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        stimulus_schemas: BTreeMap::from([("timer_fired".to_owned(), timer.reference())]),
        output_schemas: BTreeMap::new(),
        event_schemas: BTreeMap::new(),
        projection_schemas: BTreeMap::new(),
        observation_schemas: BTreeMap::new(),
        limits: PackLimitsV1 {
            maximum_state_bytes: 2_097_152,
            maximum_events: 128,
            maximum_timer_requests: 32,
            maximum_attention_signals: 32,
            maximum_projection_bytes: 262_144,
            maximum_observation_bytes: 262_144,
            maximum_nesting: 32,
            maximum_collection_items: 4096,
            maximum_text_bytes: 65_536,
        },
    };
    for code in &descriptor.rejection_codes {
        descriptor
            .output_schemas
            .insert(format!("rejection:{code}"), rejection.reference());
    }
    descriptor
        .output_schemas
        .insert("timer_request".to_owned(), timer.reference());
    for reason in &descriptor.attention_reasons {
        descriptor
            .output_schemas
            .insert(format!("attention:{reason}"), attention.reference());
    }
    // Every Heist event uses the same bounded object schema.
    descriptor.event_schemas = [
        "clue_inspected",
        "clue_published",
        "exchange_offered",
        "exchange_accepted",
        "plan_proposed",
        "plan_endorsed",
        "plan_challenged",
        "commitment_accepted",
        "commitment_reminder",
        "result_acknowledged",
        "phase_timer_fired",
        "resolution_timer_fired",
    ]
    .into_iter()
    .map(|event| {
        (
            event.to_owned(),
            event_schema_ref(&schemas, "agent-heist/event/v1"),
        )
    })
    .collect();
    for class in classes {
        descriptor
            .projection_schemas
            .insert(class, projection.reference());
        descriptor
            .observation_schemas
            .insert(class, projection.reference());
    }
    let codecs = PackCodecBundleV1::canonical_v1();
    let artifact_digest =
        Blake3DigestV1::hash(&canonical_text_artifact(include_bytes!("agent_heist.rs")));
    let lock = PackRevisionLockV1 {
        revision_lock_id: PACK_REVISION_LOCK_ID.to_owned(),
        pack_id: descriptor.pack_id.clone(),
        explanatory_version: descriptor.explanatory_version.clone(),
        host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        descriptor_digest: descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("Heist descriptor digest: {error}")),
        schema_bundle_digest: schemas
            .digest()
            .unwrap_or_else(|error| unreachable!("Heist schema digest: {error}")),
        codec_bundle_digest: codecs
            .digest()
            .unwrap_or_else(|error| unreachable!("Heist codec digest: {error}")),
        deterministic_static_data_digests: vec![crate::NamedDigestV1 {
            name: "fixture-table".to_owned(),
            digest: Blake3DigestV1::hash(b"agent-heist/fixture-table/v1"),
        }],
        rule_source_digest: artifact_digest.clone(),
        deterministic_dependency_lock_digest: Blake3DigestV1::hash(
            b"agent-heist/rust-dependencies/v1",
        ),
    };
    descriptor.revision_digest = lock
        .revision_digest()
        .unwrap_or_else(|error| unreachable!("Heist revision digest: {error}"));
    RevisionV1 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas,
        codecs,
        artifact_digest,
    }
}

fn event_schema_ref(schemas: &PackSchemaBundleV1, id: &str) -> crate::SchemaReferenceV1 {
    schemas_schema_reference(schemas, id)
}
fn schemas_schema_reference(schemas: &PackSchemaBundleV1, id: &str) -> crate::SchemaReferenceV1 {
    // schema references are reconstructed from the exact bundle bytes
    let schema = if id == "agent-heist/event/v1" {
        PackSchemaV1::new(
            id,
            CanonicalJsonV1::parse(br#"{"additionalProperties":true,"type":"object"}"#)
                .unwrap_or_else(|error| unreachable!("Heist event schema: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Heist event schema: {error}"))
    } else {
        unreachable!("unknown Heist schema {id}")
    };
    let _ = schemas;
    schema.reference()
}
fn canonical_text_artifact(source: &[u8]) -> Vec<u8> {
    source
        .iter()
        .copied()
        .filter(|byte| *byte != b'\r')
        .collect()
}
