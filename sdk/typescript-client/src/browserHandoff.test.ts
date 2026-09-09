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

function browser(
  hash: string,
  origin = "http://127.0.0.1:5173",
): BrowserNavigationTarget & { replaced: string[] } {
  const replaced: string[] = [];
  const target = {
    location: {
      origin,
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
  it("acknowledges a validated browser delivery before returning it", async () => {
    const batch = {
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
    const acknowledgement = `wsa1:${"12".repeat(32)}`;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input).endsWith("session:observe")) {
        return new Response(JSON.stringify(batch), {
          status: 200,
          headers: { "X-WorldStream-Delivery-Acknowledgement": acknowledgement },
        });
      }
      expect(input).toBe("http://127.0.0.1:9420/api/v1/participant-console/session:acknowledge");
      expect(init).toMatchObject({ method: "POST", credentials: "include", cache: "no-store" });
      expect(JSON.parse(String(init?.body))).toEqual({ acknowledgement, frame_head: 8 });
      expect(new Headers(init?.headers).has("authorization")).toBe(false);
      return new Response(JSON.stringify({
        version: "participant_console_session.v1", state: "usable", next_action: "continue",
      }), { status: 200 });
    });
    const client = new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch);
    await expect(client.observe(null)).resolves.toEqual(batch);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

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
    const legacyFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(safe), { status: 200 }));
    const accepted = new ActivityClientHandoffClient("http://127.0.0.1:9420", legacyFetch);
    await expect(accepted.observe(null)).resolves.toEqual(safe);
    expect(legacyFetch).toHaveBeenCalledTimes(1);

    const leaking = structuredClone(safe) as typeof safe & { room_head: typeof safe.room_head & { room_id: string } };
    leaking.room_head.room_id = ROOM_ID;
    const invalidFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(leaking), {
      status: 200, headers: { "X-WorldStream-Delivery-Acknowledgement": `wsa1:${"34".repeat(32)}` },
    }));
    const rejected = new ActivityClientHandoffClient("http://127.0.0.1:9420", invalidFetch);
    await expect(rejected.observe(null)).rejects.toMatchObject({ code: "participant_session_invalid_response" });
    expect(invalidFetch).toHaveBeenCalledTimes(1);
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

  it("retries an exact idempotent Action after a bounded transient upstream failure", async () => {
    vi.useFakeTimers();
    try {
      const submission = {
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
        based_on_room_seq: 7,
        offer_id: "7:inspect_clue:0",
        schema_digest: `blake3:${"a".repeat(64)}`,
        action_type: "inspect_clue",
        payload: { clue_id: "vault" },
      };
      const unavailable = () => new Response(JSON.stringify({
        code: "participant_session_unavailable",
        message: "The participant client cannot reach the Room service safely.",
        next_action: "reconnect",
        retryable: true,
      }), { status: 502 });
      const fetch = vi.fn()
        .mockResolvedValueOnce(unavailable())
        .mockResolvedValueOnce(unavailable())
        .mockResolvedValueOnce(Response.json({ state: "accepted", room_seq: 8 }));
      const client = new ActivityClientHandoffClient("http://127.0.0.1:9420", fetch);

      const receipt = client.act(submission);
      await vi.runAllTimersAsync();
      await expect(receipt).resolves.toEqual({ state: "accepted", room_seq: 8 });
      expect(fetch).toHaveBeenCalledTimes(3);
      expect(fetch.mock.calls.map((call) => call[1]?.body)).toEqual([
        JSON.stringify(submission),
        JSON.stringify(submission),
        JSON.stringify(submission),
      ]);

      const exhaustedFetch = vi.fn(() => Promise.resolve(unavailable()));
      const exhausted = new ActivityClientHandoffClient("http://127.0.0.1:9420", exhaustedFetch);
      const failure = expect(exhausted.act(submission)).rejects.toMatchObject({
        code: "participant_session_unavailable",
        nextAction: "reconnect",
        retryable: true,
      });
      await vi.runAllTimersAsync();
      await failure;
      expect(exhaustedFetch).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("redeems hosted handoffs only through the exact same-origin BFF", async () => {
    const csrf = "c".repeat(43);
    const target = browser(`#handoff=${HANDOFF}`, "https://arena.example");
    const startup = selectActivityClientStartup(target);
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(target.location.hash).toBe("");
      expect(String(input)).toBe(
        "https://arena.example/api/v1/participant-console/handoffs:redeem",
      );
      expect(init).toMatchObject({
        method: "POST",
        credentials: "include",
        cache: "no-store",
        body: "{}",
      });
      const headers = new Headers(init?.headers);
      expect(headers.get("content-type")).toBe("application/json");
      expect(headers.get("x-worldstream-csrf")).toBe(csrf);
      expect(headers.get("x-worldstream-participant-handoff")).toBe(HANDOFF);
      expect(headers.has("authorization")).toBe(false);
      return Response.json({
        version: "participant_console_session.v1",
        state: "usable",
        next_action: "continue",
      });
    });
    const client = new ActivityClientHandoffClient(
      "https://arena.example",
      fetch,
      { browserOrigin: "https://arena.example", csrf },
    );
    await expect(
      client.redeem(startup.kind === "handoff" ? startup.handoff : ""),
    ).resolves.toMatchObject({ state: "usable" });
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("uses CSRF-protected hosted logout and rejects cross-origin configuration", async () => {
    const csrf = "d".repeat(43);
    expect(() => new ActivityClientHandoffClient(
      "https://gateway.example",
      vi.fn(),
      { browserOrigin: "https://arena.example", csrf },
    )).toThrow(/exact browser origin/u);
    expect(() => new ActivityClientHandoffClient(
      "http://arena.example",
      vi.fn(),
      { browserOrigin: "http://arena.example", csrf },
    )).toThrow(/exact browser origin/u);

    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe(
        "https://arena.example/api/v1/participant-console/session:logout",
      );
      expect(init).toMatchObject({ method: "POST", credentials: "include", body: "{}" });
      const headers = new Headers(init?.headers);
      expect(headers.get("x-worldstream-csrf")).toBe(csrf);
      return Response.json({
        version: "participant_console_logout.v1",
        logged_out: true,
      });
    });
    await expect(new ActivityClientHandoffClient(
      "https://arena.example",
      fetch,
      { browserOrigin: "https://arena.example", csrf },
    ).logout()).resolves.toBeUndefined();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("mints a hosted stream ticket with only the retained cookie and Cursor", async () => {
    const csrf = "e".repeat(43);
    const ticket = `wst1:${"ef".repeat(32)}`;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe(
        "https://arena.example/api/v1/participant-console/session:stream-ticket",
      );
      expect(init).toMatchObject({
        method: "POST",
        credentials: "include",
        cache: "no-store",
        body: '{"after_frame_seq":37}',
      });
      const headers = new Headers(init?.headers);
      expect(headers.get("x-worldstream-csrf")).toBe(csrf);
      expect(headers.has("authorization")).toBe(false);
      return Response.json({
        version: "participant_console_stream_ticket.v1",
        ticket,
        expires_in_ms: 15_000,
      });
    });
    const client = new ActivityClientHandoffClient(
      "https://arena.example",
      fetch,
      { browserOrigin: "https://arena.example", csrf },
    );

    await expect(client.issueStreamTicket(37)).resolves.toEqual({
      version: "participant_console_stream_ticket.v1",
      ticket,
      expiresInMs: 15_000,
    });
    expect(JSON.stringify(fetch.mock.calls)).not.toMatch(
      /room_id|member_id|membership_id|principal_id|wsb1:/u,
    );
  });

  it("rejects widened, long-lived, or non-hosted stream admission", async () => {
    const csrf = "f".repeat(43);
    const widened = new ActivityClientHandoffClient(
      "https://arena.example",
      vi.fn().mockResolvedValue(Response.json({
        version: "participant_console_stream_ticket.v1",
        ticket: `wst1:${"ab".repeat(32)}`,
        expires_in_ms: 15_001,
        room_id: ROOM_ID,
      })),
      { browserOrigin: "https://arena.example", csrf },
    );
    await expect(widened.issueStreamTicket(null)).rejects.toMatchObject({
      code: "participant_session_invalid_response",
    });

    const local = new ActivityClientHandoffClient(
      "http://127.0.0.1:9420",
      vi.fn(),
    );
    await expect(local.issueStreamTicket(null)).rejects.toMatchObject({
      code: "participant_stream_ticket_unavailable",
    });
  });
});
