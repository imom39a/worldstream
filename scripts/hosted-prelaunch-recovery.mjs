import { lstat, readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createRequire } from "node:module";

// Reuse the Pack SDK's pinned implementation of the kernel's capability hash.
// This check compares authority; it never exports secret bytes or their digest.
const requirePack = createRequire(new URL("../sdk/typescript-pack/packages/pack-sdk/package.json", import.meta.url));
const { blake3 } = requirePack("@noble/hashes/blake3.js");

export const EMPTY_CONTROLLER_LEDGERS = Object.freeze([
  "hosted-launches", "hosted-public-relays", "hosted-house-runners/reservations",
  "hosted-house-runners/launch-bindings", "hosted-house-runners/runtime-bindings",
  "hosted-house-runners/units", "room-creations", "task-setups", "agent-profiles/assignments",
  "assignment-mcp-progress", "assignment-mcp-launches", "managed-agent-hosts/operations",
  "managed-agent-hosts/turns/profiles", "runner-attention/restarts", "attention-inbox",
  "room-drafts", "task-templates/usages",
]);

export const EMPTY_RUNTIME_TABLES = Object.freeze([
  "rooms", "room_genesis", "room_materializations", "room_members", "room_integrity",
  "transitions", "timers", "room_snapshots", "observation_frames", "observation_consequences",
  "activation_decisions", "activation_intents", "activation_operation_receipts",
  "semantic_receipts", "external_input_preparations", "runners", "runner_capability_memberships",
]);

export const EMPTY_PLATFORM_TABLES = Object.freeze([
  "launch_requests", "capacity_reservations", "seat_invitations", "seat_claims",
  "house_fill_operations", "house_fill_candidate_evidence", "house_runner_reservations",
  "house_agent_assignments", "activity_runs", "activity_run_memberships",
  "reconciliation_receipts", "reconciliation_attempts", "activity_run_terminal_evidence",
  "indexed_activity_results", "indexed_activity_result_payloads", "activity_run_index_events",
  "public_projection_relay_bindings",
]);

async function protectedPath(path, directory = false) {
  const info = await lstat(path);
  if (info.isSymbolicLink() || (directory ? !info.isDirectory() : !info.isFile()) ||
      (info.mode & 0o077) !== 0 || (!directory && (info.nlink !== 1 || info.size > 1_048_576))) {
    throw new Error("prelaunch_protected_controller_state_required");
  }
  return info;
}

async function protectedDirectory(root, relative = "") {
  await protectedPath(root, true);
  let path = root;
  for (const part of relative.split("/").filter(Boolean)) {
    path = join(path, part);
    await protectedPath(path, true);
  }
  return path;
}

async function metadata(studio, file, schema) {
  const path = join(studio, file);
  await protectedPath(path);
  const value = JSON.parse(await readFile(path, "utf8"));
  if (value?.schema !== schema) throw new Error("prelaunch_initialized_controller_required");
  return value;
}

// Deliberately narrow: this is not a populated-installation correspondence
// verifier. Any retained activity fails closed, even with a ready Runtime.
export async function verifyPrelaunchController({ isolatedDirectory, runtimeReadiness }) {
  if (runtimeReadiness?.rooms_replayed !== 0 || runtimeReadiness?.isolated_rooms_skipped !== 0) {
    throw new Error("populated_checkpoint_correspondence_required");
  }
  const studio = await protectedDirectory(isolatedDirectory, "studio");
  for (const ledger of EMPTY_CONTROLLER_LEDGERS) {
    const path = await protectedDirectory(studio, ledger);
    if ((await readdir(path)).length !== 0) throw new Error("populated_checkpoint_correspondence_required");
  }
  const maintenance = await protectedDirectory(isolatedDirectory, "maintenance");
  const markers = await readdir(maintenance);
  const fence = markers.includes("recovery-house-calls-fenced") ? "recovery-house-calls-fenced" : "closed";
  if (!markers.includes(fence)) throw new Error("prelaunch_controller_not_closed");
  await protectedPath(join(maintenance, fence));
  await metadata(studio, "managed-controller-config.v1.json", "worldstream/controller-configuration/v1");
  await metadata(studio, "managed-runtime-launch.v1.json", "worldstream/managed-runtime-launch/v1");
  const binding = await metadata(studio, "host-authority-reference.json", "worldstream/studio-host-authority-binding/v1");
  if (!/^[0-9a-f]{64}$/u.test(binding.reference)) throw new Error("prelaunch_initialized_controller_required");
  const secrets = await protectedDirectory(studio, "secrets");
  const hostFile = `host-${binding.reference}.secret`;
  for (const name of await readdir(secrets)) {
    if (name !== hostFile && !/^model-provider-[0-9a-f]{64}\.secret$/u.test(name)) {
      throw new Error("populated_checkpoint_correspondence_required");
    }
    await protectedPath(join(secrets, name));
  }
  const secretPath = join(secrets, hostFile);
  if ((await protectedPath(secretPath)).size !== 32) throw new Error("prelaunch_initialized_controller_required");
  const secret = await readFile(secretPath);
  let database;
  try {
    const runtime = await protectedDirectory(isolatedDirectory, "runtime");
    // The production CLI has already admitted and replayed this disposable copy.
    // Opening read-only here cannot manufacture a missing Runtime as an empty DB.
    database = new DatabaseSync(join(runtime, "worldstream.sqlite3"), { readOnly: true });
    for (const table of EMPTY_RUNTIME_TABLES) {
      if (database.prepare(`select count(*) as count from ${table}`).get().count !== 0) {
        throw new Error("populated_checkpoint_correspondence_required");
      }
    }
    const tokenHash = blake3(Buffer.concat([Buffer.from("worldstream/capability-token-hash/v1\0"), secret]));
    const match = database.prepare(`select count(*) as count from capabilities c
      join principals p on p.principal_id = c.principal_id
      where c.token_hash = ? and c.profile_kind = 'host_operator' and c.target_room_id is null
      and c.revoked_at is null and c.expires_at is null and p.authority_status = 'enabled'`).get(tokenHash);
    if (match.count !== 1) throw new Error("controller_runtime_authority_mismatch");
  } finally {
    secret.fill(0);
    database?.close();
  }
  return {
    runtime_activity_records: 0, controller_activity_records: 0,
    controller_host_authority: "matched_enabled_runtime_host",
    retained_controller_admission: fence === "closed" ? "maintenance_closed" : "recovery_fenced",
  };
}

export function validatePrelaunchPlatform(value, installationId) {
  if (value?.installation_id !== installationId || value.launches_open !== false ||
      value.house_fill_open !== false || (value.maintenance_mode !== true && value.recovery_fenced !== true)) {
    throw new Error("prelaunch_platform_not_closed");
  }
  if (EMPTY_PLATFORM_TABLES.some((table) => value.counts?.[table] !== 0)) {
    throw new Error("populated_checkpoint_correspondence_required");
  }
  return {
    installation_id: installationId, platform_activity_records: 0,
    retained_platform_admission: value.recovery_fenced ? "recovery_fenced" : "maintenance_closed",
  };
}
