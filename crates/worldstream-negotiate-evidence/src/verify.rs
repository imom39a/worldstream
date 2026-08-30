use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature;
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{
    canonical::{content_hash, parse_and_check_exact, serialize, signature_message},
    model::{
        CheckResultV1, EvidenceCheckV1, EvidenceScopeV1, EvidenceSectionV1,
        NegotiationVerificationReportV1, TrustedResolverSourceV1, VerifierTrustV1,
    },
};

pub const PROOF_PACKAGE_FORMAT_V1: &str = "worldstream/negotiate-proof-package/v1";
const REPORT_FORMAT_V1: &str = "worldstream/negotiate-verification-report/v1";
const A202_REVISION: &str = "fa85aa8b49bfe7b3f7ded487c98500a600e92e41";
const A202_SPEC: &str = "a202-commercial/0.1";
const A202_MEDIA_TYPE: &str = "application/a202-commercial+json";
const A202_RULES: &str = "1.3";
const A202_PROFILE: &str = "a202-profile/calibration-service/0.1";
const A202_OPERATED_SCOPE: &str = "a202-scope/operated/0.1";
const A202_BILATERAL_SCOPE: &str = "a202-scope/bilateral/0.1";

/// Verify the independently parseable evidence carried by a proof package.
///
/// The returned report deliberately has no aggregate success boolean; every
/// check is classified as verified, failed, or not checkable.
///
/// # Errors
///
/// Returns an error when the package is not JSON or lacks the minimum structural
/// shape needed to enumerate its checks.
pub fn verify_package(
    package_bytes: &[u8],
    trust: &VerifierTrustV1,
    referenced_pack_bundle: Option<&[u8]>,
) -> Result<NegotiationVerificationReportV1, VerificationError> {
    let package: Value = serde_json::from_slice(package_bytes)
        .map_err(|error| VerificationError::PackageJson(error.to_string()))?;
    let root = package.as_object().ok_or(VerificationError::PackageShape)?;
    let party = object_member(root, "party_protocol_proof")?;
    let venue = object_member(root, "venue_runtime_proof")?;
    let mut report = NegotiationVerificationReportV1 {
        report_format: REPORT_FORMAT_V1.to_owned(),
        profile_revision: string_at(&package, "/profile/repository_revision").unwrap_or_default(),
        scope: EvidenceScopeV1 {
            transaction_id: string_member(party, "transaction_id").unwrap_or_default(),
            session_id: string_member(party, "session_id").unwrap_or_default(),
            room_id: string_member(venue, "room_id").unwrap_or_default(),
            ..EvidenceScopeV1::default()
        },
        checks: Vec::new(),
    };

    check_profile(&package, &mut report);
    let resolver_results = check_resolver_evidence(party, trust, &mut report)?;
    check_protocol_objects(party, trust, &resolver_results, &mut report)?;
    check_venue(venue, referenced_pack_bundle, &mut report)?;
    check_cross_index(party, venue, &mut report)?;
    report.scope.objects.sort();
    report.scope.objects.dedup();
    report.scope.venue_records.sort();
    report.scope.venue_records.dedup();
    Ok(report)
}

fn check_profile(package: &Value, report: &mut NegotiationVerificationReportV1) {
    let format = string_at(package, "/format");
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "proof_package_format",
        "package",
        if format.as_deref() == Some(PROOF_PACKAGE_FORMAT_V1) {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("WORLDSTREAM-EVIDENCE-FORMAT-MISMATCH"),
        "exact proof-package format",
    );
    let expected = [
        ("/profile/repository_revision", A202_REVISION),
        ("/profile/spec_version", A202_SPEC),
        ("/profile/rules_version", A202_RULES),
        ("/profile/transaction_profile", A202_PROFILE),
        ("/profile/operated_scope", A202_OPERATED_SCOPE),
        ("/profile/bilateral_scope", A202_BILATERAL_SCOPE),
    ];
    let profile_matches = expected
        .iter()
        .all(|(pointer, value)| string_at(package, pointer).as_deref() == Some(*value));
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "pinned_a202_profile",
        "profile",
        if profile_matches {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("WORLDSTREAM-A202-PROFILE-MISMATCH"),
        "revision, schemas, rules, scopes, and transaction profile remain exact",
    );
}

