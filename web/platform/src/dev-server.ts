import { Buffer } from "node:buffer";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  createDevelopmentPlatformBff,
  DEVELOPMENT_IDENTITY_MODE,
  type PlatformBff,
} from "./bff.js";
import { HttpHostedBrowserSessionClient } from "./browser-sessions.js";
import {
  HostedFormationCoordinator,
  HttpHostedFormationGateway,
} from "./hosted-formation.js";
import {
  createHostedResultReconciler,
  withHostedResultReconciliation,
} from "./reconciliation-service.js";
import { createSupabaseBffDependencies } from "./supabase.js";

const MAX_HTTP_BODY_BYTES = 32 * 1024;

export function createDevelopmentPlatformServer(environment = process.env) {
  const bind = required(environment, "WORLDSTREAM_PLATFORM_BIND");
  if (bind !== "127.0.0.1") throw new Error("development_platform_bind_must_be_loopback");
  const port = portValue(environment.WORLDSTREAM_PLATFORM_PORT ?? "3000");
  const canonicalOrigin = required(environment, "CANONICAL_ORIGIN");
  const dependencies = createSupabaseBffDependencies({
    url: required(environment, "SUPABASE_URL"),
    publishableKey: required(environment, "SUPABASE_PUBLISHABLE_KEY"),
    dataSecretKey: required(environment, "SUPABASE_DATA_SECRET_KEY"),
  });
  const hostedBrowserSessions = hostedBrowserSessionClient(environment, canonicalOrigin);
  const hostedFormation = hostedFormationDependencies(environment, dependencies);
  const platform = createDevelopmentPlatformBff(
    {
      canonicalOrigin,
      allowedReturnTargets: ["/", "/join", "/my-games"],
      sessionKey: base64Key(required(environment, "WORLDSTREAM_SESSION_KEY_BASE64")),
      oauthKey: base64Key(required(environment, "WORLDSTREAM_OAUTH_KEY_BASE64")),
      developmentMode: exactMode(environment.WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS),
      deploymentEnvironment: exactDevelopment(environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT),
      identity: {
        authUserId: required(environment, "WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID"),
        providerSubject: required(environment, "WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT"),
        githubLogin: required(environment, "WORLDSTREAM_DEVELOPMENT_GITHUB_LOGIN"),
        avatarUrl: environment.WORLDSTREAM_DEVELOPMENT_GITHUB_AVATAR_URL ?? null,
      },
    },
    dependencies.dataClient,
    hostedBrowserSessions,
    hostedFormation,
    environment.WORLDSTREAM_HOSTED_GATEWAY_URL,
  );
  const serviceAuthority = environment.WORLDSTREAM_VERCEL_SERVICE_AUTHORITY;
  const hostedGatewayUrl = environment.WORLDSTREAM_HOSTED_GATEWAY_URL;
  const bff = serviceAuthority === undefined || hostedGatewayUrl === undefined
    ? platform
    : withHostedResultReconciliation(
        platform,
        createHostedResultReconciler({
          supabaseUrl: required(environment, "SUPABASE_URL"),
          dataSecretKey: required(environment, "SUPABASE_DATA_SECRET_KEY"),
          hostedGatewayUrl,
          serviceAuthority,
        }),
        hostedFormation === undefined
          ? undefined
          : {
              canonicalOrigin,
              cronSecret: required(
                environment,
                "WORLDSTREAM_LOCAL_RECONCILIATION_SECRET",
              ),
              recover: (launchId: string) => new HostedFormationCoordinator(
                hostedFormation.data,
                hostedFormation.gateway,
                hostedFormation.hostInstallationId,
              ).recover(launchId),
              abandonPrestart: (launchId: string) => new HostedFormationCoordinator(
                hostedFormation.data,
                hostedFormation.gateway,
                hostedFormation.hostInstallationId,
              ).abandonPrestart(launchId),
            },
      );
  const server = createServer((request, response) => {
    void dispatch(bff, canonicalOrigin, request, response);
  });
  return { bind, port, server };
}

function hostedFormationDependencies(
  environment: NodeJS.ProcessEnv,
  dependencies: ReturnType<typeof createSupabaseBffDependencies>,
) {
  const baseUrl = environment.WORLDSTREAM_HOSTED_GATEWAY_URL;
  const serviceAuthority = environment.WORLDSTREAM_VERCEL_SERVICE_AUTHORITY;
  const hostInstallationId = environment.WORLDSTREAM_HOSTED_INSTALLATION_ID;
  if (baseUrl === undefined && serviceAuthority === undefined && hostInstallationId === undefined) {
    return undefined;
  }
  if (
    baseUrl === undefined ||
    serviceAuthority === undefined ||
    hostInstallationId === undefined ||
    dependencies.hostedFormationData === undefined
  ) {
    throw new Error("hosted_formation_configuration_incomplete");
  }
  return {
    data: dependencies.hostedFormationData,
    gateway: new HttpHostedFormationGateway({ baseUrl, serviceAuthority }),
    hostInstallationId,
  };
}

