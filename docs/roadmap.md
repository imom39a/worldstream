# Runtime-plus-Packs Delivery Roadmap

## Goal

Ship WorldStream as a self-hosted Room Runtime with one adoptable public
Activity Pack contract, then prove it with WorldStream Negotiate. The project
does not need another Room Kernel rewrite. It needs a narrow portable execution
adapter, a safe bundle lifecycle, author/operator tooling, and one application
whose value depends on authoritative realtime collaboration.

Planning assumes one primary developer working roughly 8–12 focused hours per
week. Correctness and first-success evidence take priority over feature count.

## Product story

A Pack Author describes deterministic Roles, Actions, visibility, phases,
timers, Attention, and Outcomes in one code-first TypeScript project. Optional
prompt assistance can draft the same project. WorldStream-owned tooling checks
it, compiles it to a WASI-free WebAssembly Component, runs conformance/privacy
proof, and builds one immutable `.wspack`.

A Host Operator inspects and explicitly approves the exact bundle digest,
installs it offline, and restarts `worldstreamd`. An Application Integrator uses
the ordinary protocol/Application SDK to configure a Room, attach humans and
external Runners, submit Actions, reconnect, recover, Replay, and export
evidence. No author or client gains a second state-mutation path.

WorldStream Negotiate is the first serious public Pack. Counter remains an
internal walking skeleton/tutorial. Agent Heist remains a visual
demo/conformance workload. Investigation Room is deferred design research.

## Frozen invariants

- One process hosts many independent Rooms; each Room pins exactly one
  `PackRevisionLockV1` digest for its whole lineage.
- `ActivityPackV1` still has exactly `descriptor`, `initialize`, `reduce`,
  `view`, and `observe`.
- Room authority, Action Offers, privacy, Semantic Time, timers, Recovery,
  Replay, integrity, storage, and one-process deployment semantics do not
  change for portable Packs.
- Public Components use `worldstream/component-deterministic/v1`: zero imports,
  no WASI linker, synchronous calls, fresh Store/instance, and fixed
  revision-bound limits.
- Host Operator approval names exact verified bytes. Names, semantic versions,
  authorship, and signatures alone never select or authorize behavior.
- Original bundle and Component bytes follow retained Room lineage through
  backup, restore, transfer, upgrade, export, and safe removal checks.
- Models, prompts, private memory, tools, credentials, strategy, commercial
  signing keys, and network calls remain in external clients/Runners.

## Phase 0 — Contract migration

Outcome: every normative source says Runtime plus installable Packs and fails
closed while portable identities are not implemented.

Deliver ADRs 0014/0015, partial-supersession annotations for ADRs 0001/0010,
reconciled normative/manual sources, and a compatibility pair reset to
`manifest_kind = "specification"` and `release_ready = false`. Any unresolved
required identity must be named explicitly; Phase 5 replaces the Negotiate
placeholders with the exact production-proven identities.

Exit gate: documentation, manifest parity, and embedded-manifest validation
pass without claiming a release.

## Phase 1 — One portable admission seam

Outcome: the existing checked `ActivityPackV1` host remains the only semantic
validator while a portable adapter supplies owned descriptor and canonical
request/result bytes.

Deliver an adapter-owned descriptor lifetime, canonical owned envelopes for all
five operations, one narrow validated registry admission interface, and one
startup registry injected into SQLite and PostgreSQL. Embedded Rust revisions
remain internal oracle/compatibility entries; no legality, privacy, timer, or
fault semantics are duplicated.

Exit gate: existing Counter/Heist and storage/Replay tests retain their meaning,
and an invalid portable registry entry cannot become runnable.

## Phase 2 — Bundle verifier, Component Host, and local CAS

Outcome: exact approved portable revisions load safely and reproducibly after
restart.

Deliver one deep bundle/CAS module with a small public surface: `inspect`,
`install`, `set_selectable`, `export`, and safe `remove`. Its implementation
owns deterministic ustar parsing, path/size policy, member digests, revision
cross-identity, Component preflight, Wasmtime configuration, golden/privacy
execution, fsync/atomic rename, approval recovery, retained-reference checks,
and tombstones.

The Component Host pins Wasmtime 48.0.1 or newer within the tested line,
preflights exact imports/exports before linking, installs no WASI, creates a
fresh Store/instance for each callback, and enforces fixed fuel/memory/table/
stack/input/output/concurrency limits. AOT cache bytes are host-owned,
engine/config/source-bound, integrity checked, and disposable.

Exit gates:

1. Byte tampering, traversal, links, duplicates, forbidden imports/exports,
   malformed output, resource exhaustion, and cache mismatch fail closed.
2. Crash-point and idempotent reinstall tests preserve exact originals.
3. Both storage profiles use the same startup registry.
4. Retained missing/corrupt bundles close readiness.
5. No callback failure reaches persistence.

## Phase 3 — Operator lifecycle and portability

Outcome: a Host Operator can manage Packs without editing Rust or databases.

Deliver `worldstreamctl pack` commands for inspect, approve, install,
inventory/status, restart-readiness, export, selection changes, safe removal,
Replay, and offline evidence verification.

Extend backup, restore, and SQLite-to-PostgreSQL transfer inventories to include
the original bundle bytes for every referenced semantic revision. Restore may
reestablish retained-runnable status but selection defaults off. Upgrade
preflight recompiles from original Component bytes and proves every retained
Replay before readiness.

Exit gate: a database-only backup with referenced portable lineage is reported
incomplete; there is no force-remove or newer-revision substitution path.

## Phase 4 — TypeScript Pack Author experience

