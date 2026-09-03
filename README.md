# WorldStream

> Agent orchestrators coordinate agents to complete work. WorldStream governs a shared reality in which humans and agents participate.

WorldStream is a self-hosted realtime room runtime for multi-agent applications. It lets external humans and AI agents participate in the same durable Room. An Activity Pack defines Activity State, Roles, exact Action Offers, visibility rules, timers, and Outcomes. WorldStream owns Core Room State—exactly Room Status plus the semantic Membership map—and supplies ordering, integrity, persistence, scoped realtime observations, cursor-based catch-up, targeted activation, recovery, and deterministic Replay.

The simplest architectural analogy is a multiplayer game server whose players may be humans or AI agents. The product is not game-specific: the server owns the shared reality and an installable Activity Pack supplies the domain rules. Participants see only their authorized view and submit typed actions. An AI model does not remain alive inside WorldStream; an application-owned Runner invokes it when work is available.

## Frozen product statement

> WorldStream is a self-hosted realtime room runtime for multi-agent applications. Its precise model is multi-participant: external humans and independently hosted AI agents join a rule-governed room, receive scoped realtime observations, submit typed actions, and retain continuity across connections and ephemeral agent invocations. Multiplayer server design is the architectural analogy, not a game-only product boundary.

The first public users are Pack Authors and Application Integrators. They author one code-first TypeScript project, build a deterministic WASI-free Activity Pack Bundle, and let a Host Operator approve and install its exact digest. WorldStream Negotiate is the first serious public Pack. Counter remains an internal tutorial/conformance fixture, Agent Heist is a visual demo/conformance Pack, and Investigation Room is deferred.

## How it works

~~~mermaid
flowchart LR
    H["Human client"] --> G["HTTP and WebSocket gateway"]
    R["External agent runner"] --> G
    G --> W["WorldStream room runtime"]
    W --> P["One approved Activity Pack revision"]
    W --> D["Bundled SQLite or PostgreSQL 17 primary"]
    P --> O["Authorized Membership projections"]
    O --> G
    W --> A["Durable activation intents"]
    A --> R
~~~

One accepted action follows this path:

    typed action
      → validate against current Authoritative Room State
      → commit one ordered transition
      → update Authoritative Room State
      → persist authorized observation frames and activation intents
      → acknowledge and stream the committed result

This is why the name includes Stream. WorldStream does not primarily stream LLM tokens. It streams meaningful, ordered changes in a shared situation and lets a disconnected participant catch up from a durable cursor.

## Quick start

WorldStream is a runtime workspace rather than a single web application. The
fastest way to build the real daemon and run it locally with bundled SQLite is:

```sh
cargo build --locked -p worldstream-server --bins
umask 077
mkdir -p .worldstream/data
if [ ! -e .worldstream/authority.secret ]; then
  head -c 32 /dev/urandom > .worldstream/authority.secret
fi
chmod 600 .worldstream/authority.secret
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamd --config config/development.toml
```

In another terminal, verify the running process:

```sh
curl -fsS http://127.0.0.1:9410/healthz
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
```

Use `Ctrl-C` to stop the daemon. The local database and bootstrap secret stay
under the ignored `.worldstream/` directory so the same authority can be used
on the next start.

For the first-party browser clients, install the locked JavaScript dependencies
and start the exact-directory Client Host:

```sh
pnpm install --frozen-lockfile
pnpm ui:dev
```

See [Getting started](docs/getting-started.md) for exact tool versions,
configuration, component tests, full verification, SDK usage, and the limits
of the fixture UI versus a live Room session.

