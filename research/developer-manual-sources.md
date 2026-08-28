# WorldStream developer manual source brief

> **RESEARCH SNAPSHOT, NOT A NORMATIVE SPECIFICATION.** Last reviewed: **2026-08-27**. This note is the source brief for a local-first developer manual. Repository behavior remains defined by the root [domain context](../CONTEXT.md), accepted [ADRs](../docs/adr/), and executable source. Provider and hosting details are time-sensitive and must be rechecked against the linked first-party documentation before a public v0.1 release.

## Executive findings

The manual can accurately present WorldStream as a self-hosted, deterministic Room runtime in which humans and independently hosted agents share typed, scoped, durable state. WorldStream owns authority, ordering, persistence, observation delivery, activation, and replay; Activity Packs own domain rules; external runners own models, prompts, private memory, credentials, and tools. This boundary is stated in the [README](../README.md#the-frozen-boundary), [architecture summary](../docs/architecture.md#architecture-summary), and [ADR 0001](../docs/adr/0001-product-boundary.md).

The best local developer journey is already present in source, but it is distributed across many documents and implementation files. A useful manual should organize it into five paths:

1. install and run `worldstreamd` with bundled SQLite;
2. start and operate WorldStream Studio;
3. run the Agent Heist examples and distinguish offline demonstrations from live acceptance evidence;
4. connect an external agent through the assignment-bound local MCP helper;
5. author, retain, test, and register a trusted compiled-in Activity Pack.

The current source does **not** support an honest “connect any hosted model by entering its public API URL” tutorial. The managed reference host accepts only an `open_ai_compatible` contract at a loopback socket and sends plain HTTP to a fixed `/v1/chat/completions` path; public OpenAI, xAI, and DeepSeek endpoints therefore require a trusted local proxy, and Anthropic's native API requires a translating adapter. The foundational path is local assignment-bound MCP, not the managed reference host ([managed host isolation boundary](../docs/managed-agent-host.md#isolation-boundary), [`ManagedReferenceProviderV1`](../crates/worldstream-studio-supervisor/src/agent_profiles.rs#L81), [`request_model`](../crates/worldstream-studio-supervisor/src/managed_agent_host_main.rs#L155)).

The repository has two confirmed documentation drifts that should be corrected or prominently worked around before publishing the manual:

- the Supervisor CLI defaults `studio_origin` to port `5173` and `participant_console_origin` to `5174`, while the actual applications and getting-started guide use Studio on `5174` and Participant Console on `5173`; use explicit origin flags in every run command ([Supervisor defaults](../crates/worldstream-studio-supervisor/src/main.rs#L90), [local port table](../docs/getting-started.md#run-worldstream-studio));
- the live Heist acceptance document still selects Python `3.13.0`, while `.python-version`, the compatibility contract, workflows, and getting-started guide select `3.14.7` ([MVP gate command](../docs/mvp-agent-heist-acceptance.md#running-the-gate), [prerequisites](../docs/getting-started.md#prerequisites), [compatibility contract](../compatibility.toml)).

Finally, repository inspection found only a GitHub `origin` and no checked-in `.gitlab-ci.yml`. GitLab Pages deployment cannot be claimed complete until a GitLab project/remote and its Pages runner context exist. GitLab's official documentation says Pages publishes static sites through GitLab CI/CD, so the manual site can be prepared in this repository but deployment needs that missing project context ([GitLab Pages](https://docs.gitlab.com/user/project/pages/)).

## Truth hierarchy for the manual

When sources disagree, use this order:

1. accepted ADRs for frozen boundaries and invariants;
2. `CONTEXT.md`, frozen requirements, and the decision index for canonical terminology;
3. executable source, compatibility manifests, and lockfiles for implemented commands and versions;
4. operator and developer guides for workflows;
5. examples for demonstrations, with their evidence level labeled explicitly.

The manual should link prominently to the [domain context](../CONTEXT.md), [requirements](../docs/requirements.md), [decision index](../docs/decision-index.md), and [glossary](../docs/glossary.md). Avoid silently turning a design-only section into an implemented capability.

## Product and architecture facts

### What WorldStream is

WorldStream is one self-hosted Rust process with one startup-selected durable storage profile. A Room has one logical writer and one ordered Transition history. Human clients and external agent runners communicate through HTTP/WebSocket boundaries; a trusted Activity Pack deterministically derives domain state, authorized projections, action offers, observations, timers, attention, and outcomes. SQLite is the bundled default and PostgreSQL 17 is the alternative primary profile ([architecture summary](../docs/architecture.md#architecture-summary), [frozen technology stack](../docs/architecture.md#frozen-technology-stack), [ADR 0004](../docs/adr/0004-supported-storage-profiles-and-offline-portability.md)).

The canonical unit of change is a typed Stimulus reduced to a Transition. Operational facts that do not affect domain truth stay outside the Room history ([ADR 0002](../docs/adr/0002-sequence-domain-relevant-room-changes.md)). Accepted writes are prepared and committed atomically before their outcome is published ([ADR 0006](../docs/adr/0006-backend-neutral-atomic-room-commit.md)). Core state, Activity state, aggregate authoritative state, and lineage are hash-verified for recovery and Replay ([ADR 0005](../docs/adr/0005-canonical-core-state-integrity-and-hash-lineage.md)).

Membership-addressed Observation Streams provide scoped, durable catch-up and explicit reset behavior ([ADR 0008](../docs/adr/0008-membership-observation-streams-and-reset-barriers.md), [observation contract](../docs/observation-and-activation.md#observation-contract)). Activation Intents durably request work from independently hosted runners, with exact context and lease fencing; Runner activation authority remains separate from participant Action authority ([ADR 0003](../docs/adr/0003-separate-activation-and-action-authority.md), [ADR 0009](../docs/adr/0009-activation-intents-context-and-lease-fencing.md)).

### What WorldStream is not

The manual should repeat the negative boundary because it prevents integration mistakes: WorldStream is not a model host, workflow engine, agent memory database, broker, marketplace, connector catalog, or general plugin runtime. It does not own prompts, provider selection, private agent memory, or arbitrary external effects ([README: What this project is not](../README.md#what-this-project-is-not), [ADR 0001](../docs/adr/0001-product-boundary.md)).

Studio does not become a second Room authority. It is a local operator companion that requests bounded operations through the daemon's typed APIs; `worldstreamd` remains the only authoritative runtime ([ADR 0013](../docs/adr/0013-studio-companion-control-plane.md), [Studio introduction](../docs/studio.md)).

### Implemented workspace map

The Rust workspace and versions are declared in [`Cargo.toml`](../Cargo.toml#L1). The manual's architecture page should describe these implementation units:

| Package | Developer-facing responsibility | Primary source |
| --- | --- | --- |
| `worldstream-core` | Canonical reducers, Activity Pack seam, Room execution contracts, replay-neutral domain types | [`crates/worldstream-core`](../crates/worldstream-core/) |
| `worldstream-protocol` | Versioned IDs, envelopes, errors, schemas, and wire contracts | [`crates/worldstream-protocol`](../crates/worldstream-protocol/) and [protocol guide](../docs/protocol.md) |
| `worldstream-runtime` | Configuration loading, filesystem policy, compatibility identity | [`crates/worldstream-runtime`](../crates/worldstream-runtime/) |
| `worldstream-server` | `worldstreamd`, `worldstreamctl`, HTTP/WebSocket gateway, operator APIs | [`crates/worldstream-server`](../crates/worldstream-server/) |
| `worldstream-sqlite` / `worldstream-postgres` | The two storage implementations with the same logical Room Commit semantics | [ADR 0004](../docs/adr/0004-supported-storage-profiles-and-offline-portability.md) |
| `worldstream-conformance` | Black-box and cross-adapter conformance support | [`crates/worldstream-conformance`](../crates/worldstream-conformance/) |
| `worldstream-backup` / `worldstream-transfer` | Read-only verification and offline portability workflows | [operator storage](../docs/operator-storage.md) |
| `worldstream-studio-supervisor` | Local typed Studio control plane, handoffs, assignment MCP, runner/managed-host operations | [`crates/worldstream-studio-supervisor`](../crates/worldstream-studio-supervisor/) |
| `xtask` | Deterministic compatibility and gate maintenance commands | [`xtask/src/main.rs`](../xtask/src/main.rs) |

The web workspace contains a separate Studio operator portal and Participant Console. The Python SDK is an external-participant client, not an embedded policy runtime ([getting-started entry points](../docs/getting-started.md), [Python SDK](../sdk/python/README.md)).

## Local-first installation and operation

### Pinned prerequisites

The current local toolchain is Rust `1.97.1`/edition 2024, Node.js `24.18.1`, pnpm `11.19.0`, Python `3.14.7`, and uv `0.12.5. Docker and PostgreSQL clients are optional unless a PostgreSQL, OCI, or release-specific workflow needs them. The canonical source for the install commands is [Getting started: Prerequisites](../docs/getting-started.md#prerequisites); Rust and workspace metadata are in [`Cargo.toml`](../Cargo.toml#L19).

### Minimal daemon path

The manual can safely prescribe this sequence, copied from the repository's checked-in local guide:

```sh
uv sync --project sdk/python --locked --python 3.14.7
pnpm install --frozen-lockfile
cargo build --locked -p worldstream-server --bins

umask 077
mkdir -p .worldstream/data
head -c 32 /dev/urandom > .worldstream/authority.secret
chmod 600 .worldstream/authority.secret

target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamctl --config config/development.toml config effective
target/debug/worldstreamctl --config config/development.toml doctor
RUST_LOG=info target/debug/worldstreamd --config config/development.toml
```

The authority secret is exactly 32 raw bytes with no textual encoding or trailing newline and must stay paired with the database. Effective configuration redacts secret paths and values. These are implementation requirements, not generic deployment advice ([Run the daemon with SQLite](../docs/getting-started.md#run-the-daemon-with-sqlite), [`config/development.toml`](../config/development.toml)).

The live verification sequence is:

```sh
curl -fsS http://127.0.0.1:9410/healthz
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
target/debug/worldstreamctl --config config/development.toml health
```

`/healthz` is process liveness, `/readyz` is verified runtime readiness, and `/version` identifies the embedded compatibility contract and selected storage engine ([getting-started endpoint meanings](../docs/getting-started.md#run-the-daemon-with-sqlite), [`operator_router`](../crates/worldstream-server/src/lib.rs#L1764)).

Configuration precedence is defaults, TOML, environment, then CLI. Environment variables use the `WORLDSTREAM__` prefix, while `WORLDSTREAM_CONFIG` selects a file. Secret authority and PostgreSQL credentials should be supplied by an owner-only file or inherited handle, not command-line arguments ([configuration source](../crates/worldstream-runtime/src/config.rs#L21), [getting-started configuration guidance](../docs/getting-started.md#run-the-daemon-with-sqlite)).

### Local ports and process ownership

| Component | Default local address | Owner and purpose |
| --- | --- | --- |
| `worldstreamd` | `127.0.0.1:9410` | Authoritative Room runtime and operator API |
| Studio Supervisor | `127.0.0.1:9420` | Local typed control plane |
| Participant Console | `127.0.0.1:5173` | Human participant UI; fixture mode by default |
| Studio portal | `127.0.0.1:5174` | Operator UI |

These addresses are documented in [Getting started: Run WorldStream Studio](../docs/getting-started.md#run-worldstream-studio). Development servers and the Supervisor should remain on exact loopback origins.

### Studio setup and healthy path

Studio requires the daemon configuration, SQLite state directory, and bootstrap secret described above. A fresh checkout should build the daemon and **all** Supervisor binaries because external-agent workflows need `worldstream-assignment-mcp`, and managed-reference workflows additionally need `worldstream-managed-agent-host` ([Studio setup](../docs/getting-started.md#studio-setup)).

```sh
cargo build --locked -p worldstream-server --bin worldstreamd
cargo build --locked -p worldstream-studio-supervisor --bins
pnpm ui:dev
```

In a second terminal, use explicit origins to avoid the current inverted CLI defaults:

```sh
scripts/studio-dev.sh \
  --studio-origin http://127.0.0.1:5174 \
  --participant-console-origin http://127.0.0.1:5173
```

Open Studio at `http://127.0.0.1:5174`, use Operations to start the configured daemon if needed, wait for health then readiness, and proceed to Build/Tasks. The wrapper builds the daemon and starts the Supervisor plus Studio Vite server; it does not delete `.worldstream/` state on exit ([Start the operator portal](../docs/getting-started.md#start-the-operator-portal), [`scripts/studio-dev.sh`](../scripts/studio-dev.sh)).

The manual should explain that `.worldstream/studio/` contains owner-only drafts, setup progress, opaque credential references, runner state, attention history, and backup-operation records. It must not be committed or copied casually ([Studio setup](../docs/getting-started.md#studio-setup), [protected credential references](../docs/studio.md#protected-credential-references)).

### Studio capability map

The current portal exposes Home, Tasks, Build, and Operations navigation and composes these functional surfaces in [`App.tsx`](../web/studio/src/App.tsx#L179):

- daemon status and fixed lifecycle start/stop/restart;
- attention inbox and notification preference;
- Activity Pack catalog and exact revision metadata;
- Room inventory and Room detail;
- Agent Profile build/edit flows;
- Room draft wizard and durable Room creation;
- Task Templates, Task setup/retry, and explicit launch;
- one-use Participant Console handoff;
- verified live backup status/operation;
- installed Runner Templates, instances, and runner attention;
- post-MVP managed Agent Host lifecycle/status.

The Supervisor routes are separately implemented by `lifecycle.rs`, `activity_packs.rs`, `rooms.rs`, `agent_profiles.rs`, `room_drafts.rs`, `room_creation.rs`, `task_templates.rs`, `task_setup.rs`, `participant_handoff.rs`, `backups.rs`, `runner_templates.rs`, `runner_attention.rs`, `attention_inbox.rs`, `assignment_mcp.rs`, and `managed_agent_host.rs` under [`crates/worldstream-studio-supervisor/src`](../crates/worldstream-studio-supervisor/src/). The manual should describe capabilities rather than expose those internal routes as a stable public API unless the protocol is explicitly frozen.

The Participant Console is distinct. It starts in fixture mode and does not discover the daemon or create a Room. A live session requires an application-supplied Room ID, Membership ID, scoped bearer, and explicit `window.__WORLDSTREAM_LIVE_SESSION__` bootstrap ([Run the web console](../docs/getting-started.md#run-the-web-console)).

Participant handoffs are one-use and short-lived. The user-facing URL carries a fragment bootstrap, and the console scrubs it; it does not expose Room, Membership, or bearer material to the operator ([MVP gate boundaries](../docs/mvp-agent-heist-acceptance.md#boundaries-exercised), [Participant handoff source](../crates/worldstream-studio-supervisor/src/participant_handoff.rs)).

## CLI, API, protocol, and SDK inventory

### Operator CLI

`worldstreamctl` implements:

- `config validate` and `config effective`;
- `doctor`, `health`, and `version`;
- PostgreSQL `migrate`, `verify`, transfer `begin|resume|finalize|abort`, snapshot rebuild, native directory identity, restore, and recover;
- SQLite `backup`, `restore`, and `verify`.

The command enum is the concrete source of truth in [`worldstreamctl.rs`](../crates/worldstream-server/src/bin/worldstreamctl.rs#L55). Operational safety and full workflows are documented in [Offline storage commands](../docs/operator-storage.md), not merely in `--help` output.

The maintenance tool provides compatibility manifest generation/verification and gate execution through [`xtask`](../xtask/src/main.rs). The manual should point ordinary contributors to `cargo run --locked -p xtask -- compat verify` and the gate wrappers rather than invite manual edits to generated compatibility data ([Automated compatibility gates](../docs/gates.md)).

### Daemon HTTP and WebSocket surface

The implemented router includes liveness, readiness, version, Prometheus metrics, Activity Pack catalog/revision, Room creation, member and Runner capability provisioning, operator Room inventory/detail, activation and runner presence, live backup, timer firing, lobby launch, current projection, historical Replay, browser ticket, Room WebSocket stream, and Runner WebSocket stream ([`operator_router`](../crates/worldstream-server/src/lib.rs#L1764)).

The stable message model belongs in [Wire Protocol and Data Model](../docs/protocol.md), especially:

- [transport boundaries](../docs/protocol.md#transport);
- [handshake and room attachment](../docs/protocol.md#handshake);
- [action submission](../docs/protocol.md#action-submission);
- [observation frames](../docs/protocol.md#observation-frames);
- [activation protocol](../docs/protocol.md#activation-protocol);
- [HTTP resource APIs](../docs/protocol.md#http-resource-apis);
- [error envelope](../docs/protocol.md#error-envelope);
- [canonical hashing](../docs/protocol.md#canonical-hashing).

An integration guide should not encourage clients to manufacture offers or guess payloads. Clients read the current authorized Projection, select an exact Action Offer, preserve its current precondition, and submit a stable operation identity. On an ambiguous response, retry the identical identity and body; on a stale head, resync and use a new Action ID. This is stated by the [Python SDK](../sdk/python/README.md) and enforced by the assignment MCP schema ([tool definitions](../crates/worldstream-studio-supervisor/src/assignment_mcp.rs#L2890)).

### Assignment-bound MCP

`worldstream-assignment-mcp` is a local, line-delimited JSON-RPC MCP server over stdio. It accepts one opaque Supervisor-issued `--launch-reference` and an owner-only `--state-dir` (default `.worldstream/studio`) ([CLI definition](../crates/worldstream-studio-supervisor/src/assignment_mcp.rs#L66), [stdio server](../crates/worldstream-studio-supervisor/src/assignment_mcp.rs#L2707)).

Its exact tool surface is:

| Tool | Purpose |
| --- | --- |
| `worldstream.list_assigned_tasks` | List only Tasks sealed into the assignment context |
| `worldstream.observe` | Resume the assigned Membership Observation Stream from its durable Cursor |
| `worldstream.acknowledge` | Durably acknowledge processed Observation Frames |
| `worldstream.list_current_action_offers` | Return current offered actions and their pinned payload schemas |
| `worldstream.submit_action` | Submit one exact offered Action with stable operation identity and head precondition |
| `worldstream.next_activation` | Acquire or resume the next Activation in the sealed Runner scope |
| `worldstream.complete_activation` | Complete the exact lease as `handled`, `declined`, or `failed` |

The names, descriptions, and input schemas are executable in [`tool_definitions`](../crates/worldstream-studio-supervisor/src/assignment_mcp.rs#L2890). A manual tutorial should show the safe loop: acquire/resume activation, observe, list offers, choose an exact offer, submit it, durably retain the receipt, acknowledge processed frames, then complete the exact activation. The managed reference host follows that order ([managed host lifecycle and recovery](../docs/managed-agent-host.md#lifecycle-and-recovery)).

Launch references and state directories are authority-bearing operational material. Examples must use placeholders, must not commit a real reference, and should tell developers to obtain the active reference from the Studio/Supervisor assignment flow. The helper seals participant and Runner bearers away from the model-facing tool input ([managed host isolation boundary](../docs/managed-agent-host.md#isolation-boundary), [security: agent runner and activation](../docs/security.md#agent-runner-and-activation-security)).

## Activity Pack authoring

### Fit test

An Activity Pack is appropriate when multiple participants share one evolving situation, require typed current-state actions, need viewer-specific visibility, benefit from deterministic timers/outcomes, and must recover/replay without invoking a model. A workflow dominated by arbitrary external side effects, unbounded computation, private model memory, or cross-Room orchestration is a poor fit ([When an activity fits](../docs/activity-packs.md#when-an-activity-fits)).

### Frozen seam

`ActivityPackV1` has exactly five synchronous operations:

1. `descriptor` — immutable revision metadata;
2. `initialize` — initial canonical Activity State and timer requests;
3. `reduce` — one normalized Stimulus to Apply/Reject/fault;
4. `view` — authorized Activity Projection plus the sole Action Offer list;
5. `observe` — zero or one bounded viewer observation.

The exact Rust trait is in [`activity_pack.rs`](../crates/worldstream-core/src/activity_pack.rs#L67) and the complete semantics are in [ActivityPackV1](../docs/activity-packs.md#activitypackv1). Calls are pure, synchronous, and bounded. A pack receives deterministic recorded context, not clock, filesystem, storage, network, scheduler, model, Session, delivery, or telemetry capability ([DeterministicContextV1](../docs/activity-packs.md#deterministiccontextv1)).

### Package and retention contract

A current pack is trusted Rust compiled into the binary. The package includes typed configuration/state/action/projection/observation/outcome definitions, deterministic reducer/view logic, schemas, limits, golden corpus, and retained codecs/executors. It must not bundle model-provider SDKs or perform side effects ([Package contents](../docs/activity-packs.md#package-contents-in-the-frozen-releases)).

Each Room pins an exact `PackRevisionLock`; the registry selects by exact digest and must retain old executors/codecs for live Rooms and Replay. Rooms do not upgrade in place. Dynamic public plugins and a stable external ABI are explicitly deferred ([PackRevisionLock and embedded registry](../docs/activity-packs.md#packrevisionlock-and-embedded-registry), [Public plugins](../docs/activity-packs.md#public-plugins), [ADR 0010](../docs/adr/0010-activity-pack-v1-and-executable-replay-retention.md)). The manual must not market Activity Packs as downloadable plugins.

### Recommended step-by-step authoring chapter

The website can turn the existing contract into this practical checklist, with every step linked to source:

1. **Decide suitability.** Apply the fit/poor-fit test above.
2. **Freeze vocabulary and schemas.** Define configuration, Activity State, Actions, projections, observations, timers, attention, and outcome as strict versioned types ([package contents](../docs/activity-packs.md#package-contents-in-the-frozen-releases)).
3. **Define a descriptor and exact revision identity.** Set stable pack ID, explanatory version, schema identities, limits, and build-computed digest ([Descriptor and revision identity](../docs/activity-packs.md#descriptor-and-revision-identity)).
4. **Implement initialization.** Produce canonical initial state and ordered generation-free timer requests only ([Initialization](../docs/activity-packs.md#initialization)).
5. **Implement one reducer.** Handle the normalized Stimulus variants against immutable Core-before/proposed-after views, returning Apply, declared Reject where allowed, or PackFault ([Reduction input](../docs/activity-packs.md#reduction-input-and-normalized-stimulus), [Reduction dispositions](../docs/activity-packs.md#reduction-dispositions-and-core-veto)).
6. **Derive offers and validate from the same predicates.** `view` emits exact currently executable Action Offers; `reduce` revalidates them against current state. Never accept a client-invented offer ([View, Action Offers, and observation](../docs/activity-packs.md#view-action-offers-and-observation)).
7. **Implement viewer-scoped output.** Keep hidden facts out of projections, observations, attention, errors, logs, and Replay surfaces ([security: projection and privacy isolation](../docs/security.md#projection-and-privacy-isolation)).
8. **Use host timers, not clocks.** Request timers by semantic identifiers and generations; consume only recorded timer stimuli ([Host-owned timer generations](../docs/activity-packs.md#host-owned-timer-generations), [ADR 0007](../docs/adr/0007-semantic-time-timer-ordering-and-catch-up.md)).
9. **Bound everything and fail closed.** Enforce document sizes, collection counts, schema depth, and deterministic faults ([Bounds, faults, and containment](../docs/activity-packs.md#bounds-faults-and-containment)).
10. **Register and retain exact revisions.** Add the compiled implementation and codecs to the embedded registry without replacing old semantic bytes ([PackRevisionLock and embedded registry](../docs/activity-packs.md#packrevisionlock-and-embedded-registry)).
11. **Create conformance evidence.** Add golden Genesis/Transition/replay vectors, visibility tests, stale/retry tests, timer races, and dependency audit evidence ([Activity conformance suite](../docs/activity-packs.md#activity-conformance-suite)).
12. **Run gates.** Start with focused tests and `scripts/gates.sh fast`, then the full workspace/pre-push gate appropriate to the change ([Automated compatibility gates](../docs/gates.md)).

### Examples the manual can honestly teach

**Agent Heist** is the production reference Activity Pack. It has three fixed roles (`navigator`, `insider`, `broker`), typed actions, a six-phase timer machine, sealed two-of-three selection, five-check outcome, viewer-scoped privacy, and deterministic fixtures. The full domain contract is [Reference Activity A: Agent Heist](../docs/activity-packs.md#reference-activity-a-agent-heist); executable source is [`agent_heist.rs`](../crates/worldstream-core/src/agent_heist.rs).

The offline absent-Broker story is a useful zero-infrastructure tutorial but is not live-service evidence ([example README](../examples/heist/README.md), [offline reference story](../docs/getting-started.md#run-an-offline-reference-story)). The live Agent Heist MVP gate uses real `worldstreamd`, Supervisor, assignment MCP, production Participant Console, and Studio DTOs, and verifies crash/restart lease recovery plus live-vs-Replay hashes ([MVP required story](../docs/mvp-agent-heist-acceptance.md#required-story), [boundaries exercised](../docs/mvp-agent-heist-acceptance.md#boundaries-exercised)).

**Counter** is a deliberately small, test-only conformance example. Its retained revisions increment by different amounts to prove exact revision selection/replay, and it has a private acknowledgement path ([`counter.rs`](../crates/worldstream-core/src/counter.rs)). The manual can use it to teach the seam, but must label it “test-only,” not a second production Activity Pack.

**Investigation Room** is fully specified in the design document but repository search found no executable `investigation` implementation under `crates/`, `examples/`, or `web/`. It belongs in an “upcoming / design reference” section, not the runnable tutorial ([Reference Activity B](../docs/activity-packs.md#reference-activity-b-investigation-room), [roadmap](../docs/roadmap.md)).

## External agent integration facts

The correct integration boundary is the WorldStream assignment MCP helper. Provider APIs are an optional model-execution choice behind a developer-owned runner or trusted loopback adapter. No current repository gate proves end-to-end behavior for the five products below, so the website should label the commands as integration recipes and add a dated acceptance result only after executing each one.

### Compatibility matrix

| Agent or provider | Best current local path | What first-party docs establish | WorldStream-specific caveat |
| --- | --- | --- | --- |
| OpenAI Codex / ChatGPT desktop | Configure the Supervisor-issued `worldstream-assignment-mcp` command as a local stdio MCP server | OpenAI says the ChatGPT desktop app, Codex CLI, and IDE extension support MCP on a Codex host, including stdio servers, and share host configuration ([OpenAI MCP documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)) | The launch reference is assignment-specific authority and must be injected locally, not checked in. This path still needs a real WorldStream acceptance run. |
| ChatGPT web | Expose an approved remote MCP boundary or use Secure MCP Tunnel in a later integration | OpenAI states ChatGPT web connects to remote MCP servers and cannot connect directly to a local MCP server; private/local servers require Secure MCP Tunnel. Full write-capable MCP is plan/admin dependent ([OpenAI Help: developer mode and MCP apps](https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt)) | The current WorldStream helper is local stdio only. Do not claim direct ChatGPT-web support today. |
| Anthropic Claude Code | Add the assignment helper as a local stdio MCP server with `claude mcp add ... -- <command> [args...]` | Anthropic documents local stdio MCP processes and the exact `claude mcp add [options] <name> -- <command> [args...]` shape ([Claude Code MCP](https://code.claude.com/docs/en/mcp)) | Use placeholders for the active launch reference and owner-only state path. No checked-in Claude acceptance fixture exists. |
| Anthropic Claude API | Developer-owned runner/adapter | Anthropic's Messages API uses its own API surface and authentication contract ([Claude API overview](https://platform.claude.com/docs/en/api/overview)) | It is not the current managed host's fixed OpenAI-compatible chat-completions contract, so a translator is required. |
| xAI Grok API | Trusted loopback OpenAI-compatible proxy feeding the managed reference host, or a custom external MCP agent | xAI documents a legacy-compatible `POST /v1/chat/completions` API in addition to its preferred Responses path ([xAI Chat Completions](https://docs.x.ai/developers/model-capabilities/legacy/chat-completions), [xAI text generation](https://docs.x.ai/developers/model-capabilities/text/generate-text)) | The WorldStream managed host accepts a loopback socket and emits plain HTTP, so it cannot point directly at xAI's public HTTPS endpoint. Verify JSON-object behavior and exact action payloads through the proxy. |
| DeepSeek API | Trusted loopback OpenAI-compatible proxy feeding the managed reference host, or a custom external MCP agent | DeepSeek documents `POST /chat/completions`, a `response_format` JSON-object mode, and a chat-completion response with `choices[].message.content` ([DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion/)) | The payload shape is close to the managed host's expectation, but the host hard-codes `/v1/chat/completions` at loopback; the proxy must normalize base path/TLS and preserve the exact bounded response. Model IDs are time-sensitive. |
| OpenClaw | Register the assignment helper as an outbound stdio MCP server | OpenClaw documents `openclaw mcp add` with `--command`, repeated `--arg`, `--env`, and `--cwd`, plus `doctor --probe`; its runtime can launch configured stdio servers ([OpenClaw MCP CLI](https://docs.openclaw.ai/cli/mcp)) | This is structurally compatible, but no WorldStream OpenClaw fixture or gate exists. Keep the assignment launch reference private and verify tool discovery before a real turn. |

### Provider API notes

OpenAI's official quickstart stores `OPENAI_API_KEY` in the environment and demonstrates the Responses API through the official SDK ([OpenAI developer quickstart](https://platform.openai.com/docs/quickstart/make-your-first-api-request)). That is appropriate for a developer-owned external runner, but it is not proof that the current managed reference host implements the Responses API.

DeepSeek and xAI model names change independently of WorldStream. The manual should avoid baking a “latest” model ID into conceptual pages. Put model IDs in clearly dated, tested recipes and link to each provider's official model documentation. The same rule applies to OpenClaw provider routes; its first-party provider directory confirms support for OpenAI, Anthropic, DeepSeek, and xAI, but that does not prove WorldStream interoperability ([OpenClaw provider directory](https://docs.openclaw.ai/providers)).

### Safe recipe shape

For Codex, Claude Code, or OpenClaw, the manual can show a placeholder command shape after Studio has created the assignment:

```text
target/debug/worldstream-assignment-mcp \
  --launch-reference <SUPERVISOR_ISSUED_REFERENCE> \
  --state-dir <OWNER_ONLY_STUDIO_STATE_DIR>
```

The provider-specific client should own that command as a local stdio MCP server. The developer should then verify that all seven `worldstream.*` tools are discoverable before asking the agent to act. Do not put a real launch reference in a shell transcript, screenshot, Git-tracked config, issue, or browser-visible page.

For managed-provider recipes, explain all current constraints together: `managed_reference` profile, exact Runner Template revision, `open_ai_compatible` provider, loopback provider address, exact model ID, and one `MODEL_PROVIDER_TOKEN` secret reference. The local adapter must accept plain HTTP `/v1/chat/completions`, Bearer auth, `response_format: {"type":"json_object"}`, and return the selected exact `offer_id` plus `payload` in `choices[0].message.content` within the host's byte/time limits ([managed host operational contract](../docs/managed-agent-host.md), [`request_model`](../crates/worldstream-studio-supervisor/src/managed_agent_host_main.rs#L155)).

## Verification and maintenance runbook

### Evidence ladder

The website should label every command by what it proves:

| Command or workflow | What it proves | What it does not prove |
| --- | --- | --- |
| offline Heist story | Deterministic reference fixture and Replay corpus | Live daemon, database, browser, network, or agent integration |
| `scripts/gates.sh fast` | Source/manifest drift, formatting, focused lint/tests, goldens, lock resolution, tracked secret patterns | Full workspace or release readiness |
| `scripts/smoke-operator.sh` | Real disposable SQLite daemon reaches health/readiness/version | Full Room/client story |
| component suites | Rust/Python/UI unit and integration behavior | Packaged deployment |
| `scripts/verify-local.sh` | Broad pinned local checkout verification | Signed release distribution |
| strict pre-push/minimal CI | Full repository/native matrix obligations declared by the manifest | Detached release evidence unless release tier runs |
| live Heist MVP gate | Complete production-boundary human + external-agent story and replay equality | Provider SLA, PostgreSQL failover, public network, or multi-browser behavior |
| release gate | Detached artifacts/evidence, checksums, SBOM, provenance, and signature identities | A universal performance or availability SLA |

The exact gate semantics and deadlines are in [Automated compatibility gates](../docs/gates.md); live Agent Heist scope and non-goals are in [MVP completion gate](../docs/mvp-agent-heist-acceptance.md). `release_ready = true` in the embedded compatibility manifest is only contract completeness, not proof that a signed distribution exists ([release packaging](../docs/release-packaging.md)).

### Storage operations

Local development should default to bundled SQLite. Back up and verify before destructive maintenance. SQLite backup/restore and one-way offline SQLite-to-PostgreSQL transfer are supported operator workflows; runtime switching, dual writes, reverse transfer, and automatic provider failover are not ([Offline storage commands](../docs/operator-storage.md), [ADR 0004](../docs/adr/0004-supported-storage-profiles-and-offline-portability.md)).

Studio's live backup is deliberately narrower than the full offline semantic restore verifier. For SQLite it produces and reopens a verified artifact and compares source/destination exports, but reports full semantic restore verification as unavailable; PostgreSQL live backup is provider-managed and unsupported by this local flow ([Verified live backups](../docs/studio.md#verified-live-backups)).

### Security checklist

The manual should make these non-optional:

- keep the daemon, Supervisor, provider adapters, and Vite development servers on loopback;
- never expose Host, Membership, Runner, launch, or model-provider credentials to browser input, logs, screenshots, model prompts, or committed config;
- use kind-bound secret references and owner-only state directories;
- treat all pack/user/agent text as hostile and keep it out of authority decisions;
- process an Observation durably before acknowledging its cursor;
- reuse an exact operation identity only with the identical Action/Activation body;
- keep Runner activation authority separate from participant Action authority;
- run tracked-file secret scanning and privacy/noninterference tests.

The authoritative rationale and required tests are in [Security and Trust Model](../docs/security.md), especially [authentication and capabilities](../docs/security.md#authentication-and-capabilities), [prompt injection and hostile text](../docs/security.md#prompt-injection-and-hostile-text), [Activity Pack security](../docs/security.md#activity-pack-security), and [required security tests](../docs/security.md#required-security-tests).

## GitLab Pages publishing facts

GitLab Pages hosts static sites generated by common frameworks or plain HTML and deploys them through GitLab CI/CD. A Pages job publishes a directory, and the project needs a working GitLab runner/Pages configuration ([GitLab Pages overview](https://docs.gitlab.com/user/project/pages/), [GitLab CI/CD YAML reference](https://docs.gitlab.com/ci/yaml/)).

For this repository, the manual implementation should therefore:

1. produce a deterministic static build in a documented output directory;
2. keep local `dev`, `build`, `test`, and `preview` commands in the workspace;
3. add a Pages CI job only after the actual GitLab project path, default branch, runner availability, and Pages URL are known;
4. verify internal links and asset base paths against the final GitLab Pages project/group URL;
5. record the deployed URL and pipeline proof in the repository/ticket only after the pipeline succeeds.

Current inspection cannot resolve the GitLab namespace, project ID, runner, protected-branch policy, credentials, or final Pages URL. A manual site can be source-controlled now, but “hosted on GitLab Pages” remains unproven until those external facts are provided and observed.

## Recommended website information architecture

The research supports this navigation without inventing capabilities:

1. **Start here** — product mental model, prerequisites, five-minute daemon run, Studio quickstart.
2. **Concepts** — Room, Membership, Stimulus, Transition, Projection, Action Offer, Observation Cursor, Activation, Replay, Integrity.
3. **Operate locally** — configuration, ports, daemon lifecycle, health/readiness/version, Studio, backup, reset, troubleshooting.
4. **Build clients** — HTTP/WebSocket protocol, Python SDK, participant handoff, idempotency/retry examples.
5. **Connect agents** — assignment MCP lifecycle, seven tools, Codex, Claude Code, OpenClaw recipes, provider adapter boundaries.
6. **Build Activity Packs** — fit test, five-operation seam, types/schemas, timers, visibility, retention, conformance.
7. **Examples** — Counter test pack, offline Agent Heist, live Agent Heist, upcoming Investigation Room.
8. **Maintain** — tests/gates, compatibility manifests, storage operations, security, release evidence.
9. **Reference** — CLI command inventory, ports, APIs, repo map, glossary, ADR/decision index.

Interactive elements should clarify the system rather than simulate unsupported behavior. High-value components are a role/authority explorer, an Action retry decision tree, an Observation/Activation timeline, a Studio port/process map, an Activity Pack five-operation explorer, copyable local command sequences, searchable glossary, and a provider compatibility matrix with “supported,” “adapter required,” and “not yet proven” states.

## Verified gaps and no-claim boundaries

These gaps should be visible in the manual and implementation tickets:

- **GitLab project context is missing.** Only a GitHub `origin` was visible on 2026-08-27, and no `.gitlab-ci.yml` was present before the documentation-site work. Deployment proof requires an actual GitLab project and successful pipeline.
- **Studio origin defaults are inverted.** Always pass the two origin flags explicitly until source defaults are corrected.
- **MVP Python instructions drift.** The live acceptance document says `3.13.0`; the current pinned quickstart is `3.14.7`.
- **No provider acceptance fixtures.** There is no checked-in end-to-end acceptance result for Codex/ChatGPT desktop, Claude Code, Grok, DeepSeek, or OpenClaw.
- **ChatGPT web is not a local-stdio client.** The current helper cannot connect directly without a remote MCP/tunnel boundary.
- **Managed provider support is intentionally narrow.** It is post-MVP, loopback-only, OpenAI-compatible chat completions; it is not a general provider SDK or arbitrary URL feature.
- **Counter is test-only.** It is safe as an authoring example but not a production activity claim.
- **Investigation Room is design-only in this checkout.** Do not advertise a runnable second production pack.
- **No dynamic Activity Pack plugin ABI.** Packs are trusted, compiled in, exact-revision retained code.
- **Fixture UI is not live authority.** Starting the Participant Console alone does not provision a Room or capability.
- **Performance results are reference measurements.** They are not universal SLAs ([README status](../README.md#status), [gates](../docs/gates.md)).

## Source index

### Repository primary sources

- [Domain context](../CONTEXT.md)
- [README](../README.md)
- [Getting started](../docs/getting-started.md)
- [System architecture](../docs/architecture.md)
- [Wire protocol](../docs/protocol.md)
- [Observation and Activation](../docs/observation-and-activation.md)
- [Activity Pack design](../docs/activity-packs.md)
- [WorldStream Studio](../docs/studio.md)
- [Managed reference Agent Host](../docs/managed-agent-host.md)
- [Security model](../docs/security.md)
- [Offline storage commands](../docs/operator-storage.md)
- [Automated gates](../docs/gates.md)
- [MVP Agent Heist completion gate](../docs/mvp-agent-heist-acceptance.md)
- [Release packaging](../docs/release-packaging.md)
- [Accepted ADRs](../docs/adr/)
- [`ActivityPackV1` implementation](../crates/worldstream-core/src/activity_pack.rs#L67)
- [Assignment MCP implementation](../crates/worldstream-studio-supervisor/src/assignment_mcp.rs)
- [Managed Agent Host implementation](../crates/worldstream-studio-supervisor/src/managed_agent_host_main.rs)
- [Daemon router](../crates/worldstream-server/src/lib.rs#L1764)
- [`worldstreamctl`](../crates/worldstream-server/src/bin/worldstreamctl.rs)

### External first-party sources

- [OpenAI MCP documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
- [OpenAI ChatGPT developer mode and MCP apps](https://help.openai.com/en/articles/12584461-developer-mode-and-full-mcp-connectors-in-chatgpt)
- [OpenAI API quickstart](https://platform.openai.com/docs/quickstart/make-your-first-api-request)
- [Anthropic Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [Anthropic API overview](https://platform.claude.com/docs/en/api/overview)
- [xAI Chat Completions](https://docs.x.ai/developers/model-capabilities/legacy/chat-completions)
- [xAI text generation](https://docs.x.ai/developers/model-capabilities/text/generate-text)
- [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion/)
- [OpenClaw MCP CLI](https://docs.openclaw.ai/cli/mcp)
- [OpenClaw provider directory](https://docs.openclaw.ai/providers)
- [GitLab Pages](https://docs.gitlab.com/user/project/pages/)
- [GitLab CI/CD YAML reference](https://docs.gitlab.com/ci/yaml/)
