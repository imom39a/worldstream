# Proposal: a CLI-first WorldStream

Status: Q1–Q18 approved on 2026-09-02: direction, retirement gate, initial
administration scope, file-driven setup, Runner integration modes, managed
process owner, one-Room input boundary, reference participant experience,
controller failure policy, immutable retries, owner-scoped access, and
unchanged Pack start semantics, JSON input, additive commands, client-neutral
readiness, explicit managed lifecycle, authenticated loopback control, and the
consolidated implementation contract. The design review is complete. The
[consolidated implementation plan](cli-first-implementation-plan.md) is approved.
Acceptance does not mean the
replacement exists or authorize immediate removal of Studio. See
[ADR 0018](adr/0018-cli-first-operator-surface.md).

Date: 2026-09-02. Code reviewed: `1d8e6dc` on `main`.

## Accepted direction

Make the command-line interface the primary Host Operator interface. Retire
the Studio web app after a working replacement for the essential user flow is
verified. Stop adding Studio features during that transition.

Keep the Runtime, Activity Packs, SDKs, and independent Activity Clients.
Reuse the useful headless operations currently in the Studio Supervisor. Do
not delete that crate merely because its name contains `studio`.

In simple terms:

> Install WorldStream. Use commands to run the server, manage Packs, and set
> up a Room. Use an agent, SDK, terminal client, or a Pack-associated browser
> client to participate. No administration website is required.

This changes how people operate WorldStream. It does not change WorldStream
into a coding agent, workflow engine, or application-specific platform.

## Why change direction?

The operator web app adds a second interface to maintain before the main
user flow is easy to use. The current configuration form exposes schema
mechanics, and the Pack Operations screen asks users to run commands and
paste the results back into the browser. The screen does not install Packs.
[Studio documentation](studio.md) already calls `worldstreamctl pack` the
complete Pack operator interface.

A CLI fits the current audience: developers who host the Runtime or build
Packs and clients. It can also serve scripts and CI without a second set of
operator workflows. People who participate in an activity can still have a
browser UI. They do not need the operator CLI.

There is a cost: a CLI is less discoverable for some users. Good help, usable
defaults, clear errors, and one complete example are therefore release work,
not optional polish. Removing the browser alone will not fix confusing
configuration, credential handling, or Room setup.

## What exists today

| Need | Current implementation | Gap |
| --- | --- | --- |
| Run a server in the foreground | `worldstreamd` | A clear first-run path and optional background management |
| Validate and inspect configuration | `worldstreamctl config validate` and `config effective` | Better first-run guidance; retain redaction |
| Check the server | `health`, `version`, and bounded local `doctor` checks | A combined status that separates liveness, readiness, and process ownership |
| Manage portable Packs | Inspect, prove, approve, install, inventory, selectability, readiness, export, revoke, and safe removal | A readable view of installed and running revisions |
| Start, stop, and restart a managed daemon | Supervisor lifecycle API | A supported CLI interface and a persistent ownership model |
| Create Rooms and provision Memberships | Runtime APIs plus durable Supervisor operations | A coherent CLI flow using those operations |
| Attach Runners | Runtime and Supervisor infrastructure | A documented external-agent path without Studio |
| Launch browser Activity Clients | Client bindings and one-use Supervisor handoff | A CLI-authorized launch entrypoint |
| Backup and recovery | Offline CLI operations; Supervisor live SQLite backup | Clear command boundaries and retained verification evidence |

Sources: [operator CLI](../crates/worldstream-server/src/bin/worldstreamctl.rs),
[daemon routes](../crates/worldstream-server/src/lib.rs),
[Supervisor lifecycle](../crates/worldstream-studio-supervisor/src/lifecycle.rs),
and [Supervisor modules](../crates/worldstream-studio-supervisor/src/main.rs).

Two limits matter when describing the current tools:

- `health` checks `/healthz`, not full readiness. `doctor` reports bounded local
  checks; it is not a complete operational health check.
- `pack inventory` covers installed portable Bundles. It is not a combined
  view of embedded Packs, next-start selection, and the running registry.

## Keep the boundaries clear

These are existing domain distinctions, not new concepts:

