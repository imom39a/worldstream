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

1. Create the Supabase project and configure GitHub OAuth.
2. Apply the committed Supabase migrations. Confirm the migration head.
3. Create one Fly app and one volume in the same region. Keep the app name out
   of the reusable `fly.toml` and pass it with `-a`.
4. Install the four Fly secrets listed in `.env.example`.
5. Deploy the source-built image with an exact clean Git revision, digest-pinned
   base-image build arguments, public Fly authority, and HTTPS Activity Client
   origin.
6. Confirm `/healthz`, `/readyz`, and `/version`. Confirm ports 9410 and 9420
   have no Fly service.
7. Deploy the product UI and BFF. The BFF uses HTTPS requests to Fly; browser
   WebSockets connect directly to Fly.
8. Create and record `hosted-deployment.json` with
   `scripts/hosted-checkpoint.mjs deployment create` and `deployment record`.
9. Run the local and deployed acceptance story before advertising the preview.

Validate the reusable Fly configuration without binding it to a real app:

```text
fly config validate \
  -a worldstream-preview-validation \
  -c packaging/hosted/fly.toml
```

Build from the repository root:

```text
docker build \
  --build-arg WORLDSTREAM_RUST_BUILDER_IMAGE=<rust-image@sha256:digest> \
  --build-arg WORLDSTREAM_NODE_RUNTIME_IMAGE=<node-image@sha256:digest> \
  --build-arg SOURCE_REVISION=<clean-git-commit> \
  -f packaging/hosted/Dockerfile \
  -t worldstream-hosted:<clean-git-commit> .
```

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
5. Download that complete directory with `fly sftp get`.
6. On the operator machine, run `checkpoint pair`. It uses PostgreSQL 17.11,
   an owner-only `PGSERVICEFILE`, and distinct source and disposable-restore
   service names. The command dumps `platform_store`, restores both sides in
   isolation, and records `created` plus `isolated_restore_verified` events.
7. Apply a backward-compatible Supabase migration, deploy Fly, then deploy the
   UI/BFF.
8. Remove only the `closed` marker. Wait for `/readyz` to return 200.
9. Run `maintenance open`. If readiness fails, recreate the marker and keep
   Supabase closed.

`checkpoint pair` requires this exact destructive-drill acknowledgement:

```text
--confirm-disposable-restore worldstream-disposable-restore-target
```

The restore service must identify a disposable database that already has the
same migration set. The command truncates `platform_store` there. It refuses to
use the source service as the restore service. Credentials stay in the
owner-only service file; password and DSN command options are not accepted.

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
