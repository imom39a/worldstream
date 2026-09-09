//! Counter v4, the target-qualified attention-bearing Counter revision.
//!
//! This revision delegates the unchanged Counter v2 behavior for genesis and
//! views, while its own reducer adds deterministic Attention after a human
//! private acknowledgement. Its artifact locks both source files because the
//! delegation is part of its executable behavior. Its Attention
//! deduplication keys additionally bind the target Membership so each durable
//! Activation decision has a distinct identity.

use std::{str::FromStr, sync::OnceLock};

use serde::Serialize;

use crate::{
    ACTIVITY_PACK_HOST_CONTRACT_ID, AccessModeV1, ActivityDispositionV1, ActivityGenesisInputV1,
    ActivityPackV1, ActivityReduceInputV1, Blake3DigestV1, CANONICAL_CODEC_ID, CanonicalJsonV1,
    DeterministicContextV1, InitialOutputV1, MembershipStandingV1, ObserveInputV1,
    PACK_REVISION_LOCK_ID, PackCodecBundleV1, PackDigestV1, PackFaultV1, PackLimitsV1,
    PackObservationV1, PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1,
    PackSchemaV1, PackViewV1, PrincipalKindV1, ViewInputV1, counter::CounterV2,
};

const ATTENTION_REASON: &str = "counter_private_acknowledged";
const PRIVATE_ACK: &str = "private_ack";

#[derive(Clone, Copy)]
pub(crate) struct CounterV4;

struct CounterAttentionRevisionV4 {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact_digest: Blake3DigestV1,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CounterAttentionSignalV1 {
    action_types: [&'static str; 2],
    deduplication_key: String,
    priority: u32,
    reason: &'static str,
    target_member_id: String,
}

impl ActivityPackV1 for CounterV4 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        ActivityPackV1::initialize(&CounterV2, input, cx)
    }

    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        let disposition = ActivityPackV1::reduce(&CounterV2, input, cx)?;
        let crate::RecordedStimulusV1::ParticipantAction(action) = input.recorded_stimulus else {
            return Ok(disposition);
        };
        if action.action_type != PRIVATE_ACK || !is_enabled_human_participant(input, action) {
            return Ok(disposition);
        }
        let ActivityDispositionV1::Apply(mut apply) = disposition else {
            return Ok(disposition);
        };
        apply.ordered_attention_signals = input
            .proposed_core_after
            .memberships()
            .iter()
            .filter(|(_, membership)| {
                membership.standing() == MembershipStandingV1::Enabled
                    && membership.access_mode() == AccessModeV1::Participant
                    && membership.principal_kind() == PrincipalKindV1::Agent
            })
            .map(|(member_id, _)| {
                canonical(&CounterAttentionSignalV1 {
                    action_types: ["increment", PRIVATE_ACK],
                    deduplication_key: format!(
                        "{ATTENTION_REASON}:{}:{member_id}",
                        action.action_id
                    ),
                    priority: 1,
                    reason: ATTENTION_REASON,
                    target_member_id: member_id.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ActivityDispositionV1::Apply(apply))
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        ActivityPackV1::view(&CounterV2, input)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        ActivityPackV1::observe(&CounterV2, input)
    }
}

fn is_enabled_human_participant(
    input: &ActivityReduceInputV1<'_>,
    action: &crate::ParticipantActionV1,
) -> bool {
    input
        .core_before
        .membership(&action.member_id)
        .is_some_and(|membership| {
            membership.standing() == MembershipStandingV1::Enabled
                && membership.access_mode() == AccessModeV1::Participant
                && membership.principal_kind() == PrincipalKindV1::Human
        })
}

fn canonical<T: Serialize>(value: &T) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(value)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}

fn revision() -> &'static CounterAttentionRevisionV4 {
    static REVISION: OnceLock<CounterAttentionRevisionV4> = OnceLock::new();
    REVISION.get_or_init(build_revision)
}

