//! Reviewed Archive policies use current authorized facts, never stream history.

use serde_json::{Map, Value, json};

use super::HouseModelErrorV1;

const PACK_ID: &str = "worldstream.midnight-archive";
const PROJECTION_SCHEMA: &str = "worldstream.midnight-archive/participant-projection/v4";
const OBSERVATION_SCHEMA: &str = "worldstream.midnight-archive/participant-observation/v4";
const PROJECTION_SCHEMA_V5: &str = "worldstream.midnight-archive/participant-projection/v5";
const OBSERVATION_SCHEMA_V5: &str = "worldstream.midnight-archive/participant-observation/v5";

// Deliberately closed: future projection fields require
// review before they become model input. The Pack remains the visibility owner.
// v5's session deadline remains Pack timer metadata; companion plans use the
// same reviewed facts and current planning opportunity as retained v4.
const ACTIVITY_FIELDS: &[&str] = &[
    "phase",
    "scenario",
    "objective",
    "location",
    "turns_used",
    "turns_remaining",
    "power",
    "initial_power",
    "method_costs",
    "operation_costs",
    "gates",
    "map",
    "candidates",
    "staged_action",
    "carried_candidate",
    "verifier_result",
    "preservation_agreement",
    "optional_objectives",
    "turn_resolution",
    "extraction",
    "companion_dialogue",
];

pub(super) fn current_context(
    policy_id: &str,
    policy_revision: &str,
    observation: &Value,
) -> Result<Option<Value>, HouseModelErrorV1> {
    let role = match policy_id {
        "worldstream.house.mira" => "mira",
        "worldstream.house.jonah" => "jonah",
        _ => return Ok(None),
    };
    if policy_revision != "1"
        || observation["schema"] != "worldstream/assignment-observation/v1"
        || observation["role"] != role
        || observation["pack"]["id"] != PACK_ID
    {
        return Err(HouseModelErrorV1::InvalidInput);
    }
    let frame_head = observation["stream"]["frame_head"]
        .as_u64()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let frames = observation["observations"]
        .as_array()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let activity = if let Some(latest) = frames.last() {
        // The assignment helper independently checks stream ordering and hashes.
        // Never fall back to an older reset when the latest frame is unusable.
        if latest["frame_seq"].as_u64() != Some(frame_head)
            || !matches!(
                latest["observation_schema"].as_str(),
                Some(OBSERVATION_SCHEMA | OBSERVATION_SCHEMA_V5)
            )
        {
            return Err(HouseModelErrorV1::InvalidInput);
        }
        &latest["observation"]
    } else {
        let reset = &observation["projection_reset"];
        if reset["baseline_frame_head"].as_u64() != Some(frame_head)
            || !matches!(
                reset["projection_schema"].as_str(),
                Some(PROJECTION_SCHEMA | PROJECTION_SCHEMA_V5)
            )
        {
            return Err(HouseModelErrorV1::InvalidInput);
        }
        &reset["projection"]["activity"]
    };
    let activity = activity
        .as_object()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    let companion = activity
        .get(role)
        .and_then(Value::as_object)
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    // Plans are requested only by the Pack's explicit planning opportunity.
    // This selector does not invent tasks, reopen blocked work, or call on ticks.
    if activity.get("phase").and_then(Value::as_str) != Some("active")
        || companion.get("presence").and_then(Value::as_str) != Some("active")
        || companion.get("mode").and_then(Value::as_str) != Some("tasked")
        || companion
            .get("planning")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            != Some("waiting")
        || companion
            .get("task")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            != Some("assigned")
    {
        return Err(HouseModelErrorV1::InvalidInput);
    }
    let mut facts = Map::new();
    for &field in ACTIVITY_FIELDS {
        let value = activity.get(field).ok_or(HouseModelErrorV1::InvalidInput)?;
        facts.insert(field.to_owned(), value.clone());
    }
    facts.insert(role.to_owned(), Value::Object(companion.clone()));
    Ok(Some(json!({
        "schema": "worldstream/archive-companion-context/v1",
        "role": role,
        "activity": facts,
    })))
}

