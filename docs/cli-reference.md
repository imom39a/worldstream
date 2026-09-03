# Operator CLI contract

This is the additive `worldstreamctl` interface frozen by IMO-135 and
[ADR 0018](adr/0018-cli-first-operator-surface.md). Existing commands continue
to work. The new commands below are **parser/output contracts only**: until
their implementation tickets land, valid invocations return `not_implemented`
with exit status 3, without reading configuration or starting services.
Help listing a command does not mean its backend is available.

During implementation, the internal `cli-operator-preview` Cargo feature
enables completed replacement slices together. It is not a second supported
installation mode. The default build keeps the current Studio path until the
CLI-only acceptance and cutover gates pass. The cutover must remove this
temporary feature boundary and the unauthenticated startup branch; disabling
default features must not restore an authentication bypass.

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
| `rejected` | `operation_rejected` | 1 |
| `failed` | `operation_failed` | 1 |
| `invalid_arguments` | `invalid_arguments` | 2 |
| `unavailable` | `not_implemented`, `controller_unavailable`, `stale_evidence` | 3 |
| `partial` | `setup_incomplete`, `lifecycle_incomplete` | 4 |

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

The formatter supports these outcomes for later adapters; current accepted
CLI invocations still return only `not_implemented` (or input rejection).

## Installation and server

`init` creates or validates one protected local installation. It starts no
processes, installs no boot service, and does not replace existing configuration,
bootstrap authority, or retained Room data. Existing installations missing the
new control credential require this explicit initialization/migration step.

The controller credential belongs to the local operating-system owner. It is
not a Runtime Host, Membership, Runner, or model-provider credential. The CLI
loads it from protected local state for a control request; it does not accept
it as an argument or print it. The controller listens only on a literal
loopback address with a nonzero port. Loopback reachability, cookies, and a
Studio Origin header are not operator authentication.

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

## Room setup and participation

`pack list` is the new read-only aggregate inventory command. It must keep
installed inventory, next-start selectability, and live Core selectability
separate. A missing controller or Runtime does not turn live state into an
empty successful list. Existing `pack inventory`, approval, install, selection,
export, and removal commands remain unchanged; `pack list` is not a second
package-management implementation.

Room Setup Specifications are versioned JSON; server configuration stays TOML.
Schema details belong to the Room Setup Specification implementation. Input
resolves to one exact immutable Pack reference before mutation. It never
describes desired replacement state for an existing Room.

Room Setup Operation references are printed by creation/status commands.
Use those references and the specification's stable seat labels for subsequent
commands; do not inspect internal databases to discover identifiers.
Room/operation references, seat labels, and client binding identifiers are
1–128 ASCII bytes, beginning with a letter or digit and followed only by
letters, digits, `.`, `_`, `:`, or `-`. Local input/output paths use the same
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

Get the exact installed `ID@VERSION` selector from `pack list`; example
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
client through a one-use handoff, not a fabricated Studio Origin header.

Read-only commands never start the controller, Runtime, or Runners. Mutations
with missing input do not prompt implicitly or infer artifact approval.
Pack lobby launch is explicit; active-at-Genesis Packs start their activity
and deadlines at creation. No CLI command pauses domain time.

See [the implementation plan](cli-first-implementation-plan.md) for the
verified retirement gate. Current runnable steps remain in
[Getting started](getting-started.md).
