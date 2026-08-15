# Frozen Delivery Roadmap

## Planning assumptions

- One primary developer working 8–12 focused hours per week.
- Design and implementation begin from the current documentation-only repository.
- The project favors correctness evidence and a memorable demo over feature count.
- Agent Heist v0.1 is the first usable milestone.
- Investigation Room v0.2 is the only committed application beyond Heist.
- Total realistic horizon is approximately 20–22 hobby weeks, not a weekend build.
- Deterministic local runners are mandatory; paid LLM APIs are optional demonstrations.
- Every newly discovered idea goes to the non-normative backlog unless a frozen acceptance gate cannot pass without it.

The normative scope is [Frozen Requirements](requirements.md).

## Release story

### v0.1: Agent Heist

Three independently run Agent Participants enter one authoritative Room with different private clues. They exchange structured information, propose a plan, and submit sealed decisions within one shared window. One Invocation ends before a critical phase. A later Transition creates an Activation Intent; an authorized Runner starts a fresh Invocation, catches up from the Membership Cursor, and acts with separate participant authority. The server is forcibly terminated after another Action commits, restarts without losing acknowledged state, and replays the same final Outcome.

### v0.2: Investigation Room

The unchanged Room Kernel, plus the preplanned generic v0.2 artifact subsystem, runs a serious evidence investigation with a human Lead and three external agents. Immutable evidence arrives in recorded waves. Participants publish source-linked facts, claims, and challenges. A timestamp correction supersedes earlier evidence, deterministically marks dependent claims stale, and activates affected agents. The human submits a structured brief that a deterministic rubric scores. Restart and replay reconstruct the same board, brief, and result.

If these two stories work without hiding domain special cases in core, the project has earned a broader Activity Pack conversation.

## Frozen release scope

### v0.1 required

- Rust/Tokio/Axum server.
- Trusted compiled-in Rust Activity Pack host interface.
- One Activity Pack: Agent Heist.
- One logical writer actor per active room.
- Rusqlite with bundled SQLite WAL and forward-only migrations.
- Atomic transition/action-receipt/timer/frame/activation commit.
- JSON HTTP/WebSocket protocol.
- Human and agent principals/memberships.
- Typed action validation and idempotency.
- Public and participant-specific projections.
- Durable observation frames, cursors, reconnect, and explicit projection reset.
- Durable timers.
- Durable activation intents with control WebSocket or HTTP long poll and claim leases.
- External ephemeral agent runners.
- Deterministic recovery and read-only replay.
- Async Python SDK.
- Three deterministic Heist runners.
- Small first-party Heist public/operator-membership UI.
- Docker image, native binary, quickstart, failure tests, and reproducible benchmark report.

### v0.2 required

- One additional Activity Pack: Investigation Room.
- One acting human Lead client path.
- Generic local content-addressed artifact storage.
- Exact evidence versions, source references, and visibility.
- Deterministic evidence correction and dependency invalidation.
- Investigation projections and first-party UI.
- Deterministic structured brief rubric.
- Generality/conformance report proving no domain-specific Room Kernel behavior.

### Frozen out through v0.2

- timeline branching or merge;
- Project/Workspace entities and cross-room exchange;
- workflow/node canvas and connector catalog;
- coding harness, shell/container execution, or model hosting;
- public plugin upload, WASM, or pack registry;
- dynamic or LLM-generated UI;
- A2A/MCP platform integrations;
- vector search and automatic summaries;
- marketplace, payments, crypto, wallets, or token;
- Postgres, NATS, Redis, Kafka, clustering, federation, or multi-region writes;
- production SaaS tenancy and billing.

## Phase A: Agent Heist v0.1

### Milestone A0 — Repository and contracts

Target: week 1, 8–12 hours

Build:

- create Cargo workspace and four initial crates;
- pin Rust toolchain and dependencies;
- add format, Clippy, unit-test, dependency-audit, and secret-scan CI;
- add embedded forward-only migration harness;
- define canonical JSON, ID, room sequence, frame cursor, hash, error, and message envelope types;
- define the provisional Activity Pack trait;
- implement worldstreamd health, readiness, version, and one WebSocket handshake;
- implement a tiny test-only Counter activity solely as a walking skeleton; it is not registered in the release binary;
- create Python SDK package skeleton and protocol golden fixtures.

Exit tests:

1. Two local clients authenticate, attach, submit typed Counter actions, and see one persisted order.
2. Restart reconstructs the Counter value.
3. Rust and Python agree on canonical payload hash fixtures.

Scope gate:

Do not add Heist content, UI, plugins, adapters, or multiple storage backends until this path works.

### Milestone A1 — Durable Room Kernel

Target: weeks 2–4, 26–36 hours

Build:

- room supervisor and lazy room loading;
- one bounded actor per active room;
- SQLite schema, foreign keys, WAL, FULL synchronous durability, and supported-version check;
- dedicated database writer thread;
- immutable canonical room genesis and pinned pack digest;
- Transition/Activity State hash chain;
- action receipts and same-ID/different-payload detection;
- atomic accepted-transition commit;
- stable receipts only for deterministic admitted domain rejections; transient errors do not consume action IDs;
- periodic snapshots and load from snapshot plus tail, with genesis fallback when all snapshots are absent;
- durable timers and idempotent TimerFired stimulus;
- conditional timer-generation update inside the transition transaction;
- supervisor Loading/CatchingUp/Active/Passivating lifecycle and generation-fenced Room passivation;
- generic HTTP mutation receipts for idempotent room/membership administration;
- failure-injection points before/after commit and before reply.

Exit tests:

1. Concurrent clients create one deterministic room order.
2. Retrying one action one hundred times creates one accepted transition.
3. A reused action ID with changed payload is rejected.
4. Termination after commit and before reply returns the original result on retry.
5. Removing every snapshot still reconstructs the same Core Room State and Activity State hash from immutable genesis and Transitions.
6. A timer due during shutdown fires once logically after restart.
7. A duplicated TimerFired candidate conditionally commits at most once.
8. A lost room-creation HTTP reply returns the original room on idempotent retry.

Gate A:

Stop feature work if durability, idempotency, or replay hashes remain flaky. They are the Room Kernel's foundation.

### Milestone A2 — Membership, scoped projections, and reconnect

Target: weeks 5–6, 18–26 hours

Build:

- development principal and scoped bearer-capability creation;
- separate human/agent Principal kind and pack-defined Role;
- membership lifecycle independent of session state;
- participant room.attach;
- public/participant/operator-membership Viewer types;
- durable observation frames and one frame sequence per membership;
- observation acknowledgement cursor;
- actor-barrier attach with complete Room/frame capture, Session sync-token acknowledgement, and explicit Projection Reset;
- zero-or-one coalesced frame per Transition/viewer, no Genesis frame, and independent frame-head/retained-floor/Cursor tracking;
- bounded WebSocket input/output;
- slow-consumer disconnect;
- Python SDK connection, reconnect, ack, and safe action retry loop;
- randomized projection noninterference test harness.

Exit tests:

1. Human and agent clients use the same action path.
2. Disconnect after frame receipt but before acknowledgement causes safe redelivery.
3. A first/pruned Cursor produces an explicit authorized Reset; pruning never advances Cursor or reuses a frame sequence.
4. A slow consumer cannot block another participant or grow process memory without bound.
5. Hidden Counter fixture fields never enter the unauthorized frame serializer.
6. A Transition committed during catch-up is buffered; only the matching Session token ACK enters Live and delivers it without a gap.

Gate B:

A runner author should no longer write raw WebSocket recovery or idempotency logic.

### Milestone A3 — Activation and ephemeral runners

Target: week 7, 10–16 hours

Build:

- manifest-declared attention reason types;
- per-member exact activation policy;
- activation_intents persistence in transition transaction;
- runner control WebSocket and HTTP long-poll fallback;
- activation offer, claim, renew, complete, release, expiry, and cancellation;
- five-state intents, one live lease per Membership, independent operation receipts, lease generation, and atomic claim;
- post-claim exact authorized context with complete Head/witnesses and retained-frames-or-reset union;
- Python runner abstraction that starts a fresh callback/invocation;
- replay suppression of activation delivery.

