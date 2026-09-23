# Agent Swarm application boundary

The default build exposes only the deterministic fixture smoke path. It does
not create a WorldStream Room or imply that a provider agent ran.

The optional `managed-local-runtime` Cargo feature adds authenticated
`create`, `list`, `open`, `observe`, `act`, and `tui` commands backed by an
already initialized, running local WorldStream Controller and Runtime. This is
additive: enabling it does not change Core, SQLite, Room schemas, or the
default fixture path.

The Room is authoritative for goal, roster, Pack state, offered Actions, and
accepted outcomes. The separate `worldstream-agent-swarmd` daemon owns only
operational concerns: provider process trees, scheduling, budgets, priorities,
and pause/stop/resume state. Its journal is not a second source of Room truth.

`worker-run --autonomy-policy /absolute/path/policy.json` enables an opt-in
adaptive planning loop for a confirmed goal. A roster LLM chooses the next
authorized actions, worker assignments, dependency revisions, revised attempts,
or waiting/handoff. The coordinator retains each decision before dispatch and
can launch independently owned Work Attempts in parallel. Without that policy,
`worker-run` executes staged plans and handles due Progress Reviews as before.
See [adaptive planning](../../docs/agent-swarm-adaptive-planning.md) for its
scope and limits. Contributions still require integration, checks, and review
before Result acceptance. External providers remain blocked until their native
qualification evidence is complete.

The [parallel coding challenge](../../docs/evidence/agent-swarm-parallel-challenge/README.md)
exercises the managed system with explicitly scripted workers. Its timing and
correctness results describe the coordination machinery, not real model coding
performance or autonomous planning.

## Identity and authorization

Each new Swarm receives a random 96-bit application/operation identifier. That
identifier is for naming and idempotent recovery; it is not authorization.
The managed backend still uses WorldStream's protected Controller credential
and a Room-scoped human Membership credential internally. Bearer material is
kept in memory only and is never written to the Agent Swarm index or printed by
these commands.

## Prerequisites

Follow [Getting started](../../docs/getting-started.md) to initialize the local
installation and learn the offline Pack approval/install/selectability flow.
Build this Pack with `pnpm --dir packs/agent-swarm pack:build`, apply that flow
to the emitted `.wspack`, and then start the Controller and Runtime. Use
`worldstreamctl pack list` to obtain the exact running Pack ID, version, and
digest.

Provider discovery is read-only and fail-closed. `providers` reports missing,
unsupported, or unresolved installations without logging in, substituting a
model, changing provider configuration, or calling a model API directly.
Controlled-worker tests are not evidence that Codex, Claude Code, or Kiro ran.
After genuine native qualification, `install-provider-qualification` validates
an assembler-produced record, binds it to one exact executable, and publishes
that binding as a new owner-only file. `qualify-provider` re-probes the bound
executable and checks one exact model/effort selection against it. Neither
command creates evidence or performs provider login/model execution. See
[native qualification](../../docs/agent-swarm-qualification.md).

Provider tool scope fails closed. Claude's exact non-empty named-tool
allow-list is passed to its native CLI while `Agent` and `Task` remain denied.
The qualified Codex contract has a sandboxed minimal built-in surface but no
exact named-tool allow-list, so Codex accepts only an empty `allowed_tools`
request and rejects every non-empty list instead of silently broadening it.
The new Codex app-server transport checks effective model/effort and sandbox
reports before a turn and scrubs direct-API environment variables. Its exact
configuration isolation and native containment still require qualification;
app-server does not expose `exec`'s `--ignore-user-config` flag. See
[Codex adapter status](../../docs/agent-swarm-codex-app-server.md).

