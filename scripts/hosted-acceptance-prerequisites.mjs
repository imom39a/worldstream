// These gates must finish before the Runner binary is pinned or a match starts.
// No pre-existing CI status or operator checkbox substitutes for this run.
export const HOSTED_ACCEPTANCE_PREREQUISITES = Object.freeze([
  ["cargo", ["test", "--locked", "-p", "worldstream-hosted-gateway"]],
  ["cargo", ["test", "--locked", "-p", "worldstream-hosted-contract"]],
  ["cargo", ["test", "--locked", "-p", "worldstream-studio-supervisor", "--lib"]],
  ...["task_setup", "room_setup_spec", "room_setup_operations"].map((suite) =>
    ["cargo", ["test", "--locked", "-p", "worldstream-studio-supervisor", "--test", suite]]),
  ["cargo", ["test", "--locked", "-p", "worldstream-studio-supervisor", "--test", "room_creation", "production_http_creator_reuses_one_sealed_spectator_bearer_on_retry"]],
  ["cargo", ["test", "--locked", "-p", "worldstream-server", "--lib", "hosted_room_creation_is_atomic_restartable_and_non_playing"]],
  ["cargo", ["test", "--locked", "-p", "worldstream-server", "--test", "hosted_browser_stream"]],
  ["pnpm", ["--filter", "@worldstream/hosted-contract", "lint"]],
  ["pnpm", ["--filter", "@worldstream/hosted-contract", "test"]],
  ["pnpm", ["--filter", "@worldstream/client", "lint"]],
  ["pnpm", ["--filter", "@worldstream/client", "test"]],
  ...["hosted-platform:check", "activity-clients:check", "demos:lint", "demos:test",
    "hosted:dev:test", "hosted:package:test", "hosted:package:smoke"].map((suite) => ["pnpm", [suite]]),
  ["supabase", ["test", "db"]],
  ["pnpm", ["hosted-formation:concurrency"]],
  ["supabase", ["db", "lint", "--local", "--schema", "platform_store,platform_api", "--level", "warning", "--fail-on", "error"]],
  ...["security", "performance"].map((kind) =>
    ["supabase", ["db", "advisors", "--local", "--type", kind, "--level", "warn", "--fail-on", "error"]]),
]);

export async function runHostedAcceptancePrerequisites(runCommand) {
  for (const [command, args] of HOSTED_ACCEPTANCE_PREREQUISITES) {
    await runCommand(command, args);
  }
}
