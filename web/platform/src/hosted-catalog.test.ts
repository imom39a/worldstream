import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { test } from "vitest";

import { encodeCanonical } from "@worldstream/pack-sdk";
import { deriveRoomSetup } from "@worldstream/hosted-contract";

import {
  AGENT_HEIST_LISTING_DIGEST,
  listPublicHostedActivities,
  reviewedActivityByDigest,
  reviewedActivityBySlug,
  reviewedPublicViewerClientPath,
  reviewedSeatId,
  reviewedSeatKey,
} from "./hosted-catalog.js";

test("the hosted catalog resolves only the reviewed Agent Heist revision", async () => {
  const activity = reviewedActivityBySlug("agent-heist");
  assert.ok(activity);
  assert.equal(activity.listing.digest, AGENT_HEIST_LISTING_DIGEST);
  assert.equal(reviewedActivityByDigest(AGENT_HEIST_LISTING_DIGEST), activity);
  assert.equal(reviewedActivityBySlug("worldstream.agent-heist"), null);
  assert.equal(reviewedActivityByDigest(`blake3:${"0".repeat(64)}`), null);

  const source = JSON.parse(
    await readFile(resolve("../..", "config/hosted/listings/agent-heist-0.25.0.json"), "utf8"),
  );
  assert.deepEqual(
    [...activity.listing.canonicalBytes],
    [...encodeCanonical(source)],
  );
  const release = JSON.parse(await readFile(
    resolve("../..", "config/activity-clients/releases/agent-heist-web-v8.json"), "utf8",
  ));
  assert.equal(activity.listing.value.client.release_digest, release.release_digest);
  assert.equal(activity.listing.value.client.surface_id, "heist-hosted-web");
  assert.equal(
    activity.public.clientPath,
    release.surfaces.find((surface: { surface_id: string }) => surface.surface_id === "heist-hosted-web")?.entrypoint,
  );
  assert.equal(activity.houseAgents.size, 2);
  assert.deepEqual(
    new Set(activity.houseAgents.keys()),
    new Set([
      "blake3:9788fe46953cf5c049c2dc457dac4dc5f916c45627457d4647c7e6d16c9308b9",
      "blake3:2ec02c67644b04dcdcfcd76c9bd05e56cc4a4bf3e84b549be705fb26e43d5117",
    ]),
  );
  assert.deepEqual(activity.public.seats.map(({ key }) => key), ["seat-1", "seat-2", "seat-3"]);
  const firstReviewedSeat = activity.listing.value.seats[0]?.seat_id;
  assert.ok(firstReviewedSeat);
  assert.equal(reviewedSeatId(activity, "seat-1"), firstReviewedSeat);
  assert.equal(reviewedSeatKey(activity, firstReviewedSeat), "seat-1");
  assert.equal(reviewedSeatId(activity, firstReviewedSeat), null);
  assert.equal(reviewedSeatKey(activity, "worldstream.unknown-role"), null);
});

test("client selection is a reviewed client-contract concern, not a Pack branch", () => {
  assert.equal(
    reviewedPublicViewerClientPath({ publicViewerClientPath: "/negotiate-v1/hosted/" }),
    "/negotiate-v1/hosted/",
  );
  assert.equal(reviewedPublicViewerClientPath({ publicViewerClientPath: null }), null);
  const current = reviewedActivityBySlug("agent-heist");
  assert.ok(current);
  assert.equal(reviewedPublicViewerClientPath(current.public), "/agent-heist-v8/hosted/");
});

test("new discovery retains old exact Listing resolution without replacing its client", () => {
  const old = reviewedActivityByDigest("blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956");
  const current = reviewedActivityBySlug("agent-heist");
  assert.ok(old);
  assert.ok(current);
  assert.equal(old.listing.value.version, "0.3.0");
  assert.equal(current.listing.value.version, "0.25.0");
  assert.notEqual(old.listing.value.client.release_digest, current.listing.value.client.release_digest);
  assert.equal(old.public.clientPath, null);
  assert.equal(old.public.availability, "dependency_unavailable");
});

