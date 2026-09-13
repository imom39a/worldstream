# Foundations for long-running agents in WorldStream

Status: Non-normative research and design recommendation, 2026-09-12.
No accepted ADR, protocol, codec, or implementation changes are made here.
Reported research results are not independently reproduced. The companion
[paper review](long-horizon-agent-papers-research.md) records eight primary
papers, experimental horizons, limitations, and implementation links. The
[existing primitives proposal](long-running-room-primitives-proposal.md)
contains the detailed repository findings and qualification workload.

## Decision

Build WorldStream as the durable, authoritative participation boundary for
replaceable agent executions. A logical participant can continue for days
without keeping a process, model request, connection, or context window alive.
Its current obligations and accepted contributions must survive independently
of the Runner's recollection.

Three problems need separate contracts:

| Problem | Required property | Appropriate owner |
| --- | --- | --- |
| Execution continuity | Recover recorded progress after crashes, retries, waits, and replacement | External Runner and durable operational execution state |
| Cognitive continuity | Construct useful bounded context and retrieve older evidence | Runner context and Agent-Private Memory policy |
| Shared-world continuity | Preserve authorized current facts, work, causality, and accepted changes despite concurrent participants | WorldStream Room authority and Activity Pack rules |

Published work supports these ingredients. The reviewed evidence does not
establish 100,000 correct open-world agent decisions in a changing shared
environment. Runtime durability and model competence require different tests.

## What Codex and Claude publicly document

The local Codex installation inspected for this question contains a JavaScript
launcher and a platform binary, not the Rust engine source. The following
account therefore uses public first-party documentation. It does not claim
knowledge of undisclosed service internals.

