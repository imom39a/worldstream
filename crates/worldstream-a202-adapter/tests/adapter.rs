use std::{error::Error, io};

use ring::{
    rand::SystemRandom,
    signature::{self, EcdsaKeyPair, KeyPair as _},
};
use serde_json::{Value, json};
use worldstream_a202_adapter::{
    A202_COMMERCIAL_MEDIA_TYPE, A202AdapterV1, AdapterError, DetachedEs256SignatureV1,
    ExpectedA202ObjectV1, ExpectedSignatureV1, HostValidityResultV1, KeyStatusV1,
    LogicalA202HeadV1, OpaqueA202InputV1, OperatedSessionBindingV1, ProfilePinsV1,
    ResolvedPublicKeyV1, ResolverEvidencePolicyV1, RetryDecisionV1, RoomHeadWitnessV1,
    SubmissionBasisV1, UnsignedResolverEvidenceV1, a202_content_hash, a202_signature_message,
    canonical_bytes, encode_base64url, exact_blake3_digest, resolver_attestation_message,
};

const SIGNED_AT: &str = "2026-08-30T12:00:00Z";
const TRANSACTION_ID: &str = "txn_worldstream_a202_001";

struct SigningFixture {
    key_pair: EcdsaKeyPair,
    key: ResolvedPublicKeyV1,
}

fn signing_fixture(key_id: &str, subject_id: &str) -> Result<SigningFixture, Box<dyn Error>> {
    let random = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random)
        .map_err(|_| io::Error::other("could not generate test key"))?;
    let key_pair = EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &random,
    )
    .map_err(|_| io::Error::other("could not parse test key"))?;
    let key = ResolvedPublicKeyV1 {
        key_id: key_id.to_owned(),
        subject_id: subject_id.to_owned(),
        public_key_sec1_base64url: encode_base64url(key_pair.public_key().as_ref()),
        status_at_signing_time: KeyStatusV1::Active,
        status_at: SIGNED_AT.to_owned(),
        current_status: KeyStatusV1::Active,
        status_evidence_ids: vec!["resolver-key-status-1".to_owned()],
    };
    Ok(SigningFixture { key_pair, key })
}

fn sign_offer(fixture: &SigningFixture) -> Result<(Value, Vec<u8>), Box<dyn Error>> {
    let mut object = json!({
        "content_hash": "",
        "created_at": SIGNED_AT,
        "created_by": {
            "agent_id": fixture.key.subject_id,
            "mandate_id": "mnd_worldstream_a202_001",
            "organization_id": "org_worldstream_buyer_001"
        },
        "id": "off_worldstream_a202_001",
        "object_type": "offer",
        "payload": {
            "evidence_refs": [],
            "offeree": "org_worldstream_seller_001",
            "offeror": "org_worldstream_buyer_001",
            "session_id": "ses_worldstream_a202_001",
            "supersedes_offer_id": null,
            "terms": {
                "core": {
                    "description": "Calibrate 20 pressure transmitters",
                    "quantity": "20",
                    "total": {"amount": "3200.00", "currency": "EUR"},
                    "unit_code": "H87",
                    "unit_name": "piece"
                },
                "profile": "a202-profile/calibration-service/0.1",
                "profile_terms": {
                    "acceptance": {
                        "certificate_required": true,
                        "machine_readable_result_required": true,
                        "qualification_standard": "ISO/IEC 17025:2017"
                    },
                    "completion": {"business_calendar": "NL", "business_days_after_collection": 15},
                    "payment": {"balance_trigger": "buyer_acceptance", "prepayment_percent": "20"},
                    "rework": {"included_attempts": 1}
                }
            },
            "valid_until": "2026-09-30T12:00:00Z"
        },
        "previous_version_id": null,
        "signatures": [],
        "spec_version": "a202-commercial/0.1",
        "transaction_id": TRANSACTION_ID,
        "version": 1
    });
    let content_hash = a202_content_hash(&object)?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("offer fixture is not an object").into());
    };
    map.insert("content_hash".to_owned(), Value::String(content_hash));
    let message = a202_signature_message(
        &object,
        &fixture.key.key_id,
        "ES256",
        "offer_submission",
        SIGNED_AT,
    )?;
    let random = SystemRandom::new();
    let signature = fixture
        .key_pair
        .sign(&random, &message)
        .map_err(|_| io::Error::other("could not sign offer fixture"))?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("offer fixture is not an object").into());
    };
    map.insert(
        "signatures".to_owned(),
        json!([{
            "algorithm": "ES256",
            "key_id": fixture.key.key_id,
            "purpose": "offer_submission",
            "signature": encode_base64url(signature.as_ref()),
            "signed_at": SIGNED_AT
        }]),
    );
    let bytes = canonical_bytes(&object)?;
    Ok((object, bytes))
}

