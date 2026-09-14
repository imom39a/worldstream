use std::str::FromStr;

use worldstream_core::{
    CanonicalJsonV1, EXTERNAL_INPUT_INGRESS_SOURCE_ID, EXTERNAL_INPUT_INGRESS_TYPE,
    ExternalInputOperationIdentityV1, ExternalInputRecordedAt, ExternalInputV1, InputId,
    OperationIdentityV1, PackDigestV1, RoomId, RoomSequenceV1, SemanticResultV1, SourceId,
    StoredSemanticResultV1, external_input_request_hash,
};
use worldstream_protocol::{
    EXTERNAL_INPUT_INGRESS_REQUEST_VERSION, ExternalInputIngressRequestV1,
    ExternalInputIngressResponseV1, RoomHead,
};

use crate::BackendError;

#[cfg(test)]
#[path = "external_input_acceptance.rs"]
mod acceptance;

pub(crate) const EXTERNAL_INPUT_INGRESS_VERSION: &str = EXTERNAL_INPUT_INGRESS_REQUEST_VERSION;
pub(crate) const EXTERNAL_INPUT_INGRESS_MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// Parsed ingress envelope. The source/type allowlist is checked before any
/// Room or Pack state is read; `recorded_at` is later replaced by the durable
/// first-preparation value on retries.
pub(crate) struct ExternalInputIngressPlanV1 {
    pub(crate) room_id: RoomId,
    pub(crate) based_on_room_seq: RoomSequenceV1,
    pub(crate) input: ExternalInputV1,
    pub(crate) pack_digest: PackDigestV1,
    pub(crate) identity: OperationIdentityV1,
    pub(crate) request_hash: worldstream_core::CanonicalRequestHashV1,
}

pub(crate) fn prepare_external_input_ingress(
    room_id: RoomId,
    request: &ExternalInputIngressRequestV1,
    checked_at: &worldstream_core::AuthorityCheckedAt,
) -> Result<ExternalInputIngressPlanV1, BackendError> {
    if request.version != EXTERNAL_INPUT_INGRESS_REQUEST_VERSION
        || request.source_id != EXTERNAL_INPUT_INGRESS_SOURCE_ID
        || request.input_type != EXTERNAL_INPUT_INGRESS_TYPE
    {
        return Err(BackendError::Rejected);
    }
    let source_id = SourceId::from_str(&request.source_id).map_err(|_| BackendError::Rejected)?;
    let input_id = InputId::from_str(&request.input_id).map_err(|_| BackendError::Rejected)?;
    let based_on_room_seq =
        RoomSequenceV1::new(request.based_on_room_seq).map_err(|_| BackendError::Rejected)?;
    let pack_digest =
        PackDigestV1::from_str(&request.pack_digest).map_err(|_| BackendError::Rejected)?;
    let payload_bytes = serde_json::to_vec(&request.payload).map_err(|_| BackendError::Rejected)?;
    let canonical_payload =
        CanonicalJsonV1::parse(&payload_bytes).map_err(|_| BackendError::Rejected)?;
    if canonical_payload
        .to_bytes()
        .map_err(|_| BackendError::Rejected)?
        .len()
        > EXTERNAL_INPUT_INGRESS_MAX_PAYLOAD_BYTES
    {
        return Err(BackendError::Rejected);
    }
    let recorded_at = request
        .recorded_at
        .as_deref()
        .map(ExternalInputRecordedAt::from_str)
        .transpose()
        .map_err(|_| BackendError::Rejected)?
        .unwrap_or_else(|| {
            ExternalInputRecordedAt::from_str(checked_at.as_str())
                .unwrap_or_else(|_| unreachable!("checked authority time is canonical"))
        });
    let input = ExternalInputV1 {
        source_id,
        input_id,
        input_type: request.input_type.clone(),
        recorded_at,
        canonical_payload,
        immutable_resource_references: Vec::new(),
    };
    let identity = OperationIdentityV1::ExternalInput(Box::new(ExternalInputOperationIdentityV1 {
        room_id: room_id.clone(),
        source_id: input.source_id.clone(),
        input_id: input.input_id.clone(),
    }));
    let request_hash = external_input_request_hash(&room_id, based_on_room_seq, &input)
        .map_err(|_| BackendError::Rejected)?;
    Ok(ExternalInputIngressPlanV1 {
        room_id,
        based_on_room_seq,
        input,
        pack_digest,
        identity,
        request_hash,
    })
}

