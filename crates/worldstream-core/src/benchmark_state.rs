//! Fixed synthetic benchmark revision. Compiled only for conformance tracing.
//! No provider, storage, or production registry is part of this Pack.
use crate::activity_pack::{CanonicalPackCodecV1, PackRegistryArtifactsV1, PackRegistryEntryV1};
use crate::{
    ACTION_OFFER_DOMAIN, ACTIVITY_PACK_HOST_CONTRACT_ID, AccessModeV1, ActionDefinitionV1,
    ActionOfferV1, ActivityApplyV1, ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, Blake3DigestV1, CANONICAL_CODEC_ID, CanonicalJsonV1, CoreRoomStateV1,
    DeterministicContextV1, InitialOutputV1, MembershipStandingV1, MembershipV1, ObserveInputV1,
    PACK_REVISION_LOCK_ID, PackCodecBundleV1, PackFaultV1, PackGenesisRequestV1,
    PackGoldenActionV1, PackGoldenCorpusV1, PackGoldenViewerKindV1, PackGoldenViewerV1,
    PackLimitsV1, PackObservationV1, PackRegistryErrorV1, PackRegistryStatusV1, PackRegistryV1,
    PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1, PackSchemaV1, PackViewV1,
    PackViewerClassV1, PackViewerV1, PrincipalKindV1, RecordedStimulusV1, RoleDefinitionV1,
    ViewInputV1,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

/// Exact embedded executor for the synthetic state sweep.
pub struct BenchmarkStatePack;

#[allow(clippy::needless_pass_by_value)]
fn value(json: Value) -> Result<CanonicalJsonV1, PackFaultV1> {
    CanonicalJsonV1::from_serialize(&json)
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}
fn decoded(json: &CanonicalJsonV1) -> Result<Value, PackFaultV1> {
    serde_json::from_slice(
        &json
            .to_bytes()
            .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?,
    )
    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))
}

impl ActivityPackV1 for BenchmarkStatePack {
    fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        revision().descriptor
    }
    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        _: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        let config = decoded(input.configuration)?;
        let size = usize::try_from(
            config["state_bytes"]
                .as_u64()
                .ok_or_else(|| PackFaultV1::InvalidOutput("state_bytes".to_owned()))?,
        )
        .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?;
        let maximum = config["maximum_counter"]
            .as_u64()
            .ok_or_else(|| PackFaultV1::InvalidOutput("maximum_counter".to_owned()))?;
        // Reserve all digit growth. The final state fits the target exactly.
        let mut state = json!({"counter":0,"maximum_counter":maximum,"reservoir":[]});
        let mut payload = size
            .checked_sub(
                value(state.clone())?
                    .to_bytes()
                    .map_err(|error| PackFaultV1::InvalidOutput(error.to_string()))?
                    .len()
                    + maximum.to_string().len()
                    - 1,
            )
            .ok_or_else(|| PackFaultV1::InvalidOutput("state target too small".to_owned()))?;
        let chunks = payload.div_ceil(4096);
        // Each JSON string adds two quotes; each item after the first adds a comma.
        payload = payload
            .checked_sub(chunks * 2 + chunks.saturating_sub(1))
            .ok_or_else(|| PackFaultV1::InvalidOutput("reservoir overhead".to_owned()))?;
        let mut reservoir = Vec::new();
        for _ in 0..chunks {
            let len = payload.min(4096);
            reservoir.push("r".repeat(len));
            payload -= len;
        }
        state["reservoir"] = json!(reservoir);
        Ok(InitialOutputV1 {
            initial_activity_state: value(state)?,
            timer_requests: Vec::new(),
        })
    }
    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        _: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        let mut state = decoded(input.prior_activity_state)?;
        let mut events = Vec::new();
        if let RecordedStimulusV1::ParticipantAction(action) = input.recorded_stimulus {
            if action.action_type != "increment" {
                return Err(PackFaultV1::InvalidOutput("unsupported Action".to_owned()));
            }
            let next = state["counter"]
                .as_u64()
                .ok_or_else(|| PackFaultV1::InvalidOutput("counter".to_owned()))?
                + 1;
            if next > state["maximum_counter"].as_u64().unwrap_or(0) {
                return Err(PackFaultV1::InvalidOutput("counter exhausted".to_owned()));
            }
            state["counter"] = json!(next);
            events.push(value(json!({"event_type":"incremented","counter":next}))?);
        }
        Ok(ActivityDispositionV1::Apply(ActivityApplyV1 {
            next_activity_state: value(state)?,
            ordered_domain_events: events,
            timer_requests: Vec::new(),
            ordered_attention_signals: Vec::new(),
        }))
    }
    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        let state = decoded(input.activity_state)?;
        let offers = if matches!(input.viewer, PackViewerV1::Participant(_))
            && state["counter"].as_u64() < state["maximum_counter"].as_u64()
        {
            vec![ActionOfferV1 {
                domain: ACTION_OFFER_DOMAIN.to_owned(),
                action_type: "increment".to_owned(),
                payload_schema_digest: self.descriptor().actions[0]
                    .payload_schema
                    .schema_digest
                    .clone(),
                eligibility_window: None,
            }]
        } else {
            Vec::new()
        };
        Ok(PackViewV1 {
            projection_schema: "benchmark-state/projection/v1".to_owned(),
            projection: value(json!({"counter":state["counter"]}))?,
            action_offers: offers,
        })
    }
    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        let state = decoded(input.activity_after)?;
        let mut observation = PackObservationV1::new(
            "benchmark-state/projection/v1",
            value(json!({"counter":state["counter"]}))?,
        );
        let before = decoded(input.activity_before)?;
        if matches!(input.viewer, PackViewerV1::Participant(_))
            && (before["counter"].as_u64() < before["maximum_counter"].as_u64())
                != (state["counter"].as_u64() < state["maximum_counter"].as_u64())
        {
            observation = observation.with_action_offers(input.after_view.action_offers().clone());
        }
        Ok(Some(observation))
    }
}