- A **Host Operator** manages the installation. Host authority does not grant
  a participant's private context or Action authority.
- A **Room** is one authoritative shared situation with one pinned Pack
  Revision. It is not a CLI working directory or a collection of Packs.
- An **Activity Pack** supplies rules and schemas. It is not an administration
  screen, model process, or mandatory browser application.
- An **Activity Client** participates through the authorized protocol. It can
  be a browser application, terminal application, SDK-based program, or agent
  integration. It executes independently of the operator interface.
- A **Runner** executes outside the kernel. Removing Studio does not move
  model execution into the Runtime.

The [domain glossary](../CONTEXT.md) distinguishes the reusable Room Setup
Specification from a particular Room Setup Operation and its progress.
General CLI syntax and file-format details remain implementation contracts,
not separate domain entities.

## Accepted architecture boundary

The CLI is a short-lived interface. It must not become a second Runtime.

```text
Host Operator
    |
    +-- worldstreamctl
          +-- offline config and Pack operations
          +-- authorized Runtime operator APIs
          +-- headless control service, when needed
                +-- managed daemon and Runner processes
                +-- protected host records and credential references
                +-- optional browser-client session broker

Independent Activity Clients and external Runners
    +-- authorized Runtime protocol
    +-- browser session broker for the current browser-client model

worldstreamd
    +-- sole authority for Room state, authorization, and history
```

Managed local operation will retain the Supervisor as a bounded headless
service. The CLI sends commands to that service, which owns managed daemon
and Runner processes without the Studio frontend. A healthy managed Runtime
survives CLI exit and Supervisor failure. Recovery must prove ownership before
control resumes; uncertainty must not trigger a duplicate daemon or unsafe
stop. The first local installation has one operating-system owner and uses a
separate protected control credential that the CLI reads automatically.
Local control uses authenticated loopback HTTP. Managed start is explicit;
read-only commands do not start services. Managed stop shuts down owned
managed Runners and then the Runtime, leaving the controller available.
Controller-only shutdown does not request Runtime shutdown. Current names,
state paths, and existing wire/receipt shapes remain compatibility contracts;
unauthenticated operator access does not. Exact implementation mechanisms and
the consolidated command spelling are covered by the implementation plan.

A foreground `worldstreamd` plus an external service manager must remain a
supported deployment pattern. Do not require the headless companion for
every direct SDK or external Runner connection.

### Constraints that must survive the change

1. **Process ownership must be real.** The current Supervisor owns a child
   process in memory. After a Supervisor restart it can detect that daemon,
   but it reports it as unmanaged and refuses to stop it. A CLI cannot start
   a child, exit, and assume a later invocation inherits ownership. Select a
   safe ownership-recovery strategy for the retained Supervisor before
   implementing background `start` and `stop`. Never stop an unrelated
   process by a stale PID alone.
2. **Shared stores need one mutation owner or interprocess coordination.**
   Some Supervisor stores use process-local locks. Calling those libraries
   from several CLI processes is not automatically safe. Initially keep
   these mutations behind the retained service.
3. **Browser sessions are not the Studio UI.** Current browser clients use
   the Supervisor for approved client selection, one-use handoff, and
   Membership-scoped sessions. Keep that broker while those clients need it.
   CLI issuance needs an authenticated operator entrypoint, not a fabricated
   Studio Origin header. Do not print long-lived bearer credentials in URLs
   or put them in command arguments or normal logs.
4. **Participation must be client-neutral.** The current setup code marks a
   human seat ready through browser-session health. It can report
   `ConsoleMissing` for a non-browser participant. Define a readiness path
   for supported terminal/SDK participants before claiming that the entire
   setup flow is client-neutral. A successful connection alone is not proof
   of valid authority or participant readiness.
5. **Pack installation remains explicit and offline.** Inspection, exact
   approval, installation, selectability, and restart readiness are distinct
   facts. Do not copy mutable package-manager upgrade semantics. No automatic
   trust, hot loading, or deletion of a Revision required by retained Rooms.
6. **Retirement must preserve data.** Do not delete Supervisor state,
   credentials, bindings, or setup records with the frontend. The crate also
   builds the assignment-MCP and managed-agent-host binaries.
