//! Deterministic Counter conformance pack revisions.
//!
//! Counter is deliberately test-only compatibility evidence, not a release
//! Activity. Both exact revisions remain compiled and runnable so retained
//! replay can prove that a newer executor is never substituted for an older
//! digest.

use std::{collections::BTreeMap, str::FromStr, sync::OnceLock};

use serde::{Deserialize, Serialize};

use crate::{
    ACTION_OFFER_DOMAIN, ACTIVITY_PACK_HOST_CONTRACT_ID, AccessModeV1, ActionDefinitionV1,
    ActionOfferV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, Blake3DigestV1, CANONICAL_CODEC_ID, CanonicalJsonV1, CoreRoomStateV1,
    DeterministicContextV1, InitialOutputV1, ObserveInputV1, PACK_REVISION_LOCK_ID,
    PackCodecBundleV1, PackDigestV1, PackFaultV1, PackLimitsV1, PackObservationV1,
    PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1, PackSchemaV1, PackViewV1,
    PackViewerClassV1, PackViewerV1, RoleDefinitionV1, ViewInputV1,
};

const COUNTER_PACK_ID: &str = "worldstream.counter";
const INCREMENT: &str = "increment";
const PRIVATE_ACK: &str = "private_ack";
const LIMIT_REACHED: &str = "counter_limit_reached";

#[derive(Clone, Copy)]
pub(crate) struct CounterV1;

#[derive(Clone, Copy)]
pub(crate) struct CounterV2;

#[derive(Clone, Copy)]
enum Revision {
    V1,
    V2,
}

impl Revision {
    const fn version(self) -> &'static str {
        match self {
            Self::V1 => "1.0.0",
            Self::V2 => "2.0.0",
        }
    }

    const fn increment(self) -> u32 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }

    fn artifact_digest(self) -> Blake3DigestV1 {
        // The exact production executor source is the reviewed rule artifact.
        // A revision tag keeps the two concrete constructors independently
        // addressable while any semantic source edit changes both digests.
        let mut artifact = b"worldstream/counter-executor-source/v1\0".to_vec();
        artifact.extend_from_slice(self.version().as_bytes());
        artifact.push(0);
        artifact.extend_from_slice(&canonical_text_artifact(include_bytes!("counter.rs")));
        Blake3DigestV1::hash(&artifact)
    }
}

fn canonical_text_artifact(source: &[u8]) -> Vec<u8> {
    let mut canonical = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'\r' {
            canonical.push(b'\n');
            if source.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else {
            canonical.push(source[index]);
        }
        index += 1;
    }
    canonical
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CounterConfigurationV1 {
    initial_value: u32,
    maximum_value: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CounterStateV1 {
    private_ack_count: u32,
    value: u32,
    maximum_value: u32,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CounterPublicProjectionV1 {
    value: u32,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CounterParticipantProjectionV1 {
    private_ack_count: u32,
    value: u32,
}

struct CounterRevisionV1 {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact_digest: Blake3DigestV1,
}

impl ActivityPackV1 for CounterV1 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        counter_revision(Revision::V1).descriptor
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
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        reduce(Revision::V1, input, cx)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        view(Revision::V1, input)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        observe(Revision::V1, input)
    }
}

impl ActivityPackV1 for CounterV2 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        counter_revision(Revision::V2).descriptor
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
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        reduce(Revision::V2, input, cx)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        view(Revision::V2, input)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        observe(Revision::V2, input)
    }
}

