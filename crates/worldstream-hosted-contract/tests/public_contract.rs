use std::error::Error;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use worldstream_activity_client::read_activity_client_release;
use worldstream_core::{CanonicalJsonV1, projection_hash_for_canonical_bytes};
use worldstream_hosted_contract::{
    ContractError, HostedAuthorizedPublicProjectionV1, HostedBrowserHandoffRedeemRequestV1,
    HostedBrowserHandoffRedeemResponseV1, HostedBrowserHandoffRequestV1,
    HostedBrowserHandoffResponseV1, HostedBrowserSessionLogoutV1, HostedBrowserSessionRequestV1,
    HostedBrowserSessionStateV1, HostedBrowserSessionStatusV1, HostedBrowserStreamTicketRequestV1,
    HostedBrowserStreamTicketResponseV1, HostedCapacityAuthorizationV1, HostedGenesisAccessModeV1,
    HostedGenesisEvidenceV1, HostedGenesisHeadV1, HostedGenesisMembershipPurposeV1,
    HostedGenesisMembershipV1, HostedGenesisPrincipalKindV1, HostedHouseRunnerAssignmentV1,
    HostedHouseRunnerReservationOutcomeV1, HostedHouseRunnerReservationReceiptV1,
    HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1, HostedPublicRelayBindReceiptV1,
    HostedPublicRelayBindRequestV1, HostedPublicStreamTicketRequestV1,
    HostedResultIntegrityStatusV1, HostedResultReplayEvidenceV1, HostedResultSourceEvidenceV1,
    HostedResultSourceHeadV1, HostedResultSourceRequestV1, HouseAgentRevision, ListingRevision,
    PackReference, ResolvedResultProjector, ResultProjectorRevision, derive_room_setup,
    derive_room_setup_with_house_agents, project_result,
    validate_hosted_browser_handoff_redeem_request,
    validate_hosted_browser_handoff_redeem_response, validate_hosted_browser_handoff_request,
    validate_hosted_browser_handoff_response, validate_hosted_browser_session_logout,
    validate_hosted_browser_session_request, validate_hosted_browser_session_status,
    validate_hosted_browser_stream_ticket_request, validate_hosted_browser_stream_ticket_response,
    validate_hosted_genesis_evidence, validate_hosted_launch_evidence_request,
    validate_hosted_launch_request, validate_hosted_public_relay_bind_receipt,
    validate_hosted_public_relay_bind_request, validate_hosted_public_stream_ticket_request,
    validate_hosted_result_source_evidence, validate_hosted_result_source_request,
};

const LISTING: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.2.0.json");
const PROJECTOR: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/result-projectors/agent-heist-0.2.0.json");
const RUNTIME: &[u8] = include_bytes!(
    "../../../tests/fixtures/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"
);
const PROJECTION_SCHEMA: &[u8] = include_bytes!(
    "../../../tests/fixtures/hosted/schemas/agent-heist-public-projection-v1.schema.json"
);
const RESULT_SCHEMA: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/schemas/result-summary-v1.schema.json");
const CLIENT_RELEASE: &[u8] =
    include_bytes!("../../../examples/clients/catalog/releases/agent-heist-web.json");
const CORPUS: &[u8] = include_bytes!("../../../tests/fixtures/hosted-contract/corpus.json");
const LAUNCH: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted-contract/valid/agent-heist-launch-request.json");
const ROSTER: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
const NONTERMINAL: &[u8] = include_bytes!(
    "../../../tests/fixtures/hosted-contract/valid/agent-heist-nonterminal-input.json"
);
const WITHOUT_OUTCOME: &[u8] = include_bytes!(
    "../../../tests/fixtures/hosted-contract/valid/agent-heist-terminal-without-outcome-input.json"
);
const TERMINAL: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted-contract/valid/agent-heist-terminal-input.json");
const EXPECTED_SETUP: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted-contract/expected/agent-heist-room-setup.json");
const EXPECTED_SUMMARY: &[u8] = include_bytes!(
    "../../../tests/fixtures/hosted-contract/expected/agent-heist-result-summary.json"
);
const COOPERATIVE_HOUSE_AGENT: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-1.json");
const SKEPTICAL_HOUSE_AGENT: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-1.json");
const HOUSE_LISTING: &[u8] =
    include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.3.0.json");

fn canonical(source: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(CanonicalJsonV1::parse(source)?.to_bytes()?)
}

fn canonical_value(value: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(CanonicalJsonV1::parse(&serde_json::to_vec(value)?)?.to_bytes()?)
}

fn source_value(source: &[u8]) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(source)?)
}

fn tagged_sha256(bytes: &[u8]) -> String {
    let mut value = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        value.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        value.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    value
}

fn contracts() -> Result<(ListingRevision, ResultProjectorRevision), Box<dyn Error>> {
    let listing = ListingRevision::from_canonical_bytes(&canonical(LISTING)?)?;
    let projector = ResultProjectorRevision::from_canonical_bytes(&canonical(PROJECTOR)?)?;
    Ok((listing, projector))
}

