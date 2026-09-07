import { execFile } from "node:child_process";
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { writeFile } from "node:fs/promises";
import { promisify } from "node:util";

import {
  initialAgentHeistLiveState,
  reduceAgentHeistObservation,
  type AgentHeistLiveState,
  type AgentHeistReadyState,
} from "@worldstream/agent-heist-client/live-adapter";
import { AgentHeistWebMcpBridge } from "@worldstream/agent-heist-client/webmcp";
import {
  ActivityClientHandoffClient,
  HostedLiveSessionController,
  PublicProjectionSessionController,
  type HostedLiveSessionControllerOptions,
  type HostedLiveActionInput,
  type HostedLiveActionReceipt,
  type JsonObject,
} from "@worldstream/client";
import { strict as assert } from "node:assert";
import { test } from "vitest";

import { createDevelopmentPlatformBff, DEVELOPMENT_IDENTITY_MODE } from "./bff.js";
import { HttpHostedBrowserSessionClient } from "./browser-sessions.js";
import { HttpHostedFormationGateway } from "./hosted-formation.js";
import { createSupabaseBffDependencies } from "./supabase.js";

const ACCEPTANCE_MODE = "visible-local-only";
const ACTION_IDS = [
  "01ARZ3NDEKTSV4RRFFQ69G5FNC",
  "01ARZ3NDEKTSV4RRFFQ69G5FND",
  "01ARZ3NDEKTSV4RRFFQ69G5FNE",
  "01ARZ3NDEKTSV4RRFFQ69G5FNF",
  "01ARZ3NDEKTSV4RRFFQ69G5FNG",
] as const;
const executeFile = promisify(execFile);

type JsonRecord = Record<string, unknown>;