fn sign_action_envelope(
    fixture: &SigningFixture,
    expected_sequence: u64,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut object = json!({
        "content_hash": "",
        "created_at": SIGNED_AT,
        "created_by": {
            "agent_id": fixture.key.subject_id,
            "mandate_id": "mnd_worldstream_a202_001",
            "organization_id": "org_worldstream_buyer_001"
        },
        "id": "act_worldstream_a202_001",
        "object_type": "action_envelope",
        "payload": {
            "action_type": "offer.submit",
            "expected_sequence": expected_sequence,
            "idempotency_key": "worldstream-a202-action-001",
            "proposed_object": {},
            "session_id": "ses_worldstream_a202_001"
        },
        "previous_version_id": null,
        "signatures": [],
        "spec_version": "a202-commercial/0.1",
        "transaction_id": TRANSACTION_ID,
        "version": 1
    });
    let content_hash = a202_content_hash(&object)?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("ActionEnvelope fixture is not an object").into());
    };
    map.insert("content_hash".to_owned(), Value::String(content_hash));
    let message = a202_signature_message(
        &object,
        &fixture.key.key_id,
        "ES256",
        "action_submission",
        SIGNED_AT,
    )?;
    let random = SystemRandom::new();
    let signature = fixture
        .key_pair
        .sign(&random, &message)
        .map_err(|_| io::Error::other("could not sign ActionEnvelope fixture"))?;
    let Some(map) = object.as_object_mut() else {
        return Err(io::Error::other("ActionEnvelope fixture is not an object").into());
    };
    map.insert(
        "signatures".to_owned(),
        json!([{
            "algorithm": "ES256",
            "key_id": fixture.key.key_id,
            "purpose": "action_submission",
            "signature": encode_base64url(signature.as_ref()),
            "signed_at": SIGNED_AT
        }]),
    );
    Ok(canonical_bytes(&object)?)
}

fn expected(key_id: &str, subject_id: &str) -> ExpectedA202ObjectV1 {
    ExpectedA202ObjectV1 {
        object_type: "offer".to_owned(),
        transaction_id: TRANSACTION_ID.to_owned(),
        signatures: vec![ExpectedSignatureV1 {
            key_id: key_id.to_owned(),
            subject_id: subject_id.to_owned(),
            purpose: "offer_submission".to_owned(),
            require_current_active: true,
        }],
    }
}

fn expected_action(key_id: &str, subject_id: &str) -> ExpectedA202ObjectV1 {
    ExpectedA202ObjectV1 {
        object_type: "action_envelope".to_owned(),
        transaction_id: TRANSACTION_ID.to_owned(),
        signatures: vec![ExpectedSignatureV1 {
            key_id: key_id.to_owned(),
            subject_id: subject_id.to_owned(),
            purpose: "action_submission".to_owned(),
            require_current_active: true,
        }],
    }
}