Codex App Server exposes **Thread**, **Turn**, and **Item**: a durable
conversation, one user request and ensuing work, and individual messages or
operations. Its interface includes resume, stored-history reads, streaming
item lifecycles, interruption, steering, and explicit history compaction.
A thread ID provides continuity across these operations; it is not a count
of model calls. [Codex App Server](https://learn.chatgpt.com/docs/app-server).

OpenAI's Responses API separately documents threshold-triggered and standalone
compaction. Compaction returns a smaller context containing an opaque encrypted
item carrying prior state. This is a documented mechanism for a custom Runner,
not evidence of the exact compaction implementation in every Codex interface.
[OpenAI compaction](https://developers.openai.com/api/docs/guides/compaction).

Claude Code documents a context/action/verification loop, locally persisted
session events, resume under the same session ID, and context management that
clears older tool outputs and summarizes history. Project files and persistent
instructions provide additional continuity. Its documentation explicitly notes
that early instructions can be lost during compaction. File rewind does not
undo effects on remote systems.
[How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works).

The common engineering interpretation is:

```text
durable work and history
        ↓ select a bounded context
model proposes a tool call or contribution
        ↓ harness executes / authoritative system admits
record outcome → verify → continue, wait, or finish this execution
        ↓
compact or replace execution when needed; preserve logical identity
```

The long-lived object is recorded work. A context window is one working view
of that work. A conversation transcript alone does not establish which
external actions happened or whether the goal was achieved.

### A particularly close production architecture

Anthropic's April 2026 Managed Agents report separates a durable session log,
a replaceable harness, and tool/sandbox environments. Harness recovery reads
the session; context construction can retrieve slices of earlier events.
This separates recoverable history from choices about what fits in a prompt.
It is a first-party architecture report, not a 100,000-action correctness
benchmark. [Managed Agents architecture](https://www.anthropic.com/engineering/managed-agents).

WorldStream can adopt that separation while keeping its additional shared
authority boundary. A Runner's session records its execution; a Room records
accepted shared changes. These logs serve different purposes and may be
linked by Action and operation identities without being merged.

Anthropic's November 2025 harness uses explicit requirements, incremental
work, persistent progress artifacts, and tests to prevent premature completion.
Its March 2026 follow-up found that a newer model eliminated the need for one
explicit context-reset mechanism. Preserve stable interfaces while measuring
model-specific context policies.
[Earlier harness](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents),
[follow-up](https://www.anthropic.com/engineering/harness-design-long-running-apps).

## Evidence worth implementing around

| Evidence | Supported mechanism | Limit of the evidence |
| --- | --- | --- |
| [MemGPT, 2023](https://arxiv.org/html/2310.08560v2) | Explicit working memory and external recall | Small multi-session evaluation, not arbitrary task continuity |
| [ReSum, 2025](https://arxiv.org/html/2509.13313v1) and [AgentFold, 2025](https://arxiv.org/html/2510.24699v1) | Deliberate context condensation and continuation | ReSum evaluates at most 60 tool calls; AgentFold tests accuracy through 256 turns and separately traces context through 500 |
| [MAKER, 2025](https://arxiv.org/html/2511.09030v1) | Small checkable steps and error correction | 1,048,575 correct Hanoi moves using supplied decomposition; not discovery of an open-world plan |
| [RLM, 2025; revised 2026](https://arxiv.org/html/2512.24601v3) | Programmatic access to large external context | Large-input reasoning experiments, not durable realtime execution |
| [LongMemEval-V2, 2026](https://arxiv.org/html/2605.12493v1) | Evaluate memory quality and latency separately | Histories up to 115M tokens; reported retrieval can take minutes; offline questions rather than live effects |

The stronger lesson is architectural: retain evidence, construct a bounded
working view, record open work explicitly, and verify small contributions.
No particular summarization prompt should become a Core invariant.

Two existing implementations address different portions of the problem:

- **Temporal:** Continue-As-New carries explicit state into a fresh execution
  history while preserving the Workflow ID. Activities record individual
  operations; their code may be nondeterministic and should be idempotent.
  This is a useful optional Runner substrate for days of waits and retries.
  It does not require rotating Room identity or inserting workflow execution
  into the Pack reducer.
  [Continue-As-New](https://docs.temporal.io/workflow-execution/continue-as-new),
  [Activities](https://docs.temporal.io/activities).
- **LangGraph:** Checkpointers persist graph state at execution boundaries;
  stores hold information across threads. Its sync and async durability modes
  have different crash guarantees. Current documentation explicitly discusses
  the write amplification of full accumulated state and a beta delta format.
  This is another Runner option, with storage and recovery behavior to qualify.
  [Persistence](https://docs.langchain.com/oss/python/langgraph/persistence),
  [checkpointers](https://docs.langchain.com/oss/python/langgraph/checkpointers).

## First-principles derivation

These are our design deductions, not additional experimental results.

### 1. Persistent work must outlive its producer

If agent A leaves before agent B arrives, communication must exist outside
both processes. Linda's 1985 generative-communication model explicitly
developed this separation across process identity and time. This is a useful
theoretical ancestor of durable shared contributions; adopting the insight
does not require adopting a general tuple-space API.
[Gelernter, Generative Communication in Linda, section 2.4](https://www.cs.tufts.edu/comp/150FP/archive/david-gelernter/generative-linda.pdf).

For WorldStream: Principal and Membership persist; Invocations and Sessions
are replaceable. An unresolved Pack-defined work item remains discoverable
even if the originating Invocation disappears. Completion of an Activation
is a handling disposition, not proof of completion of that work item.

### 2. Reasoning and authoritative mutation have different semantics

Conceptually, for accepted Stimulus `u` and Room state `S`:

```text
S[n+1] = reduce(S[n], u[n+1])
O[a,n] = authorized_view(S[n], Membership[a])
proposal = policy(O[a,n], Agent-Private Memory)
```

The policy may be stochastic, expensive, mistaken, or unavailable. Admission
and reduction must enforce exact authority, basis, and domain rules. A replay
uses recorded inputs and exact Pack revisions; it does not call the model
again. Membership changes and other Core Stimuli still use their own accepted
rules; this notation is not a replacement Pack API.

An accepted claim establishes its attribution and allowed disposition. Its
truth needs evidence and an applicable verification rule. For example, an
airline source update and an agent's inferred connection risk must remain
distinguishable even when both are durably recorded.

### 3. Bounded memory requires a defined current state

There are more possible unbounded histories than finite summaries. Two
different histories must eventually share a summary, and a later question
can distinguish them. A fixed-size summary therefore cannot preserve every
possible future answer exactly.

Define the current facts sufficient for ordinary legal Actions. Preserve
unresolved work explicitly, and retain authorized historical evidence for
other questions. Keep working context bounded independently of retained
history. This requires limits on admitted active work; never achieve a bound
by silently discarding an unresolved obligation.

Use four distinct records:

| Record | What it establishes |
| --- | --- |
| Verified recovery checkpoint | Exact authoritative state and the witnesses required to recover it |
| Pack-defined work record | What remains open, its revision, evidence, and permitted progress |
| Runner memory/summary | A fallible aid to subsequent reasoning |
| Membership Cursor and synchronization state | Delivery processing and attachment progress under their separate contracts |

Context compaction must not rewrite canonical history, advance a Cursor, or
claim that pending work has been completed.

### 4. Realtime decisions need validity across the reasoning interval

Perfect memory does not guarantee a fresh decision. An agent may observe a
Room, spend ten seconds reasoning, and return after multiple commits.

Our current `based_on_room_seq` rule rejects any proposal based on an older
Head. Under an illustrative Poisson arrival model, if Room commits occur at
rate `lambda` and observation-to-submit takes `T`, then:

```text
P(no intervening commit) = exp(-lambda * T)
100,000 Room commits/day → lambda = 1.1574/second
T = 10 seconds → P = 0.0000094068, or about 0.00094%
```

This is a mathematical scenario, not a measurement of WorldStream. It assumes
every intervening commit invalidates the whole-Head basis, an observation at
the beginning, and no intermediate refresh. Bursty arrivals or different Room
boundaries produce different results. It demonstrates why durable storage
alone cannot establish useful realtime participation.

Measure this failure mode early. If unrelated changes starve useful work,
evaluate a versioned Action contract whose Pack-declared dependency revisions
are validated atomically with the accepted change. The Pack must specify all
relevant dependencies, including absence/range predicates where necessary;
an agent-supplied list alone cannot prove completeness. Authority, visibility,
policy, and domain invariants still apply at commit. Conflicting work must
refresh and re-evaluate. This needs an ADR and protocol design; current
Pack-level revision fields cannot bypass the whole-Head check.

For an airline example, a connection assessment might depend on specific
arrival, departure, and itinerary revisions. A new unrelated comment should
not necessarily invalidate it in that future contract. A changed departure
must invalidate or explicitly qualify the old assessment. External execution
must also enforce the target system's own preconditions.

### 5. Delivery, decisions, and effects need separate acknowledgements

A lost reply cannot distinguish a failed request from a committed request.
Retry the same operation identity and request to recover its recorded result.
Use a new identity for a revised proposal. Preserve separate meanings for
transport synchronization, processed observation, admitted Action, completed
Activation, and verified domain outcome.

For an external effect, a crash can occur after the remote system changes but
before its result is recorded. Preserve the operation identity, use target
idempotency where supported, and reconcile uncertain outcomes. Model retries,
Room receipts, and Runner leases cannot create an exactly-once guarantee in
an arbitrary external service. A stale Runner also needs domain revision and
ownership checks; its expired execution lease does not revoke separately held
participant Action authority.

### 6. Attention and history have different growth laws

If arrivals exceed execution capacity indefinitely, a queue cannot remain
bounded without admission control, coalescing, or shedding work. Refreshable
state updates may be coalesced under an explicit contract. Obligations and
required timer outcomes need retained dispositions and eventual handling.
Existing Activation deduplication across one cause does not coalesce all
successive causes.

Let `N` be history length, `S` active state, `B` context budget, and `K` a
verified recovery tail. Target warm operations that depend on bounded `S`
and `B` plus indexed lookup costs, and restart work bounded by an accepted
checkpoint contract and `K`. Retained storage can grow with `N`; hot memory
and every new prompt should not. Also measure input-to-accepted-contribution
latency. A fast commit acknowledgement alone is not fast useful reasoning.

## The smallest useful participation contract

The labels below describe capabilities, not new wire method names.

| Capability | Required guarantee | Current or proposed location |
| --- | --- | --- |
| Observe | Authorized current Projection, Action Offers, explicit basis, bounded changes, gap detection and Reset | Existing protocol; complete context bounds remain work |
| Propose | Typed Action with durable identity, strict current basis, authority and Pack validation | Existing contract; dependency admission is a separate future proposal |
| Resolve | Recover the original disposition after a lost reply without repeating a domain change | Existing operation identities and receipts |
| Acknowledge | Record durable observation processing separately from Session synchronization | Existing Cursor and attachment contracts |
| Activate | Durable wake-up intent; bounded context; fenced claim/renew/release/complete | Existing operational model; scale gaps identified |
| Retrieve evidence | Authorized bounded historical slices or immutable references with provenance | Proposed integration surface; do not expose raw Room history indiscriminately |

Host-authorized external-input admission supplies changing source facts through
the existing Stimulus model; general feed ingress is still an integration gap.
Pack-defined Actions carry assessments, work claims, blockers, and verified
results. Heartbeats, model token counts, private scratch work, and transient
execution statuses belong in operational or Runner state.

Do not make generic task graphs, vector memory, model voting, or summarization
schemas Core primitives. An application can put a bounded work state machine
in its Activity Pack and choose any qualified external Runner.

## What to build and measure next

1. **Close bounded-cost gaps under current contracts.** Activation preparation
   currently reaches full-history recovery, retained context can collect an
   entire cursor-to-head range, and recovery replays from Genesis. Establish
   shared current-executor use, total context limits, and an explicitly
   verified checkpoint/recovery contract. Checkpoints must not silently weaken
   the accepted corruption-detection requirements.
2. **Build one reference Activity Pack and replaceable Runner integration.**
   Use a nonterminal changing-world scenario with source revisions, attributed
   assessments, bounded unresolved work, and verification evidence. Replace
   an Invocation after each bounded contribution, recover original receipts
   after lost replies, and demonstrate continuity without private memory.
3. **Compare context strategies and measure contention.** Hold model and
   budgets constant: recent messages; summary plus retrieval; current
   Projection plus explicit work plus retrieval. Independently vary update
   rate and model delay. Record stale rejections, missed work, unsupported
   completion, evidence correctness, and time to a useful accepted Action.
4. **Qualify one Room at 1k, 10k, 100k, and 1m Transitions.** Use deterministic
   Runners for infrastructure measurements, with crashes, lost responses,
   offline participants, corrupt checkpoints, and a 24–72 hour soak. Include
   export/restore/transfer. Test model quality separately on a bounded sample.
5. **Use measured failures to propose semantic changes.** Whole-Head starvation
   may justify dependency admission. State-copying costs may justify a new
   canonical format. Neither is an invisible implementation optimization.

The inspected `TransitionV1` embeds resulting state, so bounded active state
is essential even before history-aware storage improvements. Current transfer
and verifier ceilings also require attention for 100k-plus histories. Source
anchors and numbers are in the [implementation findings](long-running-room-primitives-proposal.md#implementation-findings).

The existing single-process, one-authority, one-Pack-per-Room boundary remains
in force. Long history, high concurrent Room count, one busy Room, and one
agent making 100,000 model calls are separate scaling requirements. Published
memory results do not justify conflating them.
