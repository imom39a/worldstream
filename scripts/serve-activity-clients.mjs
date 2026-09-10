import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { activityClientBuildDigest } from "./activity-client-identities.mjs";

const workspace = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const activityClientMounts = Object.freeze([
  Object.freeze({ prefix: "/agent-heist-v7/", root: resolve(workspace, "clients/agent-heist-web/dist") }),
  Object.freeze({ prefix: "/agent-heist-v6/", root: resolve(workspace, "config/activity-clients/artifacts/agent-heist-web-v6") }),
  Object.freeze({ prefix: "/agent-heist-v5/", root: resolve(workspace, "config/activity-clients/artifacts/agent-heist-web-v5") }),
  Object.freeze({ prefix: "/agent-heist-v4/", root: resolve(workspace, "config/activity-clients/artifacts/agent-heist-web-v4") }),
  Object.freeze({ prefix: "/agent-heist-v3/", root: resolve(workspace, "config/activity-clients/artifacts/agent-heist-web-v3") }),
  Object.freeze({ prefix: "/agent-heist-v2/", root: resolve(workspace, "config/activity-clients/artifacts/agent-heist-web-v2") }),
  Object.freeze({ prefix: "/negotiate-v3/", root: resolve(workspace, "clients/negotiate-web/dist") }),
  Object.freeze({ prefix: "/negotiate-v2/", root: resolve(workspace, "config/activity-clients/artifacts/negotiate-web-v2") }),
  Object.freeze({ prefix: "/negotiate/", root: resolve(workspace, "config/activity-clients/artifacts/negotiate-web-v1") }),
  Object.freeze({ prefix: "/midnight-archive-v10/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v10") }),
  Object.freeze({ prefix: "/midnight-archive-v9/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v9") }),
  Object.freeze({ prefix: "/midnight-archive-v8/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v8") }),
  Object.freeze({ prefix: "/midnight-archive-v7/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v7") }),
  Object.freeze({ prefix: "/midnight-archive-v6/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v6") }),
  Object.freeze({ prefix: "/midnight-archive-v5/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v5") }),
  Object.freeze({ prefix: "/midnight-archive-v4/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v4") }),
  Object.freeze({ prefix: "/midnight-archive-v3/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v3") }),
  Object.freeze({ prefix: "/midnight-archive-v2/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v2") }),
  Object.freeze({ prefix: "/midnight-archive-v1/", root: resolve(workspace, "config/activity-clients/artifacts/midnight-archive-web-v1") }),
  Object.freeze({ prefix: "/inspector-v2/", root: resolve(workspace, "web/console/dist") }),
  Object.freeze({ prefix: "/inspector/", root: resolve(workspace, "config/activity-clients/artifacts/inspector-web-v1") }),
  // Inspector v1 used root-relative assets. Preserve those exact bytes too.
  Object.freeze({ prefix: "/", root: resolve(workspace, "config/activity-clients/artifacts/inspector-web-v1") }),
]);

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".map", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".webp", "image/webp"],
  [".ttf", "font/ttf"],
]);

/**
 * Serves each first-party Activity Client from the exact directory retained by
 * its Release. Pack-specific clients are never imported into the Inspector or
 * another client build.
 */
export async function startActivityClientHost({
  hostname = "127.0.0.1",
  port = 5173,
  controllerOrigin = "http://127.0.0.1:9420",
} = {}) {
  if (hostname !== "127.0.0.1" && hostname !== "localhost") throw new Error("Activity Client Host must bind to loopback");
  if (!Number.isSafeInteger(port) || port < 0 || port > 65_535) throw new Error("Activity Client Host port is invalid");
  const authorizedControllerOrigin = exactLoopbackOrigin(controllerOrigin);
  await Promise.all(activityClientMounts.map(async ({ root }) => {
    await activityClientBuildDigest(root);
    await stat(resolve(root, "index.html"));
  }));
  const server = createServer((request, response) => {
    void serve(request, response, authorizedControllerOrigin).catch(() => {
      if (!response.headersSent) response.writeHead(500, commonHeaders("text/plain; charset=utf-8", authorizedControllerOrigin));
      response.end("Activity Client Host failed to read a retained artifact.\n");
    });
  });
  await new Promise((resolveListening, reject) => {
    server.once("error", reject);
    server.listen(port, hostname, resolveListening);
  });
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("Activity Client Host did not bind a TCP address");
  return {
    origin: `http://${hostname}:${address.port}`,
    close: () => new Promise((resolveClose, reject) => {
      server.close((error) => error === undefined ? resolveClose() : reject(error));
    }),
  };
}

