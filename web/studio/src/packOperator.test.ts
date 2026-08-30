import { describe, expect, it } from "vitest";

import {
  EMPTY_PACK_OPERATOR_WORKFLOW,
  PackOperatorReceiptError,
  applyPackOperatorReceipt,
  commandForPackOperatorPhase,
  computeStartupPackInventoryDigest,
  confirmPackDaemonStopped,
  readPackOperatorReceipt,
  type PackInventoryEntry,
  type PackInventoryReceipt,
  type PackReadinessReceipt,
  type PackStorageProfile,
} from "./packOperator";

const bundle = `blake3:${"a".repeat(64)}`;
const revision = `blake3:${"b".repeat(64)}`;
const deploymentBinding = `blake3:${"e".repeat(64)}`;
const common = { schema: "worldstream/pack-operator-receipt/v1" as const };

function inventoryEntry(installState: PackInventoryEntry["install_state"]): PackInventoryEntry {
  return {
    pack_id: "worldstream.negotiate",
    explanatory_version: "0.1.0",
    bundle_digest: bundle,
    revision_digest: revision,
    install_state: installState,
  };
}

function inventoryReceipt(
  installState: PackInventoryEntry["install_state"],
  storageProfile: PackStorageProfile = "sqlite-bundled",
): PackInventoryReceipt {
  const entries = [inventoryEntry(installState)];
  const selectable = installState === "selectable" ? 1 : 0;
  return {
    ...common,
    status: "complete",
    operation: "inventory",
    installed: entries.length,
    selectable,
    retained_only: entries.length - selectable,
    storage_profile: storageProfile,
    inventory_digest: computeStartupPackInventoryDigest(entries),
    entries,
    restart_required_for_pending_changes: true,
  };
}

function readinessReceipt(
  overrides: Partial<PackReadinessReceipt> = {},
): PackReadinessReceipt {
  return {
    ...common,
    status: "ready",
    operation: "restart_readiness",
    embedded_revisions: 6,
    installed_bundles: 1,
    installed_selectable: 1,
    installed_retained_only: 0,
    storage_profile: "sqlite-bundled",
    deployment_binding: deploymentBinding,
    inventory_digest: inventoryReceipt("selectable").inventory_digest,
    total_revisions: 7,
    original_bytes_reverified: true,
    production_component_host_admission: true,
    room_replay_checked: true,
    rooms_replayed: 3,
    isolated_rooms_skipped: 1,
    ...overrides,
  };
}

function completedWorkflow(storageProfile: PackStorageProfile = "sqlite-bundled") {
  let state = applyPackOperatorReceipt(EMPTY_PACK_OPERATOR_WORKFLOW, {
    ...common, status: "complete", operation: "inspect", pack_id: "worldstream.negotiate",
    explanatory_version: "0.1.0", bundle_digest: bundle, revision_digest: revision,
    member_count: 9, byte_count: 1024, approval_imported: false,
  });
  state = confirmPackDaemonStopped(state);
  state = applyPackOperatorReceipt(state, {
    ...common, status: "complete", operation: "approve", bundle_digest: bundle,
    restart_required: false, approval_transferred: false,
  });
  state = applyPackOperatorReceipt(state, {
    ...common, status: "complete", operation: "install", bundle_digest: bundle,
    revision_digest: revision, install_state: "retained_only", restart_required: true,
    approval_transferred: false,
  });
  state = applyPackOperatorReceipt(state, inventoryReceipt("retained_only", storageProfile));
  state = applyPackOperatorReceipt(state, {
    ...common, status: "complete", operation: "set_selectable", bundle_digest: bundle,
    revision_digest: revision, install_state: "selectable", restart_required: true,
    approval_transferred: false,
  });
  return applyPackOperatorReceipt(state, inventoryReceipt("selectable", storageProfile));
}

