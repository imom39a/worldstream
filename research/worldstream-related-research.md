# WorldStream Related Research

> **RESEARCH SNAPSHOT, NOT A NORMATIVE SPECIFICATION.** Last reviewed: **2026-08-14**. This note compares the frozen WorldStream proposal with primary research sources available on that date. Product behavior remains defined by the [frozen requirements](../docs/requirements.md) and accepted [product-boundary ADR](../docs/adr/0001-product-boundary.md).

## Executive verdict

Among the primary sources reviewed here, **no single research system duplicates WorldStream's complete boundary**. WorldStream combines ideas that already exist separately - authoritative multiplayer state, event sourcing, deterministic replay, per-agent observations, durable actors, scoped disclosure, and external agent runtimes - behind one deliberately narrow boundary. This is a related-work finding, not proof of global novelty against every unpublished or unreviewed system.

The honest research claim is therefore not that WorldStream invents any one primitive. It is that this particular combination may remove a recurring systems layer for developers building shared human-agent applications:

- one deterministic authoritative Room;
- humans and independently hosted agents as first-class participants;
- typed, role-governed Actions;
- participant-specific Projections and durable observation Cursors;
- explicit durable Activation across ephemeral model invocations;
- commit-before-publish recovery and deterministic Replay;
- domain rules supplied by a trusted Activity Pack rather than an LLM.

The papers closest to one part of this boundary do not provide the whole combination. Conversely, WorldStream should not claim to be the first authoritative world, first multi-agent environment, first event-sourced agent runtime, or first replayable agent system.

### Which layer is WorldStream?

Calling WorldStream only an “application layer” is too imprecise. The project has three relevant layers:

| Layer | Responsibility | Examples |
|---|---|---|
| Agent policy and evaluation | Model selection, prompts, learning, private memory, planning, and task-stream evaluation | AgentStream, external WorldStream runners |
| Shared-environment runtime | Authoritative shared state, ordering, authorization, participant views, catch-up, activation, recovery, and replay | **WorldStream Room Kernel** |
| Domain application | Rules, actions, visibility policy, outcomes, and product UI | WorldStream Activity Packs, Agent Heist, Investigation Room |

This matches the repository's [frozen boundary](../README.md#the-frozen-boundary), [product vision](../docs/vision.md), and [context-and-memory model](../docs/context-and-memory.md): WorldStream does not host an LLM loop or own an agent's private memory. It governs the shared reality in which external policies act.

## Comparison criteria

A paper or system is a direct substitute for WorldStream only if it covers most of the following together:

1. **Multi-participant environment:** at least two independently acting humans or agents share one changing situation.
2. **Authoritative deterministic state:** one trusted state and one ordered history decide what happened.
3. **Typed admission:** actions are validated against current state and role before mutation.
4. **Scoped perception:** different Memberships can receive different authorized views without receiving canonical state.
5. **Durable continuity:** disconnected clients catch up by Cursor, and agent identity survives termination of a model invocation.
6. **Explicit activation:** the server can durably request work from an external runner without pretending the model remains continuously alive.
7. **Recovery and replay:** committed results survive crashes and can be reconstructed without rerunning an LLM or tool.
8. **Framework neutrality:** the runtime does not require one model, prompt loop, memory system, or agent framework.
9. **Domain separation:** reusable room semantics remain distinct from Activity Pack rules.

These criteria are derived from the [requirements](../docs/requirements.md), [Activity Pack boundary](../docs/activity-packs.md), [ADR 0001](../docs/adr/0001-product-boundary.md), the decision to [sequence all domain-relevant room changes](../docs/adr/0002-sequence-domain-relevant-room-changes.md), and the decision to [separate activation handling from participant action authority](../docs/adr/0003-separate-activation-and-action-authority.md).

## Closest research

### AgentStream: same word, different systems problem