Kiro remains visibly unsupported for execution. The current documented Kiro
headless contract provides non-interactive JSON-lines output, explicit effort,
specific-session resume, and tool allow-lists, but it does not provide an
invocation-time model flag or a documented output record that proves the
effective model, effort, and session. Agent Swarm therefore reports the model
control gap and never treats a requested value as a reported value. The owned
process boundary can safely materialize non-secret, invocation-local provider
profiles below protected daemon state: files are exclusively created, never
overwritten, and removed after the guarded process tree exits; stale remnants
are removed only after the daemon acquires sole journal ownership. This seam
does not copy or relocate Kiro credentials and is not evidence that the current
Kiro CLI contract meets Ticket 06.

An example creation request is:

```json
{
  "goal": "Prepare a short report",
  "constraints": ["Use only the approved working area"],
  "acceptance_criteria": [{ "text": "The report can be reopened" }],
  "working_area": "/absolute/path/to/work",
  "progress_review_interval_seconds": 300,
  "correction_failure_limit": 3,
  "roster": [
    {
      "member_key": "researcher-a",
      "label": "Researcher A",
      "provider": "codex",
      "requested_model": "gpt-5.2-codex",
      "requested_effort": "medium",
      "configuration_state": "resolution_unreported"
    }
  ]
}
```

Both supervision settings are optional in JSON: Progress Reviews default to
300 seconds and corrective work escalates after 3 failed outcomes. Explicit
values must be positive; the application validates them before Room setup and
the current values remain visible in managed participant views and the TUI.

Create a Swarm with the feature enabled:

```sh
cargo run -p worldstream-agent-swarm \
  --features managed-local-runtime -- create \
  --state-dir .worldstream/studio \
  --controller-address 127.0.0.1:9420 \
  --runtime-address 127.0.0.1:9410 \
  --pack-id worldstream.agent-swarm \
  --pack-version 0.2.0 \
  --pack-digest blake3:EXACT_64_CHARACTER_LOWERCASE_DIGEST \
  --request /absolute/path/to/create-swarm.json
```

Use the same local and exact Pack arguments with `list`, `open --swarm-id
swarm-...`, `observe --swarm-id swarm-... --actor human` (or
`worker:<member-key>`), and `act --swarm-id swarm-... --request action.json`.
An Action JSON document retains its exact actor, Action ID, observed Room
sequence, action type, and payload. Direct `act` accepts Human coordinator
authority only; production worker Actions are submitted by the supervised
coordinator from retained provider output. The packaged native fixture has a
separate hidden flag that works only for an authoritative `controlled` roster
member in `fixture_unavailable` state, so it cannot impersonate Codex, Claude,
or Kiro membership. Re-observe after stale or uncertain outcomes; do not invent
a replacement identity. Reopening reconciles the retained setup operation and
observes the authoritative projection without creating a replacement Room.

Use `artifact` to resolve a Room-referenced local result without trusting an
arbitrary absolute path. Supply the Swarm, a portable relative `--path` under
its approved working area, and an owner-only `--artifact-state` directory,
along with the same local/Pack arguments:

```sh
worldstream-agent-swarm artifact \
  --state-dir .worldstream/studio \
  --controller-address 127.0.0.1:9420 \
  --runtime-address 127.0.0.1:9410 \
  --pack-id worldstream.agent-swarm \
  --pack-version 0.2.0 \
  --pack-digest blake3:EXACT_64_CHARACTER_LOWERCASE_DIGEST \
  --swarm-id swarm-... \
  --path reports/result.md \
  --artifact-state /absolute/private/path/artifacts \
  --expected-digest blake3:EXPECTED_CONTENT_DIGEST
```

The optional `--expected-digest` rejects bytes that do not match the
authoritative artifact reference. On success the command captures the bytes in
owner-only content-addressed state and prints the verified digest, length, and
safe absolute path that a user can open.

## Execution daemon

Start one long-lived daemon with the exact packaged process guard:

```sh
bin/worldstream-agent-swarmd \
  --state /absolute/private/path/execution \
  --process-guard /absolute/install/path/bin/worldstream-agent-swarm-process-guard
```

