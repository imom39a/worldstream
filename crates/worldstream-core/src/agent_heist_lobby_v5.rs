//! Heist 0.5.0: action schemas that describe executable payloads.
//!
//! Retained revisions remain untouched. This successor preserves the 0.4.0
//! rules and attention behavior while replacing the permissive shared Action
//! object schema with one closed schema per Action type. Generic clients and
//! Runners can therefore reject structurally unusable payloads before reduce.

use crate::{
    ACCEPT_EXCHANGE, ACKNOWLEDGE_RESULT, ActivityDispositionV1, ActivityGenesisInputV1,
    ActivityPackV1, ActivityReduceInputV1, CHALLENGE_PLAN, COMMIT_MOVE, CanonicalJsonV1,
    DeterministicContextV1, ENDORSE_PLAN, INSPECT_CLUE, InitialOutputV1, OFFER_EXCHANGE,
    ObserveInputV1, PROPOSE_PLAN, PUBLISH_CLUE, PackCodecBundleV1, PackDigestV1, PackFaultV1,
    PackObservationV1, PackRevisionDescriptorV1, PackRevisionLockV1, PackSchemaBundleV1,
    PackSchemaV1, PackViewV1, ViewInputV1,
};
use std::{str::FromStr, sync::OnceLock};

pub const AGENT_HEIST_SCHEMA_SAFE_VERSION: &str = "0.5.0";

#[derive(Clone, Copy)]
pub struct AgentHeistLobbyV5;

impl ActivityPackV1 for AgentHeistLobbyV5 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 {
        revision().descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        crate::AgentHeistLobbyV4.initialize(input, context)
    }

    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        crate::AgentHeistLobbyV4.reduce(input, context)
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        let mut view = crate::AgentHeistLobbyV4.view(input)?;
        for offer in &mut view.action_offers {
            offer.payload_schema_digest = revision()
                .descriptor
                .actions
                .iter()
                .find(|definition| definition.action_type == offer.action_type)
                .map(|definition| definition.payload_schema.schema_digest.clone())
                .ok_or_else(|| PackFaultV1::InvalidOutput("missing Heist 0.5 Action".to_owned()))?;
        }
        Ok(view)
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        crate::AgentHeistLobbyV4.observe(input)
    }
}

struct Revision {
    descriptor: &'static PackRevisionDescriptorV1,
    lock: PackRevisionLockV1,
    schemas: PackSchemaBundleV1,
    codecs: PackCodecBundleV1,
    artifact: crate::Blake3DigestV1,
}

