# Install and update Agent Swarm locally

Agent Swarm is a local terminal application. It connects to an initialized
WorldStream Controller and Runtime and can attach to a separate local execution
daemon. Installation does not create a hosted service or send provider
credentials through WorldStream.

## Install

1. Obtain the archive for the required application version and exact native
   target: macOS arm64, macOS x86_64, or Windows x64. The bundled Activity Pack
   is v0.2.0. Verify the archive with
   `scripts/agent-swarm-package.py verify` before extracting it. The companion
   `extract` command verifies the same bounded file inventory and refuses to
   replace an existing destination before writing the application tree. The native
   package smoke runs the extracted application, managed Room/TUI flow, and
   Room-scoped artifact capture plus reviewed, guarded code-change write-back
   and concurrent-edit preservation from an unrelated working directory.
2. Extract each application version into its own read-only directory. Add that
   directory's `bin` folder to your shell path, or invoke binaries by absolute
   path. The application is independent of the checkout and current directory;
   use the integrity-bound `config/local.toml` from the extracted bundle.
3. Follow [Getting started](getting-started.md) to initialize the local
   WorldStream state, approve the bundled `.wspack`, install it by exact
   semantic digest, and start the local Controller and Runtime. Never replace
   an older Pack revision that an existing Room references.
4. Install and sign in to Codex, Claude Code, or Kiro using each provider's own
   supported installer and normal subscription-login flow. Agent Swarm only
   inspects installed CLI capabilities. It does not install a provider, change
   provider-global preferences, extract credentials, or fall back to API keys.
5. Start `bin/worldstream-agent-swarmd` with an owner-only execution state
   directory and the exact absolute path to the sibling
   `bin/worldstream-agent-swarm-process-guard`. Do not substitute another guard
   executable. The daemon publication is non-secret; its journal remains
   owner-only. Creating a Swarm with `--execution-state` registers its
   authoritative roster as the initial provider-cap source; no separate
   `set-provider-cap` step is required. The automatic global cap is the largest
   selected count for that provider across registered Swarms, not a sum. An
   explicit human cap remains a durable override.
6. Run `worldstream-agent-swarm worker-run` with
   the same execution state, one protected coordinator state root, the exact
   Swarm ID, and installed provider qualification files. It restores retained
   plans, discovers due Progress Reviews, and remains alive across idle and
   capacity-wait states. Its default poll interval is 1000 ms; use `--once`
   only for one bounded external-driver cycle. The process never resumes
   paused/recovered execution or retries an uncertain external effect
   automatically. When no exact retained member plan exists, review-only
   bootstrap uses a fixed fail-closed `read_only`/empty-tools/fresh-session
   application profile; provider/model/effort/revision/alias acknowledgement
   are still copied from the fresh authoritative roster and cannot be supplied
   by provider output.
7. Run `worldstream-agent-swarm tui` with the protected state directory,
   Controller/Runtime loopback addresses, and the exact selected Pack
   ID/version/digest. Add `--execution-state` to attach daemon controls and
   `--coordinator-state` to show its separate local operational evidence. The
   full argument example is in
   [the crate README](../crates/worldstream-agent-swarm/README.md).

The extracted application bundle has these stable locations:

- `bin/`: application, daemon, control client, exact process guard, controlled
  worker, Controller, Runtime, and companion executables;
- `packs/worldstream.agent-swarm/<digest>/`: the immutable v0.2 `.wspack`;
- `config/local.toml`: the integrity-bound standalone local configuration;
- `metadata/install.json`: target, application/Pack identity, state policy, and
  file inventory;
- `checksums.sha256`: payload hashes used by package verification.

The application command surface is intentionally explicit: `create` makes one
Room, `list` reads retained Swarm identities, `open` reconciles and reopens one
without replacing it, `observe` reads one authenticated participant view,
`act` submits one caller-retained exact Action, and `tui` presents those flows
interactively. See the crate README for complete arguments and Action identity
requirements.

For a local artifact referenced by a Room, use `artifact` with `--swarm-id`, a
portable relative `--path`, and an owner-only `--artifact-state` directory. The
path must remain beneath that Swarm's approved working area. Add
`--expected-digest blake3:...` when the Room supplies a digest; mismatched bytes
are rejected. A successful command captures the file in owner-only
content-addressed state and prints the safe absolute path for opening it. It
does not authorize arbitrary paths or make working-area files authoritative.

