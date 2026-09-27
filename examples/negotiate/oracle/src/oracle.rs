use serde::Serialize;

use crate::{
    A202_PINNED_REVISION, Action, ActionBasis, ActionKind, AgreementSignature, ApprovalBinding,
    ApprovalDecision, Attention, AttentionReason, BoundaryProbe, BundleCondition, DeadlineProgress,
    EvidenceLink, ExactA202Object, ExactApproval, FixtureSignature, LogicalHead, OracleError,
    OracleState, Outcome, Phase, Proposal, RejectionCode, Role, RoomHead, Stimulus, TimerFired,
    TimerKind, Transition, canonical_bytes, tagged_blake3,
};

const ORACLE_ID: &str = "worldstream/negotiate-oracle/v1";
const MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_A202_OBJECT_BYTES: usize = 256 * 1024;
const BUYER_ID: &str = "agent:northstar:buyer";
const SELLER_ID: &str = "agent:delta:seller";
const APPROVER_ID: &str = "principal:northstar:procurement_director";
const VENUE_ID: &str = "agent:worldstream:venue_signer";

#[derive(Clone, Debug, Default)]
pub struct Oracle;

impl Oracle {
    #[must_use]
    pub fn initialize(formation_deadline: u64) -> OracleState {
        OracleState {
            oracle_id: ORACLE_ID.to_owned(),
            a202_revision: A202_PINNED_REVISION.to_owned(),
            transaction_id: "txn_calibration_worldstream_01".to_owned(),
            session_id: "ses_northstar_delta_worldstream_01".to_owned(),
            phase: Phase::FormationOpen,
            aggregate_state: "negotiating".to_owned(),
            session_state: "opened".to_owned(),
            room_head: RoomHead {
                sequence: 0,
                digest: tagged_blake3(b"worldstream/negotiate/genesis/v1"),
            },
            transaction_head: LogicalHead {
                sequence: 3,
                event_hash:
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                        .to_owned(),
            },
            session_head: LogicalHead {
                sequence: 1,
                event_hash:
                    "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                        .to_owned(),
            },
            formation_deadline,
            proposal_expired: false,
            current_proposal: None,
            pending_approval: None,
            recorded_approval: None,
            accepted_offer_id: None,
            accepted_offer_hash: None,
            agreement_id: None,
            agreement_content_hash: None,
            agreement_canonical_json: None,
            agreement_signatures: std::collections::BTreeMap::new(),
            deadline_progress: DeadlineProgress::None,
            outcome: None,
            evidence: Vec::new(),
        }
    }

    pub fn apply(
        &self,
        state: &OracleState,
        stimulus: &Stimulus,
    ) -> Result<Transition, OracleError> {
        match stimulus {
            Stimulus::Action(action) => self.apply_action(state, action),
            Stimulus::TimerFired(timer) => self.apply_timer(state, timer),
        }
    }

    #[must_use]
    pub fn offers(&self, state: &OracleState, role: Role) -> Vec<ActionKind> {
        if matches!(state.phase, Phase::Complete | Phase::Expired) {
            return Vec::new();
        }
        match state.phase {
            Phase::FormationOpen => self.formation_offers(state, role),
            Phase::ApprovalPending => self.approval_offers(state, role),
            Phase::OfferAccepted => {
                if role == Role::BuyerAgent {
                    vec![ActionKind::SelectAcceptedProposal]
                } else {
                    Vec::new()
                }
            }
            Phase::AgreementPending => self.agreement_offers(state, role),
            Phase::DeadlineResolution => self.deadline_offers(state, role),
            Phase::WithdrawnPendingExpiry | Phase::Complete | Phase::Expired => Vec::new(),
        }
    }

    fn formation_offers(&self, state: &OracleState, role: Role) -> Vec<ActionKind> {
        let Some(proposal) = &state.current_proposal else {
            return match role {
                Role::BuyerAgent | Role::SellerAgent => {
                    vec![ActionKind::SubmitProposalRevision]
                }
                Role::BuyerApprover | Role::VenueSigner => Vec::new(),
            };
        };
        if state.proposal_expired {
            return match role {
                Role::BuyerAgent | Role::SellerAgent => {
                    vec![ActionKind::SubmitProposalRevision]
                }
                Role::BuyerApprover | Role::VenueSigner => Vec::new(),
            };
        }
        if role == proposal.author {
            return vec![ActionKind::WithdrawLiveProposal];
        }
        if is_counterparty(role, proposal.author) {
            let mut offers = vec![ActionKind::SubmitProposalRevision];
            if role == Role::BuyerAgent && proposal.author == Role::SellerAgent {
                offers.push(ActionKind::RequestExactApproval);
            }
            return offers;
        }
        Vec::new()
    }

