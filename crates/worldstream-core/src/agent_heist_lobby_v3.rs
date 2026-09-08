//! Clock-safe Agent Heist 0.3.0 Lobby revision; retained 0.2.0 stays unchanged.

use std::{str::FromStr, sync::OnceLock};

use serde_json::Value;

use crate::{
    AccessModeV1, ActivityApplyV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, Blake3DigestV1, CanonicalJsonV1, CoreRoomStateV1,
    DeterministicContextV1, InitialOutputV1, MemberId, MembershipStandingV1, MembershipV1,
    ObserveInputV1, PackCodecBundleV1, PackDigestV1, PackFaultV1, PackObservationV1,
    PackRegistryV1, PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1, PackViewV1,
    PrincipalId, PrincipalKindV1, RecordedStimulusV1, TimerId, TimerRequestV1, ViewInputV1,
    agent_heist::{BROKER, agent_heist_revision},
    agent_heist_clock_safe::AgentHeistV1,
};

/// Explanatory identity of the selectable Lobby revision.
pub const AGENT_HEIST_LOBBY_VERSION: &str = "0.3.0";
/// Fixed Host Stimulus Source accepted by this Activity revision.
pub const HOST_LOBBY_LAUNCH_SOURCE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH1";
/// Descriptor-declared `ExternalInput` kind that launches a Lobby.
pub const HOST_LAUNCH_INPUT_TYPE: &str = "host_launch";

/// The Lobby revision retains a departed participant's declared Role solely
/// as its Core-departure cardinality witness. Genesis requires Navigator and
/// Insider; Broker is the one optional MVP Role. Active members still obey
/// every declared maximum.
pub(crate) const LOBBY_RETAINS_DEPARTED_ROLE_MINIMA: bool = true;

const PHASE_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH0";
const BRIEFING_DURATION_SECONDS: i64 = 30;

/// Exact Agent Heist revision that waits for an authenticated host launch.
#[derive(Clone, Copy)]
pub struct AgentHeistLobbyV3;

impl ActivityPackV1 for AgentHeistLobbyV3 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        lobby_revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        let mut output = initialize_with_optional_broker(input, cx)?;
        let mut state = json_value(&output.initial_activity_state)?;
        let object = state
            .as_object_mut()
            .ok_or_else(|| PackFaultV1::InvalidOutput("Heist state is not an object".to_owned()))?;
        object.insert("phase".to_owned(), Value::String("lobby".to_owned()));
        object.insert("phase_generation".to_owned(), Value::from(0));
        object.insert(
            "phase_start".to_owned(),
            Value::String(input.created_at.as_str().to_owned()),
        );
        object.insert("phase_deadline".to_owned(), Value::Null);
        output.initial_activity_state = canonical(&state)?;
        output.timer_requests.clear();
        Ok(output)
    }

    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        if agent_heist_lobby_launch_applicable(input.prior_activity_state) {
            return match input.recorded_stimulus {
                RecordedStimulusV1::ExternalInput(external)
                    if external.input_type == HOST_LAUNCH_INPUT_TYPE =>
                {
                    reduce_lobby_launch(input)
                }
                RecordedStimulusV1::CoreProposed(_) => reduce_lobby_core_proposed(input, cx),
                _ => Err(PackFaultV1::InvalidOutput(
                    "Lobby accepts only its host launch or a Core proposal".to_owned(),
                )),
            };
        }
        if matches!(
            input.recorded_stimulus,
            RecordedStimulusV1::ExternalInput(external)
                if external.input_type == HOST_LAUNCH_INPUT_TYPE
        ) {
            return Err(PackFaultV1::InvalidOutput(
                "host launch is only applicable in Lobby".to_owned(),
            ));
        }
        AgentHeistV1.reduce(input, cx)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        if !agent_heist_lobby_launch_applicable(input.activity_state) {
            return AgentHeistV1.view(input);
        }
        let mut state = json_value(input.activity_state)?;
        let object = state
            .as_object_mut()
            .ok_or_else(|| PackFaultV1::InvalidOutput("Heist state is not an object".to_owned()))?;
        object.insert("phase".to_owned(), Value::String("briefing".to_owned()));
        object.insert("phase_generation".to_owned(), Value::from(1));
        object.insert(
            "phase_deadline".to_owned(),
            Value::String(add_seconds(
                object
                    .get("phase_start")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        PackFaultV1::InvalidOutput("Lobby phase_start is absent".to_owned())
                    })?,
                BRIEFING_DURATION_SECONDS,
            )?),
        );
        let briefing_state = canonical(&state)?;
        let mut view = AgentHeistV1.view(&ViewInputV1 {
            activity_state: &briefing_state,
            ..*input
        })?;
        let mut projection = json_value(&view.projection)?;
        if let Some(object) = projection.as_object_mut() {
            object.insert("phase".to_owned(), Value::String("lobby".to_owned()));
            object.insert("phase_generation".to_owned(), Value::from(0));
            object.insert("phase_deadline".to_owned(), Value::Null);
        }
        view.projection = canonical(&projection)?;
        view.action_offers.clear();
        Ok(view)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        if matches!(
            input.recorded_stimulus,
            RecordedStimulusV1::ExternalInput(external)
                if external.input_type == HOST_LAUNCH_INPUT_TYPE
        ) {
            let mut before = json_value(input.activity_before)?;
            let object = before.as_object_mut().ok_or_else(|| {
                PackFaultV1::InvalidOutput("Heist state is not an object".to_owned())
            })?;
            object.insert("phase".to_owned(), Value::String("briefing".to_owned()));
            object.insert("phase_generation".to_owned(), Value::from(1));
            object.insert(
                "phase_deadline".to_owned(),
                Value::String(add_seconds(
                    object
                        .get("phase_start")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            PackFaultV1::InvalidOutput("Lobby phase_start is absent".to_owned())
                        })?,
                    BRIEFING_DURATION_SECONDS,
                )?),
            );
            let briefing_before = canonical(&before)?;
            let observation = AgentHeistV1.observe(&ObserveInputV1 {
                activity_before: &briefing_before,
                ..*input
            })?;
            return Ok(observation.map(|observation| {
                if matches!(input.viewer, crate::PackViewerV1::Participant(_)) {
                    observation.with_action_offers(input.after_view.action_offers().clone())
                } else {
                    observation
                }
            }));
        }
        AgentHeistV1.observe(input)
    }
}

