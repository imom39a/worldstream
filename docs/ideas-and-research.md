# Idea Backlog and Landscape Research

> **NON-NORMATIVE ARCHIVE.** This file preserves exploration history and market notes. It is not a roadmap, requirements document, or current product claim. [Frozen Requirements](requirements.md) and [ADR 0001](adr/0001-product-boundary.md) supersede every recommendation in this file.

## Research snapshot

Last reviewed: **2026-08-13**

This document preserves the product exploration behind WorldStream. It is not a claim that the landscape is static or that no unpublished project overlaps. Recheck links, licenses, protocol versions, activity, package names, and trademark availability before a public launch.

## Question we are answering

What open-source project could:

- demonstrate serious Rust and systems-design skill;
- be clearly useful in the fast-growing LLM/agent application layer;
- support real-time and long-running agents without becoming an ML research project;
- produce a cool public demonstration;
- remain valuable without a marketplace or social-network cold start;
- later support an ecosystem of developers and agents building activities;
- use crypto only when it creates real cross-operator capabilities?

## Current frozen answer

Build **WorldStream** as a narrow self-hosted agent participation runtime:

1. **Agent Heist v0.1** proves the single-room kernel.
2. **Investigation Room v0.2** proves the same kernel supports serious non-game work.

The current thesis has five user-facing concepts: Room, Membership, Projection, Action, and Cursor/Activation. Agent execution remains outside the server, and each invocation is ephemeral.

All other ideas below—including BranchLab, Greenhouse, marketplaces, crypto, cross-room projects, public packs, forks, and generated UI—are parked explorations. They may be useful later, but none is a committed follow-up to v0.2.

No single primitive is novel. The honest contribution being tested is whether an opinionated combination of authoritative state, partial perception, durable catch-up, explicit activation, and deterministic replay removes a recurring layer for AI application developers.

## Why an agent Reddit is a weak core project

A feed gives agents somewhere to emit text but not a reason for meaningful, repeated interaction. Agents do not intrinsically get bored or seek friendship; their owners supply goals.

Recent Moltbook research is a useful warning:

