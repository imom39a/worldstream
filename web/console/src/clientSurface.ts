export type ClientHostSurface = "agent-heist" | "inspector" | "legacy";

/**
 * Selects one first-party Client Host surface from a closed pathname.
 * Activity Pack data never controls this browser-side dispatch.
 */
export function clientSurfaceForPath(pathname: string): ClientHostSurface {
  if (pathname === "/agent-heist" || pathname === "/agent-heist/") return "agent-heist";
  if (pathname === "/inspector" || pathname === "/inspector/") return "inspector";
  return "legacy";
}