fn hosted_browser_handoff_request() -> HostedBrowserHandoffRequestV1 {
    HostedBrowserHandoffRequestV1 {
        schema: "worldstream/hosted-browser-handoff-request/v1".to_owned(),
        platform_account_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: format!("blake3:{}", "1".repeat(64)),
        host_installation_id: "hosted-preview-1".to_owned(),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        pack: PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: format!("blake3:{}", "2".repeat(64)),
        },
        client_release_digest: format!("sha256:{}", "3".repeat(64)),
        client_surface_id: "participant".to_owned(),
        access_mode: HostedGenesisAccessModeV1::Participant,
        purpose: HostedGenesisMembershipPurposeV1::Participant,
        seat_id: Some("navigator".to_owned()),
        role: Some("navigator".to_owned()),
        principal_kind: HostedGenesisPrincipalKindV1::Human,
        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
        membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
    }
}

fn hosted_public_relay_request() -> HostedPublicRelayBindRequestV1 {
    HostedPublicRelayBindRequestV1 {
        schema: "worldstream/hosted-public-relay-bind-request/v1".to_owned(),
        public_run_id: "0123456789abcdef0123456789abcdef".to_owned(),
        activity_run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        launch_request_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: format!("blake3:{}", "1".repeat(64)),
        launch_request_digest: format!("blake3:{}", "2".repeat(64)),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        pack: PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: format!("blake3:{}", "3".repeat(64)),
        },
        relay_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
        relay_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
    }
}

#[test]
fn hosted_browser_contract_accepts_account_controlled_human_and_agent_participants() {
    let participant = hosted_browser_handoff_request();
    assert_eq!(
        validate_hosted_browser_handoff_request(&participant),
        Ok(())
    );

    let mut spectator = participant.clone();
    spectator.access_mode = HostedGenesisAccessModeV1::Spectator;
    spectator.purpose = HostedGenesisMembershipPurposeV1::CreatorSpectator;
    spectator.seat_id = None;
    spectator.role = None;
    assert_eq!(validate_hosted_browser_handoff_request(&spectator), Ok(()));

    let mut agent = participant.clone();
    agent.principal_kind = HostedGenesisPrincipalKindV1::Agent;
    assert_eq!(validate_hosted_browser_handoff_request(&agent), Ok(()));

    let mut agent_spectator = spectator.clone();
    agent_spectator.principal_kind = HostedGenesisPrincipalKindV1::Agent;
    assert_eq!(
        validate_hosted_browser_handoff_request(&agent_spectator),
        Err(ContractError::InvalidShape)
    );

    let mut widened = spectator;
    widened.purpose = HostedGenesisMembershipPurposeV1::ResultIndexer;
    assert_eq!(
        validate_hosted_browser_handoff_request(&widened),
        Err(ContractError::InvalidShape)
    );

    let mut mismatched = participant;
    mismatched.access_mode = HostedGenesisAccessModeV1::Spectator;
    assert_eq!(
        validate_hosted_browser_handoff_request(&mismatched),
        Err(ContractError::InvalidShape)
    );
}

