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
    await readFile(resolve("../..", "config/hosted/listings/agent-heist-0.5.0.json"), "utf8"),
  );
  assert.deepEqual(
    [...activity.listing.canonicalBytes],
    [...encodeCanonical(source)],
  );
  const release = JSON.parse(await readFile(
    resolve("../..", "config/activity-clients/releases/agent-heist-web-v3.json"), "utf8",
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
  assert.equal(current.listing.value.version, "0.5.0");
  assert.notEqual(old.listing.value.client.release_digest, current.listing.value.client.release_digest);
  assert.equal(old.public.clientPath, null);
  assert.equal(old.public.availability, "dependency_unavailable");
});

test("the public catalog contains only friendly bounded product choices", () => {
  const available = listPublicHostedActivities(true);
  assert.deepEqual(available.map(({ slug }) => slug), ["agent-heist", "negotiate"]);
  assert.equal(available[0]?.availability, "available");
  assert.equal(available[0]?.clientPath, "/agent-heist-v3/hosted/");
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
