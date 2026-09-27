---
status: accepted
date: 2026-08-30
---

# Install operator-approved WASI-free Activity Pack Bundles

WorldStream will expose one language-neutral public Activity Pack contract as a
WASI-free WebAssembly Component with exactly the existing five
`ActivityPackV1` operations: `descriptor`, `initialize`, `reduce`, `view`, and
`observe`. The Component is the deployment target, not the authoring language;
the first supported Pack Author SDK is TypeScript. A verified, deterministic
`.wspack` Activity Pack Bundle carries the exact original Component, revision
lock, descriptor, schemas, codecs, immutable static material, dependency lock,
golden corpus, and conformance facts. A Room continues to pin the semantic
`PackRevisionLockV1` digest for its whole lineage; the exact Component BLAKE3 is
bound through `rule_source_digest`, while a separate BLAKE3 identifies every
byte of the physical bundle.

The Host Operator must explicitly approve one verified bundle digest before an
offline installation can become runnable or selectable. The daemon builds one
immutable registry at startup from embedded revisions and approved local
bundles, and injects that same registry into every storage backend. Original
bundle bytes are retained in a local content-addressed store and must accompany
backup, restore, and transfer whenever retained Room lineage references them.
Selectable implies retained-runnable; safe removal requires proof that no Room
lineage references the revision. There is no name/version fallback, force
removal, hot loading, network registry, or automatic approval.

The pre-existing immutable `DeploymentIdentityV1` ledger continues to identify
the base Runtime Distribution and its embedded Pack set. It is not mutable
installed-Pack authority and portable bundles are not appended to it. Portable
installation state lives in the separately verified local bundle inventory;
backup and transfer carry every referenced original bundle in a distinct
section, and the destination establishes its own approval state. This split
allows offline Pack installation followed by restart without weakening the
immutable distribution or transfer witness.

“Official” does not mean “embedded.” Agent Heist remains part of the base
Runtime Distribution. Negotiate is an official portable Bundle carried by the
Starter Distribution and admitted through the same explicit operator lifecycle
as any other portable Pack. Adding an official portable release never rewrites
the base Distribution identity.

Portable Components execute synchronously under the
`worldstream/component-deterministic/v1` profile in a pinned Wasmtime host. The
launch profile permits zero imports and installs no WASI linker; every callback
uses a fresh Store and instance under fixed fuel, memory, table, stack, byte,
and admission-concurrency limits. The Host may use bounded internal parallel
Cranelift compilation to keep cold startup usable; Pack admission remains
serialized and the compilation schedule has no semantic or authority effect.
Original Component bytes are authoritative and any AOT
cache is disposable. Traps, malformed output, policy violations, and identity
or Replay disagreement fail closed before persistence under the existing
`PackFault`, Room fault, and quarantine semantics. This is capability-denying
execution of operator-approved untrusted application code, not a claim of
hostile multi-tenant process isolation.

The public author path is one code-first TypeScript project. Deterministic
scaffolding and optional first-release prompt assistance generate the same
reviewable project and pass the same strict checks, production-host tests,
privacy corpus, build, and proof. Prompt assistance, models, credentials,
signing keys, private strategy, and arbitrary effects stay outside
`worldstreamd` and outside the bundle's authoritative behavior.

This decision partially supersedes ADR 0001's Heist-first / Investigation-only
release order and ADR 0010's compiled-in-only distribution conclusion. Counter
remains an internal tutorial/conformance Pack, Agent Heist becomes a bundled
demo/conformance Pack, Investigation Room is deferred, and WorldStream
Negotiate is the public Pack selected in this decision. All other Room Kernel boundaries
remain unchanged.
