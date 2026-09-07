import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import {
  DEPLOYED_ACCEPTANCE_CHECKS,
  HOSTED_ACCEPTANCE_SCHEMA,
  readHostedAcceptanceEvidence,
  writeHostedAcceptanceEvidence,
} from "./hosted-acceptance-evidence.mjs";

const PUBLIC_ID = /^[0-9a-f]{32}$/u;

async function main() {
  const argumentsValue = process.argv.slice(2);
  if (argumentsValue[0] === "--write-template" && argumentsValue.length === 2) {
    const output = await writeHostedAcceptanceEvidence(
      resolve(argumentsValue[1]),
      template(),
    );
    process.stdout.write(`Deployed acceptance template: ${output}\n`);
    return;
  }
  if (
    argumentsValue.length !== 7 ||
    argumentsValue[0] !== "--verify" ||
    argumentsValue[3] !== "--origin" ||
    argumentsValue[5] !== "--public-id"
  ) {
    throw new Error(
      "usage: pnpm hosted:acceptance:deployed --write-template FILE | " +
        "--verify INPUT OUTPUT --origin HTTPS_ORIGIN --public-id PUBLIC_ID",
    );
  }
  const [, input, output, , originValue, , publicId] = argumentsValue;
  const origin = productionOrigin(originValue);
  if (!PUBLIC_ID.test(publicId)) throw new Error("public_id_invalid");
  const evidence = await readHostedAcceptanceEvidence(resolve(input), "deployed");
  if (evidence.outcome !== "passed") throw new Error("deployed_evidence_is_blocked");
  await verifyPublicDeployment(origin, publicId, evidence);
  const retained = await writeHostedAcceptanceEvidence(resolve(output), evidence);
  process.stdout.write(`Verified deployed acceptance evidence: ${retained}\n`);
}

export async function verifyPublicDeployment(origin, publicId, evidence, fetchImplementation = fetch) {
  const fetch = (url, options = {}) => fetchImplementation(url, {
    ...options, cache: "no-store", signal: AbortSignal.timeout(15_000),
  });
  const observed = await exactJson(await fetch(`${origin}/api/deployment`, { redirect: "error" }), 200);
  if (observed.version !== "hosted_deployment_identity.v1" || observed.commit !== evidence.commit) {
    throw new Error("deployment_revision_mismatch");
  }
  for (const [field, expected] of Object.entries(evidence.deployment)) {
    if (field !== "gateway_revision" && observed[field] !== expected) throw new Error("deployment_revision_mismatch");
  }
  const gatewayOrigin = productionOrigin(observed.gateway_origin);
  const gateway = await exactJson(await fetch(`${gatewayOrigin}/version`, { redirect: "error" }), 200);
  if (gateway.version !== "hosted_gateway_deployment.v1" || gateway.deployment !== evidence.deployment.gateway_revision) {
    throw new Error("gateway_revision_mismatch");
  }
  const catalog = await fetch(`${origin}/api/catalog`, {
    headers: { accept: "application/json" },
    redirect: "error",
  });
  const catalogBody = await exactJson(catalog, 200);
  if (
    catalog.headers.get("x-worldstream-development-substitute") !== null ||
    catalogBody.activities?.[0]?.availability !== "available"
  ) {
    throw new Error("production_catalog_invalid");
  }

  const websocket = await fetch(`${origin}/api/ws`, {
    headers: { accept: "application/json" },
    redirect: "error",
  });
  if (websocket.status !== 404) throw new Error("vercel_websocket_route_present");

  const oauth = await fetch(`${origin}/api/auth/github/start?return_to=%2F`, {
    redirect: "manual",
  });
  const location = oauth.headers.get("location");
  if (oauth.status !== 302 || location === null) throw new Error("github_oauth_start_invalid");
  const authorize = new URL(location);
  if (
    authorize.protocol !== "https:" ||
    authorize.pathname !== "/auth/v1/authorize" ||
    authorize.searchParams.get("provider") !== "github"
  ) {
    throw new Error("github_oauth_provider_invalid");
  }

  const run = await exactJson(
    await fetch(`${origin}/api/runs/${publicId}`, { redirect: "error" }),
    200,
  );
  if (
    run.state !== "result" ||
    run.public_id !== publicId ||
    run.evidence?.class !== "exhibition_platform_house_agents"
  ) {
    throw new Error("public_terminal_result_invalid");
  }
  const recent = await exactJson(
    await fetch(`${origin}/api/results/agent-heist/recent`, { redirect: "error" }),
    200,
  );
  if (!recent.results?.some((candidate) => candidate?.public_id === publicId)) {
    throw new Error("recent_result_unavailable");
  }
  assertPublic(JSON.stringify({ run, recent }));
}

async function exactJson(response, status) {
  const text = await response.text();
  if (response.status !== status || Buffer.byteLength(text) > 256 * 1024) {
    throw new Error(`deployment_probe_failed_${response.status}`);
  }
  try {
    return JSON.parse(text);
  } catch {
    throw new Error("deployment_probe_json_invalid");
  }
}

function assertPublic(text) {
  if (
    /"(?:account_id|entry_selector|handoff|invitation_token|membership_id|principal_id|provider_response|replay|room_id|service_scope_digest)"\s*:/u.test(text) ||
    /\b(?:wsh1|wss1|wst1|wsb1):[0-9a-f]{64}\b/iu.test(text)
  ) {
    throw new Error("private_public_response_detected");
  }
}

function productionOrigin(value) {
  const url = new URL(value);
  if (
    url.protocol !== "https:" ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== "" ||
    value !== url.origin
  ) {
    throw new Error("production_origin_invalid");
  }
  return url.origin;
}

function template() {
  return {
    schema: HOSTED_ACCEPTANCE_SCHEMA,
    candidate_kind: "deployed",
    outcome: "blocked",
    commit: "0".repeat(40),
    recorded_at: new Date().toISOString(),
    deployment: {
      platform_revision: "replace-with-vercel-deployment",
      gateway_revision: "replace-with-clean-git-commit-running-on-fly",
      schema_head: "replace-with-supabase-migration",
      listing_revision_digest: `blake3:${"0".repeat(64)}`,
      pack_digest: `blake3:${"0".repeat(64)}`,
      client_release_digest: `sha256:${"0".repeat(64)}`,
      projector_digest: `blake3:${"0".repeat(64)}`,
    },
    checks: Object.fromEntries(
      DEPLOYED_ACCEPTANCE_CHECKS.map((name) => [name, { status: "blocked" }]),
    ),
    metrics: { provider_calls: 0, maximum_direct_push_seconds: 0 },
    redaction: { private_projections_retained: false, credentials_retained: false },
  };
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) void main().catch((error) => {
  const message = error instanceof Error ? error.message : "deployed_acceptance_failed";
  process.stderr.write(`hosted deployed acceptance failed: ${message}\n`);
  process.exitCode = 1;
});