7. **Local reachability is not operator authentication.** Ordinary Supervisor
   routes currently have no shared inbound caller-authentication check. Its
   default loopback address does not identify the installation owner, and its
   `--bind` argument is not restricted to loopback. Browser Origin checks apply
   to the handoff/session routes, not the whole Supervisor. Add a real local
   operator admission contract before exposing these operations through the
   managed CLI. Retain the existing, separate Supervisor-to-daemon Host
   authentication and Membership-scoped client credentials.
8. **Creation does not universally mean waiting in a lobby.** Heist 0.2.0
   starts in a Pack-defined lobby without phase timers; explicit `host_launch`
   starts briefing. Heist 0.1.0 starts briefing at Genesis. Negotiate 0.1.0
   schedules its formation deadline at Genesis and declares no host-launch
   seam. Derive start behavior from exact Pack metadata, not the Pack name.
   A CLI cannot pause domain time or add a new canonical phase on its own.
9. **Retry and edited input are distinct.** Room creation already persists
   immutable intent and an idempotency key before contacting the daemon.
   Existing retry reuses that intent, but it does not compare a subsequently
   edited draft against it. A CLI contract that rejects replacement input
   must add that check or accept only the retained operation reference on
   retry. Do not claim this input check already exists.
10. **Readiness needs honest evidence.** The Runtime already registers live
    Membership streams after a session-bound synchronization acknowledgement
    and removes them on disconnect, but exposes no Host-facing Membership
    connection/synchronization endpoint. The current browser broker's health
    check uses a retained session and a newly opened probe connection; it does
    not prove the actual browser is open or synchronized. A generic status
    endpoint must expose only bounded operational metadata, not participant
    Projections, Action Offers, Cursors, or private observations. Connection
    evidence and any operator confirmation are different facts. Agent
    readiness uses an eligible Runner, not a continuously running model.

Sources: [process ownership](../crates/worldstream-studio-supervisor/src/lifecycle.rs),
[Room setup and readiness](../crates/worldstream-studio-supervisor/src/task_setup.rs),
[browser handoff](../crates/worldstream-studio-supervisor/src/participant_handoff.rs),
[crate binaries](../crates/worldstream-studio-supervisor/Cargo.toml), and
[Pack installation decision](adr/0014-installable-wasi-free-activity-pack-bundles.md).

Additional checks: [Supervisor route composition](../crates/worldstream-studio-supervisor/src/main.rs),
[creation retry](../crates/worldstream-studio-supervisor/src/room_creation.rs),
[Heist lobby rules](../crates/worldstream-core/src/agent_heist_lobby.rs), and
[Negotiate initialization](../packs/negotiate/src/pack.ts).
The connection evidence comes from the Runtime's `LiveStreamRegistry` in
[the server](../crates/worldstream-server/src/lib.rs); the
[Python SDK](../sdk/python/src/worldstream_sdk/client.py) already uses the
session-bound synchronization acknowledgement.

## Accepted user experience

Keep `worldstreamctl` and its existing commands compatible. New aliases and
executable renaming are not part of this migration.

### Accepted usage model

- The first CLI administers an installation from the same machine as the
  Runtime. Operators can run it locally or through SSH on a server.
  First-class remote administration from another machine is deferred. This
  does not restrict existing participant or Runner protocol connections.
- A reviewable, reusable setup file is the main Room setup input. Provide a
  working example, fill fixed schema values automatically, and use declared
  defaults. Optional prompts collect missing choices and save them for reuse.
  Secrets and operational approvals remain outside that file. It describes
  one Room's creation settings and participant/Runner setup references, not
  desired state for the installation or an existing Room. The versioned JSON
  schema is an implementation deliverable under the accepted input contract.
- Support external Runner attachment and the existing approved reference
  Runner launch path. The reference path gives a beginner a working example;
  external attachment permits independently hosted agent implementations.
  Do not add a new general-purpose agent manager.
- The local human Heist example uses the existing standalone browser client
  through a secure CLI-issued handoff. A direct SDK/terminal participant path
  must also be verified. No browser is universally required, and the operator
  CLI does not implement Heist gameplay or a new terminal renderer.

