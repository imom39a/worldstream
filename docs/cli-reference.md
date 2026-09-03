# Operator CLI contract

This is the authenticated `worldstreamctl` interface frozen by IMO-135 and
[ADR 0018](adr/0018-cli-first-operator-surface.md). The CLI-first commands are
part of the default build. The retired internal preview feature is not an
installation mode and must not appear in runnable instructions. The
[ticket plan](cli-first-ticket-plan.md) records the completed cutover.

## Compatibility and output

The existing `config`, `doctor`, `health`, `version`, `pack` operations,
`sqlite`, and `postgres` commands retain their arguments, scope, output
defaults, and receipts. Existing global `--config FILE`, `--bind ADDRESS`,
`--storage-profile PROFILE`, and `--data-dir DIRECTORY` remain available.
New output options do not silently change existing command behavior.

New leaves additionally accept `--state-dir DIRECTORY` (default
`.worldstream/studio`), `--controller ADDRESS` (default `127.0.0.1:9420`), and
`--timeout-seconds N` (1–300, default 30). These options belong after the new
leaf, not to legacy commands. `--controller` requires a literal loopback IP
and nonzero port; hostnames and remote addresses are rejected. `--bind`
continues to select the **Runtime** listener, never the controller.

Every new leaf accepts `--json`. New commands otherwise produce readable
output. JSON mode writes one document to stdout, with diagnostics on stderr.
Help remains ordinary text and exits successfully. Parsing errors never echo
new-command input values; their diagnostic points to the relevant help.
No command reads a secret value from an argument or Room setup file.

| Exit | Meaning for new commands |
| --- | --- |
| 0 | Complete, or help/version requested |
| 1 | Rejected or failed operation |
| 2 | Invalid command arguments; no operation attempted |
| 3 | Unavailable, including a backend not yet implemented |
| 4 | Partial outcome; retained progress needs inspection/recovery |

Unavailability is not an empty successful list. Stale evidence must remain
labelled stale and must not become a freshly verified fact. Partial results
must identify retained work, never imply rollback or full success. Commands
must not print bearer material in ordinary text, JSON, or diagnostics.

The stable envelope is `schema`, `command`, `status`, `code`, `message`, and
`next_action`. `schema` is `worldstream/operator-command/v1`; command labels
are the space-separated leaf names below. Parser errors use the recognized
family label because no valid leaf has necessarily been selected. For example:

```json
{"schema":"worldstream/operator-command/v1","command":"server status","status":"unavailable","code":"not_implemented","message":"This operator command is not implemented yet.","next_action":"Use the documented existing operator interface until this backend is available."}
```

Backend result fields and persisted specification schemas are separate from
the CLI argument contract. No placeholder output claims a real inventory,
initialization preview, successful mutation, or live status observation.

| `status` | Closed `code` values | Exit |
| --- | --- | --- |
| `complete` | `complete` | 0 |
| `rejected` | `operation_rejected`, `managed_runner_restart_unsupported`, `start_acknowledgement_required` | 1 |
| `failed` | `operation_failed` | 1 |
| `invalid_arguments` | `invalid_arguments` | 2 |
| `unavailable` | `not_implemented`, `controller_unavailable`, `pack_inventory_unavailable`, `stale_evidence` | 3 |
| `partial` | `setup_incomplete`, `lifecycle_incomplete` | 4 |

Launch uses `room_launch_not_ready` and `room_launch_inapplicable` (exit 1),
`room_readiness_unavailable` (exit 3), and `room_launch_unconfirmed` (exit 4).
Credential exports use `credential_selection_invalid`, `credential_output_exists`,
`credential_destination_unsafe`, and `scoped_credentials_unavailable` for rejected
requests. `credential_write_incomplete` means the write failed. These all use
exit 1 and do not print the credential document.

Client launch uses `client_selection_required`, `client_handoff_unavailable`,
and `client_open_failed` (exit 1). Selection output lists safe candidates; use
`--binding` to make a choice. Success means the system was asked to open the
client. It does not mean the participant has connected or synchronized.

