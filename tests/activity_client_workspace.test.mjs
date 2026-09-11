import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { activityClientBuildDigest } from "../scripts/activity-client-identities.mjs";
import { activityClientMounts, startActivityClientHost } from "../scripts/serve-activity-clients.mjs";

const workspace = new URL("../", import.meta.url);

async function readJson(path) {
  return JSON.parse(await readFile(new URL(path, workspace), "utf8"));
}

test("standalone Activity Client commands do not build or test Studio", async () => {
  const rootPackage = await readJson("package.json");

  for (const name of ["activity-clients:build", "activity-clients:check"]) {
    const command = rootPackage.scripts[name];
    assert.equal(typeof command, "string", `${name} must remain a workspace command`);
    assert.doesNotMatch(command, /(?:^|\s)(?:pnpm\s+)?studio(?::|\s)|web\/studio/);
  }
});

test("the standalone Console host has no Studio prototype entry point", async () => {
  const consolePackage = await readJson("web/console/package.json");
  const main = await readFile(new URL("web/console/src/main.tsx", workspace), "utf8");

  assert.equal(consolePackage.scripts["prototype:studio"], undefined);
  assert.doesNotMatch(main, /StudioPrototype|\/prototype\/studio/);
});

test("each selected Negotiate deployment target serves its declared exact artifact", async () => {
  const bootstrap = await readJson("config/activity-clients/local-bindings.json");
  const releases = await Promise.all([
    readJson("config/activity-clients/releases/negotiate-web.json"),
    readJson("config/activity-clients/releases/negotiate-web-v3.json"),
  ]);
  const releasesByDigest = new Map(releases.map((release) => [release.release_digest, release]));
  const mountsByEntrypoint = new Map(activityClientMounts.map((mount) => [mount.prefix, mount.root]));
  const selectedDeploymentIds = new Set(
    bootstrap.bindings
      .filter((binding) => binding.pack.id === "worldstream.negotiate")
      .map((binding) => binding.deployment_id),
  );
  const deployments = bootstrap.deployments.filter(
    (deployment) => selectedDeploymentIds.has(deployment.deployment_id),
  );

  assert.equal(deployments.length, 2);
  assert.deepEqual(
    deployments.flatMap((deployment) => deployment.surfaces.map(
      (surface) => new URL(surface.launch_url).pathname,
    )).sort(),
    ["/negotiate-v3/", "/negotiate/"],
  );
  for (const deployment of deployments) {
    const release = releasesByDigest.get(deployment.release_digest);
    assert.notEqual(release, undefined, `${deployment.deployment_id} Release must be imported`);
    for (const surface of deployment.surfaces) {
      const declared = release.surfaces.find((candidate) => candidate.surface_id === surface.surface_id);
      const entrypoint = new URL(surface.launch_url).pathname;
      assert.equal(declared?.entrypoint, entrypoint);
      const root = mountsByEntrypoint.get(entrypoint);
      assert.notEqual(root, undefined, `${deployment.deployment_id} must have an exact Host mount`);
      assert.equal(await activityClientBuildDigest(root), declared.artifact_digest);
    }
  }
});

