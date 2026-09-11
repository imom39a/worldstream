//! Reviewed Agent Heist policies consume one current authorized projection.
//!
//! Heist observations are complete replacements. Retaining every replacement
//! in a stateless provider request grows the request without adding facts and
//! can exhaust the fixed input allowance before the 30-second Commitment
//! window opens. This selector is deliberately pinned to the reviewed Heist
//! policies and schemas; generic packs may use delta observations and must
//! retain their full history.

use serde_json::{Map, Value, json};

use super::HouseModelErrorV1;

const PACK_ID: &str = "worldstream.agent-heist";
const PACK_VERSION: &str = "0.5.0";
const PACK_DIGEST: &str = "blake3:56449d0830d1137d69b1b7c11ed25e8f0d9b7188d40e8290c58e5a2caff2bef9";
const PROJECTION_SCHEMA: &str = "agent-heist/projection/v1";
const ASSIGNMENT_OBSERVATION_SCHEMA: &str = "worldstream/assignment-observation/v1";
const POLICY_REVISION: &str = "3";
const REVIEWED_POLICIES: &[&str] = &[
    "worldstream.house.cooperative-planner",
    "worldstream.house.skeptical-auditor",
];
const ROLES: &[&str] = &["navigator", "insider", "broker"];
const ACTIVITY_FIELDS: &[&str] = &[
    "phase",
    "phase_generation",
    "phase_start",
    "phase_deadline",
    "seats",
    "public_claims",
    "plans",
    "endorsements",
    "challenges",
    "commitment_count",
    "outcome",
    "private_clues",
    "own_commitment",
    "addressed_offers",
];

pub(super) fn current_context(
    policy_id: &str,
    policy_revision: &str,
    observation: &Value,
) -> Result<Option<Value>, HouseModelErrorV1> {
    if !REVIEWED_POLICIES.contains(&policy_id) || policy_revision != POLICY_REVISION {
        return Ok(None);
    }
    // The same retained policy revision served earlier Heist Pack revisions.
    // Only claim the exact Pack whose complete-replacement shape was reviewed;
    // every other Pack keeps the generic history-preserving request path.
    if observation["pack"]["id"] != PACK_ID
        || observation["pack"]["version"] != PACK_VERSION
        || observation["pack"]["digest"] != PACK_DIGEST
    {
        return Ok(None);
    }
    let role = observation["role"]
        .as_str()
        .filter(|role| ROLES.contains(role))
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    if observation["schema"] != ASSIGNMENT_OBSERVATION_SCHEMA
        || observation["head"]["pack_digest"] != PACK_DIGEST
    {
        return Err(HouseModelErrorV1::InvalidInput);
    }
    let frame_head = observation["stream"]["frame_head"]
        .as_u64()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let cursor = match &observation["stream"]["cursor"] {
        Value::Null => Value::Null,
        value => Value::from(
            value
                .as_u64()
                .filter(|cursor| *cursor <= frame_head)
                .ok_or(HouseModelErrorV1::InvalidInput)?,
        ),
    };
    let retained_floor = observation["stream"]["retained_floor"]
        .as_u64()
        .filter(|floor| *floor <= frame_head)
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let frames = observation["observations"]
        .as_array()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let (projection_reset, observations, next_action) = if let Some(latest) = frames.last() {
        if latest["frame_seq"].as_u64() != Some(frame_head)
            || latest["observation_schema"] != PROJECTION_SCHEMA
        {
            return Err(HouseModelErrorV1::InvalidInput);
        }
        let activity = reviewed_activity(&latest["observation"])?;
        (
            Value::Null,
            json!([{
                "frame_seq": frame_head,
                "observation_schema": PROJECTION_SCHEMA,
                "observation": activity,
            }]),
            "process_then_acknowledge",
        )
    } else {
        let reset = &observation["projection_reset"];
        if reset["baseline_frame_head"].as_u64() != Some(frame_head)
            || reset["projection_schema"] != PROJECTION_SCHEMA
        {
            return Err(HouseModelErrorV1::InvalidInput);
        }
        let activity = reviewed_activity(&reset["projection"]["activity"])?;
        (
            json!({
                "baseline_frame_head": frame_head,
                "projection_schema": PROJECTION_SCHEMA,
                "projection": {"activity": activity},
            }),
            Value::Array(Vec::new()),
            "wait_or_observe_again",
        )
    };

    // Rebuild the Assignment observation with its reviewed keys. The newest
    // full Activity replacement retains all facts authorized to this role;
    // older replacements and an older Reset are deliberately excluded.
    Ok(Some(json!({
        "schema": ASSIGNMENT_OBSERVATION_SCHEMA,
        "role": role,
        "pack": {"id": PACK_ID, "version": PACK_VERSION, "digest": PACK_DIGEST},
        "stream": {
            "cursor": cursor,
            "frame_head": frame_head,
            "retained_floor": retained_floor,
        },
        "projection_reset": projection_reset,
        "observations": observations,
        "next_action": next_action,
    })))
}

