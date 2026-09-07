---
status: accepted
date: 2026-09-04
---

# Operate a single-authority hobby preview

## Context

[ADR 0019](0019-separate-hosted-activity-platform-from-worldstream.md)
separates the Hosted Activity Platform from WorldStream,
[ADR 0020](0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md)
splits HTTPS product control from direct browser streams, and
[ADRs 0021 through 0023](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)
freeze formation, House Agent, and platform-store boundaries. They do not make
the current release OCI image a safe hosted deployment or define how one person
operates the inexpensive public preview.

The MVP is a hobby project. Multiple Runtime authorities, a second cloud
environment, zero-downtime volume migration, and production disaster-recovery
machinery would obscure the product proof. Exposing the existing mixed
operator router, relying on serverless WebSockets, automatically stopping the
Room authority, or treating provider snapshots as verified backups would make
the cheaper topology incorrect rather than merely less available.

## Decision

Operate the hosted MVP as a downtime-tolerant public preview with one
WorldStream authority and no uptime SLA. One **Hosted Deployment Revision**
binds one Git commit to the Vercel deployment, source-built Fly image digest,
Supabase migration head, Activity Listing Revisions, Activity Pack Bundle
digests, Activity Client Releases, and Result Projector Revisions deployed
together. It is deployment evidence only and must not be presented as a
verified WorldStream release while the repository's release contract remains
unresolved.

### Deployment topology

The preview has one production cloud stack and one local-development stack:

- Vercel serves the Vite product, independently built Activity Clients, and
  fixed-route HTTPS BFF. It terminates no WebSocket.
- Fly runs one continuously available Machine with one persistent volume and
  one preview host image. Runtime and headless Controller listeners remain on
  loopback. A separate **Hosted Gateway** is the only public listener and
  exposes only reviewed Vercel service operations, browser stream admission,
  the isolated public-spectator path, and bounded health and version surfaces.
  Generic Runtime, Controller, Studio, and operator routes are never public.
- The same Fly Machine runs only the bounded House Runner units admitted by
  ADR 0022. Studio is absent.
- Supabase runs one production project for Platform Accounts, platform-only
  coordination, and derived result evidence. It owns no Room fact and is never
  used as a live-stream relay.

A small PID-1 process monitor owns child startup, graceful drain, and fatal
failure propagation. This is an externally managed deployment profile, not the
WorldStream managed Supervisor defined by
[ADR 0018](0018-cli-first-operator-surface.md). The volume root is
`/var/lib/worldstream`; Runtime data, Controller state, retained Pack Bundles,
House allowance evidence, and backup staging occupy explicit children beneath
that root. Browser sessions and one-use admission material remain ephemeral.

The image carries each reviewed first-party Activity Pack Bundle as an
immutable seed artifact. Startup installs only an absent exact digest into the
persistent Pack store. It never overwrites or deletes a bundle needed by a
retained Room. Secrets and mutable production configuration are excluded from
the image and every client artifact.

### Machine and readiness contract

Fly automatic stopping is disabled. The Machine uses an `on-failure` restart
policy with ten retries, handles both `SIGINT` and `SIGTERM`, and permits a
120-second graceful shutdown. A one-Machine deployment is an expected live
interruption; every direct stream closes and each participant explicitly
re-enters the same durable Room afterward.

`/healthz` reports process liveness. `/readyz` admits traffic only after
persistent storage, schema, writer, authority, required hosted services, and
the exact deployed catalog and Pack inventory are ready. The public response
reveals no diagnostic or authority material. Because a failed Fly health check
does not itself restart a Machine, a fatal required-child or sustained fatal
readiness failure makes the process monitor exit unsuccessfully. The deployment
is failed if Fly remains unhealthy for five minutes, the smoke match cannot
finish, or a retained Room cannot resume.

### Secret boundaries

Supabase retains the GitHub OAuth provider secret. As required by ADR 0023,
Vercel keeps separate request-scoped publishable-key Auth, secret-key data, and
secret-key Auth Admin clients; it never collapses the two server-only clients
or gives either a browser session token. Vercel production functions also
retain platform-session and CSRF keys and their least-privilege Fly gateway
credential. Fly retains only its matching gateway authority, WorldStream
bootstrap and vault material, reconciliation hint secret, and OpenRouter
credential; it receives no Supabase database key. The browser receives none of
those values, no OAuth token, and no reusable Membership authority.

