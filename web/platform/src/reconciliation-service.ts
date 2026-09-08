import { Buffer } from "node:buffer";
import { timingSafeEqual } from "node:crypto";

import {
  agentHeistListingBase64,
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
  retainedAgentHeistResultProjector02Base64,
  declarativeResultProjectorRuntimeBase64,
  resultSummarySchemaBase64,
} from "./hosted-artifacts.generated.js";
import type { PlatformBff } from "./bff.js";
import {
  HttpHostedResultSourceClient,
  PinnedResultProjectorRegistry,
  reconcileActivityResultCandidates,
  reconcileActivityResult,
  type ResultReconcilerDependencies,
  type ResultReconciliationCandidate,
} from "./result-reconciliation.js";
import { createSupabaseResultReconciliationData } from "./supabase.js";

const PUBLIC_RUN = /^\/api\/runs\/[0-9a-f]{32}$/u;
const RECENT_RESULTS = "/api/results/agent-heist/recent";

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
    projectors: new PinnedResultProjectorRegistry([
      agentHeistListingBase64, retainedAgentHeistListing02Base64, retainedAgentHeistListing03Base64,
      retainedAgentHeistListing04Base64, retainedAgentHeistListing05Base64, retainedAgentHeistListing06Base64, retainedAgentHeistListing07Base64, retainedAgentHeistListing08Base64, retainedAgentHeistListing09Base64,
    ].map((listingBytes) => ({
        listingBytes: decode(listingBytes),
        projectorBytes: decode(listingBytes === agentHeistListingBase64 || listingBytes === retainedAgentHeistListing07Base64 || listingBytes === retainedAgentHeistListing08Base64 || listingBytes === retainedAgentHeistListing09Base64
          ? agentHeistResultProjectorBase64 : retainedAgentHeistResultProjector02Base64),
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
  },
): PlatformBff {
  if (recovery !== undefined && !/^[\x21-\x7e]{32,256}$/u.test(recovery.cronSecret)) {
    throw new Error("invalid_reconciliation_cron_secret");
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
        for (const candidate of candidates) {
          try {
            await dependencies.data.markAttempt(candidate.launchRequestId);
            await recovery.recover(candidate.launchRequestId);
            if (candidate.candidateKind === "result_source") {
              await reconcileActivityResult(candidate, dependencies);
            }
          } catch { failed += 1; }
        }
        return Response.json({ attempted: candidates.length, failed }, {
          status: failed === 0 ? 200 : 503, headers: { "cache-control": "no-store" },
        });
      }
      if (
        request.method === "GET" &&
        (PUBLIC_RUN.test(url.pathname) || url.pathname === RECENT_RESULTS)
      ) {
        try {
          await reconcileActivityResultCandidates(dependencies, 10);
        } catch {
          return new Response('{"error":{"code":"temporarily_unavailable"}}', {
            status: 503,
            headers: {
              "cache-control": "public, no-store, max-age=0",
              "content-type": "application/json; charset=utf-8",
              "x-content-type-options": "nosniff",
            },
          });
        }
      }
      return platform.fetch(request);
    },
  };
}

function decode(value: string): Uint8Array {
  return Buffer.from(value, "base64");
}
