import { Buffer } from "node:buffer";
import { timingSafeEqual } from "node:crypto";
import { reportPlatformFailure } from "./diagnostics.js";

import {
  agentHeistListingBase64,
  retainedAgentHeistListing025Base64,
  retainedAgentHeistListing024Base64,
  retainedAgentHeistListing023Base64,
  retainedAgentHeistListing022Base64,
  retainedAgentHeistListing021Base64,
  retainedAgentHeistListing020Base64,
  retainedAgentHeistListing019Base64,
  retainedAgentHeistListing018Base64,
  retainedAgentHeistListing017Base64,
  retainedAgentHeistListing016Base64,
  retainedAgentHeistListing015Base64,
  retainedAgentHeistListing014Base64,
  retainedAgentHeistListing013Base64,
  retainedAgentHeistListing012Base64,
  retainedAgentHeistListing011Base64,
  retainedAgentHeistResultProjector03Base64,
  retainedAgentHeistListing010Base64,
  retainedAgentHeistListing02Base64,
  retainedAgentHeistListing03Base64,
  retainedAgentHeistListing04Base64,
  retainedAgentHeistListing05Base64,
  retainedAgentHeistListing06Base64,
  retainedAgentHeistListing07Base64,
  retainedAgentHeistListing08Base64,
  retainedAgentHeistListing09Base64,
  agentHeistPublicProjectionSchemaBase64,
  agentHeistResultProjectorBase64,
  retainedAgentHeistResultProjector04Base64,
  retainedAgentHeistResultProjector02Base64,
  declarativeResultProjectorRuntimeBase64,
  resultSummarySchemaBase64,
} from "./hosted-artifacts.generated.js";
import type { PlatformBff } from "./bff.js";
import {
  HttpHostedResultSourceClient,
  HttpHostedHouseRetirementClient,
  PinnedResultProjectorRegistry,
  reconcileActivityResultCandidates,
  reconcileActivityResult,
  reconcileTerminalActivityCapacity,
  reconcilePrestartHouseRunnerRetirementCandidates,
  reconcileTerminalHouseRunnerRetirementCandidates,
  type ResultReconcilerDependencies,
  type ResultReconciliationCandidate,
} from "./result-reconciliation.js";
import { createSupabaseResultReconciliationData } from "./supabase.js";

