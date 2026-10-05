import { describe, expect, it } from "vitest";
import { canonicalPayloadBytes, checkDeclaredArtifactReference, checkFreshPayload, PAYLOAD_BUDGET_V1_ID } from "./index";

describe("explicit fresh compact payload budgets", () => {
  it.each([-1, 0, 1])("counts canonical UTF-8 at the inclusive boundary %i", (delta) => {
    const value = "é".repeat(16382) + "x".repeat(delta + 2);
    expect(canonicalPayloadBytes(value).byteLength).toBe(32768 + delta);
    if (delta <= 0) expect(checkFreshPayload(value, "action_payload", PAYLOAD_BUDGET_V1_ID)).toBe(32768 + delta);
    else expect(() => checkFreshPayload(value, "action_payload", PAYLOAD_BUDGET_V1_ID)).toThrow();
  });
  it("sorts canonical keys by UTF-8, including integer keys and supplementary characters", () => {
    expect(new TextDecoder().decode(canonicalPayloadBytes({ "😀": false, "2": 1, "10": "é", "\uffff": null }))).toBe('{"10":"é","2":1,"\uffff":null,"😀":false}');
  });
  it("checks complete aggregate arrays independently of their items", () => {
    const items = Array(60).fill("x".repeat(5000));
    items.forEach((item) => expect(checkFreshPayload(item, "domain_event_item", PAYLOAD_BUDGET_V1_ID)).toBe(5002));
    expect(() => checkFreshPayload(items, "domain_events_array", PAYLOAD_BUDGET_V1_ID)).toThrow();
  });
  it("requires exact policy and explicit Artifact classification", () => {
    expect(() => checkFreshPayload({}, "action_payload", "future")).toThrow();
    // Runtime callers cannot bypass the closed TypeScript kind with a cast.
    expect(() => checkFreshPayload({}, "unknown" as "action_payload", PAYLOAD_BUDGET_V1_ID)).toThrow();
    expect(() => checkFreshPayload({}, "artifact_reference" as "action_payload", PAYLOAD_BUDGET_V1_ID)).toThrow();
    expect(checkFreshPayload({ artifact: "x".repeat(3000) }, "action_payload", PAYLOAD_BUDGET_V1_ID)).toBeGreaterThan(2048);
    expect(() => checkDeclaredArtifactReference({}, "", PAYLOAD_BUDGET_V1_ID)).toThrow();
    expect(checkDeclaredArtifactReference({}, "app/reference/exact-v1", PAYLOAD_BUDGET_V1_ID)).toBe(2);
  });
  it.each([1.5, Number.MAX_SAFE_INTEGER + 1, undefined, new Date(), "\ud800"]) ("rejects noncanonical values", (value) => {
    expect(() => canonicalPayloadBytes(value)).toThrow();
  });
});