fn input(bytes: Vec<u8>) -> OpaqueA202InputV1 {
    OpaqueA202InputV1 {
        media_type: A202_COMMERCIAL_MEDIA_TYPE.to_owned(),
        wire_digest: exact_blake3_digest(&bytes),
        exact_bytes: bytes,
    }
}

fn session_binding() -> OperatedSessionBindingV1 {
    OperatedSessionBindingV1 {
        transaction_id: TRANSACTION_ID.to_owned(),
        session_id: "ses_worldstream_a202_001".to_owned(),
    }
}

fn basis(
    room_sequence: u64,
    transaction_sequence: u64,
    session_sequence: u64,
) -> SubmissionBasisV1 {
    SubmissionBasisV1 {
        room: RoomHeadWitnessV1 {
            sequence: room_sequence,
            digest: format!("blake3:room-{room_sequence}"),
        },
        transaction: LogicalA202HeadV1 {
            sequence: transaction_sequence,
            event_hash: format!("sha256:transaction-{transaction_sequence}"),
        },
        session: LogicalA202HeadV1 {
            sequence: session_sequence,
            event_hash: format!("sha256:session-{session_sequence}"),
        },
    }
}

#[test]
fn exact_signed_object_is_verified_without_reserialization() -> Result<(), Box<dyn Error>> {
    let fixture = signing_fixture("key_buyer_001", "agt_buyer_001")?;
    let (_, bytes) = sign_offer(&fixture)?;
    let adapter = A202AdapterV1::new(
        ProfilePinsV1::default(),
        session_binding(),
        [fixture.key.clone()],
    )?;
    let object = adapter.ingest_object(
        input(bytes.clone()),
        &expected("key_buyer_001", "agt_buyer_001"),
    )?;

    assert_eq!(object.exact_bytes_base64url, encode_base64url(&bytes));
    assert_eq!(object.wire_digest, exact_blake3_digest(&bytes));
    assert_eq!(object.signature_verifications.len(), 1);
    Ok(())
}

#[test]
fn mutation_wrong_signer_and_missing_key_fail_closed() -> Result<(), Box<dyn Error>> {
    let fixture = signing_fixture("key_buyer_001", "agt_buyer_001")?;
    let (mut object, _) = sign_offer(&fixture)?;
    let Some(amount) = object.pointer_mut("/payload/terms/core/total/amount") else {
        return Err(io::Error::other("missing amount fixture member").into());
    };
    *amount = Value::String("3199.99".to_owned());
    let mutated = canonical_bytes(&object)?;
    let adapter = A202AdapterV1::new(
        ProfilePinsV1::default(),
        session_binding(),
        [fixture.key.clone()],
    )?;
    assert!(matches!(
        adapter.ingest_object(input(mutated), &expected("key_buyer_001", "agt_buyer_001")),
        Err(AdapterError::ContentHashMismatch)
    ));

    let (_, valid) = sign_offer(&fixture)?;
    assert!(matches!(
        adapter.ingest_object(
            input(valid.clone()),
            &expected("key_seller_001", "agt_seller_001")
        ),
        Err(AdapterError::WrongSigner)
    ));
    let no_keys = A202AdapterV1::new(ProfilePinsV1::default(), session_binding(), [])?;
    assert!(matches!(
        no_keys.ingest_object(input(valid), &expected("key_buyer_001", "agt_buyer_001")),
        Err(AdapterError::SignatureNotCheckable)
    ));
    Ok(())
}