Exit tests:

1. One transition creates one logical activation despite duplicate offers.
2. Two authorized runners race; one receives the live lease.
3. Lease expiration permits another claim.
4. An offer contains no private room projection.
5. A successful claim returns only the target Agent Participant Membership's exact authorized context and one delivery branch.
6. Replay verifies Attention/decision evidence but creates no intent or Runner effect.
7. A lost claim/control reply returns the exact original result on same-operation retry; a changed hash conflicts.
8. An expired prior lease generation cannot complete a newer claim.
9. Archive, eligibility change, capability revocation, and backward-clock anomaly fence stale leases.

Terminology gate:

Code, UI, and normative docs use membership, runner, invocation, activation, and catch-up. They do not model a sleeping or continuously alive agent.

### Milestone A4 — Agent Heist rules

Target: weeks 8–9, 22–32 hours

Build:

- frozen small facility fixture and seed-derived hidden configuration;
- Navigator, Insider, and Broker roles;
- Briefing, Negotiation, Commitment, Resolution, Result, and Complete phases;
- private clues and structured exchanges;
- public clue claims, plans, endorsements, and challenges;
- sealed commitment action;
- deterministic resolution and result explanation;
- pack projections and observation deltas;
- typed attention reasons;
- three deterministic strategies: cooperative, cautious, and withholding;
- golden replay fixtures and privacy matrix tests.

Exit tests:

1. Three deterministic runners complete success and failure fixtures through the public SDK.
2. Private clues, exchanges, and commitments remain isolated.
3. Concurrent sealed submissions serialize deterministically; stale submissions catch up and retry before the deadline.
4. Pack code performs no I/O and uses no floating-point state.
5. The same stimuli produce byte-identical selected and final hashes.

Scope guard:

No avatar system, inventory tree, procedural map, combat engine, general chat, profile/feed, marketplace, or rich game content.

### Milestone A5 — Heist end-to-end demo, replay, and UI

Target: weeks 10–12, 24–34 hours

Build:

- scripted run in which one invocation exits before Commitment;
- targeted activation and fresh runner invocation;
- public Heist board and small SVG map;
- phase/deadline, participant, session, runner, and activation status;
- public plans/clues and outcome timeline;
- operator-membership room/cursor/timer/activation inspector;
- read-only replay slider and hash status;
- completed final-reveal projection;
- controlled termination after commit and automatic recovery;
- optional LLM-backed one-role example behind a user-owned API key;
- one-command deterministic demo harness.

Exit demonstration:

1. Start server, UI, and runners.
2. Three agents receive different views.
3. One invocation terminates.
4. Commitment opens and creates activation.
5. A fresh invocation claims it, catches up, and acts.
6. The server is killed after another committed action but before delivery.
7. Restart and retry preserve the original result.
8. The Heist activity reaches its terminal Complete phase; the core room remains active for authorized inspection/replay until the host operator archives it.
9. Replay produces the same outcome and hashes.

UI gate:

If UI work exceeds two weeks, cut animation and decoration. Keep map, state, timeline, activation, and replay.

### Milestone A6 — Hardening and v0.1 release

Target: weeks 13–14, 18–28 hours

Build:

- kill-point matrix around actions, frames, timers, and activation leases;
- protocol fuzzing and payload-limit tests;
- one-hour soak and reproducible load profile;
- metrics and JSON structured logs;
- database/WAL/version/integrity startup diagnostics;
- safe backup and replay verification command;
- non-root Docker image and persistent-volume example;
- native quickstart;
- SECURITY.md, CONTRIBUTING.md, code of conduct, issue templates, and limitations;
- architecture article and 60–90 second demo recording.

v0.1 exit:

- every Heist release gate passes;
- fresh checkout to running deterministic demo is under ten minutes;
- benchmark report labels targets versus measured results;
- single-node developer-preview warning is prominent;
- tag v0.1.0.

