import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HEIST_ARTWORK_LABELS, HeistArtwork } from "./HeistArtwork";
import { PlayerPrototype } from "./prototype-player/PlayerPrototype";

describe("shared Heist artwork", () => {
  it("keeps the four presentation slots and readable labels explicit", () => {
    expect(Object.keys(HEIST_ARTWORK_LABELS)).toEqual(["route", "entry_window", "required_tool", "extraction"]);
    const markup = renderToStaticMarkup(<HeistArtwork kind="route" value="service" />);
    expect(markup).toContain("Service entrance");
    expect(markup).toContain('data-art-kind="route"');
    expect(HEIST_ARTWORK_LABELS.route.roof).toBe("Rooftop");
    expect(HEIST_ARTWORK_LABELS.required_tool.brass_token).toBeUndefined();
  });

  it("does not infer private data for unknown artwork values", () => {
    const markup = renderToStaticMarkup(<HeistArtwork kind="route" value="private_clue_42" />);
    expect(markup).toContain("ROUTE");
    expect(markup).not.toContain("private_clue_42");
  });

  it("renders production practice as Mission focus without study controls", () => {
    const markup = renderToStaticMarkup(<PlayerPrototype practiceMode />);
    expect(markup).toContain("variant-A");
    expect(markup).toContain("UNTIMED PRACTICE");
    expect(markup).not.toContain("Study controls");
  });
});
