import { isIP } from "node:net";

// Temporary [DEBUG-vercel-boundary-v1] probe. Remove after the deployed envelope
// is captured. Never include query values, credentials, header values, or IPs.
export function describeDeploymentRequestBoundary(request) {
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
  return {
    version: "[DEBUG-vercel-boundary-v1]",
    url_scheme: url.protocol,
    url_hostname: url.hostname.slice(0, 253),
    url_port: url.port,
    url_path: url.pathname,
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
    unrecognized_forwarding_header_present: [...headers.keys()].some((name) =>
      name === "forwarded" || name.startsWith("x-original-") ||
      (name.startsWith("x-forwarded-") && !recognized.has(name))),
  };
}
