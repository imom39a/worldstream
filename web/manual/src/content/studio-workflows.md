# Studio operator workflows

Studio groups bounded operations around stable IDs and retained state. It never
accepts an arbitrary command, executable, storage path, Room ID, Membership
credential, or environment block from the browser.

## Home and attention

The Home inbox aggregates already-modeled conditions across daemon lifecycle,
Room creation, Task setup/readiness, Runner capacity/freshness, Activation
leases/backlog, managed hosts, and backup operations.

- duplicate observations collapse into one stable item;
- authoritative observation time is preserved;
- stale sources are labeled;
- resolved items leave the active inbox with bounded transition history;
- optional local notifications are deduplicated and rate-limited;
- dismissal changes notification UI only, never Room/operation state.

## Build: Activity Packs, profiles, templates

**Activity Packs** reads the exact daemon-installed catalog and revision detail.
Studio guides exact bundle inspection, Host Operator approval, offline install
status, and restart readiness. It cannot download, hot-load, auto-approve, or
substitute Pack code.

The Pack Operations card shows the seven exact `worldstreamctl pack` commands
in order and consumes their typed JSON receipts. Every command starts with
`worldstreamctl --config <WORLDSTREAM_CONFIG> pack`; PostgreSQL
`restart-readiness` also shows
`--dsn-file <POSTGRES_ADMIN_DSN_FILE>`. Use one reviewed config for every step.

The first inspection pins the physical bundle and semantic revision for the
whole browser workflow. Studio requires an explicit stopped-daemon confirmation
before approval or inventory mutation and rejects a reordered or substituted
bundle-bearing receipt. For each inventory it verifies the declared row count,
strict digest order, and recomputed canonical `inventory_digest`; it also
requires the receipts' `storage_profile` to match the selected configuration.

After selectability changes, Studio matches the second inventory's
`storage_profile` and `inventory_digest` to restart readiness. That final check
uses original-byte production Component admission and configured-store
executable Replay, then writes a durable startup-readiness seal binding the
exact inventory to a pathless `deployment_binding`. The binding is derived from
the canonical data-directory identity, storage profile, and available provider
metadata. The daemon independently verifies that seal against its actual target
before accepting installed portable bundles.

Install, approval revocation, selectability change, retained-bundle restore, or
removal clears the seal and removes “Start-ready” status until restart readiness
runs again. The readiness receipt remains aggregate and pathless, so it must be
pasted directly from the displayed config-aware command. Paths, bundle bytes,
operator approval inputs, DSNs, process control, and shell execution stay in the
owner's terminal.

**Agent Profiles** publishes immutable exact revisions that pin external or
managed host behavior, provider/model configuration, Runner Template revision,
and kind-bound secret requirements. Updating behavior means a new revision.

**Runner Templates** loads owner-installed JSON manifests from the fixed
`config/runner-templates` directory. Each manifest binds an executable digest,
compatibility, capacity, health endpoint, safe environment names, secret
references, and instance IDs. Studio exposes no marketplace or upload route.

**Task Templates** retain reusable exact draft inputs and instantiate new Room
drafts without mutating the template revision.

## Create: draft to Room

1. Create or instantiate a Room draft.
2. Select an exact Activity Pack revision and valid configuration.
3. Define seats, human/agent kind, Role, and required readiness.
4. Assign an immutable Agent Profile revision where needed.
5. Review the normalized immutable draft.
6. Start Room creation with the stable draft identity.
7. Retry only the retained operation when state is retryable/indeterminate.

The Room creation supervisor retains intent before remote work. It reconciles a
lost response from durable daemon receipts and never silently creates a second
Room.

## Task setup and launch

Task setup checkpoints separate stages:

- Room creation confirmed;
- Participant authority provisioned per seat;
- Runner authority provisioned separately for agent seats;
- Agent assignment retained;
- human session and required Runner readiness observed.

Lobby launch remains blocked until all declared required readiness is satisfied
and the operator explicitly chooses launch. Studio does not infer launch from
process health.

## Human Activity Client handoff

**Open participant client** requests a short-lived, one-use handoff for one
exact provisioned human seat. Studio opens an independent client on the
configured Client Host; it does not own or render that participant UI. The new
window receives only an opaque fragment, redeems it at the configured origin,
and obtains an HttpOnly scoped session cookie. Raw Room ID, Membership ID, and
bearer are absent from the URL.

The Supervisor resolves the current Membership through the Host-local Client
Binding Store before launch and every retained operation. Resolution uses the
semantic Pack Revision digest, client contract, Access Mode, Role, Deployment
trust policy, readiness, and Host
preference. The checked-in bindings launch Agent Heist at `/agent-heist/` and
Negotiate at `/negotiate/`; when no specialized binding is eligible, the Host
offers the separately configured Pack-neutral `/inspector/` fallback. Studio
receives only generic candidate metadata and opaque selection IDs. There is no
name-only, SemVer, or Pack-specific route fallback.

## Operations

| Surface | Safe actions |
| --- | --- |
| Daemon | status, start, graceful stop, restart |
| Rooms | bounded inventory and privacy-safe detail |
| Runners | installed status, start, graceful stop, restart |
| Agent attention | inspect, reconcile, safe Runner restart |
| Managed Agent Host | exact assigned start, status, stop, retry |
| Backups | storage capability/health, stable operation start/retry/status |

## Backups

For bundled SQLite, Studio supplies only a stable operation ID. The daemon
derives the destination under the startup-fixed backup root, performs an online
backup, reopens it, and returns a pathless name/size/checksum summary. This does
not claim the separate full semantic restore verification has run. PostgreSQL
and ephemeral profiles report their distinct support status honestly.

Source: [Studio control-plane ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0013-studio-companion-control-plane.md),
[Studio operations](https://github.com/imom39a/worldstream/blob/main/docs/studio.md),
and [Supervisor source tree](https://github.com/imom39a/worldstream/tree/main/crates/worldstream-studio-supervisor/src).
