import { AsyncLocalStorage } from "node:async_hooks";
import { randomUUID } from "node:crypto";
import type { PlatformBff } from "./bff.js";

type Operation = "request_failed" | "request_threw" | "history_reconciliation" |
  "history_read" | "account_verification" | "run_entry" | "formation" |
  "fly_formation" | "fly_browser_session" | "fly_result_source" | "supabase_rpc";
type LogSink = (line: string) => void;
const context = new AsyncLocalStorage<{
  requestId: string; route: string; method: string; started: number; log: LogSink;
}>();

/** No messages, stacks, URLs, headers, arguments, or response bodies enter logs. */
export function reportPlatformFailure(
  operation: Operation,
  error?: unknown,
  details: { status?: number; rpc?: string; code?: string } = {},
): void {
  const current = context.getStore();
  if (current === undefined) return;
  try {
    current.log(JSON.stringify({
      level: "error", event: "platform_dependency_failure", operation,
      request_id: current.requestId, route: current.route, method: current.method,
      elapsed_ms: Math.max(0, Date.now() - current.started),
      error_kind: errorKind(error),
      ...(Number.isInteger(details.status) && details.status! >= 100 && details.status! <= 599 ? { status: details.status } : {}),
      // RPC names are supplied only by the source-controlled adapter, not request data.
      ...(details.rpc !== undefined && /^[a-z][a-z0-9_]{0,95}$/u.test(details.rpc) ? { rpc: details.rpc } : {}),
      ...(details.code !== undefined && /^(?:[0-9]{2}[0-9A-Z]{3}|PGRST[0-9]{3})$/u.test(details.code) ? { dependency_code: details.code } : {}),
    }));
  } catch { /* Telemetry must never change request semantics. */ }
}

export function withPlatformDiagnostics(platform: PlatformBff, log: LogSink = (line) => console.error(line)): PlatformBff {
  return { fetch: async (request) => context.run({
    requestId: randomUUID(), route: routeCategory(new URL(request.url).pathname),
    method: ["GET", "POST", "DELETE", "PUT", "PATCH", "HEAD", "OPTIONS"].includes(request.method) ? request.method : "other",
    started: Date.now(), log,
  }, async () => {
    let response: Response;
    try {
      response = await platform.fetch(request);
      if (response.status >= 500) reportPlatformFailure("request_failed", undefined, { status: response.status });
    } catch (error) {
      reportPlatformFailure("request_threw", error, { status: 503 });
      response = Response.json({ error: { code: "temporarily_unavailable" } }, {
        status: 503, headers: { "cache-control": "private, no-store, max-age=0", "x-content-type-options": "nosniff" },
      });
    }
    // Copy immutable fetch headers too; preserve all original cookies and policy.
    const headers = new Headers(response.headers);
    headers.set("x-worldstream-request-id", context.getStore()!.requestId);
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  }) };
}

function routeCategory(path: string): string {
  if (path === "/api/my-games") return "my_games";
  if (path === "/api/runs/enter") return "run_entry";
  if (path === "/api/launches") return "launch_create";
  if (/^\/api\/launches\/[^/]+\/start$/u.test(path)) return "launch_start";
  if (path.startsWith("/api/launches/")) return "launch";
  if (path.startsWith("/api/auth/")) return "auth";
  if (path.startsWith("/api/participant/")) return "participant";
  if (path.startsWith("/api/runs/") || path.startsWith("/api/results/")) return "results";
  if (path === "/api/internal/reconcile") return "maintenance";
  return "other";
}

function errorKind(error: unknown): string {
  for (let depth = 0; depth < 4 && error instanceof Error; depth += 1) {
    if (error.name === "TimeoutError") return "timeout";
    if (error.name === "AbortError") return "aborted";
    const code = (error as Error & { code?: unknown }).code;
    if (["ECONNRESET", "ECONNREFUSED", "ENOTFOUND", "EAI_AGAIN", "ETIMEDOUT", "UND_ERR_CONNECT_TIMEOUT", "UND_ERR_HEADERS_TIMEOUT", "UND_ERR_SOCKET"].includes(String(code))) return "network";
    error = error.cause;
  }
  return "unclassified";
}