test.skipIf(process.env.WORLDSTREAM_LOCAL_ACCEPTANCE !== ACCEPTANCE_MODE)(
  "the hosted local candidate passes the canonical browser-to-result story",
  async () => {
    const productOrigin = required("WORLDSTREAM_ACCEPTANCE_PRODUCT_ORIGIN");
    const gatewayOrigin = required("WORLDSTREAM_HOSTED_GATEWAY_URL");
    const fakeProviderOrigin = required("WORLDSTREAM_ACCEPTANCE_FAKE_PROVIDER_ORIGIN");
    const serviceAuthority = required("WORLDSTREAM_VERCEL_SERVICE_AUTHORITY");
    const supabaseUrl = required("SUPABASE_URL");
    const publishableKey = required("SUPABASE_PUBLISHABLE_KEY");
    const dataSecretKey = required("SUPABASE_DATA_SECRET_KEY");

    const creator = new CookieBrowser(productOrigin, globalThis.fetch);
    await creator.signIn();

    const secondaryDependencies = createSupabaseBffDependencies({
      url: supabaseUrl,
      publishableKey,
      dataSecretKey,
    });
    assert.ok(secondaryDependencies.hostedFormationData);
    const secondaryBff = createDevelopmentPlatformBff(
      {
        canonicalOrigin: productOrigin,
        allowedReturnTargets: ["/", "/join"],
        sessionKey: Buffer.alloc(32, 7),
        oauthKey: Buffer.alloc(32, 9),
        developmentMode: DEVELOPMENT_IDENTITY_MODE,
        deploymentEnvironment: "development",
        identity: {
          authUserId: required("WORLDSTREAM_ACCEPTANCE_AGENT_USER_ID"),
          providerSubject: required("WORLDSTREAM_ACCEPTANCE_AGENT_PROVIDER_SUBJECT"),
          githubLogin: required("WORLDSTREAM_ACCEPTANCE_AGENT_LOGIN"),
          avatarUrl: null,
        },
      },
      secondaryDependencies.dataClient,
      new HttpHostedBrowserSessionClient({
        baseUrl: gatewayOrigin,
        clientOrigin: productOrigin,
        serviceAuthority,
      }),
      {
        data: secondaryDependencies.hostedFormationData,
        gateway: new HttpHostedFormationGateway({
          baseUrl: gatewayOrigin,
          serviceAuthority,
        }),
        hostInstallationId: required("WORLDSTREAM_HOSTED_INSTALLATION_ID"),
      },
      gatewayOrigin,
    );
    const browserAgent = new CookieBrowser(
      productOrigin,
      (input, init) => secondaryBff.fetch(new Request(input, init)),
    );
    await browserAgent.signIn();

    const launch = await creator.mutate("/api/launches", {
      listing_slug: "agent-heist",
      creator_access: "seat",
      creator_seat: "seat-1",
      fill_mode: "house_agents",
      idempotency_key: `hosted_local_canonical_${required("WORLDSTREAM_ACCEPTANCE_NONCE")}`,
    }, [200, 201]);
    const launchId = stringField(launch, "launch_id");
    const invitation = await creator.mutate(
      `/api/launches/${launchId}/seats/seat-3/invitation`,
      {},
      [201],
    );
    await browserAgent.mutate("/api/invitations/claim", {
      invitation_token: stringField(invitation, "invitation_token"),
      participation: "external_agent",
    }, [201]);
    const unauthorizedStart = await browserAgent.mutateResponse(
      `/api/launches/${launchId}/start`,
      {},
    );
    assert.ok(unauthorizedStart.status >= 400);

    const formed = await startLaunch(creator, launchId);
    const run = recordField(formed, "run");
    const runId = stringField(run, "run_id");
    const publicId = stringField(run, "public_id");
    const creatorEntry = firstEntry(run);
    const agentLaunch = await browserAgent.read(`/api/launches/${launchId}`);
    const agentEntry = firstEntry(recordField(agentLaunch, "run"));

    const anonymousEnter = await fetch(`${productOrigin}/api/runs/enter`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        origin: productOrigin,
        "sec-fetch-site": "same-origin",
      },
      body: JSON.stringify({ run_id: runId, entry_selector: creatorEntry.entrySelector }),
    });
    assert.equal(anonymousEnter.status, 403);
    assert.equal((await fetch(`${productOrigin}/api/ws`)).status, 404);

    const creatorHandoff = await enter(creator, runId, creatorEntry.entrySelector);
    const agentHandoff = await enter(browserAgent, runId, agentEntry.entrySelector);
    const creatorHttpFailures: string[] = [];
    const agentHttpFailures: string[] = [];
    const creatorAuthority = new ActivityClientHandoffClient(
      productOrigin,
      traceFailures(creator.fetch, creatorHttpFailures),
      { browserOrigin: productOrigin, csrf: creator.csrf },
    );
    const agentAuthority = new ActivityClientHandoffClient(
      productOrigin,
      traceFailures(browserAgent.fetch, agentHttpFailures),
      { browserOrigin: productOrigin, csrf: browserAgent.csrf },
    );
    const creatorSockets: WebSocket[] = [];
    const agentSockets: WebSocket[] = [];
    const directPush = new DirectPushMeasurement();
    const creatorController = controller(
      gatewayOrigin,
      productOrigin,
      creatorAuthority,
      creatorSockets,
      directPush,
    );
    const agentController = controller(
      gatewayOrigin,
      productOrigin,
      agentAuthority,
      agentSockets,
      directPush,
    );
    const creatorLive = trackHeist(creatorController);
    const agentLive = trackHeist(agentController);
    const [creatorSession, agentSession] = await Promise.all([
      creatorAuthority.redeem(creatorHandoff),
      agentAuthority.redeem(agentHandoff),
    ]);
    assert.equal(creatorSession.state, "usable");
    assert.equal(agentSession.state, "usable");
    const [creatorRevalidated, agentRevalidated] = await Promise.all([
      creatorAuthority.resume(),
      agentAuthority.resume(),
    ]);
    assert.equal(creatorRevalidated.state, "usable");
    assert.equal(agentRevalidated.state, "usable");
    const [creatorStarted, agentStarted] = await Promise.all([
      creatorController.start({ kind: "retained", status: creatorRevalidated }),
      agentController.start({ kind: "retained", status: agentRevalidated }),
    ]);
    assert.equal(
      creatorStarted.status,
      "live",
      `creator browser session did not become live: ${creatorStarted.message ?? "no detail"}; ${creatorHttpFailures.at(-1) ?? "no HTTP failure"}`,
    );
    assert.equal(
      agentStarted.status,
      "live",
      `external browser-agent session did not become live: ${agentStarted.message ?? "no detail"}; ${agentHttpFailures.at(-1) ?? "no HTTP failure"}`,
    );
    await Promise.all([
      waitForHeist(creatorController, creatorLive, (state) => state.authorization.role === "navigator"),
      waitForHeist(agentController, agentLive, (state) => state.authorization.role === "broker"),
    ]);

    const publicRun = await readPublicRun(productOrigin, publicId, "live");
    const live = recordField(publicRun, "live");
    const publicController = new PublicProjectionSessionController({
      streamUrl: stringField(live, "stream_url"),
      webSocketFactory: trackingWebSocketFactory([], productOrigin, directPush),
    });
    const publicLive = trackHeist(publicController);
    await publicController.start({ kind: "direct" });
    await waitForHeist(
      publicController,
      publicLive,
      (state) => state.authorization.accessMode === "spectator",
    );
    await assert.rejects(() => publicController.submitAction({
      actionId: ACTION_IDS[0],
      basedOnRoomSeq: 0,
      actionType: "inspect_clue",
      payload: {},
    }));

    const webMcp = new AgentHeistWebMcpBridge({
      controller: agentController,
      readLiveState: agentLive,
    });
    console.info("Hosted acceptance: all participant and spectator streams are live.");
    await waitForOffer(creatorController, creatorLive, "inspect_clue", 60_000);
    const initialAgentRead = await webMcp.read();
    assert.equal(initialAgentRead.ok, true);
    const waitTask = webMcp.wait({
      after_state_token: stringField(initialAgentRead, "state_token"),
    });
    await submit(
      creatorController,
      creatorLive,
      "inspect_clue",
      { clue_id: "route" },
      ACTION_IDS[0],
    );
    assert.equal((await waitTask).ok, true);

    const disconnectedFrame = agentController.state.lastAcknowledgedFrameSeq ?? 0;
    agentSockets.at(-1)?.close(1000, "acceptance disconnect");
    await agentController.waitFor((state) => state.status === "disconnected", {
      timeoutMs: 15_000,
    });
    await waitForOffer(creatorController, creatorLive, "publish_clue", 60_000);
    const routeClaim = claim(ready(creatorLive()).projection.privateClues, "route");
    await submit(
      creatorController,
      creatorLive,
      "publish_clue",
      { clue_id: "route", claim_code: routeClaim },
      ACTION_IDS[1],
    );
    const reconnected = await agentController.reconnect();
    assert.equal(
      reconnected.status,
      "live",
      `Agent reconnect failed: ${reconnected.message ?? "no detail"}; ${agentHttpFailures.at(-1) ?? "no HTTP failure"}`,
    );
    await agentController.waitFor(
      (state) => state.status === "live" && (state.lastAcknowledgedFrameSeq ?? 0) > disconnectedFrame,
      { timeoutMs: 15_000 },
    );
    console.info("Hosted acceptance: external-agent disconnect and Catch-up passed.");

    // The retained Pack is reactive: proposing a plan emits the endorsement
    // Activation. Waiting for unsolicited House work before this would deadlock.
    const plan = fixturePlanForRoute(routeClaim);
    await submit(
      creatorController,
      creatorLive,
      "propose_plan",
      plan,
      ACTION_IDS[2],
    );
    await waitForHeist(
      creatorController,
      creatorLive,
      (state) => state.projection.plans.length === 1 &&
        (state.projection.plans[0]?.endorsements ?? 0) > 0,
    );
    console.info("Hosted acceptance: the House Agent endorsed the human proposal.");

    const frameBeforeRestart = creatorController.state.lastAcknowledgedFrameSeq ?? 0;
    console.info("Hosted acceptance: restarting the retained Runtime after an acknowledged plan.");
    await restartRetainedRuntime();
    const reentry = await enter(creator, runId, creatorEntry.entrySelector);
    await creatorAuthority.redeem(reentry);
    await Promise.all([
      creatorController.reconnect(),
      agentController.reconnect(),
      publicController.reconnect(),
    ]);
    await Promise.all([
      waitForHeist(creatorController, creatorLive, () => true),
      waitForHeist(agentController, agentLive, () => true),
      waitForHeist(publicController, publicLive, () => true),
    ]);
    assert.ok((creatorController.state.lastAcknowledgedFrameSeq ?? 0) >= frameBeforeRestart);
    console.info("Hosted acceptance: retained Runtime restart and stream re-entry passed.");

    await waitForOffer(agentController, agentLive, "commit_move", 120_000);
    const brokerDecision = await webMcp.read();
    const availableMoves = arrayField(brokerDecision, "available_moves");
    const move = record(availableMoves[0]);
    const planId = ready(agentLive()).projection.plans[0]?.planId;
    assert.ok(planId);
    const brokerCommit = await webMcp.commit({
      action_token: stringField(move, "action_token"),
      plan_id: planId,
      contribute_required_resource: true,
    });
    assert.equal(brokerCommit.status, "accepted");

    await waitForOffer(creatorController, creatorLive, "commit_move", 60_000);
    await submit(
      creatorController,
      creatorLive,
      "commit_move",
      { selected_plan_id: planId, contribute_required_resource: false },
      ACTION_IDS[3],
    );
    await waitForOffer(creatorController, creatorLive, "acknowledge_result", 60_000);
    await submit(
      creatorController,
      creatorLive,
      "acknowledge_result",
      {},
      ACTION_IDS[4],
    );
    await Promise.all([
      waitForHeist(
        creatorController,
        creatorLive,
        (state) => state.projection.phase === "complete",
        60_000,
      ),
      waitForHeist(
        publicController,
        publicLive,
        (state) => state.projection.phase === "complete",
        60_000,
      ),
    ]);

    const terminal = await pollPublicResult(productOrigin, publicId);
    assert.equal(recordField(terminal, "evidence").class, "exhibition_platform_house_agents");
    const recent = await readJsonResponse(
      await fetch(`${productOrigin}/api/results/agent-heist/recent`),
      [200],
    );
    assert.ok(
      arrayField(recent, "results").some(
        (item) => record(item).public_id === publicId,
      ),
    );
    assertNoPrivatePublicFields(terminal);
    assertNoPrivatePublicFields(recent);
    console.info("Hosted acceptance: terminal Replay-verified exhibition is in Recent Results.");

    const providerMetrics = await readJsonResponse(
      await fetch(`${fakeProviderOrigin}/development/metrics`, {
        headers: {
          authorization: `Bearer ${required("WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY")}`,
        },
      }),
      [200],
    );
    const providerCalls = integerField(providerMetrics, "house_completion_count");
    assert.ok(providerCalls >= 1 && providerCalls <= 10);
    const resultPath = required("WORLDSTREAM_ACCEPTANCE_RESULT_PATH");
    await writeFile(resultPath, `${JSON.stringify({
      version: "worldstream_hosted_local_acceptance_result.v1",
      provider_calls: providerCalls,
      maximum_direct_push_seconds: directPush.seconds,
    })}\n`, { flag: "wx", mode: 0o600 });

    creatorController.close();
    agentController.close();
    publicController.close();
  },
  360_000,
);

