use std::{error::Error, io};

use ring::{
    rand::SystemRandom,
    signature::{self, EcdsaKeyPair, KeyPair as _},
};
use serde_json::{Value, json};
use worldstream_a202_adapter::{
    A202_COMMERCIAL_MEDIA_TYPE, A202AdapterV1, CrossIndexEntryV1, ExactA202ObjectV1,
    ExpectedA202ObjectV1, ExpectedSignatureV1, KeyStatusV1, LogicalA202HeadV1,
    NegotiationProofPackageV1, OpaqueA202InputV1, OperatedSessionBindingV1, PackBundleReferenceV1,
    PartyProtocolProofV1, ProfilePinsV1, ReplayEvidenceV1, ResolvedPublicKeyV1, RoomHeadWitnessV1,
    VenueRecordV1, VenueRuntimeProofV1, VerificationOutcomeV1, a202_content_hash,
    a202_signature_message, canonical_bytes, encode_base64url, exact_blake3_digest,
};
use worldstream_negotiate_evidence::{
    CheckResultV1, TrustedKeyV1, VerifierTrustV1, verify_package,
};

const SIGNED_AT: &str = "2026-08-30T12:00:00Z";
const TRANSACTION_ID: &str = "txn_worldstream_a202_002";

struct Fixture {
    package: Vec<u8>,
    pack_bundle: Vec<u8>,
    trust: VerifierTrustV1,
}

struct SignedObjectFixture {
    object: ExactA202ObjectV1,
    key: ResolvedPublicKeyV1,
    public_key: String,
}