[AgentStream: How Well Do Self-Evolving LLM Agents Perform Under Streaming Tasks?](https://arxiv.org/abs/2608.00155) is an evaluation framework for self-evolving agents. It converts agent benchmarks into isolated, sequential, and interleaved task streams and evaluates five self-evolution methods across three foundation models. Its research question is whether accumulated agent experience helps under changing task streams.

The overlap is continuity over time and the observation that interleaving changes agent behavior. The layer is different:

- AgentStream studies the **policy/evaluation layer** and accumulated experience inside self-evolving agents.
- WorldStream supplies a **shared-environment runtime** in which several independently operated policies act on authoritative state.

AgentStream could eventually become an evaluation workload for WorldStream runners, but it does not provide rooms, participant-specific authorization, ordered shared mutation, durable observation cursors, activation leases, or crash replay. The two projects are complementary, not duplicate implementations.

### MASS: strongest validation of state/view separation

[MASS: Multiplayer World Models with Authoritative Shared State](https://arxiv.org/abs/2608.06257) separates a global typed state from camera-specific rendering. A learned Logic Engine advances the shared state from joint actions, and a learned Rendering Engine derives each requested view. In its matched two-player Snake benchmark, the paper reports parser recovery of 0.764 versus 0.128 for the strongest video baseline and zero measured cross-view disagreement. A separate population-scale study advances 1,024 player entities for 10,000 recurrent ticks.

This is close to WorldStream's Authoritative Room State and participant Projection split. It provides strong experimental support for testing transition logic independently from view generation and for making the typed state the synchronization reference.

It is not the same runtime. MASS deliberately uses a learned transition without a hand-written game reducer; “authoritative” means the version clients adopt, not that the prediction is correct. It does not address durable action receipts, authorization, cursor catch-up, external activation, humans, or deterministic crash replay. WorldStream should borrow the separation and typed schemas, but retain a trusted deterministic reducer for canonical truth.

### Concordia: conceptual predecessor to an Activity Pack

[Concordia](https://arxiv.org/abs/2312.03664) uses a Game Master to maintain the environment, give agents observations, judge the plausibility of natural-language actions, and describe their effects. Its component model permits player-specific partial state and can combine LLM behavior with conventional state machines or other simulation models.

The Game Master is conceptually close to an Activity Pack: it grounds actions in a shared environment and resolves their consequences. However, the paper's primary interface and authority path are language-mediated and frequently LLM-driven. It also warns that concurrent-action operation can create inconsistencies, while persistence, snapshotting, and audit support were prospective rather than demonstrated runtime guarantees. WorldStream can reuse the environment/agent separation while replacing natural-language authority with typed deterministic admission and reduction.

### ActiveGraph: event sourcing, replay, lineage, and forks

[The Log is the Agent: Event-Sourced Reactive Graphs for Auditable, Forkable Agentic Systems](https://arxiv.org/abs/2605.21997) makes an append-only log the source of truth and folds it into a typed working graph. Model and tool responses are content-addressed and recorded so strict replay makes no fresh calls. The runtime also supports causal lineage, fork at an event, and structural diff over the resulting branches.

WorldStream should borrow its explicit determinism contract, causal identifiers, recording of nondeterministic model/tool results, and strict replay that serves those records without re-executing the calls and reports the first divergence. The paper also states the important boundary clearly: it does not yet provide checkpointing or compaction; outside-world mutations are recorded but not made atomic; and concurrent or distributed writers and multi-agent contention over one graph are unresolved. WorldStream's single logical Room writer, scoped projections, and participant action conflicts address a different and complementary part of the problem. Timeline forks remain deliberately outside WorldStream v0.1 and v0.2.

### Continuity Kernel: transactional authority and activation

[Beyond Memory: A Transactional Continuity Kernel for Long-Lived AI Agents](https://arxiv.org/abs/2608.11632) argues that retained storage is not automatically authoritative state. Untrusted actors propose a typed update against an exact predecessor; a short transaction revalidates ownership, pre-state authority, freshness, and effect uniqueness; and only a Commit disposition advances the authoritative branch head. Reject, Quarantine, and Defer remain stable non-commit outcomes. The authors report bounded model exploration over 2,808,230 reachable states and 5,526,474 state-changing transitions with no encoded invariant violations.

The exact-predecessor rule, pre-state authorization, stable dispositions, fencing, effect outbox, and inclusion-aware receipts are directly useful to WorldStream Actions and Activations. The main difference is scope: the kernel governs the lineage of a long-lived agent's state, while WorldStream governs a multi-participant environment and produces different authorized views. Its bounded model result is valuable evidence for the encoded state space, not an unbounded proof or verification of physical storage behavior.

### ESAA: event-sourced multi-agent software work

[ESAA: Event Sourcing for Autonomous Agents in LLM-Based Software Engineering](https://arxiv.org/abs/2602.23193) separates probabilistic intention from deterministic mutation. Agents emit validated JSON intentions; an orchestrator appends events, applies file effects, materializes state, and verifies replay through hashes. The paper reports a 9-task/49-event single-agent case and a 50-task/86-event, four-agent case.

This is close to WorldStream's stochastic proposer/deterministic reducer boundary, but it is a vertical software-engineering orchestrator rather than a general durable room runtime. Its case studies do not establish participant-specific projections, reconnect cursors, timer semantics, activation leases, or general contention safety.

### PettingZoo: observation and ordering semantics

[PettingZoo: Gym for Multi-Agent Reinforcement Learning](https://proceedings.neurips.cc/paper_files/paper/2021/hash/803f7c4c3ff61b71be53a0c803bfb57f-Abstract.html) introduces the Agent Environment Cycle model. It exposes the acting agent, per-agent observations and action spaces, optional global state, and explicit environment steps. Its case studies show that forcing a truly sequential environment through an apparently simultaneous API can hide tie-breaking races and produce incorrect observations.

PettingZoo is an in-process research environment, not a durable network service. Its semantics still suggest three concrete WorldStream rules:

- represent timers and exogenous inputs as explicit environment Stimuli;
- attribute each Transition to an explicit cause;
- for a genuinely simultaneous phase, collect a sealed set and resolve it deterministically rather than treating network arrival order as a game rule.

### Orchestrated Reality: structured world state with an LLM transition

[Orchestrated Reality](https://arxiv.org/abs/2606.16014) models a persistent game world as canonical JSON entities, structured parameterized actions, narrative observations, and an LLM-driven Plan-Diff-Validate-Apply transition pipeline. It reports a worked turn and 15 illustrative incidents from a deployment.

The canonical-state and validated-diff ideas overlap with WorldStream. The implementation remains single-human-player and LLM-authoritative, with its human study, concurrent non-player agency, and RL-environment evaluation described as future work. WorldStream should take the structured action and validate-before-apply discipline, but should not place an LLM inside the authoritative reducer.

## Mature or comparatively strong foundations

### Reliable State Machines

[Reliable State Machines](https://doi.org/10.4230/LIPIcs.ECOOP.2019.18) is the strongest mature systems foundation in this review. The ECOOP 2019 paper gives actor-like machines a durable identity, inbox, outbox, and persistent state. Processing is single-threaded, and one transaction atomically commits input dequeue, persistent-state mutation, and output enqueue. The work includes a formal failure-transparency result, Service Fabric and Kafka backends, systematic P# interleaving tests, performance evaluation, and a Microsoft Azure production-service case study.

WorldStream's room transaction should follow the same shape:

```text
admitted Stimulus
  + ordered Transition
  + updated Core and Activity State
  + state hashes
  + audience-specific Observation Frames
  + timer changes
  + Activation Intents
  + durable Action disposition
```

The paper's exactly-once claim applies inside the closed RSM runtime. WorldStream should retain its more precise at-least-once delivery plus idempotent admission language for network clients and external runner/tool effects.

### Property-based noninterference testing

[Testing Noninterference, Quickly](https://doi.org/10.1017/S0956796816000058) develops random, property-based testing techniques for information-flow machines. It emphasizes executable noninterference properties, state generation, multi-step execution, and shrinking failures to minimal counterexamples; the authors report quickly finding simple counterexamples for more than 45 injected bugs.

The direct WorldStream adaptation is a paired-state projection test for the repository's [projection and privacy isolation requirements](../docs/security.md#projection-and-privacy-isolation):

> Given two valid room states that are identical in everything visible to Membership M but differ in hidden facts, every authorized Projection, persisted Observation Frame, reconnect result, replay view, and public log visible to M must be indistinguishable.

The generator should vary hidden clues, sealed actions, private evidence, roles, Membership changes, Cursor positions, reconnect paths, and replay points. Failing traces should shrink to the smallest state/history pair that leaks information. This tests implementation behavior; it does not by itself prove noninterference for all possible programs.

### Production executability gating

[Don't Offer What Can't Be Done](https://arxiv.org/abs/2608.01050) reports a deployed Wix Helpmate pipeline in which semantic retrieval is followed by a deterministic executability gate over fresh account state. Across 756,641 messages and 267,612 conversations, the gate removed 59.4% of post-semantic skill-message candidates and 59.1% of their skill-description tokens. In a risk-enriched replay of 1,000 conversations, a model exposed to all skills selected a production-blocked skill in 78 cases (7.8%). The authors explicitly warn that the replay does not measure tool execution or customer outcomes and cannot be extrapolated to all traffic.

The transferable technique is predicate parity: derive participant `legal_actions` and the final Action validator from the same domain predicates. Version those predicates, test boundary/missing/stale cases, and revalidate at commit against fresh authoritative state. Passing an affordance gate is not permission to skip final validation.

## Additional relevant work

| Work | What it contributes | WorldStream implication |
|---|---|---|
| [Temporary Authority, Permanent Effects](https://arxiv.org/abs/2607.10487) | Defines commit-time authorization: an authority witness must still be fresh, causally prior, bound to the same effect, and eligible at durability. In its controlled 54-task matrix, 207 of 216 invalidating rows still committed after the authority path failed. | Recheck Membership, Role, room head, deadline, artifact version, and lease generation inside the commit boundary. Treat endpoint success and authorized commit as separate metrics. |
| [CapLease](https://arxiv.org/abs/2608.01710) | Shows why token-local “single use” does not stop semantic reissuance after replanning, retry, delegation, or crash. Proposes durable Issue-Prepare-Commit state bound to a canonical action and confirmation. | Keep durable monotonic consumption state; bind authorization to canonical Action bytes and a bounded budget; use an idempotent sink for external effects. |
| [MNC](https://arxiv.org/abs/2608.01719) | Attaches recipient, purpose, allowed fields, lifetime, forwarding/logging sinks, and memory permission to task-sufficient disclosures, then uses a reference monitor to enforce the scope at mediated downstream operations. | Metadata alone is insufficient. Enforce narrowing at every server-mediated sink and state clearly that WorldStream cannot stop an authorized external runner from retaining or forwarding already disclosed data outside that boundary. |
| [LatticeMind](https://arxiv.org/abs/2608.08236) | Gives claims explicit `PROPOSED`, `CONFIRMED`, `CONTESTED`, and `SUPERSEDED` status and applies symbolic conflict checks before using an LLM reconciler. It reports 0.97 versus 0.61 accuracy on its label-blind ConflictBank evaluation, with mixed results on secondary planning tasks; its coordination-conflict branch remains design-level rather than empirically validated. | For Investigation Room, model proposed, confirmed, contested, superseded, and dependency-invalidated claims explicitly. An LLM reconciliation remains a proposed Action, not truth. |
| [No Attacker Needed](https://arxiv.org/abs/2604.01350) | Defines unintentional cross-user contamination (UCC) and reports 57-71% contamination under raw shared state; text sanitization leaves residual risk for executable artifacts. | Supports keeping runner-private memory outside WorldStream, using exact audiences, and treating artifact scope separately from text sanitization. |
| [CoAgent](https://arxiv.org/abs/2606.15376) | Uses a predetermined serialization order, speculative writes, advisory LLM repair, and saga-style inverse tools for contended multi-agent work. It reports near-serial correctness within 5% at 1.4x speed on ten workloads. | Do not add this complexity to the single-writer Room Kernel. Revisit only for future external-effect workflows where operations are explicitly undoable and contention is measured. |
| [OpenSpiel](https://arxiv.org/abs/1908.09453) | Supplies established terminology and APIs for sequential/simultaneous actions, perfect/imperfect information, legal actions, and observations. | Use it as a semantic reference and possible conformance-adapter target, not as a persistence design. |
| [AIOS](https://arxiv.org/abs/2403.16971) | Moves LLM scheduling, context, memory, storage, and access control into an agent OS kernel; reports up to 2.1x faster execution across agent frameworks. | Adjacent agent-execution layer. WorldStream should integrate through runners rather than absorb model scheduling or memory management. |
| [AgentScope](https://arxiv.org/abs/2402.14034) | Provides message-centric agent composition, fault-tolerance hooks, and actor-based local/distributed execution. | Adjacent orchestration/execution framework; it does not replace authoritative shared-world semantics. |
| [OASIS](https://arxiv.org/abs/2411.11581) | Provides a scalable social-media simulation environment and reports simulations of up to one million agents. | Evidence that large agent environments and dynamic action spaces already exist; WorldStream differentiation must be durable participation semantics, not agent count or “agents in a world.” |

[A Methodology for Selecting and Composing Runtime Architecture Patterns for Production LLM Agents](https://arxiv.org/abs/2605.20173) also names the **stochastic-deterministic boundary**: proposer, deterministic verifier, durable commit, and typed rejection. This is useful vocabulary for WorldStream, but the paper is a recent single-author methodology preprint with constructed workloads and a stylized reliability decomposition. Treat it as an architectural lens, not validated quantitative theory.

## Evidence strength and caveats

The sources do not all provide the same kind of evidence.

| Evidence tier | Sources | What can safely be concluded |
|---|---|---|
| Mature peer-reviewed systems or testing work | Reliable State Machines; Testing Noninterference, Quickly; PettingZoo | Their specific models, implementations, and evaluated techniques are credible foundations. Applying them to WorldStream still requires new tests. |
| Production deployment evidence in a preprint | Wix executability gating | Deterministic eligibility filtering can materially reduce invalid options and context in the reported deployment. The exact rates do not generalize automatically. |
| Controlled recent security or state experiments | commit-time authorization, CapLease, MNC, unintentional cross-user contamination, LatticeMind, CoAgent | The reported failure modes and mechanisms deserve design tests. They are not yet broad production guarantees. |
| Architecture preprints and demonstrations | MASS, ActiveGraph, Continuity Kernel, ESAA, Orchestrated Reality, runtime-pattern methodology | These provide close designs, formalizations, bounded checks, or demonstrations. Most are 2026 preprints and some were released only days before this snapshot; independent replication and long-term operational evidence are absent. |
| Evaluation/framework papers | AgentStream, Concordia, OpenSpiel, AIOS, AgentScope, OASIS | They establish adjacent problem formulations and APIs, not WorldStream's complete runtime contract. |

Specific cautions:

- “Model checked” means checked within the encoded bounded model, not proven for all deployments.
- “Deterministic replay” is only honest when all nondeterministic inputs and model/tool results needed by replay were recorded and replay releases no effects.
- “Exactly once” must not be extended from an internal transactional subsystem to uncontrolled external APIs.
- A typed Projection can constrain what WorldStream releases, but cannot force an independently operated runner to forget authorized data.
- Learned authoritative state, as in MASS or Orchestrated Reality, may be the adopted state while still being factually wrong. WorldStream's canonical reducer should remain deterministic and trusted.

## Recommended implementation program

### v0.1: correctness before feature breadth

1. **One atomic transition transaction.** Commit input admission, Transition, state hash, projections/frames, timer changes, Activation Intents, and Action disposition together. Base the failure model on Reliable State Machines.
2. **One source of legal-action truth.** Generate projection affordances and final validation from shared Activity Pack predicates. Add parity and stale-state tests following the Wix gate's maintenance discipline.
3. **Commit-time pre-state authorization.** Recheck Membership, Role, exact room head, phase/deadline, action binding, idempotency key, and lease generation inside the Room actor transaction.
4. **Stable typed dispositions and receipts.** Distinguish accepted, deterministic domain rejection, transient error, quarantine/fault, and deferred work. Distinguish server acknowledgement from evidence of durable inclusion.
5. **Record nondeterminism before reduction.** Wall time, randomness, external input, administrative change, and any future model/tool output that affects state must enter as a typed recorded Stimulus. Replay must not call an LLM, runner, webhook, or tool.
6. **Projection noninterference tests.** Generate low-equivalent paired states and multi-step histories; compare Projection, frame, catch-up, replay, UI payload, and log output byte-for-byte where practical; shrink counterexamples.
7. **Systematic failure and schedule testing.** Explore crash points before and after SQLite commit, duplicate Actions, lost acknowledgements, timer retries, competing Activation claims, lease expiry, fencing, reconnect, and snapshot deletion. A small executable state model can precede full implementation.
8. **Explicit order semantics.** Model timers and external changes as Stimuli. Keep ordinary Actions serialized. Implement simultaneous phases only as sealed collection plus one deterministic resolution Transition.

These recommendations strengthen existing requirements rather than expand the product boundary.

### v0.2: Investigation Room semantics

1. Add immutable evidence versions, content digests, causal `supersedes` and dependency edges, and deterministic invalidation.
2. Give claims explicit states such as proposed, confirmed, contested, superseded, and stale; use symbolic conflict/dependency checks before asking an agent to interpret a conflict.
3. Treat any LLM-generated reconciliation or final synthesis as a typed participant proposal. Keep acceptance and scoring deterministic.
4. Extend privacy tests across artifact metadata, bytes, claims derived from private evidence, correction waves, Activation Context, and replay.
5. Evaluate purpose/lifetime/no-forward/no-memory scope metadata where the server mediates the sink, while documenting the external-runner retention limit.

### Later, only after both reference releases

- Consider ActiveGraph-style fork/diff only after checkpointing, compaction, effect isolation, and the frozen replay contract are proven. Forking is currently excluded by ADR 0001.
- Consider PettingZoo, OpenSpiel, OpenEnv, A2A, or MCP adapters as compatibility layers; none should become the canonical Room log.
- Consider CoAgent-style saga/concurrency machinery only if a later product owns undoable external effects. It is unnecessary for the frozen single-writer kernel.
- Measure whether Activity Pack authors can reuse the Room Kernel without adding new core concepts. Investigation Room is the first falsification test.

## WorldStream's defensible differentiation

The strongest defensible statement is:

> WorldStream is a self-hosted shared-environment runtime that combines a deterministic authoritative room, participant-specific perception, durable catch-up, explicit activation of ephemeral external agents, and crash-safe replay behind a framework-neutral Activity Pack boundary.

That statement is narrower and more credible than claiming a new form of agent intelligence or a first-of-kind multi-agent world. The closest papers divide the space differently:

- MASS and Concordia focus on world modeling or simulation but not WorldStream's durability and authority contract.
- ActiveGraph and ESAA focus on event-sourced agent execution but not scoped realtime multi-participant rooms.
- Continuity Kernel, commit-time authorization, and CapLease focus on authoritative state/effect admission but not participant perception and catch-up.
- PettingZoo and OpenSpiel formalize agent-environment interaction but not network durability and activation.
- AgentStream evaluates evolving policies rather than supplying shared-environment infrastructure.
- AIOS and AgentScope run agents; WorldStream governs the shared reality those agents act upon.

The research therefore supports continuing the frozen design. The highest-value next step is not another LLM orchestration feature; it is evidence that the deterministic kernel preserves atomicity, authorization, projection isolation, cursor continuity, activation fencing, and replay under adversarial schedules and crashes.