/// The frozen v0.1 executor requires all three seats. The selectable Lobby
/// revision deliberately admits the two-role MVP without changing that
/// retained executor: when Broker is absent, Genesis is evaluated against a
/// private, transient validation witness and the witness seat is removed from
/// the resulting Activity State before it can be persisted or projected.
fn initialize_with_optional_broker(
    input: &ActivityGenesisInputV1<'_>,
    cx: &DeterministicContextV1<'_>,
) -> Result<InitialOutputV1, PackFaultV1> {
    let broker_count = input
        .initial_core_state
        .memberships()
        .values()
        .filter(|membership| {
            membership.standing() == MembershipStandingV1::Enabled
                && membership.access_mode() == AccessModeV1::Participant
                && membership.role() == Some(BROKER)
        })
        .count();
    if broker_count != 0 {
        return AgentHeistV1.initialize(input, cx);
    }

    let broker = transient_broker(input.initial_core_state)?;
    let core = CoreRoomStateV1::active(
        input
            .initial_core_state
            .memberships()
            .values()
            .cloned()
            .chain(std::iter::once(broker)),
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    let mut output = AgentHeistV1.initialize(
        &ActivityGenesisInputV1 {
            initial_core_state: &core,
            ..*input
        },
        cx,
    )?;
    let mut state = json_value(&output.initial_activity_state)?;
    let seats = state
        .get_mut("seats")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| PackFaultV1::InvalidOutput("Heist seats are absent".to_owned()))?;
    seats.retain(|seat| seat.get("role").and_then(Value::as_str) != Some(BROKER));
    output.initial_activity_state = canonical(&state)?;
    Ok(output)
}

fn transient_broker(core: &CoreRoomStateV1) -> Result<MembershipV1, PackFaultV1> {
    // Bounded reserved candidates avoid colliding with hostile-but-valid IDs.
    const CANDIDATES: [(&str, &str); 3] = [
        ("01ARZ3NDEKTSV4RRFFQ69G5FA0", "01ARZ3NDEKTSV4RRFFQ69G5FB0"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FA1", "01ARZ3NDEKTSV4RRFFQ69G5FB1"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FA2", "01ARZ3NDEKTSV4RRFFQ69G5FB2"),
    ];
    for (member, principal) in CANDIDATES {
        let member_id = MemberId::from_str(member)
            .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
        let principal_id = PrincipalId::from_str(principal)
            .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
        if core.membership(&member_id).is_some()
            || core
                .memberships()
                .values()
                .any(|membership| membership.principal_id() == &principal_id)
        {
            continue;
        }
        return MembershipV1::new(
            member_id,
            principal_id,
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some(BROKER.to_owned()),
        )
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()));
    }
    Err(PackFaultV1::InvalidOutput(
        "Lobby Genesis could not allocate its private Broker validation witness".to_owned(),
    ))
}

