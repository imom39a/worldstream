import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

import type { NegotiateState, Persona, Role } from "./model.js";
import { asCanonical, isRole, record, stringValue } from "./model.js";
import { offers } from "./rules.js";

export interface AuthorizedView {
  readonly actionOffers: string[];
  readonly persona: Persona;
  readonly projection: CanonicalObject;
  readonly schema: "participant" | "public";
}

export function authorizedView(
  state: NegotiateState,
  core: Record<string, CanonicalJson>,
  viewer: Record<string, CanonicalJson>,
): AuthorizedView {
  const persona = personaForViewer(core, viewer);
  if (isParticipantPersona(persona)) {
    return {
      actionOffers:
        viewer.viewer_type === "participant" ? offers(state, persona) : [],
      persona,
      projection: participantProjection(state, persona),
      schema: "participant",
    };
  }
  return {
    actionOffers: [],
    persona,
    projection:
      persona === "operator"
        ? operatorProjection(state)
        : spectatorProjection(state),
    schema: "public",
  };
}

export function personaForViewer(
  core: Record<string, CanonicalJson>,
  viewer: Record<string, CanonicalJson>,
): Persona {
  const viewerType = stringValue(viewer.viewer_type, "viewer_type");
  if (viewerType === "public") return "spectator";
  if (viewerType === "operator") return "operator";
  if (viewerType === "final_reveal") return "spectator";
  const memberId = stringValue(viewer.member_id, "viewer member_id");
  const memberships = record(core.memberships, "core memberships");
  const membership = record(memberships[memberId], "viewer membership");
  const access = stringValue(membership.access_mode, "viewer access mode");
  if (viewerType === "historical" && access === "operator") return "operator";
  if (viewerType === "historical" && access === "spectator") return "spectator";
  const role = stringValue(membership.role, "viewer Role");
  return isRole(role) ? role : "spectator";
}

export function participantProjection(
  state: NegotiateState,
  role: Role,
): CanonicalObject {
  const shared = sharedProtocolProjection(state);
  if (role === "buyer_agent") {
    return asCanonical({
      ...shared,
      persona: role,
      agreement: agreementForCommercialParty(state),
      buyer_acceptance_binding: state.pending_approval,
      recorded_approval: state.recorded_approval,
    });
  }
  if (role === "seller_agent") {
    return asCanonical({
      ...shared,
      persona: role,
      agreement: agreementForCommercialParty(state),
      recorded_approval: approvalWithoutAcceptanceCandidate(state),
    });
  }
  if (role === "buyer_approver") {
    return asCanonical({
      ...statusProjection(state),
      persona: role,
      current_proposal: state.current_proposal,
      pending_approval: state.pending_approval,
      recorded_approval: state.recorded_approval,
      agreement: agreementForApprover(state),
    });
  }
  return asCanonical({
    ...statusProjection(state),
    persona: role,
    current_proposal_reference: proposalReference(state),
    pending_approval_reference: approvalReference(state),
    transaction_head: state.transaction_head,
    session_head: state.session_head,
    deadline_progress: state.deadline_progress,
    event_candidates_required: venueEventCandidateKinds(state),
    committed_agreement_reference: committedAgreementReference(state),
  });
}

export function operatorProjection(state: NegotiateState): CanonicalObject {
  return asCanonical({
    ...statusProjection(state),
    persona: "operator",
    room_sequence: state.room_head.sequence,
    transaction_sequence: state.transaction_head.sequence,
    session_sequence: state.session_head.sequence,
    current_proposal_reference: proposalReference(state),
    pending_approval: state.pending_approval === null
      ? null
      : {
          approver_id: state.pending_approval.approver_id,
          expires_at: state.pending_approval.expires_at,
          proposal_id: state.pending_approval.proposal_id,
        },
    signature_count: Object.keys(state.agreement_signatures).length,
    evidence_count: state.evidence.length,
    committed_agreement:
      state.phase === "complete" ? agreementForCommercialParty(state) : null,
  });
}

export function spectatorProjection(state: NegotiateState): CanonicalObject {
  return asCanonical({ ...statusProjection(state), persona: "spectator" });
}

function sharedProtocolProjection(state: NegotiateState): Record<string, CanonicalJson> {
  return {
    ...statusProjection(state),
    current_proposal: state.current_proposal as unknown as CanonicalJson,
    proposal_expired: state.proposal_expired,
    formation_deadline: state.formation_deadline,
    transaction_head: state.transaction_head as unknown as CanonicalJson,
    session_head: state.session_head as unknown as CanonicalJson,
    evidence: state.evidence as unknown as CanonicalJson,
  };
}

function statusProjection(state: NegotiateState): Record<string, CanonicalJson> {
  return {
    aggregate_state: state.aggregate_state,
    outcome: state.outcome as unknown as CanonicalJson,
    phase: state.phase,
    session_state: state.session_state,
  };
}

function agreementForCommercialParty(state: NegotiateState): CanonicalJson {
  if (state.agreement_id === null) return null;
  return {
    agreement_id: state.agreement_id,
    agreement_content_hash: state.agreement_content_hash,
    agreement_canonical_json: state.agreement_canonical_json,
    signatures: state.agreement_signatures as unknown as CanonicalJson,
  };
}

function agreementForApprover(state: NegotiateState): CanonicalJson {
  return agreementForCommercialParty(state);
}

function approvalWithoutAcceptanceCandidate(state: NegotiateState): CanonicalJson {
  const approval = state.recorded_approval;
  if (approval === null) return null;
  return {
    approval: approval.approval as unknown as CanonicalJson,
    decision: approval.decision,
    binding_reference: {
      approver_id: approval.binding.approver_id,
      expires_at: approval.binding.expires_at,
      proposal_content_hash: approval.binding.proposal_content_hash,
      proposal_id: approval.binding.proposal_id,
    },
  };
}

function proposalReference(state: NegotiateState): CanonicalJson {
  const proposal = state.current_proposal;
  if (proposal === null) return null;
  return {
    author: proposal.author,
    content_hash: proposal.offer.declared_content_hash,
    object_id: proposal.offer.object_id,
    valid_until: proposal.valid_until,
    wire_digest: proposal.offer.wire_digest,
  };
}

function approvalReference(state: NegotiateState): CanonicalJson {
  if (state.pending_approval === null) return null;
  return {
    approver_id: state.pending_approval.approver_id,
    candidate_wire_digest: state.pending_approval.candidate_wire_digest,
    expires_at: state.pending_approval.expires_at,
    proposal_id: state.pending_approval.proposal_id,
  };
}

function committedAgreementReference(state: NegotiateState): CanonicalJson {
  if (state.phase !== "complete" || state.agreement_id === null) return null;
  return {
    agreement_content_hash: state.agreement_content_hash,
    agreement_id: state.agreement_id,
  };
}

function venueEventCandidateKinds(state: NegotiateState): CanonicalJson {
  if (state.deadline_progress === "awaiting_session_expiry") {
    return ["record_session_deadline_elapsed"];
  }
  if (state.deadline_progress === "awaiting_transaction_expiry") {
    return ["record_transaction_deadline_elapsed"];
  }
  if (state.deadline_progress === "awaiting_session_close") {
    return ["close_transaction_expired_session"];
  }
  return [];
}

function isParticipantPersona(persona: Persona): persona is Role {
  return isRole(persona);
}