class CookieBrowser {
  readonly #cookies = new Map<string, string>();
  readonly #origin: string;
  readonly #dispatch: typeof fetch;
  csrf = "";

  constructor(origin: string, dispatch: typeof fetch) {
    this.#origin = origin;
    this.#dispatch = dispatch;
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    const incoming = new Request(input, init);
    const headers = new Headers(incoming.headers);
    const cookie = [...this.#cookies].map(([name, value]) => `${name}=${value}`).join("; ");
    if (cookie !== "") headers.set("cookie", cookie);
    headers.set("origin", this.#origin);
    headers.set("sec-fetch-site", "same-origin");
    const request = new Request(incoming, { headers });
    const response = await this.#dispatch(request);
    for (const value of response.headers.getSetCookie()) {
      const [pair] = value.split(";", 1);
      const separator = pair?.indexOf("=") ?? -1;
      if (pair === undefined || separator < 1) continue;
      const name = pair.slice(0, separator);
      const content = pair.slice(separator + 1);
      if (content === "") this.#cookies.delete(name);
      else this.#cookies.set(name, content);
    }
    return response;
  };

  async signIn(): Promise<void> {
    const response = await this.fetch(`${this.#origin}/api/dev/sign-in`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mode: DEVELOPMENT_IDENTITY_MODE }),
    });
    const body = await readJsonResponse(response, [200]);
    this.csrf = stringField(body, "csrf");
  }

  async read(path: string): Promise<JsonRecord> {
    return readJsonResponse(await this.fetch(`${this.#origin}${path}`), [200]);
  }

  async mutate(
    path: string,
    body: JsonRecord,
    expected: readonly number[] = [200],
  ): Promise<JsonRecord> {
    return readJsonResponse(await this.mutateResponse(path, body), expected);
  }

  mutateResponse(path: string, body: JsonRecord): Promise<Response> {
    return this.fetch(`${this.#origin}${path}`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-worldstream-csrf": this.csrf,
      },
      body: JSON.stringify(body),
    });
  }
}

