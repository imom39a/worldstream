import { describe, expect, it } from "vitest";
import workflow from "../../../.github/workflows/deploy-developer-manual.yml?raw";

function step(name: string): string {
  const section = workflow.split(`      - name: ${name}\n`)[1]?.split("\n      - name:")[0];
  expect(section, `${name} step must remain present`).toBeDefined();
  return section ?? "";
}

describe("optional GitHub Pages publication", () => {
  const optIn = "if: vars.WORLDSTREAM_DEPLOY_MANUAL_PAGES == 'true'";

  it("verifies and builds without requiring a configured Pages site", () => {
    for (const name of ["Verify developer manual", "Build developer manual"]) {
      expect(step(name)).not.toMatch(/^\s+if:/m);
    }
    expect(step("Build developer manual")).toContain("run: pnpm docs:build");
    expect(step("Configure GitHub Pages")).toContain(optIn);
  });

  it("requires the same explicit opt-in for upload and deployment", () => {
    expect(step("Upload Pages artifact")).toContain(optIn);
    const deployJob = workflow.split("\n  deploy:\n")[1]?.split("\n    steps:")[0];
    expect(deployJob).toContain(optIn);
  });
});