    fn approval_offers(&self, state: &OracleState, role: Role) -> Vec<ActionKind> {
        let Some(proposal) = &state.current_proposal else {
            return Vec::new();
        };
        match role {
            Role::SellerAgent => vec![ActionKind::WithdrawLiveProposal],
            Role::BuyerAgent
                if state
                    .recorded_approval
                    .as_ref()
                    .is_some_and(|approval| approval.decision == ApprovalDecision::Approved) =>
            {
                vec![
                    ActionKind::SubmitProposalRevision,
                    ActionKind::AcceptCurrentProposal,
                ]
            }
            Role::BuyerAgent => vec![ActionKind::SubmitProposalRevision],
            Role::BuyerApprover if state.recorded_approval.is_none() => {
                vec![ActionKind::RecordExactApproval]
            }
            _ if proposal.author != Role::SellerAgent => Vec::new(),
            _ => Vec::new(),
        }
    }

    fn agreement_offers(&self, state: &OracleState, role: Role) -> Vec<ActionKind> {
        match role {
            Role::BuyerAgent | Role::SellerAgent => {
                let mut offers = Vec::new();
                if !state.agreement_signatures.contains_key(&role) {
                    offers.push(ActionKind::RecordAgreementSignature);
                }
                if role == Role::BuyerAgent && state.agreement_signatures.len() == 2 {
                    offers.push(ActionKind::CommitAgreement);
                }
                offers
            }
            Role::BuyerApprover | Role::VenueSigner => Vec::new(),
        }
    }

    fn deadline_offers(&self, state: &OracleState, role: Role) -> Vec<ActionKind> {
        if role != Role::VenueSigner {
            return Vec::new();
        }
        match state.deadline_progress {
            DeadlineProgress::AwaitingSessionExpiry => {
                vec![ActionKind::RecordSessionDeadlineElapsed]
            }
            DeadlineProgress::AwaitingTransactionExpiry => {
                vec![ActionKind::RecordTransactionDeadlineElapsed]
            }
            DeadlineProgress::AwaitingSessionClose => {
                vec![ActionKind::CloseTransactionExpiredSession]
            }
            DeadlineProgress::None | DeadlineProgress::Resolved => Vec::new(),
        }
    }

