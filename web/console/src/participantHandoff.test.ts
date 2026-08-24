import { describe, expect, it, vi } from "vitest";

import {
  ParticipantHandoffClient,
  consumeParticipantHandoffFragment,
  resumeRetainedParticipantConsole,
  selectParticipantConsoleStartup,
  type BrowserNavigationTarget,
} from "./participantHandoff";

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

describe("Participant Console opaque handoff", () => {
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

    const startup = selectParticipantConsoleStartup(target);
    expect(startup).toEqual({ kind: "handoff", handoff: HANDOFF });
    expect(target.replaced).toEqual(["/"]);
    const status = await new ParticipantHandoffClient("http://127.0.0.1:9420", fetch).redeem(startup.kind === "handoff" ? startup.handoff : "");
    expect(status.state).toBe("usable");
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(ROOM_ID);
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(MEMBER_ID);
    expect(JSON.stringify(fetch.mock.calls)).not.toContain(BEARER);
  });

  it("scrubs malformed handoff material and never converts it into direct setup", () => {
    const malformed = browser(`#handoff=${HANDOFF}&room_id=${ROOM_ID}`);
    expect(consumeParticipantHandoffFragment(malformed)).toEqual({ kind: "invalid_handoff" });
    expect(malformed.location.hash).toBe("");

    const direct = browser("");
    expect(selectParticipantConsoleStartup(direct)).toEqual({ kind: "direct" });
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
    const client = new ParticipantHandoffClient("http://127.0.0.1:9420", fetch);
    await expect(client.resume()).resolves.toMatchObject({ state: "disconnected" });
    expect(fetch.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9420/api/v1/participant-console/session");
    expect(fetch.mock.calls[0]?.[1]).toMatchObject({ method: "GET", credentials: "include", cache: "no-store" });
    await expect(client.resume()).rejects.toMatchObject({ code: "participant_session_invalid_response", nextAction: "return_to_task_setup" });
  });

  it("resumes a retained cookie before defaulting to direct and keeps invalid authority actionable", async () => {
    const usable = new ParticipantHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "participant_console_session.v1",
      state: "usable",
      next_action: "continue",
    }), { status: 200 })));
    await expect(resumeRetainedParticipantConsole({ kind: "direct" }, usable)).resolves.toMatchObject({ kind: "retained" });

    const missing = new ParticipantHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      code: "participant_session_missing",
      message: "No retained Participant session is available.",
      next_action: "return_to_task_setup",
      retryable: false,
    }), { status: 401 })));
    await expect(resumeRetainedParticipantConsole({ kind: "direct" }, missing)).resolves.toEqual({ kind: "direct" });

    const invalid = new ParticipantHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify({
      code: "participant_session_authority_invalid",
      message: "The retained Participant authority is no longer valid.",
      next_action: "return_to_task_setup",
      retryable: false,
    }), { status: 401 })));
    await expect(resumeRetainedParticipantConsole({ kind: "direct" }, invalid)).resolves.toMatchObject({ kind: "retained_error" });
  });

  it("accepts only the exact browser-safe observation projection", async () => {
    const safe = {
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
    const accepted = new ParticipantHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify(safe), { status: 200 })));
    await expect(accepted.observe(null)).resolves.toEqual(safe);

    const leaking = structuredClone(safe) as typeof safe & { room_head: typeof safe.room_head & { room_id: string } };
    leaking.room_head.room_id = ROOM_ID;
    const rejected = new ParticipantHandoffClient("http://127.0.0.1:9420", vi.fn().mockResolvedValue(new Response(JSON.stringify(leaking), { status: 200 })));
    await expect(rejected.observe(null)).rejects.toMatchObject({ code: "participant_session_invalid_response" });
  });
});
