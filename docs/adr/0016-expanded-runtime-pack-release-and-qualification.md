---
status: accepted
date: 2026-08-30
---

# Expand the detached release and add post-sign qualification

WorldStream will release the Runtime and its first public Pack ecosystem as
one cryptographically closed graph. The primary detached release inventory has
exactly 33 pre-sign subjects: four Runtime/source payloads, twelve portable
Starter subjects, and seventeen non-supply-chain source reports. The signed
inventory's checksum, SPDX, and SLSA documents cover that exact set. After its
signature is verified, the eighteenth source report binds those three
sidecars, the inventory, and the inventory signature. Final assembly
normalizes exactly eighteen release-gated evidence reports into the separately
signed `release-manifest.json`.

The twelve portable subjects are deterministic artifacts for the A202
adapter, deterministic agents, documentation, examples, licenses, official
Negotiate Bundle, Negotiate evidence verifier, Pack Toolchain, Participant
Console, release metadata, Studio, and TypeScript Pack SDK. They are first-
class release subjects rather than untracked files copied into a Starter. The
Negotiate Bundle remains byte-identical to its digest-named `.wspack`; every
other directory input is converted into a canonical gzip/ustar artifact with
a closed file inventory. Cache, dependency, approval, credential, secret,
backup, and mutable database paths fail closed.

Four new primary evidence producers cover:

- exact Bundle, Component Host, and Core conformance;
- the independent Negotiate oracle, participant privacy, offline dual-evidence
  verifier, and pinned A202 operated profile;
- released-artifact-only SQLite restart, four independent Membership cursor
  reconnects, exact executable Replay, and offline dual evidence; and
- the same released-artifact-only acceptance on PostgreSQL primary.

The producer adapter accepts only bounded machine reports tied to the exact
Linux archive and official Bundle identities. A source checkout, narrative,
prototype, debug binary, a bare `release_evidence=true` self-assertion without
the closed released-artifact policy and identities, manual database access, or
a report for another storage profile cannot be promoted. Bundle conformance is
read from the detached `.wspack`, and its Component Host/Core proof must be
emitted by `worldstreamctl` from the exact detached native Linux archive. The
local golden-flow diagnostic explicitly disclaims release-evidence status and
is not an eligible policy qualification input.
Existing backup, restore, transfer, migration, and platform rows remain
mandatory; the original fourteen evidence IDs cannot be removed by this
expansion.

A Starter necessarily embeds an already signed primary release manifest.
Outside-adopter trials necessarily happen after that Starter exists. Hashing
either result back into the primary manifest would create a signature cycle.
WorldStream therefore uses a second, separately signed
`release-qualification-manifest.json`. It binds the exact primary manifest and
signature, at least one authenticated official Starter, at least one
authenticated custom Starter, and one canonical summary of six real outside-
adopter receipts. Its three qualification rows cover Starter/custom recovery,
the Pack Author journey, and the Application Integrator journey. They are
`qualification_gate=true` and explicitly not primary `release_gate` rows.

The pre-sign aggregation build type is v4. Its URI is the SHA-256-addressed
definition URN, while its statement also binds every material and workflow
byte. This avoids an unpublished-commit placeholder without weakening source
identity. V3 remains historical; the withdrawn v2 tombstone remains required.

## Consequences

- Primary release verification is non-recursive and closes exactly 16 payload,
  17 pre-sign source, and 18 normalized evidence identities.
- Starter and adoption claims cannot weaken or retroactively modify the signed
  primary release.
- Structural-only Starter checks, templates, maintainer trials, and generated
  receipts remain non-evidence.
- `release_ready` is never changed by an evidence generator. The compatibility
  pair remains a fail-closed specification until every primary producer and
  external release prerequisite genuinely exists.
- The current workflow deliberately reports the Negotiate policy and both
  released-artifact restart producers as unavailable. The post-sign
  qualification manifest likewise cannot be authenticated until real Starter
  signatures and six outside-adopter receipts exist. Those are release
  blockers, not values that repository tests may manufacture.
