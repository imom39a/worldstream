use std::error::Error;

use serde::Deserialize;
use serde_json::{Value, json};
use worldstream_activity_client::read_activity_client_release;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    ContractError, HouseAgentRevision, ListingRevision, PackReference, ResolvedResultProjector,
    ResultProjectorRevision, derive_room_setup, project_result,
};

const LISTING: &[u8] = include_bytes!("../../../config/hosted/listings/agent-heist-0.2.0.json");
const PROJECTOR: &[u8] =
    include_bytes!("../../../config/hosted/result-projectors/agent-heist-0.2.0.json");
const RUNTIME: &[u8] = include_bytes!(
    "../../../config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"
);
const PROJECTION_SCHEMA: &[u8] =
    include_bytes!("../../../config/hosted/schemas/agent-heist-public-projection-v1.schema.json");
const RESULT_SCHEMA: &[u8] =
    include_bytes!("../../../config/hosted/schemas/result-summary-v1.schema.json");
const CLIENT_RELEASE: &[u8] =
    include_bytes!("../../../config/activity-clients/releases/agent-heist-web.json");
const CORPUS: &[u8] = include_bytes!("../../../fixtures/hosted-contract/corpus.json");
const LAUNCH: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-launch-request.json");
const ROSTER: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
const NONTERMINAL: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-nonterminal-input.json");
const WITHOUT_OUTCOME: &[u8] = include_bytes!(
    "../../../fixtures/hosted-contract/valid/agent-heist-terminal-without-outcome-input.json"
);
const TERMINAL: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-terminal-input.json");
const EXPECTED_SETUP: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/expected/agent-heist-room-setup.json");
const EXPECTED_SUMMARY: &[u8] =
    include_bytes!("../../../fixtures/hosted-contract/expected/agent-heist-result-summary.json");
const COOPERATIVE_HOUSE_AGENT: &[u8] =
    include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json");
const SKEPTICAL_HOUSE_AGENT: &[u8] =
    include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-1.json");
const HOUSE_LISTING: &[u8] =
    include_bytes!("../../../config/hosted/listings/agent-heist-0.3.0.json");

fn canonical(source: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(CanonicalJsonV1::parse(source)?.to_bytes()?)
}

fn canonical_value(value: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(CanonicalJsonV1::parse(&serde_json::to_vec(value)?)?.to_bytes()?)
}

fn source_value(source: &[u8]) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(source)?)
}

fn contracts() -> Result<(ListingRevision, ResultProjectorRevision), Box<dyn Error>> {
    let listing = ListingRevision::from_canonical_bytes(&canonical(LISTING)?)?;
    let projector = ResultProjectorRevision::from_canonical_bytes(&canonical(PROJECTOR)?)?;
    Ok((listing, projector))
}

#[test]
fn validates_two_exact_bounded_house_agent_revisions() -> Result<(), Box<dyn Error>> {
    let cooperative =
        HouseAgentRevision::from_canonical_bytes(&canonical(COOPERATIVE_HOUSE_AGENT)?)?;
    let skeptical = HouseAgentRevision::from_canonical_bytes(&canonical(SKEPTICAL_HOUSE_AGENT)?)?;
    let listing = ListingRevision::from_canonical_bytes(&canonical(HOUSE_LISTING)?)?;
    assert_eq!(
        cooperative.digest(),
        "blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81"
    );
    assert_eq!(
        skeptical.digest(),
        "blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"
    );
    assert_eq!(
        listing.digest(),
        "blake3:66926f7d6c88d0799ec0671a4230141272843e98ed4297dabd0d18cb64ada447"
    );
    assert_ne!(cooperative.digest(), skeptical.digest());
    assert_ne!(cooperative.model_slug(), skeptical.model_slug());
    assert_ne!(cooperative.provider_slug(), skeptical.provider_slug());
    assert_eq!(cooperative.allowance().model_call_attempts, 10);
    assert_eq!(cooperative.allowance().total_input_tokens, 120_000);
    assert_eq!(cooperative.allowance().total_output_tokens, 10_000);
    assert_eq!(cooperative.allowance().input_tokens_per_call, 12_000);
    assert_eq!(cooperative.allowance().output_tokens_per_call, 1_000);
    assert_eq!(cooperative.allowance().concurrent_calls, 1);
    assert_eq!(cooperative.allowance().call_timeout_seconds, 60);
    Ok(())
}

