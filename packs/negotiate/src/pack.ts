import {
  canonicalStringify,
  defineActivityPack,
  taggedBlake3Text,
  type ActivityPackDefinition,
  type CanonicalJson,
  type CanonicalObject,
  type PackReduceOutput,
} from "@worldstream/pack-sdk";

import type { Attention, NegotiateState, Role, RuleTransition } from "./model.js";
import {
  RuleRejection,
  asCanonical,
  cloneState,
  isRole,
  record,
  roleValue,
  stateValue,
  stringValue,
} from "./model.js";
import {
  applyParticipantAction,
  applyTimer,
  initializeState,
  stateAsCanonical,
} from "./rules.js";
import {
  initialTimerRequests,
  semanticSecondFromTimestamp,
  timerIdForKind,
  timerRequestsForTransition,
  timestampFromSemanticSecond,
} from "./time.js";
import { authorizedView } from "./view.js";

export function reduceNegotiate(input: CanonicalObject): PackReduceOutput {
  const before = stateValue(input.prior_activity_state);
  const coreBefore = record(input.core_before, "core_before");
  const coreAfter = record(input.proposed_core_after, "proposed_core_after");
  const stimulus = record(input.recorded_stimulus, "recorded_stimulus");
  const scheduled = record(input.scheduled_timers, "scheduled_timers");
  const stimulusType = stringValue(stimulus.stimulus_type, "stimulus_type");

  try {
    validateCoreInvariant(coreBefore);
    validateCoreInvariant(coreAfter);
    if (stimulusType === "participant_action") {
      const publicPayload = record(stimulus.canonical_payload, "Action payload");
      const action = adaptParticipantAction(before, stimulus, publicPayload, coreBefore);
      return appliedDisposition(
        before,
        applyParticipantAction(before, action),
        { ...stimulus, canonical_payload: action },
        scheduled,
        coreAfter,
      );
    }
    if (stimulusType === "timer_fired") {
      const timer = record(stimulus.canonical_payload, "Timer payload");
      validateTimerEnvelope(stimulus, timer);
      return appliedDisposition(
        before,
        applyTimer(before, timer),
        stimulus,
        scheduled,
        coreAfter,
      );
    }
    if (stimulusType === "core_proposed") {
      const next = applyCoreProposal(before, stimulus, input.next_room_seq);
      return {
        activity_disposition_type: "apply",
        next_activity_state: stateAsCanonical(next),
        ordered_domain_events: [],
        timer_requests: [],
        ordered_attention_signals: [],
      };
    }
    throw new TypeError("Negotiate does not declare an external-input source");
  } catch (error) {
    if (error instanceof RuleRejection && cleanRejectionAllowed(stimulus)) {
      return {
        activity_disposition_type: "reject",
        declared_code: error.code,
        bounded_safe_details: { reason: error.message },
      };
    }
    throw error;
  }
}

export function validateCoreInvariant(core: Record<string, CanonicalJson>): void {
  const memberships = record(core.memberships, "Core memberships");
  const counts: Record<Role, number> = {
    buyer_agent: 0,
    seller_agent: 0,
    buyer_approver: 0,
    venue_signer: 0,
  };
  for (const value of Object.values(memberships)) {
    const membership = record(value, "Core membership");
    if (membership.role === null) {
      if (
        membership.access_mode !== "spectator" &&
        membership.access_mode !== "operator"
      ) {
        throw new RuleRejection(
          "core_role_invariant",
          "nonparticipant Memberships must use spectator or operator access",
        );
      }
      continue;
    }
    const roleText = stringValue(membership.role, "membership Role");
    if (!isRole(roleText)) continue;
    counts[roleText] += 1;
    const requiredKind = roleText === "buyer_approver" ? "human" : "agent";
    if (
      membership.principal_kind !== requiredKind ||
      membership.access_mode !== "participant"
    ) {
      throw new RuleRejection(
        "core_role_invariant",
        `${roleText} must retain its pinned principal kind and participant access`,
      );
    }
  }
  for (const count of Object.values(counts)) {
    if (count !== 1) {
      throw new RuleRejection(
        "core_role_invariant",
        "Negotiate requires exactly one retained Membership for each of four Roles",
      );
    }
  }
}

function appliedDisposition(
  before: NegotiateState,
  transition: RuleTransition,
  stimulus: Record<string, CanonicalJson>,
  scheduled: Record<string, CanonicalJson>,
  coreAfter: Record<string, CanonicalJson>,
): PackReduceOutput {
  const next = transition.state;
  return {
    activity_disposition_type: "apply",
    next_activity_state: stateAsCanonical(next),
    ordered_domain_events: [
      {
        action_id: transition.actionId,
        event_type: transition.eventType,
        phase: next.phase,
        room_head: next.room_head as unknown as CanonicalJson,
      },
    ],
    timer_requests: timerRequestsForTransition(
      before as unknown as Record<string, CanonicalJson>,
      next as unknown as Record<string, CanonicalJson>,
      stimulus,
      scheduled,
    ),
    ordered_attention_signals: transition.attention.map((attention) =>
      attentionSignal(attention, next, coreAfter)
    ),
  };
}

