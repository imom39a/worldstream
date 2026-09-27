---
status: accepted
date: 2026-09-02
---

# Make the Host Operator interface CLI-first

## Delivery-phase clarification — 2026-09-03

The user directed MVP/POC delivery first and production hardening later. The
verified MVP replacement gate passed and the Studio web application was
retired on 2026-09-03.
The usable CLI-only Heist flow, Negotiate check, and getting-started guide take
priority. Existing safety boundaries remain; critical defects are fixed when
found. Automatic bound-Runner restoration, optional interactive setup UX, and
expanded failure/platform qualification are follow-up work in IMO-147. An
unsupported bound-Runner restart must reject before process mutation. The
long-term decisions below are not a claim that the MVP is production-qualified.

WorldStream will use the operator CLI as its primary administration interface
and has retired the Studio web app after verifying the replacement.
Maintaining a second administration interface before the core setup flow is
usable adds cost without resolving the main adoption problem. Keep the
Runtime, reusable headless operations, SDKs, external Runners, and independent
Activity Clients; do not replace them or delete the Supervisor wholesale.

Server, configuration, and Pack administration are the first delivery slice.
The retirement gate required a new developer to complete a live Agent Heist
Room flow without the administration website, including participant and
agent setup. Check the same generic setup with Negotiate. Full
screen-for-command parity is not required.

The first CLI administers the installation from the Runtime's machine,
including commands run over SSH. First-class remote administration is
deferred, not external participant or Runner protocol access. Room setup
uses a reviewable, reusable file with fixed schema values and declared
defaults supplied automatically; optional prompts collect missing choices.
The file contains no secret values or operational approvals. Support external
Runner attachment and the existing approved reference Runner launch path
without adding a general-purpose agent manager.

Managed operation uses a bounded headless Supervisor to own managed Runtime
and Runner processes; direct foreground and externally managed deployments
remain supported. The setup file describes creation settings and participant
setup references for one Room, not installation-wide desired state or
replacement state for an existing Room. The local human reference flow uses
the standalone Heist browser client through a secure CLI-issued handoff;
direct SDK/terminal participation must also be verified. The operator CLI
does not implement Pack-specific gameplay.

For managed operation, a healthy Runtime survives CLI exit and Supervisor
failure. The recovering Supervisor must prove ownership before controlling
it; uncertainty blocks unsafe stop/restart and duplicate startup. The first
installation has one operating-system owner and a separate protected local
control credential, read automatically by the CLI. Local reachability or a
browser Origin header is not operator authentication.

Room setup persists its resolved immutable intent before making changes.
Retry resumes that saved operation, not a newly read or edited recipe, and
must not silently create a second Room. The migration preserves exact Pack
start semantics: use the existing Heist 0.2.0 lobby for the beginner flow and
disclose Negotiate's start-at-creation behavior. It adds no generic pause and
does not change retained Pack rules.

The public setup input is one versioned, schema-validated JSON format with
generated examples; server configuration remains TOML. Extend `worldstreamctl`
with bounded server, Room, Runner, and client operations. Preserve current
commands, receipt/output contracts, state locations, and wire shapes. New
commands have readable output and explicit machine-readable output; scripts
must not wait for hidden prompts or infer approval.

Readiness is a Host-local operational assessment of required seats, not a new
canonical Room value. Distinguish valid provisioning/authority, observed
authenticated connection/synchronization, eligible Runner availability, and
any explicitly permitted operator confirmation. Confirmation cannot become a
false connection claim or bypass Pack rules and authorization. Optional seats
need not block launch.

Explicit managed-start starts or reuses the Supervisor and starts its
configured Runtime. Read-only commands start no processes. Managed-stop stops
owned managed Runners and then gracefully stops the Runtime, leaving the
Supervisor available; external processes are not stop targets. Controller-only
shutdown is separate and leaves the Runtime running. Do not install OS boot
services. Runtime survival does not promise browser-session, Runner-process,
or model-Invocation continuity.

Managed restart records the previously running managed Runner assignments and
restores only that set after rechecking approval, compatibility, and eligibility.
It never starts external or previously inactive Runners. Failed restoration
is an explicit partial result, not a successful full restart or a promise of
model process-memory continuation.

Use authenticated loopback HTTP for initial local control. Require the
separate owner-protected credential on every privileged operator entry point,
without an unauthenticated legacy bypass. Initialize or rotate it explicitly;
do not replace daemon bootstrap or participant/Runner authority. Preserve
browser cookie, Origin, and one-use handoff boundaries separately. Requiring
operator authentication is an intentional admission change, not a violation
of wire-format compatibility. Authentication and frontend retirement were
coordinated at the verified cutover.

## Consequences

- The Studio web interface is retired. Keep the CLI and Controller Pack-neutral.
- Retain the Supervisor's necessary headless capabilities initially. Exact
  internal types, ownership-recovery proofs, and implementation layout must
  satisfy the accepted contract and its tests. They do not authorize a new
  process manager, canonical readiness state, or general workflow engine.
- Preserve Room authority, immutable Pack identity, offline installation,
  authorized participant access, and independent client execution. Host
  authority never grants implicit participant authority.
- Getting-started documentation must show runnable current commands and
  label planned commands as unavailable. An automated reference test does
  not prove that manual CLI Room setup is complete.
- Partially supersede ADR 0013's Studio web product choice and ADR 0017's
  permanent Studio-as-operator-surface commitment. Their authority and client
  boundaries remain in force. Studio-specific interface obligations are historical.
- Use the CLI-first successor release inventory. Continue to verify historical
  formats as compatibility evidence. This ADR does not claim release qualification.

See the [CLI-first proposal](https://github.com/imom39a/worldstream/blob/ac7f443562bce33088229351d5f9eba40bb67bdd/docs/cli-first-operator-proposal.md) for the source
audit and approved Q1–Q18 decisions. The consolidated
[implementation plan](https://github.com/imom39a/worldstream/blob/ac7f443562bce33088229351d5f9eba40bb67bdd/docs/cli-first-implementation-plan.md) is confirmed and
the design review and MVP cutover are complete. Production hardening and
external release qualification remain separate work.