## v0.1 definition of done

### Correctness

- [ ] One total committed order exists per room.
- [ ] Accepted actions are acknowledged only after durable commit.
- [ ] Retrying an action never mutates twice.
- [ ] Same action ID with different payload is rejected.
- [ ] Timer fires once logically across restart/retry.
- [ ] Snapshot plus tail reproduces room head hash.
- [ ] Genesis plus full transition history reproduces room head after every snapshot is removed.
- [ ] Full read-only replay reproduces final hash.

### Participation

- [ ] Human and agent Principals can occupy participant Memberships and use the same Action path.
- [ ] Membership survives session disconnect and invocation termination.
- [ ] Private projection tests cover live, catch-up, reset, replay, logs, and public UI.
- [ ] The Session-token actor barrier returns no silent gaps, including a commit during handoff and an acknowledgement from another Session.
- [ ] Slow consumer memory is bounded.

### Activation

- [ ] Canonical Attention and noncanonical policy/intent evidence commit atomically with the causing Transition.
- [ ] Offers are at least once; every Activation control operation has a durable receipt, lease generation witnesses, and idempotent retry.
- [ ] One live lease per Membership and exact retained-or-reset Invocation Context are enforced.
- [ ] No runner means no model execution claim.
- [ ] One fresh invocation catches up and acts in the Heist demo.
- [ ] Replay never offers or claims an activation.

### Developer experience

- [ ] Python SDK hides raw reconnect/ack/retry mechanics.
- [ ] Deterministic runners require no paid API.
- [ ] Public protocol examples and golden fixtures exist.
- [ ] One command starts server, Heist, runners, and UI.
- [ ] Local filesystem and backup requirements are documented.

### Performance target

Measured on documented hardware:

- [ ] 1,000 mostly idle WebSocket sessions.
- [ ] 100 loaded small rooms.
- [ ] 100 accepted transitions per second aggregate target tested.
- [ ] p95 local commit-to-ack under 100 ms target tested.
- [ ] 100,000-transition recovery target tested.
- [ ] one-hour soak has bounded memory, queue, WAL, and temp growth.

A missed performance target does not justify hiding results. Publish the profile, identify the bottleneck, and decide whether it blocks the intended demo.

## Phase B: Investigation Room v0.2

Before beginning, freeze the Activity Pack host interface used by Heist. Investigation may add generic artifact metadata that was explicitly planned, but it may not add domain fields to core.

### Milestone B0 — Post-Heist review

Target: week 15, 6–10 hours

- remove Heist-specific naming from core;
- document every Activity Pack host-interface change made during Heist;
- freeze conformance fixtures;
- record any proposed Room Kernel change and prove Investigation requires it;
- tag a protocol/pack candidate baseline.

Exit:

Counter and Heist compile and pass without any Investigation code.

### Milestone B1 — Immutable evidence storage

Target: weeks 15–16, 14–22 hours

Build:

- local content-addressed artifact layout;
- streaming bounded upload to temp, BLAKE3 verification, file fsync, atomic rename, and parent-directory fsync;
- durable owner-scoped staged-upload records and conservative orphan reconciliation;
- artifacts and room_artifacts generic metadata, with artifact metadata created atomically with each staged-upload receipt;
- room/member authorization before upload/download;
- safe media handling and text/image preview;
- quota, orphan-temp cleanup, backup consistency, and digest audit;
- Cold Chain immutable local fixture installer.

Exit tests:

1. Path traversal, symlink, size, MIME, and digest attacks fail safely.
2. Unauthorized members cannot infer or fetch hidden evidence.
3. Duplicate bytes deduplicate without changing evidence version semantics.
4. Database backup plus artifact copy verifies every reference.
5. Forced termination around rename/stage/link never leaves a committed room reference to missing bytes.
6. Expired unlinked uploads are reclaimed only after the grace period, while linked or still-staged bytes survive reconciliation.

FTS and embeddings are not required.