test("the public catalog contains only friendly bounded product choices", () => {
  const available = listPublicHostedActivities(true);
  assert.deepEqual(available.map(({ slug }) => slug), ["agent-heist", "negotiate"]);
  assert.equal(available[0]?.availability, "available");
  assert.equal(available[0]?.clientPath, "/agent-heist-v8/hosted/");
  assert.equal(available[0]?.houseTerms?.maximumAgents, 2);
  assert.equal(available[1]?.availability, "coming_soon");

  const unavailable = listPublicHostedActivities(false);
  assert.equal(unavailable[0]?.availability, "dependency_unavailable");
  assert.equal(unavailable[0]?.availabilityMessage.includes("temporarily"), true);
  assert.equal(JSON.stringify(unavailable).includes(AGENT_HEIST_LISTING_DIGEST), false);
  assert.equal(JSON.stringify(unavailable).includes("worldstream.agent-heist"), false);
  assert.equal(JSON.stringify(unavailable).includes("worldstream.agent-heist.role"), false);
});

test("the first live Room retains its exact v4 client and pre-TLS House identities", () => {
  const retained = reviewedActivityByDigest("blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.7.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v4/hosted/");
  assert.equal(retained.public.publicViewerClientPath, null);
  assert.equal(retained.houseAgents.size, 2);
  assert.deepEqual([...retained.houseAgents.values()].map((r) => r.value.runner_template.revision), ["2", "2"]);
});

test("the prior live gameplay release retains its original Pack and client", () => {
  const retained = reviewedActivityByDigest("blake3:e202f7b24dbd99caeef6d8a1c904ae8131143d38af17a9512e562fe523654ed0");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.11.0");
  assert.equal(retained.listing.value.pack.version, "0.3.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v4/hosted/");
  assert.equal(retained.listing.value.client.release_digest, "sha256:12714052c8e1cac59a95b0439e8e86bf17766c8f689f5782d1dd5d4c0efd4cd6");
  assert.equal(retained.houseAgents.size, 2);
  for (const house of retained.houseAgents.values()) assert.equal(house.value.runner_template.revision, "6");
});

test("the current discovery Listing binds the current client to the schema-safe Heist Pack", async () => {
  const candidate = reviewedActivityByDigest("blake3:8be1c66c9c69a4a67800dadf8e60d66bdf8a8b9118fb3baa96b5e8cdaf272b7d");
  assert.ok(candidate);
  assert.equal(candidate, reviewedActivityBySlug("agent-heist"));
  assert.equal(candidate.listing.value.version, "0.25.0");
  assert.equal(candidate.listing.value.pack.version, "0.5.0");
  assert.equal(candidate.listing.value.pack.digest, "blake3:56449d0830d1137d69b1b7c11ed25e8f0d9b7188d40e8290c58e5a2caff2bef9");
  assert.equal(candidate.listing.value.client.release_digest, "sha256:5398514701e6f86c0eb3dd7f877d2b7b00283b540220aa61e7723126f4224895");
  assert.equal(candidate.public.clientPath, "/agent-heist-v8/hosted/");
  const source = JSON.parse(await readFile(resolve("../..", "config/hosted/listings/agent-heist-0.25.0.json"), "utf8"));
  const predecessor = JSON.parse(await readFile(resolve("../..", "config/hosted/listings/agent-heist-0.24.0.json"), "utf8"));
  assert.deepEqual([...candidate.listing.canonicalBytes], [...encodeCanonical(source)]);
  assert.deepEqual(source.pack, predecessor.pack);
  assert.notDeepEqual(source.client, predecessor.client);
  assert.deepEqual(source.result.projector, predecessor.result.projector);
  assert.deepEqual(source.result.projection, predecessor.result.projection);
  assert.deepEqual(source.result.publication, predecessor.result.publication);
  assert.deepEqual(source.room_setup, predecessor.room_setup);
});

test("Listing 0.24 retains its exact v7 public viewer and unchanged game dependencies", () => {
  const retained = reviewedActivityByDigest("blake3:71805434c2530094d3a575336cb0a44d71b411ccb089e37f142d9764af860397");
  const current = reviewedActivityBySlug("agent-heist");
  assert.ok(retained);
  assert.ok(current);
  assert.equal(retained.listing.value.version, "0.24.0");
  assert.equal(retained.listing.value.client.release_digest, "sha256:e1efd39ff8da4cddaa48e87ed4333d2c16fb71dd5ad0321f1245ac0a55aad33c");
  assert.equal(retained.public.clientPath, "/agent-heist-v7/hosted/");
  assert.equal(retained.public.publicViewerClientPath, "/agent-heist-v7/hosted/");
  assert.deepEqual(retained.listing.value.pack, current.listing.value.pack);
  assert.deepEqual(retained.listing.value.result, current.listing.value.result);
  assert.deepEqual(new Set(retained.houseAgents.keys()), new Set(current.houseAgents.keys()));
});