These are accepted requirements for the replacement, not claims that the
corresponding CLI commands already exist.

The following is an interface sketch, **not commands available today**:

| Command family | User question it answers |
| --- | --- |
| `init` | How do I create a safe local configuration and state directory? |
| `server start / stop / restart / status / logs` | Is my configured server ready, and how do I control or diagnose it? |
| `config validate / effective` | Which configuration will this installation use? |
| `pack list` plus existing exact Pack operations | What is installed, selectable on next start, and loaded now? |
| `room validate / create / list / inspect` | Can I create this Room, and what is its current state? |
| Membership and Runner commands | How do authorized people and agents join this Room? |
| `client open` | How do I open an approved browser client for this Membership? |

Use readable output by default and explicit machine-readable output for
scripts. Preserve existing receipt formats. Errors should identify the failed
step and one safe next action. Noninteractive commands must not wait for
hidden prompts or silently grant approval.

### Configuration must not repeat the current form problem

- Fill fixed schema values automatically. Do not ask users to guess them.
- Use declared defaults for editable fields. Ask for, or report, only missing
  choices that genuinely need user input.
- Validate the complete configuration before creating the Room. Reject
  explicit values that violate a fixed constraint; do not silently change
  the user's supplied values.
- Supply one reviewed, runnable example setup file for Agent Heist. Reuse the
  same generic flow with Negotiate as a portability check.
- Keep Pack-specific values out of generic CLI code. The exact schema and
  reviewed example configuration supply them.
- Keep versioned JSON Room setup distinct from the existing TOML server
  configuration. Publish and validate its public schema without exposing
  private operation records or wizard state.

Changing the administration interface does not require changing existing
immutable Pack Revisions. Example setup files can provide required values
that the current schemas do not declare as defaults.

## Delivery plan, subject to approval

### Milestone 1: make local operation coherent

Complete the Supervisor ownership-recovery design. Add safe initialization
and a thin CLI surface for status, managed lifecycle, and logs. Reuse the existing config
and Pack operations. Clearly distinguish installed inventory from the live
registry. Keep direct foreground execution documented.

Verify from a fresh protected directory: initialize, validate, start, wait for
readiness, inspect inventory, stop, approve and install an exact Bundle while
offline, select it, prove restart readiness, start again, and confirm the
running revision. Use separate CLI invocations. Test duplicate starts,
unmanaged processes, startup failure, stop timeout, and secret-safe output.

### Milestone 2: complete one activity without Studio

Use an exact Pack and reviewed setup file to create and inspect an Agent
Heist Room. Provision the required Memberships and agent connections. Open
an optional standalone browser client through a secure handoff. Complete a
live activity with the required participants, then verify reconnect and
retained Room recovery. Include a supported non-browser participant path in
the acceptance matrix.

Reuse durable creation and provisioning operations. Do not duplicate their
retry and reconciliation logic in shell scripts. Use Negotiate to check that
the commands are not hardcoded to Heist; do not require a new Negotiate UI.

This is the accepted Studio retirement gate. Merely starting a daemon
would leave the user's original problem—getting an activity working—unsolved.

### Milestone 3: remove the web product and update release evidence

Remove Studio from onboarding and normal development startup. Decouple
Activity Client builds from Studio. Remove the frontend and web-only tests
after the replacement flow passes. Keep useful backend and security tests.
Do not port the attention inbox, dashboard, or visual editors just for parity.

Update requirements, architecture decisions, roadmap, manual, examples,
package scripts, CI, and release inventories together. The release currently
has a closed signed-subject inventory that includes Studio. Removing one
artifact without updating that contract is not a valid release change.
Preserve historical verification of already defined release formats.

Sources: [workspace scripts](../package.json),
[local Client Host](../scripts/serve-activity-clients.mjs),
[release inventory decision](adr/0016-expanded-runtime-pack-release-and-qualification.md),
and [release subject validation](../scripts/starter-release-subjects.py).

### Explicitly defer