#[test]
fn house_agent_revision_rejects_tools_fallback_and_mutable_allowance() -> Result<(), Box<dyn Error>>
{
    let source = source_value(COOPERATIVE_HOUSE_AGENT)?;
    for mutation in ["tools", "route", "allowance"] {
        let mut value = source.clone();
        match mutation {
            "tools" => value["tools"] = json!(["browser"]),
            "route" => value["route"]["zero_data_retention"] = json!(false),
            "allowance" => value["allowance"]["model_call_attempts"] = json!(11),
            _ => unreachable!(),
        }
        assert!(matches!(
            HouseAgentRevision::from_canonical_bytes(&canonical_value(&value)?),
            Err(ContractError::InvalidShape)
        ));
    }
    Ok(())
}

fn resolve_projector(
    projector: &ResultProjectorRevision,
) -> Result<ResolvedResultProjector, Box<dyn Error>> {
    Ok(projector.resolve_artifacts(
        &canonical(RUNTIME)?,
        &canonical(PROJECTION_SCHEMA)?,
        &canonical(RESULT_SCHEMA)?,
    )?)
}

#[test]
fn resolves_every_exact_pre_genesis_identity() -> Result<(), Box<dyn Error>> {
    let (listing, projector) = contracts()?;
    let client = read_activity_client_release(CLIENT_RELEASE)?;
    listing.verify_pack(listing.pack())?;
    listing.verify_client_release(&client)?;
    listing.verify_projector(&projector)?;
    let _ = resolve_projector(&projector)?;

    let wrong_pack = PackReference {
        id: "worldstream.wrong".to_owned(),
        version: listing.pack().version.clone(),
        digest: listing.pack().digest.clone(),
    };
    assert_eq!(
        listing.verify_pack(&wrong_pack),
        Err(ContractError::ReferenceMismatch)
    );

    let mut wrong_client = client.clone();
    wrong_client.client_id = "worldstream.wrong.web".to_owned();
    assert_eq!(
        listing.verify_client_release(&wrong_client),
        Err(ContractError::ReferenceMismatch)
    );
    let mut wrong_surface = client;
    wrong_surface.surfaces[0].surface_id = "wrong-surface".to_owned();
    assert_eq!(
        listing.verify_client_release(&wrong_surface),
        Err(ContractError::ReferenceMismatch)
    );
    Ok(())
}

#[test]
fn artifact_verification_requires_canonical_duplicate_free_bytes() -> Result<(), Box<dyn Error>> {
    let (_, projector) = contracts()?;
    let duplicate_key = br#"{"$id":1,"$id":2}"#;
    assert!(matches!(
        projector.resolve_artifacts(
            &canonical(RUNTIME)?,
            duplicate_key,
            &canonical(RESULT_SCHEMA)?
        ),
        Err(ContractError::NonCanonical)
    ));
    Ok(())
}

#[test]
fn derives_exact_downstream_setup_without_operator_privilege() -> Result<(), Box<dyn Error>> {
    let (listing, _) = contracts()?;
    let first = derive_room_setup(&listing, &canonical(LAUNCH)?, &canonical(ROSTER)?)?;
    let second = derive_room_setup(&listing, &canonical(LAUNCH)?, &canonical(ROSTER)?)?;
    let bytes = first.canonical_bytes()?;
    let value: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(bytes, second.canonical_bytes()?);
    assert_eq!(bytes, canonical(EXPECTED_SETUP)?);
    assert_eq!(value["schema"], "worldstream/room-setup/v2");
    assert_eq!(value["operator_view"], false);
    assert_eq!(value["spectators"][0]["purpose"], "result_indexer");
    assert_eq!(value["spectators"][1]["purpose"], "public_relay");
    assert!(value["spectators"][0].get("role").is_none());
    assert!(value["spectators"][0].get("scopes").is_none());
    assert_eq!(value["seats"][0]["role"], "navigator");
    assert_eq!(value["seats"][1]["assignment"]["mode"], "external");
    Ok(())
}