#[test]
fn hosted_browser_contract_keeps_handoffs_and_sessions_opaque_and_bounded() {
    let handoff = format!("wsh1:{}", "a".repeat(64));
    let session = format!("wss1:{}", "b".repeat(64));
    assert_eq!(
        validate_hosted_browser_handoff_response(&HostedBrowserHandoffResponseV1 {
            schema: "worldstream/hosted-browser-handoff-response/v1".to_owned(),
            client_url: format!("https://arena.example/clients/heist/#handoff={handoff}"),
        }),
        Ok(())
    );
    assert_eq!(
        validate_hosted_browser_handoff_redeem_request(&HostedBrowserHandoffRedeemRequestV1 {
            schema: "worldstream/hosted-browser-handoff-redeem-request/v1".to_owned(),
            platform_account_id: "10000000-0000-4000-8000-000000000001".to_owned(),
            handoff,
            prior_session: Some(session.clone()),
        }),
        Ok(())
    );
    assert_eq!(
        validate_hosted_browser_handoff_redeem_response(&HostedBrowserHandoffRedeemResponseV1 {
            schema: "worldstream/hosted-browser-handoff-redeem-response/v1".to_owned(),
            session: session.clone(),
        }),
        Ok(())
    );
    assert_eq!(
        validate_hosted_browser_session_request(&HostedBrowserSessionRequestV1 {
            schema: "worldstream/hosted-browser-session-request/v1".to_owned(),
            session: session.clone(),
        }),
        Ok(())
    );
    assert_eq!(
        validate_hosted_browser_session_status(&HostedBrowserSessionStatusV1 {
            schema: "worldstream/hosted-browser-session-status/v1".to_owned(),
            state: HostedBrowserSessionStateV1::Usable,
        }),
        Ok(())
    );
    assert_eq!(
        validate_hosted_browser_session_logout(&HostedBrowserSessionLogoutV1 {
            schema: "worldstream/hosted-browser-session-logout/v1".to_owned(),
            logged_out: true,
        }),
        Ok(())
    );
    let ticket = format!("wst1:{}", "c".repeat(64));
    assert_eq!(
        validate_hosted_browser_stream_ticket_request(&HostedBrowserStreamTicketRequestV1 {
            schema: "worldstream/hosted-browser-stream-ticket-request/v1".to_owned(),
            session: session.clone(),
            after_frame_seq: Some(41),
        }),
        Ok(())
    );
    let response = HostedBrowserStreamTicketResponseV1 {
        schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
        ticket: ticket.clone(),
        expires_in_ms: 15_000,
    };
    assert_eq!(
        validate_hosted_browser_stream_ticket_response(&response),
        Ok(())
    );
    assert!(!format!("{response:?}").contains(&ticket));

    assert!(
        validate_hosted_browser_handoff_response(&HostedBrowserHandoffResponseV1 {
            schema: "worldstream/hosted-browser-handoff-response/v1".to_owned(),
            client_url: format!(
                "https://arena.example/clients/heist/?room_id=private#handoff=wsh1:{}",
                "a".repeat(64)
            ),
        })
        .is_err()
    );
    assert!(
        validate_hosted_browser_session_request(&HostedBrowserSessionRequestV1 {
            schema: "worldstream/hosted-browser-session-request/v1".to_owned(),
            session: session.to_uppercase(),
        })
        .is_err()
    );
    assert!(
        validate_hosted_browser_stream_ticket_response(&HostedBrowserStreamTicketResponseV1 {
            schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
            ticket,
            expires_in_ms: 15_001,
        })
        .is_err()
    );
}

#[test]
fn hosted_public_relay_contract_is_exact_and_secret_free() -> Result<(), Box<dyn Error>> {
    let request = hosted_public_relay_request();
    assert_eq!(validate_hosted_public_relay_bind_request(&request), Ok(()));
    let request_json = serde_json::to_value(&request)?;
    for forbidden in ["credential", "bearer", "secret_reference"] {
        assert!(request_json.get(forbidden).is_none());
    }
    let receipt = HostedPublicRelayBindReceiptV1 {
        schema: "worldstream/hosted-public-relay-bind-receipt/v1".to_owned(),
        public_run_id: request.public_run_id.clone(),
        activity_run_id: request.activity_run_id.clone(),
        binding_request_digest: format!("sha256:{}", "4".repeat(64)),
        bound: true,
    };
    assert_eq!(validate_hosted_public_relay_bind_receipt(&receipt), Ok(()));
    assert_eq!(
        validate_hosted_public_stream_ticket_request(&HostedPublicStreamTicketRequestV1 {
            schema: "worldstream/hosted-public-stream-ticket-request/v1".to_owned(),
            public_run_id: request.public_run_id.clone(),
        }),
        Ok(())
    );

    let mut result_only_style_id = request.clone();
    result_only_style_id.public_run_id = "A".repeat(32);
    assert!(validate_hosted_public_relay_bind_request(&result_only_style_id).is_err());
    let mut widened = request;
    widened.relay_membership_id = widened.relay_principal_id.clone();
    assert!(validate_hosted_public_relay_bind_request(&widened).is_err());
    Ok(())
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
        "blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956"
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
    let listing = ListingRevision::from_canonical_bytes(&canonical(HOUSE_LISTING)?)?;
    let house = HouseAgentRevision::from_canonical_bytes(&canonical(COOPERATIVE_HOUSE_AGENT)?)?;
    let mut launch = source_value(LAUNCH)?;
    launch["listing_revision_digest"] = json!(listing.digest());
    let mut roster = source_value(ROSTER)?;
    roster["listing_revision_digest"] = json!(listing.digest());
    roster["members"][1] = json!({
        "seat_id": "insider",
        "participation": "house_agent_fill",
        "principal_reference": "house:insider-1",
        "display_name": "Cooperative Planner",
        "house_agent_revision_digest": house.digest(),
        "agent_profile": {"profile_id": "house-cooperative-planner", "revision": "1"},
        "runner_template": {"template_id": "openrouter-house", "revision": "1"}
    });
    assert_eq!(
        derive_room_setup(
            &listing,
            &canonical_value(&launch)?,
            &canonical_value(&roster)?,
        ),
        Err(ContractError::ReferenceMismatch)
    );
    let setup = derive_room_setup_with_house_agents(
        &listing,
        &canonical_value(&launch)?,
        &canonical_value(&roster)?,
        std::slice::from_ref(&house),
    )?;
    let value: Value = serde_json::from_slice(&setup.canonical_bytes()?)?;
    assert_eq!(value["seats"][1]["assignment"]["mode"], "managed");
    assert_eq!(
        value["seats"][1]["assignment"]["agent_profile"]["profile_id"],
        "house-cooperative-planner"
    );

    roster["members"][1]["agent_profile"]["revision"] = json!("2");
    assert_eq!(
        derive_room_setup_with_house_agents(
            &listing,
            &canonical_value(&launch)?,
            &canonical_value(&roster)?,
            std::slice::from_ref(&house),
        ),
        Err(ContractError::ReferenceMismatch)
    );

    roster["members"][1]["agent_profile"]["revision"] = json!("1");
    roster["members"][1]["display_name"] = json!("Mutable Alias");
    assert_eq!(
        derive_room_setup_with_house_agents(
            &listing,
            &canonical_value(&launch)?,
            &canonical_value(&roster)?,
            std::slice::from_ref(&house),
        ),
        Err(ContractError::ReferenceMismatch)
    );
    Ok(())
}

