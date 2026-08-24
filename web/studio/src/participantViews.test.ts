import { describe, expect, it, vi } from "vitest";
import { openParticipantView } from "./participantViews";

describe("Participant View handoff", () => {
  it("opens only the validated one-use fragment URL without exposing it to the DOM", async () => {
    const consoleUrl = `http://127.0.0.1:5174/#handoff=wsh1:${"a".repeat(64)}`;
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "participant_handoff.v1", console_url: consoleUrl,
    }), { status: 201 }));
    const opener = { open: vi.fn() };
    await expect(openParticipantView("setup-alpha", "human-1", fetcher, opener)).resolves.toBe(true);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/participant-console/handoffs", expect.objectContaining({
      method: "POST", body: JSON.stringify({ draft_id: "setup-alpha", seat_id: "human-1" }),
    }));
    expect(opener.open).toHaveBeenCalledWith(consoleUrl, "_blank", "noopener,noreferrer");
  });

  it("fails closed for a non-loopback or query-carried token", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      version: "participant_handoff.v1",
      console_url: `https://example.com/?handoff=wsh1:${"a".repeat(64)}`,
    }), { status: 201 }));
    const opener = { open: vi.fn() };
    await expect(openParticipantView("setup-alpha", "human-1", fetcher, opener)).resolves.toBe(false);
    expect(opener.open).not.toHaveBeenCalled();
  });
});
