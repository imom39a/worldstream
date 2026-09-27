import {
  canonicalStringify,
  taggedBlake3Text,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import {
  signerId,
  validateAgreementProof,
  validateCanonicalText,
  validateExactObject,
} from "./crypto.js";
import type {
  ActionBasis,
  ActionKind,
  AgreementSignature,
  ApprovalBinding,
  Attention,
  ExactA202Object,
  ExactApproval,
  LogicalHead,
  NegotiateState,
  Phase,
  Proposal,
  Role,
  RoomHead,
  RuleTransition,
} from "./model.js";
import {
  asCanonical,
  cloneState,
  integerValue,
  optionalString,
  record,
  reject,
  roleValue,
  sameCanonical,
  stringValue,
} from "./model.js";

export function initializeState(configuration: CanonicalObject): NegotiateState {
  const formationDeadline = integerValue(
    configuration.formation_deadline,
    "formation_deadline",
  );
  const revision = configuration.a202_revision === undefined
    ? pinnedRevision()
    : stringValue(configuration.a202_revision, "a202_revision");
  if (revision !== pinnedRevision()) {
    reject("invalid_phase", "configuration does not use the pinned A202 revision");
  }
  return {
    oracle_id: "worldstream/negotiate-oracle/v1",
    a202_revision: revision,
    transaction_id:
      configuration.transaction_id === undefined
        ? "txn_calibration_worldstream_01"
        : stringValue(configuration.transaction_id, "transaction_id"),
    session_id:
      configuration.session_id === undefined
        ? "ses_northstar_delta_worldstream_01"
        : stringValue(configuration.session_id, "session_id"),
    phase: "formation_open",
    aggregate_state: "negotiating",
    session_state: "opened",
    room_head: {
      sequence: 0,
      digest: taggedBlake3Text("worldstream/negotiate/genesis/v1"),
    },
    transaction_head: initialLogicalHead(
      configuration.transaction_head,
      3,
      `sha256:${"1".repeat(64)}`,
    ),
    session_head: initialLogicalHead(
      configuration.session_head,
      1,
      `sha256:${"2".repeat(64)}`,
    ),
    formation_deadline: formationDeadline,
    proposal_expired: false,
    current_proposal: null,
    pending_approval: null,
    recorded_approval: null,
    accepted_offer_id: null,
    accepted_offer_hash: null,
    agreement_id: null,
    agreement_content_hash: null,
    agreement_canonical_json: null,
    agreement_signatures: {},
    deadline_progress: "none",
    outcome: null,
    evidence: [],
  };
}

export function applyParticipantAction(
  state: NegotiateState,
  action: Record<string, CanonicalJson>,
): RuleTransition {
  const kind = actionKind(action.action);
  const actionId = stringValue(action.action_id, "action_id");
  const actor = roleValue(action.actor, "actor");
  const admittedAt = integerValue(action.admitted_at, "admitted_at");
  validateBasis(state, record(action.basis, "Action basis") as unknown as ActionBasis);
  if (!isDeadlineAction(kind) && admittedAt >= state.formation_deadline) {
    reject("deadline_reached", "ordinary Action is at or after the formation deadline");
  }

  const next = cloneState(state);
  const attention: Attention[] = [];
  const materials: ExactA202Object[] = [];
  let eventType: string = kind;

  switch (kind) {
    case "submit_proposal_revision": {
      const proposal = record(action.proposal, "proposal") as unknown as Proposal;
      validateSubmit(state, actor, admittedAt, proposal);
      validateExactObject(proposal.offer, actor, "offer_submission");
      validateExactObject(proposal.session_event, "venue_signer", "event_append");
      next.phase = "formation_open";
      next.session_state = "active";
      next.proposal_expired = false;
      next.pending_approval = null;
      next.recorded_approval = null;
      next.current_proposal = proposal;
      advanceSession(next, proposal.session_event);
      attention.push({ target: counterparty(actor), reason: "proposal_received" });
      materials.push(proposal.offer, proposal.session_event);
      eventType = "proposal_revised";
      break;
    }
    case "withdraw_live_proposal": {
      requirePhase(state, ["formation_open", "approval_pending"]);
      if (state.current_proposal === null) reject("invalid_phase", "there is no live proposal");
      if (state.current_proposal.author !== actor) {
        reject("role_violation", "only the current offeror may withdraw");
      }
      const withdrawal = record(
        action.withdrawal_event,
        "withdrawal_event",
      ) as unknown as ExactA202Object;
      validateExactObject(withdrawal, "venue_signer", "event_append");
      next.phase = "withdrawn_pending_expiry";
      next.session_state = "withdrawn";
      next.current_proposal = null;
      next.pending_approval = null;
      next.recorded_approval = null;
      advanceSession(next, withdrawal);
      materials.push(withdrawal);
      eventType = "proposal_withdrawn";
      break;
    }
    case "request_exact_approval": {
      requirePhase(state, ["formation_open"]);
      if (actor !== "buyer_agent") {
        reject("role_violation", "only buyer_agent may request exact approval");
      }
      const binding = record(action.binding, "approval binding") as unknown as ApprovalBinding;
      validateBinding(state, binding, admittedAt);
      next.phase = "approval_pending";
      next.pending_approval = binding;
      next.recorded_approval = null;
      eventType = "approval_requested";
      break;
    }
    case "record_exact_approval": {
      requirePhase(state, ["approval_pending"]);
      if (actor !== "buyer_approver") {
        reject("role_violation", "only buyer_approver may record exact approval");
      }
      if (state.pending_approval === null) {
        reject("approval_binding_mismatch", "no exact approval is pending");
      }
      const approval = record(action.approval, "exact approval") as unknown as ExactApproval;
      validateApproval(state.pending_approval, approval, admittedAt);
      next.recorded_approval = approval;
      materials.push(approval.approval);
      if (approval.decision === "rejected") {
        next.phase = "formation_open";
        next.pending_approval = null;
      }
      eventType = "approval_recorded";
      break;
    }
    case "accept_current_proposal": {
      requirePhase(state, ["approval_pending"]);
      if (actor !== "buyer_agent") reject("role_violation", "only buyer_agent may accept");
      const candidate = stringValue(
        action.candidate_canonical_json,
        "candidate_canonical_json",
      );
      const approval = record(action.approval, "exact approval") as unknown as ExactApproval;
      validateAcceptance(state, admittedAt, candidate, approval);
      const acceptance = record(action.acceptance, "acceptance") as unknown as ExactA202Object;
      const acceptanceEvent = record(
        action.acceptance_event,
        "acceptance_event",
      ) as unknown as ExactA202Object;
      validateExactObject(acceptance, "buyer_agent", "offer_acceptance");
      validateExactObject(acceptanceEvent, "venue_signer", "event_append");
      if (state.current_proposal === null) reject("invalid_phase", "current proposal is missing");
      next.phase = "offer_accepted";
      next.session_state = "accepted";
      next.accepted_offer_id = state.current_proposal.offer.object_id;
      next.accepted_offer_hash = state.current_proposal.offer.declared_content_hash;
      next.pending_approval = null;
      advanceSession(next, acceptanceEvent);
      materials.push(acceptance, acceptanceEvent);
      eventType = "proposal_accepted";
      break;
    }
    case "select_accepted_proposal": {
      requirePhase(state, ["offer_accepted"]);
      if (actor !== "buyer_agent") reject("role_violation", "only buyer_agent may select");
      const selection = record(
        action.selection_event,
        "selection_event",
      ) as unknown as ExactA202Object;
      validateExactObject(selection, "buyer_agent", "event_append");
      next.phase = "agreement_pending";
      next.aggregate_state = "agreement_pending";
      advanceTransaction(next, selection);
      attention.push(
        { target: "buyer_agent", reason: "agreement_signature_required" },
        { target: "seller_agent", reason: "agreement_signature_required" },
      );
      materials.push(selection);
      eventType = "proposal_selected";
      break;
    }
    case "record_agreement_signature": {
      requirePhase(state, ["agreement_pending"]);
      const signature = record(
        action.signature,
        "agreement signature",
      ) as unknown as AgreementSignature;
      validateAgreementSignature(state, actor, signature);
      next.agreement_id = signature.agreement_id;
      next.agreement_content_hash = signature.agreement_content_hash;
      next.agreement_canonical_json = signature.agreement_canonical_json;
      next.agreement_signatures[actor] = signature;
      if (Object.keys(next.agreement_signatures).length === 2) {
        attention.push({ target: "buyer_agent", reason: "agreement_ready" });
      } else {
        attention.push({ target: counterparty(actor), reason: "agreement_signature_required" });
      }
      eventType = "agreement_signature_recorded";
      break;
    }
    case "commit_agreement": {
      requirePhase(state, ["agreement_pending"]);
      if (actor !== "buyer_agent") reject("role_violation", "only buyer_agent may commit");
      if (Object.keys(state.agreement_signatures).length !== 2) {
        reject("invalid_phase", "both independent Agreement signatures are required");
      }
      const agreement = record(action.agreement, "agreement") as unknown as ExactA202Object;
      const commitment = record(
        action.commitment_event,
        "commitment_event",
      ) as unknown as ExactA202Object;
      validateExactObject(agreement, "buyer_agent", "agreement_commitment");
      validateExactObject(commitment, "buyer_agent", "event_append");
      validateCommitment(state, agreement);
      next.phase = "complete";
      next.aggregate_state = "committed";
      next.outcome = {
        kind: "agreement_committed",
        transaction_id: state.transaction_id,
        agreement_id: agreement.object_id,
        agreement_hash: agreement.declared_content_hash,
      };
      advanceTransaction(next, commitment);
      materials.push(agreement, commitment);
      eventType = "agreement_committed";
      break;
    }
    case "record_session_deadline_elapsed": {
      const occurredAt = integerValue(action.occurred_at, "occurred_at");
      requireDeadlineAction(
        state,
        actor,
        admittedAt,
        occurredAt,
        "awaiting_session_expiry",
      );
      if (state.session_state !== "active") {
        reject("invalid_phase", "session deadline requires an active session");
      }
      const deadlineEvent = record(
        action.deadline_event,
        "deadline_event",
      ) as unknown as ExactA202Object;
      validateExactObject(deadlineEvent, "venue_signer", "event_append");
      next.session_state = "expired";
      next.deadline_progress = "awaiting_transaction_expiry";
      advanceSession(next, deadlineEvent);
      materials.push(deadlineEvent);
      eventType = "deadline_resolution_recorded";
      break;
    }
    case "record_transaction_deadline_elapsed": {
      const occurredAt = integerValue(action.occurred_at, "occurred_at");
      requireDeadlineAction(
        state,
        actor,
        admittedAt,
        occurredAt,
        "awaiting_transaction_expiry",
      );
      if (state.session_state === "active") {
        reject("invalid_phase", "the active session must expire on its own stream first");
      }
      const deadlineEvent = record(
        action.deadline_event,
        "deadline_event",
      ) as unknown as ExactA202Object;
      validateExactObject(deadlineEvent, "venue_signer", "event_append");
      next.aggregate_state = "expired";
      advanceTransaction(next, deadlineEvent);
      materials.push(deadlineEvent);
      if (state.session_state === "opened" || state.session_state === "accepted") {
        next.deadline_progress = "awaiting_session_close";
      } else {
        finishExpired(next);
      }
      eventType = next.phase === "expired" ? "formation_expired" : "deadline_resolution_recorded";
      break;
    }
    case "close_transaction_expired_session": {
      requirePhase(state, ["deadline_resolution"]);
      if (actor !== "venue_signer") {
        reject("role_violation", "only venue_signer may close an expired session");
      }
      if (
        state.deadline_progress !== "awaiting_session_close" ||
        state.aggregate_state !== "expired" ||
        (state.session_state !== "opened" && state.session_state !== "accepted")
      ) {
        reject("invalid_phase", "expired transaction session is not awaiting close");
      }
      const close = record(action.close_event, "close_event") as unknown as ExactA202Object;
      validateExactObject(close, "venue_signer", "event_append");
      next.session_state = "closed";
      advanceSession(next, close);
      materials.push(close);
      finishExpired(next);
      eventType = "formation_expired";
      break;
    }
  }

  advanceRoom(next, actionId, action);
  recordEvidence(next, actionId, materials);
  return { actionId, actionKind: kind, attention, eventType, state: next };
}

export function applyTimer(
  state: NegotiateState,
  timer: Record<string, CanonicalJson>,
): RuleTransition {
  const kind = stringValue(timer.timer, "Timer kind");
  const generation = integerValue(timer.generation, "Timer generation");
  const scheduledFor = integerValue(timer.scheduled_for, "Timer scheduled_for");
  const firedAt = integerValue(timer.fired_at, "Timer fired_at");
  if (generation === 0 || firedAt < scheduledFor) {
    reject("invalid_phase", "Timer generation and scheduled time must be authoritative");
  }
  const next = cloneState(state);
  const attention: Attention[] = [];
  if (kind === "proposal_validity") {
    if (state.current_proposal === null) reject("invalid_phase", "proposal Timer has no proposal");
    if (
      optionalString(timer.expected_object_id, "expected_object_id") !==
        state.current_proposal.offer.object_id ||
      scheduledFor !== state.current_proposal.valid_until
    ) {
      reject("stale_session_head", "proposal Timer does not name the current revision");
    }
    next.proposal_expired = true;
    next.pending_approval = null;
    next.recorded_approval = null;
    next.phase = "formation_open";
  } else if (kind === "approval_expiry") {
    if (state.pending_approval === null) reject("invalid_phase", "approval Timer has no binding");
    if (
      optionalString(timer.expected_object_id, "expected_object_id") !==
        state.pending_approval.candidate_wire_digest ||
      scheduledFor !== state.pending_approval.expires_at
    ) {
      reject("approval_binding_mismatch", "approval Timer does not name the exact candidate");
    }
    next.pending_approval = null;
    next.recorded_approval = null;
    next.phase = "formation_open";
  } else if (kind === "formation_deadline") {
    if (scheduledFor !== state.formation_deadline) {
      reject("invalid_phase", "formation Timer differs from the immutable deadline");
    }
    if (state.phase !== "complete" && state.phase !== "expired") {
      next.phase = "deadline_resolution";
      next.pending_approval = null;
      next.recorded_approval = null;
      next.deadline_progress =
        state.session_state === "active"
          ? "awaiting_session_expiry"
          : "awaiting_transaction_expiry";
      attention.push({ target: "venue_signer", reason: "venue_signature_required" });
    }
  } else {
    reject("invalid_phase", "Timer kind is not declared by Negotiate");
  }
  const timerName =
    kind === "proposal_validity"
      ? "proposalvalidity"
      : kind === "approval_expiry"
        ? "approvalexpiry"
        : "formationdeadline";
  const actionId = `timer:${timerName}:${generation}`;
  advanceRoom(next, actionId, timer);
  return {
    actionId,
    actionKind: null,
    attention,
    eventType: "timer_elapsed",
    state: next,
  };
}

export function offers(state: NegotiateState, role: Role): ActionKind[] {
  if (state.phase === "complete" || state.phase === "expired") return [];
  if (state.phase === "formation_open") return formationOffers(state, role);
  if (state.phase === "approval_pending") return approvalOffers(state, role);
  if (state.phase === "offer_accepted") {
    return role === "buyer_agent" ? ["select_accepted_proposal"] : [];
  }
  if (state.phase === "agreement_pending") return agreementOffers(state, role);
  if (state.phase === "deadline_resolution") return deadlineOffers(state, role);
  return [];
}

function formationOffers(state: NegotiateState, role: Role): ActionKind[] {
  if (state.current_proposal === null || state.proposal_expired) {
    return role === "buyer_agent" || role === "seller_agent"
      ? ["submit_proposal_revision"]
      : [];
  }
  if (role === state.current_proposal.author) return ["withdraw_live_proposal"];
  if (!isCounterparty(role, state.current_proposal.author)) return [];
  const result: ActionKind[] = ["submit_proposal_revision"];
  if (role === "buyer_agent" && state.current_proposal.author === "seller_agent") {
    result.push("request_exact_approval");
  }
  return result;
}

function approvalOffers(state: NegotiateState, role: Role): ActionKind[] {
  if (state.current_proposal === null) return [];
  if (role === "seller_agent") return ["withdraw_live_proposal"];
  if (role === "buyer_agent") {
    return state.recorded_approval?.decision === "approved"
      ? ["submit_proposal_revision", "accept_current_proposal"]
      : ["submit_proposal_revision"];
  }
  if (role === "buyer_approver" && state.recorded_approval === null) {
    return ["record_exact_approval"];
  }
  return [];
}

function agreementOffers(state: NegotiateState, role: Role): ActionKind[] {
  if (role !== "buyer_agent" && role !== "seller_agent") return [];
  const result: ActionKind[] = [];
  if (state.agreement_signatures[role] === undefined) {
    result.push("record_agreement_signature");
  }
  if (role === "buyer_agent" && Object.keys(state.agreement_signatures).length === 2) {
    result.push("commit_agreement");
  }
  return result;
}

function deadlineOffers(state: NegotiateState, role: Role): ActionKind[] {
  if (role !== "venue_signer") return [];
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

function validateSubmit(
  state: NegotiateState,
  actor: Role,
  admittedAt: number,
  proposal: Proposal,
): void {
  requirePhase(state, ["formation_open", "approval_pending"]);
  if (
    (actor !== "buyer_agent" && actor !== "seller_agent") ||
    proposal.author !== actor
  ) {
    reject("role_violation", "proposal author must be the acting commercial Role");
  }
  if (
    proposal.valid_until <= admittedAt ||
    proposal.valid_until > state.formation_deadline
  ) {
    reject("proposal_expired", "proposal validity is outside the formation window");
  }
  if (state.current_proposal === null) {
    if (proposal.supersedes_offer_id !== null) {
      reject("approval_binding_mismatch", "the first proposal cannot supersede another proposal");
    }
    return;
  }
  if (actor === state.current_proposal.author) {
    reject("role_violation", "only the current offeree may counter");
  }
  if (proposal.supersedes_offer_id !== state.current_proposal.offer.object_id) {
    reject("stale_session_head", "counter does not supersede the immediate proposal");
  }
}

function validateBinding(
  state: NegotiateState,
  binding: ApprovalBinding,
  admittedAt: number,
): void {
  const proposal = state.current_proposal;
  if (proposal === null) reject("approval_binding_mismatch", "proposal is missing");
  if (proposal.author !== "seller_agent" || state.proposal_expired) {
    reject("approval_binding_mismatch", "approval requires a live seller-authored proposal");
  }
  validateCanonicalText(binding.candidate_canonical_json);
  if (
    taggedBlake3Text(binding.candidate_canonical_json) !== binding.candidate_wire_digest ||
    binding.transaction_id !== state.transaction_id ||
    binding.proposal_id !== proposal.offer.object_id ||
    binding.proposal_content_hash !== proposal.offer.declared_content_hash ||
    !sameCanonical(binding.room_head_at_request, state.room_head) ||
    !sameCanonical(binding.transaction_head_at_request, state.transaction_head) ||
    !sameCanonical(binding.session_head_at_request, state.session_head) ||
    binding.approver_id !== signerId("buyer_approver")
  ) {
    reject("approval_binding_mismatch", "approval does not bind exact bytes, heads, and approver");
  }
  if (
    binding.expires_at <= admittedAt ||
    binding.expires_at > proposal.valid_until ||
    binding.expires_at > state.formation_deadline
  ) {
    reject("approval_expired", "approval expiry is outside the live proposal window");
  }
}

function validateApproval(
  pending: ApprovalBinding,
  approval: ExactApproval,
  admittedAt: number,
): void {
  if (!sameCanonical(approval.binding, pending)) {
    reject("approval_binding_mismatch", "signed approval differs from the pending candidate");
  }
  if (admittedAt >= pending.expires_at) {
    reject("approval_expired", "approval was recorded at or after expiry");
  }
  validateExactObject(approval.approval, "buyer_approver", "object_issuance");
}

function validateAcceptance(
  state: NegotiateState,
  admittedAt: number,
  candidate: string,
  approval: ExactApproval,
): void {
  if (state.pending_approval === null || state.recorded_approval === null) {
    reject("approval_binding_mismatch", "pending or recorded approval is missing");
  }
  if (
    !sameCanonical(state.recorded_approval, approval) ||
    !sameCanonical(approval.binding, state.pending_approval) ||
    approval.decision !== "approved"
  ) {
    reject("approval_binding_mismatch", "submitted approval is not the recorded approval");
  }
  if (
    candidate !== state.pending_approval.candidate_canonical_json ||
    taggedBlake3Text(candidate) !== state.pending_approval.candidate_wire_digest
  ) {
    reject("byte_mutation", "acceptance candidate is not byte-identical to approval");
  }
  if (!sameCanonical(state.transaction_head, state.pending_approval.transaction_head_at_request)) {
    reject("stale_transaction_head", "transaction stream advanced after approval");
  }
  if (!sameCanonical(state.session_head, state.pending_approval.session_head_at_request)) {
    reject("stale_session_head", "session stream advanced after approval");
  }
  if (state.current_proposal === null) {
    reject("approval_binding_mismatch", "current proposal is missing");
  }
  if (
    state.pending_approval.proposal_id !== state.current_proposal.offer.object_id ||
    state.pending_approval.proposal_content_hash !==
      state.current_proposal.offer.declared_content_hash
  ) {
    reject("approval_binding_mismatch", "current proposal differs from approval");
  }
  if (admittedAt >= state.current_proposal.valid_until) {
    reject("proposal_expired", "proposal expired before acceptance");
  }
  if (admittedAt >= state.pending_approval.expires_at) {
    reject("approval_expired", "approval expired before acceptance");
  }
}

function validateAgreementSignature(
  state: NegotiateState,
  actor: Role,
  signature: AgreementSignature,
): void {
  if (
    (actor !== "buyer_agent" && actor !== "seller_agent") ||
    signature.signer_role !== actor ||
    signature.signer_id !== signerId(actor)
  ) {
    reject("wrong_signer", "Agreement signer does not match the acting commercial Role");
  }
  if (state.agreement_signatures[actor] !== undefined) {
    reject("invalid_phase", "Agreement signer was already recorded");
  }
  if (
    taggedBlake3Text(signature.agreement_canonical_json) !== signature.agreement_wire_digest
  ) {
    reject("invalid_signature", "Agreement signature does not bind exact bytes");
  }
  validateAgreementProof(signature);
  if (
    state.agreement_id !== null &&
    (state.agreement_id !== signature.agreement_id ||
      state.agreement_content_hash !== signature.agreement_content_hash ||
      state.agreement_canonical_json !== signature.agreement_canonical_json)
  ) {
    reject("byte_mutation", "commercial parties did not sign identical Agreement bytes");
  }
}

function validateCommitment(state: NegotiateState, agreement: ExactA202Object): void {
  if (
    state.agreement_id !== agreement.object_id ||
    state.agreement_content_hash !== agreement.declared_content_hash ||
    state.agreement_canonical_json !== agreement.canonical_json ||
    Object.values(state.agreement_signatures).some(
      (signature) => signature?.agreement_wire_digest !== agreement.wire_digest,
    )
  ) {
    reject("byte_mutation", "committed Agreement differs from independently signed bytes");
  }
}

function validateBasis(state: NegotiateState, basis: ActionBasis): void {
  if (!sameCanonical(state.room_head, basis.room)) {
    reject("stale_room_head", "Room Head is stale");
  }
  if (!sameCanonical(state.transaction_head, basis.transaction)) {
    reject("stale_transaction_head", "A202 transaction Head is stale");
  }
  if (!sameCanonical(state.session_head, basis.session)) {
    reject("stale_session_head", "A202 session Head is stale");
  }
}

function requireDeadlineAction(
  state: NegotiateState,
  actor: Role,
  admittedAt: number,
  occurredAt: number,
  progress: NegotiateState["deadline_progress"],
): void {
  requirePhase(state, ["deadline_resolution"]);
  if (actor !== "venue_signer") {
    reject("role_violation", "only venue_signer may append deadline events");
  }
  if (state.deadline_progress !== progress) {
    reject("invalid_phase", "deadline event is not the next signed stream act");
  }
  if (
    admittedAt <= state.formation_deadline ||
    occurredAt <= state.formation_deadline ||
    occurredAt > admittedAt
  ) {
    reject("deadline_not_passed", "signed deadline time must be strictly after the deadline");
  }
}

function requirePhase(state: NegotiateState, phases: readonly Phase[]): void {
  if (!phases.includes(state.phase)) reject("invalid_phase", "Action is not legal in this phase");
}

function advanceRoom(
  state: NegotiateState,
  actionId: string,
  material: Record<string, CanonicalJson>,
): void {
  state.room_head = {
    sequence: state.room_head.sequence + 1,
    digest: taggedBlake3Text(
      canonicalStringify({
        action_id: actionId,
        domain: "worldstream/negotiate-room-head/v1",
        material,
        previous: state.room_head as unknown as CanonicalJson,
      }),
    ),
  };
}

function advanceSession(state: NegotiateState, event: ExactA202Object): void {
  state.session_head = {
    sequence: state.session_head.sequence + 1,
    event_hash: `sha256:${event.declared_content_hash}`,
  };
}

function advanceTransaction(state: NegotiateState, event: ExactA202Object): void {
  state.transaction_head = {
    sequence: state.transaction_head.sequence + 1,
    event_hash: `sha256:${event.declared_content_hash}`,
  };
}

function recordEvidence(
  state: NegotiateState,
  actionId: string,
  objects: readonly ExactA202Object[],
): void {
  for (const object of objects) {
    state.evidence.push({
      object_id: object.object_id,
      object_type: object.object_type,
      content_hash: object.declared_content_hash,
      wire_digest: object.wire_digest,
      action_id: actionId,
      room_sequence: state.room_head.sequence,
      room_digest: state.room_head.digest,
      transaction_head: cloneHead(state.transaction_head),
      session_head: cloneHead(state.session_head),
    });
  }
}

function finishExpired(state: NegotiateState): void {
  state.phase = "expired";
  state.deadline_progress = "resolved";
  state.outcome = {
    kind: "formation_expired",
    transaction_id: state.transaction_id,
    final_session_state: state.session_state,
  };
}

function initialLogicalHead(
  configured: CanonicalJson | undefined,
  sequence: number,
  eventHash: string,
): LogicalHead {
  if (configured === undefined) return { sequence, event_hash: eventHash };
  const value = record(configured, "logical Head");
  return {
    sequence: integerValue(value.sequence, "logical Head sequence"),
    event_hash: stringValue(value.event_hash, "logical Head digest"),
  };
}

function cloneHead(head: LogicalHead): LogicalHead {
  return { sequence: head.sequence, event_hash: head.event_hash };
}

function actionKind(value: CanonicalJson | undefined): ActionKind {
  const kind = stringValue(value, "Action kind");
  if (
    kind === "submit_proposal_revision" ||
    kind === "withdraw_live_proposal" ||
    kind === "request_exact_approval" ||
    kind === "record_exact_approval" ||
    kind === "accept_current_proposal" ||
    kind === "select_accepted_proposal" ||
    kind === "record_agreement_signature" ||
    kind === "commit_agreement" ||
    kind === "record_session_deadline_elapsed" ||
    kind === "record_transaction_deadline_elapsed" ||
    kind === "close_transaction_expired_session"
  ) {
    return kind;
  }
  reject("invalid_phase", "Action kind is not declared by Negotiate");
}

function isDeadlineAction(kind: ActionKind): boolean {
  return (
    kind === "record_session_deadline_elapsed" ||
    kind === "record_transaction_deadline_elapsed" ||
    kind === "close_transaction_expired_session"
  );
}

function isCounterparty(left: Role, right: Role): boolean {
  return (
    (left === "buyer_agent" && right === "seller_agent") ||
    (left === "seller_agent" && right === "buyer_agent")
  );
}

function counterparty(role: Role): Role {
  if (role === "buyer_agent") return "seller_agent";
  if (role === "seller_agent") return "buyer_agent";
  return role;
}

function pinnedRevision(): string {
  return "fa85aa8b49bfe7b3f7ded487c98500a600e92e41";
}

export function stateAsCanonical(state: NegotiateState): CanonicalObject {
  return asCanonical(state);
}