Vercel Preview deployments are fixture-only and receive no production
secrets. Production OAuth permits only the canonical production origin. Local
values live in ignored `.env.local` files and owner-only secret files;
`.env.example` contains names and explanations only. A privileged Fly init
step materializes Runtime secret files in temporary storage, assigns UID 65532
and owner-only mode `0600`, then drops privileges before starting application
processes.

### Local development and deployment

One supported `pnpm hosted:dev` command starts the local Supabase CLI stack,
committed migrations and deterministic seed, source-built Runtime, Controller,
Hosted Gateway, Vite applications, and a deterministic fake OpenRouter
provider. The default local identity bypass is visibly development-only and a
production build fails closed if that bypass or fake provider is enabled.
Local routes, schemas, Listing Revisions, Pack identities, and browser
admission behavior otherwise match production.

An ordinary production deployment is a guarded operator action from one clean,
committed candidate revision:

1. pass local and provider build checks;
2. close new launches and House fill, then wait for active Runs and provider
   calls to drain;
3. enter full maintenance, perform the reviewed offline Runtime shutdown, and
   create and verify one Hosted Recovery Checkpoint;
4. apply only additive, adjacent-release-compatible Supabase migrations;
5. deploy and verify the source-built Fly image from that commit;
6. push the exact commit to `main` for the Vercel production deployment;
7. complete the browser-to-Runtime smoke match and retained-Room restart test;
8. record the Hosted Deployment Revision and reopen launches.

The currently deployed Vercel application must remain compatible with the next
Fly revision, and the new Fly revision must remain compatible with the current
Vercel application during the transition. An emergency security deployment
may interrupt streams, but it must preserve recoverable Room data.

### Backup, rollback, and preview reset

The MVP adds no backup-storage product. While platform mutations are closed,
House calls are drained, and WorldStream is stopped, the hosted operational
recovery profile captures the complete Runtime directory (including any SQLite
WAL sidecars and retained Pack inventory), Controller state, House allowance
evidence, and maintenance fences. The same closed interval supplies a Supabase
logical dump, exact migration head, checksums, and Hosted Deployment Revision.
Together these form one Hosted Recovery Checkpoint. The command copies the
paired set off Fly to a private
operator-controlled directory outside the repository. A checkpoint is required
before every deployment and within 24 hours on each day with public activity;
seven verified sets are retained. Public launch stays closed until a complete
isolated restore drill succeeds. Provider snapshots are secondary evidence,
not the primary backup contract.

Routine rollback is code-only. Vercel may return to its immediately previous
compatible deployment and Fly may deploy a retained prior image digest only
when both accept the current persistent schemas. Supabase migrations are never
automatically reversed; incompatible change is repaired forward.

Data recovery restores one deliberately paired checkpoint into isolated new
storage, runs semantic and application verification, and cuts over only after
it passes. Independently timed Fly and Supabase copies are never combined and
served as coherent state. If no verified paired checkpoint exists, the
operator starts a fresh preview installation and discloses the preview-history
reset. Every House Agent Assignment present in restored Fly state is disabled
from further provider calls so an externally completed OpenRouter request is
never repeated.

#### Hosted operational recovery verification (2026-09-07 correction)

The hosted checkpoint is an **offline volume clone**, not the native SQLite
backup/envelope artifact defined in ADR 0011 and
[the storage runbook](../operator-storage.md). Those portable artifact commands
still require their sealed companion and remain unchanged. The earlier hosted
implementation incorrectly called native-only `sqlite verify` on a WAL-mode
Runtime file; that could neither admit the file nor establish restore readiness.

For the hosted clone, retain the archive bytes unchanged and extract a private
disposable copy. Require `worldstreamctl version` to identify the captured source
revision. Run the existing `pack restart-readiness` command on that copy. This
uses the production SQLite open path, verifies a source-bound standalone
snapshot across all durable tables, re-verifies the retained Pack inventory,
and executes Replay for every healthy Room against its retained Head and
materializations. Faulted or quarantined Rooms remain isolated and are counted;
they are never promoted to healthy. No daemon, Controller, Runner, or provider
call is started by this command. Any migration or readiness seal affects only
the disposable copy, not the captured archive.