function traceFailures(dispatch: typeof fetch, failures: string[]): typeof fetch {
  return async (input, init) => {
    const response = await dispatch(input, init);
    if (!response.ok) {
      const requestedUrl = input instanceof Request ? input.url : String(input);
      const path = new URL(response.url || requestedUrl).pathname;
      const body = await response.clone().json().catch(() => null) as JsonRecord | null;
      const code = body?.code ?? (body?.error as JsonRecord | undefined)?.code;
      const safeCode = typeof code === "string" && /^[a-z][a-z0-9_]{0,63}$/u.test(code)
        ? code : "unclassified";
      failures.push(`${path} returned ${response.status}: ${safeCode}`);
    }
    return response;
  };
}

async function startLaunch(browser: CookieBrowser, launchId: string): Promise<JsonRecord> {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    const response = await browser.mutateResponse(`/api/launches/${launchId}/start`, {});
    const body = await readJsonResponse(response, [200, 202]);
    if (body.state === "run_created") return body;
    if (body.state === "failed_pre_genesis") {
      throw new Error("hosted launch failed before Genesis");
    }
    const retry = typeof body.retry_after_seconds === "number"
      ? Math.max(250, Math.min(body.retry_after_seconds * 1_000, 5_000))
      : 500;
    await delay(retry);
  }
  throw new Error("hosted launch did not create a Run");
}

