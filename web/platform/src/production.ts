import { Buffer } from "node:buffer";

import {
  createVercelPlatformBff,
  type PlatformBff,
} from "./bff.js";
import { HttpHostedBrowserSessionClient } from "./browser-sessions.js";
import { HostedFormationCoordinator, HttpHostedFormationGateway } from "./hosted-formation.js";
import {
  createHostedResultReconciler,
  withHostedResultReconciliation,
} from "./reconciliation-service.js";
import { createSupabaseBffDependencies, createSupabaseSchemaHeadReader } from "./supabase.js";
import { withDeploymentIdentity } from "./deployment-identity.js";

const DEVELOPMENT_ONLY_VARIABLES = [
  "WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS",
  "WORLDSTREAM_DEVELOPMENT_AUTH_USER_ID",
  "WORLDSTREAM_DEVELOPMENT_GITHUB_SUBJECT",
  "WORLDSTREAM_DEVELOPMENT_GITHUB_LOGIN",
  "WORLDSTREAM_DEVELOPMENT_GITHUB_AVATAR_URL",
  "WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER",
  "WORLDSTREAM_DEVELOPMENT_OPENROUTER_KEY",
  "WORLDSTREAM_FAKE_OPENROUTER_BIND",
  "WORLDSTREAM_FAKE_OPENROUTER_PORT",
  "WORLDSTREAM_LOCAL_PLATFORM_BFF_TARGET",
  "WORLDSTREAM_LOCAL_ACTIVITY_CLIENT_TARGET",
] as const;

/**
 * Builds the production Vercel control-plane function from server-only
 * environment. It deliberately has no WebSocket route or development fallback.
 */
export function createProductionPlatformBff(
  environment: NodeJS.ProcessEnv = process.env,
): PlatformBff {
  assertProductionEnvironment(environment);
  const canonicalOrigin = required(environment, "CANONICAL_ORIGIN");
  const hostedGatewayUrl = required(environment, "WORLDSTREAM_HOSTED_GATEWAY_URL");
  const serviceAuthority = required(
    environment,
    "WORLDSTREAM_VERCEL_SERVICE_AUTHORITY",
  );
  const hostInstallationId = required(
    environment,
    "WORLDSTREAM_HOSTED_INSTALLATION_ID",
  );
  const supabaseUrl = required(environment, "SUPABASE_URL");
  const dataSecretKey = required(environment, "SUPABASE_DATA_SECRET_KEY");
  const supabase = createSupabaseBffDependencies({
    url: supabaseUrl,
    publishableKey: required(environment, "SUPABASE_PUBLISHABLE_KEY"),
    dataSecretKey,
  });
  const hostedFormationData = supabase.hostedFormationData;
  if (hostedFormationData === undefined) {
    throw new Error("hosted_formation_configuration_incomplete");
  }
  const hostedFormationGateway = new HttpHostedFormationGateway({ baseUrl: hostedGatewayUrl, serviceAuthority });
  const platform = createVercelPlatformBff(
    {
      canonicalOrigin,
      allowedReturnTargets: ["/", "/join", "/my-games"],
      sessionKey: exactBase64Key(
        required(environment, "WORLDSTREAM_SESSION_KEY_BASE64"),
      ),
      oauthKey: exactBase64Key(
        required(environment, "WORLDSTREAM_OAUTH_KEY_BASE64"),
      ),
    },
    {
      ...supabase,
      hostedBrowserSessions: new HttpHostedBrowserSessionClient({
        baseUrl: hostedGatewayUrl,
        clientOrigin: canonicalOrigin,
        serviceAuthority,
      }),
      hostedFormationData,
      hostedFormationGateway,
      hostedFormationHostInstallationId: hostInstallationId,
      hostedPublicStreamBaseUrl: hostedGatewayUrl,
    },
  );
  return withHostedResultReconciliation(
    withDeploymentIdentity(platform, {
      canonicalOrigin,
      commit: environment.VERCEL_GIT_COMMIT_SHA,
      platformRevision: environment.VERCEL_DEPLOYMENT_ID,
      gatewayOrigin: new URL(hostedGatewayUrl).origin,
      readSchemaHead: createSupabaseSchemaHeadReader(supabaseUrl, dataSecretKey),
    }),
    createHostedResultReconciler({
      supabaseUrl,
      dataSecretKey,
      hostedGatewayUrl,
      serviceAuthority,
    }),
    {
      canonicalOrigin,
      cronSecret: required(environment, "CRON_SECRET"),
      recover: (launchId) => new HostedFormationCoordinator(
        hostedFormationData, hostedFormationGateway, hostInstallationId,
      ).recover(launchId),
      abandonPrestart: (launchId) => new HostedFormationCoordinator(
        hostedFormationData, hostedFormationGateway, hostInstallationId,
      ).abandonPrestart(launchId),
    },
  );
}

function assertProductionEnvironment(environment: NodeJS.ProcessEnv): void {
  if (
    environment.NODE_ENV !== "production" ||
    environment.VERCEL_ENV !== "production" ||
    environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT !== "production" ||
    DEVELOPMENT_ONLY_VARIABLES.some((name) => environment[name] !== undefined)
  ) {
    throw new Error("production_platform_configuration_required");
  }
}

function exactBase64Key(value: string): Uint8Array {
  const decoded = Buffer.from(value, "base64");
  if (decoded.byteLength !== 32 || decoded.toString("base64") !== value) {
    throw new Error("invalid_production_encryption_key");
  }
  return decoded;
}

function required(environment: NodeJS.ProcessEnv, name: string): string {
  const value = environment[name];
  if (
    value === undefined ||
    value.length === 0 ||
    value.length > 4096 ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    throw new Error(`${name.toLowerCase()}_required`);
  }
  return value;
}