Choose explicit, protected state roots outside the application directory,
preferably below `~/Library/Application Support/...` on macOS or
`%LOCALAPPDATA%\...` on Windows, and pass them with `--state-dir`,
`--execution-state`, `--coordinator-state`, and `--artifact-state` as the
command requires. Contributions and accepted artifacts also remain outside the
install directory. Copying an application archive does not copy or authorize
that operator-selected state.

## Provider discovery and login

Run `worldstream-agent-swarm providers` to inspect candidates. Discovery is
read-only and fail-closed. A missing CLI, an unsupported installed version, an
unavailable requested model/effort, or a setting whose effective value cannot
be established is shown as blocked or unreported. It is not silently replaced
with another model, an API-key path, or a direct model API call.

Complete provider login in a separate normal terminal session using the
provider's documented interactive flow. Do not put tokens, login databases, or
provider configuration directories inside an Agent Swarm archive or Swarm
artifact directory.

Only after genuine native qualification, import an assembler-generated record
with `install-provider-qualification --input ... --executable ... --output
...`, then use `qualify-provider --qualification ... --model ... [--effort
...]` to recheck the bound local executable and requested selection. Pass the
installed owner-only file to coordinator commands with
`--provider-qualification`; arbitrary capability JSON is not accepted. These
commands validate retained evidence; they do not manufacture it. See
[the qualification guide](agent-swarm-qualification.md).

## Update or roll back

1. Detach from the TUI. Pause or Stop if you do not want existing owned turns
   to finish while updating.
2. Verify the new archive before extraction, then extract it beside the old
   version. Keep the old binary directory and all referenced Pack revisions
   until the new version has reopened the existing Swarms successfully.
3. Stop the old daemon cleanly, point your launcher or shell path at the new
   `bin` directory, and restart the daemon with the same execution state and
   the new bundle's exact process guard. Start the application against the same
   protected WorldStream and application state. Reuse the exact configuration
   path originally selected for that initialized Controller; an adjacent
   bundle's configuration is a different explicit selection even when its
   bytes match, and is not substituted implicitly. An explicit managed Start
   may rebind the Runtime executable to the new bundle only after the prior
   generation recorded a clean stop and released its lease. The configuration,
   state root, working directory, data directory, endpoint, and normalized
   policy must remain exact.
4. Recovered execution remains stopped/suspended; there is no automatic
   Resume. Inspect reconciliation and unresolved effects, then explicitly
   Resume. Updating never changes roster selections, provider settings,
   Participant identities, priorities, or budgets.

For a genuine update qualification, pass an independently built older archive
to `scripts/verify-agent-swarm-package.py --previous-archive OLD ...`. The
verifier requires distinct application versions, starts the old daemon against
external state, creates reviewed Room/Contribution/Result/artifact state with
the old application, hands both state stores to the new application and daemon,
proves execution recovery stays suspended until explicit Resume, reopens the
same Room and exact artifact without changing roster selections, and then runs
a guarded worker. Without that argument the same state-preservation machinery
is exercised as a reinstall, but the receipt explicitly says
`reinstall_only_update_not_exercised`; building one archive twice is not
accepted as update evidence.

Rollback means pointing the launcher back at the previous extracted version;
it does not rewind Room facts, external effects, shared files, or accepted
results. Do not use an older application if it reports that the retained state
or Pack revision is unsupported.

## Terminal behavior

The native TUI supports keyboard navigation, resizing, narrow layouts, and
Unicode content. `g` authors a live Suggestion and `d` authors a live
Direction; Tab switches between goal and work targets, and `i` inspects the
latest local artifact. `a` and `l` stage caller-supplied Room Actions and
operational policies for Enter confirmation; Escape cancels. `p`, `s`, and `u`
pause, stop, and resume. `q` detaches without stopping the daemon. Pause drains
current turns; Stop interrupts only the owned process tree. Normal exit and
errors restore raw mode, the alternate screen, and the cursor. A terminal
closure is detach, not authorization to delete a Room or launch a replacement
worker. Native package gates extract the verified archive and run the PTY
keyboard, paste, resize, Unicode, normal-exit, and error-exit checks against
that exact packaged application executable rather than Cargo's build output.