Runner control uses `runner_not_managed_or_not_ready` and
`runner_operation_failed` (exit 1), or `runner_operation_incomplete` (exit 4)
when a process change cannot be confirmed. Inspect the same operation and seat
before retrying. External Runners can be inspected but not started or stopped
by these commands. Typed Runner status contains no credential values.

Retained partial setup reports may additionally contain `operation_id`,
`room_id`, and `stage`; absent fields are omitted. References use the bounded
public identifier grammar below. Setup stages are `room_creation`,
`member_capability`, and `runner_capability`. Resume the reported operation
using the same installation options; do not supply a replacement setup file.

Partial managed shutdown/restart reports use `lifecycle_incomplete` and a
`stage` of `managed_runner_stop`, `runtime_stop`, `runtime_restart`, or
`managed_runner_restore`. They contain no Room or setup-operation reference
and offer no Room setup resume command. Inspect `server status` and
`runner list` with the same installation options before taking further action;
the report makes no rollback claim. Restoration concerns only the previously
running owned managed Runners after current approval and eligibility checks.

Completed adapters use these outcomes; commands with pending backends still
return `not_implemented`. Initialization has additional closed result codes
documented by its input and approval contract.

## Installation and server

`init` creates or validates one protected local installation. It starts no
processes, installs no boot service, and does not replace existing configuration,
bootstrap authority, or retained Room data. Existing installations missing the
new control credential require this explicit initialization/migration step.

The controller credential belongs to the local operating-system owner. It is
not a Runtime Host, Membership, Runner, or model-provider credential. The CLI
loads it from protected local state for a control request; it does not accept
it as an argument or print it. The controller listens only on a literal
loopback address with a nonzero port. Loopback reachability, cookies, and an
Activity Client Origin header are not operator authentication.

Control rotation changes only this controller credential. Existing controller
instances read the current protected record for each authenticated request,
so old access stops working without a controller restart. Concurrent rotation
may fail as unavailable; inspect the installation before retrying. A failed
durability check does not establish that the old credential remains current.
Neither initialization nor rotation resets or relocates existing Room data.

Initialization and rotation use protected local locks. If another mutation is
in progress, the command fails without waiting for a hidden timeout. Retry
after that mutation finishes. Repeated initialization keeps the same control
credential and generation. It republishes those exact bytes under the lock
to complete file publication after a possible earlier interruption.

For the current shared-backup layout, the controller state directory must be
`studio` beside the Runtime data directory. The defaults are
`.worldstream/studio` and `.worldstream/data`. A different Runtime data
directory name is allowed under the same parent. Initialization rejects a
different layout; it does not move retained files. Configuration and secret
source paths must not conflict with each other or with Runtime storage,
controller records, or the protected vault.

On macOS and Windows, initialization conservatively compares path roles
without ASCII case distinctions and requires not-yet-existing path components
to be ASCII. Windows also rejects new components ending in a dot or space.
Existing Unicode ancestors and retained paths remain supported; these checks
do not rewrite filesystem paths or claim native Windows qualification.

The new publication helper uses file synchronization and atomic path
publication on POSIX, and Windows write-through move/replace operations.
The Windows helper and Runtime protection code have passed cross-target
typechecking. This is not native Windows crash, ACL, or release qualification.

Browser participant routes keep their own one-use handoff, Membership session,
Origin, and revocation checks. They never receive the controller credential.
The authenticated operator boundary covers the complete composed controller
router, including legacy route aliases, not only new CLI-specific routes.

Initialization uses explicit `--config FILE`, otherwise `WORLDSTREAM_CONFIG`.
If neither is supplied, it proposes/creates `.worldstream/worldstream.toml`
and reports the exact `--config` argument to use next. Existing config files
are preserved byte-for-byte. This does not add implicit file discovery to the
legacy configuration loader; subsequent commands must select the new config.

Use protected file sources for bootstrap authority and the PostgreSQL DSN
when you initialize a managed installation. `init` rejects inherited handles,
including handles named in an existing config. A later CLI invocation cannot
rely on the same inherited handle. Foreground Runtime configuration options
are unchanged.

