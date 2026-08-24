import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { AgentProfileBuild } from "./AgentProfileBuild";
import type { AgentProfileCatalog } from "./agentProfiles";

const catalog: AgentProfileCatalog = {
  schema: "worldstream/studio-agent-profile-catalog/v1",
  profiles: [{
    profile_id: "careful-counter",
    revision: "2",
    display_name: "Careful Counter",
    non_secret_configuration: { policy: "deliberate" },
    secret_settings: [{
      key: "MODEL_PROVIDER_TOKEN",
      kind: "model_provider",
      availability: "configured",
    }],
  }],
};

describe("Agent Profile Build workflow", () => {
  it("offers immutable publication and an explicit new-revision path", () => {
    const dom = renderToStaticMarkup(<AgentProfileBuild catalog={catalog} />);

    expect(dom).toContain("Agent Profiles");
    expect(dom).toContain("Careful Counter");
    expect(dom).toContain("Revision 2");
    expect(dom).toContain("Start new revision");
    expect(dom).toContain("Publish immutable revision");
    expect(dom).toContain("Model provider secret reference");
    expect(dom).toContain("reference is used only for publication and is not returned to the browser");
    expect(dom).not.toContain("a".repeat(64));
  });

  it("fails closed when the profile catalog is unavailable", () => {
    const dom = renderToStaticMarkup(<AgentProfileBuild catalog={null} />);

    expect(dom).toContain("Agent Profile catalog unavailable");
    expect(dom).toContain("Publishing is disabled");
    expect(dom).not.toContain("Publish immutable revision");
  });
});