function hostedBrowserSessionClient(
  environment: NodeJS.ProcessEnv,
  canonicalOrigin: string,
): HttpHostedBrowserSessionClient | undefined {
  const baseUrl = environment.WORLDSTREAM_HOSTED_GATEWAY_URL;
  const serviceAuthority = environment.WORLDSTREAM_VERCEL_SERVICE_AUTHORITY;
  if (baseUrl === undefined && serviceAuthority === undefined) return undefined;
  if (baseUrl === undefined || serviceAuthority === undefined) {
    throw new Error("hosted_browser_session_configuration_incomplete");
  }
  return new HttpHostedBrowserSessionClient({
    baseUrl,
    clientOrigin: canonicalOrigin,
    serviceAuthority,
  });
}

async function dispatch(
  bff: PlatformBff,
  canonicalOrigin: string,
  incoming: IncomingMessage,
  outgoing: ServerResponse,
): Promise<void> {
  try {
    const body = await boundedBody(incoming);
    const headers = new Headers();
    for (const [name, value] of Object.entries(incoming.headers)) {
      if (value === undefined) continue;
      headers.set(name, Array.isArray(value) ? value.join(", ") : value);
    }
    const init: RequestInit = {
      method: incoming.method ?? "GET",
      headers,
    };
    if (body.byteLength > 0) init.body = new Uint8Array(body);
    const request = new Request(new URL(incoming.url ?? "/", canonicalOrigin), init);
    await writeResponse(outgoing, await bff.fetch(request));
  } catch (error) {
    const status =
      error instanceof Error && error.message === "request_body_too_large" ? 413 : 400;
    outgoing.writeHead(status, {
      "cache-control": "private, no-store, max-age=0",
      "content-type": "application/json",
    });
    outgoing.end('{"error":{"code":"invalid_request"}}');
  }
}

async function boundedBody(request: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const value of request) {
    const chunk = Buffer.isBuffer(value) ? value : Buffer.from(value);
    size += chunk.byteLength;
    if (size > MAX_HTTP_BODY_BYTES) throw new Error("request_body_too_large");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

async function writeResponse(outgoing: ServerResponse, response: Response): Promise<void> {
  for (const [name, value] of response.headers) {
    if (name !== "set-cookie") outgoing.setHeader(name, value);
  }
  const cookies = response.headers.getSetCookie();
  if (cookies.length > 0) outgoing.setHeader("set-cookie", cookies);
  outgoing.writeHead(response.status);
  outgoing.end(Buffer.from(await response.arrayBuffer()));
}

function base64Key(value: string): Uint8Array {
  const key = Buffer.from(value, "base64");
  if (key.byteLength !== 32) throw new Error("invalid_development_encryption_key");
  return key;
}

function exactMode(value: string | undefined): typeof DEVELOPMENT_IDENTITY_MODE {
  if (value !== DEVELOPMENT_IDENTITY_MODE) {
    throw new Error("development_identity_bypass_acknowledgement_required");
  }
  return value;
}

function exactDevelopment(value: string | undefined): "development" {
  if (value !== "development") throw new Error("development_environment_required");
  return value;
}

function portValue(value: string): number {
  if (!/^\d{1,5}$/u.test(value)) throw new Error("invalid_development_platform_port");
  const port = Number(value);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error("invalid_development_platform_port");
  }
  return port;
}

function required(environment: NodeJS.ProcessEnv, name: string): string {
  const value = environment[name];
  if (value === undefined || value.length === 0) throw new Error(`${name}_is_required`);
  return value;
}

async function main(): Promise<void> {
  const { bind, port, server } = createDevelopmentPlatformServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, bind, resolve);
  });
  process.stdout.write(
    `WorldStream development BFF listening on http://${bind}:${port} ` +
      "(VISIBLE DEVELOPMENT IDENTITY BYPASS)\n",
  );
}

if (
  process.argv[1] !== undefined &&
  fileURLToPath(import.meta.url) === resolve(process.argv[1])
) {
  void main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : "development_platform_failed";
    process.stderr.write(`WorldStream development BFF failed: ${message}\n`);
    process.exitCode = 1;
  });
}