    fn apply_action(
        &self,
        state: &OracleState,
        action: &Action,
    ) -> Result<Transition, OracleError> {
        validate_basis(state, action.basis())?;
        if !is_deadline_action(action.kind()) && action.admitted_at() >= state.formation_deadline {
            return Err(OracleError::new(
                RejectionCode::DeadlineReached,
                "ordinary action admitted at or after the half-open formation deadline",
            ));
        }

        let previous_room_head = state.room_head.clone();
        let mut next = state.clone();
        let mut attention = Vec::new();
        let mut materials = Vec::new();

        match action {
            Action::SubmitProposalRevision {
                actor,
                admitted_at,
                proposal,
                ..
            } => {
                validate_submit(state, *actor, *admitted_at, proposal)?;
                validate_object(&proposal.offer, *actor, "offer_submission")?;
                validate_object(&proposal.session_event, Role::VenueSigner, "event_append")?;
                next.phase = Phase::FormationOpen;
                "active".clone_into(&mut next.session_state);
                next.proposal_expired = false;
                next.pending_approval = None;
                next.recorded_approval = None;
                next.current_proposal = Some(proposal.clone());
                advance_session(&mut next, &proposal.session_event);
                attention.push(Attention {
                    target: counterparty(*actor),
                    reason: AttentionReason::ProposalReceived,
                });
                materials.extend([proposal.offer.clone(), proposal.session_event.clone()]);
            }
            Action::WithdrawLiveProposal {
                actor,
                withdrawal_event,
                ..
            } => {
                require_phase(state, &[Phase::FormationOpen, Phase::ApprovalPending])?;
                let proposal = state.current_proposal.as_ref().ok_or_else(|| {
                    OracleError::new(RejectionCode::InvalidPhase, "there is no live proposal")
                })?;
                if proposal.author != *actor {
                    return Err(role_violation("only the current offeror may withdraw"));
                }
                validate_object(withdrawal_event, Role::VenueSigner, "event_append")?;
                next.phase = Phase::WithdrawnPendingExpiry;
                "withdrawn".clone_into(&mut next.session_state);
                next.current_proposal = None;
                next.pending_approval = None;
                next.recorded_approval = None;
                advance_session(&mut next, withdrawal_event);
                materials.push(withdrawal_event.clone());
            }
            Action::RequestExactApproval {
                actor,
                admitted_at,
                binding,
                ..
            } => {
                require_phase(state, &[Phase::FormationOpen])?;
                if *actor != Role::BuyerAgent {
                    return Err(role_violation("only buyer_agent may request approval"));
                }
                validate_binding(state, binding, *admitted_at)?;
                next.phase = Phase::ApprovalPending;
                next.pending_approval = Some(binding.clone());
                next.recorded_approval = None;
            }
            Action::RecordExactApproval {
                actor,
                admitted_at,
                approval,
                ..
            } => {
                require_phase(state, &[Phase::ApprovalPending])?;
                if *actor != Role::BuyerApprover {
                    return Err(role_violation(
                        "only buyer_approver may record exact approval",
                    ));
                }
                let pending = state.pending_approval.as_ref().ok_or_else(|| {
                    OracleError::new(
                        RejectionCode::ApprovalBindingMismatch,
                        "no pending exact approval",
                    )
                })?;
                validate_approval(pending, approval, *admitted_at)?;
                next.recorded_approval = Some(approval.clone());
                materials.push(approval.approval.clone());
                if approval.decision == ApprovalDecision::Rejected {
                    next.phase = Phase::FormationOpen;
                    next.pending_approval = None;
                }
            }
            Action::AcceptCurrentProposal {
                actor,
                admitted_at,
                candidate_canonical_json,
                approval,
                acceptance,
                acceptance_event,
                ..
            } => {
                require_phase(state, &[Phase::ApprovalPending])?;
                if *actor != Role::BuyerAgent {
                    return Err(role_violation("only buyer_agent may accept"));
                }
                validate_acceptance(state, *admitted_at, candidate_canonical_json, approval)?;
                validate_object(acceptance, Role::BuyerAgent, "offer_acceptance")?;
                validate_object(acceptance_event, Role::VenueSigner, "event_append")?;
                let proposal = state.current_proposal.as_ref().ok_or_else(|| {
                    OracleError::new(RejectionCode::InvalidPhase, "current proposal missing")
                })?;
                next.phase = Phase::OfferAccepted;
                "accepted".clone_into(&mut next.session_state);
                next.accepted_offer_id = Some(proposal.offer.object_id.clone());
                next.accepted_offer_hash = Some(proposal.offer.declared_content_hash.clone());
                next.pending_approval = None;
                advance_session(&mut next, acceptance_event);
                materials.extend([acceptance.clone(), acceptance_event.clone()]);
            }
            Action::SelectAcceptedProposal {
                actor,
                selection_event,
                ..
            } => {
                require_phase(state, &[Phase::OfferAccepted])?;
                if *actor != Role::BuyerAgent {
                    return Err(role_violation("only buyer_agent may select"));
                }
                validate_object(selection_event, Role::BuyerAgent, "event_append")?;
                next.phase = Phase::AgreementPending;
                "agreement_pending".clone_into(&mut next.aggregate_state);
                advance_transaction(&mut next, selection_event);
                attention.extend([
                    Attention {
                        target: Role::BuyerAgent,
                        reason: AttentionReason::AgreementSignatureRequired,
                    },
                    Attention {
                        target: Role::SellerAgent,
                        reason: AttentionReason::AgreementSignatureRequired,
                    },
                ]);
                materials.push(selection_event.clone());
            }
            Action::RecordAgreementSignature {
                actor, signature, ..
            } => {
                require_phase(state, &[Phase::AgreementPending])?;
                validate_agreement_signature(state, *actor, signature)?;
                next.agreement_id = Some(signature.agreement_id.clone());
                next.agreement_content_hash = Some(signature.agreement_content_hash.clone());
                next.agreement_canonical_json = Some(signature.agreement_canonical_json.clone());
                next.agreement_signatures.insert(*actor, signature.clone());
                if next.agreement_signatures.len() == 2 {
                    attention.push(Attention {
                        target: Role::BuyerAgent,
                        reason: AttentionReason::AgreementReady,
                    });
                } else {
                    attention.push(Attention {
                        target: counterparty(*actor),
                        reason: AttentionReason::AgreementSignatureRequired,
                    });
                }
            }
            Action::CommitAgreement {
                actor,
                agreement,
                commitment_event,
                ..
            } => {
                require_phase(state, &[Phase::AgreementPending])?;
                if *actor != Role::BuyerAgent {
                    return Err(role_violation("only buyer_agent may commit"));
                }
                if state.agreement_signatures.len() != 2 {
                    return Err(OracleError::new(
                        RejectionCode::InvalidPhase,
                        "both independent agreement signatures are required",
                    ));
                }
                validate_object(agreement, Role::BuyerAgent, "agreement_commitment")?;
                validate_object(commitment_event, Role::BuyerAgent, "event_append")?;
                validate_commitment(state, agreement)?;
                next.phase = Phase::Complete;
                "committed".clone_into(&mut next.aggregate_state);
                next.outcome = Some(Outcome::AgreementCommitted {
                    transaction_id: state.transaction_id.clone(),
                    agreement_id: agreement.object_id.clone(),
                    agreement_hash: agreement.declared_content_hash.clone(),
                });
                advance_transaction(&mut next, commitment_event);
                materials.extend([agreement.clone(), commitment_event.clone()]);
            }
            Action::RecordSessionDeadlineElapsed {
                actor,
                admitted_at,
                occurred_at,
                deadline_event,
                ..
            } => {
                require_deadline_action(
                    state,
                    *actor,
                    *admitted_at,
                    *occurred_at,
                    DeadlineProgress::AwaitingSessionExpiry,
                )?;
                if state.session_state != "active" {
                    return Err(OracleError::new(
                        RejectionCode::InvalidPhase,
                        "session deadline event requires active session",
                    ));
                }
                validate_object(deadline_event, Role::VenueSigner, "event_append")?;
                "expired".clone_into(&mut next.session_state);
                next.deadline_progress = DeadlineProgress::AwaitingTransactionExpiry;
                advance_session(&mut next, deadline_event);
                materials.push(deadline_event.clone());
            }
            Action::RecordTransactionDeadlineElapsed {
                actor,
                admitted_at,
                occurred_at,
                deadline_event,
                ..
            } => {
                require_deadline_action(
                    state,
                    *actor,
                    *admitted_at,
                    *occurred_at,
                    DeadlineProgress::AwaitingTransactionExpiry,
                )?;
                if state.session_state == "active" {
                    return Err(OracleError::new(
                        RejectionCode::InvalidPhase,
                        "active session must expire on its own stream first",
                    ));
                }
                validate_object(deadline_event, Role::VenueSigner, "event_append")?;
                "expired".clone_into(&mut next.aggregate_state);
                advance_transaction(&mut next, deadline_event);
                materials.push(deadline_event.clone());
                if matches!(state.session_state.as_str(), "opened" | "accepted") {
                    next.deadline_progress = DeadlineProgress::AwaitingSessionClose;
                } else {
                    finish_expired(&mut next);
                }
            }
            Action::CloseTransactionExpiredSession {
                actor, close_event, ..
            } => {
                require_phase(state, &[Phase::DeadlineResolution])?;
                if *actor != Role::VenueSigner {
                    return Err(role_violation(
                        "only venue_signer may close an expired transaction session",
                    ));
                }
                if state.deadline_progress != DeadlineProgress::AwaitingSessionClose
                    || state.aggregate_state != "expired"
                    || !matches!(state.session_state.as_str(), "opened" | "accepted")
                {
                    return Err(OracleError::new(
                        RejectionCode::InvalidPhase,
                        "transaction-expired session is not awaiting close",
                    ));
                }
                validate_object(close_event, Role::VenueSigner, "event_append")?;
                "closed".clone_into(&mut next.session_state);
                advance_session(&mut next, close_event);
                materials.push(close_event.clone());
                finish_expired(&mut next);
            }
        }

        advance_room(&mut next, action.action_id(), action)?;
        record_evidence(&mut next, action.action_id(), materials);
        Ok(Transition {
            action_id: action.action_id().to_owned(),
            action_kind: Some(action.kind()),
            previous_room_head,
            state: next,
            attention,
        })
    }

