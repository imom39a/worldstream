import assert from "node:assert/strict";
import { test } from "node:test";
import { verifyPublicDeployment } from "./hosted-deployed-acceptance.mjs";

test("deployed verification rejects a different source, schema, artifact, or gateway", async () => {
  const evidence = { commit: "a".repeat(40), deployment: {
    platform_revision: "dpl_fixture", gateway_revision: "a".repeat(40), schema_head: "20260907132124",
    listing_revision_digest: "listing", pack_digest: "pack", client_release_digest: "client", projector_digest: "projector",
  } };
  const observed = { version: "hosted_deployment_identity.v1", commit: evidence.commit,
    ...evidence.deployment, gateway_origin: "https://gateway.example" };
  const publicId = "b".repeat(32);
  const fetcher = (identity, gatewayRevision) => async (url) => {
    if (url.endsWith("/api/deployment")) return Response.json(identity);
    if (url.endsWith("/version")) return Response.json({ version: "hosted_gateway_deployment.v1", deployment: gatewayRevision });
    if (url.endsWith("/api/catalog")) return Response.json({ activities: [{ availability: "available" }] });
    if (url.endsWith("/api/ws")) return new Response(null, { status: 404 });
    if (url.includes("/api/auth/")) return new Response(null, { status: 302, headers: { location: "https://project.supabase.co/auth/v1/authorize?provider=github" } });
    if (url.includes("/api/runs/")) return Response.json({ state: "result", public_id: publicId, evidence: { class: "exhibition_platform_house_agents" } });
    return Response.json({ results: [{ public_id: publicId }] });
  };
  await verifyPublicDeployment("https://arena.example", publicId, evidence, fetcher(observed, evidence.commit));
  for (const field of ["commit", "platform_revision", "schema_head", "listing_revision_digest", "pack_digest", "client_release_digest", "projector_digest"]) {
    await assert.rejects(() => verifyPublicDeployment("https://arena.example", publicId, evidence,
      fetcher({ ...observed, [field]: "different" }, evidence.commit)), /revision_mismatch/u);
  }
  await assert.rejects(() => verifyPublicDeployment("https://arena.example", publicId, evidence,
    fetcher(observed, "different")), /gateway_revision_mismatch/u);
});
