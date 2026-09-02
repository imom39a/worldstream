export type ClientHostSurface = "inspector" | "legacy";

/**
 * Selects one first-party Client Host surface from a closed pathname.
 * Activity Pack data never controls this browser-side dispatch.
 */
export function clientSurfaceForPath(pathname: string): ClientHostSurface {
  if (pathname === "/inspector" || pathname === "/inspector/") return "inspector";
  return "legacy";
}