fn reduce_lobby_core_proposed(
    input: &ActivityReduceInputV1<'_>,
    cx: &DeterministicContextV1<'_>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let mut briefing_state = json_value(input.prior_activity_state)?;
    let object = briefing_state
        .as_object_mut()
        .ok_or_else(|| PackFaultV1::InvalidOutput("Heist state is not an object".to_owned()))?;
    object.insert("phase".to_owned(), Value::String("briefing".to_owned()));
    object.insert("phase_generation".to_owned(), Value::from(1));
    object.insert(
        "phase_deadline".to_owned(),
        Value::String(add_seconds(
            object
                .get("phase_start")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    PackFaultV1::InvalidOutput("Lobby phase_start is absent".to_owned())
                })?,
            BRIEFING_DURATION_SECONDS,
        )?),
    );
    let briefing_state = canonical(&briefing_state)?;
    let disposition = AgentHeistV1.reduce(
        &ActivityReduceInputV1 {
            prior_activity_state: &briefing_state,
            ..input.clone()
        },
        cx,
    )?;
    let ActivityDispositionV1::Apply(mut apply) = disposition else {
        return Ok(disposition);
    };
    let mut next_state = json_value(&apply.next_activity_state)?;
    let object = next_state
        .as_object_mut()
        .ok_or_else(|| PackFaultV1::InvalidOutput("Heist state is not an object".to_owned()))?;
    object.insert("phase".to_owned(), Value::String("lobby".to_owned()));
    object.insert("phase_generation".to_owned(), Value::from(0));
    object.insert("phase_deadline".to_owned(), Value::Null);
    apply.next_activity_state = canonical(&next_state)?;
    apply.timer_requests.clear();
    Ok(ActivityDispositionV1::Apply(apply))
}

/// Returns whether an exact Activity State is waiting in Lobby.
#[must_use]
pub fn agent_heist_lobby_launch_applicable(activity_state: &CanonicalJsonV1) -> bool {
    serde_json::to_value(activity_state)
        .is_ok_and(|state| state.get("phase").and_then(Value::as_str) == Some("lobby"))
}

/// Checks that an exact pinned revision is the reviewed Lobby revision and
/// still declares its fixed host-launch stimulus contract.
#[must_use]
pub fn agent_heist_lobby_contract_declared(
    registry: &PackRegistryV1,
    digest: &PackDigestV1,
) -> bool {
    if digest != &lobby_revision().descriptor.revision_digest {
        return false;
    }
    registry.load_retained(digest).is_ok_and(|retained| {
        retained
            .descriptor()
            .stimulus_schemas
            .contains_key(HOST_LAUNCH_INPUT_TYPE)
    })
}

fn reduce_lobby_launch(
    input: &ActivityReduceInputV1<'_>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let RecordedStimulusV1::ExternalInput(external) = input.recorded_stimulus else {
        return Err(PackFaultV1::InvalidOutput(
            "Lobby accepts only a host launch input".to_owned(),
        ));
    };
    if external.source_id.as_str() != HOST_LOBBY_LAUNCH_SOURCE
        || external.input_type != HOST_LAUNCH_INPUT_TYPE
        || external.canonical_payload != CanonicalJsonV1::parse(br"{}").map_err(json_fault)?
        || !external.immutable_resource_references.is_empty()
    {
        return Err(PackFaultV1::InvalidOutput(
            "Lobby launch input does not match its declared contract".to_owned(),
        ));
    }
    let mut state = json_value(input.prior_activity_state)?;
    let object = state
        .as_object_mut()
        .ok_or_else(|| PackFaultV1::InvalidOutput("Heist state is not an object".to_owned()))?;
    let phase_start = external.recorded_at.as_str().to_owned();
    let phase_deadline = add_seconds(&phase_start, BRIEFING_DURATION_SECONDS)?;
    object.insert("phase".to_owned(), Value::String("briefing".to_owned()));
    object.insert("phase_generation".to_owned(), Value::from(1));
    object.insert("phase_start".to_owned(), Value::String(phase_start));
    object.insert(
        "phase_deadline".to_owned(),
        Value::String(phase_deadline.clone()),
    );
    Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
        next_activity_state: canonical(&state)?,
        ordered_domain_events: vec![canonical(&serde_json::json!({
            "event_type": "lobby_launched"
        }))?],
        timer_requests: vec![TimerRequestV1::ScheduleNext {
            timer_id: TimerId::from_str(PHASE_TIMER).map_err(json_fault)?,
            due: phase_deadline.parse().map_err(json_fault)?,
            canonical_payload: canonical(&serde_json::json!({
                "kind": "phase_deadline",
                "phase": "briefing",
                "phase_generation": 1
            }))?,
        }],
        ordered_attention_signals: Vec::new(),
    }))
}

