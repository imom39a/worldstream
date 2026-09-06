import { strict as assert } from "node:assert";
import { test } from "vitest";

import { readPublicRunDto, readRecentResultsDto } from "./public-runs.js";

const digest = `blake3:${"a".repeat(64)}`;

function resultRun(overrides: Record<string, unknown> = {}) {
  return {
    version: "public_run.v1",
    state: "result",
    public_id: "1".repeat(32),
    activity: {
      listing_key: "worldstream.agent-heist.public-preview",
      title: "Agent Heist",
      description: "A live social strategy activity.",
      listing_revision: digest,
      pack: { id: "worldstream.agent-heist", version: "0.2.0", revision: digest },
    },
    started_at: "2026-09-05T10:00:00.000Z",
    completed_at: "2026-09-05T10:05:00.000Z",
    evidence: { class: "unranked", label: "Unranked activity" },
    participants: [
      {
        seat_label: "Navigator",
        role: "navigator",
        kind: "human",
        identity: {
          kind: "github",
          login: "octocat",
          avatar_url: "https://avatars.githubusercontent.com/u/1?v=4",
          fallback_label: "Navigator",
        },
      },
      {
        seat_label: "Insider",
        role: "insider",
        kind: "external_agent",
        identity: { kind: "pseudonym", label: "Insider" },
        notice: "External agent — unverified",
      },
    ],
    result: {
      status: "summary",
      summary: {
        schema: "worldstream/result-summary/v1",
        outcome: "success",
        selected_plan_id: "plan-alpha",
        score: 5,
        reason: "scored_selected_plan",
      },
    },
    ...overrides,
  };
}

test("accepts only the reviewed public Run envelope", () => {
  const parsed = readPublicRunDto(resultRun());
  assert.equal(parsed.state, "result");
  if (parsed.state !== "result") return;
  assert.equal(parsed.public_id, "1".repeat(32));
  assert.equal(parsed.participants[1]?.kind, "external_agent");
  assert.equal(parsed.result.status, "summary");
});

test("an unavailable Run cannot carry a stale summary or attribution", () => {
  assert.deepEqual(
    readPublicRunDto({ version: "public_run.v1", state: "unavailable" }),
    { version: "public_run.v1", state: "unavailable" },
  );
  assert.throws(() => readPublicRunDto({
    version: "public_run.v1",
    state: "unavailable",
    participants: [],
    result: { status: "summary" },
  }), /public_run_dto_rejected/u);
});

test("rejects private correspondence and raw execution material at every result depth", () => {
  for (const forbidden of [
    "room_id",
    "membership_id",
    "principal_id",
    "account_id",
    "entry_selector",
    "invitation_token",
    "replay",
    "prompt",
    "provider_response",
  ]) {
    assert.throws(
      () => readPublicRunDto(resultRun({ result: { summary: { [forbidden]: "secret" } } })),
      /public_run_dto_rejected/u,
      forbidden,
    );
  }
});

test("accepts exact House attribution without prompts or provider responses", () => {
  const value = resultRun({
    evidence: {
      class: "exhibition_platform_house_agents",
      label: "Exhibition — platform-supplied agents",
    },
    participants: [{
      seat_label: "Navigator",
      role: "navigator",
      kind: "house_agent",
      notice: "Exhibition — platform-supplied agents",
      house_agent: {
        display_name: "Cooperative Planner",
        revision_digest: digest,
        route: {
          gateway: "openrouter",
          provider_slug: "alibaba",
          model_slug: "qwen/qwen3.8-flash-20260826",
        },
        allowance: {
          model_call_attempts: 10,
          total_input_tokens: 120000,
          total_output_tokens: 10000,
          input_tokens_per_call: 12000,
          output_tokens_per_call: 1000,
          concurrent_calls: 1,
          call_timeout_seconds: 60,
        },
      },
    }],
  });
  const parsed = readPublicRunDto(value);
  assert.equal(parsed.state, "result");
  if (parsed.state !== "result") return;
  assert.equal(parsed.participants[0]?.kind, "house_agent");
});

test("Recent Results is Agent-Heist-only, newest-first, and capped at twenty", () => {
  const second = resultRun({
    public_id: "2".repeat(32),
    completed_at: "2026-09-05T10:04:00.000Z",
  });
  const parsed = readRecentResultsDto({
    version: "recent_results.v1",
    activity: "agent-heist",
    order: "newest_first",
    maximum: 20,
    results: [resultRun(), second],
  });
  assert.equal(parsed.results.length, 2);

  assert.throws(() => readRecentResultsDto({
    version: "recent_results.v1",
    activity: "agent-heist",
    order: "newest_first",
    maximum: 20,
    results: Array.from({ length: 21 }, (_, index) => resultRun({
      public_id: index.toString(16).padStart(32, "0"),
    })),
  }), /public_run_dto_rejected/u);
  assert.throws(() => readRecentResultsDto({
    version: "recent_results.v1",
    activity: "agent-heist",
    order: "newest_first",
    maximum: 20,
    results: [resultRun({
      activity: {
        listing_key: "worldstream.negotiate.preview",
        title: "Negotiate",
        description: "Negotiation",
        listing_revision: digest,
        pack: { id: "worldstream.negotiate", version: "1.0.0", revision: digest },
      },
    })],
  }), /public_run_dto_rejected/u);
});
