import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const workspace = resolve(fileURLToPath(new URL("..", import.meta.url)));
const mounts = Object.freeze([
  Object.freeze({ prefix: "/agent-heist/", root: resolve(workspace, "clients/agent-heist-web/dist") }),
  Object.freeze({ prefix: "/negotiate/", root: resolve(workspace, "clients/negotiate-web/dist") }),
  Object.freeze({ prefix: "/", root: resolve(workspace, "web/console/dist") }),
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
]);

/**
 * Serves each first-party Activity Client from the exact directory retained by
 * its Release. Pack-specific clients are never imported into the Inspector or
 * Studio build.
 */
export async function startActivityClientHost({ hostname = "127.0.0.1", port = 5173 } = {}) {
  await Promise.all(mounts.map(({ root }) => stat(resolve(root, "index.html"))));
  const server = createServer((request, response) => {
    void serve(request, response).catch(() => {
      if (!response.headersSent) response.writeHead(500, commonHeaders("text/plain; charset=utf-8"));
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

async function serve(request, response) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.writeHead(405, { ...commonHeaders("text/plain; charset=utf-8"), Allow: "GET, HEAD" });
    response.end("Method not allowed.\n");
    return;
  }
  const url = new URL(request.url ?? "/", "http://activity-client-host.invalid");
  if (url.pathname === "/agent-heist" || url.pathname === "/negotiate") {
    response.writeHead(308, { ...commonHeaders("text/plain; charset=utf-8"), Location: `${url.pathname}/${url.search}` });
    response.end();
    return;
  }
  const mount = mounts.find(({ prefix }) => url.pathname.startsWith(prefix));
  if (mount === undefined) {
    response.writeHead(404, commonHeaders("text/plain; charset=utf-8"));
    response.end("Not found.\n");
    return;
  }
  let requested;
  try {
    requested = decodeURIComponent(url.pathname.slice(mount.prefix.length));
  } catch {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8"));
    response.end("Invalid path.\n");
    return;
  }
  if (requested.includes("\0") || requested.includes("\\") || requested.split("/").includes("..")) {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8"));
    response.end("Invalid path.\n");
    return;
  }
  const candidate = resolve(mount.root, requested || "index.html");
  const candidateRelative = relative(mount.root, candidate);
  if (candidateRelative.startsWith("..") || candidateRelative === "") {
    response.writeHead(400, commonHeaders("text/plain; charset=utf-8"));
    response.end("Invalid path.\n");
    return;
  }
  const file = await readableFile(candidate) ? candidate : resolve(mount.root, "index.html");
  const body = await readFile(file);
  response.writeHead(200, {
    ...commonHeaders(contentTypes.get(extname(file)) ?? "application/octet-stream"),
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

function commonHeaders(contentType) {
  return {
    "Cache-Control": "no-store",
    "Content-Security-Policy": "default-src 'self'; connect-src 'self' http://127.0.0.1:9420; style-src 'self' 'unsafe-inline'; script-src 'self'",
    "Content-Type": contentType,
    "X-Content-Type-Options": "nosniff",
  };
}

const invokedPath = process.argv[1] === undefined ? null : resolve(process.argv[1]);
if (invokedPath === fileURLToPath(import.meta.url)) {
  const host = await startActivityClientHost();
  console.log(`WorldStream Activity Client Host listening on ${host.origin}`);
}
