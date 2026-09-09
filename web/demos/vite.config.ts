import { fileURLToPath } from "node:url";

import { defineConfig } from "vite";
import { resolveBuildRevision } from "./buildRevision.ts";
import bindings from "../../config/activity-clients/hosted-local-bindings.json" with { type: "json" };
import { hostedLocalClientProxy } from "./src/hostedLocalClientRouting.ts";

const sourceRevision = resolveBuildRevision();
const platformBffTarget = localPlatformBffTarget(process.env.WORLDSTREAM_LOCAL_PLATFORM_BFF_TARGET);
const activityClientTarget = localActivityClientTarget(
  process.env.WORLDSTREAM_LOCAL_ACTIVITY_CLIENT_TARGET,
);

export default defineConfig({
  define: {
    __BUILD_REVISION__: JSON.stringify(sourceRevision),
  },
  build: {
    rollupOptions: {
      input: {
        catalog: fileURLToPath(new URL("./index.html", import.meta.url)),
        agentHeist: fileURLToPath(new URL("./demos/agent-heist/index.html", import.meta.url)),
        agentHeistDocs: fileURLToPath(new URL("./docs/agent-heist/index.html", import.meta.url)),
      },
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5180,
    ...(platformBffTarget === undefined
      ? {}
      : {
          proxy: {
            "/api": {
              target: platformBffTarget,
              changeOrigin: false,
            },
            ...(activityClientTarget === undefined
              ? {}
              : hostedLocalClientProxy(activityClientTarget, bindings)),
          },
        }),
  },
});

function localPlatformBffTarget(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  const url = new URL(value);
  if (
    url.protocol !== "http:" ||
    !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new Error("invalid local Platform BFF target");
  }
  return url.origin;
}

function localActivityClientTarget(value: string | undefined): string | undefined {
  return localLoopbackTarget(value, "Activity Client");
}

function localLoopbackTarget(
  value: string | undefined,
  label: string,
): string | undefined {
  if (value === undefined) return undefined;
  const url = new URL(value);
  if (
    url.protocol !== "http:" ||
    !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new Error(`invalid local ${label} target`);
  }
  return url.origin;
}
