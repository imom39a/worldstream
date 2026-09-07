import { chmod, mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { createRequire } from "node:module";
import { EMPTY_CONTROLLER_LEDGERS, EMPTY_RUNTIME_TABLES } from "../hosted-prelaunch-recovery.mjs";

const requirePack = createRequire(new URL("../../sdk/typescript-pack/packages/pack-sdk/package.json", import.meta.url));
const { blake3 } = requirePack("@noble/hashes/blake3.js");

// Synthetic authority/ledger fixture for the correspondence seam only. Runtime
// schema admission and WAL Replay are tested separately against the real CLI.
export async function seedPrelaunchFixture(root) {
  const runtime = join(root, "runtime");
  const studio = join(root, "studio");
  await mkdir(runtime, { recursive: true, mode: 0o700 });
  await mkdir(join(studio, "secrets"), { recursive: true, mode: 0o700 });
  await mkdir(join(root, "maintenance"), { recursive: true, mode: 0o700 });
  await writeFile(join(root, "maintenance", "closed"), "closed", { mode: 0o600 });
  for (const name of EMPTY_CONTROLLER_LEDGERS) await mkdir(join(studio, name), { recursive: true, mode: 0o700 });
  const secret = Buffer.alloc(32, 7);
  const reference = "a".repeat(64);
  await writeFile(join(studio, "host-authority-reference.json"), JSON.stringify({
    schema: "worldstream/studio-host-authority-binding/v1", reference,
  }), { mode: 0o600 });
  await writeFile(join(studio, "secrets", `host-${reference}.secret`), secret, { mode: 0o600 });
  for (const [file, schema] of [["managed-controller-config.v1.json", "worldstream/controller-configuration/v1"],
    ["managed-runtime-launch.v1.json", "worldstream/managed-runtime-launch/v1"]]) {
    await writeFile(join(studio, file), JSON.stringify({ schema }), { mode: 0o600 });
  }
  const database = join(runtime, "worldstream.sqlite3");
  const db = new DatabaseSync(database);
  for (const table of EMPTY_RUNTIME_TABLES) db.exec(`create table ${table}(id text);`);
  db.exec("create table principals(principal_id text, authority_status text); create table capabilities(token_hash blob, principal_id text, profile_kind text, target_room_id text, revoked_at text, expires_at text);");
  db.prepare("insert into principals values (?,?)").run("host", "enabled");
  db.prepare("insert into capabilities values (?,?,'host_operator',null,null,null)").run(
    blake3(Buffer.concat([Buffer.from("worldstream/capability-token-hash/v1\0"), secret])), "host");
  db.close();
  await chmod(database, 0o600);
  return { database, studio, runtime };
}