#[test]
fn hosted_house_fill_is_bound_to_the_exact_launch_reference() -> Result<(), Box<dyn Error>> {
    let listing = ListingRevision::from_canonical_bytes(&canonical(HOUSE_LISTING)?)?;
    let house = HouseAgentRevision::from_canonical_bytes(&canonical(COOPERATIVE_HOUSE_AGENT)?)?;
    let launch_reference = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let mut launch = source_value(LAUNCH)?;
    launch["listing_revision_digest"] = json!(listing.digest());
    let mut roster = source_value(ROSTER)?;
    roster["listing_revision_digest"] = json!(listing.digest());
    roster["members"][1] = json!({
        "seat_id": "insider",
        "participation": "house_agent_fill",
        "principal_reference": format!("house:{launch_reference}:insider"),
        "display_name": house.display_name(),
        "house_agent_revision_digest": house.digest(),
        "agent_profile": {"profile_id": "house-cooperative-planner", "revision": "1"},
        "runner_template": {"template_id": "openrouter-house", "revision": "1"}
    });
    let launch_bytes = canonical_value(&launch)?;
    let roster_bytes = canonical_value(&roster)?;
    let setup = derive_room_setup_with_house_agents(
        &listing,
        &launch_bytes,
        &roster_bytes,
        std::slice::from_ref(&house),
    )?
    .canonical_bytes()?;
    let mut request = HostedLaunchRequestV1 {
        schema: "worldstream/hosted-launch-request/v1".to_owned(),
        listing_revision_digest: listing.digest().to_owned(),
        launch_request_digest: format!("blake3:{}", blake3::hash(&launch_bytes).to_hex()),
        launch_input_digest: tagged_sha256(b"{}"),
        frozen_roster_digest: tagged_sha256(&roster_bytes),
        room_setup_specification_digest: format!("blake3:{}", blake3::hash(&setup).to_hex()),
        room_setup_operation_id: "hosted-house-launch-01".to_owned(),
        capacity_authorization: HostedCapacityAuthorizationV1 {
            schema: "worldstream/platform-capacity-authorization/v1".to_owned(),
            host_installation_id: "hosted-preview-1".to_owned(),
            reservation_reference: launch_reference.to_owned(),
        },
        house_runner_assignments: vec![HostedHouseRunnerAssignmentV1 {
            house_agent_assignment_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc".to_owned(),
            reservation_receipt: HostedHouseRunnerReservationReceiptV1 {
                schema: "worldstream/house-runner-reservation-receipt/v1".to_owned(),
                host_installation_id: "hosted-preview-1".to_owned(),
                reservation_operation_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_owned(),
                launch_request_id: launch_reference.to_owned(),
                listing_revision_digest: listing.digest().to_owned(),
                seat_id: "insider".to_owned(),
                house_agent_revision_digest: house.digest().to_owned(),
                outcome: HostedHouseRunnerReservationOutcomeV1::Succeeded,
                runner_unit_id: Some("house-insider-01".to_owned()),
                failure_code: None,
                binding_digest: format!("blake3:{}", "e".repeat(64)),
                authentication_tag: "f".repeat(64),
            },
        }],
        frozen_launch_request: launch,
        frozen_roster: roster,
        frozen_room_setup_specification: source_value(&setup)?,
    };
    assert!(
        validate_hosted_launch_request(
            &request,
            "hosted-preview-1",
            &listing,
            std::slice::from_ref(&house),
        )
        .is_ok()
    );

    request.frozen_roster["members"][1]["principal_reference"] =
        json!("house:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb:insider");
    request.frozen_roster_digest = tagged_sha256(&canonical_value(&request.frozen_roster)?);
    assert_eq!(
        validate_hosted_launch_request(
            &request,
            "hosted-preview-1",
            &listing,
            std::slice::from_ref(&house),
        ),
        Err(ContractError::ReferenceMismatch)
    );
    Ok(())
}