fn reviewed_activity(value: &Value) -> Result<Value, HouseModelErrorV1> {
    let source = value.as_object().ok_or(HouseModelErrorV1::InvalidInput)?;
    let mut activity = Map::new();
    for &field in ACTIVITY_FIELDS {
        activity.insert(
            field.to_owned(),
            source
                .get(field)
                .ok_or(HouseModelErrorV1::InvalidInput)?
                .clone(),
        );
    }
    Ok(Value::Object(activity))
}

#[cfg(test)]
mod tests {
    use std::{error::Error, sync::Arc};

    use serde_json::{Value, json};
    use tempfile::tempdir;
    use worldstream_core::CanonicalJsonV1;
    use worldstream_hosted_contract::HouseAgentRevision;
    use zeroize::Zeroizing;

    use super::super::{
        DeterministicHouseProviderFaultV1, DeterministicHouseProviderPortV1,
        FileHouseAllowanceLedgerV1, HouseAllowanceErrorV1, HouseAllowancePeriodV1,
        HouseInvocationIdentityV1, HouseModelErrorV1, HouseModelExecutorV1, HouseProposedActionV1,
        HouseProviderCredentialV1, HouseSpendLimitsV1, build_provider_request,
        encode_provider_request, exact_offer_schemas,
    };
    use super::*;

    const COOPERATIVE_REVISION: &[u8] =
        include_bytes!("../../../../config/hosted/house-agents/cooperative-planner-17.json");
    const SKEPTICAL_REVISION: &[u8] =
        include_bytes!("../../../../config/hosted/house-agents/skeptical-auditor-16.json");
    const RETAINED_COOPERATIVE_REVISION: &[u8] =
        include_bytes!("../../../../config/hosted/house-agents/cooperative-planner-8.json");
    const RETAINED_SKEPTICAL_REVISION: &[u8] =
        include_bytes!("../../../../config/hosted/house-agents/skeptical-auditor-7.json");
    const RETAINED_PACK_LISTING: &[u8] =
        include_bytes!("../../../../config/hosted/listings/agent-heist-0.13.0.json");

    fn digest(character: char) -> String {
        format!("blake3:{}", character.to_string().repeat(64))
    }

    fn activity(role: &str) -> Value {
        let private_clues = match role {
            "insider" => json!([{
                "clue_id":"entry_window", "known":true, "owner_role":"insider",
                "claim_code":"entry_window_late"
            }]),
            "broker" => json!([
                {"clue_id":"required_tool", "known":true, "owner_role":"broker",
                 "claim_code":"required_tool_disguise"},
                {"clue_id":"extraction", "known":true, "owner_role":"broker",
                 "claim_code":"extraction_van"}
            ]),
            _ => json!([{
                "clue_id":"route", "known":true, "owner_role":"navigator",
                "claim_code":"route_canal"
            }]),
        };
        json!({
            "phase":"commitment", "phase_generation":3,
            "phase_start":"2026-09-11T10:04:08Z",
            "phase_deadline":"2026-09-11T10:04:38Z",
            "seats":[
                {"role":"navigator","present":true},
                {"role":"insider","present":true},
                {"role":"broker","present":true}
            ],
            "public_claims":[
                {"clue_id":"route","claim_code":"route_canal"},
                {"clue_id":"entry_window","claim_code":"entry_window_late"},
                {"clue_id":"required_tool","claim_code":"required_tool_disguise"},
                {"clue_id":"extraction","claim_code":"extraction_van"}
            ],
            "plans":[{
                "plan_id":"plan-journey", "proposer_role":"navigator", "created_room_seq":17,
                "route":"canal", "entry_window":"late", "required_tool":"disguise",
                "extraction":"van"
            }],
            "endorsements":{"insider":"plan-journey"}, "challenges":[],
            "commitment_count":1, "outcome":null, "private_clues":private_clues,
            "own_commitment":null, "addressed_offers":[]
        })
    }