fn check_resolver_evidence(
    party: &Map<String, Value>,
    trust: &VerifierTrustV1,
    report: &mut NegotiationVerificationReportV1,
) -> Result<BTreeMap<String, CheckResultV1>, VerificationError> {
    let mut results = BTreeMap::new();
    let evidence = array_member(party, "resolver_evidence")?;
    for entry in evidence {
        let object = entry.as_object().ok_or(VerificationError::PackageShape)?;
        let evidence_id = string_member(object, "evidence_id")?;
        let response = decode(&string_member(object, "exact_response_base64url")?);
        let digest_result = response.as_ref().is_ok_and(|bytes| {
            blake3_digest(bytes) == string_member(object, "response_digest").unwrap_or_default()
        });
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "resolver_exact_response_digest",
            &evidence_id,
            if digest_result {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("WORLDSTREAM-RESOLVER-RESPONSE-MUTATED"),
            "exact resolver response remains content-addressed",
        );

        let observed = u64_member(object, "observed_at_unix_ms")?;
        let valid_until = u64_member(object, "valid_until_unix_ms")?;
        let bounded = valid_until
            .checked_sub(observed)
            .is_some_and(|duration| duration > 0 && duration <= 60_000);
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "resolver_validity_window",
            &evidence_id,
            if bounded {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("WORLDSTREAM-RESOLVER-WINDOW-INVALID"),
            "pilot resolver evidence is bounded to at most sixty seconds",
        );

        let source_id = string_member(object, "source_id")?;
        let attestation = object_member(object, "source_attestation")?;
        let key_id = string_member(attestation, "key_id")?;
        let trusted = trust
            .resolver_sources
            .iter()
            .find(|source| source.source_id == source_id && source.key_id == key_id);
        let auth_result = match trusted {
            None => CheckResultV1::NotCheckable,
            Some(source) => verify_resolver_attestation(object, attestation, source),
        };
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "resolver_source_authentication",
            &evidence_id,
            auth_result,
            Some("WORLDSTREAM-RESOLVER-AUTHENTICATION-FAILED"),
            if trusted.is_some() {
                "source attestation verified against caller-supplied trust"
            } else {
                "resolver source trust key was not supplied"
            },
        );
        let semantic_result = match string_member(object, "host_validity_result")?.as_str() {
            "active" => CheckResultV1::Verified,
            "unresolved" => CheckResultV1::NotCheckable,
            _ => CheckResultV1::Failed,
        };
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "resolver_subject_status",
            &evidence_id,
            semantic_result,
            Some("WORLDSTREAM-RESOLVER-SUBJECT-INACTIVE"),
            "authenticated host interpretation of the exact resolver response",
        );
        results.insert(
            evidence_id,
            combine_results([
                boolean_result(digest_result),
                boolean_result(bounded),
                auth_result,
                semantic_result,
            ]),
        );
    }
    Ok(results)
}

fn boolean_result(value: bool) -> CheckResultV1 {
    if value {
        CheckResultV1::Verified
    } else {
        CheckResultV1::Failed
    }
}

fn combine_results(results: impl IntoIterator<Item = CheckResultV1>) -> CheckResultV1 {
    let mut combined = CheckResultV1::Verified;
    for result in results {
        if result == CheckResultV1::Failed {
            return CheckResultV1::Failed;
        }
        if result == CheckResultV1::NotCheckable {
            combined = CheckResultV1::NotCheckable;
        }
    }
    combined
}

