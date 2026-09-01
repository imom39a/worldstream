import { describe, expect, it, vi } from "vitest";

import {
  ActivityClientHandoffClient,
  selectActivityClientStartup,
  consumeActivityClientHandoffFragment,
  resumeRetainedActivityClient,
  type BrowserNavigationTarget,
} from "./index";

const HANDOFF = `wsh1:${"ab".repeat(32)}`;
const ROOM_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const BEARER = `wsb1:${"cd".repeat(32)}`;

function browser(hash: string): BrowserNavigationTarget & { replaced: string[] } {
  const replaced: string[] = [];
  const target = {
    location: {
      origin: "http://127.0.0.1:5173",
      pathname: "/",
      search: "",
      hash,
    },
    history: {
      replaceState: (_data: unknown, _unused: string, url?: string | URL | null) => {
        replaced.push(String(url));
        target.location.hash = "";
      },
    },
    replaced,
  };
  return target;
}

describe("Activity Client opaque handoff", () => {
  it("uses Activity Client names without changing the frozen handoff wire contract", async () => {
    const target = browser(`#handoff=${HANDOFF}`);
    const startup = selectActivityClientStartup(target);
    expect(startup).toEqual({ kind: "handoff", handoff: HANDOFF });
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "participant_console_session.v1",
      state: "usable",
      next_action: "continue",
    }), { status: 200 }));
    const client = new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch);
    await expect(client.redeem(startup.kind === "handoff" ? startup.handoff : "")).resolves.toMatchObject({
      state: "usable",
    });
  });

  it("consumes only an exact fragment and scrubs it before any network work", async () => {
    const target = browser(`#handoff=${HANDOFF}`);
    const fetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      expect(target.location.hash).toBe("");
      expect(init?.credentials).toBe("include");
      expect(init?.body).toBeUndefined();
      const headers = new Headers(init?.headers);
      expect(headers.has("authorization")).toBe(false);
      expect(headers.get("x-worldstream-participant-handoff")).toBe(HANDOFF);
      return new Response(JSON.stringify({
        version: "participant_console_session.v1",
        state: "usable",
        next_action: "continue",
      }), { status: 200, headers: { "content-type": "application/json" } });
    });

    const startup = selectActivityClientStartup(target);
    expect(startup).toEqual({ kind: "handoff", handoff: HANDOFF });
    expect(target.replaced).toEqual(["/"]);
    const status = await new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch).redeem(startup.kind === "handoff" ? startup.handoff : "");
    expect(status.state).toBe("usable");
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(ROOM_ID);
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(MEMBER_ID);
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(BEARER);
  });

  it("scrubs malformed handoff material and never converts it into direct setup", () => {
    const malformed = browser(`#handoff=${HANDOFF}&room_id=${ROOM_ID}`);
    expect(consumeActivityClientHandoffFragment(malformed)).toEqual({ kind: "invalid_handoff" });
    expect(malformed.location.hash).toBe("");

    const direct = browser("");
    expect(selectActivityClientStartup(direct)).toEqual({ kind: "direct" });
    expect(direct.replaced).toEqual([]);
  });

  it("keeps retained-cookie refresh browser-safe and fails closed on unknown response fields", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({
        version: "participant_console_session.v1",
        state: "disconnected",
        next_action: "reconnect",
      }), { status: 200, headers: { "content-type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({
        version: "participant_console_session.v1",
        state: "usable",
        next_action: "continue",
        member_id: MEMBER_ID,
      }), { status: 200, headers: { "content-type": "application/json" } }));
    const client = new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch);
    await expect(client.resume()).resolves.toMatchObject({ state: "disconnected" });
    expect(fetch.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9420/api/v1/participant-console/session");
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ method: "GET", credentials: "include", cache: "no-store" });
    await expect(client.resume()).rejects.toMatchObject({ code: "participant_session_invalid_response", nextAction: "return_to_task_setup" });
  });

  it("calls the native browser fetch with its global receiver", async () => {
    const fetch = vi.fn(function (this: typeof globalThis) {
      expect(this).toBe(globalThis);
      return Promise.resolve(new Response(JSON.stringify({
        version: "participant_console_session.v1", state: "usable", next_action: "continue",
      }), { status: 200 }));
    });
    vi.stubGlobal("fetch", fetch);
    await expect(new ActivityClientHandoffClient("http://127.0.0.1:9420").resume()).resolves.toMatchObject({ state: "usable" });
    vi.unstubAllGlobals();
  });

  it("resumes a retained cookie before defaulting to direct and keeps invalid authority actionable", async () => {
    const usable = new ActivityClientHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "participant_console_session.v1",
      state: "usable",
      next_action: "continue",
    }), { status: 200 })));
    await expect(resumeRetainedActivityClient({ kind: "direct" }, usable)).resolves.toMatchObject({ kind: "retained" });

    const missing = new ActivityClientHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      code: "participant_session_missing",
      message: "No retained Participant session is available.",
      next_action: "return_to_task_setup",
      retryable: false,
    }), { status: 401 })));
    await expect(resumeRetainedActivityClient({ kind: "direct" }, missing)).resolves.toEqual({ kind: "direct" });

    const invalid = new ActivityClientHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      code: "participant_session_authority_invalid",
      message: "The retained Participant authority is no longer valid.",
      next_action: "return_to_task_setup",
      retryable: false,
    }), { status: 401 })));
    await expect(resumeRetainedActivityClient({ kind: "direct" }, invalid)).resolves.toMatchObject({ kind: "retained_error" });
  });

  it("accepts only the exact browser-safe authorized delivery batch", async () => {
    const safe = {
      pack: { id: "worldstream.counter", version: "0.1.0", digest: `blake3:${"2".repeat(64)}` },
      room_head: {
        room_seq: 7,
        genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
        core_schema_version: "core.v1",
        pack_digest: `blake3:${"2".repeat(64)}`,
        core_state_hash: `blake3:${"3".repeat(64)}`,
        activity_state_hash: `blake3:${"4".repeat(64)}`,
        authoritative_state_hash: `blake3:${"5".repeat(64)}`,
      },
      frame_head: 8,
      delivery: [{ kind: "projection_reset", body: { projection: { action_offers: [] } } }],
    };
    const accepted = new ActivityClientHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify(safe), { status: 200 })));
    await expect(accepted.observe(null)).resolves.toEqual(safe);

    const leaking = structuredClone(safe) as typeof safe & { room_head: typeof safe.room_head & { room_id: string } };
    leaking.room_head.room_id = ROOM_ID;
    const rejected = new ActivityClientHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify(leaking), { status: 200 })));
    await expect(rejected.observe(null)).rejects.toMatchObject({ code: "participant_session_invalid_response" });
  });

  it("requests authorized Replay through the cookie-only Supervisor route", async () => {
    const replay = {
      requested_room_seq: 2,
      room_head: {
        room_seq: 2,
        genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
        core_schema_version: "core.v1",
        pack_digest: `blake3:${"2".repeat(64)}`,
        core_state_hash: `blake3:${"3".repeat(64)}`,
        activity_state_hash: `blake3:${"4".repeat(64)}`,
        authoritative_state_hash: `blake3:${"5".repeat(64)}`,
      },
      projection: { core: {}, activity: { value: 2 }, action_offers: [] },
      projection_hash: `blake3:${"6".repeat(64)}`,
      verification: "verified" as const,
      room_health: "healthy",
      integrity_generation: 1,
    };
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(replay), { status: 200 }));
    const client = new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch);
    await expect(client.replay(2)).resolves.toEqual(replay);
    expect(fetch.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9420/api/v1/participant-console/session:replay");
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ method: "POST", credentials: "include", cache: "no-store" });
    expect(fetch.mock.calls[0]?.[1]?.body).toBe('{"at_room_seq":2}');
    expect(JSON.stringify(fetch.mock.calls)).not.toMatch(/room_id|member_id|wsb1:|prompt|provider_response/);
  });

  it("returns only explicitly accepted or rejected Action receipt records", async () => {
    const submission = {
      action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      based_on_room_seq: 7,
      offer_id: "7:inspect_clue:0",
      schema_digest: `blake3:${"a".repeat(64)}`,
      action_type: "inspect_clue",
      payload: { clue_id: "vault" },
    };
    const acceptedReceipt = { state: "accepted", room_seq: 8 };
    const accepted = new ActivityClientHandoffClient(
      "http://127.0.0.1:9420",
      vi.fn().mockResolvedValue(new Response(JSON.stringify(acceptedReceipt), { status: 200 })),
    );
    await expect(accepted.act(submission)).resolves.toEqual(acceptedReceipt);

    const rejectedReceipt = { state: "rejected", code: "offer_expired" };
    const explicitlyRejected = new ActivityClientHandoffClient(
      "http://127.0.0.1:9420",
      vi.fn().mockResolvedValue(new Response(JSON.stringify(rejectedReceipt), { status: 200 })),
    );
    await expect(explicitlyRejected.act(submission)).resolves.toEqual(rejectedReceipt);

    const primitive = new ActivityClientHandoffClient(
      "http://127.0.0.1:9420",
      vi.fn().mockResolvedValue(new Response(JSON.stringify("accepted"), { status: 200 })),
    );
    await expect(primitive.act(submission)).rejects.toMatchObject({ code: "participant_session_invalid_response" });

    const unknownState = new ActivityClientHandoffClient(
      "http://127.0.0.1:9420",
      vi.fn().mockResolvedValue(new Response(JSON.stringify({ state: "queued" }), { status: 200 })),
    );
    await expect(unknownState.act(submission)).rejects.toMatchObject({ code: "participant_session_invalid_response" });
  });
});