### Milestone B2 — Investigation domain model

Target: weeks 17–18, 22–32 hours

Build:

- human Lead, Timeline analyst, Evidence analyst, and Challenger roles;
- Intake, Analysis, Review, Brief, and Closed phases;
- evidence assignment and visibility;
- immutable facts, claims, revisions, support/challenge edges;
- verification requests and dispositions;
- structured final brief;
- deterministic answer key and scoring;
- private drafts and public/participant/operator-membership projections;
- deterministic evidence-release timers.

Exit tests:

1. Human Lead and deterministic agents complete the original uncorrected fixture.
2. Every fact and claim names exact source/revision IDs.
3. Private drafts and assignments remain isolated.
4. Scoring uses no LLM judge.
5. Replay reproduces board, brief, and score.

### Milestone B3 — Correction, invalidation, and activation

Target: weeks 19–20, 18–28 hours

Build:

- corrected clock-offset evidence version;
- explicit supersedes graph;
- deterministic fact/claim/verification invalidation;
- cited_evidence_superseded and dependent_claim_stale attention reasons;
- fresh analyst invocation with authorized corrected context;
- human Lead review and brief revision;
- timeline view of the invalidation chain.

Exit demonstration:

1. An agent publishes a claim using original evidence.
2. Its invocation exits.
3. Correction arrives as a recorded stimulus.
4. Dependent work becomes stale by explicit IDs.
5. One activation is claimed by a fresh invocation.
6. The agent revises or withdraws its work.
7. Human Lead submits the corrected brief.
8. Replay reconstructs all decisions and score.

### Milestone B4 — Investigation UI, generality audit, and v0.2

Target: weeks 21–22, 18–28 hours

Build:

- evidence panel and safe artifact viewer;
- fact/claim/challenge board;
- accessible dependency list and optional small graph;
- correction markers and case timeline;
- typed human Lead forms;
- final brief and deterministic score breakdown;
- both demos in one quickstart;
- cross-pack core-change audit;
- updated benchmark with Investigation payload profile.

v0.2 exit:

- every Investigation acceptance gate passes;
- no Investigation-specific protocol message or Room Kernel table exists;
- existing Heist conformance remains green;
- the Activity Pack host interface is documented from two real implementations;
- tag v0.2.0.

## v0.2 generality gate

The abstraction passes if Investigation adds only:

- a new pack;
- new activity schemas/state/actions;
- the preplanned generic artifact store;
- new first-party UI components;
- conformance fixtures.

It fails if Investigation requires:

- a new room lifecycle;
- a domain-specific core protocol branch;
- a workflow executor;
- cross-room project state;
- server-side LLM or tool execution;
- a generic semantic memory system;
- special-case storage beyond generic artifact metadata;
- weakening Heist privacy or determinism.

Failure means revise the Activity boundary and repeat the gate. It does not automatically justify expanding the platform.

## Testing strategy

| Layer | Required tests |
|---|---|
| Canonical data | Cross-language golden JSON/hash vectors, duplicate keys, prohibited floats |
| Pack | Golden replay, property tests, invalid actions, size limits, projection noninterference |
| Room actor | Ordering, stale action, mailbox bound, passivation/reload, pack fault |
| SQLite | Genesis recovery, atomic commit, idempotency/FK constraints, WAL recovery, disk-full path, migration |
| Timer | Schedule/cancel/firing retry, overdue restart storm |
| Observation | audience isolation, duplicate delivery, actor-barrier handoff, cursor ack, reset, slow consumer |
| Activation | duplicate offer, claim receipt, claim race, lease generation/expiry, authorization, replay suppression |
| SDK | reconnect state machine, retry, cancellation, runner callback failure |
| UI | public/private DOM isolation, XSS, replay read-only, stale-action disable |
| Artifact | path, size, MIME, digest, quota, authorization, backup consistency |
| System | forced termination matrix, deterministic demo, soak, quickstart |

## Risk register