fn verify_resolver_attestation(
    entry: &Map<String, Value>,
    attestation: &Map<String, Value>,
    trusted: &TrustedResolverSourceV1,
) -> CheckResultV1 {
    if string_member(attestation, "algorithm").ok().as_deref() != Some("ES256")
        || string_member(attestation, "purpose").ok().as_deref()
            != Some("resolver_evidence_authentication")
    {
        return CheckResultV1::Failed;
    }
    let mut observation = Map::new();
    for name in [
        "evidence_id",
        "source_id",
        "subject",
        "observed_at_unix_ms",
        "valid_until_unix_ms",
        "media_type",
        "exact_response_base64url",
        "response_digest",
        "host_validity_result",
    ] {
        let Some(value) = entry.get(name) else {
            return CheckResultV1::Failed;
        };
        observation.insert(name.to_owned(), value.clone());
    }
    let protected = json!({
        "algorithm": string_member(attestation, "algorithm").unwrap_or_default(),
        "key_id": string_member(attestation, "key_id").unwrap_or_default(),
        "purpose": string_member(attestation, "purpose").unwrap_or_default(),
        "signed_at": string_member(attestation, "signed_at").unwrap_or_default(),
    });
    let Ok(mut message) = serialize(&Value::Object(observation)) else {
        return CheckResultV1::Failed;
    };
    let mut covered = b"worldstream/resolver-evidence-authentication/v1\0".to_vec();
    covered.append(&mut message);
    covered.push(b'.');
    let Ok(mut protected_bytes) = serialize(&protected) else {
        return CheckResultV1::Failed;
    };
    covered.append(&mut protected_bytes);
    verify_crypto(
        &trusted.public_key_sec1_base64url,
        &covered,
        &string_member(attestation, "signature_base64url").unwrap_or_default(),
    )
}

fn check_protocol_objects(
    party: &Map<String, Value>,
    trust: &VerifierTrustV1,
    resolver_results: &BTreeMap<String, CheckResultV1>,
    report: &mut NegotiationVerificationReportV1,
) -> Result<(), VerificationError> {
    let records = array_member(party, "exact_objects")?;
    let transaction_id = string_member(party, "transaction_id")?;
    for record in records {
        let record = record.as_object().ok_or(VerificationError::PackageShape)?;
        let object_id = string_member(record, "object_id")?;
        report.scope.objects.push(object_id.clone());
        check_object_media_type(record, &object_id, report);
        let decoded = decode(&string_member(record, "exact_bytes_base64url")?);
        let Ok(bytes) = decoded else {
            push(
                report,
                EvidenceSectionV1::PartyProtocol,
                "exact_a202_bytes",
                &object_id,
                CheckResultV1::Failed,
                Some("A202-EVIDENCE-HASH-MISMATCH"),
                "opaque bytes are not valid base64url",
            );
            continue;
        };
        let wire_matches = blake3_digest(&bytes) == string_member(record, "wire_digest")?;
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "exact_a202_wire_digest",
            &object_id,
            if wire_matches {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("A202-EVIDENCE-HASH-MISMATCH"),
            "BLAKE3 identifies the exact received bytes without replacing A202 SHA-256",
        );
        let parsed = parse_and_check_exact(&bytes);
        let Ok(value) = parsed else {
            push(
                report,
                EvidenceSectionV1::PartyProtocol,
                "a202_canonical_bytes",
                &object_id,
                CheckResultV1::Failed,
                Some("A202-EVIDENCE-HASH-MISMATCH"),
                "independent serializer rejected non-canonical exact bytes",
            );
            continue;
        };
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "a202_canonical_bytes",
            &object_id,
            CheckResultV1::Verified,
            None,
            "independent RFC 8785-subset serializer reproduced the exact bytes",
        );
        let declared_hash = string_at(&value, "/content_hash").unwrap_or_default();
        let recomputed = content_hash(&value).unwrap_or_default();
        let hash_matches = declared_hash == recomputed
            && string_member(record, "a202_content_hash").ok().as_deref()
                == Some(recomputed.as_str());
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "a202_content_hash",
            &object_id,
            if hash_matches {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("A202-EVIDENCE-HASH-MISMATCH"),
            "SHA-256 recomputed with only A202's three excluded top-level members",
        );
        let binding_matches = string_at(&value, "/id").as_deref() == Some(object_id.as_str())
            && string_at(&value, "/object_type").as_deref()
                == string_member(record, "object_type").ok().as_deref()
            && string_at(&value, "/transaction_id").as_deref() == Some(transaction_id.as_str());
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "a202_object_binding",
            &object_id,
            if binding_matches {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("A202-STREAM-MISMATCH"),
            "opaque object identity, type, and transaction agree with the export index",
        );
        check_signatures(record, &value, &object_id, trust, resolver_results, report)?;
    }
    Ok(())
}

