import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";

const repository = new URL("..", import.meta.url).pathname;
const runNamespace = `concurrency-${randomUUID()}`;
const heistListing = "blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1";
const houseHeistListing = "blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956";
const singleListing = `blake3:${sha256(runNamespace)}`;
const houseHost = `house-concurrency-${sha256(runNamespace).slice(0, 16)}`;
const packDigest = `blake3:${"2".repeat(64)}`;
const accounts = Array.from({ length: 22 }, () => randomUUID());
const createdLaunches = [];

const status = parseEnv(execFileSync("supabase", ["status", "-o", "env"], {
  cwd: repository,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "ignore"],
}));
const databaseUrl = required(status, "DB_URL");
const restUrl = required(status, "REST_URL");
const serviceKey = required(status, "SERVICE_ROLE_KEY");

try {
  psql(`
    insert into platform_store.platform_accounts(account_id)
    values ${accounts.map((id) => `('${id}')`).join(",")};
    insert into platform_store.activity_listing_revisions (
      listing_revision_digest, listing_key, canonical_document,
      pack_revision_digest, client_release_digest, client_surface_id,
      catalog_visibility, creator_access, public_viewing_policy,
      result_publication_policy, result_projector_revision_digest,
      public_projection_schema, public_projection_schema_digest,
      result_output_schema, result_output_schema_digest,
      result_canonicalizer_version, result_output_max_bytes,
      room_setup_configuration, seat_templates,
      allow_multiple_seats_per_account, pre_start_deadline_seconds
    ) values (
      '${singleListing}', 'worldstream.test.${sha256(runNamespace).slice(0, 16)}',
      convert_to('{}', 'utf8'), '${packDigest}',
      'sha256:${"3".repeat(64)}', 'test-web', 'private',
      'must_claim_seat', 'disabled', 'disabled',
      'blake3:${"4".repeat(64)}', 'test/projection/v1',
      'blake3:${"5".repeat(64)}', 'test/result/v1',
      'blake3:${"6".repeat(64)}', 'worldstream/canonical-json/v1',
      1024, '{}'::jsonb,
      '[{"seat_id":"host","role":"host","display_name":"Host","required":true,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
      false, 1800
    );
    insert into platform_store.house_agent_host_approvals (
      host_installation_id, house_agent_revision_digest,
      agent_profile_revision_digest, runner_template_revision_digest,
      runner_executable_digest, named_credential_reference,
      approval_receipt_digest, available_for_new_assignments
    )
    select '${houseHost}', revisions.house_agent_revision_digest,
      'blake3:${"1".repeat(64)}', 'blake3:${"2".repeat(64)}',
      'blake3:${"3".repeat(64)}', 'openrouter-house',
      extensions.digest(convert_to('concurrency:' || revisions.house_agent_revision_digest, 'utf8'), 'sha256'),
      true
    from platform_store.house_agent_revisions revisions;
    notify pgrst, 'reload schema';
  `);

  const idempotentBody = launchBody({
    accountId: accounts[0],
    listingDigest: heistListing,
    namespace: runNamespace,
    key: "identical-launch",
    seatId: "navigator",
  });
  const identical = await Promise.all(
    Array.from({ length: 16 }, () => rpc("create_launch_request_v1", idempotentBody)),
  );
  const successfulIdentical = identical.map(expectSingleRow);
  const retainedLaunchIds = new Set(successfulIdentical.map((row) => stringField(row, "launch_request_id")));
  assert(retainedLaunchIds.size === 1, "concurrent identical launches returned different requests");
  assert(
    successfulIdentical.filter((row) => row.was_created === true).length === 1,
    "concurrent identical launches did not have exactly one creator",
  );
  createdLaunches.push([...retainedLaunchIds][0]);

  const invitation = expectSingleRow(await rpc("rotate_seat_invitation_v1", {
    p_creator_account_id: accounts[0],
    p_launch_request_id: createdLaunches[0],
    p_seat_id: "insider",
  }));
  const invitationToken = stringField(invitation, "invitation_token");
  const tokenDigest = bytea(sha256Bytes(invitationToken));
  const claims = await Promise.all(
    accounts.slice(1, 9).map((accountId) => rpcOutcome("claim_invited_seat_v1", {
      p_claiming_account_id: accountId,
      p_invitation_token_digest: tokenDigest,
      p_participation_kind: "account_external_agent",
    })),
  );
  assert(claims.filter((outcome) => outcome.ok).length === 1, "concurrent invitation claims did not have one winner");

  const houseLaunch = expectSingleRow(await rpc("create_launch_request_v1", launchBody({
    accountId: accounts[20],
    listingDigest: houseHeistListing,
    namespace: runNamespace,
    key: "house-launch",
    seatId: "navigator",
    houseFillChoice: "fill_unclaimed",
  })));
  const houseLaunchId = stringField(houseLaunch, "launch_request_id");
  createdLaunches.push(houseLaunchId);
  const starts = await Promise.all(
    Array.from({ length: 16 }, () => rpc("start_house_fill_v1", {
      p_creator_account_id: accounts[20],
      p_launch_request_id: houseLaunchId,
    })),
  );
  const fillOperationIds = new Set(starts.map((value) =>
    stringField(expectRecord(value), "house_fill_operation_id")
  ));
  assert(fillOperationIds.size === 1, "concurrent House starts returned different operations");
  psql(`
    set session_replication_role = replica;
    update platform_store.house_fill_operations
    set claim_window_opened_at = claim_window_opened_at - interval '31 seconds',
        claim_window_closes_at = claim_window_closes_at - interval '31 seconds'
    where launch_request_id = '${houseLaunchId}';
    set session_replication_role = origin;
  `);
  const selections = await Promise.all(
    Array.from({ length: 16 }, () => rpc("retain_house_fill_selection_v1", {
      p_launch_request_id: houseLaunchId,
      p_host_installation_id: houseHost,
    })),
  );
  const reservationSets = new Set(selections.map((value) => {
    const operation = expectRecord(value);
    assert(operation.state === "reserving", "concurrent House selection did not retain reserving state");
    const reservations = operation.reservations;
    assert(Array.isArray(reservations) && reservations.length === 2, "House selection did not retain two reservations");
    return JSON.stringify(reservations.map((reservation) =>
      stringField(expectRecord(reservation), "reservation_operation_id")
    ).sort());
  }));
  assert(reservationSets.size === 1, "concurrent House selection rerolled reservation identities");

  const capacityLaunches = [];
  for (let index = 0; index < 11; index += 1) {
    const accountId = accounts[index + 9];
    const launch = expectSingleRow(await rpc("create_launch_request_v1", launchBody({
      accountId,
      listingDigest: singleListing,
      namespace: runNamespace,
      key: `gate-${index}`,
      seatId: "host",
    })));
    const launchId = stringField(launch, "launch_request_id");
    createdLaunches.push(launchId);
    capacityLaunches.push({ accountId, launchId, operation: `operation-${index}` });
    const roster = Buffer.from(JSON.stringify({
      schema: "worldstream/frozen-roster/v1",
      listing_revision_digest: singleListing,
      members: [{
        seat_id: "host",
        participation: "account_human",
        principal_reference: "seat:host",
      }],
    }));
    const setup = Buffer.from(JSON.stringify({
      schema: "worldstream/room-setup/v2",
      pack: { digest: packDigest },
      seats: [],
      spectators: [{ purpose: "result_indexer" }],
      operator_view: false,
    }));
    await rpc("freeze_launch_request_v1", {
      p_creator_account_id: accountId,
      p_launch_request_id: launchId,
      p_frozen_roster: bytea(roster),
      p_frozen_roster_digest: bytea(sha256Bytes(roster)),
      p_frozen_room_setup_specification: bytea(setup),
      p_frozen_room_setup_specification_digest: `blake3:${"7".repeat(64)}`,
      p_host_installation_id: "host-local",
      p_room_setup_operation_id: `operation-${index}`,
    });
  }

  const authorizations = await Promise.all(capacityLaunches.map((launch) =>
    rpcOutcome("authorize_host_mutation_v1", {
      p_creator_account_id: launch.accountId,
      p_launch_request_id: launch.launchId,
      p_host_installation_id: "host-local",
      p_room_setup_operation_id: launch.operation,
    })
  ));
  assert(authorizations.filter((outcome) => outcome.ok).length === 10, "the global gate did not admit exactly ten concurrent Runs");
  assert(authorizations.filter((outcome) => !outcome.ok).length === 1, "the global gate did not reject exactly one concurrent Run");

  console.log("Formation concurrency verified: one launch, one invitation winner, one House draw, and ten global Run reservations.");
} finally {
  cleanup();
}