fn signed_object_fixture() -> Result<SignedObjectFixture, Box<dyn Error>> {
    let random = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random)
        .map_err(|_| io::Error::other("could not generate evidence test key"))?;
    let key_pair = EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &random,
    )
    .map_err(|_| io::Error::other("could not parse evidence test key"))?;
    let public_key = encode_base64url(key_pair.public_key().as_ref());
    let key = ResolvedPublicKeyV1 {
        key_id: "key_approver_002".to_owned(),
        subject_id: "agt_approver_002".to_owned(),
        public_key_sec1_base64url: public_key.clone(),
        status_at_signing_time: KeyStatusV1::Active,
        status_at: SIGNED_AT.to_owned(),
        current_status: KeyStatusV1::Active,
        status_evidence_ids: Vec::new(),
    };
    let mut object = json!({
        "content_hash": "",
        "created_at": SIGNED_AT,
        "created_by": {
            "agent_id": "agt_approver_002",
            "mandate_id": "mnd_approver_002",
            "organization_id": "org_buyer_002"
        },
        "id": "apr_worldstream_a202_002",
        "object_type": "approval",
        "payload": {
            "action_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "approver": "prn_approver_002",
            "conditions": [],
            "decision": "approved",
            "expires_at": "2026-08-30T12:05:00Z",
            "role": "procurement_director"
        },
        "previous_version_id": null,
        "signatures": [],
        "spec_version": "a202-commercial/0.1",
        "transaction_id": TRANSACTION_ID,
        "version": 1
    });
    let content_hash = a202_content_hash(&object)?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("approval fixture is not an object").into());
    };
    map.insert("content_hash".to_owned(), Value::String(content_hash));
    let message =
        a202_signature_message(&object, &key.key_id, "ES256", "object_issuance", SIGNED_AT)?;
    let signed = key_pair
        .sign(&random, &message)
        .map_err(|_| io::Error::other("could not sign evidence test object"))?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("approval fixture is not an object").into());
    };
    map.insert(
        "signatures".to_owned(),
        json!([{
            "algorithm": "ES256",
            "key_id": key.key_id,
            "purpose": "object_issuance",
            "signature": encode_base64url(signed.as_ref()),
            "signed_at": SIGNED_AT
        }]),
    );
    let exact_bytes = canonical_bytes(&object)?;
    let adapter = A202AdapterV1::new(
        ProfilePinsV1::default(),
        OperatedSessionBindingV1 {
            transaction_id: TRANSACTION_ID.to_owned(),
            session_id: "ses_worldstream_a202_002".to_owned(),
        },
        [key.clone()],
    )?;
    let object = adapter.ingest_object(
        OpaqueA202InputV1 {
            media_type: A202_COMMERCIAL_MEDIA_TYPE.to_owned(),
            wire_digest: exact_blake3_digest(&exact_bytes),
            exact_bytes,
        },
        &ExpectedA202ObjectV1 {
            object_type: "approval".to_owned(),
            transaction_id: TRANSACTION_ID.to_owned(),
            signatures: vec![ExpectedSignatureV1 {
                key_id: key.key_id.clone(),
                subject_id: key.subject_id.clone(),
                purpose: "object_issuance".to_owned(),
                require_current_active: true,
            }],
        },
    )?;
    Ok(SignedObjectFixture {
        object,
        key,
        public_key,
    })
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let signed = signed_object_fixture()?;
    let action_bytes = br#"{"action_id":"action-room-002-1","kind":"action"}"#.to_vec();
    let transition_bytes = br#"{"kind":"transition","room_sequence":1}"#.to_vec();
    let replay_bytes = br#"{"room_sequence":1,"status":"verified"}"#.to_vec();
    let pack_bundle = b"fixture-wspack-exact-bytes".to_vec();
    let transition_id = "transition-room-002-1".to_owned();
    let package = NegotiationProofPackageV1::new(
        PartyProtocolProofV1 {
            transaction_id: TRANSACTION_ID.to_owned(),
            session_id: "ses_worldstream_a202_002".to_owned(),
            transaction_head: LogicalA202HeadV1 {
                sequence: 1,
                event_hash: "sha256:transaction-head-1".to_owned(),
            },
            session_head: LogicalA202HeadV1 {
                sequence: 1,
                event_hash: "sha256:session-head-1".to_owned(),
            },
            exact_objects: vec![signed.object.clone()],
            key_resolution_evidence: vec![signed.key.clone()],
            resolver_evidence: Vec::new(),
        },
        VenueRuntimeProofV1 {
            room_id: "room-worldstream-a202-002".to_owned(),
            genesis_hash: "blake3:genesis-002".to_owned(),
            pack: PackBundleReferenceV1 {
                pack_id: "worldstream.negotiate".to_owned(),
                revision_digest: "blake3:negotiate-revision-002".to_owned(),
                physical_bundle_digest: exact_blake3_digest(&pack_bundle),
            },
            final_room_head: RoomHeadWitnessV1 {
                sequence: 1,
                digest: "blake3:room-head-002-1".to_owned(),
            },
            records: vec![
                VenueRecordV1 {
                    record_kind: "action".to_owned(),
                    record_id: "action-room-002-1".to_owned(),
                    room_sequence: 1,
                    exact_bytes_base64url: encode_base64url(&action_bytes),
                    byte_digest: exact_blake3_digest(&action_bytes),
                },
                VenueRecordV1 {
                    record_kind: "transition".to_owned(),
                    record_id: transition_id.clone(),
                    room_sequence: 1,
                    exact_bytes_base64url: encode_base64url(&transition_bytes),
                    byte_digest: exact_blake3_digest(&transition_bytes),
                },
            ],
            replay: ReplayEvidenceV1 {
                declared_result: VerificationOutcomeV1::Verified,
                exact_report_base64url: encode_base64url(&replay_bytes),
                report_digest: exact_blake3_digest(&replay_bytes),
            },
            cross_index: vec![CrossIndexEntryV1 {
                a202_object_id: signed.object.object_id.clone(),
                a202_wire_digest: signed.object.wire_digest.clone(),
                worldstream_action_id: "action-room-002-1".to_owned(),
                worldstream_transition_record_id: transition_id,
                room_sequence: 1,
                room_digest: "blake3:room-head-002-1".to_owned(),
            }],
        },
    )?;
    Ok(Fixture {
        package: package.to_json_bytes()?,
        pack_bundle,
        trust: VerifierTrustV1 {
            a202_keys: vec![TrustedKeyV1 {
                key_id: signed.key.key_id,
                subject_id: signed.key.subject_id,
                public_key_sec1_base64url: signed.public_key,
            }],
            resolver_sources: Vec::new(),
        },
    })
}