    fn apply_timer(
        &self,
        state: &OracleState,
        timer: &TimerFired,
    ) -> Result<Transition, OracleError> {
        if timer.generation == 0 || timer.fired_at < timer.scheduled_for {
            return Err(OracleError::new(
                RejectionCode::InvalidPhase,
                "timer generation and scheduled time must be authoritative",
            ));
        }
        let previous_room_head = state.room_head.clone();
        let mut next = state.clone();
        let mut attention = Vec::new();
        match timer.timer {
            TimerKind::ProposalValidity => {
                let proposal = state.current_proposal.as_ref().ok_or_else(|| {
                    OracleError::new(
                        RejectionCode::InvalidPhase,
                        "proposal timer has no proposal",
                    )
                })?;
                if timer.expected_object_id.as_deref() != Some(proposal.offer.object_id.as_str())
                    || timer.scheduled_for != proposal.valid_until
                {
                    return Err(OracleError::new(
                        RejectionCode::StaleSessionHead,
                        "proposal timer does not name the current revision",
                    ));
                }
                next.proposal_expired = true;
                next.pending_approval = None;
                next.recorded_approval = None;
                next.phase = Phase::FormationOpen;
            }
            TimerKind::ApprovalExpiry => {
                let binding = state.pending_approval.as_ref().ok_or_else(|| {
                    OracleError::new(RejectionCode::InvalidPhase, "approval timer has no binding")
                })?;
                if timer.expected_object_id.as_deref()
                    != Some(binding.candidate_wire_digest.as_str())
                    || timer.scheduled_for != binding.expires_at
                {
                    return Err(OracleError::new(
                        RejectionCode::ApprovalBindingMismatch,
                        "approval timer does not name the pending exact candidate",
                    ));
                }
                next.pending_approval = None;
                next.recorded_approval = None;
                next.phase = Phase::FormationOpen;
            }
            TimerKind::FormationDeadline => {
                if timer.scheduled_for != state.formation_deadline {
                    return Err(OracleError::new(
                        RejectionCode::InvalidPhase,
                        "formation timer schedule is not the immutable deadline",
                    ));
                }
                if !matches!(state.phase, Phase::Complete | Phase::Expired) {
                    next.phase = Phase::DeadlineResolution;
                    next.pending_approval = None;
                    next.recorded_approval = None;
                    next.deadline_progress = if state.session_state == "active" {
                        DeadlineProgress::AwaitingSessionExpiry
                    } else {
                        DeadlineProgress::AwaitingTransactionExpiry
                    };
                    attention.push(Attention {
                        target: Role::VenueSigner,
                        reason: AttentionReason::VenueSignatureRequired,
                    });
                }
            }
        }
        let action_id = format!("timer:{:?}:{}", timer.timer, timer.generation).to_lowercase();
        advance_room(&mut next, &action_id, timer)?;
        Ok(Transition {
            action_id,
            action_kind: None,
            previous_room_head,
            state: next,
            attention,
        })
    }
}