struct LobbyRevisionV3 {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact_digest: Blake3DigestV1,
}

fn lobby_revision() -> &'static LobbyRevisionV3 {
    static REVISION: OnceLock<LobbyRevisionV3> = OnceLock::new();
    REVISION.get_or_init(build_lobby_revision)
}

pub(crate) fn agent_heist_lobby_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let revision = lobby_revision();
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact_digest,
    )
}

pub(crate) fn agent_heist_lobby_artifact_digest() -> Blake3DigestV1 {
    lobby_revision().artifact_digest.clone()
}

fn build_lobby_revision() -> LobbyRevisionV3 {
    let (prior_descriptor, prior_lock, schemas, codecs, _) = agent_heist_revision();
    let mut descriptor = prior_descriptor.clone();
    AGENT_HEIST_LOBBY_VERSION.clone_into(&mut descriptor.explanatory_version);
    descriptor
        .roles
        .iter_mut()
        .find(|role| role.role == crate::agent_heist::BROKER)
        .unwrap_or_else(|| unreachable!("Agent Heist Broker Role"))
        .minimum = 0;
    let empty_payload = descriptor
        .actions
        .last()
        .unwrap_or_else(|| unreachable!("Heist empty action schema"))
        .payload_schema
        .clone();
    descriptor
        .stimulus_schemas
        .insert(HOST_LAUNCH_INPUT_TYPE.to_owned(), empty_payload);
    let event_schema = descriptor
        .actions
        .first()
        .unwrap_or_else(|| unreachable!("Heist object action schema"))
        .payload_schema
        .clone();
    descriptor
        .event_schemas
        .insert("lobby_launched".to_owned(), event_schema);
    let placeholder = crate::PackDigestV1::from_str(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| unreachable!("Lobby revision placeholder: {error}"));
    descriptor.revision_digest = placeholder;
    let artifact_digest = Blake3DigestV1::hash(
        &include_bytes!("agent_heist_lobby_v3.rs")
            .iter()
            .chain(include_bytes!("agent_heist_clock_safe.rs").iter())
            .copied()
            .filter(|byte| *byte != b'\r')
            .collect::<Vec<_>>(),
    );
    let mut lock = prior_lock.clone();
    AGENT_HEIST_LOBBY_VERSION.clone_into(&mut lock.explanatory_version);
    lock.descriptor_digest = descriptor
        .content_digest()
        .unwrap_or_else(|error| unreachable!("Lobby descriptor digest: {error}"));
    lock.deterministic_static_data_digests
        .push(crate::NamedDigestV1 {
            name: "lobby-launch-contract".to_owned(),
            digest: Blake3DigestV1::hash(b"agent-heist/lobby-launch/v1"),
        });
    lock.rule_source_digest = artifact_digest.clone();
    descriptor.revision_digest = lock
        .revision_digest()
        .unwrap_or_else(|error| unreachable!("Lobby revision digest: {error}"));
    LobbyRevisionV3 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas: schemas.clone(),
        codecs: codecs.clone(),
        artifact_digest,
    }
}

fn add_seconds(value: &str, seconds: i64) -> Result<String, PackFaultV1> {
    crate::agent_heist_clock_safe::add_seconds(value, u64::try_from(seconds).map_err(json_fault)?)
}

fn json_value(value: &CanonicalJsonV1) -> Result<Value, PackFaultV1> {
    serde_json::to_value(value).map_err(json_fault)
}

fn canonical(value: &Value) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(value).map_err(json_fault)
}

fn json_fault(error: impl std::fmt::Display) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}