#[test]
fn hosted_launch_rederives_every_frozen_value_and_capacity_binding() -> Result<(), Box<dyn Error>> {
    let listing = ListingRevision::from_canonical_bytes(&canonical(LISTING)?)?;
    let launch = canonical(LAUNCH)?;
    let roster = canonical(ROSTER)?;
    let setup = canonical(EXPECTED_SETUP)?;
    let request = HostedLaunchRequestV1 {
        schema: "worldstream/hosted-launch-request/v1".to_owned(),
        listing_revision_digest: listing.digest().to_owned(),
        launch_request_digest: format!("blake3:{}", blake3::hash(&launch).to_hex()),
        launch_input_digest: tagged_sha256(b"{}"),
        frozen_roster_digest: tagged_sha256(&roster),
        room_setup_specification_digest: format!("blake3:{}", blake3::hash(&setup).to_hex()),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        capacity_authorization: HostedCapacityAuthorizationV1 {
            schema: "worldstream/platform-capacity-authorization/v1".to_owned(),
            host_installation_id: "hosted-preview-1".to_owned(),
            reservation_reference: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        },
        house_runner_assignments: vec![],
        frozen_launch_request: source_value(&launch)?,
        frozen_roster: source_value(&roster)?,
        frozen_room_setup_specification: source_value(&setup)?,
    };
    assert!(validate_hosted_launch_request(&request, "hosted-preview-1", &listing, &[]).is_ok());

    let mut changed = request.clone();
    changed.frozen_room_setup_specification["operator_view"] = json!(true);
    assert_eq!(
        validate_hosted_launch_request(&changed, "hosted-preview-1", &listing, &[]),
        Err(ContractError::ReferenceMismatch)
    );
    let mut changed_input_digest = request.clone();
    changed_input_digest.launch_input_digest = format!("sha256:{}", "0".repeat(64));
    assert_eq!(
        validate_hosted_launch_request(&changed_input_digest, "hosted-preview-1", &listing, &[],),
        Err(ContractError::ReferenceMismatch)
    );
    assert_eq!(
        validate_hosted_launch_request(&request, "different-host", &listing, &[]),
        Err(ContractError::ReferenceMismatch)
    );

    let evidence = HostedLaunchEvidenceRequestV1 {
        schema: "worldstream/hosted-launch-evidence-request/v1".to_owned(),
        listing_revision_digest: request.listing_revision_digest,
        launch_request_digest: request.launch_request_digest,
        room_setup_operation_id: request.room_setup_operation_id,
    };
    assert!(validate_hosted_launch_evidence_request(&evidence).is_ok());
    let mut wrong_operation = evidence;
    wrong_operation.room_setup_operation_id = "UPSTREAM/path".to_owned();
    assert_eq!(
        validate_hosted_launch_evidence_request(&wrong_operation),
        Err(ContractError::InvalidShape)
    );
    Ok(())
}

fn genesis_evidence() -> HostedGenesisEvidenceV1 {
    let digest = |value: char| format!("blake3:{}", value.to_string().repeat(64));
    let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned();
    HostedGenesisEvidenceV1 {
        schema: "worldstream/hosted-genesis-evidence/v1".to_owned(),
        host_installation_id: "hosted-preview-1".to_owned(),
        launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        listing_revision_digest: digest('1'),
        launch_request_digest: digest('2'),
        frozen_roster_digest: format!("sha256:{}", "3".repeat(64)),
        room_setup_specification_digest: digest('4'),
        room_setup_operation_id: "hosted-launch-01".to_owned(),
        room_id: room_id.clone(),
        pack: PackReference {
            id: "worldstream.test".to_owned(),
            version: "1.0.0".to_owned(),
            digest: digest('5'),
        },
        genesis_head: HostedGenesisHeadV1 {
            room_id,
            room_seq: 0,
            genesis_or_transition_hash: digest('6'),
            core_schema_version: "worldstream/core-room-state/v1".to_owned(),
            pack_digest: digest('5'),
            core_state_hash: digest('7'),
            activity_state_hash: digest('8'),
            authoritative_state_hash: digest('9'),
        },
        memberships: vec![
            HostedGenesisMembershipV1 {
                access_mode: HostedGenesisAccessModeV1::Participant,
                purpose: HostedGenesisMembershipPurposeV1::Participant,
                seat_id: Some("navigator".to_owned()),
                role: Some("navigator".to_owned()),
                principal_kind: HostedGenesisPrincipalKindV1::Human,
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
                membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
                scopes: vec![],
            },
            HostedGenesisMembershipV1 {
                access_mode: HostedGenesisAccessModeV1::Spectator,
                purpose: HostedGenesisMembershipPurposeV1::ResultIndexer,
                seat_id: None,
                role: None,
                principal_kind: HostedGenesisPrincipalKindV1::Agent,
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
                membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
                scopes: vec![
                    "room:attach".to_owned(),
                    "room:observe_public".to_owned(),
                    "room:replay".to_owned(),
                ],
            },
        ],
    }
}

