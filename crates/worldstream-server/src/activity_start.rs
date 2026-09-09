use std::str::FromStr;

use worldstream_core::{
    ActivityStartCompatibilityV1, AuthorityCheckedAt, CanonicalJsonV1, CanonicalRequestHashV1,
    ExternalInputOperationIdentityV1, ExternalInputRecordedAt, ExternalInputV1, InputId,
    OperationIdentityV1, PackDigestV1, PackRegistryV1, RoomCommitResolutionV1, RoomId,
    RoomSequenceV1, SemanticResultV1, SourceId, StoredSemanticResultV1,
    external_input_request_hash,
};
use worldstream_protocol::{LobbyLaunchRequest, LobbyLaunchResponse, RoomHead};

use crate::BackendError;

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActivityStartPackExpectationV1 {
    Exact(PackDigestV1),
    RetainedHeist,
}

/// Parsed, metadata-derived Activity Start request whose identity and hash can
/// be resolved without consulting mutable Room state.
pub(crate) struct ActivityStartRequestPlanV1 {
    pub(crate) room_id: RoomId,
    pub(crate) based_on_room_seq: RoomSequenceV1,
    pub(crate) input: ExternalInputV1,
    pub(crate) identity: OperationIdentityV1,
    pub(crate) request_hash: CanonicalRequestHashV1,
    expected_pack: ActivityStartPackExpectationV1,
}

impl ActivityStartRequestPlanV1 {
    /// Checks the caller's immutable Pack expectation against the recovered
    /// Room only after receipt resolution has proven the identity absent.
    pub(crate) fn validate_room_pack(
        &self,
        registry: &PackRegistryV1,
        actual: &PackDigestV1,
    ) -> Result<(), BackendError> {
        match &self.expected_pack {
            ActivityStartPackExpectationV1::Exact(expected) if expected != actual => {
                return Err(BackendError::Conflict);
            }
            ActivityStartPackExpectationV1::Exact(_) => {}
            ActivityStartPackExpectationV1::RetainedHeist
                if !worldstream_core::agent_heist_lobby_contract_declared(registry, actual) =>
            {
                return Err(BackendError::WrongPhase);
            }
            ActivityStartPackExpectationV1::RetainedHeist => {}
        }

        let ActivityStartCompatibilityV1::Supported(contract) = registry
            .activity_start_compatibility(actual)
            .map_err(|_| BackendError::InvalidResult)?
        else {
            return Err(BackendError::WrongPhase);
        };
        if contract.input_type != self.input.input_type
            || contract.canonical_payload != self.input.canonical_payload
        {
            return Err(BackendError::Conflict);
        }
        Ok(())
    }
}

/// Derives the only bounded Activity Start input and its stable receipt
/// address without reading Room lifecycle, integrity, or Membership state.
pub(crate) fn prepare_activity_start_request(
    registry: &PackRegistryV1,
    room_id: RoomId,
    request: &LobbyLaunchRequest,
    checked_at: &AuthorityCheckedAt,
) -> Result<ActivityStartRequestPlanV1, BackendError> {
    let input_id = InputId::from_str(&request.input_id).map_err(|_| BackendError::Rejected)?;
    let based_on_room_seq =
        RoomSequenceV1::new(request.based_on_room_seq).map_err(|_| BackendError::Rejected)?;

    let (expected_pack, input_type, canonical_payload) =
        if let Some(pack_digest) = request.pack_digest.as_deref() {
            let digest = PackDigestV1::from_str(pack_digest).map_err(|_| BackendError::Rejected)?;
            let ActivityStartCompatibilityV1::Supported(contract) = registry
                .activity_start_compatibility(&digest)
                .map_err(|_| BackendError::WrongPhase)?
            else {
                return Err(BackendError::WrongPhase);
            };
            (
                ActivityStartPackExpectationV1::Exact(digest),
                contract.input_type,
                contract.canonical_payload,
            )
        } else {
            // The absent field is the retained pre-Activity-Start wire form.
            // Its fixed Heist input is checked against the Room after a guarded
            // KnownAbsent result; generic Packs must supply their exact digest.
            (
                ActivityStartPackExpectationV1::RetainedHeist,
                worldstream_core::HOST_LAUNCH_INPUT_TYPE.to_owned(),
                CanonicalJsonV1::parse(br"{}").map_err(|_| BackendError::InvalidResult)?,
            )
        };

    let input = ExternalInputV1 {
        source_id: SourceId::from_str(worldstream_core::ACTIVITY_START_SOURCE_ID)
            .map_err(|_| BackendError::InvalidResult)?,
        input_id,
        input_type,
        recorded_at: ExternalInputRecordedAt::from_str(checked_at.as_str())
            .map_err(|_| BackendError::StorageUnavailable)?,
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
    Ok(ActivityStartRequestPlanV1 {
        room_id,
        based_on_room_seq,
        input,
        identity,
        request_hash,
        expected_pack,
    })
}

pub(crate) fn lobby_response_from_result(
    plan: &ActivityStartRequestPlanV1,
    registry: &PackRegistryV1,
    input_id: &str,
    result: &StoredSemanticResultV1,
    duplicate: bool,
) -> Result<LobbyLaunchResponse, BackendError> {
    let SemanticResultV1::TransitionCommitted {
        room_id,
        transition_id,
        complete_head,
        ..
    } = result.result()
    else {
        return Err(BackendError::InvalidResult);
    };
    // The external-input hash is intentionally the existing Core hash and
    // does not add a Host-only Pack field. When two revisions declare the
    // same fixed input, the stored receipt's immutable Complete Head is the
    // final exact-Pack check; this still happens before any live Room read.
    plan.validate_room_pack(registry, complete_head.pack_digest())?;
    Ok(LobbyLaunchResponse {
        room_id: room_id.to_string(),
        input_id: input_id.to_owned(),
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

pub(crate) fn lobby_response_from_resolution(
    plan: &ActivityStartRequestPlanV1,
    registry: &PackRegistryV1,
    input_id: &str,
    resolution: &RoomCommitResolutionV1,
) -> Result<LobbyLaunchResponse, BackendError> {
    if let Some(result) = resolution.stored_result() {
        return lobby_response_from_result(
            plan,
            registry,
            input_id,
            result,
            resolution.duplicate(),
        );
    }
    match resolution {
        RoomCommitResolutionV1::Conflict { .. } => Err(BackendError::Conflict),
        RoomCommitResolutionV1::Fenced
        | RoomCommitResolutionV1::Reprepare
        | RoomCommitResolutionV1::RetryableKnownAbsent
        | RoomCommitResolutionV1::NotApplicable => Err(BackendError::Busy),
        RoomCommitResolutionV1::Indeterminate => Err(BackendError::Indeterminate),
        _ => Err(BackendError::InvalidResult),
    }
}
