import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createDevelopmentPlatformServer } from "../web/platform/dist/dev-server.js";
import { PinnedResultProjectorRegistry } from "../web/platform/dist/result-reconciliation.js";
import { hostedProjectorArtifactBundles } from "../web/platform/dist/hosted-artifacts.generated.js";
import { encodeCanonical } from "../sdk/typescript-pack/packages/pack-sdk/dist/index.js";
import { internalCandidateAvailable } from "./hosted-internal-candidates.mjs";
import { readRosterFixture } from "./hosted-roster-fixture.mjs";

export async function createRosterFixturePlatform(environment = process.env) {
  assert.equal(environment.WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION, "visible-local-only");
  assert.equal(environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT, "development");
  assert.notEqual(environment.NODE_ENV, "production");
  assert.notEqual(environment.VERCEL_ENV, "production");
  const { fixture, listing, house } = await readRosterFixture();
  const reviewed = { slug: fixture.slug, listing, houseAgents: new Map([[house.digest, house]]), public: {
    slug: fixture.slug, title: listing.value.title, description: listing.value.description, availability: "available",
    availabilityMessage: "Controlled local qualification fixture", seatSummary: "Solo or one supplied assistant",
    participationKinds: ["human", "house_agent"], seats: [{ key: "seat-1", label: "Expedition lead", required: true },
      { key: "seat-2", label: "Qualification assistant", required: false }], creatorMaySpectate: false,
    houseFillAvailable: true, publicViewingAvailable: false, resultPublication: "Private terminal evidence only.",
    attribution: "Controlled local fixture; no paid provider calls or cross-run memory.",
    clientPath: "/midnight-archive-v10/hosted/", publicViewerClientPath: null,
    houseTerms: { exhibition: true, maximumAgents: 1, maximumCallsPerAgent: 10,
      maximumInputTokensPerAgent: 120000, maximumOutputTokensPerAgent: 10000, callTimeoutSeconds: 60 } } };
  const bytes = async (path) => encodeCanonical(JSON.parse(await readFile(resolve(path), "utf8")));
  const bundles = hostedProjectorArtifactBundles.map((bundle) => ({ listingBytes: Buffer.from(bundle.listingBase64, "base64"),
    projectorBytes: Buffer.from(bundle.projectorBase64, "base64"), runtimeBytes: Buffer.from(bundle.runtimeBase64, "base64"),
    projectionSchemaBytes: Buffer.from(bundle.projectionSchemaBase64, "base64"), outputSchemaBytes: Buffer.from(bundle.outputSchemaBase64, "base64") }));
  bundles.push({ listingBytes: encodeCanonical(listing.value),
    projectorBytes: await bytes("config/hosted/result-projectors/midnight-archive-0.1.0.json"),
    runtimeBytes: await bytes("config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"),
    projectionSchemaBytes: await bytes("config/hosted/schemas/midnight-archive-public-projection-v1.schema.json"),
    outputSchemaBytes: await bytes("config/hosted/schemas/midnight-archive-terminal-summary-v1.schema.json") });
  const allowed = (environment.WORLDSTREAM_LOCAL_INTERNAL_LISTING_DIGESTS ?? "").split(",").filter(Boolean);
  assert.ok(allowed.includes(listing.digest), "fixture must be explicitly allowlisted");
  return createDevelopmentPlatformServer(environment, {
    projectors: new PinnedResultProjectorRegistry(bundles),
    internalCandidates: { reviewedActivities: [reviewed], internalCandidateListingDigests: allowed,
      hostedActivityAvailable: async (digest) => {
        if (!allowed.includes(digest)) return false;
        // Exact retained Pack/client availability remains mandatory. House reservations
        // independently verify approved Profile/Template/executable and real capacity.
        return internalCandidateAvailable({ ctl: environment.WORLDSTREAM_LOCAL_CTL,
          config: environment.WORLDSTREAM_LOCAL_CONFIG, stateDirectory: environment.WORLDSTREAM_LOCAL_STATE_DIRECTORY,
          controller: environment.WORLDSTREAM_LOCAL_CONTROLLER, clientOrigin: environment.CANONICAL_ORIGIN,
          clientHostOrigin: environment.WORLDSTREAM_LOCAL_ACTIVITY_CLIENT_TARGET,
          listingDigest: digest === listing.digest ? fixture.base_listing_digest : digest });
      } },
  });
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  const { server, port, bind } = await createRosterFixturePlatform();
  server.listen(port, bind, () => console.log("Qualification BFF listening on loopback with explicit fixture allowlist."));
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => server.close());
}