fn validate_submit(
    state: &OracleState,
    actor: Role,
    admitted_at: u64,
    proposal: &Proposal,
) -> Result<(), OracleError> {
    require_phase(state, &[Phase::FormationOpen, Phase::ApprovalPending])?;
    if !matches!(actor, Role::BuyerAgent | Role::SellerAgent) || proposal.author != actor {
        return Err(role_violation(
            "proposal author must be the acting commercial Role",
        ));
    }
    if proposal.valid_until <= admitted_at || proposal.valid_until > state.formation_deadline {
        return Err(OracleError::new(
            RejectionCode::ProposalExpired,
            "proposal validity must be future and no later than formation deadline",
        ));
    }
    match &state.current_proposal {
        None if proposal.supersedes_offer_id.is_none() => Ok(()),
        None => Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "first proposal cannot supersede another proposal",
        )),
        Some(current) if actor == current.author => Err(role_violation(
            "only current offeree may submit the next counter",
        )),
        Some(current)
            if proposal.supersedes_offer_id.as_deref()
                == Some(current.offer.object_id.as_str()) =>
        {
            Ok(())
        }
        Some(_) => Err(OracleError::new(
            RejectionCode::StaleSessionHead,
            "counter must supersede the immediate current proposal",
        )),
    }
}

fn validate_binding(
    state: &OracleState,
    binding: &ApprovalBinding,
    admitted_at: u64,
) -> Result<(), OracleError> {
    let proposal = state.current_proposal.as_ref().ok_or_else(|| {
        OracleError::new(RejectionCode::ApprovalBindingMismatch, "proposal missing")
    })?;
    if proposal.author != Role::SellerAgent || state.proposal_expired {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "approval requires a live seller-authored proposal",
        ));
    }
    let candidate_digest = tagged_blake3(binding.candidate_canonical_json.as_bytes());
    let parsed: serde_json::Value = serde_json::from_str(&binding.candidate_canonical_json)
        .map_err(|_| OracleError::new(RejectionCode::ByteMutation, "candidate is not JSON"))?;
    if canonical_bytes(&parsed)? != binding.candidate_canonical_json.as_bytes()
        || candidate_digest != binding.candidate_wire_digest
        || binding.transaction_id != state.transaction_id
        || binding.proposal_id != proposal.offer.object_id
        || binding.proposal_content_hash != proposal.offer.declared_content_hash
        || binding.room_head_at_request != state.room_head
        || binding.transaction_head_at_request != state.transaction_head
        || binding.session_head_at_request != state.session_head
        || binding.approver_id != APPROVER_ID
    {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "approval does not bind exact bytes, hashes, heads, transaction, and approver",
        ));
    }
    if binding.expires_at <= admitted_at
        || binding.expires_at > proposal.valid_until
        || binding.expires_at > state.formation_deadline
    {
        return Err(OracleError::new(
            RejectionCode::ApprovalExpired,
            "approval expiry is outside the live proposal window",
        ));
    }
    Ok(())
}

fn validate_approval(
    pending: &ApprovalBinding,
    approval: &ExactApproval,
    admitted_at: u64,
) -> Result<(), OracleError> {
    if approval.binding != *pending {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "signed approval binding differs from pending exact candidate",
        ));
    }
    if admitted_at >= pending.expires_at {
        return Err(OracleError::new(
            RejectionCode::ApprovalExpired,
            "approval recorded at or after expiry",
        ));
    }
    validate_object(&approval.approval, Role::BuyerApprover, "object_issuance")
}

