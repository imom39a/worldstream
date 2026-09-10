---
status: accepted
date: 2026-09-09
---

# Declare generic hosted starts and reviewed roster options

Midnight Archive needs a genuine solo configuration, optional named
companions, and hosted start through the common platform. Current authoring
emits mandatory Roles, and hosted start recognizes retained Heist revisions.
Use a versioned, Pack-neutral **Activity Start Contract** and immutable
**Roster Options** instead of adding another game-name branch or inventing
absent participants to satisfy Heist-shaped setup.

An Activity Start Contract declares compatibility with one bounded pre-start
operation. Its typed ExternalInput is reduced through the existing five Pack
operations. The Host verifies the approved exact Pack and contract, the
pre-start condition, resolved roster, required readiness and existing narrow
launch authority. The Pack enforces its domain start predicate. This exposes
neither arbitrary ExternalInput submission nor general Room administration to
the platform or browser. A declaration alone is not Host approval. Catalog,
Host, Runtime backends and authoring must agree on the contract; metadata
without an implemented and qualified start path is insufficient.

This decision partially supersedes ADR 0010's enumeration of two registry
status dimensions. The registry now records an independent exact-revision
`approved_for_activity_start` status alongside selection and retained
execution. Approval authorizes only this bounded operation; it does not make a
revision selectable or runnable, and the launch path still requires the exact
retained executor to be available. Revocation blocks a new start while leaving
the compatibility declaration readable for durable receipt recovery.

A new version of the hosted launch-input contract permits selection from
reviewed Roster Options pinned by the Activity Listing Revision. Each option
selects a bounded subset of its predeclared seats and server-owned exact
configuration. Required seats remain required. Callers cannot provide new
Roles, Principal identifiers, prompts, model routes, code or arbitrary
configuration. The chosen option freezes with the Launch Request before any
claim and is carried into the one resolved Room Setup Operation. Recovery
continues that exact option and lineage.

For the first Archive release, options are solo, Mira, Jonah and both
companions, always with one human lead. The named characters use exact
reviewed companion definitions compatible with their Roles; this is a bounded
extension to ADR 0022's random exhibition-fill selection, not a change to
existing fill or permission for paid execution. Retain the existing execution
caps, one assignment identity per launch lineage and exhibition disclosure.
Purchases, custom prompts, user-selected providers, mid-run replacement and
cross-run character memory remain separate work.

Keep existing launch-input-v1 `none` behavior and retained Heist start
compatibility readable. New behavior requires new reviewed source/artifact
identities, listings and qualification; existing Rooms and listings are not
migrated. Authoring extensions for optional cardinality, supported bounded
schemas, start declarations and non-Heist conformance fixtures must preserve
older source forms without rewriting retained artifacts.

This extends the formation/start integration in ADRs 0019 and 0021 and the
selection scope of ADR 0022 for the new reviewed companion mode. It adds no
kernel callback or game-specific Room authority. Implementation, Host
approval, deployment and qualification are outstanding; this decision records
the design chosen under the user's delegation of remaining recommendations.

References: [Hosted platform](0019-separate-hosted-activity-platform-from-worldstream.md),
[formation](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md),
[House execution](0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md),
[Archive design](../midnight-archive-design.md).