Attach application commands with `--execution-state` pointing at that same
owner-only directory. New Swarms are registered stopped, and their
authoritative roster supplies the initial per-provider caps. Across registered
Swarms, each automatic cap is the largest selected count for that provider,
not the sum of every roster. A human `set-provider-cap` policy is a durable
manual override, including an explicit zero, and later Swarm registration does
not replace it. Providers absent from both the registered rosters and manual
policy have no capacity. Use
`worldstream-agent-swarmctl --state ... status`, `pause`, `stop`, and `resume`
for explicit operational control. Pause prevents new admissions while current
work drains; Stop terminates the owned process tree; Resume is always explicit,
including after recovery or update. Detaching a client leaves the daemon and
Room intact.

`worldstream-agent-swarmctl --state ... cancel-invocation SWARM_ID
INVOCATION_ID` targets only that retained active Invocation. A successful
termination leaves the Swarm's desired Running state and its other active work
unchanged. Uncertain containment is retained as Unknown and requires explicit
reconciliation. Only an exact retained terminal cancellation is idempotent;
an arbitrary missing identity never reports success.

External write-back uses the same durable daemon boundary. Call
`prepare-effect --intent effect.json` before target contact,
`mark-effect-dispatched OPERATION_ID` immediately before dispatch, and
`observe-effect OPERATION_ID applied|duplicate|not-applied|rejected|unknown`
only from an exact target/idempotency observation. Finish with
`acknowledge-effect OPERATION_ID` after a terminal observation. A crash after
dispatch restores the operation as Unknown, blocks the affected Swarm, and
requires explicit observation plus Resume; neither the daemon nor Room replay
repeats the external effect.

The low-level control client keeps its existing `register SWARM_ID` form for
compatibility; that legacy form registers no automatic capacity. When using it
instead of the application create/TUI path, pass the authoritative selected
counts explicitly with `--codex-count`, `--claude-count`, `--kiro-count`, or
`--controlled-count`. Counts are bounded by the 16-member roster limit.
Execution-state v1 journals migrate their retained `provider_caps` as manual
overrides; migration never invents roster capacity for an older registration.

The internal coordinator is the only component that joins Room authority to
daemon execution. Before scheduling, it durably retains the bounded set of
currently offered Action types that a worker may propose. After execution it
validates and retains the provider's output and effective configuration,
refreshes worker authority, captures any declared local artifact, creates and
journals one exact Action, and only then submits it and acknowledges daemon
completion. Uncertain launch/submission and changed authority fail closed for
explicit reconciliation; opening retained coordinator state never resumes
execution. Provider credentials and daemon control secrets are not journaled.

The feature-enabled CLI exposes that lifecycle as `worker-stage`, `worker-run`,
`worker-dispatch`, `worker-harvest`, `worker-retry-action`,
`worker-retry-launch`, and read-only `worker-intent`. Every command requires
the normal exact local/Pack arguments plus `--execution-state`, `--swarm-id`,
an owner-protected `--coordinator-state`, and one repeated
`--provider-qualification` for each owner-only installed provider binding.
Caller-authored capability JSON is never accepted. Every command re-probes the
bound executable and rejects changed bytes/version/platform or a model/effort
pair absent from the retained native evidence. `worker-stage` also requires
`--plan` with one `WorkerActionPlan`; retry and intent commands require
`--invocation-id`. The CLI opens the authoritative Swarm before constructing
the artifact workspace from its canonical working area. It never accepts or
prints bearer material. Dispatch and harvest are explicit ticks, and the two
retry commands preserve the retained Action or launch identity rather than
regenerating work.

Deterministic native tests may use `--controlled-path` with the exact packaged
`worldstream-agent-swarm-controlled-worker`. That narrow fixture seam performs
the controlled adapter's static executable/version/flag inspection and cannot
enable Codex, Claude Code, or Kiro. Real providers still require installed
native qualification records.

