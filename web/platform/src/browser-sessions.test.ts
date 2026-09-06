import { strict as assert } from "node:assert";
import { describe, it, vi } from "vitest";

import {
  HostedBrowserSessionMissingError,
  HostedBrowserSessionRejectedError,
  HostedBrowserSessionUnavailableError,
  HttpHostedBrowserSessionClient,
  type OwnedRunMembershipCorrespondence,
} from "./browser-sessions.js";

const ACCOUNT = "10000000-0000-4000-8000-000000000001";
const HANDOFF = `wsh1:${"a".repeat(64)}`;
const SESSION = `wss1:${"b".repeat(64)}`;
const TICKET = `wst1:${"c".repeat(64)}`;
const SERVICE_AUTHORITY = `service-${"s".repeat(40)}`;

function correspondence(): OwnedRunMembershipCorrespondence {
  return {
    runId: "20000000-0000-4000-8000-000000000001",
    listingRevisionDigest: `blake3:${"1".repeat(64)}`,
    hostInstallationId: "hosted-preview-1",
    roomSetupOperationId: "hosted-launch-01",
    roomId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    pack: {
      id: "worldstream.agent-heist",
      version: "0.2.0",
      digest: `blake3:${"2".repeat(64)}`,
    },
    clientReleaseDigest: `blake3:${"3".repeat(64)}`,
    clientSurfaceId: "participant",
    accessMode: "participant",
    purpose: "participant",
    seatId: "navigator",
    role: "navigator",
    principalKind: "human",
    principalId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
    membershipId: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
  };
}

function client(fetchImplementation: typeof fetch): HttpHostedBrowserSessionClient {
  return new HttpHostedBrowserSessionClient({
    baseUrl: "https://gateway.example/",
    clientOrigin: "https://arena.example",
    serviceAuthority: SERVICE_AUTHORITY,
    fetchImplementation,
  });
}

describe("hosted Browser Activity Session service client", () => {
  it("uses only fixed authenticated routes and exact canonical contracts", async () => {
    const observations: Array<{ path: string; body: Record<string, unknown> }> = [];
    const fetchImplementation = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const body = JSON.parse(await request.text()) as Record<string, unknown>;
      observations.push({ path: new URL(request.url).pathname, body });
      assert.equal(request.method, "POST");
      assert.equal(request.headers.get("authorization"), `Bearer ${SERVICE_AUTHORITY}`);
      assert.equal(request.headers.get("content-type"), "application/json");
      assert.equal(request.cache, "no-store");
      assert.equal(request.redirect, "error");
      switch (new URL(request.url).pathname) {
        case "/v1/hosted/browser-handoffs/issue":
          return Response.json({
            schema: "worldstream/hosted-browser-handoff-response/v1",
            client_url: `https://arena.example/clients/heist/#handoff=${HANDOFF}`,
          });
        case "/v1/hosted/browser-sessions/admit":
          return Response.json({
            schema: "worldstream/hosted-browser-handoff-redeem-response/v1",
            session: SESSION,
          }, { status: 201 });
        case "/v1/hosted/browser-sessions/status":
          return Response.json({
            schema: "worldstream/hosted-browser-session-status/v1",
            state: "usable",
          });
        case "/v1/hosted/browser-sessions/stream-ticket":
          return Response.json({
            schema: "worldstream/hosted-browser-stream-ticket-response/v1",
            ticket: TICKET,
            expires_in_ms: 15_000,
          }, { status: 201 });
        case "/v1/hosted/browser-sessions/logout":
          return Response.json({
            schema: "worldstream/hosted-browser-session-logout/v1",
            logged_out: true,
          });
        default:
          return Response.json({}, { status: 404 });
      }
    });
    const hosted = client(fetchImplementation);
    assert.deepEqual(await hosted.issueHandoff(ACCOUNT, correspondence()), {
      clientUrl: `https://arena.example/clients/heist/#handoff=${HANDOFF}`,
    });
    assert.equal(await hosted.redeemHandoff(ACCOUNT, HANDOFF, null), SESSION);
    assert.deepEqual(await hosted.sessionStatus(SESSION), { state: "usable" });
    assert.deepEqual(await hosted.issueStreamTicket(SESSION, 37), {
      ticket: TICKET,
      expiresInMs: 15_000,
    });
    await hosted.logoutSession(SESSION);

    assert.equal(fetchImplementation.mock.calls.length, 5);
    assert.deepEqual(observations.map(({ path }) => path), [
      "/v1/hosted/browser-handoffs/issue",
      "/v1/hosted/browser-sessions/admit",
      "/v1/hosted/browser-sessions/status",
      "/v1/hosted/browser-sessions/stream-ticket",
      "/v1/hosted/browser-sessions/logout",
    ]);
    assert.deepEqual(observations[1]?.body, {
      schema: "worldstream/hosted-browser-handoff-redeem-request/v1",
      platform_account_id: ACCOUNT,
      handoff: HANDOFF,
      prior_session: null,
    });
    assert.deepEqual(observations[2]?.body, {
      schema: "worldstream/hosted-browser-session-request/v1",
      session: SESSION,
    });
    assert.deepEqual(observations[3]?.body, {
      schema: "worldstream/hosted-browser-stream-ticket-request/v1",
      session: SESSION,
      after_frame_seq: 37,
    });
  });

  it("fails closed on changed origins, widened memberships, and response fields", async () => {
    assert.throws(
      () => new HttpHostedBrowserSessionClient({
        baseUrl: "http://gateway.example/",
        clientOrigin: "https://arena.example",
        serviceAuthority: SERVICE_AUTHORITY,
      }),
      HostedBrowserSessionRejectedError,
    );
    const widened = correspondence() as OwnedRunMembershipCorrespondence & {
      purpose: "participant";
    };
    Object.assign(widened, { accessMode: "spectator" });
    await assert.rejects(
      client(vi.fn()).issueHandoff(ACCOUNT, widened),
      HostedBrowserSessionRejectedError,
    );

    const malformed = client(vi.fn().mockResolvedValue(Response.json({
      schema: "worldstream/hosted-browser-handoff-redeem-response/v1",
      session: SESSION,
      membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX",
    })));
    await assert.rejects(
      malformed.redeemHandoff(ACCOUNT, HANDOFF, null),
      HostedBrowserSessionUnavailableError,
    );
  });

  it("never retries an uncertain redemption response", async () => {
    const unavailableFetch = vi.fn().mockResolvedValue(Response.json(
      { error: { code: "browser_session_unavailable" } },
      { status: 503 },
    ));
    await assert.rejects(
      client(unavailableFetch).redeemHandoff(ACCOUNT, HANDOFF, null),
      HostedBrowserSessionUnavailableError,
    );
    assert.equal(unavailableFetch.mock.calls.length, 1);

    const missingFetch = vi.fn().mockResolvedValue(Response.json(
      { error: { code: "browser_session_missing" } },
      { status: 401 },
    ));
    await assert.rejects(
      client(missingFetch).redeemHandoff(ACCOUNT, HANDOFF, null),
      HostedBrowserSessionMissingError,
    );
    assert.equal(missingFetch.mock.calls.length, 1);
  });
});
