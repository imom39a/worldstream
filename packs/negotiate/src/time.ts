import type { CanonicalJson } from "@worldstream/pack-sdk";

import { integerValue, record, reject, stringValue } from "./model.js";

export function timestampFromSemanticSecond(value: number): string {
  if (!Number.isSafeInteger(value) || value < 0) {
    reject("resource_limit", "semantic seconds must be a non-negative safe integer");
  }
  const days = Math.floor(value / 86_400);
  const secondOfDay = value - days * 86_400;
  const z = days + 719_468;
  const era = Math.floor((z >= 0 ? z : z - 146_096) / 146_097);
  const dayOfEra = z - era * 146_097;
  const yearOfEra = Math.floor(
    (dayOfEra -
      Math.floor(dayOfEra / 1_460) +
      Math.floor(dayOfEra / 36_524) -
      Math.floor(dayOfEra / 146_096)) /
      365,
  );
  let year = yearOfEra + era * 400;
  const dayOfYear =
    dayOfEra -
    (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
  const monthPrime = Math.floor((5 * dayOfYear + 2) / 153);
  const day = dayOfYear - Math.floor((153 * monthPrime + 2) / 5) + 1;
  const month = monthPrime + (monthPrime < 10 ? 3 : -9);
  year += month <= 2 ? 1 : 0;
  if (year < 0 || year > 9999) {
    reject("resource_limit", "semantic seconds are outside the supported timestamp range");
  }
  const hour = Math.floor(secondOfDay / 3_600);
  const minute = Math.floor((secondOfDay % 3_600) / 60);
  const second = secondOfDay % 60;
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}T${pad(hour, 2)}:${pad(minute, 2)}:${pad(second, 2)}Z`;
}

export function timerRequestsForTransition(
  before: Record<string, CanonicalJson>,
  after: Record<string, CanonicalJson>,
  stimulus: Record<string, CanonicalJson>,
  scheduled: Record<string, CanonicalJson>,
): CanonicalJson[] {
  const requests: CanonicalJson[] = [];
  if (stimulus.stimulus_type === "participant_action") {
    const action = record(stimulus.canonical_payload, "Action payload");
    const kind = stringValue(action.action, "Action kind");
    if (kind === "submit_proposal_revision") {
      const proposal = record(action.proposal, "proposal");
      replaceTimer(
        requests,
        scheduled,
        proposalTimerId(),
        integerValue(proposal.valid_until, "proposal validity"),
        {
          expected_object_id: stringValue(
            record(proposal.offer, "proposal offer").object_id,
            "offer identity",
          ),
          fired_at: integerValue(proposal.valid_until, "proposal validity"),
          generation: nextGeneration(scheduled, proposalTimerId()),
          scheduled_for: integerValue(proposal.valid_until, "proposal validity"),
          timer: "proposal_validity",
        },
      );
      cancelTimer(requests, scheduled, approvalTimerId());
    } else if (kind === "request_exact_approval") {
      const binding = record(action.binding, "approval binding");
      replaceTimer(
        requests,
        scheduled,
        approvalTimerId(),
        integerValue(binding.expires_at, "approval expiry"),
        {
          expected_object_id: stringValue(
            binding.candidate_wire_digest,
            "candidate digest",
          ),
          fired_at: integerValue(binding.expires_at, "approval expiry"),
          generation: nextGeneration(scheduled, approvalTimerId()),
          scheduled_for: integerValue(binding.expires_at, "approval expiry"),
          timer: "approval_expiry",
        },
      );
    } else if (kind === "withdraw_live_proposal" || kind === "accept_current_proposal") {
      cancelTimer(requests, scheduled, proposalTimerId());
      cancelTimer(requests, scheduled, approvalTimerId());
    } else if (kind === "record_exact_approval") {
      const approval = record(action.approval, "exact approval");
      if (approval.decision === "rejected") cancelTimer(requests, scheduled, approvalTimerId());
    } else if (kind === "commit_agreement") {
      cancelTimer(requests, scheduled, formationTimerId());
    }
  } else if (stimulus.stimulus_type === "timer_fired") {
    const payload = record(stimulus.canonical_payload, "Timer payload");
    if (payload.timer === "formation_deadline") {
      cancelTimer(requests, scheduled, proposalTimerId());
      cancelTimer(requests, scheduled, approvalTimerId());
    }
  }
  if (before.phase === after.phase && requests.length === 0) return [];
  return requests;
}

export function initialTimerRequests(formationDeadline: number): CanonicalJson[] {
  return [
    {
      canonical_payload: {
        expected_object_id: null,
        fired_at: formationDeadline,
        generation: 1,
        scheduled_for: formationDeadline,
        timer: "formation_deadline",
      },
      due: timestampFromSemanticSecond(formationDeadline),
      timer_id: formationTimerId(),
      timer_request_type: "schedule_next",
    },
  ];
}

export function timerIdForKind(kind: string): string {
  if (kind === "proposal_validity") return proposalTimerId();
  if (kind === "approval_expiry") return approvalTimerId();
  if (kind === "formation_deadline") return formationTimerId();
  reject("invalid_phase", "Timer kind is not declared by Negotiate");
}

function replaceTimer(
  output: CanonicalJson[],
  scheduled: Record<string, CanonicalJson>,
  timerId: string,
  due: number,
  payload: CanonicalJson,
): void {
  const current = scheduled[timerId];
  if (current === undefined) {
    output.push({
      canonical_payload: payload,
      due: timestampFromSemanticSecond(due),
      timer_id: timerId,
      timer_request_type: "schedule_next",
    });
    return;
  }
  output.push({
    expected_generation: integerValue(
      record(current, "scheduled Timer").generation,
      "Timer generation",
    ),
    new_canonical_payload: payload,
    new_due: timestampFromSemanticSecond(due),
    timer_id: timerId,
    timer_request_type: "reschedule_current",
  });
}

function cancelTimer(
  output: CanonicalJson[],
  scheduled: Record<string, CanonicalJson>,
  timerId: string,
): void {
  const current = scheduled[timerId];
  if (current === undefined) return;
  output.push({
    expected_generation: integerValue(
      record(current, "scheduled Timer").generation,
      "Timer generation",
    ),
    timer_id: timerId,
    timer_request_type: "cancel_current",
  });
}

function nextGeneration(scheduled: Record<string, CanonicalJson>, timerId: string): number {
  const current = scheduled[timerId];
  if (current === undefined) return 1;
  return integerValue(record(current, "scheduled Timer").generation, "Timer generation") + 1;
}

function proposalTimerId(): string {
  return "01ARZ3NDEKTSV4RRFFQ69G5FA0";
}

function approvalTimerId(): string {
  return "01ARZ3NDEKTSV4RRFFQ69G5FA1";
}

function formationTimerId(): string {
  return "01ARZ3NDEKTSV4RRFFQ69G5FA2";
}

function pad(value: number, length: number): string {
  return String(value).padStart(length, "0");
}