#[test]
fn genesis_evidence_is_sequence_zero_duplicate_free_and_scope_closed() {
    let evidence = genesis_evidence();
    assert!(validate_hosted_genesis_evidence(&evidence).is_ok());

    let mut transitioned = evidence.clone();
    transitioned.genesis_head.room_seq = 1;
    assert_eq!(
        validate_hosted_genesis_evidence(&transitioned),
        Err(ContractError::ReferenceMismatch)
    );

    let mut duplicate = evidence.clone();
    duplicate.memberships[1].principal_id = duplicate.memberships[0].principal_id.clone();
    assert_eq!(
        validate_hosted_genesis_evidence(&duplicate),
        Err(ContractError::InvalidShape)
    );

    let mut widened = evidence;
    widened.memberships[1].scopes.push("room:act".to_owned());
    assert_eq!(
        validate_hosted_genesis_evidence(&widened),
        Err(ContractError::InvalidShape)
    );
}

fn result_source_evidence() -> Result<HostedResultSourceEvidenceV1, Box<dyn Error>> {
    let input = source_value(TERMINAL)?;
    let source_head =
        serde_json::from_value::<HostedResultSourceHeadV1>(input["source_head"].clone())?;
    let public_projection = HostedAuthorizedPublicProjectionV1 {
        projection_schema: "agent-heist/projection/v1".to_owned(),
        authorized_core: json!({"access_mode": "spectator"}),
        projection: input["public_projection"].clone(),
        action_offers: vec![],
    };
    let projection_bytes = canonical_value(&serde_json::to_value(&public_projection)?)?;
    let projection_hash = projection_hash_for_canonical_bytes(&projection_bytes)?.to_string();
    Ok(HostedResultSourceEvidenceV1 {
        schema: "worldstream/hosted-result-source-evidence/v1".to_owned(),
        host_installation_id: "fly-primary".to_owned(),
        launch_request_id: "00000000-0000-4000-8000-000000000001".to_owned(),
        run_id: "00000000-0000-4000-8000-000000000002".to_owned(),
        listing_revision_digest:
            "blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1".to_owned(),
        launch_request_digest: format!("blake3:{}", "1".repeat(64)),
        room_setup_operation_id: "launch-result-source".to_owned(),
        room_id: source_head.room_id.clone(),
        pack: serde_json::from_value(input["pack"].clone())?,
        result_indexer_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
        access_mode: HostedGenesisAccessModeV1::Spectator,
        source_head: source_head.clone(),
        integrity_status: HostedResultIntegrityStatusV1::Healthy,
        integrity_generation: 7,
        projection_schema: "agent-heist/projection/v1".to_owned(),
        public_projection,
        projection_hash: projection_hash.clone(),
        replay: Some(HostedResultReplayEvidenceV1 {
            verifier_revision: "worldstream.authorized-replay/v1".to_owned(),
            verified_head: source_head,
            projection_hash,
            verification_receipt_digest: format!("sha256:{}", "2".repeat(64)),
        }),
    })
}

