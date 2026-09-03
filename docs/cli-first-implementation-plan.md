# CLI-first implementation plan

Status: Q1–Q18 approved on 2026-09-02. The design review is complete.
Implementation was authorized on 2026-09-03 and is in progress. No completed
replacement, frontend removal, or release qualification is claimed.

This plan implements [ADR 0018](adr/0018-cli-first-operator-surface.md).
The [decision record and source audit](cli-first-operator-proposal.md) retain
the reasoning. This document collects the work into one usable contract; it
does not replace the Runtime, Pack, client, or authority specifications.

## MVP delivery update — 2026-09-03

The user confirmed that this delivery is an MVP/POC. Prioritize a usable
CLI-only Heist flow, verify the same setup contract with Negotiate, and provide
one working getting-started guide. Fix critical defects as they appear; do not
expand production hardening before this flow works.

Keep implemented credential protection, ownership checks, bounded input, and
durable setup reuse. Defer automatic restoration of bound Runner instances,
additional fault-injection/platform qualification, and optional interactive
setup UX to [IMO-147](https://linear.app/imom39a/issue/IMO-147/post-mvp-hardening-for-cli-managed-installations).
An unsupported bound-Runner restart must reject before stopping that Runner;
use explicit stop/start and deliberate agent startup instead. Keep existing
working behavior and tests. The long-term requirements below remain the target,
not a claim of MVP production qualification.

## Outcome

A developer can operate a local installation, configure one Room, connect
humans and external agents, and inspect progress without Studio. The operator
uses `worldstreamctl`. Participants use independent Activity Clients.

The first complete example uses the existing Heist 0.2.0 lobby and standalone
browser client. A direct SDK participant must also work. Negotiate checks the
same generic setup path with its existing start-at-creation semantics.

Retire only the Studio web product. Preserve useful headless services, exact
Pack identities, retained Room data, and independently executing clients.

## Boundaries

| Component | Owns | Does not own |
| --- | --- | --- |
| Operator CLI | Input parsing, explicit user intent, typed requests, readable/JSON output | Pack rules, participant UI, a second setup state machine |
| Headless Supervisor | Local operator admission, managed processes, protected host records, durable setup coordination, browser handoff | Canonical Room state, model policy, arbitrary shell execution |
| Runtime | Authoritative Room state, authorization, protocol, observation delivery, recovery and Replay | Administration screens or a generic paused-workflow state |
| Activity Pack | Exact rules, configuration schema, Roles, phases, and declared launch behavior | Host approval, process management, client deployment |
| Activity Client / Runner | Scoped participation or external agent execution under its own authority | Implicit installation-wide authority |

Thin CLI means a narrow responsibility, not zero backend work. Authentication,
safe process recovery, schema-driven setup, and truthful readiness need changes
in headless services and the Runtime gateway. They do not require rewriting
the canonical Room Kernel.

## Approved public command contract

The names below are the approved public command spellings. **These
commands are not available yet unless explicitly marked as existing.** Public
flags, schemas, and exit statuses must be written down and tested before the
new interface ships. Internal Rust names and HTTP route layout are engineering
details, not new product decisions.

The additive parser and output contract is recorded in the
[operator CLI reference](cli-reference.md). A listed command that returns
`not_implemented` is not an available backend operation.

| Interface | Behavior |
| --- | --- |
| `init` | Create or validate protected local configuration and control access; optionally import explicitly reviewed local prerequisites; preserve existing authority and data; start no services |
| `server start / stop / restart / status / logs` | Operate the configured managed installation and report failures honestly |
| `server controller-stop` | Stop only the controller; do not request Runtime shutdown |
| `server rotate-control-credential` | Explicitly rotate local control access without changing other authority |
| Existing `config`, `health`, `doctor`, `version` | Preserve current commands, flags, scope, and machine output |
| Existing `pack` commands | Preserve exact offline approval/install/selectability/readiness/export/removal semantics and receipts |
| `pack list` | Display installed, next-start, and running facts separately; mark unavailable or stale evidence |
| `room example / validate / create / list / inspect / launch` | Generate a reviewable input, validate it, create/provision a Room, inspect it, and invoke only its declared launch seam |
| `room setup status [operation]` | List unfinished setup operations or inspect one operation's checkpoint and next action |
| `room setup resume <operation>` | Continue the retained intent; accept no replacement input file |
| `runner list / inspect / start / stop` | Inspect external/managed assignments and control only approved managed integrations |
| `runner export-credentials` | Deliver only the selected Runner-control credential to a protected file |
| `client open` | Open the selected approved browser client through a scoped one-use handoff |
| `client export-credentials` | Deliver only the selected Membership credential to a protected file for a direct client |

External Runner attachment is a complete flow, not necessarily another command:
setup provisions the assignment, scoped credential delivery supplies the
external process, and that process connects through the existing protocol.
An agent can need both Runner and Membership authority; deliver them separately.
Do not add a generic secret-export command that mixes those scopes.

New commands have readable output plus explicit stable JSON. Existing commands
keep their current output defaults and receipt formats. Diagnostics go to
stderr so JSON stdout remains parseable. A noninteractive mutation must have
complete input and explicit approval where required; it must not wait for a
prompt or turn a generic confirmation into trust for unknown artifacts.

Read-only commands do not start the controller or daemon. An unavailable
controller is a reported condition, not a reason to spawn one. A combined
Pack display must not stop a running server to perform offline verification,
or present an old inventory snapshot as freshly verified.

## Setup input and retained operations

Publish one bounded, versioned JSON Room Setup Specification and its editor
schema. Reuse existing draft/template semantics but exclude wizard state,
duplicated readiness rows, generated capability values, and private execution
records. The public input carries:

- one exact Activity Pack reference;
- Pack configuration, with supplied constants/defaults and required choices;
- stable seat labels, Roles, Principal kind and permitted identity references;
- required-seat readiness policy;
- external or managed agent assignment intent and exact approved-dependency
  references where needed;
- explicit supported read-only Operator Membership opt-in, never implicit
  operator access to participant-private data.

Server configuration stays TOML. Pack/client installation and operational
approvals stay outside Room setup. Existing approved Client Bindings determine
browser launch; a setup file does not authorize a new client target. No
executable config, arbitrary environment expansion, secret values, or mutable
package tags can become exact authority through this input.

Generation may prompt for genuinely missing choices and save the result.
Validation resolves declared fixed/default values and rejects explicit
conflicts, unknown fields, malformed references, and unsupported schema
constructs. It must not fall back to Heist-specific field tables. Do not ask
users to supply raw internal database IDs just to run the first example.

Before mutation, retain the exact resolved intent and a Room Setup Operation
reference. Resolve per-attempt identities once, then reuse them on retry.
Creation and provisioning reuse existing durable operations and their
idempotency rules. If a response is lost, show the retained operation and
reconcile it before issuing another mutation. If setup partly succeeds, report
the created Room and the incomplete provisioning stage; do not claim the entire
attempt rolled back or silently discard retained work.

Resume uses that operation alone. A modified file requires an explicit new
creation attempt. A setup file is not a desired-state controller for an
existing Room. Room inspection and setup-progress inspection remain distinct.

## Managed lifecycle and security

The first controller operates for one OS-user-owned local installation. Use
authenticated loopback HTTP with a distinct owner-protected control
credential. The CLI reads it internally. Preserve daemon Host, Membership,
Runner, and provider credentials as separate scopes.

Initialization is explicit. It must not regenerate missing bootstrap
authority beside retained state, overwrite existing configuration, silently
rotate credentials, relocate state, or install OS boot services. Existing
state missing the new control credential requires a reviewed initialization
or migration step, not a guessed reset of the installation.

Fresh-state implementation must also prepare dependencies that Studio or test
scripts currently register. Bounded, explicit `init` import flags may reference
the existing versioned Agent Profile, Runner Template, provider-credential-name,
and first-party Client Release/Deployment/Binding descriptors. This is local
initialization, not a second Room setup contract or a desired-state controller.
Bind executable/client approval to the exact reviewed inputs and targets;
re-import must preserve immutable identities and disabled/revoked bindings.
Read provider secrets only from explicitly selected protected input and retain
named references, not secret values, in declarations. External-only setup needs
no managed provider credential. `runner start` and `client open` must not import
or approve missing prerequisites. Starting the independent Client Host remains
a separate documented step; opening a client does not deploy arbitrary code.

Managed start creates or reuses one verified controller and starts its
configured daemon. Singleton startup and process identity must prevent races,
duplicate daemons, PID-reuse mistakes, and control of another installation.
Detach managed process I/O safely so closing the launching terminal does not
break the daemon. Logs stay bounded and secret-safe.

Managed stop first shuts down owned managed Runners, then gracefully stops
the daemon. Do not kill external processes. Timeouts and partial shutdown are
visible outcomes, not success. Keep the controller available for inspection.
Controller-only stop leaves the Runtime running.

Managed restart records the previously
running managed Runner set, performs the coordinated restart, and restores
only those assignments after checking current approval, compatibility, and
eligibility. It never starts external or previously inactive Runners. Failed
restoration produces an explicit partial result. A restarted Runner can
perform new Invocations through the existing lease/fencing contract; no model
process-memory continuation is promised. Q18 approved this restart
postcondition as part of the consolidated contract.

A healthy Runtime survives controller failure, but this is **not** a promise
that browser sessions, Runner processes, or model Invocations survive. Restore
safe ownership and reconnectability; do not claim restored process memory.
If ownership cannot be proved, report unmanaged state and block unsafe control
or duplicate startup. Credential rotation must not make healthy Room authority
or retained history unusable.

Authenticate every privileged control entry point. Preserve schema and route
compatibility where promised, not unauthenticated access. No old alias can
bypass the new admission check. Browser Membership routes retain their own
cookie, Origin, revocation, and one-use handoff checks; they do not receive the
installation control credential. CLI launch must not impersonate Studio's
Origin header.

Credential export requires an explicit target and writes only owner-protected
files without silently overwriting them. Never print bearer values in ordinary
stdout, JSON reports, logs, command arguments, or setup examples.

## Readiness and activity start

Keep these facts separate: provisioning/authority validity, observed connection
evidence, Runner availability, and any permitted operator confirmation.

- Human connection evidence must come from current authenticated,
  synchronized Membership sessions. Reuse Runtime delivery bookkeeping and
  expose a bounded Host-authorized metadata view, not participant content.
- An agent needs a valid assignment and a fresh compatible Runner with
  capacity. It does not need a permanent model process or participant socket.
- Explicit policy may permit operator confirmation where presence cannot be
  observed. Label it as confirmation, not a verified connection, and never
  let it bypass authority or Pack legality.
- Required seats gate a declared launch. Optional seats need not. Readiness
  is a current operational assessment, not consent or a guarantee of future
  action. It does not enter canonical Room hashes or Replay.

Heist 0.2.0 creates a lobby, so the flow is validate → create/provision →
connect participants → inspect readiness → explicit launch. The live client
starts empty and installs only its authorized Projection Reset.

The Heist reference proves external-agent participation. Verify the existing
managed reference Runner separately against a Pack its exact template supports
(the checked-in managed fixture currently declares Counter v4). Do not infer
Heist compatibility from the presence of a managed host binary. A broader
compatibility declaration requires its own exact reviewed template and proof.

Negotiate starts at Genesis. Validate available dependencies first, disclose
that creation starts its activity and deadlines, then report provisioning and
connection progress honestly. Never imply that incomplete setup pauses time.
Do not add a generic Runtime pause or change a retained Pack revision.

## Delivery order and review gates

These are implementation milestones, not separate claims of a supported
release. Q18 approved preparation of dependency-ordered implementation tickets.
The [ticket plan](cli-first-ticket-plan.md) records the implementation frontier
and verification ownership.

| Milestone | Work | Exit evidence |
| --- | --- | --- |
| 1. Local operator foundation | Freeze CLI/schema contracts; protected init and control admission; process ownership/recovery; status/logs; preserve existing config/Pack commands | Fresh and retained-state tests; authenticated route matrix; duplicate-start, terminal-exit, crash/restart, rotation, partial-stop, and wrong-process negatives |
| 2. Generic Room setup | Versioned JSON, schema defaults, examples, complete validation, immutable setup operations, status/resume, existing provisioning | Valid and invalid Heist/Negotiate inputs; no hardcoded Pack fields; lost replies and restart at each mutation checkpoint; no duplicate Room on resume |
| 3. Participation and reference proof | Approved reference Runner and external Runner flow; scoped credential delivery; client-neutral readiness; secure browser launch | Complete local Heist flow with human and agent participation, separate direct-SDK path, Negotiate start-at-creation flow, privacy/revocation/reconnect tests |
| 4. Coordinated Studio retirement | Remove frontend/development coupling; enforce final operator admission; preserve backend services and state; update docs, packaging and release verification | Clean-checkout walkthrough without Studio, retained-installation upgrade, client builds independent of Studio, complete replacement regression and artifact-inventory tests |

The default rollout is a coordinated verified cutover. Keep the currently
supported path until the replacement passes its gate. Develop and test the
hardened CLI path without publishing an intermediate combination of protected
new routes and unprotected legacy routes. At cutover, enforce authentication
and retire the web surface together. A temporary Studio access adapter is
allowed only if necessary and equally authenticated; a second permanent web
administration product is not part of the plan.

Do not delete Supervisor records, the credential vault, client bindings, or
setup state with the frontend. Preserve current state paths and compatibility
names. If a data change is actually required, specify and test an explicit
recoverable migration before applying it. Do not rename the Supervisor crate
merely to remove the word Studio; it also provides agent-integration binaries.

Replace Studio-dependent tests with CLI entry points where they prove required
behavior; retain backend, authorization, and client conformance tests. Do not
port dashboards, attention inbox screens, or visual editors just for parity.

The closed release artifact inventory needs a versioned successor when Studio
is removed. Preserve verification of historical formats and their original
artifact sets. Do not globally erase Studio from every historic verifier or
claim existing signed evidence covers a changed payload set. Genuine release
qualification gaps remain gaps until real evidence exists.

## Completion checklist

- [ ] A new developer follows one guide using shipped or built CLI tools,
  with no Studio step, Rust edit, or manual internal-ID lookup.
- [ ] Fixed schema values populate correctly, editable defaults remain
  editable, and invalid explicit values fail before Room creation.
- [ ] Server/Pack operations work across separate CLI invocations, with
  truthful installed-versus-running state and preserved offline safeguards.
- [ ] Managed start/stop, terminal exit, controller failure, and recovery work
  on supported targets; no wrong-PID, wrong-installation, or duplicate control.
- [ ] Heist completes with a human client and agents. A separate direct SDK
  path proves that a browser is not universally required.
- [ ] Negotiate uses the same generic setup interface without changing its
  Pack identity, rules, or start-at-creation semantics.
- [ ] Interrupted creation/provisioning resumes the saved operation without
  duplicate Rooms, secret loss, or silently altered input.
- [ ] Readiness exposes no participant-private data and does not equate a
  retained browser cookie or operator confirmation with a live connection.
- [ ] Unauthorized local callers and legacy-route bypass attempts fail.
  Credential files, stdout, logs, JSON, browser handoffs, and revocation pass
  their scope and secret-handling tests.
- [ ] Old local state remains usable; old command/receipt and supported
  wire-format fixtures still pass, apart from the deliberate auth admission
  change. Foreground and externally managed deployments remain supported.
- [ ] Activity Client builds, onboarding, and normal startup do not require
  Studio. Required backend and agent-integration binaries remain available.
- [ ] Successor release inventory and historical verification tests pass.
  Unavailable external qualification is not replaced by synthetic receipts.
- [ ] Getting started is rerun from a fresh checkout and an existing
  installation. Planned-command labels change only when those commands work.

## Explicit exclusions

No kernel rewrite, workflow engine, universal paused Room, Pack-specific
operator renderer, replacement dashboard/TUI, new agent framework, remote
administration product, automatic OS boot service, public marketplace, broad
renaming, or automatic trust. Existing Pack-author prompt assistance remains
a separate commitment, not a new LLM-driven operator feature.

## Final confirmation

Q18 is approved. The user confirmed the consolidated contract, command
spellings, managed-Runners restart postcondition, and gated retirement plan.
The design review is complete and implementation ticket preparation is
authorized. Ticket creation is not code implementation, Studio removal, or
release qualification; those outcomes require the evidence above.