function adaptParticipantAction(
  state: NegotiateState,
  stimulus: Record<string, CanonicalJson>,
  publicPayload: Record<string, CanonicalJson>,
  core: Record<string, CanonicalJson>,
): Record<string, CanonicalJson> {
  for (const field of ["admitted_at", "basis"] as const) {
    if (Object.hasOwn(publicPayload, field)) {
      throw new RuleRejection(
        "invalid_phase",
        `Action payload must not supply Host-owned field ${field}`,
      );
    }
  }
  const outerType = stringValue(stimulus.action_type, "Action type");
  const innerType = stringValue(publicPayload.action, "Action payload type");
  if (outerType !== innerType) {
    throw new RuleRejection("invalid_phase", "Action envelope and payload type differ");
  }
  const memberId = stringValue(stimulus.member_id, "acting member_id");
  const membership = record(
    record(core.memberships, "Core memberships")[memberId],
    "acting membership",
  );
  const role = roleValue(membership.role, "acting membership Role");
  if (roleValue(publicPayload.actor, "Action actor") !== role) {
    throw new RuleRejection("role_violation", "Action actor differs from Core Membership Role");
  }
  return {
    ...publicPayload,
    admitted_at: semanticSecondFromTimestamp(
      stringValue(stimulus.admitted_at, "Action admitted_at"),
    ),
    basis: {
      room: state.room_head as unknown as CanonicalJson,
      transaction: state.transaction_head as unknown as CanonicalJson,
      session: state.session_head as unknown as CanonicalJson,
    },
  };
}

function validateTimerEnvelope(
  stimulus: Record<string, CanonicalJson>,
  timer: Record<string, CanonicalJson>,
): void {
  const kind = stringValue(timer.timer, "Timer kind");
  if (stringValue(stimulus.timer_id, "Timer id") !== timerIdForKind(kind)) {
    throw new RuleRejection("invalid_phase", "Timer identity differs from its immutable payload");
  }
  if (
    stimulus.generation !== timer.generation ||
    typeof timer.scheduled_for !== "number" ||
    !Number.isSafeInteger(timer.scheduled_for) ||
    stimulus.scheduled_for !== timestampFromSemanticSecond(timer.scheduled_for) ||
    timer.fired_at !== timer.scheduled_for
  ) {
    throw new RuleRejection(
      "invalid_phase",
      "Timer generation or scheduled semantic time differs from the host generation",
    );
  }
}

function applyCoreProposal(
  before: NegotiateState,
  stimulus: Record<string, CanonicalJson>,
  nextRoomSequence: CanonicalJson | undefined,
): NegotiateState {
  const next = cloneState(before);
  if (next.pending_approval !== null || next.recorded_approval !== null) {
    next.pending_approval = null;
    next.recorded_approval = null;
    if (next.phase === "approval_pending") next.phase = "formation_open";
  }
  if (typeof nextRoomSequence !== "number" || !Number.isSafeInteger(nextRoomSequence)) {
    throw new TypeError("next_room_seq must be a safe integer");
  }
  next.room_head = {
    digest: canonicalCoreDigest(stimulus, before.room_head.digest),
    sequence: nextRoomSequence,
  };
  return next;
}

function canonicalCoreDigest(
  stimulus: Record<string, CanonicalJson>,
  previous: string,
): string {
  const digest = canonicalStringify({
    domain: "worldstream/negotiate-core-head/v1",
    previous,
    stimulus,
  });
  return taggedBlake3Text(digest);
}

function attentionSignal(
  attention: Attention,
  state: NegotiateState,
  core: Record<string, CanonicalJson>,
): CanonicalJson {
  const memberId = memberForRole(core, attention.target, true);
  const deadline = attention.reason === "venue_signature_required"
    ? timestampFromSemanticSecond(state.formation_deadline + 1)
    : null;
  return {
    action_types: actionTypesForAttention(attention.reason),
    deadline,
    deduplication_key: `${attention.reason}:${attention.target}:${state.room_head.sequence}`,
    priority: 1,
    reason: attention.reason,
    target_member_id: memberId,
  };
}

