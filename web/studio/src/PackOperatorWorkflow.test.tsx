import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { PackOperatorWorkflow } from "./PackOperatorWorkflow";

describe("Pack operator experience", () => {
  it("guides the offline typed-receipt workflow without becoming a mutation API", () => {
    const dom = renderToStaticMarkup(<PackOperatorWorkflow />);
    expect(dom).toContain("Approve one exact bundle offline");
    expect(dom).toContain("worldstreamctl --config &lt;WORLDSTREAM_CONFIG&gt; pack");
    expect(dom).toContain("inspect --bundle &lt;BUNDLE.wspack&gt;");
    expect(dom).toContain("Studio never receives a path, bundle bytes, shell");
    expect(dom).not.toContain("type=\"file\"");
    expect(dom).not.toContain("Upload bundle");
    expect(dom).not.toContain("Submit Action");
    expect(dom).not.toContain("I stopped worldstreamd");
  });
});