Do not add a replacement web dashboard, rich TUI, hosted control plane,
remote cluster manager, public Pack registry, new agent orchestration engine,
or automatic model-driven operator. Rich template editors and attention
history are not prerequisites for a useful CLI.

The existing prompt-assisted Pack-authoring commitment is separate. Do not
silently remove it, or move Pack authoring into the operator CLI, as part of
retiring Studio.

## Decision record

Q1–Q18 are approved. The design tree is complete, including the consolidated
implementation contract and final confirmation. Exact
internal types, ownership proofs, and transport implementation must satisfy
these decisions and their tests; they are engineering work, not silently
expanded product scope.

```text
Q1. Retirement boundary — accepted
    Q3. Same-machine administration first — accepted
        Q6. Headless Supervisor for managed operation — accepted
            Q9. Healthy Runtime survives controller failure — accepted
                Q16. Explicit managed start/stop, no boot service — accepted
            Q11. Single OS-owner and protected local control credential — accepted
                Q17. Authenticated loopback HTTP; explicit credential rotation — accepted
    Q14. Additive CLI; retain state/wire/receipt compatibility — accepted

Q2. First usable outcome — accepted
    Q4. Reviewable reusable setup file — accepted
        Q7. Creation settings and setup references for one Room — accepted
            Q13. Versioned JSON with schema and examples — accepted
            Q10. Resume immutable retained setup intent — accepted
    Q5. External and approved reference Runners — accepted
        Q15. Required-seat, client-neutral readiness assessment — accepted
    Q8. Standalone browser reference plus direct SDK/terminal path — accepted
        Q12. Keep exact Pack start behavior unchanged — accepted

Both branches settled: Q1–Q17
    -> consolidated contract, migration order, and verification checklist
    -> Q18 final shared-understanding confirmation — approved
    -> dependency-ordered implementation tickets — authorized
    -> implementation and verification — not yet completed
```

**Q1 — Retirement boundary:** Should Studio stop being a supported operator
web product, while headless operations and independent Activity Clients stay?

Accepted: yes. Retire the frontend after the agreed replacement gate.
Do not delete the Supervisor wholesale or remove participant browser clients.

**Q2 — First usable outcome:** Is server/Pack administration sufficient, or
must the replacement also let a new developer complete a real Room flow
without Studio?

Accepted: ship administration as the first slice, but require one
complete Heist flow before removing the supported Studio path. Verify generic
behavior with Negotiate. Do not require full screen-for-command parity.

**Q3 — Administration location:** Must the first CLI administer a remote
server from a laptop, or can it run on the Runtime's machine?

Accepted: same-machine administration first, including commands run over SSH
on a server. Defer first-class remote administration without making the
Runtime or its participant protocol local-only.

**Q4 — Setup input:** Should users repeat an interactive wizard or reuse a
saved setup file?

Accepted: a reviewable setup file is the primary input. Supply a working
example, populate fixed values and declared defaults, and let optional prompts
collect and save missing choices. Do not store secrets or approvals in it.

**Q5 — Agent execution:** Must users bring running agents, or can WorldStream
launch a reference integration?

Accepted: support both external Runner attachment and the existing approved
reference Runner launch path. Keep model execution outside the Runtime. Do
not build a new general-purpose agent manager.

### Round 3 — accepted decisions

**Q6 — Background process ownership:** Should managed operation use a
persistent headless companion, or rely entirely on an external service
manager?

Accepted: reuse a bounded headless Supervisor for managed local
operation, accessed through the CLI. It owns managed daemon/Runner processes
and required host operations, with no Studio frontend. Keep foreground
`worldstreamd` and externally managed deployments supported. Define safe
startup, shutdown, and ownership recovery before claiming background
management is complete; the existing in-memory child handle is insufficient
after a Supervisor crash.

**Q7 — Setup-file boundary:** Does a setup file describe one Room or the
desired state of an entire installation?

Accepted: one Room's creation settings and participant/Runner setup
references. Keep server configuration, artifact installation, and operational
approval separate. Reuse existing draft/template semantics while removing
wizard-specific metadata. A setup input is not an Activity Distribution and
not a mutable desired-state definition that rewrites an existing Room.
Q10 and Q13 settle its retry and input-format boundaries. The public schema
is an implementation deliverable, not the persisted Studio draft format.

