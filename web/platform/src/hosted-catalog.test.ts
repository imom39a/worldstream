import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { test } from "vitest";

import { encodeCanonical } from "@worldstream/pack-sdk";

import {
  AGENT_HEIST_LISTING_DIGEST,
  listPublicHostedActivities,
  reviewedActivityByDigest,
  reviewedActivityBySlug,
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
    await readFile(resolve("../..", "config/hosted/listings/agent-heist-0.7.0.json"), "utf8"),
  );
  assert.deepEqual(
    [...activity.listing.canonicalBytes],
    [...encodeCanonical(source)],
  );
  const release = JSON.parse(await readFile(
    resolve("../..", "config/activity-clients/releases/agent-heist-web-v4.json"), "utf8",
  ));
  assert.equal(activity.listing.value.client.release_digest, release.release_digest);
  assert.equal(activity.listing.value.client.surface_id, "heist-hosted-web");
  assert.equal(
    activity.public.clientPath,
    release.surfaces.find((surface: { surface_id: string }) => surface.surface_id === "heist-hosted-web")?.entrypoint,
  );
  assert.equal(activity.houseAgents.size, 2);
  assert.deepEqual(activity.public.seats.map(({ key }) => key), ["seat-1", "seat-2", "seat-3"]);
  const firstReviewedSeat = activity.listing.value.seats[0]?.seat_id;
  assert.ok(firstReviewedSeat);
  assert.equal(reviewedSeatId(activity, "seat-1"), firstReviewedSeat);
  assert.equal(reviewedSeatKey(activity, firstReviewedSeat), "seat-1");
  assert.equal(reviewedSeatId(activity, firstReviewedSeat), null);
  assert.equal(reviewedSeatKey(activity, "worldstream.unknown-role"), null);
});

test("new discovery retains old exact Listing resolution without replacing its client", () => {
  const old = reviewedActivityByDigest("blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956");
  const current = reviewedActivityBySlug("agent-heist");
  assert.ok(old);
  assert.ok(current);
  assert.equal(old.listing.value.version, "0.3.0");
  assert.equal(current.listing.value.version, "0.7.0");
  assert.notEqual(old.listing.value.client.release_digest, current.listing.value.client.release_digest);
  assert.equal(old.public.clientPath, null);
  assert.equal(old.public.availability, "dependency_unavailable");
});

test("the public catalog contains only friendly bounded product choices", () => {
  const available = listPublicHostedActivities(true);
  assert.deepEqual(available.map(({ slug }) => slug), ["agent-heist", "negotiate"]);
  assert.equal(available[0]?.availability, "available");
  assert.equal(available[0]?.clientPath, "/agent-heist-v4/hosted/");
  assert.equal(available[0]?.houseTerms?.maximumAgents, 2);
  assert.equal(available[1]?.availability, "coming_soon");

  const unavailable = listPublicHostedActivities(false);
  assert.equal(unavailable[0]?.availability, "dependency_unavailable");
  assert.equal(unavailable[0]?.availabilityMessage.includes("temporarily"), true);
  assert.equal(JSON.stringify(unavailable).includes(AGENT_HEIST_LISTING_DIGEST), false);
  assert.equal(JSON.stringify(unavailable).includes("worldstream.agent-heist"), false);
  assert.equal(JSON.stringify(unavailable).includes("worldstream.agent-heist.role"), false);
});

test("retained Rooms keep the original v2 client after the design release", () => {
  const retained = reviewedActivityByDigest("blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80");
  assert.ok(retained);
  assert.equal(retained.listing.value.version, "0.4.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v2/hosted/");
  assert.equal(retained.public.availability, "available");
  assert.equal(retained.listing.value.client.release_digest, "sha256:493260d162be2bdfda28e71b6e4f94d5ef5c11e7f03adfbee3ab9295bdf4594b");
});

test("discovery uses two Granite strategies while retained Listings keep their original House routes", () => {
  const current = reviewedActivityBySlug("agent-heist");
  const retained = reviewedActivityByDigest("blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1");
  assert.ok(current);
  assert.ok(retained);
  assert.equal(current.listing.value.version, "0.7.0");
  assert.equal(retained.listing.value.version, "0.5.0");
  assert.equal(retained.public.clientPath, "/agent-heist-v3/hosted/");
  assert.notDeepEqual(current.listing.value.client, retained.listing.value.client);
  assert.equal(current.listing.value.pack.version, "0.3.0");
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
  assert.equal(planner?.value.agent_profile.revision, "3");
  assert.equal(planner?.value.version, "3");
  assert.equal(planner?.value.runner_template.revision, "2");
  const oldPlanner = [...retained.houseAgents.values()].find(({ value }) => value.house_agent_id === "worldstream.house.cooperative-planner");
  assert.equal(oldPlanner?.value.route.model_slug, "qwen/qwen3.8-flash-20260826");
  assert.equal(oldPlanner?.value.agent_profile.revision, "1");
  assert.deepEqual(planner?.value.behavior_policy, oldPlanner?.value.behavior_policy);
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