fn schema(id: &str, source: &[u8]) -> PackSchemaV1 {
    PackSchemaV1::new(
        id,
        CanonicalJsonV1::parse(source)
            .unwrap_or_else(|error| unreachable!("Heist 0.5 schema JSON: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("Heist 0.5 schema: {error}"))
}

#[allow(clippy::too_many_lines)]
fn action_schemas() -> Vec<(&'static str, PackSchemaV1)> {
    vec![
        (
            INSPECT_CLUE,
            schema(
                "agent-heist/inspect-clue-action/v1",
                br#"{"additionalProperties":false,"properties":{"clue_id":{"description":"One clue identifier currently owned by this role.","enum":["route","entry_window","required_tool","extraction"],"type":"string"}},"required":["clue_id"],"type":"object"}"#,
            ),
        ),
        (
            PUBLISH_CLUE,
            schema(
                "agent-heist/publish-clue-action/v1",
                br#"{"additionalProperties":false,"properties":{"claim_code":{"description":"The exact claim_code from one authorized private_clues entry.","enum":["route_canal","route_service","route_roof","entry_window_late","entry_window_early","entry_window_middle","required_tool_disguise","required_tool_thermal_key","required_tool_jammer","extraction_van","extraction_boat","extraction_motorbike"],"type":"string"},"clue_id":{"description":"The matching clue_id from the same private_clues entry.","enum":["route","entry_window","required_tool","extraction"],"type":"string"}},"required":["clue_id","claim_code"],"type":"object"}"#,
            ),
        ),
        (
            OFFER_EXCHANGE,
            schema(
                "agent-heist/offer-exchange-action/v1",
                br#"{"additionalProperties":false,"properties":{"consideration":{"additionalProperties":false,"description":"Use clue_id when kind is clue_disclosure; use plan_id when kind is plan_endorsement.","properties":{"clue_id":{"maxLength":64,"minLength":1,"type":"string"},"kind":{"enum":["clue_disclosure","plan_endorsement"],"type":"string"},"plan_id":{"maxLength":64,"minLength":1,"type":"string"}},"required":["kind"],"type":"object"},"offered_clue_id":{"enum":["route","entry_window","required_tool","extraction"],"type":"string"},"recipient_role":{"enum":["navigator","insider","broker"],"type":"string"}},"required":["recipient_role","offered_clue_id","consideration"],"type":"object"}"#,
            ),
        ),
        (
            ACCEPT_EXCHANGE,
            schema(
                "agent-heist/accept-exchange-action/v1",
                br#"{"additionalProperties":false,"properties":{"offer_id":{"description":"An exact open exchange offer_id from the authorized projection.","maxLength":64,"minLength":1,"type":"string"}},"required":["offer_id"],"type":"object"}"#,
            ),
        ),
        (
            PROPOSE_PLAN,
            schema(
                "agent-heist/propose-plan-action/v1",
                br#"{"additionalProperties":false,"properties":{"entry_window":{"enum":["late","early","middle"],"type":"string"},"extraction":{"enum":["van","boat","motorbike"],"type":"string"},"required_tool":{"enum":["disguise","thermal_key","jammer"],"type":"string"},"route":{"enum":["canal","service","roof"],"type":"string"}},"required":["route","entry_window","required_tool","extraction"],"type":"object"}"#,
            ),
        ),
        (
            ENDORSE_PLAN,
            schema(
                "agent-heist/endorse-plan-action/v1",
                br#"{"additionalProperties":false,"properties":{"plan_id":{"description":"An exact plan_id from plans in the authorized projection.","maxLength":64,"minLength":1,"type":"string"}},"required":["plan_id"],"type":"object"}"#,
            ),
        ),
        (
            CHALLENGE_PLAN,
            schema(
                "agent-heist/challenge-plan-action/v1",
                br#"{"additionalProperties":false,"properties":{"plan_id":{"description":"An exact plan_id from plans in the authorized projection.","maxLength":64,"minLength":1,"type":"string"},"reason":{"enum":["route_conflict","timing_conflict","tool_conflict","extraction_conflict"],"type":"string"}},"required":["plan_id","reason"],"type":"object"}"#,
            ),
        ),
        (
            COMMIT_MOVE,
            schema(
                "agent-heist/commit-move-action/v1",
                br#"{"additionalProperties":false,"properties":{"contribute_required_resource":{"type":"boolean"},"selected_plan_id":{"description":"An exact plan_id from plans in the authorized projection.","maxLength":64,"minLength":1,"type":"string"}},"required":["selected_plan_id","contribute_required_resource"],"type":"object"}"#,
            ),
        ),
        (
            ACKNOWLEDGE_RESULT,
            schema(
                "agent-heist/acknowledge-result-action/v1",
                br#"{"additionalProperties":false,"properties":{},"type":"object"}"#,
            ),
        ),
    ]
}

fn revision() -> &'static Revision {
    static VALUE: OnceLock<Revision> = OnceLock::new();
    VALUE.get_or_init(|| {
        let (prior, prior_lock, prior_schemas, codecs, _) =
            crate::agent_heist_lobby_v4::agent_heist_agent_ready_revision();
        let additions = action_schemas();
        let schemas = prior_schemas
            .with_additions(additions.iter().map(|(_, schema)| schema.clone()))
            .unwrap_or_else(|error| unreachable!("Heist 0.5 schema bundle: {error}"));
        let mut descriptor = prior.clone();
        AGENT_HEIST_SCHEMA_SAFE_VERSION.clone_into(&mut descriptor.explanatory_version);
        for definition in &mut descriptor.actions {
            definition.payload_schema = additions
                .iter()
                .find(|(action_type, _)| *action_type == definition.action_type)
                .map(|(_, schema)| schema.reference())
                .unwrap_or_else(|| unreachable!("Heist 0.5 Action schema"));
        }
        descriptor.revision_digest = PackDigestV1::from_str(&format!("blake3:{}", "0".repeat(64)))
            .unwrap_or_else(|error| unreachable!("Heist 0.5 fixed digest: {error}"));
        let artifact = crate::Blake3DigestV1::hash(
            &include_bytes!("agent_heist_lobby_v5.rs")
                .iter()
                .chain(include_bytes!("agent_heist_lobby_v4.rs"))
                .chain(include_bytes!("agent_heist_lobby_v3.rs"))
                .chain(include_bytes!("agent_heist_clock_safe.rs"))
                .copied()
                .filter(|byte| *byte != b'\r')
                .collect::<Vec<_>>(),
        );
        let mut lock = prior_lock.clone();
        AGENT_HEIST_SCHEMA_SAFE_VERSION.clone_into(&mut lock.explanatory_version);
        lock.schema_bundle_digest = schemas
            .digest()
            .unwrap_or_else(|error| unreachable!("Heist 0.5 schema digest: {error}"));
        lock.descriptor_digest = descriptor
            .content_digest()
            .unwrap_or_else(|error| unreachable!("Heist 0.5 descriptor: {error}"));
        lock.rule_source_digest = artifact.clone();
        descriptor.revision_digest = lock
            .revision_digest()
            .unwrap_or_else(|error| unreachable!("Heist 0.5 revision: {error}"));
        Revision {
            descriptor: Box::leak(Box::new(descriptor)),
            lock,
            schemas,
            codecs: codecs.clone(),
            artifact,
        }
    })
}

pub(crate) fn agent_heist_schema_safe_revision() -> (
    &'static PackRevisionDescriptorV1,
    &'static PackRevisionLockV1,
    &'static PackSchemaBundleV1,
    &'static PackCodecBundleV1,
    &'static crate::Blake3DigestV1,
) {
    let revision = revision();
    (
        revision.descriptor,
        &revision.lock,
        &revision.schemas,
        &revision.codecs,
        &revision.artifact,
    )
}

pub(crate) fn artifact_digest() -> crate::Blake3DigestV1 {
    revision().artifact.clone()
}

#[must_use]
pub fn agent_heist_schema_safe_digest() -> PackDigestV1 {
    revision().descriptor.revision_digest.clone()
}
