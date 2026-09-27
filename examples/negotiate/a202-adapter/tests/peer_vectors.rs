use std::{error::Error, fs, io, path::PathBuf};

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use worldstream_a202_adapter::{
    A202AdapterV1, AdapterError, ExpectedA202ObjectV1, ExpectedSignatureV1, KeyStatusV1,
    OpaqueA202InputV1, OperatedSessionBindingV1, ProfilePinsV1, ResolvedPublicKeyV1,
    RetryDecisionV1, SubmissionBasisV1, a202_signature_message, canonical_content_bytes,
    decode_base64url, exact_blake3_digest,
};

fn invitation() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../interop/exchange.json");
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn row<'a>(value: &'a Value, collection: &str, id: &str) -> Result<&'a Value, Box<dyn Error>> {
    value[collection]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["id"] == id || row["case_id"] == id)
        })
        .ok_or_else(|| io::Error::other(format!("missing {collection} row {id}")).into())
}

fn object_bytes(object: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(decode_base64url(
        object["exact_bytes_base64url"]
            .as_str()
            .ok_or_else(|| io::Error::other("object has no exact bytes"))?,
    )?)
}

fn expected(object: &Value) -> Result<ExpectedA202ObjectV1, Box<dyn Error>> {
    Ok(ExpectedA202ObjectV1 {
        object_type: object["object_type"]
            .as_str()
            .ok_or_else(|| io::Error::other("object has no type"))?
            .to_owned(),
        transaction_id: "txn_peer_001".to_owned(),
        signatures: vec![ExpectedSignatureV1 {
            key_id: object["expected_signature"]["key_id"]
                .as_str()
                .ok_or_else(|| io::Error::other("object has no signature key"))?
                .to_owned(),
            subject_id: object["expected_signature"]["subject_id"]
                .as_str()
                .ok_or_else(|| io::Error::other("object has no signature subject"))?
                .to_owned(),
            purpose: object["expected_signature"]["purpose"]
                .as_str()
                .ok_or_else(|| io::Error::other("object has no signature purpose"))?
                .to_owned(),
            require_current_active: true,
        }],
    })
}

