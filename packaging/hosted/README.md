# Fly hobby preview package

This package runs one continuously available WorldStream authority on one Fly
Machine and one Fly volume. It is an MVP deployment, not a high-availability
cluster.

Only the Hosted Gateway listens publicly on port 8080. Runtime and Controller
listen on `127.0.0.1:9410` and `127.0.0.1:9420`. The hosted image contains no
Studio UI. The retained volume has these explicit children:

```text
/var/lib/worldstream/
  runtime/       # Runtime database and exact Pack inventory
  studio/        # Controller state; the legacy kernel path name is retained
  retained-runner-executables/ # Exact executable bytes pinned by Runner Templates
  maintenance/   # closed and recovery-fence markers
  checkpoints/   # no-clobber volume captures awaiting operator download
```

The `studio/` name does not mean that Studio runs in production. The kernel's
backup contract derives that exact path from the Runtime directory.

## Required boundaries

- Use one Machine. Do not attach the volume to a second Machine.
- Keep automatic stop disabled. A stopped Machine cannot serve WebSockets.
- Supply all four process secrets with `fly secrets set`. Do not put their
  values in `fly.toml`, deployment arguments, or a committed environment file.
- Supply digest-pinned Rust builder and Node runtime image references at build
  time. A tag alone is not a release identity.
- Keep Fly Runtime/Controller addresses on loopback. Do not add services for
  ports 9410 or 9420.
- Use the same service authority in the Fly secret and the server-side platform
  BFF secret. Never expose it to browser code.

## First deployment

For this hobby MVP, build, test, and publish from the local operator checkout.
GitHub Actions is optional, not a deployment prerequisite. Use the tested clean
Git revision for both the Fly image and Vercel deployment, then verify the real
browser-to-Room flow on the live site. Local publication does not bypass the
existing secret, spending, maintenance, or data-preservation checks below.

1. Create the Supabase project and configure GitHub OAuth.
2. Apply the committed Supabase migrations. Confirm the migration head.
3. Create one Fly app and one volume in the same region. Keep the app name out
   of the reusable `fly.toml` and pass it with `-a`.
4. Install the four Fly secrets listed in `.env.example`.
5. From the repository root, run the revision preflight before publishing. It
   checks that the checkout is clean and that image and runtime identities use
   the same exact source revision:

   ```text
   REVISION=$(git rev-parse HEAD)
   node scripts/hosted-deploy-preflight.mjs plan <fly-app> \
     <https-client-origin> \
     <rust-image@sha256:digest> <node-image@sha256:digest>
   ```

   Review the JSON command plan, then use its exact `fly deploy` arguments. The
   plan performs a pinned remote Docker build from the repository root and
   supplies all non-secret app-specific runtime bindings. A failed preflight is a release stop before image
   publication or Machine mutation.
6. Deploy the source-built image with an exact clean Git revision, digest-pinned
   base-image build arguments, public Fly authority, and HTTPS Activity Client
   origin.
7. Confirm `/healthz`, `/readyz`, and `/version`. Confirm ports 9410 and 9420
   have no Fly service.
8. [Prepare and review the actual House Agent approvals](../../docs/hosted-house-approval.md).
   The initial database records stay disabled. Do not use development approval
   hashes or activate model calls before the credential and budget checks pass.
9. While maintenance remains closed, approve and install the reviewed Midnight
   Archive Bundle at `/opt/worldstream/hosted/midnight-archive.wspack`, make its
   exact Bundle digest selectable, and run `pack restart-readiness` with the
   appliance configuration. Retain those JSON receipts and the private
   `/run/worldstream/generated/initialization-import-apply.json` receipt as
   deployment evidence and House approval input.
10. Deploy the product UI and BFF. Set
   `WORLDSTREAM_INTERNAL_CANDIDATE_LISTING_DIGEST` to the exact reviewed
   Midnight Archive Listing digest only after the live Host availability probe
   succeeds. The BFF uses HTTPS requests to Fly; browser WebSockets connect
   directly to Fly.
11. Create and record `hosted-deployment.json` with
   `scripts/hosted-checkpoint.mjs deployment create` and `deployment record`.
