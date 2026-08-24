import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ActivityPackCatalogView } from "./ActivityPackCatalog";
import type { ActivityPackCatalog, ActivityPackDetailResponse } from "./activityPacks";

const oldDigest = `blake3:${"a".repeat(64)}`;
const currentDigest = `blake3:${"b".repeat(64)}`;
const missingDigest = `blake3:${"c".repeat(64)}`;

const catalog: ActivityPackCatalog = {
  version: "activity_pack_catalog.v1",
  revisions: [
    {
      pack: { id: "counter", version: "1.0.0", digest: oldDigest },
      name: "Counter",
      selectable_for_new_rooms: false,
      runnable_for_retained_rooms: true,
    },
    {
      pack: { id: "counter", version: "2.0.0", digest: currentDigest },
      name: "Counter",
      selectable_for_new_rooms: true,
      runnable_for_retained_rooms: true,
    },
  ],
};

const detail: ActivityPackDetailResponse = {
  version: "activity_pack_catalog.v1",
  revision: {
    summary: catalog.revisions[1],
    roles: [{ role: "player", minimum: 1, maximum: 4 }],
    configuration_schema: {
      schema_id: "counter.config.v2",
      schema_digest: `blake3:${"d".repeat(64)}`,
      schema: { type: "object" },
    },
    actions: [
      {
        action_type: "increment",
        payload_schema: {
          schema_id: "counter.increment.v1",
          schema_digest: `blake3:${"e".repeat(64)}`,
          schema: { type: "object" },
        },
      },
    ],
  },
};

describe("Activity Pack Build view", () => {
  it("keeps same-name revisions distinct and exposes only selectable revisions", () => {
    const dom = renderToStaticMarkup(
      <ActivityPackCatalogView
        catalog={catalog}
        detail={detail}
        inspectedDigest={currentDigest}
        selection={catalog.revisions[1].pack}
      />,
    );

    expect(dom).toContain("Installed Activity Packs");
    expect(dom).toContain("1.0.0");
    expect(dom).toContain("2.0.0");
    expect(dom).toContain(oldDigest.slice(0, 20));
    expect(dom).toContain(currentDigest.slice(0, 20));
    expect(dom).toContain("Retained Rooms only");
    expect(dom).toContain("Selected for new Room drafts");
    expect(dom).toContain("player · 1–4");
    expect(dom).toContain("increment");
    expect(dom).toContain("No Lobby contract declared");
    expect((dom.match(/Select exact revision/g) ?? []).length).toBe(0);
  });

  it("shows an unavailable exact saved reference without choosing another revision", () => {
    const dom = renderToStaticMarkup(
      <ActivityPackCatalogView
        catalog={catalog}
        detail={null}
        inspectedDigest={null}
        selection={{ id: "counter", version: "3.0.0", digest: missingDigest }}
      />,
    );

    expect(dom).toContain("Saved revision is unavailable");
    expect(dom).toContain("counter 3.0.0");
    expect(dom).toContain(missingDigest.slice(0, 20));
    expect(dom).not.toContain("counter 2.0.0 is selected");
  });

  it("fails closed when the bounded Supervisor catalog is unavailable", () => {
    const dom = renderToStaticMarkup(
      <ActivityPackCatalogView
        catalog={null}
        detail={null}
        inspectedDigest={null}
        selection={null}
      />,
    );

    expect(dom).toContain("Activity Pack catalog unavailable");
    expect(dom).toContain("Configure retained Host authority");
    expect(dom).not.toContain("Select exact revision");
    expect(dom).not.toContain("Upload");
  });
});
