# Hosted contract artifacts

This directory contains reviewed authoring documents for the hosted activity platform.
They are not a Room Host allowlist.

- `listings/` selects an exact Pack, Activity Client surface, launch-input contract,
  seat policy, creator access, public viewing, pre-start deadline, result publication,
  and Result Projector Revision.
- `result-projectors/` declares a deterministic, networkless, declarative projection
  program. It pins its runtime revision, complete input contract, accepted public
  projection schema, result schema, canonicalizer, and byte bounds.
- `result-projector-runtimes/` retains the immutable semantic artifact for each
  runtime registry entry, including digests of the exact Rust and TypeScript
  interpreter sources. Its canonical-byte digest is part of every projector identity,
  so runtime behavior changes require a new artifact and projector revision.
- `house-agents/` contains the two reviewed exhibition-only House Agent
  Revisions. Each revision pins one behavior policy, exact OpenRouter model and
  provider route, Host profile and Runner template references, empty tool set,
  accounting tokenizer, and fixed execution allowance.
- `schemas/` describes the closed JSON shapes. Rust and TypeScript readers also
  enforce byte, tree-depth, string, item-count, and immutable-reference bounds.

The checked-in JSON is formatted for review. Publication converts the document to
`worldstream/canonical-json/v1` bytes and computes `blake3:<hex>` over those exact
bytes. Runtime boundaries accept canonical bytes only. Reformatting an authoring
document does not change its revision identity; changing one semantic value does.
Deployed runtime and schema artifacts are verified only from canonical, duplicate-free
bytes; a generic JSON parse-and-reserialize step is not an artifact verifier.
Artifact resolution returns the only executable Result Projector handle. Projection
cannot run directly from an unverified Result Projector Revision.

The Agent Heist `0.2.0` listing permits account-owned people and external agents.
It contains no placeholder House Agent identities. The separate `0.3.0` listing
adds `house_agent_fill` and allowlists only the exact two reviewed House Agent
Revision digests. Existing Launch Requests remain pinned to the revision they
selected.

Listing `0.4.0` keeps the same Pack and House pool and pins Client Release v2.
Its original verified bytes remain at `/agent-heist-v2/` and
`/agent-heist-v2/hosted/` for retained Runs. Listing `0.5.0` pins the new
game-themed Client Release v3 without changing the Pack or House pool.
Its hosted surface at `/agent-heist-v3/hosted/` reads the exact deployment-owned
stream URL from the authenticated Platform session. The same artifact contains
the distinct local kernel surface at `/agent-heist-v3/`; an authentication
failure never switches between them. Neither the user nor the handoff URL
chooses a Room, Membership, or upstream.

Listing `0.2.0` and `0.3.0` and their Client Release documents remain immutable
for retained Runs. Their original `/agent-heist/` artifact is not shipped by this build:
that path returns 404, and the Platform rejects their live start/re-entry.
Public history and reconciliation still resolve the old exact Listing and
projector. Supporting old live entry again requires the original verified
artifact at its original approved deployment, not new bytes behind its URL.

Catalog review and Host execution authority are separate controls. A checked-in
listing is review evidence; it is not a Room Host allowlist. The Host must still
allowlist the exact Listing Revision, Pack, client surface, and projector identities.

The retained Listing `0.12.0` pins Heist `0.4.0`, Client v5, Planner 8,
Auditor 7 and Template 7. It remains addressable for Runs created before the
current discovery revision. Client v5 uses `/agent-heist-v5/` and
`/agent-heist-v5/hosted/`; v4 remains a retained verified build at its original
paths. The old Pack and Result Projector remain available for those Runs.

Listing `0.13.0` is retained as the exact Heist `0.3.0`/Client v5 profile. It
does not replace or mutate Listing `0.12.0`, Listing `0.11.0`, or any existing
Run. Exact Listing digest resolution remains available for retained formation
and result reads.

Listing `0.14.0` is a retained discovery and fresh-setup profile for the
frozen Heist `0.3.0` Pack. It binds immutable Client v6 bytes, including the
independent public viewer surface at `/agent-heist-v6/hosted/`. A new Listing
revision changes discovery only; it does not rewrite Listing `0.13.0` or any
existing Run.

Listing `0.24.0` is the current discovery profile. It pins the schema-safe
Heist `0.5.0` Pack and its exact `0.5.0` Result Projector while keeping Client
v7, the two Runner template `16` House successors, public policy, and limits
unchanged. Client v7 adds exact Pack `0.5.0` compatibility without changing the
participant or public-viewer protocol. Every non-empty Heist Action now declares a closed payload schema,
so a generic client or House Runner rejects an unusable `{}` payload before it
can reach the Pack reducer. Listing `0.23.0` and all earlier revisions remain
retained for existing Rooms and Assignments; no retained Pack, Listing,
projector, Runner executable, or House approval is rebound.

The launch request is intentionally small:

```json
{
  "schema": "worldstream/launch-request/v2",
  "listing_revision_digest": "blake3:<listing revision>",
  "inputs": {},
  "creator": {
    "participation": "seat",
    "principal_reference": "<server-derived run-scoped principal>"
  }
}
```

It cannot select a Pack, role, seat, client, projector, or complete Room Setup
Spec. The server combines it with a frozen roster and the reviewed listing to
derive `worldstream/room-setup/v2` deterministically, including the required
result-indexer and any elected, listing-authorized creator or public-relay
spectators. The server freezes exactly one creator participation mode. A `seat`
election must reference the creator's claimed human seat; a `spectator` election
is accepted only when the Listing permits it and uses the reserved
`worldstream:creator-spectator` reference.

Result projection accepts the exact Listing Revision, Result Projector Revision,
Pack, privacy-reviewed Public Projection schema, and eight-field Complete Head.
It returns only `not_terminal`, `terminal_without_outcome`, or the bounded public
summary schema. Hosted formation cannot request an Operator view.