fn check_object_media_type(
    record: &Map<String, Value>,
    object_id: &str,
    report: &mut NegotiationVerificationReportV1,
) {
    let matches = string_member(record, "media_type").ok().as_deref() == Some(A202_MEDIA_TYPE);
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "a202_media_type",
        object_id,
        if matches {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("A202-EVIDENCE-MEDIA-TYPE-MISMATCH"),
        "exact signed bytes retain the pinned A202 commercial media type",
    );
}

fn check_signatures(
    record: &Map<String, Value>,
    object: &Value,
    object_id: &str,
    trust: &VerifierTrustV1,
    resolver_results: &BTreeMap<String, CheckResultV1>,
    report: &mut NegotiationVerificationReportV1,
) -> Result<(), VerificationError> {
    let object_type = string_at(object, "/object_type").unwrap_or_default();
    let minimum = if object_type == "agreement" { 2 } else { 1 };
    let Some(signatures) = object.get("signatures").and_then(Value::as_array) else {
        push(
            report,
            EvidenceSectionV1::PartyProtocol,
            "signature_count",
            object_id,
            CheckResultV1::Failed,
            Some("A202-EVIDENCE-SIGNATURE-INVALID"),
            "A202 object carries no readable signatures array",
        );
        return Ok(());
    };
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "signature_count",
        object_id,
        if signatures.len() >= minimum {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("A202-EVIDENCE-SIGNATURE-INVALID"),
        "minimum signature count for the A202 object type",
    );
    let declared_bindings = array_member(record, "signature_verifications")?;
    let mut context = SignatureCheckContext {
        object,
        object_type: &object_type,
        object_id,
        declared_bindings,
        trust,
        resolver_results,
        report,
    };
    for (index, signature) in signatures.iter().enumerate() {
        let signature = signature
            .as_object()
            .ok_or(VerificationError::PackageShape)?;
        check_one_signature(signature, index, &mut context)?;
    }
    Ok(())
}

struct SignatureCheckContext<'a> {
    object: &'a Value,
    object_type: &'a str,
    object_id: &'a str,
    declared_bindings: &'a [Value],
    trust: &'a VerifierTrustV1,
    resolver_results: &'a BTreeMap<String, CheckResultV1>,
    report: &'a mut NegotiationVerificationReportV1,
}