The frozen releases run exactly one WorldStream process and support exactly two startup-selected durable profiles under the same Room semantics: release-bundled SQLite by default, or one hosted/self-managed PostgreSQL 17 primary. Linux x86-64 and Windows x64 are native release profiles, Linux/amd64 is the only OCI profile, and macOS is a source-build quickstart only. See [ADR 0004](docs/adr/0004-supported-storage-profiles-and-offline-portability.md) and [ADR 0011](docs/adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

## The frozen boundary

WorldStream owns:

- rooms, Membership lifecycle, Access Mode, current Role assignment, and scoped authorization;
- one total order of accepted transitions per room;
- idempotent action submission and commit-before-acknowledgement;
- three-hash canonical lineage, paired disposable snapshots, verified state reconstruction, recovery, and Replay;
- generation-fenced operational Room Integrity State and verifier-only repair without history rewrite;
- Membership-addressed observations and exact participant Action Offers;
- durable activation intents for external agent runners;
- bounded WebSocket delivery and cursor-based reconnect;
- independently executing first-party Activity Clients plus a Pack-neutral
  Inspector; Studio is the transitional Host Operator interface while the
  [CLI-first replacement](docs/adr/0018-cli-first-operator-surface.md) is built
  and verified.

Activity Packs own:

- domain state and configuration;
- Role definitions and constraints, typed actions, validation, and deterministic reduction;
- public and private projection rules;
- timers, activation reasons, and terminal outcomes.

The semantic seam remains `ActivityPackV1`: `descriptor`, `initialize`, `reduce`, `view`, and `observe`. Each Room pins a build-computed `PackRevisionLockV1`; retained exact executors/codecs remain runnable, and Rooms never upgrade in place. Public portable revisions use the same seam as a WASI-free WebAssembly Component inside an operator-approved `.wspack`; there is no second rules contract.

Agent runners own:

- models, prompts, private memory, tools, credentials, and execution;
- turning an activation intent into a bounded agent invocation;
- submitting typed actions back to the room.

## Starter activities

1. WorldStream Negotiate is the first serious public Pack: two commercial agents, one Human approver, and one external venue signer perform a pinned single-session A202 formation under exact approval, signature, deadline, evidence, Recovery, and Replay rules.
2. Agent Heist remains a bundled visual demo and conformance workload for privacy, timers, attention, recovery, and replay.
3. Counter remains an internal walking skeleton and tutorial, not the product story. Investigation Room is retained as deferred design research, not a committed release.

There is exactly one Activity Pack per Room in every starter activity.

## What this project is not

WorldStream is not n8n, Temporal, a general project manager, a message broker, a context database, a model host, a coding harness, or an agent marketplace. The first public release excludes workflow canvases, connector catalogs, cross-room projects, payments, crypto, cloud agent execution, vector memory, arbitrary pack effects, generated UI, a network Pack marketplace, hot loading, clustering, multiple live WorldStream processes, live/dual-write/reverse storage transfer, provider HA services, cloud resources, and multi-region operation.

## Documentation

- [Getting started](docs/getting-started.md) — build, run, inspect, test, and troubleshoot a local checkout
- [WorldStream domain context](CONTEXT.md) — canonical whole-product language and concept boundaries
- [Frozen requirements](docs/requirements.md) — normative release scope and change control
- [Canonical decision index](docs/decision-index.md) — frozen invariant, normative source, ADR, conformance evidence, and implementation ownership map
- [Compatibility manifest](compatibility.toml) and [canonical JSON mirror](compatibility.json) — fail-closed specification with exact portable-Pack identities; final distribution evidence remains detached and signed
- [Extended terminology](docs/glossary.md) — protocol, runtime, storage, UI, and lifecycle reference
- [Product vision](docs/vision.md) — audience, value, and boundaries
- [System architecture](docs/architecture.md) — stack, storage, filesystem, failure semantics, and scaling
- [Operator storage and transfer](docs/operator-storage.md) — packaged SQLite backup/restore and resumable SQLite-to-PostgreSQL transfer
- [Wire protocol](docs/protocol.md) — sessions, actions, observations, cursors, and activations
- [Observation and Activation](docs/observation-and-activation.md) — frozen attach/reset, delivery, intent, lease, and Invocation Context contracts
- [Activity Packs](docs/activity-packs.md) — host, bundle, authoring, installation, and retained-execution contract
- [Activity Clients](docs/activity-clients.md) — independent releases, Host-local deployments and bindings, opaque launch, packaging, and conformance
- [WorldStream Negotiate](docs/negotiate.md) — exact first public Pack and pinned A202 compatibility profile
- [Context model](docs/context-and-memory.md) — Authoritative Room State versus Invocation Context
- [UI architecture](docs/ui-architecture.md) — deliberately small first-party presentation layer
- [Security model](docs/security.md) — trust boundary and required tests
- [Automated compatibility gates](docs/gates.md) — local, native matrix, release, and credential-free provider-smoke tiers
- [Starter Distribution](docs/starter-distribution.md) — deterministic offline carrier for the exact Runtime, UIs, Pack authoring kit, Negotiate material, adapters, fixtures, docs, and legal subjects
- [Outside-adopter trial kit](docs/outside-adopter-trials.md) — redacted receipt contract for the six genuine 0.1.0 adoption trials
- [Delivery roadmap](docs/roadmap.md) — portable Pack platform followed by the Negotiate product proof
- [Architecture decisions](docs/adr/) — accepted product, Room sequencing, Core/integrity lineage, observation, Activation, retained pack, storage, portability, recovery, and release decisions
- [Idea archive](docs/ideas-and-research.md) — non-normative research only

## Implementation workspace

The implemented frontiers currently provide a pinned, deliberately narrow workspace:

- Rust `1.97.1` (edition 2024), Node `24.18.1` with pnpm `11.19.0`, Python `3.14.7`, and uv `0.12.5`;
- `worldstreamd` and `worldstreamctl` operator-shell binaries, protocol/runtime support crates, and a deterministic `xtask` manifest verifier;
- a pure canonical Core reducer, lineage tracer, strict Replay verifier, and storage-neutral operational Principal/capability/Membership authority module;
- the retained exact-revision `ActivityPackV1` host and registry, WASI-free WebAssembly Component Host, immutable `.wspack` verifier/store, exact-digest operator lifecycle, and executable Counter/Heist compatibility revisions;
- the public TypeScript Pack SDK/CLI with mandatory first-release prompt assistance and the exact proven WorldStream Negotiate Component Bundle;
- an independent Negotiate oracle, pinned A202 operated-profile adapter, and offline dual-evidence verifier;
- bundled-SQLite and PostgreSQL adapters for authenticated Room creation, atomic commit/recovery, durable authority changes, scoped observations, and authorized historical Replay;
- a locked public Python application SDK, generic TypeScript client seam,
  standalone Agent Heist and Negotiate browser clients, and Pack-neutral
  Inspector;
- native Linux and Windows bootstrap checks plus opt-in repository hooks.

Install the exact tools named above. The POSIX verifier also requires Python 3.11+ as `python3`
and `curl`; the native Windows verifier requires PowerShell 7.4+. Then run:

```sh
scripts/verify-local.sh
scripts/install-hooks.sh
```

The PowerShell 7.4+ equivalents are `scripts/verify-local.ps1` and `scripts/install-hooks.ps1`.
`cargo run --locked -p xtask -- compat verify` independently proves that the authored
[`compatibility.toml`](compatibility.toml) and its deterministic JSON mirror agree.

On macOS 15+ with APFS and the pinned toolchain active, the source-only
quickstart builds and tests the workspace, then drives the complete six-phase
Heist reference-client story through the source-built `worldstreamd` and
production UI in an exact Chrome-for-Testing browser. It verifies stale-Head
rejection/resync, typed actions, privacy, replay, and the final DOM reveal in
under ten minutes. The bootstrap workflow safely extracts and supplies the
architecture-specific pinned browser; use the command help to see the same
explicit local identity inputs:

```sh
scripts/macos-source-quickstart.sh --help
```

It creates no macOS release archive and makes no signing or notarization
claim. Native Linux/Windows archive and Linux/amd64 OCI packaging and
verification are documented in [Release packaging](docs/release-packaging.md).

The operator shell binds to loopback by default. `GET /healthz` reports process liveness,
`GET /readyz` returns `200` only when the startup-selected Room store is verified and ready and
otherwise fails closed with a stable `503` code, and `GET /version` reports the embedded
manifest/build and exact selected-engine identity. The bootstrap does not by itself claim that a
signed distribution exists.

## Status

The repository contains the pinned workspace and operator shell, canonical Core/lineage and operational-authority implementation, exact retained Counter and Agent Heist executors, the bundled-SQLite gateway, PostgreSQL storage/runtime adapters, a public Python application SDK, and first-party UI foundations. The checked-in compatibility pair is intentionally `manifest_kind = "specification"` and `release_ready = false`. The exact official Negotiate Component, semantic revision, schema/codec/golden identities, and digest-named bundle now exist and pass the production Bundle → Component Host → Core proof. The expanded release tooling closes four Runtime payloads, twelve portable Starter subjects, and seventeen pre-sign evidence reports into one 33-subject inventory, but genuine Negotiate policy/A202 and released-artifact SQLite/PostgreSQL restart reports, final artifacts, signatures, and outside-adopter qualification remain unavailable. Those gaps fail closed; a separately signed post-release qualification manifest prevents Starter/adopter evidence from creating a recursive primary signature.
