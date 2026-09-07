// Temporary [DEBUG-reentry-ad71] acceptance-only fetch probe. Remove after diagnosis.
export function diagnosticFetch(dispatch, configuration, emit) {
  const { actor } = configuration;
  if (!["creator_bff", "acceptance_process"].includes(actor)) throw new Error("invalid_diagnostic_actor");
  const gateway = loopbackOrigin(configuration.gatewayOrigin);
  const database = loopbackOrigin(configuration.supabaseOrigin);
  let successes = 0;
  let failures = 0;
  const report = (stage, status) => {
    const success = status !== null && status >= 200 && status < 300;
    // Successful setup traffic must never consume the failure budget.
    if (success ? successes++ >= 64 : failures++ >= 64) return;
    const category = status === null ? "fetch_rejected"
      : success ? "http_success"
      : status === 429 ? "rate_limited"
      : status === 503 ? "service_unavailable"
      : status >= 500 ? "server_error"
      : status >= 400 ? "request_rejected" : "unexpected_status";
    try { emit({ actor, stage, status, category }); } catch { /* Diagnostic sinks cannot change fetch. */ }
  };
  return async (input, init) => {
    let stage = null;
    try {
      const url = new URL(input instanceof Request ? input.url : String(input));
      const method = init?.method ?? (input instanceof Request ? input.method : "GET");
      if (url.search === "" && url.hash === "" && url.username === "" && url.password === "") {
        if (url.origin === gateway && method === "POST" && url.pathname === "/v1/hosted/browser-handoffs/issue") stage = "gateway_issue_handoff";
        if (url.origin === database) {
          if (method === "GET" && url.pathname === "/auth/v1/user") stage = "auth_user";
          if (method === "POST" && url.pathname === "/rest/v1/rpc/sync_github_identity_v1") stage = "database_identity";
          if (method === "POST" && url.pathname === "/rest/v1/rpc/resolve_owned_run_membership_v1") stage = "database_run_membership";
        }
      }
    } catch { /* An unknown request is passed through without inspecting it. */ }
    let response;
    try { response = await dispatch(input, init); }
    catch (error) {
      if (stage !== null) report(stage, null);
      throw error;
    }
    if (stage !== null) report(stage, response.status);
    // Never clone/read a response or replace its body, headers, or identity.
    return response;
  };
}

function loopbackOrigin(value) {
  const url = new URL(value);
  if (url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname) ||
      url.username !== "" || url.password !== "" || url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    throw new Error("diagnostics_require_exact_loopback_origin");
  }
  return url.origin;
}

// The canonical CI step preloads this module. Only the existing development
// BFF and explicit acceptance process install it; providers/normal dev do not.
const creatorBff = process.argv[1]?.endsWith("/web/platform/dist/dev-server.js") === true;
const acceptance = process.env.WORLDSTREAM_LOCAL_ACCEPTANCE === "visible-local-only";
if (process.env.WORLDSTREAM_REENTRY_DIAGNOSTICS === "visible-local-only" && (creatorBff || acceptance)) {
  if (process.env.CI !== "true" || process.env.NODE_ENV === "production" || process.env.VERCEL_ENV === "production") {
    throw new Error("reentry_diagnostics_are_local_ci_only");
  }
  globalThis.fetch = diagnosticFetch(globalThis.fetch, {
    actor: creatorBff ? "creator_bff" : "acceptance_process",
    gatewayOrigin: process.env.WORLDSTREAM_HOSTED_GATEWAY_URL,
    supabaseOrigin: process.env.SUPABASE_URL,
  }, (event) => process.stdout.write(`[DEBUG-reentry-ad71] ${JSON.stringify(event)}\n`));
}
