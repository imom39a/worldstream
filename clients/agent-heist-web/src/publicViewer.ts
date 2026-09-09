const PUBLIC_ID = /^[0-9a-f]{32}$/u;

export interface PublicViewerContext {
  readonly publicId: string;
  readonly backToGames: "/";
  readonly resultPath: string;
}

/** A client accepts the platform return target only when it is the fixed root. */
export function readPlatformReturnTarget(target: Pick<Window, "location">): "/" {
  return new URLSearchParams(target.location.search).get("platform_return") === "/"
    ? "/"
    : "/";
}

/**
 * Reads only the small, server-shaped public viewer context. Query parameters
 * are untrusted browser input: accepting a different return destination would
 * turn an Activity Client launch into an open redirect.
 */
export function readPublicViewerContext(target: Pick<Window, "location">): PublicViewerContext | null {
  const params = new URLSearchParams(target.location.search);
  const publicId = params.get("public_run");
  if (
    publicId === null || !PUBLIC_ID.test(publicId) ||
    params.getAll("public_run").length !== 1 ||
    params.getAll("platform_return").length !== 1 ||
    params.getAll("platform_result").length !== 1 ||
    params.size !== 3 ||
    params.get("platform_return") !== "/" ||
    params.get("platform_result") !== `/runs/${publicId}`
  ) return null;
  return { publicId, backToGames: "/", resultPath: `/runs/${publicId}` };
}

export function isSelectedPublicViewerLaunch(
  target: Pick<Window, "location">,
  launchUrl: unknown,
): boolean {
  if (typeof launchUrl !== "string") return false;
  try {
    const selected = new URL(launchUrl);
    const current = new URL(target.location.href);
    return selected.origin === current.origin &&
      selected.pathname === current.pathname &&
      selected.search === current.search &&
      selected.hash === "";
  } catch {
    return false;
  }
}
