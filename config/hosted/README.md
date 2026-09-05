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

Catalog review and Host execution authority are separate controls. A checked-in
listing is review evidence; it is not a Room Host allowlist. The Host must still
allowlist the exact Listing Revision, Pack, client surface, and projector identities.

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