**Q8 — Human participation in the reference flow:** Should the local Heist
example reuse the standalone browser client or require a new terminal game
interface?

Accepted: use the standalone Heist browser client for the local human
example, launched through a secure CLI-issued handoff. Preserve direct
SDK/terminal participation as an independent protocol path and verify that
setup does not require a browser for every human. Do not implement Heist
gameplay in the operator CLI or build a new terminal renderer. Remote browser
handoff remains outside the accepted loopback browser contract.

The dependent product choices are recorded in Q9–Q17 below.

### Round 4 — accepted decisions

**Q9 — Controller failure:** Should a healthy Runtime stop when the CLI exits
or its Supervisor fails?

Accepted: no. In managed mode, Runtime shutdown is an explicit operation, not a side
effect of closing a terminal client or losing the controller. On recovery,
verify ownership before resuming control. If that proof is unavailable,
report the process as unmanaged and block unsafe stop/restart or duplicate
start. This requires recovery work beyond today's in-memory child handle;
automatic operating-system boot startup remains a separate decision.

**Q10 — Interrupted Room setup:** After a timeout or partial failure, should
retry use the retained setup intent or read the possibly edited input again?

Accepted: persist an immutable, fully resolved creation/setup intent
and a protected operation record before sending mutations. Resume by that
operation reference using existing idempotent operations. Retry must not
accept replacement configuration or silently create another Room. A changed
recipe requires a new explicit creation attempt. Status must expose unfinished
operations so users can recover without knowing internal database IDs.

**Q11 — Local operator access:** Is the first managed installation owned by
one operating-system user, or does it need a shared multi-user administration
system?

Accepted: one operating-system owner initially. Initialize a separate
protected local control credential and let the CLI read it automatically;
do not require routine token copying or login. Authenticate privileged
Supervisor operations, constrain the local control listener, and keep that
credential separate from daemon Host, Membership, Runner, and model-provider
credentials. Loopback reachability and a Studio Origin header grant nothing.
Shared administrative accounts and first-class remote control remain deferred.
Q17 settles the transport and credential lifecycle boundary.

**Q12 — Pack changes during migration:** Should the CLI migration also add a
new Negotiate revision with a lobby, or preserve its current start-at-creation
behavior?

Accepted: preserve current Pack rules in this migration. Validate the
complete recipe and available dependencies before creating a Room. For a
declared lobby, create and provision the Room, check readiness, then explicitly
launch it. For an active-at-Genesis Pack,
disclose that creation starts the activity and its timers; do not imply that
it can wait indefinitely for participants. Use Heist 0.2.0's existing lobby
for the beginner flow. Keep current Negotiate rules unchanged and test its
active-at-Genesis path separately. Adding a generic pause or changing retained
Pack rules is outside this operator-interface migration.

Q9–Q12 are accepted. Q13–Q17 settle the remaining product-contract choices.

### Round 5 — accepted product-contract decisions

**Q13 — Setup-file format:** Should the first public setup input support
multiple formats or one versioned, schema-validated JSON format?

Accepted: one JSON format first, with an editor schema and a generated
example. Resolve fixed values and declared defaults before validation and
review, while rejecting conflicting explicit values. Reuse the existing
draft/template meaning, not its wizard steps or private per-operation records.
Keep server configuration in its existing TOML format. Do not add executable
configuration, secret expansion, or several equivalent input parsers in this
slice. A later convenience format must resolve to the same validated input.

**Q14 — Command and compatibility contract:** Should the new CLI replace the
current executable and command names, or extend them?

Accepted: extend `worldstreamctl`; retain existing commands, receipt
formats, state locations, and wire compatibility. Group new operations under
`server`, `room`, `runner`, and `client`, alongside existing `config` and `pack`
commands, with a top-level safe initialization operation. Room setup status
and resume remain Room operations, not a general workflow engine. Use readable
output for new interactive commands and explicit stable JSON for scripts.
Preserve current machine-output behavior for existing commands. Automation
must not wait for prompts or infer approval. Add no large rename or automatic
state relocation to the Studio retirement work.

