# Portable Activity Pack execution — decision record

Date: 2026-08-30

## Verdict

**PASS the WASI-free WebAssembly Component execution path. Select TypeScript as
the first public SDK language, conditional on a WorldStream-owned build and
conformance wrapper. Do not exercise the core-WebAssembly fallback.**

The experiment ran a meaningful buyer/seller negotiation through the same
five semantic operations as `ActivityPackV1`. A Rust host loaded the retained
original Component bytes, rejected ambient imports, created a fresh instance
for every callback, checked byte-for-byte output against a native oracle, and
proved that fuel, memory growth, stack exhaustion, and host-boundary excess
cannot produce committable output. A second startup reconstructed the same
transcript from the retained bytes.

This proves the portable execution mechanism. It does not make the prototype
host, its WIT, or its JSON envelopes the final production ABI.

## Gate evidence

| Gate | Result | Evidence |
| --- | --- | --- |
| Meaningful non-Rust two-Role Pack | Pass | Buyer and seller propose, counter, reject a stale acceptance, receive different private views and Action Offers, and reach an agreement. |
| Five-operation semantic boundary | Pass | The Component exports exactly `descriptor`, `initialize`, `reduce`, `view`, and `observe`. |
| No ambient capabilities | Pass | The built Component has zero imports; a fixture importing a WASI clock is rejected before invocation. |
| Canonical parity | Pass | Nine callback results match independently constructed canonical Rust oracle bytes. |
| Deterministic containment | Pass | Fuel burn, 512 MiB allocation, recursive stack exhaustion, oversized output, and oversized input all fail without a usable result. |
| Callback isolation | Pass | A deliberately mutable module counter returns `1` on every call because the host creates a fresh Store and instance. |
| Restart and Replay basis | Pass | Two independent Engine compilations from the retained original bytes produce an identical callback transcript digest. |
| Offline startup loading | Pass | The Component is staged under its SHA-256 in a local content-addressed directory and reloaded without a registry or network fetch. |
| Cache authority | Pass | The original Component remains authoritative; serialized Wasmtime output is labeled and treated only as a disposable host cache. |

One observed run produced:

```text
component bytes: 12,488,754
component sha256: 2d2401666a0bb889607a2571ed283cf67cbdb4cd59ff824c1c9970d6541bf6f7
callback count: 9
transcript sha256: d2704194cc2850a65f18ea6eeb7cc4eddca19bc3c4a65299b9f7a2a1f9c69dca
first compile: 9,121 ms
restart compile: 9,119 ms
first callback set: 10 ms
restart callback set: 10 ms
```

Build digests are evidence for exact retained bytes, not a reproducible-build
claim: ComponentizeJS emitted different bytes from otherwise identical builds
while runtime outputs remained equal.

## First SDK decision

### TypeScript: selected behind WorldStream tooling

Jco 1.32.1 and ComponentizeJS 0.22.0 produced and executed the import-free
Component using only Node/npm. This is the shortest viable non-Rust authoring
route, but raw upstream scaffolding fails the public journey:

- its generated build imports eighteen WASI interfaces unless WorldStream
  forces `--disable all`;
- componentization erases TypeScript types but does not type-check them;
- its implementation skeleton weakens generated parameter types to `any`;
- pure-mode failures are opaque traps;
- the StarlingMonkey artifact is about 12.5 MiB and the build is memory-heavy;
- the much smaller QuickJS backend cannot disable all capabilities, retains
  eighteen imports, and adds an `init` export; and
- the upstream toolchain is explicitly experimental and must be pinned and
  supply-chain reviewed.

Therefore the supported journey is a WorldStream-owned
`pack new/build/check/test` surface that pins the toolchain, vendors the WIT
and declarations, runs `tsc`, always disables capabilities, statically rejects
ambient APIs and mutable module state, verifies exact imports and exports,
smoke-invokes every operation, and runs the host conformance suite.

### Go: not selected for the first SDK

The current stock Go/componentize-go path produced a functional 4.52 MiB
Component, but it imported eighteen ambient WASI interfaces. TinyGo produced
an 8.3 KiB zero-import Component, but only after incompatible tool-version
workarounds, manual module pinning, separately installed Wasm tools, nondefault
GC flags, and hand-written unsafe canonical-ABI allocation glue. That is not a
credible 60-minute public Pack Author journey. The result remains useful as a
future SDK engineering path after generated bindings and memory ownership are
audited and hidden behind WorldStream tooling.

## Production requirements learned from the host probe

- Preflight exact import names and versions before linking. Wasmtime's linker
  may otherwise apply semver compatibility matching.
- Pin Wasmtime at least to 48.0.1; 48.0.0 had a relevant Component composition
  context-slot defect.
- Compile synchronous Component support only; expose no WASI linker and no
  general host effects.
- Use a new Store and Component instance per callback.
- Enable fuel and NaN canonicalization; disable threads and relaxed SIMD in the
  supported execution profile.
- Bound host input/output copies, stack, tables, instances, memories, and each
  memory. Per-memory limits are not aggregate process-RSS limits, so production
  also needs an aggregate containment strategy and operational measurement.
- Never put a blocking host call behind fuel accounting. The initial ABI has no
  such imports.
- Treat undersized native-thread stacks carefully because Wasmtime documents
  abort risk; prove the selected stack/thread configuration in production tests.
- Validate cache key, engine/config identity, and digest before any unsafe AOT
  deserialization; cache failure falls back to compiling retained original
  Component bytes.
- A trap, malformed/noncanonical result, limit excess, interface mismatch, or
  host fault becomes a fail-closed `PackFault` and never reaches persistence.

## Explicit limits

The prototype does not prove hostile-code security, aggregate process
isolation, production ABI stability, engine-upgrade compatibility, signed
bundle provenance, or the complete 60-minute outside-adopter experience. Those
remain production Pack Host, bundle, SDK, and acceptance-test obligations. It
does prove enough to continue the Component design without a second portable
ABI.

Primary upstream references used by the authoring probes:

- [Jco authoring guide](https://bytecodealliance.github.io/jco/creating-new-js-components.html)
- [Jco repository](https://github.com/bytecodealliance/jco)
- [ComponentizeJS](https://github.com/bytecodealliance/ComponentizeJS)
- [Wasmtime Rust API](https://docs.rs/wasmtime/latest/wasmtime/)
- [TinyGo WebAssembly Component Model guide](https://tinygo.org/docs/guides/webassembly/wasi/)
