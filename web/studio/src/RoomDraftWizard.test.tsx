import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { RoomDraftWizard } from "./RoomDraftWizard";
import type { ActivityPackCatalog, ActivityPackDetailResponse } from "./activityPacks";
import { buildSeatPolicy, type RoomDraft } from "./roomDrafts";
import type { AgentProfileCatalog } from "./agentProfiles";
import type { RunnerTemplateCatalog } from "./runnerTemplates";

const digest = `blake3:${"a".repeat(64)}`;
const pack = { id: "counter", version: "2.0.0", digest };
const catalog: ActivityPackCatalog = {
  version: "activity_pack_catalog.v1",
  revisions: [{
    pack,
    name: "Counter",
    selectable_for_new_rooms: true,
    runnable_for_retained_rooms: true,
  }],
};
const detail: ActivityPackDetailResponse = {
  version: "activity_pack_catalog.v1",
  revision: {
    summary: catalog.revisions[0],
    roles: [
      { role: "player", minimum: 1, maximum: 2 },
      { role: "observer", minimum: 0, maximum: 1 },
    ],
    configuration_schema: {
      schema_id: "counter.config.v2",
      schema_digest: `blake3:${"b".repeat(64)}`,
      schema: {
        type: "object",
        properties: {
          title: { type: "string", minLength: 3, description: "Room title" },
          rounds: { type: "integer", minimum: 1, maximum: 10 },
        },
        required: ["title", "rounds"],
        additionalProperties: false,
      },
    },
    actions: [],
  },
};

function configuredDraft(): RoomDraft {
  const policy = buildSeatPolicy(detail.revision.roles);
  return {
    schema: "worldstream/studio-room-draft/v1",
    draft_id: "launch-alpha",
    pack,
    configuration: { title: "Alpha", rounds: 3 },
    ...policy,
    last_valid_step: "readiness",
  };
}

describe("five-step Room draft wizard", () => {
  it("renders the fixed five-step sequence and exact Activity revision", () => {
    const dom = renderToStaticMarkup(
      <RoomDraftWizard
        draft={configuredDraft()}
        catalog={catalog}
        detail={detail}
        activeStep="activity"
      />,
    );

    expect(dom).toContain("1. Activity");
    expect(dom).toContain("2. Configuration");
    expect(dom).toContain("3. Seats");
    expect(dom).toContain("4. Readiness");
    expect(dom).toContain("5. Review");
    expect(dom).toContain("Counter");
    expect(dom).toContain("counter 2.0.0");
    expect(dom).toContain(digest.slice(0, 20));
  });

  it("renders schema-derived fields and bounded pointer errors", () => {
    const dom = renderToStaticMarkup(
      <RoomDraftWizard
        draft={{ ...configuredDraft(), configuration: { title: "x", rounds: 11 } }}
        catalog={catalog}
        detail={detail}
        activeStep="configuration"
      />,
    );

    expect(dom).toContain("Room title");
    expect(dom).toContain("title");
    expect(dom).toContain("rounds");
    expect(dom).toContain("The field is shorter than allowed.");
    expect(dom).toContain("The field is above the declared maximum.");
    expect(dom).toContain("/configuration/title");
  });

  it("shows required and optional declared Role seats without live readiness claims", () => {
    const seats = renderToStaticMarkup(
      <RoomDraftWizard
        draft={configuredDraft()}
        catalog={catalog}
        detail={detail}
        activeStep="seats"
      />,
    );
    const readiness = renderToStaticMarkup(
      <RoomDraftWizard
        draft={configuredDraft()}
        catalog={catalog}
        detail={detail}
        activeStep="readiness"
      />,
    );

    expect(seats).toContain("player 1");
    expect(seats).toContain("Required");
    expect(seats).toContain("Optional");
    expect(readiness).toContain("Required before later Room creation");
    expect(readiness).toContain("Optional seat may remain unfilled");
    expect(readiness).not.toContain("Participant online");
  });

  it("keeps an unavailable saved exact revision inspectable without migration", () => {
    const unavailable = { id: "counter", version: "1.0.0", digest: `blake3:${"c".repeat(64)}` };
    const dom = renderToStaticMarkup(
      <RoomDraftWizard
        draft={{ ...configuredDraft(), pack: unavailable }}
        catalog={catalog}
        detail={null}
        activeStep="activity"
      />,
    );

    expect(dom).toContain("Saved exact revision is unavailable");
    expect(dom).toContain("counter 1.0.0");
    expect(dom).toContain(unavailable.digest.slice(0, 20));
    expect(dom).toContain("No replacement was selected");
  });

  it("reviews an immutable snapshot shape but exposes no Room or authority creation", () => {
    const dom = renderToStaticMarkup(
      <RoomDraftWizard
        draft={configuredDraft()}
        catalog={catalog}
        detail={detail}
        activeStep="review"
      />,
    );

    expect(dom).toContain("Review draft snapshot");
    expect(dom).toContain("Alpha");
    expect(dom).toContain("Save draft");
    expect(dom).toContain("does not create a Room");
    expect(dom).not.toContain("Create Room");
    expect(dom).not.toContain("Create authority");
  });

  it("keeps missing exact Profile and Runner pins visible and blocks progression", () => {
    const base = configuredDraft();
    const draft: RoomDraft = {
      ...base,
      seats: base.seats.map((seat, index) => index === 0 ? {
        ...seat,
        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        principal_kind: "agent",
        agent_assignment: "managed",
        agent_profile: { profile_id: "missing-profile", revision: "r2" },
        runner_template: { template_id: "missing-runner", revision: "r3" },
      } : seat),
    };
    const dom = renderToStaticMarkup(
      <RoomDraftWizard
        draft={draft}
        catalog={catalog}
        detail={detail}
        agentProfiles={{
          schema: "worldstream/studio-agent-profile-catalog/v1",
          profiles: [],
        } satisfies AgentProfileCatalog}
        runnerTemplates={{
          schema: "worldstream/studio-runner-template-catalog/v1",
          templates: [],
        } satisfies RunnerTemplateCatalog}
        activeStep="seats"
      />,
    );

    expect(dom).toContain("Pinned exact Profile unavailable · missing-profile · r2");
    expect(dom).toContain("Pinned exact Runner unavailable or incompatible · missing-runner · r3");
    expect(dom).toMatch(/<button[^>]*disabled=""[^>]*>Continue<\/button>/);
  });
});
