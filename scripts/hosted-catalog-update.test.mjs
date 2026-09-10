import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const overlay = "packaging/hosted/Dockerfile.catalog-update";
const deployedRuntime = "registry.fly.io/worldstream-preview@sha256:8d5a44b2d547b1f0bb970bf2d9a67ba7db4fd7ea66840e93a4e1e1f29a1f0f07";

test("the catalog overlay is pinned to the deployed r16 appliance and replaces only reviewed inputs", async () => {
  const dockerfile = await readFile(overlay, "utf8");

  assert.match(
    dockerfile,
    new RegExp(`ARG WORLDSTREAM_DEPLOYED_RUNTIME_IMAGE=${deployedRuntime.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}`, "u"),
  );
  assert.match(dockerfile, /ARG WORLDSTREAM_CONTROLLER_BUILDER_IMAGE\s+FROM \$\{WORLDSTREAM_CONTROLLER_BUILDER_IMAGE\} AS controller-builder/u);
  assert.match(dockerfile, /FROM \$\{WORLDSTREAM_DEPLOYED_RUNTIME_IMAGE\}/u);
  for (const label of [
    "io.worldstream.base-image",
    "io.worldstream.controller-builder-image",
    "io.worldstream.controller-source-revision",
  ]) assert.match(dockerfile, new RegExp(label, "u"));
  assert.doesNotMatch(dockerfile, /org\.opencontainers\.image\.revision/u,
    "the inherited Runtime source revision remains its own provenance record");

  const copies = [...dockerfile.matchAll(/^COPY(?: --from=controller-builder)? (\S+) (\S+)$/gmu)]
    .map(([, source, destination]) => ({ source, destination }));
  assert.deepEqual(copies, [
    {
      source: "/src/target/release/worldstream-studio-supervisor",
      destination: "/usr/local/bin/worldstream-studio-supervisor",
    },
    { source: "scripts/hosted-runtime.mjs", destination: "/opt/worldstream/hosted/hosted-runtime.mjs" },
    { source: "config/activity-clients/releases/agent-heist-web-v9.json", destination: "/opt/worldstream/hosted/agent-heist-web.json" },
    { source: "config/activity-clients/releases/agent-heist-web-v8.json", destination: "/opt/worldstream/hosted/agent-heist-web-v8.json" },
    { source: "config/activity-clients/releases/agent-heist-web-v7.json", destination: "/opt/worldstream/hosted/agent-heist-web-v7.json" },
    { source: "config/activity-clients/hosted-local-bindings.json", destination: "/opt/worldstream/hosted/activity-client-bindings.json" },
  ]);

  for (const retained of [
    "worldstreamd",
    "worldstreamctl",
    "worldstream-assignment-mcp",
    "worldstream-managed-agent-host",
    "worldstream-hosted-gateway",
    "worldstream-hosted-artifact-digest",
    "managed-agent-host.blake3",
  ]) assert.doesNotMatch(dockerfile, new RegExp(`COPY[^\\n]*${retained}`, "u"));
  assert.doesNotMatch(dockerfile, /cargo build|COPY \. \./u);
});

test("the overlay source has the exact Listing 0.26 catalog and v9 binding inputs", async () => {
  const [catalog, listingBytes, bindingsBytes] = await Promise.all([
    readFile("crates/worldstream-studio-supervisor/src/hosted_artifacts.rs", "utf8"),
    readFile("config/hosted/listings/agent-heist-0.26.0.json", "utf8"),
    readFile("config/activity-clients/hosted-local-bindings.json", "utf8"),
  ]);
  const listing = JSON.parse(listingBytes);
  const bindings = JSON.parse(bindingsBytes);

  assert.match(catalog, /include_bytes!\("\.\.\/\.\.\/\.\.\/config\/hosted\/listings\/agent-heist-0\.26\.0\.json"\)/u);
  assert.equal(listing.version, "0.26.0");
  assert.equal(listing.client.release_digest, "sha256:6ee6747c63cf0090a7549a1c505893c49307c475d1332eb4164c1170a97645d8");
  assert.equal(
    bindings.deployments.filter((deployment) => deployment.client_id === listing.client.client_id
      && deployment.release_digest === listing.client.release_digest).length,
    1,
  );
});