struct Revision {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
}
pub(crate) fn artifact_digest() -> Blake3DigestV1 {
    // The exact source before the evidence marker contains executor, schemas,
    // limits, and revision construction. Derived golden literals are excluded
    // to avoid a source/transcript self-hash cycle.
    let source = include_str!("benchmark_state.rs");
    let boundary = source
        .find("\n// GOLDEN_EVIDENCE_BOUNDARY\n")
        .unwrap_or_else(|| unreachable!("source marker"));
    Blake3DigestV1::hash(&source.as_bytes()[..boundary])
}
fn revision() -> &'static Revision {
    static REVISION: OnceLock<Revision> = OnceLock::new();
    REVISION.get_or_init(|| {
        let schema = |name: &str, body: Value| PackSchemaV1::new(name, CanonicalJsonV1::from_serialize(&body).unwrap_or_else(|error| unreachable!("schema JSON: {error}"))).unwrap_or_else(|error| unreachable!("schema: {error}"));
        let integer = json!({"type":"integer","minimum":0,"maximum":100_000});
        let config = schema("benchmark-state/configuration/v1",json!({"type":"object","additionalProperties":false,"properties":{"state_bytes":{"type":"integer","minimum":1024,"maximum":262_144},"maximum_counter":{"type":"integer","minimum":1,"maximum":100_000}},"required":["state_bytes","maximum_counter"]}));
        let state = schema("benchmark-state/state/v1",json!({"type":"object","additionalProperties":false,"properties":{"counter":integer,"maximum_counter":{"type":"integer","minimum":1,"maximum":100_000},"reservoir":{"type":"array","maxItems":65,"items":{"type":"string","maxLength":4096}}},"required":["counter","maximum_counter","reservoir"]}));
        let empty = schema("benchmark-state/empty/v1",json!({"type":"object","additionalProperties":false,"properties":{}}));
        let event = schema("benchmark-state/event/v1",json!({"type":"object","additionalProperties":false,"properties":{"counter":integer,"event_type":{"type":"string","const":"incremented"}},"required":["counter","event_type"]}));
        let projection = schema("benchmark-state/projection/v1",json!({"type":"object","additionalProperties":false,"properties":{"counter":integer},"required":["counter"]}));
        let classes = [PackViewerClassV1::Public,PackViewerClassV1::Participant,PackViewerClassV1::Operator,PackViewerClassV1::HistoricalPublic,PackViewerClassV1::HistoricalParticipant,PackViewerClassV1::HistoricalOperator,PackViewerClassV1::FinalReveal];
        let views: BTreeMap<_,_> = classes.into_iter().map(|class|(class,projection.reference())).collect();
        let mut descriptor = PackRevisionDescriptorV1 {
            pack_id:"worldstream.benchmark-state".to_owned(),name:"Synthetic state benchmark".to_owned(),explanatory_version:"1.0.0".to_owned(),revision_digest:"blake3:0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap_or_else(|error| unreachable!("digest: {error}")),
            host_contract:ACTIVITY_PACK_HOST_CONTRACT_ID.to_owned(),canonical_codec:CANONICAL_CODEC_ID.to_owned(),activity_start_contract:None,
            configuration_schema:config.reference(),state_schema:state.reference(),roles:vec![RoleDefinitionV1 {role:"counter".to_owned(),minimum:1,maximum:1}],
            actions:vec![ActionDefinitionV1 {action_type:"increment".to_owned(),payload_schema:empty.reference()}],rejection_codes:Vec::new(),attention_reasons:Vec::new(),stimulus_schemas:BTreeMap::new(),output_schemas:BTreeMap::new(),event_schemas:BTreeMap::from([("incremented".to_owned(),event.reference())]),projection_schemas:views.clone(),observation_schemas:views,
            limits:PackLimitsV1 {maximum_state_bytes:262_144,maximum_events:1,maximum_timer_requests:1,maximum_attention_signals:1,maximum_projection_bytes:4096,maximum_observation_bytes:4096,maximum_nesting:8,maximum_collection_items:128,maximum_text_bytes:4096},
        };
        let schemas = PackSchemaBundleV1::new([config,state,empty,event,projection]).unwrap_or_else(|error| unreachable!("schema bundle: {error}"));
        let codecs = PackCodecBundleV1::canonical_v1();
        let lock = PackRevisionLockV1 {revision_lock_id:PACK_REVISION_LOCK_ID.to_owned(),pack_id:descriptor.pack_id.clone(),explanatory_version:descriptor.explanatory_version.clone(),host_contract:descriptor.host_contract.clone(),canonical_codec:descriptor.canonical_codec.clone(),descriptor_digest:descriptor.content_digest().unwrap_or_else(|error| unreachable!("descriptor: {error}")),schema_bundle_digest:schemas.digest().unwrap_or_else(|error| unreachable!("schemas: {error}")),codec_bundle_digest:codecs.digest().unwrap_or_else(|error| unreachable!("codecs: {error}")),deterministic_static_data_digests:Vec::new(),rule_source_digest:artifact_digest(),deterministic_dependency_lock_digest:Blake3DigestV1::hash(include_bytes!("../../../Cargo.lock"))};
        descriptor.revision_digest = lock.revision_digest().unwrap_or_else(|error| unreachable!("revision: {error}"));
        Revision {descriptor:Box::leak(Box::new(descriptor)),lock,schemas,codecs}
    })
}

