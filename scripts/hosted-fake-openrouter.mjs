import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

export const DEVELOPMENT_FAKE_OPENROUTER_MODE = "visible-local-only";
const MAX_BODY_BYTES = 64 * 1024;
const RESPONSE_TEXT = "DEVELOPMENT_FAKE_RESPONSE";

export function createDevelopmentFakeOpenRouter(environment = process.env, { controlledCompletion } = {}) {
  const bind = required(environment, "WORLDSTREAM_FAKE_OPENROUTER_BIND");
  const port = portValue(required(environment, "WORLDSTREAM_FAKE_OPENROUTER_PORT"));
  const apiKey = required(environment, "WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY");
  assertDevelopmentFakeOpenRouterAllowed(environment, bind, apiKey);
  const state = { completionCount: 0, houseCompletionCount: 0 };
  return {
    bind,
    port,
    server: createServer((request, response) => {
      void dispatch(request, response, apiKey, state, controlledCompletion);
    }),
  };
}

export function assertDevelopmentFakeOpenRouterAllowed(environment, bind, apiKey) {
  if (
    environment.WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER !==
      DEVELOPMENT_FAKE_OPENROUTER_MODE ||
    environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT !== "development" ||
    environment.NODE_ENV === "production" ||
    environment.VERCEL_ENV === "production" ||
    bind !== "127.0.0.1" ||
    apiKey.length < 32 ||
    apiKey.length > 256
  ) {
    throw new Error("development_fake_openrouter_forbidden");
  }
}

async function dispatch(request, response, apiKey, state, controlledCompletion) {
  response.setHeader("x-worldstream-development-substitute", "fake-openrouter");
  response.setHeader("cache-control", "no-store");
  if (request.method === "GET" && request.url === "/healthz") {
    return json(response, 200, {
      status: "ready",
      version: "worldstream_development_fake_openrouter.v1",
      warning: "Development substitute. Never enable in production.",
    });
  }
  if (request.method === "GET" && request.url === "/api/v1/models") {
    if (!authorized(request, apiKey)) return json(response, 401, error("unauthorized"));
    return json(response, 200, {
      data: [{ id: "worldstream/development-deterministic", object: "model" }],
    });
  }
  if (request.method === "GET" && request.url === "/development/metrics") {
    if (!authorized(request, apiKey)) return json(response, 401, error("unauthorized"));
    return json(response, 200, {
      version: "worldstream_development_fake_openrouter_metrics.v1",
      completion_count: state.completionCount,
      house_completion_count: state.houseCompletionCount,
    });
  }
  if (request.method !== "POST" || request.url !== "/api/v1/chat/completions") {
    return json(response, 404, error("route_not_found"));
  }
  if (!authorized(request, apiKey)) return json(response, 401, error("unauthorized"));

  let input;
  try {
    input = JSON.parse((await boundedBody(request)).toString("utf8"));
  } catch {
    return json(response, 400, error("invalid_request"));
  }
  if (!validCompletion(input)) return json(response, 400, error("invalid_request"));
  const house = controlledCompletion?.(input) ?? deterministicHouseCompletion(input);
  state.completionCount += 1;
  if (house !== null) state.houseCompletionCount += 1;
  const content = house?.content ?? RESPONSE_TEXT;
  const completion = {
    id: `gen-worldstream-development-${createHash("sha256").update(JSON.stringify(input)).digest("hex").slice(0, 16)}`,
    object: "chat.completion",
    created: 0,
    model: input.model,
    choices: [
      {
        index: 0,
        finish_reason: "stop",
        message: { role: "assistant", content },
      },
    ],
    usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2, cost: 0 },
    ...(house === null
      ? {}
      : {
          openrouter_metadata: {
            requested: input.model,
            strategy: "direct",
            attempt: 1,
            endpoints: {
              total: 1,
              available: [{ provider: house.provider, model: input.model, selected: true }],
            },
            attempts: [{ provider: house.provider, model: input.model, status: 200 }],
            pipeline: [],
          },
        }),
  };
  if (input.stream !== true) return json(response, 200, completion);

  response.writeHead(200, { "content-type": "text/event-stream; charset=utf-8" });
  response.write(
    `data: ${JSON.stringify({
      ...completion,
      object: "chat.completion.chunk",
      choices: [
        { index: 0, finish_reason: "stop", delta: { role: "assistant", content } },
      ],
      usage: undefined,
    })}\n\n`,
  );
  response.end("data: [DONE]\n\n");
}

