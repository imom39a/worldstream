import { readFileSync } from "node:fs";

import type { CanonicalJson } from "@worldstream/pack-sdk";

interface OracleStep {
  readonly stimulus: {
    readonly stimulus: "action";
    readonly value: Record<string, CanonicalJson>;
  };
}

interface OracleCorpus {
  readonly a202_revision: string;
  readonly golden: {
    readonly a202_revision: string;
    readonly steps: readonly OracleStep[];
  };
}

const corpus = JSON.parse(
  readFileSync(
    new URL(
      "../../../../../crates/worldstream-negotiate-oracle/fixtures/corpus-v1.json",
      import.meta.url,
    ),
    "utf8",
  ),
) as OracleCorpus;

const participants = [
  participant("01ARZ3NDEKTSV4RRFFQ69G5FC0", "01ARZ3NDEKTSV4RRFFQ69G5FD0", "buyer_agent", "agent"),
  participant("01ARZ3NDEKTSV4RRFFQ69G5FC1", "01ARZ3NDEKTSV4RRFFQ69G5FD1", "seller_agent", "agent"),
  participant("01ARZ3NDEKTSV4RRFFQ69G5FC2", "01ARZ3NDEKTSV4RRFFQ69G5FD2", "buyer_approver", "human"),
  participant("01ARZ3NDEKTSV4RRFFQ69G5FC3", "01ARZ3NDEKTSV4RRFFQ69G5FD3", "venue_signer", "agent"),
  observer("01ARZ3NDEKTSV4RRFFQ69G5FC8", "01ARZ3NDEKTSV4RRFFQ69G5FD8", "spectator"),
  observer("01ARZ3NDEKTSV4RRFFQ69G5FC9", "01ARZ3NDEKTSV4RRFFQ69G5FD9", "operator"),
] as const;

const accepted = corpus.golden.steps.map((step) => {
  const action = step.stimulus.value;
  const actor = String(action.actor);
  return {
    action_type: String(action.action),
    admitted_at: semanticTimestamp(Number(action.admitted_at)),
    canonical_payload: action,
    member_id: memberForRole(actor),
  };
});

const rejectedAction = corpus.golden.steps[4]!.stimulus.value;

export const goldenFixture = {
  accepted,
  buyer: participants[0],
  configuration: {
    a202_revision: corpus.golden.a202_revision,
    formation_deadline: 1_000,
    session_id: "ses_northstar_delta_worldstream_01",
    transaction_id: "txn_calibration_worldstream_01",
  },
  created_at: semanticTimestamp(0),
  participants,
  rejected: {
    action_type: String(rejectedAction.action),
    admitted_at: semanticTimestamp(Number(rejectedAction.admitted_at)),
    canonical_payload: rejectedAction,
    member_id: memberForRole(String(rejectedAction.actor)),
  },
  seller: participants[1],
} as const satisfies CanonicalJson;

function participant(
  memberId: string,
  principalId: string,
  role: string,
  principalKind: "agent" | "human",
): CanonicalJson {
  return {
    access_mode: "participant",
    member_id: memberId,
    principal_id: principalId,
    principal_kind: principalKind,
    role,
    standing: "enabled",
  };
}

function observer(
  memberId: string,
  principalId: string,
  accessMode: "operator" | "spectator",
): CanonicalJson {
  return {
    access_mode: accessMode,
    member_id: memberId,
    principal_id: principalId,
    principal_kind: "human",
    role: null,
    standing: "enabled",
  };
}

function memberForRole(role: string): string {
  if (role === "buyer_agent") return "01ARZ3NDEKTSV4RRFFQ69G5FC0";
  if (role === "seller_agent") return "01ARZ3NDEKTSV4RRFFQ69G5FC1";
  if (role === "buyer_approver") return "01ARZ3NDEKTSV4RRFFQ69G5FC2";
  if (role === "venue_signer") return "01ARZ3NDEKTSV4RRFFQ69G5FC3";
  throw new TypeError(`unknown oracle Role ${role}`);
}

function semanticTimestamp(value: number): string {
  return new Date(value * 1_000).toISOString().replace(".000Z", "Z");
}
