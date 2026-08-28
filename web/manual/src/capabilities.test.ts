import { describe, expect, it } from "vitest";
import { filterCapabilities, type Capability } from "./capabilities";

const capabilities: Capability[] = [
  { name: "Room commit", area: "Kernel", interface: "Rust", status: "Implemented", description: "Atomic shared-state change.", route: "/concepts/kernel" },
  { name: "Assignment MCP", area: "Agents", interface: "MCP", status: "Implemented", description: "Scoped external agent tools.", route: "/agents/mcp" },
  { name: "Public plugins", area: "Activity Packs", interface: "ABI", status: "Deferred", description: "Portable untrusted pack loading.", route: "/activity-packs/overview" },
];

describe("capability filtering", () => {
  it("combines text, area, interface, and status filters", () => {
    expect(filterCapabilities(capabilities, { query: "agent", area: "Agents", interface: "MCP", status: "Implemented" })).toHaveLength(1);
    expect(filterCapabilities(capabilities, { query: "", area: "All", interface: "All", status: "Deferred" }).map((item) => item.name)).toEqual(["Public plugins"]);
  });

  it("returns all capabilities when every filter is open", () => {
    expect(filterCapabilities(capabilities, { query: "", area: "All", interface: "All", status: "All" })).toEqual(capabilities);
  });
});