test("the r11 Listing remains resolvable after the r12 successor advances discovery", () => {
  const retained = reviewedActivityByDigest("blake3:21d7d5439208df0b1dbb18f7f42f3a3687d248a523b03fb5b4184b2dd0dcb626");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.19.0");
  assert.deepEqual(
    [...retained.houseAgents.values()].map((house) => house.value.runner_template.revision),
    ["11", "11"],
  );
});

test("the r12 Listing remains resolvable after the r13 successor advances discovery", () => {
  const retained = reviewedActivityByDigest("blake3:1cf75abcb30d77fdbe0abc5e39813a315bea6900c61e9b49c51b84d995335d74");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.20.0");
  assert.deepEqual(
    [...retained.houseAgents.values()].map((house) => house.value.runner_template.revision),
    ["12", "12"],
  );
});

test("the r13 Listing remains resolvable after the approval-profile successor", () => {
  const retained = reviewedActivityByDigest("blake3:3cdaaa7b2402b816ded0b36d5419f405b1be1428b37c89155a805d39bf826069");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.21.0");
  assert.deepEqual(
    [...retained.houseAgents.values()].map((house) => house.value.runner_template.revision),
    ["13", "13"],
  );
  assert.deepEqual(
    new Set(retained.houseAgents.keys()),
    new Set([
      "blake3:bb9c56ffe925a130fe64c61386ca9f3d0638967719cc3dfed98d6ba06eead7fe",
      "blake3:5f718a17c4de50e72c67e441dec37c628582b2bf6d6bad2a6e10b6ef356a4a4d",
    ]),
  );
});

test("the retained r9 Listing remains resolvable across every reviewed surface", () => {
  const retained = reviewedActivityByDigest("blake3:ab6d61d35786e51e3849c68467aad664bd35cb41f299b86d1bcce3f52e4249db");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.16.0");
  assert.deepEqual(
    [...retained.houseAgents.values()].map((house) => house.value.runner_template.revision),
    ["9", "9"],
  );
});

test("the r10 Listing remains resolvable with its original House identities", () => {
  const retained = reviewedActivityByDigest("blake3:5b0993de4c858771cce34b16cb25e03b2bf509cbe16cd1ce7249a789ea8c426f");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.18.0");
  assert.deepEqual(
    [...retained.houseAgents.values()].map((house) => house.value.runner_template.revision),
    ["10", "10"],
  );
});

test("the prior Heist 0.3 discovery Listing remains pinned to v5", () => {
  const retained = reviewedActivityByDigest("blake3:a12a29ad0382c7053a14295a60d017fbdc817c24e513521f85be982f5ec07dfc");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.13.0");
  assert.equal(retained.listing.value.pack.version, "0.3.0");
  assert.equal(retained.listing.value.client.release_digest, "sha256:4228b6f0cd6f7fdb19fe03e2e8b997a0d6ced8b0d2069c67952afb1ddceeb0ca");
  assert.equal(retained.public.clientPath, "/agent-heist-v5/hosted/");
  assert.equal(retained.public.publicViewerClientPath, null);
});

test("retained 0.12 formation can derive its current House revision", () => {
  const retained = reviewedActivityByDigest("blake3:10135b2b12664dec3fc51a23c917d8468474af93b9c20ee5ed068b61e2dee61c");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.12.0");
  const house = retained.houseAgents.get("blake3:5a826962c0f09c40a1b760d2c0eec9af216b9eb703b2e6f971fc24e32f0564d1");
  assert.ok(house);
  const launch = encodeCanonical({
    schema: "worldstream/launch-request/v2",
    listing_revision_digest: retained.listing.digest,
    inputs: {},
    creator: { participation: "seat", principal_reference: "account:creator" },
  });
  const roster = encodeCanonical({
    schema: "worldstream/frozen-roster/v1",
    listing_revision_digest: retained.listing.digest,
    members: [
      {
        seat_id: "navigator",
        participation: "account_human",
        principal_reference: "account:creator",
        display_name: "Navigator",
      },
      {
        seat_id: "insider",
        participation: "house_agent_fill",
        principal_reference: "house:retained:insider",
        display_name: house.value.display_name,
        house_agent_revision_digest: house.digest,
        agent_profile: {
          profile_id: house.value.agent_profile.profile_id,
          revision: house.value.agent_profile.revision,
        },
        runner_template: {
          template_id: house.value.runner_template.template_id,
          revision: house.value.runner_template.revision,
        },
      },
    ],
  });
  const setup = deriveRoomSetup(retained.listing, launch, roster, [...retained.houseAgents.values()]);
  const setupValue = JSON.parse(new TextDecoder().decode(setup));
  assert.equal(setupValue.pack.version, "0.4.0");
});