function launchBody({ accountId, listingDigest, namespace, key, seatId, houseFillChoice = "disabled" }) {
  const canonicalInput = Buffer.from("{}");
  return {
    p_creator_account_id: accountId,
    p_listing_revision_digest: listingDigest,
    p_idempotency_namespace: namespace,
    p_idempotency_key_digest: bytea(sha256Bytes(key)),
    p_canonical_launch_input: bytea(canonicalInput),
    p_launch_input_digest: bytea(sha256Bytes(canonicalInput)),
    p_canonicalizer_version: "worldstream/canonical-json/v1",
    p_house_fill_choice: houseFillChoice,
    p_creator_access_choice: "seat",
    p_creator_seat_id: seatId,
    p_creator_participation_kind: "account_human",
  };
}

async function rpc(name, body) {
  const response = await fetch(`${restUrl}/rpc/${name}`, {
    method: "POST",
    headers: {
      apikey: serviceKey,
      authorization: `Bearer ${serviceKey}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const payload = await response.json().catch(() => null);
  if (!response.ok) {
    const code = payload && typeof payload === "object" ? payload.code : "unknown";
    throw new Error(`${name} failed with ${response.status}/${String(code)}`);
  }
  return payload;
}

async function rpcOutcome(name, body) {
  try {
    return { ok: true, value: await rpc(name, body) };
  } catch (error) {
    return { ok: false, error };
  }
}

function expectSingleRow(value) {
  assert(Array.isArray(value) && value.length === 1 && isRecord(value[0]), "RPC did not return one row");
  return value[0];
}

function expectRecord(value) {
  assert(isRecord(value), "RPC did not return an object");
  return value;
}

function stringField(record, name) {
  const value = record[name];
  assert(typeof value === "string" && value.length > 0, `RPC omitted ${name}`);
  return value;
}

function cleanup() {
  const quotedAccounts = accounts.map((id) => `'${id}'`).join(",");
  psql(`
    set session_replication_role = replica;
    delete from platform_store.house_agent_assignments where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.house_runner_reservations where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.house_fill_candidate_evidence where house_fill_operation_id in (
      select house_fill_operation_id from platform_store.house_fill_operations
      where launch_request_id in (
        select launch_request_id from platform_store.launch_requests
        where idempotency_namespace = '${runNamespace}'
      )
    );
    delete from platform_store.house_fill_operations where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.seat_invitations where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.seat_claims where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.capacity_reservations where launch_request_id in (
      select launch_request_id from platform_store.launch_requests
      where idempotency_namespace = '${runNamespace}'
    );
    delete from platform_store.launch_requests where idempotency_namespace = '${runNamespace}';
    delete from platform_store.activity_listing_revisions where listing_revision_digest = '${singleListing}';
    delete from platform_store.house_agent_host_approvals where host_installation_id = '${houseHost}';
    delete from platform_store.platform_accounts where account_id in (${quotedAccounts});
    set session_replication_role = origin;
  `);
}

function psql(sql) {
  execFileSync("psql", ["-X", "-v", "ON_ERROR_STOP=1", databaseUrl], {
    cwd: repository,
    input: sql,
    stdio: ["pipe", "ignore", "inherit"],
  });
}

function parseEnv(value) {
  return Object.fromEntries(value.trim().split("\n").map((line) => {
    const match = /^([A-Z_]+)="(.*)"$/u.exec(line);
    if (match === null) throw new Error("Supabase status returned an invalid environment line");
    return [match[1], match[2]];
  }));
}

function required(values, name) {
  const value = values[name];
  if (value === undefined || value.length === 0) throw new Error(`Supabase status omitted ${name}`);
  return value;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function sha256Bytes(value) {
  return createHash("sha256").update(value).digest();
}

function bytea(value) {
  return `\\x${Buffer.from(value).toString("hex")}`;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