#[test]
fn result_source_pull_is_run_bound_hash_verified_and_replay_exact() -> Result<(), Box<dyn Error>> {
    let request = HostedResultSourceRequestV1 {
        schema: "worldstream/hosted-result-source-request/v1".to_owned(),
        run_id: "00000000-0000-4000-8000-000000000002".to_owned(),
        listing_revision_digest:
            "blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1".to_owned(),
        launch_request_digest: format!("blake3:{}", "1".repeat(64)),
        room_setup_operation_id: "launch-result-source".to_owned(),
    };
    assert!(validate_hosted_result_source_request(&request).is_ok());

    let evidence = result_source_evidence()?;
    assert_eq!(validate_hosted_result_source_evidence(&evidence), Ok(()));
    let mut observation_only = evidence.clone();
    observation_only.replay = None;
    assert!(validate_hosted_result_source_evidence(&observation_only).is_ok());

    let mut changed_projection = evidence.clone();
    changed_projection.public_projection.projection["fixture"] = json!(true);
    assert_eq!(
        validate_hosted_result_source_evidence(&changed_projection),
        Err(ContractError::ReferenceMismatch)
    );

    let mut changed_replay = evidence;
    changed_replay
        .replay
        .as_mut()
        .ok_or("missing replay")?
        .verified_head
        .room_seq += 1;
    assert_eq!(
        validate_hosted_result_source_evidence(&changed_replay),
        Err(ContractError::ReferenceMismatch)
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

#[test]
fn generic_projector_program_is_bounded_without_inheriting_heist_fields()
-> Result<(), Box<dyn Error>> {
    let pack = json!({
        "id": "example.social-puzzle",
        "version": "1.0.0",
        "digest": format!("blake3:{}", "a".repeat(64)),
    });
    let projection_schema =
        canonical_value(&json!({"schema":"example/social-puzzle-projection/v1"}))?;
    let output_schema = canonical_value(&json!({"schema":"example/social-puzzle-result/v1"}))?;
    let projection_digest = format!("blake3:{}", blake3::hash(&projection_schema).to_hex());
    let output_digest = format!("blake3:{}", blake3::hash(&output_schema).to_hex());
    let runtime = source_value(PROJECTOR)?["runtime"].clone();
    let projector_source = json!({
        "schema": "worldstream/result-projector-revision/v1",
        "projector_id": "example.social-puzzle.result",
        "version": "1.0.0",
        "runtime": runtime,
        "input": {
            "pack": pack,
            "listing_schema": "worldstream/activity-listing-revision/v1",
            "complete_head_schema": "worldstream/complete-head/v1",
            "projection": {"schema":"example/social-puzzle-projection/v1","digest":projection_digest},
        },
        "program": {
            "schema": "worldstream/result-projector-program/v1",
            "terminal": {"field":"state","equals":"done"},
            "outcome_field": "result",
            "summary_fields": [
                {"output":"verdict","source":"verdict","kind":"enum","values":["pass","fail"]},
                {"output":"points","source":"points","kind":"integer","minimum":0,"maximum":100},
            ],
        },
        "output": {
            "schema": "example/social-puzzle-result/v1",
            "schema_digest": output_digest,
            "canonicalizer": "worldstream/canonical-json/v1",
            "maximum_bytes": 1024,
        },
        "maximum_input_bytes": 4096,
    });
    let projector_bytes = canonical_value(&projector_source)?;
    let projector = ResultProjectorRevision::from_canonical_bytes(&projector_bytes)
        .map_err(|error| format!("generic projector: {error:?}"))?;
    let mut listing_source = source_value(LISTING)?;
    listing_source["pack"] = projector_source["input"]["pack"].clone();
    listing_source["result"]["projection"] = projector_source["input"]["projection"].clone();
    listing_source["result"]["projector"] = json!({
        "id": "example.social-puzzle.result",
        "version": "1.0.0",
        "digest": projector.digest(),
    });
    let listing = ListingRevision::from_canonical_bytes(&canonical_value(&listing_source)?)
        .map_err(|error| format!("generic listing: {error:?}"))?;
    let resolved = projector
        .resolve_artifacts(&canonical(RUNTIME)?, &projection_schema, &output_schema)
        .map_err(|error| format!("generic artifacts: {error:?}"))?;
    let mut source_head = source_value(TERMINAL)?["source_head"].clone();
    source_head["pack_digest"] = projector_source["input"]["pack"]["digest"].clone();
    let input = json!({
        "schema": "worldstream/result-projector-input/v1",
        "listing_revision_digest": listing.digest(),
        "projector_revision_digest": projector.digest(),
        "pack": projector_source["input"]["pack"].clone(),
        "projection_schema": "example/social-puzzle-projection/v1",
        "source_head": source_head,
        "public_projection": {
            "state": "done",
            "result": {"verdict":"pass","points":7},
            "reviewed_public_context": "allowed by this generic projector",
        },
    });
    let result = project_result(&listing, &resolved, &canonical_value(&input)?)
        .map_err(|error| format!("generic projection: {error:?}"))?;
    assert_eq!(
        serde_json::from_slice::<Value>(&result.canonical_bytes()?)?,
        json!({"status":"summary","summary":{"schema":"example/social-puzzle-result/v1","verdict":"pass","points":7}}),
    );
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

#[test]
fn roster_options_derive_only_exact_selected_seats_and_assignments() -> Result<(), Box<dyn Error>> {
    let source =
        include_bytes!("../../../tests/fixtures/hosted-contract/valid/roster-options-listing.json");
    let listing = ListingRevision::from_canonical_bytes(&canonical(source)?)?;
    let agents = [
        HouseAgentRevision::from_canonical_bytes(&canonical(COOPERATIVE_HOUSE_AGENT)?)?,
        HouseAgentRevision::from_canonical_bytes(&canonical(SKEPTICAL_HOUSE_AGENT)?)?,
    ];
    let document = source_value(source)?;
    for option in document["launch_input_schema"]["roster_options"]
        .as_array()
        .ok_or("options")?
    {
        let mut launch = json!({"schema":"worldstream/launch-request/v2", "listing_revision_digest":listing.digest(),
            "inputs":{"roster_option":option["option_id"]}, "creator":{"participation":"seat","principal_reference":"seat:lead"}});
        let mut members = vec![
            json!({"seat_id":"lead","participation":"account_human","principal_reference":"seat:lead","display_name":"Lead"}),
        ];
        for assignment in option["house_agent_assignments"]
            .as_array()
            .ok_or("assignments")?
        {
            let agent = agents
                .iter()
                .find(|agent| assignment["house_agent_revision_digest"] == agent.digest())
                .ok_or("agent")?;
            let value: Value = serde_json::from_slice(agent.canonical_bytes())?;
            members.push(json!({"seat_id":assignment["seat_id"],"participation":"house_agent_fill",
                "principal_reference":format!("seat:{}", assignment["seat_id"].as_str().ok_or("seat")?),
                "display_name":value["display_name"],"house_agent_revision_digest":agent.digest(),
                "agent_profile":value["agent_profile"],"runner_template":value["runner_template"]}));
        }
        let mut roster = json!({"schema":"worldstream/frozen-roster/v1","listing_revision_digest":listing.digest(),"members":members});
        let setup = derive_room_setup_with_house_agents(
            &listing,
            &canonical_value(&launch)?,
            &canonical_value(&roster)?,
            &agents,
        )?;
        let setup: Value = serde_json::from_slice(&setup.canonical_bytes()?)?;
        assert_eq!(setup["configuration"], option["configuration"]);
        assert_eq!(
            setup["seats"]
                .as_array()
                .ok_or("seats")?
                .iter()
                .map(|seat| seat["label"].clone())
                .collect::<Vec<_>>(),
            *option["seat_ids"].as_array().ok_or("seat_ids")?
        );
        if members.len() > 1 {
            launch["inputs"]["roster_option"] = json!("solo");
            assert!(
                derive_room_setup_with_house_agents(
                    &listing,
                    &canonical_value(&launch)?,
                    &canonical_value(&roster)?,
                    &agents
                )
                .is_err()
            );
            launch["inputs"]["roster_option"] = option["option_id"].clone();
            roster["members"].as_array_mut().ok_or("members")?.pop();
            assert!(
                derive_room_setup_with_house_agents(
                    &listing,
                    &canonical_value(&launch)?,
                    &canonical_value(&roster)?,
                    &agents
                )
                .is_err()
            );
        }
        for field in [
            "role",
            "principal_id",
            "prompt",
            "provider",
            "model",
            "code",
            "configuration",
            "setup",
        ] {
            let mut injected = launch.clone();
            injected["inputs"][field] = json!("arbitrary");
            assert!(
                derive_room_setup_with_house_agents(
                    &listing,
                    &canonical_value(&injected)?,
                    &canonical_value(&roster)?,
                    &agents
                )
                .is_err()
            );
        }
    }
    Ok(())
}

#[test]
fn roster_options_reject_missing_required_seats_and_unknown_defaults() -> Result<(), Box<dyn Error>>
{
    let document = source_value(include_bytes!(
        "../../../tests/fixtures/hosted-contract/valid/roster-options-listing.json"
    ))?;
    let mut invalid = document.clone();
    invalid["launch_input_schema"]["roster_options"][0]["seat_ids"] = json!(["mira"]);
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&invalid)?).is_err());
    let mut invalid = document;
    invalid["launch_input_schema"]["defaults"]["roster_option"] = json!("unknown");
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&invalid)?).is_err());
    Ok(())
}