const PUBLIC_RUN = /^\/api\/runs\/[0-9a-f]{32}$/u;
const RECENT_RESULTS = "/api/results/agent-heist/recent";
const LAUNCH_START = /^\/api\/launches\/[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\/start$/u;
// This is deliberately process-local: the preview has one Fly machine and no
// worker queue. It prevents a browser poll fan-out from starting equivalent
// global maintenance passes while preserving two five-second poll intervals.
const READ_RECONCILIATION_COOLDOWN_MS = 10_000;
const FAILED_READ_RECONCILIATION_COOLDOWN_MS = 5_000;

export function createHostedResultReconciler(input: {
  readonly supabaseUrl: string;
  readonly dataSecretKey: string;
  readonly hostedGatewayUrl: string;
  readonly serviceAuthority: string;
}): ResultReconcilerDependencies {
  return {
    data: createSupabaseResultReconciliationData(
      input.supabaseUrl,
      input.dataSecretKey,
    ),
    source: new HttpHostedResultSourceClient({
      baseUrl: input.hostedGatewayUrl,
      serviceAuthority: input.serviceAuthority,
    }),
    houseRetirement: new HttpHostedHouseRetirementClient({
      baseUrl: input.hostedGatewayUrl,
      serviceAuthority: input.serviceAuthority,
    }),
    projectors: new PinnedResultProjectorRegistry([
      agentHeistListingBase64, retainedAgentHeistListing025Base64, retainedAgentHeistListing024Base64, retainedAgentHeistListing023Base64, retainedAgentHeistListing022Base64, retainedAgentHeistListing021Base64, retainedAgentHeistListing020Base64, retainedAgentHeistListing019Base64, retainedAgentHeistListing018Base64, retainedAgentHeistListing017Base64, retainedAgentHeistListing016Base64, retainedAgentHeistListing015Base64, retainedAgentHeistListing014Base64, retainedAgentHeistListing013Base64, retainedAgentHeistListing012Base64, retainedAgentHeistListing011Base64, retainedAgentHeistListing02Base64, retainedAgentHeistListing03Base64,
      retainedAgentHeistListing04Base64, retainedAgentHeistListing05Base64, retainedAgentHeistListing06Base64, retainedAgentHeistListing07Base64, retainedAgentHeistListing08Base64, retainedAgentHeistListing09Base64, retainedAgentHeistListing010Base64,
    ].map((listingBytes) => ({
        listingBytes: decode(listingBytes),
        projectorBytes: decode(listingBytes === agentHeistListingBase64 || listingBytes === retainedAgentHeistListing025Base64 || listingBytes === retainedAgentHeistListing024Base64 ? agentHeistResultProjectorBase64
          : listingBytes === retainedAgentHeistListing012Base64 ? retainedAgentHeistResultProjector04Base64
          : listingBytes === retainedAgentHeistListing023Base64 || listingBytes === retainedAgentHeistListing022Base64 || listingBytes === retainedAgentHeistListing021Base64 || listingBytes === retainedAgentHeistListing020Base64 || listingBytes === retainedAgentHeistListing019Base64 || listingBytes === retainedAgentHeistListing018Base64 || listingBytes === retainedAgentHeistListing017Base64 || listingBytes === retainedAgentHeistListing016Base64 || listingBytes === retainedAgentHeistListing015Base64 || listingBytes === retainedAgentHeistListing014Base64 || listingBytes === retainedAgentHeistListing013Base64 || listingBytes === retainedAgentHeistListing011Base64 || listingBytes === retainedAgentHeistListing07Base64 || listingBytes === retainedAgentHeistListing08Base64 || listingBytes === retainedAgentHeistListing09Base64 || listingBytes === retainedAgentHeistListing010Base64
            ? retainedAgentHeistResultProjector03Base64 : retainedAgentHeistResultProjector02Base64),
        runtimeBytes: decode(declarativeResultProjectorRuntimeBase64),
        projectionSchemaBytes: decode(agentHeistPublicProjectionSchemaBase64),
        outputSchemaBytes: decode(resultSummarySchemaBase64),
      }))),
  };
}

/**
 * Reconciles a bounded candidate batch before an anonymous result read. This
 * keeps the hobby preview operational without a second always-on worker or a
 * WebSocket path through Vercel. Every write remains Replay-verified and
 * idempotent in the shared reconciler.
 */
export function withHostedResultReconciliation(
  platform: PlatformBff,
  dependencies: ResultReconcilerDependencies,
  recovery?: {
    readonly canonicalOrigin: string;
    readonly cronSecret: string;
    readonly recover: (launchRequestId: string) => Promise<unknown>;
    readonly listPendingClosures: (limit: number) => Promise<readonly string[]>;
    readonly recoverClosure: (launchRequestId: string) => Promise<unknown>;
    /** Uses the Host's current task state; DB time only selects candidates. */
    readonly abandonPrestart: (launchRequestId: string) => Promise<unknown>;
  },
): PlatformBff {
  if (recovery !== undefined && !/^[\x21-\x7e]{32,256}$/u.test(recovery.cronSecret)) {
    throw new Error("invalid_reconciliation_cron_secret");
  }
  let readReconciliation: Promise<boolean> | undefined;
  let readReconciliationAvailableAt = 0;
  let readReconciliationHealthy = true;

  async function reconcileForRead(): Promise<boolean> {
    if (Date.now() < readReconciliationAvailableAt) return readReconciliationHealthy;
    if (readReconciliation !== undefined) {
      return readReconciliation;
    }

    const pass = reconcileActivityResultCandidates(dependencies, 10).then(() => {
      readReconciliationHealthy = true;
      readReconciliationAvailableAt = Date.now() + READ_RECONCILIATION_COOLDOWN_MS;
      return true;
    }).catch((error: unknown) => {
      reportPlatformFailure("history_reconciliation", error);
      readReconciliationHealthy = false;
      readReconciliationAvailableAt = Date.now() + FAILED_READ_RECONCILIATION_COOLDOWN_MS;
      return false;
    });
    readReconciliation = pass;
    try {
      return await pass;
    } finally {
      if (readReconciliation === pass) {
        readReconciliation = undefined;
      }
    }
  }

  return {
    async fetch(request: Request): Promise<Response> {
      const url = new URL(request.url);
      if (url.pathname === "/api/internal/reconcile") {
        const expected = Buffer.from(`Bearer ${recovery?.cronSecret ?? ""}`);
        const actual = Buffer.from(request.headers.get("authorization") ?? "");
        if (recovery === undefined || request.method !== "GET" ||
            url.origin !== recovery.canonicalOrigin || url.search !== "" ||
            actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
          return Response.json({ error: { code: "not_authorized" } }, { status: 401, headers: { "cache-control": "no-store" } });
        }
        // One small, sequential pass; no queue, new launch, or unbounded retry.
        // A broken candidate must not prevent repair of the rest of this batch.
        let candidates: readonly ResultReconciliationCandidate[];
        try {
          candidates = await dependencies.data.listCandidates(10);
        } catch {
          return Response.json({ error: { code: "temporarily_unavailable" } }, { status: 503, headers: { "cache-control": "no-store" } });
        }
        if (candidates.length > 10) return Response.json({ error: { code: "candidate_limit_exceeded" } }, { status: 503 });
        let failed = 0;
        let pendingClosures: readonly string[] = [];
        try {
          pendingClosures = await recovery.listPendingClosures(10);
          if (pendingClosures.length > 10) throw new Error("candidate_limit_exceeded");
          for (const launchRequestId of pendingClosures) {
            try { await recovery.recoverClosure(launchRequestId); } catch { failed += 1; }
          }
        } catch { failed += 1; }
        for (const candidate of candidates) {
          try {
            await dependencies.data.markAttempt(candidate.launchRequestId);
            if (candidate.candidateKind === "genesis") {
              await recovery.recover(candidate.launchRequestId);
            } else {
              await reconcileActivityResult(candidate, dependencies);
            }
          } catch (error) {
            failed += 1;
          }
        }
        try {
          await reconcileTerminalActivityCapacity(dependencies, 10);
        } catch { failed += 1; }
        // This is a distinct setup-reconciliation lane. The database applies
        // the reviewed pre-start deadline; the coordinator still asks the
        // Host whether its retained Lobby task has actually launched.
        let prestartLaunches: readonly string[] = [];
        try {
          prestartLaunches = await dependencies.data.listPrestartAbandonmentLaunches(10);
          if (prestartLaunches.length > 10) throw new Error("candidate_limit_exceeded");
          for (const launchRequestId of prestartLaunches) {
            try { await recovery.abandonPrestart(launchRequestId); } catch { failed += 1; }
          }
        } catch { failed += 1; }
        // Result publication and Host cleanup have distinct completion rules.
        // A terminal Run remains in this bounded lane until its receipt exists.
        try {
          await reconcileTerminalHouseRunnerRetirementCandidates(dependencies, 10);
          await reconcilePrestartHouseRunnerRetirementCandidates(dependencies, 10);
        } catch { failed += 1; }
        return Response.json({ attempted: pendingClosures.length + candidates.length + prestartLaunches.length, failed }, {
          status: failed === 0 ? 200 : 503, headers: { "cache-control": "no-store" },
        });
      }
      if (
        request.method === "GET" &&
        url.pathname === "/api/my-games"
      ) {
        // Authenticate and enforce account scope before doing any global
        // reconciliation work. Anonymous callers must not be able to trigger
        // even the bounded maintenance pass. A second private read returns
        // the refreshed account-scoped index after the pass.
        const initial = await platform.fetch(request);
        if (initial.status !== 200) return initial;
        const healthy = await reconcileForRead();
        // Recheck authentication/account scope even after failed maintenance:
        // never reuse the earlier body after revocation or a durable-read outage.
        const refreshed = await platform.fetch(request);
        if (healthy || refreshed.status !== 200) return refreshed;
        const headers = new Headers(refreshed.headers);
        headers.set("x-worldstream-refresh", "delayed");
        return new Response(refreshed.body, { status: refreshed.status, headers });
      }
      if (
        request.method === "GET" &&
        (PUBLIC_RUN.test(url.pathname) || url.pathname === RECENT_RESULTS)
      ) {
        try {
          await reconcileForRead();
        } catch {
          // Reconciliation is an opportunistic write repair, not authority for
          // the anonymous read. The underlying BFF can still serve only its
          // durable, already privacy-reviewed Supabase projection. A genuine
          // durable-read outage keeps the BFF's existing 503 boundary.
        }
      }
      const retryStart = request.method === "POST" && LAUNCH_START.test(url.pathname)
        ? request.clone()
        : null;
      const response = await platform.fetch(request);
      if (retryStart === null || !(await isActivityCapacityUnavailable(response))) {
        return response;
      }
      try {
        // The first request has already passed the underlying BFF's
        // authentication and CSRF checks. One bounded repair may clear only
        // terminal-evidence-backed legacy capacity before the same
        // idempotent start is attempted once more.
        await reconcileTerminalActivityCapacity(dependencies, 10);
      } catch {
        return response;
      }
      return platform.fetch(retryStart);
    },
  };
}

async function isActivityCapacityUnavailable(response: Response): Promise<boolean> {
  if (response.status !== 409) return false;
  const payload = await response.clone().json().catch(() => null);
  if (typeof payload !== "object" || payload === null || Array.isArray(payload)) return false;
  const error = (payload as Record<string, unknown>).error;
  return typeof error === "object" && error !== null && !Array.isArray(error) &&
    (error as Record<string, unknown>).code === "activity_capacity_unavailable";
}

function decode(value: string): Uint8Array {
  return Buffer.from(value, "base64");
}