fn check_one_signature(
    signature: &Map<String, Value>,
    index: usize,
    context: &mut SignatureCheckContext<'_>,
) -> Result<(), VerificationError> {
    let key_id = string_member(signature, "key_id")?;
    let purpose = string_member(signature, "purpose")?;
    let subject = format!("{}#signature-{index}", context.object_id);
    if !purpose_allowed(context.object_type, &purpose) {
        push(
            context.report,
            EvidenceSectionV1::PartyProtocol,
            "signature_purpose",
            &subject,
            CheckResultV1::Failed,
            Some("A202-EVIDENCE-SIGNATURE-INVALID"),
            "signature purpose is not registered for this object type",
        );
        return Ok(());
    }
    let Some(trusted) = context
        .trust
        .a202_keys
        .iter()
        .find(|key| key.key_id == key_id)
    else {
        push(
            context.report,
            EvidenceSectionV1::PartyProtocol,
            "a202_signature",
            &subject,
            CheckResultV1::NotCheckable,
            None,
            "caller supplied no trusted public key for this key id",
        );
        return Ok(());
    };
    let binding = context.declared_bindings.iter().find_map(|candidate| {
        let object = candidate.as_object()?;
        (string_member(object, "key_id").ok().as_deref() == Some(key_id.as_str())).then_some(object)
    });
    let Some(binding) = binding else {
        push(
            context.report,
            EvidenceSectionV1::PartyProtocol,
            "signature_signer_binding",
            &subject,
            CheckResultV1::NotCheckable,
            None,
            "venue export carries no signer-identity binding for this signature",
        );
        return Ok(());
    };
    let signer_matches = string_member(binding, "subject_id").ok().as_deref()
        == Some(trusted.subject_id.as_str())
        && string_member(binding, "purpose").ok().as_deref() == Some(purpose.as_str());
    push(
        context.report,
        EvidenceSectionV1::PartyProtocol,
        "signature_signer_binding",
        &subject,
        if signer_matches {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("A202-EVIDENCE-SIGNATURE-INVALID"),
        "trusted key subject agrees with the admitted signer and purpose",
    );
    check_signature_crypto(
        signature,
        context.object,
        &subject,
        &key_id,
        &purpose,
        &trusted.public_key_sec1_base64url,
        context.report,
    )?;
    check_key_status(binding, &subject, context.resolver_results, context.report);
    Ok(())
}

fn check_signature_crypto(
    signature: &Map<String, Value>,
    object: &Value,
    subject: &str,
    key_id: &str,
    purpose: &str,
    public_key: &str,
    report: &mut NegotiationVerificationReportV1,
) -> Result<(), VerificationError> {
    let algorithm = string_member(signature, "algorithm")?;
    let signed_at = string_member(signature, "signed_at")?;
    let signature_value = string_member(signature, "signature")?;
    let crypto_result = if algorithm == "ES256" {
        signature_message(object, key_id, &algorithm, purpose, &signed_at)
            .ok()
            .map_or(CheckResultV1::Failed, |message| {
                verify_crypto(public_key, &message, &signature_value)
            })
    } else {
        CheckResultV1::NotCheckable
    };
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "a202_signature",
        subject,
        crypto_result,
        Some("A202-EVIDENCE-SIGNATURE-INVALID"),
        if algorithm == "ES256" {
            "ES256 verified over independent canonical content and protected metadata"
        } else {
            "offline verifier currently implements the pinned ES256 path only"
        },
    );
    Ok(())
}

fn check_key_status(
    binding: &Map<String, Value>,
    subject: &str,
    resolver_results: &BTreeMap<String, CheckResultV1>,
    report: &mut NegotiationVerificationReportV1,
) {
    let status_ids: Vec<_> = binding
        .get("status_evidence_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let status_result = if status_ids.is_empty() {
        CheckResultV1::NotCheckable
    } else if status_ids
        .iter()
        .all(|id| resolver_results.get(*id) == Some(&CheckResultV1::Verified))
    {
        CheckResultV1::Verified
    } else if status_ids
        .iter()
        .any(|id| resolver_results.get(*id) == Some(&CheckResultV1::Failed))
    {
        CheckResultV1::Failed
    } else {
        CheckResultV1::NotCheckable
    };
    push(
        report,
        EvidenceSectionV1::PartyProtocol,
        "key_status_evidence",
        subject,
        status_result,
        Some("A202-EVIDENCE-SIGNATURE-INVALID"),
        "key status at signing and verification time requires authenticated resolver evidence",
    );
}