async function enter(
  browser: CookieBrowser,
  runId: string,
  entrySelector: string,
): Promise<string> {
  const response = await browser.mutate("/api/runs/enter", {
    run_id: runId,
    entry_selector: entrySelector,
  }, [201]);
  const clientUrl = new URL(stringField(response, "client_url"));
  const handoff = clientUrl.hash.match(/^#handoff=(wsh1:[0-9a-f]{64})$/u)?.[1];
  assert.ok(handoff);
  return handoff;
}

function controller(
  gatewayOrigin: string,
  browserOrigin: string,
  authority: ActivityClientHandoffClient,
  sockets: WebSocket[],
  directPush: DirectPushMeasurement,
): HostedLiveSessionController {
  const stream = new URL(gatewayOrigin);
  stream.protocol = stream.protocol === "https:" ? "wss:" : "ws:";
  stream.pathname = "/v1/hosted/browser-stream";
  return new HostedLiveSessionController({
    streamUrl: stream.toString(),
    authority,
    webSocketFactory: trackingWebSocketFactory(sockets, browserOrigin, directPush),
  });
}

function trackingWebSocketFactory(
  sockets: WebSocket[],
  browserOrigin: string,
  directPush: DirectPushMeasurement,
): NonNullable<HostedLiveSessionControllerOptions["webSocketFactory"]> {
  return (url, protocols) => {
    const observePush = directPush.connection();
    // Node's WebSocket has no ambient page origin. Its WebSocketInit extension
    // supplies the exact Origin that a real browser adds automatically.
    const socket = new WebSocket(url, {
      protocols: [...protocols],
      headers: { Origin: browserOrigin },
    } as unknown as string[]);
    socket.addEventListener("message", (event) => {
      if (typeof event.data !== "string") return;
      try {
        const message = JSON.parse(event.data) as { type?: unknown; body?: { code?: unknown } };
        if (message.type === "projection.reset" || message.type === "observation.deliver") {
          observePush(Date.now());
        }
        if (message.type === "error" && typeof message.body?.code === "string") {
          console.info(`Hosted acceptance: realtime error code ${message.body.code}`);
        }
      } catch {
        // The controller performs protocol validation. Never print raw frames.
      }
    });
    sockets.push(socket);
    return socket;
  };
}

class DirectPushMeasurement {
  #maximumMs = 0;

  get seconds(): number {
    return Math.floor(this.#maximumMs / 1_000);
  }

  connection(): (now: number) => void {
    let firstPushAt: number | null = null;
    return (now) => {
      firstPushAt ??= now;
      this.#maximumMs = Math.max(this.#maximumMs, now - firstPushAt);
    };
  }
}

test("direct push evidence measures delivered state on one connection, not setup time or downtime", () => {
  const measurement = new DirectPushMeasurement();
  const first = measurement.connection();
  first(50_000);
  first(52_500);
  assert.equal(measurement.seconds, 2);
  const reconnected = measurement.connection();
  reconnected(300_000);
  assert.equal(measurement.seconds, 2);
  reconnected(304_250);
  assert.equal(measurement.seconds, 4);
});

function trackHeist(
  controller: Pick<HostedLiveSessionController, "state" | "subscribe">,
): () => AgentHeistLiveState {
  let state = initialAgentHeistLiveState();
  controller.subscribe((snapshot) => {
    if (snapshot.deliveryBatch !== null) {
      state = reduceAgentHeistObservation(state, snapshot.deliveryBatch);
    }
  });
  return () => state;
}

async function waitForHeist(
  controller: Pick<HostedLiveSessionController, "waitFor">,
  read: () => AgentHeistLiveState,
  predicate: (state: AgentHeistReadyState) => boolean,
  timeoutMs = 30_000,
): Promise<AgentHeistReadyState> {
  const current = read();
  if (current.kind === "ready" && predicate(current)) return current;
  await controller.waitFor(() => {
    const state = read();
    return state.kind === "ready" && predicate(state);
  }, { timeoutMs: Math.min(timeoutMs, 60_000) });
  const result = ready(read());
  if (!predicate(result)) throw new Error("Agent Heist state predicate did not hold");
  return result;
}

async function waitForOffer(
  controller: Pick<HostedLiveSessionController, "state" | "waitFor">,
  read: () => AgentHeistLiveState,
  actionType: string,
  timeoutMs: number,
  minimumRoomSeq = 0,
): Promise<AgentHeistReadyState> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return await waitForHeist(
        controller,
        read,
        (state) => controller.state.canAct && state.roomSequence >= minimumRoomSeq &&
          state.offers.some((offer) => offer.actionType === actionType),
        Math.min(60_000, deadline - Date.now()),
      );
    } catch {
      if (Date.now() >= deadline) break;
    }
  }
  const state = read();
  const detail = state.kind === "ready"
    ? `phase=${state.projection.phase},role=${state.authorization.role},offers=${state.offers.map(({ actionType: offered }) => offered).join(",") || "none"}`
    : `projection=${state.kind}`;
  throw new Error(
    `Agent Heist never offered ${actionType} (session=${controller.state.status},${detail})`,
  );
}