fn validate_acceptance(
    state: &OracleState,
    admitted_at: u64,
    candidate_canonical_json: &str,
    approval: &ExactApproval,
) -> Result<(), OracleError> {
    let pending = state.pending_approval.as_ref().ok_or_else(|| {
        OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "pending approval missing",
        )
    })?;
    let recorded = state.recorded_approval.as_ref().ok_or_else(|| {
        OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "recorded approval missing",
        )
    })?;
    if recorded != approval || approval.binding != *pending {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "submitted approval is not the recorded exact approval",
        ));
    }
    if approval.decision != ApprovalDecision::Approved {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "approval decision is not approved",
        ));
    }
    if candidate_canonical_json != pending.candidate_canonical_json
        || tagged_blake3(candidate_canonical_json.as_bytes()) != pending.candidate_wire_digest
    {
        return Err(OracleError::new(
            RejectionCode::ByteMutation,
            "acceptance candidate is not byte-identical to approved bytes",
        ));
    }
    if state.transaction_head != pending.transaction_head_at_request {
        return Err(OracleError::new(
            RejectionCode::StaleTransactionHead,
            "transaction stream advanced after approval request",
        ));
    }
    if state.session_head != pending.session_head_at_request {
        return Err(OracleError::new(
            RejectionCode::StaleSessionHead,
            "session stream advanced after approval request",
        ));
    }
    let proposal = state.current_proposal.as_ref().ok_or_else(|| {
        OracleError::new(RejectionCode::ApprovalBindingMismatch, "proposal missing")
    })?;
    if pending.proposal_id != proposal.offer.object_id
        || pending.proposal_content_hash != proposal.offer.declared_content_hash
    {
        return Err(OracleError::new(
            RejectionCode::ApprovalBindingMismatch,
            "current proposal differs from approved proposal",
        ));
    }
    if admitted_at >= proposal.valid_until {
        return Err(OracleError::new(
            RejectionCode::ProposalExpired,
            "proposal expired before acceptance",
        ));
    }
    if admitted_at >= pending.expires_at {
        return Err(OracleError::new(
            RejectionCode::ApprovalExpired,
            "approval expired before acceptance",
        ));
    }
    Ok(())
}

fn validate_agreement_signature(
    state: &OracleState,
    actor: Role,
    signature: &AgreementSignature,
) -> Result<(), OracleError> {
    if !matches!(actor, Role::BuyerAgent | Role::SellerAgent)
        || signature.signer_role != actor
        || signature.signer_id != signer_id(actor)
    {
        return Err(OracleError::new(
            RejectionCode::WrongSigner,
            "agreement signature signer does not match acting commercial Role",
        ));
    }
    if state.agreement_signatures.contains_key(&actor) {
        return Err(OracleError::new(
            RejectionCode::InvalidPhase,
            "agreement signer already recorded",
        ));
    }
    if tagged_blake3(signature.agreement_canonical_json.as_bytes())
        != signature.agreement_wire_digest
        || signature.proof
            != agreement_proof(
                actor,
                &signature.signer_id,
                &signature.agreement_wire_digest,
            )
    {
        return Err(OracleError::new(
            RejectionCode::InvalidSignature,
            "agreement signature does not bind exact agreement bytes",
        ));
    }
    if let (Some(id), Some(hash), Some(bytes)) = (
        &state.agreement_id,
        &state.agreement_content_hash,
        &state.agreement_canonical_json,
    ) && (id != &signature.agreement_id
        || hash != &signature.agreement_content_hash
        || bytes != &signature.agreement_canonical_json)
    {
        return Err(OracleError::new(
            RejectionCode::ByteMutation,
            "commercial parties did not sign identical agreement bytes",
        ));
    }
    Ok(())
}

fn validate_commitment(
    state: &OracleState,
    agreement: &ExactA202Object,
) -> Result<(), OracleError> {
    if state.agreement_id.as_deref() != Some(agreement.object_id.as_str())
        || state.agreement_content_hash.as_deref() != Some(agreement.declared_content_hash.as_str())
        || state.agreement_canonical_json.as_deref() != Some(agreement.canonical_json.as_str())
        || state
            .agreement_signatures
            .values()
            .any(|signature| signature.agreement_wire_digest != agreement.wire_digest)
    {
        return Err(OracleError::new(
            RejectionCode::ByteMutation,
            "committed agreement differs from independently signed bytes",
        ));
    }
    Ok(())
}

fn validate_basis(state: &OracleState, basis: &ActionBasis) -> Result<(), OracleError> {
    if basis.room != state.room_head {
        return Err(OracleError::new(
            RejectionCode::StaleRoomHead,
            "Room Head is stale",
        ));
    }
    if basis.transaction != state.transaction_head {
        return Err(OracleError::new(
            RejectionCode::StaleTransactionHead,
            "A202 transaction head is stale",
        ));
    }
    if basis.session != state.session_head {
        return Err(OracleError::new(
            RejectionCode::StaleSessionHead,
            "A202 session head is stale",
        ));
    }
    Ok(())
}