Outcome: an outside TypeScript developer can build a nontrivial Pack without
Rust or manual WIT/schema/digest work.

Publish release-pinned `@worldstream/pack-sdk` and
`@worldstream/pack-cli`. One project contains:

```text
worldstream-pack.json
src/pack.ts
fixtures/golden.ts
fixtures/privacy.ts
test/pack.test.ts
package.json
package-lock.json
tsconfig.json
```

Supported commands are scaffold, `pack:check`, `pack:test`, `pack:build`,
`pack:inspect`, and `pack:prove`. The owned wrapper pins TypeScript, Jco 1.32.1,
and ComponentizeJS 0.22.0; runs strict `tsc`; rejects ambient nondeterministic
APIs and mutable module authority; forces `--disable all`; checks exactly five
exports/zero imports; generates schemas/codecs/locks/evidence; invokes the
production host; and emits one `.wspack` plus a first-success receipt.

Prompt assistance is required for the first release but optional for the
author. It asks bounded domain questions and a trusted generator writes the
same project. It cannot execute model-supplied shell commands, escape the
project root, approve/install a Pack, or place provider/model/key/prompt data in
the bundle, Room, daemon, or evidence.

Exit gates:

- 60-minute clean-directory outside-adopter path from scaffold to one real
  local Room, two distinct Roles/views, one accepted Action, one expected
  rejection, complete proof, and retained old-revision verification;
- 30-minute guided prompt-assisted path to the same project and proof;
- no Rust, paid service, manual digest editing, or maintainer intervention.

## Phase 5 — WorldStream Negotiate

Outcome: the platform proves a serious realtime application that benefits from
shared authority, exact privacy, independent participants, deadlines,
reconnect, and audit evidence.

Implement `worldstream.negotiate` through the public TypeScript path and a
first-party native oracle. The exact profile, Roles, phases, Actions, approval,
privacy, timers, Attention, Outcomes, adapter authority, and dual evidence
export are frozen in [WorldStream Negotiate](negotiate.md) and ADR 0015.

The golden story is:

`buyer Proposal → seller counter → approval request → Human exact signed
approval → restart/reconnect → buyer acceptance → selection → independent
Agreement signatures → commitment → identical Replay/evidence export`.

Exit gates include byte mutation, stale Room and A202 heads, expiry, wrong
signer, Role violations, privacy noninterference, signed deadline resolution,
original-byte restart, native/Component parity, and offline dual-proof
verification.

## Phase 6 — Product surfaces

Outcome: users operate the platform without learning Kernel internals.

- **Studio:** bundle digest/compatibility inspection, approval/install/restart
  readiness, schema-driven Genesis setup, Membership/handoff, opaque runner
  credential references, runner readiness, timers, Replay, and evidence export.
- **Participant Console:** independent cursors/reconnect, Projections/Frames,
  exact Action Offers, approval/signing panels, receipts, Replay, and evidence
  download.
- **Application SDK:** sessions, cursor/reset/reconnect, Actions/receipts,
  canonical wrappers, signer hooks, and evidence verification. Activation and
  Participant Action capabilities remain separate.
- **Runners:** own every model, prompt, tool, private memory, strategy,
  credential, invocation, and private signature.

Studio never becomes Pack source, a participant, a commercial signer, an
arbitrary shell, a private-Projection bypass, or a second rules engine.

## Phase 7 — Release qualification

Promote the compatibility pair to `manifest_kind = "release"` and
`release_ready = true` only after every Negotiate semantic/component/schema/
codec/golden/bundle identity is independently resolved and checked. Then run
the existing native Linux, native Windows, Linux/amd64 OCI, macOS source,
SQLite, PostgreSQL, migration, transfer, restore, failure, resource, soak,
privacy, and supply-chain gates with the new Pack/bundle subjects.

A distribution is verified only when its detached `release-manifest.json`,
checksums, Sigstore material, SPDX SBOM, SLSA provenance, and every hard-gate
evidence subject bind the exact final bytes.

## Deferred work

- Go and additional Pack Author SDK languages;
- network Pack registry, discovery, marketplace, or hot loading;
- generic host imports/effects or WASI capabilities;
- generated arbitrary UI or third-party renderer code;
- Investigation Room and generic application artifact upload;
- multi-supplier/auction negotiation and post-commit commercial lifecycle;
- A2CN adapter;
- payments, crypto, agent labor marketplace, workflow canvas, coding harness,
  model hosting, vector memory, multiple live WorldStream processes,
  clustering, federation, and multi-region writes.

## Principal risks

| Risk | Containment |
|---|---|
| Portable adapter duplicates semantic checks | Keep `ActivityPackHostV1` as the sole checked validator; adapter only translates exact canonical envelopes. |
| Bundle identity diverges from Room identity | Preserve `PackRevisionLockV1`, bind exact Component through `rule_source_digest`, and keep physical bundle digest separate. |
| Backup claims durability without executable rules | Treat every referenced original `.wspack` as required recovery material. |
| TypeScript toolchain is large or nondeterministic | Pin owned wrappers, retain exact bytes, compare behavior/goldens, and do not promise byte-reproducible upstream output. |
| Wasmtime limits are mistaken for hostile tenancy | State the capability/fail-closed claim narrowly; add process/container ceilings and upgrade tests. |
| Negotiate becomes a second protocol authority | Keep one Room order and one Pack; adapters only submit recorded input and export linked proof. |
| Studio becomes orchestration or hidden rules | Keep it a typed operator client and Participant Console separate. |
| Adoption gate is self-certified | Require clean-directory outside-developer observations before making the time-to-first-success claim. |