pub(crate) fn ingress_response_from_result(
    plan: &ExternalInputIngressPlanV1,
    result: &StoredSemanticResultV1,
    duplicate: bool,
) -> Result<ExternalInputIngressResponseV1, BackendError> {
    let SemanticResultV1::TransitionCommitted {
        room_id,
        transition_id,
        complete_head,
        ..
    } = result.result()
    else {
        return Err(BackendError::InvalidResult);
    };
    if room_id != &plan.room_id || complete_head.pack_digest() != &plan.pack_digest {
        return Err(BackendError::Conflict);
    }
    let recorded_at = match result.semantic_time() {
        worldstream_core::ReceiptSemanticTimeV1::ExternalInputRecorded(value) => value.as_str(),
        _ => return Err(BackendError::InvalidResult),
    };
    Ok(ExternalInputIngressResponseV1 {
        version: EXTERNAL_INPUT_INGRESS_VERSION.to_owned(),
        room_id: room_id.to_string(),
        source_id: plan.input.source_id.to_string(),
        input_id: plan.input.input_id.to_string(),
        input_type: plan.input.input_type.clone(),
        recorded_at: recorded_at.to_owned(),
        transition_id: transition_id.to_string(),
        room_head: RoomHead {
            room_id: complete_head.room_id().to_string(),
            room_seq: complete_head.room_seq().get(),
            genesis_or_transition_hash: complete_head.genesis_or_transition_hash().to_string(),
            core_schema_version: complete_head.core_schema_version().to_owned(),
            pack_digest: complete_head.pack_digest().to_string(),
            core_state_hash: complete_head.core_state_hash().to_string(),
            activity_state_hash: complete_head.activity_state_hash().to_string(),
            authoritative_state_hash: complete_head.authoritative_state_hash().to_string(),
        },
        duplicate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(recorded_at: Option<&str>) -> ExternalInputIngressRequestV1 {
        ExternalInputIngressRequestV1 {
            version: EXTERNAL_INPUT_INGRESS_REQUEST_VERSION.to_owned(),
            source_id: EXTERNAL_INPUT_INGRESS_SOURCE_ID.to_owned(),
            input_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            input_type: EXTERNAL_INPUT_INGRESS_TYPE.to_owned(),
            based_on_room_seq: 7,
            pack_digest: "blake3:0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            payload: json!({"revision": 3, "value": "stable"}),
            recorded_at: recorded_at.map(str::to_owned),
        }
    }

    #[test]
    fn ingress_allowlist_and_hash_ignore_retry_sample_time() {
        let checked = worldstream_core::AuthorityCheckedAt::from_str("2026-09-12T12:00:00Z")
            .unwrap_or_else(|_| unreachable!("canonical checked time"));
        let first = prepare_external_input_ingress(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            &request(Some("2026-09-12T12:00:01.123456Z")),
            &checked,
        )
        .unwrap_or_else(|error| panic!("valid ingress: {error:?}"));
        let retry = prepare_external_input_ingress(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            &request(Some("2026-09-12T12:05:01.123456Z")),
            &checked,
        )
        .unwrap_or_else(|error| panic!("valid retry ingress: {error:?}"));
        assert_eq!(first.identity, retry.identity);
        assert_eq!(first.request_hash, retry.request_hash);
        assert!(
            prepare_external_input_ingress(
                first.room_id.clone(),
                &ExternalInputIngressRequestV1 {
                    input_type: "connector/arbitrary".to_owned(),
                    ..request(None)
                },
                &checked,
            )
            .is_err()
        );
    }

    #[test]
    fn ingress_rejects_invalid_revision_basis_and_payload_before_room_access() {
        let checked = worldstream_core::AuthorityCheckedAt::from_str("2026-09-12T12:00:00Z")
            .unwrap_or_else(|_| unreachable!("canonical checked time"));
        let room: RoomId = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|_| unreachable!());
        let invalid_revision = ExternalInputIngressRequestV1 {
            pack_digest: "not-a-pack".to_owned(),
            ..request(None)
        };
        assert!(prepare_external_input_ingress(room.clone(), &invalid_revision, &checked).is_err());
        let invalid_basis = ExternalInputIngressRequestV1 {
            based_on_room_seq: u64::MAX,
            ..request(None)
        };
        assert!(prepare_external_input_ingress(room.clone(), &invalid_basis, &checked).is_err());
        let oversized = ExternalInputIngressRequestV1 {
            payload: serde_json::json!({ "source": "x".repeat(EXTERNAL_INPUT_INGRESS_MAX_PAYLOAD_BYTES) }),
            ..request(None)
        };
        assert!(prepare_external_input_ingress(room, &oversized, &checked).is_err());
    }
}