describe("Studio exact Pack operator workflow", () => {
  it("binds all lifecycle receipts to one physical, semantic, inventory, and storage identity", () => {
    const beforeReadiness = completedWorkflow();
    expect(beforeReadiness.phase).toBe("awaiting_readiness");
    expect(commandForPackOperatorPhase(beforeReadiness)).toBe(
      "worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness",
    );
    const ready = applyPackOperatorReceipt(beforeReadiness, readinessReceipt());
    expect(ready.phase).toBe("ready");
    expect(commandForPackOperatorPhase(ready)).toContain("immutable startup registry");
  });

  it("uses the configured profile and asks for direct-admin material only for PostgreSQL Replay", () => {
    expect(commandForPackOperatorPhase(EMPTY_PACK_OPERATOR_WORKFLOW)).toContain(
      "worldstreamctl --config <WORLDSTREAM_CONFIG> pack inspect",
    );
    expect(commandForPackOperatorPhase(completedWorkflow("postgres-primary"))).toBe(
      "worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness --dsn-file <POSTGRES_ADMIN_DSN_FILE>",
    );
  });

  it("rejects reordering and readiness without executable Replay", () => {
    expect(() => applyPackOperatorReceipt(EMPTY_PACK_OPERATOR_WORKFLOW, {
      ...common, status: "complete", operation: "approve", bundle_digest: bundle,
      restart_required: false, approval_transferred: false,
    })).toThrow(PackOperatorReceiptError);
    expect(() => applyPackOperatorReceipt(completedWorkflow(), readinessReceipt({
      room_replay_checked: false,
      rooms_replayed: 0,
      isolated_rooms_skipped: 0,
    }))).toThrow("did not execute Replay");
  });

  it("binds readiness to the post-selection exact inventory", () => {
    expect(() => applyPackOperatorReceipt(completedWorkflow(), readinessReceipt({
      inventory_digest: inventoryReceipt("retained_only").inventory_digest,
    }))).toThrow("different post-selection Pack inventory");
  });

  it("binds readiness to the selected inventory storage profile", () => {
    expect(() => applyPackOperatorReceipt(
      completedWorkflow("postgres-primary"),
      readinessReceipt({ storage_profile: "sqlite-bundled" }),
    )).toThrow("different configured storage profile");
  });

  it("requires an explicit stopped-daemon boundary before local mutation", () => {
    const inspected = applyPackOperatorReceipt(EMPTY_PACK_OPERATOR_WORKFLOW, {
      ...common, status: "complete", operation: "inspect", pack_id: "worldstream.negotiate",
      explanatory_version: "0.1.0", bundle_digest: bundle, revision_digest: revision,
      member_count: 9, byte_count: 1024, approval_imported: false,
    });
    const approval = {
      ...common, status: "complete" as const, operation: "approve" as const,
      bundle_digest: bundle, restart_required: false, approval_transferred: false as const,
    };
    expect(commandForPackOperatorPhase(inspected)).toContain("Stop worldstreamd");
    expect(() => applyPackOperatorReceipt(inspected, approval)).toThrow("Stop worldstreamd");
    const stopped = confirmPackDaemonStopped(inspected);
    expect(commandForPackOperatorPhase(stopped)).toContain("pack approve");
    expect(applyPackOperatorReceipt(stopped, approval).phase).toBe("awaiting_install");
  });

  it("accepts only exact pathless receipt shapes", () => {
    const receipt = {
      ...common, status: "complete", operation: "inspect", pack_id: "worldstream.negotiate",
      explanatory_version: "0.1.0", bundle_digest: bundle, revision_digest: revision,
      member_count: 9, byte_count: 1024, approval_imported: false,
    };
    expect(readPackOperatorReceipt(receipt)?.operation).toBe("inspect");
    expect(readPackOperatorReceipt(inventoryReceipt("selectable"))?.operation).toBe("inventory");
    expect(readPackOperatorReceipt(readinessReceipt())?.operation).toBe("restart_readiness");
    expect(readPackOperatorReceipt({ ...receipt, path: "/private/bundle.wspack" })).toBeNull();
    expect(readPackOperatorReceipt({ ...receipt, bundle_digest: `blake3:${"c".repeat(63)}x` })).toBeNull();
  });

  it("recomputes the Rust startup identity from lexical canonical JSON", () => {
    expect(computeStartupPackInventoryDigest([inventoryEntry("retained_only")])).toBe(
      "blake3:75fc83c5c2d8602f90283074d698111ff145db7dc5179cda2b9442bd440bfbf1",
    );
    expect(computeStartupPackInventoryDigest([inventoryEntry("selectable")])).toBe(
      "blake3:cfff5faf36f0fa0a252f1192209adf0941eadd7242c041993f954f319dec78b0",
    );
  });

  it("rejects inventory counts that contradict the entry install states", () => {
    const receipt = inventoryReceipt("selectable");
    expect(readPackOperatorReceipt({ ...receipt, selectable: 0, retained_only: 1 })).toBeNull();
  });

  it("rejects unsorted startup inventory even when its digest covers that order", () => {
    const lowerEntry: PackInventoryEntry = {
      ...inventoryEntry("retained_only"),
      bundle_digest: `blake3:${"0".repeat(64)}`,
      revision_digest: `blake3:${"1".repeat(64)}`,
    };
    const entries = [inventoryEntry("selectable"), lowerEntry];
    const receipt = {
      ...inventoryReceipt("selectable"),
      installed: 2,
      selectable: 1,
      retained_only: 1,
      entries,
      inventory_digest: computeStartupPackInventoryDigest(entries),
    };
    expect(readPackOperatorReceipt(receipt)).toBeNull();
  });

  it("rejects a declared inventory digest that does not cover the exact entries", () => {
    const receipt = inventoryReceipt("selectable");
    expect(readPackOperatorReceipt({
      ...receipt,
      inventory_digest: `blake3:${"f".repeat(64)}`,
    })).toBeNull();
  });

  it("rejects unknown storage profiles and malformed deployment bindings", () => {
    expect(readPackOperatorReceipt({
      ...inventoryReceipt("selectable"),
      storage_profile: "ephemeral",
    })).toBeNull();
    expect(readPackOperatorReceipt({
      ...readinessReceipt(),
      deployment_binding: `blake3:${"e".repeat(63)}x`,
    })).toBeNull();
  });
});
