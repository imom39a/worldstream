# Deterministic Local Activity Pack Execution Formats

Date: 2026-08-29

Status: Research for the Wayfinder decision “Research deterministic local Activity Pack execution formats”

Scope: execution and packaging formats for authoritative Activity Pack Revisions; Rust dynamic libraries and remote authoritative reducers are intentionally excluded

## Executive finding

WorldStream should preserve `ActivityPackV1` as its semantic contract while separating that contract from one physical execution format.

The strongest path is:

1. **Use separately packaged, statically composed Rust crates as the bridge from the current codebase.** This proves that the Room Kernel is independent of Counter, Agent Heist, and Negotiation without weakening current guarantees. It is not an installable plugin system and does not solve non-Rust authoring.
2. **Prototype a WebAssembly Component Model execution adapter as the target public pack format.** The component must be synchronous, local, content-addressed, instantiated without WASI, and limited to the five `ActivityPackV1` operations plus a narrowly specified deterministic helper capability. Wasmtime must run it under an explicit deterministic feature profile, deterministic fuel, memory/table/instance/stack limits, bounded host-boundary copying, and a fresh execution instance for every callback.
3. **Retain original component bytes, not Wasmtime precompiled artifacts, as the executable identity.** Precompiled artifacts are caches only: Wasmtime documents that deserialization succeeds only with the same Wasmtime version and engine configuration. Every WorldStream upgrade must recompile the original component bytes and pass the existing golden-corpus and full-Replay compatibility gates before serving retained Rooms.
4. **Keep core WebAssembly with a custom byte-buffer ABI only as a contingency.** It has the same sandbox and deterministic execution foundation, but WorldStream would have to invent and maintain memory ownership, calling, error, and language-binding conventions that WIT and the Component Model already define.
5. **Do not make Starlark the authoritative public pack runtime.** Starlark is genuinely hermetic and deterministic by design and is attractive for Python-like authoring, but the available Rust interpreter describes its memory limit as best-effort and explicitly says it is not a security boundary for malicious code. Starlark remains plausible for configuration, pack scaffolding, or a future source language that compiles to the portable artifact.

The research therefore supports a **native bridge plus portable target**, not an immediate all-at-once replacement:

```text
WorldStream Room Kernel
        |
        v
ActivityPackHostV1
  canonical input/output validation, privacy checks, bounds, Replay
        |
        +-----------------------------+
        |                             |
        v                             v
trusted native executor          retained Wasm Component
(statically composed Rust)       (future installable format)
```

The Component Model is not the same thing as WASI. WorldStream should use WIT/component linking for a typed boundary while rejecting WASI imports for clocks, random, filesystem, sockets, HTTP, environment, and stdio. WASI exists specifically to expose such system capabilities; the pack contract must not receive them.

This recommendation is conditional on a prototype. The decisive unknowns are not whether Wasmtime can call a component—it can—but whether WorldStream can prove callback byte parity, deterministic resource-exhaustion behavior, safe hard limits at the component boundary, acceptable fresh-instance cost, and a realistic non-Rust authoring path on all supported WorldStream platforms.

## What the current contract requires

WorldStream is not choosing a generic extension mechanism. It is choosing how to execute code that decides Authoritative Room State.