pub(crate) fn counter_artifact_digest_v4() -> Blake3DigestV1 {
    revision().artifact_digest.clone()
}

pub(crate) fn counter_v4_revision() -> (
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

#[allow(clippy::too_many_lines)]
fn build_revision() -> CounterAttentionRevisionV4 {
    let schema = |schema_id: &str, source: &[u8]| {
        PackSchemaV1::new(
            schema_id,
            CanonicalJsonV1::parse(source)
                .unwrap_or_else(|error| unreachable!("Counter v4 schema JSON: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("Counter v4 schema: {error}"))
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
    let attention = schema(
        "counter/private-ack-attention/v1",
        br#"{"additionalProperties":false,"properties":{"action_types":{"items":{"enum":["increment","private_ack"],"type":"string"},"maxItems":2,"minItems":2,"type":"array"},"deduplication_key":{"maxLength":128,"minLength":1,"type":"string"},"priority":{"const":1,"type":"integer"},"reason":{"const":"counter_private_acknowledged","type":"string"},"target_member_id":{"type":"string"}},"required":["action_types","deduplication_key","priority","reason","target_member_id"],"type":"object"}"#,
    );
    let attention_ref = attention.reference();
    let schemas = PackSchemaBundleV1::new([
        config,
        state,
        empty,
        event,
        public,
        participant,
        rejection,
        attention,
    ])
    .unwrap_or_else(|error| unreachable!("Counter v4 schema bundle: {error}"));
    let mut descriptor = crate::counter::counter_v2_revision().0.clone();
    descriptor.explanatory_version = "4.0.0".to_owned();
    descriptor.revision_digest = PackDigestV1::from_str(
        "blake3:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| unreachable!("Counter v4 placeholder digest: {error}"));
    descriptor.attention_reasons = vec![ATTENTION_REASON.to_owned()];
    descriptor
        .output_schemas
        .insert(format!("attention:{ATTENTION_REASON}"), attention_ref);
    descriptor.limits = PackLimitsV1 {
        maximum_attention_signals: 8,
        ..descriptor.limits
    };
    let codecs = PackCodecBundleV1::canonical_v1();
    let artifact_digest = artifact_digest();
    let lock = PackRevisionLockV1 {
        revision_lock_id: PACK_REVISION_LOCK_ID.to_owned(),
        pack_id: descriptor.pack_id.clone(),
        explanatory_version: descriptor.explanatory_version.clone(),
        host_contract: ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        descriptor_digest: descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("Counter v4 descriptor digest: {error}")),
        schema_bundle_digest: schemas
            .digest()
            .unwrap_or_else(|error| unreachable!("Counter v4 schemas digest: {error}")),
        codec_bundle_digest: codecs
            .digest()
            .unwrap_or_else(|error| unreachable!("Counter v4 codecs digest: {error}")),
        deterministic_static_data_digests: Vec::new(),
        rule_source_digest: artifact_digest.clone(),
        deterministic_dependency_lock_digest: Blake3DigestV1::hash(include_bytes!(
            "counter-v4-dependency-closure.json"
        )),
    };
    descriptor.revision_digest = lock
        .revision_digest()
        .unwrap_or_else(|error| unreachable!("Counter v4 revision digest: {error}"));
    CounterAttentionRevisionV4 {
        descriptor: Box::leak(Box::new(descriptor)),
        lock,
        schemas,
        codecs,
        artifact_digest,
    }
}

fn artifact_digest() -> Blake3DigestV1 {
    let mut artifact = b"worldstream/counter-attention-executor-source/v4\0".to_vec();
    artifact.extend_from_slice(&canonical_text_artifact(include_bytes!(
        "counter_attention_v4.rs"
    )));
    artifact.push(0);
    artifact.extend_from_slice(&canonical_text_artifact(include_bytes!("counter.rs")));
    Blake3DigestV1::hash(&artifact)
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