async function submit(
  controller: HostedLiveSessionController,
  read: () => AgentHeistLiveState,
  actionType: string,
  payload: JsonObject,
  actionId: string,
): Promise<void> {
  await submitWithFreshState(controller, async (nextActionId, minimumRoomSeq) => {
    const state = await waitForOffer(controller, read, actionType, 60_000, minimumRoomSeq);
    return { actionId: nextActionId, basedOnRoomSeq: state.roomSequence, actionType, payload };
  }, actionId);
}

async function submitWithFreshState(
  controller: Pick<HostedLiveSessionController, "submitAction" | "reconnect">,
  prepare: (actionId: string, minimumRoomSeq: number) => Promise<HostedLiveActionInput>,
  actionId: string,
): Promise<void> {
  let minimumRoomSeq = 0;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    // A rejection consumes its Action ID. Never retry an uncertain submission
    // or revise a command without the Runtime's explicit permission.
    const nextActionId = attempt === 0 ? actionId : `0${createHash("sha256")
      .update(`${actionId}:${attempt}`).digest("hex").slice(0, 25).toUpperCase()}`;
    const input = await prepare(nextActionId, minimumRoomSeq);
    assert.ok(input.basedOnRoomSeq >= minimumRoomSeq, "Action preparation did not synchronize to the rejected Head");
    const receipt = await controller.submitAction(input);
    if (receipt.state === "accepted") return;
    const mayRevise = receipt.code === "stale_room_state" &&
      receipt.maySubmitRevisedAction && !receipt.retryableWithSameActionId;
    assert.ok(mayRevise, "Action rejected without safe stale-state revision permission");
    minimumRoomSeq = Math.max(minimumRoomSeq, receipt.currentRoomSeq);
    if (attempt < 4) await controller.reconnect();
  }
  throw new Error("Action still stale after five freshly synchronized attempts");
}