| Risk | Early warning | Response |
|---|---|---|
| The product still feels like a WebSocket wrapper | Demo shows frames but no scoped legal actions, activation, or recovery | Preserve projection, activation, cursor, and deterministic replay before integrations |
| Activity Pack tax is too high | Most application code rebuilds infrastructure or edits core | Measure both packs; stop broadening if the reusable share is small |
| Scope turns into a game platform | Time goes to art, profiles, chat, or content | Keep Heist fixture and SVG map fixed |
| Scope turns into n8n | Generic nodes, connectors, jobs, or workflow canvas appear | Keep participant-selected typed actions inside one state machine |
| Agent metaphor stays misleading | UI says asleep/awake or suggests continuous cognition | Enforce lifecycle vocabulary in code review and docs |
| Projection leaks private state | Shared serializer or cache key omits audience | Separate types, exact audiences, randomized noninterference tests |
| Determinism fails | Replay hash differs by run/platform | Eliminate ambient I/O/floats and expand golden fixtures |
| SQLite contention appears early | Commit latency and mailbox depth rise | Short transactions, one writer, profile; do not introduce a broker |
| Activation becomes a workflow runtime | Predicates, retries, callbacks, and job state expand | Exact typed reasons and one simple lease lifecycle only |
| UI consumes the schedule | General renderer/components grow | First-party reference screens only; cut polish |
| LLM demos are flaky or costly | Provider availability controls release | Deterministic runners are normative; LLM example optional |
| Investigation becomes enterprise PM | Generic tasks, approvals, dashboards appear | Keep one evidence/claim state machine and one fixture |
| Artifact handling creates security burden | Format parsing and previews expand | Small allowlist, bounded bytes, no conversion/unpacking |
| Premature scale architecture | NATS/Postgres/Kubernetes appears before data | Publish single-node numbers and defer distributed RFC |
| OSS adoption remains weak | Setup is slow or explanation remains abstract | One-command demos, short video, clear extension seam and limitations |

## Frozen cut order

If the schedule slips, cut in this order:

1. Decorative animation and graph layout.
2. Optional LLM-backed example.
3. HTTP activation long poll if runner WebSocket is complete.
4. Operator-membership convenience screens beyond required diagnostics.
5. Optional artifact preview types; retain safe download.
6. Performance beyond the documented core profile.

Do not cut:

- commit-before-ack and idempotency;
- private projections;
- cursor catch-up/reset;
- explicit activation and fresh invocation;
- timer durability;
- restart recovery;
- deterministic replay;
- complete Heist;
- complete Investigation correction/generalization story;
- acting human participant;
- deterministic scoring.

## Open-source recognition plan

Recognition should come from a small, demonstrably correct system:

1. One sentence:
   WorldStream is a realtime room runtime where human and Agent Participants share Authoritative Room State.
2. One Heist video:
   private views, absent invocation, activation, server termination, recovery, and replay.
3. One serious follow-up video:
   human plus agents, evidence correction, dependency invalidation, fresh invocation, source-linked brief.
4. One-command deterministic reproduction:
   no provider key or internet dependency.
5. One architecture article:
   single-writer rooms, SQLite transaction boundary, audience-specific frames, activation lease, and honest failure semantics.
6. One reproducible benchmark:
   ordinary hardware and clear payload profile.
7. One conformance package:
   protocol fixtures and Activity privacy/replay tests that contributors can run.
8. Public RFC only after v0.2:
   ask whether outside activity authors need a stable Rust API, WASM, or neither.

## First implementation backlog

The first ten issues should be:

1. Scaffold Cargo workspace and pinned toolchain.
2. Define canonical JSON profile and Rust/Python hash vectors.
3. Add SQLite bundled-version assertion and initial migration.
4. Implement Counter Activity Pack.
5. Implement single-writer room actor and atomic accepted transition.
6. Implement action receipts and idempotency conflict.
7. Implement snapshots and recovery hash verification.
8. Implement WebSocket hello/attach/action/observation path.
9. Implement membership cursor acknowledgement and projection reset.
10. Add forced termination test after commit and before reply.

Heist work begins only after these establish the Room Kernel.