    fn activity_with_unreviewed_fields(role: &str) -> Value {
        let mut result = activity(role);
        for field in ["fixture", "clues", "commitments", "exchanges", "unknown"] {
            result[field] = json!({"hidden":"must-not-reach-provider"});
        }
        result
    }

    fn maximum_activity(role: &str) -> Value {
        let mut result = activity(role);
        let plan_ids = (0..12)
            .map(|index| format!("plan-{index:02}-{}", "p".repeat(48)))
            .collect::<Vec<_>>();
        result["plans"] = Value::Array(
            plan_ids
                .iter()
                .enumerate()
                .map(|(index, plan_id)| {
                    json!({
                        "plan_id":plan_id,
                        "proposer_role":ROLES[index % ROLES.len()],
                        "created_room_seq":index + 1,
                        "route":"service", "entry_window":"middle",
                        "required_tool":"thermal_key", "extraction":"motorbike"
                    })
                })
                .collect(),
        );
        result["endorsements"] = json!({
            "navigator":plan_ids[0], "insider":plan_ids[1], "broker":plan_ids[2]
        });
        result["challenges"] = Value::Array(
            plan_ids
                .iter()
                .flat_map(|plan_id| {
                    [
                        ("navigator", "route_conflict"),
                        ("insider", "timing_conflict"),
                        ("broker", "tool_conflict"),
                        ("broker", "extraction_conflict"),
                    ]
                    .map(|(role, reason)| {
                        json!({
                            "role":role, "plan_id":plan_id, "reason":reason
                        })
                    })
                })
                .collect(),
        );
        result["addressed_offers"] = Value::Array(
            (0..8)
                .map(|index| {
                    json!({
                        "offer_id":format!("exchange-{index}-{}", "e".repeat(40)),
                        "sender_role":if index % 2 == 0 { "navigator" } else { "broker" },
                        "recipient_role":role, "offered_clue_id":"route",
                        "consideration_kind":"plan_endorsement",
                        "consideration_id":plan_ids[index % plan_ids.len()], "status":"open"
                    })
                })
                .collect(),
        );
        result
    }

    fn observation(role: &str, retained_frames: u64) -> Value {
        observation_with_activity(role, retained_frames, &activity(role))
    }

    fn observation_with_activity(
        role: &str,
        retained_frames: u64,
        current_activity: &Value,
    ) -> Value {
        let frame_head = 20;
        let first = frame_head - retained_frames + 1;
        let frames = (first..=frame_head)
            .map(|frame_seq| {
                json!({
                    "frame_seq":frame_seq, "cause_room_seq":frame_seq,
                    "frame_kind":"transition", "observation_schema":PROJECTION_SCHEMA,
                    "observation":current_activity, "frame_payload_hash":digest('d')
                })
            })
            .collect::<Vec<_>>();
        json!({
            "schema":ASSIGNMENT_OBSERVATION_SCHEMA, "role":role,
            "head":{
                "room_seq":20, "genesis_or_transition_hash":digest('e'),
                "core_schema_version":"worldstream/core-room-state/v1",
                "pack_digest":PACK_DIGEST, "core_state_hash":digest('1'),
                "activity_state_hash":digest('2'), "authoritative_state_hash":digest('3')
            },
            "pack":{"id":PACK_ID,"version":PACK_VERSION,"digest":PACK_DIGEST},
            "stream":{"cursor":first - 1,"frame_head":frame_head,"retained_floor":0},
            "action_offers":[{
                "domain":"worldstream/action-offer/v1", "action_type":"commit_move",
                "payload_schema_digest":digest('b'),
                "eligibility_window":{"start_room_seq":19,"end_room_seq":99}
            }],
            "projection_reset":{
                "baseline_frame_head":frame_head, "projection_schema":PROJECTION_SCHEMA,
                "projection":{"activity":current_activity}
            },
            "observations":frames, "next_action":"process_then_acknowledge"
        })
    }