An explicit prerequisite import is part of initialization, not Room setup:

```sh
worldstreamctl --config config/development.toml init --preview \
  --runner-template reviewed-runner.json \
  --provider-declaration reviewed-provider.json \
  --client-declaration reviewed-clients.json \
  --agent-profile reviewed-profile.json --json
```

Preview resolves a bounded set of non-secret declarations and reports the
aggregate exact-input digest. To import, repeat the same inputs with
`--approve-imports blake3:DIGEST` instead of `--preview`, using the actual
64-lowercase-hex digest returned by preview. A boolean approval, missing
approval, changed bytes/targets, or a stale digest cannot authorize import.
Never include provider secret bytes in preview or its digest/output.
Imported records retain separate artifact, deployment, and binding trust
boundaries; an existing disabled/revoked record stays disabled/revoked.
This is not a wildcard approval for artifacts discovered later.

Each import flag can occur at most 16 times (64 total inputs). Each occurrence
names one UTF-8 local path, at most 4096 bytes with no control characters.
Directory scanning, glob expansion by the CLI, executable shell input, and
implicit import of nearby declarations are not supported. Shell-expanded
inputs still count toward the same limit. Preview is nonmutating; approval
authorizes only the reviewed bytes and exact declared identities/targets.

The exact versioned JSON fields, per-file bounds, referenced-file rules, and
provider secret-file boundary are specified in
[Initialization input contracts](cli-initialization-inputs.md). A file selected
by one flag cannot be interpreted as another declaration kind.

| Command | Required input / behavior |
| --- | --- |
| `server start` | Explicitly start/reuse the authenticated controller and configured Runtime |
| `server stop` | Stop owned managed Runners before the Runtime; leave controller available |
| `server restart` | Restore only previously running eligible managed Runner assignments |
| `server status` | Report availability, ownership, liveness, readiness; never spawn |
| `server logs` | Read bounded secret-safe logs; never spawn |
| `server controller-stop` | Stop only controller, not Runtime |
| `server rotate-control-credential` | Explicitly replace local control access, not other authority |

Lifecycle commands accept `--timeout-seconds N` (1–300, default 30). `logs`
accepts `--tail N` (1–1000, default 100); live following is not part of this
contract. Partial shutdown or restart-restoration failure exits 4. A controller
that cannot prove ownership must refuse unsafe control, not kill by stale PID.

`server start` also accepts `--participant-console-origin ORIGIN` for a separate
local Client Host, for example:

```sh
worldstreamctl --config .worldstream/worldstream.toml server start \
  --controller 127.0.0.1:19420 \
  --participant-console-origin http://127.0.0.1:15173
```

The exact local HTTP origin is retained with the Controller configuration.
Omission reuses it; fresh installations and older records default to
`http://127.0.0.1:5173`. A conflicting explicit origin is rejected before
starting or reusing a Controller. Origins have no trailing slash or path;
`http://127.0.0.1:5174` is the retired historical Studio origin. This setting
neither starts a Client Host nor changes approved
Deployment URLs. Review/import matching launch URLs separately, and configure
the client build's `VITE_WORLDSTREAM_SUPERVISOR_URL` for a nondefault Controller.

MVP limit: `server restart` rejects before stopping anything when a running
Runner Template instance is bound to a task. Its result code is
`managed_runner_restart_unsupported`. Use `server stop`, `server start`, and
explicit agent startup instead. Automatic restoration of these bound instances
is deferred to IMO-147. Existing supported managed Agent Host restoration is
unchanged. This preview is not production-qualified process supervision.

## Room setup and participation

`pack list` is the new read-only aggregate inventory command. It must keep
installed inventory, next-start selectability, and live Core selectability
separate. A missing controller or Runtime does not turn live state into an
empty successful list. Existing `pack inventory`, approval, install, selection,
export, and removal commands remain unchanged; `pack list` is not a second
package-management implementation.