Record the actual restart-readiness receipt with the captured revision and
archive digests. Explicitly record that the native envelope verifier was not
invoked; do not translate this evidence into its `semantic_verifier: pass`
claim. Production acceptance additionally exercises the exact Linux image on
Linux-local isolated storage with network access denied, checks the restored
platform/Controller correspondence, and tests ordinary same-volume Room
restart/re-entry. Desktop shared filesystem mounts are not a substitute when
they fail the native identity/durability checks. This corrects the hosted drill
method without creating a new kernel storage profile or release certification.

The first executable correspondence gate is deliberately narrower than the
full recovery goal: `worldstream/hosted-prelaunch-zero-history/v1`. It requires
zero healthy **and** isolated Runtime Rooms, zero retained Runtime activity
tables, empty Controller launch/setup/assignment/runner ledgers, and zero
platform launch, Run, seat, assignment, reconciliation, result, and relay rows.
It requires initialized, protected Controller metadata and proves that its
retained Host secret resolves to an enabled Host capability in the restored
Runtime. Both retained Controller and platform admission must be closed. The
offline verifier starts no Controller, Runner, or provider call; ordinary
maintenance is not described as restored-assignment recovery fencing.

This gate writes `verification-prelaunch-v1.json`, using receipt schema
`worldstream/hosted-recovery-verification/v3` and binding all three archive/dump
digests plus the deployment digest. The old v2 receipt proved extraction and
Runtime readiness but did **not** prove cross-store correspondence. Retain it
unchanged as historical evidence; neither it nor its existing idempotent
`isolated_restore_verified` event qualifies a checkpoint without the new,
digest-bound prelaunch receipt. Re-running the stronger drill appends that
separate receipt and never overwrites or silently upgrades the old proof.

This permits the initial empty-installation rehearsal only. After the first
real Run or retained activity, this command fails closed with
`populated_checkpoint_correspondence_required`. A complete populated
Controller–Runtime–platform correspondence verifier, including restored House
Assignment provider fencing, is still required to qualify populated recovery.
Until that verifier exists, captured populated archives remain unqualified
recovery evidence: do not claim the full recovery target or cut over to them.
Use the disclosed fresh-preview reset path if recovery is needed. The original
coherent recovery goal above is not waived by this bounded first profile.

The source and disposable platform databases must have the same ordered
migration version set, ending at the Hosted Deployment Revision's schema head.
Check that correspondence before the destructive disposable restore and again
after it; a matching final version alone is insufficient when an earlier
migration is absent. The cloud schema and migration files remain independently
reviewed deployment inputs; the data-only dump does not recreate them.

The operating targets, not provider promises, are a recovery point within 24
hours of active preview use and either verified recovery or a disclosed reset
within one operator day. OpenRouter automatic top-up remains disabled. In
addition to each assignment's immutable token allowance, the deployment stops
House fill at aggregate operator limits of USD 2 per day or USD 10 per month;
it never substitutes, rerolls, silently overrides a model, or permits an
unrecorded operator override of either spending gate.

### Acceptance boundary

Accepted preview behavior includes visible dependency degradation, planned
maintenance, dropped streams followed by explicit re-entry, delayed result
indexing, conservative capacity retention, unavailable House participation,
and a disclosed catastrophic preview reset. An unavailable Vercel or Supabase
dependency returns one generic temporarily-unavailable state for dependent
operations while already-admitted direct streams continue whenever ADR 0020
permits them.

The MVP has failed if it exposes generic operator authority; creates a
duplicate Room, Run, or provider call; loses acknowledged Room history during
an ordinary same-volume restart; publishes stale or unverified results;
releases capacity from a hint or timeout; serves a partial cross-store restore
as coherent truth; runs a binary against an incompatible schema; exceeds a
House allowance; exposes a secret; or routes traffic while readiness is false.

## Consequences

The public proof remains cheap and operationally honest, while correctness and
authority boundaries stay stronger than availability. Implementation must add
the preview host image, narrow Hosted Gateway, process supervision, production
secret bootstrap, Supabase migrations, maintenance switch, complete paired
backup workflow, restore drill, and end-to-end deployment smoke test. A later
HA, online-backup, multi-Machine, object-storage, or commercial service
contract requires a separate decision.