The repository’s canonical model says that every Room pins one immutable Activity Pack Revision—rules, schemas, codecs, and executor—for its whole lineage ([`CONTEXT.md`](../CONTEXT.md#rooms-and-rules)). The accepted contract has exactly five operations:

- `descriptor`;
- `initialize`;
- `reduce`;
- `view`; and
- `observe`.

Every callback is pure, synchronous, bounded, and completes before persistence handoff. The pack receives no storage, network, filesystem, environment, clock, scheduler, Session, delivery, telemetry, Activation, model, wallet, secret, or artifact-byte capability. For equal canonical inputs and the same revision lock, it must return byte-identical canonical outputs on every supported platform ([Activity Pack Design](activity-packs.md#activitypackv1), [`ActivityPackV1`](../crates/worldstream-core/src/activity_pack.rs)).

The host already performs substantial work outside the executor:

- exact-Head and Action Offer admission;
- canonical input and output validation;
- JSON-schema and byte/count/nesting bounds;
- Role and viewer authorization checks;
- timer normalization;
- observation/privacy consistency checks;
- panic-to-`PackFault` containment where Rust unwinding is possible; and
- golden transcript verification at registry construction.

The current daemon registry statically combines Counter and Agent Heist registries ([`registry.rs`](../crates/worldstream-core/src/registry.rs)). An exact semantic digest resolves to an in-process `Arc<dyn ActivityPackV1>`, descriptor/schema/codec artifacts, an executor artifact digest, and golden evidence. Missing runnable support is a compatibility failure; the registry never substitutes a newer executor ([ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md)).

Replay is stronger than event deserialization. It runs the exact retained pack reducer, recomputes state and lineage hashes, and performs no external effects ([ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md#replay-authorization)). Release, restore, and storage-transfer gates must exercise exact executors through full Replay ([ADR 0011](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md)).

These facts produce the following non-negotiable evaluation criteria.

### Evaluation criteria

| Criterion | Required property |
|---|---|
| Synchronous semantics | One callback returns or fails before `PreparedRoomWriteV1` persistence; no future, remote callback, or continuation |
| Exact retained executability | A Room’s pinned revision remains runnable for load, advance, view, observe, Recovery, and Replay |
| Deterministic result | Equal canonical inputs produce byte-identical canonical outputs across supported platforms and storage profiles |
| Capability denial | No ambient time, entropy, environment, filesystem, network, database, model, or other effects |
| Resource containment | CPU, memory, stack, tables, instances, and host-boundary copies are bounded before untrusted output can affect the host |
| Failure containment | Trap, panic, malformed output, limit breach, or unavailable executor yields no canonical commit |
| Stable compatibility | The host ABI and revision identity are explicit; engine/toolchain upgrades cannot silently reinterpret retained Rooms |
| Portable packaging | One immutable artifact can be installed and retained without rebuilding `worldstreamd` |
| Authoring reach | A credible path exists for Rust and non-Rust pack authors without weakening deterministic Replay |
| Operational fit | SQLite/PostgreSQL semantics, the one-process authority model, and atomic commit-before-ack remain unchanged |

## Comparison

| Format | Synchronous fit | Capability isolation | Resource bounds | Retained artifact | Portable install | Non-Rust authoring | Overall role |
|---|---|---|---|---|---|---|---|
| Statically composed Rust crate | Native and already proven | Convention and code review only | Output bounds; no safe callback CPU/memory preemption | Release binary plus source/dependency lock | No; rebuild server distribution | No | Best bridge and trusted first-party path |
| WASI-free Wasm Component + WIT + Wasmtime | Direct synchronous exports | Strong: only linked imports exist | Fuel, hostcall fuel, `StoreLimits`, stack and instance limits; supervisor still needed for engine/OOM failure | Original component bytes plus execution profile | Yes | Yes in principle; tooling maturity varies | Best target public format, pending prototype |
| Core Wasm module + custom buffer ABI | Direct exports | Strong: only module imports exist | Same Wasmtime controls | Original module bytes plus execution profile | Yes | Compiler-dependent; bindings are WorldStream’s burden | Viable fallback if Component Model tooling blocks |
| Embedded Starlark source | Direct interpreter calls | Hermetic language if host adds no effectful built-ins | Instruction limit; in-process memory limit is best-effort | Source/static data plus exact interpreter semantics | Yes | One Python-like language | Useful source/config layer; not recommended as authoritative public runtime |

## Option A: statically composed Rust packs

### Fit

This is the current execution model, and it fits the five-operation seam exactly. Calls are ordinary typed Rust calls, so there is no serialization boundary between `ActivityPackHostV1` and the executor. The host catches unwindable panics and validates every returned value before persistence. The existing golden execution check binds the concrete executor, revision metadata, codecs, and transcript.

Extracting Counter, Agent Heist, and Negotiation into independent crates would create the correct source and release boundaries:

```text
worldstream-core              no named Activity Pack implementation
worldstream-pack-counter      tutorial/conformance pack crate
worldstream-pack-agent-heist  systems demonstration pack crate
worldstream-pack-negotiate    flagship reference pack crate
worldstreamd-starter          statically composes selected pack crates
```

This is meaningfully similar to a runtime with adapter implementations even though the final executable is statically linked. It lets the project prove that the Kernel has no pack-name branches and that first-party packs can be independently versioned and tested.

### Strengths

- It preserves every accepted correctness property with the least new machinery.
- Rust types make the complete host contract explicit at compile time.
- Existing conformance, schemas, canonical codecs, privacy checks, and golden transcripts remain directly reusable.
- Performance is predictable and callback overhead is minimal.
- The current release compatibility manifest already knows how to enumerate embedded executors.

### Limits

- A Host Operator cannot install a pack into an existing binary. A custom pack means building and distributing a custom WorldStream server.
- Pack authors must use Rust and align with the WorldStream crate/toolchain versions.
- Trusted same-process code is not sandboxed. The existing design explicitly notes that OOM, aborting panic, and permanently blocked callbacks cannot be safely preempted; the process supervisor is the recovery boundary ([Activity Pack Design](activity-packs.md#bounds-faults-and-containment)).
- Static composition is a source/package boundary, not a durable public binary ABI. This is acceptable for the bridge because Rust itself says the native Rust ABI offers no stability guarantees; it is another reason not to turn the bridge into dynamically loaded Rust libraries ([Rust Reference: external blocks and ABI](https://doc.rust-lang.org/stable/reference/items/external-blocks.html#abi)).

### Assessment

Static Rust should be the **first architectural cleanup**, but it should not be presented as the final “any developer can install a pack” story. It proves separation without prematurely freezing a portable ABI.

## Option B: WASI-free WebAssembly Components

### Why the Component Model fits

WebAssembly modules are portable deployment and compilation units. Core WebAssembly deliberately specifies no syscalls or APIs; all external functionality arrives through imports chosen by the embedding host ([WebAssembly portability](https://webassembly.org/docs/portability/#api)). The Component Model adds typed imports and exports through WIT. WIT packages and worlds are intended to describe shared interfaces and support generated bindings across source languages ([Component Model WIT specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)).

This maps naturally to `ActivityPackV1`:

```text
worldstream:activity-pack@1.0.0
  exports descriptor
  exports initialize
  exports reduce
  exports view
  exports observe
  imports, at most, worldstream:deterministic-context@1.0.0
```

The WIT surface should keep WorldStream’s canonical codec authoritative. A practical first interface would use typed operation envelopes but carry canonical WorldStream values as `list<u8>` rather than duplicating every evolving Core and pack-owned JSON schema as WIT records. Generated language SDKs can expose ergonomic native types above those exact bytes. The host decodes and validates the same `CanonicalJsonV1` values it validates today.

The Component Model and WASI must be kept conceptually separate. WASI 0.2 and 0.3 include explicit interfaces for clocks, random, filesystem, sockets, CLI/environment, and HTTP ([WASI 0.2 interface inventory](https://wasi.dev/releases/wasi-p2), [WASI repository](https://github.com/WebAssembly/WASI)). Those imports are the capabilities WorldStream’s contract forbids. A pack component should therefore be **WASI-free**, not “given a restricted WASI context.” Installation should fail if component inspection finds any import outside the exact WorldStream deterministic helper interface.

Wasmtime’s component linker resolves imports by name and fails instantiation when a required import is absent or has the wrong type. This gives the host an enforceable allowlist rather than a convention ([Wasmtime component `Linker`](https://docs.wasmtime.dev/api/wasmtime/component/struct.Linker.html)).

### A candidate WIT shape

This is illustrative, not a frozen ABI:

```wit
package worldstream:activity-pack@1.0.0;

interface deterministic-context {
  variant deterministic-fault {
    invalid-bound,
  }

  uniform-index: func(label: string, index: u64, bound: u64)
    -> result<u64, deterministic-fault>;
}

interface pack {
  record pack-fault {
    code: string,
    safe-detail: option<string>,
  }

  descriptor: func() -> result<list<u8>, pack-fault>;
  initialize: func(input: list<u8>) -> result<list<u8>, pack-fault>;
  reduce: func(input: list<u8>) -> result<list<u8>, pack-fault>;
  view: func(input: list<u8>) -> result<list<u8>, pack-fault>;
  observe: func(input: list<u8>) -> result<option<list<u8>>, pack-fault>;
}

world activity-pack {
  import deterministic-context;
  export pack;
}
```

The deterministic helper import is not a sixth pack operation. It is the portable equivalent of the current `DeterministicContextV1` argument. It must be a pure function of Room seed, pack digest, next Room sequence, label, index, and bound. The host binds those facts for each call; the guest cannot provide or change them. A prototype should compare this form against embedding a prederived deterministic context in the input and implementing the algorithm in each SDK. A host import minimizes cross-language drift, while an input-only interface is even easier to audit and meter.

### Determinism profile

WebAssembly is close to deterministic but “plain Wasm” is not a sufficient policy statement. The WebAssembly 3.0 specification now defines a deterministic profile: it canonicalizes generated NaNs and fixes relaxed-vector behavior. It still explicitly permits nondeterminism for `memory.grow` and `table.grow` resource exhaustion ([WebAssembly 3.0 deterministic profile](https://webassembly.github.io/spec/core/appendix/profiles.html), [WebAssembly 3.0 specification PDF](https://webassembly.github.io/spec/core/_download/WebAssembly.pdf)).

Wasmtime exposes the controls WorldStream needs:

- `Config::consume_fuel` instruments execution so fuel exhaustion deterministically traps; Wasmtime explicitly distinguishes it from epoch interruption, which is not deterministic ([Wasmtime `Config::consume_fuel` and epoch interruption](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.consume_fuel)).
- `Store::set_fuel` supplies the per-callback budget ([Wasmtime `Store`](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html#method.set_fuel)).
- `Store::limiter` with `StoreLimits` bounds memory, tables, and instances ([Wasmtime `Store::limiter`](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html#method.limiter)).
- `Config::max_wasm_stack` traps WebAssembly stack overflow near a configured limit, with documented caveats about the native caller stack ([Wasmtime `Config::max_wasm_stack`](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.max_wasm_stack)).
- `Config::cranelift_nan_canonicalization` exists specifically for deterministic computation, and `Config::relaxed_simd_deterministic` fixes architecture-dependent relaxed SIMD behavior ([Wasmtime determinism configuration](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.cranelift_nan_canonicalization)).
- Recent Wasmtime releases expose hostcall fuel because a guest can otherwise force large host allocations and copies through WIT `string` and `list<T>` values. The Wasmtime security advisory instructs embedders to bound their own host APIs as well ([Wasmtime advisory GHSA-852m-cvvp-9p4w](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-852m-cvvp-9p4w)).

WorldStream must set these controls explicitly. Wasmtime documents that `Config` defaults can differ with Cargo features and target, so the compatibility contract cannot depend on defaults ([Wasmtime `Config` defaults](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#defaults)).

A candidate WorldStream component execution profile is:

1. Synchronous Component Model calls only. Reject component-model async, futures, streams, threading, shared memory, and shared-everything features.
2. No WASI imports and no unknown imports. Allow only the exact versioned WorldStream deterministic helper, if the prototype retains it.
3. Disable threads and shared memory. Disable relaxed SIMD, or enable Wasmtime’s deterministic lowering; enable NaN canonicalization even though canonical Activity State already forbids floating point.
4. Use a fixed deterministic fuel schedule and a separate budget for each of the five operations. Record the budget/profile version in the compatibility manifest, not in Canonical History.
5. Apply hard per-store limits for memory bytes, tables, instances, and component resources; apply a stack limit and bounded hostcall copying.
6. Compile and cache the immutable `Component`, but create a fresh `Store` and component instance for every callback. No guest global, linear-memory mutation, resource handle, or language-runtime state survives across callbacks.
7. Permit no host logging, stdio, environment, or metrics import. Traps and host-level diagnostics are captured outside canonical output with bounded, sanitized detail.
8. Decode and validate every returned byte sequence through the existing host schema, privacy, Action Offer, timer, count, nesting, and canonical-byte checks before persistence.
9. Use deterministic fuel as the canonical execution cutoff. Epoch interruption may exist only as a last-resort operational kill switch; because it is nondeterministic, reaching it cannot become a domain rejection or accepted output.
10. Keep the external process supervisor. A WebAssembly sandbox reduces guest authority, but an engine bug, native OOM, or host integration bug can still threaten the process. Wasmtime itself treats safe execution of untrusted code as a security goal, not a reason for embedders to omit defense in depth ([Wasmtime security documentation](https://docs.wasmtime.dev/security.html)).

### The resource-exhaustion caveat

Fuel provides a deterministic CPU cutoff, but memory behavior needs a prototype-level proof. The WebAssembly deterministic profile retains resource-exhaustion nondeterminism for `memory.grow` and `table.grow`. A guest that observes a failed growth and returns an otherwise valid result could theoretically produce different output under different resource pressure.

WorldStream should not claim deterministic portable packs until the prototype establishes one of these enforceable policies:

- every declared memory/table maximum is within a reserved, startup-proven budget and growth up to that limit is guaranteed for a callback instance;
- growth beyond the deterministic limit traps rather than returning a guest-observable failure that pack code can handle as domain logic; or
- accepted producer profiles use fixed initial/maximum memory with no reachable growth instruction.

Unexpected host allocation failure must remain an operational failure with no commit. It must never be converted into an Activity rejection or valid alternate state. This is a genuine acceptance gate, not documentation polish.

### Retained executability and engine upgrades

The retained artifact should be the **original validated component binary**. Its digest, exact WIT host-contract version, deterministic execution profile, descriptor/schema bundle, codecs, static data, producer lock, and golden corpus should feed the Activity Pack Revision identity.

Wasmtime can serialize compiled modules and components for fast startup, but those blobs are not portable retained executors. Wasmtime documents that a serialized module is accepted only by the same Wasmtime version, and its component bindings additionally require the same engine configuration ([Wasmtime `Module::deserialize`](https://docs.wasmtime.dev/api/wasmtime/struct.Module.html#method.deserialize), [Wasmtime component serialization](https://bytecodealliance.github.io/wasmtime-py/component/index.html)). Therefore:

- original `.wasm` component bytes are durable and content-addressed;
- Wasmtime-compiled/AOT bytes are disposable caches keyed by component digest, Wasmtime build, target, and complete engine configuration;
- a WorldStream upgrade recompiles original bytes under the new engine;
- registry startup runs the exact golden corpus;
- release, restore, and transfer gates fully Replay every retained digest;
- any divergence blocks readiness or isolates the affected Room under the existing integrity model.

Wasmtime’s LTS policy is useful operationally: every twelfth release is supported for two years with security-fix backports. It does not remove WorldStream’s need for upgrade Replay gates, and old engines cannot be retained indefinitely without security maintenance ([Wasmtime LTS RFC](https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasmtime-lts.md)).

### Portable packaging

A first portable pack bundle can remain deliberately small:

```text
<pack-id>-<revision>.wspack/
  manifest.json                 canonical, versioned
  executor.component.wasm       exact retained executable bytes
  schemas/                      exact schema bytes
  static/                       immutable deterministic data
  golden/                       corpus and expected transcript digest
  provenance/                   producer lock, SBOM/signature if present
```

The physical envelope may later be a deterministic archive or OCI artifact; that choice does not belong in the executor ABI. The canonical manifest must hash every retained member. Installation is local and explicit in the first version—no registry discovery, marketplace, or runtime download is required.

The install transaction should:

1. verify archive and manifest shape;
2. recompute every content digest;
3. inspect the component’s exact imports, exports, and enabled feature set;
4. reject any undeclared import or forbidden WebAssembly feature;
5. compile with the explicit engine profile and hard limits;
6. execute the golden corpus and negative conformance cases;
7. atomically make the revision selectable; and
8. retain the exact immutable bundle for the lifetime of every Room lineage that pins it.

Replay and Recovery must never fetch code from a network. A revision may become non-selectable for new Rooms while its exact bundle remains locally runnable for retained Rooms, preserving the existing registry semantics.

### Rust and non-Rust authoring

The Component Model creates a credible multi-language path, but “can produce a component” and “can produce a deterministic WorldStream pack” are different claims.

The Bytecode Alliance’s `wit-bindgen` project currently provides guest generators for Rust, C, C++, C#, and Go, and points to JavaScript and Python componentizers. Its own CLI remains pre-1.0 and explicitly warns that it may change ([`wit-bindgen` README](https://github.com/bytecodealliance/wit-bindgen)). WIT itself supports semver-qualified packages and feature gates, but the broader Component Model is still developed incrementally through stable developer-preview subsets ([Component Model repository](https://github.com/WebAssembly/component-model), [WIT package/version rules](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#package-names)).

Practical authoring tiers should therefore be explicit:

- **Tier 1 initially: Rust guest SDK.** It is the fastest way to validate exact parity with the current native implementation and build a conformance oracle.
- **Tier 2 after proof: Go or C/C++.** These compiled toolchains have direct `wit-bindgen` support and can run the same cross-platform golden suite.
- **Tier 3 experimental: JavaScript/TypeScript.** ComponentizeJS can disable random, stdio, clocks, HTTP, and fetch to create a component with only its target-world imports. Its documentation says disabled random becomes deterministic and disabled timers panic; WorldStream must still forbid authors from treating those implementation stubs as its labeled deterministic randomness ([ComponentizeJS features](https://github.com/bytecodealliance/ComponentizeJS#features)).
- **Tier 3 experimental: Python.** `componentize-py` can emit a component with stubbed WASI, but its own current getting-started example pins an older `wasmtime-py` because of compatibility problems with newer versions. That is evidence to treat this as a later developer-experience track rather than the first compatibility promise ([`componentize-py` README](https://github.com/bytecodealliance/componentize-py)).

Each language SDK must make illegal ambient operations difficult, expose the same canonical codec and deterministic helpers, and ship the same golden vectors. Component import inspection remains authoritative; SDK linting is not a security boundary.

### Assessment

A WASI-free Component is the best target public format because it combines enforceable capability denial, local synchronous invocation, content-addressable executable bytes, portable installation, and a multi-language path. The price is a real subsystem: engine configuration, metering, artifact retention, conformance, security updates, and language SDKs. It should be adopted only after a focused prototype proves those obligations.

## Option C: core WebAssembly with a custom ABI

WorldStream could bypass WIT and the Component Model and define core module exports such as:

```text
descriptor() -> packed pointer/length
initialize(ptr, len) -> packed pointer/length/status
reduce(ptr, len) -> packed pointer/length/status
view(ptr, len) -> packed pointer/length/status
observe(ptr, len) -> packed pointer/length/status
alloc(len) -> ptr
dealloc(ptr, len)
```

This option retains most execution properties of Option B:

- core Wasm has no ambient APIs; imports are host-defined;
- Wasmtime fuel, memory/table limits, stack limits, deterministic configuration, and fresh instances still apply;
- raw module bytes can be content-addressed and retained;
- compiled artifacts remain version/configuration-specific caches.

It may also use a more mature and smaller portion of the WebAssembly specification than the Component Model.

The cost is that WorldStream becomes responsible for an ABI that the Component Model already standardizes:

- linear-memory allocation and ownership;
- pointer/length validation and integer overflow;
- string/list encoding;
- error and trap conventions;
- host/guest bindings for every supported language;
- toolchain-specific export naming and adapters;
- interface versioning and compatibility;
- component inspection/composition tooling that WIT otherwise supplies.

The Core WebAssembly binary format is a stable portable foundation, but custom sections are ignored by core semantics and imports/exports remain low-level ([WebAssembly core binary modules](https://webassembly.github.io/spec/core/binary/modules.html)). A WorldStream-specific byte ABI would be simple for one Rust proof but costly as a public ecosystem contract.

### Assessment

Core Wasm is the valid fallback if a prototype shows that Component Model tooling, binary size, or fresh instantiation is unacceptable. It should not be developed in parallel with the WIT route. The prototype should keep the internal executor adapter independent enough that swapping the wire binding does not affect Room semantics.

## Option D: embedded Starlark source

Starlark is the only language-specific embedded interpreter that merits serious consideration here. Its project explicitly lists deterministic evaluation and hermetic execution—no filesystem, network, or system clock—as design principles. It also avoids unbounded `while` loops, provides deterministic dictionary iteration, and is familiar to Python users ([Starlark project](https://github.com/bazelbuild/starlark), [Starlark language design](https://github.com/bazelbuild/starlark/blob/master/design.md)).

A pack could be a source bundle defining five functions. WorldStream would parse canonical input values, call those functions in a fresh interpreter, and canonicalize the returned values. Source and deterministic static data would be content-addressed; the exact interpreter/version and enabled built-ins would belong to the compatibility manifest.

This is attractive because:

- authoring is simpler than Rust;
- source is portable and inspectable;
- the language is deliberately small;
- ambient I/O is absent unless the host adds it;
- finite loops plus evaluator tick limits can bound common CPU abuse.

It is not the best authoritative runtime because:

- it provides one language rather than a portable target for several languages;
- WorldStream would own a second type/codec/SDK system beside Rust;
- exact retained semantics depend on the particular interpreter and its host-defined values/built-ins, not only the language source;
- dynamic typing moves more errors to installation or runtime;
- float and application-defined value behavior would require additional restriction;
- the canonical Rust implementation’s memory limit is explicitly best-effort, checks only part of memory, may substantially overshoot, and says the interpreter should not be considered secure against truly malicious code ([`starlark-rust` evaluator limits](https://docs.rs/starlark/latest/starlark/eval/struct.Evaluator.html)).

The Starlark specification also allows applications to add built-ins with side effects, so WorldStream would need an exact allowlist and its own stable host surface ([Starlark specification](https://github.com/bazelbuild/starlark/blob/master/spec.md#built-in-constants-and-functions)).

### Assessment

Starlark is viable as:

- configuration logic evaluated before Room creation;
- a Studio/scaffolding source format;
- a constrained authoring language that later compiles or lowers into the portable pack artifact; or
- a trusted local-only experimental executor.

It should not be the first public authoritative pack runtime because it is weaker than Wasmtime on isolation/resource enforcement and narrower than the Component Model on ecosystem reach.

## Why other embedded language VMs do not advance

Lua, JavaScript engines, Rhai, and similar embedded runtimes can all call five synchronous functions. They do not offer a better combined answer than the two viable routes above:

- each creates a WorldStream-specific source-language and interpreter compatibility commitment;
- time, random, filesystem, module loading, floating point, hash iteration, and other built-ins must be audited or replaced;
- bytecode is generally interpreter-version-specific, so source plus an exact interpreter remains the retained executable contract;
- safe hard memory and host-allocation limits are difficult in the same process; and
- supporting multiple source languages multiplies the authoritative compatibility surface.

JavaScript and Python are more compelling **as producers of a validated Wasm Component**, where WorldStream can enforce one installed artifact contract, than as additional host-native interpreters.

## Recommended architecture

### Preserve one semantic host

`ActivityPackHostV1` should remain the only code authorized to admit executor output into Core. The execution-format adapter belongs behind it:

```text
RoomTransitionPreparerV1
  -> ActivityPackHostV1
       -> validate canonical input and exact Head
       -> invoke one RetainedPackExecutorV1
            Native(Arc<dyn ActivityPackV1>)
            Component(RetainedComponentV1)
       -> validate canonical output, schemas, bounds, privacy, timers
  -> PreparedRoomWriteV1
  -> atomic storage commit
```

Native and Component execution must not become two semantic contracts. Both resolve the same five operations and must pass the same conformance transcripts. The executor adapter is not allowed to make storage calls, publish frames, start Invocations, or mutate a Room.

### Evolve revision identity deliberately

The current `PackRevisionLockV1` records descriptor/schema/codec digests, static data, rule-source digest, and deterministic dependency-lock digest. Registry validation also requires the executor artifact digest to equal the rule-source digest ([`PackRevisionLockV1`](../crates/worldstream-core/src/activity_pack.rs)). That assumption fits built-in Rust source but not a retained portable binary.

A portable revision lock needs, at minimum:

- executor format: `native-static-v1` or `wasm-component-v1`;
- exact executor component digest;
- exact WIT world/host-contract digest;
- canonical codec version;
- descriptor and schema bundle digests;
- deterministic static-data digests;
- deterministic execution-profile version;
- producer/toolchain dependency lock and provenance digests; and
- golden-corpus and expected-transcript digests.

The Room should continue to pin one semantic Activity Pack Revision digest. It should not separately choose or migrate execution formats after creation. If a native pack and a Component pack are expected to be semantically equivalent, they are still different revision artifacts unless a future ADR defines a formally verified cross-format identity scheme.

### Separate distribution from execution

WorldStream should expose three artifacts, not one blurred binary:

- **WorldStream runtime/server:** generic Room Kernel, storage, sessions, WebSockets, activation, Replay, and the pack host.
- **Official starter distribution:** runtime plus retained official packs so a new user can run a meaningful Room immediately.
- **Activity Pack bundle:** independently versioned rules and assets, initially native crates for custom distributions and later installable Components.

Agent runners, A2A/MCP adapters, LLM clients, and human UIs remain external clients. They do not run inside the Activity Pack sandbox.

## Recommended sequence

### Step 1: native separation

Extract Counter and Agent Heist from `worldstream-core` into independent pack crates. Add Negotiation as an independent pack crate. The Kernel crate must not import or branch on those pack names. Compose them only in the starter server distribution.

This step validates the runtime-plus-packs architecture without changing ADR 0010’s trusted compiled execution guarantee.

### Step 2: one narrow Component prototype

Build a throwaway executor adapter and port a small but semantically meaningful pack—not only Counter—to the WIT interface. A reduced Negotiation slice or one privacy/timer phase of Agent Heist is better because it exercises:

- Participant-private `view`;
- Action Offers;
- apply and clean rejection;
- timer requests;
- Attention;
- hidden and visible `observe` results; and
- deterministic Replay after restart.

Use a current Wasmtime LTS line, but bind every deterministic and resource setting explicitly.

### Step 3: prove parity and containment

The prototype is successful only if all of these pass:

1. **Semantic parity:** native and component executors produce identical canonical Genesis, dispositions, views, observations, timers, Attention, and golden transcript hashes for the same vectors.
2. **Cross-platform parity:** Linux x86-64, Windows x64, Linux/amd64 OCI, and the macOS source-build path produce byte-identical accepted outputs.
3. **Restart/Replay:** original component bytes are reloaded after restart and fully Replay retained Rooms without an AOT cache.
4. **Upgrade:** the same retained component is recompiled under a second supported Wasmtime build and passes full Replay; a deliberately changed output fails readiness.
5. **Capability denial:** components importing WASI clock, random, filesystem, environment, stdio, sockets, or HTTP fail installation.
6. **CPU bound:** an infinite or very expensive guest deterministically exhausts fuel with no commit.
7. **Memory bound:** memory/table growth, oversized WIT lists, language-runtime allocation, and host-boundary copying fail within hard limits and cannot return an alternate valid domain output.
8. **Stack bound:** deep recursion traps without aborting the server.
9. **State isolation:** a component attempting to retain mutable globals between calls observes a fresh instance and cannot influence the next callback.
10. **Privacy:** malformed or overbroad projections and observations fail through the existing host validation before exposure.
11. **Performance:** compile-cache and fresh-instance latency/memory are measured for Rust and at least one non-Rust guest. The release may choose conservative Room limits; it must publish measurements rather than assume negligible overhead.
12. **Packaging:** an immutable local pack bundle can be installed, retired from new selection, restarted, backed up, restored, and still run for retained Rooms without rebuilding `worldstreamd` or contacting a registry.

### Step 4: decide, then record an ADR

If the prototype passes, a new ADR should supersede the “compiled-in only” portions of ADR 0001 and ADR 0010 while preserving their semantic guarantees. It should freeze:

- the one public Component/WIT contract;
- the deterministic engine feature profile;
- resource and failure semantics;
- revision and bundle identity;
- installation, retirement, backup, restore, and purge rules;
- supported authoring tiers; and
- release compatibility evidence.

If the Component prototype fails only because Component Model tooling is unstable or too costly, repeat the same executor tests with the core Wasm custom ABI. If it fails because deterministic resource behavior cannot be made enforceable, keep public packs statically composed and do not weaken Replay to claim plugin support.

## Decisions this research supports

- **Yes:** WorldStream should be a runtime with Activity Packs running on that runtime.
- **Yes:** the Kernel and pack implementations should be independently packaged even before dynamic installation exists.
- **Yes:** static Rust is an appropriate first implementation and trusted first-party path.
- **Yes, conditionally:** a WASI-free WebAssembly Component is the best candidate for the installable public pack format.
- **No:** Activity Packs should not be ordinary remote adapters; they decide authoritative state synchronously.
- **No:** WASI should not be exposed to authoritative packs merely because the executor is WebAssembly.
- **No:** Wasmtime AOT/serialized bytes should not be the retained executor identity.
- **No:** “supports WebAssembly” alone is not a determinism guarantee; the runtime must freeze and enforce an explicit profile.
- **No:** non-Rust component production should not be advertised as stable until each supported language passes the same conformance and resource suite.
- **No:** a marketplace, dynamic download service, arbitrary frontend code, or general effect ABI is necessary for the first installable pack release.

## Remaining decision questions after the prototype

This research narrows but does not answer these implementation-policy choices:

1. Does the portable WIT world import the host’s deterministic helper, or receive all deterministic derivation material in each input and use SDK code?
2. Can WorldStream guarantee deterministic memory growth under fixed limits, or must the producer profile prohibit reachable `memory.grow`/`table.grow`?
3. What exact fuel and memory classes are part of pack metadata versus operator policy, and which changes create a new Activity Pack Revision?
4. Are native static packs a permanently supported advanced format or only a bootstrap/first-party implementation detail once Components ship?
5. Which non-Rust guest language earns the first supported tier based on artifact size, deterministic behavior, tooling stability, and developer demand?
6. What explicit retention/export/purge operation lets a Host Operator remove the last executable bundle after every dependent Room lineage is intentionally retired?

Those are now prototype- or product-policy questions. They do not require reopening the Room Kernel’s authority, ordering, privacy, atomic commit, activation, or Replay model.

## Primary sources

### WorldStream repository

- [`CONTEXT.md`](../CONTEXT.md)
- [Activity Pack Design](activity-packs.md)
- [ADR 0001: Freeze WorldStream as a Realtime Room Runtime](adr/0001-product-boundary.md)
- [ADR 0005: Canonical Core State, Integrity, and Hash Lineage](adr/0005-canonical-core-state-integrity-and-hash-lineage.md)
- [ADR 0006: Use One Backend-Neutral Atomic Room Commit](adr/0006-backend-neutral-atomic-room-commit.md)
- [ADR 0010: Activity Pack v1 and executable Replay retention](adr/0010-activity-pack-v1-and-executable-replay-retention.md)
- [ADR 0011: Gate Releases on Compatibility, Recovery, and Supply-Chain Evidence](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md)
- [`ActivityPackV1` implementation and registry validation](../crates/worldstream-core/src/activity_pack.rs)
- [Built-in WorldStream registry](../crates/worldstream-core/src/registry.rs)

### WebAssembly, Component Model, and WASI

- [WebAssembly 3.0 specification](https://webassembly.github.io/spec/core/)
- [WebAssembly 3.0 deterministic profile](https://webassembly.github.io/spec/core/appendix/profiles.html)
- [WebAssembly portability and import model](https://webassembly.org/docs/portability/)
- [WebAssembly Component Model specification repository](https://github.com/WebAssembly/component-model)
- [WIT specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [WASI repository and preview status](https://github.com/WebAssembly/WASI)
- [WASI 0.2 interface inventory](https://wasi.dev/releases/wasi-p2)

### Wasmtime and language tooling

- [Wasmtime `Config`](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html)
- [Wasmtime `Store`](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html)
- [Wasmtime Component `Linker`](https://docs.wasmtime.dev/api/wasmtime/component/struct.Linker.html)
- [Wasmtime `Module` serialization](https://docs.wasmtime.dev/api/wasmtime/struct.Module.html)
- [Wasmtime security model](https://docs.wasmtime.dev/security.html)
- [Wasmtime guest-controlled resource-exhaustion advisory](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-852m-cvvp-9p4w)
- [Wasmtime LTS policy](https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasmtime-lts.md)
- [`wit-bindgen` guest language support and versioning](https://github.com/bytecodealliance/wit-bindgen)
- [ComponentizeJS pure-component feature controls](https://github.com/bytecodealliance/ComponentizeJS#features)
- [`componentize-py`](https://github.com/bytecodealliance/componentize-py)

### Starlark and Rust

- [Starlark language project and principles](https://github.com/bazelbuild/starlark)
- [Starlark language design](https://github.com/bazelbuild/starlark/blob/master/design.md)
- [Starlark specification](https://github.com/bazelbuild/starlark/blob/master/spec.md)
- [`starlark-rust` evaluator resource limits](https://docs.rs/starlark/latest/starlark/eval/struct.Evaluator.html)
- [Rust Reference: external blocks and ABI](https://doc.rust-lang.org/stable/reference/items/external-blocks.html#abi)
