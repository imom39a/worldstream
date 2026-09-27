import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("responsive and accessible client shell", () => {
  it("declares a phone viewport and touch-sized controls", async () => {
    const [html, hosted, styles] = await Promise.all([
      readFile(resolve(import.meta.dirname, "../index.html"), "utf8"),
      readFile(resolve(import.meta.dirname, "../hosted/index.html"), "utf8"),
      readFile(resolve(import.meta.dirname, "styles.css"), "utf8"),
    ]);
    expect(html).toContain('name="viewport"');
    expect(hosted).toContain('name="viewport"');
    expect(styles).toMatch(/@media \(max-width: 480px\)/u);
    expect(styles).toMatch(/\.candidate-card button, \.action-card button[\s\S]*min-height: 46px/u);
    expect(styles).toMatch(/\.commit-button[\s\S]*min-height: 64px/u);
    expect(styles).toContain("prefers-reduced-motion");
  });

  it("contains no production fixture or optimistic success path", async () => {
    const files = ["MidnightArchiveClient.tsx", "liveAdapter.ts", "main.tsx", "standalone.tsx"];
    const source = (await Promise.all(files.map((file) => (
      readFile(resolve(import.meta.dirname, file), "utf8")
    )))).join("\n");
    expect(source).not.toContain('from "./testFixtures"');
    expect(source).not.toMatch(/mockResolvedValue|demo projection/iu);
    expect(source).toContain("HostedLiveSessionController");
    expect(source).toContain("reduceMidnightArchiveObservation");
  });
});
