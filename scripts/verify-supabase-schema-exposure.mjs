import { readFileSync } from "node:fs";

const [, , anonymousPath, servicePath] = process.argv;
if (anonymousPath === undefined || servicePath === undefined) {
  throw new Error("usage: verify-supabase-schema-exposure.mjs <anonymous-openapi> <service-openapi>");
}

const readPaths = (path) => {
  const document = JSON.parse(readFileSync(path, "utf8"));
  if (typeof document !== "object" || document === null || Array.isArray(document)) {
    throw new Error("invalid OpenAPI document");
  }
  const paths = document.paths;
  if (typeof paths !== "object" || paths === null || Array.isArray(paths)) {
    throw new Error("missing OpenAPI paths");
  }
  return Object.keys(paths).sort();
};

const anonymousPaths = readPaths(anonymousPath);
if (JSON.stringify(anonymousPaths) !== JSON.stringify(["/"])) {
  throw new Error("anonymous role can discover application relations or RPCs");
}

const servicePaths = readPaths(servicePath);
const expectedServicePaths = [
  "/",
  "/rpc/authorize_host_mutation_v1",
  "/rpc/begin_account_erasure_v1",
  "/rpc/begin_github_oauth_v1",
  "/rpc/cancel_launch_request_v1",
  "/rpc/claim_invited_seat_v1",
  "/rpc/complete_house_fill_v1",
  "/rpc/consume_github_oauth_v1",
  "/rpc/create_launch_request_v1",
  "/rpc/expire_launch_request_v1",
  "/rpc/freeze_launch_request_v1",
  "/rpc/purge_expired_oauth_attempts_v1",
  "/rpc/read_genesis_reconciliation_v1",
  "/rpc/read_house_fill_v1",
  "/rpc/read_launch_request_v1",
  "/rpc/read_owned_run_v1",
  "/rpc/record_genesis_v1",
  "/rpc/record_house_runner_reservation_v1",
  "/rpc/release_seat_claim_v1",
  "/rpc/reset_seat_claim_v1",
  "/rpc/resolve_owned_run_membership_v1",
  "/rpc/retain_house_fill_selection_v1",
  "/rpc/rotate_seat_invitation_v1",
  "/rpc/set_public_profile_v1",
  "/rpc/start_house_fill_v1",
  "/rpc/sync_github_identity_v1",
].sort();
if (JSON.stringify(servicePaths) !== JSON.stringify(expectedServicePaths)) {
  throw new Error("service role exposure differs from the exact platform_api contract");
}

console.log("Supabase exposes only the reviewed RPC surface.");