`worker-run` is the normal persistent coordinator entry point. It restores
service-retained plans into the per-Invocation coordinator, then dispatches
and harvests eligible work. It also discovers due or claimed Progress Reviews
from a fresh authoritative observation. An exact retained member profile keeps
its resource, tool, and session policy. If the roster has no retained profile,
automatic review bootstrap uses the deliberately fixed fail-closed application
default: `read_only`, no named tools, and a fresh session without a requested
session ID. Provider, model, effort, configuration revision, and moving-alias
acknowledgement still come only from the exact current roster; provider output
cannot choose or widen them. `--once` performs one bounded
restore/dispatch/harvest cycle; `--poll-interval-ms` controls continuous
polling. Reopening the service itself performs no daemon call. A paused,
stopped, recovery-required, or unknown-effect Swarm remains waiting for
explicit external reconciliation and `resume`. Uncertain launch or submission
returns from the loop so the exact explicit retry command can acquire the
coordinator journal. Reevaluation remains visible as human attention but does
not block independent or replacement plans. The loop never calls Resume or
either retry operation on its own.

`WorkerActionPlan` JSON now uses `allowed_action_types` (one to 32 unique
currently offered Action type names). The former caller-authored `action_id`,
`action_type`, and `payload` fields are rejected.

Every plan also requires a `semantic_target`. Production-provider plans use a
narrow target that binds the exact current execution epoch and authoritative
entity revisions for one Work proposal, Work claim, owned Work Attempt,
independent Candidate review, Finding resolution, correction outcome, or
Progress Review phase. The coordinator checks that target against fresh Room
observation before launch and again when materializing a proposal. Omission
fails closed. `controlled_fixture_admin` is the only deliberately unbound
target and is accepted only by the deterministic `controlled` adapter; Codex,
Claude Code, and Kiro cannot claim it. Native Check execution and write-back
remain trusted application bridges through `CodeChangeService`; provider
plans cannot authorize `record_check`, `request_writeback`, or
`record_writeback_outcome` themselves.

`configuration_revision` is the authoritative Pack roster's per-member
selection revision, not the Pack's setup revision. One revision is durably
bound to the provider, requested model/effort, moving-alias acknowledgement,
and explicit fresh/resumed session selection. Reusing it with different
settings or staging an older revision is rejected; a higher revision records
the selection or session handoff for that member's next Invocation while an
already-running turn retains its original configuration.

A provider's final answer must be only this strict, bounded JSON envelope,
without Markdown, commentary, unknown fields, or trailing values:

```json
{
  "schema": "worldstream/agent-swarm-action-proposal@1",
  "action_type": "submit_contribution",
  "payload": {},
  "artifact": {
    "artifact_id": "contribution-1",
    "local_path": "contributions/contribution-1.md",
    "media_type": "text/markdown"
  }
}
```

`payload` must be an object and must omit its top-level `artifact` field. The
`artifact` declaration is required for contribution, candidate, and late-
contribution Actions and forbidden for other Actions. Its path is portable and
relative to the authorized working area; the coordinator supplies the captured
digest and canonical absolute path when materializing the exact Pack Action.
Providers may also declare `expected_digest` with an exact lowercase BLAKE3
digest when they can bind the proposal to already-produced bytes.

## Interactive terminal

The feature-enabled `tui` command uses the same managed backend and exact Pack
arguments. A request file is not required. If present,
`--request /absolute/path/to/create-swarm.json` only prefills the native form
opened by `n`:

```sh
cargo run -p worldstream-agent-swarm \
  --features managed-local-runtime -- tui \
  --state-dir .worldstream/studio \
  --controller-address 127.0.0.1:9420 \
  --runtime-address 127.0.0.1:9410 \
  --pack-id worldstream.agent-swarm \
  --pack-version 0.2.0 \
  --pack-digest blake3:EXACT_64_CHARACTER_LOWERCASE_DIGEST \
  --execution-state /absolute/private/path/execution \
  --coordinator-state /absolute/private/path/coordinator
```