The preview command reports three independent sections:

- `installed`: bounded local inventory metadata, explicitly unverified. It does
  not open or verify Bundle objects while the Runtime is running.
- `next_start`: retained selectable Bundle digests, not a readiness verdict.
  Use the existing offline `pack restart-readiness` command before startup.
- `running`: exact embedded revisions and portable Bundle identities captured
  by the current Runtime at startup, read through authenticated control.

Human output prints copyable `ID@VERSION` selectors only from running facts.
JSON retains the exact semantic Revision and physical Bundle digests separately.
`pending_changes` compares installed identity/selectability with that frozen
running inventory; it is `null` when either source is unavailable. It is not a
configuration or deployment-readiness check. A malformed running response is
labelled `stale`; an unreachable Runtime is `unavailable`. Available sections
remain visible when another section fails, with `pack_inventory_unavailable`
and exit 3. An empty installed inventory is not an error by itself.

Room Setup Specifications are versioned JSON; server configuration stays TOML.
Schema details belong to the Room Setup Specification implementation. Input
resolves to one exact immutable Pack reference before mutation. It never
describes desired replacement state for an existing Room.

Room Setup Operation references are printed by creation/status commands.
Creation saves the resolved configuration and participant identities before
calling the Runtime. `room setup resume OPERATION` uses only those saved
records, never the source file. A complete setup means that provisioning is
complete; it does not mean that clients are connected or the Activity has
launched. `room list` and `room inspect ROOM` read separate Host diagnostics,
not participant-private content.

A lost reply returns exit 4 and the operation reference, even when the CLI
cannot confirm a Room ID. Check `room setup status OPERATION` with the same
installation options before deciding what to do next. Status without an
operation lists unfinished attempts. Do not repeat `room create` as a retry:
that starts a new attempt with new identities.

New CLI creation records retain whether setup preparation has started. If the
provisioning record is later missing, status reports `restore_setup_record`
and the original Room ID. Resume does not create replacement credentials.
Restore the original record when one is available; this preview does not
provide automatic repair of lost intent. Existing legacy creation records
with an intact setup remain resumable. A legacy creation without a setup has
no preparation marker, so the CLI cannot distinguish an unprepared attempt
from a lost setup. Finish a genuinely unprepared legacy attempt through its
original workflow before using CLI resume, or restore its original setup.

An active-at-Genesis Pack requires `--acknowledge-start`. Without it, creation
returns `start_acknowledgement_required` before creating a Room. Once accepted,
activity and deadlines begin at creation; partial provisioning does not pause
time. Heist 0.2.0 instead enters its declared Lobby.

Use those references and the specification's stable seat labels for subsequent
commands; do not inspect internal databases to discover identifiers.
Room/operation references and client binding identifiers are
1–128 ASCII bytes, beginning with a letter or digit and followed only by
letters, digits, `.`, `_`, `:`, or `-`. Setup seat labels have a narrower bound:
1–64 lowercase ASCII letters, digits, or hyphens, starting with a letter or digit.
Pack Role names are separate and are not changed to match seat labels.
Local input/output paths use the same
nonempty UTF-8, 4096-byte, no-control-character bound as import paths.

| Command | Input |
| --- | --- |
| `room example` | `--pack ID@VERSION --output FILE`, optional `--interactive` |
| `room validate` | `--file FILE` |
| `room create` | `--file FILE`, plus `--acknowledge-start` for an active-at-Genesis Pack |
| `room list` | No selector |
| `room inspect` / `room launch` | Positional `ROOM` printed by creation/list/status |
| `room setup status` | Optional positional `OPERATION`; absent means unfinished operations |
| `room setup resume` | Required positional `OPERATION`; no file or replacement-input flags |
| `runner list` | Optional `--operation OPERATION` |
| `runner inspect` / `start` / `stop` | `--operation OPERATION --seat LABEL` |
| `runner export-credentials` | `--operation OPERATION --seat LABEL --output FILE` |
| `client open` | `--operation OPERATION --seat LABEL`, optional `--binding BINDING` |
| `client export-credentials` | `--operation OPERATION --seat LABEL --output FILE` |

