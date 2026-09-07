import { isIP } from "node:net";

// Temporary [DEBUG-vercel-boundary-v1] probe. Remove after the deployed envelope
// is captured. Never include query values, credentials, header values, or IPs.
export function describeDeploymentRequestBoundary(request, canonicalOrigin) {
  const url = new URL(request.url);
  if (request.method !== "GET" || url.pathname !== "/api/deployment") return null;
  const headers = request.headers;
  const host = headers.get("host");
  const forwardedHost = headers.get("x-forwarded-host");
  const forwardedProto = headers.get("x-forwarded-proto");
  const forwardedPort = headers.get("x-forwarded-port");
  const forwardedFor = headers.get("x-forwarded-for");
  const realIp = headers.get("x-real-ip");
  const vercelFor = headers.get("x-vercel-forwarded-for");
  const vercelId = headers.get("x-vercel-id");
  const recognized = new Set(["x-forwarded-host", "x-forwarded-proto", "x-forwarded-port", "x-forwarded-for"]);
  const unknownForwardingNames = [...headers.keys()].filter((name) =>
    name === "forwarded" || name.startsWith("x-original-") ||
    (name.startsWith("x-forwarded-") && !recognized.has(name)));
  let normalizedCanonicalOrigin = null;
  try { normalizedCanonicalOrigin = new URL(canonicalOrigin).origin; } catch { /* Boolean diagnosis only. */ }
  return {
    version: "[DEBUG-vercel-boundary-v1]",
    url_scheme: url.protocol,
    url_hostname: url.hostname.slice(0, 253),
    url_port: url.port,
    url_path: url.pathname,
    url_query_present: url.search !== "",
    url_query_parameter_names: [...new Set(url.searchParams.keys())].sort().slice(0, 8)
      .map((name) => /^[a-zA-Z0-9_-]{1,64}$/u.test(name) ? name : "<other-name>"),
    canonical_origin_matches_request_origin: canonicalOrigin === url.origin,
    normalized_canonical_origin_matches_request_origin: normalizedCanonicalOrigin === url.origin,
    host_present: host !== null,
    host_matches_url_host: host === url.host,
    forwarded_host_present: forwardedHost !== null,
    forwarded_host_matches_host: forwardedHost === host,
    forwarded_host_matches_url_host: forwardedHost === url.host,
    forwarded_proto_present: forwardedProto !== null,
    forwarded_proto_matches_url_scheme: forwardedProto === url.protocol.slice(0, -1),
    forwarded_port_present: forwardedPort !== null,
    forwarded_port_matches_url_port: forwardedPort === null || forwardedPort === (url.port || "443"),
    forwarded_for_present: forwardedFor !== null,
    forwarded_for_is_ip: forwardedFor !== null && isIP(forwardedFor) !== 0,
    real_ip_present: realIp !== null,
    real_ip_matches_forwarded_for: realIp === null || realIp === forwardedFor,
    vercel_forwarded_for_present: vercelFor !== null,
    vercel_forwarded_for_matches_forwarded_for: vercelFor === null || vercelFor === forwardedFor,
    vercel_id_present: vercelId !== null,
    vercel_id_valid: vercelId !== null && vercelId.length > 0 && vercelId.length <= 256 && !/[^\x21-\x7e]/u.test(vercelId),
    unrecognized_forwarding_header_present: unknownForwardingNames.length !== 0,
    unknown_forwarding_header_names: unknownForwardingNames.sort().slice(0, 8)
      .map((name) => /^[a-z0-9-]{1,64}$/u.test(name) ? name : "<other-name>"),
  };
}