fn initialize(
    input: &ActivityGenesisInputV1<'_>,
    _cx: &DeterministicContextV1<'_>,
) -> Result<InitialOutputV1, PackFaultV1> {
    let configuration: CounterConfigurationV1 = serde_json::from_value(
        serde_json::to_value(input.configuration)
            .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
    if configuration.initial_value > configuration.maximum_value {
        return Err(PackFaultV1::InvalidOutput(
            "initial_value exceeds maximum_value".to_owned(),
        ));
    }
    Ok(InitialOutputV1 {
        initial_activity_state: canonical(&CounterStateV1 {
            private_ack_count: 0,
            value: configuration.initial_value,
            maximum_value: configuration.maximum_value,
        })?,
        timer_requests: Vec::new(),
    })
}

fn reduce(
    revision: Revision,
    input: &ActivityReduceInputV1<'_>,
    _cx: &DeterministicContextV1<'_>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    let mut state = decode_state(input.prior_activity_state)?;
    let crate::RecordedStimulusV1::ParticipantAction(action) = input.recorded_stimulus else {
        return apply(state, Vec::new());
    };
    match action.action_type.as_str() {
        INCREMENT => {
            let delta = revision.increment();
            let Some(next) = state.value.checked_add(delta) else {
                return Err(PackFaultV1::InvalidOutput(
                    "Counter value overflow".to_owned(),
                ));
            };
            if next > state.maximum_value {
                return Ok(ActivityDispositionV1::Reject(crate::ActivityRejectionV1 {
                    declared_code: LIMIT_REACHED.to_owned(),
                    bounded_safe_details: canonical(&BTreeMap::<String, String>::new())?,
                }));
            }
            state.value = next;
            apply(
                state,
                vec![canonical(&CounterIncrementedV1 {
                    delta,
                    event_type: "incremented",
                    value: next,
                })?],
            )
        }
        PRIVATE_ACK => {
            if state.private_ack_count >= 16 {
                return Ok(ActivityDispositionV1::Reject(crate::ActivityRejectionV1 {
                    declared_code: LIMIT_REACHED.to_owned(),
                    bounded_safe_details: canonical(&BTreeMap::<String, String>::new())?,
                }));
            }
            state.private_ack_count += 1;
            apply(state, Vec::new())
        }
        _ => Err(PackFaultV1::InvalidOutput(
            "host admitted an undeclared Counter Action".to_owned(),
        )),
    }
}

#[derive(Serialize)]
struct CounterIncrementedV1 {
    delta: u32,
    event_type: &'static str,
    value: u32,
}

fn apply(
    state: CounterStateV1,
    ordered_domain_events: Vec<CanonicalJsonV1>,
) -> Result<ActivityDispositionV1, PackFaultV1> {
    Ok(ActivityDispositionV1::Apply(crate::ActivityApplyV1 {
        next_activity_state: canonical(&state)?,
        ordered_domain_events,
        timer_requests: Vec::new(),
        ordered_attention_signals: Vec::new(),
    }))
}

fn view(revision: Revision, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
    let state = decode_state(input.activity_state)?;
    if matches!(input.viewer, PackViewerV1::FinalReveal(_)) && state.value < state.maximum_value {
        return Err(PackFaultV1::PrivacyContract(
            "Counter final reveal is unavailable before completion".to_owned(),
        ));
    }
    let class = viewer_class(input.core, input.viewer)?;
    let participant_private = input
        .core
        .membership(input.viewer.member_id())
        .is_some_and(|membership| membership.access_mode() == AccessModeV1::Participant)
        && !matches!(input.viewer, PackViewerV1::FinalReveal(_));
    let projection = projection(state, participant_private)?;
    let descriptor = counter_revision(revision).descriptor;
    let action_offers = if matches!(input.viewer, PackViewerV1::Participant(_)) {
        let mut offers = Vec::new();
        if state
            .value
            .checked_add(revision.increment())
            .is_some_and(|next| next <= state.maximum_value)
        {
            offers.push(offer(&descriptor.actions[0]));
        }
        if state.private_ack_count < 16 {
            offers.push(offer(&descriptor.actions[1]));
        }
        offers
    } else {
        Vec::new()
    };
    Ok(PackViewV1 {
        projection_schema: descriptor.projection_schemas[&class].schema_id.clone(),
        projection,
        action_offers,
    })
}

fn observe(
    revision: Revision,
    input: &ObserveInputV1<'_>,
) -> Result<Option<PackObservationV1>, PackFaultV1> {
    let before = decode_state(input.activity_before)?;
    let after = decode_state(input.activity_after)?;
    let participant_private = input
        .core_after
        .membership(input.viewer.member_id())
        .is_some_and(|membership| membership.access_mode() == AccessModeV1::Participant)
        && !matches!(input.viewer, PackViewerV1::FinalReveal(_));
    let before_projection = projection(before, participant_private)?;
    let after_projection = projection(after, participant_private)?;
    let authorized_core_changed = input.core_before.room_status() != input.core_after.room_status()
        || input
            .core_before
            .membership(input.viewer.member_id())
            .zip(input.core_after.membership(input.viewer.member_id()))
            .is_some_and(|(before_membership, after_membership)| {
                before_membership.standing() != after_membership.standing()
                    || before_membership.access_mode() != after_membership.access_mode()
                    || before_membership.role() != after_membership.role()
            });
    if before_projection == after_projection && !authorized_core_changed {
        return Ok(None);
    }
    let class = viewer_class(input.core_after, input.viewer)?;
    let descriptor = counter_revision(revision).descriptor;
    let mut observation = PackObservationV1::new(
        descriptor.observation_schemas[&class].schema_id.clone(),
        after_projection,
    );
    let increment_was_offered = before
        .value
        .checked_add(revision.increment())
        .is_some_and(|next| next <= before.maximum_value);
    let increment_is_offered = after
        .value
        .checked_add(revision.increment())
        .is_some_and(|next| next <= after.maximum_value);
    let private_ack_was_offered = before.private_ack_count < 16;
    let private_ack_is_offered = after.private_ack_count < 16;
    if matches!(input.viewer, PackViewerV1::Participant(_))
        && (increment_was_offered != increment_is_offered
            || private_ack_was_offered != private_ack_is_offered)
    {
        observation = observation.with_action_offers(input.after_view.action_offers().clone());
    }
    Ok(Some(observation))
}

fn projection(
    state: CounterStateV1,
    participant_private: bool,
) -> Result<CanonicalJsonV1, PackFaultV1> {
    if participant_private {
        canonical(&CounterParticipantProjectionV1 {
            private_ack_count: state.private_ack_count,
            value: state.value,
        })
    } else {
        canonical(&CounterPublicProjectionV1 { value: state.value })
    }
}

fn offer(definition: &ActionDefinitionV1) -> ActionOfferV1 {
    ActionOfferV1 {
        domain: ACTION_OFFER_DOMAIN.to_owned(),
        action_type: definition.action_type.clone(),
        payload_schema_digest: definition.payload_schema.schema_digest.clone(),
        eligibility_window: None,
    }
}

fn viewer_class(
    core: &CoreRoomStateV1,
    viewer: &PackViewerV1,
) -> Result<PackViewerClassV1, PackFaultV1> {
    let membership = core.membership(viewer.member_id()).ok_or_else(|| {
        PackFaultV1::PrivacyContract("Counter viewer Membership is absent".to_owned())
    })?;
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
            "Counter typed viewer disagrees with Access Mode".to_owned(),
        )),
    }
}