#[test]
fn setup_rejects_duplicate_or_invalid_public_principals() -> Result<(), Box<dyn Error>> {
    let (listing, _) = contracts()?;
    let mut roster = source_value(ROSTER)?;
    roster["members"][1]["principal_reference"] =
        roster["members"][0]["principal_reference"].clone();
    assert_eq!(
        derive_room_setup(&listing, &canonical(LAUNCH)?, &canonical_value(&roster)?),
        Err(ContractError::InvalidShape)
    );
    roster["members"][1]["principal_reference"] = json!("agent/invalid");
    assert_eq!(
        derive_room_setup(&listing, &canonical(LAUNCH)?, &canonical_value(&roster)?),
        Err(ContractError::InvalidShape)
    );
    let mut roster = source_value(ROSTER)?;
    roster["members"][0]["principal_reference"] = json!("worldstream:result-indexer");
    assert_eq!(
        derive_room_setup(&listing, &canonical(LAUNCH)?, &canonical_value(&roster)?),
        Err(ContractError::InvalidShape)
    );
    Ok(())
}

#[test]
fn browser_launch_request_cannot_select_server_owned_fields() -> Result<(), Box<dyn Error>> {
    let (listing, _) = contracts()?;
    for field in [
        "pack",
        "role",
        "seat",
        "client",
        "client_url",
        "projector",
        "room_setup",
    ] {
        let mut launch = source_value(LAUNCH)?;
        launch[field] = json!("not-browser-owned");
        assert_eq!(
            derive_room_setup(&listing, &canonical_value(&launch)?, &canonical(ROSTER)?),
            Err(ContractError::InvalidShape)
        );
    }
    Ok(())
}

#[test]
fn creator_participation_is_frozen_and_mutually_exclusive() -> Result<(), Box<dyn Error>> {
    let (must_claim_listing, _) = contracts()?;
    let mut invalid_seat_launch = source_value(LAUNCH)?;
    invalid_seat_launch["creator"]["principal_reference"] = json!("github:not-in-roster");
    assert_eq!(
        derive_room_setup(
            &must_claim_listing,
            &canonical_value(&invalid_seat_launch)?,
            &canonical(ROSTER)?
        ),
        Err(ContractError::InvalidShape)
    );

    let mut listing_value = source_value(LISTING)?;
    listing_value["creator_access"] = json!("may_spectate");
    let listing = ListingRevision::from_canonical_bytes(&canonical_value(&listing_value)?)?;
    let mut launch = source_value(LAUNCH)?;
    launch["listing_revision_digest"] = json!(listing.digest());
    let mut roster = source_value(ROSTER)?;
    roster["listing_revision_digest"] = json!(listing.digest());

    let seated = derive_room_setup(
        &listing,
        &canonical_value(&launch)?,
        &canonical_value(&roster)?,
    )?;
    let seated_value: Value = serde_json::from_slice(&seated.canonical_bytes()?)?;
    assert!(
        seated_value["spectators"]
            .as_array()
            .is_some_and(|spectators| spectators.iter().all(|item| item["purpose"] != "creator"))
    );

    launch["creator"] = json!({
        "participation": "spectator",
        "principal_reference": "worldstream:creator-spectator"
    });
    roster["members"][0]["principal_reference"] = json!("github:2002");
    let spectating = derive_room_setup(
        &listing,
        &canonical_value(&launch)?,
        &canonical_value(&roster)?,
    )?;
    let spectating_value: Value = serde_json::from_slice(&spectating.canonical_bytes()?)?;
    assert!(
        spectating_value["spectators"]
            .as_array()
            .is_some_and(|spectators| {
                spectators.iter().any(|item| {
                    item["purpose"] == "creator"
                        && item["principal"]["reference"] == "worldstream:creator-spectator"
                        && item["principal"]["kind"] == "human"
                })
            })
    );

    let mut forbidden_launch = source_value(LAUNCH)?;
    forbidden_launch["creator"] = launch["creator"].clone();
    assert_eq!(
        derive_room_setup(
            &must_claim_listing,
            &canonical_value(&forbidden_launch)?,
            &canonical(ROSTER)?
        ),
        Err(ContractError::InvalidShape)
    );
    roster["members"][0]["principal_reference"] = json!("worldstream:creator-spectator");
    assert_eq!(
        derive_room_setup(
            &listing,
            &canonical_value(&launch)?,
            &canonical_value(&roster)?
        ),
        Err(ContractError::InvalidShape)
    );
    Ok(())
}