function actionTypesForAttention(reason: Attention["reason"]): CanonicalJson {
  if (reason === "proposal_received") {
    return ["submit_proposal_revision", "request_exact_approval"];
  }
  if (reason === "agreement_signature_required") return ["record_agreement_signature"];
  if (reason === "agreement_ready") return ["commit_agreement"];
  return [
    "record_session_deadline_elapsed",
    "record_transaction_deadline_elapsed",
    "close_transaction_expired_session",
  ];
}

function memberForRole(
  core: Record<string, CanonicalJson>,
  role: Role,
  requireEnabledAgent: boolean,
): string {
  const memberships = record(core.memberships, "Core memberships");
  for (const [memberId, value] of Object.entries(memberships)) {
    const membership = record(value, "Core membership");
    if (membership.role !== role) continue;
    if (
      requireEnabledAgent &&
      (membership.standing !== "enabled" ||
        membership.principal_kind !== "agent" ||
        membership.access_mode !== "participant")
    ) {
      throw new RuleRejection(
        "core_role_invariant",
        "Attention target must be an enabled Agent participant",
      );
    }
    return memberId;
  }
  throw new RuleRejection("core_role_invariant", `Core has no Membership for ${role}`);
}

function cleanRejectionAllowed(stimulus: Record<string, CanonicalJson>): boolean {
  if (stimulus.stimulus_type === "participant_action") return true;
  if (stimulus.stimulus_type !== "core_proposed") return false;
  return (
    stimulus.kind === "join" ||
    stimulus.kind === "resume" ||
    stimulus.kind === "access_mode_change" ||
    stimulus.kind === "role_change" ||
    stimulus.kind === "membership_change_set"
  );
}

export default defineActivityPack({
  descriptor: {
    packId: "worldstream.negotiate",
    name: "WorldStream Negotiate",
    version: "0.2.0",
    roles: ["buyer_agent", "seller_agent", "buyer_approver", "venue_signer"],
    actions: [
      "submit_proposal_revision",
      "withdraw_live_proposal",
      "request_exact_approval",
      "record_exact_approval",
      "accept_current_proposal",
      "select_accepted_proposal",
      "record_agreement_signature",
      "commit_agreement",
      "record_session_deadline_elapsed",
      "record_transaction_deadline_elapsed",
      "close_transaction_expired_session",
    ],
    rejectionCodes: [
      "stale_room_head",
      "stale_transaction_head",
      "stale_session_head",
      "invalid_phase",
      "role_violation",
      "wrong_signer",
      "invalid_signature",
      "non_canonical_a202_bytes",
      "byte_mutation",
      "approval_binding_mismatch",
      "approval_expired",
      "proposal_expired",
      "deadline_reached",
      "deadline_not_passed",
      "resource_limit",
      "core_role_invariant",
    ],
    events: [
      "proposal_revised",
      "proposal_withdrawn",
      "approval_requested",
      "approval_recorded",
      "proposal_accepted",
      "proposal_selected",
      "agreement_signature_recorded",
      "agreement_committed",
      "deadline_resolution_recorded",
      "formation_expired",
      "timer_elapsed",
    ],
    attentionReasons: [
      "proposal_received",
      "agreement_signature_required",
      "agreement_ready",
      "venue_signature_required",
    ],
  },

  initialize(input) {
    const configuration = record(input.configuration, "configuration");
    validateCoreInvariant(record(input.initial_core_state, "initial_core_state"));
    const state = initializeState(configuration);
    return {
      initial_activity_state: stateAsCanonical(state),
      timer_requests: initialTimerRequests(state.formation_deadline),
    };
  },

  reduce(input) {
    return reduceNegotiate(input);
  },

  view(input) {
    const state = stateValue(input.activity_state);
    const result = authorizedView(
      state,
      record(input.core, "core"),
      record(input.viewer, "viewer"),
    );
    return {
      projection_schema: result.schema,
      projection: result.projection,
      action_offers: result.actionOffers,
    };
  },

  observe(input) {
    const coreBefore = record(input.core_before, "core_before");
    const coreAfter = record(input.core_after, "core_after");
    const viewer = record(input.viewer, "viewer");
    const before = authorizedView(stateValue(input.activity_before), coreBefore, viewer);
    const after = authorizedView(stateValue(input.activity_after), coreAfter, viewer);
    const projectionChanged =
      canonicalStringify(before.projection) !== canonicalStringify(after.projection);
    const offersChanged =
      canonicalStringify(before.actionOffers) !== canonicalStringify(after.actionOffers);
    if (!projectionChanged && !offersChanged) return null;
    return {
      observation_schema: after.schema,
      observation: {
        change_type: "projection_replaced",
        projection: after.projection,
      },
      action_offers: offersChanged ? "reuse_after_view" : "unchanged",
    };
  },
} satisfies ActivityPackDefinition);