fn validate_object(
    object: &ExactA202Object,
    expected_signer: Role,
    expected_purpose: &str,
) -> Result<(), OracleError> {
    if object.canonical_json.len() > MAX_A202_OBJECT_BYTES {
        return Err(OracleError::new(
            RejectionCode::ResourceLimit,
            "A202 object exceeds oracle resource limit",
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(&object.canonical_json).map_err(|_| {
        OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            "A202 bytes are not valid UTF-8 JSON",
        )
    })?;
    if canonical_bytes(&parsed)? != object.canonical_json.as_bytes() {
        return Err(OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            "A202 bytes are not the exact canonical form",
        ));
    }
    if tagged_blake3(object.canonical_json.as_bytes()) != object.wire_digest {
        return Err(OracleError::new(
            RejectionCode::ByteMutation,
            "A202 exact-byte digest mismatch",
        ));
    }
    if object.signature.signed_wire_digest != object.wire_digest {
        return Err(OracleError::new(
            RejectionCode::ByteMutation,
            "signature does not bind this exact A202 byte digest",
        ));
    }
    validate_fixture_signature(&object.signature, expected_signer, expected_purpose)
}

fn validate_fixture_signature(
    signature: &FixtureSignature,
    expected_signer: Role,
    expected_purpose: &str,
) -> Result<(), OracleError> {
    if signature.signer_role != expected_signer || signature.signer_id != signer_id(expected_signer)
    {
        return Err(OracleError::new(
            RejectionCode::WrongSigner,
            "fixture signature signer does not match required protocol authority",
        ));
    }
    if signature.purpose != expected_purpose
        || signature.proof
            != fixture_proof(
                expected_signer,
                &signature.signer_id,
                expected_purpose,
                &signature.signed_wire_digest,
            )
    {
        return Err(OracleError::new(
            RejectionCode::InvalidSignature,
            "fixture signature purpose or proof is invalid",
        ));
    }
    Ok(())
}

fn require_phase(state: &OracleState, phases: &[Phase]) -> Result<(), OracleError> {
    if phases.contains(&state.phase) {
        Ok(())
    } else {
        Err(OracleError::new(
            RejectionCode::InvalidPhase,
            format!("action is not legal in phase {:?}", state.phase),
        ))
    }
}

fn require_deadline_action(
    state: &OracleState,
    actor: Role,
    admitted_at: u64,
    occurred_at: u64,
    progress: DeadlineProgress,
) -> Result<(), OracleError> {
    require_phase(state, &[Phase::DeadlineResolution])?;
    if actor != Role::VenueSigner {
        return Err(role_violation(
            "only venue_signer may append deadline events",
        ));
    }
    if state.deadline_progress != progress {
        return Err(OracleError::new(
            RejectionCode::InvalidPhase,
            "deadline event is not the next independently signed stream act",
        ));
    }
    if admitted_at <= state.formation_deadline
        || occurred_at <= state.formation_deadline
        || occurred_at > admitted_at
    {
        return Err(OracleError::new(
            RejectionCode::DeadlineNotPassed,
            "signed and admitted deadline time must be strictly after formation deadline",
        ));
    }
    Ok(())
}

fn advance_room<T: Serialize>(
    state: &mut OracleState,
    action_id: &str,
    material: &T,
) -> Result<(), OracleError> {
    #[derive(Serialize)]
    struct Advance<'a, T> {
        domain: &'static str,
        previous: &'a RoomHead,
        action_id: &'a str,
        material: &'a T,
    }
    let bytes = canonical_bytes(&Advance {
        domain: "worldstream/negotiate-room-head/v1",
        previous: &state.room_head,
        action_id,
        material,
    })?;
    state.room_head = RoomHead {
        sequence: state.room_head.sequence.saturating_add(1),
        digest: tagged_blake3(&bytes),
    };
    Ok(())
}

fn advance_session(state: &mut OracleState, event: &ExactA202Object) {
    state.session_head = LogicalHead {
        sequence: state.session_head.sequence.saturating_add(1),
        event_hash: format!("sha256:{}", event.declared_content_hash),
    };
}

fn advance_transaction(state: &mut OracleState, event: &ExactA202Object) {
    state.transaction_head = LogicalHead {
        sequence: state.transaction_head.sequence.saturating_add(1),
        event_hash: format!("sha256:{}", event.declared_content_hash),
    };
}

