import type { PlatformBff } from "./bff.js";
import { AGENT_HEIST_LISTING_DIGEST, reviewedActivityByDigest } from "./hosted-catalog.js";

/** Public release identifiers only; never serialize the process environment. */
export function withDeploymentIdentity(platform: PlatformBff, input: {
  readonly canonicalOrigin: string;
  readonly commit: string | undefined;
  readonly platformRevision: string | undefined;
  readonly gatewayOrigin: string;
  readonly readSchemaHead: () => Promise<string>;
}): PlatformBff {
  return {
    async fetch(request) {
      const url = new URL(request.url);
      if (url.pathname !== "/api/deployment") return platform.fetch(request);
      const headers = { "cache-control": "no-store", "x-content-type-options": "nosniff" };
      if (request.method !== "GET" || url.origin !== input.canonicalOrigin || url.search !== "") {
        return Response.json({ error: { code: "not_found" } }, { status: 404, headers });
      }
      if (input.commit === undefined || !/^(?!0{40}$)[0-9a-f]{40}$/u.test(input.commit) ||
          input.platformRevision === undefined || !/^dpl_[A-Za-z0-9]+$/u.test(input.platformRevision)) {
        return Response.json({ error: { code: "deployment_identity_unavailable" } }, { status: 503, headers });
      }
      try {
        const activity = reviewedActivityByDigest(AGENT_HEIST_LISTING_DIGEST);
        if (activity === null) throw new Error("listing_unavailable");
        const schemaHead = await input.readSchemaHead();
        if (!/^\d{14}$/u.test(schemaHead)) throw new Error("schema_identity_unavailable");
        return Response.json({
          version: "hosted_deployment_identity.v1",
          commit: input.commit,
          platform_revision: input.platformRevision,
          gateway_origin: input.gatewayOrigin,
          schema_head: schemaHead,
          listing_revision_digest: activity.listing.digest,
          pack_digest: activity.listing.value.pack.digest,
          client_release_digest: activity.listing.value.client.release_digest,
          projector_digest: activity.listing.value.result.projector.digest,
        }, { headers });
      } catch {
        return Response.json({ error: { code: "deployment_identity_unavailable" } }, { status: 503, headers });
      }
    },
  };
}
