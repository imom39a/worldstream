import { execFile } from "node:child_process";
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
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
  type HostedLiveSessionSnapshot,
  type HostedLiveActionInput,
  type HostedLiveActionReceipt,
  type JsonObject,
} from "@worldstream/client";
import { strict as assert } from "node:assert";
import { test } from "vitest";

import { createDevelopmentPlatformBff, DEVELOPMENT_IDENTITY_MODE } from "./bff.js";
import { HttpHostedBrowserSessionClient } from "./browser-sessions.js";
import { HttpHostedFormationGateway } from "./hosted-formation.js";
import { createHostedResultReconciler } from "./reconciliation-service.js";
import {
  reconcilePrestartHouseRunnerRetirementCandidates,
  reconcileTerminalHouseRunnerRetirementCandidates,
  type ResultReconciliationData,
  type ResultReconcilerDependencies,
} from "./result-reconciliation.js";
import { createSupabaseBffDependencies } from "./supabase.js";

const renderedBrowserJourney = await import(
  new URL("../../../scripts/hosted-rendered-browser-journey.mjs", import.meta.url).href
);

const ACCEPTANCE_MODE = "visible-local-only";
const HOUSE_ALLOWANCE_LEDGER_SCHEMA = "worldstream/house-allowance-ledger@1";
const MAX_HOUSE_ALLOWANCE_LEDGER_BYTES = 2 * 1024 * 1024;
const ACTION_IDS = [
  "01ARZ3NDEKTSV4RRFFQ69G5FNC",
  "01ARZ3NDEKTSV4RRFFQ69G5FND",
  "01ARZ3NDEKTSV4RRFFQ69G5FNE",
  "01ARZ3NDEKTSV4RRFFQ69G5FNF",
  "01ARZ3NDEKTSV4RRFFQ69G5FNG",
] as const;
const LAUNCH_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const PUBLIC_ID_PATTERN = /^[0-9a-f]{32}$/u;
const executeFile = promisify(execFile);

type JsonRecord = Record<string, unknown>;

type HouseAllowanceObservation = Readonly<{
  completedAttempts: number;
  consumedAttempts: number;
  consumedInputUnits: number;
  consumedOutputUnits: number;
}>;