#[test]
fn room_head_only_retry_reuses_bytes_but_a202_head_change_requires_resigning()
-> Result<(), Box<dyn Error>> {
    let fixture = signing_fixture("key_buyer_001", "agt_buyer_001")?;
    let bytes = sign_action_envelope(&fixture, 7)?;
    let adapter = A202AdapterV1::new(
        ProfilePinsV1::default(),
        session_binding(),
        [fixture.key.clone()],
    )?;
    let object = adapter.ingest_object(
        input(bytes),
        &expected_action("key_buyer_001", "agt_buyer_001"),
    )?;
    let prepared = A202AdapterV1::prepare_action_submission(object.clone(), basis(10, 3, 7))?;

    let RetryDecisionV1::ReuseExactBytes { submission } = prepared.retry_against(basis(11, 3, 7))
    else {
        return Err(io::Error::other("Room-only retry did not reuse exact bytes").into());
    };
    assert_eq!(submission.object, object);
    assert_eq!(submission.basis.room.sequence, 11);

    assert!(matches!(
        prepared.retry_against(basis(11, 4, 7)),
        RetryDecisionV1::RebuildAndResign {
            transaction_head_changed: true,
            session_head_changed: false
        }
    ));
    assert!(matches!(
        prepared.retry_against(basis(11, 3, 8)),
        RetryDecisionV1::RebuildAndResign {
            transaction_head_changed: false,
            session_head_changed: true
        }
    ));
    assert!(matches!(
        A202AdapterV1::prepare_action_submission(object, basis(10, 3, 8)),
        Err(AdapterError::A202ExpectedSequenceMismatch)
    ));
    Ok(())
}

#[test]
fn resolver_evidence_requires_authenticated_exact_response() -> Result<(), Box<dyn Error>> {
    let fixture = signing_fixture("key_resolver_001", "host_source_001")?;
    let response = br#"{"status":"active","subject":"mnd_worldstream_a202_001"}"#;
    let observation = UnsignedResolverEvidenceV1 {
        evidence_id: "resolver-evidence-001".to_owned(),
        source_id: "host-source-001".to_owned(),
        subject: "mnd_worldstream_a202_001".to_owned(),
        observed_at_unix_ms: 1_000,
        valid_until_unix_ms: 61_000,
        media_type: "application/json".to_owned(),
        exact_response_base64url: encode_base64url(response),
        response_digest: exact_blake3_digest(response),
        host_validity_result: HostValidityResultV1::Active,
    };
    let mut attestation = DetachedEs256SignatureV1 {
        key_id: fixture.key.key_id.clone(),
        algorithm: "ES256".to_owned(),
        purpose: "resolver_evidence_authentication".to_owned(),
        signed_at: SIGNED_AT.to_owned(),
        signature_base64url: String::new(),
    };
    let message = resolver_attestation_message(&observation, &attestation)?;
    let random = SystemRandom::new();
    let signature = fixture
        .key_pair
        .sign(&random, &message)
        .map_err(|_| io::Error::other("could not sign resolver fixture"))?;
    attestation.signature_base64url = encode_base64url(signature.as_ref());
    let policy = ResolverEvidencePolicyV1 {
        source_id: observation.source_id.clone(),
        attestation_key_id: fixture.key.key_id.clone(),
        attestation_public_key_sec1_base64url: fixture.key.public_key_sec1_base64url.clone(),
        maximum_validity_ms: 60_000,
    };
    let adapter = A202AdapterV1::new(ProfilePinsV1::default(), session_binding(), [])?;
    let verified =
        adapter.ingest_resolver_evidence(observation.clone(), attestation.clone(), &policy)?;
    verified.require_active_at(2_000)?;
    assert!(matches!(
        verified.require_active_at(61_000),
        Err(AdapterError::ResolverEvidenceStale)
    ));

    let mut wrong_attestation = attestation.clone();
    wrong_attestation.signature_base64url = encode_base64url(&[0; 64]);
    assert!(matches!(
        adapter.ingest_resolver_evidence(observation.clone(), wrong_attestation, &policy),
        Err(AdapterError::ResolverAuthenticationFailed)
    ));

    let mut mutated = observation;
    mutated.exact_response_base64url = encode_base64url(br#"{"status":"revoked"}"#);
    assert!(matches!(
        adapter.ingest_resolver_evidence(mutated, attestation, &policy),
        Err(AdapterError::ResolverResponseDigestMismatch)
    ));
    Ok(())
}