test("a concurrent explicit stale rejection uses a new Action ID and a freshly synchronized Head", async () => {
  const inputs: HostedLiveActionInput[] = [];
  const events: string[] = [];
  const minimums: number[] = [];
  await submitWithFreshState({
    async submitAction(input): Promise<HostedLiveActionReceipt> {
      inputs.push(input);
      events.push("submit");
      if (inputs.length === 1) return staleReceipt(input.actionId);
      // Only the accepted discriminator is used by this helper.
      return { state: "accepted" } as HostedLiveActionReceipt;
    },
    async reconnect() {
      events.push("reconnect");
      return {} as HostedLiveSessionController["state"];
    },
  }, async (actionId, minimumRoomSeq) => {
    events.push("prepare");
    minimums.push(minimumRoomSeq);
    return { actionId, basedOnRoomSeq: minimumRoomSeq || 7, actionType: "commit_move", payload: {} };
  }, ACTION_IDS[3]);
  assert.deepEqual(events, ["prepare", "submit", "reconnect", "prepare", "submit"]);
  assert.deepEqual(minimums, [0, 8]);
  assert.equal(inputs[0]?.actionId, ACTION_IDS[3]);
  assert.notEqual(inputs[1]?.actionId, inputs[0]?.actionId);
  assert.match(inputs[1]?.actionId ?? "", /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/u);
  assert.equal(inputs[1]?.basedOnRoomSeq, 8);
});

function staleReceipt(actionId: string): Extract<HostedLiveActionReceipt, { state: "rejected" }> {
  return {
    state: "rejected", actionId, code: "stale_room_state", currentRoomSeq: 8,
    maySubmitRevisedAction: true, retryableWithSameActionId: false, duplicate: false,
  };
}

test("Action revision is bounded and never retries ambiguous or non-revisable failures", async () => {
  const scenarios = ["always-stale", "ambiguous", "policy", "no-revision", "same-id-retry"];
  for (const scenario of scenarios) {
    const ids: string[] = [];
    let reconnects = 0;
    await assert.rejects(() => submitWithFreshState({
      async submitAction(input) {
        ids.push(input.actionId);
        if (scenario === "ambiguous") throw new Error("uncertain submission");
        return {
          ...staleReceipt(input.actionId), state: "rejected" as const,
          code: scenario === "policy" ? "policy_denied" : "stale_room_state",
          maySubmitRevisedAction: scenario !== "no-revision",
          retryableWithSameActionId: scenario === "same-id-retry",
        };
      },
      async reconnect() {
        reconnects += 1;
        return {} as HostedLiveSessionController["state"];
      },
    }, async (actionId, minimumRoomSeq) => ({
      actionId, basedOnRoomSeq: minimumRoomSeq, actionType: "commit_move", payload: {},
    }), ACTION_IDS[3]));
    assert.equal(ids.length, scenario === "always-stale" ? 5 : 1, scenario);
    assert.equal(reconnects, scenario === "always-stale" ? 4 : 0, scenario);
    assert.equal(new Set(ids).size, ids.length);
    for (const id of ids.slice(1)) assert.equal(ACTION_IDS.includes(id as typeof ACTION_IDS[number]), false);
  }
});

async function restartRetainedRuntime(): Promise<void> {
  const { stdout } = await executeFile(required("WORLDSTREAM_ACCEPTANCE_CTL"), [
    "--config",
    required("WORLDSTREAM_ACCEPTANCE_CONFIG"),
    "server",
    "restart",
    "--state-dir",
    required("WORLDSTREAM_ACCEPTANCE_STATE_DIR"),
    "--controller",
    required("WORLDSTREAM_ACCEPTANCE_CONTROLLER"),
    "--json",
  ], {
    cwd: required("WORLDSTREAM_ACCEPTANCE_REPOSITORY_ROOT"),
    env: process.env,
    timeout: 60_000,
  });
  const response = record(JSON.parse(stdout));
  assert.equal(response.status, "complete", "Runtime restart is still pending or failed");
  const server = recordField(response, "server");
  assert.equal(server.runtime, "ready");
  const operation = recordField(server, "operation");
  assert.equal(operation.stage, "complete", "managed process restoration did not complete");
  assert.deepEqual(operation.restore_remaining, []);
}

