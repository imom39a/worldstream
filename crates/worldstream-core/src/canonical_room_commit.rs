//! Sealed additive Room Commit contracts. Legacy adapters cannot accept these
//! types until they implement the explicit format-neutral storage port.
use super::*;
use crate::{
    CanonicalAdvanceDisposition, CanonicalHistoryFormat, CanonicalRoomTrace, GenesisRecord,
    PreparedCanonicalRoomTransition, RoomCreationRequestWithFormat, TransitionRecord,
};

/// Complete initial persistence bundle. Only Core can construct it.
#[derive(Clone)]
pub struct PreparedCanonicalCreationPersistence {
    pack_revision_lock: PackRevisionLockV1,
    canonical_pack_revision_lock_bytes: Vec<u8>,
    genesis: GenesisRecord,
    canonical_genesis_bytes: Vec<u8>,
    complete_head: CompleteHeadV1,
    canonical_head_bytes: Vec<u8>,
    core_state: CoreRoomStateV1,
    canonical_core_state_bytes: Vec<u8>,
    activity_state: CanonicalJsonV1,
    canonical_activity_state_bytes: Vec<u8>,
    memberships: Vec<PreparedMembershipMaterializationV1>,
    initial_timers: Vec<PreparedTimerMaterializationV1>,
    integrity_generation: IntegrityGenerationV1,
}

/// Complete Advance bundle. V2 state is a separate verified materialization.
#[derive(Clone)]
pub struct PreparedCanonicalAdvancePersistence {
    transition_id: TransitionId,
    transition: TransitionRecord,
    canonical_transition_bytes: Vec<u8>,
    resulting_complete_head: CompleteHeadV1,
    canonical_resulting_head_bytes: Vec<u8>,
    resulting_core_state: CoreRoomStateV1,
    canonical_resulting_core_state_bytes: Vec<u8>,
    resulting_activity_state: CanonicalJsonV1,
    canonical_resulting_activity_state_bytes: Vec<u8>,
    resulting_memberships: Vec<PreparedMembershipMaterializationV1>,
    timer_changes: Vec<PreparedTimerMutationV1>,
    delivery_consequences: Vec<PreparedObservationConsequenceV1>,
    activation_decisions: Vec<PreparedActivationDecisionV1>,
}

/// The two existing-Room intents share atomic receipt and fencing rules.
#[derive(Clone)]
pub enum PreparedCanonicalExistingIntent {
    /// One complete immutable Advance bundle.
    Advance(Box<PreparedCanonicalAdvancePersistence>),
    /// Stable rejection or NoChange, with no accepted Transition.
    DurableDisposition,
}

/// Opaque sealed existing-Room plan with exact identity and input witnesses.
pub struct PreparedCanonicalRoomCommit {
    identity: OperationIdentityV1,
    request_hash: CanonicalRequestHashV1,
    basis_complete_head: CompleteHeadV1,
    integrity_generation: IntegrityGenerationV1,
    authority_witness: PreparedAuthorityWitnessV1,
    input_witness: PreparedOperationInputWitnessV1,
    intent: PreparedCanonicalExistingIntent,
    semantic_result: StoredSemanticResultV1,
    pending_transition: Option<PreparedCanonicalRoomTransition>,
}

/// Opaque sealed initial Room plan. It releases no serving executor before
/// storage returns its exact new durable creation receipt.
pub struct PreparedCanonicalRoomCreation {
    identity: OperationIdentityV1,
    request_hash: CanonicalRequestHashV1,
    authority_witness: PreparedAuthorityWitnessV1,
    persistence: PreparedCanonicalCreationPersistence,
    semantic_result: StoredSemanticResultV1,
    pending_trace: Option<CanonicalRoomTrace>,
}