test.skipIf(process.env.WORLDSTREAM_LOCAL_ACCEPTANCE !== ACCEPTANCE_MODE)(
  "the hosted local candidate passes the canonical headless HTTP/WebSocket-to-result story",
  () => withSessionCleanup(async (register) => {
    const productOrigin = required("WORLDSTREAM_ACCEPTANCE_PRODUCT_ORIGIN");
    const gatewayOrigin = required("WORLDSTREAM_HOSTED_GATEWAY_URL");
    const browserStreamOrigin = localBrowserStreamOrigin(
      gatewayOrigin,
      required("WORLDSTREAM_LOCAL_BROWSER_STREAM_URL"),
    );
    const fakeProviderOrigin = required("WORLDSTREAM_ACCEPTANCE_FAKE_PROVIDER_ORIGIN");
    const serviceAuthority = required("WORLDSTREAM_VERCEL_SERVICE_AUTHORITY");
    const supabaseUrl = required("SUPABASE_URL");
    const publishableKey = required("SUPABASE_PUBLISHABLE_KEY");
    const dataSecretKey = required("SUPABASE_DATA_SECRET_KEY");
    const houseRetirementReconciler = createHostedResultReconciler({
      supabaseUrl,
      dataSecretKey,
      hostedGatewayUrl: gatewayOrigin,
      serviceAuthority,
    });

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
      browserStreamOrigin,
    );
    const browserAgent = new CookieBrowser(
      productOrigin,
      (input, init) => secondaryBff.fetch(new Request(input, init)),
    );
    await browserAgent.signIn();
    const directPush = new DirectPushMeasurement();

    // A local setup may be interrupted before or after Genesis. Resolve at
    // most one exact owner-authorized setup before starting the canonical
    // matrix. We never reset, directly release, or create a replacement for
    // retained state.
    const retainedGames = await creator.read("/api/my-games");
    const retainedSetupId = retainedHostedSetupLaunchId(retainedGames);
    if (retainedSetupId !== null) {
      await preflightRetainedHostedSetup(creator, retainedSetupId);
    }
    const retainedLaunchId = retainedHostedLiveLaunchId(
      retainedSetupId === null ? retainedGames : await creator.read("/api/my-games"),
    );
    if (retainedLaunchId !== null) {
      await completeRetainedRun({
        creator,
        browserAgent,
        productOrigin,
        browserStreamOrigin,
        directPush,
        register,
        launchId: retainedLaunchId,
      });
    }

    const providerBefore = await readProviderMetrics(fakeProviderOrigin);
    const acceptanceStateDirectory = required("WORLDSTREAM_ACCEPTANCE_STATE_DIR");
    const allowanceBefore = await readHouseAllowanceObservation(acceptanceStateDirectory);

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
    const creatorEntrySelector = creatorEntry.entrySelector;
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
    const creatorController = register(controller(
      browserStreamOrigin,
      productOrigin,
      creatorAuthority,
      creatorSockets,
      directPush,
    ));
    const agentController = register(controller(
      browserStreamOrigin,
      productOrigin,
      agentAuthority,
      agentSockets,
      directPush,
    ));
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
    const publicController = register(new PublicProjectionSessionController({
      streamUrl: stringField(live, "stream_url"),
      webSocketFactory: trackingWebSocketFactory([], productOrigin, directPush),
    }));
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
    // Each authenticated participant explicitly rejoins the same retained seat.
    // A cached Browser Activity Session is not post-restart admission evidence.
    const reentries = await Promise.allSettled([
      enter(creator, runId, creatorEntry.entrySelector, "creator"),
      enter(browserAgent, runId, agentEntry.entrySelector, "external_agent"),
    ]);
    const failedRoles = reentries.flatMap((entry, index) => entry.status === "rejected"
      ? [index === 0 ? "creator" : "external_agent"] : []);
    if (failedRoles.length > 0) {
      throw new Error(`Post-restart Run entry failed for ${failedRoles.join(",")}; see [DEBUG-reentry-ad71] status-only evidence.`);
    }
    const creatorReentry = (reentries[0] as PromiseFulfilledResult<string>).value;
    const agentReentry = (reentries[1] as PromiseFulfilledResult<string>).value;
    const [creatorReadmitted, agentReadmitted] = await Promise.all([
      creatorAuthority.redeem(creatorReentry),
      agentAuthority.redeem(agentReentry),
    ]);
    assert.equal(creatorReadmitted.state, "usable");
    assert.equal(agentReadmitted.state, "usable");
    await Promise.all([
      reconnectForAcceptance(creatorController, "creator", () => creatorHttpFailures.at(-1)),
      reconnectForAcceptance(agentController, "external_agent", () => agentHttpFailures.at(-1)),
      reconnectForAcceptance(publicController, "public_spectator"),
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

    const firstProviderMetrics = await readProviderMetrics(fakeProviderOrigin);
    const firstProviderCalls = firstProviderMetrics.house_completion_count;
    assert.ok(firstProviderCalls > providerBefore.house_completion_count);
    const allowanceAfterFirst = await readHouseAllowanceObservation(acceptanceStateDirectory);
    assert.ok(allowanceConsumptionAdvanced(allowanceBefore, allowanceAfterFirst));
    const firstHouseCapacityReleased = await reconcileAndObserveExactHouseRetirement({
      reconciler: houseRetirementReconciler,
      runId,
      lane: "terminal",
    });
    const firstHistory = await creator.read("/api/my-games");
    assertVerifiedMyGamesResult(firstHistory, launchId, publicId);

    // Match 2 deliberately uses the rendered Activity Client against the
    // same retained Runtime, database, House allowances, and qualification
    // identities. A new Run is the only new authority.
    const creatorAccount = await secondaryDependencies.dataClient.resolveGithubAccount({
      authUserId: required("WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID"),
      providerSubject: required("WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT"),
    });
    assert.ok(creatorAccount);
    const secondHistoryBefore = await creator.read("/api/my-games");
    const secondProviderBefore = await readProviderMetrics(fakeProviderOrigin);
    const rendered = await renderedBrowserJourney.runHostedRenderedBrowserJourney({
      productOrigin,
      formationTimeoutMs: 240_000,
    });
    assert.equal(rendered.outcome, "passed");
    assert.equal(rendered.completed, true);
    const renderedRunId = renderedBrowserJourney.renderedJourneyRunId(rendered);
    const renderedEntrySelector = renderedBrowserJourney.renderedJourneyEntrySelector(rendered);
    const renderedOwnedRun = await secondaryDependencies.hostedFormationData.readOwnedRun(
      creatorAccount.accountId,
      renderedRunId,
    );
    assert.ok(renderedOwnedRun);
    const renderedPublicId = renderedOwnedRun.publicId;
    if (typeof renderedPublicId !== "string") throw new Error("rendered Run has no public identity");
    assert.match(renderedPublicId, PUBLIC_ID_PATTERN);
    const secondHistory = await creator.read("/api/my-games");
    const secondLaunchId = exactNewVerifiedResultLaunchId(
      secondHistoryBefore,
      secondHistory,
      renderedPublicId,
    );
    const secondProviderMetrics = await readProviderMetrics(fakeProviderOrigin);
    const secondProviderCalls = secondProviderMetrics.house_completion_count;
    const secondProviderBeforeCalls = secondProviderBefore.house_completion_count;
    assert.ok(secondProviderCalls > secondProviderBeforeCalls);
    const allowanceAfterSecond = await readHouseAllowanceObservation(acceptanceStateDirectory);
    assert.ok(allowanceConsumptionAdvanced(allowanceAfterFirst, allowanceAfterSecond));
    const secondHouseCapacityReleased = await reconcileAndObserveExactHouseRetirement({
      reconciler: houseRetirementReconciler,
      runId: renderedRunId,
      lane: "terminal",
    });

    // Match 3 uses the people-only formation. No client submits an Action; the
    // Pack's persisted timers produce its valid failed outcome and the normal
    // result lane retires the Run.
    const thirdHistoryBefore = await creator.read("/api/my-games");
    const third = await runPeopleOnlyNoActionMatch({
      creator,
      browserAgent,
      productOrigin,
      browserStreamOrigin,
      directPush,
      register,
      matchNumber: 3,
    });
    const thirdHistory = await creator.read("/api/my-games");
    const thirdHistoryMatch = assertVerifiedMyGamesResult(thirdHistory, third.launchId, third.publicId);
    assert.equal(exactNewLaunchId(thirdHistoryBefore, thirdHistory), third.launchId);
    const readRoomIdentity = async (runId: string, entrySelector: string): Promise<string> => {
      const membership = await secondaryDependencies.dataClient.resolveOwnedRunMembership({
        accountId: creatorAccount.accountId,
        runId,
        entrySelector,
      });
      assert.ok(membership);
      return membership.roomId;
    };
    const runIdentities = [runId, renderedRunId, third.runId];
    assert.equal(new Set(runIdentities).size, 3, "three matches must create distinct Runs");
    const roomIdentities = await Promise.all([
      readRoomIdentity(runId, creatorEntrySelector),
      readRoomIdentity(renderedRunId, renderedEntrySelector),
      readRoomIdentity(third.runId, third.entrySelector),
    ]);
    assert.equal(new Set(roomIdentities).size, 3, "three matches must create distinct Rooms");
    const thirdProviderMetrics = await readProviderMetrics(fakeProviderOrigin);
    const thirdProviderCalls = thirdProviderMetrics.house_completion_count;
    assert.equal(thirdProviderCalls, secondProviderCalls);
    const peopleOnlyHasNoHouseRetirement = await waitForExactHouseRetirementClearance(
      houseRetirementReconciler.data,
      third.runId,
      "terminal",
    );
    const finalHistory = await creator.read("/api/my-games");
    const finalFirstHistoryMatch = assertVerifiedMyGamesResult(finalHistory, launchId, publicId);
    const finalSecondHistoryMatch = assertVerifiedMyGamesResult(
      finalHistory,
      secondLaunchId,
      renderedPublicId,
    );
    const finalThirdHistoryMatch = assertVerifiedMyGamesResult(
      finalHistory,
      third.launchId,
      third.publicId,
    );
    // Count only the exact qualified results above. This is deliberately not
    // an aggregate query over My Games: unrelated retained results cannot
    // inflate the acceptance claim, and verified_result is the API's exact
    // terminal marker.
    const retainedHistoryCount = [
      finalFirstHistoryMatch,
      finalSecondHistoryMatch,
      finalThirdHistoryMatch,
    ].filter((item) => item.state === "verified_result").length;
    assert.equal(retainedHistoryCount, 3);
    assert.equal(finalFirstHistoryMatch.launch_id, launchId);
    assert.equal(finalSecondHistoryMatch.launch_id, secondLaunchId);
    assert.equal(finalThirdHistoryMatch.launch_id, third.launchId);

    // A fourth launch proves active capacity was released after the failed
    // people-only Run. Cancel this probe after proving collection so the
    // dedicated qualification identity remains reusable on the next run.
    const fresh = await creator.mutate("/api/launches", {
      listing_slug: "agent-heist",
      creator_access: "seat",
      creator_seat: "seat-1",
      fill_mode: "people_only",
      idempotency_key: `hosted_local_fresh_${required("WORLDSTREAM_ACCEPTANCE_NONCE")}`,
    }, [201, 200]);
    assert.equal(fresh.state, "collecting");
    const freshCancellation = await creator.mutate(
      `/api/launches/${stringField(fresh, "launch_id")}/cancel`,
      {},
      [200],
    );
    assert.equal(freshCancellation.version, "hosted_launch_cancelled.v1");
    assert.equal(freshCancellation.cancelled, true);
    assert.equal(
      (await creator.read(`/api/launches/${stringField(fresh, "launch_id")}`)).state,
      "cancelled",
    );
    // This is an extra repeat-admission check. Each Match's capacity evidence
    // below comes from its own exact retirement lane, not this later probe.
    const finalHouseCapacityProbe = await formAndAbandonHouseCapacityProbe({
      creator,
      browserAgent,
      matchNumber: 4,
    });
    const finalHouseProbeCapacityReleased = await reconcileAndObserveExactHouseRetirement({
      reconciler: houseRetirementReconciler,
      runId: finalHouseCapacityProbe.runId,
      lane: "prestart",
    });
    assert.equal(finalHouseCapacityProbe.formed, true);
    assert.equal(finalHouseProbeCapacityReleased, true);
    const consumedAllowanceObserved = allowanceConsumptionAdvanced(allowanceBefore, allowanceAfterFirst) &&
      allowanceConsumptionAdvanced(allowanceAfterFirst, allowanceAfterSecond);
    const resultPath = required("WORLDSTREAM_ACCEPTANCE_RESULT_PATH");
    await writeFile(resultPath, `${JSON.stringify({
      version: "worldstream_hosted_local_acceptance_result.v2",
      provider_calls: thirdProviderCalls - providerBefore.house_completion_count,
      maximum_direct_push_seconds: directPush.seconds,
      matches: [
        {
          match: 1,
          mode: "house_backed",
          outcome: "success",
          provider_call_delta: firstProviderCalls - providerBefore.house_completion_count,
          capacity_released: firstHouseCapacityReleased,
          history_retained: false,
          disconnect_and_catch_up: true,
          restart_and_reentry: true,
          no_actions: false,
        },
        {
          match: 2,
          mode: "house_backed",
          outcome: "success",
          provider_call_delta: secondProviderCalls - secondProviderBeforeCalls,
          capacity_released: secondHouseCapacityReleased,
          history_retained: finalSecondHistoryMatch.launch_id === secondLaunchId &&
            finalSecondHistoryMatch.result_public_id === renderedPublicId,
          disconnect_and_catch_up: false,
          restart_and_reentry: false,
          no_actions: false,
        },
        {
          match: 3,
          mode: "people_only",
          outcome: "failure",
          provider_call_delta: thirdProviderCalls - secondProviderCalls,
          capacity_released: peopleOnlyHasNoHouseRetirement &&
            thirdProviderCalls === secondProviderCalls,
          history_retained: thirdHistoryMatch.launch_id === third.launchId &&
            thirdHistoryMatch.result_public_id === third.publicId,
          disconnect_and_catch_up: false,
          restart_and_reentry: false,
          no_actions: true,
        },
      ],
      retained_history_count: retainedHistoryCount,
      consumed_allowance_observed: consumedAllowanceObserved,
      fresh_setup: fresh.state === "collecting",
      retained_upgrade: rendered.completed === true &&
        finalSecondHistoryMatch.launch_id === secondLaunchId &&
        finalSecondHistoryMatch.result_public_id === renderedPublicId,
      ordinary_restart: true,
      populated_recovery: "deferred_not_verified",
      rendered_client: rendered,
    })}\n`, { flag: "wx", mode: 0o600 });

  }),
  900_000,
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
  diagnosticRole?: "creator" | "external_agent",
): Promise<string> {
  const diagnostic = (status: number | null, category: string) => {
    if (diagnosticRole !== undefined) console.info(`[DEBUG-reentry-ad71] ${JSON.stringify({
      actor: diagnosticRole, stage: "run_enter", status, category,
    })}`);
  };
  let response: Response;
  try {
    response = await browser.mutateResponse("/api/runs/enter", {
      run_id: runId,
      entry_selector: entrySelector,
    });
  } catch (error) {
    diagnostic(null, "fetch_rejected");
    throw error;
  }
  diagnostic(response.status, response.status === 201 ? "http_success" : "http_failure");
  if (diagnosticRole !== undefined && response.status !== 201) {
    // Do not reflect an error body that could contain arbitrary upstream data.
    await response.body?.cancel();
    throw new Error("post_restart_run_entry_http_failure");
  }
  try {
    const value = await readJsonResponse(response, [201]);
    const clientUrl = new URL(stringField(value, "client_url"));
    const handoff = clientUrl.hash.match(/^#handoff=(wsh1:[0-9a-f]{64})$/u)?.[1];
    assert.ok(handoff);
    return handoff;
  } catch (error) {
    diagnostic(response.status, "invalid_entry_response");
    throw error;
  }
}

function controller(
  browserStreamOrigin: string,
  browserOrigin: string,
  authority: ActivityClientHandoffClient,
  sockets: WebSocket[],
  directPush: DirectPushMeasurement,
): HostedLiveSessionController {
  const stream = new URL(browserStreamOrigin);
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

async function readProviderMetrics(origin: string): Promise<{
  readonly house_completion_count: number;
}> {
  const metrics = await readJsonResponse(
    await fetch(`${origin}/development/metrics`, {
      headers: { authorization: `Bearer ${required("WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY")}` },
    }),
    [200],
  );
  return Object.freeze({
    house_completion_count: integerField(metrics, "house_completion_count"),
  });
}

function houseAllowanceLedgerPath(stateDirectory: string): string {
  return join(
    stateDirectory,
    "hosted-house-runners",
    "units",
    "allowance",
    "allowances.json",
  );
}

async function readHouseAllowanceObservation(
  stateDirectory: string,
): Promise<HouseAllowanceObservation> {
  let bytes: Buffer;
  try {
    bytes = await readFile(houseAllowanceLedgerPath(stateDirectory));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return emptyHouseAllowanceObservation();
    throw error;
  }
  if (bytes.byteLength > MAX_HOUSE_ALLOWANCE_LEDGER_BYTES) {
    throw new Error("House allowance ledger exceeds its retained byte bound");
  }
  return observeHouseAllowanceLedger(JSON.parse(bytes.toString("utf8")));
}

function emptyHouseAllowanceObservation(): HouseAllowanceObservation {
  return Object.freeze({
    completedAttempts: 0,
    consumedAttempts: 0,
    consumedInputUnits: 0,
    consumedOutputUnits: 0,
  });
}

function observeHouseAllowanceLedger(value: unknown): HouseAllowanceObservation {
  const ledger = record(value);
  if (ledger.schema !== HOUSE_ALLOWANCE_LEDGER_SCHEMA) {
    throw new Error("House allowance ledger schema is invalid");
  }
  const assignments = recordField(ledger, "assignments");
  const assignmentEntries = Object.entries(assignments);
  if (assignmentEntries.length > 256) throw new Error("House allowance ledger assignments exceed bounds");
  let completedAttempts = 0;
  let consumedAttempts = 0;
  let consumedInputUnits = 0;
  let consumedOutputUnits = 0;
  for (const [, assignmentValue] of assignmentEntries) {
    const assignment = record(assignmentValue);
    const assignmentInput = nonNegativeIntegerField(assignment, "consumed_input_units");
    const assignmentOutput = nonNegativeIntegerField(assignment, "consumed_output_units");
    const attempts = recordField(assignment, "attempts");
    const attemptEntries = Object.entries(attempts);
    if (attemptEntries.length > 10) throw new Error("House allowance attempts exceed bounds");
    for (const [, attemptValue] of attemptEntries) {
      const attempt = record(attemptValue);
      const state = stringField(attempt, "state");
      if (![
        "reserved",
        "completed",
        "ambiguous",
        "provider_failed",
      ].includes(state)) {
        throw new Error("House allowance attempt state is invalid");
      }
      if (state === "completed") completedAttempts += 1;
      if (state !== "reserved") consumedAttempts += 1;
    }
    consumedInputUnits += assignmentInput;
    consumedOutputUnits += assignmentOutput;
  }
  return Object.freeze({
    completedAttempts,
    consumedAttempts,
    consumedInputUnits,
    consumedOutputUnits,
  });
}

function allowanceConsumptionAdvanced(
  before: HouseAllowanceObservation,
  after: HouseAllowanceObservation,
): boolean {
  return after.completedAttempts > before.completedAttempts &&
    after.consumedAttempts > before.consumedAttempts &&
    after.consumedInputUnits > before.consumedInputUnits &&
    after.consumedOutputUnits > before.consumedOutputUnits;
}

function matchActionId(matchNumber: number, actionNumber: number): string {
  return `0${createHash("sha256")
    .update(`worldstream-hosted-acceptance:${matchNumber}:${actionNumber}`)
    .digest("hex")
    .slice(0, 25)
    .toUpperCase()}`;
}

async function runHouseBackedMatch(options: {
  creator: CookieBrowser;
  browserAgent: CookieBrowser;
  directPush: DirectPushMeasurement;
  register: <T extends { close(): void }>(controller: T) => T;
  productOrigin: string;
  gatewayOrigin: string;
  browserStreamOrigin: string;
  serviceAuthority: string;
  matchNumber: number;
}): Promise<{ runId: string; publicId: string; entrySelector: string }> {
  const {
    creator,
    browserAgent,
    directPush,
    register,
    productOrigin,
    gatewayOrigin,
    browserStreamOrigin,
    serviceAuthority,
    matchNumber,
  } = options;
  const launch = await creator.mutate("/api/launches", {
    listing_slug: "agent-heist",
    creator_access: "seat",
    creator_seat: "seat-1",
    fill_mode: "house_agents",
    idempotency_key: `hosted_local_match_${matchNumber}_${required("WORLDSTREAM_ACCEPTANCE_NONCE")}`,
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
  const formed = await startLaunch(creator, launchId);
  const run = recordField(formed, "run");
  const runId = stringField(run, "run_id");
  const publicId = stringField(run, "public_id");
  const creatorEntry = firstEntry(run);
  const agentLaunch = await browserAgent.read(`/api/launches/${launchId}`);
  const agentEntry = firstEntry(recordField(agentLaunch, "run"));
  const creatorHandoff = await enter(creator, runId, creatorEntry.entrySelector);
  const agentHandoff = await enter(browserAgent, runId, agentEntry.entrySelector);
  const creatorAuthority = new ActivityClientHandoffClient(
    productOrigin,
    creator.fetch,
    { browserOrigin: productOrigin, csrf: creator.csrf },
  );
  const agentAuthority = new ActivityClientHandoffClient(
    productOrigin,
    browserAgent.fetch,
    { browserOrigin: productOrigin, csrf: browserAgent.csrf },
  );
  const creatorController = register(controller(
    browserStreamOrigin,
    productOrigin,
    creatorAuthority,
    [],
    directPush,
  ));
  const agentController = register(controller(
    browserStreamOrigin,
    productOrigin,
    agentAuthority,
    [],
    directPush,
  ));
  const creatorLive = trackHeist(creatorController);
  const agentLive = trackHeist(agentController);
  const [creatorSession, agentSession] = await Promise.all([
    creatorAuthority.redeem(creatorHandoff),
    agentAuthority.redeem(agentHandoff),
  ]);
  const [creatorRevalidated, agentRevalidated] = await Promise.all([
    creatorAuthority.resume(),
    agentAuthority.resume(),
  ]);
  assert.equal(creatorSession.state, "usable");
  assert.equal(agentSession.state, "usable");
  const [creatorStarted, agentStarted] = await Promise.all([
    creatorController.start({ kind: "retained", status: creatorRevalidated }),
    agentController.start({ kind: "retained", status: agentRevalidated }),
  ]);
  assert.equal(creatorStarted.status, "live");
  assert.equal(agentStarted.status, "live");
  await Promise.all([
    waitForHeist(creatorController, creatorLive, (state) => state.authorization.role === "navigator"),
    waitForHeist(agentController, agentLive, (state) => state.authorization.role === "broker"),
  ]);
  const webMcp = new AgentHeistWebMcpBridge({
    controller: agentController,
    readLiveState: agentLive,
  });
  await waitForOffer(creatorController, creatorLive, "inspect_clue", 60_000);
  await submit(
    creatorController,
    creatorLive,
    "inspect_clue",
    { clue_id: "route" },
    matchActionId(matchNumber, 0),
  );
  await waitForOffer(creatorController, creatorLive, "publish_clue", 60_000);
  const routeClaim = claim(ready(creatorLive()).projection.privateClues, "route");
  await submit(
    creatorController,
    creatorLive,
    "publish_clue",
    { clue_id: "route", claim_code: routeClaim },
    matchActionId(matchNumber, 1),
  );
  const plan = fixturePlanForRoute(routeClaim);
  await submit(
    creatorController,
    creatorLive,
    "propose_plan",
    plan,
    matchActionId(matchNumber, 2),
  );
  await waitForHeist(
    creatorController,
    creatorLive,
    (state) => state.projection.plans.length === 1 &&
      (state.projection.plans[0]?.endorsements ?? 0) > 0,
  );
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
    matchActionId(matchNumber, 3),
  );
  await waitForOffer(creatorController, creatorLive, "acknowledge_result", 60_000);
  await submit(
    creatorController,
    creatorLive,
    "acknowledge_result",
    {},
    matchActionId(matchNumber, 4),
  );
  const terminal = await pollPublicResult(productOrigin, publicId);
  assert.equal(recordField(terminal, "evidence").class, "exhibition_platform_house_agents");
  assertNoPrivatePublicFields(terminal);
  return { runId, publicId, entrySelector: creatorEntry.entrySelector };
}

async function runPeopleOnlyNoActionMatch(options: {
  creator: CookieBrowser;
  browserAgent: CookieBrowser;
  productOrigin: string;
  browserStreamOrigin: string;
  directPush: DirectPushMeasurement;
  register: <T extends { close(): void }>(controller: T) => T;
  matchNumber: number;
}): Promise<{ launchId: string; runId: string; publicId: string; entrySelector: string }> {
  const { creator, browserAgent, productOrigin, matchNumber } = options;
  const launch = await creator.mutate("/api/launches", {
    listing_slug: "agent-heist",
    creator_access: "seat",
    creator_seat: "seat-1",
    fill_mode: "people_only",
    idempotency_key: `hosted_local_match_${matchNumber}_${required("WORLDSTREAM_ACCEPTANCE_NONCE")}`,
  }, [200, 201]);
  const launchId = stringField(launch, "launch_id");
  const invitation = await creator.mutate(
    `/api/launches/${launchId}/seats/seat-2/invitation`,
    {},
    [201],
  );
  await browserAgent.mutate("/api/invitations/claim", {
    invitation_token: stringField(invitation, "invitation_token"),
    participation: "external_agent",
  }, [201]);
  const formed = await startLaunch(creator, launchId);
  const run = recordField(formed, "run");
  const runId = stringField(run, "run_id");
  const publicId = stringField(run, "public_id");
  const entrySelector = await synchronizePeopleOnlyRun({
    ...options,
    launchId,
    run,
  });
  const terminal = await pollPublicResult(productOrigin, publicId, 240_000);
  assert.equal(terminal.state, "result");
  const summary = recordField(terminal, "result");
  assert.equal(summary.schema, "worldstream/result-summary/v1");
  assert.equal(summary.outcome, "failure");
  assert.equal(summary.selected_plan_id, null);
  assert.equal(summary.score, 0);
  assert.equal(summary.reason, "no_strict_majority");
  assertNoPrivatePublicFields(terminal);
  return { launchId, runId, publicId, entrySelector };
}

async function synchronizePeopleOnlyRun(options: {
  creator: CookieBrowser;
  browserAgent: CookieBrowser;
  productOrigin: string;
  browserStreamOrigin: string;
  directPush: DirectPushMeasurement;
  register: <T extends { close(): void }>(controller: T) => T;
  launchId: string;
  run: JsonRecord;
}): Promise<string> {
  const {
    creator,
    browserAgent,
    productOrigin,
    browserStreamOrigin,
    directPush,
    register,
    launchId,
    run,
  } = options;
  const runId = stringField(run, "run_id");
  const creatorEntry = firstEntry(run);
  const agentLaunch = await browserAgent.read(`/api/launches/${launchId}`);
  const agentRun = recordField(agentLaunch, "run");
  assert.equal(stringField(agentRun, "run_id"), runId);
  const agentEntry = firstEntry(agentRun);
  const [creatorHandoff, agentHandoff] = await Promise.all([
    enter(creator, runId, creatorEntry.entrySelector),
    enter(browserAgent, runId, agentEntry.entrySelector),
  ]);
  const creatorAuthority = new ActivityClientHandoffClient(
    productOrigin,
    creator.fetch,
    { browserOrigin: productOrigin, csrf: creator.csrf },
  );
  const agentAuthority = new ActivityClientHandoffClient(
    productOrigin,
    browserAgent.fetch,
    { browserOrigin: productOrigin, csrf: browserAgent.csrf },
  );
  const creatorController = register(controller(
    browserStreamOrigin,
    productOrigin,
    creatorAuthority,
    [],
    directPush,
  ));
  const agentController = register(controller(
    browserStreamOrigin,
    productOrigin,
    agentAuthority,
    [],
    directPush,
  ));
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
  const [creatorStarted, agentStarted] = await Promise.all([
    creatorController.start({ kind: "retained", status: creatorRevalidated }),
    agentController.start({ kind: "retained", status: agentRevalidated }),
  ]);
  assert.equal(creatorStarted.status, "live");
  assert.equal(agentStarted.status, "live");
  await Promise.all([
    waitForHeist(
      creatorController,
      creatorLive,
      (state) => state.authorization.role === "navigator" && state.projection.phase !== "lobby",
      60_000,
    ),
    waitForHeist(
      agentController,
      agentLive,
      (state) => state.authorization.role === "insider" && state.projection.phase !== "lobby",
      60_000,
    ),
  ]);
  return creatorEntry.entrySelector;
}

/**
 * Proves that a fresh House-backed formation can reserve the released slot,
 * then removes only that unstarted probe through the evidence-bound cancel path.
 */
async function formAndAbandonHouseCapacityProbe(options: {
  creator: CookieBrowser;
  browserAgent: CookieBrowser;
  matchNumber: number;
}): Promise<{ readonly runId: string; readonly formed: boolean }> {
  const { creator, browserAgent, matchNumber } = options;
  const launch = await creator.mutate("/api/launches", {
    listing_slug: "agent-heist",
    creator_access: "seat",
    creator_seat: "seat-1",
    fill_mode: "house_agents",
    idempotency_key: `hosted_local_capacity_probe_${matchNumber}_${required("WORLDSTREAM_ACCEPTANCE_NONCE")}`,
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
  const formed = await startLaunch(creator, launchId);
  assert.equal(formed.state, "run_created");
  const runId = stringField(recordField(formed, "run"), "run_id");
  const abandonment = await creator.mutate(`/api/launches/${launchId}/cancel`, {}, [200]);
  assert.equal(abandonment.version, "hosted_launch_abandoned_prestart.v1");
  assert.equal(abandonment.abandoned, true);
  const terminalLaunch = await waitForLaunchState(creator, launchId, "abandoned_prestart");
  const formedAndAbandoned = formed.state === "run_created" &&
    abandonment.version === "hosted_launch_abandoned_prestart.v1" &&
    abandonment.abandoned === true &&
    terminalLaunch.state === "abandoned_prestart";
  assert.equal(formedAndAbandoned, true);
  return Object.freeze({ runId, formed: formedAndAbandoned });
}

type HouseRetirementLane = "terminal" | "prestart";

/**
 * Replays the bounded generic cleanup lane, then observes only the exact
 * Run's owner-only retirement records. A clear lane means its immutable Host
 * receipt was retained; it does not borrow capacity evidence from another Run.
 */
async function reconcileAndObserveExactHouseRetirement(input: {
  readonly reconciler: ResultReconcilerDependencies;
  readonly runId: string;
  readonly lane: HouseRetirementLane;
}): Promise<boolean> {
  if (input.lane === "terminal") {
    await reconcileTerminalHouseRunnerRetirementCandidates(input.reconciler, 10);
  } else {
    await reconcilePrestartHouseRunnerRetirementCandidates(input.reconciler, 10);
  }
  return waitForExactHouseRetirementClearance(
    input.reconciler.data,
    input.runId,
    input.lane,
  );
}

async function waitForExactHouseRetirementClearance(
  data: Pick<
    ResultReconciliationData,
    "readTerminalHouseRunnerRetirements" | "readPrestartHouseRunnerRetirements"
  >,
  runId: string,
  lane: HouseRetirementLane,
  timing: { readonly timeoutMs?: number; readonly pollMs?: number } = {},
): Promise<boolean> {
  const timeoutMs = timing.timeoutMs ?? 60_000;
  const pollMs = timing.pollMs ?? 500;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 60_000 ||
    !Number.isSafeInteger(pollMs) || pollMs < 1 || pollMs > 5_000) {
    throw new Error("invalid exact House retirement observation timing");
  }
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const candidates = lane === "terminal"
      ? await data.readTerminalHouseRunnerRetirements(runId)
      : await data.readPrestartHouseRunnerRetirements(runId);
    if (candidates.length === 0) return true;
    await delay(pollMs);
  }
  throw new Error(`exact ${lane} House retirement receipt was not retained before timeout`);
}

async function waitForLaunchState(
  browser: CookieBrowser,
  launchId: string,
  expectedState: string,
): Promise<JsonRecord> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const launch = await browser.read(`/api/launches/${launchId}`);
    if (launch.state === expectedState) return launch;
    await delay(500);
  }
  throw new Error("Hosted capacity probe did not reach its expected terminal state");
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

test("local acceptance requires a distinct browser-stream origin", () => {
  assert.equal(
    localBrowserStreamOrigin("http://127.0.0.1:8080", "http://localhost:8080"),
    "http://localhost:8080",
  );
  assert.throws(
    () => localBrowserStreamOrigin("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
    /topology_invalid/u,
  );
  assert.throws(
    () => localBrowserStreamOrigin("http://127.0.0.1:8080", "http://localhost:8080/stream"),
    /topology_invalid/u,
  );
  assert.throws(
    () => localBrowserStreamOrigin("http://remote.test:8080", "http://localhost:8080"),
    /topology_invalid/u,
  );
  assert.throws(
    () => localBrowserStreamOrigin("https://127.0.0.1:8080", "http://localhost:8080"),
    /topology_invalid/u,
  );
  assert.throws(
    () => localBrowserStreamOrigin("http://127.0.0.1:8080", "http://localhost:8081"),
    /topology_invalid/u,
  );
  assert.throws(
    () => localBrowserStreamOrigin("http://127.0.0.1:8080/internal", "http://localhost:8080"),
    /topology_invalid/u,
  );
});

test("retained live acceptance recovery selects one exact Agent Heist Launch", () => {
  const launchId = "10000000-0000-4000-8000-000000000001";
  assert.equal(retainedHostedLiveLaunchId({
    version: "platform_my_games.v1",
    items: [{
      launch_id: launchId,
      title: "Agent Heist",
      state: "live",
      action: "return_to_game",
    }],
  }), launchId);
  assert.equal(retainedHostedLiveLaunchId({
    version: "platform_my_games.v1",
    items: [{
      launch_id: launchId,
      title: "Agent Heist",
      state: "verified_result",
      action: "view_result",
      result_public_id: "a".repeat(32),
    }],
  }), null);
  assert.throws(() => retainedHostedLiveLaunchId({
    version: "platform_my_games.v1",
    items: [
      { launch_id: launchId, title: "Agent Heist", state: "live", action: "return_to_game" },
      { launch_id: "20000000-0000-4000-8000-000000000002", title: "Agent Heist", state: "live", action: "return_to_game" },
    ],
  }), /multiple retained live Agent Heist Runs/u);
});

test("retained setup preflight selects one setup and rejects ambiguity", () => {
  const launchId = "10000000-0000-4000-8000-000000000001";
  assert.equal(retainedHostedSetupLaunchId({
    version: "platform_my_games.v1",
    items: [{
      launch_id: launchId,
      title: "Agent Heist",
      state: "setup_pending",
      action: "continue_setup",
    }],
  }), launchId);
  assert.equal(retainedHostedSetupLaunchId({
    version: "platform_my_games.v1",
    items: [{
      launch_id: launchId,
      title: "Agent Heist",
      state: "verified_result",
      action: "view_result",
      result_public_id: "a".repeat(32),
    }],
  }), null);
  assert.throws(() => retainedHostedSetupLaunchId({
    version: "platform_my_games.v1",
    items: [
      { launch_id: launchId, state: "setup_pending", action: "continue_setup" },
      { launch_id: "20000000-0000-4000-8000-000000000002", state: "setup_pending", action: "continue_setup" },
    ],
  }), /multiple retained setups/u);
});

test("acceptance history correspondence ignores unrelated retained results", () => {
  const unrelatedLaunchId = "20000000-0000-4000-8000-000000000002";
  const qualifiedLaunchId = "30000000-0000-4000-8000-000000000003";
  const unrelatedPublicId = "b".repeat(32);
  const qualifiedPublicId = "c".repeat(32);
  const before = {
    version: "platform_my_games.v1",
    items: [{
      launch_id: unrelatedLaunchId,
      title: "Agent Heist",
      state: "verified_result",
      action: "view_result",
      result_public_id: unrelatedPublicId,
    }],
  };
  const after = {
    version: "platform_my_games.v1",
    items: [
      before.items[0],
      {
        launch_id: qualifiedLaunchId,
        title: "Agent Heist",
        state: "verified_result",
        action: "view_result",
        result_public_id: qualifiedPublicId,
      },
    ],
  };
  assert.equal(
    exactNewVerifiedResultLaunchId(before, after, qualifiedPublicId),
    qualifiedLaunchId,
  );
  assert.throws(
    () => assertVerifiedMyGamesResult(after, qualifiedLaunchId, unrelatedPublicId),
    /Expected values to be strictly equal/u,
  );
  assert.throws(
    () => exactNewVerifiedResultLaunchId(before, {
      version: "platform_my_games.v1",
      items: [...before.items],
    }, qualifiedPublicId),
    /exactly one newly qualified Launch/u,
  );
});

test("allowance evidence advances only from completed retained ledger attempts", () => {
  const before = observeHouseAllowanceLedger({
    schema: HOUSE_ALLOWANCE_LEDGER_SCHEMA,
    assignments: {},
  });
  const after = observeHouseAllowanceLedger({
    schema: HOUSE_ALLOWANCE_LEDGER_SCHEMA,
    assignments: {
      "house-unit-1": {
        consumed_input_units: 12,
        consumed_output_units: 4,
        attempts: {
          "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
            state: "completed",
          },
          "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": {
            state: "reserved",
          },
        },
      },
    },
  });
  assert.equal(allowanceConsumptionAdvanced(before, after), true);
  assert.equal(after.completedAttempts, 1);
  assert.equal(after.consumedAttempts, 1);
});

test("allowance evidence uses the shared retained House ledger path", () => {
  assert.equal(
    houseAllowanceLedgerPath("/private/worldstream-state"),
    "/private/worldstream-state/hosted-house-runners/units/allowance/allowances.json",
  );
});

test("allowance evidence rejects an unknown retained attempt state", () => {
  assert.throws(() => observeHouseAllowanceLedger({
    schema: HOUSE_ALLOWANCE_LEDGER_SCHEMA,
    assignments: {
      "house-unit-1": {
        consumed_input_units: 1,
        consumed_output_units: 1,
        attempts: {
          "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
            state: "unverified",
          },
        },
      },
    },
  }), /attempt state is invalid/u);
});

test("exact terminal capacity evidence reads only the completed Run retirement lane", async () => {
  const reads: string[] = [];
  const data: Pick<
    ResultReconciliationData,
    "readTerminalHouseRunnerRetirements" | "readPrestartHouseRunnerRetirements"
  > = {
    async readTerminalHouseRunnerRetirements(runId) {
      reads.push(`terminal:${runId}`);
      return [];
    },
    async readPrestartHouseRunnerRetirements(runId) {
      reads.push(`prestart:${runId}`);
      return [];
    },
  };
  assert.equal(
    await waitForExactHouseRetirementClearance(
      data,
      "10000000-0000-4000-8000-000000000001",
      "terminal",
    ),
    true,
  );
  assert.deepEqual(reads, ["terminal:10000000-0000-4000-8000-000000000001"]);
});

test("exact prestart capacity evidence does not treat an outstanding receipt as released", async () => {
  const data: Pick<
    ResultReconciliationData,
    "readTerminalHouseRunnerRetirements" | "readPrestartHouseRunnerRetirements"
  > = {
    async readTerminalHouseRunnerRetirements() { return []; },
    async readPrestartHouseRunnerRetirements() {
      return [{}] as never;
    },
  };
  await assert.rejects(
    () => waitForExactHouseRetirementClearance(
      data,
      "10000000-0000-4000-8000-000000000001",
      "prestart",
      { timeoutMs: 5, pollMs: 1 },
    ),
    /exact prestart House retirement receipt/u,
  );
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

type ReentryController = Pick<HostedLiveSessionController, "state" | "subscribe" | "reconnect">;

async function reconnectForAcceptance(
  controller: ReentryController,
  label: "creator" | "external_agent" | "public_spectator",
  lastHttpFailure: () => string | undefined = () => undefined,
): Promise<void> {
  let connectingObserved = false;
  let freshSynchronization = false;
  const unsubscribe = controller.subscribe((snapshot) => {
    if (snapshot.status === "connecting" || snapshot.status === "synchronizing") {
      connectingObserved = true;
    }
    if (connectingObserved && snapshot.status === "live" && snapshot.synchronized) {
      freshSynchronization = true;
    }
  });
  const failure = (returned: string) => {
    const state = controller.state;
    const http = lastHttpFailure();
    const safeHttp = typeof http === "string" &&
      /^\/api\/v1\/participant-console\/(?:session(?::[a-z-]+)?|handoffs:redeem) returned [1-5][0-9]{2}: [a-z][a-z0-9_]{0,63}$/u.test(http)
      ? http : "none_or_unclassified";
    return new Error(
      `${label} re-entry failed (returned=${returned},current=${state.status},` +
      `synchronized=${state.synchronized},fresh_sync=${freshSynchronization},` +
      `message_type=${safeSessionMessageType(state.message)},http=${safeHttp})`,
    );
  };
  try {
    let result: HostedLiveSessionSnapshot;
    try {
      result = await controller.reconnect();
    } catch {
      throw failure("rejected");
    }
    if (result.status !== "live" || !result.synchronized ||
      controller.state.status !== "live" || !controller.state.synchronized ||
      !freshSynchronization) {
      throw failure(result.status);
    }
  } finally {
    unsubscribe();
  }
}

function safeSessionMessageType(message: string | null): string {
  const known: Readonly<Record<string, string>> = {
    "Realtime admission failed.": "admission_failed",
    "Realtime connection failed.": "transport_failed",
    "Realtime connection closed. Reconnect to continue.": "transport_closed",
    "Realtime connection could not be established.": "transport_unavailable",
    "Realtime protocol validation failed.": "protocol_validation_failed",
    "WorldStream rejected the realtime operation.": "operation_rejected",
    "Realtime heartbeat timed out. Reconnect to continue.": "heartbeat_timeout",
    "Activity Client session is unavailable.": "authority_unavailable",
    "The public Projection is unavailable. Reconnect to try again.": "public_stream_unavailable",
  };
  if (message === null) return "none";
  return Object.hasOwn(known, message) ? (known[message] ?? "unclassified") : "unclassified";
}

async function withSessionCleanup(
  operation: (register: <T extends { close(): void }>(controller: T) => T) => Promise<void>,
): Promise<void> {
  const controllers: Array<{ close(): void }> = [];
  let failure: { error: unknown } | undefined;
  try {
    await operation((controller) => {
      controllers.push(controller);
      return controller;
    });
  } catch (error) {
    failure = { error };
  } finally {
    for (const controller of controllers) {
      try {
        controller.close();
      } catch {
        failure ??= { error: new Error("Acceptance session cleanup failed.") };
      }
    }
  }
  if (failure !== undefined) throw failure.error;
}

function reentrySnapshot(
  status: HostedLiveSessionSnapshot["status"],
  message: string | null = null,
): HostedLiveSessionSnapshot {
  return {
    status,
    synchronized: status === "live",
    canAct: status === "live",
    deliveryBatch: null,
    lastAcknowledgedFrameSeq: 9,
    actionReceipt: null,
    message,
  };
}

function reentryFixture(
  emissions: readonly HostedLiveSessionSnapshot[],
  result: HostedLiveSessionSnapshot,
) {
  let state = reentrySnapshot("live");
  const listeners = new Set<(snapshot: HostedLiveSessionSnapshot) => void>();
  const controller: ReentryController = {
    get state() { return state; },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async reconnect() {
      for (const snapshot of emissions) {
        state = snapshot;
        for (const listener of listeners) listener(snapshot);
      }
      return result;
    },
  };
  return { controller, listenerCount: () => listeners.size };
}

test("re-entry evidence rejects a disconnected reconnect result", async () => {
  const disconnected = reentrySnapshot("disconnected");
  const fixture = reentryFixture([disconnected], disconnected);
  await assert.rejects(() => reconnectForAcceptance(fixture.controller, "external_agent"));
  assert.equal(fixture.listenerCount(), 0);
});

test("re-entry evidence rejects cached live state without a new synchronization", async () => {
  const fixture = reentryFixture([], reentrySnapshot("live"));
  await assert.rejects(() => reconnectForAcceptance(fixture.controller, "creator"));
  assert.equal(fixture.listenerCount(), 0);
});

test("re-entry evidence accepts a fresh synchronization at the unchanged Frame Head", async () => {
  const live = reentrySnapshot("live");
  const fixture = reentryFixture([reentrySnapshot("connecting"), live], live);
  await reconnectForAcceptance(fixture.controller, "creator");
  assert.equal(fixture.controller.state.lastAcknowledgedFrameSeq, 9);
  assert.equal(fixture.listenerCount(), 0);
});

test("re-entry evidence rejects a stream that closes after fresh synchronization", async () => {
  const live = reentrySnapshot("live");
  const fixture = reentryFixture([
    reentrySnapshot("connecting"), live, reentrySnapshot("disconnected"),
  ], live);
  await assert.rejects(
    () => reconnectForAcceptance(fixture.controller, "public_spectator"),
    /current=disconnected/u,
  );
  assert.equal(fixture.listenerCount(), 0);
});

test("re-entry evidence sanitizes failures and releases its listener on rejection", async () => {
  const unknown = "opaque-untrusted-value-must-not-appear";
  const disconnected = reentrySnapshot("disconnected", unknown);
  const fixture = reentryFixture([disconnected], disconnected);
  await assert.rejects(
    () => reconnectForAcceptance(fixture.controller, "external_agent", () => unknown),
    (error: Error) => error.message.includes("message_type=unclassified") &&
      error.message.includes("http=none_or_unclassified") && !error.message.includes(unknown),
  );
  fixture.controller.reconnect = async () => { throw new Error(unknown); };
  await assert.rejects(
    () => reconnectForAcceptance(fixture.controller, "external_agent"),
    (error: Error) => error.message.includes("returned=rejected") && !error.message.includes(unknown),
  );
  assert.equal(fixture.listenerCount(), 0);
});

test("acceptance cleanup closes every session even after operation and cleanup failures", async () => {
  const expected = new Error("scenario failed");
  const closed: number[] = [];
  await assert.rejects(withSessionCleanup(async (register) => {
    register({ close() { closed.push(1); throw new Error("close failed"); } });
    register({ close() { closed.push(2); } });
    throw expected;
  }), (error) => error === expected);
  assert.deepEqual(closed, [1, 2]);
});

async function waitForHeist(
  controller: Pick<HostedLiveSessionController, "state" | "waitFor">,
  read: () => AgentHeistLiveState,
  predicate: (state: AgentHeistReadyState) => boolean,
  timeoutMs = 30_000,
): Promise<AgentHeistReadyState> {
  const isLive = () => controller.state.status === "live" && controller.state.synchronized;
  const current = read();
  if (isLive() && current.kind === "ready" && predicate(current)) return current;
  await controller.waitFor(() => {
    const state = read();
    return isLive() && state.kind === "ready" && predicate(state);
  }, { timeoutMs: Math.min(timeoutMs, 60_000) });
  const result = ready(read());
  if (!isLive() || !predicate(result)) throw new Error("Live Agent Heist state predicate did not hold");
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

async function pollPublicResult(
  origin: string,
  publicId: string,
  timeoutMs = 60_000,
): Promise<JsonRecord> {
  const deadline = Date.now() + timeoutMs;
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

/**
 * My Games is an owner index, not a counter. Acceptance claims must bind to
 * the exact Launch that produced the Run under test; an unrelated retained
 * result must never satisfy a history or upgrade assertion.
 */
function assertVerifiedMyGamesResult(
  value: unknown,
  launchId: string,
  publicId: string,
): JsonRecord {
  const item = myGamesItemForLaunch(value, launchId);
  assert.equal(item.state, "verified_result");
  assert.equal(item.action, "view_result");
  assert.equal(item.result_public_id, publicId);
  return item;
}

function myGamesItemForLaunch(value: unknown, launchId: string): JsonRecord {
  if (!LAUNCH_ID_PATTERN.test(launchId)) throw new Error("invalid expected My Games Launch identity");
  if (!isRecord(value) || value.version !== "platform_my_games.v1" || !Array.isArray(value.items)) {
    throw new Error("hosted acceptance My Games response is invalid");
  }
  const matches = value.items.filter((item): item is JsonRecord =>
    isRecord(item) && item.launch_id === launchId,
  );
  assert.equal(matches.length, 1, `My Games must contain exactly one item for Launch ${launchId}`);
  const item = matches[0];
  assert.ok(item);
  return item;
}

function exactNewLaunchId(before: unknown, after: unknown): string {
  const beforeItems = myGamesItems(before);
  const afterItems = myGamesItems(after);
  const priorIds = new Set(beforeItems.map((item) => stringField(item, "launch_id")));
  const fresh = afterItems.filter((item) => !priorIds.has(stringField(item, "launch_id")));
  assert.equal(fresh.length, 1, "acceptance must identify exactly one newly qualified Launch");
  const launchId = stringField(fresh[0] as JsonRecord, "launch_id");
  assert.match(launchId, LAUNCH_ID_PATTERN);
  return launchId;
}

function exactNewVerifiedResultLaunchId(
  before: unknown,
  after: unknown,
  publicId: string,
): string {
  assert.match(publicId, PUBLIC_ID_PATTERN);
  const launchId = exactNewLaunchId(before, after);
  assertVerifiedMyGamesResult(after, launchId, publicId);
  return launchId;
}

function myGamesItems(value: unknown): JsonRecord[] {
  if (!isRecord(value) || value.version !== "platform_my_games.v1" || !Array.isArray(value.items)) {
    throw new Error("hosted acceptance My Games response is invalid");
  }
  return value.items.map((item) => record(item));
}

/**
 * Select at most one retained pre-start setup from the qualification account.
 * This is intentionally state-based rather than tied to a launch ID. A
 * second setup is ambiguous and must stop the acceptance run before any
 * mutation is attempted.
 */
export function retainedHostedSetupLaunchId(value: unknown): string | null {
  if (!isRecord(value) || value.version !== "platform_my_games.v1" || !Array.isArray(value.items)) {
    throw new Error("hosted acceptance My Games response is invalid");
  }
  const candidates = value.items.filter((item): item is JsonRecord =>
    isRecord(item) &&
    item.state === "setup_pending" &&
    item.action === "continue_setup" &&
    item.result_public_id === undefined &&
    typeof item.launch_id === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(item.launch_id),
  );
  if (candidates.length > 1) {
    throw new Error("hosted acceptance found multiple retained setups");
  }
  return candidates[0] === undefined ? null : stringField(candidates[0], "launch_id");
}

async function preflightRetainedHostedSetup(
  creator: CookieBrowser,
  launchId: string,
): Promise<void> {
  const launch = await creator.read(`/api/launches/${launchId}`);
  const state = stringField(launch, "state");
  if (state === "collecting") {
    const cancelled = await creator.mutate(`/api/launches/${launchId}/cancel`, {}, [200]);
    assert.equal(cancelled.version, "hosted_launch_cancelled.v1");
    assert.equal(cancelled.cancelled, true);
    assert.equal((await creator.read(`/api/launches/${launchId}`)).state, "cancelled");
    console.info("Hosted acceptance: cancelled the one retained collecting setup through the normal API.");
    return;
  }
  if (state !== "provisioning" && state !== "reconciling" && state !== "run_created") {
    throw new Error(`hosted acceptance retained setup state is not resumable: ${state}`);
  }
  if (launch.activity_slug !== "agent-heist") {
    throw new Error("hosted acceptance cannot resume a retained non-Heist setup in this candidate");
  }
  if (state !== "run_created") {
    const formed = await startLaunch(creator, launchId);
    assert.equal(formed.state, "run_created");
  }
}

async function completeRetainedRun(options: {
  creator: CookieBrowser;
  browserAgent: CookieBrowser;
  productOrigin: string;
  browserStreamOrigin: string;
  directPush: DirectPushMeasurement;
  register: <T extends { close(): void }>(controller: T) => T;
  launchId: string;
}): Promise<void> {
  const retainedLaunch = await options.creator.read(`/api/launches/${options.launchId}`);
  if (retainedLaunch.fill_mode !== "people_only") {
    await completeRetainedRenderedRun(
      options.creator,
      options.productOrigin,
      options.launchId,
    );
    return;
  }
  assert.equal(retainedLaunch.activity_slug, "agent-heist");
  assert.equal(retainedLaunch.state, "run_created");
  const retainedRun = recordField(retainedLaunch, "run");
  assert.equal(retainedRun.can_enter, true);
  const publicId = stringField(retainedRun, "public_id");
  await synchronizePeopleOnlyRun({ ...options, run: retainedRun });
  const terminal = await pollPublicResult(options.productOrigin, publicId, 240_000);
  assert.equal(terminal.state, "result");
  const recoveredHistory = await options.creator.read("/api/my-games");
  assertVerifiedMyGamesResult(recoveredHistory, options.launchId, publicId);
  console.info("Hosted acceptance: resumed the exact retained people-only Run to a verified result.");
}

async function completeRetainedRenderedRun(
  creator: CookieBrowser,
  productOrigin: string,
  launchId: string,
): Promise<void> {
  const retainedLaunch = await creator.read(`/api/launches/${launchId}`);
  assert.equal(retainedLaunch.activity_slug, "agent-heist");
  assert.equal(retainedLaunch.state, "run_created");
  const retainedRun = recordField(retainedLaunch, "run");
  assert.equal(retainedRun.can_enter, true);
  const recovered = await renderedBrowserJourney.runHostedRenderedRetainedRecovery({
    productOrigin,
    existingLaunchId: launchId,
    formationTimeoutMs: 240_000,
  });
  assert.equal(recovered.outcome, "passed");
  assert.equal(recovered.completed, true);
  assert.equal(recovered.recovery, "retained_terminal_only");
  const recoveredHistory = await creator.read("/api/my-games");
  const recoveredItem = arrayField(recoveredHistory, "items")
    .map((item) => record(item))
    .find((item) => item.launch_id === launchId);
  assert.equal(recoveredItem?.state, "verified_result");
  assert.equal(recoveredItem?.action, "view_result");
  assert.equal(typeof recoveredItem?.result_public_id, "string");
  console.info("Hosted acceptance: resumed the exact retained rendered Run to a verified result.");
}

/**
 * Select only one interrupted Agent Heist Run owned by the qualification
 * identity. Multiple candidates are unsafe: choosing one could mutate a
 * different retained Room, while ignoring them would leave an abandoned
 * live setup for every acceptance retry.
 */
export function retainedHostedLiveLaunchId(value: unknown): string | null {
  if (!isRecord(value) || value.version !== "platform_my_games.v1" || !Array.isArray(value.items)) {
    throw new Error("hosted acceptance My Games response is invalid");
  }
  const candidates = value.items.filter((item): item is JsonRecord =>
    isRecord(item) &&
    item.title === "Agent Heist" &&
    item.state === "live" &&
    item.action === "return_to_game" &&
    item.result_public_id === undefined &&
    typeof item.launch_id === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(item.launch_id),
  );
  if (candidates.length > 1) {
    throw new Error("hosted acceptance found multiple retained live Agent Heist Runs");
  }
  return candidates[0] === undefined ? null : stringField(candidates[0], "launch_id");
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

function isRecord(value: unknown): value is JsonRecord {
  return value !== null && typeof value === "object" && !Array.isArray(value);
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

function nonNegativeIntegerField(value: JsonRecord, field: string): number {
  const selected = integerField(value, field);
  if (selected < 0) throw new Error(`${field} must not be negative`);
  return selected;
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0) throw new Error(`${name}_is_required`);
  return value;
}

/**
 * The local Gateway HTTP origin and browser-stream origin intentionally use
 * separate public authorities. Node's WebSocket client has no ambient browser
 * cookies, but the Gateway still validates the request Host against its
 * configured public authority. Keep this qualification-only topology exact.
 */
function localBrowserStreamOrigin(internalOrigin: string, browserOrigin: string): string {
  let internal: URL;
  let browser: URL;
  try {
    internal = new URL(internalOrigin);
    browser = new URL(browserOrigin);
  } catch {
    throw new Error("hosted_acceptance_browser_stream_origin_invalid");
  }
  if (
    internal.protocol !== "http:" ||
    browser.protocol !== "http:" ||
    internal.hostname !== "127.0.0.1" ||
    browser.hostname !== "localhost" ||
    internal.port.length === 0 ||
    browser.port !== internal.port ||
    internal.pathname !== "/" ||
    internal.search !== "" ||
    internal.hash !== "" ||
    internal.username !== "" ||
    internal.password !== "" ||
    browser.username !== "" ||
    browser.password !== "" ||
    browser.pathname !== "/" ||
    browser.search !== "" ||
    browser.hash !== ""
  ) {
    throw new Error("hosted_acceptance_browser_stream_origin_topology_invalid");
  }
  return browser.origin;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