    fn offers() -> Value {
        json!({
            "schema":"worldstream/assignment-action-offer-list/v1",
            "precondition":{"room_seq":20,"head_hash":digest('c')},
            "offers":[{
                "offer_id":"20:0:commit", "action_type":"commit_move",
                "eligibility_window":{"start_room_seq":19,"end_room_seq":99},
                "payload_schema":{
                    "schema_id":"agent-heist/commit-move-action/v1",
                    "schema_digest":digest('b'),
                    "schema":{
                        "additionalProperties":false,
                        "properties":{
                            "contribute_required_resource":{"type":"boolean"},
                            "selected_plan_id":{"maxLength":64,"minLength":1,"type":"string"}
                        },
                        "required":["selected_plan_id","contribute_required_resource"],
                        "type":"object"
                    }
                }
            }]
        })
    }

    fn revision(source: &[u8]) -> Result<HouseAgentRevision, Box<dyn Error>> {
        let canonical = CanonicalJsonV1::parse(source)?.to_bytes()?;
        Ok(HouseAgentRevision::from_canonical_bytes(&canonical)?)
    }

    fn unreviewed_policy_revision(source: &[u8]) -> Result<HouseAgentRevision, Box<dyn Error>> {
        let mut document: Value = serde_json::from_slice(source)?;
        document["behavior_policy"]["policy_id"] = json!("worldstream.house.fixture");
        let canonical = CanonicalJsonV1::parse(&serde_json::to_vec(&document)?)?.to_bytes()?;
        Ok(HouseAgentRevision::from_canonical_bytes(&canonical)?)
    }

    fn credential() -> Result<HouseProviderCredentialV1, Box<dyn Error>> {
        Ok(HouseProviderCredentialV1::new(Zeroizing::new(
            b"sk-test-01234567890123456789012345678901".to_vec(),
        ))?)
    }

    fn identity(assignment: &str) -> Result<HouseInvocationIdentityV1, Box<dyn Error>> {
        Ok(HouseInvocationIdentityV1::new(
            assignment,
            &format!("activation-{assignment}"),
            1,
            &digest('a'),
        )?)
    }

    #[test]
    fn reviewed_policies_keep_only_the_latest_complete_heist_projection() {
        for policy in REVIEWED_POLICIES {
            for role in ROLES {
                let input =
                    observation_with_activity(role, 8, &activity_with_unreviewed_fields(role));
                let context = current_context(policy, POLICY_REVISION, &input)
                    .expect("valid reviewed Heist context")
                    .expect("reviewed policy context");
                assert_eq!(context["role"], *role);
                assert_eq!(context["stream"]["cursor"], 12);
                assert_eq!(context["stream"]["frame_head"], 20);
                assert_eq!(context["observations"].as_array().map(Vec::len), Some(1));
                assert_eq!(context["observations"][0]["frame_seq"], 20);
                assert_eq!(context["projection_reset"], Value::Null);
                assert!(context.get("head").is_none());
                assert!(context.get("action_offers").is_none());
                assert!(!context.to_string().contains("must-not-reach-provider"));
                assert_eq!(
                    context["observations"][0]["observation"]
                        .as_object()
                        .map(Map::len),
                    Some(ACTIVITY_FIELDS.len()),
                );
            }
        }
    }