fn check_venue(
    venue: &Map<String, Value>,
    referenced_pack_bundle: Option<&[u8]>,
    report: &mut NegotiationVerificationReportV1,
) -> Result<(), VerificationError> {
    let pack = object_member(venue, "pack")?;
    let expected_bundle_digest = string_member(pack, "physical_bundle_digest")?;
    let bundle_result = referenced_pack_bundle.map_or(CheckResultV1::NotCheckable, |bytes| {
        if blake3_digest(bytes) == expected_bundle_digest {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        }
    });
    push(
        report,
        EvidenceSectionV1::VenueRuntime,
        "activity_pack_bundle",
        "pack",
        bundle_result,
        Some("WORLDSTREAM-PACK-BUNDLE-DIGEST-MISMATCH"),
        if referenced_pack_bundle.is_some() {
            "referenced original Activity Pack Bundle bytes"
        } else {
            "referenced Activity Pack Bundle was not supplied"
        },
    );

    let records = array_member(venue, "records")?;
    let final_sequence = venue
        .get("final_room_head")
        .and_then(Value::as_object)
        .and_then(|head| head.get("sequence"))
        .and_then(Value::as_u64)
        .ok_or(VerificationError::PackageShape)?;
    let mut previous = None;
    for record in records {
        let record = record.as_object().ok_or(VerificationError::PackageShape)?;
        let record_id = string_member(record, "record_id")?;
        report.scope.venue_records.push(record_id.clone());
        let sequence = u64_member(record, "room_sequence")?;
        let bytes = decode(&string_member(record, "exact_bytes_base64url")?);
        let digest_matches = bytes.as_ref().is_ok_and(|bytes| {
            blake3_digest(bytes) == string_member(record, "byte_digest").unwrap_or_default()
        });
        push(
            report,
            EvidenceSectionV1::VenueRuntime,
            "venue_record_digest",
            &record_id,
            if digest_matches {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("WORLDSTREAM-VENUE-RECORD-MUTATED"),
            "exact exported WorldStream record bytes",
        );
        let ordered = previous.is_none_or(|prior| sequence >= prior) && sequence <= final_sequence;
        push(
            report,
            EvidenceSectionV1::VenueRuntime,
            "venue_record_room_order",
            &record_id,
            if ordered {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("WORLDSTREAM-VENUE-ORDER-INVALID"),
            "record sequence is monotonic and no later than the exported Room Head",
        );
        previous = Some(sequence);
    }
    let replay = object_member(venue, "replay")?;
    let replay_bytes = decode(&string_member(replay, "exact_report_base64url")?);
    let replay_digest = replay_bytes.as_ref().is_ok_and(|bytes| {
        blake3_digest(bytes) == string_member(replay, "report_digest").unwrap_or_default()
    });
    push(
        report,
        EvidenceSectionV1::VenueRuntime,
        "replay_report_digest",
        "replay",
        if replay_digest {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("WORLDSTREAM-REPLAY-REPORT-MUTATED"),
        "exported Replay report bytes remain exact",
    );
    push(
        report,
        EvidenceSectionV1::VenueRuntime,
        "canonical_room_replay",
        "replay",
        CheckResultV1::NotCheckable,
        None,
        "full canonical Genesis/Transition export and Component execution are not in proof-package v1",
    );
    Ok(())
}

fn check_cross_index(
    party: &Map<String, Value>,
    venue: &Map<String, Value>,
    report: &mut NegotiationVerificationReportV1,
) -> Result<(), VerificationError> {
    let objects = array_member(party, "exact_objects")?;
    let records = array_member(venue, "records")?;
    let index = array_member(venue, "cross_index")?;
    let object_map: BTreeMap<_, _> = objects
        .iter()
        .filter_map(|value| value.as_object())
        .filter_map(|object| {
            Some((
                string_member(object, "object_id").ok()?,
                string_member(object, "wire_digest").ok()?,
            ))
        })
        .collect();
    let record_ids: BTreeSet<_> = records
        .iter()
        .filter_map(|value| value.as_object())
        .filter_map(|object| string_member(object, "record_id").ok())
        .collect();
    let mut covered = BTreeSet::new();
    for entry in index {
        let entry = entry.as_object().ok_or(VerificationError::PackageShape)?;
        let object_id = string_member(entry, "a202_object_id")?;
        let action_id = string_member(entry, "worldstream_action_id")?;
        let transition_id = string_member(entry, "worldstream_transition_record_id")?;
        let valid = object_map.get(&object_id).is_some_and(|digest| {
            digest == &string_member(entry, "a202_wire_digest").unwrap_or_default()
        }) && record_ids.contains(&action_id)
            && record_ids.contains(&transition_id)
            && covered.insert(object_id.clone());
        push(
            report,
            EvidenceSectionV1::CrossIndex,
            "protocol_to_venue_binding",
            &object_id,
            if valid {
                CheckResultV1::Verified
            } else {
                CheckResultV1::Failed
            },
            Some("WORLDSTREAM-CROSS-INDEX-INVALID"),
            "exact A202 bytes map to one admitted WorldStream Action and Transition record",
        );
    }
    let coverage = covered.len() == object_map.len();
    push(
        report,
        EvidenceSectionV1::CrossIndex,
        "cross_index_coverage",
        "cross-index",
        if coverage {
            CheckResultV1::Verified
        } else {
            CheckResultV1::Failed
        },
        Some("WORLDSTREAM-CROSS-INDEX-INCOMPLETE"),
        "every disclosed A202 object is cross-indexed exactly once",
    );
    Ok(())
}

fn purpose_allowed(object_type: &str, purpose: &str) -> bool {
    match object_type {
        "offer" => matches!(purpose, "offer_submission" | "object_issuance"),
        "acceptance" => matches!(purpose, "offer_acceptance" | "object_issuance"),
        "agreement" => matches!(purpose, "agreement_commitment" | "object_issuance"),
        "transaction_event" => purpose == "event_append",
        "policy_decision" => purpose == "policy_decision",
        "action_envelope" => purpose == "action_submission",
        "approval" | "commitment" | "evidence" | "key_record" | "revocation_record" => {
            purpose == "object_issuance"
        }
        _ => false,
    }
}

fn verify_crypto(public_key: &str, message: &[u8], signature_value: &str) -> CheckResultV1 {
    let (Ok(public_key), Ok(signature_value)) = (decode(public_key), decode(signature_value))
    else {
        return CheckResultV1::Failed;
    };
    if signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, public_key)
        .verify(message, &signature_value)
        .is_ok()
    {
        CheckResultV1::Verified
    } else {
        CheckResultV1::Failed
    }
}

fn push(
    report: &mut NegotiationVerificationReportV1,
    section: EvidenceSectionV1,
    check: &str,
    subject: &str,
    result: CheckResultV1,
    failure_code: Option<&str>,
    detail: &str,
) {
    report.checks.push(EvidenceCheckV1 {
        section,
        check: check.to_owned(),
        subject: subject.to_owned(),
        result,
        refusal_code: (result == CheckResultV1::Failed)
            .then(|| failure_code.map(str::to_owned))
            .flatten(),
        detail: detail.to_owned(),
    });
}

fn object_member<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, VerificationError> {
    object
        .get(name)
        .and_then(Value::as_object)
        .ok_or(VerificationError::PackageShape)
}

fn array_member<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Vec<Value>, VerificationError> {
    object
        .get(name)
        .and_then(Value::as_array)
        .ok_or(VerificationError::PackageShape)
}

fn string_member(object: &Map<String, Value>, name: &str) -> Result<String, VerificationError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(VerificationError::PackageShape)
}

fn u64_member(object: &Map<String, Value>, name: &str) -> Result<u64, VerificationError> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or(VerificationError::PackageShape)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn decode(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(value)
}

fn blake3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("proof package is not valid JSON: {0}")]
    PackageJson(String),
    #[error("proof package does not have the required v1 shape")]
    PackageShape,
}
