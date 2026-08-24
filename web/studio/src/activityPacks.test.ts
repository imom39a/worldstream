import { describe, expect, it, vi } from "vitest";

import {
  loadActivityPackCatalog,
  loadActivityPackDetail,
  loadActivityPackSelection,
  saveActivityPackSelection,
  type ActivityPackCatalog,
  type StorageLike,
} from "./activityPacks";

const digestV1 = `blake3:${"1".repeat(64)}`;
const digestV2 = `blake3:${"2".repeat(64)}`;

const catalog: ActivityPackCatalog = {
  version: "activity_pack_catalog.v1",
  revisions: [
    {
      pack: { id: "counter", version: "1.0.0", digest: digestV1 },
      name: "Counter",
      selectable_for_new_rooms: false,
      runnable_for_retained_rooms: true,
    },
    {
      pack: { id: "counter", version: "2.0.0", digest: digestV2 },
      name: "Counter",
      selectable_for_new_rooms: true,
      runnable_for_retained_rooms: true,
    },
  ],
};

function memoryStorage(): StorageLike {
  const values = new Map<string, string>();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
}

describe("Activity Pack catalog client", () => {
  it("loads multiple installed exact revisions without collapsing by name", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify(catalog), { status: 200 }));

    await expect(loadActivityPackCatalog(fetcher)).resolves.toEqual(catalog);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/activity-packs", {
      headers: { accept: "application/json" },
    });
  });

  it("loads detail only through the selected exact digest", async () => {
    const detail = {
      version: "activity_pack_catalog.v1",
      revision: {
        summary: catalog.revisions[1],
        roles: [{ role: "player", minimum: 1, maximum: 4 }],
        configuration_schema: {
          schema_id: "counter.config.v2",
          schema_digest: `blake3:${"3".repeat(64)}`,
          schema: { type: "object", additionalProperties: false },
        },
        actions: [
          {
            action_type: "increment",
            payload_schema: {
              schema_id: "counter.increment.v1",
              schema_digest: `blake3:${"4".repeat(64)}`,
              schema: { type: "object" },
            },
          },
        ],
      },
    };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(detail), { status: 200 }));

    await expect(loadActivityPackDetail(digestV2, fetcher)).resolves.toEqual(detail);
    expect(fetcher).toHaveBeenCalledWith(`/api/v1/activity-packs/${digestV2}`, {
      headers: { accept: "application/json" },
    });
  });

  it("fails closed for malformed or unavailable catalog data", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify({ ...catalog, extra: true }), { status: 200 }));

    await expect(loadActivityPackCatalog(fetcher)).resolves.toBeNull();
    await expect(loadActivityPackDetail("counter@2", fetcher)).resolves.toBeNull();
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("persists and restores the complete exact reference without substituting revisions", () => {
    const storage = memoryStorage();
    saveActivityPackSelection(catalog.revisions[0].pack, storage);

    expect(loadActivityPackSelection(storage)).toEqual(catalog.revisions[0].pack);
    expect(loadActivityPackSelection(storage)?.digest).not.toBe(digestV2);

    saveActivityPackSelection(null, storage);
    expect(loadActivityPackSelection(storage)).toBeNull();
  });
});