    #[test]
    fn reviewed_policy_fails_closed_for_an_unproven_current_heist_projection() {
        let base = observation("insider", 8);
        for pointer in [
            "/schema",
            "/role",
            "/head/pack_digest",
            "/stream/cursor",
            "/stream/retained_floor",
            "/observations/7/observation_schema",
            "/observations/7/frame_seq",
            "/observations/7/observation",
        ] {
            let mut invalid = base.clone();
            *invalid.pointer_mut(pointer).expect("fixture pointer") = json!("invalid");
            assert_eq!(
                current_context(
                    "worldstream.house.cooperative-planner",
                    POLICY_REVISION,
                    &invalid,
                )
                .err(),
                Some(HouseModelErrorV1::InvalidInput),
                "{pointer}",
            );
        }
        for field in ACTIVITY_FIELDS {
            let mut invalid = base.clone();
            invalid["observations"][7]["observation"]
                .as_object_mut()
                .expect("activity fixture")
                .remove(*field);
            assert_eq!(
                current_context(
                    "worldstream.house.cooperative-planner",
                    POLICY_REVISION,
                    &invalid,
                )
                .err(),
                Some(HouseModelErrorV1::InvalidInput),
                "missing {field}",
            );
        }
        for pointer in ["/pack/id", "/pack/version", "/pack/digest"] {
            let mut outside_reviewed_pack = base.clone();
            *outside_reviewed_pack
                .pointer_mut(pointer)
                .expect("fixture pointer") = json!("retained-pack-value");
            assert!(
                current_context(
                    "worldstream.house.cooperative-planner",
                    POLICY_REVISION,
                    &outside_reviewed_pack,
                )
                .expect("another Pack revision remains generic")
                .is_none(),
                "{pointer}",
            );
        }
        assert!(
            current_context("worldstream.house.cooperative-planner", "2", &base)
                .expect("retained policy revision remains generic")
                .is_none()
        );
        assert!(
            current_context("worldstream.house.other", "3", &json!({}))
                .expect("unrelated policy remains generic")
                .is_none()
        );
    }

    #[test]
    fn reset_only_context_is_current_and_uses_the_same_activity_allowlist() {
        let mut reset_only =
            observation_with_activity("broker", 1, &activity_with_unreviewed_fields("broker"));
        reset_only["observations"] = json!([]);
        let context = current_context(
            "worldstream.house.skeptical-auditor",
            POLICY_REVISION,
            &reset_only,
        )
        .expect("valid reset context")
        .expect("reviewed policy context");
        assert_eq!(context["observations"], json!([]));
        assert_eq!(context["projection_reset"]["baseline_frame_head"], 20);
        assert_eq!(
            context["projection_reset"]["projection"]["activity"]
                .as_object()
                .map(Map::len),
            Some(ACTIVITY_FIELDS.len()),
        );
        assert!(!context.to_string().contains("must-not-reach-provider"));
    }

    #[test]
    fn retained_policy_three_pack_three_keeps_generic_provider_request_bytes()
    -> Result<(), Box<dyn Error>> {
        let listing: Value = serde_json::from_slice(RETAINED_PACK_LISTING)?;
        assert_eq!(listing["pack"]["id"], PACK_ID);
        assert_eq!(listing["pack"]["version"], "0.3.0");
        assert_eq!(
            listing["pack"]["digest"],
            "blake3:4e4c970403f29a8448a1a3bcf7a96c030df713499730288f324c7e200d160b2d"
        );

        for (source, role) in [
            (RETAINED_COOPERATIVE_REVISION, "insider"),
            (RETAINED_SKEPTICAL_REVISION, "broker"),
        ] {
            let retained_revision = revision(source)?;
            let (policy_id, policy_revision, _) = retained_revision.behavior_policy();
            assert_eq!(policy_revision, POLICY_REVISION);
            assert!(
                listing["seats"][0]["allowed_house_agent_revisions"]
                    .as_array()
                    .expect("retained listing House revision allowlist")
                    .iter()
                    .any(|digest| digest.as_str() == Some(retained_revision.digest())),
                "fixture must be a House revision retained by this Pack 0.3 listing",
            );

            let mut retained_projection = observation(role, 8);
            retained_projection["pack"] = listing["pack"].clone();
            retained_projection["head"]["pack_digest"] = listing["pack"]["digest"].clone();
            assert!(
                current_context(policy_id, policy_revision, &retained_projection)?.is_none(),
                "retained Pack must not enter the Pack 0.5 selector",
            );

            let action_offers = offers();
            let (generic, _) = encode_provider_request(
                &retained_revision,
                &retained_projection,
                &action_offers,
                exact_offer_schemas(&action_offers)?,
            )?;
            let (selected, _) =
                build_provider_request(&retained_revision, &retained_projection, &action_offers)?;
            assert_eq!(
                selected.body(),
                generic.body(),
                "retained Pack request bytes must remain on the generic path",
            );
        }
        Ok(())
    }