**Q15 — Readiness policy:** Must every seat have a connected browser, or can
the operator use a client-neutral launch assessment?

Accepted: the setup input declares which seats are required for launch.
Keep provisioning and authority validity mandatory. For humans, report fresh
authenticated/synchronized connection evidence from any supported client, not
just broker-session existence. Where technical presence cannot be observed,
an explicit policy may accept a separately labelled operator confirmation;
never call that a verified connection. For agents, use the exact eligible
Runner assignment, fresh presence, compatibility, and capacity. Optional seats
need not block launch. These checks are operational snapshots, not new
canonical Room state, proof of human consent, or guarantees of future action.
For active-at-Genesis Packs, perform available preflight before creation and
report post-creation readiness without pretending timers are paused.

**Q16 — Managed service lifecycle:** Should ordinary commands install a boot
service or start background processes implicitly?

Accepted: explicit managed-start starts or reuses the owner-local
Supervisor and starts its configured Runtime. Read-only status/list commands
never start processes. Managed-stop first stops owned managed Runners for the
installation, then gracefully stops the Runtime, leaving the Supervisor
available for status and later restart. It does not kill external Runners or
unmanaged daemons. A separate explicit controller shutdown is available;
managed restart or controller failure does not promise that a model Invocation
can resume its process state. Do not install operating-system boot services in
this slice. Continue to support external service management and foreground
operation. Timeouts and partially stopped states must remain visible; do not
silently escalate to killing unrelated or unverified processes.

**Q17 — Local control transport and credentials:** Should the first managed
CLI add platform-specific control channels or reuse the existing local HTTP
service with explicit authentication?

Accepted: authenticated loopback HTTP for the initial supported
platforms, reusing typed operations. Enforce the loopback bind, require a
dedicated high-entropy local control credential for every privileged entry
point, and leave no unauthenticated legacy alias as a bypass. Initialize that
credential in owner-only state through an explicit initialization/migration
step; the CLI reads it internally, not from command arguments or setup files.
Rotation is explicit and must not replace the daemon bootstrap authority or
participant/Runner credentials. Existing browser cookie/Origin and one-use
handoff protections remain separate; CLI issuance uses authenticated Host
operations, never a spoofed Studio Origin. Any transitional Studio access must
preserve this admission boundary or be cut over with the verified replacement.

**Q18 — Final shared-understanding confirmation:** Does the consolidated
contract, including command spellings, migration order, and managed-Runners
restart behavior, reflect the agreed product?

Approved. Managed restart restores only previously running managed assignments
that remain approved, compatible, and eligible. It never starts external or
previously inactive Runners. Partial restoration is reported explicitly; model
process-memory continuation is not promised. The design review is complete,
and preparation of implementation tickets is authorized.

The [implementation plan](cli-first-implementation-plan.md) and
[ticket plan](cli-first-ticket-plan.md) record the approved interface,
migration ordering, and evidence checklist. Low-level implementation
choices must satisfy the accepted safety and compatibility contracts; they
are not permission to expand scope or claim unverified recovery.

## Documentation and release migration

ADR 0018 records the accepted direction and conditional retirement. The
getting-started guide now leads with existing CLI commands and separates
them from the pending replacement. The remaining migration must preserve
these boundaries:

- [ADR 0013](adr/0013-studio-companion-control-plane.md): supersede the Studio
  web product choice; retain the Runtime and external-agent boundaries.
- [ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md):
  supersede Studio as the required operator surface; retain independent
  Activity Clients and exact authorization/binding rules.
- [ADR 0016](adr/0016-expanded-runtime-pack-release-and-qualification.md):
  record the new release inventory contract when the artifact set changes.
- [Requirements](requirements.md), [roadmap](roadmap.md),
  [Studio guide](studio.md), and [getting started](getting-started.md): remove
  required browser administration only after the replacement is verified.

Existing Studio behavior and release artifacts remain in force during the
transition. The accepted direction changes no Runtime behavior and removes
no application by itself. Final confirmation and ticket preparation are
approved. Nothing in this decision record is evidence of completed code.
