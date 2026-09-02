import { describe, expect, it, vi } from "vitest";
import { openParticipantClient } from "./participantViews";

describe("Activity Client handoff", () => {
  it("opens an opaque loopback client URL without interpreting its path", async () => {
    const clientUrl = `http://127.0.0.1:5173/third-party-client/#handoff=wsh1:${"a".repeat(64)}`;
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "activity_client_handoff.v1", state: "ready", client_url: clientUrl,
    }), { status: 201 }));
    const opener = { open: vi.fn() };

    await expect(openParticipantClient("setup-alpha", "human-1", null, fetcher, opener))
      .resolves.toEqual({ state: "opened" });

    expect(fetcher).toHaveBeenCalledWith("/api/v1/participant-console/handoffs", expect.objectContaining({
      method: "POST", body: JSON.stringify({ draft_id: "setup-alpha", seat_id: "human-1" }),
    }));
    expect(opener.open).toHaveBeenCalledWith(clientUrl, "_blank", "noopener,noreferrer");
  });

  it("returns generic approved candidates without receiving their launch URLs", async () => {
    const candidates = [
      {
        candidate_id: "approved-client-a",
        deployment_id: "local-client-host-a",
        client_id: "example.activity-client",
        release_digest: `sha256:${"b".repeat(64)}`,
        surface_id: "participant-web",
        trust_level: "externally_trusted",
      },
      {
        candidate_id: "approved-client-b",
        deployment_id: "local-client-host-b",
        client_id: "example.alternate-client",
        release_digest: `sha256:${"c".repeat(64)}`,
        surface_id: "participant-web",
        trust_level: "verified",
      },
    ] as const;
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "activity_client_handoff.v1", state: "selection_required", candidates,
    }), { status: 200 }));
    const opener = { open: vi.fn() };

    await expect(openParticipantClient("setup-alpha", "human-1", null, fetcher, opener))
      .resolves.toEqual({ state: "selection_required", candidates });
    expect(opener.open).not.toHaveBeenCalled();
  });

  it("sends only the selected opaque candidate identity on the follow-up", async () => {
    const clientUrl = `http://localhost:5173/custom/#handoff=wsh1:${"c".repeat(64)}`;
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "activity_client_handoff.v1", state: "ready", client_url: clientUrl,
    }), { status: 201 }));
    const opener = { open: vi.fn() };

    await expect(openParticipantClient(
      "setup-alpha",
      "human-1",
      "approved-client-a",
      fetcher,
      opener,
    )).resolves.toEqual({ state: "opened" });

    expect(fetcher).toHaveBeenCalledWith("/api/v1/participant-console/handoffs", expect.objectContaining({
      body: JSON.stringify({
        draft_id: "setup-alpha",
        seat_id: "human-1",
        candidate_id: "approved-client-a",
      }),
    }));
  });

  it("fails closed for a non-loopback or query-carried token", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "activity_client_handoff.v1",
      state: "ready",
      client_url: `https://example.com/?handoff=wsh1:${"a".repeat(64)}`,
    }), { status: 201 }));
    const opener = { open: vi.fn() };

    await expect(openParticipantClient("setup-alpha", "human-1", null, fetcher, opener))
      .resolves.toEqual({ state: "failed" });
    expect(opener.open).not.toHaveBeenCalled();
  });
});