12. Run the local and deployed acceptance story before advertising the preview.

The versioned `WORLDSTREAM_HOSTED_INSTALLATION_ID` must match in the Fly
appliance, Vercel BFF, and active House approvals. A mismatch makes reviewed
agents unavailable even when every service is healthy. The current release
uses `fly-primary-r4`. Do not rename the global Supabase operating row from
`fly-primary`; that row is the singleton platform admission gate, not the
formation installation identity.

Validate the reusable Fly configuration without binding it to a real app:

```text
fly config validate \
  -a worldstream-preview-validation \
  -c packaging/hosted/fly.toml
```

Deploy from the repository root:

```text
fly deploy -a <fly-app> -c packaging/hosted/fly.toml \
  --remote-only --ha=false \
  --build-arg WORLDSTREAM_RUST_BUILDER_IMAGE=<rust-image@sha256:digest> \
  --build-arg WORLDSTREAM_NODE_RUNTIME_IMAGE=<node-image@sha256:digest> \
  --build-arg SOURCE_REVISION=<clean-git-commit> \
  --env WORLDSTREAM_DEPLOYMENT_VERSION=<clean-git-commit> \
  --env WORLDSTREAM_PUBLIC_AUTHORITY=<fly-app>.fly.dev \
  --env WORLDSTREAM_HOSTED_CLIENT_ORIGIN=<https-client-origin>
```

## Bounded Controller catalog overlay

For the reviewed Listing `0.26.0` release, use
`Dockerfile.catalog-update` only when the deployed appliance is the exact
digest declared in that file. It preserves the deployed Runtime, Gateway, MCP,
managed-agent executable, and managed-agent digest; the separate clean builder
contributes only `worldstream-studio-supervisor`.

This is a compound artifact, not a rebuilt native Runtime image. Its inherited
`org.opencontainers.image.revision` continues to identify the deployed Runtime
source. Read `io.worldstream.base-image`,
`io.worldstream.controller-source-revision`, and
`io.worldstream.controller-builder-image` together for the overlay provenance.
It does not establish a fresh full-native qualification or Hosted Recovery
Checkpoint claim.

Build the controller-builder image from the reviewed clean source archive, then
build the overlay from the same source archive:

```text
docker build \
  --build-arg WORLDSTREAM_DEPLOYED_RUNTIME_IMAGE=registry.fly.io/worldstream-preview@sha256:8d5a44b2d547b1f0bb970bf2d9a67ba7db4fd7ea66840e93a4e1e1f29a1f0f07 \
  --build-arg WORLDSTREAM_CONTROLLER_BUILDER_IMAGE=worldstream-hosted-builder:<reviewed-source> \
  --build-arg CONTROLLER_SOURCE_REVISION=<reviewed-source> \
  -f packaging/hosted/Dockerfile.catalog-update \
  -t worldstream-hosted-catalog-update:<reviewed-source> .
```

Before any catalog-only deployment, compare all six retained executable
SHA-256 values and the raw managed-agent BLAKE3 value with the deployed
appliance. Also compare the generated Template `16` canonical bytes with the
installed immutable Template `16`. A mismatch is a release stop, not an
approval rotation. The `r4` release changes the managed-host executable, so it
must use the full image path instead of this catalog overlay. Its exact binary
digest must be pinned by new Heist Template `17` and Archive Template `2`; the
import must leave retained Heist Template `16` and Archive Template `1`
unchanged.

## One-time `r3` to `r4` clean-preview transition

The `r4` image changes the managed-host bytes and formation installation
identity. It must not boot over retained `r3` House Runner reservations:
Controller startup correctly rejects a reservation whose installation identity
does not match the current one. This cutover therefore requires separate
explicit operator approval to start a fresh, coherent preview lineage and
discard the earlier preview Run history. Neither this runbook nor the recovery
ADR grants that approval. This is a destructive release transition, not an
ordinary deploy or a recovery claim.