// GOLDEN_EVIDENCE_BOUNDARY
// Bind only this module's root-lock include to its recorded fixture. Keep the
// original executor source prefix and all fixed golden vectors unchanged.
use crate::benchmark_fixture_identity::include_bytes;

// Authored through the checked golden transcript, then frozen independently.
const TRANSCRIPT: &str = "blake3:0db0144c613515aa6cc3b7224c5ffa9d58160fb7555e8dfda5293257ccfc4735";

#[allow(clippy::too_many_lines)]
fn corpus() -> PackGoldenCorpusV1 {
    let descriptor = revision().descriptor;
    let parsed = |source: &str| {
        source
            .parse()
            .unwrap_or_else(|_| unreachable!("fixture identity"))
    };
    let member = MembershipV1::new(
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0"),
        "01ARZ3NDEKTSV4RRFFQ69G5FD0"
            .parse()
            .unwrap_or_else(|_| unreachable!("principal")),
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .unwrap_or_else(|error| unreachable!("membership: {error}"));
    PackGoldenCorpusV1 {
        corpus_id: "worldstream/pack-golden-corpus/v1".to_owned(),
        genesis: PackGenesisRequestV1 {
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV"
                .parse()
                .unwrap_or_else(|_| unreachable!("Room")),
            pack_digest: descriptor.revision_digest.clone(),
            configuration: value(json!({"state_bytes":1024,"maximum_counter":100}))
                .unwrap_or_else(|_| unreachable!("config")),
            room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
                .parse()
                .unwrap_or_else(|_| unreachable!("seed")),
            created_at: "2026-08-15T12:00:00Z"
                .parse()
                .unwrap_or_else(|_| unreachable!("time")),
            initial_core_state: CoreRoomStateV1::active([
                member,
                MembershipV1::new(
                    parsed("01ARZ3NDEKTSV4RRFFQ69G5FC1"),
                    "01ARZ3NDEKTSV4RRFFQ69G5FD1"
                        .parse()
                        .unwrap_or_else(|_| unreachable!("principal")),
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Spectator,
                    None,
                )
                .unwrap_or_else(|_| unreachable!("spectator")),
                MembershipV1::new(
                    parsed("01ARZ3NDEKTSV4RRFFQ69G5FC2"),
                    "01ARZ3NDEKTSV4RRFFQ69G5FD2"
                        .parse()
                        .unwrap_or_else(|_| unreachable!("principal")),
                    PrincipalKindV1::Human,
                    MembershipStandingV1::Enabled,
                    AccessModeV1::Operator,
                    None,
                )
                .unwrap_or_else(|_| unreachable!("operator")),
            ])
            .unwrap_or_else(|_| unreachable!("Core")),
        },
        viewers: [
            (
                PackGoldenViewerKindV1::Participant,
                "01ARZ3NDEKTSV4RRFFQ69G5FC0",
            ),
            (PackGoldenViewerKindV1::Public, "01ARZ3NDEKTSV4RRFFQ69G5FC1"),
            (
                PackGoldenViewerKindV1::Operator,
                "01ARZ3NDEKTSV4RRFFQ69G5FC2",
            ),
            (
                PackGoldenViewerKindV1::Historical,
                "01ARZ3NDEKTSV4RRFFQ69G5FC0",
            ),
            (
                PackGoldenViewerKindV1::Historical,
                "01ARZ3NDEKTSV4RRFFQ69G5FC1",
            ),
            (
                PackGoldenViewerKindV1::Historical,
                "01ARZ3NDEKTSV4RRFFQ69G5FC2",
            ),
            (
                PackGoldenViewerKindV1::FinalReveal,
                "01ARZ3NDEKTSV4RRFFQ69G5FC1",
            ),
        ]
        .into_iter()
        .map(|(kind, id)| PackGoldenViewerV1 {
            kind,
            member_id: parsed(id),
            available_after_action: 0,
            denied_before_detail: None,
        })
        .collect(),
        actions: (1..=2)
            .map(|seq| PackGoldenActionV1 {
                member_id: parsed("01ARZ3NDEKTSV4RRFFQ69G5FC0"),
                action_id: format!("{seq:026X}")
                    .parse()
                    .unwrap_or_else(|_| unreachable!("Action")),
                action_type: "increment".to_owned(),
                payload_schema_digest: descriptor.actions[0].payload_schema.schema_digest.clone(),
                canonical_payload: value(json!({})).unwrap_or_else(|_| unreachable!("payload")),
                admitted_at: "2026-08-15T12:00:01Z"
                    .parse()
                    .unwrap_or_else(|_| unreachable!("time")),
            })
            .collect(),
        external_inputs: Vec::new(),
        expected_transcript_digest: TRANSCRIPT
            .parse()
            .unwrap_or_else(|_| unreachable!("transcript")),
    }
}
/// Builds the one fixed, checked embedded benchmark registry.
///
/// # Errors
/// Rejects disagreement in revision, schemas, codecs, provenance, or golden behavior.
pub fn benchmark_state_registry_for_conformance() -> Result<PackRegistryV1, PackRegistryErrorV1> {
    let revision = revision();
    let corpus = corpus();
    let artifacts = PackRegistryArtifactsV1 {
        expected_revision_digest: revision.descriptor.revision_digest.clone(),
        schemas: Some(revision.schemas.clone()),
        codecs: Some(revision.codecs.clone()),
        codec_implementation: Some(CanonicalPackCodecV1::canonical_v1()),
        executor_artifact_digest: artifact_digest(),
        golden_corpus_digest: corpus.digest()?,
        golden_corpus: Some(corpus),
    };
    PackRegistryV1::try_new([PackRegistryEntryV1::benchmark_state(
        revision.lock.clone(),
        revision.descriptor,
        artifacts,
        PackRegistryStatusV1 {
            selectable_for_new_rooms: true,
            runnable_for_retained_rooms: true,
            approved_for_activity_start: false,
        },
    )])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit authoring only; prints proposed fixed golden evidence"]
    fn author_benchmark_transcript() {
        let r = revision();
        let digest = crate::activity_pack::author_golden_transcript_digest_for_test(
            r.lock.clone(),
            r.descriptor,
            r.schemas.clone(),
            r.codecs.clone(),
            artifact_digest(),
            &corpus(),
            BenchmarkStatePack,
        )
        .unwrap_or_else(|_| unreachable!("author checked corpus"));
        println!("BENCHMARK_TRANSCRIPT={digest}");
    }
}