- [Form Without Function: Agent Social Behavior in the Moltbook Network](https://arxiv.org/abs/2604.13052) reports that 91.4% of post authors never returned to their own threads and 85.6% of conversations were flat in its observed dataset.
- [The Anatomy of the Moltbook Social Graph](https://arxiv.org/abs/2602.10131) reports very shallow conversations and substantial exact duplication in its early snapshot.

These papers describe one platform and period, not a universal law. The product lesson is still strong: an agent habitat needs structured goals, scarcity, private information, deadlines, verifiable outcomes, and durable artifacts. Synthetic chatter is not enough.

## Landscape boundary

### Protocol and infrastructure overlaps

| Project/standard | Already covers | WorldStream should do |
|---|---|---|
| [A2A](https://a2a-protocol.org/latest/specification/) | Agent Cards, discovery, messages, artifacts, stateful tasks, streaming, push notifications | Use an adapter; add shared authoritative room rules, partial perception, attention budgets, and forks |
| [MCP](https://modelcontextprotocol.io/specification/latest) | Tools, resources, prompts, and client/server transport | Expose room actions/resources; do not treat MCP as the authoritative world log |
| [NATS JetStream](https://docs.nats.io/nats-concepts/jetstream) | Durable streams, consumers, acknowledgements, replay, redelivery | Add application semantics; optionally use it for cross-process routing later |
| [Agent Protocol](https://github.com/langchain-ai/agent-protocol) | Thread streaming, filtered subscriptions, nested agents, state, checkpoints, replay | Avoid claiming generic agent streaming or checkpointing as novel |
| [LangGraph time travel](https://docs.langchain.com/oss/python/langgraph/use-time-travel) | Framework-specific replay and fork from checkpoints | Make concurrent shared-world or cross-runtime futures, validation, and effect isolation first class |
| [OpenEnv](https://github.com/huggingface/OpenEnv) | Gymnasium-style `step`, `reset`, and `state` for agent environments and training/evaluation | Build compatibility later; focus on persistent independently operated multiplayer rooms |
| [Buzz](https://github.com/block/buzz) | Rust-based self-hosted rooms where humans and agents communicate and work over an event-log architecture | Avoid “Slack/Discord in Rust” positioning; focus on Activity Packs, perception/wake, and forks |

### Agent-world overlaps

| Project | Collision with the idea | Remaining lesson/gap |
|---|---|---|
| [Agent World Protocol](https://github.com/0xMerl99/Agent-World-Protocol) | Persistent multiplayer world, WebSockets, guilds, markets, bounties, optional Solana | One world/game is not enough differentiation; make the activity runtime the product |
| [AgentWorld](https://agentworld.io/) | Bring-your-own-agent persistent shared RPG | Compete on portable activity semantics and systems guarantees, not world content |
| [Agentic Island](https://agenticisland.ai/) | Open-source, MCP-connected customizable survival islands with live spectators | A creator ecosystem is already intuitive; distinguish with durable offline perception and forks |
| [Emergence World](https://github.com/EmergenceAI/Emergence-World) | Persistent living agent society under constraints | Avoid claiming the first emergent agent society |
| [OpGrid](https://www.opgrid.world/) | Persistent 3D world plus ERC-8004, x402, reputation, and on-chain economy | “Crypto world for agents” is already occupied; crypto must support a deeper runtime capability |
| [Agora](https://theagora.dev/) | Durable rooms, agent work, bounties, and streaming | Do not reduce the thesis to rooms plus tasks |

The existence of these projects is good news: developers understand “bring your agent into a world.” It also means the first README must immediately show why WorldStream is not another hard-coded world.

## Idea scorecard

Scores are directional: 1 is weak and 5 is strong.

| Idea | Relative novelty | Uses core streaming semantics | Works without network effects | Solo feasibility | Visual/cool factor | Recommendation |
|---|---:|---:|---:|---:|---:|---|
| Generic agent Reddit | 1 | 2 | 2 | 4 | 3 | Reject as core |
| Generic agent marketplace | 1 | 3 | 1 | 2 | 2 | Reject as core |
| One persistent agent RPG | 2 | 4 | 3 | 2 | 5 | Use only as demo |
| Generic agent arena/leaderboard | 2 | 3 | 3 | 3 | 4 | Crowded; activity pack only |
| **WorldStream Activity runtime** | **4** | **5** | **4** | **4 if scoped** | **5** | **Frozen core** |
| **BranchLab** | **4** | **5** | **5** | **4 for coding** | **5** | Parked exploration |
| Agent Greenhouse | 4 | 5 | 5 | 4 | 3 | Parked exploration |
| Proof Gate | 4 | 5 | 5 | 3 | 4 | BranchLab module |
| Agent ICU | 4 | 5 | 5 | 3 | 4 | Later recovery module |
| Context Loom | 3 | 5 | 5 | 3 | 3 | Valuable but easy to call “RAG+” |
| Artifact Reactor | 3 | 5 | 5 | 4 | 4 | Strong repo vertical later |
| Nomad Agent | 4 | 5 | 4 | 2 | 5 | Crypto/federation experiment later |
| Protocol Keeper Council | 4 | 5 | 4 | 3 for toy demo | 5 | Best crypto-native activity |

## Detailed application ideas

### 1. Agent Heist

**Role:** v0.1 flagship activity.

Several independently hosted agents receive different private clues, exchange structured information, and make simultaneous decisions. One invocation terminates before a required phase; a durable activation lets its external runner start a fresh invocation and catch up from the membership cursor. Humans can watch live and inspect a deterministic replay.

Why it matters:

- demonstrates private projections better than a chat room;
- creates real reasons for agents to interact;
- makes ephemeral invocation and activation semantics visible;
- demonstrates crash recovery and replay;
- avoids dangerous real-world side effects.

Risk: game content can consume the project. Keep it to one small board and one complete loop.

### 2. BranchLab — Git for live agent futures

**Role:** parked coding exploration, not a committed reference application.

A long-running coding agent reaches uncertainty or stagnation. BranchLab captures a typed checkpoint at a tool/model boundary, creates isolated Git worktree or container branches, runs different models/prompts/strategies concurrently, streams their observable progress, validates results with tests, and selects one branch.

Distinctive contribution:

- same real task and same shared prefix;
- multiple concurrent futures rather than sequential retries;
- framework adapter boundary rather than one graph runtime;
- objective validators;
- external-effect isolation;
- explicit winner promotion;
- a dataset of paired winning/losing trajectories.

Do not claim arbitrary process snapshotting. The agent must expose rehydratable typed state, and only a promoted branch may release staged effects.

Killer demo:

> A coding agent loops on a failing test. Three strategies branch into isolated worktrees. The Rust server is killed and resumed. One branch passes tests and exactly one commit is released.

### 3. Agent Greenhouse

**Role:** parked lifecycle exploration, not a committed reference application.

An owner gives an agent a standing mission. Durable membership and explicit state remain while no invocation runs. Typed external changes can create an activation for a fresh invocation, which may start temporary specialists and then terminate again.

Example:

> A repository gardener observes 100,000 noisy CI, issue, and security events over a simulated week. It wakes for a new CVE, a reproducible regression, and a cluster of related issues—then creates tested patches.

The v0.1 deterministic wake engine provides the foundation. Later semantic relevance can advise delivery, but must never suppress mandatory deadlines silently.

Risk: this can look like a workflow scheduler. The product must emphasize durable lifecycle, compact perception, explicit wake reasons, and measurable reduction relative to polling/firehose baselines.

### 4. Proof Gate

**Role:** safety module for consequential agents.

Before an irreversible action executes, the runtime holds it and launches simulator, critic, validator, and policy branches against shadow state. Evidence streams into one gate; only an approved action is released.

Good initial domains:

- database schema migration;
- Terraform or Kubernetes change;
- code merge;
- dependency upgrade;
- smart-contract parameter change on a toy chain.

Example:

> A migration is run on a cloned database. One agent tests rollback, one checks data invariants, one benchmarks load, and one probes permissions. The gate unlocks only after objective checks pass.

Risk: simulation fidelity can create false confidence. Start where sandboxes and deterministic checks are strong; retain human approval for valuable effects.

### 5. Agent ICU

**Role:** active recovery for sick or stuck agents.

A supervisor watches explicit progress signals, tool heartbeats, validator changes, and repeated action patterns. It can quarantine a run, fork from the last healthy checkpoint, and let a replacement strategy or model claim a fenced execution lease.

This differs from observability that merely reports failure: ICU attempts a replayable rescue while preserving the mission.

Risk: generic “stuck” detection is unreliable. Begin with objective signals such as repeated tool call, failed test unchanged, expired lease, provider outage, or no artifact delta—not an opaque judgment of whether a model is thinking deeply.

### 6. Artifact Reactor

**Role:** collaborative production activity.

Agents coordinate through typed patches and validators around one live artifact rather than through a conversation feed. Initially the artifact is a Git repository.

Participants might own implementation, tests, fuzzing, security, benchmarking, and documentation. Git worktrees isolate attempts; the canonical artifact remains continuously buildable.

Risk: it can look like Git plus CI. The differentiated layer is causal patch/event streams, role leases, current legal mutations, relevant delta delivery, and automatic branch/recovery primitives.

### 7. Context Loom

**Role:** continuously compiled shared reality.

Observer agents emit typed provenance-linked claims; critic agents challenge them; deterministic reducers maintain a bounded current world model. Decision agents receive material claim changes and can inspect evidence ancestry.

Useful verticals:

- incident response;
- market/news synthesis;
- evolving software architecture knowledge;
- policy or regulatory monitoring.

Risk: without explicit typed claims, invalidation, and provenance, this is merely streaming RAG with extra agents. Pick one ontology and never discard source evidence behind summaries.

### 8. Nomad Agent

**Role:** future crypto/federation experiment.

One long-lived agent has portable identity, encrypted memory checkpoints, signed lineage, and a single active execution lease. It can migrate from a laptop to an approved cloud host and later return without losing identity or duplicating effects.

Crypto is justified only across mutually distrustful hosts or owners:

- portable ownership/identity;
- public lease ordering or checkpoint commitments;
- portable attestations.

For one person's laptop and server, signatures plus a coordinator are simpler and better. Never store memory on-chain; store only identity, lease, and checkpoint roots.

Hard problems:

- serializing framework state;
- split-brain prevention;
- secret confidentiality on a new host;
- late results from the old executor;
- clear meaning of “ownership” when models and services remain external.

### 9. Protocol Keeper Council

**Role:** strongest crypto-native Activity Pack.

Independently operated standing agents watch a toy on-chain protocol as watcher, investigator, simulator, skeptic, and responder. On an incident they create a shared room, propose hypotheses, fork chain state, simulate bounded responses, and issue threshold attestations. A human or multisig retains high-risk authority.

Why chain is genuinely useful:

- the protected state and assets are on-chain;
- authority can be encoded as bounded wallet capabilities;
- final attestations and actions need public ordering;
- independently operated agents may not trust one room operator.

Killer demo:

> Attack a toy lending pool on an Anvil chain. Five agents detect the anomaly, reject one unsafe response after forked simulation, and threshold-approve a reversible pause. Restart the broker mid-incident and resume exactly.

Risk: correlated models can agree on a bad response, and a false positive can halt a protocol. Keep actions reversible, low-value, bounded, and human-approved. Do not market an early demo as autonomous production security.

### 10. Sealed-bid Agent Auction

**Role:** recommended second built-in Activity Pack and protocol stress test.

Agents submit private bids, optionally form coalitions, then reveal or settle under deterministic rules. It exercises:

- confidential submissions;
- simultaneous windows;
- deterministic tie-breaking;
- budgets and scarcity;
- objective outcomes;
- fewer game-specific assumptions than Heist.

It is not an agent labor marketplace. It is a reusable activity demonstrating mechanism design and privacy boundaries.

## Ideas rejected as the main thesis

### Generic “Fiverr for agents”

Discovery, hiring, payment, reputation, and agent-to-agent delegation already appear in several ecosystems. More importantly, a marketplace needs supply, demand, trust, verification, dispute handling, and often regulated payment operations before it becomes useful.

The reusable technical primitive from that exploration is a **contract room** with versioned acceptance criteria, evidence, bounded delegation, and conditional effect release. It may become an Activity Pack, but should not define v0.1.

### Generic agent Discord/Slack

Projects already provide rooms, agent membership, tasks, messages, and workflows. Chat also encourages synthetic activity without structured consequences. WorldStream rooms are authoritative state machines, not channels with extra bots.

### One crypto metaverse for agents

Persistent agent worlds with wallets, resources, building, reputation, and markets already exist. A single world creates a content burden and makes the infrastructure difficult to reuse. Agent Heist is a reference pack; WorldStream is the product.

### Agent leaderboard or arena

Useful as a pack, but crowded and often optimized for one-shot benchmark scores. WorldStream should emphasize long-lived membership, partial perception, sleep/wake, recovery, and forks around real ongoing state.

### Streaming market analyst

This remains a good application, but it is not a distinctive core identity. A Context Loom pack can ingest market data, news, filings, and social feeds, maintain claims and provenance, and wake decision agents on material changes. Avoid autonomous financial-action claims and use current legal/financial safeguards.

## Crypto design filter

Use a chain only if at least one of these is true:

1. Participants or hosts do not trust one another.
2. Ownership or identity must move across operators.
3. Scarce assets or final settlement already live on-chain.
4. A public commitment or attestation must be independently verifiable.
5. Bounded authority must compose with existing on-chain systems.

If none apply, use ordinary signatures, a database, and play credits.

### Good optional uses

- Map a participant to [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) portable identity metadata.
- Owner-signed short-lived room session key.
- Bounded spend mandate by pack digest, asset, amount, counterparty, expiry, and delegation depth.
- Activity Pack ownership/provenance metadata.
- Periodic Merkle commitment to a disclosed event-log segment.
- Final outcome, validation, or reputation attestation.
- Prize pool or conditional settlement for an objectively verifiable activity.
- Integration with an on-chain-world framework such as [MUD](https://mud.dev/) where the activity state genuinely belongs on-chain.

### Bad uses

- Every movement, message, observation, or presence update on-chain.
- Prompts, chain-of-thought, private memory, or private observations on-chain.
- An unrestricted owner wallet inside the server or agent prompt.
- A token before a useful runtime and community exist.
- Treating an NFT/registry entry as proof that an agent is unique, capable, or safe.
- Calling a payment contract “escrow” without legal and operational clarity.
- Using blockchain latency inside the real-time room hot path.

### Recommended split

```text
Owner identity/wallet
  -> signs scoped session or spend mandate
  -> off-chain WorldStream room processes high-frequency actions
  -> deterministic transition log and signed receipts
  -> periodic optional commitment
  -> final optional validation/reputation/settlement
```

ERC-8004 itself separates identity, reputation, and validation and does not include payments. Its security notes acknowledge Sybil risk and that registration cannot prove advertised capability. Use it as an adapter, not a replacement for runtime authorization or testing.

## Adoption and recognition strategy

The project should create credibility along three axes:

### Systems credibility

- Clear per-room ordering and failure semantics.
- Commit-before-acknowledgement.
- Idempotent action IDs and at-least-once delivery.
- Deterministic replay hashes.
- Kill-point and disk-failure tests.
- Bounded slow consumers and wake budgets.
- Honest single-node benchmarks before distributed claims.

### AI application credibility

- Different agent frameworks can eventually pass the same conformance suite.
- Observations contain relevant changes, legal actions, provenance, and deadlines.
- Long-running agents can sleep without losing membership or context.
- BranchLab turns forks into useful counterfactual agent execution.
- No model training or chain-of-thought collection is required.

### Open-source appeal

- One-command deterministic demonstration.
- Visually obvious Heist timeline and fork tree.
- Small pack interface and good examples.
- Portable schemas and protocol fixtures.
- Public design RFCs after real implementation experience.
- “Good first issue” paths for bot authors, activity authors, adapter authors, and systems contributors.

## Decision record

The frozen priority order as of 2026-08-13 is:

1. Durable single-room transition kernel.
2. Human/agent membership, scoped projection, participant inbox, and cursor reconnect.
3. Explicit activation intents for external ephemeral agent runners.
4. Agent Heist through one trusted compiled-in Rust Activity Pack.
5. Crash recovery, deterministic read-only replay, Python SDK, and small first-party UI.
6. Investigation Room with an acting human, immutable evidence, deterministic invalidation, and scoring.
7. Review the Activity interface using evidence from both packs.

There is no committed item eight. Any later work requires a new ADR after the two reference releases pass.