#[test]
fn roster_v3_requires_bounded_descriptions_and_retained_v2_stays_closed()
-> Result<(), Box<dyn Error>> {
    let document = source_value(include_bytes!(
        "../../../tests/fixtures/hosted-contract/valid/roster-options-listing.json"
    ))?;
    ListingRevision::from_canonical_bytes(&canonical_value(&document)?)?;

    let mut described = document.clone();
    described["launch_input_schema"]["schema"] = json!("worldstream/launch-input-schema/v3");
    for option in described["launch_input_schema"]["roster_options"]
        .as_array_mut()
        .ok_or("options")?
    {
        option["description"] = json!(format!(
            "Reviewed formation description for {}.",
            option["label"].as_str().ok_or("label")?
        ));
    }
    ListingRevision::from_canonical_bytes(&canonical_value(&described)?)?;

    let mut invalid_v2 = document.clone();
    invalid_v2["launch_input_schema"]["roster_options"][1]["description"] = json!("v2 is closed");
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&invalid_v2)?).is_err());

    let mut null_v2 = document.clone();
    null_v2["launch_input_schema"]["roster_options"][1]["description"] = Value::Null;
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&null_v2)?).is_err());

    let mut missing = described.clone();
    missing["launch_input_schema"]["roster_options"][0]
        .as_object_mut()
        .ok_or("option")?
        .remove("description");
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&missing)?).is_err());

    let mut null_v3 = described.clone();
    null_v3["launch_input_schema"]["roster_options"][1]["description"] = Value::Null;
    assert!(ListingRevision::from_canonical_bytes(&canonical_value(&null_v3)?).is_err());

    for description in [String::new(), "x".repeat(257)] {
        let mut invalid = described.clone();
        invalid["launch_input_schema"]["roster_options"][1]["description"] = json!(description);
        assert!(ListingRevision::from_canonical_bytes(&canonical_value(&invalid)?).is_err());
    }
    Ok(())
}
