/**
 * Local Vite serves the product shell, while the separately built Activity
 * Client Host owns exact client bytes.  These paths are development-only
 * routing configuration, never a platform client-selection policy.
 */
const RETAINED_CLIENT_PATHS = Object.freeze([
  "/agent-heist-v2",
  "/agent-heist-v3",
  "/agent-heist-v4",
  "/agent-heist-v5",
  // Keep the present client routable after a later reviewed declaration moves
  // to v7.  The current declaration below supplies the forward path.
  "/agent-heist-v6",
]);

interface LocalBindingDocument {
  readonly deployments?: readonly {
    readonly surfaces?: readonly { readonly launch_url?: unknown }[];
  }[];
}

/**
 * Current paths come from the reviewed local binding declaration.  Historical
 * paths remain routable so a retained local Room can still open its pinned
 * Activity Client Release after the reviewed current declaration advances.
 */
export function hostedLocalClientPaths(bindings: LocalBindingDocument): readonly string[] {
  const reviewed = new Set<string>(RETAINED_CLIENT_PATHS);
  for (const deployment of bindings.deployments ?? []) {
    for (const surface of deployment.surfaces ?? []) {
      const path = localClientPath(surface.launch_url);
      if (path !== null) reviewed.add(path);
    }
  }
  return [...reviewed].sort();
}

export function hostedLocalClientProxy(
  target: string | undefined,
  bindings: LocalBindingDocument,
): Record<string, { readonly target: string; readonly changeOrigin: true }> {
  if (target === undefined) return {};
  return Object.fromEntries(hostedLocalClientPaths(bindings).map((path) => [
    path,
    { target, changeOrigin: true as const },
  ]));
}

function localClientPath(value: unknown): string | null {
  if (typeof value !== "string") return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (
    url.protocol !== "http:" ||
    !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) ||
    url.port === "" ||
    url.username !== "" ||
    url.password !== "" ||
    url.search !== "" ||
    url.hash !== ""
  ) return null;
  const match = url.pathname.match(/^(\/[a-z0-9-]+)(?:\/|$)/u);
  return match?.[1] ?? null;
}