fn decode_state(value: &CanonicalJsonV1) -> Result<CounterStateV1, PackFaultV1> {
    serde_json::from_value(
        serde_json::to_value(value)
            .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}

fn canonical<T: Serialize>(value: &T) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}

fn counter_revision(revision: Revision) -> &'static CounterRevisionV1 {
    static V1: OnceLock<CounterRevisionV1> = OnceLock::new();
    static V2: OnceLock<CounterRevisionV1> = OnceLock::new();
    match revision {
        Revision::V1 => V1.get_or_init(|| build_revision(Revision::V1)),
        Revision::V2 => V2.get_or_init(|| build_revision(Revision::V2)),
    }
}

pub(crate) fn counter_artifact_digest_v1() -> Blake3DigestV1 {
    Revision::V1.artifact_digest()
}

pub(crate) fn counter_artifact_digest_v2() -> Blake3DigestV1 {
    Revision::V2.artifact_digest()
}

#[allow(clippy::too_many_lines)]
fn build_revision(revision: Revision) -> CounterRevisionV1 {
    let schema = |schema_id: &str, source: &[u8]| {
        PackSchemaV1::new(
            schema_id,
            CanonicalJsonV1::parse(source)
                .unwrap_or_else(|error| unreachable!("Counter schema JSON: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Counter schema: {error}"))
    };
    let config = schema(
        "counter/configuration/v1",
        br#"{"additionalProperties":false,"properties":{"initial_value":{"maximum":16,"minimum":0,"type":"integer"},"maximum_value":{"maximum":16,"minimum":1,"type":"integer"}},"required":["initial_value","maximum_value"],"type":"object"}"#,
    );
    let state = schema(
        "counter/state/v1",
        br#"{"additionalProperties":false,"properties":{"maximum_value":{"maximum":16,"minimum":1,"type":"integer"},"private_ack_count":{"maximum":16,"minimum":0,"type":"integer"},"value":{"maximum":16,"minimum":0,"type":"integer"}},"required":["maximum_value","private_ack_count","value"],"type":"object"}"#,
    );
    let empty = schema(
        "counter/empty/v1",
        br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
    );
    let event = schema(
        "counter/incremented-event/v1",
        br#"{"additionalProperties":false,"properties":{"delta":{"maximum":2,"minimum":1,"type":"integer"},"event_type":{"const":"incremented","type":"string"},"value":{"maximum":16,"minimum":0,"type":"integer"}},"required":["delta","event_type","value"],"type":"object"}"#,
    );
    let public = schema(
        "counter/public-projection/v1",
        br#"{"additionalProperties":false,"properties":{"value":{"maximum":16,"minimum":0,"type":"integer"}},"required":["value"],"type":"object"}"#,
    );
    let participant = schema(
        "counter/participant-projection/v1",
        br#"{"additionalProperties":false,"properties":{"private_ack_count":{"maximum":16,"minimum":0,"type":"integer"},"value":{"maximum":16,"minimum":0,"type":"integer"}},"required":["private_ack_count","value"],"type":"object"}"#,
    );
    let rejection = schema(
        "counter/rejection/v1",
        br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
    );
    let config_ref = config.reference();
    let state_ref = state.reference();
    let empty_ref = empty.reference();
    let event_ref = event.reference();
    let public_ref = public.reference();
    let participant_ref = participant.reference();
    let rejection_ref = rejection.reference();
    let schemas =
        PackSchemaBundleV1::new([config, state, empty, event, public, participant, rejection])
            .unwrap_or_else(|error| unreachable!("Counter schema bundle: {error}"));
    let placeholder = PackDigestV1::from_str(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| unreachable!("Counter placeholder digest: {error}"));
    let mut descriptor = PackRevisionDescriptorV1 {
        pack_id: COUNTER_PACK_ID.to_owned(),
        name: "Counter Conformance".to_owned(),
        explanatory_version: revision.version().to_owned(),
        revision_digest: placeholder,
        host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        configuration_schema: config_ref,
        state_schema: state_ref,
        roles: vec![RoleDefinitionV1 {
            role: "counter".to_owned(),
            minimum: 0,
            maximum: 8,
        }],
        actions: vec![
            ActionDefinitionV1 {
                action_type: INCREMENT.to_owned(),
                payload_schema: empty_ref.clone(),
            },
            ActionDefinitionV1 {
                action_type: PRIVATE_ACK.to_owned(),
                payload_schema: empty_ref,
            },
        ],
        rejection_codes: vec![LIMIT_REACHED.to_owned()],
        attention_reasons: Vec::new(),
        stimulus_schemas: BTreeMap::new(),
        output_schemas: BTreeMap::from([(format!("rejection:{LIMIT_REACHED}"), rejection_ref)]),
        event_schemas: BTreeMap::from([("incremented".to_owned(), event_ref)]),
        projection_schemas: BTreeMap::from([
            (PackViewerClassV1::Public, public_ref.clone()),
            (PackViewerClassV1::Participant, participant_ref.clone()),
            (PackViewerClassV1::Operator, public_ref.clone()),
            (PackViewerClassV1::HistoricalPublic, public_ref.clone()),
            (
                PackViewerClassV1::HistoricalParticipant,
                participant_ref.clone(),
            ),
            (PackViewerClassV1::HistoricalOperator, public_ref.clone()),
            (PackViewerClassV1::FinalReveal, public_ref.clone()),
        ]),
        observation_schemas: BTreeMap::from([
            (PackViewerClassV1::Public, public_ref.clone()),
            (PackViewerClassV1::Participant, participant_ref.clone()),
            (PackViewerClassV1::Operator, public_ref.clone()),
            (PackViewerClassV1::HistoricalPublic, public_ref.clone()),
            (PackViewerClassV1::HistoricalParticipant, participant_ref),
            (PackViewerClassV1::HistoricalOperator, public_ref.clone()),
            (PackViewerClassV1::FinalReveal, public_ref),
        ]),
        limits: PackLimitsV1 {
            maximum_state_bytes: 256,
            maximum_events: 1,
            maximum_timer_requests: 1,
            maximum_attention_signals: 1,
            maximum_projection_bytes: 2_048,
            maximum_observation_bytes: 2_048,
            maximum_nesting: 8,
            maximum_collection_items: 16,
            maximum_text_bytes: 128,
        },
    };
    let codecs = PackCodecBundleV1::canonical_v1();
    let artifact_digest = revision.artifact_digest();
    let lock = PackRevisionLockV1 {
        revision_lock_id: PACK_REVISION_LOCK_ID.to_owned(),
        pack_id: descriptor.pack_id.clone(),
        explanatory_version: descriptor.explanatory_version.clone(),
        host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        descriptor_digest: descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("Counter descriptor digest: {error}")),
        schema_bundle_digest: schemas
            .digest()
            .unwrap_or_else(|error| unreachable!("Counter schemas digest: {error}")),
        codec_bundle_digest: codecs
            .digest()
            .unwrap_or_else(|error| unreachable!("Counter codecs digest: {error}")),
        deterministic_static_data_digests: Vec::new(),
        rule_source_digest: artifact_digest.clone(),
        deterministic_dependency_lock_digest: Blake3DigestV1::hash(include_bytes!(
            "counter-dependency-closure.json"
        )),
    };
    descriptor.revision_digest = lock
        .revision_digest()
        .unwrap_or_else(|error| unreachable!("Counter revision digest: {error}"));
    CounterRevisionV1 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas,
        codecs,
        artifact_digest,
    }
}

pub(crate) fn counter_v1_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let revision = counter_revision(Revision::V1);
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact_digest,
    )
}

pub(crate) fn counter_v2_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static Blake3DigestV1,
) {
    let revision = counter_revision(Revision::V2);
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact_digest,
    )
}