async function serve(request, response, controllerOrigin) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.writeHead(405, { ...commonHeaders("text/plain; charset=utf-8", controllerOrigin), Allow: "GET, HEAD" });
    response.end("Method not allowed.\n");
    return;
  }
  const url = new URL(request.url ?? "/", "http://activity-client-host.invalid");
  if (url.pathname === "/agent-heist" || url.pathname.startsWith("/agent-heist/")) {
    response.writeHead(404, commonHeaders("text/plain; charset=utf-8", controllerOrigin));
    response.end("This retained Activity Client artifact is not available on this Client Host.\n");
    return;
  }
  if (url.pathname === "/") {
    response.writeHead(308, { ...commonHeaders("text/plain; charset=utf-8", controllerOrigin), Location: `/inspector-v2/${url.search}` });
    response.end();
    return;
  }
  if (activityClientMounts.some(({ prefix }) => prefix !== "/" && prefix.slice(0, -1) === url.pathname)) {
    response.writeHead(308, { ...commonHeaders("text/plain; charset=utf-8", controllerOrigin), Location: `${url.pathname}/${url.search}` });
    response.end();
    return;
  }
  const mount = activityClientMounts.find(({ prefix }) => url.pathname.startsWith(prefix));
  if (mount === undefined) {
    response.writeHead(404, commonHeaders("text/plain; charset=utf-8", controllerOrigin));
    response.end("Not found.\n");
    return;
  }
  let requested;
  try {
    requested = decodeURIComponent(url.pathname.slice(mount.prefix.length));
  } catch {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8", controllerOrigin));
    response.end("Invalid path.\n");
    return;
  }
  if (requested.includes("\0") || requested.includes("\\") || requested.split("/").includes("..")) {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8", controllerOrigin));
    response.end("Invalid path.\n");
    return;
  }
  const candidate = resolve(mount.root, requested === "" || requested.endsWith("/") ? `${requested}index.html` : requested);
  const candidateRelative = relative(mount.root, candidate);
  if (candidateRelative.startsWith("..") || candidateRelative === "") {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8", controllerOrigin));
    response.end("Invalid path.\n");
    return;
  }
  const file = await readableFile(candidate) ? candidate : resolve(mount.root, "index.html");
  const body = await readFile(file);
  response.writeHead(200, {
    ...commonHeaders(contentTypes.get(extname(file)) ?? "application/octet-stream", controllerOrigin),
    "Content-Length": String(body.byteLength),
  });
  response.end(request.method === "HEAD" ? undefined : body);
}

async function readableFile(path) {
  try {
    return (await stat(path)).isFile();
  } catch (error) {
    if (error !== null && typeof error === "object" && "code" in error && error.code === "ENOENT") return false;
    throw error;
  }
}

function commonHeaders(contentType, controllerOrigin) {
  return {
    "Cache-Control": "no-store",
    "Content-Security-Policy": `default-src 'self'; connect-src 'self' ${controllerOrigin}; style-src 'self' 'unsafe-inline'; script-src 'self'`,
    "Content-Type": contentType,
    "X-Content-Type-Options": "nosniff",
  };
}

function exactLoopbackOrigin(value) {
  if (typeof value !== "string" || value.length > 256) throw new Error("Controller origin is invalid");
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error("Controller origin is invalid");
  }
  if (
    parsed.protocol !== "http:"
    || !["127.0.0.1", "localhost", "[::1]"].includes(parsed.hostname)
    || parsed.port === ""
    || parsed.username !== ""
    || parsed.password !== ""
    || parsed.pathname !== "/"
    || parsed.search !== ""
    || parsed.hash !== ""
    || parsed.origin !== value
  ) throw new Error("Controller origin must be an exact loopback HTTP origin");
  return parsed.origin;
}

function commandOptions(values) {
  const options = {};
  for (let index = 0; index < values.length; index += 2) {
    const flag = values[index];
    const value = values[index + 1];
    if (value === undefined || value.startsWith("--")) throw new Error("Activity Client Host arguments must be --name value pairs");
    if (flag === "--hostname") options.hostname = value;
    else if (flag === "--port" && /^(?:0|[1-9][0-9]{0,4})$/.test(value) && Number(value) <= 65_535) options.port = Number(value);
    else if (flag === "--controller-origin") options.controllerOrigin = value;
    else throw new Error(`Unknown Activity Client Host argument ${flag}`);
  }
  return options;
}

const invokedPath = process.argv[1] === undefined ? null : resolve(process.argv[1]);
if (invokedPath === fileURLToPath(import.meta.url)) {
  const host = await startActivityClientHost(commandOptions(process.argv.slice(2)));
  console.log(`WorldStream Activity Client Host listening on ${host.origin}`);
}
