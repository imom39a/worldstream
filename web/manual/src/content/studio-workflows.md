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
Studio cannot upload or substitute pack code.

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

## Human Participant handoff

**Open Participant View** requests a short-lived, one-use handoff for one exact
provisioned human seat. The new Console window receives only an opaque fragment,
redeems it from the configured Console origin, and obtains an HTTP-only scoped
session cookie. Raw Room ID, Membership ID, and bearer are absent from the URL.

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