Keep both Supabase admission switches closed and keep
`/var/lib/worldstream/maintenance/closed` present for the whole transition.
First capture the stopped Fly volume and platform database; label those bytes as
preservation input only because populated recovery is still unqualified. Then,
in one recorded reset procedure, clear only the platform launch/Run descendant
rows while retaining Auth identities, Platform Accounts, catalog revisions,
House revisions and approvals, deployment records, and the singleton operating
row. Replace the stopped volume's `runtime/`, `studio/`, and
`retained-runner-executables/` children together with new empty owner-only
directories. Never reset just one authority or reuse any retained `r3`
reservation under `r4`.

Deploy the full `r4` image while the marker remains present. Import the exact
new and retained declarations, install the Pack and clients, capture fresh
`r4` approval evidence for all four current House Agents, and set the Vercel
BFF installation identity to `fly-primary-r4`. Reopen only after Supabase,
Fly, and Vercel report the same exact deployment identities and the complete
local and deployed acceptance stories pass. The public release record must say
that earlier preview history was reset.

## Planned deployment or checkpoint

The order is deliberate. It closes new platform work before stopping the Room
authority and does not reopen the platform until all required services agree.

1. Run `maintenance enter` from a protected operator checkout. This closes new
   Launch Requests and new House-fill operations in Supabase.
2. Create `/var/lib/worldstream/maintenance/closed` as mode 0600 through
   `fly ssh console`. The PID-1 monitor drains and stops managed services while
   the Gateway remains live but not ready.
3. Wait until loopback ports 9410 and 9420 are closed.
4. Run the image's `/opt/worldstream/hosted/hosted-volume-capture.mjs`. It writes
   two no-clobber archives and `capture.json` below `checkpoints/{uuid}`.
   The Runtime archive contains `runtime/`; the Controller archive contains
   `studio/`, `maintenance/`, and `retained-runner-executables/`, including the
   exact executable bytes required by installed immutable Runner Templates.
   New captures require all these source directories to be real, owner-only
   directories. Older v1 Controller archives containing only `studio/` and
   `maintenance/` remain verifiable, but cannot prove that retained Runner
   executables were captured.
5. Download that complete directory with `fly sftp get`. Use a private directory
   outside Git. Set the directory to mode 0700 and its downloaded files to 0600;
   SFTP may not preserve the source permissions.
6. On the operator machine, run `checkpoint pair`. It uses PostgreSQL 17.11,
   an owner-only `PGSERVICEFILE`, and distinct source and disposable-restore
   service names. Pass `--worldstreamctl` the CLI from the captured source
   revision (or a reviewed wrapper for that exact image). The command checks
   `worldstreamctl version`, dumps `platform_store`, restores both sides in
   isolation, and runs `pack restart-readiness` against the extracted Runtime.
   It records `created` before the drill and `isolated_restore_verified` only
   after the drill completes. The currently supported qualification is an
   empty prelaunch installation, not populated recovery. The evidence identifies
   `worldstream/hosted-prelaunch-zero-history/v1` and
   retains the actual Pack/Replay readiness receipt, not a native-envelope
   semantic-verifier claim.
7. Apply a backward-compatible Supabase migration and deploy Fly. With the
   Runtime stopped, approve and install the exact bundled Midnight Archive
   archive, set it selectable, and require `pack restart-readiness` to pass.
   Activate the separately reviewed House approvals only after their installed
   executable, Template, Profile, credential route, and allowance identities
   match. Then deploy the UI/BFF with the exact internal candidate digest.
8. Remove only the `closed` marker. Wait for `/readyz` to return 200.
9. Run `maintenance open`. If readiness fails, recreate the marker and keep
   Supabase closed.

`checkpoint pair` requires this exact destructive-drill acknowledgement:

```text
--confirm-disposable-restore worldstream-disposable-restore-target
```

The restore service must identify a disposable database that already has the
same migration set. The command checks the complete ordered migration versions
and exact deployed head before truncating `platform_store`, then checks again
after restore. It refuses to
use the source service as the restore service. Credentials stay in the
owner-only service file; password and DSN command options are not accepted.

The disposable restore connection must be a PostgreSQL superuser because
`pg_restore --disable-triggers` must suspend system foreign-key triggers during
the data-only restore. This privilege is required only on the isolated test
database. Never grant it to the application or change the production role to
make a drill pass. The command checks the restore role before dumping or
truncating data and checks again immediately before the isolated drill.