    #[test]
    fn retained_commitment_history_dispatches_within_the_existing_allowance()
    -> Result<(), Box<dyn Error>> {
        for (source, role, assignment) in [
            (COOPERATIVE_REVISION, "insider", "commitment-insider"),
            (SKEPTICAL_REVISION, "broker", "commitment-broker"),
        ] {
            let projection = observation(role, 8);
            let offers = offers();
            let reviewed = revision(source)?;
            let unreviewed = unreviewed_policy_revision(source)?;
            let (before, _) = encode_provider_request(
                &unreviewed,
                &projection,
                &offers,
                exact_offer_schemas(&offers)?,
            )?;
            let (after, _) = build_provider_request(&reviewed, &projection, &offers)?;
            let maximum_projection = observation_with_activity(role, 1, &maximum_activity(role));
            let (maximum, _) = build_provider_request(&reviewed, &maximum_projection, &offers)?;
            assert!(
                before.body().len() as u64 > reviewed.allowance().input_tokens_per_call,
                "retained journey input must reproduce the pre-provider input limit"
            );
            assert!(
                after.body().len() as u64 <= reviewed.allowance().input_tokens_per_call,
                "latest authoritative projection must fit the unchanged allowance"
            );
            assert!(
                maximum.body().len() as u64 > reviewed.allowance().input_tokens_per_call,
                "the stress-populated single projection must preserve the hard input boundary"
            );

            let directory = tempdir()?;
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                directory.path().join("ledger"),
                HouseSpendLimitsV1::hobby_preview(),
                HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?,
            )?;
            let provider = Arc::new(DeterministicHouseProviderPortV1::new(
                HouseProposedActionV1 {
                    offer_id: "20:0:commit".to_owned(),
                    payload: json!({
                        "selected_plan_id":"plan-journey",
                        "contribute_required_resource":true
                    }),
                },
                DeterministicHouseProviderFaultV1::None,
            ));
            let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
            let raw_assignment = format!("{assignment}-raw");
            let before_result = executor.execute(
                &credential()?,
                &unreviewed,
                &identity(&raw_assignment)?,
                &projection,
                &offers,
                HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?,
            );
            assert_eq!(before_result.err(), Some(HouseModelErrorV1::InputLimit));
            assert_eq!(provider.call_count(), 0);
            assert_eq!(
                ledger.usage(&raw_assignment).err(),
                Some(HouseAllowanceErrorV1::InvalidInput),
            );
            let stress_assignment = format!("{assignment}-stress");
            let stress_result = executor.execute(
                &credential()?,
                &reviewed,
                &identity(&stress_assignment)?,
                &maximum_projection,
                &offers,
                HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?,
            );
            assert_eq!(stress_result.err(), Some(HouseModelErrorV1::InputLimit));
            assert_eq!(provider.call_count(), 0);
            assert_eq!(
                ledger.usage(&stress_assignment).err(),
                Some(HouseAllowanceErrorV1::InvalidInput),
            );
            let completion = executor.execute(
                &credential()?,
                &reviewed,
                &identity(assignment)?,
                &projection,
                &offers,
                HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?,
            )?;
            assert_eq!(completion.action.offer_id, "20:0:commit");
            assert_eq!(provider.call_count(), 1);
            assert_eq!(
                ledger.usage(assignment)?.consumed_input_units,
                after.body().len() as u64,
            );
            eprintln!(
                "{}",
                json!({
                    "schema":"worldstream/house-commitment-regression-diagnostic/v1",
                    "assignment":assignment,
                    "before":{
                        "turn_status":"failed", "failure_code":"input_limit",
                        "input_bytes":before.body().len(), "cursor":12,
                        "frame_head":20, "provider_call_count":0
                    },
                    "after":{
                        "turn_status":"completed", "failure_code":null,
                        "input_bytes":after.body().len(), "cursor":12,
                        "frame_head":20, "provider_call_count":provider.call_count()
                    },
                    "stress_single_projection":{
                        "turn_status":"failed", "failure_code":"input_limit",
                        "input_bytes":maximum.body().len(), "cursor":19,
                        "frame_head":20, "provider_call_count":0
                    }
                })
            );
        }
        Ok(())
    }
}