pub(super) fn validate_proposal(context: &Value, payload: &Value) -> Result<(), HouseModelErrorV1> {
    let invalid = HouseModelErrorV1::InvalidResponse;
    let text = payload["dialogue"].as_str().ok_or(invalid)?;
    let encoded = serde_json::to_string(text).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let double_encoded =
        serde_json::to_string(&encoded).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let role = context["role"]
        .as_str()
        .ok_or(HouseModelErrorV1::InvalidResponse)?;
    let companion = &context["activity"][role];
    if text.len() > 160
        || text.chars().any(|ch| ch < '\u{20}')
        || double_encoded.len() > 192
        || (!text.is_empty() && companion["dialogue_allowed"] != true)
        || payload["task_revision"] != companion["task"]["revision"]
        || payload["opportunity_revision"] != companion["planning"]["opportunity_revision"]
    {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, sync::Arc};

    use tempfile::tempdir;
    use worldstream_core::CanonicalJsonV1;
    use worldstream_hosted_contract::HouseAgentRevision;
    use zeroize::Zeroizing;

    use super::super::{
        DeterministicHouseProviderFaultV1, DeterministicHouseProviderPortV1,
        FileHouseAllowanceLedgerV1, HouseAllowancePeriodV1, HouseInvocationIdentityV1,
        HouseModelExecutorV1, HouseProposedActionV1, HouseProviderCredentialV1, HouseSpendLimitsV1,
        build_provider_request,
    };
    use super::*;

    fn revision(role: &str) -> Result<HouseAgentRevision, Box<dyn Error>> {
        let document: Value = serde_json::from_slice(if role == "mira" {
            include_bytes!("../../../../tests/fixtures/hosted/house-agents/mira-1.json").as_slice()
        } else {
            include_bytes!("../../../../tests/fixtures/hosted/house-agents/jonah-1.json").as_slice()
        })?;
        let canonical = CanonicalJsonV1::parse(&serde_json::to_vec(&document)?)?.to_bytes()?;
        Ok(HouseAgentRevision::from_canonical_bytes(&canonical)?)
    }

    fn offers() -> Value {
        offers_from_fixture(&complete_fixture())
    }

    fn offers_from_fixture(fixture: &Value) -> Value {
        json!({"schema":"worldstream/assignment-action-offer-list/v1",
            "precondition":{"room_seq":65535,"head_hash":format!("blake3:{}", "b".repeat(64))},
            "offers":[{
            "offer_id":format!("65535:0:{}", fixture["payload_schema"]["schema_digest"].as_str().expect("schema digest")),
            "action_type":"submit_companion_plan",
            "eligibility_window":fixture["eligibility_window"],
            "payload_schema": fixture["payload_schema"]
        }]})
    }

    fn complete_fixture() -> Value {
        serde_json::from_slice(include_bytes!("fixtures/archive-v4-model-context.json"))
            .expect("retained actual Pack projection fixture")
    }

    fn activity(role: &str) -> Value {
        let mut fields: Map<String, Value> = ACTIVITY_FIELDS
            .iter()
            .map(|field| ((*field).to_owned(), Value::Null))
            .collect();
        fields.insert("phase".to_owned(), json!("active"));
        fields.insert(
            role.to_owned(),
            json!({"presence":"active", "mode":"tasked", "task":{"revision":2,"status":"assigned"},
            "planning":{"status":"waiting"}, "knowledge":{"records":"unknown"}}),
        );
        fields.insert(
            "companion_dialogue".to_owned(),
            json!([{ "speaker":role,"turn":1,"text":"Accepted advice" }]),
        );
        fields.insert("unreviewed_future_field".to_owned(), json!("omit"));
        Value::Object(fields)
    }

    fn observation(role: &str) -> Value {
        json!({"schema":"worldstream/assignment-observation/v1", "role":role,
            "pack":{"id":PACK_ID}, "stream":{"frame_head":2}, "observations":[],
            "projection_reset":{"baseline_frame_head":2, "projection_schema":PROJECTION_SCHEMA,
                "projection":{"activity":activity(role)}}})
    }

    #[test]
    fn both_companions_receive_current_facts_without_history_or_unreviewed_fields() {
        for role in ["mira", "jonah"] {
            let mut input = observation(role);
            input["observations"] = json!([
                {"frame_seq":1,"observation":{"old_secret":"history only"}},
                {"frame_seq":2,"observation_schema":OBSERVATION_SCHEMA,"observation":activity(role)}]);
            input["observations"][1]["observation"]["power"] = json!(1);
            let result = current_context(&format!("worldstream.house.{role}"), "1", &input)
                .expect("valid current context")
                .expect("Archive context");
            assert_eq!(result["activity"]["power"], 1);
            assert_eq!(result["activity"][role]["task"]["revision"], 2);
            assert_eq!(
                result["activity"]["companion_dialogue"][0]["text"],
                "Accepted advice"
            );
            assert!(result["activity"].get("unreviewed_future_field").is_none());
            assert!(!result.to_string().contains("history only"));
            assert!(result.get("projection_reset").is_none());
        }
    }

    #[test]
    fn current_reset_is_accepted_but_stale_or_unusable_latest_frame_never_falls_back() {
        for (projection_schema, observation_schema) in [
            (PROJECTION_SCHEMA, OBSERVATION_SCHEMA),
            (PROJECTION_SCHEMA_V5, OBSERVATION_SCHEMA_V5),
        ] {
            let mut input = observation("mira");
            input["projection_reset"]["projection_schema"] = json!(projection_schema);
            assert!(current_context("worldstream.house.mira", "1", &input).is_ok());
            input["projection_reset"]["baseline_frame_head"] = json!(1);
            assert!(current_context("worldstream.house.mira", "1", &input).is_err());
            input["projection_reset"]["baseline_frame_head"] = json!(2);
            input["observations"] = json!([{"frame_seq":1,"observation_schema":observation_schema,
                "observation":activity("mira")}]);
            assert!(current_context("worldstream.house.mira", "1", &input).is_err());
            input["observations"][0]["frame_seq"] = json!(2);
            for unsupported in [
                projection_schema,
                "worldstream.midnight-archive/participant-observation/v6",
            ] {
                input["observations"][0]["observation_schema"] = json!(unsupported);
                assert!(current_context("worldstream.house.mira", "1", &input).is_err());
            }
            input["observations"] = json!([]);
            input["projection_reset"]["projection_schema"] =
                json!("worldstream.midnight-archive/participant-projection/v6");
            assert!(current_context("worldstream.house.mira", "1", &input).is_err());
        }
    }

    #[test]
    fn identity_phase_and_planning_mismatches_fail_closed() {
        let valid = observation("mira");
        for (pointer, replacement) in [
            ("/role", json!("jonah")),
            ("/pack/id", json!("worldstream.agent-heist")),
            (
                "/projection_reset/projection/activity/phase",
                json!("terminal"),
            ),
            (
                "/projection_reset/projection/activity/mira/presence",
                json!("absent"),
            ),
            (
                "/projection_reset/projection/activity/mira/planning/status",
                json!("ready"),
            ),
            (
                "/projection_reset/projection/activity/mira/task",
                Value::Null,
            ),
        ] {
            let mut input = valid.clone();
            *input.pointer_mut(pointer).expect("fixture path") = replacement;
            assert!(
                current_context("worldstream.house.mira", "1", &input).is_err(),
                "{pointer}"
            );
        }
        assert!(current_context("worldstream.house.mira", "2", &valid).is_err());
    }

    #[test]
    fn existing_house_policies_retain_their_input_unchanged() {
        assert_eq!(
            current_context(
                "worldstream.house.cooperative-planner",
                "3",
                &json!({"any":"shape"})
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn complete_provider_request_keeps_exact_offers_but_not_retained_history()
    -> Result<(), Box<dyn Error>> {
        let fixture = complete_fixture();
        assert_provider_requests(&fixture, PROJECTION_SCHEMA, OBSERVATION_SCHEMA)
    }

    #[test]
    fn current_v5_pack_projections_and_observations_reach_both_reviewed_companion_providers()
    -> Result<(), Box<dyn Error>> {
        let fixture: Value =
            serde_json::from_slice(include_bytes!("fixtures/archive-v5-model-context.json"))?;
        let listing: Value = serde_json::from_slice(include_bytes!(
            "../../../../tests/fixtures/hosted/listings/midnight-archive-0.3.0.json"
        ))?;
        assert_eq!(
            fixture["provenance"]["pack_revision_digest"],
            listing["pack"]["digest"]
        );
        assert_eq!(fixture["projection_schema"], PROJECTION_SCHEMA_V5);
        assert_eq!(fixture["observation_schema"], OBSERVATION_SCHEMA_V5);
        assert_eq!(
            fixture["payload_schema"]["schema_id"],
            "worldstream.midnight-archive/action-submit_companion_plan/v5"
        );
        for scenario in ["standard-v1", "low-reserve-v1"] {
            for role in ["mira", "jonah"] {
                assert_eq!(
                    fixture["projections"][scenario][role]["session_deadline"],
                    "2026-09-10T12:00:00Z"
                );
            }
        }
        assert_provider_requests(&fixture, PROJECTION_SCHEMA_V5, OBSERVATION_SCHEMA_V5)?;
        assert_dialogue_provider_dispatch(&fixture, PROJECTION_SCHEMA_V5)
    }

    fn assert_provider_requests(
        fixture: &Value,
        projection_schema: &str,
        observation_schema: &str,
    ) -> Result<(), Box<dyn Error>> {
        let offers = offers_from_fixture(fixture);
        let schema = &fixture["payload_schema"]["schema"];
        let canonical_schema = CanonicalJsonV1::parse(&serde_json::to_vec(schema)?)?.to_bytes()?;
        assert_eq!(
            fixture["payload_schema"]["schema_digest"],
            super::super::blake3_digest(&canonical_schema)
        );
        assert_eq!(schema["properties"]["steps"]["maxItems"], 3);
        for scenario in ["standard-v1", "low-reserve-v1"] {
            for role in ["mira", "jonah"] {
                let mut input = observation(role);
                input["projection_reset"]["projection_schema"] = json!(projection_schema);
                let activity = &fixture["projections"][scenario][role];
                assert_eq!(
                    activity["map"]["locations"].as_array().ok_or("map")?.len(),
                    5
                );
                assert_eq!(
                    activity["operation_costs"]
                        .as_object()
                        .ok_or("costs")?
                        .len(),
                    19
                );
                let dialogue = activity["companion_dialogue"]
                    .as_array()
                    .ok_or("dialogue")?;
                assert_eq!(dialogue.len(), 4);
                for utterance in dialogue {
                    let text = utterance["text"].as_str().ok_or("utterance")?;
                    assert_eq!(
                        serde_json::to_string(&serde_json::to_string(text)?)?.len(),
                        192
                    );
                    assert_eq!(utterance["speaker"], role);
                    assert!(utterance.get("speaker_member_id").is_none());
                }
                for candidate in activity["candidates"].as_array().ok_or("candidates")? {
                    assert_eq!(
                        candidate["observed_evidence"]
                            .as_array()
                            .ok_or("evidence")?
                            .len(),
                        2
                    );
                }
                input["projection_reset"]["projection"]["activity"] = activity.clone();
                let (reset_request, _) = build_provider_request(&revision(role)?, &input, &offers)?;
                input["observations"] = json!([
                {"frame_seq":1,"observation":{"irrelevant": "x".repeat(24_000)}},
                {"frame_seq":2,"observation_schema":observation_schema,"observation":activity}]);
                let (frame_request, _) = build_provider_request(&revision(role)?, &input, &offers)?;
                assert_eq!(reset_request.body(), frame_request.body());
                let request_bytes = frame_request.body().len();
                assert!(
                    request_bytes <= 12_000,
                    "{scenario}/{role}: {request_bytes} serialized bytes"
                );
                println!("{scenario}/{role}: {request_bytes} serialized request bytes");
                let body: Value = serde_json::from_slice(frame_request.body())?;
                let invocation: Value = serde_json::from_str(
                    body["messages"][1]["content"].as_str().ok_or("content")?,
                )?;
                assert_eq!(invocation["action_offers"], offers);
                assert!(
                    invocation["projection"]["activity"]
                        .get("session_deadline")
                        .is_none()
                );
                assert_eq!(body["provider"]["only"], json!(["deepinfra/bf16"]));
                assert_eq!(body["provider"]["allow_fallbacks"], false);
                assert_eq!(body["provider"]["zdr"], true);
            }
        }
        Ok(())
    }

    #[test]
    fn invalid_or_oversized_archive_context_never_reserves_or_calls_provider()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let period = HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period,
        )?;
        let provider = Arc::new(DeterministicHouseProviderPortV1::new(
            HouseProposedActionV1 {
                offer_id: "archive-plan-offer".to_owned(),
                payload: json!({}),
            },
            DeterministicHouseProviderFaultV1::None,
        ));
        let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
        let credential = HouseProviderCredentialV1::new(Zeroizing::new(
            b"sk-test-01234567890123456789012345678901".to_vec(),
        ))?;
        let identity = HouseInvocationIdentityV1::new(
            "archive-size",
            "archive-size",
            1,
            &format!("blake3:{}", "a".repeat(64)),
        )?;
        let mut input = observation("jonah");
        assert_eq!(
            executor
                .execute(
                    &credential,
                    &revision("mira")?,
                    &identity,
                    &input,
                    &offers(),
                    period
                )
                .err(),
            Some(HouseModelErrorV1::InvalidInput)
        );
        input = observation("mira");
        input["projection_reset"]["projection"]["activity"]["objective"] =
            json!("x".repeat(12_000));
        assert_eq!(
            executor
                .execute(
                    &credential,
                    &revision("mira")?,
                    &identity,
                    &input,
                    &offers(),
                    period
                )
                .err(),
            Some(HouseModelErrorV1::InputLimit)
        );
        assert_eq!(provider.call_count(), 0);
        assert_eq!(
            ledger.usage("archive-size").err(),
            Some(super::super::HouseAllowanceErrorV1::InvalidInput)
        );
        Ok(())
    }
    #[test]
    fn dialogue_proposals_are_independently_validated_and_failed_paid_attempts_stay_consumed()
    -> Result<(), Box<dyn Error>> {
        assert_dialogue_provider_dispatch(&complete_fixture(), PROJECTION_SCHEMA)
    }

    fn assert_dialogue_provider_dispatch(
        fixture: &Value,
        projection_schema: &str,
    ) -> Result<(), Box<dyn Error>> {
        let offers = offers_from_fixture(fixture);
        for role in ["mira", "jonah"] {
            let mut input = observation(role);
            input["projection_reset"]["projection_schema"] = json!(projection_schema);
            input["projection_reset"]["projection"]["activity"] =
                fixture["projections"]["standard-v1"][role].clone();
            let context = current_context(&format!("worldstream.house.{role}"), "1", &input)?
                .ok_or("context")?;
            let companion = &context["activity"][role];
            let payload = json!({
                "task_revision":companion["task"]["revision"],
                "opportunity_revision":companion["planning"]["opportunity_revision"],
                "dialogue":"a".repeat(160),
                "steps":[
                    {"step_type":"move","destination":"records","source_id":"none","power_cost":0},
                    {"step_type":"move","destination":"plant","source_id":"none","power_cost":0},
                    {"step_type":"open_service_hatch","destination":"none","source_id":"none","power_cost":if role == "mira" {2} else {1}},
                ]
            });
            validate_proposal(&context, &payload)?;
            let offer_id = offers["offers"][0]["offer_id"]
                .as_str()
                .ok_or("offer")?
                .to_owned();
            let action = HouseProposedActionV1 {
                offer_id,
                payload: payload.clone(),
            };
            let reply_bytes =
                serde_json::to_vec(&json!({"offer_id":action.offer_id,"payload":action.payload}))?
                    .len();
            assert!(reply_bytes <= 1_000);
            println!(
                "{role}: {reply_bytes} reply-content bytes with 160-byte utterance and three steps"
            );
            let directory = tempdir()?;
            let period = HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?;
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                directory.path().join("ledger"),
                HouseSpendLimitsV1::hobby_preview(),
                period,
            )?;
            let provider = Arc::new(DeterministicHouseProviderPortV1::new(
                action,
                DeterministicHouseProviderFaultV1::None,
            ));
            let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
            let credential = HouseProviderCredentialV1::new(Zeroizing::new(
                b"sk-test-01234567890123456789012345678901".to_vec(),
            ))?;
            let identity = HouseInvocationIdentityV1::new(
                role,
                role,
                1,
                &format!("blake3:{}", "a".repeat(64)),
            )?;
            let completion = executor.execute(
                &credential,
                &revision(role)?,
                &identity,
                &input,
                &offers,
                period,
            )?;
            assert_eq!(completion.action.payload, payload);
            assert_eq!(provider.call_count(), 1);
            assert_eq!(ledger.usage(role)?.attempts, 1);
            assert!(
                executor
                    .execute(
                        &credential,
                        &revision(role)?,
                        &identity,
                        &input,
                        &offers,
                        period
                    )
                    .is_err()
            );
            assert_eq!(provider.call_count(), 1);
            for text in [
                "a".repeat(161),
                "é".repeat(81),
                "\"".repeat(47),
                "line\nbreak".to_owned(),
            ] {
                let mut invalid = payload.clone();
                invalid["dialogue"] = json!(text);
                assert!(validate_proposal(&context, &invalid).is_err());
            }
            let mut private = context.clone();
            private["activity"][role]["dialogue_allowed"] = json!(false);
            assert!(validate_proposal(&private, &payload).is_err());
            let mut silent = payload.clone();
            silent["dialogue"] = json!("");
            validate_proposal(&private, &silent)?;
            input["projection_reset"]["projection"]["activity"][role]["dialogue_allowed"] =
                json!(false);
            let identity = HouseInvocationIdentityV1::new(
                role,
                &format!("{role}-private"),
                2,
                &format!("blake3:{}", "b".repeat(64)),
            )?;
            assert_eq!(
                executor
                    .execute(
                        &credential,
                        &revision(role)?,
                        &identity,
                        &input,
                        &offers,
                        period
                    )
                    .err(),
                Some(HouseModelErrorV1::InvalidResponse)
            );
            assert_eq!(provider.call_count(), 2);
            let usage = ledger.usage(role)?;
            assert_eq!(usage.attempts, 2);
            assert_eq!(usage.active_calls, 0);
            assert!(usage.consumed_output_units >= 1_000);
        }
        Ok(())
    }
    #[test]
    fn archive_context_rejects_extra_or_non_plan_offers_before_dispatch()
    -> Result<(), Box<dyn Error>> {
        let input = observation("mira");
        let mut wrong = offers();
        wrong["offers"][0]["action_type"] = json!("stage_wait");
        assert!(build_provider_request(&revision("mira")?, &input, &wrong).is_err());
        wrong = offers();
        let mut extra = wrong["offers"][0].clone();
        extra["offer_id"] = json!("different-offer");
        wrong["offers"].as_array_mut().ok_or("offers")?.push(extra);
        assert!(build_provider_request(&revision("mira")?, &input, &wrong).is_err());
        Ok(())
    }
}
