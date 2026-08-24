import { describe, expect, it, vi } from "vitest";

import {
  assignAgentProfileRevision,
  createAgentProfileRevisionDraft,
  loadAgentProfileAssignments,
  loadAgentProfiles,
  publishAgentProfile,
  type AgentProfileCatalog,
  type AgentProfilePublishRequest,
} from "./agentProfiles";
import { createRoomDraft } from "./roomDrafts";

const catalog: AgentProfileCatalog = {
  schema: "worldstream/studio-agent-profile-catalog/v1",
  profiles: [{
    profile_id: "careful-counter",
    revision: "2",
    display_name: "Careful Counter",
    non_secret_configuration: { policy: "deliberate", temperature: "0.2" },
    secret_settings: [{
      key: "MODEL_PROVIDER_TOKEN",
      kind: "model_provider",
      availability: "configured",
    }],
  }],
};

describe("Studio Agent Profile workflows", () => {
  it("loads exact immutable revisions without credential references", async () => {
    const fetcher = vi.fn(async () => new Response(JSON.stringify(catalog), { status: 200 }));
    await expect(loadAgentProfiles(fetcher)).resolves.toEqual(catalog);
    expect(fetcher).toHaveBeenCalledWith("/api/v1/agent-profiles", {
      headers: { accept: "application/json" },
    });

    const leaked = structuredClone(catalog) as unknown as Record<string, unknown>;
    const profile = (leaked.profiles as Array<Record<string, unknown>>)[0]!;
    profile.secret_settings = [{
      key: "MODEL_PROVIDER_TOKEN", kind: "model_provider", availability: "configured",
      secret_reference: "a".repeat(64),
    }];
    const unsafeFetcher = vi.fn(async () => new Response(JSON.stringify(leaked), { status: 200 }));
    await expect(loadAgentProfiles(unsafeFetcher)).resolves.toBeNull();
  });

  it("selects one exact revision only for an Agent seat and changes no authority identity", () => {
    const draft = createRoomDraft("launch-alpha");
    draft.seats = [{
      seat_id: "counter-1",
      role: "counter",
      required: true,
      display_name: "Counter",
      principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      principal_kind: "agent",
      agent_assignment: "managed",
    }];
    draft.readiness = [{ seat_id: "counter-1", role: "counter", required: true }];

    const selected = assignAgentProfileRevision(draft, "counter-1", catalog.profiles[0]!);
    expect(selected?.seats[0]?.agent_profile).toEqual({
      profile_id: "careful-counter",
      revision: "2",
    });
    expect(selected?.seats[0]).not.toHaveProperty("member_id");
    expect(selected?.seats[0]).not.toHaveProperty("runner_id");
    expect(assignAgentProfileRevision(draft, "missing", catalog.profiles[0]!)).toBeNull();
    expect(assignAgentProfileRevision({
      ...draft,
      seats: [{ ...draft.seats[0]!, principal_kind: "human", agent_assignment: undefined }],
    }, "counter-1", catalog.profiles[0]!)).toBeNull();
  });

  it("keeps exact Membership binding and Runner execution binding as separate records", async () => {
    const assignments = {
      schema: "worldstream/studio-agent-profile-assignment-catalog/v1",
      assignments: [{
        schema: "worldstream/studio-agent-profile-assignment/v1",
        assignment_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0",
        draft_id: "launch-alpha",
        seat_id: "counter-1",
        profile: { profile_id: "careful-counter", revision: "2" },
        membership: {
          room_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1",
          member_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2",
          principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FB3",
          role: "counter",
        },
        execution: { kind: "managed", runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FB4" },
      }],
    };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(assignments), { status: 200 }));
    await expect(loadAgentProfileAssignments(fetcher)).resolves.toEqual(assignments);

    const merged = structuredClone(assignments);
    Object.assign(merged.assignments[0]!.membership, { runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FB4" });
    const unsafeFetcher = vi.fn(async () => new Response(JSON.stringify(merged), { status: 200 }));
    await expect(loadAgentProfileAssignments(unsafeFetcher)).resolves.toBeNull();
  });

  it("publishes one exact revision and never accepts raw credential-shaped configuration", async () => {
    const request: AgentProfilePublishRequest = {
      schema: "worldstream/studio-agent-profile/v1",
      profile_id: "careful-counter",
      revision: "3",
      display_name: "Careful Counter",
      non_secret_configuration: { policy: "deliberate" },
      secret_settings: [{
        key: "MODEL_PROVIDER_TOKEN",
        kind: "model_provider",
        reference: "a".repeat(64),
      }],
    };
    const published = {
      ...catalog.profiles[0]!,
      revision: "3",
      non_secret_configuration: { policy: "deliberate" },
    };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(published), { status: 200 }));

    await expect(publishAgentProfile(request, fetcher)).resolves.toEqual({
      kind: "published",
      profile: published,
    });
    expect(fetcher).toHaveBeenCalledWith("/api/v1/agent-profiles", {
      method: "POST",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify(request),
    });

    const lowercaseSecret = {
      ...request,
      non_secret_configuration: { api_token: "not-configuration" },
    };
    await expect(publishAgentProfile(lowercaseSecret, fetcher)).resolves.toEqual({ kind: "rejected" });
    const bearerValue = {
      ...request,
      non_secret_configuration: { provider_header: "Bearer private-token" },
    };
    await expect(publishAgentProfile(bearerValue, fetcher)).resolves.toEqual({ kind: "rejected" });
    expect(fetcher).toHaveBeenCalledTimes(1);

    const mismatchedFetcher = vi.fn(async () => new Response(JSON.stringify({
      ...published,
      revision: "4",
    }), { status: 200 }));
    await expect(publishAgentProfile(request, mismatchedFetcher)).resolves.toEqual({
      kind: "unavailable",
    });
  });

  it("starts a new immutable revision without copying an opaque secret reference", () => {
    expect(createAgentProfileRevisionDraft(catalog.profiles[0]!)).toEqual({
      schema: "worldstream/studio-agent-profile/v1",
      profile_id: "careful-counter",
      revision: "",
      display_name: "Careful Counter",
      non_secret_configuration: { policy: "deliberate", temperature: "0.2" },
      secret_settings: [{
        key: "MODEL_PROVIDER_TOKEN",
        kind: "model_provider",
        reference: "",
      }],
    });
  });
});