async function readPublicRun(
  origin: string,
  publicId: string,
  expectedState: "live" | "result",
): Promise<JsonRecord> {
  const value = await readJsonResponse(await fetch(`${origin}/api/runs/${publicId}`), [200]);
  assert.equal(value.state, expectedState);
  return value;
}

async function pollPublicResult(origin: string, publicId: string): Promise<JsonRecord> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const response = await fetch(`${origin}/api/runs/${publicId}`);
    if (response.status === 200) {
      const body = await readJsonResponse(response, [200]);
      if (body.state === "result") return body;
    }
    await delay(500);
  }
  throw new Error("Replay-verified public result did not become available");
}

function firstEntry(run: JsonRecord): { entrySelector: string } {
  const entry = record(arrayField(run, "entries")[0]);
  return { entrySelector: stringField(entry, "entry_selector") };
}

function fixturePlanForRoute(routeClaim: string): JsonObject {
  const route = routeClaim.replace(/^route_/u, "");
  // This is a reviewed deterministic test strategy, not a model-quality score.
  const completion = new Map<string, readonly [string, string, string]>([
    ["canal", ["late", "disguise", "van"]],
    ["service", ["early", "thermal_key", "boat"]],
    ["roof", ["middle", "jammer", "motorbike"]],
  ]).get(route);
  if (completion === undefined) throw new Error("Heist route has no reviewed fixture plan");
  return {
    route,
    entry_window: completion[0],
    required_tool: completion[1],
    extraction: completion[2],
  };
}

function claim(
  clues: readonly { readonly clueId: string; readonly claimCode: string }[],
  clueId: string,
): string {
  const value = claimOrNull(clues, clueId);
  if (value === null) throw new Error(`missing ${clueId} claim`);
  return value;
}

function claimOrNull(
  clues: readonly { readonly clueId: string; readonly claimCode: string }[],
  clueId: string,
): string | null {
  return clues.find((clue) => clue.clueId === clueId)?.claimCode ?? null;
}

function ready(state: AgentHeistLiveState): AgentHeistReadyState {
  if (state.kind !== "ready") throw new Error("Agent Heist live state is not ready");
  return state;
}

function assertNoPrivatePublicFields(value: unknown): void {
  const forbidden = new Set([
    "account_id", "activity_run_id", "canonical_payload", "entry_selector",
    "final_reveal", "handoff", "invitation_token", "membership_id", "principal_id",
    "provider_response", "replay", "room_id", "service_scope_digest",
  ]);
  const visit = (candidate: unknown): void => {
    if (Array.isArray(candidate)) {
      candidate.forEach(visit);
    } else if (candidate !== null && typeof candidate === "object") {
      for (const [key, child] of Object.entries(candidate)) {
        assert.equal(forbidden.has(key), false, `public response exposed ${key}`);
        visit(child);
      }
    }
  };
  visit(value);
}

async function readJsonResponse(
  response: Response,
  expected: readonly number[],
): Promise<JsonRecord> {
  const text = await response.text();
  assert.ok(expected.includes(response.status), `${response.status}: ${text.slice(0, 300)}`);
  return record(JSON.parse(text));
}

function record(value: unknown): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("expected JSON object");
  }
  return value as JsonRecord;
}

function recordField(value: JsonRecord, field: string): JsonRecord {
  return record(value[field]);
}

function arrayField(value: JsonRecord, field: string): unknown[] {
  const selected = value[field];
  if (!Array.isArray(selected)) throw new Error(`${field} must be an array`);
  return selected;
}

function stringField(value: JsonRecord, field: string): string {
  const selected = value[field];
  if (typeof selected !== "string" || selected.length === 0) {
    throw new Error(`${field} must be a string`);
  }
  return selected;
}

function integerField(value: JsonRecord, field: string): number {
  const selected = value[field];
  if (!Number.isSafeInteger(selected)) throw new Error(`${field} must be an integer`);
  return selected as number;
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0) throw new Error(`${name}_is_required`);
  return value;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