Get the exact running `ID@VERSION` selector from `pack list`; example
generation resolves it to one complete exact Pack reference. Multiple matches
or unavailable metadata fail closed, never choose the first entry. Stored
semantic digests—not these diagnostic labels—remain Room authority.
No semver range, automatic latest-version lookup, or network package fetch is
implied. Optional interactive generation is incompatible with `--json` and
must reject a noninteractive input stream instead of waiting for a prompt.
The `ID` part uses the bounded identifier grammar above; `VERSION` must be
exact numeric `X.Y.Z`, each component an unsigned 64-bit integer without
leading zeroes (except `0`). Tags, ranges, prerelease, and build suffixes are
not accepted by this selector. Syntax acceptance is not proof that
the literal name/version exists locally or resolves unambiguously.

Human and agent credential delivery remain separate from administration.
An agent can require a Membership credential and a Runner-control credential;
export these separately. Exports require a protected file destination and
never silently overwrite it. `client open` launches only an approved compatible
client through a one-use handoff, not a fabricated browser Origin header.

### Connect participants, then launch

For Heist 0.2.0, complete setup first. Then connect the required participants.
Use `room inspect ROOM` or `room setup status OPERATION` to read the launch
assessment. It is live operational evidence, not stored Activity state.

- A direct SDK or terminal client must connect to its Membership and acknowledge
  synchronization. Its connection must remain open.
- A browser client must receive and acknowledge an authorized delivery. Its
  regular updates renew a five-second presence window. Closing the browser stops
  renewal; the last receipt expires within five seconds. A cookie, an opened
  window, a handoff, or a server health check alone does not count as presence.
- An external agent needs its exact Runner assignment and a connected, fresh
  Runner with compatible Pack support and free capacity. It does not need a
  permanent model process or a local Agent Profile.
- Only required seats block launch. An optional seat can be disconnected.

Run `room launch ROOM` when the required seats are ready. The command submits
the Pack's declared Lobby launch. If the reply is lost, inspect the same Room
and repeat `room launch ROOM`. The server retries the original retained launch
input. Do not create a replacement Room.

Negotiate starts at Room creation. `room launch` returns
`room_launch_inapplicable` for it. This is not a failed negotiation and does not
mean that a second Room is needed.

### Export a scoped credential file

Create an owner-only output directory first. On macOS and Linux, its permissions
must be `0700`. Choose a new filename for each export. The CLI does not change
the permissions of an existing directory or overwrite an existing file.

| File | Use | WebSocket endpoint in `runtime_url` |
| --- | --- | --- |
| `client export-credentials` output | Connect and act as the selected Membership | `/v1/stream` |
| `runner export-credentials` output | Receive, claim, and complete agent Activations | `/v1/runner/stream` |

An external agent usually needs both files. They contain different credentials;
one cannot replace the other. Pass their paths to the intended integration.
Do not put their contents in shell arguments, chat, logs, or version control.
The CLI report shows only the output path and public selection metadata.

If a write fails, the command reports failure. A protected partial output file
can remain if cleanup also fails. Check the requested path before trying a new
filename. This is not permission to use the incomplete file.

Read-only commands never start the controller, Runtime, or Runners. Mutations
with missing input do not prompt implicitly or infer artifact approval.
Pack lobby launch is explicit; active-at-Genesis Packs start their activity
and deadlines at creation. No CLI command pauses domain time.

See [the implementation plan](cli-first-implementation-plan.md) for the
verified retirement gate. Current runnable steps remain in
[Getting started](getting-started.md).

For this MVP, unattended timed Activity Packs require SQLite: its Runtime
scheduler automatically commits due timers using protected installation
authority. PostgreSQL retains Activation lease maintenance and the existing
Host-authorized manual timer API, but does not yet drive timers automatically;
startup emits an explicit warning. `scheduler_running` continues to describe
lease-maintenance readiness, not PostgreSQL automatic-timer support.