test("the Activity Client Host keeps retained and current Negotiate artifacts distinct", async () => {
  const host = await startActivityClientHost({ port: 0 });
  try {
    const retained = await (await fetch(`${host.origin}/negotiate/`)).text();
    const current = await (await fetch(`${host.origin}/negotiate-v3/`)).text();
    assert.match(retained, /\/negotiate\/assets\//);
    assert.doesNotMatch(retained, /\/negotiate-v3\/assets\//);
    assert.match(current, /\/negotiate-v3\/assets\//);
    assert.notEqual(retained, current);
  } finally {
    await host.close();
  }
});

test("the Activity Client Host authorizes its exact configured Controller origin", async () => {
  const host = await startActivityClientHost({
    hostname: "127.0.0.1",
    port: 0,
    controllerOrigin: "http://127.0.0.1:19420",
  });
  try {
    const response = await fetch(`${host.origin}/agent-heist-v12/`);
    assert.equal(response.status, 200);
    const policy = response.headers.get("content-security-policy");
    assert.match(policy, /connect-src 'self' http:\/\/127\.0\.0\.1:19420(?:;|$)/);
    assert.doesNotMatch(policy, /127\.0\.0\.1:9420/);
  } finally {
    await host.close();
  }
});

test("the Host does not serve the current Heist artifact at its unavailable retained path", async () => {
  const host = await startActivityClientHost({ port: 0 });
  try {
    assert.equal((await fetch(`${host.origin}/agent-heist/`)).status, 404);
    assert.equal((await fetch(`${host.origin}/agent-heist-v12/`)).status, 200);
    assert.equal((await fetch(`${host.origin}/agent-heist-v11/`)).status, 200);
    assert.equal((await fetch(`${host.origin}/agent-heist-v8/`)).status, 200);
  } finally {
    await host.close();
  }
});

test("the Activity Client Host rejects non-loopback or non-origin Controller values", async () => {
  for (const controllerOrigin of [
    "https://example.test:19420",
    "http://127.0.0.1:19420/path",
    "http://127.0.0.1:19420; script-src *",
  ]) {
    await assert.rejects(
      startActivityClientHost({ port: 0, controllerOrigin }),
      /Controller origin/,
    );
  }
});

test("the Activity Client Host accepts the canonical IPv6 loopback Controller origin", async () => {
  const host = await startActivityClientHost({ port: 0, controllerOrigin: "http://[::1]:19420" });
  try {
    const response = await fetch(`${host.origin}/agent-heist-v12/`);
    assert.equal(response.status, 200);
    assert.match(
      response.headers.get("content-security-policy") ?? "",
      /connect-src 'self' http:\/\/\[::1\]:19420/,
    );
  } finally {
    await host.close();
  }
});

test("the Host command applies the declared Controller origin", async () => {
  const script = fileURLToPath(new URL("../scripts/serve-activity-clients.mjs", import.meta.url));
  const child = spawn(process.execPath, [
    script,
    "--port", "0",
    "--controller-origin", "http://127.0.0.1:19420",
  ], { stdio: ["ignore", "pipe", "pipe"] });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  try {
    const origin = await new Promise((resolveOrigin, rejectOrigin) => {
      child.stdout.setEncoding("utf8");
      child.stdout.on("data", (chunk) => {
        const match = chunk.match(/listening on (http:\/\/127\.0\.0\.1:[0-9]+)/);
        if (match !== null) resolveOrigin(match[1]);
      });
      child.once("exit", (code) => rejectOrigin(new Error(`Host exited ${code}: ${stderr}`)));
    });
    const response = await fetch(`${origin}/agent-heist-v12/`);
    assert.equal(response.status, 200);
    assert.match(
      response.headers.get("content-security-policy") ?? "",
      /connect-src 'self' http:\/\/127\.0\.0\.1:19420/,
    );
  } finally {
    child.kill("SIGTERM");
    if (child.exitCode === null) await once(child, "exit");
  }
});

test("retained clients still serve their original assets after the shared design release", async () => {
  const host = await startActivityClientHost({ port: 0 });
  try {
    for (const [path, releaseFile] of [
      ["/agent-heist-v11/", "agent-heist-web-v11.json"],
      ["/agent-heist-v10/", "agent-heist-web-v10.json"],
      ["/midnight-archive-v13/", "midnight-archive-web-v13.json"],
      ["/agent-heist-v8/", "agent-heist-web-v8.json"],
      ["/agent-heist-v2/", "agent-heist-web-v2.json"],
      ["/negotiate-v2/", "negotiate-web-v2.json"],
      ["/inspector/", "inspector-web.json"],
    ]) {
      const release = await readJson(`config/activity-clients/releases/${releaseFile}`);
      const mount = activityClientMounts.find((candidate) => candidate.prefix === path);
      assert.equal(await activityClientBuildDigest(mount.root), release.artifacts[0].digest);
      const html = await (await fetch(`${host.origin}${path}`)).text();
      const asset = html.match(/src="([^"]+\.js)"/)?.[1];
      assert.ok(asset);
      const response = await fetch(new URL(asset, host.origin));
      assert.match(response.headers.get("content-type"), /javascript/);
      assert.notEqual(await response.text(), html);
    }
    const root = await fetch(host.origin, { redirect: "manual" });
    assert.equal(root.status, 308);
    assert.equal(root.headers.get("location"), "/inspector-v2/");
  } finally {
    await host.close();
  }
});