fn adapter(value: &Value, with_key: bool) -> Result<A202AdapterV1, AdapterError> {
    let keys = if with_key {
        vec![ResolvedPublicKeyV1 {
            key_id: value["verification_key"]["key_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            subject_id: value["verification_key"]["subject_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            public_key_sec1_base64url: value["verification_key"]["public_key_sec1_base64url"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            status_at_signing_time: KeyStatusV1::Active,
            status_at: value["verification_key"]["status_at"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            current_status: KeyStatusV1::Active,
            status_evidence_ids: vec!["peer-vector-key-status".to_owned()],
        }]
    } else {
        Vec::new()
    };
    A202AdapterV1::new(
        ProfilePinsV1::default(),
        OperatedSessionBindingV1 {
            transaction_id: "txn_peer_001".to_owned(),
            session_id: "ses_peer_001".to_owned(),
        },
        keys,
    )
}

fn opaque(object: &Value, media_type: &str) -> Result<OpaqueA202InputV1, Box<dyn Error>> {
    Ok(OpaqueA202InputV1 {
        media_type: media_type.to_owned(),
        exact_bytes: object_bytes(object)?,
        wire_digest: object["wire_digest"]
            .as_str()
            .ok_or_else(|| io::Error::other("object has no wire digest"))?
            .to_owned(),
    })
}

fn basis(case: &Value, name: &str) -> Result<SubmissionBasisV1, Box<dyn Error>> {
    Ok(serde_json::from_value(case["input"][name].clone())?)
}

#[test]
fn language_neutral_exact_bytes_match_all_declared_hashes() -> Result<(), Box<dyn Error>> {
    let value = invitation()?;
    for object in value["objects"]
        .as_array()
        .ok_or_else(|| io::Error::other("invitation has no objects"))?
    {
        let bytes = object_bytes(object)?;
        assert_eq!(
            format!("sha256:{:x}", Sha256::digest(&bytes)),
            object["exact_bytes_sha256"].as_str().unwrap_or_default()
        );
        assert_eq!(
            exact_blake3_digest(&bytes),
            object["wire_digest"].as_str().unwrap_or_default(),
            "wire digest for {}",
            object["id"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

#[test]
fn reference_material_is_independently_recomputable() -> Result<(), Box<dyn Error>> {
    let value = invitation()?;
    for reference in value["reference_material"]
        .as_array()
        .ok_or_else(|| io::Error::other("invitation has no reference material"))?
    {
        let object = row(
            &value,
            "objects",
            reference["object_id"].as_str().unwrap_or_default(),
        )?;
        let bytes = object_bytes(object)?;
        let parsed: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            canonical_content_bytes(&parsed)?,
            decode_base64url(
                reference["canonical_content_bytes_base64url"]
                    .as_str()
                    .unwrap_or_default()
            )?
        );
        let signature = &parsed["signatures"][0];
        assert_eq!(
            a202_signature_message(
                &parsed,
                signature["key_id"].as_str().unwrap_or_default(),
                signature["algorithm"].as_str().unwrap_or_default(),
                signature["purpose"].as_str().unwrap_or_default(),
                signature["signed_at"].as_str().unwrap_or_default(),
            )?,
            decode_base64url(
                reference["signature_message_base64url"]
                    .as_str()
                    .unwrap_or_default()
            )?
        );
    }
    Ok(())
}

#[test]
fn verified_failed_and_not_checkable_cases_are_distinct() -> Result<(), Box<dyn Error>> {
    let value = invitation()?;
    let valid = row(&value, "objects", "offer_valid")?;
    adapter(&value, true)?.ingest_object(
        opaque(valid, "application/a202-commercial+json")?,
        &expected(valid)?,
    )?;

    let tampered = row(&value, "objects", "offer_tampered")?;
    assert!(matches!(
        adapter(&value, true)?.ingest_object(
            opaque(tampered, "application/a202-commercial+json")?,
            &expected(tampered)?,
        ),
        Err(AdapterError::ContentHashMismatch)
    ));
    assert!(matches!(
        adapter(&value, false)?.ingest_object(
            opaque(valid, "application/a202-commercial+json")?,
            &expected(valid)?,
        ),
        Err(AdapterError::SignatureNotCheckable)
    ));
    assert!(matches!(
        adapter(&value, true)?.ingest_object(opaque(valid, "application/json")?, &expected(valid)?),
        Err(AdapterError::InvalidMediaType)
    ));
    Ok(())
}

#[test]
fn retry_vectors_keep_room_head_outside_signed_bytes() -> Result<(), Box<dyn Error>> {
    let value = invitation()?;
    let action = row(&value, "objects", "action_valid")?;
    let verified = adapter(&value, true)?.ingest_object(
        opaque(action, "application/a202-commercial+json")?,
        &expected(action)?,
    )?;
    let initial = row(&value, "cases", "action_initial_head")?;
    let prepared =
        A202AdapterV1::prepare_action_submission(verified, basis(initial, "submission_basis")?)?;

    let room_only = row(&value, "cases", "retry_room_head_only")?;
    let RetryDecisionV1::ReuseExactBytes { submission } =
        prepared.retry_against(basis(room_only, "observed_basis")?)
    else {
        return Err(io::Error::other("Room-only retry did not reuse exact bytes").into());
    };
    assert_eq!(
        submission.object.exact_bytes_base64url,
        action["exact_bytes_base64url"]
    );
    assert_eq!(submission.basis.room.sequence, 11);

    let transaction = row(&value, "cases", "retry_transaction_head_changed")?;
    assert!(matches!(
        prepared.retry_against(basis(transaction, "observed_basis")?),
        RetryDecisionV1::RebuildAndResign {
            transaction_head_changed: true,
            session_head_changed: false
        }
    ));
    let session = row(&value, "cases", "retry_session_head_changed")?;
    assert!(matches!(
        prepared.retry_against(basis(session, "observed_basis")?),
        RetryDecisionV1::RebuildAndResign {
            transaction_head_changed: false,
            session_head_changed: true
        }
    ));
    Ok(())
}