impl PreparedCanonicalRoomCommit {
    /// Converts one checked Counter/pack Action preparation into the complete
    /// persistence bundle. Storage performs no reducer, projection, or hash work.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Action preparation, mismatched trace basis,
    /// authority, addressed frame, or sealed transition state.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn for_action(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        trace.validate_prepared(&prepared)?;
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::ParticipantAction(action) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotParticipantAction);
        };
        if !request.matches_normalized(action) {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        let membership_before = trace
            .core_state()
            .membership(&action.member_id)
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?
            .clone();
        if membership_before.principal_id() != authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let canonical_action_offers_bytes = prepared
            .action_offer_witness()
            .ok_or(PrepareRoomWriteErrorV1::MissingActionOfferWitness)?
            .to_vec();
        let basis_complete_head = trace.head().clone();
        let (intent, result) = match prepared.disposition() {
            CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } => {
                let resulting_state = prepared
                    .resulting_state()
                    .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
                let resulting_complete_head = transition.complete_head();
                if resulting_state.head() != &resulting_complete_head
                    || transition.previous_lineage_hash()
                        != basis_complete_head.genesis_or_transition_hash()
                {
                    return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
                }
                let delivery_consequences = prepare_canonical_transition_consequences(
                    trace,
                    &prepared,
                    resulting_state.core_state(),
                    transition.room_seq(),
                    current_frame_heads,
                )?;
                let mut resulting_memberships =
                    Vec::with_capacity(resulting_state.core_state().memberships().len());
                for membership in resulting_state.core_state().memberships().values() {
                    resulting_memberships.push(PreparedMembershipMaterializationV1 {
                        membership: membership.clone(),
                        canonical_membership_bytes: encode(membership)?,
                    });
                }
                let persistence = PreparedCanonicalAdvancePersistence {
                    transition_id: transition_id.clone(),
                    transition: (**transition).clone(),
                    canonical_transition_bytes: transition.canonical_bytes()?,
                    resulting_complete_head: resulting_complete_head.clone(),
                    canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
                    resulting_core_state: resulting_state.core_state().clone(),
                    canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
                    resulting_activity_state: resulting_state.activity_state().clone(),
                    canonical_resulting_activity_state_bytes: resulting_state
                        .activity_state()
                        .to_bytes()?,
                    resulting_memberships,
                    timer_changes: transition
                        .ordered_timer_changes()
                        .iter()
                        .map(PreparedTimerMutationV1::from_change)
                        .collect::<Result<Vec<_>, _>>()?,
                    delivery_consequences,
                    activation_decisions: prepare_canonical_activation_decisions(transition)?,
                };
                (
                    PreparedCanonicalExistingIntent::Advance(Box::new(persistence)),
                    SemanticResultV1::TransitionCommitted {
                        room_id: resulting_complete_head.room_id().clone(),
                        transition_id,
                        room_seq: resulting_complete_head.room_seq(),
                        previous_lineage_hash: transition.previous_lineage_hash().clone(),
                        complete_head: resulting_complete_head,
                    },
                )
            }
            CanonicalAdvanceDisposition::RejectionRecorded { rejection, .. } => (
                PreparedCanonicalExistingIntent::DurableDisposition,
                SemanticResultV1::RejectionRecorded {
                    code: "activity_domain_rejection".to_owned(),
                    safe_details: CanonicalJsonV1::from_serialize(
                        &ActivityDomainRejectionDetailsV1 {
                            declared_code: rejection.declared_code.clone(),
                            safe_details: rejection.bounded_safe_details.clone(),
                        },
                    )?,
                },
            ),
            CanonicalAdvanceDisposition::NoChangeRecorded { .. } => {
                return Err(PrepareRoomWriteErrorV1::ActionNoChange);
            }
        };
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ParticipantAction {
                request: request.clone(),
                admitted_at: action.admitted_at.clone(),
                normalized_action: Some(Box::new(action.clone())),
                action_offer_witness: ActionOfferWitnessV1::Current {
                    canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(
                        &canonical_action_offers_bytes,
                    )?,
                },
                admission_context: ActionAdmissionContextV1 {
                    membership_before: membership_before.clone(),
                    room_status: trace.core_state().room_status(),
                },
            },
            result,
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::ParticipantAction(Box::new(
                PreparedActionInputWitnessV1 {
                    request: request.clone(),
                    admitted_at: action.admitted_at.clone(),
                    normalized_action: Some(action.clone()),
                    canonical_membership_before_bytes: encode(&membership_before)?,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    action_offer_witness: ActionOfferWitnessV1::Current {
                        canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(
                            &canonical_action_offers_bytes,
                        )?,
                    },
                    membership_before,
                },
            )),
            intent,
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Seals one exact, still-scheduled Timer generation Advance. Obsolescence
    /// remains a conditional storage result (`NotApplicable`) and never gains
    /// a Semantic Receipt.
    ///
    /// # Errors
    ///
    /// Returns an error if the prepared Timer transition or any persistence witness is invalid.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn for_timer_fired(
        trace: &CanonicalRoomTrace,
        request: &TimerFiredRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        trace.validate_prepared(&prepared)?;
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::TimerFired(timer) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotTimerFired);
        };
        if timer != &request.recorded_stimulus() {
            return Err(PrepareRoomWriteErrorV1::TimerRequestMismatch);
        }
        let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } =
            prepared.disposition()
        else {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        };
        let resulting_state = prepared
            .resulting_state()
            .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
        let resulting_complete_head = transition.complete_head();
        let basis_complete_head = trace.head().clone();
        if resulting_state.head() != &resulting_complete_head
            || transition.previous_lineage_hash()
                != basis_complete_head.genesis_or_transition_hash()
        {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        let delivery_consequences = prepare_canonical_transition_consequences(
            trace,
            &prepared,
            resulting_state.core_state(),
            transition.room_seq(),
            current_frame_heads,
        )?;
        let resulting_memberships = resulting_state
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)?,
                })
            })
            .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
        let persistence = PreparedCanonicalAdvancePersistence {
            transition_id: transition_id.clone(),
            transition: (**transition).clone(),
            canonical_transition_bytes: transition.canonical_bytes()?,
            resulting_complete_head: resulting_complete_head.clone(),
            canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
            resulting_core_state: resulting_state.core_state().clone(),
            canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
            resulting_activity_state: resulting_state.activity_state().clone(),
            canonical_resulting_activity_state_bytes: resulting_state
                .activity_state()
                .to_bytes()?,
            resulting_memberships,
            timer_changes: transition
                .ordered_timer_changes()
                .iter()
                .map(PreparedTimerMutationV1::from_change)
                .collect::<Result<Vec<_>, _>>()?,
            delivery_consequences,
            activation_decisions: prepare_canonical_activation_decisions(transition)?,
        };
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::TimerFired {
                request: request.clone(),
            },
            SemanticResultV1::TransitionCommitted {
                room_id: resulting_complete_head.room_id().clone(),
                transition_id,
                room_seq: resulting_complete_head.room_seq(),
                previous_lineage_hash: transition.previous_lineage_hash().clone(),
                complete_head: resulting_complete_head,
            },
        )?;
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::TimerFired(Box::new(
                PreparedTimerInputWitnessV1 {
                    request: request.clone(),
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    canonical_timer_payload_bytes: request.canonical_payload().to_bytes()?,
                },
            )),
            intent: PreparedCanonicalExistingIntent::Advance(Box::new(persistence)),
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Seals one exact Timer firing using the `HostOperator` Room-root grant.
    /// The grant binds the Room and request hash; this method then delegates
    /// to the existing exact Timer witness and commit-fence preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another Room/request, or if the
    /// prepared Timer transition or any persistence witness is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_timer_fired(
        trace: &CanonicalRoomTrace,
        request: &TimerFiredRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedTimerFiredV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let request_hash = request.canonical_request_hash()?;
        if authority.room_id() != request.room_id() || authority.request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = AuthorityUseV1::TimerFired {
            room_id: request.room_id().clone(),
            request_hash,
        };
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        Self::for_timer_fired(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Seals one exact host-authorized `ExternalInput` Advance.
    ///
    /// # Errors
    ///
    /// Returns an error when the Room/basis/input/grant or prepared transition differs.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn for_authorized_external_input(
        trace: &CanonicalRoomTrace,
        room_id: &RoomId,
        based_on_room_seq: RoomSequenceV1,
        input: &ExternalInputV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedExternalInputV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if !prepared.is_new()
            || prepared.basis_complete_head() != trace.head()
            || trace.head().room_id() != room_id
            || trace.head().room_seq() != based_on_room_seq
        {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::ExternalInput(recorded) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::NotExternalInput);
        };
        if recorded != input {
            return Err(PrepareRoomWriteErrorV1::ExternalInputRequestMismatch);
        }
        let request_hash = external_input_request_hash(room_id, based_on_room_seq, input)?;
        if authority.room_id() != room_id || authority.request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = AuthorityUseV1::ExternalInput {
            room_id: room_id.clone(),
            request_hash: request_hash.clone(),
        };
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        let CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } =
            prepared.disposition()
        else {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        };
        let resulting_state = prepared
            .resulting_state()
            .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
        let resulting_complete_head = transition.complete_head();
        let basis_complete_head = trace.head().clone();
        if resulting_state.head() != &resulting_complete_head
            || transition.previous_lineage_hash()
                != basis_complete_head.genesis_or_transition_hash()
        {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        let delivery_consequences = prepare_canonical_transition_consequences(
            trace,
            &prepared,
            resulting_state.core_state(),
            transition.room_seq(),
            current_frame_heads,
        )?;
        let resulting_memberships = resulting_state
            .core_state()
            .memberships()
            .values()
            .map(|membership| {
                Ok(PreparedMembershipMaterializationV1 {
                    membership: membership.clone(),
                    canonical_membership_bytes: encode(membership)?,
                })
            })
            .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
        let persistence = PreparedCanonicalAdvancePersistence {
            transition_id: transition_id.clone(),
            transition: (**transition).clone(),
            canonical_transition_bytes: transition.canonical_bytes()?,
            resulting_complete_head: resulting_complete_head.clone(),
            canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
            resulting_core_state: resulting_state.core_state().clone(),
            canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
            resulting_activity_state: resulting_state.activity_state().clone(),
            canonical_resulting_activity_state_bytes: resulting_state
                .activity_state()
                .to_bytes()?,
            resulting_memberships,
            timer_changes: transition
                .ordered_timer_changes()
                .iter()
                .map(PreparedTimerMutationV1::from_change)
                .collect::<Result<Vec<_>, _>>()?,
            delivery_consequences,
            activation_decisions: prepare_canonical_activation_decisions(transition)?,
        };
        let identity =
            OperationIdentityV1::ExternalInput(Box::new(ExternalInputOperationIdentityV1 {
                room_id: room_id.clone(),
                source_id: input.source_id.clone(),
                input_id: input.input_id.clone(),
            }));
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ExternalInput {
                room_id: room_id.clone(),
                input: input.clone(),
            },
            SemanticResultV1::TransitionCommitted {
                room_id: resulting_complete_head.room_id().clone(),
                transition_id,
                room_seq: resulting_complete_head.room_seq(),
                previous_lineage_hash: transition.previous_lineage_hash().clone(),
                complete_head: resulting_complete_head,
            },
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::ExternalInput(Box::new(
                PreparedExternalInputWitnessV1 {
                    room_id: room_id.clone(),
                    based_on_room_seq,
                    input: input.clone(),
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                },
            )),
            intent: PreparedCanonicalExistingIntent::Advance(Box::new(persistence)),
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Authorizes, normalizes, reduces, and seals one exact existing-Room
    /// Core administration request. Caller-supplied attribution is impossible:
    /// the recorded proposal is derived from the consumed authority grant.
    ///
    /// # Errors
    ///
    /// Returns an error if the request/classification/grant disagree, Core or
    /// pack validation fails, or the prepared consequence cannot be sealed.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_core_administration(
        trace: &CanonicalRoomTrace,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedCoreAdministrationV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let classified = request.classified()?;
        let request_hash = request.canonical_request_hash()?;
        let expected_use = AuthorityUseV1::CoreAdministration {
            room_id: request.room_id().clone(),
            classified: classified.clone(),
            identity: request.operation_identity().clone(),
            request_hash: request_hash.clone(),
        };
        if authority.classified() != &classified
            || authority.attribution().principal_id
                != request.operation_identity().authenticated_principal
        {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let attribution = authority.attribution().clone();
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        let proposal = request.normalized_proposal(attribution, recorded_at);
        let prepared = trace.prepare(RecordedStimulusV1::CoreProposed(proposal.clone()))?;
        Self::for_core_administration(
            trace,
            request,
            proposal,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn for_core_administration(
        trace: &CanonicalRoomTrace,
        request: &CoreAdministrationRequestV1,
        proposal: CoreProposedV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        trace.validate_prepared(&prepared)?;
        if !prepared.is_new() || prepared.basis_complete_head() != trace.head() {
            return Err(PrepareRoomWriteErrorV1::PreparedBasisMismatch);
        }
        let RecordedStimulusV1::CoreProposed(stimulus) = prepared.recorded_stimulus() else {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        };
        if stimulus != &proposal
            || request.room_id() != trace.head().room_id()
            || request.expected_room_seq() != trace.head().room_seq()
            || request.operation_identity() != proposal.operation_identity()
            || request.kind() != proposal.kind()
            || request.reason_code() != proposal.reason_code()
            || request.changeset() != proposal.changeset()
        {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        }

        let basis_complete_head = trace.head().clone();
        let identity =
            OperationIdentityV1::Administration(Box::new(request.operation_identity().clone()));
        let request_hash = request.canonical_request_hash()?;
        let (intent, result) = match prepared.disposition() {
            CanonicalAdvanceDisposition::TransitionAccepted { transition, .. } => {
                let resulting_state = prepared
                    .resulting_state()
                    .ok_or(PrepareRoomWriteErrorV1::InvalidPreparedTransition)?;
                let resulting_complete_head = transition.complete_head();
                if resulting_state.head() != &resulting_complete_head
                    || transition.previous_lineage_hash()
                        != basis_complete_head.genesis_or_transition_hash()
                {
                    return Err(PrepareRoomWriteErrorV1::InvalidPreparedTransition);
                }
                let delivery_consequences = prepare_canonical_transition_consequences(
                    trace,
                    &prepared,
                    resulting_state.core_state(),
                    transition.room_seq(),
                    current_frame_heads,
                )?;
                let resulting_memberships = resulting_state
                    .core_state()
                    .memberships()
                    .values()
                    .map(|membership| {
                        Ok(PreparedMembershipMaterializationV1 {
                            membership: membership.clone(),
                            canonical_membership_bytes: encode(membership)?,
                        })
                    })
                    .collect::<Result<Vec<_>, CanonicalJsonError>>()?;
                let persistence = PreparedCanonicalAdvancePersistence {
                    transition_id: transition_id.clone(),
                    transition: (**transition).clone(),
                    canonical_transition_bytes: transition.canonical_bytes()?,
                    resulting_complete_head: resulting_complete_head.clone(),
                    canonical_resulting_head_bytes: encode(&resulting_complete_head)?,
                    resulting_core_state: resulting_state.core_state().clone(),
                    canonical_resulting_core_state_bytes: encode(resulting_state.core_state())?,
                    resulting_activity_state: resulting_state.activity_state().clone(),
                    canonical_resulting_activity_state_bytes: resulting_state
                        .activity_state()
                        .to_bytes()?,
                    resulting_memberships,
                    timer_changes: transition
                        .ordered_timer_changes()
                        .iter()
                        .map(PreparedTimerMutationV1::from_change)
                        .collect::<Result<Vec<_>, _>>()?,
                    delivery_consequences,
                    activation_decisions: prepare_canonical_activation_decisions(transition)?,
                };
                (
                    PreparedCanonicalExistingIntent::Advance(Box::new(persistence)),
                    SemanticResultV1::TransitionCommitted {
                        room_id: resulting_complete_head.room_id().clone(),
                        transition_id,
                        room_seq: resulting_complete_head.room_seq(),
                        previous_lineage_hash: transition.previous_lineage_hash().clone(),
                        complete_head: resulting_complete_head,
                    },
                )
            }
            CanonicalAdvanceDisposition::RejectionRecorded { rejection, .. } => (
                PreparedCanonicalExistingIntent::DurableDisposition,
                SemanticResultV1::RejectionRecorded {
                    code: "activity_domain_rejection".to_owned(),
                    safe_details: CanonicalJsonV1::from_serialize(
                        &ActivityDomainRejectionDetailsV1 {
                            declared_code: rejection.declared_code.clone(),
                            safe_details: rejection.bounded_safe_details.clone(),
                        },
                    )?,
                },
            ),
            CanonicalAdvanceDisposition::NoChangeRecorded { .. } => (
                PreparedCanonicalExistingIntent::DurableDisposition,
                SemanticResultV1::NoChangeRecorded {
                    code: "administrative_no_change".to_owned(),
                    safe_details: CanonicalJsonV1::parse(br"{}")?,
                },
            ),
        };
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::CoreAdministration {
                proposal: proposal.clone(),
            },
            result,
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::CoreAdministrationMismatch);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::CoreAdministration(Box::new(
                PreparedCoreAdministrationInputWitnessV1 {
                    request: request.clone(),
                    proposal,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                },
            )),
            intent,
            semantic_result,
            pending_transition: Some(prepared),
        })
    }

    /// Seals a stable host-controlled Action rejection without invoking the
    /// Activity reducer. Malformed payload/schema input and an actually
    /// admissible Action are rejected before a storage plan exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the request is malformed, admissible, or cannot be sealed exactly.
    pub(crate) fn for_stable_action_disposition(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        request.canonical_request_hash()?;
        let membership_before = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?
            .clone();
        if membership_before.principal_id() != authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let assessment = trace
            .assess_stable_action_disposition(request, &admitted_at)?
            .ok_or(PrepareRoomWriteErrorV1::ActionCurrentlyAdmissible)?;
        let action_offer_witness = if let Some(bytes) = assessment.current_action_offers {
            ActionOfferWitnessV1::Current {
                canonical_action_offers: CanonicalJsonV1::from_canonical_bytes(&bytes)?,
            }
        } else {
            let reason = match (assessment.code, assessment.unavailable_reason) {
                ("membership_not_enabled", Some("membership_not_enabled")) => {
                    ActionOffersUnavailableReasonV1::MembershipNotEnabled
                }
                ("room_archived", Some("room_archived")) => {
                    ActionOffersUnavailableReasonV1::RoomArchived
                }
                _ => return Err(PrepareRoomWriteErrorV1::MissingActionOfferWitness),
            };
            ActionOfferWitnessV1::Unavailable { reason }
        };
        let identity = request.operation_identity();
        let request_hash = request.canonical_request_hash()?;
        let basis_complete_head = trace.head().clone();
        let semantic_result = StoredSemanticResultV1::prepare(
            identity.clone(),
            Some(basis_complete_head.clone()),
            ReceiptSemanticInputV1::ParticipantAction {
                request: request.clone(),
                admitted_at: admitted_at.clone(),
                normalized_action: assessment.normalized_action.clone().map(Box::new),
                action_offer_witness: action_offer_witness.clone(),
                admission_context: ActionAdmissionContextV1 {
                    membership_before: membership_before.clone(),
                    room_status: trace.core_state().room_status(),
                },
            },
            SemanticResultV1::RejectionRecorded {
                code: assessment.code.to_owned(),
                safe_details: CanonicalJsonV1::parse(br"{}")?,
            },
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::ActionRequestMismatch);
        }
        Ok(Self {
            identity,
            request_hash,
            basis_complete_head,
            integrity_generation,
            authority_witness,
            input_witness: PreparedOperationInputWitnessV1::ParticipantAction(Box::new(
                PreparedActionInputWitnessV1 {
                    request: request.clone(),
                    admitted_at,
                    normalized_action: assessment.normalized_action,
                    canonical_membership_before_bytes: encode(&membership_before)?,
                    canonical_core_before_bytes: encode(trace.core_state())?,
                    canonical_activity_before_bytes: trace.activity_state().to_bytes()?,
                    action_offer_witness,
                    membership_before,
                },
            )),
            intent: PreparedCanonicalExistingIntent::DurableDisposition,
            semantic_result,
            pending_transition: None,
        })
    }

    /// Seals one checked participant Action using the exact purpose-specific
    /// authority grant that admitted its caller request.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another request or Membership,
    /// or if the prepared transition/persistence bundle is inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn for_authorized_action(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority: AuthorizedParticipantActionV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let current_membership = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?;
        if authority.membership() != current_membership {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = participant_action_authority_use(request)?;
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        Self::for_action(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Seals a stable disabled-Membership Action disposition using the exact
    /// authority grant produced for that caller request.
    ///
    /// # Errors
    ///
    /// Returns an error if the grant targets another request or Membership,
    /// or the current trace does not yield a stable receiptable disposition.
    pub fn for_authorized_stable_action_disposition(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority: ParticipantActionAuthorityV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let (authorized_membership, fence) = match authority {
            ParticipantActionAuthorityV1::EnabledParticipant(authority) => {
                (authority.membership().clone(), authority.into_fence_facts())
            }
            ParticipantActionAuthorityV1::StableMembershipNotEnabled(authority) => {
                (authority.membership().clone(), authority.into_fence_facts())
            }
        };
        let current_membership = trace
            .core_state()
            .membership(request.member_id())
            .ok_or(PrepareRoomWriteErrorV1::MembershipMissing)?;
        if &authorized_membership != current_membership {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let expected_use = participant_action_authority_use(request)?;
        let authority_witness =
            PreparedAuthorityWitnessV1::from_fence_for_use(fence, &expected_use)?;
        Self::for_stable_action_disposition(
            trace,
            request,
            admitted_at,
            integrity_generation,
            authority_witness,
        )
    }

    /// Conformance-only entry point for sealing a prepared participant Action.
    /// Production callers receive sealed plans from the Room admission lane.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn for_action_for_conformance(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_action(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a prepared Core
    /// administration request without exercising an external authority
    /// provider. Production callers must consume an authority grant through
    /// [`Self::for_authorized_core_administration`].
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn for_core_administration_for_conformance(
        trace: &CanonicalRoomTrace,
        request: &CoreAdministrationRequestV1,
        recorded_at: CoreRecordedAt,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if authority_witness.authenticated_principal()
            != &request.operation_identity().authenticated_principal
        {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let proposal = request.normalized_proposal(
            crate::CoreAuthorityAttributionV1 {
                principal_id: request.operation_identity().authenticated_principal.clone(),
                authority_kind: crate::CoreAuthorityKindV1::RoomAdministrator,
            },
            recorded_at,
        );
        let prepared = trace.prepare(RecordedStimulusV1::CoreProposed(proposal.clone()))?;
        Self::for_core_administration(
            trace,
            request,
            proposal,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a prepared Timer firing.
    /// Production callers receive sealed plans from the Timer lane.
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn for_timer_fired_for_conformance(
        trace: &CanonicalRoomTrace,
        request: &TimerFiredRequestV1,
        prepared: PreparedCanonicalRoomTransition,
        transition_id: TransitionId,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
        current_frame_heads: &BTreeMap<MemberId, u64>,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_timer_fired(
            trace,
            request,
            prepared,
            transition_id,
            integrity_generation,
            authority_witness,
            current_frame_heads,
        )
    }

    /// Conformance-only entry point for sealing a stable Action disposition.
    /// Production callers receive sealed plans from the Room admission lane.
    #[cfg(feature = "conformance-tracer")]
    #[doc(hidden)]
    pub fn for_stable_action_disposition_for_conformance(
        trace: &CanonicalRoomTrace,
        request: &ParticipantActionRequestV1,
        admitted_at: ActionAdmittedAt,
        integrity_generation: IntegrityGenerationV1,
        authority_witness: PreparedAuthorityWitnessV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::for_stable_action_disposition(
            trace,
            request,
            admitted_at,
            integrity_generation,
            authority_witness,
        )
    }

    /// Returns the indivisible eight-field existing-Room witness.
    #[must_use]
    pub const fn basis_complete_head(&self) -> &CompleteHeadV1 {
        &self.basis_complete_head
    }

    #[must_use]
    pub const fn integrity_generation(&self) -> IntegrityGenerationV1 {
        self.integrity_generation
    }

    #[must_use]
    pub const fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        &self.authority_witness
    }

    #[must_use]
    pub const fn input_witness(&self) -> &PreparedOperationInputWitnessV1 {
        &self.input_witness
    }

    #[must_use]
    pub const fn intent(&self) -> &PreparedCanonicalExistingIntent {
        &self.intent
    }

    #[must_use]
    pub const fn semantic_result(&self) -> &StoredSemanticResultV1 {
        &self.semantic_result
    }
}

fn prepare_canonical_activation_decisions(
    transition: &TransitionRecord,
) -> Result<Vec<PreparedActivationDecisionV1>, CanonicalJsonError> {
    prepare_activation_decisions_from_effects(
        transition.room_seq(),
        transition.ordered_attention_signals(),
    )
}

pub(crate) fn prepare_canonical_transition_consequences(
    trace: &CanonicalRoomTrace,
    prepared: &PreparedCanonicalRoomTransition,
    resulting_core: &CoreRoomStateV1,
    cause_room_seq: RoomSequenceV1,
    current_frame_heads: &BTreeMap<MemberId, u64>,
) -> Result<Vec<PreparedObservationConsequenceV1>, PrepareRoomWriteErrorV1> {
    trace.validate_prepared(prepared)?;
    prepare_observation_consequences(
        trace.core_state(),
        resulting_core,
        cause_room_seq,
        current_frame_heads,
        |viewer| trace.observe_prepared(prepared, viewer),
    )
}

impl PreparedCanonicalRoomCreation {
    pub(crate) fn from_trace(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestWithFormat,
        authority_witness: PreparedAuthorityWitnessV1,
        trace: CanonicalRoomTrace,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        if identity.authenticated_principal != *authority_witness.authenticated_principal() {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        if identity.versioned_operation_kind != CREATE_ROOM_OPERATION_KIND {
            return Err(PrepareRoomWriteErrorV1::InvalidCreationOperationKind);
        }
        if trace.head().room_seq().get() != 0 || !trace.transitions().is_empty() {
            return Err(PrepareRoomWriteErrorV1::CreationTraceAdvanced);
        }
        let retained_pack = trace.retained_pack();
        if request.format() != trace.genesis().format() {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        trace.validate_initial_authorized_views()?;
        let legacy_request = request.legacy_request();
        if legacy_request.pack_digest != *trace.head().pack_digest()
            || legacy_request.configuration != *trace.genesis().configuration()
            || legacy_request.ordered_initial_memberships.len()
                != trace.core_state().memberships().len()
        {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        let mut memberships_by_principal = BTreeMap::new();
        for membership in trace.core_state().memberships().values() {
            if memberships_by_principal
                .insert(membership.principal_id().clone(), membership)
                .is_some()
            {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            }
        }
        let mut initial_member_ids =
            Vec::with_capacity(legacy_request.ordered_initial_memberships.len());
        for proposal in &legacy_request.ordered_initial_memberships {
            let Some(membership) = memberships_by_principal.remove(&proposal.principal_id) else {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            };
            if !proposal.matches_membership(membership) {
                return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
            }
            initial_member_ids.push(membership.member_id().clone());
        }
        if !memberships_by_principal.is_empty() {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        let request_hash = request.canonical_request_hash()?;
        let operation_identity = OperationIdentityV1::Administration(Box::new(identity));
        let complete_head = trace.head().clone();
        let mut memberships = Vec::with_capacity(trace.core_state().memberships().len());
        for membership in trace.core_state().memberships().values() {
            memberships.push(PreparedMembershipMaterializationV1 {
                membership: membership.clone(),
                canonical_membership_bytes: encode(membership)?,
            });
        }
        let semantic_result = StoredSemanticResultV1::prepare(
            operation_identity.clone(),
            None,
            match request.format() {
                CanonicalHistoryFormat::V1 => ReceiptSemanticInputV1::RoomCreation {
                    request: legacy_request.clone(),
                    created_at: trace.genesis().created_at().clone(),
                },
                CanonicalHistoryFormat::V2 => ReceiptSemanticInputV1::RoomCreationV2 {
                    request: request.clone(),
                    created_at: trace.genesis().created_at().clone(),
                },
            },
            SemanticResultV1::GenesisCreated {
                room_id: complete_head.room_id().clone(),
                initial_member_ids,
                complete_head: complete_head.clone(),
            },
        )?;
        if semantic_result.canonical_request_hash() != &request_hash {
            return Err(PrepareRoomWriteErrorV1::CreationRequestMismatch);
        }
        Ok(Self {
            identity: operation_identity,
            request_hash,
            authority_witness,
            persistence: PreparedCanonicalCreationPersistence {
                pack_revision_lock: retained_pack.revision_lock().clone(),
                canonical_pack_revision_lock_bytes: encode(retained_pack.revision_lock())?,
                genesis: trace.genesis().clone(),
                canonical_genesis_bytes: trace.genesis_bytes()?,
                complete_head: complete_head.clone(),
                canonical_head_bytes: encode(&complete_head)?,
                core_state: trace.core_state().clone(),
                canonical_core_state_bytes: encode(trace.core_state())?,
                activity_state: trace.activity_state().clone(),
                canonical_activity_state_bytes: trace.activity_state().to_bytes()?,
                memberships,
                initial_timers: trace
                    .genesis()
                    .initial_timers()
                    .iter()
                    .map(PreparedTimerMaterializationV1::from_scheduled)
                    .collect::<Result<Vec<_>, _>>()?,
                integrity_generation: IntegrityGenerationV1::new(1)
                    .map_err(|_| PrepareRoomWriteErrorV1::InvalidIntegrityGeneration)?,
            },
            semantic_result,
            pending_trace: Some(trace),
        })
    }

    pub fn from_registry_genesis(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestWithFormat,
        authority: AuthorizedRoomCreationV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let request_hash = request.canonical_request_hash()?;
        let expected_use = AuthorityUseV1::CreateRoom {
            identity: identity.clone(),
            request_hash,
        };
        if authority.attribution().principal_id != identity.authenticated_principal {
            return Err(PrepareRoomWriteErrorV1::AuthorityIdentityMismatch);
        }
        let authority_witness = PreparedAuthorityWitnessV1::from_fence_for_use(
            authority.into_fence_facts(),
            &expected_use,
        )?;
        Self::from_registry_genesis_with_witness(
            identity,
            request,
            authority_witness,
            prepared_genesis,
        )
    }

    fn from_registry_genesis_with_witness(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestWithFormat,
        authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        let trace = CanonicalRoomTrace::create_uncommitted(prepared_genesis, request.format())?;
        Self::from_trace(identity, request, authority_witness, trace)
    }

    /// Conformance-only raw witness entry point. Production creation accepts
    /// only a purpose-sealed [`AuthorizedRoomCreationV1`].
    #[cfg(any(test, feature = "conformance-tracer"))]
    #[doc(hidden)]
    pub fn from_registry_genesis_for_conformance(
        identity: AdministrationOperationIdentityV1,
        request: &RoomCreationRequestWithFormat,
        authority_witness: PreparedAuthorityWitnessV1,
        prepared_genesis: PreparedNewRoomGenesisV1,
    ) -> Result<Self, PrepareRoomWriteErrorV1> {
        Self::from_registry_genesis_with_witness(
            identity,
            request,
            authority_witness,
            prepared_genesis,
        )
    }

    #[must_use]
    pub const fn authority_witness(&self) -> &PreparedAuthorityWitnessV1 {
        &self.authority_witness
    }
}

impl PreparedCanonicalCreationPersistence {
    /// Returns the sealed pack revision lock.
    #[must_use]
    pub fn pack_revision_lock(&self) -> &PackRevisionLockV1 {
        &self.pack_revision_lock
    }
    /// Returns the sealed canonical pack revision lock bytes.
    #[must_use]
    pub fn canonical_pack_revision_lock_bytes(&self) -> &[u8] {
        &self.canonical_pack_revision_lock_bytes
    }
    /// Returns the sealed genesis.
    #[must_use]
    pub fn genesis(&self) -> &GenesisRecord {
        &self.genesis
    }
    /// Returns the sealed canonical genesis bytes.
    #[must_use]
    pub fn canonical_genesis_bytes(&self) -> &[u8] {
        &self.canonical_genesis_bytes
    }
    /// Returns the sealed complete head.
    #[must_use]
    pub fn complete_head(&self) -> &CompleteHeadV1 {
        &self.complete_head
    }
    /// Returns the sealed canonical head bytes.
    #[must_use]
    pub fn canonical_head_bytes(&self) -> &[u8] {
        &self.canonical_head_bytes
    }
    /// Returns the sealed core state.
    #[must_use]
    pub fn core_state(&self) -> &CoreRoomStateV1 {
        &self.core_state
    }
    /// Returns the sealed canonical core state bytes.
    #[must_use]
    pub fn canonical_core_state_bytes(&self) -> &[u8] {
        &self.canonical_core_state_bytes
    }
    /// Returns the sealed activity state.
    #[must_use]
    pub fn activity_state(&self) -> &CanonicalJsonV1 {
        &self.activity_state
    }
    /// Returns the sealed canonical activity state bytes.
    #[must_use]
    pub fn canonical_activity_state_bytes(&self) -> &[u8] {
        &self.canonical_activity_state_bytes
    }
    /// Returns the sealed memberships.
    #[must_use]
    pub fn memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.memberships
    }
    /// Returns the sealed initial timers.
    #[must_use]
    pub fn initial_timers(&self) -> &[PreparedTimerMaterializationV1] {
        &self.initial_timers
    }
    /// Returns the sealed integrity generation.
    #[must_use]
    pub fn integrity_generation(&self) -> IntegrityGenerationV1 {
        self.integrity_generation
    }
}

impl PreparedCanonicalAdvancePersistence {
    /// Returns the sealed transition id.
    #[must_use]
    pub fn transition_id(&self) -> &TransitionId {
        &self.transition_id
    }
    /// Returns the sealed transition.
    #[must_use]
    pub fn transition(&self) -> &TransitionRecord {
        &self.transition
    }
    /// Returns the sealed canonical transition bytes.
    #[must_use]
    pub fn canonical_transition_bytes(&self) -> &[u8] {
        &self.canonical_transition_bytes
    }
    /// Returns the sealed resulting complete head.
    #[must_use]
    pub fn resulting_complete_head(&self) -> &CompleteHeadV1 {
        &self.resulting_complete_head
    }
    /// Returns the sealed canonical resulting head bytes.
    #[must_use]
    pub fn canonical_resulting_head_bytes(&self) -> &[u8] {
        &self.canonical_resulting_head_bytes
    }
    /// Returns the sealed resulting core state.
    #[must_use]
    pub fn resulting_core_state(&self) -> &CoreRoomStateV1 {
        &self.resulting_core_state
    }
    /// Returns the sealed canonical resulting core state bytes.
    #[must_use]
    pub fn canonical_resulting_core_state_bytes(&self) -> &[u8] {
        &self.canonical_resulting_core_state_bytes
    }
    /// Returns the sealed resulting activity state.
    #[must_use]
    pub fn resulting_activity_state(&self) -> &CanonicalJsonV1 {
        &self.resulting_activity_state
    }
    /// Returns the sealed canonical resulting activity state bytes.
    #[must_use]
    pub fn canonical_resulting_activity_state_bytes(&self) -> &[u8] {
        &self.canonical_resulting_activity_state_bytes
    }
    /// Returns the sealed resulting memberships.
    #[must_use]
    pub fn resulting_memberships(&self) -> &[PreparedMembershipMaterializationV1] {
        &self.resulting_memberships
    }
    /// Returns the sealed timer changes.
    #[must_use]
    pub fn timer_changes(&self) -> &[PreparedTimerMutationV1] {
        &self.timer_changes
    }
    /// Returns the sealed delivery consequences.
    #[must_use]
    pub fn delivery_consequences(&self) -> &[PreparedObservationConsequenceV1] {
        &self.delivery_consequences
    }
    /// Returns the sealed activation decisions.
    #[must_use]
    pub fn activation_decisions(&self) -> &[PreparedActivationDecisionV1] {
        &self.activation_decisions
    }
}

impl PreparedCanonicalRoomCreation {
    /// Returns the sealed identity.
    #[must_use]
    pub fn identity(&self) -> &OperationIdentityV1 {
        &self.identity
    }
    /// Returns the sealed request hash.
    #[must_use]
    pub fn request_hash(&self) -> &CanonicalRequestHashV1 {
        &self.request_hash
    }
    /// Returns the sealed persistence.
    #[must_use]
    pub fn persistence(&self) -> &PreparedCanonicalCreationPersistence {
        &self.persistence
    }
    /// Returns the sealed semantic result.
    #[must_use]
    pub fn semantic_result(&self) -> &StoredSemanticResultV1 {
        &self.semantic_result
    }
}

impl PreparedCanonicalRoomCommit {
    /// Returns the sealed identity.
    #[must_use]
    pub fn identity(&self) -> &OperationIdentityV1 {
        &self.identity
    }
    /// Returns the sealed request hash.
    #[must_use]
    pub fn request_hash(&self) -> &CanonicalRequestHashV1 {
        &self.request_hash
    }
}

/// One sealed format-neutral write. Existing V1 ports cannot consume this type.
pub enum PreparedCanonicalRoomWrite {
    /// Initial all-or-none Genesis bundle.
    Create(Box<PreparedCanonicalRoomCreation>),
    /// Advance or durable disposition at an exact basis and integrity fence.
    Existing(Box<PreparedCanonicalRoomCommit>),
}
impl PreparedCanonicalRoomWrite {
    /// Returns the original operation identity.
    #[must_use]
    pub fn identity(&self) -> &OperationIdentityV1 {
        match self {
            Self::Create(plan) => &plan.identity,
            Self::Existing(plan) => &plan.identity,
        }
    }
    /// Returns the original caller-semantic hash.
    #[must_use]
    pub fn request_hash(&self) -> &CanonicalRequestHashV1 {
        match self {
            Self::Create(plan) => &plan.request_hash,
            Self::Existing(plan) => &plan.request_hash,
        }
    }
    /// Returns the exact prepared receipt for a new durable result.
    #[must_use]
    pub fn semantic_result(&self) -> &StoredSemanticResultV1 {
        match self {
            Self::Create(plan) => &plan.semantic_result,
            Self::Existing(plan) => &plan.semantic_result,
        }
    }
}
impl From<PreparedCanonicalRoomCreation> for PreparedCanonicalRoomWrite {
    fn from(plan: PreparedCanonicalRoomCreation) -> Self {
        Self::Create(Box::new(plan))
    }
}
impl From<PreparedCanonicalRoomCommit> for PreparedCanonicalRoomWrite {
    fn from(plan: PreparedCanonicalRoomCommit) -> Self {
        Self::Existing(Box::new(plan))
    }
}

/// Atomic persistence port for the additive bundles. It has the same operation
/// exclusion, Room-root, authority, whole-Head, and integrity fences as the V1
/// port. Adapters must explicitly implement complete format-aware lifecycle
/// support before accepting compact writes.
pub trait CanonicalRoomCommitStorage: Send + Sync {
    /// Atomically admits every canonical and operational consequence, or none.
    fn commit(&self, prepared: &PreparedCanonicalRoomWrite) -> RoomCommitResolutionV1;
    /// Resolves only the original durable identity/hash under operation exclusion.
    fn resolve(
        &self,
        identity: &OperationIdentityV1,
        request_hash: &CanonicalRequestHashV1,
    ) -> ResolveOutcomeV1;
}

/// Exact sealed work retained after a proven-absent commit outcome.
pub struct CanonicalRoomRetry {
    prepared: PreparedCanonicalRoomWrite,
}
/// Exact sealed work retained after an ambiguous outcome. It offers resolution only.
pub struct CanonicalRoomResolve {
    prepared: PreparedCanonicalRoomWrite,
}
/// Explicit continuation required by a nonterminal storage outcome.
pub enum CanonicalRoomPendingAttempt {
    /// The original sealed work can be retried without reduction or time sampling.
    Retryable(CanonicalRoomRetry),
    /// Storage must resolve the original identity before any retry.
    ResolveOnly(CanonicalRoomResolve),
    /// The basis or generated identity must be prepared again under the original caller identity/hash.
    Reprepare(PreparedCanonicalRoomWrite),
}

/// Coordinator result. An executable creation trace is released only for the
/// exact new durable receipt; an Existing result never installs speculative state.
pub struct CanonicalRoomCommitOutcome {
    resolution: RoomCommitResolutionV1,
    actor_installation: ActorInstallationV1,
    committed_trace: Option<CanonicalRoomTrace>,
    pending_attempt: Option<CanonicalRoomPendingAttempt>,
}
impl CanonicalRoomCommitOutcome {
    /// Returns the exact storage disposition.
    #[must_use]
    pub const fn resolution(&self) -> &RoomCommitResolutionV1 {
        &self.resolution
    }
    /// Returns the serving-executor installation result.
    #[must_use]
    pub const fn actor_installation(&self) -> ActorInstallationV1 {
        self.actor_installation
    }
    /// Releases the result and its explicit continuation ownership.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RoomCommitResolutionV1,
        ActorInstallationV1,
        Option<CanonicalRoomTrace>,
        Option<CanonicalRoomPendingAttempt>,
    ) {
        (
            self.resolution,
            self.actor_installation,
            self.committed_trace,
            self.pending_attempt,
        )
    }
}

/// Commits creation and releases only the exact new durable executor.
pub fn commit_canonical_room_creation(
    storage: &dyn CanonicalRoomCommitStorage,
    prepared: PreparedCanonicalRoomCreation,
) -> CanonicalRoomCommitOutcome {
    attempt_canonical_room_commit(storage, None, prepared.into())
}
/// Commits one Existing plan and installs only its exact new durable Transition.
pub fn commit_canonical_existing_room(
    storage: &dyn CanonicalRoomCommitStorage,
    trace: &mut CanonicalRoomTrace,
    prepared: PreparedCanonicalRoomCommit,
) -> CanonicalRoomCommitOutcome {
    attempt_canonical_room_commit(storage, Some(trace), prepared.into())
}

impl CanonicalRoomRetry {
    /// Retries the identical sealed plan. Supply the current executor for an Existing plan.
    pub fn retry(
        self,
        storage: &dyn CanonicalRoomCommitStorage,
        trace: Option<&mut CanonicalRoomTrace>,
    ) -> CanonicalRoomCommitOutcome {
        attempt_canonical_room_commit(storage, trace, self.prepared)
    }
}
impl CanonicalRoomResolve {
    /// Resolves an ambiguous result. No speculative state is installed from
    /// a resolved Existing receipt, and only guarded absence permits retry.
    pub fn resolve(self, storage: &dyn CanonicalRoomCommitStorage) -> CanonicalRoomCommitOutcome {
        let prepared = self.prepared;
        let (resolution, actor_installation, pending_attempt) =
            match storage.resolve(prepared.identity(), prepared.request_hash()) {
                ResolveOutcomeV1::StoredResolution(result)
                    if duplicate_result_matches_prepared(&result, prepared.semantic_result()) =>
                {
                    let install = if matches!(
                        result.result(),
                        SemanticResultV1::TransitionCommitted { .. }
                            | SemanticResultV1::GenesisCreated { .. }
                    ) {
                        ActorInstallationV1::ReloadRequired
                    } else {
                        ActorInstallationV1::Unchanged
                    };
                    (
                        RoomCommitResolutionV1::resolved(ResolutionStatusV1::Existing, *result),
                        install,
                        None,
                    )
                }
                ResolveOutcomeV1::Conflict {
                    existing_request_hash,
                } if &existing_request_hash != prepared.request_hash() => (
                    RoomCommitResolutionV1::Conflict {
                        existing_request_hash,
                    },
                    ActorInstallationV1::Unchanged,
                    None,
                ),
                ResolveOutcomeV1::KnownAbsent => (
                    RoomCommitResolutionV1::RetryableKnownAbsent,
                    ActorInstallationV1::Unchanged,
                    Some(CanonicalRoomPendingAttempt::Retryable(CanonicalRoomRetry {
                        prepared,
                    })),
                ),
                ResolveOutcomeV1::ResolutionUnavailable => (
                    RoomCommitResolutionV1::Indeterminate,
                    ActorInstallationV1::Withheld,
                    Some(CanonicalRoomPendingAttempt::ResolveOnly(Self { prepared })),
                ),
                _ => (
                    RoomCommitResolutionV1::Fault,
                    ActorInstallationV1::QuarantineRequired,
                    None,
                ),
            };
        CanonicalRoomCommitOutcome {
            resolution,
            actor_installation,
            committed_trace: None,
            pending_attempt,
        }
    }
}

fn attempt_canonical_room_commit(
    storage: &dyn CanonicalRoomCommitStorage,
    trace: Option<&mut CanonicalRoomTrace>,
    mut prepared: PreparedCanonicalRoomWrite,
) -> CanonicalRoomCommitOutcome {
    let mut resolution = storage.commit(&prepared);
    let allowed = match &prepared {
        PreparedCanonicalRoomWrite::Create(_) => matches!(
            resolution,
            RoomCommitResolutionV1::GenesisCreated { .. }
                | RoomCommitResolutionV1::Conflict { .. }
                | RoomCommitResolutionV1::Fenced
                | RoomCommitResolutionV1::Reprepare
                | RoomCommitResolutionV1::RetryableKnownAbsent
                | RoomCommitResolutionV1::Indeterminate
                | RoomCommitResolutionV1::Fault
        ),
        PreparedCanonicalRoomWrite::Existing(plan) => {
            matches!(
                resolution,
                RoomCommitResolutionV1::TransitionCommitted { .. }
                    | RoomCommitResolutionV1::RejectionRecorded { .. }
                    | RoomCommitResolutionV1::NoChangeRecorded { .. }
                    | RoomCommitResolutionV1::Conflict { .. }
                    | RoomCommitResolutionV1::Fenced
                    | RoomCommitResolutionV1::Reprepare
                    | RoomCommitResolutionV1::RetryableKnownAbsent
                    | RoomCommitResolutionV1::Indeterminate
                    | RoomCommitResolutionV1::Fault
            ) || (matches!(
                plan.input_witness,
                PreparedOperationInputWitnessV1::TimerFired(_)
            ) && matches!(resolution, RoomCommitResolutionV1::NotApplicable))
        }
    };
    let coherent_result = match &resolution {
        RoomCommitResolutionV1::GenesisCreated { result, .. } => {
            matches!(result.result, SemanticResultV1::GenesisCreated { .. })
        }
        RoomCommitResolutionV1::TransitionCommitted { result, .. } => {
            matches!(result.result, SemanticResultV1::TransitionCommitted { .. })
        }
        RoomCommitResolutionV1::RejectionRecorded { result, .. } => {
            matches!(result.result, SemanticResultV1::RejectionRecorded { .. })
        }
        RoomCommitResolutionV1::NoChangeRecorded { result, .. } => {
            matches!(result.result, SemanticResultV1::NoChangeRecorded { .. })
        }
        _ => true,
    };
    if !allowed
        || !coherent_result
        || matches!(&resolution, RoomCommitResolutionV1::Conflict { existing_request_hash }
        if existing_request_hash == prepared.request_hash())
    {
        resolution = RoomCommitResolutionV1::Fault;
    }
    let reported_new = matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::New,
            ..
        } | RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::New,
            ..
        } | RoomCommitResolutionV1::RejectionRecorded {
            status: ResolutionStatusV1::New,
            ..
        } | RoomCommitResolutionV1::NoChangeRecorded {
            status: ResolutionStatusV1::New,
            ..
        }
    );
    let reported_existing = matches!(
        resolution,
        RoomCommitResolutionV1::GenesisCreated {
            status: ResolutionStatusV1::Existing,
            ..
        } | RoomCommitResolutionV1::TransitionCommitted {
            status: ResolutionStatusV1::Existing,
            ..
        } | RoomCommitResolutionV1::RejectionRecorded {
            status: ResolutionStatusV1::Existing,
            ..
        } | RoomCommitResolutionV1::NoChangeRecorded {
            status: ResolutionStatusV1::Existing,
            ..
        }
    );
    let exact_new = resolution
        .stored_result()
        .is_some_and(|result| result == prepared.semantic_result());
    let exact_existing = resolution.stored_result().is_some_and(|result| {
        duplicate_result_matches_prepared(result, prepared.semantic_result())
    });
    let mut committed_trace = None;
    let actor_installation =
        if (reported_new && !exact_new) || (reported_existing && !exact_existing) {
            resolution = RoomCommitResolutionV1::Fault;
            ActorInstallationV1::QuarantineRequired
        } else if reported_new {
            match &mut prepared {
                PreparedCanonicalRoomWrite::Create(plan) => {
                    committed_trace = plan.pending_trace.take();
                    if committed_trace.is_some() {
                        ActorInstallationV1::Installed
                    } else {
                        resolution = RoomCommitResolutionV1::Fault;
                        ActorInstallationV1::QuarantineRequired
                    }
                }
                PreparedCanonicalRoomWrite::Existing(plan) => {
                    if matches!(plan.intent, PreparedCanonicalExistingIntent::Advance(_)) {
                        match (trace, plan.pending_transition.take()) {
                            (Some(trace), Some(pending))
                                if trace.head() == &plan.basis_complete_head =>
                            {
                                match trace.install_prepared(pending) {
                                    Ok(_) => ActorInstallationV1::Installed,
                                    Err(_) => ActorInstallationV1::ReloadRequired,
                                }
                            }
                            _ => ActorInstallationV1::ReloadRequired,
                        }
                    } else {
                        ActorInstallationV1::Unchanged
                    }
                }
            }
        } else {
            match resolution {
                RoomCommitResolutionV1::Indeterminate => ActorInstallationV1::Withheld,
                RoomCommitResolutionV1::Fault => ActorInstallationV1::QuarantineRequired,
                RoomCommitResolutionV1::GenesisCreated { .. }
                | RoomCommitResolutionV1::TransitionCommitted { .. } => {
                    ActorInstallationV1::ReloadRequired
                }
                _ => ActorInstallationV1::Unchanged,
            }
        };
    let pending_attempt = match resolution {
        RoomCommitResolutionV1::RetryableKnownAbsent => {
            Some(CanonicalRoomPendingAttempt::Retryable(CanonicalRoomRetry {
                prepared,
            }))
        }
        RoomCommitResolutionV1::Indeterminate => Some(CanonicalRoomPendingAttempt::ResolveOnly(
            CanonicalRoomResolve { prepared },
        )),
        RoomCommitResolutionV1::Reprepare => Some(CanonicalRoomPendingAttempt::Reprepare(prepared)),
        _ => None,
    };
    CanonicalRoomCommitOutcome {
        resolution,
        actor_installation,
        committed_trace,
        pending_attempt,
    }
}
