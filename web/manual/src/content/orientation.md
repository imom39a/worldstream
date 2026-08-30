# Developer orientation

> **Agent orchestrators coordinate agents to complete work. WorldStream governs
> a shared reality in which humans and agents participate.**

WorldStream is a deterministic **Room Kernel** for shared human-agent
applications. A Room has one authoritative state, one immutable ordered history,
one pinned Activity Pack revision, and Membership-scoped views. Humans and agents
propose typed Actions; the kernel decides what commits.

“Shared reality” is the product narrative; `Room` remains the canonical domain
term. WorldStream may create durable Activation Intents for external Runners,
but it does not own agent plans, prompts, tools, or model loops. Its authority
is the Room: rules, truth, visibility, ordering, continuity, and Replay.

> **Current audience:** maintainers and trusted local developers working from
> source before the published v0.1 server. This manual labels deferred behavior
> instead of presenting roadmap items as shipped features.

## The boundary in one minute

| WorldStream owns | The application or agent owns |
| --- | --- |
| Room identity, state, status, ordered Transitions | Model and provider selection |
| Membership, capabilities, standing, access mode | Prompts, policy, planning, private memory |
| Action admission and exact Action Offers | Choosing whether and how to act |
| Membership-scoped Projections and Observation Frames | Rendering product-specific experiences |
| Durable Activation, leases, Cursors, receipts | Running an Invocation and external tools |
| Timers, crash recovery, integrity, Replay | Business operations outside the Room |

An Agent Participant is a durable logical identity. A Runner is an external
execution host. An Invocation is one bounded run of an agent policy. None of
these three concepts is interchangeable.

## What is implemented now

- One `worldstreamd` process with bundled SQLite or startup-selected PostgreSQL
  17 storage.
- A typed Room protocol for attach, scoped observation delivery, ACK, exact
  Action submission, Activation leasing, and Replay.
- One five-operation `ActivityPackV1` host with retained embedded revisions and an accepted WASI-free portable Pack contract now under implementation.
- Reference Counter and Agent Heist behavior plus conformance and acceptance
  fixtures.
- WorldStream Studio, its bounded local Supervisor, Task setup, Participant
  handoff, Runner Templates, Agent Profiles, backup operations, and attention
  inbox.
- An assignment-bound local MCP server for external agents and a post-MVP
  managed reference Agent Host.
- Backup, restore, semantic verification, and one-way SQLite-to-PostgreSQL
  transfer tooling.

## Deliberately not promised

- A general workflow engine, queue, project manager, browser automation host,
  agent framework, prompt store, or vector database.
- Network Pack registries, hot loading, automatic approval, generic Pack
  effects, or arbitrary renderer code. Public bundles are offline,
  exact-digest-approved WASI-free Components.
- A distributed multi-writer Room runtime. One process and one logical writer
  per Room are core v0.1 boundaries.
- Anonymous public Room state. “Public Projection” still means an authorized
  public-view Membership.
- Model-provider credentials or model execution inside `worldstreamd`.

## Recommended reading order

1. [Local quickstart](#/quickstart) — build and run the real stack.
2. [Domain model](#/concepts/domain-model) — learn the precise vocabulary.
3. [Architecture](#/concepts/architecture) — follow one Action through commit.
4. [Studio setup](#/studio/setup) — operate the local control plane.
5. [Activity Pack overview](#/activity-packs/overview) — build rules on the
   kernel.
6. [WorldStream Negotiate](#/activity-packs/negotiate) — understand the first serious public Pack.
7. [Generic MCP agent](#/agents/mcp) — attach any external policy.
8. [Operations runbook](#/operations/runbook) — diagnose and recover safely.

## Sources of truth

This site is a developer navigation layer, not a replacement for normative
material. When claims differ, use this precedence:

1. accepted ADRs and frozen requirements;
2. root domain context and normative architecture/protocol documents;
3. executable conformance tests and current source;
4. this manual.

Primary repository sources: [domain context](https://github.com/imom39a/worldstream/blob/main/CONTEXT.md),
[requirements](https://github.com/imom39a/worldstream/blob/main/docs/requirements.md),
[decision index](https://github.com/imom39a/worldstream/blob/main/docs/decision-index.md),
and [product-boundary ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0001-product-boundary.md).