function deterministicHouseCompletion(input) {
  const provider = input.provider?.only?.[0];
  const message = input.messages.at(-1)?.content;
  if (typeof provider !== "string" || typeof message !== "string") return null;
  let invocation;
  try {
    invocation = JSON.parse(message);
  } catch {
    return null;
  }
  if (
    invocation?.schema !== "worldstream/house-model-invocation/v1" ||
    invocation.action_offers?.schema !== "worldstream/assignment-action-offer-list/v1" ||
    !Array.isArray(invocation.action_offers.offers)
  ) {
    return null;
  }
  // Heist frames replace the whole authorized activity view. A later turn may
  // contain retained frames without a Reset; never prefer an older Reset.
  const observation = invocation.projection;
  const latestFrame = Array.isArray(observation?.observations)
    ? observation.observations.at(-1) : undefined;
  const activity = latestFrame?.observation ??
    observation?.projection_reset?.projection?.activity ?? {};
  const offers = invocation.action_offers.offers;
  const findOffer = (type) => offers.find((offer) => offer?.action_type === type);
  const role = observation?.role ?? observation?.projection_reset?.projection?.core?.role;
  const owned = { navigator: ["route"], insider: ["entry_window"], broker: ["required_tool", "extraction"] }[role];
  if (!owned) return null;
  const unknown = owned.find(clue => clueClaim(activity.private_clues, clue) === null);
  const unpublished = owned.find(clue => clueClaim(activity.private_clues, clue) !== null && clueClaim(activity.public_claims, clue) === null);
  const firstPlanId = Array.isArray(activity.plans)
    ? activity.plans.find((plan) => typeof plan?.plan_id === "string")?.plan_id
    : undefined;
  const proposedPlan = planFromAuthorizedClaims(activity);
  const choices = [
    // A proposal is the Pack's explicit request for this reactive House turn.
    ...(firstPlanId === undefined ? [] : [["endorse_plan", { plan_id: firstPlanId }]]),
    ...(unknown ? [["inspect_clue", { clue_id: unknown }]] : []),
    ...(unpublished
      ? [["publish_clue", { clue_id: unpublished, claim_code: clueClaim(activity.private_clues, unpublished) }]]
      : []),
    ...(firstPlanId === undefined && proposedPlan !== null
      ? [["propose_plan", proposedPlan]] : []),
    ...(firstPlanId === undefined ? [] : [
      ["commit_move", {
        selected_plan_id: firstPlanId,
        contribute_required_resource: true,
      }],
    ]),
    ["acknowledge_result", {}],
  ];
  for (const [actionType, payload] of choices) {
    const offer = findOffer(actionType);
    if (typeof offer?.offer_id === "string") {
      return { provider, content: JSON.stringify({ offer_id: offer.offer_id, payload }) };
    }
  }
  return null;
}

function planFromAuthorizedClaims(activity) {
  const plan = {};
  for (const field of ["route", "entry_window", "required_tool", "extraction"]) {
    const claim = clueClaim(activity.public_claims, field) ?? clueClaim(activity.private_clues, field);
    const prefix = `${field}_`;
    if (claim === null || !claim.startsWith(prefix) || claim.length === prefix.length) return null;
    plan[field] = claim.slice(prefix.length);
  }
  return plan;
}

function clueClaim(clues, clueId) {
  if (!Array.isArray(clues)) return null;
  const clue = clues.find((candidate) => candidate?.clue_id === clueId);
  return typeof clue?.claim_code === "string" && clue.claim_code.length > 0
    ? clue.claim_code
    : null;
}

function validCompletion(input) {
  if (typeof input !== "object" || input === null || Array.isArray(input)) return false;
  if (typeof input.model !== "string" || input.model.length < 1 || input.model.length > 128) {
    return false;
  }
  if (!Array.isArray(input.messages) || input.messages.length < 1 || input.messages.length > 128) {
    return false;
  }
  if (
    input.max_tokens !== undefined &&
    (!Number.isSafeInteger(input.max_tokens) || input.max_tokens < 1 || input.max_tokens > 4096)
  ) {
    return false;
  }
  if (
    input.max_completion_tokens !== undefined &&
    (!Number.isSafeInteger(input.max_completion_tokens) ||
      input.max_completion_tokens < 1 ||
      input.max_completion_tokens > 4096)
  ) {
    return false;
  }
  return input.messages.every(
    (message) =>
      typeof message === "object" &&
      message !== null &&
      !Array.isArray(message) &&
      ["system", "user", "assistant", "tool"].includes(message.role) &&
      typeof message.content === "string" &&
      message.content.length <= 32 * 1024,
  );
}

function authorized(request, apiKey) {
  return request.headers.authorization === `Bearer ${apiKey}`;
}

async function boundedBody(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += bytes.byteLength;
    if (size > MAX_BODY_BYTES) throw new Error("request_body_too_large");
    chunks.push(bytes);
  }
  return Buffer.concat(chunks, size);
}

function json(response, status, body) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
}

function error(code) {
  return { error: { code } };
}

function required(environment, name) {
  const value = environment[name];
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name}_is_required`);
  return value;
}

function portValue(value) {
  if (!/^\d{1,5}$/u.test(value)) throw new Error("invalid_fake_openrouter_port");
  const port = Number(value);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error("invalid_fake_openrouter_port");
  }
  return port;
}

async function main() {
  const { bind, port, server } = createDevelopmentFakeOpenRouter();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, bind, resolve);
  });
  process.stdout.write(
    `WorldStream fake OpenRouter listening on http://${bind}:${port} ` +
      "(VISIBLE DEVELOPMENT SUBSTITUTE)\n",
  );
}

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === process.argv[1]) {
  void main().catch((caught) => {
    const message = caught instanceof Error ? caught.message : "development_fake_openrouter_failed";
    process.stderr.write(`WorldStream fake OpenRouter failed: ${message}\n`);
    process.exitCode = 1;
  });
}