fn record_evidence(state: &mut OracleState, action_id: &str, objects: Vec<ExactA202Object>) {
    for object in objects {
        state.evidence.push(EvidenceLink {
            object_id: object.object_id,
            object_type: object.object_type,
            content_hash: object.declared_content_hash,
            wire_digest: object.wire_digest,
            action_id: action_id.to_owned(),
            room_sequence: state.room_head.sequence,
            room_digest: state.room_head.digest.clone(),
            transaction_head: state.transaction_head.clone(),
            session_head: state.session_head.clone(),
        });
    }
}

fn finish_expired(state: &mut OracleState) {
    state.phase = Phase::Expired;
    state.deadline_progress = DeadlineProgress::Resolved;
    state.outcome = Some(Outcome::FormationExpired {
        transaction_id: state.transaction_id.clone(),
        final_session_state: state.session_state.clone(),
    });
}

fn is_counterparty(left: Role, right: Role) -> bool {
    matches!(
        (left, right),
        (Role::BuyerAgent, Role::SellerAgent) | (Role::SellerAgent, Role::BuyerAgent)
    )
}

fn counterparty(role: Role) -> Role {
    match role {
        Role::BuyerAgent => Role::SellerAgent,
        Role::SellerAgent => Role::BuyerAgent,
        Role::BuyerApprover | Role::VenueSigner => role,
    }
}

fn signer_id(role: Role) -> &'static str {
    match role {
        Role::BuyerAgent => BUYER_ID,
        Role::SellerAgent => SELLER_ID,
        Role::BuyerApprover => APPROVER_ID,
        Role::VenueSigner => VENUE_ID,
    }
}

fn is_deadline_action(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::RecordSessionDeadlineElapsed
            | ActionKind::RecordTransactionDeadlineElapsed
            | ActionKind::CloseTransactionExpiredSession
    )
}

fn role_violation(detail: &str) -> OracleError {
    OracleError::new(RejectionCode::RoleViolation, detail)
}

pub(crate) fn fixture_signature(
    signer_role: Role,
    purpose: &str,
    wire_digest: &str,
) -> FixtureSignature {
    let signer_id = signer_id(signer_role).to_owned();
    FixtureSignature {
        signer_id: signer_id.clone(),
        signer_role,
        purpose: purpose.to_owned(),
        signed_wire_digest: wire_digest.to_owned(),
        proof: fixture_proof(signer_role, &signer_id, purpose, wire_digest),
    }
}

fn fixture_proof(role: Role, signer_id: &str, purpose: &str, wire_digest: &str) -> String {
    tagged_blake3(
        format!(
            "worldstream/negotiate-fixture-proof/v1\0{}\0{signer_id}\0{purpose}\0{wire_digest}",
            role.as_str()
        )
        .as_bytes(),
    )
}

pub(crate) fn agreement_signature(
    signer_role: Role,
    agreement_id: &str,
    agreement_hash: &str,
    canonical_json: &str,
) -> AgreementSignature {
    let signer_id = signer_id(signer_role).to_owned();
    let wire_digest = tagged_blake3(canonical_json.as_bytes());
    AgreementSignature {
        agreement_id: agreement_id.to_owned(),
        agreement_content_hash: agreement_hash.to_owned(),
        agreement_canonical_json: canonical_json.to_owned(),
        agreement_wire_digest: wire_digest.clone(),
        signer_id: signer_id.clone(),
        signer_role,
        proof: agreement_proof(signer_role, &signer_id, &wire_digest),
    }
}

fn agreement_proof(role: Role, signer_id: &str, wire_digest: &str) -> String {
    tagged_blake3(
        format!(
            "worldstream/negotiate-agreement-proof/v1\0{}\0{signer_id}\0{wire_digest}",
            role.as_str()
        )
        .as_bytes(),
    )
}

pub fn validate_boundary_probe(probe: &BoundaryProbe) -> Result<(), OracleError> {
    if !probe.imports.is_empty() {
        return Err(OracleError::new(
            RejectionCode::ForbiddenImport,
            "portable Activity Pack Component must have zero imports",
        ));
    }
    if probe.component_bytes > MAX_COMPONENT_BYTES {
        return Err(OracleError::new(
            RejectionCode::ResourceLimit,
            "Component exceeds deterministic fixture resource limit",
        ));
    }
    if probe.callback_fault_before_commit {
        return Err(OracleError::new(
            RejectionCode::CallbackFaultBeforeCommit,
            "callback fault must leave authoritative state uncommitted",
        ));
    }
    match probe.retained_bundle {
        BundleCondition::Available => Ok(()),
        BundleCondition::Missing => Err(OracleError::new(
            RejectionCode::MissingRetainedBundle,
            "retained bundle is missing",
        )),
        BundleCondition::Corrupt => Err(OracleError::new(
            RejectionCode::CorruptRetainedBundle,
            "retained bundle is corrupt",
        )),
    }
}