#[test]
fn reviewed_house_fill_derives_existing_managed_assignment_shape() -> Result<(), Box<dyn Error>> {
    let mut listing_value = source_value(LISTING)?;
    listing_value["seats"][1]["allowed_participation"] = json!([
        "account_human",
        "account_external_agent",
        "house_agent_fill"
    ]);
    listing_value["seats"][1]["allowed_house_agent_revisions"] =
        json!(["blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]);
    let listing = ListingRevision::from_canonical_bytes(&canonical_value(&listing_value)?)?;
    let mut launch = source_value(LAUNCH)?;
    launch["listing_revision_digest"] = json!(listing.digest());
    let mut roster = source_value(ROSTER)?;
    roster["listing_revision_digest"] = json!(listing.digest());
    roster["members"][1] = json!({
        "seat_id": "insider",
        "participation": "house_agent_fill",
        "principal_reference": "house:insider-1",
        "display_name": "House Insider",
        "house_agent_revision_digest": "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "agent_profile": {"profile_id": "house-insider", "revision": "1"},
        "runner_template": {"template_id": "openrouter-house", "revision": "1"}
    });
    let setup = derive_room_setup(
        &listing,
        &canonical_value(&launch)?,
        &canonical_value(&roster)?,
    )?;
    let value: Value = serde_json::from_slice(&setup.canonical_bytes()?)?;
    assert_eq!(value["seats"][1]["assignment"]["mode"], "managed");
    assert_eq!(
        value["seats"][1]["assignment"]["agent_profile"]["profile_id"],
        "house-insider"
    );
    Ok(())
}

#[test]
fn projects_exactly_three_bounded_public_states_including_lobby() -> Result<(), Box<dyn Error>> {
    let (listing, projector) = contracts()?;
    let resolved = resolve_projector(&projector)?;
    for (source, expected) in [
        (NONTERMINAL, "not_terminal"),
        (WITHOUT_OUTCOME, "terminal_without_outcome"),
        (TERMINAL, "summary"),
    ] {
        let result = project_result(&listing, &resolved, &canonical(source)?)?;
        let bytes = result.canonical_bytes()?;
        let value: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(value["status"], expected);
        assert!(bytes.len() <= projector.maximum_output_bytes());
        if expected == "summary" {
            assert_eq!(bytes, canonical(EXPECTED_SUMMARY)?);
        }
    }
    Ok(())
}

#[test]
fn projector_requires_listing_projector_pack_schema_and_complete_head() -> Result<(), Box<dyn Error>>
{
    let (listing, projector) = contracts()?;
    let resolved = resolve_projector(&projector)?;
    for path in ["listing", "projector", "pack", "head_pack"] {
        let mut input = source_value(TERMINAL)?;
        match path {
            "listing" => {
                input["listing_revision_digest"] = json!(format!("blake3:{}", "0".repeat(64)));
            }
            "projector" => {
                input["projector_revision_digest"] = json!(format!("blake3:{}", "0".repeat(64)));
            }
            "pack" => {
                input["pack"]["digest"] = json!(format!("blake3:{}", "0".repeat(64)));
            }
            "head_pack" => {
                input["source_head"]["pack_digest"] = json!(format!("blake3:{}", "0".repeat(64)));
            }
            _ => unreachable!(),
        }
        assert_eq!(
            project_result(&listing, &resolved, &canonical_value(&input)?),
            Err(ContractError::ReferenceMismatch)
        );
    }
    Ok(())
}

#[test]
fn projector_rejects_private_unknown_or_unreviewed_public_fields() -> Result<(), Box<dyn Error>> {
    let (listing, projector) = contracts()?;
    let resolved = resolve_projector(&projector)?;
    for mutation in ["private", "outcome", "phase"] {
        let mut input = source_value(TERMINAL)?;
        match mutation {
            "private" => input["public_projection"]["fixture"] = json!({"secret": true}),
            "outcome" => input["public_projection"]["outcome"]["outcome"] = json!("spectacular"),
            "phase" => input["public_projection"]["phase"] = json!("unknown"),
            _ => unreachable!(),
        }
        assert!(project_result(&listing, &resolved, &canonical_value(&input)?).is_err());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Corpus {
    listing_digest: String,
    projector_digest: String,
    wire_cases: Vec<WireCase>,
}

#[derive(Debug, Deserialize)]
struct WireCase {
    name: String,
    target: String,
    operation: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
}

fn transformed_wire(case: &WireCase) -> Result<Vec<u8>, Box<dyn Error>> {
    let source = if case.target == "listing" {
        LISTING
    } else {
        PROJECTOR
    };
    let bytes = canonical(source)?;
    if case.operation == "prefix_space" {
        let mut result = vec![b' '];
        result.extend(bytes);
        return Ok(result);
    }
    let text = String::from_utf8(bytes)?;
    if text.match_indices(&case.from).count() != 1 {
        return Err(format!("corpus replacement is not unique: {}", case.name).into());
    }
    Ok(text.replacen(&case.from, &case.to, 1).into_bytes())
}

#[test]
fn rust_consumes_the_shared_exact_wire_rejection_corpus() -> Result<(), Box<dyn Error>> {
    let corpus: Corpus = serde_json::from_slice(CORPUS)?;
    let (listing, projector) = contracts()?;
    assert_eq!(listing.digest(), corpus.listing_digest);
    assert_eq!(projector.digest(), corpus.projector_digest);
    for case in &corpus.wire_cases {
        let wire = transformed_wire(case)?;
        let rejected = match case.target.as_str() {
            "listing" => ListingRevision::from_canonical_bytes(&wire).is_err(),
            "projector" => ResultProjectorRevision::from_canonical_bytes(&wire).is_err(),
            _ => return Err(format!("unknown corpus target: {}", case.target).into()),
        };
        assert!(rejected, "wire case was accepted: {}", case.name);
    }
    Ok(())
}

#[test]
fn semantic_contract_changes_change_identity_and_break_listing_pin() -> Result<(), Box<dyn Error>> {
    let (listing, projector) = contracts()?;
    let mut changed_listing = source_value(LISTING)?;
    changed_listing["title"] = json!("Agent Heist!");
    let changed_listing =
        ListingRevision::from_canonical_bytes(&canonical_value(&changed_listing)?)?;
    assert_ne!(listing.digest(), changed_listing.digest());

    let mut changed_program = source_value(PROJECTOR)?;
    changed_program["program"]["summary_fields"][0]["values"] =
        json!(["failure", "partial_failure", "success"]);
    let changed_projector =
        ResultProjectorRevision::from_canonical_bytes(&canonical_value(&changed_program)?)?;
    assert_ne!(projector.digest(), changed_projector.digest());
    assert_eq!(
        listing.verify_projector(&changed_projector),
        Err(ContractError::ReferenceMismatch)
    );
    Ok(())
}