Use up/down to select a Swarm or roster member, Enter to open the selected
Swarm, Escape to return/cancel, `r` to refresh authoritative state, and `q` to
detach without deleting the Room. On the Swarm list, `n` opens a five-field
form for goal, constraints, acceptance criteria, absolute working area, and
roster. Tab advances through the fields. Constraints and criteria use `|` as
their item separator; use `-` for no constraints. Roster entries use
`key | label | provider | model | effort-or-- | ack-or-unack`, separated by
`;`. The first Enter validates and stages a complete preview; a separate Enter
creates the Room. Escape cancels a draft or staged creation without mutation.
With `--actions actions.json`, `a` stages the next exact human Action and Enter
confirms it. With `--execution-policies policies.json`, `l` stages the next
provider-cap, priority, or budget policy and Enter confirms it. Policies affect
daemon admission only. `p`, `s`, and `u` pause, stop, and resume the opened
Swarm when an execution daemon is attached.

The same terminal can author these changes without JSON plan files. Select a
roster member and press `c`, then enter
`provider | model | effort-or-- | ack-or-unack`. The preview binds the exact
current member configuration revision; Enter stages the
`update_member_configuration` Action and a separate Enter confirms it. Press
`e` to author an execution policy. Tab cycles provider cap (`provider limit`),
Swarm priority (`1..16`), and budget
(`invocation-limit-or-- active-ms-or--`). Enter stages and a separate Enter
applies the policy. Escape cancels any draft or staged mutation.

Press `i` to open the bounded authoritative detail inspector and cycle its
discovered artifacts. It shows the selected member's current Work Item,
dependencies, owner and status; the latest
Candidate, Check, Review, Finding, and Progress Problem; correction attempts,
evidence and reason; plus observed artifact path, digest, and media type. The
inspector renders only authenticated Room projection metadata and never treats
local process state as a Room fact. When `--coordinator-state` is supplied, a
text, diff, or JSON artifact can also be previewed only after the inspector
re-resolves the exact descriptor from the current Room projection, proves the
path is contained by the Swarm working area, performs a stable bounded read,
and verifies the authoritative digest. It rejects binary/non-UTF-8 content,
redirected or arbitrary paths, ambiguous references, digest mismatch, and
files over 64 KiB. Display is further limited to 8 Ki characters and 80 lines,
with terminal control characters sanitized.

When `--coordinator-state` is supplied, the supervised-execution panel also
reads the coordinator's atomically published operational view. It labels the
verified effective model/effort, provider-reported session identity, and local
coordinator disposition separately from the authoritative Room workflow.
Those values are evidence about one local Invocation, not roster updates,
accepted results, or other canonical Room facts.

For deterministic CI, `--script /path/to/inputs.json` drives the same
application/backend path through an in-memory terminal. With a `--request`
prefill, the file can be an array such as
`[ {"input":"new"}, {"input":"open"}, {"input":"open"},
{"input":"resize","width":72,"height":20}, {"input":"home"},
{"input":"refresh"},
{"input":"quit"} ]`. Its receipt reports the selected Swarm and Room plus
terminal restoration facts. This mode verifies interaction and persistence;
it does not claim or imply that a provider process executed. Native external
provider qualification remains a separate release gate.

The native macOS and Windows gates additionally launch the real application
binary inside a PTY or ConPTY. That acceptance path drives native key events,
Unicode paste/text, and resize; covers normal exit plus injected backend
failure and input termination; and checks that raw mode, bracketed paste, the
alternate screen, and a post-exit terminal sentinel are restored. It does not
substitute the in-memory scripted renderer for terminal-lifecycle coverage.

The fixture-only smoke remains available without the feature:

```sh
cargo run -p worldstream-agent-swarm -- \
  smoke-test --state-dir /absolute/path/to/disposable-state
```