#[test]
fn independent_offline_verifier_checks_both_proof_sections() -> Result<(), Box<dyn Error>> {
    let fixture = fixture()?;
    let report = verify_package(&fixture.package, &fixture.trust, Some(&fixture.pack_bundle))?;
    assert!(
        report
            .checks
            .iter()
            .all(|check| check.result != CheckResultV1::Failed)
    );
    assert!(report.checks.iter().any(|check| {
        check.check == "a202_signature" && check.result == CheckResultV1::Verified
    }));
    assert!(report.checks.iter().any(|check| {
        check.check == "activity_pack_bundle" && check.result == CheckResultV1::Verified
    }));
    assert!(report.checks.iter().any(|check| {
        check.check == "canonical_room_replay" && check.result == CheckResultV1::NotCheckable
    }));
    let encoded = serde_json::to_value(&report)?;
    assert!(encoded.get("overall").is_none());
    assert!(encoded.get("passed").is_none());
    Ok(())
}

#[test]
fn mutation_wrong_signer_and_absent_trust_are_distinct_results() -> Result<(), Box<dyn Error>> {
    let fixture = fixture()?;
    let mut mutated: Value = serde_json::from_slice(&fixture.package)?;
    let Some(bytes) =
        mutated.pointer_mut("/party_protocol_proof/exact_objects/0/exact_bytes_base64url")
    else {
        return Err(io::Error::other("missing exact object bytes").into());
    };
    *bytes = Value::String(encode_base64url(br#"{"mutated":true}"#));
    let Some(media_type) = mutated.pointer_mut("/party_protocol_proof/exact_objects/0/media_type")
    else {
        return Err(io::Error::other("missing exact object media type").into());
    };
    *media_type = Value::String("application/json".to_owned());
    let Some(action_id) =
        mutated.pointer_mut("/venue_runtime_proof/cross_index/0/worldstream_action_id")
    else {
        return Err(io::Error::other("missing cross-index action id").into());
    };
    *action_id = Value::String("action-not-exported".to_owned());
    let mutation_report = verify_package(
        &serde_json::to_vec(&mutated)?,
        &fixture.trust,
        Some(&fixture.pack_bundle),
    )?;
    assert!(mutation_report.checks.iter().any(|check| {
        check.check == "exact_a202_wire_digest" && check.result == CheckResultV1::Failed
    }));
    assert!(mutation_report.checks.iter().any(|check| {
        check.check == "a202_media_type" && check.result == CheckResultV1::Failed
    }));
    assert!(mutation_report.checks.iter().any(|check| {
        check.check == "protocol_to_venue_binding" && check.result == CheckResultV1::Failed
    }));

    let mut wrong_trust = fixture.trust.clone();
    let Some(key) = wrong_trust.a202_keys.first_mut() else {
        return Err(io::Error::other("missing trust key").into());
    };
    key.subject_id = "wrong".to_owned();
    let wrong_signer = verify_package(&fixture.package, &wrong_trust, Some(&fixture.pack_bundle))?;
    assert!(wrong_signer.checks.iter().any(|check| {
        check.check == "signature_signer_binding" && check.result == CheckResultV1::Failed
    }));

    let no_trust = verify_package(
        &fixture.package,
        &VerifierTrustV1::default(),
        Some(&fixture.pack_bundle),
    )?;
    assert!(no_trust.checks.iter().any(|check| {
        check.check == "a202_signature" && check.result == CheckResultV1::NotCheckable
    }));
    Ok(())
}