test("retained Rooms keep the original v2 client after the design release", () => {
  const retained = reviewedActivityByDigest("blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.4.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v2/hosted/");
  assert.equal(retained.public.availability, "available");
  assert.equal(retained.listing.value.client.release_digest, "sha256:493260d162be2bdfda28e71b6e4f94d5ef5c11e7f03adfbee3ab9295bdf4594b");
});

test("discovery uses schema-safe House successors while retained Listings keep their original routes", () => {
  const current = reviewedActivityBySlug("agent-heist");
  const retained = reviewedActivityByDigest("blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1");
  assert.ok(current);
  assert.ok(retained);
  assert.equal(current.listing.value.version, "0.25.0");
  assert.equal(retained.listing.value.version, "0.5.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v3/hosted/");
  assert.notDeepEqual(current.listing.value.client, retained.listing.value.client);
  assert.equal(current.listing.value.pack.version, "0.5.0");
  assert.equal(retained.listing.value.pack.version, "0.2.0");
  assert.deepEqual(current.listing.value.result.publication, retained.listing.value.result.publication);
  const strategies = [...current.houseAgents.values()];
  assert.equal(strategies.length, 2);
  assert.equal(new Set(strategies.map(({ digest }) => digest)).size, 2);
  assert.equal(new Set(strategies.map(({ value }) => value.behavior_policy.instructions)).size, 2);
  for (const strategy of strategies) {
    assert.equal(strategy.value.route.model_slug, "ibm-granite/granite-4.2-8b-20260831");
    assert.equal(strategy.value.route.provider_slug, "deepinfra/bf16");
    assert.equal(strategy.value.route.zero_data_retention, true);
    assert.equal(strategy.value.route.data_collection, "deny");
    assert.equal(strategy.value.allowance.model_call_attempts, 10);
  }
  const planner = strategies.find(({ value }) => value.house_agent_id === "worldstream.house.cooperative-planner");
  assert.equal(planner?.value.agent_profile.revision, "17");
  assert.equal(planner?.value.version, "17");
  assert.equal(planner?.value.runner_template.revision, "16");
  const oldPlanner = [...retained.houseAgents.values()].find(({ value }) => value.house_agent_id === "worldstream.house.cooperative-planner");
  assert.equal(oldPlanner?.value.route.model_slug, "qwen/qwen3.8-flash-20260826");
  assert.equal(oldPlanner?.value.agent_profile.revision, "1");
  assert.equal(planner?.value.behavior_policy.revision, "3");
  assert.equal(oldPlanner?.value.behavior_policy.revision, "1");
  assert.equal(planner?.value.behavior_policy.policy_id, oldPlanner?.value.behavior_policy.policy_id);
  assert.deepEqual(planner?.value.allowance, oldPlanner?.value.allowance);
  for (const seat of current.listing.value.seats) {
    assert.deepEqual(new Set(seat.allowed_house_agent_revisions), new Set(strategies.map(({ digest }) => digest)));
  }
});

test("the last deployed Listing retains its client, Pack, and exact House definitions", () => {
  const retained = reviewedActivityByDigest("blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.6.0");
  assert.equal(retained.listing.value.pack.version, "0.2.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v3/hosted/");
  assert.equal(retained.listing.value.client.release_digest, "sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9");
  assert.equal(retained.houseAgents.size, 2);
  for (const house of retained.houseAgents.values()) assert.equal(house.value.runner_template.revision, "1");
  for (const seat of retained.listing.value.seats) {
    assert.deepEqual(new Set(seat.allowed_house_agent_revisions), new Set(retained.houseAgents.keys()));
  }
});