If pairing retained a complete manifest and archives but the drill failed,
correct the isolated environment and rerun the same immutable checkpoint:

```text
node scripts/hosted-checkpoint.mjs checkpoint drill \
  --directory /private/operator-backups/checkpoint-id \
  --source-service worldstream_source \
  --restore-service worldstream_disposable_restore \
  --worldstreamctl /private/operator-tools/worldstreamctl \
  --confirm-disposable-restore worldstream-disposable-restore-target
```

This command checks all retained hashes, the deployment document, verifier
revision, migration versions, and restore privileges. It does not make another
capture or dump, change `paired_at`, or create a different checkpoint identity.
It records the same idempotent verified event before publishing the no-clobber
`verification-prelaunch-v1.json`; if that event request fails, repeat the
command. An existing prelaunch verification file is never overwritten.
An older `verification.json` is retained unchanged and does not prevent the
stronger drill. That old v2 proof and any prior verified event do not establish
cross-store correspondence; only the new digest-bound prelaunch receipt
qualifies this narrower profile. Provider calls are not started by the drill.

The gate checks that all retained Room, launch, setup, assignment, runner,
reconciliation, and result history is empty. It also checks protected Controller
initialization metadata, proves its Host credential matches the enabled Runtime
Host capability, and requires closed Controller and platform admission. It
does not boot the Controller or claim that ordinary maintenance is restored
Assignment recovery fencing. Use Node 24 and the installed, lockfile-pinned
Pack SDK dependencies for the operator check; no secret bytes enter its output.

After the first retained activity, pairing/drilling fails with
`populated_checkpoint_correspondence_required`. Preserve the resulting
archives, but do not label them verified or use them for a recovery cutover.
A populated cross-store correspondence verifier with restored House Assignment
provider fencing remains required. Until implemented, a recovery incident
requires the disclosed fresh-preview reset path. The first empty checkpoint
is not evidence that user history can already be recovered.

This verifies a stopped operational volume clone, not the portable SQLite
backup/envelope format. Do not run `sqlite verify` directly on the captured WAL
database, change its journal mode, or replace the archive to obtain a passing
receipt. `pack restart-readiness` supports the real WAL Runtime and creates its
own bounded, source-verified snapshot on the disposable copy. An absent Runtime
database fails the drill instead of being counted as an empty installation.

The cloud acceptance drill must also run the exact image with `--network none`
and no provider credentials. Extract the read-only captured archive onto
Linux-local container storage, not a writable Docker Desktop shared filesystem
mount. Native identity/durability checks can correctly refuse shared mounts.
Keep the image digest, `worldstreamctl version` receipt, Pack restart-readiness
receipt, restored platform/Controller correspondence, and ordinary retained
Room restart/re-entry proof with the deployment evidence. A directory checksum
check alone is not a new isolated restore drill.

## Recovery and rollback

A Hosted Recovery Checkpoint is one immutable manifest binding three artifacts:
the Runtime archive, Controller archive, and Supabase platform dump. Never pair
independently timed artifacts or serve them as one recovery state.

Restore both WorldStream archives to a new isolated volume and restore the
Supabase dump to an isolated migrated database. Before capture or cutover, use
`maintenance recovery-fence` and retain
`maintenance/recovery-house-calls-fenced`. A restored authority must not start
House Runners until an operator verifies retained assignments and prior provider
receipts. After that audit, use `maintenance recovery-release`; this leaves
ordinary maintenance active. Reopen only through the normal readiness order.

A code-only rollback deploys the prior exact image while keeping the current
volume and compatible Supabase schema. It does not restore old data. If no
coherent checkpoint exists, use a new installation/database lineage and state
plainly that the preview history was reset. Never silently combine or discard
the two authorities.

The production support posture is one region, planned downtime, explicit
re-entry after disconnect, and no SLA. Fly volume snapshots and Supabase
provider backups are useful disaster inputs, but neither is a WorldStream
Hosted Recovery Checkpoint by itself.
