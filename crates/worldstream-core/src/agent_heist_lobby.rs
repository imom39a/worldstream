//! Agent Heist revision with an Activity-defined Lobby launch seam.

use std::{str::FromStr, sync::OnceLock};

use serde_json::Value;

use crate::{
    ActivityApplyV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, AgentHeistV1, Blake3DigestV1, CanonicalJsonV1, DeterministicContextV1,
    InitialOutputV1, ObserveInputV1, PackCodecBundleV1, PackDigestV1, PackFaultV1,
    PackObservationV1, PackRegistryV1, PackRevisionDescriptorV1, PackRevisionLockV1,
    PackSchemaBundleV1, PackViewV1, RecordedStimulusV1, TimerId, TimerRequestV1, ViewInputV1,
    agent_heist::agent_heist_revision,
};

/// Explanatory identity of the selectable Lobby revision.
pub const AGENT_HEIST_LOBBY_VERSION: &str = "0.2.0";
/// Fixed Host Stimulus Source accepted by this Activity revision.
pub const HOST_LOBBY_LAUNCH_SOURCE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH1";
/// Descriptor-declared `ExternalInput` kind that launches a Lobby.
pub const HOST_LAUNCH_INPUT_TYPE: &str = "host_launch";
/// Exact catalog contract exposed only by the Lobby revision.
pub const AGENT_HEIST_LOBBY_CONTRACT: &str = "worldstream/agent-heist-lobby/v1";

/// The Lobby revision retains a departed participant's declared Role solely
/// as its mandatory Core-departure minimum-cardinality witness. Genesis still
/// requires every active Role and active members still obey maximums.
pub(crate) const LOBBY_RETAINS_DEPARTED_ROLE_MINIMA: bool = true;

const PHASE_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH0";
const BRIEFING_DURATION_SECONDS: i64 = 30;

/// Exact Agent Heist revision that waits for an authenticated host launch.
#[derive(Clone, Copy)]
pub struct AgentHeistLobbyV2;

impl ActivityPackV1 for AgentHeistLobbyV2 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        lobby_revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        let mut output = AgentHeistV1.initialize(input, cx)?;
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

struct LobbyRevisionV2 {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact_digest: Blake3DigestV1,
}

fn lobby_revision() -> &'static LobbyRevisionV2 {
    static REVISION: OnceLock<LobbyRevisionV2> = OnceLock::new();
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

fn build_lobby_revision() -> LobbyRevisionV2 {
    let (prior_descriptor, prior_lock, schemas, codecs, _) = agent_heist_revision();
    let mut descriptor = prior_descriptor.clone();
    AGENT_HEIST_LOBBY_VERSION.clone_into(&mut descriptor.explanatory_version);
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
        &include_bytes!("agent_heist_lobby.rs")
            .iter()
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
    LobbyRevisionV2 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas: schemas.clone(),
        codecs: codecs.clone(),
        artifact_digest,
    }
}

fn add_seconds(value: &str, seconds: i64) -> Result<String, PackFaultV1> {
    let mut timestamp = parse_timestamp(value)?;
    let total = timestamp.hour * 3_600 + timestamp.minute * 60 + timestamp.second + seconds;
    let days =
        days_from_civil(timestamp.year, timestamp.month, timestamp.day) + total.div_euclid(86_400);
    let remainder = total.rem_euclid(86_400);
    (timestamp.year, timestamp.month, timestamp.day) = civil_from_days(days);
    timestamp.hour = remainder / 3_600;
    timestamp.minute = (remainder % 3_600) / 60;
    timestamp.second = remainder % 60;
    Ok(format_timestamp(&timestamp))
}

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
    let (date, clock) = value
        .strip_suffix('Z')
        .and_then(|value| value.split_once('T'))
        .ok_or_else(|| json_fault("timestamp must be UTC"))?;
    let mut date = date.split('-');
    let (clock, fraction) = clock.split_once('.').map_or((clock, "0"), |parts| parts);
    let mut clock = clock.split(':');
    let next = |parts: &mut std::str::Split<'_, char>| {
        parts
            .next()
            .and_then(|part| part.parse::<i64>().ok())
            .ok_or_else(|| json_fault("timestamp component is invalid"))
    };
    let fraction = fraction.chars().take(6).collect::<String>();
    let fraction_len = u32::try_from(fraction.len()).map_err(json_fault)?;
    let micros = fraction.parse::<i64>().map_err(json_fault)?
        * 10_i64.pow(6_u32.saturating_sub(fraction_len));
    Ok(Timestamp {
        year: next(&mut date)?,
        month: next(&mut date)?,
        day: next(&mut date)?,
        hour: next(&mut clock)?,
        minute: next(&mut clock)?,
        second: next(&mut clock)?,
        micros,
    })
}

fn format_timestamp(timestamp: &Timestamp) -> String {
    if timestamp.micros == 0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            timestamp.year,
            timestamp.month,
            timestamp.day,
            timestamp.hour,
            timestamp.minute,
            timestamp.second,
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            timestamp.year,
            timestamp.month,
            timestamp.day,
            timestamp.hour,
            timestamp.minute,
            timestamp.second,
            timestamp.micros,
        )
    }
}

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

fn json_value(value: &CanonicalJsonV1) -> Result<Value, PackFaultV1> {
    serde_json::to_value(value).map_err(json_fault)
}

fn canonical(value: &Value) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(value).map_err(json_fault)
}

fn json_fault(error: impl std::fmt::Display) -> PackFaultV1 {
    PackFaultV1::InvalidOutput(error.to_string())
}
